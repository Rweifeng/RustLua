use super::*;

/// M20-02：`audio` 表的总线 API——分组独立、主 × 分组、非法总线名报错。
#[test]
fn audio_bus_lua_api_groups_volumes_and_rejects_unknown_bus() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime
        .execute_console(
            "assert(#audio.buses() == 4)\n\
             assert(audio.buses()[1] == 'master' and audio.buses()[4] == 'ui')\n\
             audio.set_bus_volume('master', 0.5)\n\
             audio.set_bus_volume('bgm', 0.4)\n\
             audio.set_bus_volume('sfx', 0.8)\n\
             audio.set_bus_volume('ui', 0.2)\n\
             assert(math.abs(audio.bus_volume('master') - 0.5) < 1e-6)\n\
             assert(math.abs(audio.bus_volume('bgm') - 0.4) < 1e-6)\n\
             assert(math.abs(audio.bus_volume('sfx') - 0.8) < 1e-6)\n\
             assert(math.abs(audio.bus_volume('ui') - 0.2) < 1e-6)\n\
             assert(math.abs(audio.effective_volume('bgm') - 0.2) < 1e-6)\n\
             assert(math.abs(audio.effective_volume('master') - 0.5) < 1e-6)\n\
             assert(math.abs(audio.effective_volume('sfx') - 0.4) < 1e-6)\n\
             do local ok = pcall(function() return audio.bus_volume('nope') end)\n\
                assert(not ok, 'unknown bus must error') end\n\
             do local ok = pcall(function() return audio.set_bus_volume('nope', 0.5) end)\n\
                assert(not ok, 'unknown bus must error') end\n\
             audio.set_bus_volume('bgm', 5.0)\n\
             assert(math.abs(audio.bus_volume('bgm') - 1.0) < 1e-6)\n\
             return 'ok'",
        )
        .unwrap();
}

/// M20-02：`audio.play_on`（总线播放）与 `settings_text`/`apply_settings` 往返。
#[test]
fn audio_lua_settings_text_round_trips_and_rejects_bad_version() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let text = runtime
        .execute_console(
            "audio.set_bus_volume('master', 0.6)\n\
             audio.set_bus_volume('bgm', 0.3)\n\
             audio.set_bus_volume('sfx', 0.9)\n\
             audio.set_bus_volume('ui', 0.6)\n\
             assert(select(1, audio.apply_settings('rustlua-audio-v0\\nbgm=0.9')) == false)\n\
             assert(math.abs(audio.bus_volume('bgm') - 0.3) < 1e-6)\n\
             return audio.settings_text()",
        )
        .unwrap();
    assert!(text.starts_with(rustlua_audio::audio::AUDIO_SETTINGS_VERSION));
    assert!(text.contains("bgm=0.300"), "{text}");

    // 文本经 Lua 长字符串吃进第二个 VM，四路音量按总线路由恢复。
    let mut restored =
        LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let expression = format!(
        "assert(select(1, audio.apply_settings([==[\n{text}]==])) == true)\n\
         assert(math.abs(audio.bus_volume('master') - 0.6) < 1e-6)\n\
         assert(math.abs(audio.bus_volume('bgm') - 0.3) < 1e-6)\n\
         assert(math.abs(audio.bus_volume('sfx') - 0.9) < 1e-6)\n\
         assert(math.abs(audio.bus_volume('ui') - 0.6) < 1e-6)\n\
         assert(math.abs(audio.effective_volume('bgm') - 0.18) < 1e-6)\n\
         return 'ok'"
    );
    assert_eq!(restored.execute_console(&expression).unwrap(), "ok");

    // 播放失败（无 asset_dir）返回 false + 原因，不抛错、不中断玩法。
    let (ok, why) = {
        let raw = restored
            .execute_console(
                "local ok, why = audio.play_on('ui', '__definitely_missing__')\n\
                 return tostring(ok) .. '|' .. tostring(why ~= nil and #why > 0)",
            )
            .unwrap();
        let mut parts = raw.split('|');
        (
            parts.next().unwrap_or_default().to_owned(),
            parts.next().unwrap_or_default().to_owned(),
        )
    };
    assert_eq!(ok, "false");
    assert_eq!(why, "true");
}

/// M20-02：工程设置文件 `<project>/project.toml`（[audio]/[input] 段）的读写入口。
#[test]
fn audio_lua_project_settings_load_and_save() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-lua-settings-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("project.toml"),
        "# 说明\n[audio]\nmaster = 0.5\nbgm = 0.25\nsfx = 0.75\nui = 0.5\n\n[input]\nmap = \"keep\"\n",
    )
    .unwrap();
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.set_project_root(root.clone());
    let path = runtime
        .execute_console("assert(audio.settings_path() ~= nil); return audio.settings_path()")
        .unwrap();
    assert!(path.ends_with("project.toml"), "{path}");
    runtime
        .execute_console(
            "assert(select(1, audio.load_settings()) == true)\n\
             assert(math.abs(audio.bus_volume('bgm') - 0.25) < 1e-6)\n\
             assert(math.abs(audio.effective_volume('bgm') - 0.125) < 1e-6)\n\
             audio.set_bus_volume('ui', 0.9)\n\
             assert(select(1, audio.save_settings()) == true)\n\
             return 'ok'",
        )
        .unwrap();
    let text = fs::read_to_string(root.join("project.toml")).unwrap();
    assert!(text.contains("# 说明"), "{text}");
    assert!(text.contains("map = \"keep\""), "{text}");
    assert!(text.contains("ui = 0.9"), "{text}");
    // 缺失文件 → 默认值 + 明确返回 false, "missing"，不报错。
    let empty = root.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let mut bare = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    bare.set_project_root(empty.clone());
    assert_eq!(
        bare.execute_console(
            "local ok, why = audio.load_settings(); return tostring(ok) .. '|' .. why"
        )
        .unwrap(),
        "false|missing"
    );
    // 无工程根：明确失败而不是 panic。
    let mut rootless =
        LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(
        rootless
            .execute_console("return select(2, audio.save_settings())")
            .unwrap(),
        "no_project_root"
    );
    let _ = fs::remove_dir_all(&root);
}
