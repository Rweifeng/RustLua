//! Transform and velocity components plus the minimal movement system.

use std::{error::Error, fmt};

use crate::{
    entity::EntityId,
    world::{World, WorldError},
};

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
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransformSystemError {
    /// A simulation step must be finite and non-negative.
    InvalidDeltaTime(f32),
    /// The requested entity was invalid while applying an update.
    InvalidEntity(EntityId),
}

impl fmt::Display for TransformSystemError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDeltaTime(delta_seconds) => {
                write!(formatter, "invalid transform delta time: {delta_seconds}")
            }
            Self::InvalidEntity(entity) => write!(formatter, "invalid entity: {entity}"),
        }
    }
}

impl Error for TransformSystemError {}

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

#[cfg(test)]
mod tests {
    use super::{Transform, TransformSystemError, Vec2, Velocity, integrate_velocity};
    use crate::world::World;

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
