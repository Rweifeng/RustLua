//! `save` 全局表的绑定（`docs/62` P1 ⑤-2：从 `rustlua-scripting` 下沉到存档域）。
//!
//! ## 为什么在这里
//!
//! 存档的格式（[`crate::save`]）与数据（[`crate::save_store::SaveStore`]）都在本 crate，
//! 绑定却一直住在 VM 宿主里——结果 `rustlua-scripting` 既要懂 Lua，又要懂"存档怎么落盘、
//! 槽位怎么命名、wasm 上走哪条通道"。现在这三件事都跟着数据回来，脚本层只调一次
//! [`install`]，把能力单元交出来。
//!
//! ## 与旧实现的差别（只有两处，语义都更强）
//!
//! 1. 借用变窄：以前 `save.write` 借的是整个 `ScriptState`（重入时报"脚本状态被重入"），
//!    现在只借 `SaveStore`，报错直指 `save`。
//! 2. 落盘期间不持锁：`commit` 先把路径/槽位/编码结果取出来，写盘时不持有任何内部借用。
//!
//! Lua 面的函数名、参数、返回值**逐字未变**（`api_surface_snapshot` 零 diff）。

use mlua::{Lua, Result as LuaResult, Value};
use rustlua_lua_api::Handle;
use rustlua_lua_api::api_error;
use rustlua_lua_api::values::{json_to_lua_value, lua_value_to_json};
use std::path::{Path, PathBuf};

use crate::save::encode as encode_save;
use crate::save_store::SaveStore;

/// 装 `save` 全局表。`store` 与宿主共享同一份状态（`Handle` 是能力单元，不是拷贝）。
pub fn install(lua: &Lua, store: &Handle<SaveStore>) -> LuaResult<()> {
    let save = lua.create_table()?;

    let s = store.clone();
    save.set(
        "write",
        lua.create_function(move |_, (key, value): (String, String)| {
            // 空键会让整份存档在下次 `load` 时无法解析（`decode` 报 empty key），
            // 因此必须在写入点就拒绝，而不是等到玩家读档时整档报废。
            if key.is_empty() {
                return Err(api_error("save.write key must not be empty"));
            }
            s.borrow_mut()?.insert(key, value);
            Ok(())
        })?,
    )?;
    let s = store.clone();
    save.set(
        "read",
        lua.create_function(move |_, key: String| Ok(s.borrow()?.get(&key).cloned()))?,
    )?;
    // 「开始新游戏」必须能丢弃**引擎侧**的持久键（`__map` / `__map:spawn:*` /
    // `__mapstate:*` / `__blob:*`）。这些键不属于任何游戏侧 provider，若残留，
    // 新档会读回上一局的地图、农田、建筑与制作状态（同进程内「继续 → 玩 →
    // 返回标题 → 新游戏」即可复现）。只清内存；磁盘文件在下一次 `commit` 时
    // 被整体替换。
    let s = store.clone();
    save.set(
        "clear",
        lua.create_function(move |_, ()| {
            let removed = s.borrow_mut()?.clear();
            tracing::info!(target: "save", "[save] cleared {removed} in-memory key(s)");
            Ok(removed)
        })?,
    )?;
    let s = store.clone();
    save.set(
        "blob_write",
        lua.create_function(move |_, (key, value): (String, Value)| {
            if key.is_empty() {
                return Err(api_error("save.blob_write key must not be empty"));
            }
            let json = lua_value_to_json(value)?;
            let encoded =
                serde_json::to_string(&json).map_err(|error| api_error(error.to_string()))?;
            s.borrow_mut()?.insert(format!("__blob:{key}"), encoded);
            Ok(())
        })?,
    )?;
    let s = store.clone();
    save.set(
        "blob_read",
        lua.create_function(move |lua, key: String| {
            let encoded = s.borrow()?.get(&format!("__blob:{key}")).cloned();
            let Some(encoded) = encoded else {
                return Ok(Value::Nil);
            };
            let json = serde_json::from_str(&encoded)
                .map_err(|error| api_error(format!("save.blob_read {key} is corrupt: {error}")))?;
            json_to_lua_value(lua, &json)
        })?,
    )?;
    let s = store.clone();
    save.set(
        "slot",
        lua.create_function(move |_, slot: Option<String>| {
            s.borrow_mut()?.set_slot(slot);
            Ok(())
        })?,
    )?;
    let s = store.clone();
    save.set(
        "slot_get",
        lua.create_function(move |lua, ()| {
            let slot = s.borrow()?.slot().map(str::to_owned);
            slot.map(|value| lua.create_string(&value).map(Value::String))
                .transpose()
        })?,
    )?;
    let s = store.clone();
    save.set(
        "commit",
        lua.create_function(move |_, ()| {
            // 一次窄借用把落点与内容都取出来：写盘期间不再持有任何内部借用。
            let prepared = {
                let store = s.borrow()?;
                store.path().map(Path::to_path_buf).map(|base| {
                    let slot = store.slot().map(str::to_owned);
                    let path = save_path_for_slot(&base, slot.as_deref());
                    (path, slot, encode_save(store.data()))
                })
            };
            let Some((path, slot, text)) = prepared else {
                tracing::warn!(target: "save", "[save] no save path configured; commit ignored");
                return Ok(false);
            };
            let result = store_save(&path, slot.as_deref(), &text);
            match &result {
                Ok(()) => tracing::info!(target: "save", path = %path.display(), "[save] committed"),
                Err(error) => {
                    tracing::error!(target: "save", path = %path.display(), error = %error, "[save] commit failed")
                }
            }
            Ok(result.is_ok())
        })?,
    )?;
    let s = store.clone();
    save.set(
        "load",
        lua.create_function(move |lua, ()| {
            let (path, slot) = {
                let store = s.borrow()?;
                (store.path().map(Path::to_path_buf), store.slot().map(str::to_owned))
            };
            let Some(base) = path else {
                tracing::warn!(target: "save", "[save] no save path configured; load ignored");
                return Ok((
                    Value::Boolean(false),
                    Value::String(lua.create_string("no_save_path")?),
                ));
            };
            let path = save_path_for_slot(&base, slot.as_deref());
            let Ok(text) = load_save(&path, slot.as_deref()) else {
                // 还没有存档不是错误；但必须与「存档已损坏」区分开，否则玩家与
                // 诊断面板无法判断进度是真的没了还是从没存过。
                return Ok((
                    Value::Boolean(false),
                    Value::String(lua.create_string("missing")?),
                ));
            };
            match crate::save::decode(&text) {
                Ok(data) => {
                    s.borrow_mut()?.replace_data(data);
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

/// Resolves the save file path for the active slot. `None` keeps the original path.
///
/// 从 `rustlua-scripting` 逐字搬来（它是"槽位 → 文件名"的唯一定义，跟着绑定走）。
fn save_path_for_slot(path: &Path, slot: Option<&str>) -> PathBuf {
    let Some(slot) = slot else {
        return path.to_path_buf();
    };
    let safe: String = slot
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "default".to_owned()
    } else {
        safe
    };
    path.with_file_name(format!("save_{safe}.sav"))
}

/// 存档落盘：**装了平台槽就走槽**，否则走文件系统。
///
/// 为什么必须分叉：wasm 上没有可写的 `std::fs`，原子写在那里必然失败——于是
/// `save.commit()` 恒返回 `false`、玩家进度永远存不下来。槽的键与旧运行时同一把
/// （`rustlua:save[_<slot>]`，见 `rustlua_infra::save_slot::storage_key`），
/// 所以切到这条通道之后**已有存档仍然读得到**。
fn store_save(path: &Path, slot: Option<&str>, text: &str) -> Result<(), String> {
    if rustlua_infra::save_slot::is_installed() {
        return rustlua_infra::save_slot::write(&rustlua_infra::save_slot::storage_key(slot), text);
    }
    atomic_write(path, text).map_err(|error| error.to_string())
}

/// 存档读回：与 [`store_save`] 同一条分叉规则（`Err` 一律按"还没有存档"处理）。
fn load_save(path: &Path, slot: Option<&str>) -> Result<String, ()> {
    if rustlua_infra::save_slot::is_installed() {
        return rustlua_infra::save_slot::read(&rustlua_infra::save_slot::storage_key(slot))
            .ok_or(());
    }
    std::fs::read_to_string(path).map_err(|_| ())
}

/// 临时文件 + rename 的原子写（与 `rustlua-scripting`、编辑器 `recovery` 同一手法）。
fn atomic_write(path: &Path, text: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    /// `save` 的 Lua 面就这 9 个：搬家到存档域之后一个都不能少
    /// （`api_surface_snapshot` 盯全量，这条盯"这一步搬全了"）。
    const NAMES: [&str; 9] = [
        "write",
        "read",
        "clear",
        "blob_write",
        "blob_read",
        "slot",
        "slot_get",
        "commit",
        "load",
    ];

    fn installed() -> (Lua, Handle<SaveStore>) {
        let lua = Lua::new();
        let store = SaveStore::handle();
        install(&lua, &store).unwrap();
        (lua, store)
    }

    #[test]
    fn every_save_function_is_installed() {
        let (lua, _store) = installed();
        let save: mlua::Table = lua.globals().get("save").unwrap();
        for name in NAMES {
            let value: Value = save.get(name).unwrap();
            assert!(
                matches!(value, Value::Function(_)),
                "`save.{name}` 缺失或不是函数：{value:?}"
            );
        }
    }

    #[test]
    fn state_lives_in_the_handle_and_clears_through_it() {
        let (lua, store) = installed();
        lua.load(
            r#"
            save.write("day", "8")
            assert(save.read("day") == "8")
            save.blob_write("map", { rows = { "a", "b" }, day = 8 })
            local blob = save.blob_read("map")
            assert(blob.rows[1] == "a" and blob.rows[2] == "b" and blob.day == 8)
            save.slot("slot2")
            assert(save.slot_get() == "slot2")
            local cleared = save.clear()
            assert(cleared >= 1, "clear 应返回清掉的键数")
            assert(save.read("day") == nil, "clear 之后读不回来")
            "#,
        )
        .exec()
        .unwrap();
        // 状态确实在能力单元里（宿主读同一份）。
        let state = store.borrow().unwrap();
        assert!(state.is_empty(), "clear 已清空: {state:?}");
    }

    #[test]
    fn empty_key_is_rejected_and_slot_empty_string_is_no_slot() {
        let (lua, store) = installed();
        let error = lua
            .load(r#"save.write("", "v")"#)
            .exec()
            .unwrap_err()
            .to_string();
        // 空键会让整份存档在下次 load 时解析失败，所以写入点就拒。
        assert!(
            error.contains("must not be empty"),
            "报错应点名空键：{error}"
        );
        lua.load(r#"save.slot("  ")"#).exec().unwrap();
        assert!(
            store.borrow().unwrap().slot().is_some(),
            "非空串应保留（原样存）"
        );
        lua.load(r#"save.slot("")"#).exec().unwrap();
        assert!(store.borrow().unwrap().slot().is_none(), "空串等于没有槽位");
    }

    #[test]
    fn without_a_path_commit_and_load_report_instead_of_failing() {
        let (lua, _store) = installed();
        let committed: bool = lua.load(r#"return save.commit()"#).eval().unwrap();
        assert!(!committed, "没配路径时 commit 返回 false");
        let (loaded, reason): (bool, String) = lua.load(r#"return save.load()"#).eval().unwrap();
        assert!(!loaded);
        assert_eq!(reason, "no_save_path", "要与「没有存档(missing)」区分开");
    }
}
