//! Headless acceptance drive for the 3-layer quest board (任务板 3 层, docs/54 §4.7 /
//! docs/33 §4.1.8, 2026-10-06 1.6).
//!
//! 任务板是独立闭环的新系统（不接入 world/quest.lua 五态机）：
//!   1. 静态：systems.questboard.validate_all() 空；22 个模板只引用既有物品/阵营；
//!   2. day 1 新档：显式 on_day_start 后日常层挂 3 单（数量与模板合法），
//!      weekly 第 1 天不挂（首个锚点 day 7）、quarterly 季初 day 1 挂 1 单；
//!   3. 接单 → 备齐交付物（inv.add 目标物品）→ 交单拿奖励（金币/声望入账、
//!      last_action=questboard_deliver）；交付前物品不足时拒绝且不写 last_action；
//!   4. 跨天刷新（day.sleep 后显式 on_day_start）：日常层换新 3 单、已交单不残留；
//!   5. 周/季层刷新锚点：clock.set 到第 7 天 weekly 出现 2 单、第 29 天 quarterly 出现 1 单；
//!   6. 存档往返（capture → restore 板子不丢）；validate_data strict 仍通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m50_questboard [-- <game_jhyt-root>]

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

    banner("① 静态断言：questboard.validate_all 空、模板池合法");
    let report = query_str(
        &mut runtime,
        "(function() local ok, issues = require('systems.questboard').validate_all(); \
         if ok then return '' end return table.concat(issues, '|') end)()",
    )?;
    if !report.is_empty() {
        return Err(format!("questboard.validate_all 不通过:\n{report}").into());
    }
    let template_count = query_num(
        &mut runtime,
        "(function() return #require('data.questboard').templates end)()",
    )?;
    println!("  validate_all 空 ✓；模板池 {} 条（≥20）", template_count);

    banner("② 新档 day 1：显式 on_day_start 后日常层挂 3 单、weekly 未到、quarterly 季初 1 单");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 新档尚未接线 main.lua 的挂点（补丁由 Organizer 集成），这里显式调用 on_day_start 播种 day 1。
    lua(&mut runtime, "require('systems.questboard').on_day_start()")?;
    assert_num(
        &mut runtime,
        "require('systems.questboard').count('daily')",
        3.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.questboard').count('weekly')",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.questboard').count('quarterly')",
        1.0,
    )?;
    println!("  daily=3 / weekly=0（首个锚点 day7）/ quarterly=1（季初 day1）✓");

    banner("③ 接单 → 物品不足拒绝 → 备齐交付 → 交单拿奖励（金币/声望/last_action）");
    // 取日常层第 1 单：item|count|id|tpl
    let offer1 = query_str(
        &mut runtime,
        "(function() local o = require('systems.questboard').offers('daily')[1]; \
         return o.item .. '|' .. tostring(o.count) .. '|' .. o.id .. '|' .. o.tpl end)()",
    )?;
    let parts: Vec<&str> = offer1.split('|').collect();
    let target_item = parts[0];
    let target_count: i64 = parts[1].parse()?;
    let old_offer_id = parts[2];
    let tpl_id = parts[3];
    // 该模板的奖励金币与声望（从数据直读，断言发放金额逐格对得上）。
    let reward_money = query_num(
        &mut runtime,
        &format!(
            "require('data.questboard').template('{}').reward.money",
            tpl_id
        ),
    )?;
    let reward_rep = query_num(
        &mut runtime,
        &format!(
            "require('data.questboard').template('{}').reward.rep.amount",
            tpl_id
        ),
    )?;
    println!(
        "  接到 {tpl_id}：交 {target_item}×{target_count}，奖励 {reward_money} 文 + 声望 +{reward_rep}"
    );

    // 接单成功，写 last_action=questboard_accept。
    assert_str(
        &mut runtime,
        "(function() local ok, reason = require('systems.questboard').accept('daily', 1); return ok and 'ok' or reason end)()",
        "ok",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "questboard_accept",
    )?;
    // 物品不足：交付拒绝（objectives_unmet）且**不写** last_action（失败动作同商店语义）。
    assert_str(
        &mut runtime,
        "(function() local ok, reason = require('systems.questboard').deliver('daily', 1); return ok and 'ok' or reason end)()",
        "objectives_unmet",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "questboard_accept",
    )?;

    // 备齐交付物。
    lua(
        &mut runtime,
        &format!(
            "inv.add(require('core.state').inventory, '{}', {})",
            target_item, target_count
        ),
    )?;
    let money_before = query_num(&mut runtime, "require('core.state').money")?;
    let rep_before = query_num(
        &mut runtime,
        "require('player.relationship').reputation('village')",
    )?;
    // 交单成功：金币 +reward_money、声望 +reward_rep、last_action=questboard_deliver。
    assert_str(
        &mut runtime,
        "(function() local ok, reason = require('systems.questboard').deliver('daily', 1); return ok and 'ok' or reason end)()",
        "ok",
    )?;
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_before + reward_money,
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').reputation('village')",
        rep_before + reward_rep,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "questboard_deliver",
    )?;
    assert_num(
        &mut runtime,
        "(function() return require('systems.questboard').offers('daily')[1].done and 1 or 0 end)()",
        1.0,
    )?;
    println!("  交单入账 +{reward_money} 文、声望 +{reward_rep}、done 落定 ✓");

    banner("④ 跨天刷新：day.sleep → 显式 on_day_start，日常层换新 3 单、已交单不残留");
    lua(&mut runtime, "day.sleep()")?;
    // 真实 main.on_day_start 不会调 questboard（未接线），这里显式补挂点断言。
    lua(&mut runtime, "require('systems.questboard').on_day_start()")?;
    assert_num(
        &mut runtime,
        "require('systems.questboard').count('daily')",
        3.0,
    )?;
    let linger = query_str(
        &mut runtime,
        &format!(
            "(function() local old = '{}'; \
             for _, o in ipairs(require('systems.questboard').offers('daily')) do if o.id == old then return 'lingers' end end \
             return 'cleared' end)()",
            old_offer_id
        ),
    )?;
    if linger != "cleared" {
        return Err(format!("已交单 {old_offer_id} 跨天后仍残留").into());
    }
    println!("  day2 日常层 3 单全换、旧单 {old_offer_id} 不残留 ✓");

    banner("⑤ 周/季层刷新锚点：clock.set 到第 7 天 weekly 出现、第 29 天 quarterly 出现");
    lua(&mut runtime, "clock.set(7, 0)")?;
    lua(&mut runtime, "require('systems.questboard').on_day_start()")?;
    assert_num(
        &mut runtime,
        "require('systems.questboard').count('weekly')",
        2.0,
    )?;
    let weekly1 = query_str(
        &mut runtime,
        "(function() local o = require('systems.questboard').offers('weekly')[1]; return o.title end)()",
    )?;
    println!("  day7 weekly 挂 2 单（首张：{weekly1}）✓");

    lua(&mut runtime, "clock.set(29, 0)")?;
    lua(&mut runtime, "require('systems.questboard').on_day_start()")?;
    assert_num(
        &mut runtime,
        "require('systems.questboard').count('quarterly')",
        1.0,
    )?;
    let q_title = query_str(
        &mut runtime,
        "(function() local o = require('systems.questboard').offers('quarterly')[1]; return o.title end)()",
    )?;
    println!("  day29 quarterly 挂 1 单（{q_title}）✓");

    banner("⑥ 存档往返：capture → restore 板子不丢；validate_all 空 + validate_data strict 通过");
    let roundtrip = query_str(
        &mut runtime,
        "(function() local qb = require('systems.questboard'); \
         local before = #qb.offers('daily'); local snap = qb.capture(); \
         qb.reset(); local after_reset = #qb.offers('daily'); \
         qb.restore(snap); local after = #qb.offers('daily'); \
         return tostring(before) .. '|' .. tostring(after_reset) .. '|' .. tostring(after) end)()",
    )?;
    // before=当日3单 → reset 后 0 → restore 回 3。
    if roundtrip != "3|0|3" {
        return Err(format!("任务板存档往返 = {roundtrip:?}，应为 3|0|3").into());
    }
    let report2 = query_str(
        &mut runtime,
        "(function() local ok, issues = require('systems.questboard').validate_all(); \
         if ok then return '' end return table.concat(issues, '|') end)()",
    )?;
    if !report2.is_empty() {
        return Err(format!("链路后 questboard.validate_all 不通过:\n{report2}").into());
    }
    let report3 = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report3.is_empty() {
        return Err(format!("validate_data strict 不通过:\n{report3}").into());
    }

    println!(
        "jhyt_m50_questboard_ok project={} daily=3 weekly@day7=2 quarterly@day29=1 accept=questboard_accept deliver=questboard_deliver rep=village roundtrip=3|0|3",
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
