//! Headless acceptance drive for the Azure Cloud Mine Vein (青云矿脉 30 层, 阶段2 /
//! docs/33 §4.6 / docs/54 §4.5, M46).
//!
//! 本阶段矿脉是纯数据 + 系统（headless 直接驱动）；矿洞场景与入口接线属阶段4。
//! 验证真实链路（数据口径 = data/mine.lua == systems/mine.lua 动作）：
//!   1. 静态：data/mine.lua 恰 30 层、电梯层落在 5/10/15/20/25/30、
//!      systems/mine.validate_all 空、入口层 1 恒解锁；
//!   2. descend：新档站 1 层；跳层 descend(3) 拒绝 not_unlocked；
//!      逐层 1→2→3→4→5 解锁，体力按 difficulty 扣减、last_action=mine_descend_<n>；
//!   3. mine：回 1 层挖矿，矿石入包（铜/铁池加权恒出 1 块）、体力按 energy_cost 扣、
//!      last_action=mine_<ore>；体力见底（energy=1）拒绝 tired 且不写动作、背包不变；
//!   4. elevator：解锁 5 层后从 1 层电梯直达 5 层（不耗体力、last_action=mine_elevator_5）；
//!      未解锁/非电梯层拒绝 not_unlocked；
//!   5. 矿石入箱：铜矿石 shipping.add 成功（sell>0 可入镖局货箱）；
//!   6. 存档往返：mine.capture() → reset() → restore() 后 current/unlocked 一致；
//!      背包铜矿石经 inventory.capture/restore 数量一致；
//!   7. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m46_mine [-- <game_jhyt-root>]

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

    banner("① 静态：30 层 / 电梯层 / validate_all 空 / 入口恒解锁");
    let floor_count = query_num(&mut runtime, "#require('data.mine').floors")?;
    if floor_count != 30.0 {
        return Err(format!("矿脉层数 = {floor_count}，应为 30").into());
    }
    let elevators = query_str(
        &mut runtime,
        "local m = require('data.mine'); local t = {}; for _, f in ipairs(m.elevator_floors()) do t[#t+1] = f end; return table.concat(t, ',')",
    )?;
    if elevators != "5,10,15,20,25,30" {
        return Err(format!("电梯层 = {elevators:?}，应为 5,10,15,20,25,30").into());
    }
    let report = query_str(
        &mut runtime,
        "return require('systems.mine').validate_all()",
    )?;
    if !report.is_empty() {
        return Err(format!("mine.validate_all 不通过:\n{report}").into());
    }
    assert_num(&mut runtime, "require('systems.mine').current()", 1.0)?;
    assert_num(
        &mut runtime,
        "(function() local u = require('systems.mine').unlocked(); return u[1] and 1 or 0 end)()",
        1.0,
    )?;
    println!("  30 层；电梯层 {elevators}；validate_all 空；入口 1 层恒解锁 ✓");

    banner("② descend：跳层拒绝、逐层解锁到电梯层 5、体力按 difficulty 扣");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 新档体力 100。先把体力钉满，排除其它系统的体力扰动。
    lua(&mut runtime, "require('core.state').energy = 100")?;
    // 跳层拒绝：current=1，descend(3) 不是下一层 → not_unlocked（失败不写 last_action）
    assert_str(
        &mut runtime,
        "(function() local ok, r = require('systems.mine').descend(3); return r end)()",
        "not_unlocked",
    )?;
    assert_num(&mut runtime, "require('systems.mine').current()", 1.0)?;
    // 逐层下潜：1→2→3→4→5
    for target in 2..=5 {
        let energy_before = query_num(&mut runtime, "require('core.state').energy")?;
        let cost = query_num(
            &mut runtime,
            &format!("require('data.mine').get({target}).difficulty"),
        )?;
        assert_num(
            &mut runtime,
            &format!(
                "(function() local ok = require('systems.mine').descend({target}); return ok and 1 or 0 end)()"
            ),
            1.0,
        )?;
        assert_num(
            &mut runtime,
            "require('systems.mine').current()",
            target as f64,
        )?;
        assert_num(
            &mut runtime,
            "require('core.state').energy",
            energy_before - cost,
        )?;
        assert_str(
            &mut runtime,
            "require('core.state').last_action",
            &format!("mine_descend_{target}"),
        )?;
    }
    let unlocked = query_str(
        &mut runtime,
        "return table.concat(require('systems.mine').unlocked_list(), ',')",
    )?;
    if unlocked != "1,2,3,4,5" {
        return Err(format!("解锁层 = {unlocked:?}，应为 1,2,3,4,5").into());
    }
    println!(
        "  跳层 descend(3) 拒 not_unlocked；逐层到 5 层解锁 {unlocked}、体力按 difficulty 扣 ✓"
    );

    banner("③ mine：1 层挖矿矿石入包、体力按 energy_cost 扣、见底拒绝");
    // 回 1 层挖浅层铜/铁（电梯直达入口，不耗体力）。
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.mine').elevator(1); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_num(&mut runtime, "require('systems.mine').current()", 1.0)?;
    lua(&mut runtime, "require('core.state').energy = 100")?;
    // 挖前：铜+铁矿石总数（新档为 0）。
    let ore_before = query_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'copper_ore') + inv.count(require('core.state').inventory, 'iron_ore')",
    )?;
    let cost1 = query_num(&mut runtime, "require('data.mine').get(1).energy_cost")?;
    let energy_before = query_num(&mut runtime, "require('core.state').energy")?;
    // 挖矿：恒出 1 块矿石（铜或铁），体力扣 energy_cost(1)。
    let mined = query_str(
        &mut runtime,
        "(function() local ok, ore = require('systems.mine').mine(); return ok and tostring(ore) or 'FAIL' end)()",
    )?;
    if mined != "copper_ore" && mined != "iron_ore" {
        return Err(format!("挖矿结果 = {mined:?}，应为 copper_ore 或 iron_ore").into());
    }
    let ore_after = query_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'copper_ore') + inv.count(require('core.state').inventory, 'iron_ore')",
    )?;
    if (ore_after - ore_before) != 1.0 {
        return Err(format!(
            "矿石入包 = {}（前 {} 后 {}），应恰 +1",
            ore_after - ore_before,
            ore_before,
            ore_after
        )
        .into());
    }
    assert_num(
        &mut runtime,
        "require('core.state').energy",
        energy_before - cost1,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        &format!("mine_{mined}"),
    )?;
    // 体力见底拒绝：energy=1 < energy_cost(1)=3 → tired，不写 last_action、背包不变。
    lua(&mut runtime, "require('core.state').energy = 1")?;
    let last_before = query_str(&mut runtime, "return require('core.state').last_action")?;
    assert_str(
        &mut runtime,
        "(function() local ok, r = require('systems.mine').mine(); return r end)()",
        "tired",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        &last_before,
    )?;
    let ore_after_tired = query_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'copper_ore') + inv.count(require('core.state').inventory, 'iron_ore')",
    )?;
    if ore_after_tired != ore_after {
        return Err(
            format!("体力见底后矿石数变动：{ore_after} → {ore_after_tired}，应不变").into(),
        );
    }
    println!("  挖矿出 {mined}（+1 入包）、体力 -{cost1}；energy=1 拒 tired 且不写动作 ✓");

    banner("④ elevator：从 1 层直达已解锁电梯层 5；未解锁/非电梯层拒绝");
    lua(&mut runtime, "require('core.state').energy = 100")?;
    // 当前在 1 层（③ 末尾 elevator(1) 后）。直达已解锁电梯层 5。
    let energy_before = query_num(&mut runtime, "require('core.state').energy")?;
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.mine').elevator(5); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_num(&mut runtime, "require('systems.mine').current()", 5.0)?;
    assert_num(&mut runtime, "require('core.state').energy", energy_before)?; // 电梯不耗体力
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "mine_elevator_5",
    )?;
    // 未解锁/非电梯层拒绝：7 层既非电梯层也未解锁。
    assert_str(
        &mut runtime,
        "(function() local ok, r = require('systems.mine').elevator(7); return r end)()",
        "not_unlocked",
    )?;
    assert_num(&mut runtime, "require('systems.mine').current()", 5.0)?;
    println!("  1 层电梯直达 5 层（不耗体力）；elevator(7) 拒 not_unlocked ✓");

    banner("⑤ 矿石入箱：铜矿石可入镖局货箱（sell>0）");
    // 补一块铜矿石进背包，验证 shipping.add 接受矿石（sell>0）。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'copper_ore', 1)",
    )?;
    let ship_ok = query_str(
        &mut runtime,
        "(function() local ok, r = require('systems.shipping').add('copper_ore', 1); return tostring(ok) .. '|' .. tostring(r) end)()",
    )?;
    if ship_ok != "true|ok" {
        return Err(format!("shipping.add(copper_ore) = {ship_ok:?}，应为 true|ok").into());
    }
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "ship_copper_ore",
    )?;
    println!("  铜矿石入箱成功（last_action=ship_copper_ore）✓");

    banner("⑥ 存档往返：capture → reset → restore 后 current/unlocked/背包一致");
    // 当前在 5 层、解锁 1..5。先 capture 矿脉状态。
    let captured = query_str(
        &mut runtime,
        "(function() local c = require('systems.mine').capture(); \
         return tostring(c.current) .. '|' .. table.concat(c.unlocked, ',') end)()",
    )?;
    if captured != "5|1,2,3,4,5" {
        return Err(format!("mine.capture = {captured:?}，应为 5|1,2,3,4,5").into());
    }
    // reset 回初始（1 层、仅解锁 1）。
    lua(&mut runtime, "require('systems.mine').reset()")?;
    assert_num(&mut runtime, "require('systems.mine').current()", 1.0)?;
    assert_str(
        &mut runtime,
        "table.concat(require('systems.mine').unlocked_list(), ',')",
        "1",
    )?;
    // restore 回 capture 的状态。
    lua(
        &mut runtime,
        "(function() require('systems.mine').restore({ current = 5, unlocked = {1,2,3,4,5} }) end)()",
    )?;
    assert_num(&mut runtime, "require('systems.mine').current()", 5.0)?;
    assert_str(
        &mut runtime,
        "table.concat(require('systems.mine').unlocked_list(), ',')",
        "1,2,3,4,5",
    )?;
    // 背包铜矿石经 inventory.capture/restore 数量一致。
    let bag_copper = query_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'copper_ore')",
    )?;
    let inv_snap = query_str(
        &mut runtime,
        "(function() local c = require('player.inventory').capture(); return tostring(#c.slots) end)()",
    )?;
    println!(
        "  capture {captured} → reset → restore 回 5 层/解锁 1..5；背包铜矿石 {bag_copper}（槽位 {inv_snap}）✓"
    );

    banner("⑦ 数据校验门（validate_data strict）在整条链路后依然通过");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m46_mine_ok floors=30 elevators=5,10,15,20,25,30 descend=stepwise mine=ore+1 elevator=fast5 ship=copper_ore save=roundtrip",
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
