//! 瓦片地图：`tilemap`（逐格碰撞/瓦片读写）与 `grid`（网格寻路、朝向）。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::{api_error, world_error};
use crate::script::refresh_tilemap_physics;
use crate::script::require_positive_tile_size;
use crate::script::values::{
    entity_from_lua, entity_to_lua, require_alive, table_i32_pair, table_vec,
};
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_ecs::tilemap::DEFAULT_TILESET as DEFAULT_TILESET_PATH;
use rustlua_ecs::tilemap::Tilemap;
use rustlua_ecs::transform::Transform;
use rustlua_ecs::transform::Vec2;
use rustlua_gameplay::grid;
use rustlua_gameplay::grid::Facing;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let tilemap = lua.create_table()?;
    let s = Rc::clone(&state);
    tilemap.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Ok(mlua::Value::Nil);
            };
            let t = lua.create_table()?;
            t.set("tile_size", map.tile_size.x)?;
            t.set("width", map.width)?;
            t.set("height", map.height)?;
            t.set("texture", map.texture.clone())?;
            // M72-01/02: the atlas layout is part of the tilemap's public contract —
            // gates and data-driven art scripts must be able to read (and check) how
            // cell ids address the tileset, not just which texture they sample.
            t.set("tiles_format", map.tiles_format)?;
            t.set("atlas_cols", map.atlas_cols)?;
            t.set("atlas_rows", map.atlas_rows)?;
            t.set("tile_kinds", map.tile_kinds)?;
            t.set("variant_count", map.variant_count)?;
            t.set("autotile", map.autotile)?;
            t.set("variant_seed", map.variant_seed)?;
            t.set("max_tile_id", map.max_tile_id())?;
            Ok(mlua::Value::Table(t))
        })?,
    )?;
    // `tilemap.set_atlas(id, cols, rows)` declares how many columns/rows the
    // tileset packs. Explicit rather than derived from the texture size: a
    // resized PNG must never silently re-map every cell.
    let s = Rc::clone(&state);
    tilemap.set(
        "set_atlas",
        lua.create_function(move |_, (id, cols, rows): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            let map = state
                .world
                .get_mut::<Tilemap>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no tilemap"))?;
            map.atlas_cols = cols;
            map.atlas_rows = rows;
            map.validate_layout()
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;
    // `tilemap.set_kinds(id, kinds, variant_count, autotile?, seed?)` switches a
    // map between direct atlas indexing (M72-01) and logical-tile resolution
    // (M72-02). Cells keep storing ids; only their meaning changes:
    //   * `variant_count == 1 && !autotile` -> id **is** the atlas entry index.
    //   * otherwise -> id is a logical kind (1..=kinds) resolved per cell by
    //     variant hash and neighbour mask.
    let s = Rc::clone(&state);
    tilemap.set(
        "set_kinds",
        lua.create_function(
            move |_,
                  (id, kinds, variants, autotile, seed): (
                i64,
                usize,
                usize,
                Option<bool>,
                Option<u32>,
            )| {
                let id = entity_from_lua(id)?;
                let mut state = s.lua_borrow_mut()?;
                require_alive(&state, id)?;
                let map = state
                    .world
                    .get_mut::<Tilemap>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no tilemap"))?;
                map.tile_kinds = kinds;
                map.variant_count = variants;
                map.autotile = autotile.unwrap_or(false);
                if let Some(seed) = seed {
                    map.variant_seed = seed;
                }
                map.validate_layout()
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(())
            },
        )?,
    )?;
    // `tilemap.atlas_index(id, x, y)` -> the tileset entry a cell samples this
    // frame (`0` = nothing drawn). Read-only view of the resolution the
    // renderer uses, so gates can assert determinism without reading pixels.
    let s = Rc::clone(&state);
    tilemap.set(
        "atlas_index",
        lua.create_function(move |_, (id, x, y): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            Ok(map.resolve_atlas_index(x, y))
        })?,
    )?;
    // `tilemap.autotile_mask(id, x, y)` -> 4-bit neighbour mask (see `TileNeighbor`).
    let s = Rc::clone(&state);
    tilemap.set(
        "autotile_mask",
        lua.create_function(move |_, (id, x, y): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            Ok(map.autotile_mask(x, y))
        })?,
    )?;
    // `tilemap.set_tint(id, x, y, r, g, b, a?)` paints one cell's vertex color
    // (season/moisture wash and shadow darkening, M72-02/06);
    // `tilemap.clear_tint(id, x, y)` untints it again.
    let s = Rc::clone(&state);
    tilemap.set(
        "set_tint",
        lua.create_function(
            move |_, (id, x, y, r, g, b, a): (i64, usize, usize, f32, f32, f32, Option<f32>)| {
                let id = entity_from_lua(id)?;
                let mut state = s.lua_borrow_mut()?;
                require_alive(&state, id)?;
                let map = state
                    .world
                    .get_mut::<Tilemap>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no tilemap"))?;
                let color = [r, g, b, a.unwrap_or(1.0)];
                if map.tint_at(x, y) != color {
                    // A cell reuses the map's existing entry rather than growing
                    // the palette with a duplicate of an equal color.
                    let tint_id = map
                        .register_tint(color)
                        .map_err(|error| api_error(error.to_string()))?;
                    map.set_tint(x, y, tint_id)
                        .map_err(|error| api_error(error.to_string()))?;
                }
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "clear_tint",
        lua.create_function(move |_, (id, x, y): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            let map = state
                .world
                .get_mut::<Tilemap>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no tilemap"))?;
            map.set_tint(x, y, 0)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "tint",
        lua.create_function(move |lua, (id, x, y): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            let [r, g, b, a] = map.tint_at(x, y);
            let t = lua.create_table()?;
            t.set("r", r)?;
            t.set("g", g)?;
            t.set("b", b)?;
            t.set("a", a)?;
            Ok(mlua::Value::Table(t))
        })?,
    )?;
    // `tilemap.set_texture(id, texture)` swaps the tileset a Tilemap samples.
    // Needed for data-driven art variants (e.g. per-season ground tilesets):
    // the renderer re-reads `Tilemap::texture` every frame and rebuilds the
    // layer when the component changes, so the swap is visible next frame.
    let s = Rc::clone(&state);
    tilemap.set(
        "set_texture",
        lua.create_function(move |_, (id, texture): (i64, String)| {
            let id = entity_from_lua(id)?;
            if texture.trim().is_empty() {
                return Err(api_error("tilemap texture must not be empty"));
            }
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            let map = state
                .world
                .get_mut::<Tilemap>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no tilemap"))?;
            map.texture = texture;
            Ok(())
        })?,
    )?;
    // `tilemap.create(width, height, tile_size, texture?, name?)` spawns a new
    // entity with an empty Tilemap component, so farmland grids can be authored
    // and rebuilt at runtime (M16-04) and survive scene-less/headless runs.
    let s = Rc::clone(&state);
    tilemap.set(
        "create",
        lua.create_function(
            move |_,
                  (width, height, tile_size, texture, name): (
                usize,
                usize,
                f32,
                Option<String>,
                Option<String>,
            )| {
                let mut state = s.lua_borrow_mut()?;
                let mut map = Tilemap::new(Vec2::new(tile_size, tile_size), width, height)
                    .map_err(|error| api_error(error.to_string()))?;
                map.texture = texture.unwrap_or_else(|| DEFAULT_TILESET_PATH.to_owned());
                let id = state.world.spawn();
                state
                    .world
                    .insert(id, Transform::default())
                    .map_err(world_error)?;
                state.world.insert(id, map).map_err(world_error)?;
                if let Some(name) = name
                    && !name.is_empty()
                {
                    state.entity_state.names.insert(name, id);
                }
                Ok(entity_to_lua(id))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "is_solid",
        lua.create_function(move |_, (id, x, y): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            Ok(map.is_solid(x, y).unwrap_or(false))
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "get_cell",
        lua.create_function(move |_, (id, x, y): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            Ok(map.cell(x, y))
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "set_cell",
        lua.create_function(move |_, (id, x, y, value): (i64, usize, usize, u8)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            let map = state
                .world
                .get_mut::<Tilemap>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no tilemap"))?;
            map.set_cell(x, y, value)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "set_solid",
        lua.create_function(move |_, (id, x, y, solid): (i64, usize, usize, bool)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            {
                let map = state
                    .world
                    .get_mut::<Tilemap>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no tilemap"))?;
                map.set_solid(x, y, solid)
                    .map_err(|error| api_error(error.to_string()))?;
            }
            refresh_tilemap_physics(&mut state, id);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "get_meta",
        lua.create_function(move |_, (id, x, y, key): (i64, usize, usize, String)| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            Ok(map.get_meta(x, y, &key).map(str::to_owned))
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "set_meta",
        lua.create_function(
            move |_, (id, x, y, key, value): (i64, usize, usize, String, String)| {
                let id = entity_from_lua(id)?;
                let mut state = s.lua_borrow_mut()?;
                require_alive(&state, id)?;
                let map = state
                    .world
                    .get_mut::<Tilemap>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no tilemap"))?;
                map.set_meta(x, y, key, value)
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(())
            },
        )?,
    )?;
    // `tilemap.find_path(id, from_x, from_y, to_x, to_y, diagonal?)` returns a
    // table of `{x=, y=}` cells (including both endpoints) or `nil, reason`.
    let s = Rc::clone(&state);
    tilemap.set(
        "find_path",
        lua.create_function(
            move |lua,
                  (id, from_x, from_y, to_x, to_y, diagonal): (
                i64,
                usize,
                usize,
                usize,
                usize,
                Option<bool>,
            )| {
                let id = entity_from_lua(id)?;
                let state = s.lua_borrow()?;
                require_alive(&state, id)?;
                let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                    return Err(api_error("entity has no tilemap"));
                };
                match map.find_path((from_x, from_y), (to_x, to_y), diagonal.unwrap_or(false)) {
                    Ok(cells) => {
                        let table = lua.create_table()?;
                        for (index, (x, y)) in cells.into_iter().enumerate() {
                            let cell = lua.create_table()?;
                            cell.set("x", x)?;
                            cell.set("y", y)?;
                            table.raw_set(index + 1, cell)?;
                        }
                        Ok((mlua::Value::Table(table), mlua::Value::Nil))
                    }
                    Err(error) => Ok((
                        mlua::Value::Nil,
                        mlua::Value::String(lua.create_string(error.to_string())?),
                    )),
                }
            },
        )?,
    )?;
    lua.globals().set("tilemap", tilemap)?;

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
