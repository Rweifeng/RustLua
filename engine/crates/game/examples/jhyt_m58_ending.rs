//! Headless acceptance drive for the multi-ending system（多结局，改造计划 §7.6 /
//! docs/33 §8.2，阶段3 第6系统，M58，版本号 2.4）。
//!
//! 多结局是**查询面声明层**：data/endings.lua 纯数据 + systems/endings.lua 把
//! "阵营声望 / 熟练度 / 天数"三类既有可查询量求值成 once 解锁记录。本样例验证：
//!   1. 静态：恰 5 条结局、data/systems validate_all 均空、谓词白名单齐全；
//!   2. 新档基线：一条都没解锁，check_all 返回空；
//!   3. 各结局条件边界（差一点不触发 / 达标触发）：
//!      - 归田 village 声望 19 不触发、20 触发（docs/33 §8.2 熟识档下限）；
//!      - 入仕 sect 声望 59 不触发、60 触发（重视档下限）；
//!      - 神医 制药 XP 549（4 级）不触发、550（5 级）触发；
//!      - 武林 武学 XP 549（4 级）不触发、550（5 级）触发；
//!      - 隐世 第 111 天不触发、第 112 天触发；
//!   4. 幂等：5 条全解锁后再 check_all 不重复解锁；
//!   5. 存档往返：capture → reset(0) → restore(5)，id 一一回来；
//!   6. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! 说明：本系统尚未接进 main.lua 的 save_providers（接线交补丁文本），故存档往返直接
//! 走模块自己的 capture/restore 契约（save_manager 注册后即同一对函数）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m58_ending [-- <game_jhyt-root>]

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

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())?;
    runtime.set_project_root(project.clone());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    runtime.execute_project_script(&main_script)?;

    banner("① 静态：恰 5 条结局、data/systems validate_all 空、谓词白名单齐全");
    assert_num(&mut runtime, "#require('data.endings').endings", 5.0)?;
    assert_str(&mut runtime, "require('data.endings').validate_all()", "")?;
    assert_str(
        &mut runtime,
        "require('systems.endings').validate_all()",
        "",
    )?;
    // 谓词白名单 = reputation / mastery_level / day（ systems 求值表须一一对应）。
    assert_str(
        &mut runtime,
        "tostring(require('data.endings').predicate_types.reputation)",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('data.endings').predicate_types.mastery_level)",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('data.endings').predicate_types.day)",
        "true",
    )?;
    println!("  5 条结局；data/systems validate_all 空；谓词白名单 reputation/mastery_level/day ✓");

    banner("② 进山庄新档：基线一条都没解锁，check_all 返回空");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_num(
        &mut runtime,
        "require('systems.endings').unlocked_count()",
        0.0,
    )?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "",
    )?;
    println!("  新档基线：unlocked_count=0，check_all 空 ✓");

    banner("③ 归田：village 声望 19 不触发、20 触发（熟识档下限）");
    lua(
        &mut runtime,
        "require('player.relationship').add_reputation('village', 19, 'test', 'bound')",
    )?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.endings').is_unlocked('ending_field') and 'true' or 'false'",
        "false",
    )?;
    lua(
        &mut runtime,
        "require('player.relationship').add_reputation('village', 1, 'test', 'bound')",
    )?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "ending_field",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.endings').is_unlocked('ending_field') and 'true' or 'false'",
        "true",
    )?;
    println!("  village 19 挡住、20 放行 → ending_field ✓");

    banner("③ 入仕：sect 声望 59 不触发、60 触发（重视档下限）");
    lua(
        &mut runtime,
        "require('player.relationship').add_reputation('sect', 59, 'test', 'bound')",
    )?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.endings').is_unlocked('ending_sect') and 'true' or 'false'",
        "false",
    )?;
    lua(
        &mut runtime,
        "require('player.relationship').add_reputation('sect', 1, 'test', 'bound')",
    )?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "ending_sect",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.endings').is_unlocked('ending_sect') and 'true' or 'false'",
        "true",
    )?;
    println!("  sect 59 挡住、60 放行 → ending_sect ✓");

    banner("③ 神医：制药 XP 549（4 级）不触发、550（5 级）触发");
    lua(
        &mut runtime,
        "require('systems.mastery').add_xp('alchemy', 549)",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').level('alchemy')",
        4.0,
    )?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.endings').is_unlocked('ending_healer') and 'true' or 'false'",
        "false",
    )?;
    lua(
        &mut runtime,
        "require('systems.mastery').add_xp('alchemy', 1)",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').level('alchemy')",
        5.0,
    )?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "ending_healer",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.endings').is_unlocked('ending_healer') and 'true' or 'false'",
        "true",
    )?;
    println!("  制药 549=4 级挡住、550=5 级放行 → ending_healer ✓");

    banner("③ 武林：武学 XP 549（4 级）不触发、550（5 级）触发");
    lua(
        &mut runtime,
        "require('systems.mastery').add_xp('combat', 549)",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').level('combat')",
        4.0,
    )?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.endings').is_unlocked('ending_martial') and 'true' or 'false'",
        "false",
    )?;
    lua(
        &mut runtime,
        "require('systems.mastery').add_xp('combat', 1)",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').level('combat')",
        5.0,
    )?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "ending_martial",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.endings').is_unlocked('ending_martial') and 'true' or 'false'",
        "true",
    )?;
    println!("  武学 549=4 级挡住、550=5 级放行 → ending_martial ✓");

    banner("③ 隐世：第 111 天不触发、第 112 天触发");
    lua(&mut runtime, "clock.set(111, 600)")?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.endings').is_unlocked('ending_hermit') and 'true' or 'false'",
        "false",
    )?;
    lua(&mut runtime, "clock.set(112, 600)")?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "ending_hermit",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.endings').is_unlocked('ending_hermit') and 'true' or 'false'",
        "true",
    )?;
    println!("  第 111 天挡住、第 112 天放行 → ending_hermit ✓");

    banner("④ 幂等：5 条全解锁后再 check_all 不重复解锁");
    assert_num(
        &mut runtime,
        "require('systems.endings').unlocked_count()",
        5.0,
    )?;
    assert_str(
        &mut runtime,
        "(function() return table.concat(require('systems.endings').check_all(), ',') end)()",
        "",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.endings').unlocked_count()",
        5.0,
    )?;
    println!("  重复 check_all 空、仍 5 条 ✓");

    banner("⑤ 存档往返：capture → reset(0) → restore(5)，id 一一回来");
    let roundtrip = query_str(
        &mut runtime,
        "(function() local E = require('systems.endings'); \
           local s = E.capture(); E.reset(); local after_reset = E.unlocked_count(); \
           E.restore(s); \
           return after_reset .. ',' .. E.unlocked_count() \
             .. ',' .. tostring(E.is_unlocked('ending_field')) \
             .. ',' .. tostring(E.is_unlocked('ending_sect')) \
             .. ',' .. tostring(E.is_unlocked('ending_hermit')) end)()",
    )?;
    if roundtrip != "0,5,true,true,true" {
        return Err(
            format!("capture/restore 往返 = {roundtrip:?}，应为 0,5,true,true,true").into(),
        );
    }
    println!("  capture→reset(0)→restore(5)：field/sect/hermit 均回来 ✓");

    banner("⑥ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m58_ending_ok project={} endings=5 boundaries=19/20|59/60|549/550xp|111/112d idempotent=5 capture_restore=5 strict=green",
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
