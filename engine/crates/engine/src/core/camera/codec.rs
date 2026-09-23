//! 相机组件的**字符串编解码**：场景 / Prefab / 编辑器 / 迁移工具认的 `Camera` 载荷格式只有这一份实现。
//!
//! 文法与不变量都在这里：判别字段 `projection` 必填、字段体必须与判别匹配、`near < far`、
//! 退化 look-at 拒绝、未知键报错而不是忽略。跨字段校验的**唯一入口**仍是
//! `typed_scene::validate_component_payload`（编辑器的字段编辑必须经它），它复用本模块的
//! 解析器，因此编辑器与运行时不会各有一套口径。
//!
//! 本模块只在**边界**被调用——场景加载、编辑提交、迁移工具——不参与帧循环。
//! 运行时读的是 ECS 里的类型化组件，不是字符串。

use std::{error::Error, fmt};

use super::component::{
    CAMERA_COMPONENT, Camera, Camera2D, Camera3D, Camera3dError, CameraBounds, CameraProjection,
    PROJECTION_FIELD,
};
use crate::core::{math3d::Vec3, transform::Vec2};

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

impl Camera2D {
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
}

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

impl Camera3D {
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

impl Camera {
    /// 读统一 `Camera` 载荷：**判别字段必填**（schema 也这么声明）。
    ///
    /// 判别字段缺失或与字段体不匹配一律报错，不猜——猜错的代价是"画面变了却没人知道"。
    pub fn from_component_str(payload: &str) -> Result<Self, String> {
        let (declared, rest) = split_projection(payload);
        let Some(projection) = declared else {
            return Err(format!(
                "{CAMERA_COMPONENT} 缺少 {PROJECTION_FIELD} 字段（应为 orthographic 或 perspective）"
            ));
        };
        Self::from_projection(projection, &rest)
    }

    /// 按已知投影读取**去掉判别字段之后**的字段串。
    pub fn from_projection(projection: CameraProjection, rest: &str) -> Result<Self, String> {
        match projection {
            CameraProjection::Orthographic => Camera2D::from_component_str(rest)
                .map(Self::Orthographic)
                .map_err(|error| error.to_string()),
            CameraProjection::Perspective => Camera3D::from_component_str(rest)
                .map(Self::Perspective)
                .map_err(|error| error.to_string()),
        }
    }

    /// 统一写法（判别字段在前，字段体来自各自既有的编码器）。
    #[must_use]
    pub fn to_component_str(self) -> String {
        match self {
            Self::Orthographic(camera) => {
                canonical_payload(CameraProjection::Orthographic, &camera.to_component_str())
            }
            Self::Perspective(camera) => {
                canonical_payload(CameraProjection::Perspective, &camera.to_component_str())
            }
        }
    }
}

/// 统一写法：判别字段在前，其后是**各自解析器**写出的字段串。
fn canonical_payload(projection: CameraProjection, body: &str) -> String {
    format!("{PROJECTION_FIELD}={};{body}", projection.as_str())
}

/// 从载荷里取出判别字段，并返回**去掉它之后**的其余字段串（保持原有顺序）。
fn split_projection(payload: &str) -> (Option<CameraProjection>, String) {
    let mut declared = None;
    let mut rest: Vec<&str> = Vec::new();
    for field in payload.split(';') {
        if field.trim().is_empty() {
            continue;
        }
        match field.split_once('=') {
            Some((key, value)) if key.trim() == PROJECTION_FIELD => {
                declared = CameraProjection::parse(value);
            }
            _ => rest.push(field),
        }
    }
    (declared, rest.join(";"))
}
