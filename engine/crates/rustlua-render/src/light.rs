//! Lights for 3D scenes: one directional key light plus up to
//! [`MAX_POINT_LIGHTS`] non-shadowed point lights.

use rustlua_math::math3d::Vec3;

/// 单帧参与着色的点光源上限。着色器里的数组长度与这个数必须一致
/// （`mesh3d_pbr.wgsl` 的 `array<PointLight, 8>`）。
pub const MAX_POINT_LIGHTS: usize = 8;

/// 光源类型。`Light` 组件字符串里写作 `type=point`，缺省是平行光。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LightKind {
    #[default]
    Directional,
    Point,
}

/// A light attached to an entity and parsed from the scene's `Light`
/// component string: `direction=x,y,z;color=r,g,b;intensity=<i>`.
/// Point lights add `type=point;position=x,y,z;range=<r>`.
///
/// `direction` is the direction the light shines toward (the renderer
/// negates it to compute the surface-to-light vector), `color` is the
/// linear RGB tint, and `intensity` scales the contribution.
///
/// **符号约定（容易写反，因此在这里写死）**：`direction` 是**光传播方向**，
/// 表面→光源向量 = `-direction`（着色器 `l = normalize(-light.direction)`）。因此：
///
/// - 天空里的太阳（自上而下照）写作 `direction=0.4,-0.8,0.5`——**y 为负**；
///   写成 y 为正等于"光往上照"，朝上的表面 `diffuse = 0`，场景只剩 0.15 环境光。
/// - 阴影相机（`renderer::light_view_projection`）放在 **`-direction`** 一侧（光源那一侧），
///   与上式同源；两处符号必须一致，否则阴影贴图与光照方向互相打架。
///
/// 点光源的 `position` 是世界坐标（不跟实体 Transform 走：同一个实体可以既被移动
/// 又保留一个固定位置的光照，也便于脚本直接改数值）；`range` 是平滑衰减到 0 的半径，
/// 点光源**不投射阴影**（8 张阴影贴图的代价对移动端不划算）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    pub direction: Vec3,
    /// 点光源的世界位置；平行光忽略该字段。
    pub position: Vec3,
    pub color: [f32; 3],
    pub intensity: f32,
    /// 点光源的衰减半径（米）；平行光忽略该字段。
    pub range: f32,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            kind: LightKind::Directional,
            // 太阳在天空：光传播方向朝下。
            direction: Vec3::new(0.4, -0.8, 0.5),
            position: Vec3::new(0.0, 0.0, 0.0),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            range: 10.0,
        }
    }
}

/// 点光源的着色参数：平铺成两个 vec4 供 uniform 上传。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    pub position: Vec3,
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            position: Vec3::new(0.0, 0.0, 0.0),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            range: 10.0,
        }
    }
}

impl PointLight {
    /// 点类型的 `Light` 组件转成着色参数；平行光返回 `None`。
    #[must_use]
    pub fn from_light(light: &Light) -> Option<Self> {
        (light.kind == LightKind::Point).then_some(Self {
            position: light.position,
            color: light.color,
            intensity: light.intensity,
            range: light.range,
        })
    }

    /// 着色器友好的一对 vec4：`xyz=position, w=range`、`rgb=color, a=intensity`。
    #[must_use]
    pub fn uniform(self) -> ([f32; 4], [f32; 4]) {
        (
            [
                self.position.x,
                self.position.y,
                self.position.z,
                self.range,
            ],
            [self.color[0], self.color[1], self.color[2], self.intensity],
        )
    }
}

/// Errors while parsing the scene's `Light` component string.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum LightParseError {
    /// A field was not `key=value` as the scene format requires.
    #[error("malformed Light field: {0}")]
    MalformedField(String),
    /// A required field (`direction`/`color`/`intensity`) was missing.
    #[error("missing Light field: {0}")]
    MissingField(&'static str),
    /// A numeric or vector field could not be parsed or was not finite.
    #[error("invalid Light value: {0}")]
    InvalidValue(String),
    /// `intensity` was negative.
    #[error("Light intensity must be non-negative")]
    NegativeIntensity,
    /// A point light's `range` was zero or negative.
    #[error("Light range must be positive")]
    InvalidRange,
    /// The direction was the zero vector.
    #[error("Light direction must be non-zero")]
    ZeroDirection,
}

impl Light {
    /// Parses the scene component string written by the editor:
    /// `direction=x,y,z;color=r,g,b;intensity=<i>` for a directional light, or
    /// `type=point;position=x,y,z;range=<r>` in addition for a point light.
    pub fn from_component_str(value: &str) -> Result<Self, LightParseError> {
        let mut light = Self::default();
        let mut seen = [false; 3];
        let mut position_seen = false;
        let mut range_seen = false;
        for field in value.split(';') {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| LightParseError::MalformedField(field.to_owned()))?;
            match key {
                "direction" => {
                    light.direction = parse_light_vec3(raw)?;
                    seen[0] = true;
                }
                "color" => {
                    let color = parse_light_vec3(raw)?;
                    light.color = [color.x, color.y, color.z];
                    seen[1] = true;
                }
                "intensity" => {
                    light.intensity = parse_light_number(raw)?;
                    seen[2] = true;
                }
                "type" => {
                    light.kind = match raw.trim() {
                        "directional" => LightKind::Directional,
                        "point" => LightKind::Point,
                        other => return Err(LightParseError::InvalidValue(other.to_owned())),
                    };
                }
                "position" => {
                    light.position = parse_light_vec3(raw)?;
                    position_seen = true;
                }
                "range" => {
                    light.range = parse_light_number(raw)?;
                    range_seen = true;
                }
                _ => return Err(LightParseError::MalformedField(field.to_owned())),
            }
        }
        for (field, present) in [
            ("direction", seen[0]),
            ("color", seen[1]),
            ("intensity", seen[2]),
        ] {
            if !present {
                return Err(LightParseError::MissingField(field));
            }
        }
        if light.intensity < 0.0 {
            return Err(LightParseError::NegativeIntensity);
        }
        if light.kind == LightKind::Point {
            if !position_seen {
                return Err(LightParseError::MissingField("position"));
            }
            if !range_seen {
                return Err(LightParseError::MissingField("range"));
            }
            if light.range <= 0.0 {
                return Err(LightParseError::InvalidRange);
            }
        } else if light.direction.length_squared() <= f32::EPSILON {
            return Err(LightParseError::ZeroDirection);
        }
        Ok(light)
    }

    /// Encodes the component back to the editor's string format. Directional
    /// lights keep the pre-point-light spelling byte for byte so existing
    /// scene files round-trip unchanged.
    #[must_use]
    pub fn to_component_str(self) -> String {
        let mut text = format!(
            "direction={:.9},{:.9},{:.9};color={:.9},{:.9},{:.9};intensity={:.9}",
            self.direction.x,
            self.direction.y,
            self.direction.z,
            self.color[0],
            self.color[1],
            self.color[2],
            self.intensity
        );
        if self.kind == LightKind::Point {
            text.push_str(&format!(
                ";type=point;position={:.9},{:.9},{:.9};range={:.9}",
                self.position.x, self.position.y, self.position.z, self.range
            ));
        }
        text
    }

    /// Shader-friendly uniform pair: direction + color/intensity.
    #[must_use]
    pub fn uniform(self) -> ([f32; 4], [f32; 4]) {
        (
            [self.direction.x, self.direction.y, self.direction.z, 0.0],
            [self.color[0], self.color[1], self.color[2], self.intensity],
        )
    }
}

/// Parses an `x,y,z` vector, rejecting non-finite components.
fn parse_light_vec3(raw: &str) -> Result<Vec3, LightParseError> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 3 {
        return Err(LightParseError::InvalidValue(raw.to_owned()));
    }
    let mut values = [0.0_f32; 3];
    for (slot, part) in values.iter_mut().zip(&parts) {
        let parsed: f32 = part
            .trim()
            .parse()
            .map_err(|_| LightParseError::InvalidValue(raw.to_owned()))?;
        if !parsed.is_finite() {
            return Err(LightParseError::InvalidValue(raw.to_owned()));
        }
        *slot = parsed;
    }
    Ok(Vec3::new(values[0], values[1], values[2]))
}

/// Parses a finite scalar light parameter.
fn parse_light_number(raw: &str) -> Result<f32, LightParseError> {
    let parsed: f32 = raw
        .trim()
        .parse()
        .map_err(|_| LightParseError::InvalidValue(raw.to_owned()))?;
    if !parsed.is_finite() {
        return Err(LightParseError::InvalidValue(raw.to_owned()));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_editor_component_string() {
        let light =
            Light::from_component_str("direction=0.4,0.8,0.5;color=1,1,1;intensity=2").unwrap();
        assert_eq!(light.direction, Vec3::new(0.4, 0.8, 0.5));
        assert_eq!(light.color, [1.0, 1.0, 1.0]);
        assert_eq!(light.intensity, 2.0);
        assert_eq!(
            Light::from_component_str(&light.to_component_str()).unwrap(),
            light
        );
        assert_eq!(light.uniform().1, [1.0, 1.0, 1.0, 2.0]);
    }
    #[test]
    fn rejects_incomplete_or_invalid_component_strings() {
        assert!(matches!(
            Light::from_component_str("direction=0.4,0.8,0.5;color=1,1,1"),
            Err(LightParseError::MissingField("intensity"))
        ));
        assert!(matches!(
            Light::from_component_str("direction=0.4,0.8;color=1,1,1;intensity=1"),
            Err(LightParseError::InvalidValue(_))
        ));
        assert!(matches!(
            Light::from_component_str("direction=0.4,0.8,0.5;color=1,1,1;intensity=-1"),
            Err(LightParseError::NegativeIntensity)
        ));
        assert!(matches!(
            Light::from_component_str("direction=0,0,0;color=1,1,1;intensity=1"),
            Err(LightParseError::ZeroDirection)
        ));
        assert!(matches!(
            Light::from_component_str("direction=nan,0.8,0.5;color=1,1,1;intensity=1"),
            Err(LightParseError::InvalidValue(_))
        ));
    }
    #[test]
    fn defaults_are_valid_and_roundtrip() {
        assert_eq!(
            Light::from_component_str(&Light::default().to_component_str()).unwrap(),
            Light::default()
        );
        assert!(Light::default().to_component_str().contains("direction="));
        assert!(!Light::default().to_component_str().contains("type=point"));
    }

    #[test]
    fn parses_point_lights_and_roundtrips() {
        let light = Light::from_component_str(
            "direction=0,-1,0;color=1,0.5,0.25;intensity=2;type=point;position=1,2,3;range=6",
        )
        .unwrap();
        assert_eq!(light.kind, LightKind::Point);
        assert_eq!(light.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(light.range, 6.0);
        assert_eq!(
            Light::from_component_str(&light.to_component_str()).unwrap(),
            light
        );
        // 点光源按两个 vec4 上传：位置/半径 + 颜色/强度。
        let point = PointLight::from_light(&light).expect("point light");
        assert_eq!(point.uniform().0, [1.0, 2.0, 3.0, 6.0]);
        assert_eq!(point.uniform().1, [1.0, 0.5, 0.25, 2.0]);
        assert!(PointLight::from_light(&Light::default()).is_none());
    }

    #[test]
    fn point_lights_require_position_and_positive_range() {
        assert!(matches!(
            Light::from_component_str(
                "direction=0,-1,0;color=1,1,1;intensity=1;type=point;range=4"
            ),
            Err(LightParseError::MissingField("position"))
        ));
        assert!(matches!(
            Light::from_component_str(
                "direction=0,-1,0;color=1,1,1;intensity=1;type=point;position=0,1,0"
            ),
            Err(LightParseError::MissingField("range"))
        ));
        assert!(matches!(
            Light::from_component_str(
                "direction=0,-1,0;color=1,1,1;intensity=1;type=point;position=0,1,0;range=0"
            ),
            Err(LightParseError::InvalidRange)
        ));
        assert!(matches!(
            Light::from_component_str(
                "direction=0,-1,0;color=1,1,1;intensity=1;type=spot;position=0,1,0;range=2"
            ),
            Err(LightParseError::InvalidValue(_))
        ));
        // 点光源不使用方向：零方向可以接受。
        assert!(
            Light::from_component_str(
                "direction=0,0,0;color=1,1,1;intensity=1;type=point;position=0,1,0;range=2"
            )
            .is_ok()
        );
    }
}
