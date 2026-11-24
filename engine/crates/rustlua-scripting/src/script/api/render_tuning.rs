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

/// 往 `RenderOverrides` 能力单元里写一个参数（P1 阶段 4e）。
///
/// 这 18 个 setter 的形态完全一致——"借 VM 状态 → 借渲染参数槽 → 写一个字段"。
/// 收成一个自由函数之后，每个闭包只剩一行语义，也避免 `Rc<RefCell<>>` 的双层借用
/// 在每处各抄一遍（抄错一次就是运行期 panic）。
fn overrides_mut(
    state: &std::rc::Rc<std::cell::RefCell<ScriptState>>,
    write: impl FnOnce(&mut crate::script::domain::render::render_overrides::RenderOverrides),
) {
    let overrides = state.borrow().render_overrides.clone();
    overrides.with_mut(write);
}

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
                overrides_mut(&fog_state, |o| o.fog_density = Some(density));
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
                overrides_mut(&color_state, |o| o.fog_color = Some(color));
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
                overrides_mut(&coverage_state, |o| o.cloud_coverage = Some(coverage));
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
                overrides_mut(&speed_state, |o| o.cloud_speed = Some(speed));
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
                overrides_mut(&base_state, |o| o.cloud_base = Some(base));
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
                overrides_mut(&shadow_state, |o| o.cloud_shadow_strength = Some(strength));
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
                overrides_mut(&flash_state, |o| o.flash = Some(intensity));
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
                overrides_mut(&water_state, |o| o.water_tint = Some(tint));
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
            overrides_mut(&quality_state, |o| o.quality = Some(level as u8));
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
                overrides_mut(&cover_state, |o| o.surface_cover = Some(cover));
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
                overrides_mut(&exposure_state, |o| o.exposure = Some(ev));
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
                overrides_mut(&ripple_state, |o| o.ripple = Some([x, z]));
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
                overrides_mut(&post_state, |o| o.post_fx = Some([vignette, grain]));
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
                overrides_mut(&cap_state, |o| o.clip_cap = Some(depth));
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
                overrides_mut(&both_sides_state, |o| o.clip_cap_both_sides = Some(enabled));
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
                overrides_mut(&refract_state, |o| {
                    o.water_refract_steps = Some(value as u32)
                });
                Ok((true, String::new()))
            })?,
        )?;
    }
    #[cfg(feature = "3d")]
    {
        let caster_distance_state = state.clone();
        table.set(
            "set_shadow_caster_distance",
            lua.create_function(move |_, meters: Value| {
                // P3-R283：距离预算是**整数米档位**（0 = 关）。与 R242 的步进数同形：
                // 非数、非有限、带小数、越界一律拒绝，不静默取整——悄悄改档会让
                // 「哪些投射者丢阴影」与脚本写的数对不上。0 是合法值且必须留痕
                // Some(0)，宿主靠它把启动期的 --shadow-caster-distance 覆盖掉。
                let value = match meters {
                    Value::Integer(number) => number as f64,
                    Value::Number(number) => number,
                    _ => {
                        return Ok((false, "shadow caster distance must be a number".to_owned()));
                    }
                };
                if !value.is_finite() || value.fract() != 0.0 || !(0.0..=1000.0).contains(&value) {
                    return Ok((
                        false,
                        "shadow caster distance must be an integer in 0..=1000".to_owned(),
                    ));
                }
                overrides_mut(&caster_distance_state, |o| {
                    o.shadow_caster_distance = Some(value as u32)
                });
                Ok((true, String::new()))
            })?,
        )?;
    }
    // M95：按名字调参数——非程序员只面对"参数名"，不用记槽位号。
    {
        let named_state = state.clone();
        table.set(
            "set_effect_param",
            lua.create_function(move |_, (name, value): (String, f32)| {
                if !value.is_finite() {
                    return Ok((false, "effect param value must be finite".to_owned()));
                }
                let overrides = named_state.borrow().render_overrides.clone();
                let mut guard = match overrides.borrow_mut() {
                    Ok(guard) => guard,
                    Err(error) => return Err(mlua::Error::runtime(error.to_string())),
                };
                let Some(names) = guard.effect_param_names.clone() else {
                    return Ok((
                        false,
                        "宿主还没有加载特效参数表（先给项目配置 effect_overlay）".to_owned(),
                    ));
                };
                let Some((_, slot, component)) =
                    names.iter().find(|(candidate, _, _)| candidate == &name)
                else {
                    let known: Vec<&str> = names.iter().map(|(n, _, _)| n.as_str()).collect();
                    return Ok((
                        false,
                        format!("没有名为 {name} 的特效参数；可用参数：{}", known.join(", ")),
                    ));
                };
                let slots = guard
                    .effect_params
                    .get_or_insert_with(|| vec![[0.0; 4]; 16]);
                slots[*slot][*component] = value;
                Ok((true, String::new()))
            })?,
        )?;
    }
    // M94：逐帧调特效叠加层参数。参数是 16 个 vec4 槽位（编号见 .sgraph 的参数表），
    // 用"槽号 + 四个分量"写入；越界/非有限一律拒绝，不静默夹紧。
    {
        let params_state = state.clone();
        table.set(
            "set_effect_params",
            lua.create_function(move |_, (slot, x, y, z, w): (i64, f32, f32, f32, f32)| {
                if !(0..16).contains(&slot) {
                    return Ok((false, "effect slot must be an integer in 0..16".to_owned()));
                }
                if ![x, y, z, w].iter().all(|value| value.is_finite()) {
                    return Ok((false, "effect params must be finite".to_owned()));
                }
                overrides_mut(&params_state, |o| {
                    let slots = o.effect_params.get_or_insert_with(|| vec![[0.0; 4]; 16]);
                    slots[slot as usize] = [x, y, z, w];
                });
                Ok((true, String::new()))
            })?,
        )?;
    }
    {
        let time_state = state.clone();
        table.set(
            "set_effect_time",
            lua.create_function(move |_, seconds: f32| {
                if !seconds.is_finite() || seconds < 0.0 {
                    return Ok((false, "effect time must be finite and >= 0".to_owned()));
                }
                overrides_mut(&time_state, |o| o.effect_time = Some(seconds));
                Ok((true, String::new()))
            })?,
        )?;
    }
    // M96：按预设播特效。`effect.play("assets/fx/炎刃斩击.prefab")` 之后宿主当帧加载并挂层，
    // 逻辑里不必再关心 `.sgraph` 路径、参数槽位或工程配置——那些都封在预设里。
    {
        let play_state = state.clone();
        table.set(
            "play_effect",
            lua.create_function(move |_, (prefab, name): (String, Option<String>)| {
                if prefab.is_empty() || prefab.trim() != prefab {
                    return Ok((
                        false,
                        "effect prefab path must be a non-empty project-relative path".to_owned(),
                    ));
                }
                if !prefab.ends_with(".prefab") {
                    return Ok((
                        false,
                        format!("effect presets are .prefab files, got {prefab}"),
                    ));
                }
                overrides_mut(&play_state, |o| {
                    o.effect_command = Some(
                        crate::script::domain::render::render_overrides::EffectCommand::Play {
                            prefab,
                            name: name.unwrap_or_default(),
                        },
                    )
                });
                Ok((true, String::new()))
            })?,
        )?;
        let stop_state = state.clone();
        table.set(
            "stop_effect",
            lua.create_function(move |_, name: Option<String>| {
                overrides_mut(&stop_state, |o| {
                    o.effect_command = Some(
                        crate::script::domain::render::render_overrides::EffectCommand::Stop {
                            name: name.unwrap_or_default(),
                        },
                    )
                });
                Ok((true, String::new()))
            })?,
        )?;
    }
    let stats_state = state.clone();
    table.set(
        "stats",
        lua.create_function(move |lua, ()| {
            let stats = stats_state
                .borrow()
                .render_overrides
                .with(|o| o.stats)
                .unwrap_or([0; 4]);
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
