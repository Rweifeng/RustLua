//! 动画：`animation`（2D 帧序列）与 `animation3d`（蒙皮/骨骼描述）。

// WP11-USE:BEGIN
use crate::core::animation::AnimationStateMachine;
use crate::core::animation3d::Animation3D;
use crate::core::script::LuaStateBorrow;
use crate::core::script::ScriptState;
use crate::core::script::error::{api_error, world_error};
use crate::core::script::values::{entity_from_lua, entity_to_lua, require_alive};
use mlua::Lua;
use mlua::Result as LuaResult;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let animation = lua.create_table()?;
    let s = Rc::clone(&state);
    animation.set(
        "get",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(machine) = state
                .world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            Ok(machine.current_state().to_owned())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "set",
        lua.create_function(move |_, (id, name): (i64, String)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            let machine = state
                .world
                .get_mut::<AnimationStateMachine>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation"))?;
            machine
                .set_state(&name)
                .map_err(|error| api_error(error.to_string()))
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "frame",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(machine) = state
                .world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            Ok(machine.current_frame())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "ended",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            let machine = state
                .world
                .get_mut::<AnimationStateMachine>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation"))?;
            Ok(machine.take_ended())
        })?,
    )?;
    // 运行期挂动画：`entity.spawn` 只插入 Transform，Lua 侧此前无法给实体加
    // Animation 组件（NPC/敌人/特效因此只能显示单帧）。attach 解析同一套组件串
    // （`default=..;clip:<name>:<sheet>:<cols>:<fps>:<looped>`）并插入/覆盖组件。
    let s = Rc::clone(&state);
    animation.set(
        "attach",
        lua.create_function(move |_, (id, component): (i64, String)| {
            let machine = AnimationStateMachine::from_component_str(&component)
                .map_err(|error| api_error(error.to_string()))?;
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            state.world.insert(id, machine).map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "clips",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(machine) = state
                .world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            let names = lua.create_table()?;
            for (index, clip) in machine.clips.iter().enumerate() {
                names.set(index + 1, clip.name.clone())?;
            }
            Ok(names)
        })?,
    )?;
    // WP4 ⑦：轮询本帧触发的动画帧事件（与 `physics.events()` 同一套用法：取走即清空）。
    // 事件由宿主的 `animation_state_advance` 帧系统在推进动画时收集。
    let s = Rc::clone(&state);
    animation.set(
        "events",
        lua.create_function(move |lua, ()| {
            let mut state = s.lua_borrow_mut()?;
            let events = std::mem::take(&mut state.animation_events);
            let result = lua.create_table()?;
            for (index, (id, event)) in events.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("entity", entity_to_lua(id))?;
                entry.set("name", event.name)?;
                entry.set("frame", event.frame)?;
                result.set(index + 1, entry)?;
            }
            Ok(result)
        })?,
    )?;
    // docs/54 §4.4：与 `animation3d.time` 同名同义——当前片段的播放时刻（秒）。
    // 轨道采样是"时刻 → 值"的函数，脚本要自己按时间轴推理（或做动画工具）时需要一个可读的时钟；
    // 此前 2D 侧只能靠帧号间接猜。
    let s = Rc::clone(&state);
    animation.set(
        "time",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(machine) = state
                .world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            Ok(machine.time())
        })?,
    )?;
    lua.globals().set("animation", animation)?;
    // WP4：世界空间文字运行期挂载。与 `animation.attach` 同理——`entity.spawn` 只插
    // Transform，运行期创建的实体（伤害数字、名牌）否则无法带世界文字。

    let animation3d = lua.create_table()?;
    let s = Rc::clone(&state);
    animation3d.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Ok(mlua::Value::Nil);
            };
            Ok(mlua::Value::String(
                lua.create_string(animation.current_name())?,
            ))
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "set",
        lua.create_function(move |_, (id, name): (i64, String)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            let animation = state
                .world
                .get_mut::<Animation3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation3d"))?;
            animation
                .set_clip(&name)
                .map_err(|error| api_error(error.to_string()))
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "list",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            let result = lua.create_table()?;
            for (index, name) in animation.clip_names().into_iter().enumerate() {
                result.set(index + 1, name)?;
            }
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "paused",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            Ok(animation.is_paused())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "set_paused",
        lua.create_function(move |_, (id, paused): (i64, bool)| {
            let id = entity_from_lua(id)?;
            let mut state = s.lua_borrow_mut()?;
            require_alive(&state, id)?;
            let animation = state
                .world
                .get_mut::<Animation3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation3d"))?;
            animation.set_paused(paused);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "time",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.lua_borrow()?;
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            Ok(animation.time())
        })?,
    )?;
    lua.globals().set("animation3d", animation3d)?;

    // docs/54 §4.2：可见层遮罩。**视图侧**的遮罩由宿主声明（`RenderView::visibility`），
    // Lua 只读写实体侧，因此"脚本把东西藏起来"与"视图决定看哪些层"不会变成两件事。
    Ok(())
}
