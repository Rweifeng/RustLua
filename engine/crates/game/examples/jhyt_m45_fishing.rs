//! Headless acceptance drive for the fishing system (钓鱼, 改造计划 §4.4,
//! docs/33 §4.5, M45).
//!
//! 验证钓鱼真实链路（数据口径 = docs/33 §4.5 == data/fish.lua ==
//! data/items_fish.lua == systems/fishing.lua）：
//!   1. 静态：4 水域、23 鱼种（≥20）、3 钓竿（tier 1..3）；fishing.validate_all 空；
//!   2. 新档山庄：装备竹钓竿（tool 槽被竿占据）→ 村溪抛竿（force 鲫鱼）成功入包、
//!      体力 100→92、last_action=catch_crucian、stats+1；
//!   3. 鱼入箱/结算：鲫鱼（sell=12）入镖局货箱 → settle 入账 +12（shipping 扩展回退）；
//!   4. 装备冲突：换回锄头（tool 槽回 hoe），证明钓竿与既有工具同槽；
//!   5. 季节负例：拨到冬季，春季特色桃花鱼（peach）出不来（season_closed），且不在
//!      当前可钓池；
//!   6. 竿阶门槛：1 阶竹钓竿钓 5 难度雪莲鱼 → rod_too_low（低阶竿钓不了高阶鱼）；
//!   7. 临时钓点：夏日晴天，雨季涨水潭（需雨天）→ weather_closed；
//!   8. on_day_start 挂点显式调用：返回当日开放钓点表（含村溪）；
//!   9. 存档往返：capture → reset → restore，钓获统计 crucian=1 恢复；
//!  10. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m45_fishing [-- <game_jhyt-root>]

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

    banner("① 静态：4 水域 / 23 鱼种 / 3 钓竿，fishing.validate_all 空");
    let meta = query_str(
        &mut runtime,
        "(function() \
         local f = require('systems.fishing') \
         local w, fish, rods = 0, 0, 0 \
         for _ in pairs(f.waters()) do w = w + 1 end \
         for _ in pairs(f.fish()) do fish = fish + 1 end \
         local ext = require('data.items_fish') \
         for _, t in pairs(ext.tools) do if t.family == 'fishing_rod' then rods = rods + 1 end end \
         return tostring(w) .. '|' .. tostring(fish) .. '|' .. tostring(rods) end)()",
    )?;
    if meta != "4|23|3" {
        return Err(format!("钓鱼注册表 = {meta:?}，应为 4|23|3").into());
    }
    let report = query_str(
        &mut runtime,
        "return require('systems.fishing').validate_all()",
    )?;
    if !report.is_empty() {
        return Err(format!("fishing.validate_all 不通过:\n{report}").into());
    }
    println!("  4 水域、23 鱼种、3 钓竿（tier 1..3）、validate_all 空 ✓");

    banner("② 新档山庄：装备竹钓竿 → 村溪抛竿（force 鲫鱼）成功入包、体力 100→92");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 拨到春日上午 10:00（600 分），晴天；鲫鱼全天可钓。
    lua(
        &mut runtime,
        "clock.set(1, 600); require('world.weather').set('sunny')",
    )?;
    // 发竿并装备：tool 槽被竹钓竿占据。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'rod_basic', 1); \
         inv.equip(require('core.state').inventory, 'tool', 'rod_basic')",
    )?;
    assert_str(
        &mut runtime,
        "inv.equip_list(require('core.state').inventory).tool",
        "rod_basic",
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    let cast = query_str(
        &mut runtime,
        "(function() local ok, r = require('systems.fishing').cast('village_brook', { force = 'crucian' }); \
         return (ok and 'ok' or 'fail') .. '|' .. tostring(r) end)()",
    )?;
    if cast != "ok|crucian" {
        return Err(format!("村溪钓鲫鱼 = {cast:?}，应为 ok|crucian").into());
    }
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "catch_crucian",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'crucian')",
        1.0,
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 92.0)?; // 100 - rod_basic.energy 8
    assert_num(
        &mut runtime,
        "require('systems.fishing').stats()['crucian']",
        1.0,
    )?;
    println!("  装备竹钓竿、抛竿成功入包鲫鱼 ×1、体力 92、stats+1 ✓");

    banner("③ 鱼入箱/结算：鲫鱼 sell=12 入镖局货箱 → settle 入账 +12");
    let money_before = query_num(&mut runtime, "require('core.state').money")?;
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('crucian', 1); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "ship_crucian",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').pending_total()",
        12.0,
    )?;
    assert_num(
        &mut runtime,
        "(function() local s = require('systems.shipping').settle(); return s and s.total or 0 end)()",
        12.0,
    )?;
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_before + 12.0,
    )?;
    println!(
        "  鲫鱼入箱 → 结算 +12 文（money {money_before} → {}）✓",
        money_before + 12.0
    );

    banner("④ 装备冲突：换回锄头，tool 槽回 hoe（钓竿与既有工具同槽）");
    lua(
        &mut runtime,
        "inv.equip(require('core.state').inventory, 'tool', 'hoe')",
    )?;
    assert_str(
        &mut runtime,
        "inv.equip_list(require('core.state').inventory).tool",
        "hoe",
    )?;
    println!("  换回锄头：tool 槽 = hoe ✓");

    banner("⑤ 季节负例：拨到冬季，春季桃花鱼出不来（season_closed）且不在可钓池");
    lua(
        &mut runtime,
        "clock.set(85, 600); require('world.weather').set('sunny')",
    )?;
    lua(
        &mut runtime,
        "inv.equip(require('core.state').inventory, 'tool', 'rod_basic')",
    )?;
    let season_neg = query_str(
        &mut runtime,
        "(function() local ok, r = require('systems.fishing').cast('village_brook', { force = 'peach' }); \
         return (ok and 'ok' or 'fail') .. '|' .. tostring(r) end)()",
    )?;
    if season_neg != "fail|season_closed" {
        return Err(format!("冬季钓春季桃花鱼 = {season_neg:?}，应为 fail|season_closed").into());
    }
    let peach_in_pool = query_str(
        &mut runtime,
        "(function() local pool = require('systems.fishing').catchable('village_brook'); \
         for _, f in ipairs(pool) do if f.id == 'peach' then return 'yes' end end return 'no' end)()",
    )?;
    if peach_in_pool != "no" {
        return Err("冬季可钓池不应含桃花鱼，实际含".to_string().into());
    }
    println!("  冬季 force 桃花鱼 → season_closed、可钓池不含桃花鱼 ✓");

    banner("⑥ 竿阶门槛：1 阶竹钓竿钓 5 难度雪莲鱼 → rod_too_low");
    // 雪莲鱼在寒泉洞暗河、冬季、全天；1 阶竿可钓难度 ≤2，5 > 2 → 失败。
    let rod_gate = query_str(
        &mut runtime,
        "(function() local ok, r = require('systems.fishing').cast('cave_river', { force = 'snowlotus' }); \
         return (ok and 'ok' or 'fail') .. '|' .. tostring(r) end)()",
    )?;
    if rod_gate != "fail|rod_too_low" {
        return Err(format!("1 阶竿钓雪莲鱼 = {rod_gate:?}，应为 fail|rod_too_low").into());
    }
    println!("  1 阶竿钓难度 5 雪莲鱼 → rod_too_low ✓");

    banner("⑦ 临时钓点：夏日晴天，雨季涨水潭（需雨天）→ weather_closed");
    lua(
        &mut runtime,
        "clock.set(30, 600); require('world.weather').set('sunny')",
    )?;
    let flood = query_str(
        &mut runtime,
        "(function() local open, r = require('systems.fishing').water_open('flood_pool'); \
         return (open and 'open' or 'closed') .. '|' .. tostring(r) end)()",
    )?;
    if flood != "closed|weather_closed" {
        return Err(format!("晴天涨水潭 = {flood:?}，应为 closed|weather_closed").into());
    }
    println!("  夏日晴天涨水潭未开放（需雨天）✓");

    banner("⑧ on_day_start 挂点：显式调用返回当日开放钓点表（含村溪）");
    let active = query_str(
        &mut runtime,
        "(function() local list = require('systems.fishing').on_day_start(); \
         local has = false; for _, id in ipairs(list) do if id == 'village_brook' then has = true end end \
         return tostring(has and 1 or 0) .. '|' .. tostring(#list) end)()",
    )?;
    if !active.starts_with("1|") {
        return Err(format!("on_day_start 开放钓点表 = {active:?}，应含 village_brook").into());
    }
    println!("  on_day_start 返回开放钓点表，含村溪 ✓");

    banner("⑨ 存档往返：capture → reset → restore，钓获统计 crucian=1 恢复");
    lua(
        &mut runtime,
        "local f = require('systems.fishing'); local c = f.capture(); f.reset(); f.restore(c)",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.fishing').stats()['crucian']",
        1.0,
    )?;
    println!("  capture/restore 往返：钓获 crucian=1 恢复 ✓");

    banner("⑩ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m45_fishing_ok project={} waters=4 fish=23 rods=3 catch=crucian power=100to92 ship=+12 rod_gate=rod_too_low season=season_closed flood=weather_closed save=ok",
        project.display()
    );
    Ok(())
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

fn lua(runtime: &mut LuaRuntime, block: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(block)?;
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

fn assert_num(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_num(runtime, expr)?;
    if (got - expected).abs() > f64::EPSILON {
        return Err(format!("{expr} = {got}, want {expected}").into());
    }
    Ok(())
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
