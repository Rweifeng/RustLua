//! Headless acceptance drive for the meridian cultivation system (经脉修炼,
//! docs/33 §7.3, 改造计划 docs/54 §7.1, M53).
//!
//! 验证经脉修炼真实链路（数据口径 = docs/33 §7.3 == data/meridian.lua ==
//! systems/meridian.lua）：
//!   1. 静态：恰 12 条经脉、根链合法（ren 无根前置、du 前置 ren）、
//!      内力消耗/成功率派生正确；meridian.validate_all 通过；
//!   2. 新档基线：内力 50、加成全 0、体力/内力上限 100；
//!   3. 拒绝路径：内力不足（=5）拒绝冲任脉——不扣内力、不写 last_action；
//!      前置未通（du/xinshaoyin）拒绝——不扣不写；
//!   4. 冲通任脉（确定性掷骰成功）：内力 50→40、灵芝 3→2、last_action=meridian_ren、
//!      被动 energy_max+10 → max_energy=110；
//!   5. 冲通手太阴肺经（defense+1）：经 stats 生效——
//!      stats.query("player","defense") = 基础 1 + 经脉常驻 1 = 2；
//!   6. 失败路径（强制掷骰失败）：内力照扣 20→5、照扣人参、经脉不通、
//!      last_action=meridian_fail_du（失败耗内力不涨进度）；
//!   7. on_day_start 钩子显式直调：内力自然恢复 +30（5→35 钳上限）、
//!      stats 常驻 Buff 重挂（defense 仍为 2）；
//!   8. 冲通督脉（attack+1）：stats.query("player","attack") = 基础 2 + 1 = 3；
//!   9. 存档往返：capture → reset → restore 恢复已通三脉与聚合加成；
//!  10. 模块 validate_all 空、数据校验门（validate_data strict）依然通过。
//!
//! 注：main.lua 的 save 注册 / new_game reset / on_day_start 接线由补丁文本交付
//! （Organizer 集成），本样例对 on_day_start 类钩子显式直调断言，不依赖接线落地。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m53_meridian [-- <game_jhyt-root>]

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

    banner("① 静态断言：12 经脉注册、前置链合法、派生消耗/成功率、validate_all 通过");
    let meta = query_str(
        &mut runtime,
        "local d = require('data.meridian')\n\
         local n = 0\n\
         for _ in pairs(d.meridians) do n = n + 1 end\n\
         local m = require('systems.meridian')\n\
         local ren = m.info('ren')\n\
         local du = m.info('du')\n\
         local pipi = m.info('pipi')\n\
         return tostring(n)..'|'..tostring(ren.unlock_condition)..'|'..tostring(du.unlock_condition)..'|'..tostring(ren.inner_cost)..'|'..tostring(ren.success_rate)..'|'..tostring(pipi.inner_cost)",
    )?;
    if meta != "12|nil|ren|10|0.9|65" {
        return Err(format!("经脉注册表 = {meta:?}，应为 12|nil|ren|10|0.9|65").into());
    }
    let report = query_str(
        &mut runtime,
        "return require('systems.meridian').validate_all()",
    )?;
    if !report.is_empty() {
        return Err(format!("meridian.validate_all 不通过:\n{report}").into());
    }
    println!(
        "  12 经脉、任脉无根前置/督脉前置任脉、任脉耗内 10 成率 0.9 / 末脉 65、validate_all 空 ✓"
    );

    banner("② 进山庄新档：基线内力 50、加成全 0、上限 100/100");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_num(&mut runtime, "require('core.state').inner_power", 50.0)?;
    assert_num(
        &mut runtime,
        "require('systems.meridian').max_energy()",
        100.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.meridian').max_inner_power()",
        100.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','defense')",
        1.0,
    )?;
    assert_str(&mut runtime, "require('core.state').last_action", "none")?;
    println!("  基线：内力 50、体力/内力上限 100、防御 1、last_action=none ✓");

    banner("③ 拒绝路径：内力不足（=5）拒冲任脉——不扣内力、不写 last_action");
    lua(&mut runtime, "require('core.state').inner_power = 5")?;
    let rejected = query_str(
        &mut runtime,
        "(function() local ok, reason = require('systems.meridian').try_open('ren'); return tostring(ok)..'|'..tostring(reason) end)()",
    )?;
    if rejected != "false|not_enough_inner" {
        return Err(format!("内力不足应拒绝，实际 {rejected:?}").into());
    }
    assert_num(&mut runtime, "require('core.state').inner_power", 5.0)?;
    assert_str(&mut runtime, "require('core.state').last_action", "none")?;
    // 前置未通也拒绝（任脉尚未冲通）：du / xinshaoyin 均有前置
    let prereq = query_str(
        &mut runtime,
        "(function() local ok, reason = require('systems.meridian').try_open('du'); return tostring(ok)..'|'..tostring(reason) end)()",
    )?;
    if prereq != "false|prereq" {
        return Err(format!("前置未通应拒绝，实际 {prereq:?}").into());
    }
    assert_str(&mut runtime, "require('core.state').last_action", "none")?;
    println!("  内力不足/前置未通均拒绝，内力与 last_action 原样保留 ✓");

    banner("④ 冲通任脉：内力 50→40、灵芝 3→2、energy_max+10 → 上限 110");
    lua(&mut runtime, "require('core.state').inner_power = 50")?;
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'lingzhi', 3)",
    )?;
    lua(&mut runtime, "require('systems.meridian').set_seed(0)")?; // 首骰≈5.7e-6 < 0.90 必成
    let opened = query_str(
        &mut runtime,
        "(function() local ok, out = require('systems.meridian').try_open('ren'); return tostring(ok)..'|'..tostring(out) end)()",
    )?;
    if opened != "true|opened" {
        return Err(format!("冲任脉应成功，实际 {opened:?}").into());
    }
    assert_num(&mut runtime, "require('core.state').inner_power", 40.0)?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'lingzhi')",
        2.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "meridian_ren",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.meridian').bonus().energy_max",
        10.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.meridian').max_energy()",
        110.0,
    )?;
    println!("  任脉冲通，energy_max+10，体力上限 110 ✓");

    banner("⑤ 冲通手太阴肺经（defense+1）：经 stats 生效——防御 1→2");
    // 门槛：肺经难度 3 → 武学熟练度 ≥2 级（阈值 120 XP）；任脉难度 1 门槛 0。
    lua(
        &mut runtime,
        "require('systems.mastery').add_xp('combat', 120)",
    )?;
    lua(&mut runtime, "require('systems.meridian').set_seed(0)")?;
    let lung = query_str(
        &mut runtime,
        "(function() local ok, out = require('systems.meridian').try_open('feishouyin'); return tostring(ok)..'|'..tostring(out) end)()",
    )?;
    if lung != "true|opened" {
        return Err(format!("冲肺经应成功，实际 {lung:?}").into());
    }
    assert_num(&mut runtime, "require('core.state').inner_power", 20.0)?; // 40-20
    assert_num(
        &mut runtime,
        "require('systems.meridian').bonus().defense",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','defense')",
        2.0,
    )?;
    println!("  肺经 defense+1 经 stats 常驻：query defense=2（基础 1+经脉 1）✓");

    banner("⑥ 失败路径（强制掷骰失败）：冲督脉耗内 15→5、耗人参、不通、写 fail 动作");
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'ginseng', 1)",
    )?;
    // 搜一个让首骰 ≥ 0.85*MOD 的种子（纯数学，不掷骰），再 set_seed 强制失败。
    let forced = query_str(
        &mut runtime,
        "(function()\n\
         local MOD = 2147483648\n\
         local threshold = math.floor(0.85 * MOD)\n\
         for s = 0, 999999 do\n\
           local u = (s * 1103515245 + 12345) % MOD\n\
           if u >= threshold then return tostring(s) end\n\
         end\n\
         return 'none'\n\
         end)()",
    )?;
    if forced == "none" {
        return Err("未找到强制失败种子".into());
    }
    lua(
        &mut runtime,
        &format!("require('systems.meridian').set_seed({forced})"),
    )?;
    let failed = query_str(
        &mut runtime,
        "(function() local ok, out = require('systems.meridian').try_open('du'); return tostring(ok)..'|'..tostring(out) end)()",
    )?;
    if failed != "true|failed" {
        return Err(format!("应掷骰失败，实际 {failed:?}").into());
    }
    assert_num(&mut runtime, "require('core.state').inner_power", 5.0)?; // 20-15
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'ginseng')",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.meridian').is_open('du') and 1 or 0",
        0.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "meridian_fail_du",
    )?;
    println!("  失败耗内 15（20→5）耗人参、督脉不通、last_action=meridian_fail_du ✓");

    banner("⑦ on_day_start 钩子显式直调：内力自然恢复 +30、常驻 Buff 重挂");
    let recovered = query_num(&mut runtime, "require('systems.meridian').on_day_start()")?;
    if (recovered - 30.0).abs() > f64::EPSILON {
        return Err(format!("内力日恢复 = {recovered}，应为 30").into());
    }
    assert_num(&mut runtime, "require('core.state').inner_power", 35.0)?; // 5+30
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','defense')",
        2.0,
    )?;
    println!("  跨天内力 5→35（+30 钳上限）、肺经常驻 defense 重挂仍为 2 ✓");

    banner("⑧ 冲通督脉（attack+1）：经 stats 生效——攻击 2→3");
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'ginseng', 1)",
    )?;
    lua(&mut runtime, "require('systems.meridian').set_seed(0)")?;
    let du = query_str(
        &mut runtime,
        "(function() local ok, out = require('systems.meridian').try_open('du'); return tostring(ok)..'|'..tostring(out) end)()",
    )?;
    if du != "true|opened" {
        return Err(format!("冲督脉应成功，实际 {du:?}").into());
    }
    assert_num(
        &mut runtime,
        "require('systems.meridian').bonus().attack",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','attack')",
        3.0,
    )?;
    println!("  督脉 attack+1 经 stats 常驻：query attack=3（基础 2+经脉 1）✓");

    banner("⑨ 存档往返：capture → reset → restore 恢复已通三脉与聚合加成");
    let cap = query_str(
        &mut runtime,
        "local c = require('systems.meridian').capture(); return table.concat(c.opened, ',')",
    )?;
    if cap != "ren,du,feishouyin" {
        return Err(format!("capture opened = {cap:?}，应为 ren,du,feishouyin").into());
    }
    // capture → reset → restore 在同一段 IIFE 内完成（console 多语句须 IIFE、
    // 局部量跨 console 调用不保留），一并返回"重置后清零/恢复后回聚"双锚点。
    let roundtrip = query_str(
        &mut runtime,
        "(function() local m = require('systems.meridian')\n\
         local c = m.capture()\n\
         m.reset()\n\
         local after_reset = m.bonus().attack\n\
         m.restore(c)\n\
         return tostring(after_reset)..'|'..tostring(m.bonus().attack)..'|'..tostring(m.is_open('ren')) end)()",
    )?;
    if roundtrip != "0|1|true" {
        return Err(format!("存档往返 = {roundtrip:?}，应为 0|1|true").into());
    }
    assert_num(
        &mut runtime,
        "require('systems.meridian').bonus().defense",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.meridian').is_open('ren') and 1 or 0",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','defense')",
        2.0,
    )?;
    println!("  capture/restore 往返：任/督/肺三脉与 attack+1/defense+1 恢复 ✓");

    banner("⑩ 模块 validate_all 空 + 数据校验门（strict）整条链路后依然通过");
    let report = query_str(
        &mut runtime,
        "return require('systems.meridian').validate_all()",
    )?;
    if !report.is_empty() {
        return Err(format!("meridian.validate_all 不通过:\n{report}").into());
    }
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m53_meridian_ok project={} meridians=12 root=ren cost=10..65 rate=0.90..0.35 reject=no_penalty open=ren/du/lung passive=atk+1/def+1 fail=costly save=ok",
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
