//! Gizmo 的**纯逻辑**层：命中检测与多选变换（docs/55 阶段 2 收口）。
//!
//! 计划里阶段 2 的两条剩余缺口都在这里收口：
//!
//! 1. **精确命中**：手柄过去只按"点到端点圆"判定（`(pos - (center + arm)).length() < r`），
//!    于是只能抓住臂末端那个小球，臂中间的线段抓不住——用户看到手柄画了一整条，却只有尽头能点。
//!    现在按**点到线段距离**判定（[`distance_point_to_segment`]）；
//! 2. **多选**：3D 手柄过去只作用于"首个选中实体"，多选后拖拽只动一个。
//!    现在拖拽目标是**整个选中集合**（[`GizmoDragTargets`]），一次拖拽 = 一条 Undo 覆盖全部。
//!
//! 这一层刻意不碰 `egui::Ui`、不碰文档：输入是几何量与初始变换，输出是"每个实体应该变成
//! 什么"。因此它可以脱离帧循环单测（阶段 2 验收："绘制与交互可独立测试"）。
//!
//! M27-02 在这里补的是**手柄几何**那一半：3D 的三条旋转环（真投影椭圆，不再是"画一圈、
//! 只能绕 Y 转"）、分轴/双轴缩放手柄、以及拖动中的即时数值文案。绘制与命中**共用同一份
//! 函数返回值**，所以"看到哪里就点哪里"这件事不需要两处保持一致。

use crate::scene_interaction::{GizmoPivot, GizmoSpace};
use crate::{GizmoAxis, SceneTool};
use eframe::egui;
use rustlua_ecs::transform::Transform3D;
use rustlua_math::geom3d::Ray3;
use rustlua_math::math3d::{Aabb3, Vec3};
use rustlua_scene::scene::SceneEntityId;

/// 手柄命中半径（点）。比绘制半径大一点：细线手柄要"好抓"。
pub(crate) const GIZMO_HIT_RADIUS: f32 = 8.0;
/// 旋转/缩放手柄（单点）的命中半径（点）。
pub(crate) const GIZMO_HANDLE_RADIUS: f32 = 10.0;
/// 手柄中心（自由拖动 / 原地旋转缩放）的命中方框半边长（点）。
pub(crate) const GIZMO_CENTER_HALF: f32 = 12.0;
/// 旋转**环**的命中阈值（点）。环是投影出来的细椭圆，比圆点更难指准，阈值略宽。
pub(crate) const GIZMO_RING_HIT_RADIUS: f32 = 9.0;
/// 旋转环的采样段数（闭合折线的顶点数）。
pub(crate) const GIZMO_RING_SEGMENTS: usize = 32;
/// 缩放灵敏度：屏幕像素 → 倍数增量（与迁移前的等比缩放同一个数）。
pub(crate) const GIZMO_SCALE_PER_PIXEL: f32 = 0.01;

/// 点到**线段**的最短距离。
///
/// `a == b`（退化线段）时退化为点到点距离——手柄投影退化成一个点时仍然可抓。
pub(crate) fn distance_point_to_segment(point: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let length_squared = ab.length_sq();
    if length_squared <= f32::EPSILON {
        return (point - a).length();
    }
    let t = ((point - a).dot(ab) / length_squared).clamp(0.0, 1.0);
    (point - (a + ab * t)).length()
}

/// Gizmo 手柄命中检测（精确版）。
///
/// 与绘制共用同一份屏幕方向 `axes`，因此"看到哪里就点哪里"成立；顺序保持
/// X → Z → Y（与绘制顺序一致：重叠时优先抓先画的那根）。
pub(crate) fn pick_gizmo_axis(
    tool: SceneTool,
    center: egui::Pos2,
    position: egui::Pos2,
    locked: bool,
    axes: [egui::Vec2; 3],
) -> Option<GizmoAxis> {
    let center_box =
        egui::Rect::from_center_size(center, egui::Vec2::splat(GIZMO_CENTER_HALF * 2.0));
    match tool {
        SceneTool::Move | SceneTool::Select => {
            // **中心把手优先**：三条臂都从锚点出发，先判臂会让 24×24 的中心区域永远命中 X 轴，
            // "自由拖动"和"锁定实体不响应"两条语义就都失效了（这正是改成线段命中后暴露的问题，
            // 由 `scene_view::tests::gizmo_hit_testing_respects_tool_and_lock_state` 钉住）。
            if center_box.contains(position) {
                return (!locked).then_some(GizmoAxis::Free);
            }
            for (axis, arm) in [
                (GizmoAxis::X, axes[0]),
                (GizmoAxis::Z, axes[2]),
                (GizmoAxis::Y, axes[1]),
            ] {
                if distance_point_to_segment(position, center, center + arm) < GIZMO_HIT_RADIUS {
                    return Some(axis);
                }
            }
            None
        }
        SceneTool::Rotate => {
            // 三条环的命中要按**投影后的椭圆**判（见 `pick_rotation_ring`），这里只保留
            // 中心方框：抓住中心 = 绕世界 Y 原地转，与迁移前"整个手柄只认 Y"的行为逐字一致，
            // 因此旧手感不会因为新增两环而消失。
            if center_box.contains(position) {
                Some(GizmoAxis::Y)
            } else {
                None
            }
        }
        SceneTool::Scale => {
            // 中心 = 等比；六个把手 = 单轴 / 双轴。绘制与命中共用 [`scale_handles`]。
            // 过去那个"沿 X/Y 对角的等比把手"取消了两件事：它与 XY 平面把手同向、只靠远近
            // 区分，等于"两个能点到的东西说同一句话"。
            if center_box.contains(position) {
                return Some(GizmoAxis::Scale);
            }
            scale_handles(axes)
                .into_iter()
                .find(|(_, offset)| (position - (center + *offset)).length() < GIZMO_HANDLE_RADIUS)
                .map(|(axis, _)| axis)
        }
        SceneTool::TilePaint => None,
    }
}

/// 缩放**把手**在屏幕上的偏移（相对手柄中心）。绘制与命中都读这一份。
///
/// 顺序即优先级：三个轴把手先判，三块平面把手后判（平面把手离中心更近，容易被轴把手盖住）。
#[must_use]
pub(crate) fn scale_handles(axes: [egui::Vec2; 3]) -> Vec<(GizmoAxis, egui::Vec2)> {
    let mut handles = vec![
        (GizmoAxis::ScaleX, axes[0]),
        (GizmoAxis::ScaleY, axes[1]),
        (GizmoAxis::ScaleZ, axes[2]),
    ];
    for (axis, pair) in [
        (GizmoAxis::ScaleXY, [axes[0], axes[1]]),
        (GizmoAxis::ScaleXZ, [axes[0], axes[2]]),
        (GizmoAxis::ScaleYZ, [axes[1], axes[2]]),
    ] {
        handles.push((axis, (pair[0] + pair[1]) * 0.45));
    }
    handles
}

/// 这个缩放手柄动**哪些分量**。`Scale`（中心）= 三条都动，与迁移前一致。
#[must_use]
pub(crate) fn scale_axis_mask(axis: GizmoAxis) -> [bool; 3] {
    match axis {
        GizmoAxis::ScaleX => [true, false, false],
        GizmoAxis::ScaleY => [false, true, false],
        GizmoAxis::ScaleZ => [false, false, true],
        GizmoAxis::ScaleXY => [true, true, false],
        GizmoAxis::ScaleXZ => [true, false, true],
        GizmoAxis::ScaleYZ => [false, true, true],
        GizmoAxis::Scale => [true, true, true],
        _ => [false, false, false],
    }
}

/// 缩放手柄的拖动倍数：屏幕增量在**把手方向**上的投影 × 灵敏度。
///
/// 把手方向退化（那条轴投影成一个点）时退回水平增量，与迁移前的等比缩放手感一致。
#[must_use]
pub(crate) fn scale_factor(delta: egui::Vec2, direction: egui::Vec2) -> f32 {
    let unit = if direction.length_sq() > 1e-6 {
        direction / direction.length()
    } else {
        egui::vec2(1.0, 0.0)
    };
    (1.0 + delta.dot(unit) * GIZMO_SCALE_PER_PIXEL).max(0.05)
}

/// 这个缩放手柄"往哪儿拖才算变大"：把手方向（屏幕）。
#[must_use]
pub(crate) fn scale_handle_direction(axis: GizmoAxis, axes: [egui::Vec2; 3]) -> egui::Vec2 {
    match axis {
        // 中心方框（等比）沿用水平增量：要求用户沿对角线拖反而不清楚"往哪儿算放大"。
        GizmoAxis::Scale => egui::vec2(1.0, 0.0),
        other => scale_handles(axes)
            .into_iter()
            .find(|(handle, _)| *handle == other)
            .map(|(_, offset)| offset)
            .unwrap_or_else(|| egui::vec2(1.0, 0.0)),
    }
}

/// 环所在平面的正交基（法向 `unit`）：面内两个互相垂直的单位向量。
#[must_use]
pub(crate) fn ring_basis(unit: Vec3) -> Option<(Vec3, Vec3)> {
    let unit = unit.normalized().ok()?;
    // 参考向量的选法决定叉乘会不会退化：法向贴近 X 时改用 Y 做参考，反之用 X。
    let helper = if unit.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let first = unit.cross(helper).normalized().ok()?;
    Some((first, unit.cross(first)))
}

/// 环的**世界半径**：让环在屏幕上与臂长齐平。
///
/// 面内两向里取**投影更长**的那个当尺子——投影退化的一侧（正对着相机的那条）量不出比例。
/// 两向都退化（相机与环面几乎平行）时返回 `None`，调用方按"这一环此刻不可见也不可点"处理。
#[must_use]
pub(crate) fn ring_radius_world(
    center: Vec3,
    unit: Vec3,
    screen_len: f32,
    project: impl Fn(Vec3) -> Option<egui::Pos2>,
) -> Option<f32> {
    let (first, second) = ring_basis(unit)?;
    let screen = project(center)?;
    let per_unit = [first, second]
        .into_iter()
        .filter_map(|direction| project(center + direction))
        .map(|point| (point - screen).length())
        .fold(0.0_f32, f32::max);
    (per_unit >= 1.0).then_some(screen_len / per_unit)
}

/// 把 3D 旋转环采样成**屏幕折线**（闭合）。
///
/// 投影不上的点（相机后方）直接跳过，所以返回值可能少于 `GIZMO_RING_SEGMENTS` 个顶点；
/// 顶点不足 2 个时 [`distance_point_to_ring`] 会拒绝命中，绘制方也应跳过这一环。
#[must_use]
pub(crate) fn ring_polyline(
    center: Vec3,
    unit: Vec3,
    radius: f32,
    project: impl Fn(Vec3) -> Option<egui::Pos2>,
) -> Vec<egui::Pos2> {
    let Some((first, second)) = ring_basis(unit) else {
        return Vec::new();
    };
    (0..GIZMO_RING_SEGMENTS)
        .filter_map(|index| {
            let angle = index as f32 * (std::f32::consts::TAU / GIZMO_RING_SEGMENTS as f32);
            project(center + (first * angle.cos() + second * angle.sin()) * radius)
        })
        .collect()
}

/// 点到**闭合折线**的最短距离；顶点不足 2 个时 `None`（没有"线"可言）。
#[must_use]
pub(crate) fn distance_point_to_ring(point: egui::Pos2, ring: &[egui::Pos2]) -> Option<f32> {
    if ring.len() < 2 {
        return None;
    }
    (0..ring.len())
        .map(|index| distance_point_to_segment(point, ring[index], ring[(index + 1) % ring.len()]))
        .fold(None, |best: Option<f32>, distance| {
            Some(best.map_or(distance, |current| current.min(distance)))
        })
}

/// 命中**哪一圈**：指针到各环折线的距离里，最小且不超过阈值的那一个。
///
/// 三条环会在屏幕上互相交叉，"先判谁"是错的（同一位置离两条环都近时应该给更近的那条），
/// 所以这里取距离最小值而不是列表顺序。
#[must_use]
pub(crate) fn pick_rotation_ring(
    pointer: egui::Pos2,
    rings: &[(GizmoAxis, Vec<egui::Pos2>)],
) -> Option<GizmoAxis> {
    rings
        .iter()
        .filter_map(|(axis, ring)| {
            distance_point_to_ring(pointer, ring)
                .map(|distance| (*axis, distance))
                .filter(|(_, distance)| *distance <= GIZMO_RING_HIT_RADIUS)
        })
        .min_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(axis, _)| axis)
}

/// 射线与"过 `point`、法向 `unit`"的平面的交点。
///
/// 视线几乎平行于平面时（相机贴着环面看）交点会跑到无穷远，返回 `None` 让调用方退回
/// 屏幕增量近似——宁可不精确，也不要一拖就飞出去。
#[must_use]
pub(crate) fn ray_plane_point(ray: Ray3, point: Vec3, unit: Vec3) -> Option<Vec3> {
    let normal = unit.normalized().ok()?;
    let along = ray.direction.dot(normal);
    if along.abs() < 0.02 {
        return None;
    }
    let distance = (point - ray.origin).dot(normal) / along;
    (distance.is_finite() && distance > 0.0).then(|| ray.point_at(distance))
}

/// 环拖动的**带符号**转角（右手定则绕 `unit`），来自射线与环平面的两个交点。
///
/// 交点与环心重合时角度无定义（指针指着转轴本身），返回 `None`。
#[must_use]
pub(crate) fn ring_angle(center: Vec3, unit: Vec3, start: Vec3, current: Vec3) -> Option<f32> {
    let unit = unit.normalized().ok()?;
    let first = (start - center).normalized().ok()?;
    let second = unit.cross(first);
    let offset = current - center;
    (offset.length_squared() > 1e-12).then(|| offset.dot(second).atan2(offset.dot(first)))
}

/// 角度吸附：把"从抓取点起的转角"对齐到 `step_degrees` 的整数倍。`None` = 不吸附。
#[must_use]
pub(crate) fn snap_angle(angle: f32, step_degrees: Option<f32>) -> f32 {
    match step_degrees {
        Some(step) if step > 0.0 => (angle.to_degrees() / step).round() * step.to_radians(),
        _ => angle,
    }
}

/// 拖动中的即时数值（2D 浮标）：只报**当前工具在改的那一项**。
///
/// 旋转报的是**这一趟拖动转了多少**（带符号），不是实体的绝对朝向——拖动时眼睛要的是
/// "我改了多少"，绝对朝向 Inspector 里一直有。
#[must_use]
pub(crate) fn readout_2d(
    tool: SceneTool,
    position: (f32, f32),
    angle_delta_degrees: f32,
    scale: (f32, f32),
) -> String {
    match tool {
        SceneTool::Move | SceneTool::Select => {
            format!("位置 {:.2}, {:.2}", position.0, position.1)
        }
        SceneTool::Rotate => format!("角度 {angle_delta_degrees:+.1}°"),
        SceneTool::Scale => format!("缩放 {:.2} × {:.2}", scale.0, scale.1),
        SceneTool::TilePaint => String::new(),
    }
}

/// 拖动中的即时数值（3D 浮标）：旋转额外报**绕的是哪根轴**，因为现在有三支环。
#[must_use]
pub(crate) fn readout_3d(
    tool: SceneTool,
    axis: GizmoAxis,
    transform: &Transform3D,
    angle_delta: f32,
) -> String {
    let position = transform.position;
    let scale = transform.scale;
    match tool {
        SceneTool::Move | SceneTool::Select => {
            format!(
                "位置 {:.2}, {:.2}, {:.2}",
                position.x, position.y, position.z
            )
        }
        SceneTool::Rotate => format!("绕 {} {:+.1}°", axis_name(axis), angle_delta.to_degrees()),
        SceneTool::Scale => format!("缩放 {:.2} × {:.2} × {:.2}", scale.x, scale.y, scale.z),
        SceneTool::TilePaint => String::new(),
    }
}

/// 环 / 臂对应的轴名（浮标文案用；与 `gizmo_axis_world` 的槽位一致）。
#[must_use]
pub(crate) const fn axis_name(axis: GizmoAxis) -> &'static str {
    match axis {
        GizmoAxis::ScaleXY => "XY",
        GizmoAxis::ScaleXZ => "XZ",
        GizmoAxis::ScaleYZ => "YZ",
        GizmoAxis::X | GizmoAxis::ScaleX => "X",
        GizmoAxis::Y | GizmoAxis::ScaleY => "Y",
        GizmoAxis::Z | GizmoAxis::ScaleZ => "Z",
        GizmoAxis::Free => "自由",
        GizmoAxis::Scale => "等比",
    }
}

/// 选中集合的**并集包围盒中心**（多选时 `Center` 轴心落在这里）。
///
/// 单实体时与旧的 `gizmo_origin(Center, …)` 完全一致（`position + (min+max)/2`），
/// 因此这是"把单实体的语义推广到多选"，不是换了一套规则。
pub(crate) fn selection_bounds_center(
    targets: impl IntoIterator<Item = (Vec3, Aabb3)>,
) -> Option<Vec3> {
    let mut min: Option<Vec3> = None;
    let mut max: Option<Vec3> = None;
    for (position, bounds) in targets {
        let low = position + bounds.min;
        let high = position + bounds.max;
        min = Some(match min {
            None => low,
            Some(current) => Vec3::new(
                current.x.min(low.x),
                current.y.min(low.y),
                current.z.min(low.z),
            ),
        });
        max = Some(match max {
            None => high,
            Some(current) => Vec3::new(
                current.x.max(high.x),
                current.y.max(high.y),
                current.z.max(high.z),
            ),
        });
    }
    Some((min? + max?) * 0.5)
}

/// 一次 Gizmo 拖拽的**状态**（docs/55 阶段 2：工具控制器只保存"抓的是什么"，
/// 每帧的几何换算由本模块的纯函数给出）。
#[derive(Clone, Debug)]
pub(crate) struct GizmoDrag {
    /// 拖拽目标集合（含主实体与锁定过滤后的实体列表）。
    pub targets: GizmoDragTargets,
    pub axis: GizmoAxis,
    /// 拖动起点（屏幕点），用于算屏幕增量。
    pub start: egui::Pos2,
    /// 手柄锚点变换（位置=锚点，旋转=主实体旋转）：射线求交的参考。
    pub origin: Transform3D,
    /// 轴心模式 `Center` 时的**公转中心**；`None` = 原地旋转/缩放。
    pub pivot: Option<Vec3>,
    pub start_ray: Option<rustlua_camera::camera::Ray3>,
}

impl GizmoDrag {
    /// 主选中实体（历史条目归属与 Inspector 同步用它）。
    pub(crate) fn primary(&self) -> SceneEntityId {
        self.targets.primary
    }

    pub(crate) fn primary_initial(&self) -> Transform3D {
        self.targets.primary_initial()
    }
}

/// 一次 Gizmo 拖拽的**目标集合**：主实体 + 每个目标的初始世界变换。
///
/// `draw_scene_3d` 用它把"拖谁"这件事一次算清（含锁定过滤），拖拽期间只做纯变换。
#[derive(Clone, Debug)]
pub(crate) struct GizmoDragTargets {
    /// 主选中实体：Inspector 同步、历史条目归属、局部轴方向都以它为准。
    pub primary: SceneEntityId,
    /// `(实体, 初始世界变换)`，顺序与选中集合一致。
    pub targets: Vec<(SceneEntityId, Transform3D)>,
}

impl GizmoDragTargets {
    /// 从选中集合构造；`locked` 里的实体不参与拖拽（与"锁定不响应手柄"一致）。
    pub(crate) fn new(
        primary: SceneEntityId,
        selected: impl IntoIterator<Item = (SceneEntityId, Transform3D)>,
        locked: &std::collections::BTreeSet<SceneEntityId>,
    ) -> Option<Self> {
        let targets: Vec<(SceneEntityId, Transform3D)> = selected
            .into_iter()
            .filter(|(id, _)| !locked.contains(id))
            .collect();
        if targets.is_empty() {
            return None;
        }
        let primary = if targets.iter().any(|(id, _)| *id == primary) {
            primary
        } else {
            targets[0].0
        };
        Some(Self { primary, targets })
    }

    /// 主选中实体的初始变换（局部空间轴与旋转方向都以它为准）。
    pub(crate) fn primary_initial(&self) -> Transform3D {
        self.targets
            .iter()
            .find(|(id, _)| *id == self.primary)
            .map(|(_, transform)| *transform)
            .unwrap_or_else(|| self.targets[0].1)
    }
}

/// 单实体的锚点（等价于 `Center` 模式下"自己的 AABB 中心"，`Pivot` 模式下自己的原点）。
///
/// 保留这个单实体入口是为了让"多选推广"这件事可对照：旧行为在这里逐条钉住。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn single_anchor(pivot: GizmoPivot, position: Vec3, bounds: Aabb3) -> Vec3 {
    gizmo_anchor(pivot, position, [(position, bounds)])
}

/// 多选拖拽的锚点：`Pivot` = 主实体原点；`Center` = 选中集合并集包围盒中心。
pub(crate) fn gizmo_anchor(
    pivot: GizmoPivot,
    primary_position: Vec3,
    selection: impl IntoIterator<Item = (Vec3, Aabb3)>,
) -> Vec3 {
    match pivot {
        GizmoPivot::Pivot => primary_position,
        GizmoPivot::Center => selection_bounds_center(selection).unwrap_or(primary_position),
    }
}

/// 移动工具：**同一份世界增量**作用于每个目标（多选整体平移）。
pub(crate) fn translate_targets(
    targets: &[(SceneEntityId, Transform3D)],
    delta: Vec3,
    snap: Option<f32>,
) -> Vec<(SceneEntityId, Transform3D)> {
    targets
        .iter()
        .map(|(id, initial)| {
            let mut next = *initial;
            next.position = initial.position + delta;
            if let Some(step) = snap
                && step > 0.0
            {
                next.position = Vec3::new(
                    (next.position.x / step).round() * step,
                    (next.position.y / step).round() * step,
                    (next.position.z / step).round() * step,
                );
            }
            (*id, next)
        })
        .collect()
}

/// 单个实体的旋转：`slot` 是"哪一圈"（世界轴单位向量当槽位），`pivot = Some(中心)` 时
/// 同时绕该点公转。
pub(crate) fn rotate_about(
    space: GizmoSpace,
    slot: Vec3,
    initial: &Transform3D,
    pivot: Option<Vec3>,
    angle: f32,
) -> (Vec3, rustlua_math::math3d::Quat) {
    let delta = rotation_delta(space, initial.rotation, slot, angle);
    let rotation = delta * initial.rotation;
    let position = match pivot {
        None => initial.position,
        Some(pivot) => {
            let offset = initial.position - pivot;
            pivot + delta.rotate(offset).unwrap_or(offset)
        }
    };
    (position, rotation)
}

/// 单个实体的缩放：`mask` 外的分量与位置轴**保持不动**（拖 X 把手不该把实体沿 Y 推开）。
pub(crate) fn scale_about(
    initial: &Transform3D,
    pivot: Option<Vec3>,
    mask: [bool; 3],
    factor: f32,
) -> (Vec3, Vec3) {
    let factor = factor.max(0.05);
    let offset = initial.position - pivot.unwrap_or(Vec3::ZERO);
    let position = match pivot {
        None => initial.position,
        Some(pivot) => {
            // 没参与缩放的轴上位置不变——否则单轴缩放会把实体整体搬离中心。
            let moved = Vec3::new(
                if mask[0] { offset.x * factor } else { offset.x },
                if mask[1] { offset.y * factor } else { offset.y },
                if mask[2] { offset.z * factor } else { offset.z },
            );
            pivot + moved
        }
    };
    let scale = Vec3::new(
        if mask[0] {
            (initial.scale.x * factor).max(0.05)
        } else {
            initial.scale.x
        },
        if mask[1] {
            (initial.scale.y * factor).max(0.05)
        } else {
            initial.scale.y
        },
        if mask[2] {
            (initial.scale.z * factor).max(0.05)
        } else {
            initial.scale.z
        },
    );
    (position, scale)
}

/// 旋转工具：每个目标各自按同一角度旋转；`pivot = Some(中心)` 时同时绕该点公转。
///
/// `slot` 决定绕哪根轴：世界模式下就是这一根世界轴，局部模式下**每个目标绕它自己的**
/// 同名轴（与迁移前只认 Y 的行为同一套解析，只是那时槽位写死成 Y）。
pub(crate) fn rotate_targets(
    space: GizmoSpace,
    slot: Vec3,
    targets: &[(SceneEntityId, Transform3D)],
    pivot: Option<Vec3>,
    angle: f32,
) -> Vec<(SceneEntityId, Transform3D)> {
    targets
        .iter()
        .map(|(id, initial)| {
            let (position, rotation) = rotate_about(space, slot, initial, pivot, angle);
            let mut next = *initial;
            next.position = position;
            next.rotation = rotation;
            (*id, next)
        })
        .collect()
}

/// 缩放工具：每个目标各自按 `mask` 缩放；`pivot = Some(中心)` 时参与缩放的轴同步远离/靠近该点。
pub(crate) fn scale_axis_targets(
    targets: &[(SceneEntityId, Transform3D)],
    pivot: Option<Vec3>,
    mask: [bool; 3],
    factor: f32,
) -> Vec<(SceneEntityId, Transform3D)> {
    targets
        .iter()
        .map(|(id, initial)| {
            let (position, scale) = scale_about(initial, pivot, mask, factor);
            let mut next = *initial;
            next.position = position;
            next.scale = scale;
            (*id, next)
        })
        .collect()
}

/// 旋转增量四元数：世界模式绕给定的世界轴，局部模式绕实体**自己的**同名轴。
///
/// 四元数非法（非单位/非有限）时退回世界轴——坏掉的旋转不该让手柄整根失效。
pub(crate) fn rotation_delta(
    space: GizmoSpace,
    rotation: rustlua_math::math3d::Quat,
    slot: Vec3,
    angle: f32,
) -> rustlua_math::math3d::Quat {
    let axis = match space {
        GizmoSpace::World => slot,
        GizmoSpace::Local => rotation.rotate(slot).unwrap_or(slot),
    };
    rustlua_math::math3d::Quat::from_axis_angle(axis, angle)
        .unwrap_or_else(|_| rustlua_math::math3d::Quat::identity())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_ecs::transform::Transform3D;
    use rustlua_math::math3d::{Quat, Vec3};

    fn target(id: u64, x: f32) -> (SceneEntityId, Transform3D) {
        (
            SceneEntityId(id),
            Transform3D {
                position: Vec3::new(x, 0.0, 0.0),
                rotation: Quat::identity(),
                scale: Vec3::new(1.0, 1.0, 1.0),
            },
        )
    }

    #[test]
    fn segment_distance_uses_the_whole_arm_not_just_the_tip() {
        let a = egui::pos2(0.0, 0.0);
        let b = egui::pos2(100.0, 0.0);
        // 臂中间正上方 3 点：到线段的距离是 3，而到端点的距离是 ~50。
        assert!((distance_point_to_segment(egui::pos2(50.0, 3.0), a, b) - 3.0).abs() < 1e-3);
        // 超出末端：最近点被夹到端点 b。
        assert!((distance_point_to_segment(egui::pos2(130.0, 0.0), a, b) - 30.0).abs() < 1e-3);
        // 退化线段：退化为点到点。
        assert!((distance_point_to_segment(egui::pos2(3.0, 4.0), a, a) - 5.0).abs() < 1e-3);
    }

    #[test]
    fn move_gizmo_arm_is_grabbable_along_its_length() {
        let center = egui::pos2(100.0, 100.0);
        let axes = [
            egui::vec2(40.0, 0.0),
            egui::vec2(0.0, -40.0),
            egui::vec2(-28.0, -28.0),
        ];
        // 臂中点（旧实现抓不到，只有端点圆内才命中）。
        assert_eq!(
            pick_gizmo_axis(
                SceneTool::Move,
                center,
                egui::pos2(120.0, 101.0),
                false,
                axes
            ),
            Some(GizmoAxis::X)
        );
        // 端点仍然命中。
        assert_eq!(
            pick_gizmo_axis(
                SceneTool::Move,
                center,
                egui::pos2(140.0, 100.0),
                false,
                axes
            ),
            Some(GizmoAxis::X)
        );
        // 远离所有臂 → 未命中。
        assert_eq!(
            pick_gizmo_axis(
                SceneTool::Move,
                center,
                egui::pos2(300.0, 300.0),
                false,
                axes
            ),
            None
        );
    }

    #[test]
    fn rotation_center_still_grabs_and_rings_are_picked_separately() {
        let center = egui::pos2(100.0, 100.0);
        let axes = [
            egui::vec2(40.0, 0.0),
            egui::vec2(0.0, -40.0),
            egui::vec2(-28.0, -28.0),
        ];
        // 中心方框 = 原地绕世界 Y 转（迁移前唯一的行为，新增两环不能把它弄丢）。
        assert_eq!(
            pick_gizmo_axis(SceneTool::Rotate, center, center, false, axes),
            Some(GizmoAxis::Y)
        );
        // 环上的点不再由这个入口判定——三支环要先投影成椭圆才能比距离（见下一条）。
        assert_eq!(
            pick_gizmo_axis(
                SceneTool::Rotate,
                center,
                center + egui::vec2(40.0, 0.0),
                false,
                axes
            ),
            None
        );
    }

    #[test]
    fn rotation_ring_picking_prefers_the_nearest_ring() {
        // 两条"屏幕椭圆"用折线近似：一条正圆（绕 Z 的环），一条水平压扁（绕 Y 的环）。
        // 三条环在原实现里靠列表顺序决胜负，现在必须靠**真实距离**，所以取样点上特意让
        // 两条环各有一次"明显更近"。
        let round = ring_polyline_from(egui::pos2(100.0, 100.0), 40.0, 1.0);
        let flat = ring_polyline_from(egui::pos2(100.0, 100.0), 40.0, 0.25);
        // 正下方 40 点：只有圆环经过（扁环只到 10 点高），而列表里扁环排在前面。
        assert_eq!(
            pick_rotation_ring(
                egui::pos2(100.0, 140.0),
                &[(GizmoAxis::Y, flat.clone()), (GizmoAxis::X, round.clone())],
            ),
            Some(GizmoAxis::X),
            "扁环不覆盖的位置必须落到真正经过的那条环"
        );
        // 正下方 10 点：扁环经过、圆环差 30 ⇒ 即使圆环排在前面也必须选扁环。
        assert_eq!(
            pick_rotation_ring(
                egui::pos2(100.0, 110.0),
                &[(GizmoAxis::X, round.clone()), (GizmoAxis::Y, flat.clone())],
            ),
            Some(GizmoAxis::Y),
            "命中取的是最近的那条环，不是列表里的第一条"
        );
        // 环外：两条都不许命中（不会把整个画布变成旋转手柄）。
        assert_eq!(
            pick_rotation_ring(
                egui::pos2(100.0, 125.0),
                &[(GizmoAxis::X, round), (GizmoAxis::Y, flat)]
            ),
            None
        );
        // 折线不足两点 ⇒ 没有"线"可言。
        assert_eq!(distance_point_to_ring(egui::pos2(0.0, 0.0), &[]), None);
        assert_eq!(
            pick_rotation_ring(
                egui::pos2(0.0, 0.0),
                &[(GizmoAxis::Y, vec![egui::pos2(1.0, 1.0)])]
            ),
            None
        );
    }

    /// 测试用：把屏幕椭圆（长轴 40、短轴 40×`squash`）采样成闭合折线。
    fn ring_polyline_from(center: egui::Pos2, radius: f32, squash: f32) -> Vec<egui::Pos2> {
        (0..GIZMO_RING_SEGMENTS)
            .map(|index| {
                let angle = index as f32 * (std::f32::consts::TAU / GIZMO_RING_SEGMENTS as f32);
                center + egui::vec2(angle.cos() * radius, angle.sin() * radius * squash)
            })
            .collect()
    }

    #[test]
    fn ring_geometry_projects_a_real_ellipse_not_a_circle() {
        // 手搓一个"相机"：世界点 (x, y, z) → 屏幕 (200 + x, 200 − y)，Z 轴正对屏幕。
        let project = |point: Vec3| Some(egui::pos2(200.0 + point.x, 200.0 - point.y));
        let center = Vec3::new(0.0, 0.0, 0.0);
        // 绕 Y 的环：法向 (0,1,0) ⇒ 落在 XZ 平面 ⇒ 投影成一条横线，短轴为 0。
        let radius = ring_radius_world(center, Vec3::Y, 40.0, project).unwrap();
        assert!((radius - 40.0).abs() < 1e-3, "尺度取自面内那条可见的轴");
        let flat = ring_polyline(center, Vec3::Y, radius, project);
        assert_eq!(flat.len(), GIZMO_RING_SEGMENTS);
        assert!(
            flat.iter().all(|point| (point.y - 200.0).abs() < 1e-3),
            "绕 Y 的环必须投影成一条横线"
        );
        // 绕 Z 的环：落在 XY 平面（正对相机）⇒ 正圆。
        let radius = ring_radius_world(center, Vec3::Z, 40.0, project).unwrap();
        let round = ring_polyline(center, Vec3::Z, radius, project);
        for point in &round {
            assert!(
                ((*point - egui::pos2(200.0, 200.0)).length() - 40.0).abs() < 1e-2,
                "正对相机的环必须画成半径 40 的正圆"
            );
        }
        // 面内两向都被压扁时量不出尺度 ⇒ 拒绝（调用方跳过这一环）。
        let degenerate = |_: Vec3| Some(egui::pos2(200.0, 200.0));
        assert_eq!(ring_radius_world(center, Vec3::Y, 40.0, degenerate), None);
    }

    #[test]
    fn ring_angle_is_signed_and_degrades_to_none() {
        // 环面 = XZ 平面，法向 Y；从 +X 转到 +Z 是 -90°（右手定则绕 +Y：X→ -Z 才是正向）。
        let center = Vec3::ZERO;
        let start = Vec3::new(1.0, 0.0, 0.0);
        let quarter = Vec3::new(0.0, 0.0, -1.0);
        let angle = ring_angle(center, Vec3::Y, start, quarter).expect("有转角");
        assert!(
            (angle - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "绕 Y 正向 90°"
        );
        // 反向对称。
        let back = ring_angle(center, Vec3::Y, quarter, start).expect("有转角");
        assert!((back + std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        // 交点与环心重合：角度无定义，必须 None 而不是 NaN。
        assert!(ring_angle(center, Vec3::Y, start, center).is_none());
    }

    #[test]
    fn ray_plane_point_refuses_an_edge_on_view() {
        let ray = Ray3 {
            origin: Vec3::new(0.0, 10.0, 10.0),
            direction: Vec3::new(0.0, -1.0, -1.0).normalized().unwrap(),
        };
        // 视线与 XZ 平面（法向 Y）成 45°，交点在 (0, 0, 0)。
        let hit = ray_plane_point(ray, Vec3::ZERO, Vec3::Y).expect("相交");
        assert!((hit - Vec3::ZERO).length() < 1e-4);
        // 几乎平行 ⇒ 交点跑到无穷远，拒绝。
        let flat = Ray3 {
            origin: Vec3::new(0.0, 10.0, 0.0),
            direction: Vec3::new(1.0, 0.0, 0.0),
        };
        assert_eq!(ray_plane_point(flat, Vec3::ZERO, Vec3::Y), None);
        // 平面在相机背后 ⇒ 也拒绝（不能拿负距离当交点）。
        let behind = Ray3 {
            origin: Vec3::new(0.0, 0.0, 0.0),
            direction: Vec3::new(0.0, 1.0, 0.0),
        };
        assert_eq!(
            ray_plane_point(behind, Vec3::new(0.0, -5.0, 0.0), Vec3::Y),
            None
        );
    }

    #[test]
    fn angle_snap_rounds_to_the_step_in_radians() {
        assert!(
            (snap_angle(0.7, None) - 0.7).abs() < 1e-6,
            "不吸附时原样返回"
        );
        // 10° 以内的一半 → 0°；超过 → 整倍。
        assert!(snap_angle(4.0_f32.to_radians(), Some(15.0)).abs() < 1e-6);
        assert!(
            (snap_angle(10.0_f32.to_radians(), Some(15.0)) - 15.0_f32.to_radians()).abs() < 1e-5
        );
        assert!(
            (snap_angle(160.0_f32.to_radians(), Some(15.0)) - 165.0_f32.to_radians()).abs() < 1e-4
        );
        // 步长 0 = 关闭（不能把自己除成 NaN 再写进变换里）。
        assert!((snap_angle(0.7, Some(0.0)) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn the_drag_readout_reports_the_number_the_user_just_changed() {
        // 浮标是用户唯一能在画布上读到的数：位置报**当前值**，旋转报**转了多少**，
        // 3D 旋转还要报绕的是哪根轴（现在有三支环，不说清就看不懂跳的是哪个 15°）。
        assert_eq!(
            readout_2d(SceneTool::Move, (1.25, -2.0), 0.0, (1.0, 1.0)),
            "位置 1.25, -2.00"
        );
        assert_eq!(
            readout_2d(SceneTool::Rotate, (0.0, 0.0), 45.0, (1.0, 1.0)),
            "角度 +45.0°"
        );
        assert_eq!(
            readout_2d(SceneTool::Scale, (0.0, 0.0), 0.0, (2.0, 3.0)),
            "缩放 2.00 × 3.00"
        );
        let transform = Transform3D {
            position: Vec3::new(1.5, 2.0, 3.0),
            rotation: Quat::identity(),
            scale: Vec3::new(1.0, 2.0, 3.0),
        };
        assert_eq!(
            readout_3d(SceneTool::Move, GizmoAxis::Free, &transform, 0.0),
            "位置 1.50, 2.00, 3.00"
        );
        assert_eq!(
            readout_3d(
                SceneTool::Rotate,
                GizmoAxis::Z,
                &transform,
                -std::f32::consts::FRAC_PI_2
            ),
            "绕 Z -90.0°"
        );
        assert_eq!(
            readout_3d(SceneTool::Scale, GizmoAxis::ScaleXZ, &transform, 0.0),
            "缩放 1.00 × 2.00 × 3.00"
        );
        // 笔刷工具没有"拖出来的数值"，必须交空串让调用方跳过绘制（而不是画个 0,0,0）。
        assert_eq!(
            readout_2d(SceneTool::TilePaint, (0.0, 0.0), 0.0, (1.0, 1.0)),
            ""
        );
        assert_eq!(
            readout_3d(SceneTool::TilePaint, GizmoAxis::X, &transform, 0.0),
            ""
        );
    }

    #[test]
    fn scale_handles_are_grabbable_and_drawn_from_the_same_numbers() {
        let center = egui::pos2(100.0, 100.0);
        let axes = [
            egui::vec2(40.0, 0.0),
            egui::vec2(0.0, -40.0),
            egui::vec2(-28.0, -28.0),
        ];
        // 三个轴把手在臂端，三块平面把手在两臂夹角的中段。
        for (axis, offset) in scale_handles(axes) {
            assert_eq!(
                pick_gizmo_axis(SceneTool::Scale, center, center + offset, false, axes),
                Some(axis),
                "把手 {} 必须点在它画出来的位置上",
                axis_name(axis)
            );
        }
        // 中心 = 等比（旧的对角把手取消后，等比仍然有入口）。
        assert_eq!(
            pick_gizmo_axis(SceneTool::Scale, center, center, false, axes),
            Some(GizmoAxis::Scale)
        );
        // 把手之外没有别的东西可抓。
        assert_eq!(
            pick_gizmo_axis(
                SceneTool::Scale,
                center,
                center + egui::vec2(60.0, 60.0),
                false,
                axes
            ),
            None
        );
    }

    #[test]
    fn scaling_about_the_center_scales_positions_and_sizes() {
        let targets = vec![target(1, -5.0), target(2, 5.0)];
        let all = [true, true, true];
        let scaled = scale_axis_targets(&targets, Some(Vec3::new(0.0, 0.0, 0.0)), all, 2.0);
        assert!((scaled[0].1.position - Vec3::new(-10.0, 0.0, 0.0)).length() < 1e-4);
        assert!((scaled[1].1.position - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-4);
        assert!((scaled[0].1.scale.x - 2.0).abs() < 1e-4);
        // 缩放下界：不会翻负或归零。
        let shrunk = scale_axis_targets(&targets, None, all, -5.0);
        assert!(shrunk[0].1.scale.x >= 0.05);
    }

    #[test]
    fn a_single_axis_scale_leaves_the_other_axes_alone() {
        let mut initial = Transform3D {
            position: Vec3::new(4.0, 6.0, 8.0),
            rotation: Quat::identity(),
            scale: Vec3::new(2.0, 2.0, 2.0),
        };
        // 原地（pivot=None）拖 X 把手：只有 x 分量变。
        let (position, scale) = scale_about(&initial, None, [true, false, false], 2.0);
        assert_eq!(position, Vec3::new(4.0, 6.0, 8.0));
        assert_eq!(scale, Vec3::new(4.0, 2.0, 2.0));
        // 绕中心拖 XZ 平面把手：x/z 两轴一起缩，y 与它到中心的位置偏移都不动。
        initial.position = Vec3::new(4.0, 3.0, 0.0);
        let (position, scale) = scale_about(
            &initial,
            Some(Vec3::new(0.0, 3.0, 0.0)),
            [true, false, true],
            2.0,
        );
        assert!(
            (position - Vec3::new(8.0, 3.0, 0.0)).length() < 1e-4,
            "y 不该被搬走"
        );
        assert!((scale - Vec3::new(4.0, 2.0, 4.0)).length() < 1e-4);
    }

    #[test]
    fn the_scale_handle_moves_the_value_along_its_own_screen_direction() {
        let up = egui::vec2(0.0, -40.0);
        // 顺着把手方向往外拖 = 变大；反方向 = 变小；垂直方向的抖动不算缩放。
        assert!((scale_factor(egui::vec2(0.0, -20.0), up) - 1.2).abs() < 1e-5);
        assert!((scale_factor(egui::vec2(0.0, 20.0), up) - 0.8).abs() < 1e-5);
        assert!((scale_factor(egui::vec2(20.0, 0.0), up) - 1.0).abs() < 1e-5);
        // 把手投影退化成一个点时退回水平增量（与迁移前的等比手感一致）。
        assert!((scale_factor(egui::vec2(30.0, 0.0), egui::Vec2::ZERO) - 1.3).abs() < 1e-5);
        // 下界。
        assert!(scale_factor(egui::vec2(0.0, 100000.0), up) >= 0.05);
    }

    #[test]
    fn selection_center_is_the_union_aabb_center() {
        let bounds = Aabb3 {
            min: Vec3::new(-0.5, -0.5, -0.5),
            max: Vec3::new(0.5, 0.5, 0.5),
        };
        // 单实体：与 position + (min+max)/2 相同。
        let single = selection_bounds_center([(Vec3::new(5.0, 1.0, 0.0), bounds)]).unwrap();
        assert!((single - Vec3::new(5.0, 1.0, 0.0)).length() < 1e-4);
        // 多实体：两点中点。
        let pair = selection_bounds_center([
            (Vec3::new(0.0, 0.0, 0.0), bounds),
            (Vec3::new(10.0, 4.0, 0.0), bounds),
        ])
        .unwrap();
        assert!((pair - Vec3::new(5.0, 2.0, 0.0)).length() < 1e-4);
        assert!(selection_bounds_center([]).is_none());
    }

    #[test]
    fn translating_a_selection_moves_every_target_by_the_same_delta() {
        let targets = vec![target(1, 0.0), target(2, 10.0)];
        let moved = translate_targets(&targets, Vec3::new(2.0, 3.0, 0.0), None);
        assert_eq!(moved.len(), 2);
        assert!((moved[0].1.position - Vec3::new(2.0, 3.0, 0.0)).length() < 1e-4);
        assert!((moved[1].1.position - Vec3::new(12.0, 3.0, 0.0)).length() < 1e-4);
        // 相对距离保持不变（整体平移，不变形）。
        let before = targets[1].1.position - targets[0].1.position;
        let after = moved[1].1.position - moved[0].1.position;
        assert!((after - before).length() < 1e-4);
    }

    #[test]
    fn snapping_applies_to_every_target() {
        let targets = vec![target(1, 0.4), target(2, 10.4)];
        let snapped = translate_targets(&targets, Vec3::new(0.3, 0.0, 0.0), Some(0.5));
        for (_, transform) in &snapped {
            assert!((transform.position.x * 2.0).fract().abs() < 1e-4);
        }
    }

    #[test]
    fn rotating_about_the_center_orbits_every_target() {
        let targets = vec![target(1, -5.0), target(2, 5.0)];
        let pivot = Vec3::new(0.0, 0.0, 0.0);
        let rotated = rotate_targets(
            GizmoSpace::World,
            Vec3::Y,
            &targets,
            Some(pivot),
            std::f32::consts::PI,
        );
        // 半圈之后两个实体互换位置（围绕中心公转）。
        assert!((rotated[0].1.position - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-3);
        assert!((rotated[1].1.position - Vec3::new(-5.0, 0.0, 0.0)).length() < 1e-3);
        // 原地旋转（pivot=None）时位置不动。
        let in_place = rotate_targets(
            GizmoSpace::World,
            Vec3::Y,
            &targets,
            None,
            std::f32::consts::PI,
        );
        assert!((in_place[0].1.position - Vec3::new(-5.0, 0.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn a_ring_rotates_about_its_own_axis_not_always_y() {
        let at_front = Transform3D {
            position: Vec3::new(0.0, 0.0, 5.0),
            rotation: Quat::identity(),
            scale: Vec3::new(1.0, 1.0, 1.0),
        };
        let (position, _) = rotate_about(GizmoSpace::World, Vec3::X, &at_front, None, 0.0);
        assert_eq!(position, at_front.position, "零角不动");
        let (position, rotation) = rotate_about(
            GizmoSpace::World,
            Vec3::X,
            &at_front,
            Some(Vec3::ZERO),
            std::f32::consts::FRAC_PI_2,
        );
        // 绕 +X 转 90°：Z 轴上的点搬到 −Y（右手定则，与 `Quat::from_axis_angle` 同源）。
        assert!((position - Vec3::new(0.0, -5.0, 0.0)).length() < 1e-3);
        assert!((rotation.rotate(Vec3::Y).unwrap_or(Vec3::Y) - Vec3::Z).length() < 1e-3);
        // 绕 Y 的环在同样起点上走的是另一条轨道。
        let at_right = Transform3D {
            position: Vec3::new(5.0, 0.0, 0.0),
            ..at_front
        };
        let (position, _) = rotate_about(
            GizmoSpace::World,
            Vec3::Y,
            &at_right,
            Some(Vec3::ZERO),
            std::f32::consts::FRAC_PI_2,
        );
        assert!((position - Vec3::new(0.0, 0.0, -5.0)).length() < 1e-3);
    }

    #[test]
    fn a_local_ring_follows_each_entitys_own_axis() {
        let unit = Vec3::new(1.0, 1.0, 1.0);
        // 两个实体朝向不同：局部模式下"绕 X 圈转 90°"该各自绕自己的 X。
        let turned =
            Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2).expect("合法四元数");
        let targets = vec![
            (
                SceneEntityId(1),
                Transform3D {
                    position: Vec3::ZERO,
                    rotation: Quat::identity(),
                    scale: unit,
                },
            ),
            (
                SceneEntityId(2),
                Transform3D {
                    position: Vec3::ZERO,
                    rotation: turned,
                    scale: unit,
                },
            ),
        ];
        let rotated = rotate_targets(
            GizmoSpace::Local,
            Vec3::X,
            &targets,
            None,
            std::f32::consts::FRAC_PI_2,
        );
        assert_ne!(
            rotated[0].1.rotation, rotated[1].1.rotation,
            "局部模式该按各自的轴转，而不是共享一条世界轴"
        );
        // 第一个实体没预转 ⇒ 它的局部 X 就是世界 X：绕 +X 转 90° 把 +Y 搬到 +Z（右手定则）。
        let moved = rotated[0].1.rotation.rotate(Vec3::Y).unwrap();
        assert!((moved - Vec3::new(0.0, 0.0, 1.0)).length() < 1e-4);
    }

    #[test]
    fn anchor_follows_the_pivot_mode() {
        let bounds = Aabb3 {
            min: Vec3::new(-0.5, -0.5, -0.5),
            max: Vec3::new(0.5, 0.5, 0.5),
        };
        let selection = [
            (Vec3::new(0.0, 0.0, 0.0), bounds),
            (Vec3::new(8.0, 0.0, 0.0), bounds),
        ];
        let pivot_anchor = gizmo_anchor(GizmoPivot::Pivot, Vec3::new(0.0, 0.0, 0.0), selection);
        assert!((pivot_anchor - Vec3::new(0.0, 0.0, 0.0)).length() < 1e-4);
        let center_anchor = gizmo_anchor(GizmoPivot::Center, Vec3::new(0.0, 0.0, 0.0), selection);
        assert!((center_anchor - Vec3::new(4.0, 0.0, 0.0)).length() < 1e-4);
    }
}
