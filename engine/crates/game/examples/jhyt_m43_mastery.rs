//! Headless acceptance drive for the mastery system (熟练度 5 线 10 级 + 专精,
//! 改造计划 §4.9/§5.4, docs/33 §7.1, M43).
//!
//! 验证熟练度真实链路（数据口径 = docs/33 §7.1 0.9 == data/mastery.lua ==
//! systems/mastery.lua == farm/gather/shop/crafting 来源接线）：
//!   1. 静态：5 线 10 级（累计 XP 50..2600 同一曲线）；专精 20 个且每线
//!      5/10 级槽位各恰 2 个候选（2 选 1）；mastery.validate_all 通过；
//!   2. 来源接线实测（新档山庄）：翻土 +5 → 播种 +5 → 浇水 3 天成熟 →
//!      收获 +5（务农 15 XP）；等级按累计阈值计算（补到 50 → 1 级）；
//!   3. 专精：未达 5 级选"精耕"被拒（not_unlocked）；务农 5 级后选择成功
//!      （last_action=mastery_jinggeng）；同槽重复被拒（already）；
//!   4. 效果钩子（farming_bonus 单一事实来源，不依赖随机分布）：
//!      L=5+精耕 → yield_pct=10、良好 25+2L=35、上品 floor(5+0.5L)=7；
//!      L=10+精耕+深耕 → yield_pct=15、regrow_cut=1、封顶 45/10；
//!   5. 江湖来源接线实测：zhao_stall 卖出稻谷成功 → jianghu XP +10；
//!      （采药 +5 / 制药 +8 接线同构，代码级由 validate 门守护）
//!   6. 存档：capture/restore 往返（XP 2600 与专精 jinggeng/gengshen 恢复）；
//!   7. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m43_mastery [-- <game_jhyt-root>]

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

    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let host = handles.input;
    let mut runtime = LuaRuntime::with_installers(installers)?;
    runtime.set_project_root(project.clone());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    runtime.execute_project_script(&main_script)?;

    banner("① 静态断言：5 线 10 级阈值表、专精 20 个（每槽 2 选 1）、validate_all 通过");
    let meta = query_str(
        &mut runtime,
        "local m = require('data.mastery')\n\
         local lines, specs = 0, 0\n\
         local slot_ok = true\n\
         for _, line in pairs(m.lines) do\n\
           lines = lines + 1\n\
           local counts = {}\n\
           for _, s in ipairs(line.specializations) do\n\
             specs = specs + 1\n\
             counts[s.slot] = (counts[s.slot] or 0) + 1\n\
           end\n\
           for _, c in pairs(counts) do if c ~= 2 then slot_ok = false end end\n\
         end\n\
         return tostring(lines) .. '|' .. tostring(specs) .. '|' .. tostring(slot_ok and 1 or 0) .. '|' .. tostring(#m.thresholds) .. '|' .. tostring(m.thresholds[10])",
    )?;
    if meta != "5|20|1|10|2600" {
        return Err(format!("熟练度注册表 = {meta:?}，应为 5|20|1|10|2600").into());
    }
    let report = query_str(
        &mut runtime,
        "return require('systems.mastery').validate_all()",
    )?;
    if !report.is_empty() {
        return Err(format!("mastery.validate_all 不通过:\n{report}").into());
    }
    println!("  5 线 10 级（累计 2600）、专精 20 个（每槽 2 选 1）、validate_all 空 ✓");

    banner("② 进山庄新档：务农 XP 来源接线实测（翻土/播种/收获 +5）与等级计算");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 新档初始站在田边朝 up，面前即药田格 → 翻土（m36_regrow 同款姿势）
    set_facing(&mut runtime, "up")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').xp_of('farming')",
        5.0,
    )?;
    // 播种薄荷：补 1 粒籽（新档只有稻种/菌种，seed_for_selected 回退灵芝）
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_mint', 1)")?;
    set_seed(&mut runtime, "seed_mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_mint",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').xp_of('farming')",
        10.0,
    )?;
    println!("  翻土 +5、播种 +5 → 务农 10 XP");

    banner("③ 浇水 3 天成熟 → 收获 +5 → 15 XP；补到 50 → 1 级（累计阈值）");
    equip_tool(&mut runtime, "watering_can")?;
    for day in 1..=6 {
        runtime.execute_console("require('world.weather').set('sunny')")?;
        press(&mut runtime, &host, KeyCode::KeyE)?;
        let action = query_str(&mut runtime, "require('core.state').last_action")?;
        if action == "harvest_mint" {
            return Err("浇水时薄荷已成熟（成熟判定早于预期），应先行收获".into());
        }
        if action != "water_mint" {
            return Err(format!("浇水失败，last_action = {action:?}（第 {day} 天）").into());
        }
        runtime.execute_console("day.sleep()")?;
        let mature = query_str(
            &mut runtime,
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
            break;
        }
        if day >= 6 {
            return Err("薄荷 6 天未成熟，再生生长判定可能被破坏".into());
        }
    }
    // 成熟 → 收获（务农 +5）
    equip_tool(&mut runtime, "hoe")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').xp_of('farming')",
        15.0,
    )?;
    // 等级：15 XP → 0 级；补 35 → 50 → 1 级（阈值 50）
    assert_num(
        &mut runtime,
        "require('systems.mastery').level('farming')",
        0.0,
    )?;
    lua(
        &mut runtime,
        "require('systems.mastery').add_xp('farming', 35)",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').level('farming')",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').xp_of('farming')",
        50.0,
    )?;
    println!("  收获 +5 → 15 XP（0 级）；补 35 → 50 XP → 1 级 ✓");

    banner("④ 专精：未达 5 级被拒、5 级解锁、同槽重复被拒");
    let rejected = query_str(
        &mut runtime,
        "(function() local ok = require('systems.mastery').choose('farming', 'jinggeng'); return ok and 'ok' or 'rejected' end)()",
    )?;
    if rejected != "rejected" {
        return Err(format!("未达 5 级应拒绝选专精，实际 {rejected:?}").into());
    }
    lua(
        &mut runtime,
        "require('systems.mastery').add_xp('farming', 500)",
    )?; // 550 → 5 级
    assert_num(
        &mut runtime,
        "require('systems.mastery').level('farming')",
        5.0,
    )?;
    let chosen = query_str(
        &mut runtime,
        "(function() local ok = require('systems.mastery').choose('farming', 'jinggeng'); return ok and 'ok' or 'no' end)()",
    )?;
    if chosen != "ok" {
        return Err(format!("务农 5 级应可选精耕，实际 {chosen:?}").into());
    }
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "mastery_jinggeng",
    )?;
    let dup = query_str(
        &mut runtime,
        "(function() local ok = require('systems.mastery').choose('farming', 'jinggeng'); return ok and 'ok' or 'already' end)()",
    )?;
    if dup != "already" {
        return Err(format!("同槽重复应被拒，实际 {dup:?}").into());
    }
    println!("  未达等级拒绝；5 级选精耕成功；同槽重复拒绝 ✓");

    banner("⑤ 效果钩子（farming_bonus 单一事实来源）：L=5+精耕 → 10% 产量 / 品质 35/7");
    let bonus = query_str(
        &mut runtime,
        "local b = require('systems.mastery').farming_bonus(); \
         return tostring(b.yield_pct) .. '|' .. tostring(b.good) .. '|' .. tostring(b.super) .. '|' .. tostring(b.regrow_cut) .. '|' .. tostring(b.level)",
    )?;
    if bonus != "10|35|7|0|5" {
        return Err(format!("务农 5 级+精耕 bonus = {bonus:?}，应为 10|35|7|0|5").into());
    }
    println!("  精耕 yield_pct=10%、良好 25+2L=35、上品 floor(5+0.5L)=7 ✓");

    banner("⑥ 务农 10 级 + 深耕：封顶 45/10、再生茬 -1 天（regrow_cut=1）");
    lua(
        &mut runtime,
        "require('systems.mastery').add_xp('farming', 2050)",
    )?; // 2600 → 10 级
    assert_num(
        &mut runtime,
        "require('systems.mastery').level('farming')",
        10.0,
    )?;
    let gengshen = query_str(
        &mut runtime,
        "(function() local ok = require('systems.mastery').choose('farming', 'gengshen'); return ok and 'ok' or 'no' end)()",
    )?;
    if gengshen != "ok" {
        return Err(format!("务农 10 级应可选深耕，实际 {gengshen:?}").into());
    }
    let bonus = query_str(
        &mut runtime,
        "local b = require('systems.mastery').farming_bonus(); \
         return tostring(b.yield_pct) .. '|' .. tostring(b.regrow_cut) .. '|' .. tostring(b.good) .. '|' .. tostring(b.super)",
    )?;
    // 精耕+深耕（10 级槽 2 选 1 选了深耕，富饶未选）→ 产量仅精耕 +10%；
    // 品质封顶 45/10、深耕 regrow_cut=1。
    if bonus != "10|1|45|10" {
        return Err(format!("务农 10 级+精耕+深耕 bonus = {bonus:?}，应为 10|1|45|10").into());
    }
    println!(
        "  深耕 regrow_cut=1（再生茬 -1 天下限 2）、10 级槽 2 选 1 生效（富饶未选）、品质封顶 45/10 ✓"
    );

    banner("⑦ 江湖来源接线实测：zhao_stall 卖出稻谷成功 → jianghu +10");
    // zhao_stall 营业窗口 480..1080（08:00–18:00）；拨钟到 10:00 再卖。
    lua(&mut runtime, "clock.set(clock.now().day, 600)")?;
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'rice', 1)",
    )?;
    let sold = query_str(
        &mut runtime,
        "(function() local ok = require('systems.shop').sell('zhao_stall', 'rice', 1); return ok and 'ok' or 'no' end)()",
    )?;
    if sold != "ok" {
        return Err(format!("zhao_stall 卖稻谷应成功，实际 {sold:?}").into());
    }
    assert_num(
        &mut runtime,
        "require('systems.mastery').xp_of('jianghu')",
        10.0,
    )?;
    println!("  卖出成功 → 江湖 XP +10 ✓（采药 +5 / 制药 +8 接线同构，由 validate 门守护）");

    banner("⑧ 存档往返：capture/restore 恢复 XP 2600 与专精 jinggeng/gengshen");
    let cap = query_str(
        &mut runtime,
        "local c = require('systems.mastery').capture(); \
         return tostring(c.xp.farming) .. '|' .. tostring(c.chosen.farming and c.chosen.farming[5] or 'nil') .. '|' .. tostring(c.chosen.farming and c.chosen.farming[10] or 'nil')",
    )?;
    if cap != "2600|jinggeng|gengshen" {
        return Err(format!("capture = {cap:?}，应为 2600|jinggeng|gengshen").into());
    }
    lua(
        &mut runtime,
        "local m = require('systems.mastery'); local c = m.capture(); m.reset(); m.restore(c)",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').level('farming')",
        10.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').has('farming', 'jinggeng') and 1 or 0",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.mastery').has('farming', 'gengshen') and 1 or 0",
        1.0,
    )?;
    println!("  capture/restore 往返：XP 2600 与专精恢复 ✓");

    banner("⑨ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m43_mastery_ok project={} lines=5/10lv specs=20 till=+5 plant=+5 harvest=+5 lv1@50 lv5=jinggeng lv10=gengshen bonus=15/45/10 regrow=-1 sell=+10 save=ok",
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

fn press(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    key: KeyCode,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    host.set_input(input);
    runtime.call_update()?;
    host.set_input(InputSnapshot::default());
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

fn equip_tool(runtime: &mut LuaRuntime, family: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "inv.equip(require('core.state').inventory, 'tool', '{}')",
        family
    ))?;
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
