//! 实体身份与其上的轻量组件：`entity` / `tag` / `health` / `visibility` / `interact`。
//!
//! Lua 侧句柄是 `index << 32 | generation` 编码的整数，解码与校验集中在 `super::values`。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::UiText;
use crate::script::error::{api_error, world_error};
use crate::script::values::{entity_from_lua, entity_to_lua, require_alive};
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_ecs::transform::Transform;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let entity = lua.create_table()?;
    let s = Rc::clone(&state);
    entity.set(
        "spawn",
        lua.create_function(
            move |_, (name, persist_id): (Option<String>, Option<String>)| {
                let mut state = s.lua_borrow_mut()?;
                let id = state.world.spawn();
                state
                    .world
                    .insert(id, Transform::default())
                    .map_err(world_error)?;
                if let Some(name) = name {
                    state.entity_state.names.insert(name, id);
                }
                if let Some(persist) = persist_id.filter(|value| !value.is_empty()) {
                    if state
                        .entity_state
                        .persist
                        .get(&persist)
                        .is_some_and(|existing| state.world.is_alive(*existing))
                    {
                        return Err(api_error(format!("persist_id already in use: {persist}")));
                    }
                    state.entity_state.persist.insert(persist, id);
                }
                Ok(entity_to_lua(id))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "find",
        lua.create_function(move |_, name: String| {
            Ok(s.borrow()
                .entity_state
                .names
                .get(&name)
                .copied()
                .map(entity_to_lua))
        })?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "find_id",
        lua.create_function(move |_, persist_id: String| {
            Ok(s.borrow()
                .entity_state
                .persist
                .get(&persist_id)
                .copied()
                .map(entity_to_lua))
        })?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "persist_id",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            Ok(s.borrow()
                .entity_state
                .persist
                .iter()
                .find(|(_, entity)| **entity == id)
                .map(|(key, _)| key.clone()))
        })?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "is_alive",
        lua.create_function(move |_, id: i64| Ok(s.borrow().world.is_alive(entity_from_lua(id)?)))?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "destroy",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            state.world.despawn(id).map_err(world_error)?;
            state.entity_state.names.retain(|_, value| *value != id);
            state.entity_state.persist.retain(|_, value| *value != id);
            state.entity_state.health.remove(&id);
            let _ = state.physics.world2d.remove_body(id);
            let _ = state.physics.world3d.remove_body(id);
            Ok(true)
        })?,
    )?;
    lua.globals().set("entity", entity)?;

    let health = lua.create_table()?;
    let s = Rc::clone(&state);
    health.set(
        "get",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            Ok(state.entity_state.health.get(&id).copied().unwrap_or(0.0))
        })?,
    )?;
    let s = Rc::clone(&state);
    health.set(
        "set",
        lua.create_function(move |_, (id, value): (i64, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            if !value.is_finite() || value < 0.0 {
                return Err(api_error("health must be finite and non-negative"));
            }
            state.entity_state.health.insert(id, value);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    health.set(
        "damage",
        lua.create_function(move |_, (id, amount): (i64, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            if !amount.is_finite() || amount < 0.0 {
                return Err(api_error("damage must be finite and non-negative"));
            }
            let value = state.entity_state.health.entry(id).or_insert(0.0);
            *value = (*value - amount).max(0.0);
            Ok(*value)
        })?,
    )?;
    lua.globals().set("health", health)?;

    let tag = lua.create_table()?;
    let s = Rc::clone(&state);
    tag.set(
        "get",
        lua.create_function(move |_, (id, key): (i64, String)| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            Ok(state
                .entity_state
                .tags
                .get(&id)
                .and_then(|bag| bag.get(&key))
                .cloned())
        })?,
    )?;
    let s = Rc::clone(&state);
    tag.set(
        "set",
        lua.create_function(move |_, (id, key, value): (i64, String, String)| {
            let id = entity_from_lua(id)?;
            if key.is_empty() {
                return Err(api_error("tag key must not be empty"));
            }
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            if value.is_empty() {
                if let Some(bag) = state.entity_state.tags.get_mut(&id) {
                    bag.remove(&key);
                    if bag.is_empty() {
                        state.entity_state.tags.remove(&id);
                    }
                }
            } else {
                state
                    .entity_state
                    .tags
                    .entry(id)
                    .or_default()
                    .insert(key, value);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("tag", tag)?;

    let interact = lua.create_table()?;
    let s = Rc::clone(&state);
    interact.set(
        "query",
        lua.create_function(move |lua, (x, y, radius): (f32, f32, f32)| {
            if !radius.is_finite() || radius < 0.0 {
                return Err(api_error("interact radius must be finite and non-negative"));
            }
            let state = s.lua_borrow()?;
            let mut hits: Vec<(f32, i32, i64, String, i32)> = Vec::new();
            for id in state.world.entities_with::<Transform>() {
                let Some(tags) = state.entity_state.tags.get(&id) else {
                    continue;
                };
                if !(tags.contains_key("interact") || tags.contains_key("kind")) {
                    continue;
                }
                let Some(t) = state.world.get::<Transform>(id).map_err(world_error)? else {
                    continue;
                };
                let dx = t.position.x - x;
                let dy = t.position.y - y;
                let dist_sq = dx * dx + dy * dy;
                if dist_sq > radius * radius {
                    continue;
                }
                let kind = tags
                    .get("kind")
                    .cloned()
                    .or_else(|| tags.get("interact").cloned())
                    .unwrap_or_default();
                let priority = tags
                    .get("priority")
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(0);
                hits.push((dist_sq, -priority, entity_to_lua(id), kind, priority));
            }
            hits.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.cmp(&b.1))
            });
            let result = lua.create_table()?;
            for (index, (_, _, id, kind, priority)) in hits.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("id", id)?;
                entry.set("kind", kind)?;
                entry.set("priority", priority)?;
                result.set(index + 1, entry)?;
            }
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    interact.set(
        "prompt",
        lua.create_function(move |_, text: String| {
            let mut state = s.lua_borrow_mut()?;
            state.entity_state.interact_prompt = Some(text.clone());
            state.ui_state.texts.insert(
                "__interact_prompt".to_owned(),
                UiText {
                    text,
                    visible: true,
                    position: [16.0, 48.0],
                    scale: 1.0,
                    font_size: Some(18.0),
                },
            );
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    interact.set(
        "clear_prompt",
        lua.create_function(move |_, ()| {
            let mut state = s.lua_borrow_mut()?;
            state.entity_state.interact_prompt = None;
            state.ui_state.texts.remove("__interact_prompt");
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    interact.set(
        "get_prompt",
        lua.create_function(move |_, ()| Ok(s.borrow().entity_state.interact_prompt.clone()))?,
    )?;
    lua.globals().set("interact", interact)?;

    let visibility = lua.create_table()?;
    let s = Rc::clone(&state);
    visibility.set(
        "set",
        lua.create_function(move |_, (id, layers): (i64, i64)| {
            let id = entity_from_lua(id)?;
            let layers = u32::try_from(layers).map_err(|_| {
                api_error("visibility.set layers must be an integer in 0..=4294967295")
            })?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            state
                .world
                .insert(id, rustlua_render::visibility::Visibility::new(layers))
                .map_err(world_error)?;
            Ok(layers)
        })?,
    )?;
    let s = Rc::clone(&state);
    visibility.set(
        "get",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            Ok(state
                .world
                .get::<rustlua_render::visibility::Visibility>(id)
                .map_err(world_error)?
                .map_or(rustlua_render::visibility::ALL_LAYERS, |value| value.layers))
        })?,
    )?;
    visibility.set(
        "all",
        lua.create_function(|_, ()| Ok(rustlua_render::visibility::ALL_LAYERS))?,
    )?;
    lua.globals().set("visibility", visibility)?;

    Ok(())
}
