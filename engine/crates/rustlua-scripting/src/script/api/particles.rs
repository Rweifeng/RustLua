//! `particles` 表：粒子发射器（P1 ⑤-9 从 `api/physics.rs` 拆出）。
//!
//! 这一半**刻意留在脚本层**：粒子是 `rustlua_ecs::particle` 里的发射器实例列表，没有"粒子
//! 模拟域"需要在领域 crate 里推进规则（推进与绘制都由宿主直接调 `ParticleSystem`）。原先它与
//! `physics`/`physics3d` 同处一个文件，只是因为历史顺序，不是因为同域——搬物理绑定时顺手拆开，
//! 下次谁来做粒子域，边界已经是现成的。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::api_error;
use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_ecs::particle::{ParticleEmitter, ParticleMode};
use rustlua_ecs::transform::Vec2;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let particles = lua.create_table()?;
    let s = Rc::clone(&state);
    // `particles.burst(x, y, count [, opts])`：opts 是可选表，用来给这一次爆发不同的
    // 观感（此前引擎把速度/寿命/颜色**写死**在 120/0.6/橙色，Lua 完全无法区分尘土、
    // 水花、落叶）。缺省值与旧行为一致，因此老脚本零改动。
    //
    // P3-R07-1 起 opts 另有 `mode`（`"cpu"`/`"gpu"`，默认 cpu）与 `seed`（固定抽样序列，
    // 对拍/可复现用）——与场景里 `ParticleEmitter` 组件串同一套键名与同一份校验。
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
                    // P3-R07-1：`mode`/`seed` 与场景组件串同一口径。
                    // P3-R07-2 起 `mode="gpu"` 真的换落点：这一批进**派发队列**，宿主在脚本步进
                    // 之后把它交给渲染器（`RuntimeScene::dispatch_gpu_particle_bursts`）。设备没有
                    // compute、池预算不够、或宿主根本没接派发通道时，整批原样退回 CPU 池——
                    // **不丢粒子**。已在 GPU 池里的那批 Lua 读不到（视觉层语义，见 docs/24 §3.12）。
                    // 拼错必须报错，不能悄悄当成 cpu。
                    if let Some(value) = options.get::<Option<String>>("mode")? {
                        emitter.mode = ParticleMode::parse(&value).map_err(api_error)?;
                    }
                    if let Some(value) = options.get::<Option<i64>>("seed")? {
                        if value < 0 {
                            return Err(api_error("particles.burst seed must be >= 0"));
                        }
                        emitter.seed = Some(value as u64);
                    }
                }
                emitter.validate().map_err(api_error)?;
                // P3-R07-2：`mode` 决定落点——cpu 直接进池，gpu 进派发队列等宿主交给渲染器
                // （同一份 `spawn_particles`，Lua 与宿主 API 不会走岔）。
                s.lua_borrow_mut()?
                    .spawn_particles(Vec2::new(x, y), &emitter);
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
