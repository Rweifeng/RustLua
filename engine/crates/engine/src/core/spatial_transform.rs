//! WP2 prototype: unified local/global transforms and parent-first propagation.

use std::{collections::BTreeMap, fmt};

use crate::core::{
    math3d::{Mat4, Math3dError, Quat, Vec3},
    scene::SceneEntityId,
    transform::Transform,
    transform3d::Transform3D,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalTransform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl LocalTransform {
    pub const fn identity() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::identity(),
            scale: Vec3::new(1.0, 1.0, 1.0),
        }
    }

    pub fn from_2d(value: Transform) -> Result<Self, Math3dError> {
        Ok(Self {
            translation: Vec3::new(value.position.x, value.position.y, 0.0),
            rotation: Quat::from_axis_angle(Vec3::Z, value.rotation)?,
            scale: Vec3::new(value.scale.x, value.scale.y, 1.0),
        })
    }

    pub const fn from_3d(value: Transform3D) -> Self {
        Self {
            translation: value.position,
            rotation: value.rotation,
            scale: value.scale,
        }
    }

    pub fn matrix(self) -> Result<Mat4, Math3dError> {
        if !self.translation.is_finite()
            || !self.scale.is_finite()
            || self.scale.x <= 0.0
            || self.scale.y <= 0.0
            || self.scale.z <= 0.0
            || !self.rotation.is_unit()
        {
            return Err(Math3dError::NonFinite);
        }
        Ok(Mat4::translation(self.translation)
            * Mat4::from_quat(self.rotation)
            * Mat4::scale(self.scale))
    }
}

impl Default for LocalTransform {
    fn default() -> Self {
        Self::identity()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalTransform(pub Mat4);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PropagationError {
    MissingParent {
        entity: SceneEntityId,
        parent: SceneEntityId,
    },
    Cycle(SceneEntityId),
    InvalidLocal(SceneEntityId),
}

impl fmt::Display for PropagationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingParent { entity, parent } => {
                write!(formatter, "entity {} missing parent {}", entity.0, parent.0)
            }
            Self::Cycle(entity) => write!(formatter, "transform parent cycle at {}", entity.0),
            Self::InvalidLocal(entity) => {
                write!(formatter, "invalid local transform at {}", entity.0)
            }
        }
    }
}

/// Correctness-first propagation prototype. Incremental dirty/version tracking
/// is deliberately deferred until benchmark evidence exists.
pub fn propagate_all(
    locals: &BTreeMap<SceneEntityId, LocalTransform>,
    parents: &BTreeMap<SceneEntityId, SceneEntityId>,
) -> Result<BTreeMap<SceneEntityId, GlobalTransform>, PropagationError> {
    let mut globals = BTreeMap::new();
    let mut state = BTreeMap::new();
    fn visit(
        entity: SceneEntityId,
        locals: &BTreeMap<SceneEntityId, LocalTransform>,
        parents: &BTreeMap<SceneEntityId, SceneEntityId>,
        globals: &mut BTreeMap<SceneEntityId, GlobalTransform>,
        state: &mut BTreeMap<SceneEntityId, u8>,
    ) -> Result<Mat4, PropagationError> {
        match state.get(&entity).copied() {
            Some(2) => return Ok(globals[&entity].0),
            Some(1) => return Err(PropagationError::Cycle(entity)),
            _ => {}
        }
        state.insert(entity, 1);
        let local = locals
            .get(&entity)
            .ok_or(PropagationError::InvalidLocal(entity))?
            .matrix()
            .map_err(|_| PropagationError::InvalidLocal(entity))?;
        let global = if let Some(parent) = parents.get(&entity).copied() {
            if !locals.contains_key(&parent) {
                return Err(PropagationError::MissingParent { entity, parent });
            }
            visit(parent, locals, parents, globals, state)? * local
        } else {
            local
        };
        globals.insert(entity, GlobalTransform(global));
        state.insert(entity, 2);
        Ok(global)
    }
    for entity in locals.keys().copied() {
        visit(entity, locals, parents, &mut globals, &mut state)?;
    }
    Ok(globals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propagates_depth_100_parent_first() {
        let mut locals = BTreeMap::new();
        let mut parents = BTreeMap::new();
        for index in 0..100_u64 {
            let id = SceneEntityId(index + 1);
            locals.insert(
                id,
                LocalTransform {
                    translation: Vec3::new(1.0, 0.0, 0.0),
                    ..LocalTransform::identity()
                },
            );
            if index > 0 {
                parents.insert(id, SceneEntityId(index));
            }
        }
        let globals = propagate_all(&locals, &parents).unwrap();
        let point = globals[&SceneEntityId(100)]
            .0
            .transform_point(Vec3::ZERO)
            .unwrap();
        assert!((point.x - 100.0).abs() < 1e-4);
    }

    #[test]
    fn detects_cycles_and_missing_parents() {
        let mut locals = BTreeMap::new();
        locals.insert(SceneEntityId(1), LocalTransform::identity());
        locals.insert(SceneEntityId(2), LocalTransform::identity());
        let cycle = BTreeMap::from([
            (SceneEntityId(1), SceneEntityId(2)),
            (SceneEntityId(2), SceneEntityId(1)),
        ]);
        assert!(matches!(
            propagate_all(&locals, &cycle),
            Err(PropagationError::Cycle(_))
        ));
        let missing = BTreeMap::from([(SceneEntityId(1), SceneEntityId(9))]);
        assert!(matches!(
            propagate_all(&locals, &missing),
            Err(PropagationError::MissingParent { .. })
        ));
    }

    #[test]
    fn converts_2d_into_xy_z_rotation_without_losing_scale() {
        let local = LocalTransform::from_2d(Transform {
            position: crate::core::transform::Vec2::new(3.0, 4.0),
            rotation: std::f32::consts::FRAC_PI_2,
            scale: crate::core::transform::Vec2::new(2.0, 5.0),
        })
        .unwrap();
        assert_eq!(local.translation, Vec3::new(3.0, 4.0, 0.0));
        assert_eq!(local.scale, Vec3::new(2.0, 5.0, 1.0));
        let rotated = local.rotation.rotate(Vec3::X).unwrap();
        assert!(rotated.x.abs() < 1e-5 && (rotated.y - 1.0).abs() < 1e-5);
    }
}
