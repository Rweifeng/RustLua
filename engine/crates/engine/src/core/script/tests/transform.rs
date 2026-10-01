use super::*;

/// WP9 统一 `transform` API：同一组函数按实体**实际拥有**的组件分派到 2D / 3D。
///
/// 刻意锁住的语义：① 2D 实体的取值一字不变（`space` 只是新增字段）；② 3D 下 `set` 只改
/// XY、Z 保留；③ `attach` 不覆盖已有组件，且新建组件时把另一侧的 XY 带过来。
#[test]
fn lua_transform_api_dispatches_between_2d_and_3d() {
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(
        runtime
            .instantiate_scene(&unified_transform_scene())
            .unwrap(),
        3
    );
    let path = script(
        "function update()\n\
         local flat = entity.find('Flat')\n\
         assert(transform.space(flat) == '2d')\n\
         transform.set(flat, 2, 3, 0.5, 2, 4)\n\
         local p = transform.get(flat)\n\
         assert(p.space == '2d' and p.x == 2 and p.y == 3 and p.rotation == 0.5)\n\
         assert(p.scale_x == 2 and p.scale_y == 4)\n\
         assert(p.z == nil, '2d 结果不带 z')\n\
         assert(transform.attach(flat, '2d') == false, '已有 2d 组件时不覆盖')\n\
         local solid = entity.find('Solid')\n\
         assert(transform.space(solid) == '3d')\n\
         transform.set(solid, 10, 20)\n\
         transform.move(solid, 5, -2)\n\
         local q = transform.get(solid)\n\
         assert(q.space == '3d' and q.x == 15 and q.y == 18)\n\
         assert(q.z == 3, 'set 只改 XY，Z 必须保留')\n\
         assert(q.rw == 1 and q.scale_z == 1)\n\
         local bare = entity.find('Bare')\n\
         assert(transform.space(bare) == nil, '没有任何变换时为 nil')\n\
         assert(transform.attach(bare, '2d') == true)\n\
         assert(transform.attach(bare, '3d') == true)\n\
         return 'unified' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "unified");
    let _ = fs::remove_file(path);
    // `attach(flat,'3d')` 必须把 2D 的 XY 带过去，而不是留一个原点上的新组件。
    let flat = runtime.with_world(|world| world.entities_with::<Transform>());
    let flat = flat[0];
    assert_eq!(
        runtime.with_world(|world| {
            world
                .get::<crate::core::transform::Transform3D>(flat)
                .unwrap()
                .map(|value| (value.position.x, value.position.y))
        }),
        None,
        "attach 只在调用时才建组件"
    );
    let bridge = script(
        "function update()\n\
         local flat = entity.find('Flat')\n\
         assert(transform.attach(flat, '3d') == true)\n\
         return 'bridged' end",
    );
    runtime.execute_project_script(&bridge).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "bridged");
    let bridged = runtime
        .with_world(|world| {
            world
                .get::<crate::core::transform::Transform3D>(flat)
                .unwrap()
                .cloned()
        })
        .unwrap();
    assert_eq!((bridged.position.x, bridged.position.y), (2.0, 3.0));
    let _ = fs::remove_file(bridge);
}
/// 统一 API 的模糊之处必须报错而不是猜：3D 下喂标量 rotation、未知 space 名。
/// `transform.attach` 必须走统一的**提升/压平**实现，而不是只手抄一个 XY：
/// 手抄会把旋转与缩放悄悄丢掉；压不平的 3D 变换必须**报错而不是丢数据**。
#[test]
fn lua_transform_attach_preserves_rotation_and_refuses_lossy_flatten() {
    let mut runtime = LuaRuntime::new().unwrap();
    // 场景里 Flat 只有 2D、Solid 只有 3D（position=1,2,3 ⇒ z = 3，**非平面**）、Bare 两者皆无。
    assert_eq!(
        runtime
            .instantiate_scene(&unified_transform_scene())
            .unwrap(),
        3
    );
    let path = script(
        "function update()\n\
         -- ① 2D 实体（带旋转与缩放）提升为 3D：两者都要保留成绕 Z 旋转与 XY 缩放\n\
         local flat = entity.find('Flat')\n\
         transform.set(flat, 3, 4, 0.5, 2, 4)\n\
         assert(transform.attach(flat, '3d') == true)\n\
         local lifted = transform3d.get(flat)\n\
         assert(math.abs(lifted.x - 3) < 1e-4 and math.abs(lifted.y - 4) < 1e-4)\n\
         assert(math.abs(lifted.z) < 1e-4)\n\
         assert(math.abs(lifted.rz - math.sin(0.25)) < 1e-4, 'rotation must survive lifting')\n\
         assert(math.abs(lifted.rw - math.cos(0.25)) < 1e-4)\n\
         assert(math.abs(lifted.sx - 2) < 1e-4 and math.abs(lifted.sy - 4) < 1e-4)\n\
         -- ② 非平面 3D 实体（z = 3）压平必须被拒绝，且不得留下半成品组件\n\
         local solid = entity.find('Solid')\n\
         local refused = pcall(transform.attach, solid, '2d')\n\
         assert(not refused, 'non-planar flatten must be refused')\n\
         assert(transform.space(solid) == '3d', 'refused attach must not insert a 2d component')\n\
         -- ③ 把它改成平面（z = 0、绕 Z 旋转、z 缩放 = 1）后压平：旋转与缩放都要保留\n\
         transform3d.set(solid, 10, 20, 0, 0, 0, math.sin(0.3), math.cos(0.3), 3, 5, 1)\n\
         assert(transform.attach(solid, '2d') == true)\n\
         local flattened = transform.get(solid)\n\
         assert(math.abs(flattened.x - 10) < 1e-4 and math.abs(flattened.y - 20) < 1e-4)\n\
         assert(math.abs(flattened.rotation - 0.6) < 1e-3, 'rotation must survive flattening')\n\
         assert(math.abs(flattened.scale_x - 3) < 1e-4 and math.abs(flattened.scale_y - 5) < 1e-4)\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
#[test]
fn lua_transform_api_refuses_ambiguous_rotation_and_unknown_space() {
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(
        runtime
            .instantiate_scene(&unified_transform_scene())
            .unwrap(),
        3
    );
    let path = script(
        "function update()\n\
         local solid = entity.find('Solid')\n\
         local bad = pcall(transform.set, solid, 99, 99, 0.5)\n\
         assert(not bad, '3d 标量 rotation 必须被拒绝')\n\
         local after = transform.get(solid)\n\
         assert(after.x == 1 and after.y == 2 and after.z == 3, '被拒绝的调用不得留下半写状态')\n\
         local bad_space = pcall(transform.attach, solid, '4d')\n\
         assert(not bad_space, '未知 space 必须被拒绝')\n\
         local bare = entity.find('Bare')\n\
         local missing = pcall(transform.get, bare)\n\
         assert(not missing, '没有变换的实体 get 必须报错')\n\
         local missing_set = pcall(transform.set, bare, 1, 1)\n\
         assert(not missing_set, '没有变换的实体 set 必须报错')\n\
         return 'guarded' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "guarded");
    let _ = fs::remove_file(path);
}
/// WP2：Lua `transform.get` 读局部、`transform.world` 读**世界**（父链组合结果）。
/// 场景数据用迁移后的局部值表达，因此两者在开启继承后必然不同——这正是要区分的两件事。
#[test]
fn lua_transform_world_differs_from_local_under_inheritance() {
    let mut scene = crate::core::scene::Scene::new("world-transform");
    let parent = crate::core::scene::SceneEntityId(1);
    let child = crate::core::scene::SceneEntityId(2);
    scene.add_entity(parent, "Parent", None).unwrap();
    scene.add_entity(child, "Child", Some(parent)).unwrap();
    scene.entities.get_mut(&parent).unwrap().components.insert(
        "Transform".into(),
        "position=10,5,0;rotation=0,0,0,1;scale=2,2,1".into(),
    );
    scene.entities.get_mut(&child).unwrap().components.insert(
        "Transform".into(),
        "position=-3.5,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.instantiate_scene(&scene).unwrap();
    // 宿主侧开启空间继承并传播一次（与 `RuntimeScene::set_spatial_inheritance` 同一条路径）。
    runtime.with_world_mut(|world| {
        crate::core::hierarchy::propagate_spatial(world, true);
    });
    let path = script(
        "function update()\n\
         local c = entity.find('Child')\n\
         local local_t = transform.get(c)\n\
         local world_t = transform.world(c)\n\
         assert(local_t.space == '2d' and world_t.space == '2d')\n\
         assert(math.abs(local_t.x + 3.5) < 1e-3, 'local x should stay -3.5: ' .. tostring(local_t.x))\n\
         assert(math.abs(world_t.x - 3.0) < 1e-3, 'world x should be 3: ' .. tostring(world_t.x))\n\
         assert(math.abs(world_t.y - 5.0) < 1e-3, 'world y should be 5: ' .. tostring(world_t.y))\n\
         assert(math.abs(world_t.scale_x - 2.0) < 1e-3, 'parent scale applies')\n\
         assert(not pcall(transform.world, 999999), 'unknown entity must error')\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
#[test]
fn lua_3d_attach_and_validation_errors() {
    let mut runtime = LuaRuntime::new().unwrap();
    let path = script(
        "local e = entity.spawn('Cube')\n\
         transform3d.attach(e)\n\
         mesh.set(e, 'assets/models/cube.glb')\n\
         transform3d.set(e, 1, 2, 3)\n\
         local t = transform3d.get(e)\n\
         assert(t.x == 1 and t.y == 2 and t.z == 3)\n\
         assert(t.rw == 1 and t.sx == 1)\n\
         local ok = pcall(function() transform3d.set(e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0) end)\n\
         assert(not ok, 'zero-scale must be rejected')\n\
         local bad = pcall(function() mesh.set(e, 'assets/models/cube.obj') end)\n\
         assert(not bad, 'non-glb mesh must be rejected')\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
