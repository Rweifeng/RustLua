//! 统一相机（WP5）：2D 与 3D 的投影、视口、拾取与可见性都在这里。
//!
//! 收敛前它们分散在两个模块里（`math2d::Camera2D` 与 `camera3d::Camera3D`），各自带一套
//! 视口/投影/误差类型，改一处投影要同时想两遍"另一边是不是也这样"。现在：
//!
//! - 两个家族的类型都在本模块：`Camera2D` / `Camera3D` / `Viewport` / `CameraBounds` /
//!   `Ray3` / `Frustum` / `Plane`，以及共用的 `CoordinateError`；
//! - 上面再叠一层**统一的投影描述** [`Projection`] 与渲染视图 [`RenderView`]，让
//!   "这台相机怎么投影、投到多大视口、清屏策略与绘制顺序"只有一个说法（相机可以跨 2D 与 3D
//!   复用，但**不强制**两者共用 shader/深度规则）；
//! - 数值约定（NDC 方向、深度范围 0..1、右手系 -Z 前向、世界 y 向上）由测试逐条钉住。
//!
//! 迁移过程中两侧的**行为一字未改**：既有 3D semantic golden、2D overlay golden 与全部相机
//! 测试原样通过——这正是"旧两套 / 新一套"对拍要的证据。

use std::{error::Error, fmt};

use crate::core::{
    entity::EntityId,
    math3d::{Aabb3, Mat4, Math3dError, Vec3},
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

/// `Camera2D` 组件串的解析错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Camera2dComponentError {
    /// 空串不是一个相机。
    Empty,
    /// 缺 `center` / `zoom` 这类必填字段。
    MissingField(&'static str),
    /// 字段不是 `key=value` 或数值/形状不对。
    MalformedField(String),
    /// 未知键：报错而不是忽略。
    UnknownField(String),
    /// 通过文法但语义非法（zoom <= 0、非有限、bounds 反转等）。
    Invalid(String),
}

impl fmt::Display for Camera2dComponentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "camera2d component is empty"),
            Self::MissingField(field) => write!(formatter, "missing camera2d field: {field}"),
            Self::MalformedField(field) => write!(formatter, "malformed camera2d field: {field}"),
            Self::UnknownField(field) => write!(formatter, "unknown camera2d field: {field}"),
            Self::Invalid(message) => write!(formatter, "invalid camera2d: {message}"),
        }
    }
}

impl std::error::Error for Camera2dComponentError {}

fn parse_component_number(raw: &str, field: &str) -> Result<f32, Camera2dComponentError> {
    raw.parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| Camera2dComponentError::MalformedField(field.to_owned()))
}

fn parse_component_pair(raw: &str, field: &str) -> Result<(f32, f32), Camera2dComponentError> {
    let (x, y) = raw
        .split_once(',')
        .ok_or_else(|| Camera2dComponentError::MalformedField(field.to_owned()))?;
    Ok((
        parse_component_number(x.trim(), field)?,
        parse_component_number(y.trim(), field)?,
    ))
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

    /// 解析场景里的 `Camera2D` 组件串（docs/54 §8-①：正交相机也必须是**实体组件**，
    /// 而不是只有宿主状态，否则"同一 Entity 模型表达正交/透视相机"不成立）。
    ///
    /// 文法：`center=x,y;zoom=z` 必填，`rotation=r`（弧度）与 `bounds=min_x,min_y,max_x,max_y`
    /// 可选。未知键报错，非法数值走与构造函数同一套校验。
    pub fn from_component_str(value: &str) -> Result<Self, Camera2dComponentError> {
        let mut center: Option<Vec2> = None;
        let mut zoom: Option<f32> = None;
        let mut rotation = 0.0_f32;
        let mut bounds: Option<CameraBounds> = None;
        let mut enabled = true;
        let mut seen = false;
        for field in value
            .trim()
            .split(';')
            .filter(|field| !field.trim().is_empty())
        {
            seen = true;
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| Camera2dComponentError::MalformedField(field.to_owned()))?;
            let raw = raw.trim();
            match key.trim() {
                "center" => {
                    let (x, y) = parse_component_pair(raw, field)?;
                    center = Some(Vec2::new(x, y));
                }
                "zoom" => zoom = Some(parse_component_number(raw, field)?),
                "rotation" => rotation = parse_component_number(raw, field)?,
                // docs/54 §4.2：`enabled=false` 让这台相机退出选择（预置体/模板里"留着但关掉"）。
                "enabled" => {
                    enabled = match raw {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(Camera2dComponentError::Invalid(format!(
                                "enabled must be true/false: {field}"
                            )));
                        }
                    };
                }
                "bounds" => {
                    let parts = raw
                        .split(',')
                        .map(str::trim)
                        .map(|part| parse_component_number(part, field))
                        .collect::<Result<Vec<_>, _>>()?;
                    if parts.len() != 4 {
                        return Err(Camera2dComponentError::MalformedField(field.to_owned()));
                    }
                    bounds = Some(
                        CameraBounds::new(
                            Vec2::new(parts[0], parts[1]),
                            Vec2::new(parts[2], parts[3]),
                        )
                        .map_err(|error| Camera2dComponentError::Invalid(error.to_string()))?,
                    );
                }
                _ => return Err(Camera2dComponentError::UnknownField(field.to_owned())),
            }
        }
        if !seen {
            return Err(Camera2dComponentError::Empty);
        }
        let (Some(center), Some(zoom)) = (center, zoom) else {
            return Err(Camera2dComponentError::MissingField(if center.is_none() {
                "center"
            } else {
                "zoom"
            }));
        };
        let camera = Camera2D {
            center,
            rotation,
            zoom,
            bounds,
            enabled,
        };
        camera
            .validate()
            .map_err(|error| Camera2dComponentError::Invalid(error.to_string()))?;
        Ok(camera)
    }

    /// 写回组件串：只写非默认的可选字段，因此往返稳定且既有场景串不变长。
    #[must_use]
    pub fn to_component_str(self) -> String {
        let mut out = format!("center={:.4},{:.4}", self.center.x, self.center.y);
        if self.rotation != 0.0 {
            out.push_str(&format!(";rotation={:.6}", self.rotation));
        }
        out.push_str(&format!(";zoom={:.6}", self.zoom));
        if let Some(bounds) = self.bounds {
            out.push_str(&format!(
                ";bounds={:.4},{:.4},{:.4},{:.4}",
                bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y
            ));
        }
        if !self.enabled {
            out.push_str(";enabled=false");
        }
        out
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

    fn validate(self) -> Result<(), CoordinateError> {
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

/// Errors produced by coordinate and camera conversions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoordinateError {
    InvalidViewport {
        width: f32,
        height: f32,
    },
    InvalidZoom(f32),
    NonFiniteVector {
        name: &'static str,
        value: Vec2,
    },
    NonFiniteColor {
        value: [f32; 4],
    },
    InvalidRotation(f32),
    InvalidBounds {
        min: Vec2,
        max: Vec2,
    },
    /// 深度范围非法（`far <= near` 或含非有限值）。
    InvalidDepthRange {
        near: f32,
        far: f32,
    },
}

impl fmt::Display for CoordinateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidViewport { width, height } => {
                write!(formatter, "invalid viewport: {width}x{height}")
            }
            Self::InvalidZoom(zoom) => write!(formatter, "invalid camera zoom: {zoom}"),
            Self::NonFiniteVector { name, value } => write!(
                formatter,
                "{name} vector is not finite: ({}, {})",
                value.x, value.y
            ),
            Self::NonFiniteColor { value } => write!(formatter, "color is not finite: {value:?}"),
            Self::InvalidRotation(rotation) => {
                write!(formatter, "invalid camera rotation: {rotation}")
            }
            Self::InvalidBounds { min, max } => {
                write!(formatter, "invalid camera bounds: {min:?}..{max:?}")
            }
            Self::InvalidDepthRange { near, far } => {
                write!(formatter, "invalid depth range: near={near} far={far}")
            }
        }
    }
}
impl Error for CoordinateError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Camera3dComponentError {
    /// A field was not `key=value` as the scene format requires.
    MalformedField(String),
    /// A required field (`position`/`target`/`fov`/`near`/`far`) was missing.
    MissingField(&'static str),
    /// A numeric or vector field could not be parsed.
    InvalidValue(String),
    /// The perspective parameters were invalid (fov, near, far).
    InvalidPerspective,
    /// The camera looked at its own position or had a degenerate up vector.
    InvalidLookAt,
}
impl fmt::Display for Camera3dComponentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedField(field) => write!(f, "malformed Camera3D field: {field}"),
            Self::MissingField(field) => write!(f, "missing Camera3D field: {field}"),
            Self::InvalidValue(value) => write!(f, "invalid Camera3D value: {value}"),
            Self::InvalidPerspective => write!(f, "invalid Camera3D perspective parameters"),
            Self::InvalidLookAt => write!(f, "invalid Camera3D look-at parameters"),
        }
    }
}
impl Error for Camera3dComponentError {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray3 {
    pub origin: Vec3,
    pub direction: Vec3,
}

impl Ray3 {
    pub fn new(origin: Vec3, direction: Vec3) -> Result<Self, Math3dError> {
        Ok(Self {
            origin,
            direction: direction.normalized()?,
        })
    }

    pub fn point_at(self, distance: f32) -> Vec3 {
        self.origin + self.direction * distance
    }

    /// Intersects the ray with a plane defined by a point and unit normal.
    pub fn intersect_plane(self, point: Vec3, normal: Vec3) -> Option<f32> {
        if !point.is_finite() || !normal.is_finite() {
            return None;
        }
        let denominator = self.direction.dot(normal);
        if denominator.abs() <= f32::EPSILON {
            return None;
        }
        let distance = (point - self.origin).dot(normal) / denominator;
        distance
            .is_finite()
            .then_some(distance)
            .filter(|value| *value >= 0.0)
    }

    /// Returns the closest ray distance and line parameter for an infinite axis.
    pub fn closest_point_on_line(self, point: Vec3, axis: Vec3) -> Option<(f32, f32)> {
        let axis = axis.normalized().ok()?;
        let cross = self.direction.cross(axis);
        let denominator = cross.length_squared();
        if denominator <= f32::EPSILON {
            return None;
        }
        let offset = point - self.origin;
        let ray_distance = offset.cross(axis).dot(cross) / denominator;
        let line_parameter = offset.cross(self.direction).dot(cross) / denominator;
        (ray_distance.is_finite() && line_parameter.is_finite())
            .then_some((ray_distance, line_parameter))
    }
    pub fn intersect_aabb(self, bounds: Aabb3) -> Option<f32> {
        let mut near: f32 = 0.0;
        let mut far = f32::INFINITY;
        for (origin, direction, min, max) in [
            (self.origin.x, self.direction.x, bounds.min.x, bounds.max.x),
            (self.origin.y, self.direction.y, bounds.min.y, bounds.max.y),
            (self.origin.z, self.direction.z, bounds.min.z, bounds.max.z),
        ] {
            if direction.abs() <= f32::EPSILON {
                if origin < min || origin > max {
                    return None;
                }
                continue;
            }
            let inv = 1.0 / direction;
            let mut axis_near = (min - origin) * inv;
            let mut axis_far = (max - origin) * inv;
            if axis_near > axis_far {
                std::mem::swap(&mut axis_near, &mut axis_far);
            }
            near = near.max(axis_near);
            far = far.min(axis_far);
            if near > far {
                return None;
            }
        }
        if far >= 0.0 {
            Some(near.max(0.0))
        } else {
            None
        }
    }

    pub fn intersect_sphere(self, center: Vec3, radius: f32) -> Option<f32> {
        if radius < 0.0 || !radius.is_finite() {
            return None;
        }
        let offset = center - self.origin;
        let projection = offset.dot(self.direction);
        let discriminant = projection * projection - offset.length_squared() + radius * radius;
        if discriminant < 0.0 {
            return None;
        }
        let root = discriminant.sqrt();
        let near = projection - root;
        let far = projection + root;
        if near >= 0.0 {
            Some(near)
        } else if far >= 0.0 {
            Some(far)
        } else {
            None
        }
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

    /// Parses the scene component string written by the editor:
    /// `position=x,y,z;target=x,y,z;fov=<degrees>;near=<n>;far=<f>`
    /// （可选 `;pose=transform`，见 [`Camera3D::pose_from_transform`]）。
    ///
    /// The `up` vector defaults to +Y and `aspect_ratio` to 1.0; the host
    /// renderer replaces `aspect_ratio` with the window aspect each frame.
    pub fn from_component_str(value: &str) -> Result<Self, Camera3dComponentError> {
        let mut camera = Self {
            position: Vec3::ZERO,
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: 0.0,
            aspect_ratio: 1.0,
            near: 0.0,
            far: 0.0,
            pose_from_transform: false,
            enabled: true,
        };
        let mut seen = [false; 5];
        for field in value.split(';') {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| Camera3dComponentError::MalformedField(field.to_owned()))?;
            match key {
                "position" => {
                    camera.position = parse_camera_vec3(raw)?;
                    seen[0] = true;
                }
                "target" => {
                    camera.target = parse_camera_vec3(raw)?;
                    seen[1] = true;
                }
                "fov" => {
                    camera.fov_y = parse_camera_number(raw)?.to_radians();
                    seen[2] = true;
                }
                "near" => {
                    camera.near = parse_camera_number(raw)?;
                    seen[3] = true;
                }
                "far" => {
                    camera.far = parse_camera_number(raw)?;
                    seen[4] = true;
                }
                // WP5：姿态来源开关。只接受两个字面量，拼错报错而不是当默认值——静默当默认
                // 会让"相机没跟着变换走"变成一个查不出原因的现象。
                "pose" => match raw {
                    "transform" => camera.pose_from_transform = true,
                    "look_at" => camera.pose_from_transform = false,
                    _ => return Err(Camera3dComponentError::InvalidValue(field.to_owned())),
                },
                // docs/54 §4.2：`enabled=false` 让这台相机退出选择（与 Camera2D 同义）。
                "enabled" => match raw {
                    "true" => camera.enabled = true,
                    "false" => camera.enabled = false,
                    _ => return Err(Camera3dComponentError::InvalidValue(field.to_owned())),
                },
                _ => return Err(Camera3dComponentError::MalformedField(field.to_owned())),
            }
        }
        for (field, present) in [
            ("position", seen[0]),
            ("target", seen[1]),
            ("fov", seen[2]),
            ("near", seen[3]),
            ("far", seen[4]),
        ] {
            if !present {
                return Err(Camera3dComponentError::MissingField(field));
            }
        }
        camera.validate().map_err(|error| match error {
            Camera3dError::InvalidPerspective => Camera3dComponentError::InvalidPerspective,
            Camera3dError::InvalidLookAt => Camera3dComponentError::InvalidLookAt,
            Camera3dError::NonFinite => Camera3dComponentError::InvalidValue(value.to_owned()),
        })?;
        Ok(camera)
    }

    /// Encodes the component back to the editor's string format (fov in degrees).
    ///
    /// `pose=transform` **只在开启时**写出，因此既有场景的规范字符串一字不变。
    #[must_use]
    pub fn to_component_str(self) -> String {
        let pose = if self.pose_from_transform {
            ";pose=transform"
        } else {
            ""
        };
        let enabled = if self.enabled { "" } else { ";enabled=false" };
        format!(
            "position={:.9},{:.9},{:.9};target={:.9},{:.9},{:.9};fov={:.4};near={:.9};far={:.9}{pose}{enabled}",
            self.position.x,
            self.position.y,
            self.position.z,
            self.target.x,
            self.target.y,
            self.target.z,
            self.fov_y.to_degrees(),
            self.near,
            self.far
        )
    }

    pub fn validate(self) -> Result<(), Camera3dError> {
        if !self.position.is_finite() || !self.target.is_finite() || !self.up.is_finite() {
            return Err(Camera3dError::NonFinite);
        }
        if !self.fov_y.is_finite()
            || self.fov_y <= 0.0
            || self.fov_y >= std::f32::consts::PI
            || !self.aspect_ratio.is_finite()
            || self.aspect_ratio <= 0.0
            || !self.near.is_finite()
            || !self.far.is_finite()
            || self.near <= 0.0
            || self.far <= self.near
        {
            return Err(Camera3dError::InvalidPerspective);
        }
        if (self.target - self.position).length_squared() <= f32::EPSILON
            || self.up.cross(self.target - self.position).length_squared() <= f32::EPSILON
        {
            return Err(Camera3dError::InvalidLookAt);
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
    pub fn projection_matrix(self) -> Result<Mat4, Camera3dError> {
        self.validate()?;
        let f = 1.0 / (self.fov_y * 0.5).tan();
        let range = self.far - self.near;
        Ok(Mat4 {
            columns: [
                [f / self.aspect_ratio, 0.0, 0.0, 0.0],
                [0.0, f, 0.0, 0.0],
                [0.0, 0.0, -self.far / range, -1.0],
                [0.0, 0.0, -self.near * self.far / range, 0.0],
            ],
        })
    }
    pub fn view_projection_matrix(self) -> Result<Mat4, Camera3dError> {
        Ok(self.projection_matrix()? * self.view_matrix()?)
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
        Ok(Frustum {
            view_projection: self.view_projection_matrix()?,
        })
    }
}

/// Parses an `x,y,z` camera vector, rejecting non-finite components.
fn parse_camera_vec3(raw: &str) -> Result<Vec3, Camera3dComponentError> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 3 {
        return Err(Camera3dComponentError::InvalidValue(raw.to_owned()));
    }
    let mut values = [0.0_f32; 3];
    for (slot, part) in values.iter_mut().zip(&parts) {
        let parsed: f32 = part
            .trim()
            .parse()
            .map_err(|_| Camera3dComponentError::InvalidValue(raw.to_owned()))?;
        if !parsed.is_finite() {
            return Err(Camera3dComponentError::InvalidValue(raw.to_owned()));
        }
        *slot = parsed;
    }
    Ok(Vec3::new(values[0], values[1], values[2]))
}

/// Parses a finite scalar camera parameter.
fn parse_camera_number(raw: &str) -> Result<f32, Camera3dComponentError> {
    let parsed: f32 = raw
        .trim()
        .parse()
        .map_err(|_| Camera3dComponentError::InvalidValue(raw.to_owned()))?;
    if !parsed.is_finite() {
        return Err(Camera3dComponentError::InvalidValue(raw.to_owned()));
    }
    Ok(parsed)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    view_projection: Mat4,
}

/// A world-space frustum plane `normal · p + distance = 0`; points with a
/// positive signed distance lie on the inside (camera-facing) side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub distance: f32,
}

impl Plane {
    /// Signed distance of `point` from the plane (positive = inside).
    pub fn signed_distance(self, point: Vec3) -> f32 {
        self.normal.dot(point) + self.distance
    }
}

impl Frustum {
    pub fn contains_point(self, point: Vec3) -> bool {
        self.classify_point(point) == FrustumClassification::Inside
    }
    pub fn classify_point(self, point: Vec3) -> FrustumClassification {
        let Ok(ndc) = self.view_projection.transform_point(point) else {
            return FrustumClassification::Outside;
        };
        if ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0 && (0.0..=1.0).contains(&ndc.z) {
            FrustumClassification::Inside
        } else {
            FrustumClassification::Outside
        }
    }

    /// Extracts the six world-space clip planes (Gribb–Hartmann) from the
    /// view-projection matrix. Normals are normalized, so sphere offsets are
    /// exact.
    pub fn planes(self) -> [Plane; 6] {
        let m = &self.view_projection.columns;
        let rows = [
            [m[0][0], m[1][0], m[2][0], m[3][0]],
            [m[0][1], m[1][1], m[2][1], m[3][1]],
            [m[0][2], m[1][2], m[2][2], m[3][2]],
            [m[0][3], m[1][3], m[2][3], m[3][3]],
        ];
        // Clip-space z is [0, 1]; combine rows to build the six planes so that
        // `inside` means a positive signed distance:
        //   left = clip.x + clip.w >= 0  ->  row0 + row3
        //   right = clip.w - clip.x >= 0 -> -row0 + row3
        //   bottom = clip.y + clip.w >= 0  ->  row1 + row3
        //   top = clip.w - clip.y >= 0  ->  -row1 + row3
        //   near = clip.z >= 0  ->  row2
        //   far = clip.w - clip.z >= 0  ->  -row2 + row3
        let combos: [(usize, f32, f32); 6] = [
            (0, 1.0, 1.0),
            (1, 1.0, 1.0),
            (2, 1.0, 0.0),
            (0, -1.0, 1.0),
            (1, -1.0, 1.0),
            (2, -1.0, 1.0),
        ];
        let mut result = [Plane {
            normal: Vec3::ZERO,
            distance: 0.0,
        }; 6];
        for (index, (row_index, row_sign, row3_sign)) in combos.into_iter().enumerate() {
            let row = rows[row_index];
            let mut normal = Vec3::new(
                row_sign * row[0] + row3_sign * rows[3][0],
                row_sign * row[1] + row3_sign * rows[3][1],
                row_sign * row[2] + row3_sign * rows[3][2],
            );
            let distance = row_sign * row[3] + row3_sign * rows[3][3];
            let length = normal.length();
            if length > f32::EPSILON {
                normal = normal / length;
            }
            result[index] = Plane {
                normal,
                distance: distance / length.max(f32::EPSILON),
            };
        }
        result
    }

    /// Returns whether a world-space bounding sphere intersects the frustum.
    /// Conservative: touching the boundary counts as visible, so culling never
    /// removes geometry that could be on screen.
    pub fn contains_sphere(self, center: Vec3, radius: f32) -> bool {
        if !radius.is_finite() || radius < 0.0 {
            return true;
        }
        for plane in self.planes() {
            if plane.signed_distance(center) < -radius {
                return false;
            }
        }
        true
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrustumClassification {
    Inside,
    Outside,
}

/// 单帧最大步长：卡帧/断点恢复后不让相机瞬移一大段（也让平滑与抖动的表现可预期）。
const MAX_STEP_SECONDS: f32 = 0.1;
/// 抖动的两个频率取互质近似值，避免 x/y 同相位导致抖动看起来像单轴摆动。
const SHAKE_FREQ_X: f32 = 37.0;
const SHAKE_FREQ_Y: f32 = 53.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2dController {
    /// 跟随目标实体。`None` = 不跟随（相机中心由脚本或宿主直接设置）。
    follow: Option<EntityId>,
    /// 目标相对屏幕中心的偏移（像素）。
    offset: Vec2,
    /// 死区半宽/半高（像素）。目标在死区内时相机不动。
    dead_zone: Vec2,
    /// 平滑系数（每秒收敛率）。`0` = 立即跟随。
    smoothing: f32,
    shake_amplitude: f32,
    shake_remaining: f32,
    shake_duration: f32,
    elapsed: f32,
    /// 脚本请求的相机中心（`camera2d.set`）。宿主每帧消费一次。
    ///
    /// 相机数据在宿主（`RuntimeScene::camera`）而不在 Lua 状态里，因此这里用
    /// "脚本请求 → 宿主消费"的有界交接，与 `camera.set_active` 的既有做法一致。
    pending_center: Option<Vec2>,
    /// 脚本请求的缩放。
    pending_zoom: Option<f32>,
    /// 宿主每帧回填的真实值，供 `camera2d.get` / `get_zoom` 读到真数而非脚本的自述。
    reported_center: Vec2,
    reported_zoom: f32,
}

impl Default for Camera2dController {
    fn default() -> Self {
        Self {
            follow: None,
            offset: Vec2::zero(),
            dead_zone: Vec2::zero(),
            smoothing: 0.0,
            shake_amplitude: 0.0,
            shake_remaining: 0.0,
            shake_duration: 0.0,
            elapsed: 0.0,
            pending_center: None,
            pending_zoom: None,
            reported_center: Vec2::zero(),
            reported_zoom: 1.0,
        }
    }
}

impl Camera2dController {
    #[must_use]
    pub const fn follow_target(&self) -> Option<EntityId> {
        self.follow
    }

    #[must_use]
    pub const fn smoothing(&self) -> f32 {
        self.smoothing
    }

    #[must_use]
    pub const fn offset(&self) -> Vec2 {
        self.offset
    }

    #[must_use]
    pub const fn dead_zone(&self) -> Vec2 {
        self.dead_zone
    }

    /// 开始跟随实体，或传 `None` 取消跟随。
    pub fn follow(&mut self, target: Option<EntityId>) {
        self.follow = target;
    }

    pub fn set_offset(&mut self, offset: Vec2) {
        self.offset = offset;
    }

    pub fn set_dead_zone(&mut self, dead_zone: Vec2) {
        self.dead_zone = dead_zone;
    }

    /// 设置平滑系数（每秒收敛率）；非有限或负数按 0（立即跟随）处理。
    pub fn set_smoothing(&mut self, smoothing: f32) {
        self.smoothing = if smoothing.is_finite() && smoothing > 0.0 {
            smoothing
        } else {
            0.0
        };
    }

    /// 触发一次抖动：`amplitude` 像素、`duration` 秒，强度随剩余时间线性衰减。
    ///
    /// 同一 `(amplitude, duration)` 与同一帧序列下结果**确定**（相位由累计时间决定，
    /// 不用随机数），因此不会破坏规则回归的确定性。
    pub fn shake(&mut self, amplitude: f32, duration: f32) {
        if !amplitude.is_finite() || !duration.is_finite() || amplitude == 0.0 || duration <= 0.0 {
            return;
        }
        self.shake_amplitude = amplitude.abs();
        self.shake_duration = duration;
        self.shake_remaining = duration;
    }

    #[must_use]
    pub fn is_shaking(&self) -> bool {
        self.shake_remaining > 0.0
    }

    /// 脚本请求把相机中心设为 `center`（下一帧被宿主消费）。有限性由调用方校验。
    pub fn request_center(&mut self, center: Vec2) {
        self.pending_center = Some(center);
    }

    /// 宿主取走脚本请求的中心（取一次即清，避免重复应用）。
    pub fn take_pending_center(&mut self) -> Option<Vec2> {
        self.pending_center.take()
    }

    /// 脚本请求缩放。
    pub fn request_zoom(&mut self, zoom: f32) {
        self.pending_zoom = Some(zoom);
    }

    pub fn take_pending_zoom(&mut self) -> Option<f32> {
        self.pending_zoom.take()
    }

    /// 宿主回填真实相机状态，供 `camera2d.get` / `get_zoom` 读取。
    pub fn report(&mut self, center: Vec2, zoom: f32) {
        self.reported_center = center;
        self.reported_zoom = zoom;
    }

    #[must_use]
    pub const fn center(&self) -> Vec2 {
        self.reported_center
    }

    #[must_use]
    pub const fn zoom(&self) -> f32 {
        self.reported_zoom
    }

    /// 推进一帧，返回本帧相机中心。
    ///
    /// `desired` 是跟随目标的世界坐标（由调用方从 ECS 取；不跟随时传 `None`）。
    pub fn advance(&mut self, current: Vec2, desired: Option<Vec2>, delta_seconds: f32) -> Vec2 {
        let dt = if delta_seconds.is_finite() {
            delta_seconds.clamp(0.0, MAX_STEP_SECONDS)
        } else {
            0.0
        };
        self.elapsed += dt;

        let mut center = match desired {
            Some(desired) if self.follow.is_some() => {
                let desired = desired + self.offset;
                let target = Vec2::new(
                    advance_axis(current.x, desired.x, self.dead_zone.x),
                    advance_axis(current.y, desired.y, self.dead_zone.y),
                );
                if self.smoothing > 0.0 {
                    // 指数收敛：与帧率无关（同样的总时长给同样的结果）。
                    let blend = 1.0 - (-self.smoothing * dt).exp();
                    current + (target - current) * blend
                } else {
                    target
                }
            }
            _ => current,
        };

        if self.shake_remaining > 0.0 {
            self.shake_remaining = (self.shake_remaining - dt).max(0.0);
            let fade = if self.shake_duration > 0.0 {
                self.shake_remaining / self.shake_duration
            } else {
                0.0
            };
            let amplitude = self.shake_amplitude * fade;
            center.x += (self.elapsed * SHAKE_FREQ_X).sin() * amplitude;
            center.y += (self.elapsed * SHAKE_FREQ_Y + 1.7).sin() * amplitude;
            if self.shake_remaining <= 0.0 {
                self.shake_amplitude = 0.0;
            }
        }
        center
    }
}

/// 死区内的单轴推进：超出死区时把相机推到"目标刚好贴住死区边界"的位置。
fn advance_axis(current: f32, desired: f32, dead_zone: f32) -> f32 {
    let delta = desired - current;
    let dead_zone = dead_zone.max(0.0);
    if delta.abs() <= dead_zone {
        return current;
    }
    current + (delta.abs() - dead_zone) * delta.signum()
}

impl Camera2dController {
    /// 推进一帧并返回**本帧应当渲染用的相机**（WP5 收敛后的新边界）。
    ///
    /// 收敛前这段顺序散在宿主里：先取脚本请求的中心/缩放、再取跟随目标、再 advance、最后
    /// 用 `constrained_camera_center` 夹到 bounds。现在它只有一个入口，并且**先应用请求、
    /// 再跟随、最后夹取**的顺序被固定在类型内部——宿主不再需要知道这个次序。
    ///
    /// `follow_target` 是跟随目标的世界坐标（调用方从 ECS 取；不跟随或没有 `Transform`
    /// 时传 `None`）。`current` 的 rotation 与 bounds 原样保留：控制器只决定 center 与 zoom。
    pub fn resolve_camera(
        &mut self,
        current: Camera2D,
        follow_target: Option<Vec2>,
        viewport: Viewport,
        delta_seconds: f32,
    ) -> Camera2D {
        let mut camera = current;
        // 脚本的显式请求优先（`camera2d.set` 语义是"立刻到这儿"），取走即清。
        if let Some(center) = self.take_pending_center() {
            camera.center = center;
        }
        if let Some(zoom) = self.take_pending_zoom() {
            camera.zoom = zoom;
        }
        let advanced = self.advance(camera.center, follow_target, delta_seconds);
        camera.center = advanced;
        // bounds 钳制复用相机自己的那一份换算（与剔除用的可见域同源）。
        camera.center = camera.constrained_center(viewport).unwrap_or(advanced);
        camera
    }

    /// 推进一帧并直接给出本帧的 [`RenderView`]（2D overlay 视图）。
    ///
    /// 渲染器消费 `RenderView` 而不回查 ECS/宿主，这是 WP5 要求的形态；宿主侧接线可以分步
    /// 进行，但"相机 + 视口 + 清屏 + 顺序"的说法从这一处产生。
    pub fn resolve_view(
        &mut self,
        current: Camera2D,
        follow_target: Option<Vec2>,
        viewport: Viewport,
        delta_seconds: f32,
    ) -> RenderView {
        let camera = self.resolve_camera(current, follow_target, viewport, delta_seconds);
        RenderView::overlay_2d(camera, viewport)
    }
}

/// 统一投影描述（WP5）：正交（2D 世界）与透视（3D 世界）的唯一说法。
///
/// 相机可以跨 2D 与 3D 复用，但**不强制**两者共用 shader 与深度规则——本类型只回答
/// "怎么投影"，不回答"画出来长什么样"（那是渲染器与 [`RenderView`] 的事）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// 正交：`zoom` 是"每世界单位多少像素"，与 [`Camera2D`] 一致。
    Orthographic { zoom: f32 },
    /// 透视：`fov_y` 是**垂直**视场角（弧度），与 [`Camera3D`] 一致。
    Perspective { fov_y: f32, near: f32, far: f32 },
}

impl Projection {
    /// 从 2D 相机的 zoom 构造。
    #[must_use]
    pub const fn from_camera2d(camera: Camera2D) -> Self {
        Self::Orthographic { zoom: camera.zoom }
    }

    /// 从 3D 相机构造。
    #[must_use]
    pub const fn from_camera3d(camera: Camera3D) -> Self {
        Self::Perspective {
            fov_y: camera.fov_y,
            near: camera.near,
            far: camera.far,
        }
    }

    /// 该投影是否需要深度（透视要，2D 正交不参与世界深度）。
    #[must_use]
    pub const fn uses_depth(self) -> bool {
        matches!(self, Self::Perspective { .. })
    }

    /// 是否符合"2D 世界"的规则（世界 y 向上、不参与深度、像素对齐由 zoom 决定）。
    #[must_use]
    pub const fn is_orthographic(self) -> bool {
        matches!(self, Self::Orthographic { .. })
    }
}

/// 清屏策略：渲染视图自己声明要不要清、清成什么颜色（此前分散在渲染器与编辑器里）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClearPolicy {
    /// 不清屏（叠加到已有内容之上，例如 2D overlay 覆在 3D pass 之后）。
    None,
    Color([f32; 4]),
}

/// 3D 主视图的默认清屏色（线性空间）。渲染器此前把它写死在 pass 里；现在它既是渲染器
/// 未收到任何视图声明时的历史默认，也是宿主 `RenderView::main_3d` 的默认值来源。
pub const DEFAULT_3D_CLEAR: [f32; 4] = [0.04, 0.07, 0.12, 1.0];

/// 相机选择状态：显式选中的实体，没选就退回到"排序序第一个"。
///
/// 收敛前这段逻辑分别写在渲染器与宿主里（Lua 的 `camera.set_active` 与"首个组件"回退），
/// 多相机场景下"到底哪台生效"没有单一说法。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CameraSelection {
    pub active: Option<u64>,
}

impl CameraSelection {
    #[must_use]
    pub const fn new() -> Self {
        Self { active: None }
    }

    pub fn select(&mut self, entity: Option<u64>) {
        self.active = entity;
    }

    /// 这台实体是否是**显式选中**的那台。
    ///
    /// 宿主在"排序序遍历候选"时用它判断命中（遍历过程中"第一个"自然是回退项），
    /// 与 [`CameraSelection::resolve`] 是同一条规则的两种用法：都是"显式选中优先、
    /// 否则第一个"。
    #[must_use]
    pub const fn is_selected(self, entity: u64) -> bool {
        matches!(self.active, Some(active) if active == entity)
    }

    /// 从"排序序"的候选里挑出真正生效的那一个：优先显式选中，否则第一个。
    ///
    /// 候选顺序由调用方给出（引擎里是 ECS 的排序实体序），因此结果**确定**。
    #[must_use]
    pub fn resolve<T: Copy>(&self, candidates: impl IntoIterator<Item = (u64, T)>) -> Option<T> {
        self.resolve_enabled(candidates, |_| true)
    }

    /// [`CameraSelection::resolve`] 的**只挑启用项**版本（docs/54 §4.2 的 `enabled=false`）。
    ///
    /// `enabled` 为假的候选直接跳过：既不参与"第一个"回退，也不响应显式选中。
    /// 显式选中了一台被禁用的相机时返回下一个可用相机（或 `None`）——调用方据此记一条 warning：
    /// 静默用一台被关掉的相机（或静默什么都不画），会让"我明明关了它"变成一个查不出原因的现象。
    #[must_use]
    pub fn resolve_enabled<T: Copy>(
        &self,
        candidates: impl IntoIterator<Item = (u64, T)>,
        enabled: impl Fn(&T) -> bool,
    ) -> Option<T> {
        let mut fallback = None;
        for (entity, value) in candidates {
            if !enabled(&value) {
                continue;
            }
            if Some(entity) == self.active {
                return Some(value);
            }
            if fallback.is_none() {
                fallback = Some(value);
            }
        }
        fallback
    }
}

/// 渲染视图（WP5）：一帧渲染所需的相机侧描述——视口、投影、清屏策略、绘制顺序与可见遮罩。
///
/// 目标与 docs/40 §3 WP5 一致：**渲染器消费快照，而不回查 ECS**。2D 与 3D 各一份视图，
/// 共用同一套字段，因此"2D 用另一套投影/视口说法"这类分叉从类型上就不存在了。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderView {
    pub projection: Projection,
    pub viewport: Viewport,
    pub clear: ClearPolicy,
    /// 同帧内多个视图的绘制顺序（小的先画）。
    pub order: i32,
    /// 可见层遮罩（按位与场景层比较；`u32::MAX` = 全可见）。
    pub visibility: u32,
    /// 本视图的**视图投影矩阵**（世界 → 裁剪空间）。
    ///
    /// docs/54 §8-③：渲染器消费的必须是这一份契约，而不是自己回查 `Camera3D` 或宿主另塞一份
    /// 矩阵。此前 `RenderView` 只有投影**参数**（fov/near/far）没有视图矩阵，3D pass 因此仍从
    /// `Scene3d.camera` 取矩阵、世界 Sprite 又从宿主单独拿一份 2D 矩阵——"同一世界视图"没有
    /// 单一来源，Sprite 与 Mesh 也谈不上共享相机。
    ///
    /// `None` = 这台相机当前退化（位置与目标重合、zoom 非正等），无法给出矩阵；渲染器此时
    /// 退回历史路径并留下诊断，不猜一个矩阵。
    pub view_projection: Option<Mat4>,
}

/// 正交视图（2D 主视图 / overlay）的默认裁剪深度范围。
///
/// `z = 0` 因此落在深度**中间**，向 -z 靠近相机、向 +z 远离，两侧各 1 个世界单位可用
/// （2D 的层间偏移通常远小于此）。世界 Sprite 的 `Sprite::z` 也按这个范围解释。
pub const ORTHO_NEAR: f32 = -1.0;
/// 见 [`ORTHO_NEAR`]。
pub const ORTHO_FAR: f32 = 1.0;

impl RenderView {
    /// 2D overlay 视图：正交投影 + 不清屏（叠在 3D pass 之上）。
    #[must_use]
    pub fn overlay_2d(camera: Camera2D, viewport: Viewport) -> Self {
        Self {
            projection: Projection::from_camera2d(camera),
            viewport,
            clear: ClearPolicy::None,
            order: 1,
            visibility: u32::MAX,
            view_projection: camera.view_projection(viewport, ORTHO_NEAR, ORTHO_FAR).ok(),
        }
    }

    /// 主 3D 视图：透视投影 + 用给定颜色清屏。
    #[must_use]
    pub fn main_3d(camera: Camera3D, viewport: Viewport, clear: [f32; 4]) -> Self {
        Self {
            projection: Projection::from_camera3d(camera),
            viewport,
            clear: ClearPolicy::Color(clear),
            order: 0,
            visibility: u32::MAX,
            view_projection: camera.view_projection_matrix().ok(),
        }
    }

    /// 主 2D 视图：正交投影 + 用给定颜色清屏（编辑器 Scene 2D 视图、纯 2D 游戏画面）。
    ///
    /// "主/叠加"说的是**清屏策略**而不是 2D/3D：2D 画面也可以是这一帧的底，需要先清屏。
    #[must_use]
    pub fn main_2d(camera: Camera2D, viewport: Viewport, clear: [f32; 4]) -> Self {
        Self {
            projection: Projection::from_camera2d(camera),
            viewport,
            clear: ClearPolicy::Color(clear),
            order: 0,
            visibility: u32::MAX,
            view_projection: camera.view_projection(viewport, ORTHO_NEAR, ORTHO_FAR).ok(),
        }
    }

    /// 本视图的视图投影矩阵（世界 → 裁剪空间）。退化相机返回 `None`。
    #[must_use]
    pub const fn matrix(self) -> Option<Mat4> {
        self.view_projection
    }

    /// 视口宽高比（透视投影需要）。
    #[must_use]
    pub fn aspect(self) -> f32 {
        self.viewport.width / self.viewport.height
    }
}

/// 一帧交给渲染器的视图集合（WP5 的接线边界）。
///
/// 引擎一帧最多两类视图：3D 主视图（清屏 + 深度）与 2D overlay（叠在 3D 之上、不清屏）。
/// `main` 为 `None` 表示宿主**没有声明**主视图：渲染器保持历史默认（默认清屏色、整目标
/// 视口），因此"不调用 `set_frame_views`"就是这次收敛的回滚档——像素与收敛前逐字节相同。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameViews {
    /// 3D 主视图：决定清屏与深度 pass 的视口。
    pub main: Option<RenderView>,
    /// 2D overlay 视图：决定 2D pass 的视口（可以是目标的一个子矩形）。
    pub overlay: Option<RenderView>,
}

impl FrameViews {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            main: None,
            overlay: None,
        }
    }

    /// 主 3D 视图 + 2D overlay 的标准组合（`order` 小的先画）。
    #[must_use]
    pub const fn layered(main: RenderView, overlay: RenderView) -> Self {
        Self {
            main: Some(main),
            overlay: Some(overlay),
        }
    }

    /// 只有 2D overlay 的纯 2D 场景。
    #[must_use]
    pub const fn overlay_only(overlay: RenderView) -> Self {
        Self {
            main: None,
            overlay: Some(overlay),
        }
    }

    /// 本帧的清屏策略：**顺序最前**的那个已声明视图说了算（`order` 小的先；相等时 `main` 优先）。
    ///
    /// docs/54 §4.2：`RenderView::order` 此前是个**没有读者**的字段（清屏硬编码取 `main`）——与 §4.2
    /// 要求的"视图包含 render order"不符。现在顺序规则只有一处（[`Self::ordered`]），清屏由它推出：
    /// 声明一个 `order = -1` 的 2D 背景视图并给它 `ClearPolicy::Color`，这一帧就由它清屏。
    /// 既有宿主不受影响（`main.order = 0` 恒为最小，除非宿主显式声明更小的顺序）。
    #[must_use]
    pub fn clear(self) -> Option<ClearPolicy> {
        self.ordered()
            .into_iter()
            .flatten()
            .next()
            .map(|view| view.clear)
    }

    /// 按 `order` 升序排列的已声明视图（未声明的排在最后并被跳过）。
    #[must_use]
    pub fn ordered(self) -> [Option<RenderView>; 2] {
        let mut views = [self.main, self.overlay];
        views.sort_by_key(|view| view.map_or(i32::MAX, |view| view.order));
        views
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        math2d::{WORLD_RIGHT, WORLD_UP},
        math3d::Quat,
    };
    fn camera() -> Camera3D {
        Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            1.0,
            0.1,
            100.0,
        )
        .unwrap()
    }
    #[test]
    fn camera_center_round_trips_through_ndc() {
        let camera = camera();
        let point = Vec3::new(0.0, 0.0, 0.0);
        let ndc = camera.world_to_ndc(point).unwrap();
        assert!((ndc.x.abs() + ndc.y.abs() + ndc.z - 0.981).abs() < 0.01);
        let recovered = camera.ndc_to_world(ndc).unwrap();
        assert!((recovered - point).length() < 1e-4);
    }
    #[test]
    fn screen_center_ray_points_toward_camera_target() {
        let camera = camera();
        let (origin, direction) = camera.ray_from_screen(320.0, 240.0, 640.0, 480.0).unwrap();
        assert!(origin.z < camera.position.z);
        assert!(direction.z < -0.9);
        assert!(direction.y.abs() < 0.01);
    }
    #[test]
    fn ray_intersects_aabb_and_rejects_parallel_miss() {
        let ray = Ray3::new(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z).unwrap();
        let bounds = Aabb3::new(Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0)).unwrap();
        assert_eq!(ray.intersect_aabb(bounds), Some(4.0));
        let miss = Ray3::new(Vec3::new(2.0, 0.0, 5.0), -Vec3::Z).unwrap();
        assert_eq!(miss.intersect_aabb(bounds), None);
    }
    #[test]
    fn ray_hits_nearest_sphere_point() {
        let ray = Ray3::new(Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        assert_eq!(ray.intersect_sphere(Vec3::ZERO, 1.0), Some(4.0));
        assert_eq!(ray.intersect_sphere(Vec3::new(5.0, 0.0, 0.0), 0.5), None);
    }
    #[test]
    fn frustum_classifies_front_and_rear_points() {
        let camera = camera();
        let frustum = camera.frustum().unwrap();
        assert!(frustum.contains_point(Vec3::ZERO));
        assert!(!frustum.contains_point(Vec3::new(0.0, 0.0, 6.0)));
    }

    #[test]
    fn frustum_sphere_culling_respects_radius() {
        let camera = camera();
        let frustum = camera.frustum().unwrap();
        // Behind the camera (near plane) is culled; a large radius that reaches
        // back in front is kept.
        assert!(!frustum.contains_sphere(Vec3::new(0.0, 0.0, 6.0), 0.0));
        assert!(frustum.contains_sphere(Vec3::new(0.0, 0.0, 6.0), 1.5));
        // Far outside a side plane is culled unless the radius reaches in.
        assert!(!frustum.contains_sphere(Vec3::new(6.0, 0.0, 0.0), 0.1));
        assert!(frustum.contains_sphere(Vec3::new(6.0, 0.0, 0.0), 3.0));
        // Inside and boundary points are always visible.
        assert!(frustum.contains_sphere(Vec3::ZERO, 0.0));
        assert!(frustum.contains_sphere(Vec3::new(0.0, 0.0, 4.9), 0.0));
        // Non-finite radii degrade to visible (never cull).
        assert!(frustum.contains_sphere(Vec3::new(50.0, 50.0, 50.0), f32::NAN));
    }

    #[test]
    fn frustum_planes_agree_with_point_classification() {
        let camera = camera();
        let frustum = camera.frustum().unwrap();
        let inside = Vec3::ZERO;
        let outside = Vec3::new(0.0, 0.0, 6.0);
        // The origin is inside every plane (positive signed distance).
        for plane in frustum.planes() {
            assert!(
                plane.signed_distance(inside) >= 0.0,
                "origin inside all planes"
            );
        }
        // A point far behind the camera violates at least one plane.
        assert!(
            frustum
                .planes()
                .iter()
                .any(|plane| plane.signed_distance(outside) < 0.0)
        );
    }
    #[test]
    fn invalid_perspective_is_rejected() {
        assert!(Camera3D::perspective(Vec3::ZERO, Vec3::Z, 0.0, 1.0, 0.1, 10.0).is_err());
        assert!(Camera3D::perspective(Vec3::ZERO, Vec3::ZERO, 1.0, 1.0, 0.1, 10.0).is_err());
    }
    #[test]
    fn parses_editor_component_string() {
        let camera =
            Camera3D::from_component_str("position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100")
                .unwrap();
        assert_eq!(camera.position, Vec3::new(0.0, 0.0, 5.0));
        assert_eq!(camera.target, Vec3::ZERO);
        assert!((camera.fov_y - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(camera.aspect_ratio, 1.0);
        assert_eq!(camera.near, 0.1);
        assert_eq!(camera.far, 100.0);
        assert_eq!(
            Camera3D::from_component_str(&camera.to_component_str()).unwrap(),
            camera
        );
    }
    #[test]
    fn rejects_incomplete_or_invalid_component_strings() {
        assert!(matches!(
            Camera3D::from_component_str("position=0,0,5;target=0,0,0;near=0.1;far=100"),
            Err(Camera3dComponentError::MissingField("fov"))
        ));
        assert!(matches!(
            Camera3D::from_component_str("position=0,0;target=0,0,0;fov=90;near=0.1;far=100"),
            Err(Camera3dComponentError::InvalidValue(_))
        ));
        // fov out of (0, 180) degrees
        assert!(
            Camera3D::from_component_str("position=0,0,5;target=0,0,0;fov=0;near=0.1;far=100",)
                .is_err()
        );
        // far must exceed near
        assert!(
            Camera3D::from_component_str("position=0,0,5;target=0,0,0;fov=90;near=10;far=1",)
                .is_err()
        );
        // look-at degenerate
        assert!(
            Camera3D::from_component_str("position=0,0,5;target=0,0,5;fov=90;near=0.1;far=100",)
                .is_err()
        );
    }

    /// docs/54 §8-①：正交（2D）相机也是场景组件，因此组件串必须能往返、且拼错要报错。
    #[test]
    fn camera2d_component_string_round_trips_and_rejects_bad_input() {
        let camera =
            Camera2D::from_component_str("center=100,50;zoom=2").expect("minimal camera2d");
        assert_eq!(camera.center, Vec2::new(100.0, 50.0));
        assert_eq!(camera.zoom, 2.0);
        assert_eq!(camera.rotation, 0.0);
        assert!(camera.bounds.is_none());

        let full = Camera2D::from_component_str(
            "center=640,360;rotation=0.5;zoom=1.5;bounds=0,0,1280,720",
        )
        .expect("full camera2d");
        assert_eq!(full.rotation, 0.5);
        assert_eq!(full.bounds.expect("bounds").min, Vec2::new(0.0, 0.0));
        // 往返稳定：解析 → 写回 → 再解析得到同一台相机。
        assert_eq!(
            Camera2D::from_component_str(&full.to_component_str()).expect("reparse"),
            full
        );
        assert_eq!(
            Camera2D::from_component_str(&camera.to_component_str()).expect("reparse minimal"),
            camera
        );

        assert_eq!(
            Camera2D::from_component_str(""),
            Err(Camera2dComponentError::Empty)
        );
        assert_eq!(
            Camera2D::from_component_str("center=0,0"),
            Err(Camera2dComponentError::MissingField("zoom"))
        );
        assert_eq!(
            Camera2D::from_component_str("zoom=1"),
            Err(Camera2dComponentError::MissingField("center"))
        );
        assert!(matches!(
            Camera2D::from_component_str("center=0,0;zoom=1;fov=60"),
            Err(Camera2dComponentError::UnknownField(_))
        ));
        assert!(matches!(
            Camera2D::from_component_str("center=0,0;zoom=0"),
            Err(Camera2dComponentError::Invalid(_))
        ));
        assert!(matches!(
            Camera2D::from_component_str("center=0,0;zoom=1;bounds=10,10,0,0"),
            Err(Camera2dComponentError::Invalid(_))
        ));
    }
    use crate::core::transform::Vec2;
    fn assert_vec_close(actual: Vec2, expected: Vec2, epsilon: f32) {
        assert!(
            (actual.x - expected.x).abs() <= epsilon,
            "{actual:?} != {expected:?}"
        );
        assert!(
            (actual.y - expected.y).abs() <= epsilon,
            "{actual:?} != {expected:?}"
        );
    }
    #[test]
    fn coordinate_axes_and_center_match_convention() {
        let viewport = Viewport::new(800.0, 600.0).unwrap();
        let camera = Camera2D::new(Vec2::zero(), 10.0).unwrap();
        assert_eq!(
            camera.world_to_screen(Vec2::zero(), viewport).unwrap(),
            Vec2::new(400.0, 300.0)
        );
        assert_eq!(
            camera.world_to_screen(WORLD_RIGHT, viewport).unwrap(),
            Vec2::new(410.0, 300.0)
        );
        assert_eq!(
            camera.world_to_screen(WORLD_UP, viewport).unwrap(),
            Vec2::new(400.0, 290.0)
        );
    }
    #[test]
    fn coordinate_conversion_round_trips_at_multiple_viewports_and_zooms() {
        for (width, height, zoom) in [
            (800.0, 600.0, 1.0),
            (1920.0, 1080.0, 100.0),
            (320.0, 240.0, 0.25),
        ] {
            let viewport = Viewport::new(width, height).unwrap();
            let camera = Camera2D::new(Vec2::new(3.5, -2.0), zoom).unwrap();
            let world = Vec2::new(-12.25, 8.75);
            let screen = camera.world_to_screen(world, viewport).unwrap();
            assert_vec_close(
                camera.screen_to_world(screen, viewport).unwrap(),
                world,
                0.0001,
            );
        }
    }
    #[test]
    fn invalid_coordinate_inputs_are_rejected() {
        assert!(matches!(
            Viewport::new(0.0, 1.0),
            Err(CoordinateError::InvalidViewport { .. })
        ));
        assert!(matches!(
            Camera2D::new(Vec2::zero(), 0.0),
            Err(CoordinateError::InvalidZoom(_))
        ));
        assert!(matches!(
            Camera2D::new(Vec2::new(f32::NAN, 0.0), 1.0),
            Err(CoordinateError::NonFiniteVector { .. })
        ));
    }

    #[test]
    fn camera_rotation_round_trips_and_rotates_axes() {
        let viewport = Viewport::new(200.0, 100.0).unwrap();
        let camera = Camera2D::new(Vec2::zero(), 10.0)
            .unwrap()
            .with_rotation(std::f32::consts::FRAC_PI_2)
            .unwrap();
        assert_vec_close(
            camera.world_to_screen(WORLD_RIGHT, viewport).unwrap(),
            Vec2::new(100.0, 60.0),
            0.0001,
        );
        let world = Vec2::new(2.5, -3.0);
        assert_vec_close(
            camera
                .screen_to_world(camera.world_to_screen(world, viewport).unwrap(), viewport)
                .unwrap(),
            world,
            0.0001,
        );
    }

    #[test]
    fn camera_bounds_keep_viewport_inside_world_rectangle() {
        let viewport = Viewport::new(100.0, 100.0).unwrap();
        let camera = Camera2D::new(Vec2::new(-100.0, 100.0), 10.0)
            .unwrap()
            .with_bounds(CameraBounds::new(Vec2::new(-5.0, -5.0), Vec2::new(5.0, 5.0)).unwrap())
            .unwrap();
        assert_eq!(
            camera.world_to_screen(Vec2::zero(), viewport).unwrap(),
            Vec2::new(50.0, 50.0)
        );
        assert_eq!(
            camera
                .screen_to_world(Vec2::new(0.0, 0.0), viewport)
                .unwrap(),
            Vec2::new(-5.0, 5.0)
        );
    }

    /// WP5 旧新对拍基线：**投影与射线的约定**逐条钉住（3D 侧）。
    ///
    /// 上面那些测试是性质型的（往返一致、分类正确），它们拦不住"约定被悄悄换掉"——把 NDC 的 y
    /// 翻转、把深度范围从 0..1 换成 -1..1、把 FOV 从垂直改成水平，往返测试全都照样通过，而画面
    /// 与拾取会整体错位。相机收敛要动投影，所以先把**数值基线**钉在这里：任何一处变了都必须
    /// 是有意识的决定，而不是顺手改掉。
    #[test]
    fn projection_conventions_are_pinned_for_the_convergence() {
        let camera = camera(); // position (0,0,5)、看向原点、fov_y = π/2、near 0.1、far 100
        assert!((camera.aspect_ratio - 1.0).abs() < 1e-6);

        // ① 投影矩阵：f = 1/tan(fov_y/2) = 1；aspect 只影响 x；深度映射为 **0..1**。
        let projection = camera.projection_matrix().unwrap();
        let column = |index: usize| projection.columns[index];
        assert!(
            (column(0)[0] - 1.0).abs() < 1e-5,
            "fov_y 是**垂直**视场角，x 除以 aspect"
        );
        assert!((column(1)[1] - 1.0).abs() < 1e-5);
        let range = 100.0 - 0.1;
        assert!(
            (column(2)[2] - (-100.0 / range)).abs() < 1e-4,
            "深度范围 0..1（不是 -1..1）"
        );
        assert!((column(2)[3] + 1.0).abs() < 1e-5, "透视除法项必须是 -1");
        assert!((column(3)[2] - (-0.1 * 100.0 / range)).abs() < 1e-4);

        // ② 视图矩阵是右手系、看向 -Z：相机在 (0,0,5) 看向原点时，原点映射到 z = -5。
        let view = camera.view_matrix().unwrap();
        let origin_in_view = view.transform_point(Vec3::ZERO).unwrap();
        assert!(origin_in_view.x.abs() < 1e-5 && origin_in_view.y.abs() < 1e-5);
        assert!(
            (origin_in_view.z + 5.0).abs() < 1e-5,
            "前方向是 -Z（右手系）"
        );

        // ③ 世界 → NDC：世界 +x 落 NDC 右半边、世界 +y 落 NDC **上**半边；
        //    深度随距离单调增且始终在 0..1 内。
        let right = camera.world_to_ndc(Vec3::new(1.0, 0.0, 0.0)).unwrap();
        let up = camera.world_to_ndc(Vec3::new(0.0, 1.0, 0.0)).unwrap();
        assert!(right.x > 0.0 && right.y.abs() < 1e-5, "世界 +x → NDC +x");
        assert!(
            up.y > 0.0 && up.x.abs() < 1e-5,
            "世界 +y → NDC +y（NDC y 向上）"
        );
        let near_point = camera.world_to_ndc(Vec3::new(0.0, 0.0, 4.0)).unwrap();
        let far_point = camera.world_to_ndc(Vec3::new(0.0, 0.0, -50.0)).unwrap();
        assert!(near_point.z > 0.0 && near_point.z < far_point.z && far_point.z < 1.0);

        // ④ 射线与投影必须互为逆：把任意世界点投到屏幕，再从该屏幕坐标发射线，射线必须**穿过**
        //    该点。这条比逐元素断言更能锁住"投影和拾取是同一套"。
        let world = Vec3::new(1.5, -0.75, 0.5);
        let ndc = camera.world_to_ndc(world).unwrap();
        let (width, height) = (640.0, 480.0);
        let screen_x = (ndc.x + 1.0) * 0.5 * width;
        let screen_y = (1.0 - ndc.y) * 0.5 * height;
        let (origin, direction) = camera
            .ray_from_screen(screen_x, screen_y, width, height)
            .unwrap();
        let along = (world - origin).dot(direction);
        let closest = origin + direction * along;
        assert!(
            (closest - world).length() < 1e-3,
            "屏幕坐标反投影出的射线必须穿过该世界点：closest={closest:?} world={world:?}"
        );

        // ⑤ 视锥：正前方的点在视锥内、相机背后的点在视锥外。
        let frustum = camera.frustum().unwrap();
        assert!(frustum.contains_point(Vec3::ZERO));
        assert!(
            !frustum.contains_point(Vec3::new(0.0, 0.0, 50.0)),
            "相机背后的点必须在视锥外"
        );
    }

    /// WP5 基线（2D 侧）：剔除用的 `visible_world_bounds` 与绘制用的 `world_to_screen` 必须是
    /// 同一套换算——各算各的就会出现"剔除认为不可见、绘制却要画"的静默错位。
    #[test]
    fn visible_world_bounds_agrees_with_the_projection() {
        let viewport = Viewport::new(200.0, 100.0).unwrap();
        let camera = Camera2D::new(Vec2::new(10.0, 20.0), 2.0).unwrap();
        let bounds = camera.visible_world_bounds(viewport).unwrap();
        // 半宽 = 200/2/2 = 50，半高 = 100/2/2 = 25。
        assert_vec_close(bounds.min, Vec2::new(-40.0, -5.0), 0.0001);
        assert_vec_close(bounds.max, Vec2::new(60.0, 45.0), 0.0001);
        // 世界 y 向上、屏幕 y 向下：屏幕左上角对应 (min.x, max.y)。
        assert_vec_close(
            camera.screen_to_world(Vec2::zero(), viewport).unwrap(),
            Vec2::new(bounds.min.x, bounds.max.y),
            0.0001,
        );
        assert_vec_close(
            camera
                .screen_to_world(Vec2::new(200.0, 100.0), viewport)
                .unwrap(),
            Vec2::new(bounds.max.x, bounds.min.y),
            0.0001,
        );

        // 旋转后 bounds 是外接 AABB：视口四角仍必须落在其中（宁可多留不可少算）。
        let rotated_camera = camera.with_rotation(std::f32::consts::FRAC_PI_6).unwrap();
        let rotated = rotated_camera.visible_world_bounds(viewport).unwrap();
        for corner in [(0.0, 0.0), (200.0, 0.0), (200.0, 100.0), (0.0, 100.0)] {
            let world = rotated_camera
                .screen_to_world(Vec2::new(corner.0, corner.1), viewport)
                .unwrap();
            assert!(
                world.x >= rotated.min.x - 0.0001
                    && world.x <= rotated.max.x + 0.0001
                    && world.y >= rotated.min.y - 0.0001
                    && world.y <= rotated.max.y + 0.0001,
                "旋转后视口角点 {world:?} 落在 bounds 之外：{rotated:?}"
            );
        }
        // 有边界钳制时，bounds 必须跟着**钳制后的**中心走（否则剔除域与画面不一致）。
        let clamped = camera
            .with_bounds(CameraBounds::new(Vec2::zero(), Vec2::new(1000.0, 1000.0)).unwrap())
            .unwrap()
            .visible_world_bounds(viewport)
            .unwrap();
        assert_vec_close(clamped.min, Vec2::zero(), 0.0001);
        assert_vec_close(clamped.max, Vec2::new(100.0, 50.0), 0.0001);
    }

    /// WP5：相机姿态可以来自实体的 `Transform3D`（`pose=transform`）。
    ///
    /// 三个要点：① 默认不开启且规范字符串不变；② 开启后位置/视线/上方向都来自变换
    /// （视线 = 变换的 -Z、上方向 = 变换的 +Y）；③ 姿态无法表达时报错，不静默退回。
    #[test]
    fn camera_pose_can_come_from_a_transform() {
        let base = camera(); // 默认 look-at 姿态
        assert!(!base.pose_from_transform, "默认必须是不跟随变换");
        assert!(
            !base.to_component_str().contains("pose="),
            "默认不写 pose 段，既有规范字符串不变"
        );

        let parsed = Camera3D::from_component_str(
            "position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=transform",
        )
        .unwrap();
        assert!(parsed.pose_from_transform);
        assert!(parsed.to_component_str().ends_with(";pose=transform"));
        // 拼错的值必须报错，不能当默认。
        assert!(
            Camera3D::from_component_str(
                "position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=guess"
            )
            .is_err()
        );

        // 姿态：实体在 (5, 1, 0) 绕 Y 转 90°（右手系）⇒ 视线朝 -X。
        let pose = Transform3D {
            position: Vec3::new(5.0, 1.0, 0.0),
            rotation: Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2).unwrap(),
            scale: Vec3::new(1.0, 1.0, 1.0),
        };
        let posed = base.with_pose(pose).unwrap();
        assert!(posed.pose_from_transform);
        assert!((posed.position - pose.position).length() < 1e-5);
        let forward = (posed.target - posed.position).normalized().unwrap();
        assert!(
            (forward - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-4,
            "绕 Y 转 90° 后视线应为 -X：{forward:?}"
        );
        assert!(
            (posed.up - Vec3::Y).length() < 1e-4,
            "上方向应仍是 +Y：{:?}",
            posed.up
        );
        // 投影参数不受姿态影响。
        assert!((posed.fov_y - base.fov_y).abs() < 1e-6);
        assert!((posed.far - base.far).abs() < 1e-6);

        // 退化姿态必须报错，而不是悄悄用回旧姿态。
        let mut degenerate = pose;
        degenerate.rotation = Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 0.0,
        };
        assert!(base.with_pose(degenerate).is_err());
    }

    /// WP5 收敛本身：统一的 [`Projection`] 必须与两套相机的既有数值**一致**，
    /// [`RenderView`] 与 [`CameraSelection`] 给出"一帧怎么投影、哪台相机生效"的单一说法。
    #[test]
    fn unified_projection_and_render_view_match_both_families() {
        let viewport = Viewport::new(640.0, 360.0).unwrap();
        let camera2d = Camera2D::new(Vec2::new(100.0, 50.0), 2.0).unwrap();
        let camera3d = camera();

        // 正交投影的视野半宽 = 视口宽 / 2 / zoom（像素 → 世界单位），深度不参与。
        let orthographic = Projection::from_camera2d(camera2d);
        assert_eq!(orthographic, Projection::Orthographic { zoom: 2.0 });
        assert!(orthographic.is_orthographic() && !orthographic.uses_depth());
        // 透视侧沿用同一个 `Camera3D` 实现，因此矩阵逐元素相同（收敛不换算法）。
        let perspective = Projection::from_camera3d(camera3d);
        assert_eq!(
            perspective,
            Projection::Perspective {
                fov_y: camera3d.fov_y,
                near: camera3d.near,
                far: camera3d.far
            }
        );
        assert!(perspective.uses_depth() && !perspective.is_orthographic());

        // 渲染视图：2D overlay 不清屏、后画；3D 主视图清屏、先画——顺序与清屏策略只有这一处说法。
        let overlay = RenderView::overlay_2d(camera2d, viewport);
        let main = RenderView::main_3d(camera3d, viewport, [0.1, 0.2, 0.3, 1.0]);
        assert!(overlay.order > main.order, "overlay 必须画在主视图之后");
        assert_eq!(overlay.clear, ClearPolicy::None);
        assert_eq!(main.clear, ClearPolicy::Color([0.1, 0.2, 0.3, 1.0]));
        assert!((overlay.aspect() - main.aspect()).abs() < 1e-6);

        // 多相机选择：显式选中优先，否则退回排序序第一个（同一输入必须给同一结果）。
        let mut selection = CameraSelection::new();
        let candidates = [(7u64, "seven"), (3, "three"), (9, "nine")];
        assert_eq!(selection.resolve(candidates), Some("seven"));
        selection.select(Some(9));
        assert_eq!(selection.resolve(candidates), Some("nine"));
        selection.select(Some(42));
        assert_eq!(
            selection.resolve(candidates),
            Some("seven"),
            "选中的实体不存在时退回第一个候选，而不是什么都不画"
        );
        assert_eq!(selection.resolve(std::iter::empty::<(u64, &str)>()), None);
    }

    /// WP6：2D 相机的**矩阵形式**必须与既有的逐点换算完全一致（否则"世界 Sprite 走深度通道"
    /// 会把 2D 内容画到别处）。这条同时钉住 y 翻转、旋转、缩放与深度范围两端。
    #[test]
    fn two_d_view_projection_matches_the_pointwise_projection() {
        let viewport = Viewport::new(640.0, 360.0).unwrap();
        for rotation in [0.0_f32, 0.35, -1.2] {
            for zoom in [1.0_f32, 2.5] {
                let camera = Camera2D {
                    center: Vec2::new(100.0, 50.0),
                    rotation,
                    zoom,
                    bounds: None,
                    enabled: true,
                };
                let matrix = camera.view_projection(viewport, 0.0, 100.0).unwrap();
                for world in [
                    Vec2::new(100.0, 50.0),
                    Vec2::new(160.0, 50.0),
                    Vec2::new(100.0, 90.0),
                    Vec2::new(40.0, 10.0),
                ] {
                    // 逐点换算 → 屏幕像素 → NDC。
                    let screen = camera.world_to_screen(world, viewport).unwrap();
                    let expected_x = screen.x / viewport.width * 2.0 - 1.0;
                    let expected_y = 1.0 - screen.y / viewport.height * 2.0;
                    let ndc = matrix
                        .transform_point(Vec3::new(world.x, world.y, 0.0))
                        .unwrap();
                    assert!(
                        (ndc.x - expected_x).abs() < 1e-4 && (ndc.y - expected_y).abs() < 1e-4,
                        "rotation={rotation} zoom={zoom} world={world:?}: 矩阵 {ndc:?} vs 逐点 ({expected_x}, {expected_y})"
                    );
                    // 世界 z = near/far 正好映射到 NDC z = 0/1（与 3D 同一约定）。
                    assert!(ndc.z.abs() < 1e-5, "near 平面应映射到 z=0：{ndc:?}");
                    let far_ndc = matrix
                        .transform_point(Vec3::new(world.x, world.y, 100.0))
                        .unwrap();
                    assert!(
                        (far_ndc.z - 1.0).abs() < 1e-5,
                        "far 平面应映射到 z=1：{far_ndc:?}"
                    );
                }
            }
        }
        // 非法深度范围必须报错，而不是产出一个把深度压平的矩阵。
        let camera = Camera2D::new(Vec2::zero(), 1.0).unwrap();
        assert!(camera.view_projection(viewport, 10.0, 10.0).is_err());
        assert!(camera.view_projection(viewport, 0.0, f32::NAN).is_err());
    }
    /// WP5 接线：渲染器消费的是 [`FrameViews`]——清屏由**顺序最前**的已声明视图给出，绘制顺序由
    /// `order` 决定；**什么都没声明**时清屏策略为 `None`，渲染器据此回退到历史默认。
    ///
    /// docs/54 §4.2：本批之前清屏硬编码取 `main`，`order` 因此是个没有读者的字段。现在两者共用
    /// [`FrameViews::ordered`] 一处规则：声明一个 `order` 更小的视图就能接管清屏。唯一的语义变化是
    /// **只声明 overlay** 的场景（此前退回历史默认色、现在由该视图的 `ClearPolicy` 决定）——仓库里
    /// 没有这样的宿主（编辑器与运行时都是"声明 main"），下面这条断言把这个变化钉住。
    #[test]
    fn frame_views_declare_clear_and_draw_order() {
        let viewport = Viewport::new(320.0, 180.0).unwrap();
        let camera2d = Camera2D::new(Vec2::new(10.0, 20.0), 1.0).unwrap();
        let overlay = RenderView::overlay_2d(camera2d, viewport);

        // 什么都没声明：渲染器保持历史默认（默认清屏色 + 整目标视口）。
        assert_eq!(FrameViews::new().clear(), None);
        assert_eq!(FrameViews::default().clear(), None);

        // 纯 2D：只有 overlay 视图 ⇒ **由它决定清屏**（`overlay_2d` 声明的是"不清屏"，因此渲染器
        // 不再清屏——这是"清屏由顺序最前的视图说了算"的直接后果，见上面的测试说明）。
        let two_d = FrameViews::overlay_only(overlay);
        assert_eq!(two_d.clear(), Some(ClearPolicy::None));
        assert_eq!(two_d.ordered(), [Some(overlay), None]);

        // 2D + 3D：清屏色由主视图给出，顺序固定为"主视图先、overlay 后"。
        let main = RenderView::main_3d(camera(), viewport, DEFAULT_3D_CLEAR);
        let layered = FrameViews::layered(main, overlay);
        assert_eq!(layered.clear(), Some(ClearPolicy::Color(DEFAULT_3D_CLEAR)));
        assert_eq!(layered.ordered(), [Some(main), Some(overlay)]);

        // 主视图显式声明"不清屏"时，宿主可以把它叠到已有内容上。
        let no_clear = FrameViews {
            main: Some(RenderView {
                clear: ClearPolicy::None,
                ..main
            }),
            overlay: Some(overlay),
        };
        assert_eq!(no_clear.clear(), Some(ClearPolicy::None));

        // overlay 声明的 order 比主视图更小 ⇒ 它先画（顺序由声明决定，不是写死的）。
        let early_overlay = FrameViews::layered(
            main,
            RenderView {
                order: -1,
                // 显式声明清屏色：证明"谁清屏"真的跟着 `order` 走（而不是永远取 main）。
                clear: ClearPolicy::Color([0.0, 1.0, 0.0, 1.0]),
                ..overlay
            },
        );
        assert_eq!(early_overlay.ordered()[0].map(|view| view.order), Some(-1));
        // docs/54 §4.2：`order` 现在真的有消费者——清屏由**顺序最前**的视图说了算。
        assert_eq!(
            early_overlay.clear(),
            Some(ClearPolicy::Color([0.0, 1.0, 0.0, 1.0])),
            "order 更小的 overlay 应当拥有本帧的清屏策略"
        );
        assert_eq!(
            layered.clear(),
            Some(ClearPolicy::Color(DEFAULT_3D_CLEAR)),
            "顺序相等时 main 优先（既有宿主行为不变）"
        );
    }
}

#[cfg(test)]
mod controller_tests {
    use super::*;

    fn controller() -> Camera2dController {
        let mut controller = Camera2dController::default();
        controller.follow(Some(EntityId::from_raw(1, 1)));
        controller
    }

    /// WP5 收敛后的新边界：请求消费 → 跟随 → advance → bounds 钳制的**次序**固定在这一处，
    /// 宿主不再自己拼；`resolve_view` 再把它交给统一的 [`RenderView`]。
    #[test]
    fn resolve_camera_owns_request_follow_and_bounds_order() {
        let viewport = Viewport::new(200.0, 100.0).unwrap();
        let mut controller = Camera2dController::default();
        let base = Camera2D::new(Vec2::new(500.0, 500.0), 1.0)
            .unwrap()
            .with_bounds(CameraBounds::new(Vec2::zero(), Vec2::new(1000.0, 1000.0)).unwrap())
            .unwrap();

        // ① 脚本请求优先，且**取走即清**：第二次推进不再重放同一个请求。
        controller.request_center(Vec2::new(200.0, 120.0));
        controller.request_zoom(3.0);
        let resolved = controller.resolve_camera(base, None, viewport, 1.0 / 60.0);
        assert_eq!(resolved.zoom, 3.0);
        assert_eq!(resolved.center, Vec2::new(200.0, 120.0));
        let again = controller.resolve_camera(resolved, None, viewport, 1.0 / 60.0);
        assert_eq!(again.center, Vec2::new(200.0, 120.0), "请求只消费一次");

        // ② 跟随 + 立即跟随（smoothing = 0）：中心落到 目标 + 偏移，再被 bounds 钳制。
        //    视口 200×100、zoom 1 ⇒ 半宽 100、半高 50 ⇒ 中心最小为 (100, 50)。
        controller.follow(Some(EntityId::from_raw(1, 1)));
        controller.set_offset(Vec2::new(0.0, -20.0));
        let followed =
            controller.resolve_camera(base, Some(Vec2::new(5.0, 5.0)), viewport, 1.0 / 60.0);
        assert_eq!(
            followed.center,
            Vec2::new(100.0, 50.0),
            "bounds 必须把整个视口夹进世界矩形"
        );

        // ③ 同一条路径给出统一视图描述（2D overlay：正交 + 不清屏）。
        let view =
            controller.resolve_view(base, Some(Vec2::new(500.0, 500.0)), viewport, 1.0 / 60.0);
        assert_eq!(view.clear, ClearPolicy::None);
        assert!(view.projection.is_orthographic());
        assert_eq!(view.viewport, viewport);
    }
    #[test]
    fn without_follow_the_center_is_untouched() {
        let mut controller = Camera2dController::default();
        let center = Vec2::new(10.0, 20.0);
        assert_eq!(
            controller.advance(center, Some(Vec2::new(500.0, 500.0)), 1.0 / 60.0),
            center
        );
    }

    #[test]
    fn immediate_follow_snaps_to_target_plus_offset() {
        let mut controller = controller();
        controller.set_offset(Vec2::new(0.0, -32.0));
        let center = controller.advance(Vec2::zero(), Some(Vec2::new(100.0, 50.0)), 1.0 / 60.0);
        assert_eq!(center, Vec2::new(100.0, 18.0));
    }

    #[test]
    fn dead_zone_holds_still_until_the_target_leaves_it() {
        let mut controller = controller();
        controller.set_dead_zone(Vec2::new(16.0, 16.0));
        let current = Vec2::new(100.0, 100.0);
        // 目标在死区内：不动。
        assert_eq!(
            controller.advance(current, Some(Vec2::new(110.0, 90.0)), 0.016),
            current
        );
        // 超出死区 24px：推进到只差死区边界（x 方向超 24，推 24-16=8）。
        let moved = controller.advance(current, Some(Vec2::new(124.0, 100.0)), 0.016);
        assert!((moved.x - 108.0).abs() < 1e-4, "{moved:?}");
        assert!((moved.y - 100.0).abs() < 1e-4, "{moved:?}");
    }

    #[test]
    fn smoothing_converges_independently_of_frame_rate() {
        let mut coarse = controller();
        coarse.set_smoothing(6.0);
        let mut fine = controller();
        fine.set_smoothing(6.0);
        let target = Some(Vec2::new(100.0, 0.0));

        // 同样的 1 秒总时长，步长差 4 倍，指数收敛的结果应当几乎一致。
        let mut coarse_center = Vec2::zero();
        for _ in 0..10 {
            coarse_center = coarse.advance(coarse_center, target, 0.1);
        }
        let mut fine_center = Vec2::zero();
        for _ in 0..40 {
            fine_center = fine.advance(fine_center, target, 0.025);
        }
        assert!(
            (coarse_center.x - fine_center.x).abs() < 1.0,
            "{} vs {}",
            coarse_center.x,
            fine_center.x
        );
        // 且确实在向目标收敛。
        assert!(coarse_center.x > 90.0, "{}", coarse_center.x);
    }

    #[test]
    fn shake_decays_to_zero_and_is_deterministic() {
        let mut first = controller();
        first.shake(8.0, 0.2);
        let mut second = controller();
        second.shake(8.0, 0.2);
        let mut previous = Vec2::zero();
        let mut moved_off_center = false;
        for _ in 0..20 {
            let a = first.advance(Vec2::zero(), None, 1.0 / 60.0);
            let b = second.advance(Vec2::zero(), None, 1.0 / 60.0);
            assert_eq!(a, b, "同一帧序列必须给出同一抖动结果");
            if a.x.abs() > 1e-6 || a.y.abs() > 1e-6 {
                moved_off_center = true;
            }
            previous = a;
        }
        assert!(moved_off_center, "抖动期间应偏离基准中心");
        assert!(!first.is_shaking(), "0.2s 后抖动应结束");
        // 结束后回到基准中心。
        assert!(
            previous.x.abs() < 1e-3 && previous.y.abs() < 1e-3,
            "{previous:?}"
        );
    }

    #[test]
    fn rejects_degenerate_configuration() {
        let mut controller = Camera2dController::default();
        controller.set_smoothing(f32::NAN);
        assert_eq!(controller.smoothing(), 0.0);
        controller.set_smoothing(-3.0);
        assert_eq!(controller.smoothing(), 0.0);
        controller.shake(f32::NAN, 0.1);
        assert!(!controller.is_shaking());
        controller.shake(4.0, 0.0);
        assert!(!controller.is_shaking());
    }

    #[test]
    fn huge_delta_is_clamped_to_keep_motion_predictable() {
        let mut controller = controller();
        controller.set_smoothing(6.0);
        let moved = controller.advance(Vec2::zero(), Some(Vec2::new(1000.0, 0.0)), 10.0);
        // 步长被夹到 0.1s：收敛比例 = 1-e^-0.6 ≈ 0.451，而不是一步到位。
        assert!(moved.x > 400.0 && moved.x < 480.0, "{moved:?}");
    }
}
