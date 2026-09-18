//! Rapier-backed 2D physics: rigid bodies, colliders, sensors, and collision
//! events surfaced to Lua. A zero-gravity top-down world stepped at the fixed
//! timestep; dynamic bodies sync their positions back to ECS Transforms.

use std::{collections::HashMap, error::Error, fmt};

use rapier2d::prelude::*;

use crate::core::{
    entity::EntityId,
    transform::{Transform, Vec2},
    world::World,
};

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

/// One collider as it currently sits in the physics world, for debug drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct ColliderDebugShape {
    pub entity: EntityId,
    /// World-space centre (the body's translation plus the rotated local offset).
    pub center: Vec2,
    /// World-space rotation in radians.
    pub rotation: f32,
    pub shape: ColliderShape,
    pub sensor: bool,
}

/// A collider shape.
#[derive(Clone, Debug, PartialEq)]
pub enum ColliderShape {
    Ball(f32),
    Cuboid(Vec2),
}

/// One collider attached to a body, at a local offset. `layer` is the collision
/// group this collider belongs to and `mask` the groups it collides with (both
/// bit fields, default all bits on).
///
/// `ccd` 是**连续碰撞检测**声明：Rapier 的 CCD 是**刚体**属性（不是碰撞体属性），
/// 因此一个刚体上只要有任一 collider 声明 `ccd=true`，该刚体就开启 CCD——见
/// [`PhysicsWorld::add_body`] 的说明。默认关闭（与 Rapier 一致）。
#[derive(Clone, Debug, PartialEq)]
pub struct ColliderDesc {
    pub shape: ColliderShape,
    pub offset: Vec2,
    pub sensor: bool,
    pub layer: u8,
    pub mask: u8,
    pub ccd: bool,
}

impl ColliderDesc {
    /// Parses a scene `Collider` component string:
    /// `shape=ball;radius=..` or `shape=cuboid;half_width=..;half_height=..`,
    /// with optional `;sensor=true`, `;offset=x,y`, `;layer=N`, `;mask=N`, and
    /// `;ccd=true` (连续碰撞检测，默认关闭).
    pub fn from_component_str(value: &str) -> Result<Self, PhysicsError> {
        let mut shape = None;
        let mut offset = Vec2::zero();
        let mut sensor = false;
        let mut layer = 0xFF;
        let mut mask = 0xFF;
        let mut ccd = false;
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
                // WP7：连续碰撞检测。与 `sensor` 同样只接受 true/false（拼错报错，
                // 而不是静默当 false——那会让"高速子弹穿墙"重新出现且毫无线索）。
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
            ccd,
        })
    }
}

/// WP7：谁有权写实体的 `Transform`（docs/40 §3 WP7 的 `PhysicsAuthority`）。
///
/// 此前这件事没有类型、只有约定：`step` 返回被移动的动态刚体、宿主回写 ECS，而
/// `set_position` 是"运动学传送 + 调用方自己同步 ECS"。两套动作同时存在时，"脚本设了位置
/// 又被物理覆盖"这类问题的答案藏在调用顺序里。现在显式声明：
///
/// - [`PhysicsAuthority::Physics`]（默认）：**模拟是世界位置的真源**。每步之后把动态刚体的
///   位置写回 ECS（渲染与 Lua 因此看到模拟结果）；脚本想移动动态体应改速度。
/// - [`PhysicsAuthority::Script`]：**ECS 是权威**。物理步之前先把 ECS 位置推给模拟世界
///   （静态/运动学同步），之后的动态回写被**跳过**——否则模拟会把脚本刚写的位置覆盖掉。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PhysicsAuthority {
    /// 模拟结果回写 ECS（默认，与既有行为逐字节一致）。
    #[default]
    Physics,
    /// ECS/脚本权威：步前同步、步后不回写。
    Script,
}

impl PhysicsAuthority {
    /// 是否在物理步之后把动态刚体位置写回 ECS。
    #[must_use]
    pub const fn writes_back_to_ecs(self) -> bool {
        matches!(self, Self::Physics)
    }

    /// 是否在物理步之前把 ECS 位置推给模拟世界。
    #[must_use]
    pub const fn syncs_before_step(self) -> bool {
        matches!(self, Self::Script)
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
    ///
    /// WP7：任一 collider 声明了 `ccd` 时给该**刚体**开启连续碰撞检测（Rapier 的 CCD
    /// 是刚体属性）。这样"高速子弹/高速下落物穿墙"可以在场景里直接声明，而不必靠
    /// 调高物理步频或把速度调小来绕开。
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
        if colliders.iter().any(|desc| desc.ccd) {
            builder = builder.ccd_enabled(true);
        }
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

    /// Teleports a body to an absolute world position (used for map transitions
    /// and respawns). Mirrors `PhysicsWorld3d::set_position`: the rigid body is
    /// moved kinematically; the caller keeps the ECS `Transform` in sync.
    pub fn set_position(&mut self, entity: EntityId, position: Vec2) -> Result<(), PhysicsError> {
        let handle = self
            .entity_bodies
            .get(&entity)
            .ok_or(PhysicsError::UnknownBody(entity))?;
        if let Some(body) = self.bodies.get_mut(*handle) {
            body.set_translation(Vector::new(position.x, position.y), true);
        }
        Ok(())
    }

    /// WP7 静态/运动学同步的数据源：把**动态**刚体的当前位置从 ECS 取出来（按实体排序，
    /// 保证确定性），供 `authority = Script` 时在物理步之前推给模拟世界。
    ///
    /// 只取动态刚体：静态体是场景数据、运动学体由调用方自己同步（与 `set_position` 的
    /// 既有语义一致）。没有 `Transform` 的实体跳过。
    pub fn kinematic_positions(&self, world: &World) -> Vec<(EntityId, Vec2)> {
        let mut out = Vec::new();
        for (entity, handle) in &self.entity_bodies {
            let Some(body) = self.bodies.get(*handle) else {
                continue;
            };
            if body.body_type() != RigidBodyType::Dynamic {
                continue;
            }
            if let Ok(Some(transform)) = world.get::<Transform>(*entity) {
                out.push((*entity, transform.position));
            }
        }
        out.sort_by_key(|(entity, _)| *entity);
        out
    }

    /// WP7：刚体当前是否开启连续碰撞检测（读写的是 Rapier 里的真实状态，不是引擎侧的
    /// 影子记录——因此"声明生效了没有"可以被直接验证）。
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

    /// WP7：运行时开关连续碰撞检测（Lua `physics.set_ccd`）。
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

    /// 调试绘制用：把**物理世界里当前真实**的碰撞体姿态与形状取出来（WP7 ② 的
    /// collider debug draw 数据源）。
    ///
    /// 刻意读 Rapier 而不是场景里的 `Collider` 组件字符串：组件只在实例化时被解析一次，
    /// 之后再没人更新，而动态刚体的实际位置/旋转每步都在变——调试绘制要看的正是后者
    /// （"物理到底把它放到哪了"）。返回世界坐标下的中心点与旋转，局部偏移已经旋转叠加。
    pub fn debug_shapes(&self) -> Vec<ColliderDebugShape> {
        let mut shapes = Vec::new();
        for (entity, handle) in &self.entity_bodies {
            let Some(body) = self.bodies.get(*handle) else {
                continue;
            };
            let body_translation = body.translation();
            let body_angle = body.rotation().angle();
            for collider_handle in body.colliders() {
                let Some(collider) = self.colliders.get(*collider_handle) else {
                    continue;
                };
                // Rapier 只给出 `&dyn Shape`，靠 downcast 认形状；认不出的形状不画，
                // 不做静默近似。
                let shape_ref = collider.shape();
                let shape = if let Some(ball) = shape_ref
                    .as_any()
                    .downcast_ref::<rapier2d::geometry::Ball>()
                {
                    ColliderShape::Ball(ball.radius)
                } else if let Some(cuboid) = shape_ref
                    .as_any()
                    .downcast_ref::<rapier2d::geometry::Cuboid>()
                {
                    ColliderShape::Cuboid(Vec2::new(cuboid.half_extents.x, cuboid.half_extents.y))
                } else {
                    // 其它形状（胶囊/多边形等）暂不绘制：调试绘制不做静默近似。
                    continue;
                };
                // 局部偏移必须取**相对父刚体**的那一份：`collider.translation()` 是
                // 世界位置（`PhysicsPipeline::step` 会把它按父体姿态重算），拿它再叠一次
                // 父体位移就是把父体位置算两遍。
                let local_offset = collider
                    .position_wrt_parent()
                    .map(|pose| pose.translation.vector)
                    .unwrap_or(*collider.translation());
                let local_angle = collider
                    .position_wrt_parent()
                    .map(|pose| pose.rotation.angle())
                    .unwrap_or_else(|| collider.rotation().angle());
                let (sin, cos) = body_angle.sin_cos();
                shapes.push(ColliderDebugShape {
                    entity: *entity,
                    center: Vec2::new(
                        body_translation.x + local_offset.x * cos - local_offset.y * sin,
                        body_translation.y + local_offset.x * sin + local_offset.y * cos,
                    ),
                    rotation: body_angle + local_angle,
                    shape,
                    sensor: collider.is_sensor(),
                });
            }
        }
        // 顺序必须确定：`entity_bodies` 是 HashMap，直接遍历会让调试顶点的顺序（以及
        // 任何基于它的对拍）随哈希种子变化。
        shapes.sort_by_key(|shape| {
            (
                shape.entity,
                shape.center.x.to_bits(),
                shape.center.y.to_bits(),
            )
        });
        shapes
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

    /// WP7：连续碰撞检测（CCD）从组件串声明，落到 Rapier 的**刚体**上；拼错报错而不是
    /// 静默当 false（拼错被忽略会重新出现"高速穿墙且毫无线索"）。
    #[test]
    fn ccd_is_declared_per_body_and_can_be_toggled_at_runtime() {
        let off = ColliderDesc::from_component_str("shape=ball;radius=2").unwrap();
        assert!(!off.ccd, "默认关闭，与 Rapier 一致");
        let on = ColliderDesc::from_component_str("shape=ball;radius=2;ccd=true").unwrap();
        assert!(on.ccd);
        let explicit_off =
            ColliderDesc::from_component_str("shape=ball;radius=2;ccd=false").unwrap();
        assert!(!explicit_off.ccd);
        assert!(
            ColliderDesc::from_component_str("shape=ball;radius=2;ccd=yes").is_err(),
            "ccd 只接受 true/false"
        );

        let mut world = PhysicsWorld::new();
        let fast = entity(1);
        let slow = entity(2);
        world
            .add_body(fast, Vec2::zero(), true, vec![on.clone()])
            .unwrap();
        world.add_body(slow, Vec2::zero(), true, vec![off]).unwrap();
        // 读的是 Rapier 里的真实状态。
        assert!(world.ccd_enabled(fast).unwrap());
        assert!(!world.ccd_enabled(slow).unwrap());
        // 运行时可开关，且未知实体报错（不静默）。
        world.set_ccd(slow, true).unwrap();
        assert!(world.ccd_enabled(slow).unwrap());
        world.set_ccd(slow, false).unwrap();
        assert!(!world.ccd_enabled(slow).unwrap());
        assert!(world.set_ccd(entity(9), true).is_err());
        assert!(world.ccd_enabled(entity(9)).is_err());
    }

    /// WP7 CCD 的行为证据：一步之内跨越整堵墙的高速刚体，开启 CCD 后不再穿过去。
    ///
    /// 用速度 × 步长远超墙厚来构造"必然会隧穿"的情形（不开 CCD 时小球一步越过墙，
    /// 断言里也把它钉住），因此这条测试不会因为调参而变成空转。
    #[test]
    fn ccd_stops_a_fast_body_from_tunneling_through_a_thin_wall() {
        let run = |ccd: bool| -> f32 {
            let mut world = PhysicsWorld::new();
            let wall = entity(1);
            let bullet = entity(2);
            world
                .add_body(
                    wall,
                    Vec2::new(0.0, 0.0),
                    false,
                    vec![
                        ColliderDesc::from_component_str(
                            "shape=cuboid;half_width=1;half_height=50",
                        )
                        .unwrap(),
                    ],
                )
                .unwrap();
            let mut desc =
                ColliderDesc::from_component_str("shape=ball;radius=1").expect("ball collider");
            desc.ccd = ccd;
            world
                .add_body(bullet, Vec2::new(-50.0, 0.0), true, vec![desc])
                .unwrap();
            world.set_velocity(bullet, 60_000.0, 0.0).unwrap();
            // 一步 = 1000 世界单位，远大于墙厚 2。
            let moved = world.step(1.0 / 60.0);
            moved
                .iter()
                .find(|(entity, _)| *entity == bullet)
                .map(|(_, position)| position.x)
                .expect("bullet moved")
        };
        let without = run(false);
        let with = run(true);
        assert!(
            without > 0.0,
            "不开 CCD 时应当穿墙（这正是这条测试的对照）：x={without}"
        );
        assert!(with < 0.0, "开启 CCD 后高速小球不应穿过薄墙：x={with}");
    }

    #[test]
    fn debug_shapes_report_world_pose_offset_and_sensor() {
        // 调试绘制必须读到**物理世界的当前姿态**，而不是场景里那份只在实例化时解析过
        // 一次的组件串：动态刚体每步都在动。
        let mut world = PhysicsWorld::new();
        let moving = entity(1);
        world
            .add_body(
                moving,
                Vec2::new(10.0, 20.0),
                true,
                vec![ColliderDesc {
                    shape: ColliderShape::Ball(4.0),
                    offset: Vec2::new(2.0, 0.0),
                    sensor: true,
                    layer: 0xFF,
                    mask: 0xFF,
                    ccd: false,
                }],
            )
            .unwrap();
        let shapes = world.debug_shapes();
        assert_eq!(shapes.len(), 1, "每个碰撞体一行");
        let ball = &shapes[0];
        assert_eq!(ball.entity, moving);
        assert_eq!(ball.shape, ColliderShape::Ball(4.0));
        assert!(ball.sensor, "sensor 标记要透出来，绘制时才能换色");
        // 未旋转时局部偏移原样叠加。
        assert!((ball.center.x - 12.0).abs() < 1e-5);
        assert!((ball.center.y - 20.0).abs() < 1e-5);
        assert!(ball.rotation.abs() < 1e-5);

        // 刚体旋转 90°：局部偏移 (2,0) 必须跟着转到世界 (0,2) 方向。
        // 测试在模块内，直接改 Rapier 刚体，不为此新增生产 API。
        if let Some(handle) = world.entity_bodies.get(&moving).copied()
            && let Some(body) = world.bodies.get_mut(handle)
        {
            body.set_rotation(
                rapier2d::na::UnitComplex::new(std::f32::consts::FRAC_PI_2),
                true,
            );
        }
        let rotated = world.debug_shapes();
        let ball = &rotated[0];
        assert!((ball.center.x - 10.0).abs() < 1e-4, "{:?}", ball.center);
        assert!((ball.center.y - 22.0).abs() < 1e-4, "{:?}", ball.center);
        assert!((ball.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
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
                    ccd: false,
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
                    ccd: false,
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
                    ccd: false,
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
                    ccd: false,
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
                    ccd: false,
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
