//! 世界数据：`scene`（切换）、`map`（导航与跳转）、`asset`（资源句柄）、`data`（存档型配置）。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::api_error;
use crate::script::request_scene_load;
use crate::script::values::{json_to_lua_value, lua_value_to_json};
use mlua::Result as LuaResult;
use mlua::{Lua, Value};
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let scene = lua.create_table()?;
    let s = Rc::clone(&state);
    scene.set(
        "load",
        lua.create_function(move |_, name: String| {
            let state = s.lua_borrow()?;
            let exists = request_scene_load(&state, &name);
            if exists {
                drop(state);
                s.lua_borrow_mut()?.scene_pending_load = Some(name);
            }
            Ok(exists)
        })?,
    )?;
    let s = Rc::clone(&state);
    scene.set(
        "reload",
        lua.create_function(move |_, ()| Ok(s.lua_borrow()?.scene.clone()))?,
    )?;
    lua.globals().set("scene", scene)?;

    let map = lua.create_table()?;
    let s = Rc::clone(&state);
    map.set(
        "goto",
        lua.create_function(move |_, (name, x, y): (String, Option<f32>, Option<f32>)| {
            let root = {
                let state = s.lua_borrow()?;
                state.project_root.clone()
            };
            let exists = root
                .as_ref()
                .is_some_and(|root| root.join("scenes").join(format!("{name}.scene")).is_file());
            if !exists {
                return Ok(false);
            }
            let mut state = s.lua_borrow_mut()?;
            state.save.data.insert("__map".to_owned(), name.clone());
            if let (Some(x), Some(y)) = (x, y) {
                if !x.is_finite() || !y.is_finite() {
                    return Err(api_error("map.goto spawn must be finite"));
                }
                state
                    .save
                    .data
                    .insert("__map:spawn:x".to_owned(), x.to_string());
                state
                    .save
                    .data
                    .insert("__map:spawn:y".to_owned(), y.to_string());
            }
            state.scene_pending_load = Some(name);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "go",
        lua.create_function(move |_, (name, x, y): (String, Option<f32>, Option<f32>)| {
            let root = {
                let state = s.lua_borrow()?;
                state.project_root.clone()
            };
            let exists = root
                .as_ref()
                .is_some_and(|root| root.join("scenes").join(format!("{name}.scene")).is_file());
            if !exists {
                return Ok(false);
            }
            let mut state = s.lua_borrow_mut()?;
            state.save.data.insert("__map".to_owned(), name.clone());
            if let (Some(x), Some(y)) = (x, y) {
                if !x.is_finite() || !y.is_finite() {
                    return Err(api_error("map.go spawn must be finite"));
                }
                state
                    .save
                    .data
                    .insert("__map:spawn:x".to_owned(), x.to_string());
                state
                    .save
                    .data
                    .insert("__map:spawn:y".to_owned(), y.to_string());
            }
            state.scene_pending_load = Some(name);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "enter",
        lua.create_function(move |_, (name, x, y): (String, f32, f32)| {
            let root = {
                let state = s.lua_borrow()?;
                state.project_root.clone()
            };
            let exists = root
                .as_ref()
                .is_some_and(|root| root.join("scenes").join(format!("{name}.scene")).is_file());
            if !exists {
                return Ok(false);
            }
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("map.enter spawn must be finite"));
            }
            let mut state = s.lua_borrow_mut()?;
            state.save.data.insert("__map".to_owned(), name.clone());
            state
                .save
                .data
                .insert("__map:spawn:x".to_owned(), x.to_string());
            state
                .save
                .data
                .insert("__map:spawn:y".to_owned(), y.to_string());
            state.scene_pending_load = Some(name);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "current",
        lua.create_function(move |lua, ()| {
            let state = s.lua_borrow()?;
            let current = state
                .save
                .data
                .get("__map")
                .cloned()
                .or_else(|| state.scene.clone());
            current
                .map(|value| lua.create_string(&value).map(Value::String))
                .transpose()
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "spawn",
        lua.create_function(move |lua, ()| {
            let state = s.lua_borrow()?;
            let Some(x) = state.save.data.get("__map:spawn:x") else {
                return Ok(Value::Nil);
            };
            let Some(y) = state.save.data.get("__map:spawn:y") else {
                return Ok(Value::Nil);
            };
            let Ok(x) = x.parse::<f32>() else {
                return Ok(Value::Nil);
            };
            let Ok(y) = y.parse::<f32>() else {
                return Ok(Value::Nil);
            };
            let result = lua.create_table()?;
            result.set("x", x)?;
            result.set("y", y)?;
            Ok(Value::Table(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "set_spawn",
        lua.create_function(move |_, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("map.set_spawn must be finite"));
            }
            let mut state = s.lua_borrow_mut()?;
            state
                .save
                .data
                .insert("__map:spawn:x".to_owned(), x.to_string());
            state
                .save
                .data
                .insert("__map:spawn:y".to_owned(), y.to_string());
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "state",
        lua.create_function(move |lua, (name, value): (String, Option<Value>)| {
            let key = format!("__mapstate:{name}");
            if let Some(value) = value {
                let json = lua_value_to_json(value)?;
                // B-3：注册 `map.schema(name, spec)` 后，写入会先校验，
                // 让错误在发生处暴露，而不是延迟到读档时才发现。
                {
                    let state = s.lua_borrow()?;
                    if let Some(schema) = state.map_state_schemas.get(&name)
                        && let Some(error) = rustlua_ecs::data_schema::validate(
                            &json,
                            schema,
                            &format!("map.state {name}"),
                        )
                    {
                        return Err(api_error(error));
                    }
                }
                let encoded =
                    serde_json::to_string(&json).map_err(|error| api_error(error.to_string()))?;
                s.lua_borrow_mut()?.save.data.insert(key, encoded);
                return Ok(Value::Boolean(true));
            }
            let Some(encoded) = s.lua_borrow()?.save.data.get(&key).cloned() else {
                return Ok(Value::Nil);
            };
            let json = serde_json::from_str(&encoded)
                .map_err(|error| api_error(format!("map.state {name} is corrupt: {error}")))?;
            json_to_lua_value(lua, &json)
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "state_clear",
        lua.create_function(move |_, name: Option<String>| {
            let mut state = s.lua_borrow_mut()?;
            let before = state.save.data.len();
            match &name {
                Some(name) => {
                    let key = format!("__mapstate:{name}");
                    state.save.data.remove(&key);
                }
                None => state
                    .save
                    .data
                    .retain(|key, _| !key.starts_with("__mapstate:")),
            }
            Ok(before - state.save.data.len())
        })?,
    )?;
    let s = Rc::clone(&state);
    map.set(
        "schema",
        lua.create_function(move |_, (name, spec): (String, Option<Value>)| {
            let mut state = s.lua_borrow_mut()?;
            if let Some(spec) = spec {
                let json = lua_value_to_json(spec)?;
                state.map_state_schemas.insert(name, json);
            } else {
                state.map_state_schemas.remove(&name);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("map", map)?;

    let asset = lua.create_table()?;
    let s = Rc::clone(&state);
    asset.set(
        "load",
        lua.create_function(move |_, path: String| {
            let mut state = s.lua_borrow_mut()?;
            let token = format!("asset:{}", path);
            state.assets.insert(path, token.clone());
            Ok(token)
        })?,
    )?;
    let s = Rc::clone(&state);
    asset.set(
        "release",
        lua.create_function(move |_, token: String| {
            s.lua_borrow_mut()?.assets.retain(|_, v| v != &token);
            Ok(())
        })?,
    )?;
    lua.globals().set("asset", asset)?;

    // Validates a freshly loaded data table against its registered schema
    // (`data.schema`), rejecting bad content with a located error instead of
    // silently caching it.
    fn validate_data_schema(
        state: &Rc<RefCell<ScriptState>>,
        path: &str,
        json: &serde_json::Value,
        verb: &str,
    ) -> Result<(), mlua::Error> {
        let schema = state.borrow().schema_defs.get(path).cloned();
        if let Some(schema) = schema
            && let Some(error) = rustlua_ecs::data_schema::validate(json, &schema, path)
        {
            return Err(api_error(format!(
                "data.{verb} {path}: schema violation: {error}"
            )));
        }
        Ok(())
    }

    let data = lua.create_table()?;
    let s = Rc::clone(&state);
    data.set(
        "load",
        lua.create_function(move |lua, path: String| {
            let root = {
                let state = s.lua_borrow()?;
                state
                    .project_root
                    .clone()
                    .ok_or_else(|| api_error("data.load requires a project root"))?
            };
            if let Some(cached) = s.lua_borrow()?.data_cache.get(&path) {
                return json_to_lua_value(lua, cached);
            }
            let file_path = root.join(&path);
            let bytes = rustlua_infra::vfs::read(&file_path).map_err(|error| {
                api_error(format!("data.load {}: {error}", file_path.display()))
            })?;
            let decoded = rustlua_infra::pack::decode(&bytes).map_err(|error| {
                api_error(format!("data.load {}: {error}", file_path.display()))
            })?;
            let text = String::from_utf8_lossy(&decoded);
            let json: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
                api_error(format!("data.load {}: {error}", file_path.display()))
            })?;
            validate_data_schema(&s, &path, &json, "load")?;
            s.lua_borrow_mut()?.data_cache.insert(path, json.clone());
            json_to_lua_value(lua, &json)
        })?,
    )?;
    let s = Rc::clone(&state);
    data.set(
        "reload",
        lua.create_function(move |lua, path: String| {
            s.lua_borrow_mut()?.data_cache.remove(&path);
            let root = {
                let state = s.lua_borrow()?;
                state
                    .project_root
                    .clone()
                    .ok_or_else(|| api_error("data.reload requires a project root"))?
            };
            let file_path = root.join(&path);
            let bytes = rustlua_infra::vfs::read(&file_path).map_err(|error| {
                api_error(format!("data.reload {}: {error}", file_path.display()))
            })?;
            let decoded = rustlua_infra::pack::decode(&bytes).map_err(|error| {
                api_error(format!("data.reload {}: {error}", file_path.display()))
            })?;
            let text = String::from_utf8_lossy(&decoded);
            let json: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
                api_error(format!("data.reload {}: {error}", file_path.display()))
            })?;
            validate_data_schema(&s, &path, &json, "reload")?;
            s.lua_borrow_mut()?.data_cache.insert(path, json.clone());
            json_to_lua_value(lua, &json)
        })?,
    )?;
    let s = Rc::clone(&state);
    data.set(
        "cache_clear",
        lua.create_function(move |_, ()| {
            s.lua_borrow_mut()?.data_cache.clear();
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    data.set(
        "schema",
        lua.create_function(move |_, (path, spec): (String, Option<Value>)| {
            let mut state = s.lua_borrow_mut()?;
            if let Some(spec) = spec {
                let json = lua_value_to_json(spec)?;
                state.schema_defs.insert(path.clone(), json);
                // A schema change must not be masked by a stale cached table.
                state.data_cache.remove(&path);
            } else {
                state.schema_defs.remove(&path);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("data", data)?;
    Ok(())
}
