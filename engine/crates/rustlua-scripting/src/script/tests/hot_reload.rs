use super::*;

/// 每实体脚本的容错：某个实体的 `on_update` 报错只跳过**它自己**，另一个实体照常更新——
/// 按实体（而不是按文件）分预算就是为了这一点。
#[test]
fn entity_hook_errors_are_contained_per_entity() {
    let root = std::env::temp_dir().join(format!("rustlua-entity-guard-{}", std::process::id()));
    let scripts = root.join("scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(
        scripts.join("broken.lua"),
        "function on_update(id, dt) error('entity boom') end\n",
    )
    .unwrap();
    fs::write(
        scripts.join("healthy.lua"),
        "function on_update(id, dt)\n\
           local t = transform.get(id)\n\
           transform.set(id, t.x + 10 * dt, t.y)\n\
         end\n",
    )
    .unwrap();

    let mut scene = rustlua_scene::scene::Scene::new("entity-guard");
    let broken = rustlua_scene::scene::SceneEntityId(1);
    let healthy = rustlua_scene::scene::SceneEntityId(2);
    for id in [broken, healthy] {
        scene.add_entity(id, "Walker", None).unwrap();
        scene.entities.get_mut(&id).unwrap().components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    }
    scene
        .entities
        .get_mut(&broken)
        .unwrap()
        .components
        .insert("Script".into(), "file=scripts/broken.lua".into());
    scene
        .entities
        .get_mut(&healthy)
        .unwrap()
        .components
        .insert("Script".into(), "file=scripts/healthy.lua".into());

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.set_project_root(root.clone());
    runtime.instantiate_scene(&scene).unwrap();

    // 严格档（默认）：首个报错的实体即上报，是否继续由调用方决定。
    assert!(
        runtime.call_entity_scripts(0.5).is_err(),
        "严格档必须上报实体钩子错误"
    );

    // 容错档：错误记账，循环继续——**后面的实体照样被调用**。
    runtime.set_error_policy(ScriptErrorPolicy::Contain);
    let called = runtime
        .call_entity_scripts(0.5)
        .expect("容错档下实体钩子错误不得上报");
    assert_eq!(called, 2, "两个实体都应被调用（含报错的那个）");
    assert_eq!(runtime.script_error_total(), 1, "错误必须被记账");

    // 观察点是"另一个实体的位置真的动了"，而不是"函数被调了"。
    let healthy_id = *runtime
        .state
        .borrow()
        .entity_state
        .scene_entities
        .get(&healthy)
        .expect("健康实体的运行时实体");
    {
        let state = runtime.state.borrow();
        let transform = state
            .world
            .get::<Transform>(healthy_id)
            .unwrap()
            .expect("健康实体有 Transform");
        assert!(
            (transform.position.x - 5.0).abs() < 1e-4,
            "报错实体之后的实体仍应被推进（10*dt，dt=0.5）：实际 {}",
            transform.position.x
        );
    }

    let _ = fs::remove_dir_all(root);
}

/// docs/54 §4.1：场景声明的**每实体脚本**在加载期读入、`on_spawn` 一次、每步 `on_update`
/// 一次，并且能改自己那个实体。判据要的是"脚本归属真的下放到实体"，所以断言的是**实体位置**
/// 而不是"脚本被调用了"。
#[test]
fn entity_script_component_drives_its_own_entity() {
    let root = std::env::temp_dir().join(format!("rustlua-entity-script-{}", std::process::id()));
    let scripts = root.join("scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(
        scripts.join("patrol.lua"),
        "spawned = {}\n\
         function on_spawn(id) spawned[id] = true end\n\
         function on_update(id, dt)\n\
           local t = transform.get(id)\n\
           transform.set(id, t.x + 10 * dt, t.y)\n\
           speed_seen = dt\n\
         end\n",
    )
    .unwrap();

    let mut scene = rustlua_scene::scene::Scene::new("entity-script");
    let walker = rustlua_scene::scene::SceneEntityId(1);
    let idle = rustlua_scene::scene::SceneEntityId(2);
    scene.add_entity(walker, "Walker", None).unwrap();
    scene.add_entity(idle, "Idle", None).unwrap();
    for id in [walker, idle] {
        scene.entities.get_mut(&id).unwrap().components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    }
    scene
        .entities
        .get_mut(&walker)
        .unwrap()
        .components
        .insert("Script".into(), "file=scripts/patrol.lua".into());
    // 停用的脚本组件：既不加载也不调用。
    scene.entities.get_mut(&idle).unwrap().components.insert(
        "Script".into(),
        "file=scripts/missing.lua;enabled=false".into(),
    );

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.set_project_root(root.clone());
    assert_eq!(
        runtime.instantiate_scene(&scene).unwrap(),
        2,
        "两个实体都实例化了"
    );
    assert_eq!(
        runtime.entity_script_files(),
        vec!["scripts/patrol.lua".to_owned()],
        "只应加载启用的那个脚本；enabled=false 的不许被读（否则文件不存在就会误报）"
    );

    // 每次 `call_entity_scripts` 只驱动声明了脚本的那个实体。
    let called = runtime.call_entity_scripts(0.5).unwrap();
    assert_eq!(called, 1, "只有 Walker 有启用脚本");
    let walker_id = *runtime
        .state
        .borrow()
        .entity_state
        .scene_entities
        .get(&walker)
        .expect("Walker 的运行时实体");
    let idle_id = *runtime
        .state
        .borrow()
        .entity_state
        .scene_entities
        .get(&idle)
        .expect("Idle 的运行时实体");
    runtime.call_entity_scripts(0.5).unwrap();
    {
        let state = runtime.state.borrow();
        let walker_transform = state
            .world
            .get::<Transform>(walker_id)
            .unwrap()
            .expect("Walker 有 Transform");
        assert!(
            (walker_transform.position.x - 10.0).abs() < 1e-4,
            "脚本应把实体移动 10*dt*2 次：实际 {}",
            walker_transform.position.x
        );
        let idle_transform = state
            .world
            .get::<Transform>(idle_id)
            .unwrap()
            .expect("Idle 有 Transform");
        assert_eq!(idle_transform.position.x, 0.0, "停用的脚本不该动实体");
    }

    // 热重载：文件没变 → 0；改过 → 1，且重载后仍能继续驱动。
    assert_eq!(
        runtime.reload_entity_scripts().unwrap(),
        0,
        "未改动不该重载"
    );
    fs::write(
        scripts.join("patrol.lua"),
        "function on_update(id, dt)\n\
           local t = transform.get(id)\n\
           transform.set(id, t.x + 100 * dt, t.y)\n\
         end\n",
    )
    .unwrap();
    assert_eq!(
        runtime.reload_entity_scripts().unwrap(),
        1,
        "改动后应重载一次"
    );
    runtime.call_entity_scripts(0.5).unwrap();
    {
        let state = runtime.state.borrow();
        let walker_transform = state
            .world
            .get::<Transform>(walker_id)
            .unwrap()
            .expect("Walker 有 Transform");
        assert!(
            (walker_transform.position.x - 60.0).abs() < 1e-4,
            "重载后应走新逻辑（+100*0.5）：实际 {}",
            walker_transform.position.x
        );
    }
    let _ = fs::remove_dir_all(&root);
}

/// 脚本文件不存在必须**在加载期报错**（而不是悄悄少跑一个脚本）：这是"场景声明了脚本"
/// 与"脚本真的在跑"之间唯一的防线。
#[test]
fn entity_script_reports_missing_files_and_missing_project_root() {
    let root =
        std::env::temp_dir().join(format!("rustlua-entity-script-miss-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut scene = rustlua_scene::scene::Scene::new("entity-script-missing");
    let id = rustlua_scene::scene::SceneEntityId(1);
    scene.add_entity(id, "Walker", None).unwrap();
    scene
        .entities
        .get_mut(&id)
        .unwrap()
        .components
        .insert("Script".into(), "file=scripts/nope.lua".into());

    // 没有工程根：明确报错，而不是"脚本没跑"。
    let mut bare = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let error = bare
        .instantiate_scene(&scene)
        .expect_err("没有工程根时应报错");
    assert!(
        error.to_string().contains("project root"),
        "错误信息应指向工程根：{error}"
    );

    // 有工程根但文件不存在：同样是加载期错误。
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.set_project_root(root.clone());
    let error = runtime
        .instantiate_scene(&scene)
        .expect_err("文件不存在时应报错");
    assert!(
        error.to_string().contains("nope.lua"),
        "错误信息应点名缺失的脚本：{error}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn project_script_can_require_sibling_lua_modules() {
    let root = std::env::temp_dir().join(format!("rustlua-module-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("helper.lua"),
        "local M = {}\nfunction M.answer() return 42 end\nreturn M\n",
    )
    .unwrap();
    let main = root.join("main.lua");
    fs::write(
        &main,
        "local h = require('helper')\nassert(h.answer() == 42)\nfunction update() return 'ok' end\n",
    )
    .unwrap();

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.execute_project_script(&main).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn encrypted_project_and_modules_load_via_decode_and_require() {
    let root = std::env::temp_dir().join(format!("rustlua-enc-module-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    // 用发布包格式（压缩+加密）写模块与主脚本。
    let helper_src = "local M = {}\nfunction M.answer() return 7 end\nreturn M\n".as_bytes();
    fs::write(
        root.join("helper.lua"),
        rustlua_infra::pack::encode(helper_src, true, true),
    )
    .unwrap();
    let main = root.join("main.lua");
    let main_src = "local h = require('helper')\nassert(h.answer() == 7)\nfunction update() return 'enc-ok' end\n";
    fs::write(
        &main,
        rustlua_infra::pack::encode(main_src.as_bytes(), true, true),
    )
    .unwrap();

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.execute_project_script(&main).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "enc-ok");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn hot_reload_detects_changes_and_keeps_old_vm_on_failure() {
    let path = script("function update() return 'v1' end");
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "v1");
    assert_eq!(
        runtime.reload_if_changed().unwrap(),
        ReloadStatus::Unchanged
    );
    fs::write(&path, "function update() return 'v2' end").unwrap();
    assert_eq!(runtime.reload_if_changed().unwrap(), ReloadStatus::Reloaded);
    assert_eq!(runtime.call_update().unwrap(), "v2");
    fs::write(&path, "function update(").unwrap();
    assert!(runtime.reload_if_changed().is_err());
    assert_eq!(runtime.call_update().unwrap(), "v2");
    let _ = fs::remove_file(path);
}
#[test]
fn hot_reload_calls_lifecycle_callbacks() {
    let path = script(
        "function on_create() end; function on_destroy() end; function update() return 'v1' end",
    );
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.execute_project_script(&path).unwrap();
    fs::write(
        &path,
        "function on_create() end; function on_destroy() end; function update() return 'v2' end",
    )
    .unwrap();
    runtime.reload_if_changed().unwrap();
    assert_eq!(runtime.call_update().unwrap(), "v2");
    let _ = fs::remove_file(path);
}
