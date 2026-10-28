//! `ui` 表的运行时控件取值面：`toggle` / `slider` / `progress` / `scrollbar`。
//!
//! 四个函数同形：`ui.slider(entity_id [, value])`——不传 `value` 是读，传了就写；
//! 返回值是**写完之后**的当前值（读的时候就是现值）。实体没有对应组件时读返回 `nil`、
//! 写报错（"静默什么都没发生"是脚本最难查的一类错）。
//!
//! 值的写入走组件自己的 `set_*`（夹取/整数吸附），单选组走 `set_toggle_on`，
//! 与鼠标/键盘改值共用同一套语义。

use crate::ui_progress::Progress;
use crate::ui_scrollbar::ScrollBar;
use crate::ui_slider::Slider;
use crate::ui_toggle::{Toggle, set_toggle_on};
use mlua::Lua;
use mlua::Result as LuaResult;
use mlua::Table;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::{World, WorldError};
use rustlua_lua_api::{Handle, api_error};

fn world_error(error: WorldError) -> mlua::Error {
    api_error(error.to_string())
}

fn entity_from_lua(value: i64) -> LuaResult<EntityId> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    EntityId::from_lua_i64(value).ok_or_else(|| api_error(format!("invalid entity id: {value}")))
}

fn require_alive(world: &World, id: EntityId) -> LuaResult<()> {
    if world.is_alive(id) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {id}")))
    }
}

fn finite(value: f32, what: &str) -> LuaResult<f32> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(api_error(format!("{what} must be finite")))
    }
}

pub fn install(lua: &Lua, table: Table, world: &Handle<World>) -> LuaResult<()> {
    let ui = table;

    let w = world.clone();
    ui.set(
        "toggle",
        lua.create_function(move |_, (id, value): (i64, Option<bool>)| {
            let id = entity_from_lua(id)?;
            let mut world = w.borrow_mut()?;
            require_alive(&world, id)?;
            let Some(current) = world.get::<Toggle>(id).map_err(world_error)? else {
                return match value {
                    None => Ok(None),
                    Some(_) => Err(api_error(format!("entity {id} has no Toggle"))),
                };
            };
            let Some(on) = value else {
                return Ok(Some(current.is_on));
            };
            set_toggle_on(&mut world, id, on);
            let now = world
                .get::<Toggle>(id)
                .map_err(world_error)?
                .map(|toggle| toggle.is_on);
            Ok(now)
        })?,
    )?;

    let w = world.clone();
    ui.set(
        "slider",
        lua.create_function(move |_, (id, value): (i64, Option<f32>)| {
            let id = entity_from_lua(id)?;
            let mut world = w.borrow_mut()?;
            require_alive(&world, id)?;
            let Some(slider) = world.get_mut::<Slider>(id).map_err(world_error)? else {
                return match value {
                    None => Ok(None),
                    Some(_) => Err(api_error(format!("entity {id} has no Slider"))),
                };
            };
            if let Some(value) = value {
                slider.set_value(finite(value, "slider value")?);
            }
            Ok(Some(slider.value))
        })?,
    )?;

    let w = world.clone();
    ui.set(
        "progress",
        lua.create_function(move |_, (id, value): (i64, Option<f32>)| {
            let id = entity_from_lua(id)?;
            let mut world = w.borrow_mut()?;
            require_alive(&world, id)?;
            let Some(progress) = world.get_mut::<Progress>(id).map_err(world_error)? else {
                return match value {
                    None => Ok(None),
                    Some(_) => Err(api_error(format!("entity {id} has no Progress"))),
                };
            };
            if let Some(value) = value {
                progress.set_value(finite(value, "progress value")?);
            }
            Ok(Some(progress.value))
        })?,
    )?;

    let w = world.clone();
    ui.set(
        "scrollbar",
        lua.create_function(move |_, (id, value): (i64, Option<f32>)| {
            let id = entity_from_lua(id)?;
            let mut world = w.borrow_mut()?;
            require_alive(&world, id)?;
            let Some(scrollbar) = world.get_mut::<ScrollBar>(id).map_err(world_error)? else {
                return match value {
                    None => Ok(None),
                    Some(_) => Err(api_error(format!("entity {id} has no Scrollbar"))),
                };
            };
            if let Some(value) = value {
                scrollbar.set_value(finite(value, "scrollbar value")?);
            }
            Ok(Some(scrollbar.value))
        })?,
    )?;

    Ok(())
}
