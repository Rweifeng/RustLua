//! 视口「聚焦选中」的纯几何（Unity 的 `Frame Selected` / `F`）。
//!
//! 三个入口共用这里：层级面板的 `F`、场景视图内的 `F`。只做数值，不碰文档、选择或
//! 渲染；`EditorApp` 侧负责取包围盒并写回相机（见 [`EditorApp::frame_selection_in_scene_view`]）。
//!
//! 为什么单独成模块：`F` 以前只改相机 `target`（3D 不调距离、2D 只改 pan 不调 zoom），
//! 结果是"点了 F 画面几乎不动、选中对象仍在视口外"。Unity 的语义是**按包围盒适配**，
//! 那是三个可以无头断言的纯函数：包围盒并集、透视距离、正交缩放。

use super::*;

/// 3D 编辑器相机的距离范围（与滚轮缩放共用，见 `view_3d.rs`）。
pub(crate) const FRAME_DIST_MIN: f32 = 1.0;
pub(crate) const FRAME_DIST_MAX: f32 = 100.0;

/// 2D 聚焦时给面板留的边距（逻辑点）：与 `fit_scene_view_to_design` 同口径。
pub(crate) const FRAME_2D_MARGIN: f32 = 48.0;

/// 世界/设计坐标的轴对齐包围盒。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FocusBounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl FocusBounds {
    /// 由一个点（实体位置）与它的半边长构造：点本身没有尺寸，托盘一律给一个立方体，
    /// 否则单选相机就会退化成 `dist = 0`。
    pub(crate) fn around(point: Vec3, half: Vec3) -> Self {
        Self {
            min: Vec3::new(point.x - half.x, point.y - half.y, point.z - half.z),
            max: Vec3::new(point.x + half.x, point.y + half.y, point.z + half.z),
        }
    }

    /// 多点并集：空集合返回 `None`（调用方据此报"没有可聚焦的对象"）。
    #[cfg(test)]
    pub(crate) fn from_points(points: impl IntoIterator<Item = Vec3>) -> Option<Self> {
        let mut points = points.into_iter();
        let first = points.next()?;
        let mut bounds = Self {
            min: first,
            max: first,
        };
        for point in points {
            bounds.min = Vec3::new(
                bounds.min.x.min(point.x),
                bounds.min.y.min(point.y),
                bounds.min.z.min(point.z),
            );
            bounds.max = Vec3::new(
                bounds.max.x.max(point.x),
                bounds.max.y.max(point.y),
                bounds.max.z.max(point.z),
            );
        }
        Some(bounds)
    }

    pub(crate) fn union(self, other: Self) -> Self {
        Self {
            min: Vec3::new(
                self.min.x.min(other.min.x),
                self.min.y.min(other.min.y),
                self.min.z.min(other.min.z),
            ),
            max: Vec3::new(
                self.max.x.max(other.max.x),
                self.max.y.max(other.max.y),
                self.max.z.max(other.max.z),
            ),
        }
    }

    pub(crate) fn center(self) -> Vec3 {
        Vec3::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
            (self.min.z + self.max.z) * 0.5,
        )
    }

    /// 包围球半径：半对角线。相机要按**球**装进视锥，按盒子边长算会在斜角处裁掉。
    pub(crate) fn radius(self) -> f32 {
        let half = Vec3::new(
            (self.max.x - self.min.x) * 0.5,
            (self.max.y - self.min.y) * 0.5,
            (self.max.z - self.min.z) * 0.5,
        );
        (half.x * half.x + half.y * half.y + half.z * half.z).sqrt()
    }
}

/// 透视相机：让半径 `radius` 的包围球刚好进视野所需的轨道距离。
///
/// 取**垂直与水平视场里更紧的那一侧**：竖屏（aspect < 1）时水平方向窄，只看
/// `fov_y` 会让左右两侧跑出画面——Unity 的 Frame Selected 也是按两侧较紧的算。
pub(crate) fn framed_distance(radius: f32, fov_y_rad: f32, aspect: f32) -> f32 {
    let radius = radius.max(0.01);
    let aspect = aspect.max(0.01);
    let half_y = (fov_y_rad * 0.5).clamp(0.01, std::f32::consts::FRAC_PI_2 - 0.01);
    let half_x = (aspect * half_y.tan()).atan();
    let half_angle = half_y.min(half_x);
    (radius / half_angle.sin()).clamp(FRAME_DIST_MIN, FRAME_DIST_MAX)
}

/// 2D 正交视图：把 `size`（设计像素）的包围盒装进面板所需的 zoom。
///
/// 与 `fit_scene_view_to_design` 同一套边距与上下限，所以"聚焦选中"和"适配画面"
/// 不会给出两种手感。
pub(crate) fn framed_zoom_2d(size: egui::Vec2, viewport: egui::Vec2, margin: f32) -> f32 {
    let available = egui::vec2(
        (viewport.x - margin * 2.0).max(50.0),
        (viewport.y - margin * 2.0).max(50.0),
    );
    (available.x / size.x.max(1.0))
        .min(available.y / size.y.max(1.0))
        .clamp(SCENE_ZOOM_MIN, SCENE_ZOOM_MAX)
}

/// 2D 平移：让世界点 `center` 落在面板中心。
///
/// 口径来自 `editor_scene_camera`——相机世界中心是 `-pan / zoom`，所以 `pan = -center * zoom`。
/// （旧的层级 F 实现把 Y 写成了 `+center.y * zoom`，Y 方向因此偏离两倍。）
pub(crate) fn pan_centered_on(center: egui::Vec2, zoom: f32) -> egui::Vec2 {
    egui::vec2(-center.x * zoom, -center.y * zoom)
}

impl EditorApp {
    /// 聚焦当前选择（Unity `Frame Selected`，`F`）：按选中集合的**并集包围盒**适配视图。
    ///
    /// 返回状态栏文案；选择为空或没有可用位置时不改相机（而不是把镜头悄悄推到原点）。
    /// 2D/3D 共用一个入口，层级面板的 `F` 与视口内的 `F` 因此不会各算一套。
    pub(crate) fn frame_selection_in_scene_view(&mut self) -> String {
        if self.selection.primary().is_none() {
            return "请先选择实体再按 F 聚焦".to_owned();
        }
        let ids = self.selection.ids();
        if ids.is_empty() {
            return "请先选择实体再按 F 聚焦".to_owned();
        }
        if self.scene_view_mode == SceneViewMode::View3D {
            self.frame_selection_3d(&ids)
        } else {
            self.frame_selection_2d(&ids)
        }
    }

    fn frame_selection_3d(&mut self, ids: &[SceneEntityId]) -> String {
        self.ensure_scene_view_3d_typed();
        let mut bounds: Option<FocusBounds> = None;
        let mut focused = 0_usize;
        for id in ids {
            // 世界变换（父链已传播）优先；typed 缓存只收 3D 实体，其余回落到文档局部值。
            let position = self
                .scene_view_3d_world_transform(*id)
                .map(|transform| transform.position)
                .or_else(|| {
                    let entity = self.current_scene()?.scene.entities.get(id)?;
                    Transform3D::from_component_str(entity.components.get("Transform")?)
                        .ok()
                        .map(|transform| transform.position)
                });
            let Some(position) = position else {
                continue;
            };
            let one = FocusBounds::around(position, Vec3::new(0.5, 0.5, 0.5));
            bounds = Some(bounds.map_or(one, |bounds| bounds.union(one)));
            focused += 1;
        }
        let Some(bounds) = bounds else {
            return "当前选择没有可聚焦的 3D 位置".to_owned();
        };
        let aspect = {
            let rect = self.scene_canvas_rect;
            if rect.width() > 1.0 && rect.height() > 1.0 {
                rect.width() / rect.height()
            } else {
                // 首帧/最小化：面板还没有真实尺寸，用 16:9 兜底（比 0 更接近用户意图）。
                16.0 / 9.0
            }
        };
        let dist = framed_distance(bounds.radius(), 60f32.to_radians(), aspect);
        self.editor_camera.target = bounds.center();
        self.editor_camera.dist = dist;
        format!("已聚焦 {focused} 个实体（3D · 距离 {dist:.1}）")
    }

    fn frame_selection_2d(&mut self, ids: &[SceneEntityId]) -> String {
        let Some((center, size)) = self.selection_bounds_2d(ids) else {
            return "当前选择没有可聚焦的 2D 位置".to_owned();
        };
        let rect = self.scene_canvas_rect;
        let viewport = if rect.width() > 32.0 && rect.height() > 32.0 {
            egui::vec2(rect.width(), rect.height())
        } else {
            egui::vec2(
                self.preview_resolution.0 as f32,
                self.preview_resolution.1 as f32,
            )
        };
        let zoom = framed_zoom_2d(size, viewport, FRAME_2D_MARGIN);
        self.scene_zoom = zoom;
        self.scene_state.pan = pan_centered_on(center, zoom);
        format!("已聚焦 {} 个实体（2D · 缩放 {zoom:.2}×）", ids.len())
    }

    /// 选中集合在**设计平面**上的并集包围盒（中心 + 尺寸）。
    ///
    /// 尺寸与 `entity_2d_screen_box` 同源：Sprite 用 `size`，Tilemap 用整张网格，
    /// 其余组件回落到默认 112×44——否则"聚焦一个空实体"会算出 0 尺寸、zoom 顶到上限。
    fn selection_bounds_2d(&self, ids: &[SceneEntityId]) -> Option<(egui::Vec2, egui::Vec2)> {
        let scene = self.current_scene()?;
        let mut min = egui::vec2(f32::INFINITY, f32::INFINITY);
        let mut max = egui::vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
        let mut found = false;
        for id in ids {
            let Some(entity) = scene.scene.entities.get(id) else {
                continue;
            };
            let transform = entity
                .components
                .get("Transform")
                .and_then(|value| parse_transform(value))
                .unwrap_or_default();
            let mut size = egui::vec2(112.0, 44.0);
            if let Some(raw) = entity.components.get("Sprite")
                && let Ok(sprite) = SpriteComponent::from_component_str(raw)
            {
                size = egui::vec2(sprite.size.x, sprite.size.y);
            } else if let Some(raw) = entity.components.get("Tilemap")
                && let Ok(tilemap) = Tilemap::from_component_str(raw)
            {
                size = egui::vec2(
                    tilemap.tile_size.x * tilemap.width as f32,
                    tilemap.tile_size.y * tilemap.height as f32,
                );
            }
            let half = egui::vec2(
                size.x * transform.scale_x.abs().max(0.25) * 0.5,
                size.y * transform.scale_y.abs().max(0.25) * 0.5,
            );
            min.x = min.x.min(transform.position_x - half.x);
            min.y = min.y.min(transform.position_y - half.y);
            max.x = max.x.max(transform.position_x + half.x);
            max.y = max.y.max(transform.position_y + half.y);
            found = true;
        }
        found.then(|| {
            (
                egui::vec2((min.x + max.x) * 0.5, (min.y + max.y) * 0.5),
                egui::vec2((max.x - min.x).max(1.0), (max.y - min.y).max(1.0)),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_union_covers_every_point() {
        let a = FocusBounds::around(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0));
        let b = FocusBounds::around(Vec3::new(4.0, 2.0, -2.0), Vec3::new(1.0, 1.0, 1.0));
        let union = a.union(b);
        assert_eq!(union.min, Vec3::new(-1.0, -1.0, -3.0));
        assert_eq!(union.max, Vec3::new(5.0, 3.0, 1.0));
        assert_eq!(union.center(), Vec3::new(2.0, 1.0, -1.0));
        // 半径按半对角线算：单选一个 2×2×2 盒子 = sqrt(3)。
        assert!((a.radius() - 3.0_f32.sqrt()).abs() < 1.0e-5);
        // 空集合没有可聚焦的东西。
        assert!(FocusBounds::from_points(std::iter::empty()).is_none());
        assert_eq!(
            FocusBounds::from_points([Vec3::new(1.0, 2.0, 3.0)]).map(FocusBounds::center),
            Some(Vec3::new(1.0, 2.0, 3.0))
        );
    }

    /// 竖屏比横屏需要更远：同半径、同 fov，aspect 变窄时水平视场先兜不住。
    #[test]
    fn framed_distance_respects_the_tighter_axis() {
        let fov = 60f32.to_radians();
        let landscape = framed_distance(1.0, fov, 16.0 / 9.0);
        let square = framed_distance(1.0, fov, 1.0);
        let portrait = framed_distance(1.0, fov, 9.0 / 16.0);
        assert!(
            (landscape - 2.0).abs() < 0.05,
            "正球在 60° 垂直视场下应约 2 倍半径：{landscape}"
        );
        // 方形视口的水平视场恰好也是 60°（atan(1·tan30°)=30°），与横屏同为垂直兜底；
        // 竖屏才轮到水平兜底，因此必须推得更远。
        assert!(
            (square - landscape).abs() < 1.0e-4,
            "方形与横屏都按垂直视场兜底：{square} vs {landscape}"
        );
        assert!(portrait > square, "竖屏水平更紧：{portrait} vs {square}");
        // 极端输入不产生 NaN/0，且落在滚轮同一范围里。
        let tiny = framed_distance(0.0, fov, 0.0);
        assert!(tiny.is_finite() && tiny >= FRAME_DIST_MIN);
        let huge = framed_distance(1.0e9, fov, 1.0);
        assert_eq!(huge, FRAME_DIST_MAX);
    }

    #[test]
    fn framed_zoom_2d_fits_the_box_inside_the_margin() {
        let viewport = egui::vec2(800.0, 600.0);
        // 1920×1080：宽度先兜不住 → (800-96)/1920。
        let zoom = framed_zoom_2d(egui::vec2(1920.0, 1080.0), viewport, FRAME_2D_MARGIN);
        assert!(
            (zoom - (800.0 - 96.0) / 1920.0).abs() < 1.0e-5,
            "zoom={zoom}"
        );
        // 装进面板后两侧都不超过可用区域。
        assert!(1920.0 * zoom <= viewport.x - 96.0 + 1.0e-3);
        assert!(1080.0 * zoom <= viewport.y - 96.0 + 1.0e-3);
        // 退化输入不产生 NaN/0/无限大。
        let degenerate = framed_zoom_2d(egui::vec2(0.0, 0.0), egui::vec2(0.0, 0.0), 48.0);
        assert!((SCENE_ZOOM_MIN..=SCENE_ZOOM_MAX).contains(&degenerate));
    }

    /// 聚焦后选中点必须落在面板中心——这条同时钉住 pan 的符号约定
    /// （旧实现 Y 用了 `+center.y * zoom`，会让聚焦目标跑到两倍偏移处）。
    #[test]
    fn pan_puts_the_focus_point_at_the_viewport_center() {
        for (zoom, center) in [
            (1.0_f32, egui::vec2(0.0, 0.0)),
            (0.5, egui::vec2(400.0, -300.0)),
            (2.0, egui::vec2(-120.0, 64.0)),
        ] {
            let pan = pan_centered_on(center, zoom);
            let panel = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(800.0, 600.0));
            let projected =
                scene_view_world_to_panel(panel, pan, zoom, 1.0, Vec2::new(center.x, center.y));
            let delta = projected - panel.center();
            assert!(
                delta.length() < 1.0e-3,
                "zoom={zoom} center={center:?} 投影到 {projected:?}，应在 {panel:?} 中心"
            );
        }
    }

    /// `fit_scene_view_to_design` 与聚焦共用同一套口径：同样的盒子给出同样的 zoom。
    #[test]
    fn design_fit_and_focus_share_the_same_zoom_rule() {
        let viewport = egui::vec2(1024.0, 720.0);
        let design = egui::vec2(1280.0, 720.0);
        let zoom = framed_zoom_2d(design, viewport, FRAME_2D_MARGIN);
        let by_width: f32 = (1024.0 - 96.0) / 1280.0;
        let by_height: f32 = (720.0 - 96.0) / 720.0;
        let expected = by_width
            .min(by_height)
            .clamp(SCENE_ZOOM_MIN, SCENE_ZOOM_MAX);
        assert!((zoom - expected).abs() < 1.0e-5);
    }
}
