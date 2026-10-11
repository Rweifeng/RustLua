//! Headless acceptance drive for the shipping box (镖局收单, 改造计划 §4.2 /
//! docs/33 §4.1.7, M42).
//!
//! 验证出货箱（镖局收单）的真实链路（数据口径 = docs/33 §4.1.7 入箱范围/结算价
//! == systems/shipping.lua 放入/结算/记录/存档）：
//!   1. 静态：货箱格 (15,2)（山庄门口内侧、不与出口格 (16,1) 重合）；
//!      shipping.validate_all 自检通过；
//!   2. 交互：站 (15,3) 朝 up → 面前 (15,2) 按 E → 打开 "Shipping" 面板
//!      （mode=Shipping、ui_active），Esc 关闭复位 Playing；
//!   3. 入箱链路：
//!      - 稻谷（sell=30）放入后背包即时减少、pending 累计、last_action=ship_<id>；
//!      - 灵芝（sell=80）入箱；种子（seed_rice sell=2 > 0，卖价低于买价无套利）可入箱；
//!      - seed_mint（sell=0）与工具拒绝（not_shippable）；超持有拒绝（not_enough）；反悔取出可放回背包；
//!      - 结算前金币不变、pending_total 预演正确（30×2+80+2=142）；
//!   4. 结算：settle 按 sell 价入账（money +142）、累计 stats、当日记录落账
//!      （日期/明细/total）、pending 清空；空箱再结算幂等（返回 nil）；
//!   5. 跨天自动结算：放薄荷（sell=12）入箱 → 睡觉 → on_day_start 自动
//!      结算（money +36、stats.mint=3、log 追加第 2 条）；
//!   6. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m42_shipping_box [-- <game_jhyt-root>]

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

    banner("① 静态断言：货箱格 (15,2) 与出口格 (16,1) 不重合、shipping.validate_all 通过");
    let cell = query_str(
        &mut runtime,
        "local c = require('systems.shipping').crate_cell(); return tostring(c.x) .. ',' .. tostring(c.y)",
    )?;
    if cell != "15,2" {
        return Err(format!("货箱格 = {cell:?}，应为 15,2").into());
    }
    let report = query_str(
        &mut runtime,
        "return require('systems.shipping').validate_all()",
    )?;
    if !report.is_empty() {
        return Err(format!("shipping.validate_all 不通过:\n{report}").into());
    }
    println!("  货箱格 15,2（门口内侧）；validate_all 空 ✓");

    banner("② 进山庄新档，货箱交互：站 (15,3) 朝 down（面前 (15,2)）按 E 打开 Shipping 面板");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    stand_at(&mut runtime, (15, 3))?;
    set_facing(&mut runtime, "down")?;
    press(&mut runtime, KeyCode::KeyE)?;
    let diag = query_str(
        &mut runtime,
        "local it = require('systems.interaction')\n\
         local c = it.query('down')\n\
         local pos = require('player.player').position()\n\
         local fc = grid.facing_cell(pos.x, pos.y, 'down', 32)\n\
         local mode = require('core.state').is_mode('Shipping') and 'Shipping' or require('core.state').mode\n\
         return 'cand=' .. tostring(c and c.id or 'nil') .. '|fc=' .. tostring(fc and (fc[1] .. ',' .. fc[2]) or 'nil') .. '|mode=' .. tostring(mode) .. '|act=' .. tostring(require('core.state').last_action)",
    )?;
    println!("  [diag] {diag}");
    assert_str(
        &mut runtime,
        "require('core.state').is_mode('Shipping') and 'Shipping' or 'other'",
        "Shipping",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').ui_active() and 1 or 0",
        1.0,
    )?;
    println!("  按 E 打开货箱面板（mode=Shipping）；Esc 关闭复位 Playing");
    press(&mut runtime, KeyCode::Escape)?;
    assert_str(
        &mut runtime,
        "require('core.state').is_mode('Playing') and 'Playing' or 'other'",
        "Playing",
    )?;

    banner("③ 入箱：稻谷/灵芝入箱、种子可入箱、sell=0 拒绝、超持有拒绝、反悔可取出");
    // 新档基线：rice×2、seed_rice×4、基础工具 hoe（sell=0）。补灵芝/薄荷籽。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'lingzhi', 1)\n\
         inv.add(require('core.state').inventory, 'seed_mint', 1)",
    )?;
    // 稻谷 ×2 入箱：背包即时减少、pending 累计
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('rice', 2); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "ship_rice",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'rice')",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').pending()['rice']",
        2.0,
    )?;
    // 灵芝 ×1 入箱（sell=80）
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('lingzhi', 1); return ok and 1 or 0 end)()",
        1.0,
    )?;
    // 种子可入箱（seed_rice sell=2 > 0，卖价低于买价无套利）
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('seed_rice', 1); return ok and 1 or 0 end)()",
        1.0,
    )?;
    // sell=0 拒绝：seed_mint 与工具 hoe
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('seed_mint', 1); return ok and 1 or 0 end)()",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('hoe', 1); return ok and 1 or 0 end)()",
        0.0,
    )?;
    // 超持有拒绝：背包稻谷已清空（失败不写 last_action，与商店 sell 语义一致）
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('rice', 1); return ok and 1 or 0 end)()",
        0.0,
    )?;
    // 反悔取出：取出 1 份稻谷回背包
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').remove('rice', 1); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'rice')",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').pending()['rice']",
        1.0,
    )?;
    // 放回，pending 回到 2；待结算金额预演 = 30×2 + 80 + 2 = 142
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('rice', 1); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').pending_total()",
        142.0,
    )?;
    let money_before = query_num(&mut runtime, "require('core.state').money")?;
    println!("  入箱/拒收/反悔链路通过；待结算 142 文，结算前金币 {money_before}");

    banner("④ 睡前统一结算：按 sell 价入账 + 累计出货 + 当日记录 + 幂等");
    let settled = query_str(
        &mut runtime,
        "(function() local s = require('systems.shipping').settle(); \
         if not s then return 'nil' end \
         local items = {} for _, e in ipairs(s.entries) do items[#items + 1] = e.id .. '=' .. e.count .. '=' .. e.amount end \
         return tostring(s.total) .. '|' .. table.concat(items, ',') end)()",
    )?;
    if settled != "142|lingzhi=1=80,rice=2=60,seed_rice=1=2" {
        return Err(format!(
            "结算明细 = {settled:?}，应为 142|lingzhi=1=80,rice=2=60,seed_rice=1=2"
        )
        .into());
    }
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_before + 142.0,
    )?;
    // 累计出货 stats
    assert_num(
        &mut runtime,
        "require('systems.shipping').stats()['rice']",
        2.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').stats()['lingzhi']",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').stats()['seed_rice']",
        1.0,
    )?;
    // 当日记录：1 条、含日期与总额
    assert_num(&mut runtime, "#require('systems.shipping').log()", 1.0)?;
    let entry = query_str(
        &mut runtime,
        "local e = require('systems.shipping').log()[1]; \
         return tostring(e.date) .. '|' .. tostring(e.total) .. '|' .. tostring(#e.items)",
    )?;
    if !entry.starts_with("春 1 日|142|3") {
        return Err(format!("出货记录第 1 条 = {entry:?}，应为 春 1 日|142|3").into());
    }
    // 清空 + 幂等
    assert_num(
        &mut runtime,
        "(function() local p = require('systems.shipping').pending(); local n = 0; for _ in pairs(p) do n = n + 1 end; return n end)()",
        0.0,
    )?;
    assert_str(
        &mut runtime,
        "(function() local s = require('systems.shipping').settle(); return s and 'again' or 'nil' end)()",
        "nil",
    )?;
    println!("  结算入账 +142、stats/log 落账、pending 清空、幂等 ✓");

    banner("⑤ 跨天自动结算：放薄荷入箱 → 睡觉 → on_day_start 自动结算");
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'mint', 3)",
    )?;
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('mint', 3); return ok and 1 or 0 end)()",
        1.0,
    )?;
    let before = query_num(&mut runtime, "require('core.state').money")?;
    lua(&mut runtime, "day.sleep()")?;
    // on_day_start 挂点自动结算：mint sell=12 × 3 = 36
    assert_num(&mut runtime, "require('core.state').money", before + 36.0)?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').stats()['mint']",
        3.0,
    )?;
    assert_num(&mut runtime, "#require('systems.shipping').log()", 2.0)?;
    let entry = query_str(
        &mut runtime,
        "local e = require('systems.shipping').log()[2]; \
         return tostring(e.date) .. '|' .. tostring(e.total) .. '|' .. tostring(#e.items)",
    )?;
    if !entry.starts_with("春 2 日|36|1") {
        return Err(format!("出货记录第 2 条 = {entry:?}，应为 春 2 日|36|1").into());
    }
    println!("  跨天自动结算 +36（薄荷 12×3）、log 追加第 2 条（{entry}）");

    banner("⑥ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m42_shipping_box_ok project={} crate=15,2 open=Shipping box=rice/lingzhi/seed_rice reject=seed_mint/hoe settle=142 power=idempotent auto=+36 log=2",
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

fn press(runtime: &mut LuaRuntime, key: KeyCode) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
}

fn stand_at(runtime: &mut LuaRuntime, cell: (i64, i64)) -> Result<(), Box<dyn std::error::Error>> {
    let (cx, cy) = cell;
    lua(
        runtime,
        &format!(
            "local pid = require('player.player').ensure(); \
             transform.set(pid, {} * 32, {} * 32)",
            (cx as f64) + 0.5,
            (cy as f64) + 0.5
        ),
    )?;
    Ok(())
}

fn set_facing(runtime: &mut LuaRuntime, facing: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('core.state').facing = '{facing}'"))?;
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
