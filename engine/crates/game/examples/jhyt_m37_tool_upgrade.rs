//! Headless acceptance drive for tool upgrades (改造计划 §4.2/§5.3 / docs/33 §5.4, M37).
//!
//! 验证锄头/水壶 4 阶升级的真实减耗链路（数据口径 = docs/33 §5.4 工具升级表 ==
//! data/tools.lua 的 tier/energy == data/shops.lua han_forge 售价）：
//!   1. 基础锄翻土消耗 8、精铁 6、寒铁 5、陨铁/灵铁 4（farm 按装备工具 tier 减耗）；
//!   2. 基础壶浇水 4、精铁 3、寒铁/陨铁/灵铁 2；
//!   3. 静态断言：han_forge 售卖锄/壶 4 阶、售价 500/1500/4000/12000；
//!      工具档位 tier 1..5、energy 随档非增；
//!   4. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! 对照口径：翻土/浇水的基础档消耗必须与 0.2 基线一致（否则旧样例与玩家旧档
//! 行为漂移），由 `jhyt_m24_numeric_alignment` / `jhyt_m36_regrow` 回归兜底。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m37_tool_upgrade [-- <game_jhyt-root>]

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

    banner("① 新档进山庄：体力 100，基础锄翻土消耗 8");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    set_facing(&mut runtime, "up")?;
    let e0 = energy(&mut runtime)?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    let e1 = energy(&mut runtime)?;
    check_cost("基础锄翻土", e0, e1, 8.0)?;

    banner("② 取药田区 4 块未开垦 DIRT，精铁/寒铁/陨铁逐档翻土减耗");
    let cells = dirt_cells(&mut runtime, 4)?;
    if cells.len() < 4 {
        return Err(format!("药田区未开垦 DIRT 不足 4 格，实际 {}", cells.len()).into());
    }
    // 高阶工具只由铁铺售卖（真实玩家花钱购买）；测试角色先补给再装备，
    // 与 `jhyt_m36_regrow` 的 seed_mint 同法（inv.equip 要求背包里有货）。
    // 精铁锄头：翻土 6
    add_and_equip(&mut runtime, "hoe_iron")?;
    let cost = till_at(&mut runtime, cells[0])?;
    check_cost("精铁锄头翻土", cost.0, cost.1, 6.0)?;
    // 寒铁锄头：翻土 5
    add_and_equip(&mut runtime, "hoe_cold")?;
    let cost = till_at(&mut runtime, cells[1])?;
    check_cost("寒铁锄头翻土", cost.0, cost.1, 5.0)?;
    // 陨铁锄头：翻土 4（第 3 档封顶下限）
    add_and_equip(&mut runtime, "hoe_meteor")?;
    let cost = till_at(&mut runtime, cells[2])?;
    check_cost("陨铁锄头翻土", cost.0, cost.1, 4.0)?;
    let cost = till_at(&mut runtime, cells[3])?;
    check_cost("陨铁锄头翻土(第二格)", cost.0, cost.1, 4.0)?;

    banner("③ 在最后一格种薄荷，浇水逐档减耗（基础 4 → 精铁 3 → 寒铁 2）");
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_mint', 1)")?;
    set_seed(&mut runtime, "seed_mint")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_mint",
    )?;
    add_and_equip(&mut runtime, "watering_can")?;
    runtime.execute_console("require('world.weather').set('sunny')")?;
    let e2 = energy(&mut runtime)?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "water_mint",
    )?;
    let e3 = energy(&mut runtime)?;
    check_cost("基础壶浇水", e2, e3, 4.0)?;
    runtime.execute_console("day.sleep()")?;
    add_and_equip(&mut runtime, "water_iron")?;
    runtime.execute_console("require('world.weather').set('sunny')")?;
    let e4 = energy(&mut runtime)?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "water_mint",
    )?;
    let e5 = energy(&mut runtime)?;
    check_cost("精铁壶浇水", e4, e5, 3.0)?;
    runtime.execute_console("day.sleep()")?;
    add_and_equip(&mut runtime, "water_cold")?;
    runtime.execute_console("require('world.weather').set('sunny')")?;
    let e6 = energy(&mut runtime)?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "water_mint",
    )?;
    let e7 = energy(&mut runtime)?;
    check_cost("寒铁壶浇水", e6, e7, 2.0)?;

    banner("④ 静态断言：铁铺售价 500/1500/4000/12000、档位 tier 1..5、energy 非增");
    let prices = query_str(
        &mut runtime,
        "local s = require('data.shops').han_forge\n\
         local out = {}\n\
         for _, it in ipairs(s.stock or {}) do\n\
           if tostring(it.item):find('^hoe_') or tostring(it.item):find('^water_') then\n\
             out[#out + 1] = it.item .. '=' .. it.price\n\
           end\n\
         end\n\
         table.sort(out)\n\
         return table.concat(out, ',')",
    )?;
    let expect_prices = "hoe_cold=1500,hoe_iron=500,hoe_meteor=4000,hoe_spirit=12000,".to_owned()
        + "water_cold=1500,water_iron=500,water_meteor=4000,water_spirit=12000";
    if prices != expect_prices {
        return Err(format!("han_forge 工具售价不符：{prices:?}，应为 {expect_prices:?}").into());
    }
    let tiers = query_str(
        &mut runtime,
        "local tools = require('data.tools')\n\
         local ids = { 'hoe','hoe_iron','hoe_cold','hoe_meteor','hoe_spirit',\n\
           'watering_can','water_iron','water_cold','water_meteor','water_spirit' }\n\
         local out = {}\n\
         for _, id in ipairs(ids) do\n\
           local t = tools[id]\n\
           out[#out + 1] = id .. '|' .. tostring(t and t.tier or 0) .. '|' .. tostring(t and t.energy or -1)\n\
         end\n\
         return table.concat(out, ';')",
    )?;
    let expect_tiers = "hoe|1|5;hoe_iron|2|4;hoe_cold|3|3;hoe_meteor|4|2;hoe_spirit|5|1;"
        .to_owned()
        + "watering_can|1|2;water_iron|2|2;water_cold|3|1;water_meteor|4|1;water_spirit|5|1";
    if tiers != expect_tiers {
        return Err(
            format!("tools.lua 档位/energy 不符：{tiers:?}\n应为：{expect_tiers:?}").into(),
        );
    }
    println!("  铁铺售价与工具档位全部与 docs/33 §5.4 一致");

    banner("⑤ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m37_tool_upgrade_ok project={} till=8/6/5/4 water=4/3/2 prices=8 tools=10",
        project.display()
    );
    Ok(())
}

/// 移动到指定格下方并朝上，翻土一次；返回 (前, 后) 体力。
fn till_at(
    runtime: &mut LuaRuntime,
    cell: (i64, i64),
) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    move_to(runtime, cell.0, cell.1)?;
    let before = energy(runtime)?;
    press(runtime, KeyCode::KeyE)?;
    assert_str(runtime, "require('core.state').last_action", "till")?;
    let after = energy(runtime)?;
    Ok((before, after))
}

/// 药田区（medicinal）里前 `count` 块未开垦的 DIRT（GroundMap tile==2），
/// 排除已在 farm_plots 里的地块，返回 "cx,cy" 列表。
fn dirt_cells(
    runtime: &mut LuaRuntime,
    count: usize,
) -> Result<Vec<(i64, i64)>, Box<dyn std::error::Error>> {
    let raw = query_str(
        runtime,
        "local farm = require('data.farm')\n\
         local g = entity.find('GroundMap')\n\
         local plots = map.state('farm_plots') or {}\n\
         local out = {}\n\
         for _, land in ipairs(farm.land) do\n\
           if land.type == 'medicinal' then\n\
             for cx = land.x0, land.x1 do\n\
               for cy = land.y0, land.y1 do\n\
                 local key = cx .. ',' .. cy\n\
                 if tilemap.get_cell(g, cx, cy) == 2 and not plots[key] then\n\
                   out[#out + 1] = key\n\
                 end\n\
               end\n\
             end\n\
           end\n\
         end\n\
         return table.concat(out, ';')",
    )?;
    let mut cells = Vec::new();
    for part in raw.split(';').filter(|s| !s.is_empty()) {
        let (x, y) = part
            .split_once(',')
            .ok_or_else(|| format!("cell {part:?} 格式不符"))?;
        cells.push((x.parse()?, y.parse()?));
        if cells.len() >= count {
            break;
        }
    }
    Ok(cells)
}

fn move_to(runtime: &mut LuaRuntime, cx: i64, cy: i64) -> Result<(), Box<dyn std::error::Error>> {
    // 站到目标格下方一格、朝上，正对目标格（facing_cell 取"面前一格"）。
    runtime.execute_console(&format!(
        "local pid = require('player.player').ensure()\n\
         transform.set(pid, {}, {})",
        (cx as f64 + 0.5) * 32.0,
        (cy as f64 + 1.5) * 32.0
    ))?;
    set_facing(runtime, "up")?;
    Ok(())
}

fn check_cost(
    label: &str,
    before: f64,
    after: f64,
    expected: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let cost = before - after;
    if (cost - expected).abs() > f64::EPSILON {
        return Err(format!("{label}消耗 {cost}，应为 {expected}（docs/33 §5.4）").into());
    }
    println!("  {label}: -{cost}（符合设计案）");
    Ok(())
}

fn energy(runtime: &mut LuaRuntime) -> Result<f64, Box<dyn std::error::Error>> {
    query_num(runtime, "require('core.state').energy")
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

fn set_facing(runtime: &mut LuaRuntime, facing: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('core.state').facing = '{facing}'"))?;
    Ok(())
}

fn set_seed(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('core.state').selected_seed = '{item}'"))?;
    Ok(())
}

fn equip_tool(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "inv.equip(require('core.state').inventory, 'tool', '{item}')"
    ))?;
    Ok(())
}

/// 补给 1 件（高阶工具只由铁铺售卖，新档背包没有）再装备。
fn add_and_equip(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "inv.add(require('core.state').inventory, '{item}', 1)"
    ))?;
    equip_tool(runtime, item)
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
