//! 相机三兄弟：`camera`（兼容层）、`camera3d`、`camera2d`（含视口与屏幕↔世界换算）。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::camera_view;
use crate::script::camera_view_from;
use crate::script::camera3d_view;
use crate::script::camera3d_view_from;
use crate::script::error::{api_error, world_error};
use crate::script::values::{entity_from_lua, entity_to_lua, require_alive, table_ray, table_vec};
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_math::math3d::Vec3;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let camera = lua.create_table()?;
    let s = Rc::clone(&state);
    camera.set(
        "set_active",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            // 拆分批 9：**只有一条选择链**——认任何带 `Camera` 的实体，由它的 `projection` 决定
            // 服务哪个 pass（透视 → 3D world，正交 → 2D overlay）。
            //
            // 与批 9 之前的行为差别（已在 CHANGELOG 的「兼容契约（变更）」里逐条记录）：
            // ① 选中**正交**相机不再报 `entity has no camera3d`，而是成为 2D 相机的来源
            //    （宿主采用它的载荷作为 2D 取景基准）；② 对**完全没有相机**的实体，错误串由
            //    `entity has no camera3d` 改为 `entity has no camera`（那句话在"一条选择链"下
            //    才是真的）。`camera.get` / `camera.set` 是**透视 payload** 的读写，仍保持原错误串。
            if state
                .world
                .get::<rustlua_camera::camera::Camera>(id)
                .map_err(world_error)?
                .is_none()
            {
                return Err(api_error("entity has no camera"));
            }
            state.active_camera = Some(id);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera.set(
        "get_active",
        lua.create_function(move |_, ()| Ok(s.borrow().active_camera.map(entity_to_lua)))?,
    )?;
    let s = Rc::clone(&state);
    camera.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(camera) = state
                .world
                .get::<rustlua_camera::camera::Camera>(id)
                .map_err(world_error)?
                .copied()
                .and_then(rustlua_camera::camera::Camera::as_perspective)
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
    let s = Rc::clone(&state);
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
                let mut state = s.lua_borrow_mut()?;
                require_alive(&state, id)?;
                if !Vec3::new(x, y, z).is_finite() || !Vec3::new(tx, ty, tz).is_finite() {
                    return Err(api_error("camera.set position/target must be finite"));
                }
                if let Some(fov) = fov
                    && (!fov.is_finite() || fov <= 0.0 || fov >= 180.0)
                {
                    return Err(api_error("camera.set fov must be in (0, 180) degrees"));
                }
                let Some(camera) = state
                    .world
                    .get_mut::<rustlua_camera::camera::Camera>(id)
                    .map_err(world_error)?
                    .and_then(rustlua_camera::camera::Camera::as_perspective_mut)
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
    // docs/54 §8-⑦：3D 侧的**屏↔世界**投影链。此前 Lua 只有 `physics3d.raycast`（自己拼一条
    // 射线），没有"屏幕坐标 → 相机射线"，编辑器 3D 点选用的 `Camera3D::ray_from_screen` 脚本够不着。
    // 这里复用同一份 `Camera3D` 投影数学，于是运行时、Lua 与编辑器 picking 走同一条链。

    let camera3d = lua.create_table()?;
    let s = Rc::clone(&state);
    camera3d.set(
        "ray",
        lua.create_function(move |lua, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("camera3d.ray must be finite"));
            }
            let (camera, viewport) = camera3d_view(&s)?;
            // 共享链：`rustlua_render::picking::ray_from_screen`（编辑器 3D 点选与它同一入口）。
            let ray = rustlua_render::picking::ray_from_screen(
                camera,
                rustlua_ecs::transform::Vec2::new(x, y),
                viewport,
            )
            .map_err(api_error)?;
            table_ray(lua, ray.origin, ray.direction)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera3d.set(
        "mouse_ray",
        lua.create_function(move |lua, ()| {
            let (camera, viewport, cursor) = {
                let state = s.lua_borrow()?;
                let (camera, viewport) = camera3d_view_from(&state)?;
                (camera, viewport, state.input.cursor_position())
            };
            let ray = rustlua_render::picking::ray_from_screen(
                camera,
                rustlua_ecs::transform::Vec2::new(cursor.0 as f32, cursor.1 as f32),
                viewport,
            )
            .map_err(api_error)?;
            table_ray(lua, ray.origin, ray.direction)
        })?,
    )?;
    // 地面点选是最常见的一步（放置/寻路/点击地面）：给一条共享实现，脚本不必自己解直线与平面。
    let s = Rc::clone(&state);
    camera3d.set(
        "pick_plane",
        lua.create_function(move |lua, (x, y, plane_y): (f32, f32, f32)| {
            if !x.is_finite() || !y.is_finite() || !plane_y.is_finite() {
                return Err(api_error("camera3d.pick_plane must be finite"));
            }
            let (camera, viewport) = camera3d_view(&s)?;
            let ray = rustlua_render::picking::ray_from_screen(
                camera,
                rustlua_ecs::transform::Vec2::new(x, y),
                viewport,
            )
            .map_err(api_error)?;
            match rustlua_render::picking::ray_plane_point(ray, plane_y) {
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
    let s = Rc::clone(&state);
    camera3d.set(
        "world_to_screen",
        lua.create_function(move |lua, (x, y, z): (f32, f32, f32)| {
            if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                return Err(api_error("camera3d.world_to_screen must be finite"));
            }
            let (camera, viewport) = camera3d_view(&s)?;
            let Some((screen, visible)) =
                rustlua_render::picking::screen_from_world(camera, Vec3::new(x, y, z), viewport)
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
    let s = Rc::clone(&state);
    camera3d.set(
        "get",
        lua.create_function(move |lua, ()| {
            let (camera, viewport) = camera3d_view(&s)?;
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
    // WP4：2D 相机控制器（跟随 / 死区 / 平滑 / 抖动）。此前 Lua **完全无法控制 2D 相机**
    // （`camera` 表只管 3D 的 Camera3D 组件），大地图或跟随镜头只能靠宿主代码。
    //
    // 相机数据在宿主（`RuntimeScene::camera`），因此 `set`/`set_zoom` 只是登记请求、由宿主
    // 下一帧消费；`get`/`get_zoom` 读宿主每帧回填的真实值，不会返回脚本的自述。

    let camera2d = lua.create_table()?;
    let s = Rc::clone(&state);
    camera2d.set(
        "set",
        lua.create_function(move |_, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("camera2d.set must be finite"));
            }
            s.borrow_mut()
                .camera2d
                .request_center(rustlua_ecs::transform::Vec2::new(x, y));
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera2d.set(
        "get",
        lua.create_function(move |lua, ()| {
            let center = s.borrow().camera2d.center();
            let table = lua.create_table()?;
            table.set("x", center.x)?;
            table.set("y", center.y)?;
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera2d.set(
        "set_zoom",
        lua.create_function(move |_, zoom: f32| {
            if !zoom.is_finite() || zoom <= 0.0 {
                return Err(api_error("camera2d.set_zoom must be > 0"));
            }
            s.borrow_mut().camera2d.request_zoom(zoom);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera2d.set(
        "get_zoom",
        lua.create_function(move |_, ()| Ok(s.borrow().camera2d.zoom()))?,
    )?;
    // WP4 ⑧：输入统一 screen→world。此前 Lua 只能拿到 `input.cursor_position()` 的**屏幕像素**，
    // 想把点击换成世界坐标就得在脚本里重写一遍相机数学（zoom/center/bounds/rotation 一个都
    // 不能错）。这里直接用宿主每帧回填的相机快照 + `Camera2D` 自己的投影函数，保证与精灵、
    // 瓦片、世界文字走的是**同一份**换算（`math2d` 里只有这一份实现）。
    let s = Rc::clone(&state);
    camera2d.set(
        "screen_to_world",
        lua.create_function(move |lua, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("camera2d.screen_to_world must be finite"));
            }
            let (camera, viewport) = camera_view(&s)?;
            let world = camera
                .screen_to_world(rustlua_ecs::transform::Vec2::new(x, y), viewport)
                .map_err(|error| api_error(error.to_string()))?;
            table_vec(lua, world.x, world.y)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera2d.set(
        "world_to_screen",
        lua.create_function(move |lua, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("camera2d.world_to_screen must be finite"));
            }
            let (camera, viewport) = camera_view(&s)?;
            let screen = camera
                .world_to_screen(rustlua_ecs::transform::Vec2::new(x, y), viewport)
                .map_err(|error| api_error(error.to_string()))?;
            table_vec(lua, screen.x, screen.y)
        })?,
    )?;
    // 鼠标直接换算成世界坐标：2D 游戏里"点到哪个格"是最常见的用法，不该每个脚本都自己拼。
    let s = Rc::clone(&state);
    camera2d.set(
        "mouse_world",
        lua.create_function(move |lua, ()| {
            let (camera, viewport, cursor) = {
                let state = s.lua_borrow()?;
                let (camera, viewport) = camera_view_from(&state)?;
                (camera, viewport, state.input.cursor_position())
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
    let s = Rc::clone(&state);
    camera2d.set(
        "shake",
        lua.create_function(move |_, (amplitude, duration): (f32, f32)| {
            if !amplitude.is_finite() || !duration.is_finite() || duration <= 0.0 {
                return Err(api_error(
                    "camera2d.shake needs a finite amplitude and duration > 0",
                ));
            }
            s.borrow_mut().camera2d.shake(amplitude, duration);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera2d.set(
        "follow",
        lua.create_function(
            move |_, (id, options): (Option<i64>, Option<mlua::Table>)| {
                let target = match id {
                    Some(raw) => {
                        let entity = entity_from_lua(raw)?;
                        let state = s.lua_borrow()?;
                        require_alive(&state, entity)?;
                        Some(entity)
                    }
                    None => None,
                };
                let mut state = s.lua_borrow_mut()?;
                state.camera2d.follow(target);
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
                    state
                        .camera2d
                        .set_offset(rustlua_ecs::transform::Vec2::new(offset_x, offset_y));
                    state
                        .camera2d
                        .set_dead_zone(rustlua_ecs::transform::Vec2::new(dead_x, dead_y));
                    state.camera2d.set_smoothing(smoothing);
                }
                Ok(true)
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    camera2d.set(
        "unfollow",
        lua.create_function(move |_, ()| {
            s.borrow_mut().camera2d.follow(None);
            Ok(true)
        })?,
    )?;
    lua.globals().set("camera2d", camera2d)?;
    Ok(())
}
