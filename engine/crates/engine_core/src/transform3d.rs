//! 3D transform component and model-matrix construction.

use crate::math3d::{Mat4, Math3dError, Quat, Vec3};

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
}
