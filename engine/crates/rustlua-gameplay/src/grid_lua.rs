//! `grid` 表的 Lua 绑定（P1 ⑤-14 从脚本层搬来）。
//!
//! 纯网格换算：世界坐标 ↔ 格、朝向格、边界判定。**没有状态**，也不碰物理——所以它跟着
//! 网格数学一起住在 gameplay，而不是跟着 `tilemap` 去物理域（`tilemap.set_solid` 要重建刚体，
//! 那才是物理的事）。
//!
//! 校验口径与 `tilemap` 那张表一致：`tile_size` 必须有限且为正，否则报同一句错。

use crate::grid;
use crate::grid::Facing;
use mlua::{Lua, Result as LuaResult, Table};
use rustlua_lua_api::api_error;

/// 瓦片尺寸必须有限且为正（`tilemap.*` 与 `grid.*` 共用的口径，两条表各自校验一次）。
fn require_positive_tile_size(tile_size: f32) -> LuaResult<()> {
    if !tile_size.is_finite() || tile_size <= 0.0 {
        return Err(api_error("tile_size must be finite and > 0"));
    }
    Ok(())
}

fn table_vec(lua: &Lua, x: f32, y: f32) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set(1, x)?;
    table.set(2, y)?;
    Ok(table)
}

fn table_i32_pair(lua: &Lua, x: i32, y: i32) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set(1, x)?;
    table.set(2, y)?;
    Ok(table)
}

pub fn install(lua: &Lua) -> LuaResult<()> {
    let grid_api = lua.create_table()?;
    grid_api.set(
        "world_to_cell",
        lua.create_function(move |lua, (x, y, tile_size): (f32, f32, f32)| {
            require_positive_tile_size(tile_size)?;
            let (cx, cy) = grid::world_to_cell(x, y, tile_size);
            table_i32_pair(lua, cx, cy)
        })?,
    )?;
    grid_api.set(
        "cell_to_world",
        lua.create_function(move |lua, (cx, cy, tile_size): (i32, i32, f32)| {
            require_positive_tile_size(tile_size)?;
            let pos = grid::cell_to_world(cx, cy, tile_size);
            table_vec(lua, pos.x, pos.y)
        })?,
    )?;
    grid_api.set(
        "facing_cell",
        lua.create_function(
            move |lua, (x, y, facing, tile_size): (f32, f32, String, f32)| {
                require_positive_tile_size(tile_size)?;
                let (cx, cy) = grid::facing_cell(x, y, Facing::parse(&facing), tile_size);
                table_i32_pair(lua, cx, cy)
            },
        )?,
    )?;
    grid_api.set(
        "in_bounds",
        lua.create_function(
            move |_, (cx, cy, width, height): (i32, i32, usize, usize)| {
                Ok(grid::in_bounds(cx, cy, width, height))
            },
        )?,
    )?;
    lua.globals().set("grid", grid_api)?;
    Ok(())
}
