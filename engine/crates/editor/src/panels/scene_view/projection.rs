//! 投影与坐标换算：设计分辨率 → 面板矩形、世界 ↔ 面板、编辑器相机与拾取射线。
//! 这些都是纯函数，`tests` 里用"手写映射"逐点对照。

use super::*;
use rustlua_ui::ui_layout::CanvasConfig;

impl EditorApp {
    /// 编辑器取景相机：面板的 pan/zoom 决定姿态，投影参数固定（fov 60°、near 0.1、far 200）。
    ///
    /// 拆分批 7：**不含面板宽高比**——比例是每帧视图量，由调用方以 `Viewport` 交给
    /// `Camera3D::{view_projection_matrix, ray_from_screen}` 或 `scene_3d_with_camera`。
    /// 编辑器相机从不出现在场景数据里，因此它也不该模仿"组件里存一份比例"的旧形态。
    pub(crate) fn editor_camera(&self) -> Camera3D {
        let cam = self.editor_camera;
        let position = Vec3::new(
            cam.target.x + cam.dist * cam.pitch.cos() * cam.yaw.sin(),
            cam.target.y + cam.dist * cam.pitch.sin(),
            cam.target.z + cam.dist * cam.pitch.cos() * cam.yaw.cos(),
        );
        Camera3D {
            position,
            target: cam.target,
            up: Vec3::Y,
            fov_y: 60f32.to_radians(),
            near: 0.1,
            far: 200.0,
            // 编辑器自己的取景相机永远由面板的 pan/zoom 决定，不跟随实体变换。
            pose_from_transform: false,
            // 编辑器取景相机永远启用（`enabled` 是场景相机参与选帧的开关，见 docs/54 §4.2）。
            enabled: true,
        }
    }

    pub(crate) fn editor_camera_ray(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Option<Ray3> {
        let camera = self.editor_camera();
        // docs/54 §8-⑦：与 Lua `camera3d.ray` 走**同一条共享链**（`rustlua_render::picking`），
        // 因此"编辑器点中哪个"和"脚本算出的射线"不会各说一套。宽高比与屏幕坐标同源
        // （同一个 `viewport`），拆分批 7 之后这一点由类型保证。
        let viewport = Viewport::new(width.max(1.0), height.max(1.0)).ok()?;
        rustlua_render::picking::ray_from_screen(
            camera,
            rustlua_ecs::transform::Vec2::new(x, y),
            viewport,
        )
        .ok()
    }

    /// 把世界坐标投影到屏幕（编辑器相机透视投影）。
    pub(crate) fn project_to_screen(&self, world: Vec3, rect: egui::Rect) -> Option<egui::Pos2> {
        let camera = self.editor_camera();
        // 与旧的 `rect.width() / rect.height().max(1.0)` **同一个比例**：egui 的矩形是浮点，
        // 因此这里按浮点建视口，不经过整数化（那会让 gizmo 的投影差一点点）。
        let vp = camera
            .view_projection_matrix(Viewport::new(rect.width(), rect.height().max(1.0)).ok()?)
            .ok()?;
        let c = vp.columns;
        let x = c[0][0] * world.x + c[1][0] * world.y + c[2][0] * world.z + c[3][0];
        let y = c[0][1] * world.x + c[1][1] * world.y + c[2][1] * world.z + c[3][1];
        let w = c[0][3] * world.x + c[1][3] * world.y + c[2][3] * world.z + c[3][3];
        if w <= 0.0 {
            return None;
        }
        Some(egui::pos2(
            rect.center().x + (x / w) * rect.width() * 0.5,
            rect.center().y - (y / w) * rect.height() * 0.5,
        ))
    }
}
/// 设计分辨率包围框在面板坐标中的矩形（中心 + 尺寸）。
///
/// 与 `entity_2d_screen_box` 共用同一映射：世界像素坐标 = pan + world*zoom，
/// 再除以 pixels_per_point 折回面板逻辑坐标；设计分辨率以游戏 2D 相机中心
/// （`center`，默认 `DEFAULT_2D_CAMERA_CENTER`）为世界中心，包围框即游戏取景范围。
/// WP5：把渲染目标的像素尺寸变成声明的视口。
///
/// 编辑器各视图都渲染进"自己那张离屏纹理"，因此视口恒为整张目标；窗口最小化或
/// 布局塌缩时可能出现 0，退到 1×1 以免造出非法视口。
pub(crate) fn render_viewport(width: u32, height: u32) -> Viewport {
    Viewport::new(width.max(1) as f32, height.max(1) as f32).unwrap_or(Viewport {
        width: 1.0,
        height: 1.0,
    })
}

pub(crate) fn design_resolution_frame(
    panel: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    ppp: f32,
    center: egui::Vec2,
    resolution: (u32, u32),
) -> egui::Rect {
    let ppp = ppp.max(1.0e-3);
    let size = egui::vec2(
        resolution.0 as f32 * zoom / ppp,
        resolution.1 as f32 * zoom / ppp,
    );
    // 包围框中心 = 游戏 2D 相机中心的世界坐标经 WP5 统一映射到面板。
    let rect_center =
        scene_view_world_to_panel(panel, pan, zoom, ppp, Vec2::new(center.x, center.y));
    egui::Rect::from_center_size(rect_center, size)
}

/// 把设计分辨率像素矩形映射到 Scene 视图面板坐标（设计包围框内）。
///
/// 与 `design_resolution_frame` / `entity_2d_screen_box` 共用同一套世界→面板
/// 变换：设计像素（画布以游戏相机中心 `center` 为世界中心、Y 向下的屏幕坐标）
/// → 世界（中心为原点、Y 翻转为向上正）→ 面板（pan + world*zoom，再 /ppp）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn design_rect_to_panel(
    panel: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    ppp: f32,
    center: egui::Vec2,
    design: (u32, u32),
    origin: [f32; 2],
    size: [f32; 2],
) -> egui::Rect {
    let ppp = ppp.max(1.0e-3);
    let dw = design.0 as f32;
    let dh = design.1 as f32;
    let world_x = center.x + origin[0] - dw * 0.5;
    let world_y = center.y + dh * 0.5 - origin[1];
    // 左上角走 WP5 的统一映射（世界 y 向上 ⇒ 设计分辨率的"顶边"映射到面板较小的 y）。
    let min = scene_view_world_to_panel(panel, pan, zoom, ppp, Vec2::new(world_x, world_y));
    let size = egui::vec2(size[0] * zoom / ppp, size[1] * zoom / ppp);
    egui::Rect::from_min_size(min, size)
}

/// E18：`CanvasConfig.safe_area`（预览像素 insets）在设计框内的面板矩形。
/// 先按设计分辨率走 `CanvasConfig::safe_rect` 钳制（极端 insets 退化为零尺寸），
/// 再与设计框共用同一套设计像素 → 面板变换。四边全 0 时返回 `None`——
/// 安全区就是整块画布，再画一圈只会和设计框重叠。
pub(crate) fn design_safe_frame(
    panel: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    ppp: f32,
    center: egui::Vec2,
    design: (u32, u32),
    config: &CanvasConfig,
) -> Option<egui::Rect> {
    if config.safe_area.iter().all(|inset| *inset == 0.0) {
        return None;
    }
    let safe = config.safe_rect([design.0 as f32, design.1 as f32]);
    Some(design_rect_to_panel(
        panel,
        pan,
        zoom,
        ppp,
        center,
        design,
        [safe.x, safe.y],
        [safe.w, safe.h],
    ))
}

/// 设计框被面板裁掉的部分（设计像素，左/上/右/下）。
/// `zoom` 与设计框同口径（物理像素/设计像素）；框完整落在面板内时四边均为 0。
pub(crate) fn design_frame_crop(
    panel: egui::Rect,
    frame: egui::Rect,
    zoom: f32,
    ppp: f32,
) -> [f32; 4] {
    let design_per_point = ppp.max(1.0e-3) / zoom.max(1.0e-3);
    [
        ((panel.left() - frame.left()) * design_per_point).max(0.0),
        ((panel.top() - frame.top()) * design_per_point).max(0.0),
        ((frame.right() - panel.right()) * design_per_point).max(0.0),
        ((frame.bottom() - panel.bottom()) * design_per_point).max(0.0),
    ]
}

/// 设计分辨率坐标 → 面板逻辑坐标。
///
/// 2D 的拖拽逻辑全部工作在**设计空间**（Transform 的 authored 值），而绘制要落在面板上，
/// 所以圆心这类"由逻辑算出的设计点"必须走同一条映射的反向，不能另写一遍 `pan + world*zoom`
/// （WP5 之后这类手写映射已经统一，两处各写一份会在高 DPI 上分叉）。
pub(crate) fn design_to_panel_pos(
    panel: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    ppp: f32,
    center: egui::Vec2,
    design: (u32, u32),
    position: egui::Pos2,
) -> egui::Pos2 {
    let dw = design.0 as f32;
    let dh = design.1 as f32;
    let world = Vec2::new(
        position.x + center.x - dw * 0.5,
        center.y + dh * 0.5 - position.y,
    );
    scene_view_world_to_panel(panel, pan, zoom, ppp, world)
}

/// 计算 2D 实体在 Scene 视图中的屏幕空间包围框（中心 + 尺寸）。
///
/// 尺寸按实体组件解析：Sprite 用 `size`（含 pivot 偏移，使框与渲染 quad 对齐），
/// Tilemap 用整张图块网格；无尺寸信息的实体回退到默认框（112×44）。再乘上
/// Transform 缩放与视图 zoom 得到屏幕尺寸，pivot 偏移同步缩放/旋转并随屏幕 Y
/// 翻转映射，保证选中轮廓与命中测试贴合实际渲染结果。
pub(crate) fn entity_2d_screen_box(
    entity: &SceneEntity,
    rect: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    transform: TransformFields,
    pixels_per_point: f32,
) -> (egui::Pos2, egui::Vec2) {
    let ppp = pixels_per_point.max(1.0e-3);
    let mut size = egui::vec2(112.0, 44.0);
    let mut pivot = egui::Vec2::new(0.5, 0.5);
    if let Some(raw) = entity.components.get("Sprite")
        && let Ok(sprite) = SpriteComponent::from_component_str(raw)
    {
        size = egui::vec2(sprite.size.x, sprite.size.y);
        pivot = egui::vec2(sprite.pivot.x, sprite.pivot.y);
    } else if let Some(raw) = entity.components.get("Tilemap")
        && let Ok(tilemap) = Tilemap::from_component_str(raw)
    {
        // Tilemap 渲染以 Transform 位置为左上角向右下延伸整张网格（见
        // append_tilemap_layer：center_world = position + 半格偏移），故 pivot
        // 取 (0,0) 让包围框中心落在网格中心，而非默认的 (0.5,0.5)（否则偏左上）。
        size = egui::vec2(
            tilemap.tile_size.x * tilemap.width as f32,
            tilemap.tile_size.y * tilemap.height as f32,
        );
        pivot = egui::Vec2::new(0.0, 0.0);
    }
    let sx = transform.scale_x.abs().max(0.25);
    let sy = transform.scale_y.abs().max(0.25);
    // 与 Scene 视图实时渲染（Camera2D + 离屏纹理贴回面板）共用同一映射：像素坐标
    // = pan + world*zoom，再除以 pixels_per_point 折回面板逻辑坐标。不除 ppp 会在
    // 高 DPI 屏上让包围框相对渲染内容偏移并放大（ppp==1 时两者恰好等价）。
    let screen_size = egui::vec2(size.x * sx * zoom / ppp, size.y * sy * zoom / ppp);
    // Sprite pivot 偏移（世界像素）随缩放/旋转映射到屏幕，使包围框中心贴合渲染 quad。
    let offset = egui::vec2(size.x * (0.5 - pivot.x) * sx, size.y * (0.5 - pivot.y) * sy);
    let rotation = transform.rotation.to_radians();
    let world_center = Vec2::new(
        transform.position_x + offset.x * rotation.cos() - offset.y * rotation.sin(),
        transform.position_y + offset.x * rotation.sin() + offset.y * rotation.cos(),
    );
    // 中心走 WP5 的统一映射（与运行时 `Camera2D::world_to_screen` 同一份投影）。
    let center = scene_view_world_to_panel(rect, pan, zoom, ppp, world_center);
    (center, screen_size)
}

/// WP5：编辑器 Scene 视图的相机——与运行时**同一个** `Camera2D`（pan 折成世界中心）。
pub(crate) fn editor_scene_camera(pan: egui::Vec2, zoom: f32) -> Camera2D {
    Camera2D::new(Vec2::new(-pan.x / zoom, -pan.y / zoom), zoom)
        .unwrap_or_else(|_| Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("zero camera is valid"))
}

/// WP5：编辑器 Scene 视图的视口——面板逻辑尺寸折成物理像素（`ppp`）。
pub(crate) fn editor_scene_viewport(rect: egui::Rect, ppp: f32) -> Viewport {
    let ppp = ppp.max(1.0e-3);
    let width = rect.width() * ppp;
    let height = rect.height() * ppp;
    Viewport::new(width, height).unwrap_or(Viewport {
        width: 1.0,
        height: 1.0,
    })
}

/// WP5：世界坐标 → 面板逻辑坐标。这是 Scene 视图的**唯一**映射实现——选中轮廓、
/// 命中测试、框选、瓦片笔刷、设计分辨率包围框全部走这里，与运行时
/// `Camera2D::world_to_screen` 是同一份投影（此前这些地方各自手写了一遍
/// `pan + world * zoom` 再除 `ppp`，改一处就要同时改五处）。
pub(crate) fn scene_view_world_to_panel(
    rect: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    ppp: f32,
    world: Vec2,
) -> egui::Pos2 {
    let ppp = ppp.max(1.0e-3);
    let camera = editor_scene_camera(pan, zoom);
    match camera.world_to_screen(world, editor_scene_viewport(rect, ppp)) {
        Ok(screen) => egui::pos2(rect.min.x + screen.x / ppp, rect.min.y + screen.y / ppp),
        // 相机非法（zoom 为 0/NaN 的极端输入）：退回面板原点而不是 panic。
        Err(_) => rect.min,
    }
}

/// WP5：面板逻辑坐标 → 世界坐标（`scene_view_world_to_panel` 的逆）。
pub(crate) fn scene_view_panel_to_world(
    rect: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    ppp: f32,
    pointer: egui::Pos2,
) -> Vec2 {
    let ppp = ppp.max(1.0e-3);
    let camera = editor_scene_camera(pan, zoom);
    let screen = Vec2::new(
        (pointer.x - rect.min.x) * ppp,
        (pointer.y - rect.min.y) * ppp,
    );
    camera
        .screen_to_world(screen, editor_scene_viewport(rect, ppp))
        .unwrap_or(Vec2::new(0.0, 0.0))
}

pub(crate) fn screen_to_world_2d(
    rect: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    ppp: f32,
    pointer: egui::Pos2,
) -> Vec2 {
    scene_view_panel_to_world(rect, pan, zoom, ppp, pointer)
}
