//! Headless acceptance drive for the cooking & kitchen system
//! (料理与厨房, docs/33 §4.7 / docs/54 §4.6, M47 / 1.3).
//!
//! 验证「料理与厨房」真实链路（数据口径 = docs/33 §4.7：膳房 station=kitchen、
//! data/recipes.lua 新厨房配方、data/items_cooking.lua 菜肴 kind=dish）：
//!   1. 新档基线：金币 120 / 体力 100 / 内力 50；
//!   2. 放置膳房（kitchen，2x2）→ last_action=build_kitchen，level=1；
//!      level1 做 2 级药膳配方被拒（station_level）；补木料升到 level2；
//!   3. 做菜（真实 crafting 链路）：扣材料 → clock 推分钟 → 收取，
//!      断言 output 入包、last_action=craft_start_*/craft_collect_*；
//!      cooking.on_collect 记账（做过几道菜 / 图鉴解锁）；
//!   4. 吃菜（inventory.use）：白米粥回体力（40→60），壮骨汤给 defense+4 Buff，
//!      经 stats.query / buff_bonus / buff_count 查询；on_day_start 显式推日，
//!      Buff days 递减、归 0 移除；
//!   5. 菜可入箱/卖出：braised_pork（sell=40）shipping.add → settle 入账；
//!   6. cooking capture/restore 往返；存档快照往返；
//!   7. systems.cooking.validate_all 空 + validate_data strict 空。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m47_cooking [-- <game_jhyt-root>]

use engine::core::script::LuaRuntime;
use std::path::Path;
use std::{env, fs, path::PathBuf};

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
    if !main_script.is_file() {
        return Err(format!("missing {}", main_script.display()).into());
    }
    let save_path = fresh_save_path(&project, "jhyt_m47_cooking.sav");

    banner("① 新档基线：金币 120 / 体力 100 / 内力 50");
    let mut runtime = new_runtime(&project, &save_path)?;
    runtime.execute_project_script(&main_script)?;
    runtime.execute_console("scene.load('main')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "main")?;
    runtime.execute_console("on_click('BtnStart')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_mode(&mut runtime, "Playing")?;
    assert_num(&mut runtime, "require('core.state').money", 120.0)?;
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    assert_num(&mut runtime, "require('core.state').inner_power", 50.0)?;

    banner("② 放置膳房 kitchen（2x2）→ level1；level1 做 2 级药膳被拒 station_level");
    let (kx, ky) = find_legal_anchor(&mut runtime, "kitchen")?;
    let key = format!("{kx},{ky}");
    println!("  legal kitchen anchor: {kx},{ky}");
    lua(
        &mut runtime,
        &format!("require('systems.building').place('kitchen', {kx}, {ky})"),
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "build_kitchen",
    )?;
    assert_num(
        &mut runtime,
        &format!(
            "(function() for _, e in ipairs(require('systems.building').list()) do if e.key == '{key}' then return e.level end end return -1 end)()"
        ),
        1.0,
    )?;
    // level1 膳房做 2 级药膳（壮骨汤 min_level=2）→ station_level（先验等级后验材料）。
    assert_str(
        &mut runtime,
        &format!(
            "select(2, require('systems.crafting').start('kitchen_bone_strength_soup', '{key}'))"
        ),
        "station_level",
    )?;
    // 无材料再做一次 1 级粥（baseline rice=2 已被下面吃掉？这里先测 not_enough）。
    // 补木料升到 level2（upgrade_cost timber×4）。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'timber', 4)",
    )?;
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.building').upgrade({kx}, {ky})))"),
        "true",
    )?;
    assert_num(
        &mut runtime,
        &format!(
            "(function() for _, e in ipairs(require('systems.building').list()) do if e.key == '{key}' then return e.level end end return -1 end)()"
        ),
        2.0,
    )?;
    println!("  膳房已升至 level2，解锁 2 级药膳配方");

    banner("③ 做菜（真实 crafting 链路）：白米粥 扣米→推钟→收取，cooking 记账");
    // baseline rice=2，正好喂白米粥（rice×2）。
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'rice')",
        2.0,
    )?;
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').start('kitchen_rice_congee', '{key}')))"),
        "true",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "craft_start_kitchen_rice_congee",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'rice')",
        0.0,
    )?;
    // 推钟到 minute 30（粥 20 分钟，done=20）→ 收取。
    lua(&mut runtime, "clock.set(1, 30)")?;
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').collect('{key}')))"),
        "true",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'rice_congee')",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "craft_collect_rice_congee",
    )?;
    // 厨房已空闲、米已耗尽：再做 → not_enough（失败无副作用、不写 last_action）。
    assert_str(
        &mut runtime,
        &format!("select(2, require('systems.crafting').start('kitchen_rice_congee', '{key}'))"),
        "not_enough",
    )?;
    // cooking 记账（真实链路收取后走同一函数）。
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('systems.cooking').on_collect('rice_congee', 'kitchen_rice_congee'); return tostring(ok) .. '|' .. tostring(why) end)()",
        "true|rice_congee",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.cooking').times('rice_congee')",
        1.0,
    )?;
    assert_num(&mut runtime, "require('systems.cooking').total()", 1.0)?;
    println!("  白米粥入包 +1、cooking 记账 1 道");

    banner("④ 吃菜：白米粥回体力（40→60）；壮骨汤 defense+4 Buff，on_day_start 推日过期");
    lua(&mut runtime, "require('core.state').energy = 40")?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.inventory').use('rice_congee'); return tostring(ok) .. '|' .. tostring(why) end)()",
        "true|ok",
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 60.0)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "use_rice_congee",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'rice_congee')",
        0.0,
    )?;

    // 2 级药膳：壮骨汤（boar_meat×2 + gouqi×1，lv2）。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'boar_meat', 2)",
    )?;
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'gouqi', 1)",
    )?;
    assert_str(
        &mut runtime,
        &format!(
            "tostring((require('systems.crafting').start('kitchen_bone_strength_soup', '{key}')))"
        ),
        "true",
    )?;
    lua(&mut runtime, "clock.set(1, 200)")?; // start@minute30 +120 = done@150 < 200
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').collect('{key}')))"),
        "true",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'bone_strength_soup')",
        1.0,
    )?;
    lua(
        &mut runtime,
        "require('systems.cooking').on_collect('bone_strength_soup', 'kitchen_bone_strength_soup')",
    )?;

    // 吃壮骨汤 → defense+4（days=2）。
    let def_before = query_num(
        &mut runtime,
        "return require('player.stats').query('player','defense')",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.inventory').use('bone_strength_soup'); return tostring(ok) .. '|' .. tostring(why) end)()",
        "true|ok",
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').buff_bonus('player','defense')",
        4.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','defense')",
        def_before + 4.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').buff_count('player')",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "use_bone_strength_soup",
    )?;
    println!(
        "  壮骨汤 Buff 生效：defense {def_before}→{}",
        def_before + 4.0
    );

    // on_day_start 显式推日：days 2→1，Buff 仍在（cooking.on_day_start 返回 0）。
    assert_num(
        &mut runtime,
        "require('systems.cooking').on_day_start()",
        0.0,
    )?;
    assert_num(&mut runtime, "require('player.stats').on_day_start()", 0.0)?;
    assert_num(
        &mut runtime,
        "require('player.stats').buff_bonus('player','defense')",
        4.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').buff_count('player')",
        1.0,
    )?;
    // 再推一日：days 1→0，Buff 移除（stats.on_day_start 返回移除数 1）。
    assert_num(&mut runtime, "require('player.stats').on_day_start()", 1.0)?;
    assert_num(
        &mut runtime,
        "require('player.stats').buff_bonus('player','defense')",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').buff_count('player')",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','defense')",
        def_before,
    )?;
    println!("  on_day_start 推日：Buff days 递减、归 0 移除 ✓");

    banner("⑤ 菜可入箱/卖出：braised_pork（sell=40）入箱 → 睡前结算入账");
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'braised_pork', 1)",
    )?;
    let money_before = query_num(&mut runtime, "require('core.state').money")?;
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.shipping').add('braised_pork', 1); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "ship_braised_pork",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').pending()['braised_pork']",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "(function() local s = require('systems.shipping').settle(); return s and s.total or 0 end)()",
        40.0,
    )?;
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_before + 40.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shipping').stats()['braised_pork']",
        1.0,
    )?;
    println!("  红烧肉入箱结算 +40 文");

    banner("⑥ cooking capture/restore 往返 + 存档快照往返");
    let cooked_total = query_num(&mut runtime, "require('systems.cooking').total()")?;
    lua(
        &mut runtime,
        "__cook_snap = require('systems.cooking').capture()",
    )?;
    lua(&mut runtime, "require('systems.cooking').reset()")?;
    assert_num(&mut runtime, "require('systems.cooking').total()", 0.0)?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('systems.cooking').restore(__cook_snap); return tostring(ok) .. '|' .. tostring(why) end)()",
        "true|nil",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.cooking').total()",
        cooked_total,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.cooking').times('rice_congee')",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.cooking').times('bone_strength_soup')",
        1.0,
    )?;
    // 图鉴：两道菜已解锁。
    assert_num(
        &mut runtime,
        "#(require('systems.cooking').discovered())",
        2.0,
    )?;
    // 存档快照往返（save_manager）。
    lua(
        &mut runtime,
        "__b_captured = require('core.save_manager').capture()",
    )?;
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'braised_pork', 5)",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('core.save_manager').restore(__b_captured); return tostring(ok) .. '|' .. tostring(why) end)()",
        "true|nil",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.cooking').total()",
        cooked_total,
    )?;
    println!("  cooking 快照/restore 与存档往返一致（累计 {cooked_total} 道）");

    banner("⑦ 数据校验门：systems.cooking.validate_all 空 + validate_data strict 空");
    let cook_report = query_str(
        &mut runtime,
        "(function() local ok, issues = require('systems.cooking').validate_all(); if not ok then return table.concat(issues, '|') end; return '' end)()",
    )?;
    if !cook_report.is_empty() {
        return Err(format!("cooking.validate_all 不通过:\n{cook_report}").into());
    }
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m47_cooking_ok project={} kitchen={},{} level=2 dishes_cooked={} recipes={} ship=braised_pork:+40",
        project.display(),
        kx,
        ky,
        cooked_total,
        query_num(&mut runtime, "return #require('data.recipes').ids()")?
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers ----------

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

/// 扫描当前地图，返回第一个对该建筑可放置的锚点。
fn find_legal_anchor(
    runtime: &mut LuaRuntime,
    building_id: &str,
) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let expr = format!(
        "(function() local b = require('systems.building'); \
         for y = 0, 21 do for x = 0, 39 do \
           local ok = b.can_place('{building_id}', x, y); \
           if ok then return x .. ',' .. y end end end return 'none' end)()"
    );
    let got = query_str(runtime, &expr)?;
    let mut parts = got.split(',');
    let x = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no legal anchor for {building_id}: {got}"))?;
    let y = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no legal anchor for {building_id}: {got}"))?;
    Ok((x, y))
}

fn lua(runtime: &mut LuaRuntime, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(code)?;
    Ok(())
}

fn query_str(runtime: &mut LuaRuntime, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(expr)?.trim().to_owned())
}

fn query_num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let value = query_str(runtime, expr)?;
    value
        .parse::<f64>()
        .map_err(|e| format!("{value:?} not a number: {e}").into())
}

fn assert_num(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_num(runtime, &format!("return {expr}"))?;
    if (got - expected).abs() > 1e-6 {
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
    let scene = query_str(runtime, "scene.reload()")?;
    if scene != expected {
        return Err(format!("scene = {scene:?}, want {expected:?}").into());
    }
    Ok(())
}

fn assert_mode(runtime: &mut LuaRuntime, expected: &str) -> Result<(), Box<dyn std::error::Error>> {
    assert_str(runtime, "require('core.state').mode", expected)
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}

fn fresh_save_path(project: &Path, name: &str) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(name);
    let _ = fs::remove_file(&path);
    path
}
