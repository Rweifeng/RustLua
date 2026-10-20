//! `audio` 的 Lua 绑定（`docs/62` P1：绑定下沉到 owning crate）。
//!
//! ## 这个模块为什么在 `rustlua-audio` 而不是 `rustlua-scripting`
//!
//! 绑定原先住在 `rustlua-scripting/src/script/api/audio.rs`，通过 `ScriptState`（40 字段的中枢）
//! 摸 `AudioSystem`。代价是整个 crate 里**任何**领域的改动都要重编 VM 与全部绑定。搬到这里之后，
//! `rustlua-scripting` 不再 `use` 本 crate：它只认 [`rustlua_lua_api::Registry`]，由组合根把
//! [`install`] 塞进去。
//!
//! ## 为什么这里有一层 `SettingsPort`
//!
//! `audio.load_settings` / `audio.save_settings` 读写工程设置文件 `settings.toml` 的 `[audio]` 段，
//! 而那份文件的结构由 `rustlua-gameplay` 的 `ProjectSettings` 定义——`rustlua-gameplay` **已经**
//! 依赖本 crate，绑定下沉后若反过来 `use` 它，就成环了。所以文件读写走端口：音频只出"把这段文本
//! 应用到 `AudioSystem` / 把 `AudioSystem` 写成这段文本"的语义，文件在哪、怎么读、怎么保序写回，
//! 由宿主（`rustlua-gameplay` 的实现，见 `audio_lua.rs`）决定。
//!
//! ## 错误口径
//!
//! 与 `rustlua-scripting` 时期逐字一致：`unknown audio bus: {name}` 走 [`rustlua_lua_api::api_error`]，
//! `load_settings` / `save_settings` 返回 `(bool, 文本)` 而不是抛错——`jhyt` 的脚本靠这两条区分
//! "没有工程根" / "文件不存在"，改成抛错会破坏既有工程。

use mlua::{Lua, Result as LuaResult};
use rustlua_lua_api::{Handle, api_error};
use std::path::PathBuf;

use crate::audio::{AudioBus, AudioSystem};
use crate::spatial::Listener;

/// 工程设置文件里 `[audio]` 段的读写端口。
///
/// 实现方（`rustlua-gameplay` 的 `FileAudioSettings`）负责：文件路径口径（发布包下是用户数据
/// 目录）、经 `vfs` 读取（wasm 上没有文件系统）、写入时保留 `[input]` 等其它段。
pub trait SettingsPort {
    /// `settings.toml` 的完整路径；`None` = 当前没有工程根。
    fn path(&self) -> Option<PathBuf>;

    /// 读文件并应用到 `audio`。
    fn load(&self, audio: &mut AudioSystem) -> Result<(), SettingsLoadError>;

    /// 把 `audio` 的 `[audio]` 段写回文件（保留其余段）。
    fn save(&self, audio: &AudioSystem) -> Result<(), String>;
}

/// `load_settings` 的失败分类：Lua 侧要能区分这三件事，不能都塌成一条文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsLoadError {
    /// 没有工程根（发布包尚未确定用户数据目录）。
    NoRoot,
    /// 文件不存在——首次启动的正常状态，不是错误。
    Missing,
    /// 读到了但读/解析失败。
    Failed(String),
}

/// 把 `audio` 模块装进全局表。
///
/// `audio` 是宿主拥有的能力单元（[`Handle<AudioSystem>`]）；宿主与 Lua 共享同一份状态，
/// 重入时返回带 `audio` 名字的 Lua 错误而不是 panic。
pub fn install(
    lua: &Lua,
    audio: &Handle<AudioSystem>,
    settings: &std::rc::Rc<dyn SettingsPort>,
) -> LuaResult<()> {
    let table = lua.create_table()?;

    let handle = audio.clone();
    table.set(
        "play",
        lua.create_function(move |_, name: String| {
            if let Err(error) = handle.borrow_mut()?.play(&name) {
                tracing::warn!(target: "audio", name = %name, error = %error, "[audio] play failed");
            }
            Ok(())
        })?,
    )?;

    let handle = audio.clone();
    table.set(
        "volume",
        lua.create_function(move |_, level: f32| {
            handle.borrow_mut()?.set_volume(level);
            Ok(())
        })?,
    )?;

    // 总线名：master / bgm / sfx / ui；未知名报 Lua 错误（便于定位拼写错误），
    // 实际音量 = 主音量 × 分组音量（`AudioSystem::effective_volume`）。
    table.set(
        "buses",
        lua.create_function(move |lua, ()| {
            let buses = lua.create_table()?;
            for (index, bus) in AudioBus::ALL.into_iter().enumerate() {
                buses.set(index + 1, bus.key())?;
            }
            Ok(buses)
        })?,
    )?;

    let handle = audio.clone();
    table.set(
        "set_bus_volume",
        lua.create_function(move |_, (bus, level): (String, f32)| {
            let bus = parse_bus(&bus)?;
            handle.borrow_mut()?.set_bus_volume(bus, level);
            Ok(true)
        })?,
    )?;

    let handle = audio.clone();
    table.set(
        "bus_volume",
        lua.create_function(move |_, bus: String| {
            let bus = parse_bus(&bus)?;
            Ok(handle.borrow()?.bus_volume(bus))
        })?,
    )?;

    let handle = audio.clone();
    table.set(
        "effective_volume",
        lua.create_function(move |_, bus: String| {
            let bus = parse_bus(&bus)?;
            Ok(handle.borrow()?.effective_volume(bus))
        })?,
    )?;

    let handle = audio.clone();
    table.set(
        "play_on",
        lua.create_function(move |_, (bus, name): (String, String)| {
            let bus = parse_bus(&bus)?;
            if let Err(error) = handle.borrow_mut()?.play_on(bus, &name) {
                tracing::warn!(target: "audio", name = %name, bus = bus.key(), error = %error, "[audio] play_on failed");
                return Ok((false, error.to_string()));
            }
            Ok((true, String::new()))
        })?,
    )?;

    // 音量设置文本往返（只覆盖四路音量，不含工程设置文件的其余内容）。
    let handle = audio.clone();
    table.set(
        "settings_text",
        lua.create_function(move |_, ()| Ok(handle.borrow()?.to_settings_text()))?,
    )?;

    let handle = audio.clone();
    table.set(
        "apply_settings",
        lua.create_function(move |_, text: String| {
            match handle.borrow_mut()?.apply_settings_text(&text) {
                Ok(()) => Ok((true, String::new())),
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;

    // 工程设置文件 `settings.toml` 的 `[audio]` 段：缺失文件用默认值；写入保序保留其余段
    // （后续任务的 `[input]` 不会被覆盖）。路径口径由端口实现决定（发布包 → 用户数据目录）。
    let settings_for_path = std::rc::Rc::clone(settings);
    table.set(
        "settings_path",
        lua.create_function(move |_, ()| {
            Ok(settings_for_path
                .path()
                .map(|path| path.to_string_lossy().into_owned()))
        })?,
    )?;

    let handle = audio.clone();
    let settings_for_load = std::rc::Rc::clone(settings);
    table.set(
        "load_settings",
        lua.create_function(move |_, ()| {
            let mut audio = handle.borrow_mut()?;
            match settings_for_load.load(&mut audio) {
                Ok(()) => Ok((true, "loaded".to_owned())),
                Err(SettingsLoadError::NoRoot) => Ok((false, "no_project_root".to_owned())),
                Err(SettingsLoadError::Missing) => Ok((false, "missing".to_owned())),
                Err(SettingsLoadError::Failed(error)) => Ok((false, error)),
            }
        })?,
    )?;

    let handle = audio.clone();
    let settings_for_save = std::rc::Rc::clone(settings);
    table.set(
        "save_settings",
        lua.create_function(move |_, ()| {
            let audio = handle.borrow()?;
            match settings_for_save.save(&audio) {
                Ok(()) => Ok((true, "saved".to_owned())),
                Err(error) => Ok((false, error)),
            }
        })?,
    )?;

    let handle = audio.clone();
    table.set(
        "music",
        lua.create_function(move |_, name: String| {
            if let Err(error) = handle.borrow_mut()?.play_music(&name) {
                tracing::warn!(target: "audio", name = %name, error = %error, "[audio] music start failed");
            }
            Ok(())
        })?,
    )?;

    let handle = audio.clone();
    table.set(
        "stop_music",
        lua.create_function(move |_, ()| {
            handle.borrow_mut()?.stop_music();
            Ok(())
        })?,
    )?;

    // 空间化（M74-01）：`play_at` 用**宿主每帧灌进来的听者位姿**算增益/声像；`set_listener`
    // 给没有相机的纯逻辑用例（测试、菜单音效的固定听者）；`listener` 只读回位姿，便于断言。
    //
    // 返回值口径与 `play_on` 一致（第一个值是"这次播放发生了没有"），后面追加本次的
    // 增益/声像——它们是**引擎算出来的**，Lua 侧要做"越远越轻"的自定义淡出时得能读到。
    let handle = audio.clone();
    table.set(
        "play_at",
        lua.create_function(
            move |_, (name, x, y, z): (String, f32, f32, Option<f32>)| {
                let emitter = [x, y, z.unwrap_or(0.0)];
                match handle.borrow_mut()?.play_at(&name, emitter) {
                    Ok(params) => Ok((true, params.gain, params.pan)),
                    Err(error) => {
                        tracing::warn!(target: "audio", name = %name, error = %error, "[audio] play_at failed");
                        Ok((false, 0.0, 0.0))
                    }
                }
            },
        )?,
    )?;

    let handle = audio.clone();
    table.set(
        "set_listener",
        lua.create_function(
            move |_, (x, y, z, fx, fy, fz): (f32, f32, f32, f32, f32, f32)| {
                // 上方向固定 +Y：游戏相机没有滚转，Lua 侧不需要再多传三个数。
                handle.borrow_mut()?.set_listener(Listener::new(
                    [x, y, z],
                    [fx, fy, fz],
                    [0.0, 1.0, 0.0],
                ));
                Ok(())
            },
        )?,
    )?;

    let handle = audio.clone();
    table.set(
        "listener",
        lua.create_function(move |lua, ()| {
            let listener = handle.borrow()?.listener();
            let table = lua.create_table()?;
            let Some(listener) = listener else {
                return Ok(table);
            };
            for (index, value) in listener
                .position
                .into_iter()
                .chain(listener.forward)
                .enumerate()
            {
                table.set(index + 1, value)?;
            }
            Ok(table)
        })?,
    )?;

    lua.globals().set("audio", table)?;

    Ok(())
}

/// 音量总线名 → [`AudioBus`]；未知名报 Lua 错误（拼写错误可定位，不静默）。
fn parse_bus(name: &str) -> LuaResult<AudioBus> {
    AudioBus::from_key(&name.trim().to_ascii_lowercase())
        .ok_or_else(|| api_error(format!("unknown audio bus: {name}")))
}
