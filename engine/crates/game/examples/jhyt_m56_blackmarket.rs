//! Headless acceptance drive for the black-market choice loop (黑市抉择循环,
//! docs/54 阶段3, M56).
//!
//! 验证黑市循环的真实链路（数据口径 = data/black_market.lua == systems/black_market.lua
//! 营业窗口/买卖/风险/抉择/声望档位/存档）：
//!   1. 静态：black_market.validate_all 通过（数据引用完整）；
//!   2. 营业窗口：白天（minute=600）不营业 closed，深夜（minute=1380, 22:00–24:00）营业；
//!   3. 买入违禁品：强制「绝不被抓」→ 金币扣减、物品入包、favor 上涨、village 声望微罚、
//!      last_action=blackmarket_buy_smuggled_salt；
//!   4. 卖出稀有货：把一株人参卖给黑市 → 金币进账、物品出包；
//!   5. 抉择事件三选：接受得钱(+150)/favor+2/village-3；拒绝无事发生；报官 favor-5/village+6；
//!   6. 风险触发：强制「必被抓」买入 → 金币照扣、货物没收（数量回原价）、favor-5；
//!   7. 黑市声望档位：favor=10 → 熟客档、淬毒暗器解锁；favor=0 → 重新锁定；
//!   8. 存档往返：capture → reset → restore，favor 逐值恢复；
//!   9. 模块 validate_all 空；validate_data strict 门在整条链路后依然通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m56_blackmarket [-- <game_jhyt-root>]

use engine::core::script::LuaRuntime;
use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
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

    banner("① 静态：black_market.validate_all 通过");
    let report = query_str(
        &mut runtime,
        "(function() local ok, v, iss = require('systems.black_market').validate_all(); \
         if not ok then return 'crashed:' .. tostring(v) end \
         if v ~= true then return table.concat(iss or {}, ';') end return '' end)()",
    )?;
    if !report.is_empty() {
        return Err(format!("black_market.validate_all 不通过:\n{report}").into());
    }
    println!("  validate_all 空 ✓");

    banner("② 进山庄新档；白天不营业、深夜营业");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 白天 10:00（minute=600）→ closed。
    lua(&mut runtime, "clock.set(1, 600)")?;
    let day = query_str(
        &mut runtime,
        "(function() local o, r = require('systems.black_market').is_open(); return tostring(o) .. '|' .. tostring(r) end)()",
    )?;
    if day != "false|closed" {
        return Err(format!("白天营业判定 = {day:?}，应为 false|closed").into());
    }
    // 深夜 23:00（minute=1380，落在 1320..1440）→ open。
    lua(&mut runtime, "clock.set(1, 1380)")?;
    let night = query_str(
        &mut runtime,
        "(function() local o, r = require('systems.black_market').is_open(); return tostring(o) .. '|' .. tostring(r) end)()",
    )?;
    if night != "true|ok" {
        return Err(format!("深夜营业判定 = {night:?}，应为 true|ok").into());
    }
    println!("  白天 closed、深夜 open ✓（窗口 1320..1440）");

    banner("③ 买入违禁品（私盐）：金币扣减、入包、favor↑、village 微罚、last_action");
    // 新档基线金币 120；强制绝不被抓，走干净买入路径。
    lua(
        &mut runtime,
        "require('systems.black_market').debug_set_rng(false)",
    )?;
    let money_before = query_num(&mut runtime, "require('core.state').money")?;
    let favor_before = query_num(&mut runtime, "require('systems.black_market').favor()")?;
    let vill_before = query_num(
        &mut runtime,
        "require('player.relationship').reputation('village')",
    )?;
    assert_num(
        &mut runtime,
        "(function() local ok, r = require('systems.black_market').buy('smuggled_salt', 1); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "blackmarket_buy_smuggled_salt",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'smuggled_salt')",
        1.0,
    )?;
    // 私盐黑市价 20：金币 120 → 100。
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_before - 20.0,
    )?;
    // favor +1（favor_gain=1）。
    assert_num(
        &mut runtime,
        "require('systems.black_market').favor()",
        favor_before + 1.0,
    )?;
    // village 声望 -1（成交罚）。
    assert_num(
        &mut runtime,
        "require('player.relationship').reputation('village')",
        vill_before - 1.0,
    )?;
    println!("  买入私盐 -20 文、入包 ×1、favor+1、village-1 ✓");

    banner("④ 卖出稀有货（人参）：金币进账、物品出包");
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'ginseng', 1)",
    )?;
    let money_sell = query_num(&mut runtime, "require('core.state').money")?;
    assert_num(
        &mut runtime,
        "(function() local ok, r = require('systems.black_market').sell('ginseng', 1); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "blackmarket_sell_ginseng",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'ginseng')",
        0.0,
    )?;
    // 人参黑市收购价 120。
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_sell + 120.0,
    )?;
    println!("  卖出人参 +120 文、出包 ✓");

    banner("⑤ 抉择事件三选：接受得钱 / 拒绝无事 / 报官涨声望");
    // 接受：money+150、favor+2、village-3。
    {
        let m0 = query_num(&mut runtime, "require('core.state').money")?;
        let f0 = query_num(&mut runtime, "require('systems.black_market').favor()")?;
        let v0 = query_num(
            &mut runtime,
            "require('player.relationship').reputation('village')",
        )?;
        assert_num(
            &mut runtime,
            "(function() local ok = require('systems.black_market').resolve_event('midnight_herb', 'accept'); return ok and 1 or 0 end)()",
            1.0,
        )?;
        assert_num(&mut runtime, "require('core.state').money", m0 + 150.0)?;
        assert_num(
            &mut runtime,
            "require('systems.black_market').favor()",
            f0 + 2.0,
        )?;
        assert_num(
            &mut runtime,
            "require('player.relationship').reputation('village')",
            v0 - 3.0,
        )?;
        println!("  接受：+150 文、favor+2、village-3 ✓");
    }
    // 拒绝：三项全不变。
    {
        let m0 = query_num(&mut runtime, "require('core.state').money")?;
        let f0 = query_num(&mut runtime, "require('systems.black_market').favor()")?;
        let v0 = query_num(
            &mut runtime,
            "require('player.relationship').reputation('village')",
        )?;
        assert_num(
            &mut runtime,
            "(function() local ok = require('systems.black_market').resolve_event('midnight_herb', 'refuse'); return ok and 1 or 0 end)()",
            1.0,
        )?;
        assert_num(&mut runtime, "require('core.state').money", m0)?;
        assert_num(&mut runtime, "require('systems.black_market').favor()", f0)?;
        assert_num(
            &mut runtime,
            "require('player.relationship').reputation('village')",
            v0,
        )?;
        println!("  拒绝：无事发生 ✓");
    }
    // 报官：favor-5（clamp 0）、village+6、金币不变。
    {
        let m0 = query_num(&mut runtime, "require('core.state').money")?;
        let f0 = query_num(&mut runtime, "require('systems.black_market').favor()")?;
        let v0 = query_num(
            &mut runtime,
            "require('player.relationship').reputation('village')",
        )?;
        assert_num(
            &mut runtime,
            "(function() local ok = require('systems.black_market').resolve_event('midnight_herb', 'report'); return ok and 1 or 0 end)()",
            1.0,
        )?;
        assert_num(&mut runtime, "require('core.state').money", m0)?;
        let expect_f = (f0 - 5.0).max(0.0);
        assert_num(
            &mut runtime,
            "require('systems.black_market').favor()",
            expect_f,
        )?;
        assert_num(
            &mut runtime,
            "require('player.relationship').reputation('village')",
            v0 + 6.0,
        )?;
        println!("  报官：favor-5、village+6 ✓");
    }

    banner("⑥ 风险触发（必被抓）：金币照扣、货物没收、favor 跌落");
    lua(
        &mut runtime,
        "require('systems.black_market').debug_set_rng(true)",
    )?;
    let cnt_before = query_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'smuggled_salt')",
    )?;
    let money_b = query_num(&mut runtime, "require('core.state').money")?;
    let favor_b = query_num(&mut runtime, "require('systems.black_market').favor()")?;
    let (ok_flag, reason) = query_pair(
        &mut runtime,
        "(function() local ok, r = require('systems.black_market').buy('smuggled_salt', 1); return tostring(ok) .. ',' .. tostring(r) end)()",
    )?;
    if ok_flag != "true" || reason != "caught" {
        return Err(format!("必被抓买入应返回 (true, caught)，得到 ({ok_flag}, {reason})").into());
    }
    // 金币照扣 20；货物没收＝数量回买入前（不是 +1）；favor -5。
    assert_num(&mut runtime, "require('core.state').money", money_b - 20.0)?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'smuggled_salt')",
        cnt_before,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.black_market').favor()",
        (favor_b - 5.0).max(0.0),
    )?;
    println!("  被抓：-20 文、货物没收回原价、favor-5 ✓");
    lua(
        &mut runtime,
        "require('systems.black_market').debug_set_rng(false)",
    )?;

    banner("⑦ 黑市声望档位：favor=10 解锁淬毒暗器；favor=0 重新锁定");
    lua(
        &mut runtime,
        "require('systems.black_market').debug_set_favor(10)",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.black_market').favor_tier().key",
        "acquaintance",
    )?;
    assert_str(
        &mut runtime,
        "(function() for _, r in ipairs(require('systems.black_market').stock()) do if r.item == 'poison_dagger' then return tostring(r.locked) end end return 'nf' end)()",
        "false",
    )?;
    lua(
        &mut runtime,
        "require('systems.black_market').debug_set_favor(0)",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.black_market').favor_tier().key",
        "stranger",
    )?;
    assert_str(
        &mut runtime,
        "(function() for _, r in ipairs(require('systems.black_market').stock()) do if r.item == 'poison_dagger' then return tostring(r.locked) end end return 'nf' end)()",
        "true",
    )?;
    println!("  favor=10→熟客解锁、favor=0→生客锁定 ✓");

    banner("⑧ 存档往返：capture → reset → restore，favor 逐值恢复");
    let restored = query_num(
        &mut runtime,
        "(function() local bm = require('systems.black_market'); bm.debug_set_favor(42); \
         local snap = bm.capture(); bm.reset(); bm.restore(snap); return bm.favor() end)()",
    )?;
    if restored != 42.0 {
        return Err(format!("存档往返后 favor = {restored}，应为 42").into());
    }
    println!("  favor 42 往返一致 ✓");

    banner("⑨ 校验门：模块 validate_all 空 + validate_data strict 通过");
    let report = query_str(
        &mut runtime,
        "(function() local ok, v, iss = require('systems.black_market').validate_all(); \
         if not ok then return 'crashed:' .. tostring(v) end \
         if v ~= true then return table.concat(iss or {}, ';') end return '' end)()",
    )?;
    if !report.is_empty() {
        return Err(format!("收尾 validate_all 不通过:\n{report}").into());
    }
    let strict = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !strict.is_empty() {
        return Err(format!("validate_data strict 不通过:\n{strict}").into());
    }

    println!(
        "jhyt_m56_blackmarket_ok project={project} window=1320..1440 buy=salt-20 sell=ginseng+120 event=accept/refuse/report risk=caught tier=acquaintance/save_roundtrip",
        project = project.display()
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

/// 查询形如 "true,caught" 的二元串，拆成 (ok, reason)。
fn query_pair(
    runtime: &mut LuaRuntime,
    block: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let raw = query_str(runtime, block)?;
    let mut parts = raw.splitn(2, ',');
    let a = parts.next().unwrap_or("").to_owned();
    let b = parts.next().unwrap_or("").to_owned();
    Ok((a, b))
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
