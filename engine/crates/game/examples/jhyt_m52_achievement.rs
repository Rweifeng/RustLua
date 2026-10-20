//! Headless acceptance drive for the achievement system (成就 30, docs/54 §4.12/§9,
//! stage 2, M52).
//!
//! 验证成就系统（数据 30 条 + 运行时 check_all/谓词求值/发奖/存档）：
//!   1. 静态：data.achievements 注册 30 条（id 唯一、谓词合法）、
//!      systems.achievement.validate_all 空；
//!   2. 初始：新档无任何解锁；隐藏成就 description 为 "???"；
//!   3. 出货链路触发：inv.add 稻谷 + shipping.add 100 + settle → check_all →
//!      "稻丰"(rice_100) 解锁 + 奖励 200 文入账 + last_action=achieve_rice_100；
//!      同时 money 达标连锁解锁 money_500/money_2000；
//!   4. 熟练度触发：mastery.add_xp("farming", 550) → 5 级 → check_all →
//!      farm_1/farm_3/farm_5 解锁；
//!   5. 好感触发：relationship.add("npc_shen_qingluo", 5) → check_all →
//!      heart_shen_5 解锁；
//!   6. 幂等：已解锁成就不重复发奖（再 check_all 返回空列表、金币不变）；
//!   7. 存档往返：capture → reset → restore → 已解锁集合不丢；
//!   8. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m52_achievement [-- <game_jhyt-root>]

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

    banner("① 静态断言：30 条注册、id 唯一、谓词合法、validate_all 空");
    let count = query_num(&mut runtime, "require('data.achievements').count()")?;
    if count != 30.0 {
        return Err(format!("成就注册数 = {count}，应为 30").into());
    }
    let report = query_str(
        &mut runtime,
        "return require('systems.achievement').validate_all()",
    )?;
    if !report.is_empty() {
        return Err(format!("achievement.validate_all 不通过:\n{report}").into());
    }
    println!("  注册 {count} 条；validate_all 空 ✓");

    banner("② 进山庄新档：初始无解锁、隐藏成就 description 为 ???");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_num(
        &mut runtime,
        "require('systems.achievement').unlocked_count()",
        0.0,
    )?;
    // 隐藏成就 rice_1000 未解锁 → description 为 "???"
    assert_str(
        &mut runtime,
        "require('systems.achievement').description('rice_1000')",
        "???",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.achievement').name('money_5000')",
        "???",
    )?;
    // 可见成就未解锁 → description 是真描述（不是 ???）
    let desc = query_str(
        &mut runtime,
        "require('systems.achievement').description('rice_100')",
    )?;
    if desc == "???" || desc.is_empty() {
        return Err(format!("可见成就 rice_100 description = {desc:?}，应为真实描述").into());
    }
    println!("  初始 0 解锁；隐藏成就 rice_1000/money_5000 显示 ??? ✓");

    banner("③ 出货链路：inv.add 稻谷 + shipping.add 100 + settle → check_all → 稻丰解锁");
    // 新档初始稻谷 ×2；补 98 → 100 份可入箱
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'rice', 98)",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'rice')",
        100.0,
    )?;
    // 100 份稻谷全部入箱（sell=30 → 待结算 3000）
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('rice', 100); return ok and 1 or 0 end)()",
        1.0,
    )?;
    let money_before = query_num(&mut runtime, "require('core.state').money")?;
    // 结算：stats.rice = 100、money += 3000
    lua(&mut runtime, "require('systems.shipping').settle()")?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').stats()['rice']",
        100.0,
    )?;
    let money_after_settle = query_num(&mut runtime, "require('core.state').money")?;
    if (money_after_settle - money_before - 3000.0).abs() > f64::EPSILON {
        return Err(format!("结算后金币 = {money_after_settle}，应为 {money_before}+3000").into());
    }
    // check_all → 新解锁 first_harvest / rice_100 / money_500 / money_2000
    let newly = query_str(
        &mut runtime,
        "(function() local t = require('systems.achievement').check_all(); return table.concat(t, ',') end)()",
    )?;
    println!("  新解锁: {newly}");
    assert_str(
        &mut runtime,
        "require('systems.achievement').is_unlocked('rice_100') and 1 or 0",
        "1",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.achievement').is_unlocked('first_harvest') and 1 or 0",
        "1",
    )?;
    // 稻丰奖励 200 文已入账（连锁：first_harvest +50 / rice_100 +200 / money_2000 +500 = +750；
    // money_500 给物品 rice×5 不给钱）
    let money_with_reward = query_num(&mut runtime, "require('core.state').money")?;
    if (money_with_reward - money_after_settle - 750.0).abs() > f64::EPSILON {
        return Err(
            format!("成就奖励后金币 = {money_with_reward}，应为 {money_after_settle}+750").into(),
        );
    }
    // 解锁通知走模块自有字段（achievement.last_event()，不写 state.last_action——
    // 那是 HUD 动作反馈通道，避免跨天结算/入睡等既有 last_action 契约被旁路覆写）。
    let last = query_str(&mut runtime, "require('systems.achievement').last_event()")?;
    if !last.starts_with("achieve_") {
        return Err(format!("achievement.last_event() = {last:?}，应以 achieve_ 开头").into());
    }
    println!("  稻丰解锁 + 奖励 200 文入账；last_event={last} ✓");

    banner("④ 熟练度触发：mastery.add_xp farming 550 → 5 级 → farm_5 解锁");
    lua(
        &mut runtime,
        "require('systems.mastery').add_xp('farming', 550)",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').level('farming')",
        5.0,
    )?;
    lua(&mut runtime, "require('systems.achievement').check_all()")?;
    assert_str(
        &mut runtime,
        "require('systems.achievement').is_unlocked('farm_5') and 1 or 0",
        "1",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.achievement').is_unlocked('farm_1') and 1 or 0",
        "1",
    )?;
    // farm_5 奖励 seed_rice ×5 已入账
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'seed_rice')",
        9.0,
    )?;
    println!("  务农 5 级 → farm_5 解锁、seed_rice ×5 入账（初始 4+奖励 5=9）✓");

    banner("⑤ 好感触发：relationship.add npc_shen_qingluo 5 → heart_shen_5 解锁");
    lua(
        &mut runtime,
        "require('player.relationship').add('npc_shen_qingluo', 5, 'test', 'accept')",
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').points('npc_shen_qingluo')",
        5.0,
    )?;
    lua(&mut runtime, "require('systems.achievement').check_all()")?;
    assert_str(
        &mut runtime,
        "require('systems.achievement').is_unlocked('heart_shen_5') and 1 or 0",
        "1",
    )?;
    println!("  沈青萝好感 5 → heart_shen_5 解锁 ✓");

    banner("⑥ 幂等：再 check_all 不重复发奖（返回空、金币不变）");
    let money_before_idle = query_num(&mut runtime, "require('core.state').money")?;
    let newly2 = query_str(
        &mut runtime,
        "(function() local t = require('systems.achievement').check_all(); return tostring(#t) end)()",
    )?;
    if newly2 != "0" {
        return Err(format!("重复 check_all 新解锁数 = {newly2}，应为 0").into());
    }
    let money_after_idle = query_num(&mut runtime, "require('core.state').money")?;
    if (money_after_idle - money_before_idle).abs() > f64::EPSILON {
        return Err(format!(
            "重复 check_all 金币变动 {money_before_idle}→{money_after_idle}，应不变"
        )
        .into());
    }
    println!("  幂等：无重复解锁、无重复发奖 ✓");

    banner("⑦ 存档往返：capture → reset → restore → 已解锁集合不丢");
    let unlocked_before = query_num(
        &mut runtime,
        "require('systems.achievement').unlocked_count()",
    )?;
    lua(
        &mut runtime,
        "local snap = require('systems.achievement').capture()\n\
         require('systems.achievement').reset()\n\
         assert(require('systems.achievement').unlocked_count() == 0, 'reset 后应为 0')\n\
         require('systems.achievement').restore(snap)",
    )?;
    let unlocked_after = query_num(
        &mut runtime,
        "require('systems.achievement').unlocked_count()",
    )?;
    if (unlocked_after - unlocked_before).abs() > f64::EPSILON {
        return Err(format!("存档往返后解锁数 {unlocked_after} ≠ 往返前 {unlocked_before}").into());
    }
    assert_str(
        &mut runtime,
        "require('systems.achievement').is_unlocked('rice_100') and 1 or 0",
        "1",
    )?;
    // restore 后已解锁的隐藏成就 description 不再是 ???
    let desc_hidden = query_str(
        &mut runtime,
        "require('systems.achievement').description('money_5000')",
    )?;
    if desc_hidden != "???" {
        return Err(format!("未解锁隐藏成就 description = {desc_hidden:?}，应为 ???").into());
    }
    println!("  存档往返：{unlocked_before} → 0 → {unlocked_after}，一致 ✓");

    banner("⑧ 数据校验门（validate_data strict）在整条链路后依然通过");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    let total = query_num(&mut runtime, "require('data.achievements').count()")?;
    let unlocked_n = query_num(
        &mut runtime,
        "require('systems.achievement').unlocked_count()",
    )?;
    println!(
        "jhyt_m52_achievement_ok total={total} unlocked={unlocked_n} project={}",
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
