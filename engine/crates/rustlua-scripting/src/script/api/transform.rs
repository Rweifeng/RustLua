//! 2D/3D 位姿：`transform`（位置、缩放、朝向）与 `transform3d`。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::{api_error, world_error};
use crate::script::values::{entity_from_lua, require_alive};
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_ecs::transform::Transform;
use rustlua_ecs::transform::Transform3D;
use rustlua_ecs::transform::Vec2;
use rustlua_math::math3d::Vec3;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

/// `transform3d.set(id, x, y, z, [rx, ry, rz, rw, [sx, sy, sz]])` argument tuple.
type Transform3dSetArgs = (
    i64,
    f32,
    f32,
    f32,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
);

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let transform = lua.create_table()?;
    let s = Rc::clone(&state);
    transform.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let t = lua.create_table()?;
            if let Some(value) = state.world.get::<Transform>(id).map_err(world_error)? {
                t.set("space", "2d")?;
                t.set("x", value.position.x)?;
                t.set("y", value.position.y)?;
                t.set("rotation", value.rotation)?;
                t.set("scale_x", value.scale.x)?;
                t.set("scale_y", value.scale.y)?;
                return Ok(t);
            }
            let value = state
                .world
                .get::<Transform3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform (2d or 3d)"))?;
            t.set("space", "3d")?;
            t.set("x", value.position.x)?;
            t.set("y", value.position.y)?;
            t.set("z", value.position.z)?;
            t.set("rx", value.rotation.x)?;
            t.set("ry", value.rotation.y)?;
            t.set("rz", value.rotation.z)?;
            t.set("rw", value.rotation.w)?;
            t.set("scale_x", value.scale.x)?;
            t.set("scale_y", value.scale.y)?;
            t.set("scale_z", value.scale.z)?;
            Ok(t)
        })?,
    )?;
    let s = Rc::clone(&state);
    transform.set(
        "world",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            // WP2：世界变换（开启空间继承时是父链组合的结果，否则等于局部值）。
            let t = lua.create_table()?;
            if let Some(transform) =
                rustlua_ecs::hierarchy::world_transform_2d(&state.world, id).map_err(world_error)?
            {
                t.set("space", "2d")?;
                t.set("x", transform.position.x)?;
                t.set("y", transform.position.y)?;
                t.set("rotation", transform.rotation)?;
                t.set("scale_x", transform.scale.x)?;
                t.set("scale_y", transform.scale.y)?;
                return Ok(t);
            }
            if let Some(transform) =
                rustlua_ecs::hierarchy::world_transform_3d(&state.world, id).map_err(world_error)?
            {
                t.set("space", "3d")?;
                t.set("x", transform.position.x)?;
                t.set("y", transform.position.y)?;
                t.set("z", transform.position.z)?;
                t.set("rx", transform.rotation.x)?;
                t.set("ry", transform.rotation.y)?;
                t.set("rz", transform.rotation.z)?;
                t.set("rw", transform.rotation.w)?;
                t.set("scale_x", transform.scale.x)?;
                t.set("scale_y", transform.scale.y)?;
                t.set("scale_z", transform.scale.z)?;
                return Ok(t);
            }
            Err(api_error("entity has no transform"))
        })?,
    )?;
    let s = Rc::clone(&state);
    transform.set(
        "space",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            if state
                .world
                .get::<Transform>(id)
                .map_err(world_error)?
                .is_some()
            {
                return Ok(Some("2d"));
            }
            if state
                .world
                .get::<Transform3D>(id)
                .map_err(world_error)?
                .is_some()
            {
                return Ok(Some("3d"));
            }
            Ok(None)
        })?,
    )?;
    let s = Rc::clone(&state);
    transform.set(
        "attach",
        lua.create_function(move |_, (id, space): (i64, String)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            match space.as_str() {
                "2d" => {
                    if state
                        .world
                        .get::<Transform>(id)
                        .map_err(world_error)?
                        .is_some()
                    {
                        return Ok(false);
                    }
                    // 用统一的**压平**实现（`Transform3D::to_2d`），而不是只手抄一个 XY：
                    // 手抄会把旋转与缩放悄悄丢掉（3D 里转过 90° 的实体挂上 2D 变换后会突然
                    // 朝正面）。真能无损压平才建组件，压不平则明确报错——见下面的错误信息。
                    let flattened = state
                        .world
                        .get::<Transform3D>(id)
                        .map_err(world_error)?
                        .map(|source| source.to_2d());
                    match flattened {
                        Some(Some(flat)) => {
                            state.world.insert(id, flat).map_err(world_error)?;
                            Ok(true)
                        }
                        Some(None) => Err(api_error(
                            "entity's 3D transform is not planar (tilt, z offset or z scale); \
                             flattening to 2d would drop it — clear those first, or keep using \
                             transform3d",
                        )),
                        None => {
                            state
                                .world
                                .insert(id, Transform::default())
                                .map_err(world_error)?;
                            Ok(true)
                        }
                    }
                }
                "3d" => {
                    if state
                        .world
                        .get::<Transform3D>(id)
                        .map_err(world_error)?
                        .is_some()
                    {
                        return Ok(false);
                    }
                    // 同理走统一的**提升**实现（`Transform::to_3d`），因此 2D 的旋转与缩放
                    // 会被保留成绕 Z 的旋转与 XY 缩放。
                    let lifted = state
                        .world
                        .get::<Transform>(id)
                        .map_err(world_error)?
                        .copied()
                        .map(Transform::to_3d);
                    let value = lifted.unwrap_or_default();
                    state.world.insert(id, value).map_err(world_error)?;
                    Ok(true)
                }
                other => Err(api_error(format!(
                    "transform.attach space must be \"2d\" or \"3d\", got {other:?}"
                ))),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    transform.set(
        "set",
        lua.create_function(
            move |_,
                  (id, x, y, rotation, scale_x, scale_y): (
                i64,
                f32,
                f32,
                Option<f32>,
                Option<f32>,
                Option<f32>,
            )| {
                let id = entity_from_lua(id)?;
                let mut state = s.lua_borrow_mut()?;
                require_alive(&state, id)?;
                if let Some(t) = state.world.get_mut::<Transform>(id).map_err(world_error)? {
                t.position = Vec2::new(x, y);
                if let Some(v) = rotation {
                    t.rotation = v;
                }
                if let Some(v) = scale_x {
                    t.scale.x = v;
                }
                if let Some(v) = scale_y {
                    t.scale.y = v;
                }
                    return Ok(());
                }
                let t = state
                    .world
                    .get_mut::<Transform3D>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no transform (2d or 3d)"))?;
                if rotation.is_some() {
                    return Err(api_error(
                        "transform.set rotation is 2d-only (radians around Z); use transform3d.set for a 3d quaternion",
                    ));
                }
                t.position.x = x;
                t.position.y = y;
                if let Some(v) = scale_x {
                    t.scale.x = v;
                }
                if let Some(v) = scale_y {
                    t.scale.y = v;
                }
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    transform.set(
        "move",
        lua.create_function(move |_, (id, dx, dy): (i64, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            if let Some(t) = state.world.get_mut::<Transform>(id).map_err(world_error)? {
                t.position.x += dx;
                t.position.y += dy;
                return Ok(());
            }
            let t = state
                .world
                .get_mut::<Transform3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform (2d or 3d)"))?;
            t.position.x += dx;
            t.position.y += dy;
            Ok(())
        })?,
    )?;
    lua.globals().set("transform", transform)?;

    let transform3d = lua.create_table()?;
    let s = Rc::clone(&state);
    transform3d.set(
        "attach",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            state
                .world
                .insert(id, Transform3D::default())
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    transform3d.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let value = state
                .world
                .get::<Transform3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform3d"))?;
            let t = lua.create_table()?;
            t.set("x", value.position.x)?;
            t.set("y", value.position.y)?;
            t.set("z", value.position.z)?;
            t.set("rx", value.rotation.x)?;
            t.set("ry", value.rotation.y)?;
            t.set("rz", value.rotation.z)?;
            t.set("rw", value.rotation.w)?;
            t.set("sx", value.scale.x)?;
            t.set("sy", value.scale.y)?;
            t.set("sz", value.scale.z)?;
            Ok(t)
        })?,
    )?;
    let s = Rc::clone(&state);
    transform3d.set(
        "set",
        lua.create_function(
            move |_, (id, x, y, z, rx, ry, rz, rw, sx, sy, sz): Transform3dSetArgs| {
                let id = entity_from_lua(id)?;
                let mut state = s.lua_borrow_mut()?;
                require_alive(&state, id)?;
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    return Err(api_error("transform3d.set position must be finite"));
                }
                let t = state
                    .world
                    .get_mut::<Transform3D>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no transform3d"))?;
                t.position = Vec3::new(x, y, z);
                if let (Some(rx), Some(ry), Some(rz), Some(rw)) = (rx, ry, rz, rw) {
                    if !rx.is_finite() || !ry.is_finite() || !rz.is_finite() || !rw.is_finite() {
                        return Err(api_error("transform3d.set rotation must be finite"));
                    }
                    t.rotation = rustlua_math::math3d::Quat {
                        x: rx,
                        y: ry,
                        z: rz,
                        w: rw,
                    };
                }
                if let (Some(sx), Some(sy), Some(sz)) = (sx, sy, sz) {
                    if sx <= 0.0 || sy <= 0.0 || sz <= 0.0 {
                        return Err(api_error("transform3d.set scale must be positive"));
                    }
                    t.scale = Vec3::new(sx, sy, sz);
                }
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    transform3d.set(
        "move",
        lua.create_function(move |_, (id, dx, dy, dz): (i64, f32, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            let t = state
                .world
                .get_mut::<Transform3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform3d"))?;
            t.position.x += dx;
            t.position.y += dy;
            t.position.z += dz;
            Ok(())
        })?,
    )?;
    // docs/54 §8-⑦：`transform3d` 也要能读**世界**变换——`transform.world` 只覆盖"实体实际拥有
    // 的空间"，3D 实体走 `transform3d` 时此前拿不到父链组合后的位置（脚本只能自己推）。
    let s = Rc::clone(&state);
    transform3d.set(
        "world",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let world = rustlua_ecs::hierarchy::world_transform_3d_lazy(&state.world, id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform3d"))?;
            let table = lua.create_table()?;
            table.set("x", world.position.x)?;
            table.set("y", world.position.y)?;
            table.set("z", world.position.z)?;
            table.set("rx", world.rotation.x)?;
            table.set("ry", world.rotation.y)?;
            table.set("rz", world.rotation.z)?;
            table.set("rw", world.rotation.w)?;
            table.set("sx", world.scale.x)?;
            table.set("sy", world.scale.y)?;
            table.set("sz", world.scale.z)?;
            Ok(table)
        })?,
    )?;
    lua.globals().set("transform3d", transform3d)?;

    Ok(())
}
