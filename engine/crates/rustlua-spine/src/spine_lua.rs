//! `spine` Lua 表（M84 P1）：动画切换、皮肤、速度、开关、事件与包围盒。
//!
//! 事实源是 ECS 里的 `SpinePlayer` 组件；事件来自 `SpineState`（取走即清）。

use mlua::{Lua, Result as LuaResult};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::{World, WorldError};
use rustlua_lua_api::{Handle, api_error};
use std::cell::{Ref, RefMut};
use std::path::PathBuf;

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
// `spine.snapshot_frames` 的 7 元组参数会触发 `clippy::type_complexity`；但**不能**抽成
// `type` 别名——`scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断 Lua 侧签名，别名会让
// 生成的标题退化成 `(((id, width, …): SnapshotFramesArgs))`（实测 2026-11-26）。
// 这里按仓库既有先例（`push_image_quad` 的 too_many_arguments）在函数级点名豁免。
#[allow(clippy::type_complexity)]
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

    // `spine.snapshot(id, width, height, path)`：把当前姿态烤成一张 PNG（M85 G03）。
    //
    // 存在的理由：动态 UI 整层压在 WorldSprite 之上（render_plan 规范顺序），
    // 界面里"角色站在控件之间"做不到；烤成贴图后就能当普通图片用，
    // 同时解决"运行期换不了骨骼"（G10）与"角色没法当头像"（原 G03）。
    // 返回 true / 抛出带原因的错误（空骨架、找不到图集页、写盘失败）。
    let s = Bindings::new(world, state);
    spine.set(
        "snapshot",
        lua.create_function(
            move |lua, (id, width, height, path): (i64, Option<u32>, Option<u32>, String)| {
                let id = entity_from_lua(id)?;
                require_alive(&s, id)?;
                let w = width.unwrap_or(256).clamp(8, 4096);
                let h = height.unwrap_or(256).clamp(8, 4096);

                let world = s.world()?;
                let player = require_player(&world, id)?;
                let project_root = s.state.borrow()?.project_root().map(PathBuf::from);
                let out_path = match project_root.as_ref() {
                    Some(root) => root.join(path.replace('\\', "/")),
                    None => PathBuf::from(&path),
                };
                crate::snapshot::write_snapshot(
                    player.output(),
                    player.page_paths(),
                    project_root.as_deref(),
                    w,
                    h,
                    &out_path,
                )
                .map_err(|error| api_error(format!("spine.snapshot failed: {error}")))?;
                lua_snapshot_result(lua, &out_path)
            },
        )?,
    )?;

    // `spine.snapshot_frames(id, width, height, path, frames, dt, prefix)`：
    // 逐帧推进并各烤一张 PNG，返回文件名列表（M85 G03 的序列帧形态）。
    //
    // 与单帧 `snapshot` 的区别只有"推进时间"：每帧 `advance(dt)` 后取一次姿态。
    // 用途是把一段动作（攻击/受击/待机）烤成帧序列，UI 层按帧号换图，
    // 从而绕开"运行期换不了骨骼"（G10）并让角色能出现在任意界面层。
    let s = Bindings::new(world, state);
    spine.set(
        "snapshot_frames",
        lua.create_function(
            move |lua,
                  (id, width, height, dir, frames, dt, prefix): (
                i64,
                Option<u32>,
                Option<u32>,
                String,
                Option<u32>,
                Option<f32>,
                Option<String>,
            )| {
                let id = entity_from_lua(id)?;
                require_alive(&s, id)?;
                let w = width.unwrap_or(256).clamp(8, 4096);
                let h = height.unwrap_or(256).clamp(8, 4096);
                let count = frames.unwrap_or(1).clamp(1, 240);
                let step = dt.unwrap_or(1.0 / 30.0).max(0.0);
                let stem = prefix.unwrap_or_else(|| "frame".to_owned());

                let project_root = s.state.borrow()?.project_root().map(PathBuf::from);
                let out_dir = match project_root.as_ref() {
                    Some(root) => root.join(dir.replace('\\', "/")),
                    None => PathBuf::from(&dir),
                };

                let out = lua.create_table()?;
                let mut world = s.world_mut()?;
                let player = world
                    .get_mut::<SpinePlayer>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no spine skeleton"))?;
                for frame in 0..count {
                    // 第 0 帧先推进一个 dt，避免拿到 setup pose（未摆好的 T-pose）
                    player
                        .advance(if step > 0.0 { step } else { 0.0 })
                        .map_err(|error| api_error(error.to_string()))?;
                    let file = out_dir.join(format!("{stem}_{frame:03}.png"));
                    crate::snapshot::write_snapshot(
                        player.output(),
                        player.page_paths(),
                        project_root.as_deref(),
                        w,
                        h,
                        &file,
                    )
                    .map_err(|error| api_error(format!("spine.snapshot_frames failed: {error}")))?;
                    out.set(
                        frame + 1,
                        lua.create_string(file.to_string_lossy().as_bytes())?,
                    )?;
                }
                Ok(out)
            },
        )?,
    )?;

    lua.globals().set("spine", spine)?;
    Ok(())
}

/// `spine.snapshot` 的返回值：写出的绝对路径（脚本可直接拼进 `ui.image`）。
fn lua_snapshot_result(lua: &Lua, path: &std::path::Path) -> LuaResult<mlua::Value> {
    Ok(mlua::Value::String(
        lua.create_string(path.to_string_lossy().as_bytes())?,
    ))
}
