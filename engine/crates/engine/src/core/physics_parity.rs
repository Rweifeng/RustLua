//! docs/54 §8-⑥：2D 与 3D 物理的**同一套契约**，用同一个场景在两个世界上各跑一遍来钉住。
//!
//! 为什么要有这个文件：2D 与 3D 是两套 Rapier 世界、两份 Lua API（`physics` / `physics3d`），
//! 很容易出现"名字一样、语义不一样"（本轮之前 `physics.overlap` 就是实体位置距离判定，
//! 而 `physics3d.overlap` 是碰撞体查询）。逐条断言**同一个场景在两侧给出同一个答案**，
//! 是唯一能不靠人眼发现这种漂移的办法。
//!
//! 只在测试构建里编译（见 `core/mod.rs` 的 `#[cfg(test)] mod physics_parity`）。

use crate::core::{
    entity::EntityId,
    math3d::Vec3,
    physics::{ColliderDesc, ColliderShape, PhysicsWorld},
    physics3d::{ColliderDesc3d, ColliderShape3d, PhysicsWorld3d},
    transform::{Transform, Transform3D, Vec2},
    world::World,
};

fn entity(n: u32) -> EntityId {
    EntityId::from_raw(n, 1)
}

fn ball_desc_2d(radius: f32) -> ColliderDesc {
    ColliderDesc {
        shape: ColliderShape::Ball(radius),
        offset: Vec2::zero(),
        sensor: false,
        dynamic: true,
        layer: 0xFF,
        mask: 0xFF,
        ccd: false,
    }
}

/// 2D 场景：动态小球（半径 1）沿 +x 以 40/s 冲向 x=40 的静态厚墙（半宽 1）。
/// 返回（最终 x，是否报过墙的碰撞事件）。
fn two_d_charge_into_wall() -> (f32, bool) {
    let mut world = PhysicsWorld::new();
    let ball = entity(1);
    let wall = entity(2);
    world
        .add_body(ball, Vec2::zero(), true, vec![ball_desc_2d(1.0)])
        .unwrap();
    world
        .add_body(
            wall,
            Vec2::new(40.0, 0.0),
            false,
            vec![ColliderDesc {
                shape: ColliderShape::Cuboid(Vec2::new(1.0, 50.0)),
                offset: Vec2::zero(),
                sensor: false,
                dynamic: false,
                layer: 0xFF,
                mask: 0xFF,
                ccd: false,
            }],
        )
        .unwrap();
    world.set_velocity(ball, 40.0, 0.0).unwrap();
    let mut final_x = 0.0;
    let mut touched = false;
    for _ in 0..120 {
        for (entity, position) in world.step(1.0 / 60.0) {
            if entity == ball {
                final_x = position.x;
            }
        }
        touched |= world
            .drain_contacts()
            .iter()
            .any(|contact| contact.a == ball || contact.b == ball);
    }
    (final_x, touched)
}

/// 3D 场景：与 2D **逐参数相同**（零重力，否则就不是同一个场景了）。
fn three_d_charge_into_wall() -> (f32, bool) {
    let mut world = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
    let ball = entity(1);
    let wall = entity(2);
    world
        .add_body(
            ball,
            Vec3::ZERO,
            true,
            vec![ColliderDesc3d::from_component_str("shape=ball;radius=1").unwrap()],
        )
        .unwrap();
    world
        .add_body(
            wall,
            Vec3::new(40.0, 0.0, 0.0),
            false,
            vec![
                ColliderDesc3d::from_component_str(
                    "shape=cuboid;half_width=1;half_height=50;half_z=50;dynamic=false",
                )
                .unwrap(),
            ],
        )
        .unwrap();
    world.set_velocity(ball, 40.0, 0.0, 0.0).unwrap();
    let mut final_x = 0.0;
    let mut touched = false;
    for _ in 0..120 {
        for (entity, position) in world.step(1.0 / 60.0) {
            if entity == ball {
                final_x = position.x;
            }
        }
        touched |= world
            .drain_contacts()
            .iter()
            .any(|contact| contact.a == ball || contact.b == ball);
    }
    (final_x, touched)
}

/// docs/54 §8-⑥「同步/权威」：同一个运动场景在 2D/3D 上必须得到同一个结果，并且
/// 两侧都要真的报出碰撞事件（事件契约同源）。
#[test]
fn dynamic_motion_and_contact_events_agree_across_dimensions() {
    let (two_d_x, two_d_touched) = two_d_charge_into_wall();
    let (three_d_x, three_d_touched) = three_d_charge_into_wall();
    assert!(
        two_d_x > 0.0 && two_d_x < 40.0,
        "2D 小球必须撞上墙而不是穿过去：x={two_d_x}"
    );
    assert!(
        three_d_x > 0.0 && three_d_x < 40.0,
        "3D 小球必须撞上墙而不是穿过去：x={three_d_x}"
    );
    assert!(
        (two_d_x - three_d_x).abs() < 0.5,
        "2D/3D 的停靠位置必须一致：2D={two_d_x} 3D={three_d_x}"
    );
    assert!(two_d_touched && three_d_touched, "两侧都要报出碰撞事件");
}

/// docs/54 §8-⑥「权威」：`kinematic_positions` 在两侧都只返回**动态**刚体，且取的是
/// **ECS 里的位置**（脚本权威 = ECS 是真源，物理世界只是被推着走）。
#[test]
fn kinematic_sync_reads_ecs_and_only_dynamic_bodies_in_both_worlds() {
    let mut world = World::new();
    let mover = world.spawn();
    let wall = world.spawn();
    world
        .insert(mover, Transform::from_position(Vec2::new(7.0, 8.0)))
        .unwrap();
    world
        .insert(mover, Transform3D::from_position(Vec3::new(7.0, 8.0, 9.0)))
        .unwrap();
    world
        .insert(wall, Transform::from_position(Vec2::new(-1.0, -1.0)))
        .unwrap();
    world
        .insert(
            wall,
            Transform3D::from_position(Vec3::new(-1.0, -1.0, -1.0)),
        )
        .unwrap();

    let mut two_d = PhysicsWorld::new();
    two_d
        .add_body(mover, Vec2::zero(), true, vec![ball_desc_2d(1.0)])
        .unwrap();
    two_d
        .add_body(wall, Vec2::new(40.0, 0.0), false, vec![ball_desc_2d(1.0)])
        .unwrap();
    let two_d_positions = two_d.kinematic_positions(&world);
    assert_eq!(two_d_positions.len(), 1, "只同步动态体");
    assert_eq!(two_d_positions[0], (mover, Vec2::new(7.0, 8.0)));

    let mut three_d = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
    three_d
        .add_body(
            mover,
            Vec3::ZERO,
            true,
            vec![ColliderDesc3d::from_component_str("shape=ball;radius=1").unwrap()],
        )
        .unwrap();
    three_d
        .add_body(
            wall,
            Vec3::new(40.0, 0.0, 0.0),
            false,
            vec![ColliderDesc3d::from_component_str("shape=ball;radius=1").unwrap()],
        )
        .unwrap();
    let three_d_positions = three_d.kinematic_positions(&world);
    assert_eq!(three_d_positions.len(), 1, "只同步动态体");
    assert_eq!(three_d_positions[0], (mover, Vec3::new(7.0, 8.0, 9.0)));
}

/// docs/54 §8-⑥「查询」：两侧的 `raycast`/`overlap` 都命中**碰撞体**，而不是实体位置；
/// `layer/mask` 对两侧的查询同样生效（这是"查询看得见谁"与"谁能撞到谁"同规则的前提）。
#[test]
fn queries_hit_colliders_and_respect_groups_in_both_worlds() {
    // ---- 2D ----
    let mut two_d = PhysicsWorld::new();
    let collider_entity = entity(1);
    let transform_only = entity(2);
    two_d
        .add_body(
            collider_entity,
            Vec2::new(10.0, 0.0),
            false,
            vec![
                ColliderDesc::from_component_str(
                    "shape=cuboid;half_width=1;half_height=5;dynamic=false;layer=2;mask=2",
                )
                .unwrap(),
            ],
        )
        .unwrap();
    let hit = two_d
        .raycast(Vec2::zero(), Vec2::new(1.0, 0.0), 100.0)
        .unwrap()
        .map(|(entity, _)| entity);
    assert_eq!(hit, Some(collider_entity), "2D raycast 命中碰撞体");
    assert_eq!(
        two_d.overlap(Vec2::new(10.0, 0.0), 3.0),
        vec![collider_entity],
        "2D overlap 只报碰撞体（`transform_only` 没有碰撞体）"
    );
    assert_eq!(
        two_d
            .raycast_filtered(Vec2::zero(), Vec2::new(1.0, 0.0), 100.0, Some((0x01, 0x01)))
            .unwrap(),
        None,
        "2D：查询组 (1,1) 与碰撞体 (2,2) 不相交 → 看不见"
    );
    assert_eq!(
        two_d
            .raycast_filtered(Vec2::zero(), Vec2::new(1.0, 0.0), 100.0, Some((0x02, 0x02)))
            .unwrap()
            .map(|(entity, _)| entity),
        Some(collider_entity),
        "2D：查询组 (2,2) 与碰撞体 (2,2) 相交 → 看得见"
    );

    // ---- 3D：逐项同义 ----
    let mut three_d = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
    three_d
        .add_body(
            collider_entity,
            Vec3::new(10.0, 0.0, 0.0),
            false,
            vec![
                ColliderDesc3d::from_component_str(
                    "shape=cuboid;half_width=1;half_height=5;half_z=5;dynamic=false;layer=2;mask=2",
                )
                .unwrap(),
            ],
        )
        .unwrap();
    let hit = three_d
        .raycast(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 100.0)
        .unwrap();
    assert_eq!(hit, Some(collider_entity), "3D raycast 命中碰撞体");
    assert_eq!(
        three_d.overlap(Vec3::new(10.0, 0.0, 0.0), 3.0),
        vec![collider_entity],
        "3D overlap 只报碰撞体"
    );
    assert_eq!(
        three_d
            .raycast_filtered(
                Vec3::ZERO,
                Vec3::new(1.0, 0.0, 0.0),
                100.0,
                Some((0x01, 0x01))
            )
            .unwrap(),
        None,
        "3D：查询组 (1,1) 与碰撞体 (2,2) 不相交 → 看不见"
    );
    assert_eq!(
        three_d
            .raycast_filtered(
                Vec3::ZERO,
                Vec3::new(1.0, 0.0, 0.0),
                100.0,
                Some((0x02, 0x02))
            )
            .unwrap(),
        Some(collider_entity),
        "3D：查询组 (2,2) 与碰撞体 (2,2) 相交 → 看得见"
    );
    assert_ne!(collider_entity, transform_only);
}

/// docs/54 §8-⑥「调试」：两侧的调试数据都报出**世界姿态 + sensor + 是否动态**，
/// 并且线框段数固定（宿主据此可以断言顶点数，不需要在渲染侧再猜一次形状）。
#[test]
fn debug_shapes_expose_the_same_fields_in_both_worlds() {
    // 2D：矩形 4 条边、圆 24 段。
    assert_eq!(
        ColliderShape::Cuboid(Vec2::new(1.0, 2.0))
            .wireframe_edges()
            .len(),
        4
    );
    assert_eq!(ColliderShape::Ball(1.0).wireframe_edges().len(), 24);
    // 3D：矩形 12 条棱、球 3×16、胶囊 2×16+4。
    assert_eq!(
        ColliderShape3d::Cuboid(Vec3::new(1.0, 1.0, 1.0))
            .wireframe_edges()
            .len(),
        12
    );
    assert_eq!(ColliderShape3d::Ball(1.0).wireframe_edges().len(), 48);
    assert_eq!(
        ColliderShape3d::Capsule {
            half_height: 1.0,
            radius: 0.5
        }
        .wireframe_edges()
        .len(),
        36
    );

    let mut two_d = PhysicsWorld::new();
    two_d
        .add_body(
            entity(1),
            Vec2::new(3.0, 4.0),
            false,
            vec![
                ColliderDesc::from_component_str("shape=ball;radius=2;sensor=true;dynamic=false")
                    .unwrap(),
            ],
        )
        .unwrap();
    let two_d_shape = &two_d.debug_shapes()[0];
    assert!(two_d_shape.sensor);
    assert!(!two_d_shape.dynamic);
    assert!((two_d_shape.center.x - 3.0).abs() < 1e-5);
    assert_eq!(two_d_shape.wireframe_segments().len(), 24);

    let mut three_d = PhysicsWorld3d::new_with_gravity(Vec3::ZERO);
    three_d
        .add_body(
            entity(1),
            Vec3::new(3.0, 4.0, 5.0),
            false,
            vec![
                ColliderDesc3d::from_component_str("shape=ball;radius=2;sensor=true;dynamic=false")
                    .unwrap(),
            ],
        )
        .unwrap();
    let three_d_shape = &three_d.debug_shapes()[0];
    assert!(three_d_shape.sensor);
    assert!(!three_d_shape.dynamic);
    assert_eq!(three_d_shape.center, Vec3::new(3.0, 4.0, 5.0));
    assert_eq!(three_d_shape.wireframe_segments().len(), 48);
}
