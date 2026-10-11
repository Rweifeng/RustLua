//! Headless acceptance drive for the animal husbandry (畜牧 / 药园畜养,
//! docs/54 §4.3, docs/33 §4.4, M44).
//!
//! 验证畜牧系统真实链路（数据口径 = docs/33 §4.4 == data/animals.lua ==
//! data/items_animal.lua == systems/animal.lua）：
//!   1. 静态：6 种动物、coop/barn/stable 三畜舍；animal.validate_all 通过；
//!   2. 负例：未建鸡舍不能养（adopt → no_house）；
//!   3. 建鸡舍（building 放置 API）→ 领养白凤鸡成功；
//!   4. 喂食/抚摸：消耗饲料、加好感、写 last_action；重复喂食/抚摸被拒且不覆盖
//!      last_action；
//!   5. 跨天产出（显式调用 on_day_start 钩子）：吃饱才产（蛋进背包）、未喂不产；
//!      好感达标（≥8）出灵蛋替代普通蛋；
//!   6. 出货箱路径：shipping.add(egg) 成功（sell>0 可入箱）；
//!   7. 存档往返：capture/reset/restore 后好感与拥有数一致；
//!   8. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m44_animal [-- <game_jhyt-root>]

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

    banner("① 静态断言：6 种动物、三畜舍、animal.validate_all 通过");
    let meta = query_str(
        &mut runtime,
        "local an = require('data.animals')\n\
         local b = require('data.buildings')\n\
         local coop = b.get('coop')\n\
         local barn = b.get('barn')\n\
         local stable = b.get('stable')\n\
         return tostring(#an.ids()) .. '|' .. tostring(coop.w) .. 'x' .. tostring(coop.h) .. '|' .. tostring(coop.station) .. '|' .. tostring(#coop.levels) .. '|' .. tostring(barn.w) .. 'x' .. tostring(barn.h) .. '|' .. tostring(stable.w) .. 'x' .. tostring(stable.h)",
    )?;
    // 6 种动物；鸡舍 2x2/station=nil/2 级；畜棚 3x2；马厩 2x2。
    if meta != "6|2x2|nil|2|3x2|2x2" {
        return Err(format!("畜牧静态表 = {meta:?}，应为 6|2x2|nil|2|3x2|2x2").into());
    }
    let report = query_str(
        &mut runtime,
        "return require('systems.animal').validate_all()",
    )?;
    if !report.is_empty() {
        return Err(format!("animal.validate_all 不通过:\n{report}").into());
    }
    println!("  6 种动物；coop 2x2/station=nil/2 级、barn 3x2、stable 2x2；validate_all 空 ✓");

    banner("② 进山庄新档：未建鸡舍 → 领养白凤鸡被拒（no_house）");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    let adopt_no_house = query_str(
        &mut runtime,
        "(function() local ok, reason = require('systems.animal').adopt('bai_feng_ji'); return ok and 'ok' or reason end)()",
    )?;
    if adopt_no_house != "no_house" {
        return Err(format!("未建鸡舍领养应被拒 no_house，实际 {adopt_no_house:?}").into());
    }
    println!("  未建鸡舍 → adopt 拒绝（no_house）✓");

    banner("③ 建鸡舍（can_place 扫描合法锚点 + place）→ 领养白凤鸡成功");
    let anchor = query_str(
        &mut runtime,
        "(function() local b = require('systems.building'); for y = 0, 21 do for x = 0, 39 do \
           if b.can_place('coop', x, y) then b.place('coop', x, y); return x .. ',' .. y end end end return 'none' end)()",
    )?;
    if anchor == "none" {
        return Err("manor 无可放 coop 的锚点".into());
    }
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "build_coop",
    )?;
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.animal').adopt('bai_feng_ji'); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "adopt_bai_feng_ji",
    )?;
    assert_num(&mut runtime, "require('systems.animal').count()", 1.0)?;
    println!("  鸡舍建于 ({anchor})；领养白凤鸡成功（count=1）");

    banner("④ 喂食/抚摸：消耗饲料加好感、写 last_action；重复喂食/抚摸被拒");
    // 备足饲料（新档仅 rice×2）。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'rice', 20)",
    )?;
    let rice_before = query_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'rice')",
    )?;
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.animal').feed('bai_feng_ji'); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "feed_bai_feng_ji",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.animal').mood('bai_feng_ji')",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'rice')",
        rice_before - 1.0,
    )?;
    // 重复喂食：拒绝且不覆盖 last_action、不再扣饲料。
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.animal').feed('bai_feng_ji'); return ok and 1 or 0 end)()",
        0.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "feed_bai_feng_ji",
    )?;
    // 抚摸：mood+1。
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.animal').pet('bai_feng_ji'); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "pet_bai_feng_ji",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.animal').mood('bai_feng_ji')",
        2.0,
    )?;
    // 重复抚摸：拒绝。
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.animal').pet('bai_feng_ji'); return ok and 1 or 0 end)()",
        0.0,
    )?;
    println!("  喂食扣 1 稻（mood 0→1）、抚摸 +1（→2）；重复喂食/抚摸拒绝且 last_action 不变");

    banner("⑤ 跨天产出（显式 on_day_start）：吃饱产蛋、未喂不产");
    // 第 1 次清晨：昨天喂过 → 产 1 蛋（mood=2 < 8 普通蛋）；晴天放牧 mood→3。
    lua(
        &mut runtime,
        "require('world.weather').set('sunny'); require('systems.animal').on_day_start()",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'egg')",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.animal').mood('bai_feng_ji')",
        3.0,
    )?;
    // 第 2 次清晨：未喂 → 不产（蛋仍 1）；放牧 mood→4。
    lua(
        &mut runtime,
        "require('world.weather').set('sunny'); require('systems.animal').on_day_start()",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'egg')",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.animal').mood('bai_feng_ji')",
        4.0,
    )?;
    println!("  吃饱产蛋（egg=1）、隔一天未喂不产（仍 egg=1）；mood 经放牧 →4");

    banner("⑥ 好感达标（≥8）→ 产灵蛋替代普通蛋");
    // 喂+抚：mood 4→5→6；清晨产普通蛋（egg 1→2），放牧 mood→7。
    lua(
        &mut runtime,
        "require('systems.animal').feed('bai_feng_ji'); require('systems.animal').pet('bai_feng_ji')",
    )?;
    lua(
        &mut runtime,
        "require('world.weather').set('sunny'); require('systems.animal').on_day_start()",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'egg')",
        2.0,
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'spirit_egg')",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.animal').mood('bai_feng_ji')",
        7.0,
    )?;
    // 再喂+抚：mood 7→8→9；清晨 mood=9 ≥8 → 产灵蛋（egg 仍 2、spirit_egg=1）。
    lua(
        &mut runtime,
        "require('systems.animal').feed('bai_feng_ji'); require('systems.animal').pet('bai_feng_ji')",
    )?;
    lua(
        &mut runtime,
        "require('world.weather').set('sunny'); require('systems.animal').on_day_start()",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'egg')",
        2.0,
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'spirit_egg')",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.animal').mood('bai_feng_ji')",
        10.0,
    )?;
    println!("  mood=6 产普通蛋（egg=2）；mood=9 ≥8 产灵蛋（spirit_egg=1，egg 不增）");

    banner("⑦ 出货箱路径：shipping.add(egg) 成功（sell>0 可入箱）");
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('egg', 1); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "ship_egg",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').pending()['egg']",
        1.0,
    )?;
    println!("  蛋入镖局货箱（pending.egg=1）✓");

    banner("⑧ 存档往返：capture → reset → restore，好感 10 与拥有数 1 恢复");
    lua(
        &mut runtime,
        "local a = require('systems.animal'); local c = a.capture(); a.reset(); a.restore(c)",
    )?;
    assert_num(&mut runtime, "require('systems.animal').count()", 1.0)?;
    assert_num(
        &mut runtime,
        "require('systems.animal').mood('bai_feng_ji')",
        10.0,
    )?;
    println!("  capture/reset/restore 后：count=1、mood=10 ✓");

    banner("⑨ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m44_animal_ok project={} animals=6 houses=coop/barn/stable adopt=no_house->ok feed=pet=+mood produce=egg spirit@mood8 ship=egg save=roundtrip",
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
