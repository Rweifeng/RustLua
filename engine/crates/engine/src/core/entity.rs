//! Generational entity handles and a minimal entity allocator.

use std::fmt;

/// A stable handle for an entity.
///
/// The generation prevents a handle from referring to a different entity after
/// the original entity has been destroyed and its slot has been reused.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntityId {
    index: u32,
    generation: u32,
}

impl EntityId {
    /// The generation assigned to the first occupant of a slot.
    pub const INITIAL_GENERATION: u32 = 1;

    /// Creates an entity handle from its raw slot and generation values.
    ///
    /// This is intended for serialization and low-level integrations. Runtime
    /// validity must still be checked by [`EntityAllocator::is_alive`].
    #[must_use]
    pub const fn from_raw(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// Returns the slot index of this handle.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Returns the generation of this handle.
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "EntityId({}:g{})", self.index, self.generation)
    }
}

/// Allocates entity handles and invalidates destroyed generations.
#[derive(Debug, Default)]
pub struct EntityAllocator {
    generations: Vec<u32>,
    free_indices: Vec<u32>,
    alive_count: usize,
}

impl EntityAllocator {
    /// Creates an empty allocator.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            generations: Vec::new(),
            free_indices: Vec::new(),
            alive_count: 0,
        }
    }

    /// Creates a new entity, reusing a vacant slot when possible.
    pub fn spawn(&mut self) -> EntityId {
        let index = match self.free_indices.pop() {
            Some(index) => index,
            None => {
                let index = self.generations.len() as u32;
                self.generations.push(EntityId::INITIAL_GENERATION);
                index
            }
        };
        self.alive_count += 1;
        EntityId::from_raw(index, self.generations[index as usize])
    }

    /// Destroys an entity if its handle is currently alive.
    pub fn despawn(&mut self, entity: EntityId) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        let generation = &mut self.generations[entity.index as usize];
        *generation = next_generation(*generation);
        self.free_indices.push(entity.index);
        self.alive_count -= 1;
        true
    }

    /// Returns whether a handle refers to a currently alive entity.
    #[must_use]
    pub fn is_alive(&self, entity: EntityId) -> bool {
        self.generations
            .get(entity.index as usize)
            .is_some_and(|&generation| generation == entity.generation)
    }

    /// Returns the current generation for a slot, if the slot exists.
    #[must_use]
    pub fn generation_at(&self, index: u32) -> Option<u32> {
        self.generations.get(index as usize).copied()
    }

    /// Returns the number of currently alive entities.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.alive_count
    }

    /// Returns whether no entities are currently alive.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.alive_count == 0
    }
}

fn next_generation(generation: u32) -> u32 {
    let next = generation.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

#[cfg(test)]
mod tests {
    use super::{EntityAllocator, EntityId};

    #[test]
    fn new_entities_start_at_generation_one() {
        let mut allocator = EntityAllocator::new();
        let entity = allocator.spawn();
        assert_eq!(entity.index(), 0);
        assert_eq!(entity.generation(), EntityId::INITIAL_GENERATION);
        assert!(allocator.is_alive(entity));
        assert_eq!(allocator.len(), 1);
    }

    #[test]
    fn despawn_invalidates_the_old_handle() {
        let mut allocator = EntityAllocator::new();
        let entity = allocator.spawn();

        assert!(allocator.despawn(entity));
        assert!(!allocator.is_alive(entity));
        assert!(!allocator.despawn(entity));
        assert!(allocator.is_empty());
    }

    #[test]
    fn reused_slot_gets_a_new_generation() {
        let mut allocator = EntityAllocator::new();
        let old_entity = allocator.spawn();
        assert!(allocator.despawn(old_entity));

        let new_entity = allocator.spawn();
        assert_eq!(new_entity.index(), old_entity.index());
        assert_ne!(new_entity.generation(), old_entity.generation());
        assert!(!allocator.is_alive(old_entity));
        assert!(allocator.is_alive(new_entity));
    }

    #[test]
    fn stale_handles_cannot_despawn_a_reused_entity() {
        let mut allocator = EntityAllocator::new();
        let old_entity = allocator.spawn();
        assert!(allocator.despawn(old_entity));
        let new_entity = allocator.spawn();

        assert!(!allocator.despawn(old_entity));
        assert!(allocator.is_alive(new_entity));
        assert_eq!(allocator.len(), 1);
    }

    #[test]
    fn raw_handles_are_compared_by_index_and_generation() {
        let entity = EntityId::from_raw(7, 3);
        assert_eq!(entity.index(), 7);
        assert_eq!(entity.generation(), 3);
        assert_eq!(entity.to_string(), "EntityId(7:g3)");
    }
}
