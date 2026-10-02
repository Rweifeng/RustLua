//! 3D 几何基元：射线、视锥与平面。
//!
//! 这些类型此前住在 `core::camera` 里，但它们与相机**无关**——射线求交、Gribb–Hartmann
//! 平面提取、包围球剔除都是纯几何，消费方是拾取（`core::picking`）、编辑器 gizmo 与渲染剔除。
//! 放在相机模块里会让"改几何"看起来像"改相机"，也让 `camera.rs` 承担了不属于它的职责。
//!
//! 迁出后 `core::camera` 仍保留兼容 re-export（`core::camera::Ray3` 等路径继续有效），
//! 新代码请直接 `use crate::geom3d::…`。

use crate::math3d::{Aabb3, Mat4, Math3dError, Vec3};

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

/// A world-space view frustum, derived from a view-projection matrix.
///
/// 字段私有：视锥只能由 [`Frustum::from_view_projection`] 构造，因此"视锥总是某份
/// 视图投影矩阵的导出物"由类型保证（此前 `Camera3D::frustum` 直接写这个字段，
/// 只是因为 `Frustum` 与 `Camera3D` 同处一个模块）。
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
    /// 从**视图投影矩阵**构造（世界 → 裁剪空间，深度范围 `0..1`，与 2D/3D 同一约定）。
    #[must_use]
    pub const fn from_view_projection(view_projection: Mat4) -> Self {
        Self { view_projection }
    }

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
