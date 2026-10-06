//! Headless acceptance drive for fruit trees (改造计划 §4.2 / docs/33 §4.1.5, M40).
//!
//! 验证果树 5 种的真实耕作链路（数据口径 = docs/33 §4.1.5 果树表 ==
//! data/crops.lua 树条目 == data/items.lua 树苗/树果 == 货摊/送礼配置）：
//!   1. 静态：5 树条目（tree=true、tree_days=15、fruit_season=autumn、land=field）、
//!      5 树苗（kind=seed、sell=0）、5 果（kind=fruit、无品质派生）、树苗货摊
//!      售卖、树果货摊收购与可送礼（gift_points > 0）；
//!   2. 桃树苗种下 15 天成树（期间从不浇水）；
//!   3. 非秋季按 E 提示"未到果期"（tree_not_fruiting）；
//!   4. 秋季每天结 1 果（harvest_peach），当日已采提示 tree_picked，次日可再收；
//!   5. 树格 watered_days 恒 0（树不浇水语义）且 tree_days 持续累计；
//!   6. 跨年后的下一个秋季仍结果（年年结果）；
//!   7. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! 口径：season_length=28（data/festivals.lua），一季 28 天；树不依赖天气与浇水，
//! 样例直接 day.sleep 跨天/跨季（日历无 set API）。field 区 (24,4) 是 m24 验证过的
//! 可种格（普通田）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m40_fruit_tree [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
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

    banner("① 静态断言：5 树条目/5 树苗/5 果/树果不派生/货摊/送礼");
    let trees = query_str(
        &mut runtime,
        "local crops = require('data.crops')\n\
         local items = require('data.items')\n\
         local out = {}\n\
         for _, id in ipairs({ 'peach', 'mume', 'loquat', 'jujube', 'ginkgo' }) do\n\
           local c = crops[id]\n\
           local seed = c and items[c.seed_item] or nil\n\
           local fruit = c and items[c.harvest_item] or nil\n\
           out[#out + 1] = id .. '|' .. tostring(c and c.tree or false) .. '|' .. tostring(c and c.tree_days or -1) .. '|' .. tostring(c and c.fruit_season or 'none') .. '|' .. tostring(c and c.land or 'none') .. '|' .. tostring(seed and seed.kind or 'none') .. '|' .. tostring(seed and seed.buy or -1) .. '|' .. tostring(seed and seed.sell or -1) .. '|' .. tostring(fruit and fruit.kind or 'none') .. '|' .. tostring(fruit and fruit.sell or -1) .. '|' .. tostring(fruit and fruit.buy or -1)\n\
         end\n\
         return table.concat(out, ';')",
    )?;
    let expect_trees = "peach|true|15|autumn|field|seed|150|0|fruit|25|0;".to_owned()
        + "mume|true|15|autumn|field|seed|120|0|fruit|20|0;"
        + "loquat|true|15|autumn|field|seed|180|0|fruit|30|0;"
        + "jujube|true|15|autumn|field|seed|100|0|fruit|15|0;"
        + "ginkgo|true|15|autumn|field|seed|300|0|fruit|45|0";
    if trees != expect_trees {
        return Err(format!("果树条目不符：\n{trees}\n应为：\n{expect_trees}").into());
    }
    // 树果无品质派生（fruit 不走构建式 <id>_good/_super）
    let derived = query_str(
        &mut runtime,
        "local items = require('data.items')\n\
         local bad = {}\n\
         for _, id in ipairs({ 'peach', 'mume', 'loquat', 'jujube', 'ginkgo_fruit' }) do\n\
           if items[id .. '_good'] or items[id .. '_super'] then bad[#bad + 1] = id end\n\
         end\n\
         return table.concat(bad, ',')",
    )?;
    if !derived.is_empty() {
        return Err(format!("树果不应有品质派生条目：{derived:?}").into());
    }
    // 树苗在货摊 stock、树果在货摊 buys
    let shop = query_str(
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
         for _, id in ipairs({ 'seed_peach', 'seed_mume', 'seed_loquat', 'seed_jujube', 'seed_ginkgo' }) do\n\
           if not stock[id] then bad[#bad + 1] = 'stock:' .. id end\n\
         end\n\
         for _, id in ipairs({ 'peach', 'mume', 'loquat', 'jujube', 'ginkgo_fruit' }) do\n\
           if not buys[id] then bad[#bad + 1] = 'buys:' .. id end\n\
         end\n\
         return table.concat(bad, ',')",
    )?;
    if !shop.is_empty() {
        return Err(format!("树苗/树果商店配置缺失：{shop:?}").into());
    }
    // 树果可送礼（gift_points > 0）
    let gift = query_num(
        &mut runtime,
        "require('data.relationships').gift_points('npc_lu_guizang', 'ginkgo_fruit')",
    )?;
    if gift <= 0.0 {
        return Err(format!("银杏果应可送礼（gift_points = {gift}，应 > 0）").into());
    }
    let gift_peach = query_num(
        &mut runtime,
        "require('data.relationships').gift_points('npc_a_qi', 'peach')",
    )?;
    if gift_peach <= 0.0 {
        return Err(format!("桃子应可送礼（gift_points = {gift_peach}，应 > 0）").into());
    }
    println!("  果树 5 种/树苗/树果/商店/送礼全部与设计案一致，树果无品质派生");

    banner("② 进山庄新档，field 区 (24,4) 种桃树苗");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 站 (24,5) 朝下（引擎 Down = 当前格 y-1，面前格 = (24,4)），field land 区
    // （m24 验证过的可种格）
    stand_at(&mut runtime, (24, 5), "down")?;
    equip_tool(&mut runtime, "hoe")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_peach', 1)")?;
    set_seed(&mut runtime, "seed_peach")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_peach",
    )?;
    assert_num(&mut runtime, "inv.count('player', 'seed_peach')", 0.0)?;

    banner("③ 成树 15 天（不浇水），期间按 E 提示树苗尚幼");
    // 树成熟前：E → tree_growing（不依赖浇水）
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "tree_growing",
    )?;
    for _ in 0..15 {
        runtime.execute_console("day.sleep()")?;
    }
    // 成树后非秋季：E → tree_not_fruiting（此时仍在春季）
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "tree_not_fruiting",
    )?;
    // 树格：watered_days 恒 0（树不浇水），tree_days >= 15（成树）
    let tree_state = query_str(
        &mut runtime,
        "local p = map.state('farm_plots')['24,4']\n\
         return tostring(p and p.crop or 'none') .. '|' .. tostring(p and p.watered_days or -1) .. '|' .. tostring(p and p.tree_days or -1)",
    )?;
    let parts: Vec<&str> = tree_state.split('|').collect();
    if parts.len() != 3 || parts[0] != "peach" {
        return Err(format!("树格状态不符：{tree_state:?}").into());
    }
    if parts[1] != "0" {
        return Err(format!("树格不应累计浇水天数（watered_days={}）", parts[1]).into());
    }
    let tree_days: i64 = parts[2].parse().map_err(|e| format!("{e}"))?;
    if tree_days < 15 {
        return Err(format!("15 次睡眠后 tree_days = {tree_days}，应 >= 15（成树）").into());
    }
    println!("  桃树 15 天成树、从不浇水（watered_days=0）、春季非果期不结果");

    banner("④ 秋季每天 1 果：当日限次 + 次日可再收");
    // 从春季睡到秋季（日历无 set API；season 由累计日数 28 天一季推导，
    // 与 day 起始无关——循环至 autumn 才停，上限 100 次）
    let mut guard = 0;
    loop {
        let season = query_str(
            &mut runtime,
            "return require('world.calendar').season_of(clock.now().day)",
        )?;
        if season == "autumn" {
            break;
        }
        guard += 1;
        if guard > 100 {
            return Err(format!("100 次睡眠仍未到秋季（season={season:?}）").into());
        }
        runtime.execute_console("day.sleep()")?;
    }
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_peach",
    )?;
    assert_num(&mut runtime, "inv.count('player', 'peach')", 1.0)?;
    // 当日已采
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "tree_picked",
    )?;
    assert_num(&mut runtime, "inv.count('player', 'peach')", 1.0)?;
    // 次日可再收
    runtime.execute_console("day.sleep()")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_peach",
    )?;
    assert_num(&mut runtime, "inv.count('player', 'peach')", 2.0)?;
    println!("  秋季当天 1 果、当日限次、次日可再收（共 2 果）");

    banner("⑤ 跨年后的下一个秋季仍结果（年年结果）");
    // 冬季有"冬至封山"（docs/33：winter 15–17 天封山），固定次数的睡眠会因封山
    // 合并/跳日而漂移——改为循环睡到"下一个秋季的同一季内日"（同季同日必已跨年，
    // 同年不会重复出现秋季），上限 200 次
    let last_day_in_season = query_num(
        &mut runtime,
        "require('world.calendar').day_in_season(clock.now().day)",
    )?;
    // 先睡 1 天离开当前秋季日（否则循环起点即满足"同季同日"，0 次睡眠就 break，
    // 树当日已采会返回 tree_picked 而不是跨年后的收成）
    runtime.execute_console("day.sleep()")?;
    let mut guard = 0;
    loop {
        let season = query_str(
            &mut runtime,
            "return require('world.calendar').season_of(clock.now().day)",
        )?;
        let d = query_num(
            &mut runtime,
            "require('world.calendar').day_in_season(clock.now().day)",
        )?;
        if season == "autumn" && (d - last_day_in_season).abs() < f64::EPSILON {
            break;
        }
        guard += 1;
        if guard > 200 {
            return Err(format!("200 次睡眠仍未到下一秋（season={season:?} day={d}）").into());
        }
        runtime.execute_console("day.sleep()")?;
    }
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_peach",
    )?;
    assert_num(&mut runtime, "inv.count('player', 'peach')", 3.0)?;
    println!("  跨年后的下一个秋季照常结果（第 3 果）——树跨年保留、年年结果");

    banner("⑥ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m40_fruit_tree_ok project={} trees=5 saplings=5 fruits=5 grow=15d season=autumn daily=1 yearly=ok",
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
    runtime.execute_console(&format!("require('core.state').facing = '{facing}'"))?;
    Ok(())
}

fn equip_tool(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "inv.equip(require('core.state').inventory, 'tool', '{item}')"
    ))?;
    Ok(())
}

fn set_seed(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('core.state').selected_seed = '{item}'"))?;
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
