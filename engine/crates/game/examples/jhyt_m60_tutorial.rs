//! Headless acceptance drive for M60 / 改造计划 docs/54 §10 阶段5「新手引导重做」：
//! 首次种植五步闭环（领种子→翻土→播种→浇水→入睡跨天→收获）。
//!
//! 对齐 docs/54 §5.6「首次种植完成 ≤5 分钟」指标：把首次种植的每一步拆成可推进、
//! 有提示的引导节点。本样例断言三件事：
//!
//!   1. 静态：data/tutorial.lua.validate_all 通过；步骤总数=16；既有链逐字保留——
//!      新档第 0 步仍是 `till`（新段不抢占）；新 6 步按序落在 `craft` 之后、终态
//!      `done`（finish）之前，且 `done` 仍是最后一步（恰好一个 finish）；
//!   2. 新段不抢占：新档 boot→BtnStart 后 tutorial.status().step == "till"；
//!   3. 真实闭环：从新档进入山庄，把教程推进到新段起点（claim_seed），然后**真实**
//!      走完领种→翻土→播种→浇水→入睡跨天→（再浇一晚）→收获——调用真实 farm.act
//!      （翻土/播种/浇水/成熟收获）与真实 day.sleep()（跨天 on_day_start 自动按已
//!      浇水天数成长），教程通过真实 on_action/on_day_start 推进 API 一步步前进，
//!      最终成熟采收得到萝卜、引导收尾发奖（rewarded=true）。
//!
//! 说明：新段挂在既有 9 步链（till…craft）之后。要走到新段必须先走完既有的村/任务/
//! 制作链（含灵芝 7 日成长 + 对话交付），那是既有用例已覆盖的路径；本样例用教程真实
//! 推进 API 把索引直接拨到新段起点（claim_seed），再**真实**驱动新段到收获——既验证
//! 了新段的每一个 match 条件可被真实玩法事件触发，又不重复造既有链的重型驱动。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m60_tutorial [-- <game_jhyt-root>]

use rustlua_scripting::script::LuaRuntime;
use std::{env, path::PathBuf};

/// 耕地区 cell (24,4)（data/farm.lua：field=x24..33，避开温室 x28..33,y8..13）。
/// 玩家站 (24,3) 朝上（facing up → 目标 y+1 = (24,4)）。
const FARM_TARGET: (f64, f64) = (784.0, 112.0);
/// 萝卜 growth_days=2：需在第 1、2 天各浇一次水，跨两天后成熟。
const NOON: i64 = 720;

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

    let mut rt = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())?;
    rt.set_project_root(project.clone());
    rt.set_ui_viewport(1280, 720);
    rt.set_delta(1.0 / 60.0);

    // ---------------- ① 静态断言：数据合法、新段位置正确、既有第 0 步不变 ----------------
    banner("① 静态：tutorial.validate_all 通过、16 步、新段在 craft 之后 done 之前");
    rt.execute_project_script(&main_script)?;
    open_main_menu(&mut rt)?;

    // validate_all 必须通过（含恰好一个 finish 且在末步）。
    let report = query_str(
        &mut rt,
        "(function() local ok, issues = require('data.tutorial').validate_all() \
           return tostring(ok) .. '|' .. table.concat(issues or {}, ';') end)()",
    )?;
    if report != "true|" {
        return Err(format!("tutorial.validate_all 不通过: {report}").into());
    }
    println!("  data.tutorial.validate_all 通过（空 issues）✓");

    // 步骤序列：总数 16；前 9 步既有；10..15 新段；16=done(finish)。
    let seq = query_str(
        &mut rt,
        "(function() local s = require('ui.tutorial').steps() \
           local out = {} \
           for i=1,#s do out[i] = i .. ':' .. s[i].id .. ':' .. tostring(s[i].match and s[i].match.kind or '?') end \
           return #s .. '|' .. table.concat(out, ',') end)()",
    )?;
    let want = "16|1:till:action,2:plant:action_prefix,3:water:action_prefix,4:sleep:day_start,\
5:goto_village:enter_map,6:talk_npc:action,7:accept_quest:quest_accepted,\
8:deliver:quest_delivered,9:craft:action_prefix,10:claim_seed:action_prefix,\
11:till_loop:action,12:plant_loop:action_prefix,13:water_loop:action_prefix,\
14:sleep_loop:day_start,15:harvest_loop:action_prefix,16:done:finish";
    if seq != want {
        return Err(format!("步骤序列不符:\n got={seq}\nwant={want}").into());
    }
    println!(
        "  16 步顺序/条件合法；新段 claim_seed..harvest_loop 在 craft 后、done(finish) 末步 ✓"
    );

    // ---------------- ② 新档不抢占：boot→BtnStart 后第 0 步仍是 till ----------------
    banner("② 新档不抢占：BtnStart 后 tutorial.step == till（新段不在开局抢占）");
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;
    assert_status_step(&mut rt, "till")?;
    assert_num(&mut rt, "require('ui.tutorial').status().index", 0.0)?;
    println!("  新档第 0 步 = till，新段（索引≥9）只有走完既有链才出现 ✓");

    // ---------------- ③ 真实闭环：推进到新段起点，走完领种→…→收获 ----------------
    banner("③ 真实闭环：拨到 claim_seed → 领种/翻土/播种/浇水/跨天/成熟/收获");
    // 拨到新段起点（索引 9 = 第 10 步 claim_seed）。
    lua(
        &mut rt,
        "require('core.state').set_var('tutorial_step', 9, 'test', 'm60_jump_to_loop')",
    )?;
    assert_status_step(&mut rt, "claim_seed")?;

    // 真实购买种子，不能由测试注入库存；不验证步行/UI 或五分钟新手指标。
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'seed_radish')",
        0.0,
    )?;
    lua(&mut rt, "clock.set(1, 720)")?;
    lua(
        &mut rt,
        "assert(require('systems.shop').buy('zhao_stall', 'seed_radish', 2))",
    )?;
    assert_num(&mut rt, "require('core.state').money", 112.0)?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'seed_radish')",
        2.0,
    )?;
    // 选中它（等同背包 choose_seed：写 selected_seed + select_seed_）。
    lua(
        &mut rt,
        "require('core.state').selected_seed = 'seed_radish' \
         require('core.state').last_action = 'select_seed_seed_radish' \
         require('ui.tutorial').on_action(require('core.state').last_action)",
    )?;
    assert_status_step(&mut rt, "till_loop")?;
    println!("  领种完成（select_seed_seed_radish）→ till_loop");

    // 站到耕地上、面朝上，真实翻土（锄头已装备）。
    stand_at(&mut rt, FARM_TARGET)?;
    act_and_pump(&mut rt, "up")?;
    assert_str(&mut rt, "require('core.state').last_action", "till")?;
    assert_status_step(&mut rt, "plant_loop")?;
    println!("  翻土（till）→ plant_loop");

    // 同一块地再按 E → 播种萝卜（selected_seed=seed_radish，field/春季合法）。
    act_and_pump(&mut rt, "up")?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "plant_seed_radish",
    )?;
    assert_status_step(&mut rt, "water_loop")?;
    println!("  播种（plant_seed_radish）→ water_loop");

    // 切出水壶，面向苗浇水（day 1）。
    lua(
        &mut rt,
        "inv.equip(require('core.state').inventory, 'tool', 'watering_can')",
    )?;
    act_and_pump(&mut rt, "up")?;
    assert_str(&mut rt, "require('core.state').last_action", "water_radish")?;
    assert_status_step(&mut rt, "sleep_loop")?;
    println!("  浇水（water_radish，day1）→ sleep_loop");

    // 入睡跨天（on_day_start 自动推进 sleep_loop；萝卜 watered_days 0→1，day2 未熟）。
    lua(&mut rt, &format!("clock.set(1, {NOON})"))?;
    lua(&mut rt, "day.sleep()")?;
    assert_num(&mut rt, "clock.now().day", 2.0)?;
    assert_status_step(&mut rt, "harvest_loop")?;
    println!("  入睡跨天到 day2（watered_days=1，未熟）→ harvest_loop 等待");

    // day2 再浇一次（harvest_loop 只认 harvest_，浇水不推进、不抢跑）。
    act_and_pump(&mut rt, "up")?;
    assert_str(&mut rt, "require('core.state').last_action", "water_radish")?;
    assert_status_step(&mut rt, "harvest_loop")?;
    println!("  day2 再浇水（不推进 harvest_loop，幂等）");

    // 再睡一觉 → day3，watered_days=2 ≥ growth_days=2 → 成熟。
    lua(&mut rt, &format!("clock.set(2, {NOON})"))?;
    lua(&mut rt, "day.sleep()")?;
    assert_num(&mut rt, "clock.now().day", 3.0)?;
    assert_status_step(&mut rt, "harvest_loop")?; // on_day_start 不推进非 day_start 步
    println!("  入睡跨天到 day3（watered_days=2，成熟）");

    // 成熟按 E 采收（先记录 harvest 动作与背包，再 pump 教程——发奖会改写 last_action）。
    lua(&mut rt, "require('systems.farm').act('up')")?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "harvest_radish",
    )?;
    // 收获得到萝卜（品质可能滚成 good/super，统计三种条目合计）。
    let got = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory,'radish') \
         + inv.count(require('core.state').inventory,'radish_good') \
         + inv.count(require('core.state').inventory,'radish_super')",
    )?;
    if got < 1.0 {
        return Err(format!("收获后萝卜计数 = {got}，应 ≥1").into());
    }
    // pump 教程：推进 harvest_loop → 落终态 done(finish) → 一次性发奖。
    lua(
        &mut rt,
        "require('ui.tutorial').on_action(require('core.state').last_action)",
    )?;
    // 引导收尾：索引到末步 total、发奖锚位置位。
    assert_num(&mut rt, "require('ui.tutorial').status().index", 16.0)?;
    assert_num(
        &mut rt,
        "require('ui.tutorial').status().rewarded and 1 or 0",
        1.0,
    )?;
    println!("  成熟收获（harvest_radish），背包萝卜合计 {got}；引导收尾 rewarded ✓");

    // ---------------- ④ 校验门在整条链路后依然绿 ----------------
    banner("④ 数据校验门（validate_data strict）在链路后依然通过");
    let report = query_str(
        &mut rt,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m60_tutorial_ok project={} steps=16 seg=claim_seed..harvest_loop fresh_step0=till \
         loop=real_farm.act harvest=radish day=3 rewarded=1",
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

fn stand_at(runtime: &mut LuaRuntime, cell: (f64, f64)) -> Result<(), Box<dyn std::error::Error>> {
    lua(
        runtime,
        &format!(
            "local pid = require('player.player').ensure() \
             transform.set(pid, {}, {}) \
             require('core.state').facing = 'up'",
            cell.0, cell.1
        ),
    )?;
    Ok(())
}

/// 真实执行一次面向格的农事（farm.act，与 E 键同一入口），再用教程真实推进 API
/// 喂入本次 last_action（input.update 才自动喂；headless 直调 farm.act 需手动 pump）。
fn act_and_pump(runtime: &mut LuaRuntime, facing: &str) -> Result<(), Box<dyn std::error::Error>> {
    lua(
        runtime,
        &format!(
            "require('systems.farm').act('{facing}') \
             require('ui.tutorial').on_action(require('core.state').last_action)"
        ),
    )?;
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
    if (got - expected).abs() > 1e-6 {
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

/// 断言当前教程步骤 id（status().step）。
fn assert_status_step(
    runtime: &mut LuaRuntime,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_str(runtime, "require('ui.tutorial').status().step", expected)
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
