//! `animation` / `animation3d` 两张表的 Lua 绑定（P1 ⑤-10 从脚本层搬来）。
//!
//! 动画的组件（`AnimationStateMachine` / `Animation3D`）与"本帧刚触发的帧事件"都在
//! `rustlua-animation`，所以绑定也跟着住这里。宿主每帧把轨道推进产出的帧事件塞进
//! `AnimationState::push_events`，脚本用 `animation.events()` **取走即清**——这条"一次性队列"
//! 的语义本来就是动画域的东西，不该留在 VM 宿主里。
//!
//! 场景切换会把队列一起清掉：换图后旧实体 id 已失效，把上一张图的帧事件交给新脚本只会
//! 让调用方拿到指向死句柄的事件。

use crate::animation::{AnimationFrameEvent, AnimationStateMachine};
use crate::animation_player::{AnimationPlayer, AnimationPlayerEvent};
use crate::animation3d::Animation3D;
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::{World, WorldError};
use rustlua_lua_api::{Handle, api_error};
use std::cell::{Ref, RefMut};

/// 动画域里"跨帧的脚本可见状态"：本帧触发、尚未被脚本取走的帧事件。
///
/// 两套事件队列彼此独立：`events` 属于旧 `Animation` 状态机（帧号），`player_events` 属于
/// `AnimationPlayer`（`.anim` 时间线上的命名事件，带秒数）。合成一条队列会让脚本拿错
/// `frame`/`time` 语义，因此分开存放、分开取走。
#[derive(Debug, Default)]
pub struct AnimationState {
    events: Vec<(EntityId, AnimationFrameEvent)>,
    player_events: Vec<(EntityId, AnimationPlayerEvent)>,
}

impl AnimationState {
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "animation")
    }

    /// 宿主推进动画后把帧事件塞进队列（顺序即触发顺序）。
    pub fn push_events(&mut self, events: Vec<(EntityId, AnimationFrameEvent)>) {
        self.events.extend(events);
    }

    /// 取走队列（`animation.events()` 一次取空）。
    pub fn take_events(&mut self) -> Vec<(EntityId, AnimationFrameEvent)> {
        std::mem::take(&mut self.events)
    }

    /// `AnimationPlayer` 的命名事件，同样的"取走即清"语义。
    pub fn push_player_events(&mut self, events: Vec<(EntityId, AnimationPlayerEvent)>) {
        self.player_events.extend(events);
    }

    pub fn take_player_events(&mut self) -> Vec<(EntityId, AnimationPlayerEvent)> {
        std::mem::take(&mut self.player_events)
    }

    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.events.len()
    }

    pub fn clear_for_scene_transition(&mut self) {
        self.events.clear();
        self.player_events.clear();
    }
}

/// 绑定侧的借用面：`World` + `AnimationState`，加与脚本层同名的短写。
#[derive(Clone)]
struct Bindings {
    world: Handle<World>,
    state: Handle<AnimationState>,
}

impl Bindings {
    fn new(world: &Handle<World>, state: &Handle<AnimationState>) -> Self {
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

    fn take_events(&self) -> LuaResult<Vec<(EntityId, AnimationFrameEvent)>> {
        Ok(self.state.borrow_mut()?.take_events())
    }

    fn take_player_events(&self) -> LuaResult<Vec<(EntityId, AnimationPlayerEvent)>> {
        Ok(self.state.borrow_mut()?.take_player_events())
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
pub fn install(lua: &Lua, state: &Handle<AnimationState>, world: &Handle<World>) -> LuaResult<()> {
    let animation = lua.create_table()?;
    let s = Bindings::new(world, state);
    animation.set(
        "get",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let Some(machine) = world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            Ok(machine.current_state().to_owned())
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation.set(
        "set",
        lua.create_function(move |_, (id, name): (i64, String)| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let mut world = state.world_mut()?;
            let machine = world
                .get_mut::<AnimationStateMachine>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation"))?;
            machine
                .set_state(&name)
                .map_err(|error| api_error(error.to_string()))
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation.set(
        "frame",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let Some(machine) = world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            Ok(machine.current_frame())
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation.set(
        "ended",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let mut world = state.world_mut()?;
            let machine = world
                .get_mut::<AnimationStateMachine>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation"))?;
            Ok(machine.take_ended())
        })?,
    )?;
    // 运行期挂动画：`entity.spawn` 只插入 Transform，Lua 侧此前无法给实体加
    // Animation 组件（NPC/敌人/特效因此只能显示单帧）。attach 解析同一套组件串
    // （`default=..;clip:<name>:<sheet>:<cols>:<fps>:<looped>`）并插入/覆盖组件。
    let s = Bindings::new(world, state);
    animation.set(
        "attach",
        lua.create_function(move |_, (id, component): (i64, String)| {
            let machine = AnimationStateMachine::from_component_str(&component)
                .map_err(|error| api_error(error.to_string()))?;
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            state
                .world_mut()?
                .insert(id, machine)
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation.set(
        "clips",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let Some(machine) = world
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
    let s = Bindings::new(world, state);
    animation.set(
        "events",
        lua.create_function(move |lua, ()| {
            let state = &s;
            let events = state.take_events()?;
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
    let s = Bindings::new(world, state);
    animation.set(
        "time",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let Some(machine) = world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            Ok(machine.time())
        })?,
    )?;
    // P3-E01-2：`AnimationPlayer`（`.anim` 时间线）的播放控制。与旧状态机的
    // `animation.set` 不同，这里操作的是**时刻**而不是命名状态；实体没有 player 时报
    // "no animation player"，与 `animation.get` 的 "no animation" 区分开。
    let s = Bindings::new(world, state);
    animation.set(
        "play",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let mut world = state.world_mut()?;
            let player = world
                .get_mut::<AnimationPlayer>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation player"))?;
            player.set_playing(true);
            Ok(true)
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation.set(
        "pause",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let mut world = state.world_mut()?;
            let player = world
                .get_mut::<AnimationPlayer>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation player"))?;
            player.set_playing(false);
            Ok(true)
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation.set(
        "stop",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let mut world = state.world_mut()?;
            let player = world
                .get_mut::<AnimationPlayer>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation player"))?;
            player.stop();
            Ok(true)
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation.set(
        "seek",
        lua.create_function(move |_, (id, seconds): (i64, f64)| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let mut world = state.world_mut()?;
            let player = world
                .get_mut::<AnimationPlayer>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation player"))?;
            player
                .seek(seconds as f32)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(true)
        })?,
    )?;
    // 与 `animation.events()` 相同的"取走即清"；条目多一个 `time`（秒），因为 `.anim`
    // 的命名事件落在时间轴上，不折算成帧号。
    let s = Bindings::new(world, state);
    animation.set(
        "player_events",
        lua.create_function(move |lua, ()| {
            let state = &s;
            let events = state.take_player_events()?;
            let result = lua.create_table()?;
            for (index, (id, event)) in events.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("entity", entity_to_lua(id))?;
                entry.set("name", event.name)?;
                entry.set("time", event.time)?;
                result.set(index + 1, entry)?;
            }
            Ok(result)
        })?,
    )?;
    lua.globals().set("animation", animation)?;
    // WP4：世界空间文字运行期挂载。与 `animation.attach` 同理——`entity.spawn` 只插
    // Transform，运行期创建的实体（伤害数字、名牌）否则无法带世界文字。

    let animation3d = lua.create_table()?;
    let s = Bindings::new(world, state);
    animation3d.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let Some(animation) = world.get::<Animation3D>(id).map_err(world_error)? else {
                return Ok(mlua::Value::Nil);
            };
            Ok(mlua::Value::String(
                lua.create_string(animation.current_name())?,
            ))
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation3d.set(
        "set",
        lua.create_function(move |_, (id, name): (i64, String)| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let mut world = state.world_mut()?;
            let animation = world
                .get_mut::<Animation3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation3d"))?;
            animation
                .set_clip(&name)
                .map_err(|error| api_error(error.to_string()))
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation3d.set(
        "list",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let Some(animation) = world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            let result = lua.create_table()?;
            for (index, name) in animation.clip_names().into_iter().enumerate() {
                result.set(index + 1, name)?;
            }
            Ok(result)
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation3d.set(
        "paused",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let Some(animation) = world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            Ok(animation.is_paused())
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation3d.set(
        "set_paused",
        lua.create_function(move |_, (id, paused): (i64, bool)| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let mut world = state.world_mut()?;
            let animation = world
                .get_mut::<Animation3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation3d"))?;
            animation.set_paused(paused);
            Ok(())
        })?,
    )?;
    let s = Bindings::new(world, state);
    animation3d.set(
        "time",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = &s;
            require_alive(state, id)?;
            let world = state.world()?;
            let Some(animation) = world.get::<Animation3D>(id).map_err(world_error)? else {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 两张表合计 19 个函数，一个不少（14 个旧绑定 + 5 个 `AnimationPlayer`）。
    #[test]
    fn every_animation_function_is_installed() {
        let lua = Lua::new();
        let state = AnimationState::handle();
        let world = Handle::new(World::new(), "world");
        install(&lua, &state, &world).unwrap();
        let mut total = 0;
        for table in ["animation", "animation3d"] {
            let table: mlua::Table = lua.globals().get(table).unwrap();
            total += table.pairs::<mlua::Value, mlua::Value>().count();
        }
        assert_eq!(total, 19, "animation + animation3d 合计 19 个函数");
    }

    /// 宿主塞进来的帧事件，脚本取一次就空（一次性队列的语义）。
    #[test]
    fn events_are_drained_once() {
        let lua = Lua::new();
        let state = AnimationState::handle();
        let world = Handle::new(World::new(), "world");
        install(&lua, &state, &world).unwrap();
        let id = world.with_mut(|world| world.spawn());
        state.with_mut(|state| {
            state.push_events(vec![(
                id,
                AnimationFrameEvent {
                    frame: 3,
                    name: "hit".into(),
                },
            )]);
        });
        let first: mlua::Table = lua.load("return animation.events()").eval().unwrap();
        assert_eq!(first.raw_len(), 1);
        let second: mlua::Table = lua.load("return animation.events()").eval().unwrap();
        assert_eq!(second.raw_len(), 0, "取走即清，不能再读一次");
    }

    /// 玩家事件与旧帧事件是两条独立队列：各自的取走不会清掉对方。
    #[test]
    fn player_events_have_their_own_queue() {
        let lua = Lua::new();
        let state = AnimationState::handle();
        let world = Handle::new(World::new(), "world");
        install(&lua, &state, &world).unwrap();
        let id = world.with_mut(|world| world.spawn());
        state.with_mut(|state| {
            state.push_events(vec![(
                id,
                AnimationFrameEvent {
                    frame: 3,
                    name: "hit".into(),
                },
            )]);
            state.push_player_events(vec![(
                id,
                AnimationPlayerEvent {
                    time: 0.5,
                    name: "footstep".into(),
                },
            )]);
        });
        let players: mlua::Table = lua.load("return animation.player_events()").eval().unwrap();
        assert_eq!(players.raw_len(), 1);
        assert_eq!(
            players
                .get::<mlua::Table>(1)
                .unwrap()
                .get::<f32>("time")
                .unwrap(),
            0.5
        );
        let frames: mlua::Table = lua.load("return animation.events()").eval().unwrap();
        assert_eq!(frames.raw_len(), 1, "取走 player 事件不影响帧事件");
        let empty: mlua::Table = lua.load("return animation.player_events()").eval().unwrap();
        assert_eq!(empty.raw_len(), 0);
    }

    /// 死实体在碰到组件之前就被拒（错误串点名实体 id）。
    #[test]
    fn dead_entity_is_rejected_before_touching_the_world() {
        let lua = Lua::new();
        let state = AnimationState::handle();
        let world = Handle::new(World::new(), "world");
        install(&lua, &state, &world).unwrap();
        let dead = EntityId::from_raw(999, 1).to_lua_i64();
        for call in [
            format!("return animation.get({dead})"),
            format!("return animation.set({dead}, 'idle', 'loop')"),
            format!("return animation3d.list({dead})"),
            format!("return animation.play({dead})"),
            format!("return animation.seek({dead}, 0.5)"),
        ] {
            let message = lua
                .load(call)
                .eval::<mlua::Value>()
                .expect_err("死句柄必须报错")
                .to_string();
            assert!(message.contains("invalid entity id"), "{message}");
        }
    }

    /// 重入：宿主攥着动画状态可写借用时调 Lua，脚本收到点名 `animation` 的错。
    #[test]
    fn reentrant_borrow_names_the_animation_state() {
        let lua = Lua::new();
        let state = AnimationState::handle();
        let world = Handle::new(World::new(), "world");
        install(&lua, &state, &world).unwrap();
        let _guard = state.borrow_mut().unwrap();
        let message = lua
            .load("return animation.events()")
            .exec()
            .unwrap_err()
            .to_string();
        assert!(message.contains("already borrowed"), "{message}");
        assert!(message.contains("animation"), "{message}");
    }
}
