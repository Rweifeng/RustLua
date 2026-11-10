//! `transform` / `transform3d` 两张表的 Lua 绑定（P1 ⑤-10）。
//!
//! 组件类型（`Transform` / `Transform3D`）就住在 `rustlua-ecs`，所以它们的 Lua 面也在这里——
//! `rustlua-scripting` 不再认识"变换怎么读写"这种领域规则，只把一个 `Handle<World>` 交出来。
//!
//! 这一刀几乎没有状态面：绑定只碰 `World`（读组件、写组件、算世界变换）。真正需要小心的
//! 是**同一个闭包里不能同时持有读守卫与写守卫**——`Bindings` 把"借世界"收敛成两个短写，
//! 语义与原脚本层一致（重入时的报错由 `Handle` 点名 `world`）。

use crate::entity::EntityId;
use crate::transform::{Transform, Transform3D, Vec2};
use crate::world::{World, WorldError};
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_lua_api::{Handle, api_error};
use rustlua_math::math3d::Vec3;
use std::cell::{Ref, RefMut};

/// 绑定侧的借用面：一个 `Handle<World>` + 与脚本层同名的 `world()` / `world_mut()` 短写。
///
/// 之所以包一层而不是直接把 `&World` 传进每个闭包：句柄要跨闭包存活（`'static`），而
/// `Ref`/`RefMut` 不能；这层薄壳只负责"从能力单元借一次"，借用失败报成 Lua 错误。
#[derive(Clone)]
struct Bindings {
    world: Handle<World>,
}

impl Bindings {
    fn new(world: &Handle<World>) -> Self {
        Self {
            world: world.clone(),
        }
    }

    fn world(&self) -> LuaResult<Ref<'_, World>> {
        self.world.borrow()
    }

    fn world_mut(&self) -> LuaResult<RefMut<'_, World>> {
        self.world.borrow_mut()
    }
}

/// `WorldError` → Lua 错误（每个领域绑定各留三行，避免把 `WorldError` 塞进叶子契约层）。
fn world_error(error: WorldError) -> mlua::Error {
    api_error(error.to_string())
}

/// 句柄失效统一报成脚本侧熟悉的 `invalid entity id: …`，而不是 `WorldError` 的英文内部串。
///
/// 热路径（`transform.get` / `set` / `move`）不再先单独 `require_alive` 再借一次世界：
/// `World::get` / `get_mut` 自己会做存活校验，多一次 `RefCell` 借用与 `is_alive` 只是白付。
fn entity_error(id: EntityId, error: WorldError) -> mlua::Error {
    match error {
        WorldError::InvalidEntity(_) => api_error(format!("invalid entity id: {id}")),
    }
}

/// Lua 侧实体 id → 实体句柄（错误串与脚本层逐字相同）。
fn entity_from_lua(value: i64) -> LuaResult<EntityId> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    EntityId::from_lua_i64(value).ok_or_else(|| api_error(format!("invalid entity id: {value}")))
}

/// 写操作的前置校验：实体必须还活着。
fn require_alive(state: &Bindings, entity: EntityId) -> LuaResult<()> {
    if state.world()?.is_alive(entity) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {entity}")))
    }
}

/// 读局部位置的标量快路径：2D 优先，缺 2D 时退回 3D。
///
/// 不建 table、不做 6 次 `table.set`——这是 `transform.position` 比 `transform.get`
/// 便宜的全部原因。错误语义与 `transform.get` 保持一致。
fn local_xy(world: &World, id: EntityId) -> LuaResult<(f32, f32)> {
    if let Some(value) = world
        .get::<Transform>(id)
        .map_err(|error| entity_error(id, error))?
    {
        return Ok((value.position.x, value.position.y));
    }
    let value = world
        .get::<Transform3D>(id)
        .map_err(|error| entity_error(id, error))?
        .ok_or_else(|| api_error("entity has no transform (2d or 3d)"))?;
    Ok((value.position.x, value.position.y))
}

/// 读世界位置的标量快路径：2D 优先，缺 2D 时退回 3D。
fn world_xy(world: &World, id: EntityId) -> LuaResult<(f32, f32)> {
    if let Some(value) =
        crate::hierarchy::world_transform_2d(world, id).map_err(|error| entity_error(id, error))?
    {
        return Ok((value.position.x, value.position.y));
    }
    match crate::hierarchy::world_transform_3d(world, id)
        .map_err(|error| entity_error(id, error))?
    {
        Some(value) => Ok((value.position.x, value.position.y)),
        None => Err(api_error("entity has no transform")),
    }
}

/// 3D 局部位置：只认 `Transform3D`，不拿 2D 的 XY 冒充 Z=0。
fn local_xyz(world: &World, id: EntityId) -> LuaResult<(f32, f32, f32)> {
    let value = world
        .get::<Transform3D>(id)
        .map_err(|error| entity_error(id, error))?
        .ok_or_else(|| api_error("entity has no transform3d"))?;
    Ok((value.position.x, value.position.y, value.position.z))
}

/// 3D 世界位置：与 `transform3d.world` 同源，允许本帧尚未传播（lazy 组合）。
///
/// 两个快路径先走：已有 `GlobalTransform` 直接取；没有父节点时局部值就是世界值。
/// 只有真正需要沿父链现算时才进 `world_transform_3d_lazy`（它会为祖先链分配临时 Vec）。
fn world_xyz(world: &World, id: EntityId) -> LuaResult<(f32, f32, f32)> {
    if let Some(global) = world
        .get::<crate::hierarchy::GlobalTransform>(id)
        .map_err(|error| entity_error(id, error))?
        && let Some(space) = global.space()
    {
        return Ok((space.position.x, space.position.y, space.position.z));
    }
    if world
        .get::<crate::hierarchy::Parent>(id)
        .map_err(|error| entity_error(id, error))?
        .is_none()
    {
        return local_xyz(world, id);
    }
    let value = crate::hierarchy::world_transform_3d_lazy(world, id)
        .map_err(|error| entity_error(id, error))?
        .ok_or_else(|| api_error("entity has no transform3d"))?;
    Ok((value.position.x, value.position.y, value.position.z))
}

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

pub fn install(lua: &Lua, world: &Handle<World>) -> LuaResult<()> {
    let transform = lua.create_table()?;
    let s = Bindings::new(world);
    transform.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let world = s.world()?;
            if let Some(value) = world
                .get::<Transform>(id)
                .map_err(|error| entity_error(id, error))?
            {
                let t = lua.create_table_with_capacity(0, 6)?;
                t.set("space", "2d")?;
                t.set("x", value.position.x)?;
                t.set("y", value.position.y)?;
                t.set("rotation", value.rotation)?;
                t.set("scale_x", value.scale.x)?;
                t.set("scale_y", value.scale.y)?;
                return Ok(t);
            }
            let value = world
                .get::<Transform3D>(id)
                .map_err(|error| entity_error(id, error))?
                .ok_or_else(|| api_error("entity has no transform (2d or 3d)"))?;
            let t = lua.create_table_with_capacity(0, 10)?;
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
    // 标量快路径：热循环里只取位置时不必付 6 字段表的钱。
    let s = Bindings::new(world);
    transform.set(
        "position",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let world = s.world()?;
            local_xy(&world, id)
        })?,
    )?;
    let s = Bindings::new(world);
    transform.set(
        "world_position",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let world = s.world()?;
            world_xy(&world, id)
        })?,
    )?;
    let s = Bindings::new(world);
    transform.set(
        "world",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let world = s.world()?;
            // WP2：世界变换（开启空间继承时是父链组合的结果，否则等于局部值）。
            if let Some(transform) = crate::hierarchy::world_transform_2d(&world, id)
                .map_err(|error| entity_error(id, error))?
            {
                let t = lua.create_table_with_capacity(0, 6)?;
                t.set("space", "2d")?;
                t.set("x", transform.position.x)?;
                t.set("y", transform.position.y)?;
                t.set("rotation", transform.rotation)?;
                t.set("scale_x", transform.scale.x)?;
                t.set("scale_y", transform.scale.y)?;
                return Ok(t);
            }
            if let Some(transform) = crate::hierarchy::world_transform_3d(&world, id)
                .map_err(|error| entity_error(id, error))?
            {
                let t = lua.create_table_with_capacity(0, 10)?;
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
    let s = Bindings::new(world);
    transform.set(
        "space",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let world = s.world()?;
            if world
                .get::<Transform>(id)
                .map_err(|error| entity_error(id, error))?
                .is_some()
            {
                return Ok(Some("2d"));
            }
            if world
                .get::<Transform3D>(id)
                .map_err(|error| entity_error(id, error))?
                .is_some()
            {
                return Ok(Some("3d"));
            }
            Ok(None)
        })?,
    )?;
    let s = Bindings::new(world);
    transform.set(
        "attach",
        lua.create_function(move |_, (id, space): (i64, String)| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            match space.as_str() {
                "2d" => {
                    if state
                        .world()?
                        .get::<Transform>(id)
                        .map_err(world_error)?
                        .is_some()
                    {
                        return Ok(false);
                    }
                    // 用统一的**压平**实现（`Transform3D::to_2d`），而不是只手抄一个 XY：
                    // 手抄会把旋转与缩放悄悄丢掉（3D 里转过 90° 的实体挂上 2D 变换后会突然
                    // 朝正面）。真能无损压平才建组件，压不平则明确报错——见下面的错误信息。
                    // 守卫只包住读：下面还要写同一个世界，读守卫留着会撞重入（P1 ⑤-7）。
                    let flattened = {
                        let world = state.world()?;
                        world
                            .get::<Transform3D>(id)
                            .map_err(world_error)?
                            .map(|source| source.to_2d())
                    };
                    match flattened {
                        Some(Some(flat)) => {
                            state.world_mut()?.insert(id, flat).map_err(world_error)?;
                            Ok(true)
                        }
                        Some(None) => Err(api_error(
                            "entity's 3D transform is not planar (tilt, z offset or z scale); \
                             flattening to 2d would drop it — clear those first, or keep using \
                             transform3d",
                        )),
                        None => {
                            state
                                .world_mut()?
                                .insert(id, Transform::default())
                                .map_err(world_error)?;
                            Ok(true)
                        }
                    }
                }
                "3d" => {
                    if state
                        .world()?
                        .get::<Transform3D>(id)
                        .map_err(world_error)?
                        .is_some()
                    {
                        return Ok(false);
                    }
                    // 同理走统一的**提升**实现（`Transform::to_3d`），因此 2D 的旋转与缩放
                    // 会被保留成绕 Z 的旋转与 XY 缩放。
                    // 同上：读守卫必须先结束。
                    let lifted = {
                        let world = state.world()?;
                        world
                            .get::<Transform>(id)
                            .map_err(world_error)?
                            .copied()
                            .map(Transform::to_3d)
                    };
                    let value = lifted.unwrap_or_default();
                    state.world_mut()?.insert(id, value).map_err(world_error)?;
                    Ok(true)
                }
                other => Err(api_error(format!(
                    "transform.attach space must be \"2d\" or \"3d\", got {other:?}"
                ))),
            }
        })?,
    )?;
    let s = Bindings::new(world);
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
                let mut world = s.world_mut()?;
                if let Some(t) = world
                    .get_mut::<Transform>(id)
                    .map_err(|error| entity_error(id, error))?
                {
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
                let t = world
                    .get_mut::<Transform3D>(id)
                    .map_err(|error| entity_error(id, error))?
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
    let s = Bindings::new(world);
    transform.set(
        "move",
        lua.create_function(move |_, (id, dx, dy): (i64, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut world = s.world_mut()?;
            if let Some(t) = world
                .get_mut::<Transform>(id)
                .map_err(|error| entity_error(id, error))?
            {
                t.position.x += dx;
                t.position.y += dy;
                return Ok(());
            }
            let t = world
                .get_mut::<Transform3D>(id)
                .map_err(|error| entity_error(id, error))?
                .ok_or_else(|| api_error("entity has no transform (2d or 3d)"))?;
            t.position.x += dx;
            t.position.y += dy;
            Ok(())
        })?,
    )?;
    lua.globals().set("transform", transform)?;
    Ok(())
}

/// M81：3D 包的 `transform3d` 表。领域 crate 不带 feature，由组合根 `register_all`
/// 在 `3d` 开启时才调用（2D 构建的 Lua 面没有这张表）。统一 `transform` 表留在基底：
/// 它按实体实际拥有的组件分派，是 docs/54 的单一事实源，不按维度裁剪。
pub fn install_3d(lua: &Lua, world: &Handle<World>) -> LuaResult<()> {
    let transform3d = lua.create_table()?;
    let s = Bindings::new(world);
    transform3d.set(
        "attach",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            state
                .world_mut()?
                .insert(id, Transform3D::default())
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Bindings::new(world);
    transform3d.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let world = s.world()?;
            let value = world
                .get::<Transform3D>(id)
                .map_err(|error| entity_error(id, error))?
                .ok_or_else(|| api_error("entity has no transform3d"))?;
            let t = lua.create_table_with_capacity(0, 10)?;
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
    let s = Bindings::new(world);
    transform3d.set(
        "position",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let world = s.world()?;
            local_xyz(&world, id)
        })?,
    )?;
    let s = Bindings::new(world);
    transform3d.set(
        "world_position",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let world = s.world()?;
            world_xyz(&world, id)
        })?,
    )?;
    let s = Bindings::new(world);
    transform3d.set(
        "set",
        lua.create_function(
            move |_, (id, x, y, z, rx, ry, rz, rw, sx, sy, sz): Transform3dSetArgs| {
                let id = entity_from_lua(id)?;
                let mut world = s.world_mut()?;
                // 保持旧顺序：先判句柄，再判数值，最后才查组件。
                if !world.is_alive(id) {
                    return Err(entity_error(id, WorldError::InvalidEntity(id)));
                }
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    return Err(api_error("transform3d.set position must be finite"));
                }
                let t = world
                    .get_mut::<Transform3D>(id)
                    .map_err(|error| entity_error(id, error))?
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
    let s = Bindings::new(world);
    transform3d.set(
        "move",
        lua.create_function(move |_, (id, dx, dy, dz): (i64, f32, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut world = s.world_mut()?;
            let t = world
                .get_mut::<Transform3D>(id)
                .map_err(|error| entity_error(id, error))?
                .ok_or_else(|| api_error("entity has no transform3d"))?;
            t.position.x += dx;
            t.position.y += dy;
            t.position.z += dz;
            Ok(())
        })?,
    )?;
    // docs/54 §8-⑦：`transform3d` 也要能读**世界**变换——`transform.world` 只覆盖"实体实际拥有
    // 的空间"，3D 实体走 `transform3d` 时此前拿不到父链组合后的位置（脚本只能自己推）。
    let s = Bindings::new(world);
    transform3d.set(
        "world",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let world_borrow = s.world()?;
            let world = crate::hierarchy::world_transform_3d_lazy(&world_borrow, id)
                .map_err(|error| entity_error(id, error))?
                .ok_or_else(|| api_error("entity has no transform3d"))?;
            let table = lua.create_table_with_capacity(0, 10)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 装好两张表的夹具。
    fn harness() -> (Lua, Handle<World>) {
        let lua = Lua::new();
        let world = Handle::new(World::new(), "world");
        install(&lua, &world).unwrap();
        // 领域 crate 无 feature：测试永远装全量面（门控只发生在组合根调用点）。
        install_3d(&lua, &world).unwrap();
        (lua, world)
    }

    /// 15 个函数一个不少（两张表的**清单**，少一个脚本就会静默拿到 nil）。
    #[test]
    fn every_transform_function_is_installed() {
        let (lua, _world) = harness();
        let mut total = 0;
        for table in ["transform", "transform3d"] {
            let table: mlua::Table = lua.globals().get(table).unwrap();
            total += table.pairs::<mlua::Value, mlua::Value>().count();
        }
        assert_eq!(total, 15, "transform + transform3d 合计 15 个函数");
    }

    /// 写进去的位置，下一次读出来必须一致（组件就在同一个 `World` 里）。
    #[test]
    fn set_and_get_roundtrip_through_the_shared_world() {
        let (lua, world) = harness();
        let id = world.with_mut(|world| {
            let id = world.spawn();
            world.insert(id, Transform::default()).unwrap();
            id
        });
        let raw = id.to_lua_i64();
        lua.load(format!("transform.set({raw}, 12, -3)"))
            .exec()
            .unwrap();
        let (x, y): (f32, f32) = lua
            .load(format!("local t = transform.get({raw}); return t.x, t.y"))
            .eval()
            .unwrap();
        assert_eq!((x, y), (12.0, -3.0));
        // 同一份数据，宿主侧读到的也一致。
        let position = world.with(|world| world.get::<Transform>(id).unwrap().unwrap().position);
        assert_eq!((position.x, position.y), (12.0, -3.0));
    }

    /// 标量位置 API：不建每实体 table，2D/3D 与局部/世界都读同一个组件事实。
    #[test]
    fn scalar_positions_match_get() {
        let (lua, world) = harness();
        let (flat, deep, child) = world.with_mut(|world| {
            let flat = world.spawn();
            world.insert(flat, Transform::default()).unwrap();
            let deep = world.spawn();
            world.insert(deep, Transform3D::default()).unwrap();
            world.get_mut::<Transform>(flat).unwrap().unwrap().position = Vec2::new(4.0, 5.0);
            world
                .get_mut::<Transform3D>(deep)
                .unwrap()
                .unwrap()
                .position = Vec3::new(6.0, 7.0, 8.0);
            let child = world.spawn();
            world.insert(child, Transform3D::default()).unwrap();
            world
                .get_mut::<Transform3D>(child)
                .unwrap()
                .unwrap()
                .position = Vec3::new(1.0, 2.0, 3.0);
            world.insert(child, crate::hierarchy::Parent(deep)).unwrap();
            (flat, deep, child)
        });
        let flat = flat.to_lua_i64();
        let deep = deep.to_lua_i64();
        let child = child.to_lua_i64();
        let (x, y, wx, wy, dx, dy, dz, cx, cy, cz) = lua
            .load(format!(
                r#"
                local x, y = transform.position({flat})
                local wx, wy = transform.world_position({flat})
                local dx, dy, dz = transform3d.position({deep})
                local cx, cy, cz = transform3d.world_position({child})
                return x, y, wx, wy, dx, dy, dz, cx, cy, cz
                "#
            ))
            .eval()
            .unwrap();
        assert_eq!(
            (x, y, wx, wy, dx, dy, dz, cx, cy, cz),
            (4.0, 5.0, 4.0, 5.0, 6.0, 7.0, 8.0, 7.0, 9.0, 11.0)
        );
    }

    /// 不平移的实体：错误串必须点名"没有变换"，而不是笼统的 nil。
    #[test]
    fn dead_entity_is_rejected_before_touching_the_world() {
        let (lua, _world) = harness();
        let dead = EntityId::from_raw(999, 1).to_lua_i64();
        for call in [
            format!("return transform.get({dead})"),
            format!("return transform.set({dead}, 1, 2)"),
            format!("return transform3d.get({dead})"),
            format!("return transform3d.set({dead}, 1, 2, 3)"),
        ] {
            let message = lua
                .load(call)
                .eval::<mlua::Value>()
                .expect_err("死句柄必须报错")
                .to_string();
            assert!(message.contains("invalid entity id"), "{message}");
        }
    }

    /// 重入：宿主攥着世界可写借用时调 Lua，脚本收到点名 `world` 的错（不是 panic）。
    #[test]
    fn reentrant_borrow_names_the_world() {
        let (lua, world) = harness();
        // 必须是**活实体**：死句柄会在 `entity_from_lua` 就报错，走不到借世界那一步。
        let id = world.with_mut(|world| {
            let id = world.spawn();
            world.insert(id, Transform::default()).unwrap();
            id
        });
        let _guard = world.borrow_mut().unwrap();
        let message = lua
            .load(format!("return transform.get({})", id.to_lua_i64()))
            .exec()
            .unwrap_err()
            .to_string();
        assert!(message.contains("already borrowed"), "{message}");
        assert!(message.contains("world"), "{message}");
    }
}
