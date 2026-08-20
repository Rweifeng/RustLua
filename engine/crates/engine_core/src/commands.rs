//! Deferred world mutations for safe ECS system iteration.

use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    fmt,
};

use crate::{
    entity::EntityId,
    world::{World, WorldError},
};

/// A handle for an entity that will be created when a command batch is applied.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SpawnToken(u32);

impl SpawnToken {
    /// Returns the zero-based token sequence number.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// An entity target accepted by a deferred command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandEntity {
    Existing(EntityId),
    Pending(SpawnToken),
}

impl From<EntityId> for CommandEntity {
    fn from(entity: EntityId) -> Self {
        Self::Existing(entity)
    }
}
impl From<SpawnToken> for CommandEntity {
    fn from(token: SpawnToken) -> Self {
        Self::Pending(token)
    }
}

/// Contextual errors produced while applying a command batch.
#[derive(Debug, Eq, PartialEq)]
pub enum CommandError {
    UnresolvedToken {
        token: SpawnToken,
    },
    World {
        operation: &'static str,
        entity: EntityId,
        source: WorldError,
    },
    Failed {
        index: usize,
        operation: &'static str,
        source: Box<CommandError>,
    },
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnresolvedToken { token } => {
                write!(f, "unresolved spawn token {}", token.index())
            }
            Self::World {
                operation,
                entity,
                source,
            } => write!(f, "{operation} failed for {entity}: {source}"),
            Self::Failed {
                index,
                operation,
                source,
            } => write!(f, "command {index} ({operation}) failed: {source}"),
        }
    }
}
impl Error for CommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::World { source, .. } => Some(source),
            Self::Failed { source, .. } => Some(source),
            Self::UnresolvedToken { .. } => None,
        }
    }
}

type CommandAction = Box<dyn FnOnce(&mut World, &mut ApplyContext) -> Result<(), CommandError>>;
struct Command {
    operation: &'static str,
    action: CommandAction,
}
#[derive(Default)]
struct ApplyContext {
    spawned: HashMap<SpawnToken, EntityId>,
}
impl ApplyContext {
    fn resolve(&self, target: CommandEntity) -> Result<EntityId, CommandError> {
        match target {
            CommandEntity::Existing(entity) => Ok(entity),
            CommandEntity::Pending(token) => self
                .spawned
                .get(&token)
                .copied()
                .ok_or(CommandError::UnresolvedToken { token }),
        }
    }
}

/// Collects world mutations and applies them in deterministic FIFO order.
#[derive(Default)]
pub struct CommandQueue {
    commands: VecDeque<Command>,
    next_token: u32,
}
impl fmt::Debug for CommandQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandQueue")
            .field("len", &self.commands.len())
            .field("next_token", &self.next_token)
            .finish()
    }
}
impl CommandQueue {
    /// Creates an empty command queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Queues entity creation and returns a token usable by later commands.
    pub fn spawn(&mut self) -> SpawnToken {
        let token = SpawnToken(self.next_token);
        self.next_token = self.next_token.wrapping_add(1);
        self.commands.push_back(Command {
            operation: "spawn",
            action: Box::new(move |world, context| {
                context.spawned.insert(token, world.spawn());
                Ok(())
            }),
        });
        token
    }
    /// Queues entity destruction.
    pub fn despawn(&mut self, target: impl Into<CommandEntity>) {
        self.commands.push_back(Self::target_command(
            "despawn",
            target.into(),
            |world, entity| world.despawn(entity).map(|_| ()),
        ));
    }
    /// Queues insertion or replacement of a component.
    pub fn insert<T: 'static>(&mut self, target: impl Into<CommandEntity>, component: T) {
        self.commands.push_back(Self::target_command(
            "insert",
            target.into(),
            move |world, entity| world.insert(entity, component).map(|_| ()),
        ));
    }
    /// Queues removal of a component type.
    pub fn remove<T: 'static>(&mut self, target: impl Into<CommandEntity>) {
        self.commands.push_back(Self::target_command(
            "remove",
            target.into(),
            |world, entity| world.remove::<T>(entity).map(|_| ()),
        ));
    }
    /// Returns the number of commands waiting to be applied.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }
    /// Returns whether no commands are waiting to be applied.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
    /// Applies and consumes the current batch in FIFO order.
    pub fn apply(&mut self, world: &mut World) -> Result<(), CommandError> {
        let commands = std::mem::take(&mut self.commands);
        let mut context = ApplyContext::default();
        for (index, command) in commands.into_iter().enumerate() {
            let operation = command.operation;
            if let Err(source) = (command.action)(world, &mut context) {
                return Err(CommandError::Failed {
                    index,
                    operation,
                    source: Box::new(source),
                });
            }
        }
        Ok(())
    }
    fn target_command<F>(operation: &'static str, target: CommandEntity, action: F) -> Command
    where
        F: FnOnce(&mut World, EntityId) -> Result<(), WorldError> + 'static,
    {
        Command {
            operation,
            action: Box::new(move |world, context| {
                let entity = context.resolve(target)?;
                action(world, entity).map_err(|source| CommandError::World {
                    operation,
                    entity,
                    source,
                })
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandEntity, CommandError, CommandQueue, SpawnToken};
    use crate::{entity::EntityId, world::World};
    #[derive(Debug, PartialEq)]
    struct Marker(u32);
    #[test]
    fn queues_mutations_until_apply_and_preserves_order() {
        let mut world = World::new();
        let entity = world.spawn();
        let mut queue = CommandQueue::new();
        queue.insert(entity, Marker(1));
        queue.insert(entity, Marker(2));
        assert_eq!(queue.len(), 2);
        assert!(!world.has::<Marker>(entity).unwrap());
        queue.apply(&mut world).unwrap();
        assert_eq!(world.get::<Marker>(entity).unwrap(), Some(&Marker(2)));
        assert!(queue.is_empty());
    }
    #[test]
    fn spawn_token_can_be_used_by_later_commands_in_same_batch() {
        let mut world = World::new();
        let mut queue = CommandQueue::new();
        let token = queue.spawn();
        queue.insert(token, Marker(7));
        assert_eq!(world.entity_count(), 0);
        queue.apply(&mut world).unwrap();
        assert_eq!(world.entity_count(), 1);
        let entity = world.entities_with::<Marker>()[0];
        assert_eq!(world.get::<Marker>(entity).unwrap(), Some(&Marker(7)));
    }
    #[test]
    fn spawn_then_despawn_is_applied_safely() {
        let mut world = World::new();
        let mut queue = CommandQueue::new();
        let token = queue.spawn();
        queue.despawn(token);
        queue.apply(&mut world).unwrap();
        assert_eq!(world.entity_count(), 0);
    }
    #[test]
    fn stale_existing_targets_fail_without_touching_reused_entities() {
        let mut world = World::new();
        let old = world.spawn();
        world.despawn(old).unwrap();
        let new = world.spawn();
        let mut queue = CommandQueue::new();
        queue.insert(old, Marker(1));
        let error = queue.apply(&mut world).unwrap_err();
        assert!(matches!(
            error,
            CommandError::Failed {
                index: 0,
                operation: "insert",
                ..
            }
        ));
        assert!(!world.has::<Marker>(new).unwrap());
    }
    #[test]
    fn missing_token_reports_context_and_batch_is_consumed() {
        let mut world = World::new();
        let mut queue = CommandQueue::new();
        queue.insert(CommandEntity::Pending(SpawnToken(99)), Marker(1));
        let error = queue.apply(&mut world).unwrap_err();
        assert!(error.to_string().contains("command 0 (insert)"));
        assert!(error.to_string().contains("unresolved spawn token 99"));
        assert!(queue.is_empty());
    }
    #[test]
    fn despawn_during_iteration_is_deferred() {
        let mut world = World::new();
        let entity = world.spawn();
        let mut queue = CommandQueue::new();
        queue.despawn(entity);
        assert!(world.is_alive(entity));
        queue.apply(&mut world).unwrap();
        assert!(!world.is_alive(entity));
    }
    #[test]
    fn invalid_entity_id_is_not_accepted_as_another_entity() {
        let mut world = World::new();
        let invalid = EntityId::from_raw(42, 3);
        let mut queue = CommandQueue::new();
        queue.remove::<Marker>(invalid);
        assert!(queue.apply(&mut world).is_err());
    }
}
