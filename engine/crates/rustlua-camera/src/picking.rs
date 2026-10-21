//! 拾取的**共享投影/查询链**（docs/54 §8-⑦）：屏幕 → 视口 → 相机射线 → 世界查询。
//!
//! 为什么要有这个模块：编辑器 3D 点选、Lua `camera3d.*` 与运行时/物理查询三处各自拼过这一段
//! 数学（或干脆各写一套），于是出现"点中的位置和看到的位置不是同一个"这类只有真机上才暴露的
//! 偏差。这里把这条链写成**一处**：
//!
//! ```text
//! 屏幕像素 --ray_from_screen--> 世界射线 --ray_hits_aabb / ray_plane_point--> 命中
//! ```
//!
//! 投影数学仍在 [`crate::camera::Camera3D::ray_from_screen`] / [`crate::camera::Camera3D::world_to_ndc`]（`Camera2D` 与它同源），
//! 本模块只负责"把链子接起来"并提供最近命中挑选，因此不会产生第二份投影实现。
//!
//! **位置**（P1 ⑤-8）：这条链只依赖相机数学与几何，因此它住在相机 crate 里；`rustlua-render`
//! 保留同路径的再导出（`rustlua_render::picking::*`），编辑器与宿主的调用点一字未改。
//! 搬家的动因是 `camera3d.*` 绑定要下沉到本 crate——让相机 crate 反向依赖 render 才是错方向。

use crate::camera::{Camera3D, CameraView, Viewport};
use rustlua_math::geom3d::Ray3;
use rustlua_math::math2d::Vec2;
use rustlua_math::math3d::{Aabb3, Vec3};

/// 屏幕像素 → 世界射线。与编辑器 3D 点选、Lua `camera3d.ray` 共用同一入口。
///
/// 宽高比与屏幕坐标**必须**来自同一个视口：`Camera3D` 里没有比例字段，因此这里不可能出现
/// "投影按 A 比例、屏幕按 B 尺寸"的错位（[`crate::camera::Camera3D::ray_from_screen`] 的视口参数是唯一入口）。
pub fn ray_from_screen(camera: Camera3D, screen: Vec2, viewport: Viewport) -> Result<Ray3, String> {
    let (origin, direction) = camera
        .ray_from_screen(screen.x, screen.y, viewport)
        .map_err(|error| error.to_string())?;
    Ray3::new(origin, direction).map_err(|error| error.to_string())
}

/// 世界坐标 → 屏幕像素 +(是否可见)。`visible` 用 NDC z ∈ [0, 1] 与裁剪范围判定：相机背后的点
/// 在 NDC 里也有坐标，不报这一点会让 HUD 标记在背后画出幽灵。
///
/// 冷路径便捷入口：每次调用现烘焙一次相机（[`CameraView`]）。热路径请用
/// [`screen_from_view`]，把烘焙提到循环外。
#[must_use]
pub fn screen_from_world(
    camera: Camera3D,
    world: Vec3,
    viewport: Viewport,
) -> Option<(Vec2, bool)> {
    let view = CameraView::from_camera3d(camera, viewport).ok()?;
    screen_from_view(view, world, viewport)
}

/// [`screen_from_world`] 的**已烘焙**版本：不重建矩阵、不校验相机（每帧一次，循环里调用）。
#[must_use]
pub fn screen_from_view(view: CameraView, world: Vec3, viewport: Viewport) -> Option<(Vec2, bool)> {
    let ndc = view.world_to_ndc(world)?;
    let screen = Vec2::new(
        (ndc.x * 0.5 + 0.5) * viewport.width,
        (0.5 - ndc.y * 0.5) * viewport.height,
    );
    let visible = (0.0..=1.0).contains(&ndc.z) && ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0;
    Some((screen, visible))
}

/// 世界坐标 → 屏幕像素，**只**按"是否在相机前方"（NDC z ∈ [0, 1]）判定可用性。
///
/// 与 [`screen_from_world`] 的区别：这里不要求点落在视口矩形内。调试线段（碰撞体线框、
/// gizmo）需要"端点在屏幕外但仍在相机前方"时照样连线——视口裁剪由渲染器负责，若在这里
/// 因为一个端点出屏就丢掉整条线，物体贴近屏幕边缘时线框会整段消失。
#[must_use]
pub fn screen_from_world_unclipped(
    camera: Camera3D,
    world: Vec3,
    viewport: Viewport,
) -> Option<Vec2> {
    let view = CameraView::from_camera3d(camera, viewport).ok()?;
    screen_from_view_unclipped(view, world, viewport)
}

/// [`screen_from_world_unclipped`] 的**已烘焙**版本（逐段调用的调试路径用它）。
#[must_use]
pub fn screen_from_view_unclipped(
    view: CameraView,
    world: Vec3,
    viewport: Viewport,
) -> Option<Vec2> {
    let ndc = view.world_to_ndc(world)?;
    if !ndc.is_finite() || !(0.0..=1.0).contains(&ndc.z) {
        return None; // 相机背后或远裁剪面之外
    }
    Some(Vec2::new(
        (ndc.x * 0.5 + 0.5) * viewport.width,
        (0.5 - ndc.y * 0.5) * viewport.height,
    ))
}

/// 射线与轴对齐包围盒求交，返回沿射线方向的命中距离（`> max_distance` 视为未命中）。
#[must_use]
pub fn ray_hits_aabb(ray: Ray3, bounds: Aabb3, max_distance: f32) -> Option<f32> {
    let distance = ray.intersect_aabb(bounds)?;
    (distance <= max_distance).then_some(distance)
}

/// 射线与水平面 `y = plane_y` 求交——地面点选/放置最常用的一条，避免每个调用方手写一遍。
#[must_use]
pub fn ray_plane_point(ray: Ray3, plane_y: f32) -> Option<Vec3> {
    if ray.direction.y.abs() < 1e-6 {
        return None; // 与平面平行：无交点（不要返回一个"很远"的假点）
    }
    let t = (plane_y - ray.origin.y) / ray.direction.y;
    if !t.is_finite() {
        return None;
    }
    (t >= 0.0).then(|| ray.origin + ray.direction * t)
}

/// 从候选里挑**最近的**命中（同距取先出现者，因此结果确定）。
///
/// 编辑器点选与脚本查询共用这一条：调用方只提供 (标识, 世界 AABB, 中心/半径)，排序规则不再各写。
pub fn pick_nearest<T: Copy>(
    ray: Ray3,
    candidates: impl IntoIterator<Item = (T, Aabb3)>,
    max_distance: f32,
) -> Option<(T, f32)> {
    let mut best: Option<(T, f32)> = None;
    for (id, bounds) in candidates {
        let Some(distance) = ray_hits_aabb(ray, bounds, max_distance) else {
            continue;
        };
        if best.is_none_or(|(_, current)| distance < current) {
            best = Some((id, distance));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_math::math3d::Mat4;
    use rustlua_math::math3d::Quat;
    use std::f32::consts::FRAC_PI_2;

    fn camera() -> Camera3D {
        Camera3D::perspective(Vec3::new(0.0, 5.0, 20.0), Vec3::ZERO, FRAC_PI_2, 0.1, 100.0).unwrap()
    }

    fn viewport() -> Viewport {
        Viewport::new(800.0, 600.0).unwrap()
    }

    fn unit_aabb(center: Vec3) -> Aabb3 {
        Aabb3 {
            min: center - Vec3::new(1.0, 1.0, 1.0),
            max: center + Vec3::new(1.0, 1.0, 1.0),
        }
    }

    /// 屏幕中心 → 射线指向相机目标；地面交点落在期望位置。
    #[test]
    fn screen_ray_hits_the_ground_where_expected() {
        let camera = camera();
        let viewport = viewport();
        let ray = ray_from_screen(camera, Vec2::new(400.0, 300.0), viewport).expect("ray");
        let hit = ray_plane_point(ray, 0.0).expect("ground hit");
        // 中央射线穿过原点附近的地面。
        assert!(hit.x.abs() < 0.5 && hit.z.abs() < 0.5, "{hit:?}");

        // 与地面平行的射线没有交点（而不是返回一个很远的假点）。
        let flat = Ray3::new(Vec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, 0.0, 0.0)).unwrap();
        assert!(ray_plane_point(flat, 0.0).is_none());
    }

    /// 最近命中：两条候选都在射线上时取更近的那个。
    #[test]
    fn nearest_candidate_wins_and_far_ones_are_rejected() {
        let ray = Ray3::new(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0)).unwrap();
        let near = unit_aabb(Vec3::new(0.0, 0.0, -3.0));
        let far = unit_aabb(Vec3::new(0.0, 0.0, -9.0));
        let picked = pick_nearest(ray, [("far", far), ("near", near)], 10.0).expect("hit");
        assert_eq!(picked.0, "near");
        assert!((picked.1 - 2.0).abs() < 1e-4, "{}", picked.1);
        assert!(
            pick_nearest(ray, [("near", near)], 1.0).is_none(),
            "超出最大距离的命中必须被拒绝"
        );
    }

    /// 世界 → 屏幕：目标投到视口中心且可见；相机背后的点不可见。
    #[test]
    fn world_to_screen_reports_visibility() {
        let camera = camera();
        let viewport = viewport();
        let (screen, visible) = screen_from_world(camera, Vec3::ZERO, viewport).expect("screen");
        assert!((screen.x - 400.0).abs() < 0.5 && (screen.y - 300.0).abs() < 0.5);
        assert!(visible);
        // 相机在 +y/+z 一侧：原点反方向上的点落在相机背后。
        let behind = camera.position + (camera.position - Vec3::ZERO) * 2.0;
        let (_, visible) = screen_from_world(camera, behind, viewport).expect("behind projects");
        assert!(!visible, "相机背后的点必须报 visible=false");

        // 退化相机（位置与目标重合）不产出射线，而不是给一条胡说八道的射线。
        let mut degenerate = camera;
        degenerate.target = degenerate.position;
        assert!(ray_from_screen(degenerate, Vec2::new(400.0, 300.0), viewport).is_err());
    }

    /// 旋转过的相机同样走这条链（不依赖"相机一定朝 -Z"这类假设）。
    #[test]
    fn ray_chain_uses_the_real_camera_orientation() {
        let mut camera = camera();
        // 把"相机 → 目标"的偏移绕 Y 转 40°，中央射线必须仍指向**新的**目标。
        let offset = camera.target - camera.position;
        let rotated = Mat4::from_quat(Quat::from_axis_angle(Vec3::Y, 0.7).unwrap())
            .transform_point(offset)
            .expect("rotate offset");
        camera.target = camera.position + rotated;
        let ray = ray_from_screen(camera, Vec2::new(400.0, 300.0), viewport()).expect("ray");
        let to_target = (camera.target - camera.position).normalized().unwrap();
        let dot = ray.direction.dot(to_target);
        assert!(dot > 0.999, "中央射线必须指向目标（dot={dot}）");
    }
}
