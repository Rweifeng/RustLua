//! 统一 Transform（WP2/WP5 同族收敛）：2D 的 `Transform`/`Vec2`/`Velocity` 与 3D 的
//! `Transform3D` 现在同址，并提供 2D ↔ 3D 的无损互转。
//!
//! 收敛前 2D 在 `transform.rs`、3D 在 `transform3d.rs`，两边的解析错误类型、组件串格式与
//! "谁有权写变换"的说法各一套；现在它们只在一个模块里，互转规则也写在同一处。

use crate::core::{
    entity::EntityId,
    math3d::{Mat4, Math3dError, Quat, Vec3},
    world::{World, WorldError},
};

/// Errors while parsing the engine's scene Transform component string.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TransformParseError {
    /// A field was not `key=number` as the scene format requires.
    #[error("malformed transform field: {0}")]
    MalformedField(String),
    /// A required field (`x`, `y`, `rotation`, `scale_x`, `scale_y`) was missing.
    #[error("missing transform field: {0}")]
    MissingField(&'static str),
    /// A numeric field could not be parsed or was not finite.
    #[error("transform contains a non-finite number")]
    NonFinite,
    /// A scale component was zero or negative.
    #[error("transform scale must be positive")]
    NonPositiveScale,
}

/// A two-dimensional vector used by the minimal 2D gameplay components.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    /// Creates a vector from its two coordinates.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Returns the zero vector.
    #[must_use]
    pub const fn zero() -> Self {
        Self::new(0.0, 0.0)
    }

    /// Length (magnitude).
    #[must_use]
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    /// Dot product.
    #[must_use]
    pub const fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
    }

    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

// 2D 向量此前没有任何算术运算，导致上层到处手写分量相加。补齐最小集合；纯增量，不影响
// 既有调用方。
impl std::ops::Add for Vec2 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl std::ops::Neg for Vec2 {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self::new(-self.x, -self.y)
    }
}

/// Position, rotation and scale data attached to an entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: Vec2,
    pub rotation: f32,
    pub scale: Vec2,
}

impl Default for Transform {
    fn default() -> Self {
        Self::identity()
    }
}

impl Transform {
    /// Creates an identity transform at the origin.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            position: Vec2::zero(),
            rotation: 0.0,
            scale: Vec2::new(1.0, 1.0),
        }
    }

    /// Creates a transform with a position and identity rotation/scale.
    #[must_use]
    pub const fn from_position(position: Vec2) -> Self {
        Self {
            position,
            ..Self::identity()
        }
    }

    /// Encodes the component back to the scene/editor string format
    /// (`x=..;y=..;rotation=..;scale_x=..;scale_y=..`，与编辑器的 9 位小数写法一致）。
    ///
    /// WP2 迁移需要与 3D 的 `Transform3D::to_component_str` 对称的编码器：把"世界矩阵的逆
    /// 烘进局部变换"的结果写回场景时，格式必须与编辑器落盘的一致，否则每次迁移都会产生无谓
    /// 的文本差异。
    #[must_use]
    pub fn to_component_str(self) -> String {
        format!(
            "x={:.9};y={:.9};rotation={:.9};scale_x={:.9};scale_y={:.9}",
            self.position.x, self.position.y, self.rotation, self.scale.x, self.scale.y
        )
    }

    /// Parses the scene component string written by the editor:
    /// `x=..;y=..;rotation=..;scale_x=..;scale_y=..`.
    pub fn from_component_str(value: &str) -> Result<Self, TransformParseError> {
        let mut transform = Self::identity();
        let mut seen = [false; 5];
        for field in value.split(';') {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| TransformParseError::MalformedField(field.to_owned()))?;
            let number: f32 = raw.parse().map_err(|_| TransformParseError::NonFinite)?;
            if !number.is_finite() {
                return Err(TransformParseError::NonFinite);
            }
            match key {
                "x" => {
                    transform.position.x = number;
                    seen[0] = true;
                }
                "y" => {
                    transform.position.y = number;
                    seen[1] = true;
                }
                "rotation" => {
                    transform.rotation = number;
                    seen[2] = true;
                }
                "scale_x" => {
                    transform.scale.x = number;
                    seen[3] = true;
                }
                "scale_y" => {
                    transform.scale.y = number;
                    seen[4] = true;
                }
                _ => return Err(TransformParseError::MalformedField(field.to_owned())),
            }
        }
        for (field, present) in [
            ("x", seen[0]),
            ("y", seen[1]),
            ("rotation", seen[2]),
            ("scale_x", seen[3]),
            ("scale_y", seen[4]),
        ] {
            if !present {
                return Err(TransformParseError::MissingField(field));
            }
        }
        if transform.scale.x <= 0.0 || transform.scale.y <= 0.0 {
            return Err(TransformParseError::NonPositiveScale);
        }
        Ok(transform)
    }
}

/// Linear and angular velocity attached to an entity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Velocity {
    pub linear: Vec2,
    pub angular: f32,
}

impl Velocity {
    /// Creates a velocity from linear and angular components.
    #[must_use]
    pub const fn new(linear: Vec2, angular: f32) -> Self {
        Self { linear, angular }
    }
}

/// Errors returned by the transform integration system.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum TransformSystemError {
    /// A simulation step must be finite and non-negative.
    #[error("invalid transform delta time: {0}")]
    InvalidDeltaTime(f32),
    /// The requested entity was invalid while applying an update.
    #[error("invalid entity: {0}")]
    InvalidEntity(EntityId),
}

impl From<WorldError> for TransformSystemError {
    fn from(error: WorldError) -> Self {
        match error {
            WorldError::InvalidEntity(entity) => Self::InvalidEntity(entity),
        }
    }
}

/// Integrates every entity that has both [`Transform`] and [`Velocity`].
///
/// Position and rotation use the same fixed simulation delta. Entities with
/// only one of the two components are intentionally ignored and remain valid
/// partial compositions in the ECS.
pub fn integrate_velocity(
    world: &mut World,
    delta_seconds: f32,
) -> Result<usize, TransformSystemError> {
    if !delta_seconds.is_finite() || delta_seconds < 0.0 {
        return Err(TransformSystemError::InvalidDeltaTime(delta_seconds));
    }

    let entities = world.entities_with2::<Transform, Velocity>();
    let mut updated = 0;
    for entity in entities {
        let velocity = world
            .get::<Velocity>(entity)?
            .copied()
            .ok_or(TransformSystemError::InvalidEntity(entity))?;
        let transform = world
            .get_mut::<Transform>(entity)?
            .ok_or(TransformSystemError::InvalidEntity(entity))?;
        transform.position.x += velocity.linear.x * delta_seconds;
        transform.position.y += velocity.linear.y * delta_seconds;
        transform.rotation += velocity.angular * delta_seconds;
        updated += 1;
    }
    Ok(updated)
}
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Transform3dParseError {
    /// A field was not `key=<vector>` as the scene format requires.
    #[error("malformed Transform3D field: {0}")]
    MalformedField(String),
    /// A required field (`position`/`rotation`/`scale`) was missing.
    #[error("missing Transform3D field: {0}")]
    MissingField(&'static str),
    /// A vector component could not be parsed or was not finite.
    #[error("invalid Transform3D vector: {0}")]
    InvalidVector(String),
    /// A scale component was zero or negative.
    #[error("Transform3D scale must be positive")]
    NonPositiveScale,
    /// The rotation quaternion was not unit length.
    #[error("Transform3D rotation quaternion must be unit length")]
    NonUnitQuaternion,
}
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
/// 2D ↔ 3D 变换互转（WP2/WP4 ① 要求的"统一 Transform 与 XY 便利 API"的第一步）。
///
/// 两个方向刻意不对称，因为**信息量不对称**：
///
/// - 2D → 3D 永远无损：z = 0、绕 Z 旋转、z 缩放 = 1；
/// - 3D → 2D 只在**真能无损压平**时才给出 `Some`——必须是纯 Z 旋转（四元数只有 z/w 分量）、
///   z 位移为 0、且 z 缩放不参与 XY。带倾斜、绕 X/Y 旋转或带 z 位移的 3D 变换返回 `None`，
///   让调用方显式处理"压不下去"，而不是悄悄丢掉一个分量（与本项目"不静默近似"的一贯做法一致）。
impl Transform {
    /// 把 2D 变换提升为 3D（z = 0、绕 Z 旋转、z 缩放 = 1），**无损**。
    ///
    /// 绕 Z 的四元数直接按半角构造（`(0, 0, sin(θ/2), cos(θ/2))`），不经过
    /// `from_axis_angle`：后者对非有限角度会报错，而这里的 2D 旋转本来就允许是任意有限值，
    /// 也不该为了"构造四元数"再引入一条失败路径。
    #[must_use]
    pub fn to_3d(self) -> Transform3D {
        let (sin, cos) = (self.rotation * 0.5).sin_cos();
        Transform3D {
            position: Vec3::new(self.position.x, self.position.y, 0.0),
            rotation: Quat {
                x: 0.0,
                y: 0.0,
                z: sin,
                w: cos,
            },
            scale: Vec3::new(self.scale.x, self.scale.y, 1.0),
        }
    }
}

impl Transform3D {
    /// 把 3D 变换压回 2D；**只在无损可压平时**返回 `Some`（见上方说明）。
    #[must_use]
    pub fn to_2d(self) -> Option<Transform> {
        // z 位移必须为 0，z 缩放必须为 1（否则压平会丢信息）。
        if self.position.z != 0.0 || self.scale.z != 1.0 {
            return None;
        }
        // 只接受绕 Z 的纯旋转：四元数的 x/y 分量必须为 0（w 可正可负，负号表示同一个旋转）。
        if self.rotation.x != 0.0 || self.rotation.y != 0.0 {
            return None;
        }
        // 退化四元数（全 0）没有对应的旋转，不能当 0 度用——直接判定压不平。
        if self.rotation.z == 0.0 && self.rotation.w == 0.0 {
            return None;
        }
        Some(Transform {
            position: Vec2::new(self.position.x, self.position.y),
            // 绕 Z 的四元数：θ = 2·atan2(z, w)。
            rotation: 2.0 * self.rotation.z.atan2(self.rotation.w),
            scale: Vec2::new(self.scale.x, self.scale.y),
        })
    }

    /// 是否可以被 [`Transform3D::to_2d`] 无损压平。
    #[must_use]
    pub fn is_planar(self) -> bool {
        self.to_2d().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Transform, TransformParseError, TransformSystemError, Vec2, Velocity, integrate_velocity,
    };
    use crate::core::world::World;

    #[test]
    fn parses_editor_component_string() {
        let transform = Transform::from_component_str(
            "x=-55.000000000;y=3.500000000;rotation=0.250000000;scale_x=2.000000000;scale_y=1.000000000",
        )
        .unwrap();
        assert_eq!(
            transform,
            Transform {
                position: Vec2::new(-55.0, 3.5),
                rotation: 0.25,
                scale: Vec2::new(2.0, 1.0),
            }
        );
    }

    #[test]
    fn rejects_incomplete_or_invalid_component_strings() {
        assert!(matches!(
            Transform::from_component_str("x=1;y=2;rotation=0;scale_x=1"),
            Err(TransformParseError::MissingField("scale_y"))
        ));
        assert!(matches!(
            Transform::from_component_str("x=1;y=2;rotation=0;scale_x=1;scale_y=0"),
            Err(TransformParseError::NonPositiveScale)
        ));
        assert!(matches!(
            Transform::from_component_str("x=1;y=2;rotation=0;scale_x=1;scale_y=1;extra=1"),
            Err(TransformParseError::MalformedField(_))
        ));
        assert!(matches!(
            Transform::from_component_str("x=nan;y=2;rotation=0;scale_x=1;scale_y=1"),
            Err(TransformParseError::NonFinite)
        ));
    }

    #[test]
    fn identity_transform_has_expected_defaults() {
        assert_eq!(Transform::default(), Transform::identity());
        assert_eq!(Transform::identity().position, Vec2::zero());
        assert_eq!(Transform::identity().scale, Vec2::new(1.0, 1.0));
    }

    #[test]
    fn integrates_linear_and_angular_velocity() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Transform::from_position(Vec2::new(10.0, -2.0)))
            .unwrap();
        world
            .insert(entity, Velocity::new(Vec2::new(3.0, 4.0), 2.0))
            .unwrap();

        assert_eq!(integrate_velocity(&mut world, 0.5), Ok(1));
        assert_eq!(
            world.get::<Transform>(entity).unwrap(),
            Some(&Transform {
                position: Vec2::new(11.5, 0.0),
                rotation: 1.0,
                scale: Vec2::new(1.0, 1.0),
            })
        );
    }

    #[test]
    fn ignores_entities_with_incomplete_components() {
        let mut world = World::new();
        let transform_only = world.spawn();
        world.insert(transform_only, Transform::default()).unwrap();
        let velocity_only = world.spawn();
        world
            .insert(velocity_only, Velocity::new(Vec2::new(1.0, 0.0), 0.0))
            .unwrap();

        assert_eq!(integrate_velocity(&mut world, 1.0), Ok(0));
        assert_eq!(
            world.get::<Transform>(transform_only).unwrap(),
            Some(&Transform::default())
        );
    }

    #[test]
    fn rejects_invalid_delta_time() {
        let mut world = World::new();
        assert_eq!(
            integrate_velocity(&mut world, -0.1),
            Err(TransformSystemError::InvalidDeltaTime(-0.1))
        );
        assert!(integrate_velocity(&mut world, f32::NAN).is_err());
        assert!(integrate_velocity(&mut world, f32::INFINITY).is_err());
    }

    #[test]
    fn despawned_entities_are_not_updated() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Transform::default()).unwrap();
        world.insert(entity, Velocity::default()).unwrap();
        world.despawn(entity).unwrap();

        assert_eq!(integrate_velocity(&mut world, 1.0), Ok(0));
    }
}

/// WP2/WP4 ① 的"统一 Transform 与 XY 便利 API"：2D ↔ 3D 互转。
///
/// 关键不是"能转"，而是**两个方向的失败语义不同**：提升永远无损，压平只在真能无损时给 `Some`
/// ——带倾斜、绕 X/Y 旋转、带 z 位移或 z 缩放的 3D 变换必须被拒绝，而不是悄悄丢掉一个分量。
#[cfg(test)]
mod planar_tests {
    use super::*;

    #[test]
    fn planar_transforms_round_trip_between_2d_and_3d() {
        let flat = Transform {
            position: Vec2::new(12.0, -3.5),
            rotation: 0.75,
            scale: Vec2::new(2.0, 0.5),
        };
        let lifted = flat.to_3d();
        assert_eq!(lifted.position, Vec3::new(12.0, -3.5, 0.0));
        assert_eq!(lifted.scale, Vec3::new(2.0, 0.5, 1.0));
        assert!(lifted.is_planar());
        let back = lifted.to_2d().expect("纯 Z 旋转必须能压平");
        assert!((back.position.x - flat.position.x).abs() < 1e-6);
        assert!((back.position.y - flat.position.y).abs() < 1e-6);
        assert!((back.rotation - flat.rotation).abs() < 1e-6);
        assert!((back.scale.x - flat.scale.x).abs() < 1e-6);
        assert!((back.scale.y - flat.scale.y).abs() < 1e-6);

        // 提升后的 3D 模型矩阵在 XY 平面上必须与 2D 变换一致（z 分量保持 0）。
        let matrix = lifted.model_matrix().unwrap();
        let origin = matrix.transform_point(Vec3::new(1.0, 2.0, 0.0)).unwrap();
        let (sin, cos) = flat.rotation.sin_cos();
        let (x, y) = (1.0 * flat.scale.x, 2.0 * flat.scale.y);
        let expected = Vec3::new(
            flat.position.x + x * cos - y * sin,
            flat.position.y + x * sin + y * cos,
            0.0,
        );
        assert!(
            (origin - expected).length() < 1e-4,
            "{origin:?} != {expected:?}"
        );
    }

    #[test]
    fn non_planar_transforms_refuse_to_flatten() {
        let mut tilted = Transform3D::identity();
        tilted.rotation = Quat::from_axis_angle(Vec3::X, 0.3).unwrap();
        assert_eq!(tilted.to_2d(), None, "绕 X 旋转压平会丢信息，必须拒绝");
        assert!(!tilted.is_planar());

        let mut lifted_z = Transform3D::identity();
        lifted_z.position.z = 5.0;
        assert_eq!(lifted_z.to_2d(), None, "z 位移必须拒绝");

        let mut squashed_z = Transform3D::identity();
        squashed_z.scale.z = 0.5;
        assert_eq!(squashed_z.to_2d(), None, "z 缩放必须拒绝");

        let mut degenerate = Transform3D::identity();
        degenerate.rotation = Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 0.0,
        };
        assert_eq!(
            degenerate.to_2d(),
            None,
            "退化四元数没有对应旋转，不能当 0 度用"
        );
    }
}

#[cfg(test)]
mod transform3d_tests {
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
