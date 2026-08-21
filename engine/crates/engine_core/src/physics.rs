//! Rapier-backed 2D physics: rigid bodies, colliders, sensors, and collision
//! events surfaced to Lua. A zero-gravity top-down world stepped at the fixed
//! timestep; dynamic bodies sync their positions back to ECS Transforms.

use std::{collections::HashMap, error::Error, fmt};

use rapier2d::prelude::*;

use crate::{entity::EntityId, transform::Vec2};

/// Errors produced while building or driving the physics world.
#[derive(Clone, Debug, PartialEq)]
pub enum PhysicsError {
    UnknownBody(EntityId),
    DuplicateBody(EntityId),
    Malformed(String),
}

impl fmt::Display for PhysicsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBody(entity) => write!(f, "entity {entity} has no physics body"),
            Self::DuplicateBody(entity) => write!(f, "entity {entity} already has a physics body"),
            Self::Malformed(reason) => write!(f, "invalid collider component string: {reason}"),
        }
    }
}

impl Error for PhysicsError {}

/// A collider shape.
#[derive(Clone, Debug, PartialEq)]
pub enum ColliderShape {
    Ball(f32),
    Cuboid(Vec2),
}

/// One collider attached to a body, at a local offset. `layer` is the collision
/// group this collider belongs to and `mask` the groups it collides with (both
/// bit fields, default all bits on).
#[derive(Clone, Debug, PartialEq)]
pub struct ColliderDesc {
    pub shape: ColliderShape,
    pub offset: Vec2,
    pub sensor: bool,
    pub layer: u8,
    pub mask: u8,
}

impl ColliderDesc {
    /// Parses a scene `Collider` component string:
    /// `shape=ball;radius=..` or `shape=cuboid;half_width=..;half_height=..`,
    /// with optional `;sensor=true`, `;offset=x,y`, `;layer=N`, and `;mask=N`.
    pub fn from_component_str(value: &str) -> Result<Self, PhysicsError> {
        let mut shape = None;
        let mut offset = Vec2::zero();
        let mut sensor = false;
        let mut layer = 0xFF;
        let mut mask = 0xFF;
        for field in value.split(';') {
            let Some((key, raw)) = field.split_once('=') else {
                return Err(PhysicsError::Malformed(format!("malformed field: {field}")));
            };
            match key {
                "shape" => match raw {
                    "ball" => shape = Some(ColliderShape::Ball(0.0)),
                    "cuboid" => shape = Some(ColliderShape::Cuboid(Vec2::zero())),
                    other => {
                        return Err(PhysicsError::Malformed(format!("unknown shape: {other}")));
                    }
                },
                "radius" => {
                    let radius: f32 = raw
                        .parse()
                        .map_err(|_| PhysicsError::Malformed("radius not a number".to_owned()))?;
                    if let Some(ColliderShape::Ball(slot)) = &mut shape {
                        *slot = radius;
                    } else {
                        return Err(PhysicsError::Malformed(
                            "radius needs shape=ball".to_owned(),
                        ));
                    }
                }
                "half_width" => {
                    let value: f32 = raw.parse().map_err(|_| {
                        PhysicsError::Malformed("half_width not a number".to_owned())
                    })?;
                    if let Some(ColliderShape::Cuboid(slot)) = &mut shape {
                        slot.x = value;
                    } else {
                        return Err(PhysicsError::Malformed(
                            "half_width needs shape=cuboid".to_owned(),
                        ));
                    }
                }
                "half_height" => {
                    let value: f32 = raw.parse().map_err(|_| {
                        PhysicsError::Malformed("half_height not a number".to_owned())
                    })?;
                    if let Some(ColliderShape::Cuboid(slot)) = &mut shape {
                        slot.y = value;
                    } else {
                        return Err(PhysicsError::Malformed(
                            "half_height needs shape=cuboid".to_owned(),
                        ));
                    }
                }
                "sensor" => {
                    sensor = match raw {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(PhysicsError::Malformed(
                                "sensor must be true/false".to_owned(),
                            ));
                        }
                    };
                }
                "offset" => {
                    let (x, y) = raw.split_once(',').ok_or_else(|| {
                        PhysicsError::Malformed("offset must be <x>,<y>".to_owned())
                    })?;
                    let x: f32 = x
                        .parse()
                        .map_err(|_| PhysicsError::Malformed("offset x not a number".to_owned()))?;
                    let y: f32 = y
                        .parse()
                        .map_err(|_| PhysicsError::Malformed("offset y not a number".to_owned()))?;
                    offset = Vec2::new(x, y);
                }
                "layer" => {
                    layer = raw.parse().map_err(|_| {
                        PhysicsError::Malformed("layer must be a number".to_owned())
                    })?;
                }
                "mask" => {
                    mask = raw
                        .parse()
                        .map_err(|_| PhysicsError::Malformed("mask must be a number".to_owned()))?;
                }
                _ => return Err(PhysicsError::Malformed(format!("unknown field: {key}"))),
            }
        }
        let shape = shape.ok_or_else(|| PhysicsError::Malformed("missing shape".to_owned()))?;
        match &shape {
            ColliderShape::Ball(radius) => {
                if !radius.is_finite() || *radius <= 0.0 {
                    return Err(PhysicsError::Malformed(
                        "ball radius must be positive".to_owned(),
                    ));
                }
            }
            ColliderShape::Cuboid(half) => {
                if !half.x.is_finite() || !half.y.is_finite() || half.x <= 0.0 || half.y <= 0.0 {
                    return Err(PhysicsError::Malformed(
                        "cuboid half extents must be positive".to_owned(),
                    ));
                }
            }
        }
        Ok(Self {
            shape,
            offset,
            sensor,
            layer,
            mask,
        })
    }
}

/// A collision reported to the host (Lua) for the current step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsContact {
    pub a: EntityId,
    pub b: EntityId,
    pub started: bool,
}

/// Collects Rapier collision events into a Vec for the current step. The
/// Rapier event hooks take `&self` and require `Send + Sync`, so the buffer
/// uses a `Mutex` (stepping is single-threaded, so it never contends).
#[derive(Default)]
struct CollisionCollector {
    events: std::sync::Mutex<Vec<CollisionEvent>>,
}

impl EventHandler for CollisionCollector {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        event: CollisionEvent,
        _contact_pair: Option<&ContactPair>,
    ) {
        self.events
            .lock()
            .expect("collision collector mutex is not poisoned")
            .push(event);
    }

    fn handle_contact_force_event(
        &self,
        _impulse: f32,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &ContactPair,
        _magnitude: f32,
    ) {
    }
}

/// No-op physics hooks.
#[derive(Default)]
struct NoHooks;

impl PhysicsHooks for NoHooks {}

impl fmt::Debug for PhysicsWorld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhysicsWorld")
            .field("bodies", &self.bodies.len())
            .field("colliders", &self.colliders.len())
            .field("contacts", &self.contacts.len())
            .finish()
    }
}

/// Owns the Rapier simulation and maps engine entities to bodies/colliders.
pub struct PhysicsWorld {
    pipeline: PhysicsPipeline,
    gravity: Vector<Real>,
    islands: IslandManager,
    broad_phase: BroadPhaseMultiSap,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    query_pipeline: QueryPipeline,
    hooks: NoHooks,
    collector: CollisionCollector,
    entity_bodies: HashMap<EntityId, RigidBodyHandle>,
    collider_entities: HashMap<ColliderHandle, EntityId>,
    contacts: Vec<PhysicsContact>,
    moved: Vec<(EntityId, Vec2)>,
}

impl Default for PhysicsWorld {
    fn default() -> Self {
        Self {
            pipeline: PhysicsPipeline::new(),
            gravity: Vector::new(0.0, 0.0),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseMultiSap::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            hooks: NoHooks,
            collector: CollisionCollector::default(),
            entity_bodies: HashMap::new(),
            collider_entities: HashMap::new(),
            contacts: Vec::new(),
            moved: Vec::new(),
        }
    }
}

impl PhysicsWorld {
    /// Creates an empty zero-gravity (top-down) physics world.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a body (dynamic when `dynamic`, otherwise fixed) at `position`
    /// with the given colliders. Collider handles map back to `entity` so
    /// collision events name it.
    pub fn add_body(
        &mut self,
        entity: EntityId,
        position: Vec2,
        dynamic: bool,
        colliders: Vec<ColliderDesc>,
    ) -> Result<(), PhysicsError> {
        if self.entity_bodies.contains_key(&entity) {
            return Err(PhysicsError::DuplicateBody(entity));
        }
        let mut builder = if dynamic {
            RigidBodyBuilder::dynamic()
        } else {
            RigidBodyBuilder::fixed()
        };
        builder = builder.translation(Vector::new(position.x, position.y));
        let body_handle = self.bodies.insert(builder.build());
        self.entity_bodies.insert(entity, body_handle);
        for desc in colliders {
            let mut builder = match desc.shape {
                ColliderShape::Ball(radius) => ColliderBuilder::ball(radius),
                ColliderShape::Cuboid(half) => ColliderBuilder::cuboid(half.x, half.y),
            };
            builder = builder
                .translation(Vector::new(desc.offset.x, desc.offset.y))
                .sensor(desc.sensor)
                .active_events(ActiveEvents::COLLISION_EVENTS)
                .collision_groups(InteractionGroups::new(
                    Group::from_bits_truncate(desc.layer as u32),
                    Group::from_bits_truncate(desc.mask as u32),
                ));
            let collider_handle =
                self.colliders
                    .insert_with_parent(builder.build(), body_handle, &mut self.bodies);
            self.collider_entities.insert(collider_handle, entity);
        }
        Ok(())
    }

    /// Removes a body and its colliders (e.g. when its entity is destroyed).
    pub fn remove_body(&mut self, entity: EntityId) -> Result<(), PhysicsError> {
        let Some(handle) = self.entity_bodies.remove(&entity) else {
            return Err(PhysicsError::UnknownBody(entity));
        };
        // RigidBodySet::remove also removes the body's attached colliders.
        self.bodies.remove(
            handle,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            true,
        );
        self.collider_entities.retain(|_, owner| *owner != entity);
        Ok(())
    }

    /// Sets the linear velocity of a dynamic body.
    pub fn set_velocity(&mut self, entity: EntityId, vx: f32, vy: f32) -> Result<(), PhysicsError> {
        let handle = self
            .entity_bodies
            .get(&entity)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        if let Some(body) = self.bodies.get_mut(*handle) {
            body.set_linvel(Vector::new(vx, vy), true);
        }
        Ok(())
    }

    /// Returns the linear velocity of a body.
    pub fn velocity(&self, entity: EntityId) -> Result<Vec2, PhysicsError> {
        let handle = self
            .entity_bodies
            .get(&entity)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        let body = self
            .bodies
            .get(*handle)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        let v = body.linvel();
        Ok(Vec2::new(v.x, v.y))
    }

    /// Advances the simulation by `dt`, collecting contacts and returning the
    /// new positions of every moved dynamic body for ECS sync.
    pub fn step(&mut self, delta_seconds: f32) -> Vec<(EntityId, Vec2)> {
        let integration = IntegrationParameters {
            dt: delta_seconds.max(0.0),
            ..Default::default()
        };
        self.pipeline.step(
            &self.gravity,
            &integration,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            Some(&mut self.query_pipeline),
            &self.hooks,
            &self.collector,
        );
        // Contacts accumulate until the host drains them (Lua each frame).
        let events = self
            .collector
            .events
            .lock()
            .expect("collision collector mutex is not poisoned")
            .drain(..)
            .collect::<Vec<_>>();
        for event in events {
            let (a, b, started) = match event {
                CollisionEvent::Started(a, b, _) => (a, b, true),
                CollisionEvent::Stopped(a, b, _) => (a, b, false),
            };
            if let (Some(entity_a), Some(entity_b)) = (
                self.collider_entities.get(&a).copied(),
                self.collider_entities.get(&b).copied(),
            ) {
                self.contacts.push(PhysicsContact {
                    a: entity_a,
                    b: entity_b,
                    started,
                });
            }
        }
        self.moved.clear();
        for (entity, handle) in &self.entity_bodies {
            if let Some(body) = self.bodies.get(*handle)
                && body.body_type() == RigidBodyType::Dynamic
            {
                let position = body.translation();
                self.moved
                    .push((*entity, Vec2::new(position.x, position.y)));
            }
        }
        self.moved.clone()
    }

    /// Drains the collision contacts collected since the last call.
    pub fn drain_contacts(&mut self) -> Vec<PhysicsContact> {
        std::mem::take(&mut self.contacts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(n: u32) -> EntityId {
        EntityId::from_raw(n, 1)
    }

    #[test]
    fn parses_ball_and_cuboid_colliders() {
        let ball = ColliderDesc::from_component_str("shape=ball;radius=16").unwrap();
        assert_eq!(ball.shape, ColliderShape::Ball(16.0));
        assert!(!ball.sensor);
        let sensor = ColliderDesc::from_component_str(
            "shape=cuboid;half_width=8;half_height=12;sensor=true",
        )
        .unwrap();
        assert_eq!(sensor.shape, ColliderShape::Cuboid(Vec2::new(8.0, 12.0)));
        assert!(sensor.sensor);
        assert!(ColliderDesc::from_component_str("shape=ball").is_err());
        assert!(ColliderDesc::from_component_str("radius=16").is_err());
        let filtered =
            ColliderDesc::from_component_str("shape=ball;radius=10;layer=2;mask=4").unwrap();
        assert_eq!((filtered.layer, filtered.mask), (2, 4));
    }

    #[test]
    fn dynamic_body_integrates_velocity() {
        let mut world = PhysicsWorld::new();
        let player = entity(1);
        world
            .add_body(
                player,
                Vec2::new(0.0, 0.0),
                true,
                vec![ColliderDesc {
                    shape: ColliderShape::Ball(10.0),
                    offset: Vec2::zero(),
                    sensor: false,
                    layer: 0xFF,
                    mask: 0xFF,
                }],
            )
            .unwrap();
        world.set_velocity(player, 5.0, 0.0).unwrap();
        assert_eq!(world.velocity(player).unwrap(), Vec2::new(5.0, 0.0));
        let moved = world.step(0.5);
        assert!(moved.contains(&(player, Vec2::new(2.5, 0.0))));
    }

    #[test]
    fn collision_layers_and_masks_filter_contacts() {
        let mut world = PhysicsWorld::new();
        let player = entity(1);
        let wall = entity(2);
        // player belongs to layer 1 and only collides with layer 1; the wall
        // is on layer 2, so the player must pass straight through it.
        world
            .add_body(
                player,
                Vec2::new(0.0, 0.0),
                true,
                vec![ColliderDesc {
                    shape: ColliderShape::Ball(10.0),
                    offset: Vec2::zero(),
                    sensor: false,
                    layer: 0x01,
                    mask: 0x01,
                }],
            )
            .unwrap();
        world
            .add_body(
                wall,
                Vec2::new(40.0, 0.0),
                false,
                vec![ColliderDesc {
                    shape: ColliderShape::Cuboid(Vec2::new(10.0, 50.0)),
                    offset: Vec2::zero(),
                    sensor: false,
                    layer: 0x02,
                    mask: 0xFF,
                }],
            )
            .unwrap();
        world.set_velocity(player, 40.0, 0.0).unwrap();
        let mut final_x = 0.0;
        for _ in 0..90 {
            let moved = world.step(1.0 / 60.0);
            if let Some((_, position)) = moved.iter().find(|(e, _)| *e == player) {
                final_x = position.x;
            }
        }
        assert!(
            final_x > 35.0,
            "player should pass through the masked-out wall: x={final_x}"
        );
        let contacts = world.drain_contacts();
        assert!(
            !contacts
                .iter()
                .any(|c| (c.a == player && c.b == wall) || (c.a == wall && c.b == player)),
            "masked-out collision still emitted a contact"
        );
    }

    #[test]
    fn static_wall_blocks_dynamic_body() {
        let mut world = PhysicsWorld::new();
        let player = entity(1);
        let wall = entity(2);
        world
            .add_body(
                player,
                Vec2::new(0.0, 0.0),
                true,
                vec![ColliderDesc {
                    shape: ColliderShape::Ball(10.0),
                    offset: Vec2::zero(),
                    sensor: false,
                    layer: 0xFF,
                    mask: 0xFF,
                }],
            )
            .unwrap();
        // static cuboid wall at x=30..50
        world
            .add_body(
                wall,
                Vec2::new(40.0, 0.0),
                false,
                vec![ColliderDesc {
                    shape: ColliderShape::Cuboid(Vec2::new(10.0, 50.0)),
                    offset: Vec2::zero(),
                    sensor: false,
                    layer: 0xFF,
                    mask: 0xFF,
                }],
            )
            .unwrap();
        // move toward the wall at 40 u/s for 1.5s -> should be stopped near the wall
        world.set_velocity(player, 40.0, 0.0).unwrap();
        let mut final_x = 0.0;
        for _ in 0..90 {
            let moved = world.step(1.0 / 60.0);
            if let Some((_, position)) = moved.iter().find(|(e, _)| *e == player) {
                final_x = position.x;
            }
        }
        assert!(final_x < 30.0, "player penetrated the wall: x={final_x}");
        // a collision contact with the wall was reported
        let contacts = world.drain_contacts();
        assert!(
            contacts
                .iter()
                .any(|c| (c.a == player && c.b == wall) || (c.a == wall && c.b == player)),
            "no contact reported"
        );
    }
}
