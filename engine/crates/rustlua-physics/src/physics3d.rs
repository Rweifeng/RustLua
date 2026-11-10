//! Rapier-backed 3D physics: rigid bodies, colliders (ball/cuboid/capsule),
//! sensors, ray/overlap queries, and collision events surfaced to Lua. Uses
//! the ADR-0007 right-handed +Y-up convention; gravity defaults to −Y.
//! Dynamic bodies sync their translations back to ECS `Transform3D` each fixed
//! step (rotation and scale stay owned by the scene).

use std::{collections::HashMap, fmt};

use rapier3d::prelude::*;

use crate::physics::{PhysicsContact, PhysicsError};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::transform::Transform3D;
use rustlua_ecs::world::World;
use rustlua_math::math3d::{Quat, Vec3};

/// A 3D collider shape. `Capsule` is aligned with the local Y axis.
#[derive(Clone, Debug, PartialEq)]
pub enum ColliderShape3d {
    Ball(f32),
    Cuboid(Vec3),
    Capsule {
        half_height: f32,
        radius: f32,
    },
    /// 静态地形高度场（P3-R55）：`heights` 行主序，`rows × cols`；
    /// 局部 XZ 由 `scale.x/z` 铺开，Y 由 `scale.y` 乘高度。
    Heightfield {
        heights: Vec<f32>,
        rows: usize,
        cols: usize,
        scale: Vec3,
    },
}

/// One 3D collider as it currently sits in the physics world, for debug drawing.
///
/// 与 2D 的 [`crate::physics::ColliderDebugShape`] 同构（同名字段、同一套
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
            ColliderShape3d::Heightfield { .. } => {}
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
            ColliderShape3d::Heightfield { .. } => {
                return Err(PhysicsError::Malformed(
                    "heightfield must be added via add_heightfield".to_owned(),
                ));
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
    /// P3-③（`docs/60` §6.3）：查询 BVH 的**脏标记**。`add_body` / `remove_body` 只置脏，
    /// 真正重建推迟到下一次查询（`refresh_queries`）或 `step`（Rapier 内部重建）。
    /// 2D 早就是这样（`physics.rs`），3D 此前是"每次增删立即 `update`"——场景加载时 N 个物体
    /// 就是 N 次全量重建。
    query_dirty: bool,
    hooks: NoHooks3d,
    collector: CollisionCollector3d,
    entity_bodies: HashMap<EntityId, RigidBodyHandle>,
    collider_entities: HashMap<ColliderHandle, EntityId>,
    contacts: Vec<PhysicsContact>,
    /// E03-3：与 `contacts` 同时写入、独立消费，供实体 `on_collision` 分发；
    /// 不抢 Lua `physics.events()` 的那一份。
    event_contacts: Vec<PhysicsContact>,
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
            query_dirty: false,
            hooks: NoHooks3d,
            collector: CollisionCollector3d::default(),
            entity_bodies: HashMap::new(),
            collider_entities: HashMap::new(),
            contacts: Vec::new(),
            event_contacts: Vec::new(),
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
                ColliderShape3d::Heightfield {
                    heights,
                    rows,
                    cols,
                    scale,
                } => {
                    let matrix = nalgebra::DMatrix::from_row_slice(rows, cols, &heights);
                    ColliderBuilder::heightfield(matrix, Vector::new(scale.x, scale.y, scale.z))
                }
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
        // P3-③（docs/60 §6.3）：只置脏，不在这里重建 BVH——批量 add（场景加载）时"每次一遍
        // 全量重建"纯属浪费（N 个物体 = N 次 O(n) 重建）。兑现点有两个：查询前的
        // `refresh_queries`（保证"add 之后立刻 raycast 看得见新碰撞体"的既有契约不被延迟破坏），
        // 以及 `step`（Rapier 在内部重建）。
        self.query_dirty = true;
        Ok(())
    }

    /// 添加一个静态地形高度场（P3-R55）。
    ///
    /// `heights` 行主序、`rows × cols`；局部 XZ 由 `scale.x/z` 铺开，高度乘 `scale.y`。
    /// 高度场只作为**固定体**，用于射线/动态体碰撞；不参与 `debug_shapes`（不是简单形状）。
    pub fn add_heightfield(
        &mut self,
        entity: EntityId,
        position: Vec3,
        scale: Vec3,
        rows: usize,
        cols: usize,
        heights: Vec<f32>,
    ) -> Result<(), PhysicsError> {
        if rows < 2 || cols < 2 || rows > 1025 || cols > 1025 {
            return Err(PhysicsError::Malformed(
                "heightfield rows/cols must be 2..=1025".to_owned(),
            ));
        }
        if heights.len() != rows * cols {
            return Err(PhysicsError::Malformed(format!(
                "heightfield needs {} heights, got {}",
                rows * cols,
                heights.len()
            )));
        }
        if !position.x.is_finite()
            || !position.y.is_finite()
            || !position.z.is_finite()
            || !scale.x.is_finite()
            || !scale.y.is_finite()
            || !scale.z.is_finite()
            || scale.x <= 0.0
            || scale.y <= 0.0
            || scale.z <= 0.0
            || heights.iter().any(|height| !height.is_finite())
        {
            return Err(PhysicsError::Malformed(
                "heightfield values must be finite and scale positive".to_owned(),
            ));
        }
        let desc = ColliderDesc3d {
            shape: ColliderShape3d::Heightfield {
                heights,
                rows,
                cols,
                scale,
            },
            offset: Vec3::ZERO,
            sensor: false,
            dynamic: false,
            layer: 0xFF,
            mask: 0xFF,
            ccd: false,
        };
        self.add_body(entity, position, false, vec![desc])
    }

    /// 添加一个静态**逐格体素**碰撞体（P3-R215）。
    ///
    /// 与渲染侧 `__cells__`（`rustlua-render::cellmesh`）同源的一张实心格位图：`solids` 是
    /// 0 基平铺下标（`x + w * (z + d * y)`，与 `CellMeshSpec::index` 同式），区域 min 角落在
    /// `position`。格位图先贪心并块成尽量少的轴对齐长方体，再作为同一刚体上的多个 `Cuboid`
    /// 插入 ⇒ 碰撞体数量只跟表面的块状程度走，不跟格数走（3.5 k 格的巢并到百级）。
    ///
    /// **语义是替换**：这条 API 没有 Lua 侧的 remove 通路，而「挖一格」的唯一做法就是重建位图，
    /// 所以实体已有刚体时先删旧的。返回并块后的长方体个数，是给门的读数。
    pub fn add_cells(
        &mut self,
        entity: EntityId,
        position: Vec3,
        cell: f32,
        dims: [usize; 3],
        solids: &[usize],
    ) -> Result<usize, PhysicsError> {
        crate::voxel::validate(dims, solids).map_err(|error| {
            PhysicsError::Malformed(match error {
                crate::voxel::VoxelError::Axis => "cells axis must be 1..=64".to_owned(),
                crate::voxel::VoxelError::Total => "cells total must be <= 32768".to_owned(),
                crate::voxel::VoxelError::IndexOutOfRange(index) => {
                    format!("cell index {index} is outside the region")
                }
                crate::voxel::VoxelError::Duplicate(index) => {
                    format!("cell index {index} is listed twice")
                }
            })
        })?;
        if !position.x.is_finite()
            || !position.y.is_finite()
            || !position.z.is_finite()
            || !cell.is_finite()
            || !(crate::voxel::MIN_CELL..=crate::voxel::MAX_CELL).contains(&cell)
        {
            return Err(PhysicsError::Malformed(
                "cell size must be finite and within 0.05..=64 metres".to_owned(),
            ));
        }
        let boxes = crate::voxel::merge(dims, solids);
        let descs: Vec<ColliderDesc3d> = boxes
            .into_iter()
            .map(|voxel| {
                let extent = [
                    voxel.extent[0] as f32 * cell,
                    voxel.extent[1] as f32 * cell,
                    voxel.extent[2] as f32 * cell,
                ];
                let center = [
                    (voxel.min[0] as f32 + voxel.extent[0] as f32 * 0.5) * cell,
                    (voxel.min[1] as f32 + voxel.extent[1] as f32 * 0.5) * cell,
                    (voxel.min[2] as f32 + voxel.extent[2] as f32 * 0.5) * cell,
                ];
                ColliderDesc3d {
                    shape: ColliderShape3d::Cuboid(Vec3::new(
                        extent[0] * 0.5,
                        extent[1] * 0.5,
                        extent[2] * 0.5,
                    )),
                    offset: Vec3::new(center[0], center[1], center[2]),
                    sensor: false,
                    dynamic: false,
                    layer: 0xFF,
                    mask: 0xFF,
                    ccd: false,
                }
            })
            .collect();
        let count = descs.len();
        if self.entity_bodies.contains_key(&entity) {
            self.remove_body(entity)?;
        }
        self.add_body(entity, position, false, descs)?;
        Ok(count)
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
        // P3-③：同上只置脏——删除也常成批发生（场景切换、实体销毁），不该每次重建。
        self.query_dirty = true;
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
        // Rapier 在 step 内部（传入 `Some(&mut query_pipeline)` 时）重建查询 BVH，
        // 脏标记到此已被兑现。（与 2D 逐字同构。）
        self.query_dirty = false;
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
                let contact = PhysicsContact {
                    a: entity_a,
                    b: entity_b,
                    started,
                };
                self.contacts.push(contact);
                self.event_contacts.push(contact);
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

    /// E03-3：取走本帧给实体脚本的碰撞事件（与 `drain_contacts` 互不影响）。
    pub fn drain_event_contacts(&mut self) -> Vec<PhysicsContact> {
        std::mem::take(&mut self.event_contacts)
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

    /// P3-③（`docs/60` §6.3）：查询前兑现脏标记——保证"add 之后立刻 raycast 看得见新碰撞体"
    /// 的既有契约不因延迟重建而破坏。`step()` 里 Rapier 自己会重建，故步过之后无需再动。
    /// （与 2D 的 `PhysicsWorld::refresh_queries` 逐字同构。）
    fn refresh_queries(&mut self) {
        if self.query_dirty {
            self.query_pipeline.update(&self.colliders);
            self.query_dirty = false;
        }
    }

    /// Casts a ray and returns the closest hit entity within `max_distance`.
    /// The direction vector must be non-zero.
    pub fn raycast(
        &mut self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Result<Option<EntityId>, PhysicsError> {
        self.raycast_filtered(origin, direction, max_distance, None)
    }

    /// 射线查询 + 碰撞分组过滤（与 2D `PhysicsWorld::raycast_filtered` 同一语义：
    /// 两位测试，`None` 保持"全收"的老行为）。
    pub fn raycast_filtered(
        &mut self,
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
        // P3-③：查询前兑现脏标记（与 2D 同一处、同一理由）。

        self.refresh_queries();

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
    pub fn overlap(&mut self, center: Vec3, radius: f32) -> Vec<EntityId> {
        self.overlap_filtered(center, radius, None)
    }

    /// 球重叠查询 + 碰撞分组过滤（见 [`PhysicsWorld3d::raycast_filtered`]）。
    pub fn overlap_filtered(
        &mut self,
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
        // P3-③：查询前兑现脏标记（与 2D 同一处、同一理由）。
        self.refresh_queries();
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
    fn heightfield_is_fixed_and_raycastable() {
        let mut world = PhysicsWorld3d::new();
        let ground = entity(9);
        world
            .add_heightfield(
                ground,
                Vec3::ZERO,
                Vec3::new(20.0, 1.0, 20.0),
                3,
                3,
                vec![0.0; 9],
            )
            .unwrap();
        let hit = world
            .raycast(Vec3::new(0.0, 5.0, 0.0), Vec3::new(0.0, -1.0, 0.0), 20.0)
            .unwrap();
        assert_eq!(hit, Some(ground), "ray must hit the heightfield");
        assert!(
            world
                .add_heightfield(
                    entity(10),
                    Vec3::ZERO,
                    Vec3::new(20.0, 1.0, 20.0),
                    3,
                    3,
                    vec![0.0; 8],
                )
                .is_err(),
            "height count must equal rows*cols"
        );
        assert!(
            world
                .add_heightfield(
                    entity(11),
                    Vec3::ZERO,
                    Vec3::new(20.0, 0.0, 20.0),
                    3,
                    3,
                    vec![0.0; 9],
                )
                .is_err(),
            "scale must be positive"
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

    /// `docs/60` §6.3：把 2D 已有的查询脏标记搬到 3D。契约三条，缺一条就会退化成
    /// "每次增删立即全量重建"（旧行为）或"查询看到旧 BVH"（延迟没兑现）：
    ///  ① 增删只**置脏**，不在 add/remove 路径里重建；
    ///  ② 查询前兑现（`raycast` / `overlap` 走同一份 `refresh_queries`），
    ///     且"add 之后立刻 raycast 看得见新碰撞体"这条既有契约不被延迟破坏；
    ///  ③ `step` 之后脏标记也算已兑现（Rapier 在内部重建），无需额外一次 update。
    #[test]
    fn query_dirty_defers_the_bvh_rebuild_to_the_next_query_or_step() {
        let mut world = PhysicsWorld3d::new();
        assert!(!world.query_dirty, "新建世界不该带着脏标记");

        let target = entity(3);
        world
            .add_body(target, Vec3::new(0.0, 1.0, 0.0), false, vec![ball(1.0)])
            .unwrap();
        assert!(
            world.query_dirty,
            "add_body 只置脏——在增删路径里重建正是被修掉的旧行为"
        );

        // 兑现点 ①：查询。
        let hit = world
            .raycast(Vec3::new(0.0, -5.0, 0.0), Vec3::Y, 100.0)
            .unwrap();
        assert_eq!(hit, Some(target), "兑现脏标记后必须命中刚加的物体");
        assert!(!world.query_dirty, "查询之后脏标记必须已兑现");

        // 兑现点 ②：step（Rapier 内部重建）。
        world
            .add_body(entity(4), Vec3::new(5.0, 0.0, 0.0), false, vec![ball(1.0)])
            .unwrap();
        assert!(world.query_dirty);
        world.step(1.0 / 60.0);
        assert!(!world.query_dirty, "step 内部重建过，脏标记必须清掉");

        // 删除路径同样只置脏，且兑现后不再命中被删掉的物体（改了不兑现就会命中旧 BVH）。
        world.remove_body(target).unwrap();
        assert!(world.query_dirty, "remove_body 也只置脏");
        let gone = world
            .raycast(Vec3::new(0.0, -5.0, 0.0), Vec3::Y, 100.0)
            .unwrap();
        assert_eq!(gone, None, "兑现之后射线不该再命中已删除的物体");
        assert!(!world.query_dirty);
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

    /// 并块本身：一整块 4×3×1 只要 1 个长方体（横向贪心 + 竖向接柱），
    /// 挖掉中间一格之后必须多出一块——否则「挖」在物理侧不可见。
    #[test]
    fn voxel_merge_collapses_runs_and_columns() {
        let dims = [4usize, 3, 1];
        let full: Vec<usize> = (0..12).collect();
        let boxes = crate::voxel::merge(dims, &full);
        assert_eq!(boxes.len(), 1, "整块应该并成一个长方体：{boxes:?}");
        assert_eq!(boxes[0].min, [0, 0, 0]);
        assert_eq!(boxes[0].extent, [4, 3, 1]);

        // 挖掉 y=1 层 x=2 那一格：那一层被劈成两段（左 2 格 + 右 1 格），上下两层各自成块。
        // 竖向接柱接不上是**正确**行为：中间层断了，y=0 与 y=2 的矩形不再连续。
        let dug: Vec<usize> = full
            .iter()
            .copied()
            .filter(|&i| i != crate::voxel::flat_index(dims, 2, 1, 0))
            .collect();
        let boxes = crate::voxel::merge(dims, &dug);
        assert_eq!(boxes.len(), 4, "挖一格应该多出一段：{boxes:?}");
        let total_cells: usize = boxes
            .iter()
            .map(|b| b.extent[0] * b.extent[1] * b.extent[2])
            .sum();
        assert_eq!(total_cells, 11, "并块不能凭空增减格子");
    }

    /// 因果判据：墙挡得住射线，挖穿的那一格让射线过去，其余格仍然挡。
    #[test]
    fn add_cells_blocks_rays_and_a_dug_cell_lets_one_through() {
        let mut world = PhysicsWorld3d::new();
        let wall = entity(21);
        let dims = [1usize, 5, 3];
        let full: Vec<usize> = (0..15).collect();
        let merged = world
            .add_cells(wall, Vec3::ZERO, 1.0, dims, &full)
            .expect("wall");
        assert_eq!(merged, 1, "整面墙并成 1 个长方体");
        // 并块后的形状就是普通 Cuboid ⇒ `debug_draw` 看得见（与高度场不同），
        // 半尺寸/中心都按格算：dims [1,5,3] × cell 1 ⇒ 半边 (0.5, 2.5, 1.5)。
        let wire = world.debug_shapes();
        assert_eq!(wire.len(), 1, "debug_draw 应该收到并出来的那块：{wire:?}");
        assert_eq!(wire[0].entity, wall);
        assert!(!wire[0].dynamic);
        let ColliderShape3d::Cuboid(half) = wire[0].shape else {
            panic!("并块出来的碰撞体必须是 Cuboid，拿到 {:?}", wire[0].shape);
        };
        assert_eq!(half, Vec3::new(0.5, 2.5, 1.5));
        assert_eq!(wire[0].center, Vec3::new(0.5, 2.5, 1.5));
        let mid_y = 2.5;
        assert_eq!(
            world
                .raycast(Vec3::new(-5.0, mid_y, 1.5), Vec3::X, 20.0)
                .unwrap(),
            Some(wall),
            "未挖的墙必须挡住射线"
        );
        // 挖掉 (x=0, y=2, z=1) 这一格之后重建同一实体。
        let hole = crate::voxel::flat_index(dims, 0, 2, 1);
        let dug: Vec<usize> = full.iter().copied().filter(|&i| i != hole).collect();
        let merged = world.add_cells(wall, Vec3::ZERO, 1.0, dims, &dug).unwrap();
        assert_eq!(
            world
                .raycast(Vec3::new(-5.0, 2.5, 1.5), Vec3::X, 20.0)
                .unwrap(),
            None,
            "挖穿的那一格必须让射线过去"
        );
        assert_eq!(
            world
                .raycast(Vec3::new(-5.0, 0.5, 1.5), Vec3::X, 20.0)
                .unwrap(),
            Some(wall),
            "相邻的格还得挡"
        );
        assert!(merged > 1, "挖开后并块数应涨：{merged}");
    }

    /// 动态体真的被逐格体素接住（不是只有射线看得见）。
    #[test]
    fn dynamic_body_rests_on_a_voxel_floor() {
        let mut world = PhysicsWorld3d::new();
        let floor = entity(22);
        let dims = [4usize, 1, 4];
        let full: Vec<usize> = (0..16).collect();
        world
            .add_cells(floor, Vec3::ZERO, 1.0, dims, &full)
            .unwrap();
        let ball_id = entity(23);
        world
            .add_body(ball_id, Vec3::new(2.0, 5.0, 2.0), true, vec![ball(0.5)])
            .unwrap();
        let mut resting = Vec3::ZERO;
        for _ in 0..120 {
            for (e, p) in world.step(1.0 / 60.0) {
                if e == ball_id {
                    resting = p;
                }
            }
        }
        assert!(
            (resting.y - 1.5).abs() < 0.1,
            "球应该停在地板面上（y≈1.5），实得 y={}",
            resting.y
        );
        assert!(
            world
                .debug_shapes()
                .iter()
                .any(|shape| shape.entity == floor),
            "逐格长方体要进调试形状"
        );
    }

    /// 边界与错误形状：出界/重复/超上限都报错，不夹取（与 `cells3d` 同口径）。
    #[test]
    fn add_cells_rejects_malformed_fields() {
        let mut world = PhysicsWorld3d::new();
        let cases: [(&str, [usize; 3], Vec<usize>, f32); 5] = [
            ("单轴 65", [65, 1, 1], vec![0], 1.0),
            ("总格数超限", [64, 64, 9], vec![0], 1.0),
            ("下标出界", [2, 2, 2], vec![8], 1.0),
            ("下标重复", [2, 2, 2], vec![1, 1], 1.0),
            ("格太小", [2, 2, 2], vec![0], 0.04),
        ];
        for (name, dims, solids, cell) in cases {
            let error = world
                .add_cells(entity(30), Vec3::ZERO, cell, dims, &solids)
                .err()
                .unwrap_or_else(|| panic!("{name} 应该报错却通过了"));
            assert!(
                matches!(error, PhysicsError::Malformed(_)),
                "{name} 的形状不对：{error:?}"
            );
        }
        // 上限内侧要过：64×64×8 = 32768 正好到总格上限。
        assert_eq!(crate::voxel::MAX_AXIS, 64);
        assert_eq!(crate::voxel::MAX_CELLS, 64 * 64 * 8);
        world
            .add_cells(entity(31), Vec3::ZERO, 1.0, [64, 64, 8], &[0])
            .expect("边界内侧必须通过");
    }

    /// 平铺下标用的是**深度** `dims[2]`，不是层数 `dims[1]`：立方区域两种写法自洽，
    /// 只有非立方区域能分辨（与渲染侧 `CellMeshSpec::index` 必须同式）。
    #[test]
    fn flat_index_uses_depth_not_layer_count() {
        let dims = [2usize, 3, 5];
        assert_eq!(
            crate::voxel::flat_index(dims, 0, 1, 0),
            10,
            "一层应该是 w*d = 10 格"
        );
        assert_eq!(
            crate::voxel::flat_index(dims, 1, 2, 4),
            2 * 3 * 5 - 1,
            "区域最后一个格必须落在 total-1"
        );
    }

    /// Myrmex 尺寸的读数：一整块巢并成 1 个长方体，剖一间 3×3×3 腔之后仍是常数级。
    #[test]
    fn myrmex_sized_field_merges_to_a_few_colliders() {
        let dims = [21usize, 9, 21];
        let total = 21 * 9 * 21;
        let full: Vec<usize> = (0..total).collect();
        assert_eq!(crate::voxel::merge(dims, &full).len(), 1);

        // 挖一间室（3×3 平面 × 3 层）+ 一条 3×3 竖井，看并块后的数量级。
        let mut carved = vec![false; total];
        for y in 4..7 {
            for z in 9..12 {
                for x in 9..12 {
                    carved[crate::voxel::flat_index(dims, x, y, z)] = true;
                }
            }
        }
        for y in 0..9 {
            for z in 9..12 {
                for x in 9..12 {
                    carved[crate::voxel::flat_index(dims, x, y, z)] = true;
                }
            }
        }
        let solids: Vec<usize> = (0..total).filter(|&i| !carved[i]).collect();
        let boxes = crate::voxel::merge(dims, &solids);
        let merged: usize = boxes
            .iter()
            .map(|b| b.extent[0] * b.extent[1] * b.extent[2])
            .sum();
        println!(
            "voxel_merge_reading: cells={} boxes={} cells_covered={}",
            solids.len(),
            boxes.len(),
            merged
        );
        assert_eq!(merged, solids.len(), "并块必须恰好覆盖所有实心格");
        assert!(
            boxes.len() < 64,
            "Myrmex 尺寸的巢并块后应该只有几十级长方体，实得 {}",
            boxes.len()
        );
    }

    /// P3-R232：碰撞体个数 → 每步物理成本的**确定性**曲线（活体四档只有 13 / 8 两个点，
    /// 且整帧漂移把差值压到 0.2 ms，见 Myrmex §21.511）。这里在物理域内造三个量级：
    /// 空世界 / 并块后的十几个 / 逐格不并的几千个，同一张位图，只差碰撞体切分。
    ///
    /// 计时用 `Instant` 而不是 `pipeline.counters`：rapier 0.22 的 `Timer::start/pause/resume`
    /// 整体挂在 `#[cfg(feature = "profiler")]` 上，而该 feature 非默认 ⇒ `counters.enable()`
    /// 之后所有阶段读数恒为 0，拿它归因是空洞通过。
    ///
    /// 第四臂白盒隔离查询 BVH：`step()` 无条件传 `Some(&mut query_pipeline)`，rapier 在最后一个
    /// 子步做 `refit + rebalance`（`physics_pipeline.rs` 的 `update_incremental(..., true)`），
    /// 全静态世界也一样做。这一臂用同一条 rapier 调用但传 `None`，差值就是那一段的价钱。
    #[test]
    fn cells_collider_count_cost_curve() {
        const STEPS: usize = 30;
        let dims = [21usize, 9, 21];
        let total = dims[0] * dims[1] * dims[2];
        let mut carved = vec![false; total];
        for y in 0..dims[1] {
            for z in 9..12 {
                for x in 9..12 {
                    carved[crate::voxel::flat_index(dims, x, y, z)] = true;
                }
            }
        }
        let solids: Vec<usize> = (0..total).filter(|&i| !carved[i]).collect();

        let mut empty = PhysicsWorld3d::new();
        let mut merged = PhysicsWorld3d::new();
        let boxes = merged
            .add_cells(entity(40), Vec3::ZERO, 0.5, dims, &solids)
            .expect("merged field");
        let mut per_cell = PhysicsWorld3d::new();
        let descs: Vec<ColliderDesc3d> = solids
            .iter()
            .map(|&i| {
                let x = i % dims[0];
                let z = (i / dims[0]) % dims[2];
                let y = i / (dims[0] * dims[2]);
                ColliderDesc3d {
                    shape: ColliderShape3d::Cuboid(Vec3::new(0.25, 0.25, 0.25)),
                    offset: Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5) * 0.5,
                    sensor: false,
                    layer: 0xFF,
                    mask: 0xFF,
                    ccd: false,
                    dynamic: false,
                }
            })
            .collect();
        let cells = descs.len();
        per_cell
            .add_body(entity(41), Vec3::ZERO, false, descs)
            .expect("per-cell field");
        let mut no_query = PhysicsWorld3d::new();
        no_query
            .add_cells(entity(42), Vec3::ZERO, 0.5, dims, &solids)
            .expect("no-query field");

        let mut timings: Vec<(&str, usize, f64)> = Vec::new();
        for (name, count, world) in [
            ("empty", 0usize, &mut empty),
            ("merged", boxes, &mut merged),
            ("per_cell", cells, &mut per_cell),
        ] {
            let start = std::time::Instant::now();
            for _ in 0..STEPS {
                world.step(1.0 / 60.0);
            }
            timings.push((
                name,
                count,
                start.elapsed().as_secs_f64() * 1e6 / STEPS as f64,
            ));
        }
        // 白盒臂：与 `step()` 同一条 rapier 调用，只把查询管线换成 `None`。
        let integration = IntegrationParameters {
            dt: 1.0 / 60.0,
            ..Default::default()
        };
        let start = std::time::Instant::now();
        for _ in 0..STEPS {
            no_query.pipeline.step(
                &no_query.gravity,
                &integration,
                &mut no_query.islands,
                &mut no_query.broad_phase,
                &mut no_query.narrow_phase,
                &mut no_query.bodies,
                &mut no_query.colliders,
                &mut no_query.impulse_joints,
                &mut no_query.multibody_joints,
                &mut no_query.ccd_solver,
                None,
                &no_query.hooks,
                &no_query.collector,
            );
        }
        timings.push((
            "merged_no_query",
            boxes,
            start.elapsed().as_secs_f64() * 1e6 / STEPS as f64,
        ));

        // 活体形状复现：Myrmex 的 3D 物理世界 = 一张 33×33 地形高度场 + 地下的巢体格体
        // （`view/terrain3d.lua` 的 cells=32 ⇒ rows=cols=33）。活体四档里"巢体在/不在"差 0.9 ms，
        // 而 13 盒 vs 8 盒只差 0.2 ms ⇒ 怀疑贵在与高度场的窄相，而不是盒数。这三臂把
        // "有没有高度场"与"巢体贴不贴着地表"分开。位图/尺寸是复现，不是活体那张。
        let heights = vec![0.0f32; 33 * 33];
        let scale = Vec3::new(256.0, 1.0, 256.0);
        let mut hf_only = PhysicsWorld3d::new();
        hf_only
            .add_heightfield(entity(50), Vec3::ZERO, scale, 33, 33, heights.clone())
            .expect("heightfield");
        let mut hf_near = PhysicsWorld3d::new();
        hf_near
            .add_heightfield(entity(51), Vec3::ZERO, scale, 33, 33, heights.clone())
            .expect("heightfield");
        let near_boxes = hf_near
            .add_cells(entity(52), Vec3::new(0.0, -4.5, 0.0), 0.5, dims, &solids)
            .expect("cells near surface");
        let mut hf_far = PhysicsWorld3d::new();
        hf_far
            .add_heightfield(entity(53), Vec3::ZERO, scale, 33, 33, heights)
            .expect("heightfield");
        let far_boxes = hf_far
            .add_cells(entity(54), Vec3::new(0.0, -500.0, 0.0), 0.5, dims, &solids)
            .expect("cells far below");
        for (name, count, world) in [
            ("hf_only", 1usize, &mut hf_only),
            ("hf_near", 1 + near_boxes, &mut hf_near),
            ("hf_far", 1 + far_boxes, &mut hf_far),
        ] {
            let start = std::time::Instant::now();
            for _ in 0..STEPS {
                world.step(1.0 / 60.0);
            }
            timings.push((
                name,
                count,
                start.elapsed().as_secs_f64() * 1e6 / STEPS as f64,
            ));
        }

        for (name, count, us) in &timings {
            println!("cells_cost_curve: arm={name} colliders={count} step_us={us:.1}");
        }
        let us = |arm: &str| timings.iter().find(|(n, _, _)| *n == arm).unwrap().2;
        // 并块的存在理由：逐格比并块贵一个量级以上。红了说明 merge 没生效或成本不在碰撞体数上。
        assert!(
            us("per_cell") > us("merged") * 10.0,
            "逐格 {} us 应该远贵于并块 {} us",
            us("per_cell"),
            us("merged")
        );
        // 非空世界的计时必须真的量到东西（防"全是 0 也绿"的空洞通过）。
        assert!(us("merged") > 0.0 && us("empty") > 0.0);
        println!(
            "cells_cost_curve: query_term_us={:.1} merged_us={:.1} per_cell_us={:.1}",
            us("merged") - us("merged_no_query"),
            us("merged"),
            us("per_cell")
        );
        println!(
            "cells_cost_curve: hf_base_us={:.1} cells_near_us={:.1} cells_far_us={:.1} merge_speedup={:.0}x",
            us("hf_only"),
            us("hf_near") - us("hf_only"),
            us("hf_far") - us("hf_only"),
            us("per_cell") / us("merged")
        );
    }
}
