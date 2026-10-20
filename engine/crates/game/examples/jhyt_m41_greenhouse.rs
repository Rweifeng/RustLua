//! Headless acceptance drive for the greenhouse (地热药圃, 改造计划 §4.2 /
//! docs/33 §4.1.6, M41).
//!
//! 验证温室（地热药圃）的真实链路（数据口径 = docs/33 §4.1.6 建筑两档
//! == data/buildings.lua greenhouse == data/farm.lua 温室田区划）：
//!   1. 静态：greenhouse 建筑（station=nil、2×2、2 级、levels[2].greenhouse_bonus=1、
//!      升级成本木材 6）；data/farm.lua land 含温室田 rect（x28..33,y8..13）且
//!      置于 field 区之前（重叠区优先命中 greenhouse）；
//!   2. 未建地热药圃：温室田 (28,9) 种冬参（winter 作物、land=medicinal）
//!      提示 wrong_season——land 放宽已生效（若 land 未跳过会先 wrong_land）；
//!   3. 建地热药圃（can_place 扫描合法锚点 + place）：同格 (28,9) 种冬参成功
//!      （1 级解锁四季可种），浇水 8 天成熟收获；
//!   4. 温室田接受任何土地作物：medicinal 作物薄荷种温室田 (28,10) 成功；
//!   5. 药圃 2 级（木材 6 + upgrade）：温室田 (28,13) 薄荷收获 +1（yield 2→3），
//!      普通田 (24,4) 薄荷仍 2 个（bonus 只作用于温室田）；
//!   6. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m41_greenhouse [-- <game_jhyt-root>]

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

    banner("① 静态断言：地热药圃建筑两档 == docs/33 §4.1.6，温室田区划优先于 field");
    let bld = query_str(
        &mut runtime,
        "local b = require('data.buildings')\n\
         local g = b.get('greenhouse')\n\
         local max_level = b.max_level(g)\n\
         return tostring(g.w) .. 'x' .. tostring(g.h) .. '|' .. tostring(g.station) .. '|' .. tostring(max_level) .. '|' .. tostring(#g.levels) .. '|' .. tostring(g.upgrade_cost[1].item) .. '=' .. tostring(g.upgrade_cost[1].count)",
    )?;
    let expect_bld = "2x2|nil|2|2|timber=6";
    if bld != expect_bld {
        return Err(format!("greenhouse 建筑条目不符：{bld:?}\n应为：{expect_bld:?}").into());
    }
    // 温室田区划：x28..33,y8..13，且索引（2）在 field（3）之前——重叠区优先命中。
    let land = query_str(
        &mut runtime,
        "local farm = require('data.farm')\n\
         local gh, fi = nil, nil\n\
         for i, rect in ipairs(farm.land) do\n\
           if rect.type == 'greenhouse' then gh = i end\n\
           if rect.type == 'field' then fi = i end\n\
         end\n\
         local r = farm.land[gh]\n\
         return tostring(gh) .. '|' .. tostring(fi) .. '|' .. tostring(r and r.x0) .. ',' .. tostring(r and r.x1) .. ',' .. tostring(r and r.y0) .. ',' .. tostring(r and r.y1)",
    )?;
    let expect_land = "2|3|28,33,8,13";
    if land != expect_land {
        return Err(format!("温室田区划不符：{land:?}\n应为：{expect_land:?}").into());
    }
    println!("  greenhouse 建筑 2×2/station=nil/2 级/木材6；温室田区划优先于 field ✓");

    banner("② 进山庄新档，温室田 (28,9)：未建药圃时种冬参被季节拦截（land 放宽已生效）");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 翻土 (28,9)：站 (28,8) 朝 up → 面前 (28,9)（引擎 facing_cell(Up) = y+1）
    till_at(&mut runtime, &host, (28, 9))?;
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_winter_shen', 1)")?;
    set_seed(&mut runtime, "seed_winter_shen")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "wrong_season",
    )?;
    println!(
        "  未建药圃：冬参（winter）在温室田被 wrong_season 拦截（若是 wrong_land 说明 land 检查未放宽）"
    );

    banner("③ 建地热药圃（can_place 扫描合法锚点）→ 同格 (28,9) 种冬参成功（1 级解锁四季可种）");
    let anchor = query_str(
        &mut runtime,
        "(function() local b = require('systems.building'); for y = 0, 21 do for x = 0, 39 do \
           if b.can_place('greenhouse', x, y) then return x .. ',' .. y end end end return 'none' end)()",
    )?;
    if anchor == "none" {
        return Err("manor 无可放 greenhouse 的锚点".into());
    }
    let (ax, ay) = anchor
        .split_once(',')
        .ok_or_else(|| format!("anchor {anchor:?} 格式不符"))?;
    let ax: i64 = ax.parse()?;
    let ay: i64 = ay.parse()?;
    lua(
        &mut runtime,
        &format!("require('systems.building').place('greenhouse', {ax}, {ay})"),
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "build_greenhouse",
    )?;
    // 无木材升级被拒且不扣材料（not_enough 路径）
    assert_num(
        &mut runtime,
        &format!(
            "(function() local ok = require('systems.building').upgrade({ax}, {ay}); return ok and 1 or 0 end)()"
        ),
        0.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "not_enough",
    )?;
    // 补木材 6 升到 2 级（样例把药圃直接升满，覆盖 2 级 bonus 链路）
    lua(
        &mut runtime,
        &format!(
            "inv.add(require('core.state').inventory, 'timber', 6)\n\
             require('systems.building').upgrade({ax}, {ay})"
        ),
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "upgrade_greenhouse_2",
    )?;
    // 药圃就位：greenhouse_level() >= 2（样例把药圃直接升满，覆盖 2 级 bonus 链路）
    assert_num(
        &mut runtime,
        "(function() local b = require('systems.building'); \
        for _, r in ipairs(b.list()) do if r.building == 'greenhouse' then return r.level end end return 0 end)()",
        2.0,
    )?;
    // 同格 (28,9) 再种冬参（刚才 wrong_season 未消耗种子）
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_winter_shen",
    )?;
    let days = grow_crop(&mut runtime, &host, "water_winter_shen")?;
    if days != 8 {
        return Err(format!("温室冬参应 8 天成熟，实际 {days} 天").into());
    }
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_winter_shen",
    )?;
    let total = total_crop(&mut runtime, "winter_shen")?;
    if (total - 2.0).abs() > f64::EPSILON {
        return Err(format!("温室 2 级冬参应收 2（yield 1 + 药圃深耕 +1），实际 {total}").into());
    }
    println!("  药圃建好（2 级）后冬参四季可种：8 天成熟、收 {total} 个（yield 1 + 深耕 +1）");

    banner("④ 温室田接受任何土地作物：medicinal 薄荷种温室田 (28,10) 成功");
    till_at(&mut runtime, &host, (28, 10))?;
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_mint', 1)")?;
    set_seed(&mut runtime, "seed_mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_mint",
    )?;
    println!("  薄荷（land=medicinal）种温室田成功（任何土地类型均可入温室）");

    banner("⑤ 药圃 2 级 bonus：温室田薄荷 +1（2→3），普通药田仍 2（增量断言）");
    // 温室田 (28,13)：站 (28,12) 朝 up → 面前 (28,13)
    till_at(&mut runtime, &host, (28, 13))?;
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_mint', 1)")?;
    set_seed(&mut runtime, "seed_mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_mint",
    )?;
    let days = grow_crop(&mut runtime, &host, "water_mint")?;
    if days != 3 {
        return Err(format!("温室薄荷应 3 天成熟，实际 {days} 天").into());
    }
    let gh_before = total_crop(&mut runtime, "mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    let gh_gain = total_crop(&mut runtime, "mint")? - gh_before;
    if (gh_gain - 3.0).abs() > f64::EPSILON {
        return Err(format!("温室 2 级薄荷应 +3（yield 2 + bonus 1），实际 +{gh_gain}").into());
    }
    // 普通药田 (16,4)（medicinal 区、m38 ⑤ 验证过的安全格）：无温室 bonus
    till_at(&mut runtime, &host, (16, 4))?;
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_mint', 1)")?;
    set_seed(&mut runtime, "seed_mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_mint",
    )?;
    let days = grow_crop(&mut runtime, &host, "water_mint")?;
    if days != 3 {
        return Err(format!("普通田薄荷应 3 天成熟，实际 {days} 天").into());
    }
    let field_before = total_crop(&mut runtime, "mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    let field_gain = total_crop(&mut runtime, "mint")? - field_before;
    if (field_gain - 2.0).abs() > f64::EPSILON {
        return Err(format!("普通田薄荷应 +2（无 bonus），实际 +{field_gain}").into());
    }
    println!("  温室 2 级薄荷 +{gh_gain}（2→3）、普通药田薄荷 +{field_gain}（2，无 bonus）");

    banner("⑥ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m41_greenhouse_ok project={} bld=2x2/2lv bonus=1 land=28,33|8,13 locked=wrong_season unlocked=plant_winter_shen grow=8d any_land=mint bonus=+1",
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

fn equip_tool(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "inv.equip(require('core.state').inventory, 'tool', '{item}')"
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

/// 浇水直到成熟（装备水壶、钉晴天、断言水动作、sleep）；返回浇水的游戏日数。
fn grow_crop(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    water_action: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    equip_tool(runtime, "watering_can")?;
    for day in 1..=10 {
        runtime.execute_console("require('world.weather').set('sunny')")?;
        press(runtime, &host, KeyCode::KeyE)?;
        let action = query_str(runtime, "require('core.state').last_action")?;
        if action != water_action {
            return Err(format!(
                "浇水失败，last_action = {action:?}（第 {day} 天，期望 {water_action:?}）"
            )
            .into());
        }
        runtime.execute_console("day.sleep()")?;
        let mature = query_str(
            runtime,
            "local m = map.state('farm_plots')\n\
             for _, p in pairs(m) do\n\
               if p.crop then\n\
                 local c = require('data.crops')[p.crop]\n\
                 if c then\n\
                   local need = (c.regrow_days and p.cycles and p.cycles > 0) and c.regrow_days or c.growth_days\n\
                   if p.fertilizer == 'bone_fertilizer' and need > 1 then need = need - 1 end\n\
                   if p.watered_days >= need then return 'yes' end\n\
                 end\n\
               end\n\
             end\n\
             return 'no'",
        )?;
        if mature == "yes" {
            return Ok(day);
        }
        if day >= 10 {
            return Err("作物 10 天未成熟，成长/浇水判定可能被破坏".into());
        }
    }
    unreachable!()
}

/// 收获物背包总量 = 普通 + 良好 + 上品三条目之和（品质系统：数量断言必须合并统计）。
fn total_crop(runtime: &mut LuaRuntime, crop_id: &str) -> Result<f64, Box<dyn std::error::Error>> {
    query_num(
        runtime,
        &format!(
            "inv.count('player', '{crop_id}') + inv.count('player', '{crop_id}_good') + inv.count('player', '{crop_id}_super')"
        ),
    )
}

/// 移动到指定格上方一格并朝上，翻土一次；断言 last_action == till。
/// 引擎 `grid.facing_cell(Up)` = 当前格 y+1（屏幕 y 向下），站 (cx, cy-1) 面前才是 (cx,cy)。
fn till_at(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    cell: (i64, i64),
) -> Result<(), Box<dyn std::error::Error>> {
    equip_tool(runtime, "hoe")?;
    runtime.execute_console(&format!(
        "local pid = require('player.player').ensure()\n\
         transform.set(pid, {}, {})",
        (cell.0 as f64 + 0.5) * 32.0,
        (cell.1 as f64 - 0.5) * 32.0
    ))?;
    set_facing(runtime, "up")?;
    press(runtime, &host, KeyCode::KeyE)?;
    assert_str(runtime, "require('core.state').last_action", "till")?;
    Ok(())
}
