//! Perspective 3D camera and frustum classification.

use crate::math3d::{Mat4, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera3D {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y: f32,
    pub aspect_ratio: f32,
    pub near: f32,
    pub far: f32,
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
        };
        camera.validate()?;
        Ok(camera)
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

    pub fn frustum(self) -> Result<Frustum, Camera3dError> {
        Ok(Frustum {
            view_projection: self.view_projection_matrix()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    view_projection: Mat4,
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
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrustumClassification {
    Inside,
    Outside,
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn frustum_classifies_front_and_rear_points() {
        let camera = camera();
        let frustum = camera.frustum().unwrap();
        assert!(frustum.contains_point(Vec3::ZERO));
        assert!(!frustum.contains_point(Vec3::new(0.0, 0.0, 6.0)));
    }
    #[test]
    fn invalid_perspective_is_rejected() {
        assert!(Camera3D::perspective(Vec3::ZERO, Vec3::Z, 0.0, 1.0, 0.1, 10.0).is_err());
        assert!(Camera3D::perspective(Vec3::ZERO, Vec3::ZERO, 1.0, 1.0, 0.1, 10.0).is_err());
    }
}
