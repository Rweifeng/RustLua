//! 物理：`physics`（2D 刚体与碰撞查询）、`physics3d`、`particles`（粒子发射器）。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::{api_error, world_error};
use crate::script::values::{entity_from_lua, entity_to_lua, query_groups, require_alive};
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_ecs::particle::ParticleEmitter;
use rustlua_ecs::transform::Transform;
use rustlua_ecs::transform::Transform3D;
use rustlua_ecs::transform::Vec2;
use rustlua_math::math3d::Vec3;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    // P2-②：2D/3D 结构完全同形的绑定收敛成宏（ccd 对、events、debug_draw），
    // 两条路径不再各抄一份——修一处即两边生效。
    macro_rules! bind_ccd_pair {
        ($table:expr, $world:ident) => {{
            let s = Rc::clone(&state);
            $table.set(
                "set_ccd",
                lua.create_function(move |_, (id, enabled): (i64, bool)| {
                    let id = entity_from_lua(id)?;
                    let mut state = s.lua_borrow_mut()?;
                    require_alive(&state, id)?;
                    state
                        .physics
                        .$world
                        .set_ccd(id, enabled)
                        .map_err(|error| api_error(error.to_string()))?;
                    Ok(())
                })?,
            )?;
            let s = Rc::clone(&state);
            $table.set(
                "get_ccd",
                lua.create_function(move |_, id: i64| {
                    let id = entity_from_lua(id)?;
                    let state = s.lua_borrow()?;
                    require_alive(&state, id)?;
                    state
                        .physics
                        .$world
                        .ccd_enabled(id)
                        .map_err(|error| api_error(error.to_string()))
                })?,
            )?;
        }};
    }
    macro_rules! bind_events {
        ($table:expr, $world:ident) => {
            let s = Rc::clone(&state);
            $table.set(
                "events",
                lua.create_function(move |lua, ()| {
                    let mut state = s.lua_borrow_mut()?;
                    let events = state.physics.$world.drain_contacts();
                    let result = lua.create_table()?;
                    for (index, contact) in events.into_iter().enumerate() {
                        let entry = lua.create_table()?;
                        entry.set("a", entity_to_lua(contact.a))?;
                        entry.set("b", entity_to_lua(contact.b))?;
                        entry.set("started", contact.started)?;
                        result.set(index + 1, entry)?;
                    }
                    Ok(result)
                })?,
            )?;
        };
    }
    macro_rules! bind_debug_draw {
        ($table:expr, $request_field:ident) => {
            let s = Rc::clone(&state);
            $table.set(
                "debug_draw",
                lua.create_function(move |_, enabled: bool| {
                    let mut state = s.lua_borrow_mut()?;
                    state.physics.$request_field = Some(enabled);
                    Ok(enabled)
                })?,
            )?;
        };
    }
    // P2-② 尾款（`docs/60` §5.2）：`set_velocity` 这一对 2D/3D 绑定**逐字同形**（都是
    // entity_from_lua → 借 state → require_alive → 转发到物理世界），只有**分量数**不同，
    // 用分量重复合成一处。之所以只做这一对——其余三对逐行复核**并非同形**：
    //   · `overlap`：2D 校验 `radius`，3D **不校验**（同名不同守，见上一轮已记的缺口）；
    //   · `raycast`：2D 返回 `(entity, distance)`，3D 只返回实体；
    //   · `set_position`：2D 的 `Transform`/`Vec2` 对上 3D 的 `Transform3D`/`Vec3`；
    //   · `get_velocity`：数字下标 `[1]/[2]` 只有 2D 有（改它要动运行期 API 面）。
    // 宏只能靠"再加几个参数"把这些差异盖住——那是**把差异藏起来**，比留两份更危险。
    macro_rules! bind_set_velocity_pair {
        ($table:expr, $world:ident, ($($component:ident),+)) => {{
            let s = Rc::clone(&state);
            $table.set(
                "set_velocity",
                // 不写参数类型：`create_function` 的 `A: FromLuaMulti` 约束会推出来
                // （分量类型由下面的 `set_velocity` 调用定死）。写显式类型则要在重复里
                // 带上 `f32`，而那个重复**不含元变量**——macro_rules 不接受。
                lua.create_function(move |_, (id $(, $component)+)| {
                    let id = entity_from_lua(id)?;
                    let mut state = s.lua_borrow_mut()?;
                    require_alive(&state, id)?;
                    state
                        .physics
                        .$world
                        .set_velocity(id $(, $component)+)
                        .map_err(|error| api_error(error.to_string()))?;
                    Ok(())
                })?,
            )?;
        }};
    }
    // 命中实体列表 → Lua 数组表（overlap / positions 系查询共用）。
    fn entities_table(
        lua: &Lua,
        ids: impl IntoIterator<Item = rustlua_ecs::entity::EntityId>,
    ) -> LuaResult<mlua::Table> {
        let result = lua.create_table()?;
        for (index, id) in ids.into_iter().enumerate() {
            result.set(index + 1, entity_to_lua(id))?;
        }
        Ok(result)
    }

    let physics = lua.create_table()?;
    let s = Rc::clone(&state);
    // 场景空间**宽相**：按实体 `Transform` 位置做圆查询。它与 `physics.overlap`（碰撞体查询）
    // 是**两件事**，因此有各自的名字——`physics.overlap` 以前就是这个位置查询，与 3D 同名不同义；
    // 现在名字与语义一一对应（docs/54 §8-⑥ 的"查询契约统一"）。
    physics.set(
        "positions_in_radius",
        lua.create_function(move |lua, (x, y, radius): (f32, f32, f32)| {
            if !x.is_finite() || !y.is_finite() || !radius.is_finite() || radius < 0.0 {
                return Err(api_error(
                    "physics.positions_in_radius arguments must be finite",
                ));
            }
            let state = s.lua_borrow()?;
            let result = lua.create_table()?;
            let mut n = 1;
            for id in state.world.entities_with::<Transform>() {
                if let Some(t) = state.world.get::<Transform>(id).map_err(world_error)? {
                    let dx = t.position.x - x;
                    let dy = t.position.y - y;
                    if dx * dx + dy * dy <= radius * radius {
                        result.set(n, entity_to_lua(id))?;
                        n += 1;
                    }
                }
            }
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "overlap",
        lua.create_function(
            move |lua, (x, y, radius, opts): (f32, f32, f32, Option<mlua::Table>)| {
                if !radius.is_finite() || radius < 0.0 {
                    return Err(api_error("overlap radius must be finite and non-negative"));
                }
                let groups = query_groups(opts)?;
                // P3-③：查询可能要兑现延迟的 BVH 重建，故取可变借用。
                let mut state = s.lua_borrow_mut()?;
                // docs/54 §8-⑥：走 Rapier 的碰撞体查询（形状/偏移/是否 sensor 都参与），与 3D 的
                // `physics3d.overlap` 是同一个东西。此前这里是"实体位置距离判定"——同一个名字
                // 在 2D/3D 下语义不同。
                let ids = state
                    .physics
                    .world2d
                    .overlap_filtered(Vec2::new(x, y), radius, groups);
                entities_table(lua, ids)
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "raycast",
        lua.create_function(
            move |_,
                  (x, y, dx, dy, max_distance, opts): (
                f32,
                f32,
                f32,
                f32,
                f32,
                Option<mlua::Table>,
            )| {
                if !max_distance.is_finite() || max_distance < 0.0 {
                    return Err(api_error(
                        "raycast distance must be finite and non-negative",
                    ));
                }
                if !x.is_finite() || !y.is_finite() || !dx.is_finite() || !dy.is_finite() {
                    return Err(api_error("raycast arguments must be finite"));
                }
                let groups = query_groups(opts)?;
                // P3-③：查询可能要兑现延迟的 BVH 重建，故取可变借用。
                let mut state = s.lua_borrow_mut()?;
                // 与 `physics3d.raycast` 同构：命中**碰撞体**，返回最近的那个实体。
                match state.physics.world2d.raycast_filtered(
                    Vec2::new(x, y),
                    Vec2::new(dx, dy),
                    max_distance,
                    groups,
                ) {
                    Ok(Some((entity, _distance))) => Ok(Some(entity_to_lua(entity))),
                    Ok(None) => Ok(None),
                    Err(error) => Err(api_error(error.to_string())),
                }
            },
        )?,
    )?;
    bind_set_velocity_pair!(physics, world2d, (x, y));
    let s = Rc::clone(&state);
    physics.set(
        "get_velocity",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let velocity = state
                .physics
                .world2d
                .velocity(id)
                .map_err(|error| api_error(error.to_string()))?;
            // docs/54 §8-⑥：2D 与 3D 的速度表**同名同形**（`{x=, y=}`）。数值下标（`[1]`/`[2]`）
            // 继续可用，因此既有脚本零改动，而两份 API 不再是"一个数组一个命名键"。
            let table = lua.create_table()?;
            table.set(1, velocity.x)?;
            table.set(2, velocity.y)?;
            table.set("x", velocity.x)?;
            table.set("y", velocity.y)?;
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "set_position",
        lua.create_function(move |_, (id, x, y): (i64, f32, f32)| {
            let id = entity_from_lua(id)?;
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("physics.set_position must be finite"));
            }
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            state
                .physics
                .world2d
                .set_position(id, Vec2::new(x, y))
                .map_err(|error| api_error(error.to_string()))?;
            // Keep the ECS transform aligned so rendering/Lua reads agree with
            // the body until the next physics step writes positions back.
            if let Ok(Some(transform)) = state.world.get_mut::<Transform>(id) {
                transform.position = Vec2::new(x, y);
            }
            Ok(())
        })?,
    )?;
    bind_ccd_pair!(physics, world2d);
    bind_events!(physics, world2d);
    bind_debug_draw!(physics, collider_debug_request);
    lua.globals().set("physics", physics)?;
    // 3D physics (M9-02): same shape as `physics`, but bodies are created from
    // scene `Collider3d` components and positions sync to `Transform3D`.

    let physics3d = lua.create_table()?;
    bind_set_velocity_pair!(physics3d, world3d, (x, y, z));
    let s = Rc::clone(&state);
    physics3d.set(
        "get_velocity",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let velocity = state
                .physics
                .world3d
                .velocity(id)
                .map_err(|error| api_error(error.to_string()))?;
            let table = lua.create_table()?;
            table.set("x", velocity.x)?;
            table.set("y", velocity.y)?;
            table.set("z", velocity.z)?;
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "set_position",
        lua.create_function(move |_, (id, x, y, z): (i64, f32, f32, f32)| {
            let id = entity_from_lua(id)?;
            // docs/60 §5.2 复核：这一层校验 2D 有（`physics.set_position`）、3D 此前没有，
            // 于是 NaN/Inf 会直接推进 Rapier。同名 API 不该一边拦一边放。
            if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                return Err(api_error("physics3d.set_position must be finite"));
            }
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            state
                .physics
                .world3d
                .set_position(id, Vec3::new(x, y, z))
                .map_err(|error| api_error(error.to_string()))?;
            // Keep the ECS transform aligned so rendering/Lua reads agree with
            // the body until the next physics step writes positions back.
            // （2D 早就这么做；3D 此前漏了，导致 `physics3d.set_position` 之后
            // 渲染/Lua 读到的位置与刚体不一致，直到下一次 step。）
            if let Ok(Some(transform)) = state.world.get_mut::<Transform3D>(id) {
                transform.position = Vec3::new(x, y, z);
            }
            Ok(())
        })?,
    )?;
    bind_events!(physics3d, world3d);
    let s = Rc::clone(&state);
    physics3d.set(
        "raycast",
        lua.create_function(
            move |_,
                  (ox, oy, oz, dx, dy, dz, max_distance, opts): (
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                Option<mlua::Table>,
            )| {
                // docs/60 §5.2 复核：2D 的 `physics.raycast` 这里有两道有限性校验，3D 此前
                // 一道都没有——同一件事（喂 NaN 给 Rapier）不该只有 2D 拦。
                if !max_distance.is_finite() || max_distance < 0.0 {
                    return Err(api_error(
                        "raycast distance must be finite and non-negative",
                    ));
                }
                if !ox.is_finite()
                    || !oy.is_finite()
                    || !oz.is_finite()
                    || !dx.is_finite()
                    || !dy.is_finite()
                    || !dz.is_finite()
                {
                    return Err(api_error("raycast arguments must be finite"));
                }
                let groups = query_groups(opts)?;
                // P3-③（与 2D 同一处、同一理由）：查询可能要兑现延迟的 BVH 重建，故取可变借用。
                let mut state = s.lua_borrow_mut()?;
                let hit = state
                    .physics
                    .world3d
                    .raycast_filtered(
                        Vec3::new(ox, oy, oz),
                        Vec3::new(dx, dy, dz),
                        max_distance,
                        groups,
                    )
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(hit.map(entity_to_lua))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "overlap",
        lua.create_function(
            move |lua, (cx, cy, cz, radius, opts): (f32, f32, f32, f32, Option<mlua::Table>)| {
                let groups = query_groups(opts)?;
                // P3-③（与 2D 同一处、同一理由）：查询可能要兑现延迟的 BVH 重建，故取可变借用。
                let mut state = s.lua_borrow_mut()?;
                let hits =
                    state
                        .physics
                        .world3d
                        .overlap_filtered(Vec3::new(cx, cy, cz), radius, groups);
                entities_table(lua, hits)
            },
        )?,
    )?;
    bind_ccd_pair!(physics3d, world3d);
    bind_debug_draw!(physics3d, collider_debug_request_3d);
    lua.globals().set("physics3d", physics3d)?;

    let particles = lua.create_table()?;
    let s = Rc::clone(&state);
    // `particles.burst(x, y, count [, opts])`：opts 是可选表，用来给这一次爆发不同的
    // 观感（此前引擎把速度/寿命/颜色**写死**在 120/0.6/橙色，Lua 完全无法区分尘土、
    // 水花、落叶）。缺省值与旧行为一致，因此老脚本零改动。
    particles.set(
        "burst",
        lua.create_function(
            move |_, (x, y, count, options): (f32, f32, u32, Option<mlua::Table>)| {
                if !x.is_finite() || !y.is_finite() {
                    return Err(api_error("particles.burst position must be finite"));
                }
                let mut emitter =
                    ParticleEmitter::new(count as usize, 120.0, 0.6, [1.0, 0.6, 0.2, 1.0]);
                if let Some(options) = options {
                    let number = |key: &str| -> Result<Option<f32>, mlua::Error> {
                        options.get::<Option<f32>>(key)
                    };
                    let color = |key: &str| -> Result<Option<[f32; 4]>, mlua::Error> {
                        let Some(table) = options.get::<Option<mlua::Table>>(key)? else {
                            return Ok(None);
                        };
                        Ok(Some([
                            table.get::<Option<f32>>("r")?.unwrap_or(1.0),
                            table.get::<Option<f32>>("g")?.unwrap_or(1.0),
                            table.get::<Option<f32>>("b")?.unwrap_or(1.0),
                            table.get::<Option<f32>>("a")?.unwrap_or(1.0),
                        ]))
                    };
                    if let Some(value) = number("speed")? {
                        emitter.speed = value;
                    }
                    if let Some(value) = number("life")? {
                        emitter.life = value;
                    }
                    if let Some(value) = number("size")? {
                        emitter.size_start = value;
                        emitter.size_end = value;
                    }
                    if let Some(value) = number("size_end")? {
                        emitter.size_end = value;
                    }
                    if let Some(value) = color("color")? {
                        emitter.color_start = value;
                        emitter.color_end = value;
                    }
                    if let Some(value) = color("color_end")? {
                        emitter.color_end = value;
                    }
                    emitter.gravity = Vec2::new(
                        number("gravity_x")?.unwrap_or(0.0),
                        number("gravity_y")?.unwrap_or(0.0),
                    );
                    if let Some(value) = number("drag")? {
                        emitter.drag = value;
                    }
                    if let Some(value) = number("rotation")? {
                        emitter.rotation = value;
                    }
                    if let Some(value) = number("rotation_speed")? {
                        emitter.rotation_speed = value;
                    }
                }
                emitter.validate().map_err(api_error)?;
                s.lua_borrow_mut()?
                    .particles
                    .burst_with(Vec2::new(x, y), &emitter);
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    particles.set(
        "count",
        lua.create_function(move |_, ()| Ok(s.lua_borrow()?.particles.len() as u32))?,
    )?;
    lua.globals().set("particles", particles)?;
    Ok(())
}
