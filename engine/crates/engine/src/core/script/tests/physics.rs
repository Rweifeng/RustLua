use super::*;

/// WP7 ②：`physics.debug_draw` 只登记请求、由宿主消费，且请求取走即清空
/// （与 camera2d 的 pending 语义一致——否则开关会在之后每一帧被重复应用）。
#[test]
fn lua_physics_debug_draw_registers_and_consumes_a_request() {
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(
        runtime.take_collider_debug_request(),
        None,
        "默认没有待消费请求"
    );
    let path = script(
        "function update()\n\
         assert(physics.debug_draw(true) == true)\n\
         return 'on' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "on");
    assert_eq!(runtime.take_collider_debug_request(), Some(true));
    assert_eq!(runtime.take_collider_debug_request(), None, "取走即清空");
    // 场景里的 `Collider` 能变成调试绘制用的形状（读物理世界的当前姿态）。
    let mut scene = crate::core::scene::Scene::new("collider-debug-probe");
    scene
        .add_entity(crate::core::scene::SceneEntityId(1), "Wall", None)
        .unwrap();
    scene
        .entities
        .get_mut(&crate::core::scene::SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=32,48,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene
        .entities
        .get_mut(&crate::core::scene::SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Collider".into(),
            "shape=cuboid;half_width=16;half_height=8;sensor=true".into(),
        );
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);
    let shapes = runtime.collider_debug_shapes();
    assert_eq!(shapes.len(), 1, "每个碰撞体一个调试形状");
    assert_eq!(
        shapes[0].shape,
        crate::core::physics::ColliderShape::Cuboid(Vec2::new(16.0, 8.0))
    );
    assert!(shapes[0].sensor, "sensor 要透出来，绘制时换色");
    assert!((shapes[0].center.x - 32.0).abs() < 1e-4);
    assert!((shapes[0].center.y - 48.0).abs() < 1e-4);
    let _ = fs::remove_file(path);
}
/// docs/54 §8-⑥：3D 侧的 `physics3d.debug_draw` 与 2D 是同一条"登记请求 → 宿主取走
/// 即清空"的通道，但**两条通道互不干扰**（混合场景里只想看一侧是常态）。
#[test]
fn lua_physics3d_debug_draw_registers_and_consumes_a_request() {
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(
        runtime.take_collider_debug_request_3d(),
        None,
        "默认没有请求"
    );
    let path = script(
        "function update()\n\
         assert(physics3d.debug_draw(true) == true)\n\
         return 'on' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "on");
    assert_eq!(runtime.take_collider_debug_request_3d(), Some(true));
    assert_eq!(runtime.take_collider_debug_request_3d(), None, "取走即清空");
    assert_eq!(
        runtime.take_collider_debug_request(),
        None,
        "3D 请求不能串到 2D 通道"
    );

    // 场景里的 `Collider3d` 能变成 3D 调试线框的数据源（读物理世界的当前姿态）。
    let mut scene = crate::core::scene::Scene::new("collider-debug-3d-probe");
    scene
        .add_entity(crate::core::scene::SceneEntityId(1), "Wall", None)
        .unwrap();
    scene
        .entities
        .get_mut(&crate::core::scene::SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=1,2,3;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene
        .entities
        .get_mut(&crate::core::scene::SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Collider3d".into(),
            "shape=cuboid;half_width=1;half_height=2;half_z=3;sensor=true;dynamic=false".into(),
        );
    runtime.instantiate_scene(&scene).unwrap();
    let shapes = runtime.collider_debug_shapes_3d();
    assert_eq!(shapes.len(), 1, "每个碰撞体一个调试形状");
    assert_eq!(
        shapes[0].shape,
        crate::core::physics3d::ColliderShape3d::Cuboid(crate::core::math3d::Vec3::new(
            1.0, 2.0, 3.0
        ))
    );
    assert!(shapes[0].sensor, "sensor 要透出来");
    assert!(!shapes[0].dynamic, "dynamic=false 要透出来");
    assert_eq!(
        shapes[0].center,
        crate::core::math3d::Vec3::new(1.0, 2.0, 3.0)
    );
    let _ = fs::remove_file(path);
}
/// WP7：场景里的 `Collider` 声明 `ccd=true` 会落到 Rapier 刚体上，Lua 侧可读写该开关
/// （`physics.get_ccd` 读的是物理世界的真实状态）。
#[test]
fn lua_physics_ccd_flag_reaches_the_rigid_body() {
    let mut scene = crate::core::scene::Scene::new("ccd");
    for (index, (name, collider)) in [
        ("CcdBody", "shape=ball;radius=4;ccd=true"),
        ("PlainBody", "shape=ball;radius=4"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = crate::core::scene::SceneEntityId(index as u64 + 1);
        scene.add_entity(id, name, None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            format!("position={},0,0;rotation=0,0,0,1;scale=1,1,1", index * 100),
        );
        entity
            .components
            .insert("Collider".into(), collider.to_owned());
    }
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 2);
    let path = script(
        "function update()\n\
         local a = entity.find('CcdBody')\n\
         local b = entity.find('PlainBody')\n\
         assert(physics.get_ccd(a) == true, 'scene ccd=true should reach the body')\n\
         assert(physics.get_ccd(b) == false, 'default is off')\n\
         physics.set_ccd(b, true)\n\
         assert(physics.get_ccd(b) == true)\n\
         physics.set_ccd(a, false)\n\
         assert(physics.get_ccd(a) == false)\n\
         assert(not pcall(physics.get_ccd, 999999), 'unknown entity must error')\n\
         assert(not pcall(physics.set_ccd, 999999, true), 'unknown entity must error')\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
/// docs/54 §8-⑥：2D 查询是**碰撞体查询**（与 `physics3d.raycast/overlap` 同义），
/// 位置宽相另有其名（`physics.positions_in_radius`）。这条把两者的区别钉住：
/// 只有碰撞体的实体被物理查询命中，只有 Transform 的实体只被位置查询命中。
#[test]
fn lua_physics_queries_hit_colliders_and_the_position_broad_phase_is_separate() {
    // 只有墙有碰撞体；`Deco` 只有 Transform（装饰物）。两者都从场景实例化，
    // 因此走的是与游戏同一条注册路径。
    let mut scene = crate::core::scene::Scene::new("lua-physics-queries");
    let wall = crate::core::scene::SceneEntityId(1);
    let deco = crate::core::scene::SceneEntityId(2);
    scene.add_entity(wall, "Wall", None).unwrap();
    scene.add_entity(deco, "Deco", None).unwrap();
    scene.entities.get_mut(&wall).unwrap().components.insert(
        "Transform".into(),
        "position=10,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&deco).unwrap().components.insert(
        "Transform".into(),
        "position=12,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&wall).unwrap().components.insert(
        "Collider".into(),
        "shape=cuboid;half_width=1;half_height=5;sensor=false".into(),
    );
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 2);
    let path = script(
        "function update()\n\
         local wall = entity.find('Wall')\n\
         local deco = entity.find('Deco')\n\
         local hit = physics.raycast(0, 0, 1, 0, 100)\n\
         assert(hit == wall, 'raycast must hit the collider body')\n\
         local near = physics.overlap(10, 0, 3)\n\
         local found_wall, found_deco = false, false\n\
         for _, id in ipairs(near) do\n\
         if id == wall then found_wall = true end\n\
         if id == deco then found_deco = true end\n\
         end\n\
         assert(found_wall, 'overlap must include the collider body')\n\
         assert(not found_deco, 'overlap must not include a Transform-only entity')\n\
         local broad = physics.positions_in_radius(12, 0, 0.5)\n\
         local broad_deco = false\n\
         for _, id in ipairs(broad) do\n\
         if id == deco then broad_deco = true end\n\
         end\n\
         assert(broad_deco, 'positions_in_radius is the scene-space broad phase')\n\
         local velocity = physics.get_velocity(wall)\n\
         assert(math.abs(velocity.x) < 0.001 and math.abs(velocity.y) < 0.001, 'named velocity keys')\n\
         assert(math.abs(velocity[1]) < 0.001 and math.abs(velocity[2]) < 0.001, 'numeric keys stay')\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}

/// docs/54 §8-⑥：查询也吃 `layer/mask`（可选尾参 `{layer=, mask=}`），2D/3D 同名同义；
/// 越界值必须报错——位域是 u8，静默截断会让 `layer=256` 变成"全层可见"，
/// 那种错只在真机上才看得出来。
#[test]
fn lua_query_group_filters_are_shared_between_2d_and_3d() {
    let mut scene = crate::core::scene::Scene::new("lua-query-groups");
    let wall2d = crate::core::scene::SceneEntityId(1);
    let wall3d = crate::core::scene::SceneEntityId(2);
    scene.add_entity(wall2d, "Wall2d", None).unwrap();
    scene.add_entity(wall3d, "Wall3d", None).unwrap();
    scene.entities.get_mut(&wall2d).unwrap().components.insert(
        "Transform".into(),
        "position=10,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&wall2d).unwrap().components.insert(
        "Collider".into(),
        "shape=cuboid;half_width=1;half_height=5;dynamic=false;layer=2;mask=2".into(),
    );
    scene.entities.get_mut(&wall3d).unwrap().components.insert(
        "Transform".into(),
        "position=10,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&wall3d).unwrap().components.insert(
        "Collider3d".into(),
        "shape=cuboid;half_width=1;half_height=5;half_z=5;dynamic=false;layer=2;mask=2".into(),
    );
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 2);
    let path = script(
        "function update()\n\
         local wall2d = entity.find('Wall2d')\n\
         local wall3d = entity.find('Wall3d')\n\
         assert(physics.raycast(0, 0, 1, 0, 100) == wall2d, '默认查询看得见')\n\
         assert(physics.raycast(0, 0, 1, 0, 100, {layer=1, mask=1}) == nil, '分组不相交就看不见')\n\
         assert(physics.raycast(0, 0, 1, 0, 100, {layer=2, mask=2}) == wall2d, '分组相交看得见')\n\
         assert(#physics.overlap(10, 0, 3, {layer=1, mask=1}) == 0, 'overlap 同样吃分组')\n\
         assert(#physics.overlap(10, 0, 3, {layer=2, mask=2}) == 1)\n\
         assert(physics3d.raycast(0, 0, 0, 1, 0, 0, 100) == wall3d, '3D 默认查询看得见')\n\
         assert(physics3d.raycast(0, 0, 0, 1, 0, 0, 100, {layer=1, mask=1}) == nil)\n\
         assert(physics3d.raycast(0, 0, 0, 1, 0, 0, 100, {layer=2, mask=2}) == wall3d)\n\
         assert(#physics3d.overlap(10, 0, 0, 3, {layer=1, mask=1}) == 0)\n\
         assert(#physics3d.overlap(10, 0, 0, 3, {layer=2, mask=2}) == 1)\n\
         assert(not pcall(physics.raycast, 0, 0, 1, 0, 100, {layer=256}), '越界必须报错')\n\
         assert(not pcall(physics.raycast, 0, 0, 1, 0, 100, {mask=-1}), '负数必须报错')\n\
         assert(not pcall(physics3d.overlap, 0, 0, 0, 3, {layer='x'}), '非整数必须报错')\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}

#[test]
fn physics_blocks_player_on_tilemap_wall_and_emits_events() {
    use crate::core::scene::{Scene, SceneEntityId};

    let mut scene = Scene::new("test");
    scene.add_entity(SceneEntityId(1), "Player", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Collider".into(), "shape=ball;radius=10".into());
    scene.add_entity(SceneEntityId(2), "Map", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    // 2x2 tilemap, right column solid -> a wall spanning x=20..40
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Tilemap".into(),
            "tile_size=20x20;width=2;height=2;tiles=0000;solid=0101".into(),
        );

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.instantiate_scene(&scene).unwrap();

    let path = script(
        "function update()\nlocal p = entity.find('Player')\nphysics.set_velocity(p, 40, 0)\nreturn 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    runtime.call_update().unwrap();
    for _ in 0..60 {
        runtime.step_physics(1.0 / 60.0);
    }

    let player = runtime.with_world(|world| world.entities_with::<Transform>());
    assert_eq!(player.len(), 2);
    let player = player[0];
    let position =
        runtime.with_world(|world| world.get::<Transform>(player).unwrap().unwrap().position);
    assert!(
        position.x < 15.0,
        "player was not blocked by the tilemap wall: x={}",
        position.x
    );

    let events =
        script("function update()\nlocal events = physics.events()\nreturn tostring(#events) end");
    runtime.execute_project_script(&events).unwrap();
    let count = runtime.call_update().unwrap();
    let count: u32 = count.parse().unwrap();
    assert!(count > 0, "no collision events reached Lua");
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(events);
}

/// WP7：3D 场景的 `Collider3d` 声明 `ccd=true` 同样落到刚体上，Lua `physics3d` 侧可读写。
#[test]
fn lua_physics3d_ccd_flag_reaches_the_rigid_body() {
    let mut scene = crate::core::scene::Scene::new("ccd3d");
    for (index, (name, collider)) in [
        ("Fast", "shape=ball;radius=0.2;ccd=true"),
        ("Slow", "shape=ball;radius=0.2"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = crate::core::scene::SceneEntityId(index as u64 + 1);
        scene.add_entity(id, name, None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            format!("position={},0,0;rotation=0,0,0,1;scale=1,1,1", index * 10),
        );
        entity
            .components
            .insert("Collider3d".into(), collider.to_owned());
    }
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 2);
    let path = script(
        "function update()\n\
         local a = entity.find('Fast')\n\
         local b = entity.find('Slow')\n\
         assert(physics3d.get_ccd(a) == true, 'scene ccd=true should reach the 3D body')\n\
         assert(physics3d.get_ccd(b) == false, 'default is off')\n\
         physics3d.set_ccd(b, true)\n\
         assert(physics3d.get_ccd(b) == true)\n\
         assert(not pcall(physics3d.get_ccd, 999999), 'unknown entity must error')\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
#[test]
fn physics3d_body_falls_onto_static_ground_and_syncs_transform3d() {
    use crate::core::scene::{Scene, SceneEntityId};

    let mut scene = Scene::new("3d");
    scene.add_entity(SceneEntityId(1), "Ball", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,5,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Collider3d".into(), "shape=ball;radius=1".into());
    scene.add_entity(SceneEntityId(2), "Ground", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    // Static ground: cuboid spanning y=-2..0 (top at y=0), centered at (0,-1,0).
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Collider3d".into(),
            "shape=cuboid;half_width=10;half_height=1;half_z=10;offset=0,-1,0;dynamic=false".into(),
        );

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.instantiate_scene(&scene).unwrap();

    let path = script(
        "function update()\nlocal b = entity.find('Ball')\nphysics3d.set_velocity(b, 0, 0, 2)\nreturn 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    runtime.call_update().unwrap();
    for _ in 0..180 {
        runtime.step_physics3d(1.0 / 60.0);
        runtime.writeback_physics3d();
    }

    let positions: Vec<_> = runtime.with_world(|world| {
        world
            .entities_with::<Transform3D>()
            .into_iter()
            .filter_map(|id| {
                world
                    .get::<Transform3D>(id)
                    .ok()
                    .flatten()
                    .map(|t| t.position)
            })
            .collect()
    });
    assert_eq!(positions.len(), 2);
    let ball_pos = positions.iter().find(|p| p.y > 0.5).copied().unwrap();
    // Ball radius 1 resting on the ground top (y=0) => y ≈ 1; it also moved +z.
    assert!(
        (ball_pos.y - 1.0).abs() < 0.1,
        "ball should rest at y≈1, got y={}",
        ball_pos.y
    );
    assert!(
        ball_pos.z > 2.0,
        "ball should have moved in +z, got z={}",
        ball_pos.z
    );

    // Collision events reached Lua via the physics3d table.
    let events = script(
        "function update()\nlocal events = physics3d.events()\nreturn tostring(#events) end",
    );
    runtime.execute_project_script(&events).unwrap();
    let count = runtime.call_update().unwrap();
    let count: u32 = count.parse().unwrap();
    assert!(count > 0, "no 3D collision events reached Lua");
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(events);
}
