//! Headless acceptance drive for sprinklers (改造计划 §4.2 / docs/33 §4.1.4, M39).
//!
//! 验证洒水器 3 级的真实耕作链路（数据口径 = docs/33 §4.1.4 覆盖表 ==
//! data/items.lua 的 sprinkler 字段 == data/recipes.lua 铁匠铺配方）：
//!   1. 静态：3 条洒水器条目（kind=sprinkler、shape/radius、buy=0/sell=0）、
//!      3 条 forge 配方（机关需 2 级）、洒水器不进任何商店 stock/buys；
//!   2. 竹筒（十字 4 邻）：下邻 (16,5) 被浇成熟；下 2 格 (16,6) 与斜角 (17,5)
//!      不被浇（不成熟）——十字只浇上下左右；
//!   3. 水车（3×3）：斜角 (17,5) 被浇成熟（方阵含斜角）；
//!   4. 机关（5×5）：远格 (16,6) 被浇成熟（radius=2，水车/竹筒都浇不到）；
//!   5. 雨天保留：雨天清晨洒水器照常浇水（成长继续累计）；
//!   6. 拆除：E 拆除收回背包，地块回到已垦空地；
//!   7. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! 覆盖矩阵（S=(16,4)）：(16,5) dy=1 三器都浇；(17,5) dx=1&dy=1 竹筒十字不浇、
//! 水车/机关浇；(16,6) dy=2 竹筒/水车不浇、机关浇。薄荷生长 3 天，未浇则
//! watered_days 不累计（缺水不成长但不死亡）。
//!
//! 对照口径：洒水器制作链路由 `jhyt_m18_build`（crafting 全链路）与静态配方
//! 断言覆盖；本样例聚焦放置/拆除/清晨自动浇水（farm 新机制）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m39_sprinkler [-- <game_jhyt-root>]

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

    banner("① 静态断言：洒水器条目/覆盖字段/forge 配方/不进商店");
    let specs = query_str(
        &mut runtime,
        "local items = require('data.items')\n\
         local ids = { 'sprinkler_bamboo', 'sprinkler_waterwheel', 'sprinkler_machine' }\n\
         local out = {}\n\
         for _, id in ipairs(ids) do\n\
           local d = items[id]\n\
           local s = d and d.sprinkler or {}\n\
           out[#out + 1] = id .. '|' .. tostring(d and d.kind or 'none') .. '|' .. tostring(s.shape or 'none') .. '|' .. tostring(s.radius or -1) .. '|' .. tostring(d and d.buy or -1) .. '|' .. tostring(d and d.sell or -1) .. '\\n'\n\
         end\n\
         return table.concat(out, '')",
    )?;
    let expect_specs = "sprinkler_bamboo|sprinkler|cross|1|0|0\n".to_owned()
        + "sprinkler_waterwheel|sprinkler|square|1|0|0\n"
        + "sprinkler_machine|sprinkler|square|2|0|0";
    if specs.trim() != expect_specs {
        return Err(format!("洒水器条目不符：\n{specs}\n应为：\n{expect_specs}").into());
    }
    // forge 配方：station=forge、output 匹配、机关 min_level=2。
    let recipes = query_str(
        &mut runtime,
        "local recipes = require('data.recipes').recipes\n\
         local out = {}\n\
         for _, id in ipairs({ 'sprinkler_bamboo_build', 'sprinkler_waterwheel_build', 'sprinkler_machine_build' }) do\n\
           local r = recipes[id]\n\
           out[#out + 1] = id .. '|' .. tostring(r and r.station or 'none') .. '|' .. tostring(r and r.output and r.output.item or 'none') .. '|' .. tostring(r and r.minutes or -1) .. '|' .. tostring(r and r.min_level or -1)\n\
         end\n\
         return table.concat(out, ';')",
    )?;
    let expect_recipes = "sprinkler_bamboo_build|forge|sprinkler_bamboo|60|1;".to_owned()
        + "sprinkler_waterwheel_build|forge|sprinkler_waterwheel|120|1;"
        + "sprinkler_machine_build|forge|sprinkler_machine|180|2";
    if recipes != expect_recipes {
        return Err(format!("forge 配方不符：{recipes:?}\n应为：{expect_recipes:?}").into());
    }
    // 不进任何商店 stock/buys。
    let leaked = query_str(
        &mut runtime,
        "local shops = require('data.shops')\n\
         local stock, buys = {}, {}\n\
         for _, s in pairs(shops) do\n\
           if type(s) == 'table' then\n\
             for _, it in ipairs(s.stock or {}) do stock[it.item] = true end\n\
             for _, id in ipairs(s.buys or {}) do buys[id] = true end\n\
           end\n\
         end\n\
         local bad = {}\n\
         for _, id in ipairs({ 'sprinkler_bamboo', 'sprinkler_waterwheel', 'sprinkler_machine' }) do\n\
           if stock[id] or buys[id] then bad[#bad + 1] = id end\n\
         end\n\
         return table.concat(bad, ',')",
    )?;
    if !leaked.is_empty() {
        return Err(format!("洒水器混入商店 stock/buys：{leaked:?}").into());
    }
    println!("  洒水器 3 级与 forge 配方全部与设计案一致，未泄漏进商店");

    banner("② 进山庄新档，站 (16,3) 朝上给面前格 (16,4) 翻土 + 放竹筒引水器");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 站位 (16,3) 面前 (16,4)（m24 ⑧ 验证过的安全站位，无 NPC 干扰）。写成显式传送而不是
    // "停在出生点"：#28 把出生点挪到 (19,3)，本节要的只是这块药田几何。
    stand_at(&mut runtime, (16, 3), "up")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    add_sprinkler(&mut runtime, "sprinkler_bamboo")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "place_sprinkler_bamboo",
    )?;
    assert_num(&mut runtime, "inv.count('player', 'sprinkler_bamboo')", 0.0)?;
    assert_plot(&mut runtime, (16, 4), "sprinkler_bamboo")?;

    banner("③ 竹筒（十字 4 邻）：下邻被浇成熟，(16,6)/(17,5) 不被浇");
    // 种薄荷 (16,5)（下邻 dy=1）：站 (16,4) 面前 (16,5)
    plant_mint_at(&mut runtime, &host, (16, 5))?;
    // 种薄荷 (16,6)（下 2 格 dy=2，竹筒范围外）：站 (16,5) 面前 (16,6)
    plant_mint_at(&mut runtime, &host, (16, 6))?;
    // 种薄荷 (17,5)（斜角 dx=1 dy=1，十字不浇）：站 (16,5) 朝右面前 (17,5)
    plant_mint_right(&mut runtime, &host, (17, 5))?;
    // 钉晴天睡 4 天：洒水器在每天清晨浇水（种下的当天不计；第 4 次睡眠
    // 才结算第 3 天成长），(16,5) 累计 3 天成长成熟
    for _ in 0..4 {
        sunny_sleep(&mut runtime)?;
    }
    assert_watered_days(&mut runtime, (16, 5), 3.0, "竹筒下邻应累计 3 天成长")?;
    assert_watered_days(&mut runtime, (16, 6), 0.0, "竹筒不浇下 2 格（范围外）")?;
    assert_watered_days(&mut runtime, (17, 5), 0.0, "竹筒十字不浇斜角")?;
    // (16,5) 成熟可收
    stand_at(&mut runtime, (16, 4), "up")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    println!("  竹筒：下邻 3 天成熟收成；下 2 格与斜角均未浇（十字只浇上下左右）");

    banner("④ 水车（3×3）：斜角 (17,5) 被浇成熟");
    // 拆竹筒（站 (16,3) 面前 (16,4)）
    stand_at(&mut runtime, (16, 3), "up")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "remove_sprinkler",
    )?;
    assert_num(&mut runtime, "inv.count('player', 'sprinkler_bamboo')", 1.0)?;
    assert_plot(&mut runtime, (16, 4), "none")?;
    // 放水车
    add_sprinkler(&mut runtime, "sprinkler_waterwheel")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "place_sprinkler_waterwheel",
    )?;
    // 睡 4 天：(17,5) 斜角被水车 3×3 清晨浇 → 3 天成长成熟
    for _ in 0..4 {
        sunny_sleep(&mut runtime)?;
    }
    assert_watered_days(&mut runtime, (17, 5), 3.0, "水车 3×3 应浇斜角")?;
    assert_watered_days(
        &mut runtime,
        (16, 6),
        0.0,
        "水车 radius=1 不浇 (16,6)（dy=2）",
    )?;
    // 收获 (17,5)：站 (16,5) 朝右
    stand_at(&mut runtime, (16, 5), "right")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    println!("  水车：斜角 (17,5) 3 天成熟收成；dy=2 的 (16,6) 仍不浇（radius=1）");

    banner("⑤ 机关（5×5）：远格 (16,6) 被浇成熟（radius=2）");
    // 拆水车、放机关
    stand_at(&mut runtime, (16, 3), "up")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "remove_sprinkler",
    )?;
    add_sprinkler(&mut runtime, "sprinkler_machine")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "place_sprinkler_machine",
    )?;
    // 睡 4 天：(16,6) dy=2 被机关 radius=2 清晨浇 → 3 天成长成熟
    for _ in 0..4 {
        sunny_sleep(&mut runtime)?;
    }
    assert_watered_days(&mut runtime, (16, 6), 3.0, "机关 5×5 应浇 (16,6)（dy=2）")?;
    // 收获 (16,6)：站 (16,5) 朝上
    stand_at(&mut runtime, (16, 5), "up")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    println!("  机关：远格 (16,6) 3 天成熟收成（竹筒/水车都浇不到的格）");

    banner("⑥ 雨天保留：机关雨天清晨照常浇水");
    // (16,6) 收获后进入再生第二茬（watered_days 已重置为 0）
    runtime.execute_console("require('world.weather').set('rainy')")?;
    runtime.execute_console("day.sleep()")?;
    // 雨天清晨机关照常浇水：当天 watered_today=true（晴天/雨天同语义）
    let watered_today = query_str(
        &mut runtime,
        "local p = map.state('farm_plots')['16,6']\n\
         return tostring(p and p.watered_today or false)",
    )?;
    if watered_today != "true" {
        return Err(format!("机关雨天清晨应照常浇水（watered_today={watered_today:?}）").into());
    }
    // 再睡 1 次：雨天浇水次日结算 +1（成长继续累计）
    runtime.execute_console("require('world.weather').set('rainy')")?;
    runtime.execute_console("day.sleep()")?;
    assert_watered_days(&mut runtime, (16, 6), 1.0, "雨天浇水次日应结算 +1")?;
    println!("  雨天清晨机关照常浇水（当日标记 + 次日结算），成长继续累计");

    banner("⑦ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m39_sprinkler_ok project={} specs=3 recipes=3 bamboo=cross/1 wheel=square/1 machine=square/2 rain=ok",
        project.display()
    );
    Ok(())
}

/// 补 1 个洒水器进背包并选中（真实玩家从铁匠铺配方制作获得）。
fn add_sprinkler(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "inv.add(require('core.state').inventory, '{item}', 1)\n\
         require('core.state').selected_sprinkler = '{item}'"
    ))?;
    Ok(())
}

/// 补 1 粒薄荷籽并播种到指定格（站其上方一格朝上；面前格 = y+1，引擎 Up 语义）。
fn plant_mint_at(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    cell: (i64, i64),
) -> Result<(), Box<dyn std::error::Error>> {
    stand_at(runtime, (cell.0, cell.1 - 1), "up")?;
    equip_tool(runtime, "hoe")?;
    press(runtime, host, KeyCode::KeyE)?;
    assert_str(runtime, "require('core.state').last_action", "till")?;
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_mint', 1)")?;
    set_seed(runtime, "seed_mint")?;
    press(runtime, host, KeyCode::KeyE)?;
    assert_str(
        runtime,
        "require('core.state').last_action",
        "plant_seed_mint",
    )?;
    Ok(())
}

/// 补 1 粒薄荷籽并播种到目标格右侧（站目标格左一格朝右）。
fn plant_mint_right(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    cell: (i64, i64),
) -> Result<(), Box<dyn std::error::Error>> {
    stand_at(runtime, (cell.0 - 1, cell.1), "right")?;
    equip_tool(runtime, "hoe")?;
    press(runtime, host, KeyCode::KeyE)?;
    assert_str(runtime, "require('core.state').last_action", "till")?;
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_mint', 1)")?;
    set_seed(runtime, "seed_mint")?;
    press(runtime, host, KeyCode::KeyE)?;
    assert_str(
        runtime,
        "require('core.state').last_action",
        "plant_seed_mint",
    )?;
    Ok(())
}

/// 钉晴天 + 睡觉（跨天；洒水器在 settle_day 清晨自动浇水）。
fn sunny_sleep(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console("require('world.weather').set('sunny')")?;
    runtime.execute_console("day.sleep()")?;
    Ok(())
}

/// 断言指定格地块的 watered_days。
fn assert_watered_days(
    runtime: &mut LuaRuntime,
    cell: (i64, i64),
    expected: f64,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "local m = map.state('farm_plots')\n\
             local p = m['{},{}']\n\
             return tostring(p and p.watered_days or -1)",
            cell.0, cell.1
        ),
    )?
    .parse::<f64>()
    .map_err(|error| -> Box<dyn std::error::Error> {
        format!("watered_days parse failed: {error}").into()
    })?;
    if (got - expected).abs() > f64::EPSILON {
        return Err(format!(
            "{label}：({},{}) watered_days = {got}，应为 {expected}",
            cell.0, cell.1
        )
        .into());
    }
    Ok(())
}

/// 断言指定格地块的 sprinkler 字段（"none" = 无洒水器）。
fn assert_plot(
    runtime: &mut LuaRuntime,
    cell: (i64, i64),
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "local m = map.state('farm_plots')\n\
             local p = m['{},{}']\n\
             return tostring(p and p.sprinkler or 'none')",
            cell.0, cell.1
        ),
    )?;
    if got != expected {
        return Err(format!(
            "地块 ({},{}) sprinkler = {got:?}，应为 {expected:?}",
            cell.0, cell.1
        )
        .into());
    }
    Ok(())
}

/// 站到指定格并面向指定方向（引擎 Up=当前格 y+1，Right=x+1）。
fn stand_at(
    runtime: &mut LuaRuntime,
    cell: (i64, i64),
    facing: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "local pid = require('player.player').ensure()\n\
         transform.set(pid, {}, {})",
        (cell.0 as f64 + 0.5) * 32.0,
        (cell.1 as f64 + 0.5) * 32.0
    ))?;
    set_facing(runtime, facing)?;
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

fn equip_tool(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "inv.equip(require('core.state').inventory, 'tool', '{item}')"
    ))?;
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
