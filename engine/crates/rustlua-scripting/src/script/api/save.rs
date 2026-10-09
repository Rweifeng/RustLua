//! `save`：存档位读写与 blob 持久化（Lua 表 ↔ JSON 的转换在 `super::values`）。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::api_error;
use crate::script::require::atomic_write;
use crate::script::save_path_for_slot;
use crate::script::values::{json_to_lua_value, lua_value_to_json};
use mlua::Result as LuaResult;
use mlua::{Lua, Value};
use rustlua_gameplay::save::encode as encode_save;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let save = lua.create_table()?;
    let s = Rc::clone(&state);
    save.set(
        "write",
        lua.create_function(move |_, (key, value): (String, String)| {
            // 空键会让整份存档在下次 `load` 时无法解析（`decode` 报 empty key），
            // 因此必须在写入点就拒绝，而不是等到玩家读档时整档报废。
            if key.is_empty() {
                return Err(api_error("save.write key must not be empty"));
            }
            s.lua_borrow_mut()?.save.data.insert(key, value);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "read",
        lua.create_function(move |_, key: String| {
            Ok(s.lua_borrow()?.save.data.get(&key).cloned())
        })?,
    )?;
    // 「开始新游戏」必须能丢弃**引擎侧**的持久键（`__map` / `__map:spawn:*` /
    // `__mapstate:*` / `__blob:*`）。这些键不属于任何游戏侧 provider，若残留，
    // 新档会读回上一局的地图、农田、建筑与制作状态（同进程内「继续 → 玩 →
    // 返回标题 → 新游戏」即可复现）。只清内存；磁盘文件在下一次 `commit` 时
    // 被整体替换。
    let s = Rc::clone(&state);
    save.set(
        "clear",
        lua.create_function(move |_, ()| {
            let mut state = s.lua_borrow_mut()?;
            let removed = state.save.data.len();
            state.save.data.clear();
            tracing::info!(target: "save", "[save] cleared {removed} in-memory key(s)");
            Ok(removed)
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "blob_write",
        lua.create_function(move |_, (key, value): (String, Value)| {
            if key.is_empty() {
                return Err(api_error("save.blob_write key must not be empty"));
            }
            let json = lua_value_to_json(value)?;
            let encoded =
                serde_json::to_string(&json).map_err(|error| api_error(error.to_string()))?;
            s.lua_borrow_mut()?
                .save
                .data
                .insert(format!("__blob:{key}"), encoded);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "blob_read",
        lua.create_function(move |lua, key: String| {
            let state = s.lua_borrow()?;
            let Some(encoded) = state.save.data.get(&format!("__blob:{key}")) else {
                return Ok(Value::Nil);
            };
            let json = serde_json::from_str(encoded)
                .map_err(|error| api_error(format!("save.blob_read {key} is corrupt: {error}")))?;
            json_to_lua_value(lua, &json)
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "slot",
        lua.create_function(move |_, slot: Option<String>| {
            s.lua_borrow_mut()?.save.slot = slot.filter(|value| !value.is_empty());
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "slot_get",
        lua.create_function(move |lua, ()| {
            let slot = s.lua_borrow()?.save.slot.clone();
            slot.map(|value| lua.create_string(&value).map(Value::String))
                .transpose()
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "commit",
        lua.create_function(move |_, ()| {
            let state = s.lua_borrow()?;
            let Some(path) = state.save.path.clone() else {
                tracing::warn!(target: "save", "[save] no save path configured; commit ignored");
                return Ok(false);
            };
            let path = save_path_for_slot(&path, state.save.slot.as_deref());
            let text = encode_save(&state.save.data);
            let result = store_save(&path, state.save.slot.as_deref(), &text);
            match &result {
                Ok(()) => tracing::info!(target: "save", path = %path.display(), "[save] committed"),
                Err(error) => {
                    tracing::error!(target: "save", path = %path.display(), error = %error, "[save] commit failed")
                }
            }
            Ok(result.is_ok())
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "load",
        lua.create_function(move |lua, ()| {
            let (path, slot) = {
                let state = s.lua_borrow()?;
                (state.save.path.clone(), state.save.slot.clone())
            };
            let Some(path) = path else {
                tracing::warn!(target: "save", "[save] no save path configured; load ignored");
                return Ok((
                    Value::Boolean(false),
                    Value::String(lua.create_string("no_save_path")?),
                ));
            };
            let path = save_path_for_slot(&path, slot.as_deref());
            let Ok(text) = load_save(&path, slot.as_deref()) else {
                // 还没有存档不是错误；但必须与「存档已损坏」区分开，否则玩家与
                // 诊断面板无法判断进度是真的没了还是从没存过。
                return Ok((
                    Value::Boolean(false),
                    Value::String(lua.create_string("missing")?),
                ));
            };
            match rustlua_gameplay::save::decode(&text) {
                Ok(data) => {
                    s.lua_borrow_mut()?.save.data = data;
                    tracing::info!(target: "save", path = %path.display(), "[save] loaded");
                    Ok((Value::Boolean(true), Value::Nil))
                }
                Err(error) => {
                    // 损坏/版本不符：保留原文件（绝不覆盖），并把原因交给调用方。
                    tracing::warn!(target: "save", path = %path.display(), error = %error, "[save] corrupt save ignored");
                    let reason = format!("corrupt: {error}");
                    Ok((
                        Value::Boolean(false),
                        Value::String(lua.create_string(&reason)?),
                    ))
                }
            }
        })?,
    )?;
    lua.globals().set("save", save)?;

    Ok(())
}

/// 存档落盘：**装了平台槽就走槽**，否则走文件系统。
///
/// 为什么必须分叉：wasm 上没有可写的 `std::fs`，`atomic_write` 在那里必然失败——于是
/// `save.commit()` 恒返回 `false`、玩家进度永远存不下来（web 的旧运行时里那段注释写的
/// 就是这件事）。槽的键与旧运行时同一把（`rustlua:save[_<slot>]`，见
/// [`rustlua_infra::save_slot::storage_key`]），所以切到这条通道之后**已有存档仍然读得到**。
fn store_save(path: &std::path::Path, slot: Option<&str>, text: &str) -> Result<(), String> {
    if rustlua_infra::save_slot::is_installed() {
        return rustlua_infra::save_slot::write(&rustlua_infra::save_slot::storage_key(slot), text);
    }
    atomic_write(path, text).map_err(|error| error.to_string())
}

/// 存档读回：与 [`store_save`] 同一条分叉规则（`Err` 一律按"还没有存档"处理，与原来
/// `read_to_string` 失败同语义）。
fn load_save(path: &std::path::Path, slot: Option<&str>) -> Result<String, ()> {
    if rustlua_infra::save_slot::is_installed() {
        return rustlua_infra::save_slot::read(&rustlua_infra::save_slot::storage_key(slot))
            .ok_or(());
    }
    std::fs::read_to_string(path).map_err(|_| ())
}
