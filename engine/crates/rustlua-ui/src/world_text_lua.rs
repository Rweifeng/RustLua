//! `world_text` 表：世界空间文字（P1 ⑤-10 从 `api/renderable.rs` 拆出）。
//!
//! 组件类型 `WorldText` 住在 `rustlua-ui`，所以它的 Lua 面也在这里。原先它与 sprite/mesh/light
//! 同处一个文件只是因为"都属于可渲染对象"，但它是 UI 的排版东西（字体度量、锚点、换行）——
//! 留在 render 会让 `render → ui` 成一条没必要的边。

use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::{World, WorldError};
use rustlua_lua_api::{Handle, api_error};

use crate::world_text::WorldText;

/// `WorldError` → Lua 错误（每个领域绑定各留三行，避免把 `WorldError` 塞进叶子契约层）。
fn world_error(error: WorldError) -> mlua::Error {
    api_error(error.to_string())
}

/// Lua 侧实体 id → 实体句柄（错误串与脚本层逐字相同）。
fn entity_from_lua(value: i64) -> LuaResult<EntityId> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    EntityId::from_lua_i64(value).ok_or_else(|| api_error(format!("invalid entity id: {value}")))
}

/// 装 `world_text` 全局表（`set` / `detach`）。
pub fn install(lua: &Lua, world: &Handle<World>) -> LuaResult<()> {
    let world_text = lua.create_table()?;
    let w = world.clone();
    world_text.set(
        "attach",
        lua.create_function(move |_, (id, component): (i64, String)| {
            let id = entity_from_lua(id)?;
            let text = WorldText::from_component_str(&component)
                .map_err(|error| api_error(error.to_string()))?;
            let mut world = w.borrow_mut()?;
            if !world.is_alive(id) {
                return Err(api_error(format!("invalid entity id: {id}")));
            }
            world.insert(id, text).map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let w = world.clone();
    world_text.set(
        "detach",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut world = w.borrow_mut()?;
            if !world.is_alive(id) {
                return Err(api_error(format!("invalid entity id: {id}")));
            }
            let removed = world
                .remove::<WorldText>(id)
                .map_err(world_error)?
                .is_some();
            Ok(removed)
        })?,
    )?;
    lua.globals().set("world_text", world_text)?;
    Ok(())
}
