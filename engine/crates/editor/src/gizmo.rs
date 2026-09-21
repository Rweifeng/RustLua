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

use crate::scene_interaction::{GizmoPivot, GizmoSpace};
use crate::{GizmoAxis, SceneTool};
use eframe::egui;
use engine::core::{
    math3d::{Aabb3, Vec3},
    scene::SceneEntityId,
    transform::Transform3D,
};

/// 手柄命中半径（点）。比绘制半径大一点：细线手柄要"好抓"。
pub(crate) const GIZMO_HIT_RADIUS: f32 = 8.0;
/// 旋转/缩放手柄（单点）的命中半径（点）。
pub(crate) const GIZMO_HANDLE_RADIUS: f32 = 10.0;
/// 手柄中心（自由拖动 / 原地旋转缩放）的命中方框半边长（点）。
pub(crate) const GIZMO_CENTER_HALF: f32 = 12.0;

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
            // 旋转手柄画的是**整圈**（半径 = 臂长），命中判定必须覆盖整圈：
            // 只判臂末端那个小球的话，就成了"画了一整圈、只有一点能抓"——与刚修掉的
            // "臂只能抓端点"属同一类问题（由 `rotation_ring_is_grabbable_all_around` 钉住）。
            let ring_radius = axes[0].length();
            let from_center = (position - center).length();
            if center_box.contains(position)
                || (from_center - ring_radius).abs() < GIZMO_HANDLE_RADIUS
                || distance_point_to_segment(position, center, center + axes[0])
                    < GIZMO_HANDLE_RADIUS
            {
                Some(GizmoAxis::X)
            } else {
                None
            }
        }
        SceneTool::Scale => {
            let handle = (axes[0] + axes[1]) * 0.7;
            if center_box.contains(position)
                || distance_point_to_segment(position, center, center + handle)
                    < GIZMO_HANDLE_RADIUS
            {
                Some(GizmoAxis::Scale)
            } else {
                None
            }
        }
        SceneTool::TilePaint => None,
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
    pub start_ray: Option<engine::core::camera::Ray3>,
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

/// 单个实体的旋转：`pivot = Some(中心)` 时同时绕该点公转。
pub(crate) fn rotate_about(
    space: GizmoSpace,
    initial: &Transform3D,
    pivot: Option<Vec3>,
    angle: f32,
) -> (Vec3, engine::core::math3d::Quat) {
    let delta = rotation_delta(space, initial.rotation, angle);
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

/// 单个实体的等比缩放：`pivot = Some(中心)` 时位置同步远离/靠近该点。
pub(crate) fn scale_about(initial: &Transform3D, pivot: Option<Vec3>, factor: f32) -> (Vec3, Vec3) {
    let factor = factor.max(0.05);
    let position = match pivot {
        None => initial.position,
        Some(pivot) => pivot + (initial.position - pivot) * factor,
    };
    let scale = Vec3::new(
        (initial.scale.x * factor).max(0.05),
        (initial.scale.y * factor).max(0.05),
        (initial.scale.z * factor).max(0.05),
    );
    (position, scale)
}

/// 旋转工具：每个目标各自按同一角度旋转；`pivot = Some(中心)` 时同时绕该点公转。
pub(crate) fn rotate_targets(
    space: GizmoSpace,
    targets: &[(SceneEntityId, Transform3D)],
    pivot: Option<Vec3>,
    angle: f32,
) -> Vec<(SceneEntityId, Transform3D)> {
    targets
        .iter()
        .map(|(id, initial)| {
            let (position, rotation) = rotate_about(space, initial, pivot, angle);
            let mut next = *initial;
            next.position = position;
            next.rotation = rotation;
            (*id, next)
        })
        .collect()
}

/// 缩放工具：每个目标各自等比缩放；`pivot = Some(中心)` 时位置同步远离/靠近该点。
pub(crate) fn scale_targets(
    targets: &[(SceneEntityId, Transform3D)],
    pivot: Option<Vec3>,
    factor: f32,
) -> Vec<(SceneEntityId, Transform3D)> {
    targets
        .iter()
        .map(|(id, initial)| {
            let (position, scale) = scale_about(initial, pivot, factor);
            let mut next = *initial;
            next.position = position;
            next.scale = scale;
            (*id, next)
        })
        .collect()
}

/// 旋转增量四元数：世界模式绕 Y，局部模式绕实体自己的 Y 轴。
///
/// 四元数非法（非单位/非有限）时退回世界轴——坏掉的旋转不该让手柄整根失效。
pub(crate) fn rotation_delta(
    space: GizmoSpace,
    rotation: engine::core::math3d::Quat,
    angle: f32,
) -> engine::core::math3d::Quat {
    let axis = match space {
        GizmoSpace::World => Vec3::Y,
        GizmoSpace::Local => rotation.rotate(Vec3::Y).unwrap_or(Vec3::Y),
    };
    engine::core::math3d::Quat::from_axis_angle(axis, angle)
        .unwrap_or_else(|_| engine::core::math3d::Quat::identity())
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::{
        math3d::{Quat, Vec3},
        transform::Transform3D,
    };

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
    fn rotation_ring_is_grabbable_all_around() {
        let center = egui::pos2(100.0, 100.0);
        let len = 40.0_f32;
        let axes = [
            egui::vec2(len, 0.0),
            egui::vec2(0.0, -len),
            egui::vec2(-len * 0.7, -len * 0.7),
        ];
        // 圆环上任取几点（含与臂方向垂直的位置）都应命中：旋转手柄画的就是整圈。
        for angle in [0.0_f32, 1.0, 2.0, 3.0, 4.0, 5.0] {
            let point = center + egui::vec2(angle.cos(), angle.sin()) * len;
            assert_eq!(
                pick_gizmo_axis(SceneTool::Rotate, center, point, false, axes),
                Some(GizmoAxis::X),
                "圆环上角度 {angle} 的位置必须可抓"
            );
        }
        // 环外远处不命中（不会把整个画布都变成旋转手柄）。
        assert_eq!(
            pick_gizmo_axis(
                SceneTool::Rotate,
                center,
                center + egui::vec2(len * 3.0, 0.0),
                false,
                axes
            ),
            None
        );
        // 中心仍然可抓（原地旋转）。
        assert_eq!(
            pick_gizmo_axis(SceneTool::Rotate, center, center, false, axes),
            Some(GizmoAxis::X)
        );
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
            &targets,
            Some(pivot),
            std::f32::consts::PI,
        );
        // 半圈之后两个实体互换位置（围绕中心公转）。
        assert!((rotated[0].1.position - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-3);
        assert!((rotated[1].1.position - Vec3::new(-5.0, 0.0, 0.0)).length() < 1e-3);
        // 原地旋转（pivot=None）时位置不动。
        let in_place = rotate_targets(GizmoSpace::World, &targets, None, std::f32::consts::PI);
        assert!((in_place[0].1.position - Vec3::new(-5.0, 0.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn scaling_about_the_center_scales_positions_and_sizes() {
        let targets = vec![target(1, -5.0), target(2, 5.0)];
        let scaled = scale_targets(&targets, Some(Vec3::new(0.0, 0.0, 0.0)), 2.0);
        assert!((scaled[0].1.position - Vec3::new(-10.0, 0.0, 0.0)).length() < 1e-4);
        assert!((scaled[1].1.position - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-4);
        assert!((scaled[0].1.scale.x - 2.0).abs() < 1e-4);
        // 缩放下界：不会翻负或归零。
        let shrunk = scale_targets(&targets, None, -5.0);
        assert!(shrunk[0].1.scale.x >= 0.05);
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
