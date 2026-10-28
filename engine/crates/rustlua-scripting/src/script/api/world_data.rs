//! 场景切换与地图导航：`scene.*`、`map.goto/go/enter/current`。
//!
//! ## 已经搬走的部分（`docs/62` P1 ⑤-4c）
//!
//! `asset.*`、`data.*`、`map.state/state_clear/schema/spawn/set_spawn` 属于**工程数据面**，
//! 已下沉到 [`rustlua_gameplay::world_data_lua`]（状态在 `WorldDataState`，持久化在
//! `SaveStore`）。本模块只留"切场景"这一半：
//!
//! - `scene.load/reload` 读写 `scene_pending_load` / 当前场景名；
//! - `map.goto/go/enter` 在**校验场景文件存在**之后排队切场景（顺带写 `__map` 与出生点）；
//! - `map.current` 的兜底是"当前场景名"，同样属于场景态。
//!
//! 这条边界不是按行数划的：数据面不需要知道场景在哪个文件、下一帧切到哪，
//! 场景面也不该知道数据表 schema 怎么校验。`map` 是一张共享表——数据面由
//! `world_data_lua::install` 填，导航面在这里填（与 `ui` 的三段式同一个手法）。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::api_error;
use crate::script::request_scene_load;
use mlua::Result as LuaResult;
use mlua::{Lua, Value};
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let map = lua.create_table()?;
    // 数据面（asset/data 两张全局表 + map 的持久状态函数）由领域 crate 装：
    // 本模块只把三个能力单元的句柄交出去，不再认识数据表格式。
    {
        let state = state.borrow();
        crate::script::domain::gameplay::world_data_lua::install(
            lua,
            &map,
            &state.world_data,
            &state.save,
            &state.project_root,
        )?;
    }

    let scene = lua.create_table()?;
    let s = Rc::clone(&state);
    scene.set(
        "load",
        lua.create_function(move |_, name: String| {
            let exists = {
                let state = s.lua_borrow_mut()?;
                request_scene_load(&state, &name)
            };
            if exists {
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

    let s = Rc::clone(&state);
    map.set(
        "goto",
        lua.create_function(move |_, (name, x, y): (String, Option<f32>, Option<f32>)| {
            let exists = {
                let state = s.lua_borrow_mut()?;
                crate::script::scene_file_exists(&state, &name)
            };
            if !exists {
                return Ok(false);
            }
            let mut state = s.lua_borrow_mut()?;
            state
                .save
                .borrow_mut()?
                .insert("__map".to_owned(), name.clone());
            if let (Some(x), Some(y)) = (x, y) {
                if !x.is_finite() || !y.is_finite() {
                    return Err(api_error("map.goto spawn must be finite"));
                }
                state
                    .save
                    .borrow_mut()?
                    .insert("__map:spawn:x".to_owned(), x.to_string());
                state
                    .save
                    .borrow_mut()?
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
            let exists = {
                let state = s.lua_borrow_mut()?;
                crate::script::scene_file_exists(&state, &name)
            };
            if !exists {
                return Ok(false);
            }
            let mut state = s.lua_borrow_mut()?;
            state
                .save
                .borrow_mut()?
                .insert("__map".to_owned(), name.clone());
            if let (Some(x), Some(y)) = (x, y) {
                if !x.is_finite() || !y.is_finite() {
                    return Err(api_error("map.go spawn must be finite"));
                }
                state
                    .save
                    .borrow_mut()?
                    .insert("__map:spawn:x".to_owned(), x.to_string());
                state
                    .save
                    .borrow_mut()?
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
            let exists = {
                let state = s.lua_borrow_mut()?;
                crate::script::scene_file_exists(&state, &name)
            };
            if !exists {
                return Ok(false);
            }
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("map.enter spawn must be finite"));
            }
            let mut state = s.lua_borrow_mut()?;
            state
                .save
                .borrow_mut()?
                .insert("__map".to_owned(), name.clone());
            state
                .save
                .borrow_mut()?
                .insert("__map:spawn:x".to_owned(), x.to_string());
            state
                .save
                .borrow_mut()?
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
                .borrow()?
                .get("__map")
                .cloned()
                .or_else(|| state.scene.clone());
            current
                .map(|value| lua.create_string(&value).map(Value::String))
                .transpose()
        })?,
    )?;
    lua.globals().set("map", map)?;
    Ok(())
}
