//! Entity world and type-erased component storage.

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
};

use crate::core::entity::{EntityAllocator, EntityId};

/// Errors returned by world operations that require a live entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldError {
    /// The entity handle is stale or was never allocated by this world.
    InvalidEntity(EntityId),
}

impl fmt::Display for WorldError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEntity(entity) => write!(formatter, "invalid entity handle: {entity}"),
        }
    }
}

impl std::error::Error for WorldError {}

trait ComponentStorage: Any {
    fn remove_entity(&mut self, entity: EntityId);
    fn contains_entity(&self, entity: EntityId) -> bool;
    fn entity_ids(&self) -> Vec<EntityId>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

struct TypedStorage<T> {
    values: HashMap<EntityId, T>,
    entities: Vec<EntityId>,
}

impl<T: 'static> ComponentStorage for TypedStorage<T> {
    fn remove_entity(&mut self, entity: EntityId) {
        if self.values.remove(&entity).is_some()
            && let Ok(index) = self.entities.binary_search(&entity)
        {
            self.entities.remove(index);
        }
    }

    fn contains_entity(&self, entity: EntityId) -> bool {
        self.values.contains_key(&entity)
    }

    fn entity_ids(&self) -> Vec<EntityId> {
        self.entities.clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Owns entity lifetimes and the component stores attached to them.
#[derive(Default)]
pub struct World {
    entities: EntityAllocator,
    components: HashMap<TypeId, Box<dyn ComponentStorage>>,
}

impl fmt::Debug for World {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("World")
            .field("entity_count", &self.entities.len())
            .field("component_type_count", &self.components.len())
            .finish()
    }
}

impl World {
    /// Creates an empty world.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new entity without components.
    pub fn spawn(&mut self) -> EntityId {
        self.entities.spawn()
    }

    /// Destroys an entity and removes all components attached to it.
    pub fn despawn(&mut self, entity: EntityId) -> Result<(), WorldError> {
        if !self.entities.despawn(entity) {
            return Err(WorldError::InvalidEntity(entity));
        }
        for storage in self.components.values_mut() {
            storage.remove_entity(entity);
        }
        Ok(())
    }

    /// Returns whether an entity handle is currently alive in this world.
    #[must_use]
    pub fn is_alive(&self, entity: EntityId) -> bool {
        self.entities.is_alive(entity)
    }

    /// Returns the number of live entities.
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Inserts or replaces a component and returns the previous value, if any.
    pub fn insert<T: 'static>(
        &mut self,
        entity: EntityId,
        component: T,
    ) -> Result<Option<T>, WorldError> {
        self.ensure_alive(entity)?;
        let storage = self.components.entry(TypeId::of::<T>()).or_insert_with(|| {
            Box::new(TypedStorage::<T> {
                values: HashMap::new(),
                entities: Vec::new(),
            })
        });
        let typed = storage
            .as_any_mut()
            .downcast_mut::<TypedStorage<T>>()
            .expect("component storage type id must match its concrete storage");
        let previous = typed.values.insert(entity, component);
        if previous.is_none() {
            match typed.entities.binary_search(&entity) {
                Ok(_) => {}
                Err(index) => typed.entities.insert(index, entity),
            }
        }
        Ok(previous)
    }

    /// Reads a component attached to an entity.
    pub fn get<T: 'static>(&self, entity: EntityId) -> Result<Option<&T>, WorldError> {
        self.ensure_alive(entity)?;
        Ok(self
            .components
            .get(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any().downcast_ref::<TypedStorage<T>>())
            .and_then(|storage| storage.values.get(&entity)))
    }

    /// Mutably reads a component attached to an entity.
    pub fn get_mut<T: 'static>(&mut self, entity: EntityId) -> Result<Option<&mut T>, WorldError> {
        self.ensure_alive(entity)?;
        Ok(self
            .components
            .get_mut(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any_mut().downcast_mut::<TypedStorage<T>>())
            .and_then(|storage| storage.values.get_mut(&entity)))
    }

    /// Removes a component and returns its value, if it was present.
    pub fn remove<T: 'static>(&mut self, entity: EntityId) -> Result<Option<T>, WorldError> {
        self.ensure_alive(entity)?;
        Ok(self
            .components
            .get_mut(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any_mut().downcast_mut::<TypedStorage<T>>())
            .and_then(|storage| storage.values.remove(&entity)))
    }

    /// Returns whether an entity has a component of type `T`.
    pub fn has<T: 'static>(&self, entity: EntityId) -> Result<bool, WorldError> {
        self.ensure_alive(entity)?;
        Ok(self
            .components
            .get(&TypeId::of::<T>())
            .is_some_and(|storage| storage.contains_entity(entity)))
    }

    /// Returns all live entities that have component `T`.
    pub fn entities_with<T: 'static>(&self) -> Vec<EntityId> {
        self.components
            .get(&TypeId::of::<T>())
            .map_or_else(Vec::new, |storage| storage.entity_ids())
    }

    /// Returns all live entities that have both components `T` and `U`.
    pub fn entities_with2<T: 'static, U: 'static>(&self) -> Vec<EntityId> {
        let mut entities = self.entities_with::<T>();
        entities.retain(|entity| {
            self.components
                .get(&TypeId::of::<U>())
                .is_some_and(|storage| storage.contains_entity(*entity))
        });
        entities
    }

    /// Iterates all live entities with component `T` in sorted entity order.
    pub fn for_each<T: 'static>(&self, mut f: impl FnMut(EntityId, &T)) {
        let Some(storage) = self
            .components
            .get(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any().downcast_ref::<TypedStorage<T>>())
        else {
            return;
        };
        for &entity in &storage.entities {
            if let Some(component) = storage.values.get(&entity) {
                f(entity, component);
            }
        }
    }

    /// Iterates all live entities with component `T` mutably in sorted entity order.
    pub fn for_each_mut<T: 'static>(&mut self, mut f: impl FnMut(EntityId, &mut T)) {
        let Some(storage) = self
            .components
            .get_mut(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any_mut().downcast_mut::<TypedStorage<T>>())
        else {
            return;
        };
        let entities = storage.entities.clone();
        for entity in entities {
            if let Some(component) = storage.values.get_mut(&entity) {
                f(entity, component);
            }
        }
    }

    /// Iterates all live entities that have both components `T` and `U`.
    pub fn for_each2<T: 'static, U: 'static>(&self, mut f: impl FnMut(EntityId, &T, &U)) {
        let Some(left) = self
            .components
            .get(&TypeId::of::<T>())
            .and_then(|storage| storage.as_any().downcast_ref::<TypedStorage<T>>())
        else {
            return;
        };
        let Some(right) = self
            .components
            .get(&TypeId::of::<U>())
            .and_then(|storage| storage.as_any().downcast_ref::<TypedStorage<U>>())
        else {
            return;
        };
        if left.entities.len() <= right.entities.len() {
            for &entity in &left.entities {
                if let Some(other) = right.values.get(&entity)
                    && let Some(component) = left.values.get(&entity)
                {
                    f(entity, component, other);
                }
            }
        } else {
            for &entity in &right.entities {
                if let Some(component) = left.values.get(&entity)
                    && let Some(other) = right.values.get(&entity)
                {
                    f(entity, component, other);
                }
            }
        }
    }

    fn ensure_alive(&self, entity: EntityId) -> Result<(), WorldError> {
        self.is_alive(entity)
            .then_some(())
            .ok_or(WorldError::InvalidEntity(entity))
    }
}

#[cfg(test)]
mod tests {
    use super::{World, WorldError};
    use crate::core::entity::EntityId;

    #[derive(Debug, PartialEq)]
    struct Position {
        x: f32,
        y: f32,
    }

    #[derive(Debug, PartialEq)]
    struct Health(u32);

    #[test]
    fn creates_and_destroys_entities() {
        let mut world = World::new();
        let entity = world.spawn();
        assert!(world.is_alive(entity));
        assert_eq!(world.entity_count(), 1);
        assert_eq!(world.despawn(entity), Ok(()));
        assert!(!world.is_alive(entity));
        assert_eq!(world.entity_count(), 0);
        assert_eq!(
            world.despawn(entity),
            Err(WorldError::InvalidEntity(entity))
        );
    }

    #[test]
    fn inserts_reads_modifies_and_removes_components() {
        let mut world = World::new();
        let entity = world.spawn();
        assert_eq!(world.insert(entity, Position { x: 1.0, y: 2.0 }), Ok(None));
        assert_eq!(
            world.get(entity).unwrap(),
            Some(&Position { x: 1.0, y: 2.0 })
        );
        assert_eq!(
            world.insert(entity, Position { x: 3.0, y: 4.0 }),
            Ok(Some(Position { x: 1.0, y: 2.0 }))
        );
        world.get_mut::<Position>(entity).unwrap().unwrap().x = 9.0;
        assert_eq!(world.get::<Position>(entity).unwrap().unwrap().x, 9.0);
        assert!(world.has::<Position>(entity).unwrap());
        assert_eq!(
            world.remove::<Position>(entity),
            Ok(Some(Position { x: 9.0, y: 4.0 }))
        );
        assert!(!world.has::<Position>(entity).unwrap());
        assert_eq!(world.get::<Position>(entity).unwrap(), None);
    }

    #[test]
    fn despawn_removes_all_components() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
        world.insert(entity, Health(100)).unwrap();
        world.despawn(entity).unwrap();
        assert!(!world.entities_with::<Position>().contains(&entity));
        assert!(!world.entities_with::<Health>().contains(&entity));
        assert_eq!(
            world.get::<Position>(entity),
            Err(WorldError::InvalidEntity(entity))
        );
    }

    #[test]
    fn queries_component_combinations_in_sorted_entity_order() {
        let mut world = World::new();
        let first = world.spawn();
        let second = world.spawn();
        let third = world.spawn();
        world.insert(first, Position { x: 0.0, y: 0.0 }).unwrap();
        world.insert(second, Position { x: 1.0, y: 0.0 }).unwrap();
        world.insert(second, Health(10)).unwrap();
        world.insert(third, Health(20)).unwrap();
        assert_eq!(world.entities_with::<Position>(), vec![first, second]);
        assert_eq!(world.entities_with2::<Position, Health>(), vec![second]);
    }

    #[test]
    fn stale_handles_are_rejected_after_slot_reuse() {
        let mut world = World::new();
        let old_entity = world.spawn();
        world.insert(old_entity, Health(1)).unwrap();
        world.despawn(old_entity).unwrap();
        let new_entity = world.spawn();
        assert_ne!(old_entity, new_entity);
        assert_eq!(
            world.insert(old_entity, Health(2)),
            Err(WorldError::InvalidEntity(old_entity))
        );
        assert_eq!(world.get::<Health>(new_entity).unwrap(), None);
    }

    #[test]
    fn arbitrary_invalid_handles_are_rejected() {
        let world = World::new();
        let invalid = EntityId::from_raw(99, 1);
        assert_eq!(
            world.has::<Health>(invalid),
            Err(WorldError::InvalidEntity(invalid))
        );
    }
}
