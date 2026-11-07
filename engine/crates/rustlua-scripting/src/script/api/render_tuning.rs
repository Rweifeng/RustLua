//! `render.*`：运行期渲染调参（P3-R76）。
//!
//! 只放**宿主每帧能低成本应用**的标量（当前是雾密度）；纹理/管线级开关仍走
//! `RenderSettings`/编辑器，避免脚本把渲染器状态改到无法回滚。
use std::{cell::RefCell, rc::Rc};

use mlua::{Lua, Result as LuaResult};

use crate::script::ScriptState;

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let table = lua.create_table()?;
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
    let color_state = state.clone();
    table.set(
        "set_fog_color",
        lua.create_function(move |_, (r, g, b): (f32, f32, f32)| {
            let color = [r, g, b];
            if color.iter().any(|value| !value.is_finite() || !(0.0..=1.0).contains(value)) {
                return Ok((false, "fog color must be finite and 0..=1".to_owned()));
            }
            color_state.borrow_mut().render_fog_color = Some(color);
            Ok((true, String::new()))
        })?,
    )?;
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
    let shadow_state = state.clone();
    table.set(
        "set_cloud_shadow_strength",
        lua.create_function(move |_, strength: f32| {
            if !strength.is_finite() || !(0.0..=0.6).contains(&strength) {
                return Ok((false, "cloud shadow strength must be finite and 0..=0.6".to_owned()));
            }
            shadow_state.borrow_mut().render_cloud_shadow_strength = Some(strength);
            Ok((true, String::new()))
        })?,
    )?;
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
    let water_state = state.clone();
    table.set(
        "set_water_tint",
        lua.create_function(move |_, (r, g, b, turbidity): (f32, f32, f32, f32)| {
            let tint = [r, g, b, turbidity];
            if tint.iter().any(|value| !value.is_finite() || !(0.0..=1.0).contains(value)) {
                return Ok((false, "water tint must be finite and 0..=1".to_owned()));
            }
            water_state.borrow_mut().render_water_tint = Some(tint);
            Ok((true, String::new()))
        })?,
    )?;
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
    let post_state = state.clone();
    table.set(
        "set_post_fx",
        lua.create_function(move |_, (vignette, grain): (f32, f32)| {
            if !vignette.is_finite() || !grain.is_finite()
                || !(0.0..=1.0).contains(&vignette) || !(0.0..=1.0).contains(&grain) {
                return Ok((false, "post fx must be finite and 0..=1".to_owned()));
            }
            post_state.borrow_mut().render_post_fx = Some([vignette, grain]);
            Ok((true, String::new()))
        })?,
    )?;
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
