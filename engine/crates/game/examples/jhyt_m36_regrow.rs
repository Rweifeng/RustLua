//! Headless acceptance drive for regrowing crops (改造计划 §4.2 / docs/33 §4.1.1, M36).
//!
//! 验证再生作物（`crop.regrow_days`）的真实耕作链路：
//!   1. 薄荷（春、药田）：翻土 → 播种 → 浇 3 天水成熟 → 收获 2 个（== yield）；
//!   2. 收获后**地块保留作物**（p.crop == "mint"、watered_days 归 0、cycles == 1）；
//!   3. 再浇 `regrow_days`(3) 天水复熟 → 再收获 2 个（cycles == 2），地块仍未清空；
//!   4. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! 对照口径：非再生作物（lingzhi/rice/radish）的"收获后清空地块"已由
//! `jhyt_m24_numeric_alignment` ⑧④⑨ 覆盖，这里只锁再生分支。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m36_regrow [-- <game_jhyt-root>]

use rustlua_input::input::{InputSnapshot, KeyCode};
use rustlua_scripting::script::LuaRuntime;
use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        rustlua_infra::games_dir()
            .join("examples")
            .join("game_jhyt")
    });
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let main_script = project.join("scripts/main.lua");

    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.clone());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    runtime.execute_project_script(&main_script)?;

    banner("① 进山庄新档，药田格翻土 + 播种薄荷");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 新档背包只有稻种/菌种（真实玩家从赵货摊购买薄荷籽）；测试角色先补 1 粒，
    // 否则 seed_for_selected 回退到第一个有库存的种子（稳定字母序 = 灵芝）。
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_mint', 1)")?;
    set_facing(&mut runtime, "up")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    set_seed(&mut runtime, "seed_mint")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_mint",
    )?;

    banner("② 首茬：浇 3 天水成熟（growth_days=3），收获 2 个 == yield");
    let mint_before = total_crop(&mut runtime, "mint")?;
    grow_mint(&mut runtime)?;
    equip_tool(&mut runtime, "hoe")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    let mint_yield = total_crop(&mut runtime, "mint")? - mint_before;
    let documented = query_num(&mut runtime, "require('data.crops').mint.yield")?;
    if (mint_yield - documented).abs() > f64::EPSILON {
        return Err(format!(
            "薄荷首茬实际收获 {mint_yield}，data/crops.lua:mint.yield = {documented}"
        )
        .into());
    }
    let plot_after_first = query_str(
        &mut runtime,
        "local m = map.state('farm_plots')\n\
         for _, p in pairs(m) do\n\
           if p.crop == 'mint' then\n\
             return string.format('%s|%s|%s', tostring(p.crop), tostring(p.watered_days), tostring(p.cycles or 0))\n\
           end\n\
         end\n\
         return 'none'",
    )?;
    if plot_after_first != "mint|0|1" {
        return Err(format!(
            "首茬收获后地块应为「crop=mint, watered_days=0, cycles=1」，实际 {plot_after_first:?}"
        )
        .into());
    }
    println!("  首茬收获 {mint_yield} 个，地块状态 = {plot_after_first}（作物保留，进入再生）");

    banner("③ 二茬：再浇 3 天水（regrow_days=3）复熟，再收获 2 个 == yield");
    let mint_before_two = total_crop(&mut runtime, "mint")?;
    grow_mint(&mut runtime)?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    let mint_yield_two = total_crop(&mut runtime, "mint")? - mint_before_two;
    if (mint_yield_two - documented).abs() > f64::EPSILON {
        return Err(format!(
            "薄荷二茬实际收获 {mint_yield_two}，应为 {documented}（每茬产量不变）"
        )
        .into());
    }
    let plot_after_two = query_str(
        &mut runtime,
        "local m = map.state('farm_plots')\n\
         for _, p in pairs(m) do\n\
           if p.crop == 'mint' then\n\
             return string.format('%s|%s|%s', tostring(p.crop), tostring(p.watered_days), tostring(p.cycles or 0))\n\
           end\n\
         end\n\
         return 'none'",
    )?;
    if plot_after_two != "mint|0|2" {
        return Err(format!(
            "二茬收获后地块应为「crop=mint, watered_days=0, cycles=2」，实际 {plot_after_two:?}"
        )
        .into());
    }
    println!(
        "  二茬收获 {mint_yield_two} 个，地块状态 = {plot_after_two}（作物保留，可继续第三茬）"
    );

    banner("④ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m36_regrow_ok project={} yield={} first={} second={} plot1={} plot2={}",
        project.display(),
        documented,
        mint_yield,
        mint_yield_two,
        plot_after_first,
        plot_after_two
    );
    Ok(())
}

/// 给薄荷浇水直到成熟：每天钉晴天（M29 起天气按 (seed,day,季节) 抽签，跨天重抽）、
/// 浇水、睡觉；当前茬次的成熟所需浇水天数由地块 cycles 与 crop.regrow_days 决定
/// （首茬 = growth_days，再生茬 = regrow_days）。成熟即返回（地块停在成熟态，
/// 由调用方收获）。
fn grow_mint(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    equip_tool(runtime, "watering_can")?;
    for day in 1..=10 {
        runtime.execute_console("require('world.weather').set('sunny')")?;
        press(runtime, KeyCode::KeyE)?;
        let action = query_str(runtime, "require('core.state').last_action")?;
        if action == "harvest_mint" {
            return Err("浇水时薄荷已成熟（成熟判定早于预期），应先行收获".into());
        }
        if action != "water_mint" {
            return Err(format!("浇水失败，last_action = {action:?}（第 {day} 天）").into());
        }
        runtime.execute_console("day.sleep()")?;
        let mature = query_str(
            runtime,
            "local m = map.state('farm_plots')\n\
             local c = require('data.crops').mint\n\
             for _, p in pairs(m) do\n\
               if p.crop == 'mint' then\n\
                 local need = (c.regrow_days and p.cycles and p.cycles > 0) and c.regrow_days or c.growth_days\n\
                 if p.watered_days >= need then return 'yes' end\n\
               end\n\
             end\n\
             return 'no'",
        )?;
        if mature == "yes" {
            return Ok(());
        }
        if day >= 10 {
            return Err("薄荷 10 天未成熟，再生生长判定可能被破坏".into());
        }
    }
    unreachable!()
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}

fn open_main_menu(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console("scene.load('main')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(runtime, "main")?;
    Ok(())
}

fn click_button(runtime: &mut LuaRuntime, button: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("on_click('{button}')"))?;
    runtime.process_scene_transition()?;
    Ok(())
}

fn press(runtime: &mut LuaRuntime, key: KeyCode) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
}

fn set_facing(runtime: &mut LuaRuntime, facing: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('core.state').facing = '{facing}'"))?;
    Ok(())
}

fn set_seed(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('core.state').selected_seed = '{item}'"))?;
    Ok(())
}

fn equip_tool(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "inv.equip(require('core.state').inventory, 'tool', '{item}')"
    ))?;
    Ok(())
}

fn query_str(runtime: &mut LuaRuntime, block: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(block)?.trim().to_owned())
}

fn query_num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let value = query_str(runtime, &format!("return tostring({expr})"))?;
    value
        .parse::<f64>()
        .map_err(|error| format!("{value:?} is not a number: {error}").into())
}

/// 统计某收获物的背包总量 = 普通 + 良好 + 上品三条目之和（品质系统
/// docs/33 §4.1.2：收获可能产出 `<id>_good` / `<id>_super`，数量断言必须合并统计）。
fn total_crop(runtime: &mut LuaRuntime, crop_id: &str) -> Result<f64, Box<dyn std::error::Error>> {
    query_num(
        runtime,
        &format!(
            "inv.count('player', '{crop_id}') + inv.count('player', '{crop_id}_good') + inv.count('player', '{crop_id}_super')"
        ),
    )
}

fn assert_str(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(runtime, &format!("return tostring({expr})"))?;
    if got != expected {
        return Err(format!("{expr} = {got:?}, want {expected:?}").into());
    }
    Ok(())
}

fn assert_scene_is(
    runtime: &mut LuaRuntime,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let scene = query_str(runtime, "return scene.reload()")?;
    if scene != expected {
        return Err(format!("scene = {scene:?}, want {expected:?}").into());
    }
    Ok(())
}
