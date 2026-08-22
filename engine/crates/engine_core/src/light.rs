//! Directional light component for 3D scenes.

use std::{error::Error, fmt};

use crate::math3d::Vec3;

/// A directional light attached to an entity and parsed from the scene's
/// `Light` component string: `direction=x,y,z;color=r,g,b;intensity=<i>`.
///
/// `direction` is the direction the light shines toward (the renderer
/// negates it to compute the surface-to-light vector), `color` is the
/// linear RGB tint, and `intensity` scales the contribution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub direction: Vec3,
    pub color: [f32; 3],
    pub intensity: f32,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            direction: Vec3::new(0.4, 0.8, 0.5),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        }
    }
}

/// Errors while parsing the scene's `Light` component string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LightParseError {
    /// A field was not `key=value` as the scene format requires.
    MalformedField(String),
    /// A required field (`direction`/`color`/`intensity`) was missing.
    MissingField(&'static str),
    /// A numeric or vector field could not be parsed or was not finite.
    InvalidValue(String),
    /// `intensity` was negative.
    NegativeIntensity,
    /// The direction was the zero vector.
    ZeroDirection,
}
impl fmt::Display for LightParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedField(field) => write!(f, "malformed Light field: {field}"),
            Self::MissingField(field) => write!(f, "missing Light field: {field}"),
            Self::InvalidValue(value) => write!(f, "invalid Light value: {value}"),
            Self::NegativeIntensity => write!(f, "Light intensity must be non-negative"),
            Self::ZeroDirection => write!(f, "Light direction must be non-zero"),
        }
    }
}
impl Error for LightParseError {}

impl Light {
    /// Parses the scene component string written by the editor:
    /// `direction=x,y,z;color=r,g,b;intensity=<i>`.
    pub fn from_component_str(value: &str) -> Result<Self, LightParseError> {
        let mut light = Self::default();
        let mut seen = [false; 3];
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
        if light.direction.length_squared() <= f32::EPSILON {
            return Err(LightParseError::ZeroDirection);
        }
        Ok(light)
    }

    /// Encodes the component back to the editor's string format.
    #[must_use]
    pub fn to_component_str(self) -> String {
        format!(
            "direction={:.9},{:.9},{:.9};color={:.9},{:.9},{:.9};intensity={:.9}",
            self.direction.x,
            self.direction.y,
            self.direction.z,
            self.color[0],
            self.color[1],
            self.color[2],
            self.intensity
        )
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
    }
}
