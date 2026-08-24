//! M8 3D math primitives using the engine's right-handed, Y-up convention.

use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);

    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    #[must_use]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }
    #[must_use]
    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }
    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }
    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }
    pub fn normalized(self) -> Result<Self, Math3dError> {
        let length = self.length();
        if !length.is_finite() || length <= f32::EPSILON {
            return Err(Math3dError::ZeroLengthVector);
        }
        Ok(self / length)
    }
    #[must_use]
    pub fn lerp(self, other: Self, amount: f32) -> Self {
        self * (1.0 - amount) + other * amount
    }
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}
impl std::ops::Div<f32> for Vec3 {
    type Output = Self;
    fn div(self, rhs: f32) -> Self {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs)
    }
}
impl std::ops::Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb3 {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb3 {
    pub fn new(min: Vec3, max: Vec3) -> Result<Self, Math3dError> {
        if !min.is_finite() || !max.is_finite() || min.x > max.x || min.y > max.y || min.z > max.z {
            return Err(Math3dError::NonFinite);
        }
        Ok(Self { min, max })
    }

    #[must_use]
    pub fn from_points(points: &[Vec3]) -> Option<Self> {
        let first = *points.first()?;
        if !first.is_finite() {
            return None;
        }
        let mut result = Self {
            min: first,
            max: first,
        };
        for point in &points[1..] {
            if !point.is_finite() {
                return None;
            }
            result.min.x = result.min.x.min(point.x);
            result.min.y = result.min.y.min(point.y);
            result.min.z = result.min.z.min(point.z);
            result.max.x = result.max.x.max(point.x);
            result.max.y = result.max.y.max(point.y);
            result.max.z = result.max.z.max(point.z);
        }
        Some(result)
    }

    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self {
            min: Vec3::new(
                self.min.x.min(other.min.x),
                self.min.y.min(other.min.y),
                self.min.z.min(other.min.z),
            ),
            max: Vec3::new(
                self.max.x.max(other.max.x),
                self.max.y.max(other.max.y),
                self.max.z.max(other.max.z),
            ),
        }
    }
    #[must_use]
    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    #[must_use]
    pub fn bounding_sphere(self) -> BoundingSphere3 {
        BoundingSphere3 {
            center: self.center(),
            radius: (self.max - self.min).length() * 0.5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundingSphere3 {
    pub center: Vec3,
    pub radius: f32,
}

impl BoundingSphere3 {
    pub fn new(center: Vec3, radius: f32) -> Result<Self, Math3dError> {
        if !center.is_finite() || !radius.is_finite() || radius < 0.0 {
            return Err(Math3dError::NonFinite);
        }
        Ok(Self { center, radius })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }
    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Result<Self, Math3dError> {
        if !angle.is_finite() {
            return Err(Math3dError::NonFinite);
        }
        let axis = axis.normalized()?;
        let (sin, cos) = (angle * 0.5).sin_cos();
        Ok(Self {
            x: axis.x * sin,
            y: axis.y * sin,
            z: axis.z * sin,
            w: cos,
        })
    }
    pub fn normalized(self) -> Result<Self, Math3dError> {
        let length = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if !length.is_finite() || length <= f32::EPSILON {
            return Err(Math3dError::ZeroLengthQuaternion);
        }
        Ok(Self {
            x: self.x / length,
            y: self.y / length,
            z: self.z / length,
            w: self.w / length,
        })
    }
    /// Returns whether the quaternion is unit length within `1e-3` tolerance.
    #[must_use]
    pub fn is_unit(self) -> bool {
        let length_squared = self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w;
        length_squared.is_finite() && (length_squared - 1.0).abs() < 1e-3
    }
    #[must_use]
    pub fn conjugate(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }
    pub fn rotate(self, vector: Vec3) -> Result<Vec3, Math3dError> {
        let q = self.normalized()?;
        let qv = Vec3::new(q.x, q.y, q.z);
        Ok(vector + qv.cross(vector) * (2.0 * q.w) + qv.cross(qv.cross(vector)) * 2.0)
    }
}

impl std::ops::Mul for Quat {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Self {
            w: self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
            x: self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            y: self.w * rhs.y - self.x * rhs.z + self.y * rhs.w + self.z * rhs.x,
            z: self.w * rhs.z + self.x * rhs.y - self.y * rhs.x + self.z * rhs.w,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    pub columns: [[f32; 4]; 4],
}

impl Mat4 {
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            columns: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }
    #[must_use]
    pub fn translation(value: Vec3) -> Self {
        let mut result = Self::identity();
        result.columns[3] = [value.x, value.y, value.z, 1.0];
        result
    }
    #[must_use]
    pub fn scale(value: Vec3) -> Self {
        Self {
            columns: [
                [value.x, 0.0, 0.0, 0.0],
                [0.0, value.y, 0.0, 0.0],
                [0.0, 0.0, value.z, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }
    #[must_use]
    pub fn from_quat(rotation: Quat) -> Self {
        let q = rotation.normalized().unwrap_or(Quat::identity());
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);
        Self {
            columns: [
                [
                    1.0 - 2.0 * (y * y + z * z),
                    2.0 * (x * y + z * w),
                    2.0 * (x * z - y * w),
                    0.0,
                ],
                [
                    2.0 * (x * y - z * w),
                    1.0 - 2.0 * (x * x + z * z),
                    2.0 * (y * z + x * w),
                    0.0,
                ],
                [
                    2.0 * (x * z + y * w),
                    2.0 * (y * z - x * w),
                    1.0 - 2.0 * (x * x + y * y),
                    0.0,
                ],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }
    #[must_use]
    fn multiply(self, rhs: Self) -> Self {
        let mut result = [[0.0; 4]; 4];
        for (column, result_column) in result.iter_mut().enumerate() {
            for (row, result_value) in result_column.iter_mut().enumerate() {
                *result_value = (0..4)
                    .map(|index| self.columns[index][row] * rhs.columns[column][index])
                    .sum();
            }
        }
        Self { columns: result }
    }
    pub fn transform_point(self, point: Vec3) -> Result<Vec3, Math3dError> {
        let x = self.columns[0][0] * point.x
            + self.columns[1][0] * point.y
            + self.columns[2][0] * point.z
            + self.columns[3][0];
        let y = self.columns[0][1] * point.x
            + self.columns[1][1] * point.y
            + self.columns[2][1] * point.z
            + self.columns[3][1];
        let z = self.columns[0][2] * point.x
            + self.columns[1][2] * point.y
            + self.columns[2][2] * point.z
            + self.columns[3][2];
        let w = self.columns[0][3] * point.x
            + self.columns[1][3] * point.y
            + self.columns[2][3] * point.z
            + self.columns[3][3];
        if !w.is_finite() || w.abs() <= f32::EPSILON {
            return Err(Math3dError::InvalidHomogeneousCoordinate);
        }
        Ok(Vec3::new(x / w, y / w, z / w))
    }
    pub fn inverse(self) -> Result<Self, Math3dError> {
        let mut augmented = [[0.0f32; 8]; 4];
        for row in 0..4 {
            for (column, value) in augmented[row][..4].iter_mut().enumerate() {
                *value = self.columns[column][row];
            }
            augmented[row][row + 4] = 1.0;
        }
        for pivot in 0..4 {
            let mut best = pivot;
            for row in pivot + 1..4 {
                if augmented[row][pivot].abs() > augmented[best][pivot].abs() {
                    best = row;
                }
            }
            if augmented[best][pivot].abs() <= 1e-6 {
                return Err(Math3dError::SingularMatrix);
            }
            augmented.swap(pivot, best);
            let divisor = augmented[pivot][pivot];
            for value in augmented[pivot].iter_mut() {
                *value /= divisor;
            }
            for row in 0..4 {
                if row != pivot {
                    let factor = augmented[row][pivot];
                    let pivot_row = augmented[pivot];
                    for (value, pivot_value) in augmented[row].iter_mut().zip(pivot_row) {
                        *value -= factor * pivot_value;
                    }
                }
            }
        }
        let mut inverse = [[0.0; 4]; 4];
        for row in 0..4 {
            for column in 0..4 {
                inverse[column][row] = augmented[row][column + 4];
            }
        }
        Ok(Self { columns: inverse })
    }
}

impl std::ops::Mul for Mat4 {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        self.multiply(rhs)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Math3dError {
    NonFinite,
    ZeroLengthVector,
    ZeroLengthQuaternion,
    SingularMatrix,
    InvalidHomogeneousCoordinate,
}
impl fmt::Display for Math3dError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "3D math error: {self:?}")
    }
}
impl Error for Math3dError {}

#[cfg(test)]
mod tests {
    use super::*;
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }
    #[test]
    fn aabb_bounds_merge_and_build_sphere() {
        let bounds =
            Aabb3::from_points(&[Vec3::new(-2.0, 1.0, 3.0), Vec3::new(4.0, -1.0, 5.0)]).unwrap();
        assert_eq!(bounds.min, Vec3::new(-2.0, -1.0, 3.0));
        assert_eq!(bounds.max, Vec3::new(4.0, 1.0, 5.0));
        assert_eq!(bounds.center(), Vec3::new(1.0, 0.0, 4.0));
        assert!(close(
            bounds.bounding_sphere().radius,
            (44.0f32).sqrt() * 0.5
        ));
        assert!(Aabb3::from_points(&[Vec3::new(f32::NAN, 0.0, 0.0)]).is_none());
        assert!(Aabb3::new(Vec3::X, Vec3::ZERO).is_err());
    }
    #[test]
    fn vector_cross_product_is_right_handed() {
        assert_eq!(Vec3::X.cross(Vec3::Y), Vec3::Z);
    }
    #[test]
    fn quaternion_rotates_around_y() {
        let q = Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2).unwrap();
        let rotated = q.rotate(Vec3::Z).unwrap();
        assert!(close(rotated.x, 1.0) && close(rotated.z, 0.0));
    }
    #[test]
    fn matrix_composition_and_inverse_round_trip() {
        let matrix =
            Mat4::translation(Vec3::new(2.0, 3.0, -4.0)) * Mat4::scale(Vec3::new(2.0, 3.0, 4.0));
        let point = matrix.transform_point(Vec3::new(1.0, 1.0, 1.0)).unwrap();
        assert_eq!(point, Vec3::new(4.0, 6.0, 0.0));
        let recovered = matrix.inverse().unwrap().transform_point(point).unwrap();
        assert!(close(recovered.x, 1.0) && close(recovered.y, 1.0) && close(recovered.z, 1.0));
    }
    #[test]
    fn singular_matrix_is_rejected() {
        assert_eq!(
            Mat4::scale(Vec3::new(1.0, 0.0, 1.0)).inverse(),
            Err(Math3dError::SingularMatrix)
        );
    }
}
