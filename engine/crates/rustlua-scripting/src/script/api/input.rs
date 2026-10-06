//! `input`：按键/鼠标/触屏快照、动作映射（ActionMap）与输入诊断。
//!
//! 这一段是 API 面里最长的单表之一，改它先想清楚帧序：读快照→动作判定→UI 导航消费。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::parse_key_strict;
use crate::script::parse_mouse_button;
use crate::script::push_input_diagnostic;
use crate::script::require::atomic_write;
use crate::script::resolve_input_settings_path;
use crate::script::settings_root;
use crate::script::values::table_vec;
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_gameplay::settings::NavSettings;
use rustlua_gameplay::settings::ProjectSettings;
use rustlua_input::input::ActionMap;
use rustlua_input::input::Binding;
use rustlua_input::input::KeyCode;
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let input = lua.create_table()?;
    for (name, mode) in [("is_down", 0_u8), ("just_pressed", 1), ("just_released", 2)] {
        let s = Rc::clone(&state);
        input.set(
            name,
            lua.create_function(move |_, key: String| {
                // M22/W1：键名解析失败是**错误**（未知名不再静默落 Other(0)）。
                let key = parse_key_strict(&key)?;
                let state = s.lua_borrow()?;
                Ok(match mode {
                    0 => state.input.is_down(key),
                    1 => state.input.just_pressed(key),
                    _ => state.input.just_released(key),
                })
            })?,
        )?;
    }
    let s = Rc::clone(&state);
    input.set(
        "pressed",
        lua.create_function(move |_, key: String| {
            let key = parse_key_strict(&key)?;
            Ok(s.lua_borrow()?.input.just_pressed(key))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "released",
        lua.create_function(move |_, key: String| {
            let key = parse_key_strict(&key)?;
            Ok(s.lua_borrow()?.input.just_released(key))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "axis",
        lua.create_function(move |_, axis: String| Ok(s.lua_borrow()?.input.axis(&axis)))?,
    )?;
    for (name, mode) in [
        ("mouse_is_down", 0_u8),
        ("mouse_just_pressed", 1),
        ("mouse_just_released", 2),
    ] {
        let s = Rc::clone(&state);
        input.set(
            name,
            lua.create_function(move |_, button: String| {
                // P1-④：未知名按钮是**错误**（不再静默落 Other(0)），与键名同口径。
                let button = parse_mouse_button(&button)?;
                let input = &s.lua_borrow()?.input;
                Ok(match mode {
                    0 => input.mouse_is_down(button),
                    1 => input.mouse_just_pressed(button),
                    _ => input.mouse_just_released(button),
                })
            })?,
        )?;
    }
    let s = Rc::clone(&state);
    input.set(
        "cursor_position",
        lua.create_function(move |lua, ()| {
            let p = s.lua_borrow()?.input.cursor_position();
            table_vec(lua, p.0 as f32, p.1 as f32)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "cursor_delta",
        lua.create_function(move |lua, ()| {
            let p = s.lua_borrow()?.input.cursor_delta();
            table_vec(lua, p.0 as f32, p.1 as f32)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "scroll_delta",
        lua.create_function(move |lua, ()| {
            let p = s.lua_borrow()?.input.scroll_delta();
            table_vec(lua, p.0, p.1)
        })?,
    )?;
    // ---- M20-07：命名动作层 + [input] 设置持久化 ---------------------------
    // 三层模型：物理输入（键鼠 + 宿主喂入的手柄设备帧）→ 命名动作
    // （`ScriptState::action_map`，一份实例）→ 动作消费（下面的 `input.action*`）。
    // 旧物理键 API（`is_down`/`just_pressed`/`just_released`/`axis`/`pressed`/
    // `released`）语义**不变**并继续可用，老脚本与 headless 用例不受影响；
    // 只有 `input.action` 从「物理键别名」升级为「命名动作查询」（全仓库除本
    // 文件外无调用方，见 M20-07 文档的迁移说明）。
    let s = Rc::clone(&state);
    input.set(
        "action",
        lua.create_function(move |_, name: String| {
            let state = s.lua_borrow()?;
            Ok(state.input.action_is_down(&state.action_map, &name))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "action_pressed",
        lua.create_function(move |_, name: String| {
            let state = s.lua_borrow()?;
            Ok(state.input.action_just_pressed(&state.action_map, &name))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "action_released",
        lua.create_function(move |_, name: String| {
            let state = s.lua_borrow()?;
            Ok(state.input.action_just_released(&state.action_map, &name))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "virtual_key",
        lua.create_function(move |_, (name, down): (String, bool)| {
            let mut state = s.lua_borrow_mut()?;
            if let Some(key) = KeyCode::from_text(&name.to_lowercase()) {
                state.input.set_virtual_key(key, down);
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "virtual_pressed",
        lua.create_function(move |_, name: String| {
            let mut state = s.lua_borrow_mut()?;
            if let Some(key) = KeyCode::from_text(&name.to_lowercase()) {
                state.input.set_virtual_pressed(key);
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "action_axis",
        lua.create_function(move |_, (negative, positive): (String, String)| {
            let state = s.lua_borrow()?;
            Ok(state
                .input
                .action_axis(&state.action_map, &negative, &positive))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "pad_button",
        lua.create_function(move |_, index: i64| {
            Ok(s.lua_borrow()?
                .input
                .gamepad_button(index.clamp(0, 255) as u8))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "pad_axis",
        lua.create_function(move |_, index: i64| {
            Ok(s.lua_borrow()?
                .input
                .gamepad_axis(index.clamp(0, 255) as u8))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "actions",
        lua.create_function(move |lua, ()| {
            let state = s.lua_borrow()?;
            let table = lua.create_table()?;
            for (index, name) in state.action_map.actions().into_iter().enumerate() {
                table.set(index + 1, name)?;
            }
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "bindings",
        lua.create_function(move |lua, name: String| {
            let state = s.lua_borrow()?;
            let table = lua.create_table()?;
            for (index, binding) in state.action_map.bindings(&name).iter().enumerate() {
                table.set(index + 1, binding.to_text())?;
            }
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "rebind",
        lua.create_function(move |_, (name, binding): (String, String)| {
            let parsed = match Binding::parse(binding.trim()) {
                Ok(parsed) => parsed,
                Err(error) => return Ok((false, error.to_string())),
            };
            let mut state = s.lua_borrow_mut()?;
            // 重绑定 = 覆盖该动作的全部旧绑定（单一主绑定语义，与面板 UI 一致）；
            // 需要一键多绑定时改设置文件的 `[input].map` 文本。
            state.action_map.clear(&name);
            match state.action_map.bind(&name, parsed) {
                Ok(()) => Ok((true, String::new())),
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "unbind",
        lua.create_function(move |_, (name, binding): (String, String)| {
            let Ok(parsed) = Binding::parse(binding.trim()) else {
                return Ok(false);
            };
            Ok(s.lua_borrow_mut()?.action_map.unbind(&name, parsed))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "reset_bindings",
        lua.create_function(move |_, ()| {
            // 唯一重置入口：绑定是玩家偏好，不随场景切换/读档回滚（ADR-0021）。
            s.lua_borrow_mut()?.action_map = ActionMap::with_defaults();
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "action_map_text",
        lua.create_function(move |_, ()| Ok(s.lua_borrow()?.action_map.to_text()))?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "set_action_map",
        lua.create_function(move |_, text: String| {
            // M22/W1：接受 v1 与 v2 两种版本头，v1 的键位会被迁移成物理键，并把
            // 有损换算记进 `input.migration_notes()`。
            match ActionMap::from_text_with_notes(&text) {
                Ok((map, notes)) => {
                    let mut state = s.lua_borrow_mut()?;
                    state.action_map = map;
                    state.input_migration_notes = notes;
                    Ok((true, String::new()))
                }
                // 版本头不符 → false + 原因，并且**不改动**当前映射（不静默回退）。
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "diagnostics",
        lua.create_function(move |lua, ()| {
            let state = s.lua_borrow()?;
            let table = lua.create_table()?;
            for (index, message) in state.input_diagnostics.iter().enumerate() {
                table.set(index + 1, message.clone())?;
            }
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "migration_notes",
        // M22/W1：v1 → v2 改键迁移说明（有损换算逐条）。与 `diagnostics()` 分开：
        // 诊断 = 「出错了」，迁移说明 = 「变了但没丢」。启动时打印一次即可。
        lua.create_function(move |lua, ()| {
            let state = s.lua_borrow()?;
            let table = lua.create_table()?;
            for (index, message) in state.input_migration_notes.iter().enumerate() {
                table.set(index + 1, message.clone())?;
            }
            Ok(table)
        })?,
    )?;
    // 设置文件 `settings.toml` 的 `[input]` 段（M20-07）。路径规则与
    // `audio.settings_path/load_settings/save_settings` 一致：缺省的根由
    // `settings_root()` 决定（P3：发布包 → 用户数据目录，开发工程 → 项目根），
    // 给相对路径时相对该根解析（无根则要求绝对路径）。
    let s = Rc::clone(&state);
    input.set(
        "settings_path",
        lua.create_function(move |_, ()| {
            let root = settings_root(&*s.lua_borrow()?);
            Ok(root.map(|root| {
                ProjectSettings::path_in(&root)
                    .to_string_lossy()
                    .into_owned()
            }))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "load_settings",
        lua.create_function(move |_, path: Option<String>| {
            let resolved = resolve_input_settings_path(&*s.lua_borrow()?, path.as_deref());
            let Some(path) = resolved else {
                return Ok((false, "no_project_root".to_owned()));
            };
            let existing = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // 文件不存在：保留当前绑定（不是错误），返回 false + "missing"
                    //（与 `audio.load_settings` 的既有语义一致）。
                    return Ok((false, "missing".to_owned()));
                }
                Err(error) => {
                    let message = format!("{}: read failed: {error}", path.display());
                    push_input_diagnostic(&mut *s.lua_borrow_mut()?, message);
                    return Ok((false, error.to_string()));
                }
            };
            let parsed = rustlua_gameplay::settings::parse_input_section(&existing);
            // M22/W3：读改键的同时把同一个 `[input]` 段的**导航手感**应用到引擎
            //（阈值 + 重复节奏），这样游戏只需 `input.load_settings()` 一次调用即可，
            // 不必再单独调 `apply_nav_settings()`。文件里没有这些键时就是默认手感。
            s.lua_borrow_mut()?.nav = rustlua_gameplay::settings::parse_nav_section(&existing);
            // M22/W1：v1 → v2 迁移说明（有损换算逐条），供 `input.migration_notes()`。
            s.lua_borrow_mut()?.input_migration_notes = parsed.migration_notes.clone();
            match parsed.map {
                Some(mut map) => {
                    // 文件里没有的动作用默认绑定补齐（新增动作在旧设置文件上仍可玩），
                    // 用户重绑定不被覆盖（`merge_missing` 只补缺失动作）。
                    map.merge_missing(&ActionMap::with_defaults());
                    s.lua_borrow_mut()?.action_map = map;
                    Ok((true, "loaded".to_owned()))
                }
                None => {
                    let reason = parsed
                        .reason
                        .unwrap_or_else(|| "invalid_input_section".to_owned());
                    let missing_key = reason.starts_with("missing_key");
                    if !missing_key {
                        // 内容非法（版本头不符 / 绑定文本非法）才是「回退默认」，
                        // 并留下可定位诊断；缺键只诊断、在位的绑定保持不变。
                        s.lua_borrow_mut()?.action_map = ActionMap::with_defaults();
                    }
                    let mut state = s.lua_borrow_mut()?;
                    let message = if missing_key {
                        format!("{}: {reason}; keeping current bindings", path.display())
                    } else {
                        format!("{}: {reason}; reverting to defaults", path.display())
                    };
                    push_input_diagnostic(&mut state, message.clone());
                    Ok((false, message))
                }
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "save_settings",
        lua.create_function(move |_, path: Option<String>| {
            let (resolved, map) = {
                let state = s.lua_borrow()?;
                (
                    resolve_input_settings_path(&state, path.as_deref()),
                    state.action_map.clone(),
                )
            };
            let Some(path) = resolved else {
                return Ok((false, "no_project_root".to_owned()));
            };
            // 保序读改写：只动 `[input]` 段，`[audio]` 段与注释原样保留。
            let existing = fs::read_to_string(&path).unwrap_or_default();
            if let Some(parent) = path.parent()
                && let Err(error) = fs::create_dir_all(parent)
            {
                return Ok((false, error.to_string()));
            }
            match atomic_write(
                &path,
                &rustlua_gameplay::settings::upsert_input_section(&existing, &map),
            ) {
                Ok(()) => Ok((true, "saved".to_owned())),
                Err(error) => {
                    let message = format!("{}: save failed: {error}", path.display());
                    push_input_diagnostic(&mut *s.lua_borrow_mut()?, message);
                    Ok((false, error.to_string()))
                }
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "nav_settings",
        lua.create_function(move |lua, path: Option<String>| {
            // M20-09 / M22-W3：菜单导航手感（摇杆阈值 + 按住重复节奏）与
            // `load_settings` 同源——同一个 `[input]` 段、同一套宽松解析与钳制。
            // 缺省路径 = 工程根。重复节奏自 M22/W3 起是**毫秒**（旧的帧键仍可读，
            // 按 60Hz 换算）。
            let resolved = resolve_input_settings_path(&*s.lua_borrow()?, path.as_deref());
            let Some(path) = resolved else {
                return Ok(None);
            };
            let nav = match fs::read_to_string(&path) {
                Ok(text) => rustlua_gameplay::settings::parse_nav_section(&text),
                // 文件缺失/不可读 → 默认手感（首次运行的正常路径）。
                Err(_) => NavSettings::DEFAULT,
            };
            let table = lua.create_table()?;
            table.set("threshold", nav.threshold)?;
            table.set("delay_ms", nav.delay_ms)?;
            table.set("repeat_ms", nav.repeat_ms)?;
            Ok(Some(table))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "set_nav_settings",
        // M22/W3：把导航手感推给引擎（阈值同时作为设备层轴判定阈值，重复节奏供
        // `input.action_repeat` 使用）。三个参数都按 `NavSettings::new` 钳制。
        lua.create_function(
            move |_, (threshold, delay_ms, repeat_ms): (f32, u32, u32)| {
                let nav = NavSettings::new(threshold, delay_ms, repeat_ms);
                s.lua_borrow_mut()?.nav = nav;
                Ok((nav.threshold, nav.delay_ms, nav.repeat_ms))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "action_repeat",
        // M22/W3：菜单导航的「首次立即 + 按住连发」。时间制（毫秒），因此键盘、DPad、
        // 摇杆在不同帧率下手感一致；只有模拟量绑定会连发（数字键保持一次一步）。
        lua.create_function(move |_, name: String| Ok(s.lua_borrow_mut()?.action_repeat(&name)))?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "last_device",
        // M22/W3：最近一次产生真实输入的设备类别（"keyboard" / "gamepad"）。
        // 引擎在边沿发生时记录，因此 Lua 侧不再需要每帧扫描全部绑定来猜设备。
        lua.create_function(move |_, ()| Ok(s.lua_borrow()?.input.last_device().to_text()))?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "rumble",
        // M22/W2：请求震动（左右马达 0..1，毫秒）。这里只登记请求，宿主在帧末交给设备层
        //（`GamepadPoller::rumble` 负责时长上限与到期归零）；无手柄时静默无效。
        lua.create_function(move |_, (low, high, ms): (f32, f32, u32)| {
            s.lua_borrow_mut()?.rumble_request = Some((low, high, ms));
            Ok(())
        })?,
    )?;
    lua.globals().set("input", input)?;

    Ok(())
}
