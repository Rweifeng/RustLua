//! 与实体无关的全局设施：日志、时间与日历、应用生命周期、事件总线。
//!
//! Lua 侧的 `log` / `time` / `clock` / `calendar` / `day` / `app` / `event`。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::call_optional_day_hook;
use crate::script::error::api_error;
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
        lua.create_function(move |_, ()| Ok(s.borrow().delta_seconds))?,
    )?;
    lua.globals().set("time", time)?;

    let clock = lua.create_table()?;
    let s = Rc::clone(&state);
    clock.set(
        "now",
        lua.create_function(move |lua, ()| {
            let clock = &s.borrow().game_clock;
            let now = lua.create_table()?;
            now.set("day", clock.day as i64)?;
            now.set("minute", clock.minute as i64)?;
            now.set("hour", (clock.minute / 60) as i64)?;
            now.set("minute_of_day", clock.minute as i64)?;
            now.set("rate", clock.rate)?;
            now.set("paused", clock.paused)?;
            Ok(now)
        })?,
    )?;
    let s = Rc::clone(&state);
    clock.set(
        "set_rate",
        lua.create_function(move |_, rate: f32| {
            if !rate.is_finite() || rate < 0.0 {
                return Err(api_error("clock.set_rate must be finite and non-negative"));
            }
            let mut state = s.lua_borrow_mut()?;
            state.game_clock.rate = rate;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    clock.set(
        "pause",
        lua.create_function(move |_, paused: bool| {
            s.borrow_mut().game_clock.paused = paused;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    clock.set(
        "add",
        lua.create_function(move |_, minutes: f32| {
            if !minutes.is_finite() {
                return Err(api_error("clock.add minutes must be finite"));
            }
            s.borrow_mut().game_clock.add_minutes(minutes);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    clock.set(
        "set",
        lua.create_function(move |_, (day, minute): (i64, i64)| {
            if day < 1 || minute < 0 {
                return Err(api_error("clock.set day must be >= 1 and minute >= 0"));
            }
            let mut state = s.lua_borrow_mut()?;
            state.game_clock.set(
                day as u32,
                minute.min((rustlua_gameplay::gameplay::MINUTES_PER_DAY - 1) as i64) as u32,
            );
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    clock.set(
        "sleep_to_morning",
        lua.create_function(move |_, ()| {
            let day = s.borrow_mut().game_clock.sleep_to_morning();
            Ok(day as i64)
        })?,
    )?;
    lua.globals().set("clock", clock)?;

    // `calendar` derives season/year from the absolute in-game day (M16-08).

    let calendar = lua.create_table()?;
    let s = Rc::clone(&state);
    calendar.set(
        "now",
        lua.create_function(move |lua, ()| {
            let clock = &s.borrow().game_clock;
            let now = lua.create_table()?;
            now.set("absolute_day", clock.day as i64)?;
            now.set("day", clock.day_of_season() as i64)?;
            now.set("season", clock.season() as i64)?;
            now.set("year", clock.year() as i64)?;
            now.set("minute", clock.minute as i64)?;
            Ok(now)
        })?,
    )?;
    let s = Rc::clone(&state);
    calendar.set(
        "set",
        lua.create_function(
            move |_, (day, season, year, minute): (i64, i64, i64, i64)| {
                let days = rustlua_gameplay::gameplay::DAYS_PER_SEASON as i64;
                if day < 1 || day > days {
                    return Err(api_error(format!("calendar.set day must be in 1..={days}")));
                }
                if !(1..=rustlua_gameplay::gameplay::SEASONS_PER_YEAR as i64).contains(&season) {
                    return Err(api_error("calendar.set season must be in 1..=4"));
                }
                if year < 1 {
                    return Err(api_error("calendar.set year must be >= 1"));
                }
                let absolute = (year - 1) * rustlua_gameplay::gameplay::DAYS_PER_YEAR as i64
                    + (season - 1) * days
                    + day;
                let mut state = s.lua_borrow_mut()?;
                state
                    .game_clock
                    .set_absolute(absolute as u32, minute.max(0) as u32);
                Ok(())
            },
        )?,
    )?;
    lua.globals().set("calendar", calendar)?;

    // `day` lifecycle: sleep to the next morning, firing `on_day_end` then
    // `on_day_start` (M16-08). Gameplay registers those globals for settlement.

    let day = lua.create_table()?;
    let s = Rc::clone(&state);
    day.set(
        "sleep",
        lua.create_function(move |lua, ()| {
            call_optional_day_hook(lua, "on_day_end")?;
            let new_day = {
                let mut state = s.lua_borrow_mut()?;
                state.game_clock.sleep_to_morning()
            };
            call_optional_day_hook(lua, "on_day_start")?;
            Ok(new_day as i64)
        })?,
    )?;
    let s = Rc::clone(&state);
    day.set(
        "now",
        lua.create_function(move |lua, ()| {
            let clock = &s.borrow().game_clock;
            let now = lua.create_table()?;
            now.set("day", clock.day as i64)?;
            now.set("minute", clock.minute as i64)?;
            Ok(now)
        })?,
    )?;
    lua.globals().set("day", day)?;

    let app = lua.create_table()?;
    let s = Rc::clone(&state);
    app.set(
        "quit",
        lua.create_function(move |_, ()| {
            s.borrow_mut().quit_requested = true;
            Ok(())
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
                .as_ref()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
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
