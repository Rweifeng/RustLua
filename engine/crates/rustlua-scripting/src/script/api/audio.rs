//! `audio`：音效总线、音量与 3D 衰减。播放走 `AudioSystem`，不在此文件内。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::parse_audio_bus;
use crate::script::settings_root;
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_audio::audio::AudioBus;
use rustlua_gameplay::settings::ProjectSettings;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let audio = lua.create_table()?;
    let s = Rc::clone(&state);
    audio.set(
        "play",
        lua.create_function(move |_, name: String| {
            let mut state = s.lua_borrow_mut()?;
            if let Err(error) = state.audio.play(&name) {
                tracing::warn!(target: "audio", name = %name, error = %error, "[audio] play failed");
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "volume",
        lua.create_function(move |_, level: f32| {
            s.borrow_mut().audio.set_volume(level);
            Ok(())
        })?,
    )?;
    // ---- M20-02：音量分组（总线）------------------------------------------
    // 总线名：master / bgm / sfx / ui；未知名报 Lua 错误（便于定位拼写错误），
    // 实际音量 = 主音量 × 分组音量（`AudioSystem::effective_volume`）。
    audio.set(
        "buses",
        lua.create_function(move |lua, ()| {
            let buses = lua.create_table()?;
            for (index, bus) in AudioBus::ALL.into_iter().enumerate() {
                buses.set(index + 1, bus.key())?;
            }
            Ok(buses)
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "set_bus_volume",
        lua.create_function(move |_, (bus, level): (String, f32)| {
            let bus = parse_audio_bus(&bus)?;
            s.borrow_mut().audio.set_bus_volume(bus, level);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "bus_volume",
        lua.create_function(move |_, bus: String| {
            let bus = parse_audio_bus(&bus)?;
            Ok(s.borrow().audio.bus_volume(bus))
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "effective_volume",
        lua.create_function(move |_, bus: String| {
            let bus = parse_audio_bus(&bus)?;
            Ok(s.borrow().audio.effective_volume(bus))
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "play_on",
        lua.create_function(move |_, (bus, name): (String, String)| {
            let bus = parse_audio_bus(&bus)?;
            if let Err(error) = s.borrow_mut().audio.play_on(bus, &name) {
                tracing::warn!(target: "audio", name = %name, bus = bus.key(), error = %error, "[audio] play_on failed");
                return Ok((false, error.to_string()));
            }
            Ok((true, String::new()))
        })?,
    )?;
    // 音量设置文本往返（只覆盖四路音量，不含工程设置文件的其余内容）。
    let s = Rc::clone(&state);
    audio.set(
        "settings_text",
        lua.create_function(move |_, ()| Ok(s.borrow().audio.to_settings_text()))?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "apply_settings",
        lua.create_function(move |_, text: String| {
            match s.borrow_mut().audio.apply_settings_text(&text) {
                Ok(()) => Ok((true, String::new())),
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    // 工程设置文件 `settings.toml` 的 `[audio]` 段（M20-02）：
    // 缺失文件用默认值；写入保序保留其余段（后续任务的 [input] 不会被覆盖）。
    // 路径根由 `settings_root()` 决定（P3：发布包 → 用户数据目录）。
    let s = Rc::clone(&state);
    audio.set(
        "settings_path",
        lua.create_function(move |_, ()| {
            let root = settings_root(&s.borrow());
            Ok(root.map(|root| {
                ProjectSettings::path_in(&root)
                    .to_string_lossy()
                    .into_owned()
            }))
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "load_settings",
        lua.create_function(move |_, ()| {
            let root = settings_root(&s.borrow());
            let Some(root) = root else {
                return Ok((false, "no_project_root".to_owned()));
            };
            match ProjectSettings::load_from(&root) {
                Ok(Some(settings)) => {
                    settings.apply_to(&mut s.borrow_mut().audio);
                    Ok((true, "loaded".to_owned()))
                }
                Ok(None) => Ok((false, "missing".to_owned())),
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "save_settings",
        lua.create_function(move |_, ()| {
            let root = settings_root(&s.borrow());
            let Some(root) = root else {
                return Ok((false, "no_project_root".to_owned()));
            };
            let settings = ProjectSettings::from_audio(&s.borrow().audio);
            match settings.save_to(&root) {
                Ok(()) => Ok((true, "saved".to_owned())),
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "music",
        lua.create_function(move |_, name: String| {
            let mut state = s.lua_borrow_mut()?;
            if let Err(error) = state.audio.play_music(&name) {
                tracing::warn!(target: "audio", name = %name, error = %error, "[audio] music start failed");
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "stop_music",
        lua.create_function(move |_, ()| {
            s.borrow_mut().audio.stop_music();
            Ok(())
        })?,
    )?;
    lua.globals().set("audio", audio)?;

    Ok(())
}
