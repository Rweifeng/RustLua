//! `particles` 表：粒子发射器（P1 ⑤-9 从 `api/physics.rs` 拆出）。
//!
//! 这一半**刻意留在脚本层**：粒子是 `rustlua_ecs::particle` 里的发射器实例列表，没有"粒子
//! 模拟域"需要在领域 crate 里推进规则（推进与绘制都由宿主直接调 `ParticleSystem`）。原先它与
//! `physics`/`physics3d` 同处一个文件，只是因为历史顺序，不是因为同域——搬物理绑定时顺手拆开，
//! 下次谁来做粒子域，边界已经是现成的。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::domain::ecs::particle::{ParticleEmitter, ParticleMode, WindSource};
use crate::script::domain::ecs::transform::Vec2;
use crate::script::error::api_error;
use mlua::Lua;
use mlua::Result as LuaResult;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

/// 把 Lua 的 `opts` 表解析成与场景组件串同口径的发射器。
///
/// 缺省值与 `ParticleEmitter::default` 一致；未知键刻意忽略（Lua 表是开放的，调用方
/// 可能复用一张带 UI 字段的表），非法值仍由 `validate` 拒绝。
fn parse_emitter(count: u32, options: Option<mlua::Table>) -> LuaResult<ParticleEmitter> {
    let mut emitter = ParticleEmitter::new(count as usize, 120.0, 0.6, [1.0, 0.6, 0.2, 1.0]);
    let Some(options) = options else {
        return Ok(emitter);
    };
    let number = |key: &str| -> LuaResult<Option<f32>> { options.get::<Option<f32>>(key) };
    let color = |key: &str| -> LuaResult<Option<[f32; 4]>> {
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
    if let Some(value) = number("gravity_z")? {
        emitter.gravity_z = value;
    }
    if let Some(value) = number("buoyancy")? {
        emitter.buoyancy = value;
    }
    if let Some(value) = number("speed_z")? {
        emitter.speed_z = value;
    }
    if let Some(value) = number("spread_z")? {
        emitter.spread_z = value;
    }
    for (index, key) in ["area_x", "area_y", "area_z"].iter().enumerate() {
        if let Some(value) = number(key)? {
            emitter.spawn_area[index] = value;
        }
    }
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
            return Err(api_error("particles burst seed must be >= 0"));
        }
        emitter.seed = Some(value as u64);
    }
    Ok(emitter)
}

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
                let emitter = parse_emitter(count, options)?;
                emitter.validate().map_err(api_error)?;
                s.lua_borrow_mut()?
                    .spawn_particles(Vec2::new(x, y), &emitter);
                Ok(())
            },
        )?,
    )?;
    // `particles.burst3d(x, y, z, count [, opts])`：与 2D burst 共用同一份发射参数，
    // 额外接受 `gravity_z` / `speed_z` / `spread_z`。z 会进入 GPU 状态或 CPU 池，
    // 由宿主的 3D billboard / 世界通道绘制。
    let s = Rc::clone(&state);
    particles.set(
        "burst3d",
        lua.create_function(
            move |_, (x, y, z, count, options): (f32, f32, f32, u32, Option<mlua::Table>)| {
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    return Err(api_error("particles.burst3d position must be finite"));
                }
                let emitter = parse_emitter(count, options)?;
                emitter.validate().map_err(api_error)?;
                s.lua_borrow_mut()?.spawn_particles_3d([x, y, z], &emitter);
                Ok(())
            },
        )?,
    )?;
    // `particles.stream3d(key, x, y, z, rate [, opts])`：声明一条持续 3D 发射器。
    // 与场景 `ParticleEmitter{rate>0,mode=gpu}` 落到同一条渲染器环形流；脚本用
    // `stop_stream3d(key)` 显式停止，低频调用（每帧或改参数时）即可，不需要每帧重发。
    let s = Rc::clone(&state);
    particles.set(
        "stream3d",
        lua.create_function(
            move |_,
                  (key, x, y, z, rate, options): (
                String,
                f32,
                f32,
                f32,
                f32,
                Option<mlua::Table>,
            )| {
                if key.is_empty() || key.len() > 64 || key.chars().any(char::is_control) {
                    return Err(api_error(
                        "particles.stream3d key must be 1..=64 printable characters",
                    ));
                }
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    return Err(api_error("particles.stream3d position must be finite"));
                }
                if !rate.is_finite() || rate <= 0.0 || rate > 10_000.0 {
                    return Err(api_error("particles.stream3d rate must be in (0, 10000]"));
                }
                let mut emitter = parse_emitter(0, options)?;
                emitter.rate = rate;
                emitter.mode = ParticleMode::Gpu;
                emitter.validate().map_err(api_error)?;
                s.lua_borrow_mut()?
                    .upsert_particle_stream(key, [x, y, z], emitter);
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    particles.set(
        "stop_stream3d",
        lua.create_function(move |_, key: String| {
            Ok(s.lua_borrow_mut()?.stop_particle_stream(&key))
        })?,
    )?;
    let s = Rc::clone(&state);
    particles.set(
        "clear_streams3d",
        lua.create_function(move |_, ()| {
            s.lua_borrow_mut()?.clear_particle_streams();
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    particles.set(
        "stream_count",
        lua.create_function(move |_, ()| Ok(s.lua_borrow()?.particle_streams.len()))?,
    )?;
    // 全局风加速度（M74 欠项）：只作用于 CPU 粒子池；非有限值明确拒绝。
    let s = Rc::clone(&state);
    particles.set(
        "set_wind",
        lua.create_function(move |_, (x, y): (f32, f32)| {
            match s.lua_borrow_mut()?.particles.set_wind(Vec2::new(x, y)) {
                Ok(()) => Ok((true, String::new())),
                Err(error) => Ok((false, error)),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    particles.set(
        "set_wind3d",
        lua.create_function(move |_, (x, y, z): (f32, f32, f32)| {
            match s.lua_borrow_mut()?.particles.set_wind_3d(x, y, z) {
                Ok(()) => Ok((true, String::new())),
                Err(error) => Ok((false, error)),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    particles.set(
        "wind",
        lua.create_function(move |_, ()| {
            let wind = s.lua_borrow()?.particles.wind();
            Ok((wind.x, wind.y))
        })?,
    )?;
    let s = Rc::clone(&state);
    particles.set(
        "wind3d",
        lua.create_function(move |_, ()| {
            let state = s.lua_borrow()?;
            let wind = state.particles.wind();
            Ok((wind.x, wind.y, state.particles.wind_z()))
        })?,
    )?;
    // 风场：高度剪切 + 空间阵风。CPU 池与 GPU 后端共用同一条公式。
    let s = Rc::clone(&state);
    particles.set(
        "set_wind_profile",
        lua.create_function(move |_, (shear, gust, gust_hz): (f32, f32, f32)| {
            match s
                .lua_borrow_mut()?
                .particles
                .set_wind_profile(shear, gust, gust_hz)
            {
                Ok(()) => Ok((true, String::new())),
                Err(error) => Ok((false, error)),
            }
        })?,
    )?;
    let s = Rc::clone(&state);
    particles.set(
        "wind_profile",
        lua.create_function(move |_, ()| Ok(s.lua_borrow()?.particles.wind_profile()))?,
    )?;
    // 局部风源：最多 4 个，坐标与 burst3d 一致（x/y 水平面，y 是高度，z 深度）。
    let s = Rc::clone(&state);
    particles.set(
        "set_wind_source",
        lua.create_function(
            move |_,
                  (index, x, y, z, radius, radial, lift, swirl): (
                u32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                Option<f32>,
            )| {
                let source = WindSource {
                    x,
                    y,
                    z,
                    radius,
                    radial,
                    lift,
                    swirl: swirl.unwrap_or(0.0),
                };
                match s
                    .lua_borrow_mut()?
                    .particles
                    .set_wind_source(index as usize, source)
                {
                    Ok(()) => Ok((true, String::new())),
                    Err(error) => Ok((false, error)),
                }
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    particles.set(
        "clear_wind_sources",
        lua.create_function(move |_, ()| {
            s.lua_borrow_mut()?.particles.clear_wind_sources();
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    particles.set(
        "count",
        lua.create_function(move |_, ()| Ok(s.lua_borrow()?.particles.len() as u32))?,
    )?;
    lua.globals().set("particles", particles)?;
    Ok(())
}
