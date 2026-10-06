use super::*;

/// `docs/60` §6.1 复核（2026-10-06）：反向名索引改成「正向真源 + 惰性派生反查（脏标记）」。
/// 宿主每帧在**实体循环里**调 `entity_name`（`host/runtime_scene/mesh_ui.rs`），所以这里钉两条
/// **方向相反、缺一不可**的契约：
///  ① 惰性缓存**必须随改动失效**——否则反查会返回过期名字（这是本次修法唯一的真风险）；
///  ② 稳态反查**不许重建**——一次重建是 O(n)，而"每查一次就重建一次"正是被修掉的 O(n²)。
#[test]
fn entity_name_reverse_index_is_stale_free_and_rebuilds_only_on_change() {
    fn names_now(runtime: &LuaRuntime) -> Vec<String> {
        let ids = runtime.with_world(|world| world.entities_with::<Transform>());
        let mut names: Vec<String> = ids
            .into_iter()
            .filter_map(|id| runtime.entity_name(id))
            .collect();
        names.sort();
        names
    }

    let path = script(
        "entity.spawn('hero')\n\
         entity.spawn('foe')\n\
         function update() return 'ok' end",
    );
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");

    let before = entity_name_rebuilds();
    assert_eq!(
        names_now(&runtime),
        ["foe", "hero"],
        "反查必须给出名字（宿主每帧走的就是这条路）"
    );
    assert_eq!(
        entity_name_rebuilds(),
        before + 1,
        "首次反查只重建一次：那一次是 O(n)，付得起"
    );

    // ② 稳态：连查 64 次，一次都不许再重建。
    let settled = entity_name_rebuilds();
    for _ in 0..64 {
        let _ = names_now(&runtime);
    }
    assert_eq!(
        entity_name_rebuilds(),
        settled,
        "稳态反查不许重建——每次反查都重建就是被修掉的那条 O(n²)"
    );

    // ① 改动必须让缓存失效：`entity.spawn('hero')` 会把同名改指到一个新实体。
    let stale = runtime
        .with_world(|world| world.entities_with::<Transform>())
        .into_iter()
        .find(|id| runtime.entity_name(*id).as_deref() == Some("hero"))
        .expect("hero 必须存在");
    runtime
        .execute_console("return entity.spawn('hero')")
        .unwrap();
    assert!(
        runtime.entity_name(stale).is_none(),
        "旧实体已不再持有这个名字，反查不许返回过期名字（惰性缓存的失效必须真的生效）"
    );
    assert_eq!(
        names_now(&runtime),
        ["foe", "hero"],
        "新实体接上了这个名字，且旧实体不再出现"
    );
    assert!(
        entity_name_rebuilds() > settled,
        "改动之后必须重建——否则上面两条靠的是巧合而不是缓存失效"
    );
    let _ = fs::remove_file(path);
}

#[test]
fn lua_gameplay_api_moves_damages_and_destroys() {
    let path = script(
        "player=entity.spawn('player')\ntransform.set(player,0,0)\ntransform.move(player,2,3)\nlocal p=transform.get(player)\nassert(p.x==2 and p.y==3)\nhealth.set(player,10)\nassert(health.damage(player,4)==6)\nassert(entity.find('player')==player)\nentity.destroy(player)\nassert(not entity.is_alive(player))\nfunction update() return 'ok' end",
    );
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
#[test]
fn entity_persist_id_registers_looks_up_and_rejects_duplicates() {
    let path = script(
        "local a = entity.spawn(nil, 'house')\n\
         assert(entity.find_id('house') == a)\n\
         assert(entity.persist_id(a) == 'house')\n\
         assert(not pcall(function() entity.spawn(nil, 'house') end))\n\
         entity.destroy(a)\n\
         assert(entity.find_id('house') == nil)\n\
         function update() return 'ok' end",
    );
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
/// docs/54 §4.2：`visibility` 表读写**实体侧**的可见层遮罩——场景组件同源、缺组件=全可见、
/// 非法值报错（负数/越界/未知实体都不许静默通过）。
#[test]
fn lua_visibility_round_trips_and_validates() {
    let mut scene = rustlua_scene::scene::Scene::new("lua-visibility");
    let panel = rustlua_scene::scene::SceneEntityId(1);
    let plain = rustlua_scene::scene::SceneEntityId(2);
    scene.add_entity(panel, "Panel", None).unwrap();
    scene.add_entity(plain, "Plain", None).unwrap();
    scene
        .entities
        .get_mut(&panel)
        .unwrap()
        .components
        .insert("Visibility".into(), "layers=0x0F".into());
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 2);
    let path = script(
        "function update()\n\
         local panel = entity.find('Panel')\n\
         local plain = entity.find('Plain')\n\
         assert(visibility.get(panel) == 15, 'scene component must be readable')\n\
         assert(visibility.get(plain) == 4294967295, '缺组件 = 全可见')\n\
         assert(visibility.set(panel, 0x10) == 16, 'set 返回新值')\n\
         assert(visibility.get(panel) == 16)\n\
         assert(visibility.set(plain, 1) == 1, '缺组件时 set 会补上组件')\n\
         assert(visibility.get(plain) == 1)\n\
         assert(visibility.all() == 4294967295)\n\
         assert(not pcall(visibility.set, panel, -1), '负数必须报错')\n\
         assert(not pcall(visibility.set, panel, 4294967296), '越界必须报错')\n\
         assert(not pcall(visibility.get, 999999), '未知实体必须报错')\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}

/// docs/54 §4.3：`quad` 表读写一等 Quad（几何 + 材质，贴图可选）。判据有三条：
/// 场景里写的 Quad 能读回、运行时能改、非法串必须报错（不静默忽略）。
#[test]
fn lua_quad_round_trips_and_validates() {
    let mut scene = rustlua_scene::scene::Scene::new("lua-quad");
    let bar = rustlua_scene::scene::SceneEntityId(1);
    scene.add_entity(bar, "Bar", None).unwrap();
    scene
        .entities
        .get_mut(&bar)
        .unwrap()
        .components
        .insert("Quad".into(), "size=120,8;color=1,0,0,0.5".into());
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);
    let path = script(
        "function update()\n\
         local bar = entity.find('Bar')\n\
         local q = quad.get(bar)\n\
         assert(q.size_x == 120 and q.size_y == 8, '场景声明的尺寸必须能读回')\n\
         assert(q.color_a == 0.5, 'alpha 必须能读回')\n\
         assert(q.texture == '__white__', '没写 texture 时用内置白纹理')\n\
         quad.set(bar, 'size=60,4;color=0,1,0,1;texture=assets/textures/ui.png;z=2')\n\
         local updated = quad.get(bar)\n\
         assert(updated.size_x == 60 and updated.size_y == 4)\n\
         assert(updated.color_g == 1 and updated.color_r == 0)\n\
         assert(updated.texture == 'assets/textures/ui.png')\n\
         assert(updated.z == 2)\n\
         assert(not pcall(quad.set, bar, 'colour=1,0,0'), '拼错字段必须报错')\n\
         assert(not pcall(quad.set, bar, 'size=0,4'), '零尺寸必须报错')\n\
         assert(not pcall(quad.get, 999999), '未知实体必须报错')\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
