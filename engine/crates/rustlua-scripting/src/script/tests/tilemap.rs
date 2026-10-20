use super::*;

#[test]
fn tilemap_find_path_returns_path_or_diagnostic() {
    let path = script(
        "function on_create()\n\
         local m = tilemap.create(4, 3, 32)\n\
         tilemap.set_solid(m, 1, 0, true)\n\
         tilemap.set_solid(m, 2, 0, true)\n\
         local cells, err = tilemap.find_path(m, 0, 0, 3, 0, false)\n\
         assert(err == nil)\n\
         assert(#cells == 6)\n\
         assert(cells[1].x == 0 and cells[1].y == 0)\n\
         assert(cells[#cells].x == 3 and cells[#cells].y == 0)\n\
         local blocked, reason = tilemap.find_path(m, 0, 0, 1, 0, false)\n\
         assert(blocked == nil and reason ~= nil)\n\
         return 'ok' end\n\
         function update() return 'ok' end",
    );
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
#[test]
fn instantiated_tilemap_is_queryable_from_lua() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let mut scene = Scene::new("test");
    scene.add_entity(SceneEntityId(1), "Map", None).unwrap();
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
        .insert(
            "Tilemap".into(),
            "tile_size=32x32;width=4;height=3;tiles=100101000100;solid=100000000000".into(),
        );

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

    let path = script(
        "function on_create()\nlocal m = entity.find('Map')\nlocal d = tilemap.get(m)\nassert(d ~= nil)\nassert(d.width == 4 and d.height == 3 and d.tile_size == 32)\nassert(tilemap.is_solid(m, 0, 0))\nassert(not tilemap.is_solid(m, 1, 1))\nassert(not tilemap.is_solid(m, 9, 9))\nassert(tilemap.get_cell(m, 3, 0) == 1)\nassert(tilemap.get_cell(m, 1, 0) == 0)\nreturn 'ok' end\nfunction update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");

    let maps = runtime.with_world(|world| world.entities_with::<Tilemap>());
    assert_eq!(maps.len(), 1);
    let map = runtime.with_world(|world| world.get::<Tilemap>(maps[0]).unwrap().unwrap().clone());
    assert_eq!(map.width, 4);
    assert_eq!(map.is_solid(0, 0), Some(true));
    let _ = fs::remove_file(path);
}

#[test]
fn grid_tilemap_tag_and_interact_lua_api() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let mut scene = Scene::new("farm");
    scene.add_entity(SceneEntityId(1), "FarmMap", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Tilemap".into(),
            "tile_size=32x32;width=4;height=3;texture=assets/tileset.png;tiles=000000000000;solid=000000000000"
                .into(),
        );
    scene.add_entity(SceneEntityId(2), "Farmer", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=16,16,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.instantiate_scene(&scene).unwrap();
    let path = script(
        "local map = entity.find('FarmMap')\n\
         local player = entity.find('Farmer')\n\
         local cell = grid.world_to_cell(48, 16, 32)\n\
         assert(cell[1] == 1 and cell[2] == 0)\n\
         local front = grid.facing_cell(16, 16, 'right', 32)\n\
         assert(front[1] == 1 and front[2] == 0)\n\
         assert(grid.cell_to_world(1, 0, 32)[1] == 48)\n\
         tilemap.set_cell(map, 1, 0, 2)\n\
         tilemap.set_solid(map, 1, 0, true)\n\
         tilemap.set_meta(map, 1, 0, 'tilled', 'true')\n\
         assert(tilemap.get_meta(map, 1, 0, 'tilled') == 'true')\n\
         assert(tilemap.is_solid(map, 1, 0))\n\
         assert(tilemap.get(map).texture == 'assets/tileset.png')\n\
         tilemap.set_texture(map, 'assets/art/tilesets/manor/manor_ground_winter.png')\n\
         assert(tilemap.get(map).texture == 'assets/art/tilesets/manor/manor_ground_winter.png')\n\
         assert(not pcall(function() tilemap.set_texture(map, '  ') end))\n\
         assert(not pcall(function() tilemap.set_texture(player, 'assets/tileset.png') end))\n\
         tag.set(player, 'kind', 'farmer')\n\
         local hits = interact.query(16, 16, 100)\n\
         assert(#hits >= 1 and hits[1].kind == 'farmer')\n\
         interact.prompt('按 E 使用工具')\n\
         assert(interact.get_prompt() == '按 E 使用工具')\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
