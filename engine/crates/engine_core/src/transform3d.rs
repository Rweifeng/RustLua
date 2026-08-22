//! 3D transform component and model-matrix construction.

use std::{error::Error, fmt};

use crate::math3d::{Mat4, Math3dError, Quat, Vec3};

/// Errors while parsing the scene's `Transform3D` component string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Transform3dParseError {
    /// A field was not `key=<vector>` as the scene format requires.
    MalformedField(String),
    /// A required field (`position`/`rotation`/`scale`) was missing.
    MissingField(&'static str),
    /// A vector component could not be parsed or was not finite.
    InvalidVector(String),
    /// A scale component was zero or negative.
    NonPositiveScale,
    /// The rotation quaternion was not unit length.
    NonUnitQuaternion,
}
impl fmt::Display for Transform3dParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedField(field) => write!(f, "malformed Transform3D field: {field}"),
            Self::MissingField(field) => write!(f, "missing Transform3D field: {field}"),
            Self::InvalidVector(value) => write!(f, "invalid Transform3D vector: {value}"),
            Self::NonPositiveScale => write!(f, "Transform3D scale must be positive"),
            Self::NonUnitQuaternion => {
                write!(f, "Transform3D rotation quaternion must be unit length")
            }
        }
    }
}
impl Error for Transform3dParseError {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform3D {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform3D {
    fn default() -> Self {
        Self::identity()
    }
}

impl Transform3D {
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::identity(),
            scale: Vec3::new(1.0, 1.0, 1.0),
        }
    }
    #[must_use]
    pub const fn from_position(position: Vec3) -> Self {
        Self {
            position,
            ..Self::identity()
        }
    }
    pub fn model_matrix(self) -> Result<Mat4, Math3dError> {
        if !self.position.is_finite()
            || !self.scale.is_finite()
            || self.scale.x == 0.0
            || self.scale.y == 0.0
            || self.scale.z == 0.0
        {
            return Err(Math3dError::NonFinite);
        }
        Ok(Mat4::translation(self.position)
            * Mat4::from_quat(self.rotation)
            * Mat4::scale(self.scale))
    }

    /// Parses the scene component string written by the editor:
    /// `position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`.
    pub fn from_component_str(value: &str) -> Result<Self, Transform3dParseError> {
        let mut transform = Self::identity();
        let mut seen = [false; 3];
        for field in value.split(';') {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| Transform3dParseError::MalformedField(field.to_owned()))?;
            match key {
                "position" => {
                    transform.position = parse_vec3(raw, key)?;
                    seen[0] = true;
                }
                "rotation" => {
                    transform.rotation = parse_quat(raw, key)?;
                    seen[1] = true;
                }
                "scale" => {
                    transform.scale = parse_vec3(raw, key)?;
                    seen[2] = true;
                }
                _ => return Err(Transform3dParseError::MalformedField(field.to_owned())),
            }
        }
        for (field, present) in [
            ("position", seen[0]),
            ("rotation", seen[1]),
            ("scale", seen[2]),
        ] {
            if !present {
                return Err(Transform3dParseError::MissingField(field));
            }
        }
        if transform.scale.x <= 0.0 || transform.scale.y <= 0.0 || transform.scale.z <= 0.0 {
            return Err(Transform3dParseError::NonPositiveScale);
        }
        if !transform.rotation.is_unit() {
            return Err(Transform3dParseError::NonUnitQuaternion);
        }
        Ok(transform)
    }

    /// Encodes the component back to the editor's string format.
    #[must_use]
    pub fn to_component_str(self) -> String {
        format!(
            "position={:.9},{:.9},{:.9};rotation={:.9},{:.9},{:.9},{:.9};scale={:.9},{:.9},{:.9}",
            self.position.x,
            self.position.y,
            self.position.z,
            self.rotation.x,
            self.rotation.y,
            self.rotation.z,
            self.rotation.w,
            self.scale.x,
            self.scale.y,
            self.scale.z
        )
    }
}

/// Parses an `x,y,z` vector, rejecting non-finite components.
fn parse_vec3(raw: &str, field: &str) -> Result<Vec3, Transform3dParseError> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 3 {
        return Err(Transform3dParseError::InvalidVector(field.to_owned()));
    }
    let mut values = [0.0_f32; 3];
    for (slot, part) in values.iter_mut().zip(&parts) {
        let parsed: f32 = part
            .trim()
            .parse()
            .map_err(|_| Transform3dParseError::InvalidVector(field.to_owned()))?;
        if !parsed.is_finite() {
            return Err(Transform3dParseError::InvalidVector(field.to_owned()));
        }
        *slot = parsed;
    }
    Ok(Vec3::new(values[0], values[1], values[2]))
}

/// Parses an `rx,ry,rz,rw` quaternion, rejecting non-finite components.
fn parse_quat(raw: &str, field: &str) -> Result<Quat, Transform3dParseError> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 4 {
        return Err(Transform3dParseError::InvalidVector(field.to_owned()));
    }
    let mut values = [0.0_f32; 4];
    for (slot, part) in values.iter_mut().zip(&parts) {
        let parsed: f32 = part
            .trim()
            .parse()
            .map_err(|_| Transform3dParseError::InvalidVector(field.to_owned()))?;
        if !parsed.is_finite() {
            return Err(Transform3dParseError::InvalidVector(field.to_owned()));
        }
        *slot = parsed;
    }
    Ok(Quat {
        x: values[0],
        y: values[1],
        z: values[2],
        w: values[3],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_is_unchanged() {
        assert_eq!(
            Transform3D::identity().model_matrix().unwrap(),
            Mat4::identity()
        );
    }
    #[test]
    fn model_matrix_applies_scale_rotation_and_translation() {
        let rotation = Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2).unwrap();
        let transform = Transform3D {
            position: Vec3::new(2.0, 3.0, 4.0),
            rotation,
            scale: Vec3::new(2.0, 2.0, 2.0),
        };
        let result = transform
            .model_matrix()
            .unwrap()
            .transform_point(Vec3::X)
            .unwrap();
        assert!(
            (result.x - 2.0).abs() < 1e-5
                && (result.y - 3.0).abs() < 1e-5
                && (result.z - 2.0).abs() < 1e-5
        );
    }
    #[test]
    fn zero_scale_is_rejected() {
        assert_eq!(Transform3D::identity().model_matrix().map(|_| ()), Ok(()));
        assert!(
            Transform3D {
                scale: Vec3::new(0.0, 1.0, 1.0),
                ..Transform3D::identity()
            }
            .model_matrix()
            .is_err()
        );
    }
    #[test]
    fn parses_editor_component_string() {
        let transform =
            Transform3D::from_component_str("position=2,3,4;rotation=0,0,0,1;scale=1,1,1").unwrap();
        assert_eq!(
            transform,
            Transform3D {
                position: Vec3::new(2.0, 3.0, 4.0),
                rotation: Quat::identity(),
                scale: Vec3::new(1.0, 1.0, 1.0),
            }
        );
        assert_eq!(
            Transform3D::from_component_str(&transform.to_component_str()).unwrap(),
            transform
        );
    }
    #[test]
    fn rejects_incomplete_or_invalid_component_strings() {
        assert!(matches!(
            Transform3D::from_component_str("position=1,2,3;scale=1,1,1"),
            Err(Transform3dParseError::MissingField("rotation"))
        ));
        assert!(matches!(
            Transform3D::from_component_str("position=1,2;rotation=0,0,0,1;scale=1,1,1"),
            Err(Transform3dParseError::InvalidVector(_))
        ));
        assert!(matches!(
            Transform3D::from_component_str("position=1,2,3;rotation=0,0,0,1;scale=0,1,1"),
            Err(Transform3dParseError::NonPositiveScale)
        ));
        assert!(matches!(
            Transform3D::from_component_str("position=1,2,3;rotation=1,1,1,1;scale=1,1,1"),
            Err(Transform3dParseError::NonUnitQuaternion)
        ));
        assert!(matches!(
            Transform3D::from_component_str("position=nan,2,3;rotation=0,0,0,1;scale=1,1,1"),
            Err(Transform3dParseError::InvalidVector(_))
        ));
    }
    #[test]
    fn identity_component_string_roundtrips() {
        assert_eq!(
            Transform3D::identity().to_component_str(),
            "position=0.000000000,0.000000000,0.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000"
        );
        assert_eq!(
            Transform3D::from_component_str(&Transform3D::identity().to_component_str()).unwrap(),
            Transform3D::identity()
        );
    }
}
