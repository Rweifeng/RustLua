//! `spine` Lua 表（M84 P1）：动画切换、皮肤、速度、开关、事件与包围盒。
//!
//! 事实源是 ECS 里的 `SpinePlayer` 组件；事件来自 `SpineState`（取走即清）。

use mlua::{Lua, Result as LuaResult};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::{World, WorldError};
use rustlua_lua_api::{Handle, api_error};
use std::cell::{Ref, RefMut};

use crate::model::SpineEvent;
use crate::player::SpinePlayer;
use crate::spine_state::SpineState;

#[derive(Clone)]
struct Bindings {
    world: Handle<World>,
    state: Handle<SpineState>,
}

impl Bindings {
    fn new(world: &Handle<World>, state: &Handle<SpineState>) -> Self {
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

    fn take_events(&self) -> LuaResult<Vec<(EntityId, SpineEvent)>> {
        Ok(self.state.borrow_mut()?.take_events())
    }
}

fn world_error(error: WorldError) -> mlua::Error {
    api_error(error.to_string())
}

fn entity_from_lua(value: i64) -> LuaResult<EntityId> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    EntityId::from_lua_i64(value).ok_or_else(|| api_error(format!("invalid entity id: {value}")))
}

fn entity_to_lua(entity: EntityId) -> i64 {
    entity.to_lua_i64()
}

fn require_alive(state: &Bindings, entity: EntityId) -> LuaResult<()> {
    if state.world()?.is_alive(entity) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {entity}")))
    }
}

fn require_player(world: &World, entity: EntityId) -> LuaResult<&SpinePlayer> {
    world
        .get::<SpinePlayer>(entity)
        .map_err(world_error)?
        .ok_or_else(|| api_error("entity has no spine skeleton"))
}

/// 安装全局 `spine` 表。由组合根 `engine::host::script_packs` 延迟调用。
pub fn install(lua: &Lua, state: &Handle<SpineState>, world: &Handle<World>) -> LuaResult<()> {
    let spine = lua.create_table()?;

    let s = Bindings::new(world, state);
    spine.set(
        "get_animation",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            require_alive(&s, id)?;
            let world = s.world()?;
            Ok(require_player(&world, id)?.animation().to_owned())
        })?,
    )?;

    let s = Bindings::new(world, state);
    spine.set(
        "list",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            require_alive(&s, id)?;
            let world = s.world()?;
            let names = require_player(&world, id)?.animation_names();
            let out = lua.create_table()?;
            for (index, name) in names.iter().enumerate() {
                out.set(index + 1, name.as_str())?;
            }
            Ok(out)
        })?,
    )?;

    let s = Bindings::new(world, state);
    spine.set(
        "set_animation",
        lua.create_function(move |_, (id, name, looped): (i64, String, Option<bool>)| {
            let id = entity_from_lua(id)?;
            require_alive(&s, id)?;
            let mut world = s.world_mut()?;
            let player = world
                .get_mut::<SpinePlayer>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no spine skeleton"))?;
            player
                .set_animation(&name, looped.unwrap_or(true))
                .map_err(|error| api_error(error.to_string()))?;
            // 脚本切换动画产生的 `start` 事件**当场**进队列：宿主轮转要等到下一帧，
            // 而脚本常常在同一段逻辑里紧接着 `spine.events()` 检查它。丢掉这一批会让
            // "切了动画但收不到事件"变成时序巧合。
            let events = player.drain_events();
            // 先放开 world 的借用，再写事件队列：两个 `Handle` 可能指向同一把锁，
            // 同时持有可能当场死锁。
            drop(world);
            if !events.is_empty() {
                s.state
                    .borrow_mut()?
                    .push_events(events.into_iter().map(|event| (id, event)).collect());
            }
            Ok(())
        })?,
    )?;

    let s = Bindings::new(world, state);
    spine.set(
        "set_skin",
        lua.create_function(move |_, (id, name): (i64, String)| {
            let id = entity_from_lua(id)?;
            require_alive(&s, id)?;
            let mut world = s.world_mut()?;
            let player = world
                .get_mut::<SpinePlayer>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no spine skeleton"))?;
            player
                .set_skin(&name)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;

    let s = Bindings::new(world, state);
    spine.set(
        "set_speed",
        lua.create_function(move |_, (id, speed): (i64, f32)| {
            let id = entity_from_lua(id)?;
            require_alive(&s, id)?;
            let mut world = s.world_mut()?;
            let player = world
                .get_mut::<SpinePlayer>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no spine skeleton"))?;
            player.set_speed(speed);
            Ok(())
        })?,
    )?;

    let s = Bindings::new(world, state);
    spine.set(
        "set_enabled",
        lua.create_function(move |_, (id, enabled): (i64, bool)| {
            let id = entity_from_lua(id)?;
            require_alive(&s, id)?;
            let mut world = s.world_mut()?;
            let player = world
                .get_mut::<SpinePlayer>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no spine skeleton"))?;
            player.set_enabled(enabled);
            Ok(())
        })?,
    )?;

    let s = Bindings::new(world, state);
    spine.set(
        "events",
        lua.create_function(move |lua, ()| {
            let events = s.take_events()?;
            let result = lua.create_table()?;
            for (index, (id, event)) in events.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("entity", entity_to_lua(id))?;
                entry.set("kind", event.kind.as_str())?;
                entry.set("name", event.name)?;
                entry.set("animation", event.animation)?;
                result.set(index + 1, entry)?;
            }
            Ok(result)
        })?,
    )?;

    let s = Bindings::new(world, state);
    spine.set(
        "bounds",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            require_alive(&s, id)?;
            let world = s.world()?;
            let output = require_player(&world, id)?.output();
            let mut min_x = f32::INFINITY;
            let mut min_y = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            let mut max_y = f32::NEG_INFINITY;
            for vertex in &output.vertices {
                min_x = min_x.min(vertex.position[0]);
                min_y = min_y.min(vertex.position[1]);
                max_x = max_x.max(vertex.position[0]);
                max_y = max_y.max(vertex.position[1]);
            }
            // 空骨骼（未播放或首帧未采样）不是错误：返回四点全零，脚本按"暂无包围盒"处理。
            if !min_x.is_finite() {
                min_x = 0.0;
                min_y = 0.0;
                max_x = 0.0;
                max_y = 0.0;
            }
            // 返回 `{ min_x, min_y, max_x, max_y }`：与 `bounds` 的单值返回名一致，
            // 脚本按 `b[1]..b[4]` 取值（多返回值口径会让 `local b = spine.bounds(id)`
            // 只拿到 min_x，是易错面）。
            let out = lua.create_table()?;
            out.set(1, min_x)?;
            out.set(2, min_y)?;
            out.set(3, max_x)?;
            out.set(4, max_y)?;
            Ok(out)
        })?,
    )?;

    lua.globals().set("spine", spine)?;
    Ok(())
}
