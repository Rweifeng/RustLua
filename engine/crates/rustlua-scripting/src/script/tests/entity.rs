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

/// WP6-09：实体句柄的**跨运行时歧义**，以及谓词对**退化值**的边界语义。
///
/// 背景：`entity`/`transform` 这套在 web 旧运行时里是它自己用 Lua 预置表实现的
/// （`web-runtime.js:2040-2098`），旧运行时的实体 id 是**从 1 开始的小整数**
/// （`web-runtime.js:2027`）；而工程的持久化写法是 `map.state("buildings")` 里存 `rec.entity`
/// （`building.lua:265`）、读档时 `if rec.entity and entity.is_alive(rec.entity) then`
/// （`building.lua:244/255`）。引擎的句柄是 `index << 32 | generation` 且
/// `INITIAL_GENERATION = 1` ⇒ **引擎自己第一个实体就是 id 1**，与外来小整数逐位重合、无法区分。
///
/// 这条钉住两件**实测**得到的事实（都不是推断——第一版断言"小整数会抛错"就是错的，
/// 是浏览器里的真工程世界把它否证掉的）：
/// ① **退化值**（`generation == 0` 的 `0` / `2^32`、负数）在**查询**里只能是 `false`/`nil`：
///    它们不是任何实体的句柄，以前会抛 `invalid entity id`，把"没有实体"变成脚本崩溃
///    （存档里的哨兵值 `0` / `-1` 就会踩到）；
/// ② **写**操作对同样的值继续报错——改动一个不存在的实体是 bug，必须当场报出来。
#[test]
fn entity_predicates_return_false_for_degenerate_handles_but_writers_still_error() {
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(
        runtime
            .execute_console("return tostring(entity.is_alive(0))")
            .unwrap(),
        "false"
    );
    assert_eq!(
        runtime
            .execute_console("return tostring(entity.is_alive(4294967296))")
            .unwrap(),
        "false",
        "高位为 0 的 2^32 是 generation == 0 的退化句柄"
    );
    assert_eq!(
        runtime
            .execute_console("return tostring(entity.is_alive(-1))")
            .unwrap(),
        "false"
    );
    assert_eq!(
        runtime
            .execute_console("return tostring(entity.persist_id(0))")
            .unwrap(),
        "nil"
    );
    let error = runtime
        .execute_console("transform.set(0, 0, 0)")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("invalid entity id"),
        "写操作必须仍然报错，实际：{error}"
    );
}

/// ① 与旧运行时的**歧义**：引擎首个实体的 Lua 句柄就是 `1`，于是旧存档里的 `1` 会**指向它**
/// （而旧运行时里那个 `1` 很可能是另一个实体）。这不是本片能修的——除非改句柄编码，
/// 而那会破坏现有 Rust 侧存档里已经写下的句柄——所以本片只把它**变得可见、可断言**，
/// 并作为"退休 JS 世界"要单独决策的一项记录在计划里（旧档里的实体引用要么迁移、
/// 要么工程改用 `persist_id` 字符串引用）。
#[test]
fn first_entity_handle_collides_with_foreign_small_ids() {
    let mut runtime = LuaRuntime::new().unwrap();
    let report = runtime
        .execute_console(
            "local id = entity.spawn('First')\n\
             return tostring(id) .. '/' .. tostring(entity.is_alive(id)) .. '/' .. tostring(entity.is_alive(1))",
        )
        .unwrap();
    assert_eq!(
        report, "1/true/true",
        "首个实体的句柄是 1：外来小整数 id 与它无法区分（旧存档的实体引用因此可能指向别的实体）"
    );
}

/// 同名 API 的往返语义（工程 `npc.lua` / `enemy.lua` / `effects.lua` 走的正是这条路）：
/// `spawn(name, persist_id)` → `find` / `find_id` / `persist_id` → `destroy` 之后三者全部失效；
/// 重复注册 `persist_id` 要报错（旧运行时同一条规则：`persist_id already in use`）。
#[test]
fn entity_lifecycle_matches_the_web_prelude_round_trip() {
    let mut runtime = LuaRuntime::new().unwrap();
    let report = runtime
        .execute_console(
            "local id = entity.spawn('Npc', 'npc_1')\n\
             local same = entity.find('Npc') == id and entity.find_id('npc_1') == id\n\
             local named = entity.persist_id(id) == 'npc_1'\n\
             local alive = entity.is_alive(id)\n\
             entity.destroy(id)\n\
             local gone = (not entity.is_alive(id)) and entity.find('Npc') == nil and entity.find_id('npc_1') == nil\n\
             return table.concat({ tostring(same), tostring(named), tostring(alive), tostring(gone) }, '/')",
        )
        .unwrap();
    assert_eq!(report, "true/true/true/true");

    let error = runtime
        .execute_console("local a = entity.spawn(nil, 'dup')\nlocal b = entity.spawn(nil, 'dup')")
        .unwrap_err()
        .to_string();
    assert!(error.contains("persist_id already in use"), "实际：{error}");
}

/// `entity.find` 的**交出契约**：只交活着的句柄——死句柄交成 `nil`，而不是让调用方在使用点
/// 吃一句 `invalid entity id`。这是上面那条"退化句柄在查询里只能是 false/nil"教条的另一半：
/// 查询侧不该把"不在了"变成脚本崩溃。
///
/// 第 58 轮在 webgl 产物上实测到反例：`ScaleBar` 在场景里**确实存在**（`main.scene` 里 3 处声明）、
/// 脚本每次调用都**重新** `entity.find`（没有跨帧缓存），却拿到一个代际已过期的句柄，
/// 直到真正使用时才被引擎拒掉（`invalid entity id: EntityId(0:g13)`）。
/// 名字表里留着一个已经死掉的句柄是可能的：**换世界的那条路不止一条**
/// （`process_scene_transition` 会清名字表，而 JS 宿主自己那套场景切换走的是另一条路），
/// 所以"查找"这一侧必须自己把住。
#[test]
fn entity_find_never_hands_out_a_dead_handle() {
    let mut runtime = LuaRuntime::new().unwrap();
    assert_eq!(
        runtime
            .execute_console(
                "local id = entity.spawn('Hero')\n\
                 return tostring(entity.find('Hero') == id)",
            )
            .unwrap(),
        "true",
        "正常路径：名字查得到，且就是刚 spawn 的那个句柄"
    );

    // 绕过 `entity.destroy`（它会顺手清名字表），直接把实体从世界里摘掉——复刻实测到的状态：
    // **名字表里留着死句柄**。
    let removed = runtime.with_world_mut(|world| {
        let ids = world.entities_with::<Transform>();
        ids.into_iter()
            .filter(|id| world.despawn(*id).is_ok())
            .count()
    });
    assert!(removed >= 1, "至少要摘掉一个实体，否则这条用例什么也没验");

    assert_eq!(
        runtime
            .execute_console("return tostring(entity.find('Hero') ~= nil)")
            .unwrap(),
        "false",
        "死活校验：名字表里指向死句柄的项必须交成 nil"
    );
}

/// `entity_handle_lua`：给 JS 宿主用的「名字 → **引擎句柄**」出口（Lua 侧编码 `index << 32 | generation`）。
///
/// 它存在的理由是两个 id 空间必须能换算（计划 WP7-30）：JS 世界用 `.scene` 的作者 id 当实体 id，
/// 而引擎按 `index << 32 | generation` 解包——实测 `entity\t13\tScaleBar` 的作者 id `13`
/// 被解成 `EntityId(0:g13)`，`quad.get` 直接报 `invalid entity id`。这条用例钉两件事：
/// 出口给出的编码与 Lua 侧**逐位一致**（否则换算出来的句柄还是错的），以及死句柄不许从它漏出去。
#[test]
fn entity_handle_lua_matches_the_lua_side_encoding_and_hides_dead_handles() {
    let mut runtime = LuaRuntime::new().unwrap();
    let lua_handle = runtime
        .execute_console("return tostring(entity.spawn('Hero'))")
        .unwrap()
        .parse::<i64>()
        .expect("spawn 必须给出一个可解析的 Lua 句柄");
    assert_eq!(
        runtime.entity_handle_lua("Hero"),
        Some(lua_handle),
        "出口必须给出与 Lua 侧逐位一致的编码"
    );
    assert_eq!(
        runtime.entity_handle_lua("Nope"),
        None,
        "不认识的名字给 None"
    );

    let removed = runtime.with_world_mut(|world| {
        let ids = world.entities_with::<Transform>();
        ids.into_iter()
            .filter(|id| world.despawn(*id).is_ok())
            .count()
    });
    assert!(removed >= 1, "至少要摘掉一个实体，否则这条用例什么也没验");
    assert_eq!(
        runtime.entity_handle_lua("Hero"),
        None,
        "死句柄不许从这个出口漏给 JS 宿主"
    );
}
