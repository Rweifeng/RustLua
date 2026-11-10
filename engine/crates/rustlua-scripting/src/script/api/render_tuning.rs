//! `render.*`：运行期渲染调参（P3-R76）。
//!
//! 只放**宿主每帧能低成本应用**的标量（当前是雾密度）；纹理/管线级开关仍走
//! `RenderSettings`/编辑器，避免脚本把渲染器状态改到无法回滚。
//!
//! M81 批 2：环境/后处理 12 函数属 3D 包（雾/云/水/曝光/后处理都是 3D 环境管线），
//! 函数级 `#[cfg(feature = "3d")]`；`stats` / `set_quality` 维度无关，留基底。
use std::{cell::RefCell, rc::Rc};

use mlua::{Lua, Result as LuaResult, Value};

use crate::script::ScriptState;

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let table = lua.create_table()?;
    #[cfg(feature = "3d")]
    {
        let fog_state = state.clone();
        table.set(
            "set_fog",
            lua.create_function(move |_, density: f32| {
                if !density.is_finite() || !(0.0..=0.05).contains(&density) {
                    return Ok((false, "fog density must be finite and 0..=0.05".to_owned()));
                }
                fog_state.borrow_mut().render_fog = Some(density);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let color_state = state.clone();
        table.set(
            "set_fog_color",
            lua.create_function(move |_, (r, g, b): (f32, f32, f32)| {
                let color = [r, g, b];
                if color
                    .iter()
                    .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
                {
                    return Ok((false, "fog color must be finite and 0..=1".to_owned()));
                }
                color_state.borrow_mut().render_fog_color = Some(color);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let coverage_state = state.clone();
        table.set(
            "set_cloud_coverage",
            lua.create_function(move |_, coverage: f32| {
                if !coverage.is_finite() || !(0.0..=1.0).contains(&coverage) {
                    return Ok((false, "cloud coverage must be finite and 0..=1".to_owned()));
                }
                coverage_state.borrow_mut().render_cloud_coverage = Some(coverage);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let speed_state = state.clone();
        table.set(
            "set_cloud_speed",
            lua.create_function(move |_, speed: f32| {
                if !speed.is_finite() || !(0.0..=4.0).contains(&speed) {
                    return Ok((false, "cloud speed must be finite and 0..=4".to_owned()));
                }
                speed_state.borrow_mut().render_cloud_speed = Some(speed);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let base_state = state.clone();
        table.set(
            "set_cloud_base",
            lua.create_function(move |_, base: f32| {
                if !base.is_finite() || !(40.0..=200.0).contains(&base) {
                    return Ok((false, "cloud base must be finite and 40..=200".to_owned()));
                }
                base_state.borrow_mut().render_cloud_base = Some(base);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let shadow_state = state.clone();
        table.set(
            "set_cloud_shadow_strength",
            lua.create_function(move |_, strength: f32| {
                if !strength.is_finite() || !(0.0..=0.6).contains(&strength) {
                    return Ok((
                        false,
                        "cloud shadow strength must be finite and 0..=0.6".to_owned(),
                    ));
                }
                shadow_state.borrow_mut().render_cloud_shadow_strength = Some(strength);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let flash_state = state.clone();
        table.set(
            "set_flash",
            lua.create_function(move |_, intensity: f32| {
                if !intensity.is_finite() || !(0.0..=1.0).contains(&intensity) {
                    return Ok((false, "flash intensity must be finite and 0..=1".to_owned()));
                }
                flash_state.borrow_mut().render_flash = Some(intensity);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let water_state = state.clone();
        table.set(
            "set_water_tint",
            lua.create_function(move |_, (r, g, b, turbidity): (f32, f32, f32, f32)| {
                let tint = [r, g, b, turbidity];
                if tint
                    .iter()
                    .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
                {
                    return Ok((false, "water tint must be finite and 0..=1".to_owned()));
                }
                water_state.borrow_mut().render_water_tint = Some(tint);
                Ok((true, String::new()))
            })?,
        )?;
    }
    let quality_state = state.clone();
    table.set(
        "set_quality",
        lua.create_function(move |_, level: u32| {
            if !(1..=3).contains(&level) {
                return Ok((false, "quality must be 1..=3".to_owned()));
            }
            quality_state.borrow_mut().render_quality = Some(level as u8);
            Ok((true, String::new()))
        })?,
    )?;
    #[cfg(feature = "3d")]
    {
        let cover_state = state.clone();
        table.set(
            "set_surface_cover",
            lua.create_function(move |_, (snow, wet): (f32, f32)| {
                let cover = [snow, wet];
                if cover
                    .iter()
                    .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
                {
                    return Ok((false, "surface cover must be finite and 0..=1".to_owned()));
                }
                cover_state.borrow_mut().render_surface_cover = Some(cover);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let exposure_state = state.clone();
        table.set(
            "set_exposure",
            lua.create_function(move |_, ev: f32| {
                if !ev.is_finite() || !(-4.0..=4.0).contains(&ev) {
                    return Ok((false, "exposure must be finite and -4..=4".to_owned()));
                }
                exposure_state.borrow_mut().render_exposure = Some(ev);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let ripple_state = state.clone();
        table.set(
            "set_ripple",
            lua.create_function(move |_, (x, z): (f32, f32)| {
                if !x.is_finite() || !z.is_finite() {
                    return Ok((false, "ripple position must be finite".to_owned()));
                }
                ripple_state.borrow_mut().render_ripple = Some([x, z]);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let post_state = state.clone();
        table.set(
            "set_post_fx",
            lua.create_function(move |_, (vignette, grain): (f32, f32)| {
                if !vignette.is_finite()
                    || !grain.is_finite()
                    || !(0.0..=1.0).contains(&vignette)
                    || !(0.0..=1.0).contains(&grain)
                {
                    return Ok((false, "post fx must be finite and 0..=1".to_owned()));
                }
                post_state.borrow_mut().render_post_fx = Some([vignette, grain]);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let cap_state = state.clone();
        table.set(
            "set_clip_cap",
            lua.create_function(move |_, depth: f32| {
                // 0 是合法值 = 关档（只留旧的 0.08 m 薄带），所以这里只拦非有限与越界。
                if !depth.is_finite() || !(0.0..=128.0).contains(&depth) {
                    return Ok((false, "clip cap must be finite and 0..=128".to_owned()));
                }
                cap_state.borrow_mut().render_clip_cap = Some(depth);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let both_sides_state = state.clone();
        table.set(
            "set_clip_cap_both_sides",
            lua.create_function(move |_, on: Value| {
                // 只校类型：布尔没有越界可言，但非布尔（1 / "on" / nil）必须显式拒绝——
                // 这一档改的是**管线选键**，被静默当成真/假会换来一次说不清的背面剔除变化。
                let Value::Boolean(enabled) = on else {
                    return Ok((false, "clip cap both sides must be a boolean".to_owned()));
                };
                both_sides_state.borrow_mut().render_clip_cap_both_sides = Some(enabled);
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let refract_state = state.clone();
        table.set(
            "set_water_refract_steps",
            lua.create_function(move |_, steps: Value| {
                // P3-R242：步数是**整数档位**（0 = 关 = R207 的偏移近似）。非数、非有限、
                // 带小数、越界一律拒绝，不静默取整：步数越大折射射线越远，悄悄改档会让
                // 画面与脚本写的数对不上。Lua 5.4 把 `16` 放进 Integer、`16.5` 放进 Number，
                // 两支都要接；带小数的浮点在这里被拒。
                let value = match steps {
                    Value::Integer(number) => number as f64,
                    Value::Number(number) => number,
                    _ => return Ok((false, "water refract steps must be a number".to_owned())),
                };
                if !value.is_finite() || value.fract() != 0.0 || !(0.0..=32.0).contains(&value) {
                    return Ok((
                        false,
                        "water refract steps must be an integer in 0..=32".to_owned(),
                    ));
                }
                refract_state.borrow_mut().render_water_refract_steps = Some(value as u32);
                Ok((true, String::new()))
            })?,
        )?;
    }
    let stats_state = state.clone();
    table.set(
        "stats",
        lua.create_function(move |lua, ()| {
            let stats = stats_state.borrow().render_stats.unwrap_or([0; 4]);
            let table = lua.create_table()?;
            table.set("draw_calls", stats[0] as f64)?;
            table.set("vertices", stats[1] as f64)?;
            table.set("triangles", stats[2] as f64)?;
            table.set("set_pass_calls", stats[3] as f64)?;
            Ok(table)
        })?,
    )?;
    lua.globals().set("render", table)?;
    Ok(())
}
