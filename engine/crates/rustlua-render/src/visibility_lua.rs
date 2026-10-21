//! `visibility` 表（P1 ⑤-13 从脚本层搬来）。
//!
//! `Visibility` 是渲染域组件（哪些 pass 看得到这个实体），所以它的 Lua 面跟着住在
//! `rustlua-render`。`visibility.all()` 直接交 `ALL_LAYERS`，不复刻常量。

use crate::visibility::{ALL_LAYERS, Visibility};
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::{World, WorldError};
use rustlua_lua_api::{Handle, api_error};

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

/// 写操作的前置校验：实体必须还活着。
fn require_alive(world: &Handle<World>, entity: EntityId) -> LuaResult<()> {
    if world.borrow()?.is_alive(entity) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {entity}")))
    }
}

pub fn install(lua: &Lua, world: &Handle<World>) -> LuaResult<()> {
    let visibility = lua.create_table()?;
    let w = world.clone();
    visibility.set(
        "set",
        lua.create_function(move |_, (id, layers): (i64, i64)| {
            let id = entity_from_lua(id)?;
            let layers = u32::try_from(layers).map_err(|_| {
                api_error("visibility.set layers must be an integer in 0..=4294967295")
            })?;
            require_alive(&w, id)?;
            w.borrow_mut()?
                .insert(id, Visibility::new(layers))
                .map_err(world_error)?;
            Ok(layers)
        })?,
    )?;
    let w = world.clone();
    visibility.set(
        "get",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            require_alive(&w, id)?;
            Ok(w.borrow()?
                .get::<Visibility>(id)
                .map_err(world_error)?
                .map_or(ALL_LAYERS, |value| value.layers))
        })?,
    )?;
    visibility.set("all", lua.create_function(|_, ()| Ok(ALL_LAYERS))?)?;
    lua.globals().set("visibility", visibility)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_three_functions_and_roundtrips_layers() {
        let lua = Lua::new();
        let world = Handle::new(World::new(), "world");
        install(&lua, &world).unwrap();
        let table: mlua::Table = lua.globals().get("visibility").unwrap();
        assert_eq!(table.pairs::<mlua::Value, mlua::Value>().count(), 3);
        let id = world.with_mut(|world| world.spawn());
        let raw = id.to_lua_i64();
        lua.load(format!("return visibility.set({raw}, {})", 0b1011))
            .exec()
            .unwrap();
        let layers: u32 = lua
            .load(format!("return visibility.get({raw})"))
            .eval()
            .unwrap();
        assert_eq!(layers, 0b1011);
        let all: u32 = lua.load("return visibility.all()").eval().unwrap();
        assert_eq!(all, ALL_LAYERS);
    }

    #[test]
    fn dead_entity_is_rejected() {
        let lua = Lua::new();
        let world = Handle::new(World::new(), "world");
        install(&lua, &world).unwrap();
        let dead = EntityId::from_raw(999, 1).to_lua_i64();
        let message = lua
            .load(format!("return visibility.set({dead}, 1)"))
            .exec()
            .unwrap_err()
            .to_string();
        assert!(message.contains("invalid entity id"), "{message}");
    }
}
