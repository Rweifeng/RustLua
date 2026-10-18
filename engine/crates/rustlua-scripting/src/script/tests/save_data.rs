use super::*;

#[test]
fn clock_save_slots_blobs_map_inventory_and_data_lua_api() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let root = std::env::temp_dir().join(format!("rustlua-farm-api-{}", std::process::id()));
    let scenes = root.join("scenes");
    let data = root.join("data");
    fs::create_dir_all(&scenes).unwrap();
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("crops.json"),
        r#"{"tomato":{"name":"Tomato","sell":5,"buy":10,"stack":8}}"#,
    )
    .unwrap();
    let mut town = Scene::new("town");
    town.add_entity(SceneEntityId(1), "Spawn", None).unwrap();
    town.save(scenes.join("town.scene")).unwrap();

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    runtime.set_save_path(root.join(".data").join("save.sav"));
    runtime.set_ui_viewport(1280, 720);
    let path = script(
        "local now = clock.now()\n\
         assert(now.day == 1 and now.minute == 360)\n\
         clock.set_rate(60)\n\
         clock.pause(false)\n\
         save.slot('slot1')\n\
         save.write('hp', '42')\n\
         assert(save.commit())\n\
         assert(save.slot_get() == 'slot1')\n\
         save.blob_write('farm', { tilled = { true }, day = 2 })\n\
         local farm = save.blob_read('farm')\n\
         assert(farm.tilled[1] == true and farm.day == 2)\n\
         item.define('tomato', { name = 'Tomato', stack = 8, sell = 5, buy = 10 })\n\
         assert(item.def('tomato').sell == 5)\n\
         local bag = inv.create('bag', 4)\n\
         assert(inv.add('bag', 'tomato', 3) == 0)\n\
         assert(inv.count('bag', 'tomato') == 3)\n\
         assert(inv.move('bag', 1, 'bag', 2, nil) == 3)\n\
         assert(inv.slot('bag', 2).count == 3)\n\
         assert(inv.move('bag', 2, 'bag', 1, nil) == 3)\n\
         local box = container.open(88, 2)\n\
         assert(inv.move('bag', 1, box, 1, nil) == 3)\n\
         assert(inv.count('bag', 'tomato') == 0)\n\
         assert(inv.count(box, 'tomato') == 3)\n\
         assert(inv.swap(box, 1, 'bag', 1))\n\
         assert(inv.count(box, 'tomato') == 0)\n\
         inv.equip('bag', 'tool', 'hoe')\n\
         assert(inv.equip_get('bag', 'tool') == 'hoe')\n\
         assert(inv.equip_list('bag').tool == 'hoe')\n\
         inv.equip('bag', 'tool', nil)\n\
         assert(inv.equip_get('bag', 'tool') == nil)\n\
         assert(map.go('town', 120, 80))\n\
         assert(map.current() == 'town')\n\
         local spawn = map.spawn()\n\
         assert(spawn.x == 120 and spawn.y == 80)\n\
         map.state('farm', { tilled = true })\n\
         assert(map.state('farm').tilled == true)\n\
         local crops = data.load('data/crops.json')\n\
         assert(crops.tomato.name == 'Tomato' and crops.tomato.sell == 5)\n\
         assert(data.reload('data/crops.json').tomato.buy == 10)\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    runtime.step_clock(1.0);
    runtime
        .execute_console("assert(clock.now().minute == 420)")
        .unwrap();
    assert!(root.join(".data").join("save_slot1.sav").is_file());
    assert!(!root.join(".data").join("save.sav").is_file());
    runtime.process_scene_transition().unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(path);
}

#[test]
fn map_state_schema_rejects_invalid_writes_and_can_clear() {
    let mut runtime = LuaRuntime::new().unwrap();
    let path = script(
        "map.schema('farm', { ['$kind'] = 'object', \
         fields = { version = 'integer', \
                    tiles = { ['$kind'] = 'array', items = 'integer' }, \
                    solid = { ['$kind'] = 'array', items = 'boolean' }, \
                    meta = { ['$kind'] = 'map', values = 'any' } }, \
         required = { 'version', 'tiles', 'solid' } })\n\
         assert(map.state('farm', { version = 1, tiles = { 0 }, solid = { false }, meta = {} }))\n\
         local good = map.state('farm')\n\
         assert(good.version == 1 and good.tiles[1] == 0 and good.solid[1] == false)\n\
         local ok1, err1 = pcall(function() map.state('farm', { version = 'x' }) end)\n\
         assert(not ok1 and tostring(err1):find('map.state farm'), tostring(err1))\n\
         local ok2, err2 = pcall(function() map.state('farm', { version = 1, tiles = { 'a' }, solid = { false } }) end)\n\
         assert(not ok2 and tostring(err2):find('tiles'), tostring(err2))\n\
         local ok3, err3 = pcall(function() map.state('farm', { version = 1, tiles = { 0 } }) end)\n\
         assert(not ok3 and tostring(err3):find('missing required field'), tostring(err3))\n\
         map.schema('farm', nil)\n\
         assert(map.state('farm', { anything = true }))\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}

#[test]
fn map_state_tilemap_snapshot_survives_scene_change_and_restart() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let root = std::env::temp_dir().join(format!("rustlua-mapstate-b3-{}", std::process::id()));
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).unwrap();
    let save_path = root.join("save.sav");
    let _ = fs::remove_file(&save_path);

    // farm.scene 包含 4x3 Tilemap，初始 tile/solid 全为空。
    let mut farm = Scene::new("farm");
    farm.add_entity(SceneEntityId(1), "FarmMap", None).unwrap();
    let farm_entity = farm.entities.get_mut(&SceneEntityId(1)).unwrap();
    farm_entity.components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    farm_entity.components.insert(
        "Tilemap".into(),
        "tile_size=32x32;width=4;height=3;tiles=000000000000;solid=000000000000".into(),
    );
    farm.save(scenes.join("farm.scene")).unwrap();
    let mut home = Scene::new("home");
    home.add_entity(SceneEntityId(1), "Home", None).unwrap();
    home.save(scenes.join("home.scene")).unwrap();

    // 启动脚本注册分地图 schema 与快照/恢复 helper，模拟游戏侧
    // `map_state.lua` 的职责：脚本维护 tile/meta/solid 格式，引擎用
    // `map.schema` 校验，并存入 `__mapstate:<name>`。
    let boot = "map.schema('farm', { ['$kind'] = 'object', \
                fields = { version = 'integer', \
                           tiles = { ['$kind'] = 'array', items = 'integer' }, \
                           solid = { ['$kind'] = 'array', items = 'boolean' }, \
                           meta = { ['$kind'] = 'map', values = 'any' } }, \
                required = { 'version', 'tiles', 'solid' } })\n\
                function farm_size() return 4, 3 end\n\
                function snapshot_farm(m)\n\
                  local w, h = farm_size()\n\
                  local st = { version = 1, tiles = {}, solid = {}, meta = {} }\n\
                  for y = 0, h - 1 do\n\
                    for x = 0, w - 1 do\n\
                      local i = y * w + x + 1\n\
                      st.tiles[i] = tilemap.get_cell(m, x, y)\n\
                      st.solid[i] = tilemap.is_solid(m, x, y)\n\
                    end\n\
                  end\n\
                  for y = 0, h - 1 do\n\
                    for x = 0, w - 1 do\n\
                      local bag = {}\n\
                      for _, k in ipairs({ 'tilled', 'crop', 'state' }) do\n\
                        local v = tilemap.get_meta(m, x, y, k)\n\
                        if v then bag[k] = v end\n\
                      end\n\
                      if next(bag) then st.meta[x .. ',' .. y] = bag end\n\
                    end\n\
                  end\n\
                  return st\n\
                end\n\
                function restore_farm(m, st)\n\
                  local w, h = farm_size()\n\
                  for y = 0, h - 1 do\n\
                    for x = 0, w - 1 do\n\
                      local i = y * w + x + 1\n\
                      tilemap.set_cell(m, x, y, st.tiles[i])\n\
                      tilemap.set_solid(m, x, y, st.solid[i])\n\
                    end\n\
                  end\n\
                  for cell, bag in pairs(st.meta or {}) do\n\
                    local x, y = cell:match('^(%d+),(%d+)$')\n\
                    assert(x and y, 'bad meta cell ' .. tostring(cell))\n\
                    for k, v in pairs(bag) do\n\
                      tilemap.set_meta(m, tonumber(x), tonumber(y), k, v)\n\
                    end\n\
                  end\n\
                end\n\
                function assert_farm_restored(m)\n\
                  assert(tilemap.get_cell(m, 1, 2) == 3, 'tile 1,2')\n\
                  assert(tilemap.is_solid(m, 1, 2) == true, 'solid 1,2')\n\
                  assert(tilemap.get_meta(m, 0, 0, 'tilled') == 'yes', 'tilled 0,0')\n\
                  assert(tilemap.get_meta(m, 2, 1, 'crop') == 'tomato', 'crop 2,1')\n\
                  assert(tilemap.get_meta(m, 1, 1, 'tilled') == nil, 'no stray meta')\n\
                end\n\
                function update() return 'ok' end";

    // ---- first session: farm -> edit -> store -> home -> farm -> restore.
    // 启动脚本只加载一次；后续步骤通过 `execute_console` 在同一 VM 中执行，
    // 因为 `execute_project_script` 会重建 Lua VM，不能依赖跨脚本的全局 helper。
    // 切图不会重置 VM，因此 snapshot/restore helper 会跨 map.go 保留。
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    runtime.set_save_path(save_path.clone());
    let path = script(boot);
    runtime.execute_project_script(&path).unwrap();

    runtime
        .execute_console("assert(map.go('farm', 5, 5))")
        .unwrap();
    runtime.process_scene_transition().unwrap();
    runtime
        .execute_console(
            "local m = assert(entity.find('FarmMap'), 'FarmMap')\n\
             tilemap.set_cell(m, 1, 2, 3)\n\
             tilemap.set_solid(m, 1, 2, true)\n\
             tilemap.set_meta(m, 0, 0, 'tilled', 'yes')\n\
             tilemap.set_meta(m, 2, 1, 'crop', 'tomato')\n\
             assert(map.state('farm', snapshot_farm(m)), 'store farm state')\n\
             local st = map.state('farm')\n\
             assert(st.version == 1, 'version')\n\
             assert(st.tiles[10] == 3, 'tiles[10]')\n\
             assert(st.solid[10] == true, 'solid[10]')\n\
             assert(st.meta['0,0'].tilled == 'yes', 'meta 0,0')\n",
        )
        .unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    runtime
        .execute_console("assert(map.go('home', 5, 5))")
        .unwrap();
    runtime.process_scene_transition().unwrap();
    runtime
        .execute_console("assert(map.go('farm', 5, 5))")
        .unwrap();
    runtime.process_scene_transition().unwrap();
    // 切回地图时场景中的初始值优先；必须重新应用快照，才能保证
    // tile/meta/solid 与持久化状态一致。
    runtime
        .execute_console(
            "local m = assert(entity.find('FarmMap'), 'FarmMap')\n\
             assert(tilemap.get_cell(m, 1, 2) == 0, 'scene baseline on re-entry')\n\
             local st = assert(map.state('farm'), 'state survives scene change')\n\
             restore_farm(m, st)\n\
             assert_farm_restored(m)\n\
             assert(save.commit(), 'commit')\n",
        )
        .unwrap();
    runtime.call_update().unwrap();
    assert!(save_path.is_file(), "session one should commit a save file");

    // ---- second session: fresh runtime boots on the same project/save.
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    runtime.set_save_path(save_path.clone());
    let boot_path = script(boot);
    runtime.execute_project_script(&boot_path).unwrap();
    runtime
        .execute_console(
            "assert(save.load(), 'load save')\n\
             local st = map.state('farm')\n\
             assert(st ~= nil and st.version == 1, 'restart state present')\n\
             assert(st.tiles[10] == 3, 'restart tiles')\n\
             assert(map.go('farm', 5, 5))\n",
        )
        .unwrap();
    runtime.process_scene_transition().unwrap();
    runtime
        .execute_console(
            "local m = assert(entity.find('FarmMap'), 'FarmMap')\n\
             restore_farm(m, assert(map.state('farm'), 'restart state'))\n\
             assert_farm_restored(m)\n",
        )
        .unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(boot_path);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn data_schema_registration_rejects_invalid_tables_on_load() {
    let root = std::env::temp_dir().join(format!("rustlua-data-schema-{}", std::process::id()));
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("items.json"),
        r#"{"tomato":{"name":"Tomato","stack":8}}"#,
    )
    .unwrap();
    fs::write(data.join("bad.json"), r#"{"tomato":{"stack":"big"}}"#).unwrap();

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    let path = script(
        "local items_schema = { ['$kind'] = 'map', values = { name = 'string', stack = 'integer' } }\n\
         data.schema('data/items.json', items_schema)\n\
         data.schema('data/bad.json', items_schema)\n\
         local ok = data.load('data/items.json')\n\
         assert(ok.tomato.name == 'Tomato')\n\
         assert(not pcall(function() data.load('data/bad.json') end))\n\
         local err = select(2, pcall(function() data.load('data/bad.json') end))\n\
         assert(tostring(err):find('schema violation'), err)\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(path);
}

/// 「开始新游戏」必须能丢弃引擎侧持久键：`save.clear()` 清空全部内存键，
/// `map.state_clear()` 只清 `__mapstate:*`（可指定单个状态名）。
/// 这些键不属于任何游戏侧 provider，残留会让新档读回上一局的地图/农田/建筑。
#[test]
fn save_clear_and_map_state_clear_drop_persisted_keys() {
    let root = std::env::temp_dir().join(format!("rustlua-state-clear-{}", std::process::id()));
    let dir = root.join(".data");
    fs::create_dir_all(&dir).unwrap();
    let save_path = dir.join("save.sav");

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_save_path(save_path.clone());
    let path = script(
        "save.write('hp', '75')\n\
         local cleared = save.clear()\n\
         assert(cleared >= 1, 'clear should report removed keys: ' .. tostring(cleared))\n\
         assert(save.read('hp') == nil, 'keys are gone after clear')\n\
         assert(map.state('farm', { tiles = { 1, 2 } }))\n\
         assert(map.state('buildings', { version = 1, list = {} }))\n\
         local removed = map.state_clear()\n\
         assert(removed >= 2, 'state_clear removes every __mapstate key: ' .. tostring(removed))\n\
         assert(map.state('farm') == nil, 'farm state cleared')\n\
         assert(map.state('buildings') == nil, 'buildings state cleared')\n\
         assert(map.state('single', { x = 1 }))\n\
         map.state_clear('single')\n\
         assert(map.state('single') == nil, 'named clear removes that state')\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(path);
}

/// 新档清理的端到端语义：写档 → 读回 → `save.clear()` → 再次读回为空，
/// 且地图/出生点键同样被丢弃（`map.current()` 回落到当前场景）。
#[test]
fn new_game_style_clear_removes_map_keys_and_survives_reload() {
    let root = std::env::temp_dir().join(format!("rustlua-newgame-clear-{}", std::process::id()));
    let dir = root.join(".data");
    fs::create_dir_all(&dir).unwrap();
    let save_path = dir.join("save.sav");

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_save_path(save_path.clone());
    let writer = script(
        "map.set_spawn(120, 240)\n\
         save.write('hp', '75')\n\
         assert(save.commit())\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&writer).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    assert!(save_path.is_file());

    // 第二次会话：读档 → 清档 → 落盘 → 第三次会话读回：什么都不该剩下。
    let mut fresh = LuaRuntime::new().unwrap();
    fresh.set_save_path(save_path.clone());
    let clearer = script(
        "local ok = save.load()\n\
         assert(ok, 'save should load')\n\
         assert(save.read('hp') == '75')\n\
         assert(map.spawn() ~= nil, 'spawn persisted')\n\
         save.clear()\n\
         map.state_clear()\n\
         assert(save.read('hp') == nil)\n\
         assert(map.spawn() == nil, 'spawn key dropped by clear')\n\
         assert(save.commit())\n\
         function update() return 'ok' end",
    );
    fresh.execute_project_script(&clearer).unwrap();
    assert_eq!(fresh.call_update().unwrap(), "ok");

    let mut third = LuaRuntime::new().unwrap();
    third.set_save_path(save_path.clone());
    let reader = script(
        "local ok = save.load()\n\
         assert(ok, 'cleared save should still be a valid file')\n\
         assert(save.read('hp') == nil, 'no keys survive a cleared new game')\n\
         assert(map.spawn() == nil, 'no spawn survives a cleared new game')\n\
         function update() return 'ok' end",
    );
    third.execute_project_script(&reader).unwrap();
    assert_eq!(third.call_update().unwrap(), "ok");

    let _ = fs::remove_dir_all(root);
    for path in [writer, clearer, reader] {
        let _ = fs::remove_file(path);
    }
}

#[test]
fn save_load_rejects_older_save() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-save-version-reject-{}",
        std::process::id()
    ));
    let dir = root.join(".data");
    fs::create_dir_all(&dir).unwrap();
    let save_path = dir.join("save.sav");
    fs::write(&save_path, "save_version=0\nhp\t10\n").unwrap();

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_save_path(save_path.clone());
    let path = script("assert(not save.load())\nfunction update() return 'ok' end");
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(path);
}

/// 回归（P0）：多行/含制表符的值曾经写出伪行，使**整个存档**在下次 load
/// 时无法解析。现在 `save.write` + `commit` + 新会话 `save.load` 必须无损往返。
#[test]
fn save_write_round_trips_multiline_and_cjk_values_across_sessions() {
    let root = std::env::temp_dir().join(format!("rustlua-save-escape-{}", std::process::id()));
    let dir = root.join(".data");
    fs::create_dir_all(&dir).unwrap();
    let save_path = dir.join("save.sav");

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_save_path(save_path.clone());
    let writer = script(
        "save.write('hp', '75')\n\
         save.write('note', 'line one\\nline two')\n\
         save.write('tabbed', 'a\\tb')\n\
         save.write('草药', '灵芝 x2')\n\
         assert(save.commit(), 'commit')\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&writer).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    assert!(save_path.is_file(), "save file should exist after commit");

    // 磁盘上每个键值对只占一行（1 行版本头 + 4 行数据）。
    let text = fs::read_to_string(&save_path).unwrap();
    assert_eq!(text.lines().count(), 5, "unexpected save text: {text:?}");
    assert!(text.starts_with("save_version=2\n"));

    let mut fresh = LuaRuntime::new().unwrap();
    fresh.set_save_path(save_path.clone());
    let reader = script(
        "local ok, reason = save.load()\n\
         assert(ok, tostring(reason))\n\
         assert(save.read('hp') == '75', 'hp')\n\
         assert(save.read('note') == 'line one\\nline two', 'note')\n\
         assert(save.read('tabbed') == 'a\\tb', 'tabbed')\n\
         assert(save.read('草药') == '灵芝 x2', 'cjk')\n\
         function update() return 'ok' end",
    );
    fresh.execute_project_script(&reader).unwrap();
    assert_eq!(fresh.call_update().unwrap(), "ok");

    let _ = fs::remove_dir_all(root);
    for path in [writer, reader] {
        let _ = fs::remove_file(path);
    }
}

/// `save.load` 必须区分「没有存档」与「存档已损坏」：两者都返回 false，
/// 但第二个返回值给出原因，诊断面板据此才能告诉玩家进度是丢了还是从没存过。
#[test]
fn save_load_reports_missing_versus_corrupt() {
    let root = std::env::temp_dir().join(format!("rustlua-save-reason-{}", std::process::id()));
    let dir = root.join(".data");
    fs::create_dir_all(&dir).unwrap();
    let save_path = dir.join("save.sav");

    // 1) 文件不存在 → "missing"
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_save_path(save_path.clone());
    let missing = script(
        "local ok, reason = save.load()\n\
         assert(ok == false)\n\
         assert(reason == 'missing', tostring(reason))\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&missing).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");

    // 2) 损坏文件 → "corrupt: ..."，且原文件不被覆盖
    let corrupt_text = "save_version=1\nhp\t75\nbroken row\n";
    fs::write(&save_path, corrupt_text).unwrap();
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_save_path(save_path.clone());
    let corrupt = script(
        "local ok, reason = save.load()\n\
         assert(ok == false)\n\
         assert(reason:sub(1, 8) == 'corrupt:', tostring(reason))\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&corrupt).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    assert_eq!(
        fs::read_to_string(&save_path).unwrap(),
        corrupt_text,
        "损坏存档必须原样保留，绝不能被覆盖"
    );

    let _ = fs::remove_dir_all(root);
    for path in [missing, corrupt] {
        let _ = fs::remove_file(path);
    }
}

/// `save.write` 必须拒绝空键：它可以被写进内存，但会让整份存档在下次
/// `load` 时解析失败（等于一次误用毁掉玩家进度）。
#[test]
fn save_write_rejects_empty_key() {
    let mut runtime = LuaRuntime::new().unwrap();
    let path = script(
        "assert(not pcall(function() save.write('', 'x') end))\n\
         local err = select(2, pcall(function() save.write('', 'x') end))\n\
         assert(tostring(err):find('must not be empty'), tostring(err))\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}

/// 回归：v1 旧存档（裸行）仍然可读，且读入后能正常 commit 成 v2。
#[test]
fn legacy_v1_save_still_loads_and_upgrades_on_commit() {
    let root = std::env::temp_dir().join(format!("rustlua-save-v1-{}", std::process::id()));
    let dir = root.join(".data");
    fs::create_dir_all(&dir).unwrap();
    let save_path = dir.join("save.sav");
    fs::write(&save_path, "save_version=1\nhp\t75\nscene\tarena\n").unwrap();

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_save_path(save_path.clone());
    let path = script(
        "local ok, reason = save.load()\n\
         assert(ok, tostring(reason))\n\
         assert(save.read('hp') == '75')\n\
         assert(save.read('scene') == 'arena')\n\
         save.write('note', 'line one\\nline two')\n\
         assert(save.commit())\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let upgraded = fs::read_to_string(&save_path).unwrap();
    assert!(
        upgraded.starts_with("save_version=2\n"),
        "读入 v1 后应写成 v2: {upgraded:?}"
    );
    assert_eq!(
        rustlua_gameplay::save::decode(&upgraded).unwrap().get("hp"),
        Some(&"75".to_owned())
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(path);
}

#[test]
fn save_commit_load_and_scene_transition() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let root = std::env::temp_dir().join(format!("rustlua-save-scene-{}", std::process::id()));
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).unwrap();
    let mut arena = Scene::new("arena");
    arena.add_entity(SceneEntityId(1), "Player", None).unwrap();
    arena
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    arena
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Sprite".into(),
            "texture=assets/player.png;size=32x32".into(),
        );
    arena.save(scenes.join("arena.scene")).unwrap();

    // save + commit to disk
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    runtime.set_save_path(root.join(".data").join("save.sav"));
    let first = script(
        "save.write('hp', '75')\nsave.write('scene', 'arena')\nassert(save.commit())\nfunction update() return 'ok' end",
    );
    runtime.execute_project_script(&first).unwrap();
    runtime.call_update().unwrap();
    assert!(root.join(".data").join("save.sav").is_file());

    // a fresh runtime loads the save back
    let mut fresh = LuaRuntime::new().unwrap();
    fresh.set_save_path(root.join(".data").join("save.sav"));
    let load_script = script(
        "assert(save.load())\nassert(save.read('hp') == '75')\nfunction update() return 'ok' end",
    );
    fresh.execute_project_script(&load_script).unwrap();
    fresh.call_update().unwrap();

    // scene.load queues a transition; process_scene_transition applies it
    let switch = script("assert(scene.load('arena'))\nfunction update() return 'ok' end");
    runtime.execute_project_script(&switch).unwrap();
    runtime.process_scene_transition().unwrap();
    let players = runtime.with_world(|world| world.entities_with::<Sprite>());
    assert_eq!(players.len(), 1, "arena player instantiated after switch");
    assert_eq!(runtime.with_world(|world| world.entity_count()), 1);
    // Lua can find the freshly-instantiated player after the transition
    let check = script(
        "local p = entity.find('Player')\nassert(p ~= nil)\nfunction update() return 'ok' end",
    );
    runtime.execute_project_script(&check).unwrap();
    runtime.call_update().unwrap();

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(first);
    let _ = fs::remove_file(load_script);
    let _ = fs::remove_file(switch);
    let _ = fs::remove_file(check);
}

#[test]
fn persistent_entities_survive_scene_transition() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;
    use rustlua_ui::ui::UiText;

    let root = std::env::temp_dir().join(format!("rustlua-persist-{}", std::process::id()));
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).unwrap();

    let mut arena = Scene::new("arena");
    arena.add_entity(SceneEntityId(1), "Player", None).unwrap();
    arena
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Sprite".into(),
            "texture=assets/player.png;size=32x32".into(),
        );
    arena.add_entity(SceneEntityId(2), "Hud", None).unwrap();
    let hud = arena.entities.get_mut(&SceneEntityId(2)).unwrap();
    hud.components.insert(
        "Text".into(),
        "text=Score:0;font_size=24;color=1,1,1".into(),
    );
    hud.components.insert(
        "UiLayout".into(),
        "anchors=0,0,0,0;offsets=10,10,10,10".into(),
    );
    hud.metadata.insert("persistent".into(), "true".into());
    arena.save(scenes.join("arena.scene")).unwrap();

    let mut hub = Scene::new("hub");
    hub.add_entity(SceneEntityId(1), "Boss", None).unwrap();
    hub.entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Sprite".into(), "texture=assets/boss.png;size=32x32".into());
    hub.save(scenes.join("hub.scene")).unwrap();

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    let load_arena = script("assert(scene.load('arena'))\nfunction update() return 'ok' end");
    runtime.execute_project_script(&load_arena).unwrap();
    runtime.process_scene_transition().unwrap();
    let load_hub = script("assert(scene.load('hub'))\nfunction update() return 'ok' end");
    runtime.execute_project_script(&load_hub).unwrap();
    runtime.process_scene_transition().unwrap();

    // Hud (persistent, UI text) survives; arena Player gone; hub Boss present.
    let ui_texts: Vec<String> = runtime.with_world(|world| {
        world
            .entities_with::<UiText>()
            .into_iter()
            .filter_map(|id| {
                world
                    .get::<UiText>(id)
                    .ok()
                    .flatten()
                    .map(|t| t.text.clone())
            })
            .collect()
    });
    assert_eq!(ui_texts.len(), 1, "persistent Hud text survives");
    assert_eq!(ui_texts[0], "Score:0");
    let sprites = runtime.with_world(|world| world.entities_with::<Sprite>());
    assert_eq!(sprites.len(), 1, "only hub Boss sprite remains");
    assert_eq!(runtime.with_world(|world| world.entity_count()), 2);
    let check = script(
        "local hud = entity.find('Hud')\nassert(hud ~= nil)\nlocal boss = entity.find('Boss')\nassert(boss ~= nil)\nassert(entity.find('Player') == nil)\nfunction update() return 'ok' end",
    );
    runtime.execute_project_script(&check).unwrap();
    runtime.call_update().unwrap();

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(load_arena);
    let _ = fs::remove_file(load_hub);
    let _ = fs::remove_file(check);
}

#[test]
fn persistent_component_survives_transition_and_cascades_to_children() {
    use rustlua_scene::scene::PERSISTENT_COMPONENT;
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let root =
        std::env::temp_dir().join(format!("rustlua-persist-component-{}", std::process::id()));
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).unwrap();

    let mut arena = Scene::new("arena");
    arena.add_entity(SceneEntityId(1), "Root", None).unwrap();
    arena
        .add_entity(SceneEntityId(2), "Child", Some(SceneEntityId(1)))
        .unwrap();
    arena.add_entity(SceneEntityId(3), "Dust", None).unwrap();
    for (id, name) in [(1, "Root"), (2, "Child"), (3, "Dust")] {
        arena
            .entities
            .get_mut(&SceneEntityId(id))
            .unwrap()
            .components
            .insert(
                "Sprite".into(),
                format!("texture=assets/{name}.png;size=32x32"),
            );
    }
    // 只给父节点挂标记组件：子节点没有该组件，也必须随父节点一起存活。
    arena
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(PERSISTENT_COMPONENT.into(), String::new());
    arena.save(scenes.join("arena.scene")).unwrap();

    let mut hub = Scene::new("hub");
    hub.add_entity(SceneEntityId(1), "Boss", None).unwrap();
    hub.entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Sprite".into(), "texture=assets/boss.png;size=32x32".into());
    hub.save(scenes.join("hub.scene")).unwrap();

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    let load_arena = script("assert(scene.load('arena'))\nfunction update() return 'ok' end");
    runtime.execute_project_script(&load_arena).unwrap();
    runtime.process_scene_transition().unwrap();
    let load_hub = script("assert(scene.load('hub'))\nfunction update() return 'ok' end");
    runtime.execute_project_script(&load_hub).unwrap();
    runtime.process_scene_transition().unwrap();

    let state = runtime.state.borrow();
    let root_handle = state
        .entity_state
        .names
        .get("Root")
        .expect("persistent parent restored");
    let child_handle = state
        .entity_state
        .names
        .get("Child")
        .expect("child of persistent parent survives");
    assert_eq!(
        state
            .world
            .get::<rustlua_ecs::hierarchy::Parent>(child_handle)
            .unwrap()
            .map(|parent| parent.0),
        Some(root_handle),
        "恢复后必须按名字重连父子边"
    );
    assert!(state.entity_state.names.contains_key("Boss"));
    assert!(
        !state.entity_state.names.contains_key("Dust"),
        "没有标记组件的兄弟实体不应存活"
    );
    drop(state);

    let check = script(
        "assert(entity.find('Root') ~= nil)\nassert(entity.find('Child') ~= nil)\nassert(entity.find('Dust') == nil)\nfunction update() return 'ok' end",
    );
    runtime.execute_project_script(&check).unwrap();
    runtime.call_update().unwrap();

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(load_arena);
    let _ = fs::remove_file(load_hub);
    let _ = fs::remove_file(check);
}

#[test]
fn persistent_duplicate_name_new_scene_wins() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;
    use rustlua_ui::ui::UiText;

    let root = std::env::temp_dir().join(format!("rustlua-persist-name-{}", std::process::id()));
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).unwrap();

    let mut arena = Scene::new("arena");
    arena.add_entity(SceneEntityId(1), "Hud", None).unwrap();
    let hud = arena.entities.get_mut(&SceneEntityId(1)).unwrap();
    hud.components.insert(
        "Text".into(),
        "text=Score:0;font_size=24;color=1,1,1".into(),
    );
    hud.components.insert(
        "UiLayout".into(),
        "anchors=0,0,0,0;offsets=10,10,10,10".into(),
    );
    hud.metadata.insert("persistent".into(), "true".into());
    arena.save(scenes.join("arena.scene")).unwrap();

    let mut hub = Scene::new("hub");
    hub.add_entity(SceneEntityId(1), "Hud", None).unwrap();
    let hud = hub.entities.get_mut(&SceneEntityId(1)).unwrap();
    hud.components.insert(
        "Text".into(),
        "text=Score:99;font_size=24;color=1,1,1".into(),
    );
    hud.components.insert(
        "UiLayout".into(),
        "anchors=0,0,0,0;offsets=10,10,10,10".into(),
    );
    hud.metadata.insert("persistent".into(), "true".into());
    hub.save(scenes.join("hub.scene")).unwrap();

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    let load_arena = script("scene.load('arena')\nfunction update() return 'ok' end");
    runtime.execute_project_script(&load_arena).unwrap();
    runtime.process_scene_transition().unwrap();
    let load_hub = script("scene.load('hub')\nfunction update() return 'ok' end");
    runtime.execute_project_script(&load_hub).unwrap();
    runtime.process_scene_transition().unwrap();

    let ui_texts: Vec<String> = runtime.with_world(|world| {
        world
            .entities_with::<UiText>()
            .into_iter()
            .filter_map(|id| {
                world
                    .get::<UiText>(id)
                    .ok()
                    .flatten()
                    .map(|t| t.text.clone())
            })
            .collect()
    });
    assert_eq!(ui_texts.len(), 1, "exactly one Hud");
    assert_eq!(ui_texts[0], "Score:99", "new scene's persistent Hud wins");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(load_arena);
    let _ = fs::remove_file(load_hub);
}
