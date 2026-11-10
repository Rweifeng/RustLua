//! `physics` / `physics3d` 两张表的 Lua 绑定（P1 ⑤-9）。
//!
//! 两个表的构造形态**刻意逐字同构**（P2-② 的收敛结果），差异只有三处，且都是真实差异：
//! `overlap` 的 2D 版校验 `radius`（3D 版也校验，但错误串不同）、`raycast` 的 2D 版返回
//! `(entity, distance)` 而 3D 版只返回实体、`set_position` 的组件类型不同。宏只盖住**同形**
//! 的部分（ccd 对、events、debug_draw、set_velocity），其余逐条写——把差异藏进参数里比留两份
//! 更危险（理由见原 `script/api/physics.rs` 的注释，已随搬迁保留在下面的宏上）。
//!
//! 与相机域（⑤-8）相比多一个依赖面：这里既要 `Handle<PhysicsState>`（模拟世界与回写队列），
//! 也要 `Handle<World>`（`require_alive`、`positions_in_radius` 的 ECS 遍历、`set_position`
//! 之后把 `Transform` 对齐）。两者都是句柄，因此**不会**互相借住 `&mut` 的状态。

use crate::physics_state::PhysicsState;
use mlua::{Lua, Result as LuaResult, Table};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::transform::{Transform, Transform3D};
use rustlua_ecs::world::World;
use rustlua_lua_api::{Handle, api_error};
use rustlua_math::math2d::Vec2;
use rustlua_math::math3d::Vec3;

/// Lua 侧实体 id → 实体句柄。错误串与脚本层逐字相同（负数与"句柄形态不对"分开报）。
fn entity_from_lua(value: i64) -> LuaResult<EntityId> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    EntityId::from_lua_i64(value).ok_or_else(|| api_error(format!("invalid entity id: {value}")))
}

fn entity_to_lua(entity: EntityId) -> i64 {
    entity.to_lua_i64()
}

/// 写操作的前置校验：实体必须还活着。
fn require_alive(world: &World, entity: EntityId) -> LuaResult<()> {
    if world.is_alive(entity) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {entity}")))
    }
}

/// 解析查询的 `{ layer = N, mask = N }` 参数（`docs/54` §8-⑥：查询也吃 `layer/mask`）。
///
/// - 缺省/空表 = `None`（全收，老行为）；
/// - 只给一侧时另一侧取 `0xFF`；
/// - 越界或非整数**报错**：位域是 u8，静默截断会让 `layer=256` 变成"全层可见"。
///
/// 这段实现原先在 `rustlua-scripting::script::values`（P1 ⑤-9 随绑定搬来），错误串与语义
/// 逐字保留——它不是"新写法"，所以不要顺手改成 `Option<u32>` 之类（会换掉报错文案）。
fn query_groups(options: Option<Table>) -> LuaResult<Option<(u8, u8)>> {
    let Some(options) = options else {
        return Ok(None);
    };
    let read = |key: &str| -> LuaResult<Option<u8>> {
        let Some(raw) = options.get::<Option<i64>>(key)? else {
            return Ok(None);
        };
        u8::try_from(raw).map(Some).map_err(|_| {
            api_error(format!(
                "query {key} must be an integer in 0..=255 (bit field), got {raw}"
            ))
        })
    };
    let layer = read("layer")?;
    let mask = read("mask")?;
    if layer.is_none() && mask.is_none() {
        return Ok(None);
    }
    Ok(Some((layer.unwrap_or(0xFF), mask.unwrap_or(0xFF))))
}

fn entities_table(lua: &Lua, ids: impl IntoIterator<Item = EntityId>) -> LuaResult<Table> {
    let result = lua.create_table()?;
    for (index, id) in ids.into_iter().enumerate() {
        result.set(index + 1, entity_to_lua(id))?;
    }
    Ok(result)
}

/// 宏参数用的访问器：`&mut PhysicsState -> &mut PhysicsWorld`。写成函数项而不是闭包——
/// 闭包返回引用时 rustc 推不出 `for<'a>` 的高阶约束（会报"lifetime may not live long enough"）。
fn world2d_mut(physics: &mut PhysicsState) -> &mut crate::physics::PhysicsWorld {
    physics.world2d_mut()
}

fn world3d_mut(physics: &mut PhysicsState) -> &mut crate::physics3d::PhysicsWorld3d {
    physics.world3d_mut()
}

// P2-②：2D/3D 结构完全同形的绑定收敛成宏（ccd 对、events、debug_draw、set_velocity），
// 两条路径不再各抄一份——修一处即两边生效。
// M81：宏从 install 函数体提到模块级——install_3d 拆出去后仍要共用这同一份。
// macro_rules 的局部变量卫生性按**定义点**解析，裸写 physics/world/lua 会找不到值；
// 因此把三个名字作为 ident 参数从调用点传入（两个函数的参数同名同型）。宏体里再
// `let lua = $lua;` 落回裸名：docgen 的绑定点正则只认裸 lua 句柄的字面调用形状。
macro_rules! bind_ccd_pair {
    ($table:expr, $access:expr, $physics:ident, $world:ident, $lua:ident) => {{
        let lua = $lua;
        let p = $physics.clone();
        let w = $world.clone();
        let access = $access;
        $table.set(
            "set_ccd",
            lua.create_function(move |_, (id, enabled): (i64, bool)| {
                let id = entity_from_lua(id)?;
                {
                    let world = w.borrow()?;
                    require_alive(&world, id)?;
                }
                let mut physics = p.borrow_mut()?;
                access(&mut physics)
                    .set_ccd(id, enabled)
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(())
            })?,
        )?;
        let p = $physics.clone();
        let w = $world.clone();
        let access = $access;
        $table.set(
            "get_ccd",
            lua.create_function(move |_, id: i64| {
                let id = entity_from_lua(id)?;
                {
                    let world = w.borrow()?;
                    require_alive(&world, id)?;
                }
                let mut physics = p.borrow_mut()?;
                access(&mut physics)
                    .ccd_enabled(id)
                    .map_err(|error| api_error(error.to_string()))
            })?,
        )?;
    }};
}
macro_rules! bind_events {
    ($table:expr, $access:expr, $physics:ident, $lua:ident) => {{
        let lua = $lua;
        let p = $physics.clone();
        let access = $access;
        $table.set(
            "events",
            lua.create_function(move |lua, ()| {
                let events = {
                    let mut physics = p.borrow_mut()?;
                    access(&mut physics).drain_contacts()
                };
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
    }};
}
macro_rules! bind_debug_draw {
    ($table:expr, $request:ident, $physics:ident, $lua:ident) => {{
        let lua = $lua;
        let p = $physics.clone();
        $table.set(
            "debug_draw",
            lua.create_function(move |_, enabled: bool| {
                let mut physics = p.borrow_mut()?;
                Ok(physics.$request(enabled))
            })?,
        )?;
    }};
}
// P2-② 尾款（`docs/60` §5.2）：`set_velocity` 这一对 2D/3D 绑定**逐字同形**（都是
// entity_from_lua → require_alive → 转发到物理世界），只有**分量数**不同，用分量重复合成
// 一处。其余三对逐行复核**并非同形**（overlap/raycast/set_position 的返回形与组件类型不同），
// 宏只能靠"再加几个参数"盖住差异——那是把差异藏起来，比留两份更危险。
macro_rules! bind_set_velocity_pair {
    ($table:expr, $access:expr, ($($component:ident),+), $physics:ident, $world:ident, $lua:ident) => {{
        let lua = $lua;
        let p = $physics.clone();
        let w = $world.clone();
        let access = $access;
        $table.set(
            "set_velocity",
            lua.create_function(move |_, (id $(, $component)+)| {
                let id = entity_from_lua(id)?;
                {
                    let world = w.borrow()?;
                    require_alive(&world, id)?;
                }
                let mut physics = p.borrow_mut()?;
                access(&mut physics)
                    .set_velocity(id $(, $component)+)
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(())
            })?,
        )?;
    }};
}

/// 装 `physics`（2D）基底全局表（M81：`physics3d` 拆去 [`install_3d`]）。
pub fn install(lua: &Lua, physics: &Handle<PhysicsState>, world: &Handle<World>) -> LuaResult<()> {
    let table = lua.create_table()?;
    // 场景空间**宽相**：按实体 `Transform` 位置做圆查询。它与 `physics.overlap`（碰撞体查询）
    // 是**两件事**，因此有各自的名字——`docs/54` §8-⑥ 的"查询契约统一"。
    {
        let w = world.clone();
        table.set(
            "positions_in_radius",
            lua.create_function(move |lua, (x, y, radius): (f32, f32, f32)| {
                if !x.is_finite() || !y.is_finite() || !radius.is_finite() || radius < 0.0 {
                    return Err(api_error(
                        "physics.positions_in_radius arguments must be finite",
                    ));
                }
                let world = w.borrow()?;
                let result = lua.create_table()?;
                let mut n = 1;
                let mut failure = None;
                world.for_each::<Transform>(|id, transform| {
                    let dx = transform.position.x - x;
                    let dy = transform.position.y - y;
                    if failure.is_none() && dx * dx + dy * dy <= radius * radius {
                        if let Err(error) = result.set(n, entity_to_lua(id)) {
                            failure = Some(error);
                        } else {
                            n += 1;
                        }
                    }
                });
                if let Some(error) = failure {
                    return Err(error);
                }
                Ok(result)
            })?,
        )?;
    }
    {
        let p = physics.clone();
        table.set(
            "overlap",
            lua.create_function(
                move |lua, (x, y, radius, opts): (f32, f32, f32, Option<Table>)| {
                    if !radius.is_finite() || radius < 0.0 {
                        return Err(api_error("overlap radius must be finite and non-negative"));
                    }
                    let groups = query_groups(opts)?;
                    // P3-③：查询可能要兑现延迟的 BVH 重建，故取可变借用。
                    let mut physics = p.borrow_mut()?;
                    // docs/54 §8-⑥：走 Rapier 的碰撞体查询（形状/偏移/是否 sensor 都参与），
                    // 与 `physics3d.overlap` 是同一个东西。
                    let ids =
                        physics
                            .world2d_mut()
                            .overlap_filtered(Vec2::new(x, y), radius, groups);
                    entities_table(lua, ids)
                },
            )?,
        )?;
    }
    {
        let p = physics.clone();
        table.set(
            "raycast",
            lua.create_function(
                move |_,
                      (x, y, dx, dy, max_distance, opts): (
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    Option<Table>,
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
                    let mut physics = p.borrow_mut()?;
                    // 与 `physics3d.raycast` 同构：命中**碰撞体**，返回最近的那个实体。
                    match physics.world2d_mut().raycast_filtered(
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
    }
    bind_set_velocity_pair!(table, world2d_mut, (x, y), physics, world, lua);
    {
        let p = physics.clone();
        let w = world.clone();
        table.set(
            "get_velocity",
            lua.create_function(move |lua, id: i64| {
                let id = entity_from_lua(id)?;
                {
                    let world = w.borrow()?;
                    require_alive(&world, id)?;
                }
                let velocity = {
                    let physics = p.borrow()?;
                    physics
                        .world2d()
                        .velocity(id)
                        .map_err(|error| api_error(error.to_string()))?
                };
                // docs/54 §8-⑥：2D 与 3D 的速度表**同名同形**（`{x=, y=}`）。数值下标
                // （`[1]`/`[2]`）继续可用，因此既有脚本零改动。
                let table = lua.create_table()?;
                table.set(1, velocity.x)?;
                table.set(2, velocity.y)?;
                table.set("x", velocity.x)?;
                table.set("y", velocity.y)?;
                Ok(table)
            })?,
        )?;
    }
    {
        let p = physics.clone();
        let w = world.clone();
        table.set(
            "set_position",
            lua.create_function(move |_, (id, x, y): (i64, f32, f32)| {
                let id = entity_from_lua(id)?;
                if !x.is_finite() || !y.is_finite() {
                    return Err(api_error("physics.set_position must be finite"));
                }
                {
                    let world = w.borrow()?;
                    require_alive(&world, id)?;
                }
                {
                    let mut physics = p.borrow_mut()?;
                    physics
                        .world2d_mut()
                        .set_position(id, Vec2::new(x, y))
                        .map_err(|error| api_error(error.to_string()))?;
                }
                // Keep the ECS transform aligned so rendering/Lua reads agree with
                // the body until the next physics step writes positions back.
                if let Ok(Some(transform)) = w.borrow_mut()?.get_mut::<Transform>(id) {
                    transform.position = Vec2::new(x, y);
                }
                Ok(())
            })?,
        )?;
    }
    bind_ccd_pair!(table, world2d_mut, physics, world, lua);
    bind_events!(table, world2d_mut, physics, lua);
    bind_debug_draw!(table, request_debug_draw, physics, lua);
    lua.globals().set("physics", table)?;
    Ok(())
}

/// M81：3D 包的 `physics3d` 表。领域 crate 不带 feature，由组合根 `register_all`
/// 在 `3d` 开启时才调用（2D 构建的 Lua 面没有这张表）。
pub fn install_3d(
    lua: &Lua,
    physics: &Handle<PhysicsState>,
    world: &Handle<World>,
) -> LuaResult<()> {
    // 3D physics (M9-02): same shape as `physics`, but bodies are created from
    // scene `Collider3d` components and positions sync to `Transform3D`.
    let table = lua.create_table()?;
    bind_set_velocity_pair!(table, world3d_mut, (x, y, z), physics, world, lua);
    {
        let p = physics.clone();
        let w = world.clone();
        table.set(
            "get_velocity",
            lua.create_function(move |lua, id: i64| {
                let id = entity_from_lua(id)?;
                {
                    let world = w.borrow()?;
                    require_alive(&world, id)?;
                }
                let velocity = {
                    let physics = p.borrow()?;
                    physics
                        .world3d()
                        .velocity(id)
                        .map_err(|error| api_error(error.to_string()))?
                };
                let table = lua.create_table()?;
                table.set("x", velocity.x)?;
                table.set("y", velocity.y)?;
                table.set("z", velocity.z)?;
                Ok(table)
            })?,
        )?;
    }
    {
        let p = physics.clone();
        let w = world.clone();
        table.set(
            "set_position",
            lua.create_function(move |_, (id, x, y, z): (i64, f32, f32, f32)| {
                let id = entity_from_lua(id)?;
                // docs/60 §5.2 复核：这一层校验 2D 有、3D 此前没有，于是 NaN/Inf 会直接推进
                // Rapier。同名 API 不该一边拦一边放。
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    return Err(api_error("physics3d.set_position must be finite"));
                }
                {
                    let world = w.borrow()?;
                    require_alive(&world, id)?;
                }
                {
                    let mut physics = p.borrow_mut()?;
                    physics
                        .world3d_mut()
                        .set_position(id, Vec3::new(x, y, z))
                        .map_err(|error| api_error(error.to_string()))?;
                }
                // Keep the ECS transform aligned … （2D 早就这么做；3D 此前漏了，导致
                // `physics3d.set_position` 之后渲染/Lua 读到的位置与刚体不一致。）
                if let Ok(Some(transform)) = w.borrow_mut()?.get_mut::<Transform3D>(id) {
                    transform.position = Vec3::new(x, y, z);
                }
                Ok(())
            })?,
        )?;
    }
    bind_events!(table, world3d_mut, physics, lua);
    bind_ccd_pair!(table, world3d_mut, physics, world, lua);
    bind_debug_draw!(table, request_debug_draw_3d, physics, lua);
    {
        let p = physics.clone();
        let w = world.clone();
        table.set(
            "add_heightfield",
            lua.create_function(
                move |_,
                      (id, x, y, z, rows, cols, sx, sy, sz, heights): (
                    i64,
                    f32,
                    f32,
                    f32,
                    u32,
                    u32,
                    f32,
                    f32,
                    f32,
                    Table,
                )| {
                    let id = entity_from_lua(id)?;
                    if rows < 2 || cols < 2 || rows > 1025 || cols > 1025 {
                        return Err(api_error(
                            "physics3d.add_heightfield rows/cols must be 2..=1025",
                        ));
                    }
                    if !x.is_finite()
                        || !y.is_finite()
                        || !z.is_finite()
                        || !sx.is_finite()
                        || !sy.is_finite()
                        || !sz.is_finite()
                    {
                        return Err(api_error(
                            "physics3d.add_heightfield position/scale must be finite",
                        ));
                    }
                    let heights: Vec<f32> = heights
                        .sequence_values::<f32>()
                        .collect::<mlua::Result<Vec<f32>>>()?;
                    if heights.len() != (rows as usize) * (cols as usize) {
                        return Err(api_error(format!(
                            "physics3d.add_heightfield needs {} heights, got {}",
                            rows as usize * cols as usize,
                            heights.len()
                        )));
                    }
                    {
                        let world = w.borrow()?;
                        require_alive(&world, id)?;
                    }
                    p.borrow_mut()?
                        .world3d_mut()
                        .add_heightfield(
                            id,
                            Vec3::new(x, y, z),
                            Vec3::new(sx, sy, sz),
                            rows as usize,
                            cols as usize,
                            heights,
                        )
                        .map_err(|error| api_error(error.to_string()))?;
                    Ok(())
                },
            )?,
        )?;
    }
    {
        let p = physics.clone();
        let wc = world.clone();
        table.set(
            "add_cells",
            lua.create_function(move |_, (id, opts): (i64, Table)| {
                // docs/18 的 `cells3d.build(opts)` 同一张 opts：Myrmex 只要把同一个表分别交给
                // 渲染与物理，两边看到的是同一份位图，不存在"画出来一堵墙、物理里没有"。
                let id = entity_from_lua(id)?;
                let cell: f32 = opts
                    .get("cell")
                    .map_err(|_| api_error("physics3d.add_cells needs a numeric cell"))?;
                let x: f32 = opts
                    .get("x")
                    .map_err(|_| api_error("physics3d.add_cells needs a numeric x"))?;
                let y: f32 = opts
                    .get("y")
                    .map_err(|_| api_error("physics3d.add_cells needs a numeric y"))?;
                let z: f32 = opts
                    .get("z")
                    .map_err(|_| api_error("physics3d.add_cells needs a numeric z"))?;
                let axis = |name: &str| -> mlua::Result<usize> {
                    let value: i64 = opts.get(name).map_err(|_| {
                        api_error(format!("physics3d.add_cells needs an integer {name}"))
                    })?;
                    if value <= 0 || value > crate::voxel::MAX_AXIS as i64 {
                        return Err(api_error(format!(
                            "physics3d.add_cells {name} must be 1..=64, got {value}"
                        )));
                    }
                    Ok(value as usize)
                };
                let dims = [axis("w")?, axis("h")?, axis("d")?];
                let total = dims[0] * dims[1] * dims[2];
                let raw: Vec<i64> = opts
                    .get::<Table>("solids")
                    .map_err(|_| api_error("physics3d.add_cells needs a solids array"))?
                    .sequence_values::<i64>()
                    .collect::<mlua::Result<Vec<i64>>>()?;
                let mut solids: Vec<usize> = Vec::with_capacity(raw.len());
                for value in raw {
                    if value < 0 || value as usize >= total {
                        return Err(api_error(format!(
                            "physics3d.add_cells cell index {value} is outside 0..{total}"
                        )));
                    }
                    solids.push(value as usize);
                }
                {
                    let world = wc.borrow()?;
                    require_alive(&world, id)?;
                }
                let count = p
                    .borrow_mut()?
                    .world3d_mut()
                    .add_cells(id, Vec3::new(x, y, z), cell, dims, &solids)
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(count as i64)
            })?,
        )?;
    }
    {
        let p = physics.clone();
        table.set(
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
                    Option<Table>,
                )| {
                    // docs/60 §5.2 复核：2D 的 `physics.raycast` 有两道有限性校验，3D 此前
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
                    // P3-③（与 2D 同一处、同一理由）：查询可能要兑现延迟的 BVH 重建。
                    let mut physics = p.borrow_mut()?;
                    let hit = physics
                        .world3d_mut()
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
    }
    {
        let p = physics.clone();
        table.set(
            "overlap",
            lua.create_function(
                move |lua, (cx, cy, cz, radius, opts): (f32, f32, f32, f32, Option<Table>)| {
                    // docs/60 §5.2：同名 API 不该一个守一个放（3D 此前不校验 radius）。
                    if !radius.is_finite() || radius < 0.0 {
                        return Err(api_error("overlap radius must be finite and non-negative"));
                    }
                    let groups = query_groups(opts)?;
                    let mut physics = p.borrow_mut()?;
                    let hits = physics.world3d_mut().overlap_filtered(
                        Vec3::new(cx, cy, cz),
                        radius,
                        groups,
                    );
                    entities_table(lua, hits)
                },
            )?,
        )?;
    }
    lua.globals().set("physics3d", table)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_math::math2d::Vec2;

    /// 装好两张表的夹具：Lua、物理状态、世界。
    type Harness = (Lua, Handle<PhysicsState>, Handle<World>);

    fn harness() -> Harness {
        let lua = Lua::new();
        let physics = PhysicsState::handle();
        let world = Handle::new(World::new(), "world");
        install(&lua, &physics, &world).unwrap();
        // 领域 crate 无 feature：测试永远装全量面（门控只发生在组合根调用点）。
        install_3d(&lua, &physics, &world).unwrap();
        (lua, physics, world)
    }

    /// 21 个函数一个不少（两张表的**清单**，少一个脚本就会静默拿到 nil）。
    #[test]
    fn every_physics_function_is_installed() {
        let (lua, _physics, _world) = harness();
        let mut total = 0;
        for table in ["physics", "physics3d"] {
            let table: Table = lua.globals().get(table).unwrap();
            total += table.pairs::<mlua::Value, mlua::Value>().count();
        }
        assert_eq!(total, 21, "physics + physics3d 合计 21 个函数");
    }

    /// `debug_draw` 只登记请求、按维数分开，且取走即清空。
    #[test]
    fn debug_draw_is_recorded_per_dimension() {
        let (lua, physics, _world) = harness();
        lua.load("assert(physics.debug_draw(true) == true)")
            .exec()
            .unwrap();
        assert_eq!(
            physics.borrow_mut().unwrap().take_debug_request(),
            Some(true)
        );
        assert_eq!(
            physics.borrow_mut().unwrap().take_debug_request_3d(),
            None,
            "2D 的请求不该点亮 3D"
        );
        lua.load("physics3d.debug_draw(false)").exec().unwrap();
        assert_eq!(
            physics.borrow_mut().unwrap().take_debug_request_3d(),
            Some(false)
        );
    }

    /// 死句柄在**每个**入口都以同一句话报错（不是 Rapier 的内部错）。
    #[test]
    fn dead_entity_is_rejected_before_touching_rapier() {
        let (lua, _physics, _world) = harness();
        let dead = EntityId::from_raw(999, 1).to_lua_i64();
        for call in [
            format!("return physics.get_ccd({dead})"),
            format!("return physics.set_ccd({dead}, true)"),
            format!("return physics.set_velocity({dead}, 1, 2)"),
            format!("return physics.get_velocity({dead})"),
            format!("return physics.set_position({dead}, 1, 2)"),
            format!("return physics3d.set_position({dead}, 1, 2, 3)"),
            format!(
                "return physics3d.add_heightfield({dead}, 0, 0, 0, 2, 2, 1, 1, 1, {{0,0,0,0}})"
            ),
            format!(
                "return physics3d.add_cells({dead}, {{cell=1, x=0, y=0, z=0, w=2, h=1, d=1, solids={{0,1}}}})"
            ),
        ] {
            let message = lua
                .load(call)
                .eval::<mlua::Value>()
                .expect_err("死句柄必须报错")
                .to_string();
            assert!(message.contains("invalid entity id"), "{message}");
        }
    }

    /// Lua 面的 `add_cells`：opts 就是渲染侧 `cells3d.build` 的那张表，返回值是并块后的
    /// 碰撞体个数，第二次调用是**替换**（同实体不叠加），挖掉的那一格真的不再挡射线。
    #[test]
    fn add_cells_binding_merges_and_replaces_through_lua() {
        let (lua, _physics, world) = harness();
        let id = world.with_mut(|world| world.spawn()).to_lua_i64();
        let region = "cell = 1, x = 0, y = 0, z = 0, w = 4, h = 1, d = 1";
        let script = format!(
            r#"
            local n = physics3d.add_cells({id}, {{ {region}, solids = {{ 0, 1, 2, 3 }} }})
            assert(n == 1, "4 cells in a row must merge into 1 collider, got " .. tostring(n))
            assert(physics3d.raycast(2.0, 3.0, 0.5, 0.0, -1.0, 0.0, 5.0) == {id}, "wall must block")
            n = physics3d.add_cells({id}, {{ {region}, solids = {{ 0, 2, 3 }} }})
            assert(n == 2, "replace must not stack colliders, got " .. tostring(n))
            assert(physics3d.raycast(1.5, 3.0, 0.5, 0.0, -1.0, 0.0, 5.0) == nil, "dug cell must open")
            assert(physics3d.raycast(2.5, 3.0, 0.5, 0.0, -1.0, 0.0, 5.0) == {id}, "邻居还得挡")
            "#,
            region = region
        );
        lua.load(script).exec().unwrap();
        // 四类非法 opts 都要在碰到 Rapier 之前报错，并且文案指名坏掉的是哪一个。
        // 第一条排在最前是有意的：去掉单轴上限后它会**算术溢出 panic**（绑定里 `total = w*h*d`
        // 先于 validate 算），红因就是溢出本身，而不是后面那句文案对不上。
        for (call, needle) in [
            (
                // 三条轴各 4e9 时 `w*h*d` 在 debug 会溢出 panic（release 会绕回一个小数，
                // 于是越界下标反而合法）——单轴上限必须在算 total 之前就拦住。
                "physics3d.add_cells(1, { cell = 1, x = 0, y = 0, z = 0, w = 4000000000, \
                 h = 4000000000, d = 4000000000, solids = { 0 } })"
                    .to_owned(),
                "w must be 1..=64",
            ),
            (
                format!("physics3d.add_cells({id}, {{ {region}, solids = {{ 0, 4 }} }})"),
                "outside 0..4",
            ),
            (
                format!("physics3d.add_cells({id}, {{ {region}, w = 65, solids = {{ 0 }} }})"),
                "w must be 1..=64",
            ),
            (
                "physics3d.add_cells(1, { cell = 0, x = 0, y = 0, z = 0, w = 2, h = 1, d = 1, \
                 solids = { 0 } })"
                    .to_owned(),
                "0.05..=64",
            ),
            (
                format!("physics3d.add_cells({id}, {{ {region}, solids = {{ 0, 0 }} }})"),
                "listed twice",
            ),
        ] {
            let message = lua
                .load(call)
                .exec()
                .expect_err("非法 opts 必须报错")
                .to_string();
            assert!(message.contains(needle), "{needle} 没出现在 {message}");
        }
    }

    /// `positions_in_radius` 走的是 ECS `Transform` 宽相，不是碰撞体查询。
    #[test]
    fn positions_in_radius_scans_ecs_transforms() {
        let (lua, _physics, world) = harness();
        let near = world.with_mut(|world| {
            let id = world.spawn();
            world
                .insert(
                    id,
                    Transform {
                        position: Vec2::new(10.0, 0.0),
                        ..Transform::default()
                    },
                )
                .unwrap();
            id
        });
        let far = world.with_mut(|world| {
            let id = world.spawn();
            world
                .insert(
                    id,
                    Transform {
                        position: Vec2::new(100.0, 0.0),
                        ..Transform::default()
                    },
                )
                .unwrap();
            id
        });
        let tight: Table = lua
            .load("return physics.positions_in_radius(0, 0, 20)")
            .eval()
            .unwrap();
        assert_eq!(tight.raw_len(), 1);
        assert_eq!(
            tight.get::<i64>(1).unwrap(),
            near.to_lua_i64(),
            "只有近的那个在半径内"
        );
        let wide: Table = lua
            .load("return physics.positions_in_radius(0, 0, 200)")
            .eval()
            .unwrap();
        let ids: Vec<i64> = wide.sequence_values().collect::<LuaResult<_>>().unwrap();
        assert!(ids.contains(&far.to_lua_i64()), "{ids:?}");
    }

    /// `events` 取走即清空：第二次数到 0 条（同一批接触不能被消费两次）。
    #[test]
    fn events_are_drained_once() {
        let (lua, _physics, _world) = harness();
        lua.load("assert(#physics.events() == 0 and #physics3d.events() == 0)")
            .exec()
            .unwrap();
    }

    /// 重入：宿主还攥着物理句柄时调 Lua，脚本收到点名 `physics` 的错（而不是 panic）。
    #[test]
    fn reentrant_borrow_names_the_physics_state() {
        let (lua, physics, _world) = harness();
        let _guard = physics.borrow_mut().unwrap();
        let message = lua
            .load("return physics.debug_draw(true)")
            .exec()
            .unwrap_err()
            .to_string();
        assert!(message.contains("physics"), "{message}");
        assert!(message.contains("already borrowed"), "{message}");
    }
}
