//! `input`：按键/鼠标/触屏快照、动作映射（ActionMap）与输入诊断。
//!
//! 这是 API 面里最长的单表之一，改它先想清楚帧序：读快照→动作判定→UI 导航消费。
//!
//! 绑定住在这里（而不是 `rustlua-scripting`）是 `docs/62` P1 阶段 3：`input` 的**事实**
//! （快照、动作映射、导航手感、诊断）都在本 crate 的类型上，VM 只是消费者。设置文件那一半
//! 仍归 `rustlua-gameplay`（`ProjectSettings` 的 `[input]` 段），通过 [`SettingsPort`] 注入。

use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_lua_api::api_error;
use std::rc::Rc;

use crate::host::HostInput;
use crate::input::{ActionMap, Binding, KeyCode, MouseButton};
use crate::nav::NavSettings;
use crate::port::{SettingsLoadError, SettingsPort, SettingsSaveError};

/// `[x, y]` 两元素表（`input.cursor_position` 等的老形状，脚本按数字下标读）。
fn table_vec(lua: &Lua, x: f32, y: f32) -> LuaResult<mlua::Table> {
    let table = lua.create_table()?;
    table.set(1, x)?;
    table.set(2, y)?;
    Ok(table)
}

/// Lua `input.is_down/just_pressed/just_released` 的键名解析（M22/W1 起**严格**）。
///
/// 接受三种写法，与设置文件 / 编辑器捕获共用同一套键身份：
///
/// 1. **v2 物理键名**：`input.is_down("keye")`、`input.is_down("digit1")`、
///    `input.is_down("arrowup")`、`input.is_down("space")`（即 [`KeyCode::to_text`]
///    的输出，大小写不敏感，`key:` 前缀可省）；
/// 2. **v1 兼容名**：`"w"`、`"1"`、`"up"`、`"f1"`、`"space"`、`"escape"`、
///    `"other:1048577"`——老脚本与既有 headless 用例继续可用，按 US-QWERTY 位置
///    换算（见 [`KeyCode::v1_key_code`]）；
/// 3. `"key:<名字>"` 前缀形式（与 `input.rebind` 的绑定文本同源）。
///
/// **未知名报 Lua 错误**：旧实现把任何不认识的键静默映射成 `Other(0)`，于是拼错的
/// 键名与真实按键无法区分（`input.is_down("W ")` 会永远返回 false 且毫无提示）。
/// 现在与 `Binding::parse` 同一严格度，错误信息里带上原值。
fn parse_key_strict(name: &str) -> LuaResult<KeyCode> {
    let raw = name.trim();
    if raw.is_empty() {
        return Err(api_error("input key name must not be empty"));
    }
    let value = raw.strip_prefix("key:").unwrap_or(raw);
    if let Some(key) = KeyCode::from_text(value) {
        return Ok(key);
    }
    let mut notes = Vec::new();
    if let Some(key) = KeyCode::v1_key_code(value, &mut notes) {
        return Ok(key);
    }
    Err(api_error(format!(
        "unknown input key: {raw}（可用物理键名如 keye/digit1/arrowup/space，或旧名如 w/1/up）"
    )))
}

/// 鼠标按钮名 → [`MouseButton`]（P1-④：与 `parse_key_strict` 同一严格度）。
///
/// 旧实现把任何不认识的名字静默映射成 `Other(0)`，拼错的按钮名与真实按钮无法
/// 区分。现在未知名报 Lua 错误；接受的写法与 `Binding::parse` 的 `mouse:*` 同源：
/// `left` / `right` / `middle` / `other:<序号>`，`mouse:` 前缀可省，大小写不敏感。
fn parse_mouse_button(button: &str) -> LuaResult<MouseButton> {
    let raw = button.trim();
    if raw.is_empty() {
        return Err(api_error("mouse button name must not be empty"));
    }
    let value = raw
        .strip_prefix("mouse:")
        .unwrap_or(raw)
        .to_ascii_lowercase();
    match value.as_str() {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        other => other
            .strip_prefix("other:")
            .and_then(|index| index.parse::<u16>().ok())
            .map_or_else(
                || {
                    Err(api_error(format!(
                        "unknown mouse button: {raw}（可用 left/right/middle/other:<序号>）"
                    )))
                },
                |index| Ok(MouseButton::Other(index)),
            ),
    }
}

/// 装 `input` 全局表。`host` 与宿主共享同一份 [`crate::state::InputState`]，
/// `settings` 负责 `[input]` 段设置文件（文件在哪、怎么读、写完保留别的段）。
pub fn install(lua: &Lua, host: &HostInput, settings: &Rc<dyn SettingsPort>) -> LuaResult<()> {
    let input = lua.create_table()?;
    for (name, mode) in [("is_down", 0_u8), ("just_pressed", 1), ("just_released", 2)] {
        let st = host.state();
        input.set(
            name,
            lua.create_function(move |_, key: String| {
                // M22/W1：键名解析失败是**错误**（未知名不再静默落 Other(0)）。
                let key = parse_key_strict(&key)?;
                let state = st.borrow()?;
                Ok(match mode {
                    0 => state.snapshot.is_down(key),
                    1 => state.snapshot.just_pressed(key),
                    _ => state.snapshot.just_released(key),
                })
            })?,
        )?;
    }
    let st = host.state();
    input.set(
        "pressed",
        lua.create_function(move |_, key: String| {
            let key = parse_key_strict(&key)?;
            Ok(st.borrow()?.snapshot.just_pressed(key))
        })?,
    )?;
    let st = host.state();
    input.set(
        "released",
        lua.create_function(move |_, key: String| {
            let key = parse_key_strict(&key)?;
            Ok(st.borrow()?.snapshot.just_released(key))
        })?,
    )?;
    let st = host.state();
    input.set(
        "axis",
        lua.create_function(move |_, axis: String| Ok(st.borrow()?.snapshot.axis(&axis)))?,
    )?;
    for (name, mode) in [
        ("mouse_is_down", 0_u8),
        ("mouse_just_pressed", 1),
        ("mouse_just_released", 2),
    ] {
        let st = host.state();
        input.set(
            name,
            lua.create_function(move |_, button: String| {
                // P1-④：未知名按钮是**错误**（不再静默落 Other(0)），与键名同口径。
                let button = parse_mouse_button(&button)?;
                let input = &st.borrow()?.snapshot;
                Ok(match mode {
                    0 => input.mouse_is_down(button),
                    1 => input.mouse_just_pressed(button),
                    _ => input.mouse_just_released(button),
                })
            })?,
        )?;
    }
    let st = host.state();
    input.set(
        "cursor_position",
        lua.create_function(move |lua, ()| {
            let p = st.borrow()?.snapshot.cursor_position();
            table_vec(lua, p.0 as f32, p.1 as f32)
        })?,
    )?;
    let st = host.state();
    input.set(
        "press_position",
        lua.create_function(move |lua, index: Option<i64>| {
            // 语义刻意分两种：**不传**给主按下点（旧行为，`ui/virtual_pad.lua` 就是这么调的）；
            // **传索引**给那根手指的按下点。越界回 `nil`——命中测试里"错的坐标"比"没有坐标"更糟，
            // 所以宁可不给，由 Lua 侧既有的 `if pp then` 判据接住。
            let slot = index.filter(|raw| *raw >= 0).map(|raw| raw as usize);
            let Some((x, y)) = st.borrow()?.snapshot.press_position(slot) else {
                return Ok(mlua::Value::Nil);
            };
            Ok(mlua::Value::Table(table_vec(lua, x as f32, y as f32)?))
        })?,
    )?;
    let st = host.state();
    input.set(
        "pointer_count",
        // 有几根手指现在按着。命中测试可以据此只在"多指才有意义"的分支里循环。
        lua.create_function(move |_, ()| Ok(i64::from(st.borrow()?.snapshot.pointer_count())))?,
    )?;
    let st = host.state();
    input.set(
        "cursor_delta",
        lua.create_function(move |lua, ()| {
            let p = st.borrow()?.snapshot.cursor_delta();
            table_vec(lua, p.0 as f32, p.1 as f32)
        })?,
    )?;
    let st = host.state();
    input.set(
        "scroll_delta",
        lua.create_function(move |lua, ()| {
            let p = st.borrow()?.snapshot.scroll_delta();
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
    let st = host.state();
    input.set(
        "action",
        lua.create_function(move |_, name: String| {
            let state = st.borrow()?;
            Ok(state.snapshot.action_is_down(&state.action_map, &name))
        })?,
    )?;
    let st = host.state();
    input.set(
        "action_pressed",
        lua.create_function(move |_, name: String| {
            let state = st.borrow()?;
            Ok(state.snapshot.action_just_pressed(&state.action_map, &name))
        })?,
    )?;
    let st = host.state();
    input.set(
        "action_released",
        lua.create_function(move |_, name: String| {
            let state = st.borrow()?;
            Ok(state
                .snapshot
                .action_just_released(&state.action_map, &name))
        })?,
    )?;
    let st = host.state();
    input.set(
        "virtual_key",
        lua.create_function(move |_, (name, down): (String, bool)| {
            let mut state = st.borrow_mut()?;
            if let Some(key) = KeyCode::from_text(&name.to_lowercase()) {
                state.snapshot.set_virtual_key(key, down);
            }
            Ok(())
        })?,
    )?;
    let st = host.state();
    input.set(
        "virtual_pressed",
        lua.create_function(move |_, name: String| {
            let mut state = st.borrow_mut()?;
            if let Some(key) = KeyCode::from_text(&name.to_lowercase()) {
                state.snapshot.set_virtual_pressed(key);
            }
            Ok(())
        })?,
    )?;
    let st = host.state();
    input.set(
        "action_axis",
        lua.create_function(move |_, (negative, positive): (String, String)| {
            let state = st.borrow()?;
            Ok(state
                .snapshot
                .action_axis(&state.action_map, &negative, &positive))
        })?,
    )?;
    let st = host.state();
    input.set(
        "pad_button",
        lua.create_function(move |_, index: i64| {
            Ok(st
                .borrow()?
                .snapshot
                .gamepad_button(index.clamp(0, 255) as u8))
        })?,
    )?;
    let st = host.state();
    input.set(
        "pad_axis",
        lua.create_function(move |_, index: i64| {
            Ok(st
                .borrow()?
                .snapshot
                .gamepad_axis(index.clamp(0, 255) as u8))
        })?,
    )?;
    let st = host.state();
    input.set(
        "actions",
        lua.create_function(move |lua, ()| {
            let state = st.borrow()?;
            let table = lua.create_table()?;
            for (index, name) in state.action_map.actions().into_iter().enumerate() {
                table.set(index + 1, name)?;
            }
            Ok(table)
        })?,
    )?;
    let st = host.state();
    input.set(
        "bindings",
        lua.create_function(move |lua, name: String| {
            let state = st.borrow()?;
            let table = lua.create_table()?;
            for (index, binding) in state.action_map.bindings(&name).iter().enumerate() {
                table.set(index + 1, binding.to_text())?;
            }
            Ok(table)
        })?,
    )?;
    let st = host.state();
    input.set(
        "rebind",
        lua.create_function(move |_, (name, binding): (String, String)| {
            let parsed = match Binding::parse(binding.trim()) {
                Ok(parsed) => parsed,
                Err(error) => return Ok((false, error.to_string())),
            };
            let mut state = st.borrow_mut()?;
            // 重绑定 = 覆盖该动作的全部旧绑定（单一主绑定语义，与面板 UI 一致）；
            // 需要一键多绑定时改设置文件的 `[input].map` 文本。
            state.action_map.clear(&name);
            match state.action_map.bind(&name, parsed) {
                Ok(()) => Ok((true, String::new())),
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    let st = host.state();
    input.set(
        "unbind",
        lua.create_function(move |_, (name, binding): (String, String)| {
            let Ok(parsed) = Binding::parse(binding.trim()) else {
                return Ok(false);
            };
            Ok(st.borrow_mut()?.action_map.unbind(&name, parsed))
        })?,
    )?;
    let st = host.state();
    input.set(
        "reset_bindings",
        lua.create_function(move |_, ()| {
            // 唯一重置入口：绑定是玩家偏好，不随场景切换/读档回滚（ADR-0021）。
            st.borrow_mut()?.action_map = ActionMap::with_defaults();
            Ok(())
        })?,
    )?;
    let st = host.state();
    input.set(
        "action_map_text",
        lua.create_function(move |_, ()| Ok(st.borrow()?.action_map.to_text()))?,
    )?;
    let st = host.state();
    input.set(
        "set_action_map",
        lua.create_function(move |_, text: String| {
            // M22/W1：接受 v1 与 v2 两种版本头，v1 的键位会被迁移成物理键，并把
            // 有损换算记进 `input.migration_notes()`。
            match ActionMap::from_text_with_notes(&text) {
                Ok((map, notes)) => {
                    let mut state = st.borrow_mut()?;
                    state.action_map = map;
                    state.migration_notes = notes;
                    Ok((true, String::new()))
                }
                // 版本头不符 → false + 原因，并且**不改动**当前映射（不静默回退）。
                Err(error) => Ok((false, error.to_string())),
            }
        })?,
    )?;
    let st = host.state();
    input.set(
        "diagnostics",
        lua.create_function(move |lua, ()| {
            let state = st.borrow()?;
            let table = lua.create_table()?;
            for (index, message) in state.diagnostics.iter().enumerate() {
                table.set(index + 1, message.clone())?;
            }
            Ok(table)
        })?,
    )?;
    let st = host.state();
    input.set(
        "migration_notes",
        // M22/W1：v1 → v2 改键迁移说明（有损换算逐条）。与 `diagnostics()` 分开：
        // 诊断 = 「出错了」，迁移说明 = 「变了但没丢」。启动时打印一次即可。
        lua.create_function(move |lua, ()| {
            let state = st.borrow()?;
            let table = lua.create_table()?;
            for (index, message) in state.migration_notes.iter().enumerate() {
                table.set(index + 1, message.clone())?;
            }
            Ok(table)
        })?,
    )?;
    // ---- 上面是快照/动作查询；下面是设置文件、导航手感、连发与震动 ----
    // 设置文件 `settings.toml` 的 `[input]` 段（M20-07）。路径规则与
    // `audio.settings_path/load_settings/save_settings` 一致：缺省的根由注册表里的
    // 「设置根」单元决定（P3：发布包 → 用户数据目录，开发工程 → 项目根），
    // 给相对路径时相对该根解析（无根则要求绝对路径）——那套规则由端口实现持有。
    let port = Rc::clone(settings);
    input.set(
        "settings_path",
        lua.create_function(move |_, ()| {
            Ok(port.path().map(|path| path.to_string_lossy().into_owned()))
        })?,
    )?;
    let st = host.state();
    let port = Rc::clone(settings);
    input.set(
        "load_settings",
        lua.create_function(move |_, path: Option<String>| {
            let resolved = port.load(path.as_deref());
            match resolved {
                Err(SettingsLoadError::NoRoot) => Ok((false, "no_project_root".to_owned())),
                Err(SettingsLoadError::Missing) => {
                    // 文件不存在：保留当前绑定（不是错误），返回 false + "missing"
                    //（与 `audio.load_settings` 的既有语义一致）。
                    Ok((false, "missing".to_owned()))
                }
                Err(SettingsLoadError::ReadFailed {
                    path: failed_path,
                    message,
                }) => {
                    let mut state = st.borrow_mut()?;
                    state.push_diagnostic(format!(
                        "{}: read failed: {message}",
                        failed_path.display()
                    ));
                    Ok((false, message))
                }
                Ok(loaded) => {
                    let mut state = st.borrow_mut()?;
                    // M22/W3：读改键的同时把同一个 `[input]` 段的**导航手感**应用到引擎
                    //（阈值 + 重复节奏），这样游戏只需 `input.load_settings()` 一次调用即可，
                    // 不必再单独调 `apply_nav_settings()`。文件里没有这些键时就是默认手感。
                    state.nav = loaded.nav;
                    // M22/W1：v1 → v2 迁移说明（有损换算逐条），供 `input.migration_notes()`。
                    state.migration_notes = loaded.notes;
                    match loaded.map {
                        Some(mut map) => {
                            // 文件里没有的动作用默认绑定补齐（新增动作在旧设置文件上仍可玩），
                            // 用户重绑定不被覆盖（`merge_missing` 只补缺失动作）。
                            map.merge_missing(&ActionMap::with_defaults());
                            state.action_map = map;
                            Ok((true, "loaded".to_owned()))
                        }
                        None => {
                            let reason = loaded
                                .reason
                                .unwrap_or_else(|| "invalid_input_section".to_owned());
                            let missing_key = reason.starts_with("missing_key");
                            if !missing_key {
                                // 内容非法（版本头不符 / 绑定文本非法）才是「回退默认」，
                                // 并留下可定位诊断；缺键只诊断、在位的绑定保持不变。
                                state.action_map = ActionMap::with_defaults();
                            }
                            let message = if missing_key {
                                format!(
                                    "{}: {reason}; keeping current bindings",
                                    loaded.path.display()
                                )
                            } else {
                                format!(
                                    "{}: {reason}; reverting to defaults",
                                    loaded.path.display()
                                )
                            };
                            state.push_diagnostic(message.clone());
                            Ok((false, message))
                        }
                    }
                }
            }
        })?,
    )?;
    let st = host.state();
    let port = Rc::clone(settings);
    input.set(
        "save_settings",
        lua.create_function(move |_, path: Option<String>| {
            let map = st.borrow()?.action_map.clone();
            // 保序读改写：只动 `[input]` 段，`[audio]` 段与注释原样保留（端口实现里做）。
            match port.save(path.as_deref(), &map) {
                Ok(()) => Ok((true, "saved".to_owned())),
                Err(SettingsSaveError::NoRoot) => Ok((false, "no_project_root".to_owned())),
                Err(SettingsSaveError::Directory { message }) => Ok((false, message)),
                Err(SettingsSaveError::Write {
                    path: failed_path,
                    message,
                }) => {
                    st.borrow_mut()?.push_diagnostic(format!(
                        "{}: save failed: {message}",
                        failed_path.display()
                    ));
                    Ok((false, message))
                }
            }
        })?,
    )?;
    let port = Rc::clone(settings);
    input.set(
        "nav_settings",
        lua.create_function(move |lua, path: Option<String>| {
            // M20-09 / M22-W3：菜单导航手感（摇杆阈值 + 按住重复节奏）与
            // `load_settings` 同源——同一个 `[input]` 段、同一套宽松解析与钳制。
            // 缺省路径 = 工程根。重复节奏自 M22/W3 起是**毫秒**（旧的帧键仍可读，
            // 按 60Hz 换算）。
            let nav = match port.load(path.as_deref()) {
                // 没有工程根：与旧实现一致，返回 nil（而不是默认表）。
                Err(SettingsLoadError::NoRoot) => return Ok(None),
                // 文件缺失/不可读 → 默认手感（首次运行的正常路径）。
                Err(_) => NavSettings::DEFAULT,
                Ok(loaded) => loaded.nav,
            };
            let table = lua.create_table()?;
            table.set("threshold", nav.threshold)?;
            table.set("delay_ms", nav.delay_ms)?;
            table.set("repeat_ms", nav.repeat_ms)?;
            Ok(Some(table))
        })?,
    )?;
    let st = host.state();
    input.set(
        "set_nav_settings",
        // M22/W3：把导航手感推给引擎（阈值同时作为设备层轴判定阈值，重复节奏供
        // `input.action_repeat` 使用）。三个参数都按 `NavSettings::new` 钳制。
        lua.create_function(
            move |_, (threshold, delay_ms, repeat_ms): (f32, u32, u32)| {
                let nav = NavSettings::new(threshold, delay_ms, repeat_ms);
                st.borrow_mut()?.nav = nav;
                Ok((nav.threshold, nav.delay_ms, nav.repeat_ms))
            },
        )?,
    )?;
    let host_handle = host.clone();
    input.set(
        "action_repeat",
        // M22/W3：菜单导航的「首次立即 + 按住连发」。时间制（毫秒），因此键盘、DPad、
        // 摇杆在不同帧率下手感一致；只有模拟量绑定会连发（数字键保持一次一步）。
        lua.create_function(move |_, name: String| Ok(host_handle.action_repeat(&name)))?,
    )?;
    let st = host.state();
    input.set(
        "last_device",
        // M22/W3：最近一次产生真实输入的设备类别（"keyboard" / "gamepad"）。
        // 引擎在边沿发生时记录，因此 Lua 侧不再需要每帧扫描全部绑定来猜设备。
        lua.create_function(move |_, ()| Ok(st.borrow()?.snapshot.last_device().to_text()))?,
    )?;
    let st = host.state();
    input.set(
        "rumble",
        // M22/W2：请求震动（左右马达 0..1，毫秒）。这里只登记请求，宿主在帧末交给设备层
        //（`GamepadPoller::rumble` 负责时长上限与到期归零）；无手柄时静默无效。
        lua.create_function(move |_, (low, high, ms): (f32, f32, u32)| {
            st.borrow_mut()?.rumble_request = Some((low, high, ms));
            Ok(())
        })?,
    )?;
    lua.globals().set("input", input)?;

    Ok(())
}
