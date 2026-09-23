//! 每帧**烘焙**：把相机组件编译成"本帧热路径唯一消费"的结构。
//!
//! ## 为什么需要这一层
//!
//! [`Camera2D::world_to_screen`] 每次调用都要 `validate()`、重新 `constrained_center()`
//! （内部再算一次 `sin_cos`）并再算一次 `sin_cos`。而它处在引擎最热的路径上——每个精灵
//! 4 次（`Sprite::quad` 的四个角）、剔除再多 1 次、每帧精灵批又要走 3 遍，瓦片与点击目标
//! 同理。于是同一组只依赖"相机 + 视口"的量（中心、sin/cos、zoom、可见域）被反复重算。
//!
//! 这一层把那些量**每帧算一次**：
//!
//! - [`OrthoTerms`]：正交相机的预算项，逐点换算只剩乘加，且**不可失败**（非法输入在烘焙
//!   时已被拒）。它是逐精灵、逐瓦片、逐点击目标路径上唯一该出现的相机类型。
//! - [`CameraView`]：本帧的完整相机视图（视图投影矩阵、投影参数、可见域、正交预算项），
//!   宿主每帧烘焙一次，交给宿主自己的绘制代码、`Scene3d` 与渲染器。
//!
//! 数值与既有的逐点换算**逐项一致**：`OrthoTerms` 与 `Camera2D::world_to_screen` 同源，
//! `CameraView::view_projection` 就是 `Camera2D::view_projection` 的那一份——本模块不引入
//! 第二套投影，只把第一套的执行次数从"每精灵"降到"每帧"。
//!
//! 刻意**不**把逆矩阵常驻在 `CameraView` 里：docs/54 §11.2 已裁定"逆矩阵按需计算"（拾取是
//! 低频路径）。需要逆矩阵的循环应当把 [`CameraView::inverse`] 提到循环**外面**调用一次。

use super::component::{Camera2D, Camera3D, Camera3dError, CameraBounds, Viewport};
use super::error::CoordinateError;
use super::view::{ORTHO_FAR, ORTHO_NEAR, Projection};
use crate::core::{
    geom3d::Frustum,
    math3d::{Mat4, Vec3},
    transform::Vec2,
};

/// 正交相机的**预算项**：中心（已按 bounds 钳制）、旋转的 `sin`/`cos`、`zoom` 与视口。
///
/// 一次烘焙，之后逐点换算不再校验、不再算三角函数。数值与
/// [`Camera2D::world_to_screen`] / [`Camera2D::screen_to_world`] 逐项一致。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrthoTerms {
    center: Vec2,
    sin: f32,
    cos: f32,
    zoom: f32,
    viewport: Viewport,
}

impl OrthoTerms {
    /// 从正交相机烘焙。内部走 `constrained_center`，因此"挡住相机的那块区域"与
    /// 剔除用的可见域仍然是同一份换算。
    pub fn new(camera: Camera2D, viewport: Viewport) -> Result<Self, CoordinateError> {
        camera.validate()?;
        let center = camera.constrained_center(viewport)?;
        let (sin, cos) = camera.rotation.sin_cos();
        Ok(Self {
            center,
            sin,
            cos,
            zoom: camera.zoom,
            viewport,
        })
    }

    #[must_use]
    pub const fn center(self) -> Vec2 {
        self.center
    }

    /// Pixels per world unit.
    #[must_use]
    pub const fn zoom(self) -> f32 {
        self.zoom
    }

    #[must_use]
    pub const fn viewport(self) -> Viewport {
        self.viewport
    }

    /// 世界 → 屏幕像素（左上原点）。**不可失败**：非法相机在烘焙时已被拒。
    #[inline]
    #[must_use]
    pub fn world_to_screen(self, world: Vec2) -> Vec2 {
        let delta = Vec2::new(world.x - self.center.x, world.y - self.center.y);
        let view = Vec2::new(
            delta.x * self.cos + delta.y * self.sin,
            -delta.x * self.sin + delta.y * self.cos,
        );
        Vec2::new(
            view.x * self.zoom + self.viewport.width * 0.5,
            self.viewport.height * 0.5 - view.y * self.zoom,
        )
    }

    /// 屏幕像素 → 世界，[`Self::world_to_screen`] 的逆。
    #[inline]
    #[must_use]
    pub fn screen_to_world(self, screen: Vec2) -> Vec2 {
        let view = Vec2::new(
            (screen.x - self.viewport.width * 0.5) / self.zoom,
            (self.viewport.height * 0.5 - screen.y) / self.zoom,
        );
        Vec2::new(
            self.center.x + view.x * self.cos - view.y * self.sin,
            self.center.y + view.x * self.sin + view.y * self.cos,
        )
    }

    /// 世界**位移** → 屏幕位移（不含中心平移；整层共用一组常量时用它）。
    #[inline]
    #[must_use]
    pub fn world_delta_to_screen(self, delta: Vec2) -> Vec2 {
        let view = Vec2::new(
            delta.x * self.cos + delta.y * self.sin,
            -delta.x * self.sin + delta.y * self.cos,
        );
        Vec2::new(view.x * self.zoom, -view.y * self.zoom)
    }

    /// 可见区域在 X/Y 上的半宽半高（世界单位，含旋转外接 AABB）。
    #[must_use]
    pub fn half_extent(self) -> (f32, f32) {
        let half_width = self.viewport.width * 0.5 / self.zoom;
        let half_height = self.viewport.height * 0.5 / self.zoom;
        (
            half_width * self.cos.abs() + half_height * self.sin.abs(),
            half_width * self.sin.abs() + half_height * self.cos.abs(),
        )
    }
}

/// 本帧相机的**烘焙结果**：视图投影矩阵、投影参数、可见域与（正交的）预算项。
///
/// 宿主每帧烘焙一次，之后自己的绘制代码、`Scene3d` 与渲染器都消费这一份，
/// 因此"同一世界视图"只有一处来源，也不会每帧重算矩阵。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraView {
    projection: Projection,
    viewport: Viewport,
    view_projection: Mat4,
    ortho: Option<OrthoTerms>,
    visible_bounds: Option<CameraBounds>,
}

impl CameraView {
    /// 正交（2D）视图：深度范围取 [`ORTHO_NEAR`]..[`ORTHO_FAR`]（与 `RenderView::overlay_2d` 同一约定）。
    pub fn from_camera2d(camera: Camera2D, viewport: Viewport) -> Result<Self, CoordinateError> {
        Ok(Self {
            projection: Projection::from_camera2d(camera),
            viewport,
            view_projection: camera.view_projection(viewport, ORTHO_NEAR, ORTHO_FAR)?,
            ortho: Some(OrthoTerms::new(camera, viewport)?),
            visible_bounds: Some(camera.visible_world_bounds(viewport)?),
        })
    }

    /// 透视（3D）视图。`aspect_ratio` 由调用方按视口设好（与既有宿主一致）。
    pub fn from_camera3d(camera: Camera3D, viewport: Viewport) -> Result<Self, Camera3dError> {
        Ok(Self {
            projection: Projection::from_camera3d(camera),
            viewport,
            view_projection: camera.view_projection_matrix()?,
            ortho: None,
            visible_bounds: None,
        })
    }

    /// 世界 → 裁剪空间矩阵（渲染器与深度通道消费的那一份）。
    #[must_use]
    pub const fn matrix(self) -> Mat4 {
        self.view_projection
    }

    #[must_use]
    pub const fn projection(self) -> Projection {
        self.projection
    }

    #[must_use]
    pub const fn viewport(self) -> Viewport {
        self.viewport
    }

    /// 正交视图的预算项；透视视图为 `None`。
    #[must_use]
    pub const fn ortho(self) -> Option<OrthoTerms> {
        self.ortho
    }

    /// 视口在**世界坐标**下的可见域（正交视图才有；透视视图返回 `None`）。
    #[must_use]
    pub const fn visible_bounds(self) -> Option<CameraBounds> {
        self.visible_bounds
    }

    /// 视图投影矩阵的逆（**按需**计算；循环里请提到循环外调用一次）。
    #[must_use]
    pub fn inverse(self) -> Option<Mat4> {
        self.view_projection.inverse().ok()
    }

    /// 世界 → NDC。与 [`Camera3D::world_to_ndc`] 同一份矩阵，但不再每次重建矩阵与校验。
    #[must_use]
    pub fn world_to_ndc(self, world: Vec3) -> Option<Vec3> {
        if !world.is_finite() {
            return None;
        }
        self.view_projection.transform_point(world).ok()
    }

    /// 屏幕像素 → 世界射线。与 [`Camera3D::ray_from_screen`] 同一套数学（经由同一份矩阵的逆）。
    #[must_use]
    pub fn ray_from_screen(self, x: f32, y: f32) -> Option<(Vec3, Vec3)> {
        let viewport = self.viewport;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let ndc = Vec3::new(
            x / viewport.width * 2.0 - 1.0,
            1.0 - y / viewport.height * 2.0,
            0.0,
        );
        if !ndc.is_finite() {
            return None;
        }
        let inverse = self.inverse()?;
        let near = inverse.transform_point(ndc).ok()?;
        let far = inverse.transform_point(Vec3::new(ndc.x, ndc.y, 1.0)).ok()?;
        let direction = (far - near).normalized().ok()?;
        Some((near, direction))
    }

    /// 本视图的视锥（不可失败：矩阵已经算好并校验过）。
    #[must_use]
    pub fn frustum(self) -> Frustum {
        Frustum::from_view_projection(self.view_projection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::math3d::Quat;
    use crate::core::transform::Transform3D;

    fn assert_vec2_close(actual: Vec2, expected: Vec2, epsilon: f32) {
        assert!(
            (actual.x - expected.x).abs() <= epsilon && (actual.y - expected.y).abs() <= epsilon,
            "{actual:?} != {expected:?}"
        );
    }

    fn perspective() -> Camera3D {
        Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            1.0,
            0.1,
            100.0,
        )
        .expect("camera")
    }

    /// 预算项必须与逐点换算**逐项一致**：这是"烘焙只提速、不换算法"的证据。
    #[test]
    fn ortho_terms_match_the_pointwise_projection() {
        for (width, height) in [(640.0_f32, 360.0_f32), (1280.0, 720.0), (320.0, 240.0)] {
            let viewport = Viewport::new(width, height).unwrap();
            for rotation in [0.0_f32, 0.35, -1.2, std::f32::consts::FRAC_PI_2] {
                for zoom in [1.0_f32, 2.5, 0.25] {
                    let camera = Camera2D::new(Vec2::new(100.0, 50.0), zoom)
                        .unwrap()
                        .with_rotation(rotation)
                        .unwrap();
                    let terms = OrthoTerms::new(camera, viewport).unwrap();
                    for world in [
                        Vec2::new(100.0, 50.0),
                        Vec2::new(160.0, 50.0),
                        Vec2::new(100.0, 90.0),
                        Vec2::new(40.0, 10.0),
                    ] {
                        assert_vec2_close(
                            terms.world_to_screen(world),
                            camera.world_to_screen(world, viewport).unwrap(),
                            1e-3,
                        );
                        assert_vec2_close(
                            terms.screen_to_world(terms.world_to_screen(world)),
                            world,
                            1e-3,
                        );
                        // 位移换算与逐点换算的差一致（瓦片层整层共用一组常量时用它）。
                        let base = terms.world_to_screen(Vec2::zero());
                        assert_vec2_close(
                            terms.world_delta_to_screen(world),
                            terms.world_to_screen(world) - base,
                            1e-3,
                        );
                    }
                }
            }
        }
    }

    /// 有 bounds 时，预算项的中心必须是**钳制后**的中心（否则绘制与剔除会错位）。
    #[test]
    fn ortho_terms_use_the_constrained_center() {
        let viewport = Viewport::new(200.0, 100.0).unwrap();
        let camera = Camera2D::new(Vec2::new(-100.0, 100.0), 10.0)
            .unwrap()
            .with_bounds(CameraBounds::new(Vec2::new(-5.0, -5.0), Vec2::new(5.0, 5.0)).unwrap())
            .unwrap();
        let terms = OrthoTerms::new(camera, viewport).unwrap();
        assert_eq!(terms.center(), camera.constrained_center(viewport).unwrap());
        // 视口 200×100、zoom 10 ⇒ 半宽 10、半高 5：屏幕左上角对应中心的 (-10, +5)。
        assert_vec2_close(
            terms.screen_to_world(Vec2::zero()),
            Vec2::new(-10.0, 5.0),
            1e-4,
        );
        // 非法相机在**烘焙**时就被拒，而不是在逐点换算里。
        let invalid = Camera2D {
            center: Vec2::zero(),
            rotation: 0.0,
            zoom: 0.0,
            bounds: None,
            enabled: true,
        };
        assert!(OrthoTerms::new(invalid, viewport).is_err());
    }

    /// `CameraView` 的矩阵与可见域必须就是相机自己算的那一份（不引入第二套投影）。
    #[test]
    fn camera_view_reuses_the_camera_projection() {
        let viewport = Viewport::new(640.0, 360.0).unwrap();
        let camera = Camera2D::new(Vec2::new(100.0, 50.0), 2.0).unwrap();
        let view = CameraView::from_camera2d(camera, viewport).unwrap();
        assert_eq!(
            view.matrix(),
            camera
                .view_projection(viewport, ORTHO_NEAR, ORTHO_FAR)
                .unwrap()
        );
        assert_eq!(
            view.visible_bounds(),
            Some(camera.visible_world_bounds(viewport).unwrap())
        );
        assert_eq!(view.ortho().unwrap().zoom(), 2.0);
        assert!(view.inverse().is_some());
    }

    /// 3D 视图的 NDC 与射线必须与 `Camera3D` 的方法给同一结果（同源同约定）。
    #[test]
    fn camera_view_matches_camera3d_ndc_and_ray() {
        let viewport = Viewport::new(640.0, 480.0).unwrap();
        let camera = perspective();
        let view = CameraView::from_camera3d(camera, viewport).unwrap();
        assert!(view.ortho().is_none() && view.visible_bounds().is_none());

        for world in [
            Vec3::ZERO,
            Vec3::new(1.5, -0.75, 0.5),
            Vec3::new(-0.2, 0.4, 4.0),
        ] {
            let expected = camera.world_to_ndc(world).unwrap();
            let actual = view.world_to_ndc(world).unwrap();
            assert!(
                (actual - expected).length() < 1e-4,
                "{actual:?} != {expected:?}"
            );
        }

        let (origin, direction) = view.ray_from_screen(320.0, 240.0).unwrap();
        let (expected_origin, expected_direction) =
            camera.ray_from_screen(320.0, 240.0, 640.0, 480.0).unwrap();
        assert!((origin - expected_origin).length() < 1e-3);
        assert!((direction - expected_direction).length() < 1e-3);

        // 视锥：正前方在锥内、相机背后在锥外（与 `Camera3D::frustum` 同源）。
        let frustum = view.frustum();
        assert!(frustum.contains_sphere(Vec3::ZERO, 0.0));
        assert!(!frustum.contains_sphere(Vec3::new(0.0, 0.0, 6.0), 0.0));
    }

    /// 退化相机在**烘焙**时报错（而不是产出一个坏矩阵让后面每帧都算错）。
    #[test]
    fn degenerate_cameras_fail_at_bake_time() {
        let viewport = Viewport::new(640.0, 360.0).unwrap();
        let invalid_ortho = Camera2D {
            center: Vec2::zero(),
            rotation: 0.0,
            zoom: -1.0,
            bounds: None,
            enabled: true,
        };
        assert!(CameraView::from_camera2d(invalid_ortho, viewport).is_err());

        // 位置与目标重合（退化 look-at）。
        let degenerate = Camera3D {
            position: Vec3::ZERO,
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: std::f32::consts::FRAC_PI_3,
            aspect_ratio: 1.0,
            near: 0.1,
            far: 10.0,
            pose_from_transform: false,
            enabled: true,
        };
        assert!(CameraView::from_camera3d(degenerate, viewport).is_err());

        // 姿态来自 Transform 的相机同样能在烘焙时给出矩阵。
        let posed = Camera3D::default()
            .with_pose(Transform3D {
                position: Vec3::new(0.0, 1.0, 6.0),
                rotation: Quat::from_axis_angle(Vec3::Y, 0.0).unwrap(),
                scale: Vec3::new(1.0, 1.0, 1.0),
            })
            .unwrap();
        let view = CameraView::from_camera3d(posed, viewport).unwrap();
        assert!(view.inverse().is_some());
    }
}
