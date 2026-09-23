//! 相机**类型与数学**：正交（2D）的逐点换算与边界钳制、透视（3D）的矩阵、NDC 往返与射线入口。
//!
//! 这里只有相机本身的数学：不认识组件串（[`super::codec`]）、不认识控制器策略
//! （[`super::controller`]）、不认识每帧视图契约（[`super::view`]）、也不认识 ECS 与宿主。
//! 组件串的解析与编码**刻意不在这里**——那是数据格式，不是相机数学。
//!
//! 数值约定（NDC y 向上、深度范围 0..1、右手系 -Z 前向、世界 y 向上）由 `super::tests` 钉住。

use super::error::CoordinateError;
use crate::core::{
    geom3d::Frustum,
    math3d::{Mat4, Vec3},
    transform::{Transform3D, Vec2},
};

/// 2D 游戏相机默认中心（世界坐标）。相机未显式设置时，游戏视图以该点为屏幕中心，
/// 设计分辨率包围框以它为中心（编辑器 Scene 视图据此定位游戏取景范围）。
pub const DEFAULT_2D_CAMERA_CENTER: Vec2 = Vec2::new(640.0, 360.0);

/// A positive pixel viewport used by [`Camera2D`] conversions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

impl Viewport {
    /// Creates a viewport, rejecting non-finite or non-positive dimensions.
    pub fn new(width: f32, height: f32) -> Result<Self, CoordinateError> {
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Err(CoordinateError::InvalidViewport { width, height });
        }
        Ok(Self { width, height })
    }
}

/// An axis-aligned world rectangle used to constrain a [`Camera2D`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraBounds {
    pub min: Vec2,
    pub max: Vec2,
}

impl CameraBounds {
    pub fn new(min: Vec2, max: Vec2) -> Result<Self, CoordinateError> {
        validate_vec2("camera bounds minimum", min)?;
        validate_vec2("camera bounds maximum", max)?;
        if min.x > max.x || min.y > max.y {
            return Err(CoordinateError::InvalidBounds { min, max });
        }
        Ok(Self { min, max })
    }
}

/// A 2D camera with center, rotation, zoom, and optional world bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2D {
    pub center: Vec2,
    /// Clockwise screen rotation in radians. Positive world rotation appears clockwise on screen.
    pub rotation: f32,
    /// Pixels per world unit.
    pub zoom: f32,
    pub bounds: Option<CameraBounds>,
    /// docs/54 §4.2：相机是否**启用**（组件串里的 `enabled=false`）。默认启用。
    ///
    /// 禁用的相机不参与选择：`camera.set_active` 选中一台禁用相机会回退到下一台可用相机并记
    /// 一条 warning；场景里第一台 `Camera2D` 若被禁用，也不会成为运行时的初始 2D 相机。
    /// 之所以要有这个字段而不是"把组件删掉"：相机常常是**预置体/模板**的一部分，动画或脚本
    /// 需要临时关掉它而不丢配置。
    pub enabled: bool,
}

impl Camera2D {
    /// Creates an unrotated camera, where `zoom` is pixels per world unit.
    pub fn new(center: Vec2, zoom: f32) -> Result<Self, CoordinateError> {
        Self::with_settings(center, 0.0, zoom, None)
    }

    pub fn with_rotation(mut self, rotation: f32) -> Result<Self, CoordinateError> {
        self.rotation = rotation;
        self.validate()?;
        Ok(self)
    }

    pub fn with_bounds(mut self, bounds: CameraBounds) -> Result<Self, CoordinateError> {
        self.bounds = Some(bounds);
        self.validate()?;
        Ok(self)
    }

    pub fn without_bounds(mut self) -> Self {
        self.bounds = None;
        self
    }

    fn with_settings(
        center: Vec2,
        rotation: f32,
        zoom: f32,
        bounds: Option<CameraBounds>,
    ) -> Result<Self, CoordinateError> {
        let camera = Self {
            center,
            rotation,
            zoom,
            bounds,
            enabled: true,
        };
        camera.validate()?;
        Ok(camera)
    }

    /// Converts a world position to top-left-origin screen pixels.
    pub fn world_to_screen(self, world: Vec2, viewport: Viewport) -> Result<Vec2, CoordinateError> {
        self.validate()?;
        validate_vec2("world", world)?;
        validate_viewport(viewport)?;
        let center = self.constrained_center(viewport)?;
        let delta = Vec2::new(world.x - center.x, world.y - center.y);
        let (sin, cos) = self.rotation.sin_cos();
        let view = Vec2::new(
            delta.x * cos + delta.y * sin,
            -delta.x * sin + delta.y * cos,
        );
        Ok(Vec2::new(
            view.x * self.zoom + viewport.width * 0.5,
            viewport.height * 0.5 - view.y * self.zoom,
        ))
    }

    /// Converts a top-left-origin screen pixel position back to world space.
    pub fn screen_to_world(
        self,
        screen: Vec2,
        viewport: Viewport,
    ) -> Result<Vec2, CoordinateError> {
        self.validate()?;
        validate_vec2("screen", screen)?;
        validate_viewport(viewport)?;
        let center = self.constrained_center(viewport)?;
        let view = Vec2::new(
            (screen.x - viewport.width * 0.5) / self.zoom,
            (viewport.height * 0.5 - screen.y) / self.zoom,
        );
        let (sin, cos) = self.rotation.sin_cos();
        Ok(Vec2::new(
            center.x + view.x * cos - view.y * sin,
            center.y + view.x * sin + view.y * cos,
        ))
    }

    /// 视口在**世界坐标**下的轴对齐包围盒（已计入缩放与旋转）。
    ///
    /// 旋转时取的是外接 AABB（比真实可见区域大，**宁可多留不可少算**——剔除用它当判定域）。
    /// 相机边界钳制（[`Camera2D::constrained_center`]）与视口外剔除共用这一份换算，避免
    /// "挡住相机的那块区域"与"剔除时认为可见的区域"两套算法各说各话。
    pub fn visible_world_bounds(self, viewport: Viewport) -> Result<CameraBounds, CoordinateError> {
        self.validate()?;
        validate_viewport(viewport)?;
        let center = self.constrained_center(viewport)?;
        let (extent_x, extent_y) = self.visible_extent(viewport);
        CameraBounds::new(
            Vec2::new(center.x - extent_x, center.y - extent_y),
            Vec2::new(center.x + extent_x, center.y + extent_y),
        )
    }

    /// 可见区域在 X/Y 上的半宽半高（世界单位，含旋转外接）。
    fn visible_extent(self, viewport: Viewport) -> (f32, f32) {
        let half_width = viewport.width * 0.5 / self.zoom;
        let half_height = viewport.height * 0.5 / self.zoom;
        let (sin, cos) = self.rotation.sin_cos();
        (
            half_width * cos.abs() + half_height * sin.abs(),
            half_width * sin.abs() + half_height * cos.abs(),
        )
    }

    /// 2D 相机的**视图投影矩阵**（WP6）：把世界点 `(x, y, z)` 直接变到裁剪空间。
    ///
    /// 这是"把 2D 内容放进带深度的渲染通道"所缺的那一块：顶点着色器需要一份矩阵，而不是
    /// CPU 侧逐个算屏幕坐标。矩阵与 [`Camera2D::world_to_screen`] **同源同约定**——NDC 的
    /// x/y 就是屏幕坐标线性映射到 `[-1, 1]`（y 向上），z 直接取世界 z 在 `near..far` 上的
    /// 线性映射，因此深度比较在 2D 内部自洽，也让 2D 相机与 3D 相机可以用同一种方式描述
    /// （docs/54：正交投影是 2D 的默认相机配置，不是另一套相机系统）。
    ///
    /// `near`/`far` 是裁剪空间的深度范围（`near` 对应 NDC z = 0，`far` 对应 1，与 3D 一致）。
    pub fn view_projection(
        self,
        viewport: Viewport,
        near: f32,
        far: f32,
    ) -> Result<Mat4, CoordinateError> {
        self.validate()?;
        validate_viewport(viewport)?;
        if !near.is_finite() || !far.is_finite() || far <= near {
            return Err(CoordinateError::InvalidDepthRange { near, far });
        }
        let center = self.constrained_center(viewport)?;
        let (sin, cos) = self.rotation.sin_cos();
        // 世界 → 视图：先平移掉相机中心，再按**屏幕旋转**的逆旋转（与 world_to_screen 的
        // `view.x = dx*cos + dy*sin, view.y = -dx*sin + dy*cos` 逐项一致）。
        // 视图 → NDC：x 乘 zoom / (宽/2)，y 取负（屏幕 y 向下、NDC y 向上）。
        let sx = self.zoom * 2.0 / viewport.width;
        // 注意 y 的系数是**正的**：`world_to_screen` 里 `screen.y = h/2 - view.y*zoom`，
        // 折到 NDC (`1 - screen/h*2`) 后两次取负相抵，因此 NDC y 随世界 y 单调递增。
        let sy = self.zoom * 2.0 / viewport.height;
        let depth = 1.0 / (far - near);
        Ok(Mat4 {
            columns: [
                [sx * cos, sy * -sin, 0.0, 0.0],
                [sx * sin, sy * cos, 0.0, 0.0],
                [0.0, 0.0, depth, 0.0],
                [
                    -sx * (center.x * cos + center.y * sin),
                    -sy * (-center.x * sin + center.y * cos),
                    -near * depth,
                    1.0,
                ],
            ],
        })
    }

    /// bounds 钳制后的相机中心：保证**整个视口**仍落在 `bounds` 内（无 `bounds` 时原样返回）。
    ///
    /// 收敛前宿主里也有一份同名同算法的副本（瓦片相机与相机控制器各用各的），所以这里把它
    /// 提升为公开 API——夹取规则只留这一份，剔除用的可见域也走 [`Camera2D::visible_extent`]。
    pub fn constrained_center(self, viewport: Viewport) -> Result<Vec2, CoordinateError> {
        let Some(bounds) = self.bounds else {
            return Ok(self.center);
        };
        let (extent_x, extent_y) = self.visible_extent(viewport);
        Ok(Vec2::new(
            clamp_center(
                self.center.x,
                bounds.min.x + extent_x,
                bounds.max.x - extent_x,
            ),
            clamp_center(
                self.center.y,
                bounds.min.y + extent_y,
                bounds.max.y - extent_y,
            ),
        ))
    }

    pub(super) fn validate(self) -> Result<(), CoordinateError> {
        validate_vec2("camera center", self.center)?;
        if !self.rotation.is_finite() {
            return Err(CoordinateError::InvalidRotation(self.rotation));
        }
        if !self.zoom.is_finite() || self.zoom <= 0.0 {
            return Err(CoordinateError::InvalidZoom(self.zoom));
        }
        if let Some(bounds) = self.bounds {
            CameraBounds::new(bounds.min, bounds.max)?;
        }
        Ok(())
    }
}

fn clamp_center(value: f32, minimum: f32, maximum: f32) -> f32 {
    if minimum > maximum {
        (minimum + maximum) * 0.5
    } else {
        value.clamp(minimum, maximum)
    }
}

fn validate_viewport(viewport: Viewport) -> Result<(), CoordinateError> {
    Viewport::new(viewport.width, viewport.height).map(|_| ())
}

fn validate_vec2(name: &'static str, value: Vec2) -> Result<(), CoordinateError> {
    if value.x.is_finite() && value.y.is_finite() {
        Ok(())
    } else {
        Err(CoordinateError::NonFiniteVector { name, value })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera3D {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y: f32,
    pub aspect_ratio: f32,
    pub near: f32,
    pub far: f32,
    /// WP5：姿态是否由**实体的 `Transform3D`** 提供（组件串里的 `pose=transform`）。
    ///
    /// 默认 `false` = 用本组件的 `position`/`target`（look-at），既有场景的规范字符串一字不变。
    /// 打开后本组件的 position/target/up 变成"没有 Transform3D 时的回退值"，实体若无
    /// `Transform3D` 则报诊断（见 `scene_extract`）——姿态来源只有一处，不再两边各说各话。
    pub pose_from_transform: bool,
    /// docs/54 §4.2：相机是否**启用**（组件串里的 `enabled=false`）。默认启用；禁用的相机不参与
    /// 选择（`camera.set_active` 选中禁用相机会回退到下一台可用相机并记 warning）。
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Camera3dError {
    InvalidPerspective,
    InvalidLookAt,
    NonFinite,
}
impl std::fmt::Display for Camera3dError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "3D camera error: {self:?}")
    }
}
impl std::error::Error for Camera3dError {}

impl Default for Camera3D {
    /// 新建相机的默认参数：位置 `(0,0,5)` 看向原点、FOV 90°、near 0.1、far 100。
    ///
    /// 编辑器「添加组件 ▸ Camera3D」直接用这个类型的编码，编辑器里不再手写这份默认值
    /// （与迁移后的其它组件同一条规则：默认值只有引擎一处定义）。
    /// `aspect_ratio` 只是占位——宿主每帧用窗口比例覆盖它。
    fn default() -> Self {
        Self {
            position: Vec3::new(0.0, 0.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: std::f32::consts::FRAC_PI_2,
            aspect_ratio: 1.0,
            near: 0.1,
            far: 100.0,
            pose_from_transform: false,
            enabled: true,
        }
    }
}

impl Camera3D {
    pub fn perspective(
        position: Vec3,
        target: Vec3,
        fov_y: f32,
        aspect_ratio: f32,
        near: f32,
        far: f32,
    ) -> Result<Self, Camera3dError> {
        let camera = Self {
            position,
            target,
            up: Vec3::Y,
            fov_y,
            aspect_ratio,
            near,
            far,
            pose_from_transform: false,
            enabled: true,
        };
        camera.validate()?;
        Ok(camera)
    }
    /// 用实体的 `Transform3D` 作为相机姿态（WP5）：位置取变换位置，视线取变换的 **-Z**，
    /// 上方向取变换的 +Y。fov/near/far/aspect 仍来自本组件。
    ///
    /// 只改姿态来源，不改投影参数——因此"相机也是一台普通实体"这件事不需要第二套相机类型。
    /// 姿态无法表达时（非有限、退化旋转）返回错误，绝不静默退回旧姿态。
    pub fn with_pose(self, pose: Transform3D) -> Result<Self, Camera3dError> {
        if !pose.position.is_finite() || !pose.rotation.is_unit() {
            return Err(Camera3dError::NonFinite);
        }
        let forward = pose
            .rotation
            .rotate(Vec3::new(0.0, 0.0, -1.0))
            .map_err(|_| Camera3dError::InvalidLookAt)?;
        let up = pose
            .rotation
            .rotate(Vec3::Y)
            .map_err(|_| Camera3dError::InvalidLookAt)?;
        let camera = Self {
            position: pose.position,
            target: pose.position + forward,
            up,
            pose_from_transform: true,
            ..self
        };
        camera.validate()?;
        Ok(camera)
    }

    pub fn validate(self) -> Result<(), Camera3dError> {
        if !self.position.is_finite() || !self.target.is_finite() || !self.up.is_finite() {
            return Err(Camera3dError::NonFinite);
        }
        self.validate_projection()?;
        // 组件自带的 `aspect_ratio` 也必须合法：**没有视口**的调用方（宿主未声明视图时的回退、
        // 光照/阴影相机）仍在用它。一经成帧（`CameraView` / `RenderView`），宽高比的权威就是
        // 视口，这个字段不再参与（见 [`Camera3D::projection_matrix_for`]）。
        if !self.aspect_ratio.is_finite() || self.aspect_ratio <= 0.0 {
            return Err(Camera3dError::InvalidPerspective);
        }
        if (self.target - self.position).length_squared() <= f32::EPSILON
            || self.up.cross(self.target - self.position).length_squared() <= f32::EPSILON
        {
            return Err(Camera3dError::InvalidLookAt);
        }
        Ok(())
    }

    /// 投影参数的校验（不含姿态，也不含宽高比——后者是成帧时的输入）。
    fn validate_projection(self) -> Result<(), Camera3dError> {
        if !self.fov_y.is_finite()
            || self.fov_y <= 0.0
            || self.fov_y >= std::f32::consts::PI
            || !self.near.is_finite()
            || !self.far.is_finite()
            || self.near <= 0.0
            || self.far <= self.near
        {
            return Err(Camera3dError::InvalidPerspective);
        }
        Ok(())
    }
    pub fn view_matrix(self) -> Result<Mat4, Camera3dError> {
        self.validate()?;
        let forward = (self.target - self.position)
            .normalized()
            .map_err(|_| Camera3dError::InvalidLookAt)?;
        let right = forward
            .cross(self.up)
            .normalized()
            .map_err(|_| Camera3dError::InvalidLookAt)?;
        let up = right.cross(forward);
        Ok(Mat4 {
            columns: [
                [right.x, up.x, -forward.x, 0.0],
                [right.y, up.y, -forward.y, 0.0],
                [right.z, up.z, -forward.z, 0.0],
                [
                    -right.dot(self.position),
                    -up.dot(self.position),
                    forward.dot(self.position),
                    1.0,
                ],
            ],
        })
    }
    /// 透视投影矩阵，**宽高比由调用方给出**。
    ///
    /// 成帧路径（[`crate::core::camera::CameraView::from_camera3d`]）传的是**视口**比例。这样
    /// "组件里存的比例"与"实际渲染用的比例"不会再各说各话：此前宿主每帧把窗口比例写回
    /// `aspect_ratio`，读者却分不清"这个值现在是权威还是上一帧的残留"。
    pub fn projection_matrix_for(self, aspect_ratio: f32) -> Result<Mat4, Camera3dError> {
        self.validate()?;
        if !aspect_ratio.is_finite() || aspect_ratio <= 0.0 {
            return Err(Camera3dError::InvalidPerspective);
        }
        let f = 1.0 / (self.fov_y * 0.5).tan();
        let range = self.far - self.near;
        Ok(Mat4 {
            columns: [
                [f / aspect_ratio, 0.0, 0.0, 0.0],
                [0.0, f, 0.0, 0.0],
                [0.0, 0.0, -self.far / range, -1.0],
                [0.0, 0.0, -self.near * self.far / range, 0.0],
            ],
        })
    }

    /// 用**组件自带** `aspect_ratio` 的版本。只有**没有视口**的调用方该用它：宿主未声明
    /// `FrameViews` 时的历史回退、光照/阴影相机、以及只做姿态变换的路径。
    pub fn projection_matrix(self) -> Result<Mat4, Camera3dError> {
        self.projection_matrix_for(self.aspect_ratio)
    }

    pub fn view_projection_matrix(self) -> Result<Mat4, Camera3dError> {
        self.view_projection_matrix_for(self.aspect_ratio)
    }

    /// 与 [`Camera3D::view_projection_matrix`] 相同，但宽高比由调用方给出（成帧路径传视口比例）。
    pub fn view_projection_matrix_for(self, aspect_ratio: f32) -> Result<Mat4, Camera3dError> {
        Ok(self.projection_matrix_for(aspect_ratio)? * self.view_matrix()?)
    }
    pub fn world_to_ndc(self, world: Vec3) -> Result<Vec3, Camera3dError> {
        if !world.is_finite() {
            return Err(Camera3dError::NonFinite);
        }
        self.view_projection_matrix()?
            .transform_point(world)
            .map_err(|_| Camera3dError::InvalidLookAt)
    }
    pub fn ndc_to_world(self, ndc: Vec3) -> Result<Vec3, Camera3dError> {
        if !ndc.is_finite() {
            return Err(Camera3dError::NonFinite);
        }
        self.view_projection_matrix()?
            .inverse()
            .map_err(|_| Camera3dError::InvalidLookAt)?
            .transform_point(ndc)
            .map_err(|_| Camera3dError::InvalidLookAt)
    }

    /// Converts a viewport pixel coordinate into a world-space ray.
    pub fn ray_from_screen(
        self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Result<(Vec3, Vec3), Camera3dError> {
        if width <= 0.0 || height <= 0.0 || !x.is_finite() || !y.is_finite() {
            return Err(Camera3dError::NonFinite);
        }
        let ndc = Vec3::new(x / width * 2.0 - 1.0, 1.0 - y / height * 2.0, 0.0);
        let near = self.ndc_to_world(ndc)?;
        let far = self.ndc_to_world(Vec3::new(ndc.x, ndc.y, 1.0))?;
        let direction = (far - near)
            .normalized()
            .map_err(|_| Camera3dError::InvalidLookAt)?;
        if direction.length_squared() <= f32::EPSILON {
            return Err(Camera3dError::InvalidLookAt);
        }
        Ok((near, direction))
    }

    pub fn frustum(self) -> Result<Frustum, Camera3dError> {
        Ok(Frustum::from_view_projection(
            self.view_projection_matrix()?,
        ))
    }
}

/// 统一后的相机组件名：场景里只有这一个名字，2D/3D 由投影判别字段表达。
pub const CAMERA_COMPONENT: &str = "Camera";

/// 统一 `Camera` 组件里的投影判别字段名。
pub const PROJECTION_FIELD: &str = "projection";

/// 统一组件承载的两种投影语义（判别字段的取值）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraProjection {
    Orthographic,
    Perspective,
}

impl CameraProjection {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Orthographic => "orthographic",
            Self::Perspective => "perspective",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "orthographic" => Some(Self::Orthographic),
            "perspective" => Some(Self::Perspective),
            _ => None,
        }
    }
}

/// 统一 `Camera` 组件的**取值**：一个显式的投影分支。
///
/// 消费方（编辑器 / 运行时 / 场景编译）拿到它之后**按投影分支**处理，而不是按组件名——
/// 这正是"不再区分 2D/3D 组件"要表达的语义。两种语义的字段体仍由**各自既有的编码器**
/// 读写（正交 = `center`/`zoom`/`bounds`，透视 = `position`/`target`/`fov`/`near`/`far`），
/// 因此统一组件不做任何会改变画面的映射：
///
/// ```text
/// projection=orthographic;center=..,..;rotation=..;zoom=..[;bounds=..][;enabled=false]
/// projection=perspective;position=..,..,..;target=..,..,..;fov=..;near=..;far=..[;pose=transform][;enabled=false]
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Camera {
    Orthographic(Camera2D),
    Perspective(Camera3D),
}

impl Camera {
    /// 本组件承载的投影语义（组件的**判别**，不是字段体）。
    #[must_use]
    pub const fn projection(self) -> CameraProjection {
        match self {
            Self::Orthographic(_) => CameraProjection::Orthographic,
            Self::Perspective(_) => CameraProjection::Perspective,
        }
    }

    /// docs/54 §4.2：相机是否**启用**（组件串里的 `enabled=false`）。默认启用；禁用的相机
    /// 不参与选择（`camera.set_active` 选中禁用相机会回退并记 warning）。
    #[must_use]
    pub const fn enabled(self) -> bool {
        match self {
            Self::Orthographic(camera) => camera.enabled,
            Self::Perspective(camera) => camera.enabled,
        }
    }

    /// 正交 payload（2D 视图、初始 2D 相机、编辑器 2D 场景视图用）。
    #[must_use]
    pub fn as_orthographic(self) -> Option<Camera2D> {
        match self {
            Self::Orthographic(camera) => Some(camera),
            Self::Perspective(_) => None,
        }
    }

    /// 透视 payload（3D pass、射线/拾取、gizmo 用）。
    #[must_use]
    pub fn as_perspective(self) -> Option<Camera3D> {
        match self {
            Self::Perspective(camera) => Some(camera),
            Self::Orthographic(_) => None,
        }
    }

    /// 透视 payload 的**可变**借用（Lua `camera.set` 与运行时写入用）。
    ///
    /// 与 [`Camera::as_perspective`] 同一分支判断：正交相机拿不到透视 payload，因此
    /// "只有透视相机能被 `camera.set` 改写"这条既有语义由**类型**保证，而不是靠调用点各自
    /// 记得先判断投影。
    #[must_use]
    pub fn as_perspective_mut(&mut self) -> Option<&mut Camera3D> {
        match self {
            Self::Perspective(camera) => Some(camera),
            Self::Orthographic(_) => None,
        }
    }
}
