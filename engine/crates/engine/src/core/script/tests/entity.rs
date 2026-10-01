use super::*;

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
    let mut scene = crate::core::scene::Scene::new("lua-visibility");
    let panel = crate::core::scene::SceneEntityId(1);
    let plain = crate::core::scene::SceneEntityId(2);
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
    let mut scene = crate::core::scene::Scene::new("lua-quad");
    let bar = crate::core::scene::SceneEntityId(1);
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
