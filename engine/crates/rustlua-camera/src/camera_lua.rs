//! 相机绑定：`camera`（兼容层，读写透视 payload）、`camera3d`（射线/投影）、`camera2d`
//! （控制器 + 视口换算）。P1 ⑤-8 从脚本层整块下沉到这里。
//!
//! ## 一条选择链、两个投影
//!
//! `camera.set_active` 只认"带 `Camera` 组件的实体"，由它的 `projection` 决定服务哪个 pass
//! （透视 → 3D world，正交 → 2D overlay）——[`Camera`] 枚举就是这条链的实现。选中项本身
//! 存在 [`CameraState`] 里（会话态，不是实体数据）。
//!
//! ## 谁是谁的权威
//!
//! - **相机组件**（position/target/fov/center/zoom/bounds）在 ECS 里，绑定直接读写 `World`；
//! - **控制器**（跟随/偏移/死区/平滑/抖动）与**选中项**在 [`CameraState`] 里；
//! - `camera2d.set`/`set_zoom` 只是**登记请求**、由宿主下一帧消费；`get`/`get_zoom` 读宿主
//!   回填的真实值，不会返回脚本的自述；
//! - `camera2d.screen_to_world` / `camera3d.ray` 这些必须用宿主每帧回填的**已烘焙快照**
//!   （`view_2d`/`view_3d`）——没渲染过一帧就明确报错，不猜默认相机。
//!
//! ## Lua 面
//!
//! 与下沉前的 `script/api/camera.rs` **逐字一致**：`camera.set_active/get_active/get/set`、
//! `camera3d.ray/mouse_ray/pick_plane/world_to_screen/get`、`camera2d.set/get/set_zoom/get_zoom/
//! screen_to_world/world_to_screen/mouse_world/shake/follow/unfollow`，共 **19 个**函数；
//! 报错文本、返回值与"没渲染过一帧"的口径都不变（`api_surface_snapshot` 零 diff）。

use mlua::Lua;
use mlua::Result as LuaResult;
use mlua::Table;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::{World, WorldError};
use rustlua_lua_api::Handle;
use rustlua_lua_api::api_error;
use rustlua_math::math3d::Vec3;
use std::cell::Cell;
use std::rc::Rc;

use crate::camera::{Camera, Camera2D, Camera3D, Viewport};
use crate::camera_state::CameraState;
use crate::picking;

/// 输入侧共享的视口指针单元（Registry::pointer_cell 的类型）。
pub type PointerCell = Rc<Cell<Option<(f32, f32)>>>;

/// 装 `camera` / `camera3d` / `camera2d` 三张全局表。
///
/// `pointer` 是输入侧共享的视口指针单元（`Registry::pointer_cell`）：`camera3d.mouse_ray`
/// 与 `camera2d.mouse_world` 读它，因此它必须与 `input.cursor_position()` 是同一个 `Cell`。
pub fn install(
    lua: &Lua,
    state: &Handle<CameraState>,
    world: &Handle<World>,
    pointer: PointerCell,
) -> LuaResult<()> {
    install_camera(lua, state, world)?;
    install_camera3d(lua, state, world, Rc::clone(&pointer))?;
    install_camera2d(lua, state, world, pointer)?;
    Ok(())
}

// ── 共享助手（与脚本层 `values` 同一口径，逐字保留错误串） ─────────────────────────

fn world_error(error: WorldError) -> mlua::Error {
    api_error(error.to_string())
}

/// 严格解码：写操作的前置校验用它（改动一个不存在的实体是 bug，要当场报出来）。
fn entity_from_lua(value: i64) -> LuaResult<EntityId> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    EntityId::from_lua_i64(value).ok_or_else(|| api_error(format!("invalid entity id: {value}")))
}

fn require_alive(world: &World, entity: EntityId) -> LuaResult<()> {
    if world.is_alive(entity) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {entity}")))
    }
}

fn table_vec(lua: &Lua, x: f32, y: f32) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set(1, x)?;
    table.set(2, y)?;
    Ok(table)
}

/// 屏幕像素 → 世界射线（六个分量：ox,oy,oz,dx,dy,dz）——可直接喂给 `physics3d.raycast`。
fn table_ray(lua: &Lua, origin: Vec3, direction: Vec3) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set("ox", origin.x)?;
    table.set("oy", origin.y)?;
    table.set("oz", origin.z)?;
    table.set("dx", direction.x)?;
    table.set("dy", direction.y)?;
    table.set("dz", direction.z)?;
    Ok(table)
}

/// 取宿主每帧回填的 (2D 相机, 视口)。**还没有渲染过任何一帧时报错**而不是猜一个默认值：
/// 猜出来的世界坐标会让"点击没反应/点错位置"变成一桩无头案。
fn view_2d(state: &CameraState) -> LuaResult<(Camera2D, Viewport)> {
    let (camera, viewport) = state.view_2d().ok_or_else(|| {
        api_error("camera2d screen/world conversion needs one rendered frame first")
    })?;
    let viewport =
        Viewport::new(viewport[0], viewport[1]).map_err(|error| api_error(error.to_string()))?;
    Ok((camera, viewport))
}

/// 取宿主每帧回填的 (3D 相机, 视口)。还没渲染过任何一帧时报错——与 2D 同样的理由：
/// 猜一台相机做射线，会让"点不中/点错"变成无头案。
fn view_3d(state: &CameraState) -> LuaResult<(Camera3D, Viewport)> {
    let (camera, viewport) = state.view_3d().ok_or_else(|| {
        api_error("camera3d ray/screen conversion needs one rendered frame first")
    })?;
    let viewport =
        Viewport::new(viewport[0], viewport[1]).map_err(|error| api_error(error.to_string()))?;
    Ok((camera, viewport))
}

/// 借句柄取 2D 快照。写成短助手而不是 `view_2d_of(&s)?`：后者会被解析成
/// `(&s.borrow())?`（`&` 比 `?` 结合得紧），正是 ⑤-7 记下的第二个坑。
fn view_2d_of(state: &Handle<CameraState>) -> LuaResult<(Camera2D, Viewport)> {
    let state = state.borrow()?;
    view_2d(&state)
}

/// 借句柄取 3D 快照（理由同上）。
fn view_3d_of(state: &Handle<CameraState>) -> LuaResult<(Camera3D, Viewport)> {
    let state = state.borrow()?;
    view_3d(&state)
}
// ── camera：显式选中 + 透视 payload 读写 ──────────────────────────────────────────

fn install_camera(lua: &Lua, state: &Handle<CameraState>, world: &Handle<World>) -> LuaResult<()> {
    let camera = lua.create_table()?;
    let s = state.clone();
    let w = world.clone();
    camera.set(
        "set_active",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            // 拆分批 9：**只有一条选择链**——认任何带 `Camera` 的实体，由它的 `projection` 决定
            // 服务哪个 pass（透视 → 3D world，正交 → 2D overlay）。
            //
            // 与批 9 之前的行为差别（已在 CHANGELOG 的「兼容契约（变更）」里逐条记录）：
            // ① 选中**正交**相机不再报 `entity has no camera3d`，而是成为 2D 相机的来源
            //    （宿主采用它的载荷作为 2D 取景基准）；② 对**完全没有相机**的实体，错误串由
            //    `entity has no camera3d` 改为 `entity has no camera`（那句话在"一条选择链"下
            //    才是真的）。`camera.get` / `camera.set` 是**透视 payload** 的读写，仍保持原错误串。
            let world = w.borrow()?;
            require_alive(&world, id)?;
            if world.get::<Camera>(id).map_err(world_error)?.is_none() {
                return Err(api_error("entity has no camera"));
            }
            drop(world);
            s.borrow_mut()?.set_active(Some(id));
            Ok(true)
        })?,
    )?;
    let s = state.clone();
    camera.set(
        "get_active",
        lua.create_function(move |_, ()| {
            let active = s.borrow()?.active().map(EntityId::to_lua_i64);
            Ok(active)
        })?,
    )?;
    let w = world.clone();
    camera.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let world = w.borrow()?;
            require_alive(&world, id)?;
            let Some(camera) = world
                .get::<Camera>(id)
                .map_err(world_error)?
                .copied()
                .and_then(Camera::as_perspective)
            else {
                return Err(api_error("entity has no camera3d"));
            };
            let t = lua.create_table()?;
            t.set("x", camera.position.x)?;
            t.set("y", camera.position.y)?;
            t.set("z", camera.position.z)?;
            t.set("tx", camera.target.x)?;
            t.set("ty", camera.target.y)?;
            t.set("tz", camera.target.z)?;
            t.set("fov", camera.fov_y.to_degrees())?;
            t.set("near", camera.near)?;
            t.set("far", camera.far)?;
            Ok(t)
        })?,
    )?;
    let w = world.clone();
    camera.set(
        "set",
        lua.create_function(
            move |_,
                  (id, x, y, z, tx, ty, tz, fov): (
                i64,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                Option<f32>,
            )| {
                let id = entity_from_lua(id)?;
                {
                    let world = w.borrow()?;
                    require_alive(&world, id)?;
                }
                if !Vec3::new(x, y, z).is_finite() || !Vec3::new(tx, ty, tz).is_finite() {
                    return Err(api_error("camera.set position/target must be finite"));
                }
                if let Some(fov) = fov
                    && (!fov.is_finite() || fov <= 0.0 || fov >= 180.0)
                {
                    return Err(api_error("camera.set fov must be in (0, 180) degrees"));
                }
                let mut world = w.borrow_mut()?;
                let Some(camera) = world
                    .get_mut::<Camera>(id)
                    .map_err(world_error)?
                    .and_then(Camera::as_perspective_mut)
                else {
                    return Err(api_error("entity has no camera3d"));
                };
                camera.position = Vec3::new(x, y, z);
                camera.target = Vec3::new(tx, ty, tz);
                if let Some(fov) = fov {
                    camera.fov_y = fov.to_radians();
                }
                Ok(true)
            },
        )?,
    )?;
    lua.globals().set("camera", camera)?;
    Ok(())
}

// ── camera3d：射线 / 地面点选 / 世界→屏幕 ────────────────────────────────────────

fn install_camera3d(
    lua: &Lua,
    state: &Handle<CameraState>,
    _world: &Handle<World>,
    pointer: PointerCell,
) -> LuaResult<()> {
    // docs/54 §8-⑦：3D 侧的**屏↔世界**投影链。此前 Lua 只有 `physics3d.raycast`（自己拼一条
    // 射线），没有"屏幕坐标 → 相机射线"，编辑器 3D 点选用的 `Camera3D::ray_from_screen` 脚本够不着。
    // 这里复用同一份投影数学（`crate::picking`），于是运行时、Lua 与编辑器 picking 走同一条链。
    let camera3d = lua.create_table()?;
    let s = state.clone();
    camera3d.set(
        "ray",
        lua.create_function(move |lua, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("camera3d.ray must be finite"));
            }
            let (camera, viewport) = view_3d_of(&s)?;
            let ray =
                picking::ray_from_screen(camera, rustlua_ecs::transform::Vec2::new(x, y), viewport)
                    .map_err(api_error)?;
            table_ray(lua, ray.origin, ray.direction)
        })?,
    )?;
    let s = state.clone();
    let pointer_mouse = Rc::clone(&pointer);
    camera3d.set(
        "mouse_ray",
        lua.create_function(move |lua, ()| {
            let (camera, viewport, cursor) = {
                let state = s.borrow()?;
                let (camera, viewport) = view_3d(&state)?;
                (
                    camera,
                    viewport,
                    pointer_mouse
                        .get()
                        .map_or((0.0, 0.0), |(x, y)| (f64::from(x), f64::from(y))),
                )
            };
            let ray = picking::ray_from_screen(
                camera,
                rustlua_ecs::transform::Vec2::new(cursor.0 as f32, cursor.1 as f32),
                viewport,
            )
            .map_err(api_error)?;
            table_ray(lua, ray.origin, ray.direction)
        })?,
    )?;
    // 地面点选是最常见的一步（放置/寻路/点击地面）：给一条共享实现，脚本不必自己解直线与平面。
    let s = state.clone();
    camera3d.set(
        "pick_plane",
        lua.create_function(move |lua, (x, y, plane_y): (f32, f32, f32)| {
            if !x.is_finite() || !y.is_finite() || !plane_y.is_finite() {
                return Err(api_error("camera3d.pick_plane must be finite"));
            }
            let (camera, viewport) = view_3d_of(&s)?;
            let ray =
                picking::ray_from_screen(camera, rustlua_ecs::transform::Vec2::new(x, y), viewport)
                    .map_err(api_error)?;
            match picking::ray_plane_point(ray, plane_y) {
                // 没命中（射线与平面平行或背向）返回 nil 而不是假点。
                Some(point) => {
                    let table = lua.create_table()?;
                    table.set("x", point.x)?;
                    table.set("y", point.y)?;
                    table.set("z", point.z)?;
                    Ok(mlua::Value::Table(table))
                }
                None => Ok(mlua::Value::Nil),
            }
        })?,
    )?;
    let s = state.clone();
    camera3d.set(
        "world_to_screen",
        lua.create_function(move |lua, (x, y, z): (f32, f32, f32)| {
            if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                return Err(api_error("camera3d.world_to_screen must be finite"));
            }
            let (camera, viewport) = view_3d_of(&s)?;
            let Some((screen, visible)) =
                picking::screen_from_world(camera, Vec3::new(x, y, z), viewport)
            else {
                return Ok(mlua::Value::Nil);
            };
            let table = lua.create_table()?;
            table.set("x", screen.x)?;
            table.set("y", screen.y)?;
            table.set("visible", visible)?;
            Ok(mlua::Value::Table(table))
        })?,
    )?;
    let s = state.clone();
    camera3d.set(
        "get",
        lua.create_function(move |lua, ()| {
            let (camera, viewport) = view_3d_of(&s)?;
            let table = lua.create_table()?;
            table.set("px", camera.position.x)?;
            table.set("py", camera.position.y)?;
            table.set("pz", camera.position.z)?;
            table.set("tx", camera.target.x)?;
            table.set("ty", camera.target.y)?;
            table.set("tz", camera.target.z)?;
            table.set("fov", camera.fov_y.to_degrees())?;
            table.set("near", camera.near)?;
            table.set("far", camera.far)?;
            table.set("width", viewport.width)?;
            table.set("height", viewport.height)?;
            Ok(table)
        })?,
    )?;
    lua.globals().set("camera3d", camera3d)?;
    Ok(())
}

// ── camera2d：控制器（跟随/死区/平滑/抖动）+ 视口换算 ────────────────────────────

fn install_camera2d(
    lua: &Lua,
    state: &Handle<CameraState>,
    world: &Handle<World>,
    pointer: PointerCell,
) -> LuaResult<()> {
    // WP4：2D 相机控制器（跟随 / 死区 / 平滑 / 抖动）。
    //
    // 相机数据在宿主（`RuntimeScene::camera`），因此 `set`/`set_zoom` 只是登记请求、由宿主
    // 下一帧消费；`get`/`get_zoom` 读宿主每帧回填的真实值，不会返回脚本的自述。
    let camera2d = lua.create_table()?;
    let s = state.clone();
    camera2d.set(
        "set",
        lua.create_function(move |_, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("camera2d.set must be finite"));
            }
            s.borrow_mut()?
                .controller_mut()
                .request_center(rustlua_ecs::transform::Vec2::new(x, y));
            Ok(true)
        })?,
    )?;
    let s = state.clone();
    camera2d.set(
        "get",
        lua.create_function(move |lua, ()| {
            let center = s.borrow()?.controller().center();
            let table = lua.create_table()?;
            table.set("x", center.x)?;
            table.set("y", center.y)?;
            Ok(table)
        })?,
    )?;
    let s = state.clone();
    camera2d.set(
        "set_zoom",
        lua.create_function(move |_, zoom: f32| {
            if !zoom.is_finite() || zoom <= 0.0 {
                return Err(api_error("camera2d.set_zoom must be > 0"));
            }
            s.borrow_mut()?.controller_mut().request_zoom(zoom);
            Ok(true)
        })?,
    )?;
    let s = state.clone();
    camera2d.set(
        "get_zoom",
        lua.create_function(move |_, ()| Ok(s.borrow()?.controller().zoom()))?,
    )?;
    // WP4 ⑧：输入统一 screen→world。此前 Lua 只能拿到 `input.cursor_position()` 的**屏幕像素**，
    // 想把点击换成世界坐标就得在脚本里重写一遍相机数学（zoom/center/bounds/rotation 一个都
    // 不能错）。这里直接用宿主每帧回填的相机快照 + `Camera2D` 自己的投影函数，保证与精灵、
    // 瓦片、世界文字走的是**同一份**换算（`math2d` 里只有这一份实现）。
    let s = state.clone();
    camera2d.set(
        "screen_to_world",
        lua.create_function(move |lua, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("camera2d.screen_to_world must be finite"));
            }
            let (camera, viewport) = view_2d_of(&s)?;
            let world = camera
                .screen_to_world(rustlua_ecs::transform::Vec2::new(x, y), viewport)
                .map_err(|error| api_error(error.to_string()))?;
            table_vec(lua, world.x, world.y)
        })?,
    )?;
    let s = state.clone();
    camera2d.set(
        "world_to_screen",
        lua.create_function(move |lua, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("camera2d.world_to_screen must be finite"));
            }
            let (camera, viewport) = view_2d_of(&s)?;
            let screen = camera
                .world_to_screen(rustlua_ecs::transform::Vec2::new(x, y), viewport)
                .map_err(|error| api_error(error.to_string()))?;
            table_vec(lua, screen.x, screen.y)
        })?,
    )?;
    // 鼠标直接换算成世界坐标：2D 游戏里"点到哪个格"是最常见的用法，不该每个脚本都自己拼。
    let s = state.clone();
    let pointer_screen = Rc::clone(&pointer);
    camera2d.set(
        "mouse_world",
        lua.create_function(move |lua, ()| {
            let (camera, viewport, cursor) = {
                let state = s.borrow()?;
                let (camera, viewport) = view_2d(&state)?;
                (
                    camera,
                    viewport,
                    pointer_screen
                        .get()
                        .map_or((0.0, 0.0), |(x, y)| (f64::from(x), f64::from(y))),
                )
            };
            let world = camera
                .screen_to_world(
                    rustlua_ecs::transform::Vec2::new(cursor.0 as f32, cursor.1 as f32),
                    viewport,
                )
                .map_err(|error| api_error(error.to_string()))?;
            table_vec(lua, world.x, world.y)
        })?,
    )?;
    let s = state.clone();
    camera2d.set(
        "shake",
        lua.create_function(move |_, (amplitude, duration): (f32, f32)| {
            if !amplitude.is_finite() || !duration.is_finite() || duration <= 0.0 {
                return Err(api_error(
                    "camera2d.shake needs a finite amplitude and duration > 0",
                ));
            }
            s.borrow_mut()?.controller_mut().shake(amplitude, duration);
            Ok(true)
        })?,
    )?;
    let s = state.clone();
    let w = world.clone();
    camera2d.set(
        "follow",
        lua.create_function(
            move |_, (id, options): (Option<i64>, Option<mlua::Table>)| {
                let target = match id {
                    Some(raw) => {
                        let entity = entity_from_lua(raw)?;
                        {
                            let world = w.borrow()?;
                            require_alive(&world, entity)?;
                        }
                        Some(entity)
                    }
                    None => None,
                };
                let mut state = s.borrow_mut()?;
                state.controller_mut().follow(target);
                if let Some(options) = options {
                    let offset_x = options.get::<Option<f32>>("offset_x")?.unwrap_or(0.0);
                    let offset_y = options.get::<Option<f32>>("offset_y")?.unwrap_or(0.0);
                    let dead_x = options.get::<Option<f32>>("dead_zone_x")?.unwrap_or(0.0);
                    let dead_y = options.get::<Option<f32>>("dead_zone_y")?.unwrap_or(0.0);
                    let smoothing = options.get::<Option<f32>>("smoothing")?.unwrap_or(0.0);
                    for value in [offset_x, offset_y, dead_x, dead_y, smoothing] {
                        if !value.is_finite() {
                            return Err(api_error("camera2d.follow options must be finite"));
                        }
                    }
                    if dead_x < 0.0 || dead_y < 0.0 {
                        return Err(api_error("camera2d.follow dead zone must be >= 0"));
                    }
                    let controller = state.controller_mut();
                    controller.set_offset(rustlua_ecs::transform::Vec2::new(offset_x, offset_y));
                    controller.set_dead_zone(rustlua_ecs::transform::Vec2::new(dead_x, dead_y));
                    controller.set_smoothing(smoothing);
                }
                Ok(true)
            },
        )?,
    )?;
    let s = state.clone();
    camera2d.set(
        "unfollow",
        lua.create_function(move |_, ()| {
            s.borrow_mut()?.controller_mut().follow(None);
            Ok(true)
        })?,
    )?;
    lua.globals().set("camera2d", camera2d)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Camera2D, Camera3D};
    use rustlua_ecs::world::World;
    use rustlua_math::math2d::Vec2;
    use std::cell::Cell;

    /// 装好三张表的测试夹具：Lua、相机会话态、世界、输入指针单元。
    type Harness = (Lua, Handle<CameraState>, Handle<World>, PointerCell);

    fn harness() -> Harness {
        let lua = Lua::new();
        let state = CameraState::handle();
        let world = Handle::new(World::new(), "world");
        let pointer = Rc::new(Cell::new(Some((400.0, 300.0))));
        install(&lua, &state, &world, Rc::clone(&pointer)).unwrap();
        (lua, state, world, pointer)
    }

    /// 19 个函数一个不少（这是 Lua 面的**清单**，少一个脚本就会静默拿到 nil）。
    #[test]
    fn every_camera_function_is_installed() {
        let (lua, _state, _world, _pointer) = harness();
        let mut total = 0;
        for table in ["camera", "camera3d", "camera2d"] {
            let table: Table = lua.globals().get(table).unwrap();
            let count = table.pairs::<mlua::Value, mlua::Value>().count();
            total += count;
        }
        assert_eq!(total, 19, "camera/camera3d/camera2d 合计 19 个函数");
    }

    /// `camera2d` 的请求-回填交接：脚本写的只是 pending，宿主回填的才是 `get` 的答案。
    #[test]
    fn camera2d_requests_are_pending_until_the_host_reports() {
        let (lua, state, _world, _pointer) = harness();
        lua.load("camera2d.set(120, -30); camera2d.set_zoom(2) ; camera2d.shake(4, 0.5)")
            .exec()
            .unwrap();
        {
            let mut camera = state.borrow_mut().unwrap();
            assert_eq!(
                camera.controller_mut().take_pending_center(),
                Some(Vec2::new(120.0, -30.0))
            );
            assert_eq!(camera.controller_mut().take_pending_zoom(), Some(2.0));
        }
        // 没有回填之前，`get` 读到的仍是默认值（不会回报脚本的自述）。
        // 注意：不能在持有 `state.borrow_mut()` 时调 Lua——那正是句柄要抓的重入。
        let get: (f32, f32) = lua
            .load("local c = camera2d.get(); return c.x, c.y")
            .eval()
            .unwrap();
        assert_eq!(get, (0.0, 0.0));
        state
            .borrow_mut()
            .unwrap()
            .controller_mut()
            .report(Vec2::new(9.0, 8.0), 1.5);
        let zoom: f32 = lua.load("return camera2d.get_zoom()").eval().unwrap();
        assert_eq!(zoom, 1.5, "回填之后 get_zoom 才说真话");
    }

    /// 没渲染过一帧时，屏幕↔世界换算必须报错（猜一个默认相机会让"点错位置"变成无头案）。
    #[test]
    fn screen_world_conversion_requires_one_rendered_frame() {
        let (lua, state, _world, _pointer) = harness();
        let error = lua
            .load("return camera2d.screen_to_world(10, 10)")
            .eval::<mlua::Value>()
            .unwrap_err()
            .to_string();
        assert!(error.contains("needs one rendered frame first"), "{error}");

        state
            .borrow_mut()
            .unwrap()
            .set_view_2d(Camera2D::new(Vec2::zero(), 1.0).unwrap(), [800.0, 600.0]);
        let point: Table = lua
            .load("return camera2d.screen_to_world(400, 300)")
            .eval()
            .unwrap();
        let x: f32 = point.get(1).unwrap();
        let y: f32 = point.get(2).unwrap();
        // 视口中心 + 默认相机（center 为原点、zoom 1）⇒ 世界原点。
        assert!(x.abs() < 1e-3 && y.abs() < 1e-3, "{x},{y}");
    }

    /// `camera.set_active` 只认带 `Camera` 组件的实体；没有相机时给的是原文的
    /// `entity has no camera`（不是 `no camera3d`）。
    #[test]
    fn set_active_validates_the_component_and_records_the_selection() {
        let (lua, state, world, _pointer) = harness();
        let id = world.with_mut(|world| {
            let id = world.spawn();
            world
                .insert(
                    id,
                    Camera::Orthographic(Camera2D::new(Vec2::zero(), 1.0).unwrap()),
                )
                .unwrap();
            id
        });
        let raw = id.to_lua_i64();
        lua.load(format!("assert(camera.set_active({raw}) == true)"))
            .exec()
            .unwrap();
        assert_eq!(state.borrow().unwrap().active(), Some(id));

        // 活着但**没有相机组件**的实体：错误串必须是 `entity has no camera`。
        let bare = world.with_mut(|world| world.spawn());
        let message = lua
            .load(format!("return camera.set_active({})", bare.to_lua_i64()))
            .eval::<mlua::Value>()
            .unwrap_err()
            .to_string();
        assert!(message.contains("entity has no camera"), "{message}");

        // 死的句柄走的是"这个实体不在了"，与所有绑定的前置校验同一口径。
        let message = lua
            .load(format!(
                "return camera.set_active({})",
                EntityId::from_raw(999, 1).to_lua_i64()
            ))
            .eval::<mlua::Value>()
            .unwrap_err()
            .to_string();
        assert!(message.contains("invalid entity id"), "{message}");
    }

    /// `camera2d.follow` 校验目标存活，并把选项写进控制器。
    #[test]
    fn follow_validates_target_and_stores_options() {
        let (lua, state, world, _pointer) = harness();
        let id = world.with_mut(|world| world.spawn());
        let raw = id.to_lua_i64();
        lua.load(format!(
            "camera2d.follow({raw}, {{dead_zone_x=8, dead_zone_y=4, smoothing=6, offset_x=1}})"
        ))
        .exec()
        .unwrap();
        let camera = state.borrow().unwrap();
        let controller = camera.controller();
        assert_eq!(controller.follow_target(), Some(id));
        assert_eq!(controller.dead_zone(), Vec2::new(8.0, 4.0));
        assert_eq!(controller.smoothing(), 6.0);
        assert_eq!(controller.offset(), Vec2::new(1.0, 0.0));
        drop(camera);
        lua.load("camera2d.unfollow()").exec().unwrap();
        assert_eq!(state.borrow().unwrap().controller().follow_target(), None);
    }

    /// `camera3d.ray` 用回填快照走共享拾取链：中央射线指向相机目标。
    #[test]
    fn camera3d_ray_uses_the_shared_picking_chain() {
        let (lua, state, _world, _pointer) = harness();
        let camera = Camera3D::perspective(
            Vec3::new(0.0, 5.0, 20.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )
        .unwrap();
        state
            .borrow_mut()
            .unwrap()
            .set_view_3d(camera, [800.0, 600.0]);
        let ray: Table = lua.load("return camera3d.ray(400, 300)").eval().unwrap();
        let dy: f32 = ray.get("dy").unwrap();
        assert!(dy < 0.0, "中央射线必须朝下看向目标（dy={dy}）");
    }
}
