//! Rapier-backed 3D physics: rigid bodies, colliders (ball/cuboid/capsule),
//! sensors, ray/overlap queries, and collision events surfaced to Lua. Uses
//! the ADR-0007 right-handed +Y-up convention; gravity defaults to −Y.
//! Dynamic bodies sync their translations back to ECS `Transform3D` each fixed
//! step (rotation and scale stay owned by the scene).

use std::{collections::HashMap, fmt};

use rapier3d::prelude::*;

use crate::core::{
    entity::EntityId,
    math3d::{Quat, Vec3},
    physics::{PhysicsContact, PhysicsError},
    transform::Transform3D,
    world::World,
};

/// A 3D collider shape. `Capsule` is aligned with the local Y axis.
#[derive(Clone, Debug, PartialEq)]
pub enum ColliderShape3d {
    Ball(f32),
    Cuboid(Vec3),
    Capsule { half_height: f32, radius: f32 },
}

/// One 3D collider as it currently sits in the physics world, for debug drawing.
///
/// 与 2D 的 [`crate::core::physics::ColliderDebugShape`] 同构（同名字段、同一套
/// "读 Rapier 当前姿态"的来源），docs/54 §8-⑥ 因此可以用一条对拍测试把两侧钉在一起。
#[derive(Clone, Debug, PartialEq)]
pub struct ColliderDebugShape3d {
    pub entity: EntityId,
    /// World-space centre (the body's translation plus the rotated local offset).
    pub center: Vec3,
    /// World-space rotation.
    pub rotation: Quat,
    pub shape: ColliderShape3d,
    pub sensor: bool,
    /// 刚体是否动态（`;dynamic=false` 的墙/地面为 `false`）。
    pub dynamic: bool,
}

impl ColliderShape3d {
    /// 局部空间的调试线框边：`Ball` 用三条正交圆环（各 16 段）、`Cuboid` 用 12 条棱、
    /// `Capsule` 用上下两个圆环加 4 条母线。
    ///
    /// 段数**固定**（不随半径/尺寸变化）：调试顶点的数量因此可断言，宿主也能据此预留缓冲。
    /// 圆环是近似——但它只是调试轮廓，碰撞体本身仍是 Rapier 的精确形状。
    #[must_use]
    pub fn wireframe_edges(&self) -> Vec<(Vec3, Vec3)> {
        const SEGMENTS: usize = 16;
        let mut edges = Vec::new();
        let ring = |center: Vec3, axis: usize, radius: f32, edges: &mut Vec<(Vec3, Vec3)>| {
            // axis: 0 = XY（绕 Z）、1 = XZ（绕 Y）、2 = YZ（绕 X）
            let point = |step: usize| -> Vec3 {
                let angle = step as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                let (sin, cos) = angle.sin_cos();
                match axis {
                    0 => center + Vec3::new(cos * radius, sin * radius, 0.0),
                    1 => center + Vec3::new(cos * radius, 0.0, sin * radius),
                    _ => center + Vec3::new(0.0, cos * radius, sin * radius),
                }
            };
            for step in 0..SEGMENTS {
                edges.push((point(step), point((step + 1) % SEGMENTS)));
            }
        };
        match *self {
            ColliderShape3d::Ball(radius) => {
                for axis in 0..3 {
                    ring(Vec3::ZERO, axis, radius, &mut edges);
                }
            }
            ColliderShape3d::Cuboid(half) => {
                let mut corners = [Vec3::ZERO; 8];
                for (index, corner) in corners.iter_mut().enumerate() {
                    *corner = Vec3::new(
                        if index & 1 == 0 { -half.x } else { half.x },
                        if index & 2 == 0 { -half.y } else { half.y },
                        if index & 4 == 0 { -half.z } else { half.z },
                    );
                }
                // 12 条棱：只连"恰好一位不同"的角。
                for (a, from) in corners.iter().enumerate() {
                    for (b, to) in corners.iter().enumerate().skip(a + 1) {
                        if (a ^ b).count_ones() == 1 {
                            edges.push((*from, *to));
                        }
                    }
                }
            }
            ColliderShape3d::Capsule {
                half_height,
                radius,
            } => {
                let top = Vec3::new(0.0, half_height, 0.0);
                let bottom = Vec3::new(0.0, -half_height, 0.0);
                ring(top, 1, radius, &mut edges);
                ring(bottom, 1, radius, &mut edges);
                for step in 0..4 {
                    let angle = step as f32 / 4.0 * std::f32::consts::TAU;
                    let (sin, cos) = angle.sin_cos();
                    let offset = Vec3::new(cos * radius, 0.0, sin * radius);
                    edges.push((top + offset, bottom + offset));
                }
            }
        }
        edges
    }
}

impl ColliderDebugShape3d {
    /// 世界空间的调试线段（局部边经刚体旋转平移）。旋转不可用时**跳过**该边——宁可少画
    /// 一条线，也不画一条静默近似出来的线。
    #[must_use]
    pub fn wireframe_segments(&self) -> Vec<(Vec3, Vec3)> {
        let mut segments = Vec::new();
        for (from, to) in self.shape.wireframe_edges() {
            let (Ok(from), Ok(to)) = (self.rotation.rotate(from), self.rotation.rotate(to)) else {
                continue;
            };
            segments.push((
                Vec3::new(
                    self.center.x + from.x,
                    self.center.y + from.y,
                    self.center.z + from.z,
                ),
                Vec3::new(
                    self.center.x + to.x,
                    self.center.y + to.y,
                    self.center.z + to.z,
                ),
            ));
        }
        segments
    }
}

/// One collider attached to a body, at a local offset. `layer` is the collision
/// group this collider belongs to and `mask` the groups it collides with (both
/// bit fields, default all bits on).
///
/// `ccd` 是连续碰撞检测声明，与 2D 侧同义（Rapier 的 CCD 是**刚体**属性，任一本刚体上的
/// collider 声明即对该刚体生效），默认关闭。
#[derive(Clone, Debug, PartialEq)]
pub struct ColliderDesc3d {
    pub shape: ColliderShape3d,
    pub offset: Vec3,
    pub sensor: bool,
    /// Whether the owning body is dynamic (default true); `;dynamic=false`
    /// creates a fixed body (ground, walls).
    pub dynamic: bool,
    pub layer: u8,
    pub mask: u8,
    pub ccd: bool,
}

impl ColliderDesc3d {
    /// Parses a scene `Collider3d` component string:
    /// `shape=ball;radius=..`, `shape=cuboid;half_x=..;half_y=..;half_z=..`, or
    /// `shape=capsule;half_height=..;radius=..`, with optional `;sensor=true`,
    /// `;dynamic=false`, `;offset=x,y,z`, `;layer=N`, `;mask=N`, and `;ccd=true`.
    pub fn from_component_str(value: &str) -> Result<Self, PhysicsError> {
        let mut shape = None;
        let mut offset = Vec3::ZERO;
        let mut sensor = false;
        let mut dynamic = true;
        let mut layer = 0xFF;
        let mut mask = 0xFF;
        let mut ccd = false;
        for field in value.split(';') {
            let Some((key, raw)) = field.split_once('=') else {
                return Err(PhysicsError::Malformed(format!("malformed field: {field}")));
            };
            match key {
                "shape" => match raw {
                    "ball" => shape = Some(ColliderShape3d::Ball(0.0)),
                    "cuboid" => shape = Some(ColliderShape3d::Cuboid(Vec3::ZERO)),
                    "capsule" => {
                        shape = Some(ColliderShape3d::Capsule {
                            half_height: 0.0,
                            radius: 0.0,
                        })
                    }
                    other => {
                        return Err(PhysicsError::Malformed(format!("unknown shape: {other}")));
                    }
                },
                "radius" => {
                    let radius: f32 = raw
                        .parse()
                        .map_err(|_| PhysicsError::Malformed("radius not a number".to_owned()))?;
                    match &mut shape {
                        Some(ColliderShape3d::Ball(slot)) => *slot = radius,
                        Some(ColliderShape3d::Capsule { radius: slot, .. }) => *slot = radius,
                        _ => {
                            return Err(PhysicsError::Malformed(
                                "radius needs shape=ball or shape=capsule".to_owned(),
                            ));
                        }
                    }
                }
                "half_width" => {
                    let value: f32 = raw.parse().map_err(|_| {
                        PhysicsError::Malformed("half_width not a number".to_owned())
                    })?;
                    if let Some(ColliderShape3d::Cuboid(slot)) = &mut shape {
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
                    match &mut shape {
                        Some(ColliderShape3d::Cuboid(slot)) => slot.y = value,
                        Some(ColliderShape3d::Capsule {
                            half_height: slot, ..
                        }) => *slot = value,
                        _ => {
                            return Err(PhysicsError::Malformed(
                                "half_height needs shape=cuboid or shape=capsule".to_owned(),
                            ));
                        }
                    }
                }
                "half_z" => {
                    let value: f32 = raw
                        .parse()
                        .map_err(|_| PhysicsError::Malformed("half_z not a number".to_owned()))?;
                    if let Some(ColliderShape3d::Cuboid(slot)) = &mut shape {
                        slot.z = value;
                    } else {
                        return Err(PhysicsError::Malformed(
                            "half_z needs shape=cuboid".to_owned(),
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
                "dynamic" => {
                    dynamic = match raw {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(PhysicsError::Malformed(
                                "dynamic must be true/false".to_owned(),
                            ));
                        }
                    };
                }
                "offset" => {
                    let mut parts = raw.split(',');
                    let x: f32 = parts
                        .next()
                        .ok_or_else(|| {
                            PhysicsError::Malformed("offset must be <x>,<y>,<z>".to_owned())
                        })?
                        .parse()
                        .map_err(|_| PhysicsError::Malformed("offset x not a number".to_owned()))?;
                    let y: f32 = parts
                        .next()
                        .ok_or_else(|| {
                            PhysicsError::Malformed("offset must be <x>,<y>,<z>".to_owned())
                        })?
                        .parse()
                        .map_err(|_| PhysicsError::Malformed("offset y not a number".to_owned()))?;
                    let z: f32 = parts
                        .next()
                        .ok_or_else(|| {
                            PhysicsError::Malformed("offset must be <x>,<y>,<z>".to_owned())
                        })?
                        .parse()
                        .map_err(|_| PhysicsError::Malformed("offset z not a number".to_owned()))?;
                    if parts.next().is_some() {
                        return Err(PhysicsError::Malformed(
                            "offset must be <x>,<y>,<z>".to_owned(),
                        ));
                    }
                    offset = Vec3::new(x, y, z);
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
                // WP7：连续碰撞检测（与 2D `Collider` 同义）。拼错报错而不是静默当 false。
                "ccd" => {
                    ccd = match raw {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(PhysicsError::Malformed(
                                "ccd must be true/false".to_owned(),
                            ));
                        }
                    };
                }
                _ => return Err(PhysicsError::Malformed(format!("unknown field: {key}"))),
            }
        }
        let shape = shape.ok_or_else(|| PhysicsError::Malformed("missing shape".to_owned()))?;
        match &shape {
            ColliderShape3d::Ball(radius) => {
                if !radius.is_finite() || *radius <= 0.0 {
                    return Err(PhysicsError::Malformed(
                        "ball radius must be positive".to_owned(),
                    ));
                }
            }
            ColliderShape3d::Cuboid(half) => {
                if !half.is_finite() || half.x <= 0.0 || half.y <= 0.0 || half.z <= 0.0 {
                    return Err(PhysicsError::Malformed(
                        "cuboid half extents must be positive".to_owned(),
                    ));
                }
            }
            ColliderShape3d::Capsule {
                half_height,
                radius,
            } => {
                if !half_height.is_finite() || *half_height <= 0.0 {
                    return Err(PhysicsError::Malformed(
                        "capsule half_height must be positive".to_owned(),
                    ));
                }
                if !radius.is_finite() || *radius <= 0.0 {
                    return Err(PhysicsError::Malformed(
                        "capsule radius must be positive".to_owned(),
                    ));
                }
            }
        }
        Ok(Self {
            shape,
            offset,
            sensor,
            dynamic,
            layer,
            mask,
            ccd,
        })
    }
}

/// Collects Rapier 3D collision events into a Vec for the current step.
#[derive(Default)]
struct CollisionCollector3d {
    events: std::sync::Mutex<Vec<CollisionEvent>>,
}

impl EventHandler for CollisionCollector3d {
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
struct NoHooks3d;

impl PhysicsHooks for NoHooks3d {}

impl fmt::Debug for PhysicsWorld3d {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhysicsWorld3d")
            .field("bodies", &self.bodies.len())
            .field("colliders", &self.colliders.len())
            .field("contacts", &self.contacts.len())
            .finish()
    }
}

/// Owns the Rapier 3D simulation and maps engine entities to bodies/colliders.
pub struct PhysicsWorld3d {
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
    hooks: NoHooks3d,
    collector: CollisionCollector3d,
    entity_bodies: HashMap<EntityId, RigidBodyHandle>,
    collider_entities: HashMap<ColliderHandle, EntityId>,
    contacts: Vec<PhysicsContact>,
    moved: Vec<(EntityId, Vec3)>,
}

impl Default for PhysicsWorld3d {
    fn default() -> Self {
        Self::new()
    }
}

impl PhysicsWorld3d {
    /// Creates a 3D physics world with default gravity `(0, −9.81, 0)`.
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_gravity(Vec3::new(0.0, -9.81, 0.0))
    }

    /// Creates a 3D physics world with an explicit gravity vector.
    #[must_use]
    pub fn new_with_gravity(gravity: Vec3) -> Self {
        Self {
            pipeline: PhysicsPipeline::new(),
            gravity: Vector::new(gravity.x, gravity.y, gravity.z),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseMultiSap::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            hooks: NoHooks3d,
            collector: CollisionCollector3d::default(),
            entity_bodies: HashMap::new(),
            collider_entities: HashMap::new(),
            contacts: Vec::new(),
            moved: Vec::new(),
        }
    }

    /// Adds a body (dynamic when `dynamic`, otherwise fixed) at `position`
    /// with the given colliders. Collider handles map back to `entity` so
    /// collision events and queries name it.
    ///
    /// 与 2D 侧一致：任一 collider 声明 `ccd` 时给该刚体开启连续碰撞检测。
    pub fn add_body(
        &mut self,
        entity: EntityId,
        position: Vec3,
        dynamic: bool,
        colliders: Vec<ColliderDesc3d>,
    ) -> Result<(), PhysicsError> {
        if self.entity_bodies.contains_key(&entity) {
            return Err(PhysicsError::DuplicateBody(entity));
        }
        let mut builder = if dynamic {
            RigidBodyBuilder::dynamic()
        } else {
            RigidBodyBuilder::fixed()
        };
        if colliders.iter().any(|desc| desc.ccd) {
            builder = builder.ccd_enabled(true);
        }
        builder = builder.translation(Vector::new(position.x, position.y, position.z));
        let body_handle = self.bodies.insert(builder.build());
        self.entity_bodies.insert(entity, body_handle);
        for desc in colliders {
            let mut builder = match desc.shape {
                ColliderShape3d::Ball(radius) => ColliderBuilder::ball(radius),
                ColliderShape3d::Cuboid(half) => ColliderBuilder::cuboid(half.x, half.y, half.z),
                ColliderShape3d::Capsule {
                    half_height,
                    radius,
                } => ColliderBuilder::capsule_y(half_height, radius),
            };
            builder = builder
                .translation(Vector::new(desc.offset.x, desc.offset.y, desc.offset.z))
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
        // Keep the query pipeline's BVH in sync so ray/overlap queries see
        // bodies added since the last `step`.
        self.query_pipeline.update(&self.colliders);
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
        self.query_pipeline.update(&self.colliders);
        Ok(())
    }

    /// Sets the linear velocity of a dynamic body.
    pub fn set_velocity(
        &mut self,
        entity: EntityId,
        vx: f32,
        vy: f32,
        vz: f32,
    ) -> Result<(), PhysicsError> {
        let handle = self
            .entity_bodies
            .get(&entity)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        if let Some(body) = self.bodies.get_mut(*handle) {
            body.set_linvel(Vector::new(vx, vy, vz), true);
        }
        Ok(())
    }

    /// Returns the linear velocity of a body.
    pub fn velocity(&self, entity: EntityId) -> Result<Vec3, PhysicsError> {
        let handle = self
            .entity_bodies
            .get(&entity)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        let body = self
            .bodies
            .get(*handle)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        let v = body.linvel();
        Ok(Vec3::new(v.x, v.y, v.z))
    }

    /// Teleports a body's translation (used for respawn/placement; the scene
    /// Transform3D stays the source of truth until the next step syncs back).
    pub fn set_position(&mut self, entity: EntityId, position: Vec3) -> Result<(), PhysicsError> {
        let handle = self
            .entity_bodies
            .get(&entity)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        if let Some(body) = self.bodies.get_mut(*handle) {
            body.set_translation(Vector::new(position.x, position.y, position.z), true);
        }
        Ok(())
    }

    /// 刚体当前是否开启连续碰撞检测（读 Rapier 的真实状态）。
    pub fn ccd_enabled(&self, entity: EntityId) -> Result<bool, PhysicsError> {
        let handle = self
            .entity_bodies
            .get(&entity)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        self.bodies
            .get(*handle)
            .map(|body| body.is_ccd_enabled())
            .ok_or(PhysicsError::UnknownBody(entity))
    }

    /// 运行时开关连续碰撞检测（Lua `physics3d.set_ccd`）。
    pub fn set_ccd(&mut self, entity: EntityId, enabled: bool) -> Result<(), PhysicsError> {
        let handle = self
            .entity_bodies
            .get(&entity)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        let body = self
            .bodies
            .get_mut(*handle)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        body.enable_ccd(enabled);
        Ok(())
    }

    /// Advances the simulation by `dt`, collecting contacts and returning the
    /// new positions of every moved dynamic body for ECS sync.
    pub fn step(&mut self, delta_seconds: f32) -> Vec<(EntityId, Vec3)> {
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
                    .push((*entity, Vec3::new(position.x, position.y, position.z)));
            }
        }
        self.moved.clone()
    }

    /// Drains the collision contacts collected since the last call.
    pub fn drain_contacts(&mut self) -> Vec<PhysicsContact> {
        std::mem::take(&mut self.contacts)
    }

    /// WP7 静态/运动学同步的数据源（**与 2D 同构**）：把**动态**刚体当前的位置从 ECS 取出来
    /// （按实体排序，保证确定性），供 `authority = Script` 时在物理步之前推给模拟世界。
    ///
    /// 只取动态刚体：静态体是场景数据、运动学体由调用方自己同步（与 `set_position` 的既有语义
    /// 一致）。没有 `Transform3D` 的实体跳过。
    pub fn kinematic_positions(&self, world: &World) -> Vec<(EntityId, Vec3)> {
        let mut out = Vec::new();
        for (entity, handle) in &self.entity_bodies {
            let Some(body) = self.bodies.get(*handle) else {
                continue;
            };
            if body.body_type() != RigidBodyType::Dynamic {
                continue;
            }
            if let Ok(Some(transform)) = world.get::<Transform3D>(*entity) {
                out.push((*entity, transform.position));
            }
        }
        out.sort_by_key(|(entity, _)| *entity);
        out
    }

    /// Casts a ray and returns the closest hit entity within `max_distance`.
    /// The direction vector must be non-zero.
    pub fn raycast(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Result<Option<EntityId>, PhysicsError> {
        self.raycast_filtered(origin, direction, max_distance, None)
    }

    /// 射线查询 + 碰撞分组过滤（与 2D `PhysicsWorld::raycast_filtered` 同一语义：
    /// 两位测试，`None` 保持"全收"的老行为）。
    pub fn raycast_filtered(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        groups: Option<(u8, u8)>,
    ) -> Result<Option<EntityId>, PhysicsError> {
        if !direction.is_finite() || direction.length_squared() <= f32::EPSILON {
            return Err(PhysicsError::Malformed(
                "ray direction must be non-zero".to_owned(),
            ));
        }
        if !max_distance.is_finite() || max_distance < 0.0 {
            return Err(PhysicsError::Malformed(
                "raycast distance must be finite and non-negative".to_owned(),
            ));
        }
        let ray = Ray::new(
            Point::new(origin.x, origin.y, origin.z),
            Vector::new(direction.x, direction.y, direction.z),
        );
        let hit = self.query_pipeline.cast_ray(
            &self.bodies,
            &self.colliders,
            &ray,
            max_distance,
            true,
            query_filter3d(groups),
        );
        Ok(hit.and_then(|(handle, _)| self.collider_entities.get(&handle).copied()))
    }

    /// Returns every entity whose collider overlaps a sphere centered at
    /// `center` with the given `radius`.
    pub fn overlap(&self, center: Vec3, radius: f32) -> Vec<EntityId> {
        self.overlap_filtered(center, radius, None)
    }

    /// 球重叠查询 + 碰撞分组过滤（见 [`PhysicsWorld3d::raycast_filtered`]）。
    pub fn overlap_filtered(
        &self,
        center: Vec3,
        radius: f32,
        groups: Option<(u8, u8)>,
    ) -> Vec<EntityId> {
        if !center.is_finite() || !radius.is_finite() || radius <= 0.0 {
            return Vec::new();
        }
        let shape = SharedShape::ball(radius);
        let shape_pos = Isometry::translation(center.x, center.y, center.z);
        let mut hits = Vec::new();
        self.query_pipeline.intersections_with_shape(
            &self.bodies,
            &self.colliders,
            &shape_pos,
            &*shape,
            query_filter3d(groups),
            |handle| {
                if let Some(entity) = self.collider_entities.get(&handle) {
                    hits.push(*entity);
                }
                true
            },
        );
        hits
    }

    /// 调试绘制用：把**物理世界里当前真实**的 3D 碰撞体姿态与形状取出来
    /// （docs/54 §8-⑥ 的 3D 调试契约；与 2D `PhysicsWorld::debug_shapes` 同源同义）。
    ///
    /// 与 2D 一样刻意读 Rapier 而不是场景里的 `Collider3d` 组件串：组件只在实例化时解析一次，
    /// 而动态刚体的位置/旋转每步都在变。局部偏移取**相对父刚体**的那一份，再按刚体旋转叠加。
    pub fn debug_shapes(&self) -> Vec<ColliderDebugShape3d> {
        let mut shapes = Vec::new();
        for (entity, handle) in &self.entity_bodies {
            let Some(body) = self.bodies.get(*handle) else {
                continue;
            };
            let body_translation = body.translation();
            let body_rotation = *body.rotation();
            let body_quat = Quat {
                x: body_rotation.coords.x,
                y: body_rotation.coords.y,
                z: body_rotation.coords.z,
                w: body_rotation.coords.w,
            };
            for collider_handle in body.colliders() {
                let Some(collider) = self.colliders.get(*collider_handle) else {
                    continue;
                };
                // Rapier 只给出 `&dyn Shape`，靠 downcast 认形状；认不出的形状不画，
                // 不做静默近似。
                let shape_ref = collider.shape();
                let shape = if let Some(ball) = shape_ref
                    .as_any()
                    .downcast_ref::<rapier3d::geometry::Ball>()
                {
                    ColliderShape3d::Ball(ball.radius)
                } else if let Some(cuboid) = shape_ref
                    .as_any()
                    .downcast_ref::<rapier3d::geometry::Cuboid>()
                {
                    ColliderShape3d::Cuboid(Vec3::new(
                        cuboid.half_extents.x,
                        cuboid.half_extents.y,
                        cuboid.half_extents.z,
                    ))
                } else if let Some(capsule) = shape_ref
                    .as_any()
                    .downcast_ref::<rapier3d::geometry::Capsule>()
                {
                    ColliderShape3d::Capsule {
                        half_height: capsule.half_height(),
                        radius: capsule.radius,
                    }
                } else {
                    continue;
                };
                let (local_offset, local_rotation) = match collider.position_wrt_parent() {
                    Some(pose) => (pose.translation.vector, *pose.rotation.quaternion()),
                    None => (*collider.translation(), *collider.rotation().quaternion()),
                };
                let offset = Vec3::new(local_offset.x, local_offset.y, local_offset.z);
                let Ok(rotated) = body_quat.rotate(offset) else {
                    continue; // 非单位/非有限旋转：不画，不做静默近似
                };
                let center = Vec3::new(
                    body_translation.x + rotated.x,
                    body_translation.y + rotated.y,
                    body_translation.z + rotated.z,
                );
                let rotation = body_quat
                    * Quat {
                        x: local_rotation.coords.x,
                        y: local_rotation.coords.y,
                        z: local_rotation.coords.z,
                        w: local_rotation.coords.w,
                    };
                shapes.push(ColliderDebugShape3d {
                    entity: *entity,
                    center,
                    rotation,
                    shape,
                    sensor: collider.is_sensor(),
                    dynamic: body.is_dynamic(),
                });
            }
        }
        // 与 2D 同一理由：`entity_bodies` 是 HashMap，顺序必须确定。
        shapes.sort_by_key(|shape| {
            (
                shape.entity,
                shape.center.x.to_bits(),
                shape.center.y.to_bits(),
                shape.center.z.to_bits(),
            )
        });
        shapes
    }
}

/// `(layer, mask)` → Rapier 查询过滤组；`None` = 默认（全收）。与 2D 的
/// `physics::query_filter` 逐字同义（两个 rapier crate 各有自己的类型，因此只能各写一份）。
fn query_filter3d(groups: Option<(u8, u8)>) -> QueryFilter<'static> {
    match groups {
        Some((layer, mask)) => QueryFilter::default().groups(InteractionGroups::new(
            Group::from_bits_truncate(layer as u32),
            Group::from_bits_truncate(mask as u32),
        )),
        None => QueryFilter::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(n: u32) -> EntityId {
        EntityId::from_raw(n, 1)
    }

    fn ball(radius: f32) -> ColliderDesc3d {
        ColliderDesc3d {
            shape: ColliderShape3d::Ball(radius),
            offset: Vec3::ZERO,
            sensor: false,
            layer: 0xFF,
            mask: 0xFF,
            ccd: false,
            dynamic: true,
        }
    }

    #[test]
    fn parses_ball_cuboid_and_capsule_colliders() {
        let ball = ColliderDesc3d::from_component_str("shape=ball;radius=16").unwrap();
        assert_eq!(ball.shape, ColliderShape3d::Ball(16.0));
        assert!(!ball.sensor);
        let sensor = ColliderDesc3d::from_component_str(
            "shape=cuboid;half_width=8;half_height=12;half_z=6;sensor=true",
        )
        .unwrap();
        assert_eq!(
            sensor.shape,
            ColliderShape3d::Cuboid(Vec3::new(8.0, 12.0, 6.0))
        );
        assert!(sensor.sensor);
        let capsule = ColliderDesc3d::from_component_str(
            "shape=capsule;half_height=1;radius=0.5;offset=0,1,0;layer=2;mask=4;dynamic=false",
        )
        .unwrap();
        assert_eq!(
            capsule.shape,
            ColliderShape3d::Capsule {
                half_height: 1.0,
                radius: 0.5
            }
        );
        assert_eq!(capsule.offset, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!((capsule.layer, capsule.mask), (2, 4));
        assert!(!capsule.dynamic, "dynamic=false must produce a fixed body");
        assert!(ColliderDesc3d::from_component_str("shape=ball").is_err());
        assert!(ColliderDesc3d::from_component_str("radius=16").is_err());
        assert!(ColliderDesc3d::from_component_str("shape=cuboid;half_width=1").is_err());
        assert!(ColliderDesc3d::from_component_str("shape=capsule;radius=1").is_err());
    }

    /// docs/54 §8-⑥：3D 调试数据与 2D 同构——读物理世界当前姿态、报出 sensor/dynamic、
    /// 线框段数固定（不随尺寸变化，宿主据此可以断言顶点数）。
    #[test]
    fn debug_shapes_report_pose_dynamic_and_fixed_wireframes() {
        // 段数固定：Ball = 3×16、Cuboid = 12、Capsule = 2×16 + 4。
        assert_eq!(ColliderShape3d::Ball(1.0).wireframe_edges().len(), 48);
        assert_eq!(
            ColliderShape3d::Cuboid(Vec3::new(1.0, 1.0, 1.0))
                .wireframe_edges()
                .len(),
            12
        );
        assert_eq!(
            ColliderShape3d::Capsule {
                half_height: 1.0,
                radius: 1.0
            }
            .wireframe_edges()
            .len(),
            36
        );

        let mut world = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
        let wall = entity(1);
        let ball = entity(2);
        world
            .add_body(
                wall,
                Vec3::new(0.0, 0.0, 0.0),
                false,
                vec![
                    ColliderDesc3d::from_component_str(
                        "shape=cuboid;half_width=2;half_height=2;half_z=2;sensor=true",
                    )
                    .unwrap(),
                ],
            )
            .unwrap();
        world
            .add_body(
                ball,
                Vec3::new(10.0, 0.0, 0.0),
                true,
                vec![ColliderDesc3d::from_component_str("shape=ball;radius=1").unwrap()],
            )
            .unwrap();

        let shapes = world.debug_shapes();
        assert_eq!(shapes.len(), 2, "每个碰撞体一行");
        assert_eq!(shapes[0].entity, wall);
        assert_eq!(
            shapes[0].shape,
            ColliderShape3d::Cuboid(Vec3::new(2.0, 2.0, 2.0))
        );
        assert!(shapes[0].sensor, "sensor 要透出来，绘制时换色");
        assert!(!shapes[0].dynamic, "固定体必须报 dynamic=false");
        assert_eq!(shapes[0].center, Vec3::ZERO);
        assert_eq!(shapes[1].entity, ball);
        assert!(shapes[1].dynamic);
        assert!((shapes[1].center.x - 10.0).abs() < 1e-5);

        // 线框是世界空间的：中心点跟着刚体走，且每条线段都从中心附近展开。
        let segments = shapes[1].wireframe_segments();
        assert_eq!(segments.len(), 48);
        for (from, to) in segments {
            assert!(from.is_finite() && to.is_finite());
            assert!((from.x - 10.0).abs() <= 1.0001, "半径 1 的球：{from:?}");
            assert!((to.x - 10.0).abs() <= 1.0001);
        }
    }

    /// WP7：3D 侧的 `ccd` 与 2D 同义（刚体属性；拼错报错），并且真的阻止高速隧穿。
    #[test]
    fn ccd_stops_a_fast_body_from_tunneling_in_3d() {
        let parsed = ColliderDesc3d::from_component_str("shape=ball;radius=1;ccd=true").unwrap();
        assert!(parsed.ccd);
        assert!(
            ColliderDesc3d::from_component_str("shape=ball;radius=1;ccd=yes").is_err(),
            "ccd 只接受 true/false"
        );

        let run = |ccd: bool| -> f32 {
            let mut world = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
            let wall = entity(1);
            let bullet = entity(2);
            world
                .add_body(
                    wall,
                    Vec3::ZERO,
                    false,
                    vec![
                        ColliderDesc3d::from_component_str(
                            "shape=cuboid;half_width=0.05;half_height=50;half_z=50",
                        )
                        .unwrap(),
                    ],
                )
                .unwrap();
            let mut desc = ColliderDesc3d::from_component_str("shape=ball;radius=0.05").unwrap();
            desc.ccd = ccd;
            world
                .add_body(bullet, Vec3::new(-50.0, 0.0, 0.0), true, vec![desc])
                .unwrap();
            world.set_velocity(bullet, 60_000.0, 0.0, 0.0).unwrap();
            assert!(!world.ccd_enabled(wall).unwrap());
            assert_eq!(world.ccd_enabled(bullet).unwrap(), ccd);
            let moved = world.step(1.0 / 60.0);
            moved
                .iter()
                .find(|(entity, _)| *entity == bullet)
                .map(|(_, position)| position.x)
                .expect("bullet moved")
        };
        let without = run(false);
        let with = run(true);
        assert!(without > 0.0, "不开 CCD 时应穿墙（对照）：x={without}");
        assert!(with < 0.0, "开启 CCD 后不应穿过薄墙：x={with}");

        // 运行时开关与未知实体报错。
        let mut world = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
        let id = entity(1);
        world
            .add_body(id, Vec3::ZERO, true, vec![ball(1.0)])
            .unwrap();
        assert!(!world.ccd_enabled(id).unwrap());
        world.set_ccd(id, true).unwrap();
        assert!(world.ccd_enabled(id).unwrap());
        assert!(world.set_ccd(entity(9), true).is_err());
        assert!(world.ccd_enabled(entity(9)).is_err());
    }

    #[test]
    fn dynamic_body_integrates_velocity_in_zero_gravity() {
        let mut world = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
        let player = entity(1);
        world
            .add_body(player, Vec3::ZERO, true, vec![ball(1.0)])
            .unwrap();
        world.set_velocity(player, 5.0, 0.0, 0.0).unwrap();
        assert_eq!(world.velocity(player).unwrap(), Vec3::new(5.0, 0.0, 0.0));
        let moved = world.step(0.5);
        assert!(moved.contains(&(player, Vec3::new(2.5, 0.0, 0.0))));
    }

    #[test]
    fn body_falls_under_gravity_until_landing_on_ground() {
        let mut world = PhysicsWorld3d::new();
        // A static ground plane whose top surface is y = 0.
        world
            .add_body(
                entity(2),
                Vec3::ZERO,
                false,
                vec![ColliderDesc3d {
                    shape: ColliderShape3d::Cuboid(Vec3::new(20.0, 0.5, 20.0)),
                    offset: Vec3::new(0.0, -0.5, 0.0),
                    sensor: false,
                    layer: 0xFF,
                    mask: 0xFF,
                    ccd: false,
                    dynamic: true,
                }],
            )
            .unwrap();
        let ball_id = entity(1);
        world
            .add_body(ball_id, Vec3::new(0.0, 10.0, 0.0), true, vec![ball(1.0)])
            .unwrap();
        // Step long enough for the ball to fall and settle on the ground.
        let mut final_y = 10.0;
        for _ in 0..600 {
            let moved = world.step(1.0 / 60.0);
            if let Some((_, position)) = moved.iter().find(|(e, _)| *e == ball_id) {
                final_y = position.y;
            }
        }
        // Ball radius 1 resting on top of the plane (top at y=0) => y ≈ 1.
        assert!(
            (final_y - 1.0).abs() < 0.1,
            "ball should rest at y≈1, got y={final_y}"
        );
    }

    #[test]
    fn collision_layers_and_masks_filter_contacts() {
        let mut world = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
        let player = entity(1);
        let wall = entity(2);
        world
            .add_body(
                player,
                Vec3::ZERO,
                true,
                vec![ColliderDesc3d {
                    shape: ColliderShape3d::Ball(1.0),
                    offset: Vec3::ZERO,
                    sensor: false,
                    dynamic: true,
                    layer: 0x01,
                    mask: 0x01,
                    ccd: false,
                }],
            )
            .unwrap();
        world
            .add_body(
                wall,
                Vec3::new(4.0, 0.0, 0.0),
                false,
                vec![ColliderDesc3d {
                    shape: ColliderShape3d::Cuboid(Vec3::new(1.0, 5.0, 5.0)),
                    offset: Vec3::ZERO,
                    sensor: false,
                    layer: 0x02,
                    mask: 0xFF,
                    ccd: false,
                    dynamic: true,
                }],
            )
            .unwrap();
        world.set_velocity(player, 4.0, 0.0, 0.0).unwrap();
        let mut final_x = 0.0;
        for _ in 0..90 {
            let moved = world.step(1.0 / 60.0);
            if let Some((_, position)) = moved.iter().find(|(e, _)| *e == player) {
                final_x = position.x;
            }
        }
        assert!(
            final_x > 3.5,
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
        let mut world = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
        let player = entity(1);
        let wall = entity(2);
        world
            .add_body(player, Vec3::ZERO, true, vec![ball(1.0)])
            .unwrap();
        world
            .add_body(
                wall,
                Vec3::new(4.0, 0.0, 0.0),
                false,
                vec![ColliderDesc3d {
                    shape: ColliderShape3d::Cuboid(Vec3::new(1.0, 5.0, 5.0)),
                    offset: Vec3::ZERO,
                    sensor: false,
                    layer: 0xFF,
                    mask: 0xFF,
                    ccd: false,
                    dynamic: true,
                }],
            )
            .unwrap();
        world.set_velocity(player, 4.0, 0.0, 0.0).unwrap();
        let mut final_x = 0.0;
        for _ in 0..90 {
            let moved = world.step(1.0 / 60.0);
            if let Some((_, position)) = moved.iter().find(|(e, _)| *e == player) {
                final_x = position.x;
            }
        }
        assert!(final_x < 3.0, "player penetrated the wall: x={final_x}");
        let contacts = world.drain_contacts();
        assert!(
            contacts
                .iter()
                .any(|c| (c.a == player && c.b == wall) || (c.a == wall && c.b == player)),
            "no contact reported"
        );
    }

    #[test]
    fn sensor_reports_contact_without_blocking() {
        let mut world = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
        let player = entity(1);
        let trigger = entity(2);
        world
            .add_body(player, Vec3::ZERO, true, vec![ball(1.0)])
            .unwrap();
        world
            .add_body(
                trigger,
                Vec3::new(2.0, 0.0, 0.0),
                false,
                vec![ColliderDesc3d {
                    shape: ColliderShape3d::Ball(1.0),
                    offset: Vec3::ZERO,
                    sensor: true,
                    layer: 0xFF,
                    mask: 0xFF,
                    ccd: false,
                    dynamic: true,
                }],
            )
            .unwrap();
        // Player crosses the sensor; it must report but not stop the player.
        world.set_velocity(player, 4.0, 0.0, 0.0).unwrap();
        let mut final_x = 0.0;
        for _ in 0..120 {
            let moved = world.step(1.0 / 60.0);
            if let Some((_, position)) = moved.iter().find(|(e, _)| *e == player) {
                final_x = position.x;
            }
        }
        assert!(
            final_x > 5.0,
            "sensor must not block the player: x={final_x}"
        );
        let contacts = world.drain_contacts();
        assert!(
            contacts
                .iter()
                .any(|c| (c.a == player && c.b == trigger) || (c.a == trigger && c.b == player)),
            "sensor contact not reported"
        );
    }

    #[test]
    fn raycast_and_overlap_queries_name_entities() {
        let mut world = PhysicsWorld3d::new();
        let target = entity(7);
        world
            .add_body(target, Vec3::new(0.0, 1.0, 0.0), false, vec![ball(1.0)])
            .unwrap();
        // A ray from below straight up hits the sphere.
        let hit = world
            .raycast(Vec3::new(0.0, -5.0, 0.0), Vec3::Y, 100.0)
            .unwrap();
        assert_eq!(hit, Some(target));
        // A ray pointing sideways misses it.
        let miss = world
            .raycast(Vec3::new(0.0, -5.0, 0.0), Vec3::Z, 100.0)
            .unwrap();
        assert_eq!(miss, None);
        // A sphere overlap at the body's position finds it.
        let overlap = world.overlap(Vec3::new(0.0, 1.0, 0.0), 2.0);
        assert!(overlap.contains(&target));
        let empty = world.overlap(Vec3::new(100.0, 100.0, 100.0), 1.0);
        assert!(empty.is_empty());
    }

    #[test]
    fn set_position_teleports_a_body() {
        let mut world = PhysicsWorld3d::new();
        let player = entity(1);
        world
            .add_body(player, Vec3::ZERO, true, vec![ball(1.0)])
            .unwrap();
        world
            .set_position(player, Vec3::new(3.0, 2.0, 1.0))
            .unwrap();
        let moved = world.step(0.016);
        assert!(moved.iter().any(|(e, p)| *e == player && p.y > 1.9));
    }
}
