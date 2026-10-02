use super::*;

use rustlua_input::input::Binding;

/// M22/W3：「已按住但没有按下边沿」不补步——同一根摇杆同时驱动 `move_*` 与
/// `ui_*` 时（玩家把移动改绑到摇杆），消费了边沿的那一帧走一步、下一帧的
/// `ui_*` 路径**不得**再补一步（只会按连发延迟继续）。
#[test]
fn action_repeat_does_not_double_step_without_edge() {
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_nav_settings(NavSettings::new(0.5, 50, 50));
    runtime.set_delta(1.0 / 60.0);

    // 边沿已在别处被消费：连续三帧「按住但无边沿」都不得走步。
    for frame in 1..=3 {
        runtime.set_input(held_stick_up(0.9));
        assert!(
            !runtime.action_repeat("ui_up"),
            "第 {frame} 帧无边沿不得补步"
        );
    }
    // 第 4 帧（约 50ms 后）到连发延迟 → 正常连发一步。
    runtime.set_input(held_stick_up(0.9));
    assert!(runtime.action_repeat("ui_up"), "到延迟后应正常连发");

    // 边沿帧才是「立即一步」（设备层在越过阈值的那一帧调 `gamepad_axis_event`，
    // 它同时写数字态与边沿）。
    let mut edge = InputSnapshot::default();
    edge.set_gamepad_axis(1, 0.9);
    edge.gamepad_axis_event(1, rustlua_input::input::AxisDirection::Positive, true);
    let mut fresh = LuaRuntime::new().unwrap();
    fresh.set_nav_settings(NavSettings::new(0.5, 50, 50));
    fresh.set_delta(1.0 / 60.0);
    fresh.set_input(edge);
    assert!(fresh.action_repeat("ui_up"), "有边沿立即走一步");
}

/// M22/W3：数字量绑定（键盘/DPad）按住**不连发**——保持 M20-08 的
/// 「一次按下 = 一步」菜单语义。
#[test]
fn action_repeat_does_not_repeat_digital_holds() {
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_nav_settings(NavSettings::new(0.5, 50, 50));
    runtime.set_delta(1.0 / 60.0);
    let mut fires = 0;
    for _ in 1..=120 {
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::ArrowUp, true);
        runtime.set_input(input);
        if runtime.action_repeat("ui_up") {
            fires += 1;
        }
    }
    assert_eq!(fires, 1, "键盘按住只应走一步");
}

/// M22/W4：**一步之内完成的「按下 + 松开」也必须走一步**。
///
/// 边沿就是「刚按下」，与「此刻是否仍按住」无关。编辑器 Play 预览把同一帧内的
/// 按下/松开发成 down + up 边沿（`game_view::bridge_preview_input`），若这里要求
/// `down` 才认边沿，那一下就会被整帧丢掉——用户实测的「编辑器里按上下键切换按钮
/// 有时没反应、发布 game.exe 正常」正是这条路径（game.exe 用事件流，按下与松开
/// 通常落在不同逻辑步）。
#[test]
fn action_repeat_honors_a_tap_released_within_one_step() {
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_nav_settings(NavSettings::new(0.5, 50, 50));
    runtime.set_delta(1.0 / 60.0);

    // 一步内：按下 + 松开 → 快照里 `held` 为空、`pressed` 有该键。
    let mut tap = InputSnapshot::default();
    tap.key_event(KeyCode::ArrowUp, true);
    tap.key_event(KeyCode::ArrowUp, false);
    assert!(!tap.is_down(KeyCode::ArrowUp), "帧末应已是松开态");
    assert!(tap.just_pressed(KeyCode::ArrowUp));
    runtime.set_input(tap);
    assert!(
        runtime.action_repeat("ui_up"),
        "有按下边沿就必须走一步（哪怕同一步内已经松开）"
    );

    // 松开帧（无边沿、未按住）不得再走一步，也不得残留状态。
    runtime.set_input(InputSnapshot::default());
    assert!(
        !runtime.action_repeat("ui_up"),
        "无边沿的一步不得补步（一次按下 = 一步）"
    );
}
#[test]
fn wasd_input_axis_moves_the_instantiated_player() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

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

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.instantiate_scene(&scene).unwrap();
    let path = script(
        "local speed = 10\nfunction update()\nlocal p = entity.find('Player')\nlocal h = input.axis('horizontal')\nlocal v = input.axis('vertical')\ntransform.move(p, h * speed, v * speed)\nreturn 'moved' end",
    );
    runtime.execute_project_script(&path).unwrap();

    let player = runtime.with_world(|world| world.entities_with::<Transform>());
    assert_eq!(player.len(), 1);
    let player = player[0];

    // Hold D -> horizontal axis = +1 -> player moves right.
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyD, true);
    runtime.set_input(input);
    runtime.call_update().unwrap();
    let position =
        runtime.with_world(|world| world.get::<Transform>(player).unwrap().unwrap().position);
    assert_eq!(position.x, 10.0);
    assert_eq!(position.y, 0.0);

    // Release D, hold W -> vertical axis = +1 -> player moves up only.
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyW, true);
    runtime.set_input(input);
    runtime.call_update().unwrap();
    let position =
        runtime.with_world(|world| world.get::<Transform>(player).unwrap().unwrap().position);
    assert_eq!(position.x, 10.0);
    assert_eq!(position.y, 10.0);

    let _ = fs::remove_file(path);
}

#[test]
fn input_action_map_defaults_are_available_to_lua() {
    let mut runtime = LuaRuntime::new().unwrap();
    let raw = runtime
        .execute_console(
            "assert(#input.actions() >= 15, 'default actions must be registered')\n\
             local text = input.action_map_text()\n\
             assert(text:find('rustlua-actions-v2', 1, true) == 1)\n\
             assert(text:find('interact=key:keye', 1, true) ~= nil)\n\
             local list = input.bindings('confirm')\n\
             assert(#list == 2 and list[1] == 'key:space' and list[2] == 'pad:0')\n\
             assert(#input.bindings('missing_action') == 0)\n\
             return tostring(#input.actions())",
        )
        .unwrap();
    assert!(raw.parse::<usize>().unwrap_or_default() >= 15, "{raw}");
    // 宿主读到的映射与脚本同源（同一份实例）。
    assert_eq!(
        runtime.action_map().bindings("interact"),
        [Binding::Key(KeyCode::KeyE)]
    );
    assert!(runtime.input_diagnostics().is_empty());
}

/// M20-07：键盘键位与抽象手柄按钮触发同一动作，结论一致；
/// 摇杆给模拟量、键位给 0/1；原始手柄查询（`pad_*`）与动作层一致。
#[test]
fn input_action_api_matches_keyboard_and_gamepad_bindings() {
    let mut runtime = LuaRuntime::new().unwrap();
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyE, true);
    runtime.set_input(input);
    runtime
        .execute_console(
            "assert(input.action('interact') == true)\n\
             assert(input.action_pressed('interact') == true)\n\
             assert(input.action_released('interact') == false)\n\
             assert(input.action('move_up') == false)\n\
             assert(input.action_axis('move_left', 'move_right') == 0)\n\
             assert(input.pad_button(0) == false)\n\
             assert(input.pad_axis(1) == 0)\n\
             return 'ok'",
        )
        .unwrap();
    // 同一动作改由手柄 A 键（confirm）触发：语义与键盘一致。
    let mut input = InputSnapshot::default();
    input.gamepad_button_event(0, true);
    runtime.set_input(input);
    let raw = runtime
        .execute_console(
            "assert(input.action('confirm') == true)\n\
             assert(input.action_pressed('confirm') == true)\n\
             assert(input.action_released('confirm') == false)\n\
             assert(input.pad_button(0) == true)\n\
             assert(input.pad_button(1) == false)\n\
             assert(input.axis('horizontal') == 0, 'physical axis unaffected by gamepad')\n\
             assert(input.action_axis('move_left', 'move_right') == 0)\n\
             return tostring(input.action('confirm'))",
        )
        .unwrap();
    assert_eq!(raw, "true");
    // 手柄轴：动作过 0.5 阈值，模拟量按方向符号取正。
    let mut input = InputSnapshot::default();
    input.set_gamepad_axis(1, 0.8);
    runtime.set_input(input);
    runtime
        .execute_console(
            "assert(input.action('ui_up') == true, 'stick up drives ui_up')\n\
             assert(input.action('ui_down') == false)\n\
             assert(input.action('confirm') == false)\n\
             assert(math.abs(input.pad_axis(1) - 0.8) < 1e-6)\n\
             assert(math.abs(input.action_axis('ui_down', 'ui_up') - 0.8) < 1e-6, 'up is positive')\n\
             assert(math.abs(input.action_axis('stick_up', 'stick_down')) < 1e-6)\n\
             return 'ok'",
        )
        .unwrap();
    // 自定义摇杆动作（负向在前、正向在后）→ 模拟量而非 0/1。
    runtime
        .execute_console(
            "assert(select(1, input.rebind('stick_down', 'padaxis:1-')) == true)\n\
             assert(select(1, input.rebind('stick_up', 'padaxis:1+')) == true)\n\
             assert(math.abs(input.action_axis('stick_down', 'stick_up') - 0.8) < 1e-6)\n\
             return 'ok'",
        )
        .unwrap();
    // 键位绑定：轴是 0/1（而非手柄模拟量）。
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyD, true);
    runtime.set_input(input);
    runtime
        .execute_console(
            "assert(input.action('move_right') == true)\n\
             assert(input.action_axis('move_left', 'move_right') == 1)\n\
             return 'ok'",
        )
        .unwrap();
    // 边沿在本逻辑帧内保留，`finish_frame` 后清除（由宿主每帧调用）。
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyE, true);
    input.finish_frame();
    runtime.set_input(input);
    runtime
        .execute_console(
            "assert(input.action('interact') == true)\n\
             assert(input.action_pressed('interact') == false)\n\
             return 'ok'",
        )
        .unwrap();
}

/// M20-07：重绑定 / 解绑 / 重置 / 映射文本往返，含明确错误原因。
#[test]
fn input_action_map_rebind_unbind_and_text_round_trip() {
    let mut runtime = LuaRuntime::new().unwrap();
    runtime
        .execute_console(
            "assert(input.rebind('interact', 'key:f') == true)\n\
             local list = input.bindings('interact')\n\
             assert(#list == 1 and list[1] == 'key:keyf', 'v1 写法键位读回必须是 v2 物理键名')\n\
             assert(input.rebind('interact', 'key:keye') == true)\n\
             assert(input.bindings('interact')[1] == 'key:keye', 'v2 物理键名也可直接重绑定')\n\
             assert(input.rebind('interact', 'key:f') == true)\n\
             return 'ok'",
        )
        .unwrap();
    // 旧绑定失效、新绑定生效。
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyE, true);
    runtime.set_input(input);
    runtime
        .execute_console("assert(input.action('interact') == false); return 'ok'")
        .unwrap();
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyF, true);
    runtime.set_input(input);
    runtime
        .execute_console(
            "assert(input.action('interact') == true)\n\
             assert(input.action_pressed('interact') == true)\n\
             return 'ok'",
        )
        .unwrap();
    // 物理键 API 不受重绑定影响（证明只加了动作层）。M22/W1：键名同时接受
    // v1 可读名（`f`/`e`）与 v2 物理键名（`keyf`/`keye`），解析到同一物理键。
    runtime
        .execute_console(
            "assert(input.is_down('f') == true)\n\
             assert(input.is_down('e') == false)\n\
             assert(input.is_down('keyf') == true, 'v2 物理键名与 v1 可读名同源')\n\
             assert(input.pressed('f') == true)\n\
             return 'ok'",
        )
        .unwrap();
    // 未知名报错（旧实现静默落 Other(0)，拼错与真按键无法区分）。
    let error = runtime
        .execute_console("return input.is_down('not-a-key')")
        .expect_err("未知名必须报错");
    assert!(
        format!("{error:?}").contains("not-a-key"),
        "错误信息要带上原值：{error:?}"
    );
    // 非法绑定 / 重复绑定 → false + 明确原因，不 panic，也不破坏当前绑定。
    let raw = runtime
        .execute_console(
            "local ok, why = input.rebind('interact', 'joystick:1')\n\
             assert(ok == false and why:find('joystick:1') ~= nil, tostring(why))\n\
             assert(input.bindings('interact')[1] == 'key:keyf', 'invalid rebind must not clear bindings')\n\
             local dup_ok, dup_why = input.rebind('interact', 'key:f')\n\
             assert(dup_ok == true, 'rebind is an overwrite, not a duplicate add')\n\
             local bad = input.rebind('interact', 'not-a-binding')\n\
             assert(bad == false)\n\
             return tostring(ok) .. '|' .. tostring(dup_ok) .. '|' .. tostring(bad)",
        )
        .unwrap();
    assert_eq!(raw, "false|true|false");
    // 解绑 → 动作不再命中；重置 → 恢复默认绑定。
    runtime
        .execute_console(
            "assert(input.unbind('interact', 'key:f') == true)\n\
             assert(input.unbind('interact', 'key:f') == false)\n\
             assert(#input.bindings('interact') == 0)\n\
             assert(input.unbind('interact', 'key:e') == false)\n\
             input.reset_bindings()\n\
             local list = input.bindings('interact')\n\
             assert(#list == 1 and list[1] == 'key:keye')\n\
             return 'ok'",
        )
        .unwrap();
    // 映射文本往返：导出 → 改绑 → 导入 → 与导出时一致；版本头不符 → false + 原因。
    let raw = runtime
        .execute_console(
            "local text = input.action_map_text()\n\
             input.rebind('interact', 'key:g')\n\
             assert(input.bindings('interact')[1] == 'key:keyg')\n\
             assert(select(1, input.set_action_map(text)) == true)\n\
             assert(input.bindings('interact')[1] == 'key:keye')\n\
             local ok, why = input.set_action_map('rustlua-actions-v0\\ninteract=key:e')\n\
             assert(ok == false and why:find('rustlua-actions-v0', 1, true) ~= nil, tostring(why))\n\
             assert(input.bindings('interact')[1] == 'key:keye', 'bad version must not apply')\n\
             return tostring(ok)",
        )
        .unwrap();
    assert_eq!(raw, "false");
    // 宿主注入的映射对脚本立即可见（单一事实来源）。
    let mut map = ActionMap::with_defaults();
    map.clear("interact");
    map.bind(
        "interact",
        Binding::Key(KeyCode::from_char('z').expect("QWERTY 字符必须有对应物理键")),
    )
    .unwrap();
    runtime.set_action_map(map);
    runtime
        .execute_console(
            "assert(input.bindings('interact')[1] == 'key:keyz')\n\
             assert(input.is_down('z') == false)\n\
             return 'ok'",
        )
        .unwrap();
}

/// M20-07：`[input]` 段设置文件往返 —— 与内存映射相等、保留 `[audio]` 段与
/// 注释、版本头不符回退默认并产生可定位诊断、无工程根返回明确错误。
#[test]
fn input_action_settings_round_trip_and_version_fallback() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-lua-input-settings-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let settings_path = root.join("settings.toml");
    fs::write(
        &settings_path,
        "# 玩家设置\n[audio]\nmaster = 0.5\nbgm = 0.25\nsfx = 1.0\nui = 1.0\n",
    )
    .unwrap();
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    let path = runtime
        .execute_console("assert(input.settings_path() ~= nil); return input.settings_path()")
        .unwrap();
    assert!(path.ends_with("settings.toml"), "{path}");
    // 改绑 → 写盘 → 读盘：设置文件里的映射与内存映射相等。
    runtime
        .execute_console(
            "assert(input.rebind('interact', 'key:f') == true)\n\
             assert(select(1, input.save_settings()) == true)\n\
             input.reset_bindings()\n\
             assert(input.bindings('interact')[1] == 'key:keye')\n\
             assert(select(1, input.load_settings()) == true)\n\
             assert(input.bindings('interact')[1] == 'key:keyf', 'saved binding must survive round trip')\n\
             return 'ok'",
        )
        .unwrap();
    let text = fs::read_to_string(&settings_path).unwrap();
    // 写入 [input] 后 [audio] 段与注释仍在（保序读改写）。
    assert!(text.contains("# 玩家设置"), "{text}");
    assert!(text.contains("master = 0.5"), "{text}");
    assert!(text.contains("bgm = 0.25"), "{text}");
    assert!(text.contains("[input]"), "{text}");
    assert!(text.contains("[audio]"), "{text}");
    // M22/W1：写盘的是 v2 物理键文本。
    assert!(text.contains("rustlua-actions-v2"), "{text}");
    assert!(text.contains("interact=key:keyf"), "{text}");
    // 设置文件文本 == 当前内存映射的 to_text（宿主与脚本同源）。
    assert_eq!(runtime.action_map().to_text(), {
        let start = text.find("map = \"").expect("map line") + "map = \"".len();
        let rest = &text[start..];
        let end = rest.find('"').expect("closing quote");
        rest[..end].replace("\\n", "\n")
    });
    assert!(runtime.input_diagnostics().is_empty());
    // 旧物理键 API 仍然可用。
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyF, true);
    runtime.set_input(input);
    runtime
        .execute_console(
            "assert(input.action('interact') == true)\n\
             assert(input.is_down('f') == true)\n\
             return 'ok'",
        )
        .unwrap();
    // 版本头不符 → 回退默认 + 可定位诊断，不崩溃、不静默。
    fs::write(
        &settings_path,
        "[input]\nmap = \"rustlua-actions-v0\\ninteract=key:q\"\n",
    )
    .unwrap();
    let raw = runtime
        .execute_console(
            "local ok, why = input.load_settings()\n\
             assert(ok == false and why:find('rustlua-actions-v0', 1, true) ~= nil, tostring(why))\n\
             assert(input.bindings('interact')[1] == 'key:keye', 'fallback to defaults')\n\
             local list = input.diagnostics()\n\
             assert(#list >= 1 and list[#list]:find('reverting to defaults') ~= nil, tostring(#list))\n\
             return list[#list]",
        )
        .unwrap();
    assert!(raw.contains("settings.toml"), "{raw}");
    // 非法绑定文本 → 同样回退默认并给可定位原因。
    runtime
        .execute_console(
            "assert(input.rebind('interact', 'key:r') == true)\n\
             return 'ok'",
        )
        .unwrap();
    fs::write(
        &settings_path,
        "[input]\nmap = \"rustlua-actions-v1\\ninteract=joystick:1\"\n",
    )
    .unwrap();
    let raw = runtime
        .execute_console(
            "local ok, why = input.load_settings()\n\
             assert(ok == false and why:find('joystick:1') ~= nil, tostring(why))\n\
             assert(input.bindings('interact')[1] == 'key:keye')\n\
             return why",
        )
        .unwrap();
    assert!(raw.contains("joystick:1"), "{raw}");
    // 缺 [input] 段（或文件缺失）→ 保留当前绑定 + 诊断，不清空用户改键。
    fs::write(&settings_path, "[audio]\nmaster = 1.0\n").unwrap();
    runtime
        .execute_console(
            "assert(input.rebind('interact', 'key:v') == true)\n\
             local ok, why = input.load_settings()\n\
             assert(ok == false and why:find('keeping current bindings') ~= nil, tostring(why))\n\
             assert(input.bindings('interact')[1] == 'key:keyv', 'missing [input] must not wipe bindings')\n\
             return 'ok'",
        )
        .unwrap();
    let _ = fs::remove_file(&settings_path);
    let raw = runtime
        .execute_console(
            "local ok, why = input.load_settings()\n\
             assert(ok == false and why == 'missing', tostring(why))\n\
             assert(input.bindings('interact')[1] == 'key:keyv')\n\
             return why",
        )
        .unwrap();
    assert_eq!(raw, "missing");
    // 显式绝对路径：无工程根也能读（无头/测试场景）。
    let alt = root.join("alt.toml");
    fs::write(
        &alt,
        "[input]\nmap = \"rustlua-actions-v1\\ninteract=key:m\"\n",
    )
    .unwrap();
    runtime
        .execute_console(&format!(
            "assert(select(1, input.load_settings([==[{}]==])) == true)\n\
             assert(input.bindings('interact')[1] == 'key:keym')\n\
             return 'ok'",
            alt.display()
        ))
        .unwrap();
    // 无工程根且未给路径 → 明确错误（不 panic）。
    let mut rootless = LuaRuntime::new().unwrap();
    assert_eq!(
        rootless
            .execute_console("return select(2, input.save_settings())")
            .unwrap(),
        "no_project_root"
    );
    assert_eq!(
        rootless
            .execute_console("return select(2, input.load_settings())")
            .unwrap(),
        "no_project_root"
    );
    // 绑定不随场景切换/读档回滚：切场景只清 ECS/UI，保留玩家改键。
    let scene_dir = root.join("scenes");
    fs::create_dir_all(&scene_dir).unwrap();
    let mut scene = rustlua_scene::scene::Scene::new("town");
    scene
        .add_entity(rustlua_scene::scene::SceneEntityId(1), "Player", None)
        .unwrap();
    scene.save(scene_dir.join("town.scene")).unwrap();
    runtime.set_action_map({
        let mut map = ActionMap::with_defaults();
        map.clear("interact");
        map.bind(
            "interact",
            Binding::Key(KeyCode::from_char('y').expect("QWERTY 字符必须有对应物理键")),
        )
        .unwrap();
        map
    });
    runtime
        .execute_console("assert(select(1, scene.load('town')) == true); return 'ok'")
        .unwrap();
    runtime.process_scene_transition().unwrap();
    runtime
        .execute_console(
            "assert(input.bindings('interact')[1] == 'key:keyy', 'scene switch keeps user bindings')\n\
             return 'ok'",
        )
        .unwrap();
    let _ = fs::remove_dir_all(&root);
}

/// P3：玩家设置根与工程根分离（发布包）。`settings_root` 指向用户数据目录后，
/// `input.*` 与 `audio.*` 的 `settings_path/load_settings/save_settings` 全部落到
/// 用户目录，**一个字节都不写发布包**；不设置时退回工程根（开发工程形态）。
#[test]
fn settings_root_redirects_player_settings_out_of_the_release_package() {
    let base = std::env::temp_dir().join(format!(
        "rustlua-settings-root-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let pack = base.join("pack");
    let user = base.join("user");
    fs::create_dir_all(&pack).unwrap();
    fs::create_dir_all(&user).unwrap();
    // 发布包自带一份默认设置（首次运行由宿主播种到用户目录）。
    fs::write(
        pack.join("settings.toml"),
        "[audio]\nmaster = 0.5\nbgm = 0.25\nsfx = 1.0\nui = 1.0\n",
    )
    .unwrap();

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(pack.clone());
    // 未设置设置根 → 仍是开发工程形态：读写工程内的 settings.toml。
    assert_eq!(
        runtime
            .execute_console("return input.settings_path()")
            .unwrap(),
        pack.join("settings.toml").to_string_lossy()
    );
    // 指向用户目录（发布包形态）。
    runtime.set_settings_root(user.clone());
    assert_eq!(
        runtime
            .execute_console("return input.settings_path()")
            .unwrap(),
        user.join("settings.toml").to_string_lossy()
    );
    assert_eq!(
        runtime
            .execute_console("return audio.settings_path()")
            .unwrap(),
        user.join("settings.toml").to_string_lossy()
    );
    // 改键 + 音量落盘 → 都写进用户目录，发布包目录保持原样。
    runtime
        .execute_console(
            "assert(input.rebind('interact', 'key:f') == true)\n\
             assert(select(1, input.save_settings()) == true)\n\
             assert(select(1, audio.save_settings()) == true)\n\
             return 'ok'",
        )
        .unwrap();
    let user_text = fs::read_to_string(user.join("settings.toml")).unwrap();
    assert!(user_text.contains("interact=key:keyf"), "{user_text}");
    assert!(user_text.contains("[audio]"), "{user_text}");
    assert_eq!(
        fs::read_to_string(pack.join("settings.toml")).unwrap(),
        "[audio]\nmaster = 0.5\nbgm = 0.25\nsfx = 1.0\nui = 1.0\n",
        "发布包不得被写入玩家设置"
    );
    // 相对路径参数也相对设置根解析（不是工程根）。
    runtime
        .execute_console(
            "assert(select(1, input.save_settings('player/custom.toml')) == true)\n\
             return 'ok'",
        )
        .unwrap();
    assert!(user.join("player").join("custom.toml").is_file());
    assert!(!pack.join("player").exists(), "不得在发布包里建目录");
    let _ = fs::remove_dir_all(&base);
}

/// M20-07：动作层可驱动面板导航（`ui.focus_*`）——默认绑定（方向键 + 左摇杆）
/// 直接可用，面板不需要自己解析物理键。
#[test]
fn input_actions_drive_ui_focus_navigation() {
    let mut runtime = LuaRuntime::new().unwrap();
    // 左摇杆上推 → `ui_up` 命中，合成轴为 +0.9。
    let mut input = InputSnapshot::default();
    input.set_gamepad_axis(1, 0.9);
    runtime.set_input(input);
    runtime
        .execute_console(
            "assert(input.action('ui_up') == true)\n\
             assert(input.action('ui_down') == false)\n\
             assert(math.abs(input.action_axis('ui_down', 'ui_up') - 0.9) < 1e-6)\n\
             return 'ok'",
        )
        .unwrap();
    // 面板把（过阈值的）动作 + 模拟量接到 M20-02 的焦点 API。
    runtime
        .execute_console(
            "ui.focus_open('bag', 3)\n\
             local step = 0\n\
             if math.abs(input.action_axis('ui_down', 'ui_up')) >= 0.5 then\n\
               step = input.action_axis('ui_down', 'ui_up') > 0 and 1 or -1\n\
             end\n\
             assert(step == 1)\n\
             assert(ui.focus_move('bag', 'down') == 0)\n\
             assert(ui.focus_move('bag', 'down') == 1)\n\
             assert(ui.focus_index('bag') == 1)\n\
             return 'ok'",
        )
        .unwrap();
    // 手柄 A/B → `confirm` / `cancel`：焦点环上确认与取消都成立。
    let mut input = InputSnapshot::default();
    input.gamepad_button_event(0, true);
    runtime.set_input(input);
    runtime
        .execute_console(
            "assert(input.action('confirm') == true)\n\
             local confirmed = false\n\
             if input.action_pressed('confirm') then confirmed = ui.focus_select('bag', 2) end\n\
             assert(confirmed == true and ui.focus_index('bag') == 2)\n\
             return 'ok'",
        )
        .unwrap();
    let mut input = InputSnapshot::default();
    input.gamepad_button_event(1, true);
    runtime.set_input(input);
    runtime
        .execute_console(
            "assert(input.action('cancel') == true)\n\
             local closed = false\n\
             if input.action('cancel') then closed = ui.focus_close('bag') end\n\
             assert(closed == true and ui.focus_count('bag') == 0)\n\
             return 'ok'",
        )
        .unwrap();
    // 方向键走同一动作层：↓ 键 → `ui_down` → 焦点下移（键盘与手柄同源）。
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::ArrowDown, true);
    runtime.set_input(input);
    runtime
        .execute_console(
            "assert(input.is_down('down') == true, 'physical arrow key works')\n\
             assert(input.action('ui_down') == true, 'arrows drive the same actions as the stick')\n\
             ui.focus_open('arrows', 3)\n\
             local first, second = nil, nil\n\
             if input.action('ui_down') then first = ui.focus_move('arrows', 'down') end\n\
             if input.action('ui_down') then second = ui.focus_move('arrows', 'down') end\n\
             assert(first == 0 and second == 1, 'arrow key drives focus through the action layer')\n\
             assert(ui.focus_index('arrows') == 1)\n\
             return 'ok'",
        )
        .unwrap();
}

/// M20-02 + M22/W1：方向键是物理键（宿主喂入），`parse_key_strict` 同时认
/// v1 可读名（`up`/`down`/`left`/`right`）与 v2 物理键名（`arrowup`…）。
#[test]
fn arrow_keys_are_physical_keys_distinct_from_letters() {
    assert_eq!(parse_key_strict("up").unwrap(), KeyCode::ArrowUp);
    assert_eq!(parse_key_strict("DOWN").unwrap(), KeyCode::ArrowDown);
    assert_eq!(parse_key_strict("Left").unwrap(), KeyCode::ArrowLeft);
    assert_eq!(parse_key_strict("right").unwrap(), KeyCode::ArrowRight);
    // v2 物理键名与 v1 可读名必须解析到同一物理键。
    assert_eq!(parse_key_strict("arrowup").unwrap(), KeyCode::ArrowUp);
    assert_eq!(parse_key_strict("key:arrowup").unwrap(), KeyCode::ArrowUp);
    assert_ne!(
        parse_key_strict("up").unwrap(),
        parse_key_strict("w").unwrap()
    );
    // 未知名报错（旧实现静默落 Other(0)，拼错与真按键无法区分）。
    let error = parse_key_strict("not-a-key").unwrap_err().to_string();
    assert!(error.contains("not-a-key"), "{error}");
    assert!(parse_key_strict("").is_err());
    let mut runtime = LuaRuntime::new().unwrap();
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::ArrowDown, true);
    runtime.set_input(input);
    assert_eq!(
        runtime
            .execute_console(
                "return tostring(input.just_pressed('down')) .. '|' .. tostring(input.is_down('w'))"
            )
            .unwrap(),
        "true|false"
    );
}

#[test]
fn virtual_key_reaches_action_axis_within_and_across_frames() {
    let mut runtime = LuaRuntime::new().unwrap();
    // 同一逻辑帧内注入 → 立即可读（触屏摇杆 `update()` 之后同帧的 `update_movement`）。
    let mut input = InputSnapshot::default();
    runtime.set_input(input.clone());
    runtime
        .execute_console(
            "input.virtual_key('keyw', true)\n\
             return input.action_axis('move_down', 'move_up')",
        )
        .unwrap();
    let same_frame = runtime
        .execute_console("return input.action_axis('move_down', 'move_up')")
        .unwrap();
    // 下一帧宿主重新喂入快照（物理状态重建）→ 虚拟键必须仍然按住。
    input.finish_frame();
    runtime.set_input(input);
    let next_frame = runtime
        .execute_console(
            "return input.action_axis('move_down', 'move_up') .. '|' .. tostring(input.action('move_up'))",
        )
        .unwrap();
    runtime
        .execute_console("input.virtual_key('keyw', false)")
        .unwrap();
    let released = runtime
        .execute_console("return input.action_axis('move_down', 'move_up')")
        .unwrap();
    assert_eq!(same_frame, "1", "同帧注入必须立刻可读");
    assert_eq!(next_frame, "1.0|true", "跨帧必须保持按住，直到显式释放");
    assert_eq!(released, "0", "显式释放后归零");
}
