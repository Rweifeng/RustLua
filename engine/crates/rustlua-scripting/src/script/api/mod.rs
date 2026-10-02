//! Lua API 的分模块注册（WP11 阶段 2，由原 `register_api` 一整块拆出）。
//!
//! 每个子模块一个 `register(lua, state)`，把自己负责的 Lua 全局模块表装进 `lua`。
//! 模块表名字互不相同、且注册期不互相读取，所以子模块之间的先后不影响行为。
//! 唯一的例外是 `ui`：它有 45 个函数，按职责拆成 `ui` / `ui_panel` / `ui_nav` 三段共享同一张
//! 表——表在本文件建好、三段填完，最后才 `globals().set("ui", …)`。

use std::{cell::RefCell, rc::Rc};

use mlua::{Lua, Result as LuaResult};

use super::ScriptState;

mod animation;
mod audio;
mod camera;
mod entity;
mod globals;
mod input;
mod inventory;
mod physics;
mod renderable;
mod save;
mod tilemap;
mod transform;
mod ui;
mod ui_nav;
mod ui_panel;
mod world_data;

/// 装配全部 Lua 全局模块表。`LuaRuntime` 每建一个 VM 调用一次。
pub(super) fn register_all(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    globals::register(lua, Rc::clone(&state))?;
    input::register(lua, Rc::clone(&state))?;
    entity::register(lua, Rc::clone(&state))?;
    renderable::register(lua, Rc::clone(&state))?;
    transform::register(lua, Rc::clone(&state))?;
    camera::register(lua, Rc::clone(&state))?;
    tilemap::register(lua, Rc::clone(&state))?;
    animation::register(lua, Rc::clone(&state))?;
    physics::register(lua, Rc::clone(&state))?;
    audio::register(lua, Rc::clone(&state))?;
    world_data::register(lua, Rc::clone(&state))?;
    // `ui` 表由三段共享：mlua 的 `Table` 是引用语义，克隆只是多一个句柄。
    let ui = lua.create_table()?;
    ui::register(lua, ui.clone(), Rc::clone(&state))?;
    ui_panel::register(lua, ui.clone(), Rc::clone(&state))?;
    ui_nav::register(lua, ui.clone(), Rc::clone(&state))?;
    lua.globals().set("ui", ui)?;
    inventory::register(lua, Rc::clone(&state))?;
    save::register(lua, Rc::clone(&state))?;
    Ok(())
}
