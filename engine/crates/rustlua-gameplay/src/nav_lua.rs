//! `nav` 表的 Lua 绑定：网格创建、阻挡标记与平滑路径查询。
//!
//! 绑定只是 [`rustlua_navigation`] 的薄壳，网格状态按名字登记在注册表捕获的能力单元里。
//! 名字而不是裸 id 是刻意的：热重载会重建 VM，脚本重新执行 `nav.create_grid("arena", …)`
//! 时应当**替换**同一张网格，而不是每 reload 一次就漏一张。
//!
//! 坐标口径与引擎一致：格子 `(x, y)` 对应 3D 战场的 XZ 平面，`cell_center` 的 `y` 就是
//! 世界 `z`。`find_world_path` 返回 `{x=, z=}`，可直接喂给 `transform3d.set`。
//!
//! 每张网格自带一个流场缓存（[`FlowFieldCache`]）：`set_flow_goal` 定目标，
//! `update_flow` 每帧按预算推进，`flow_vector` 给格里单位一个朝目标的世界方向。
//! 动态障碍走独立的 `set_dynamic_blocked` 覆盖层，`clear_dynamic` 能整层清掉而不动静态地图。

use mlua::{Lua, Result as LuaResult, Table};
use rustlua_ecs::transform::Vec2;
use rustlua_lua_api::{Handle, api_error};
use rustlua_navigation::{FlowFieldCache, FlowStatus, NavError, NavGrid};
use std::collections::BTreeMap;

/// 一张具名网格与它的流场缓存。
struct NavSlot {
    grid: NavGrid,
    flow: FlowFieldCache,
}

/// 注册表捕获的导航网格集合。
#[derive(Default)]
pub struct NavState {
    slots: Vec<Option<NavSlot>>,
    names: BTreeMap<String, usize>,
}

impl NavState {
    /// 创建或替换一张具名网格，返回 1 基 id。
    fn upsert(&mut self, name: String, grid: NavGrid) -> i64 {
        let slot = NavSlot {
            grid,
            flow: FlowFieldCache::new(),
        };
        if let Some(index) = self.names.get(&name).copied() {
            self.slots[index] = Some(slot);
            return index as i64 + 1;
        }
        let index = match self.slots.iter().position(Option::is_none) {
            Some(index) => index,
            None => {
                self.slots.push(None);
                self.slots.len() - 1
            }
        };
        self.slots[index] = Some(slot);
        self.names.insert(name, index);
        index as i64 + 1
    }

    fn slot(&self, id: i64) -> Result<&NavSlot, String> {
        self.slots
            .get(id_index(id)?)
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("unknown navigation grid id: {id}"))
    }

    fn slot_mut(&mut self, id: i64) -> Result<&mut NavSlot, String> {
        self.slots
            .get_mut(id_index(id)?)
            .and_then(Option::as_mut)
            .ok_or_else(|| format!("unknown navigation grid id: {id}"))
    }

    fn get(&self, id: i64) -> Result<&NavGrid, String> {
        self.slot(id).map(|slot| &slot.grid)
    }

    fn get_mut(&mut self, id: i64) -> Result<&mut NavGrid, String> {
        self.slot_mut(id).map(|slot| &mut slot.grid)
    }

    fn remove(&mut self, id: i64) -> Result<bool, String> {
        let index = id_index(id)?;
        let Some(slot) = self.slots.get_mut(index) else {
            return Ok(false);
        };
        if slot.take().is_none() {
            return Ok(false);
        }
        self.names.retain(|_, named| *named != index);
        Ok(true)
    }
}

fn id_index(id: i64) -> Result<usize, String> {
    if id <= 0 {
        return Err(format!("invalid navigation grid id: {id}"));
    }
    Ok((id - 1) as usize)
}

/// `update_flow` 不传预算时一次推进的格数上限：一张 64×64 的图一帧建完。
const DEFAULT_FLOW_BUDGET: usize = 4096;

fn flow_status_name(status: FlowStatus) -> &'static str {
    match status {
        FlowStatus::Idle => "idle",
        FlowStatus::Building => "building",
        FlowStatus::Ready => "ready",
        FlowStatus::Unreachable => "unreachable",
    }
}

/// 建一个导航网格能力单元（宿主不需要它；注册表捕获同一份）。
#[must_use]
pub fn state() -> Handle<NavState> {
    Handle::new(NavState::default(), "nav")
}

fn nav_error(error: NavError) -> mlua::Error {
    api_error(error.to_string())
}

fn string_error(error: String) -> mlua::Error {
    api_error(error)
}

fn index(value: i64, what: &str) -> LuaResult<usize> {
    usize::try_from(value).map_err(|_| {
        api_error(format!(
            "{what} must be a non-negative integer (got {value})"
        ))
    })
}

/// 把 `NavGrid::find_path` 的格子路径转成 Lua 数组。
fn cells_to_table(lua: &Lua, cells: &[(usize, usize)]) -> LuaResult<Table> {
    let out = lua.create_table_with_capacity(cells.len(), 0)?;
    for (index, (x, y)) in cells.iter().enumerate() {
        let cell = lua.create_table_with_capacity(0, 2)?;
        cell.set("x", *x)?;
        cell.set("y", *y)?;
        out.set(index + 1, cell)?;
    }
    Ok(out)
}

/// 把世界路径转成 `{x=, z=}` 数组（导航平面的 `y` 映射到 3D 的 `z`）。
fn points_to_table(lua: &Lua, points: &[Vec2]) -> LuaResult<Table> {
    let out = lua.create_table_with_capacity(points.len(), 0)?;
    for (index, point) in points.iter().enumerate() {
        let point_table = lua.create_table_with_capacity(0, 2)?;
        point_table.set("x", point.x)?;
        point_table.set("z", point.y)?;
        out.set(index + 1, point_table)?;
    }
    Ok(out)
}

/// 装 `nav` 全局表。
pub fn install(lua: &Lua, state: &Handle<NavState>) -> LuaResult<()> {
    let nav = lua.create_table()?;

    let s = state.clone();
    nav.set(
        "create_grid",
        lua.create_function(
            move |_,
                  (name, width, height, cell_size, origin_x, origin_z): (
                String,
                i64,
                i64,
                f64,
                f64,
                f64,
            )| {
                if name.trim().is_empty() {
                    return Err(api_error("navigation grid name must not be empty"));
                }
                let grid = NavGrid::with_origin(
                    index(width, "width")?,
                    index(height, "height")?,
                    cell_size as f32,
                    Vec2::new(origin_x as f32, origin_z as f32),
                )
                .map_err(nav_error)?;
                let id = s.borrow_mut()?.upsert(name, grid);
                Ok(id)
            },
        )?,
    )?;

    let s = state.clone();
    nav.set(
        "destroy_grid",
        lua.create_function(move |_, id: i64| s.borrow_mut()?.remove(id).map_err(string_error))?,
    )?;

    let s = state.clone();
    nav.set(
        "grid_info",
        lua.create_function(move |lua, id: i64| {
            let state = s.borrow()?;
            let grid = state.get(id).map_err(string_error)?;
            let out = lua.create_table_with_capacity(0, 6)?;
            out.set("width", grid.width())?;
            out.set("height", grid.height())?;
            out.set("cell_size", grid.cell_size())?;
            out.set("origin_x", grid.origin().x)?;
            out.set("origin_z", grid.origin().y)?;
            Ok(out)
        })?,
    )?;

    let s = state.clone();
    nav.set(
        "set_blocked",
        lua.create_function(move |_, (id, x, y, blocked): (i64, i64, i64, bool)| {
            let mut state = s.borrow_mut()?;
            let grid = state.get_mut(id).map_err(string_error)?;
            grid.set_blocked(index(x, "x")?, index(y, "y")?, blocked)
                .map_err(nav_error)
        })?,
    )?;

    let s = state.clone();
    nav.set(
        "find_path",
        lua.create_function(
            move |lua, (id, sx, sy, gx, gy, diagonal): (i64, i64, i64, i64, i64, Option<bool>)| {
                let state = s.borrow()?;
                let grid = state.get(id).map_err(string_error)?;
                let cells = grid
                    .find_path(
                        (index(sx, "start_x")?, index(sy, "start_y")?),
                        (index(gx, "goal_x")?, index(gy, "goal_y")?),
                        diagonal.unwrap_or(false),
                    )
                    .map_err(nav_error)?;
                cells_to_table(lua, &cells)
            },
        )?,
    )?;

    let s = state.clone();
    nav.set(
        "find_world_path",
        lua.create_function(
            move |lua, (id, sx, sz, gx, gz, diagonal): (i64, f64, f64, f64, f64, Option<bool>)| {
                let state = s.borrow()?;
                let grid = state.get(id).map_err(string_error)?;
                let start = Vec2::new(sx as f32, sz as f32);
                let goal = Vec2::new(gx as f32, gz as f32);
                let path = grid
                    .find_world_path(start, goal, diagonal.unwrap_or(false))
                    .map_err(nav_error)?;
                points_to_table(lua, path.points())
            },
        )?,
    )?;

    let s = state.clone();
    nav.set(
        "set_dynamic_blocked",
        lua.create_function(move |_, (id, x, y, blocked): (i64, i64, i64, bool)| {
            let mut state = s.borrow_mut()?;
            let grid = state.get_mut(id).map_err(string_error)?;
            grid.set_dynamic_blocked(index(x, "x")?, index(y, "y")?, blocked)
                .map_err(nav_error)
        })?,
    )?;

    let s = state.clone();
    nav.set(
        "clear_dynamic",
        lua.create_function(move |_, id: i64| {
            let mut state = s.borrow_mut()?;
            let grid = state.get_mut(id).map_err(string_error)?;
            grid.clear_dynamic();
            Ok(())
        })?,
    )?;

    let s = state.clone();
    nav.set(
        "set_flow_goal",
        lua.create_function(move |_, (id, gx, gz): (i64, Option<f64>, Option<f64>)| {
            let mut state = s.borrow_mut()?;
            let slot = state.slot_mut(id).map_err(string_error)?;
            match (gx, gz) {
                (None, None) => {
                    slot.flow.set_goal(None);
                    Ok(())
                }
                (Some(gx), Some(gz)) => {
                    let goal = slot
                        .grid
                        .world_to_cell(Vec2::new(gx as f32, gz as f32))
                        .map_err(nav_error)?;
                    slot.flow.set_goal(Some(goal));
                    Ok(())
                }
                _ => Err(api_error(
                    "flow goal requires both gx and gz (pass neither to clear)",
                )),
            }
        })?,
    )?;

    let s = state.clone();
    nav.set(
        "update_flow",
        lua.create_function(move |_, (id, budget): (i64, Option<i64>)| {
            let budget = match budget {
                Some(value) if value < 0 => {
                    return Err(api_error(format!(
                        "flow budget must be a non-negative integer (got {value})"
                    )));
                }
                Some(value) => usize::try_from(value).unwrap_or(usize::MAX),
                None => DEFAULT_FLOW_BUDGET,
            };
            let mut state = s.borrow_mut()?;
            let slot = state.slot_mut(id).map_err(string_error)?;
            let NavSlot { grid, flow } = slot;
            Ok(flow_status_name(flow.update(grid, budget)))
        })?,
    )?;

    let s = state.clone();
    nav.set(
        "flow_vector",
        lua.create_function(move |lua, (id, x, z): (i64, f64, f64)| {
            let state = s.borrow()?;
            let slot = state.slot(id).map_err(string_error)?;
            let Some(direction) = slot
                .flow
                .direction_at(&slot.grid, Vec2::new(x as f32, z as f32))
            else {
                return Ok(None);
            };
            let out = lua.create_table_with_capacity(0, 2)?;
            out.set("x", direction.x)?;
            out.set("z", direction.y)?;
            Ok(Some(out))
        })?,
    )?;

    lua.globals().set("nav", nav)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    fn lua_with_nav() -> Lua {
        let lua = Lua::new();
        install(&lua, &state()).expect("install nav");
        lua
    }

    #[test]
    fn lua_script_creates_grid_blocks_cells_and_finds_smoothed_path() {
        let lua = lua_with_nav();
        let result: (i64, bool, bool, bool, f64, f64) = lua
            .load(
                r#"
                local id = nav.create_grid("arena", 7, 5, 1.0, 0.0, 0.0)
                for y = 0, 3 do nav.set_blocked(id, 3, y, true) end
                local cells = nav.find_path(id, 0, 0, 6, 0, true)
                local world = nav.find_world_path(id, 0.5, 0.5, 6.5, 0.5, true)
                local info = nav.grid_info(id)
                return id, #cells > 0, #world > 0,
                    info.width == 7 and info.height == 5,
                    world[1].x, world[#world].x
                "#,
            )
            .eval()
            .expect("script");
        assert_eq!(result.0, 1);
        assert!(result.1 && result.2 && result.3);
        assert_eq!(result.4, 0.5);
        assert_eq!(result.5, 6.5);
    }

    #[test]
    fn named_grid_is_replaced_on_reload_instead_of_leaking() {
        let lua = lua_with_nav();
        let (first, second, width, destroyed): (i64, i64, usize, bool) = lua
            .load(
                r#"
                local a = nav.create_grid("arena", 3, 3, 1.0, 0.0, 0.0)
                local b = nav.create_grid("arena", 4, 4, 1.0, 0.0, 0.0)
                local width = nav.grid_info(b).width
                return a, b, width, nav.destroy_grid(b)
                "#,
            )
            .eval()
            .expect("script");
        assert_eq!(first, second);
        assert_eq!(width, 4);
        assert!(destroyed);
    }

    #[test]
    fn blocking_the_only_corridor_reports_an_error() {
        let lua = lua_with_nav();
        let message: String = lua
            .load(
                r#"
                local id = nav.create_grid("maze", 3, 3, 1.0, 0.0, 0.0)
                for y = 0, 2 do nav.set_blocked(id, 1, y, true) end
                local ok, err = pcall(nav.find_path, id, 0, 1, 2, 1, false)
                return tostring(err)
                "#,
            )
            .eval()
            .expect("script");
        assert!(message.contains("no route"), "{message}");
    }

    #[test]
    fn invalid_ids_and_coordinates_are_lua_errors_not_panics() {
        let lua = lua_with_nav();
        let (bad_id, bad_cell): (String, String) = lua
            .load(
                r#"
                local _, id_err = pcall(nav.find_path, 99, 0, 0, 1, 1, false)
                local id = nav.create_grid("g", 2, 2, 1.0, 0.0, 0.0)
                local _, cell_err = pcall(nav.set_blocked, id, 9, 9, true)
                return tostring(id_err), tostring(cell_err)
                "#,
            )
            .eval()
            .expect("script");
        assert!(bad_id.contains("unknown navigation grid id"), "{bad_id}");
        assert!(bad_cell.contains("out of bounds"), "{bad_cell}");
    }

    #[test]
    fn dynamic_obstacles_clear_without_losing_static_walls() {
        let lua = lua_with_nav();
        let (detour, sealed, reopened, wall_kept): (bool, String, bool, String) = lua
            .load(
                r#"
                local id = nav.create_grid("yard", 7, 7, 1.0, 0.0, 0.0)
                for y = 0, 4 do nav.set_blocked(id, 3, y, true) end
                nav.set_dynamic_blocked(id, 3, 5, true)
                local detour = #nav.find_path(id, 0, 5, 6, 5, false) > 0
                nav.set_dynamic_blocked(id, 3, 6, true)
                local _, err = pcall(nav.find_path, id, 0, 5, 6, 5, false)
                nav.clear_dynamic(id)
                local reopened = #nav.find_path(id, 0, 5, 6, 5, false) > 0
                local _, wall_err = pcall(nav.find_path, id, 3, 0, 4, 0, false)
                return detour, tostring(err), reopened, tostring(wall_err)
                "#,
            )
            .eval()
            .expect("script");
        assert!(detour);
        assert!(sealed.contains("no route"), "{sealed}");
        assert!(reopened);
        assert!(wall_kept.contains("blocked"), "{wall_kept}");
    }

    #[test]
    fn flow_field_builds_over_budget_and_reports_direction() {
        let lua = lua_with_nav();
        let (early, before, ready, dx, dz, on_goal, cleared): (
            String,
            bool,
            String,
            f64,
            f64,
            bool,
            String,
        ) = lua
            .load(
                r#"
                local id = nav.create_grid("flow", 7, 7, 1.0, 0.0, 0.0)
                nav.set_flow_goal(id, 0.5, 0.5)
                local early = nav.update_flow(id, 0)
                local before = nav.flow_vector(id, 6.5, 6.5) == nil
                local ready = nav.update_flow(id, 4096)
                local direction = nav.flow_vector(id, 6.5, 6.5)
                local on_goal = nav.flow_vector(id, 0.5, 0.5) == nil
                nav.set_flow_goal(id)
                local cleared = nav.update_flow(id)
                return early, before, ready, direction.x, direction.z, on_goal, cleared
                "#,
            )
            .eval()
            .expect("script");
        assert_eq!(early, "building");
        assert!(
            before,
            "no direction may be served before the field is ready"
        );
        assert_eq!(ready, "ready");
        assert!(dx < 0.0 && dz < 0.0, "direction should point at the goal");
        assert!((dx * dx + dz * dz - 1.0).abs() < 1e-5);
        assert!(on_goal, "standing on the goal has no direction");
        assert_eq!(cleared, "idle");
    }

    #[test]
    fn flow_errors_name_the_problem() {
        let lua = lua_with_nav();
        let (half_goal, bad_budget, out_of_bounds, unknown): (String, String, String, String) = lua
            .load(
                r#"
                local id = nav.create_grid("e", 3, 3, 1.0, 0.0, 0.0)
                local _, half = pcall(nav.set_flow_goal, id, 1.0)
                local _, budget = pcall(nav.update_flow, id, -5)
                local _, far = pcall(nav.set_flow_goal, id, 99.0, 99.0)
                local _, missing = pcall(nav.update_flow, 42)
                return tostring(half), tostring(budget), tostring(far), tostring(missing)
                "#,
            )
            .eval()
            .expect("script");
        assert!(half_goal.contains("both gx and gz"), "{half_goal}");
        assert!(bad_budget.contains("non-negative"), "{bad_budget}");
        assert!(out_of_bounds.contains("out of bounds"), "{out_of_bounds}");
        assert!(unknown.contains("unknown navigation grid id"), "{unknown}");
    }

    #[test]
    fn replacing_a_named_grid_resets_its_flow_field() {
        let lua = lua_with_nav();
        let (before, after): (String, String) = lua
            .load(
                r#"
                local id = nav.create_grid("arena", 5, 5, 1.0, 0.0, 0.0)
                nav.set_flow_goal(id, 0.5, 0.5)
                local before = nav.update_flow(id, 4096)
                local replaced = nav.create_grid("arena", 5, 5, 1.0, 0.0, 0.0)
                local after = nav.update_flow(replaced)
                return before, after
                "#,
            )
            .eval()
            .expect("script");
        assert_eq!(before, "ready");
        assert_eq!(
            after, "idle",
            "a replaced grid must not inherit the old goal"
        );
    }
}
