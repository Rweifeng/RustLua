//! 实体身份与其上的轻量组件：`entity` / `health` / `tag` / `interact.query`（P1 ⑤-13 从脚本层搬来）。
//!
//! 实体账本（名字/持久 id/血量/标签）与 `EntityId` 一起住在 ECS，所以这些表的 Lua 面也在这里。
//! Lua 侧句柄是 `index << 32 | generation` 编码的整数，解码集中在 [`EntityId::from_lua_i64`]。
//!
//! 两张表**不在这里**，它们跟着各自的类型走：
//!
//! - `interact.prompt` / `clear_prompt` / `get_prompt`：要写 UI 的覆盖层文本 → 脚本层
//!   （`api/interact_prompt.rs`），本文件只往同一张 `interact` 表上补 `query`。
//! - `visibility`：`Visibility` 是渲染域组件 → `rustlua_render::visibility_lua`。
//!
//! `entity.destroy` 不再直接摘物理刚体，而是往 [`EntityState::pending_physics_removals`] 记一笔，
//! 由宿主在推进物理前统一摘。

use crate::entity::EntityId;
use crate::entity_state::EntityState;
use crate::transform::Transform;
use crate::world::{World, WorldError};
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_lua_api::{Handle, api_error};
use std::cell::{Ref, RefMut};

/// 绑定侧的借用面：世界 + 实体账本两个能力单元。
#[derive(Clone)]
struct Bindings {
    world: Handle<World>,
    state: Handle<EntityState>,
}

impl Bindings {
    fn new(world: &Handle<World>, state: &Handle<EntityState>) -> Self {
        Self {
            world: world.clone(),
            state: state.clone(),
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

/// Lua 侧实体 id → 实体句柄（错误串与脚本层逐字相同）。
fn entity_from_lua(value: i64) -> LuaResult<EntityId> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    EntityId::from_lua_i64(value).ok_or_else(|| api_error(format!("invalid entity id: {value}")))
}

/// 宽容解码：旧运行时存档里的 id（1、2、3…）不是合法句柄，谓词按"不在世"处理。
fn entity_from_lua_lenient(value: i64) -> Option<EntityId> {
    EntityId::from_lua_i64(value)
}

/// 实体句柄 → Lua 侧 id。
fn entity_to_lua(entity: EntityId) -> i64 {
    entity.to_lua_i64()
}

/// 写操作的前置校验：实体必须还活着。
fn require_alive(state: &Bindings, entity: EntityId) -> LuaResult<()> {
    if state.world()?.is_alive(entity) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {entity}")))
    }
}

pub fn install(lua: &Lua, world: &Handle<World>, state: &Handle<EntityState>) -> LuaResult<()> {
    let entity = lua.create_table()?;
    let s = Bindings::new(world, state);
    entity.set(
        "spawn",
        lua.create_function(
            move |_, (name, persist_id): (Option<String>, Option<String>)| {
                let mut entities = s.state.borrow_mut()?;
                // 一次借用覆盖"造实体 + 插组件 + 问持久 id 是否还活着"：世界句柄是独立的
                // `RefCell`，这里再借一次会撞重入错（P1 ⑤-7 换借用面时踩到的第一个坑）。
                let world_handle = s.world.clone();
                let mut world = world_handle.borrow_mut()?;
                let id = world.spawn();
                world
                    .insert(id, Transform::default())
                    .map_err(world_error)?;
                if let Some(name) = name {
                    entities.names.insert(name, id);
                }
                if let Some(persist) = persist_id.filter(|value| !value.is_empty()) {
                    let already_alive = entities
                        .persist
                        .get(&persist)
                        .is_some_and(|existing| world.is_alive(*existing));
                    if already_alive {
                        return Err(api_error(format!("persist_id already in use: {persist}")));
                    }
                    entities.persist.insert(persist, id);
                }
                Ok(entity_to_lua(id))
            },
        )?,
    )?;
    let s = Bindings::new(world, state);
    entity.set(
        "find",
        lua.create_function(move |_, name: String| {
            let entities = s.state.borrow()?;
            let Some(id) = entities.names.get(&name) else {
                return Ok(None);
            };
            // **死活校验**：名字表里留着一个已经死掉的句柄是可能的（第 58 轮 webgl 产物上实测到
            // `invalid entity id: EntityId(0:g13)`——`ScaleBar` 在场景里确实存在、脚本每次调用都
            // 重新 `entity.find`，但拿到的句柄代际已经过期，直到真正使用时才被引擎拒掉）。
            // 语义与同文件的 `is_alive` 一致："不合法/不在了"在这里就是 `nil`；
            // 也与 `persist_id` 的注册路径（那里已经先问 `world.is_alive`）一致。
            // 返回死句柄没有任何好处：调用方拿它做任何事都只会得到一句 `invalid entity id`。
            Ok(s.world()?.is_alive(id).then_some(entity_to_lua(id)))
        })?,
    )?;
    let s = Bindings::new(world, state);
    entity.set(
        "find_id",
        lua.create_function(move |_, persist_id: String| {
            Ok(s.state
                .borrow()?
                .persist
                .get(&persist_id)
                .copied()
                .map(entity_to_lua))
        })?,
    )?;
    let s = Bindings::new(world, state);
    entity.set(
        "persist_id",
        lua.create_function(move |_, id: i64| {
            // 查询用宽容解码：旧运行时存档里的 id（1、2、3…）不该让脚本报错，返回 nil 即可。
            let Some(id) = entity_from_lua_lenient(id) else {
                return Ok(None);
            };
            Ok(s.state
                .borrow()?
                .persist
                .iter()
                .find(|(_, entity)| **entity == id)
                .map(|(key, _)| key.clone()))
        })?,
    )?;
    let s = Bindings::new(world, state);
    entity.set(
        "is_alive",
        lua.create_function(move |_, id: i64| {
            // 谓词**绝不能抛错**：`generation == 0` 的整数（旧运行时的实体 id、以及存档里
            // 遗留的值）在引擎里不是合法句柄，但"不合法"在这里的语义就是"不在了"。
            // 旧运行时同名字段对未知 id 也返回 false；两边一致，读旧档才不会炸。
            let Some(id) = entity_from_lua_lenient(id) else {
                return Ok(false);
            };
            Ok(s.world()?.is_alive(id))
        })?,
    )?;
    let s = Bindings::new(world, state);
    entity.set(
        "destroy",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut entities = s.state.borrow_mut()?;
            require_alive(&s, id)?;
            s.world_mut()?.despawn(id).map_err(world_error)?;
            entities.names.remove_entity(id);
            entities.persist.retain(|_, value| *value != id);
            entities.health.remove(&id);
            // P1 ⑤-13：物理刚体摘除改成"记账"，由宿主在推进物理前统一做——
            // ECS 绑定不该反向依赖物理域（此前这里直接 `physics.world_mut().remove_body`）。
            entities.queue_physics_removal(id);
            Ok(true)
        })?,
    )?;
    lua.globals().set("entity", entity)?;

    let health = lua.create_table()?;
    let s = Bindings::new(world, state);
    health.set(
        "get",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let entities = s.state.borrow()?;
            require_alive(&s, id)?;
            Ok(entities.health.get(&id).copied().unwrap_or(0.0))
        })?,
    )?;
    let s = Bindings::new(world, state);
    health.set(
        "set",
        lua.create_function(move |_, (id, value): (i64, f32)| {
            let id = entity_from_lua(id)?;
            let mut entities = s.state.borrow_mut()?;
            require_alive(&s, id)?;
            if !value.is_finite() || value < 0.0 {
                return Err(api_error("health must be finite and non-negative"));
            }
            entities.health.insert(id, value);
            Ok(())
        })?,
    )?;
    let s = Bindings::new(world, state);
    health.set(
        "damage",
        lua.create_function(move |_, (id, amount): (i64, f32)| {
            let id = entity_from_lua(id)?;
            let mut entities = s.state.borrow_mut()?;
            require_alive(&s, id)?;
            if !amount.is_finite() || amount < 0.0 {
                return Err(api_error("damage must be finite and non-negative"));
            }
            let value = entities.health.entry(id).or_insert(0.0);
            *value = (*value - amount).max(0.0);
            Ok(*value)
        })?,
    )?;
    lua.globals().set("health", health)?;

    let tag = lua.create_table()?;
    let s = Bindings::new(world, state);
    tag.set(
        "get",
        lua.create_function(move |_, (id, key): (i64, String)| {
            let id = entity_from_lua(id)?;
            let entities = s.state.borrow()?;
            require_alive(&s, id)?;
            Ok(entities
                .tags
                .get(&id)
                .and_then(|bag| bag.get(&key))
                .cloned())
        })?,
    )?;
    let s = Bindings::new(world, state);
    tag.set(
        "set",
        lua.create_function(move |_, (id, key, value): (i64, String, String)| {
            let id = entity_from_lua(id)?;
            if key.is_empty() {
                return Err(api_error("tag key must not be empty"));
            }
            let mut entities = s.state.borrow_mut()?;
            require_alive(&s, id)?;
            if value.is_empty() {
                if let Some(bag) = entities.tags.get_mut(&id) {
                    bag.remove(&key);
                    if bag.is_empty() {
                        entities.tags.remove(&id);
                    }
                }
            } else {
                entities.tags.entry(id).or_default().insert(key, value);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("tag", tag)?;

    let interact = lua.create_table()?;
    let s = Bindings::new(world, state);
    interact.set(
        "query",
        lua.create_function(move |lua, (x, y, radius): (f32, f32, f32)| {
            if !radius.is_finite() || radius < 0.0 {
                return Err(api_error("interact radius must be finite and non-negative"));
            }
            let entities = s.state.borrow()?;
            let mut hits: Vec<(f32, i32, i64, String, i32)> = Vec::new();
            for id in s.world()?.entities_with::<Transform>() {
                let Some(tags) = entities.tags.get(&id) else {
                    continue;
                };
                if !(tags.contains_key("interact") || tags.contains_key("kind")) {
                    continue;
                }
                let world = s.world()?;
                let Some(t) = world.get::<Transform>(id).map_err(world_error)? else {
                    continue;
                };
                let dx = t.position.x - x;
                let dy = t.position.y - y;
                let dist_sq = dx * dx + dy * dy;
                if dist_sq > radius * radius {
                    continue;
                }
                let kind = tags
                    .get("kind")
                    .cloned()
                    .or_else(|| tags.get("interact").cloned())
                    .unwrap_or_default();
                let priority = tags
                    .get("priority")
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(0);
                hits.push((dist_sq, -priority, entity_to_lua(id), kind, priority));
            }
            hits.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.cmp(&b.1))
            });
            let result = lua.create_table()?;
            for (index, (_, _, id, kind, priority)) in hits.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("id", id)?;
                entry.set("kind", kind)?;
                entry.set("priority", priority)?;
                result.set(index + 1, entry)?;
            }
            Ok(result)
        })?,
    )?;
    lua.globals().set("interact", interact)?;
    Ok(())
}
