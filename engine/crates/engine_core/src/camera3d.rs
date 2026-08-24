//! Perspective 3D camera and frustum classification.

use std::{error::Error, fmt};

use crate::math3d::{Mat4, Math3dError, Vec3};

/// Errors while parsing the scene's `Camera3D` component string.
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
    /// Parses the scene component string written by the editor:
    /// `position=x,y,z;target=x,y,z;fov=<degrees>;near=<n>;far=<f>`.
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
    #[must_use]
    pub fn to_component_str(self) -> String {
        format!(
            "position={:.9},{:.9},{:.9};target={:.9},{:.9},{:.9};fov={:.4};near={:.9};far={:.9}",
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
    fn screen_center_ray_points_toward_camera_target() {
        let camera = camera();
        let (origin, direction) = camera.ray_from_screen(320.0, 240.0, 640.0, 480.0).unwrap();
        assert!(origin.z < camera.position.z);
        assert!(direction.z < -0.9);
        assert!(direction.y.abs() < 0.01);
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
}
