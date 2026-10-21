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
use rustlua_ecs::particle::ParticleEmitter;
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
