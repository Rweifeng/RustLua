//! 与实体无关的全局设施：日志、帧时长、应用生命周期、事件总线。
//!
//! Lua 侧的 `log` / `time` / `app` / `event`。
//!
//! **P1 ⑤-5**：`clock` / `calendar` / `day` 三张表已下沉到游戏时钟域
//! （`rustlua_gameplay::clock_lua`）——它们读写的是 `GameClock`，那台时钟的类型与推进规则
//! 本来就在 `rustlua-gameplay`。留在这里的这四张表**没有任何领域状态**：`log`/`event` 是纯
//! VM 设施（订阅表归当前 VM 所有），`time.delta` 是宿主逐帧写入的帧事实，`app` 只转发宿主
//! 生命周期（`quit`）、平台常量与工程根能力单元。VM 宿主自己拥有"宿主的全局表"是合理的边界，
//! 不该为了凑模块数把 `log.info` 搬进某个领域 crate。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use mlua::Result as LuaResult;
use mlua::{Function, Lua, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use tracing::{error, info, warn};
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let log = lua.create_table()?;
    log.set(
        "info",
        lua.create_function(|_, message: String| {
            info!(target: "script", message = %message, "Lua log.info");
            Ok(())
        })?,
    )?;
    log.set(
        "warn",
        lua.create_function(|_, message: String| {
            warn!(target: "script", message = %message, "Lua log.warn");
            Ok(())
        })?,
    )?;
    log.set(
        "error",
        lua.create_function(|_, message: String| {
            error!(target: "script", message = %message, "Lua log.error");
            Ok(())
        })?,
    )?;
    lua.globals().set("log", log)?;

    let time = lua.create_table()?;
    let s = Rc::clone(&state);
    time.set(
        "delta",
        lua.create_function(move |_, ()| Ok(s.lua_borrow()?.delta_seconds))?,
    )?;
    lua.globals().set("time", time)?;

    // `clock` / `calendar` / `day` 已下沉到游戏时钟域（P1 ⑤-5）：
    // `rustlua_gameplay::clock_lua::install(lua, &game_clock)`。
    let app = lua.create_table()?;
    let s = Rc::clone(&state);
    app.set(
        "quit",
        lua.create_function(move |_, ()| {
            s.lua_borrow_mut()?.quit_requested = true;
            Ok(())
        })?,
    )?;
    // `app.platform()`：宿主形态标识（`"desktop"` / `"webgl"`）。
    //
    // 为什么需要它：触屏设备没有键盘，玩法必须靠屏上摇杆与虚拟按钮；桌面/手柄则相反——
    // 常驻的虚拟控件只会盖住画面。此前 `ui/virtual_pad.lua` 只能按「宿主有没有实现
    // `input.virtual_key` / `ui.circle`」来判，而这四个 API 由本模块统一注册，桌面也全有，
    // 于是判定恒为真、桌面版照画摇杆。**能力存在 ≠ 需要触屏控件**，必须要平台标识。
    // wasm 两个平台发布变体（浏览器 / 微信小游戏）都由同一个 `game_web` 源核心编译；
    // 最终二进制可不同，但这里是同一个 wasm32 编译期常量，因此都返回 `"webgl"`。
    app.set(
        "platform",
        lua.create_function(|_, ()| {
            Ok(if cfg!(target_arch = "wasm32") {
                "webgl"
            } else {
                "desktop"
            })
        })?,
    )?;
    // `app.project_root()`：工程根目录的**绝对路径**（`set_project_root` 写入的值）。
    //
    // 为什么 Lua 需要它：`io.open` 的相对路径按**进程工作目录**解析，而工程根与工作目录
    // 并不总是一致——`game --check --project ../examples/game_jhyt` 在 `engine/` 下跑时，
    // `io.open("scenes/x.scene")` 直接返回 nil。于是"跨文件契约"类校验器（`validate_data`
    // 里那些要读 `scenes/*.scene` / `project.toml` 的规则，见 `examples/game_jhyt` 的
    // `ui/layout_check.lua` 与 `world/camera_view.lua`）会**静默跳过**全部检查而仍然打绿灯。
    // 有了这个查询，校验器就能拼绝对路径，跑在哪个目录下结果都一样。
    //
    // 路径分隔符统一成 `/`（Lua 侧字符串拼接用，Windows 的反斜杠在 Lua 模式里是转义符）。
    // 没配置工程根时返回 `""`——Web 等宿主本来也没有文件系统，调用方据此自行跳过。
    let s = Rc::clone(&state);
    app.set(
        "project_root",
        lua.create_function(move |_, ()| {
            let root = s
                .borrow()
                .project_root
                .with(|project| {
                    project
                        .path()
                        .map(|path| path.to_string_lossy().replace('\\', "/"))
                })
                .unwrap_or_default();
            Ok(root)
        })?,
    )?;
    lua.globals().set("app", app)?;

    // Generic named event bus: `event.emit(name, data?)` dispatches to every
    // callback registered with `event.on`/`event.subscribe` under `name`.
    // Subscriptions live in a per-VM map (not in `ScriptState`) because Lua
    // callbacks are owned by the VM that created them: hot reload builds a fresh
    // VM and re-runs `on_create`, which re-subscribes on the new VM.

    let event = lua.create_table()?;
    let subscribers: Rc<RefCell<BTreeMap<String, Vec<Function>>>> =
        Rc::new(RefCell::new(BTreeMap::new()));

    let subs = Rc::clone(&subscribers);
    event.set(
        "subscribe",
        lua.create_function(move |_, (name, callback): (String, Function)| {
            subs.borrow_mut().entry(name).or_default().push(callback);
            Ok(())
        })?,
    )?;
    let subs = Rc::clone(&subscribers);
    event.set(
        "on",
        lua.create_function(move |_, (name, callback): (String, Function)| {
            subs.borrow_mut().entry(name).or_default().push(callback);
            Ok(())
        })?,
    )?;

    let subs = Rc::clone(&subscribers);
    event.set(
        "emit",
        lua.create_function(move |_, (name, data): (String, Option<Value>)| {
            let data = data.unwrap_or(Value::Nil);
            // Snapshot the listener list first so a callback may itself emit,
            // subscribe, or unsubscribe without tripping the shared borrow.
            let callbacks = subs.borrow().get(&name).cloned().unwrap_or_default();
            for callback in callbacks {
                callback.call::<Value>((data.clone(),))?;
            }
            Ok(())
        })?,
    )?;

    let subs = Rc::clone(&subscribers);
    event.set(
        "unsubscribe",
        lua.create_function(move |_, (name, callback): (String, Option<Function>)| {
            let mut map = subs.borrow_mut();
            let Some(callback) = callback else {
                map.remove(&name);
                return Ok(());
            };
            if let Some(listeners) = map.get_mut(&name) {
                let target = callback.to_pointer();
                listeners.retain(|listener| listener.to_pointer() != target);
            }
            Ok(())
        })?,
    )?;
    let subs = Rc::clone(&subscribers);
    event.set(
        "off",
        lua.create_function(move |_, (name, callback): (String, Option<Function>)| {
            let mut map = subs.borrow_mut();
            let Some(callback) = callback else {
                map.remove(&name);
                return Ok(());
            };
            if let Some(listeners) = map.get_mut(&name) {
                let target = callback.to_pointer();
                listeners.retain(|listener| listener.to_pointer() != target);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("event", event)?;
    Ok(())
}
