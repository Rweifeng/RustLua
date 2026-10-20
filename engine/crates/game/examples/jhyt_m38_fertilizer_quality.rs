//! Headless acceptance drive for fertilizers & crop quality (改造计划 §4.2 /
//! docs/33 §4.1.2/§4.1.3, M38).
//!
//! 验证肥料 3 级 + 收获品质的真实链路（数据口径 = docs/33 §4.1.2 品质三档
//! 70/25/5、系数 1.0/1.25/1.6 == data/items.lua 派生条目 == docs/33 §4.1.3
//! 肥料效果表 == data/shops.lua zhao_stall 售价）：
//!   1. 静态：
//!      - 3 种肥料条目（kind=fertilizer、effect 三选一、售价 20/25/30、sell=0 不进 buys）；
//!      - 品质派生条目存在且 sell = floor(基础 × 1.25/1.6)、buy=0、不进任何商店 stock/buys；
//!   2. 骨肥：薄荷 growth 3→2 天成熟（施肥消耗 1 个肥料），收获总量 == yield；
//!      收获后非再生地块清空 ⇒ 地块 fertilizer 归 nil；
//!   3. 灵肥：薄荷单茬 2 → 2+floor(2×0.5)=3 个；
//!   4. 草药肥：薄荷收获全为良好/上品（保底良好，普通档并入良好），
//!      再生第二茬（地块保留肥料）同样全为良好/上品；
//!   5. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! 对照口径：不施肥地块的品质是随机 70/25/5，收获数量断言一律统计
//! 基础+良好+上品三条目总量（`total_crop`，与 m24/m36_regrow 同法）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m38_fertilizer_quality [-- <game_jhyt-root>]

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

    banner("① 静态断言：肥料条目/售价/effect == docs/33 §4.1.3，品质派生 == §4.1.2");
    // 肥料：kind=fertilizer、buy=售价、sell=0、effect 三选一。
    let fert = query_str(
        &mut runtime,
        "local items = require('data.items')\n\
         local ids = { 'herb_fertilizer', 'bone_fertilizer', 'spirit_fertilizer' }\n\
         local out = {}\n\
         for _, id in ipairs(ids) do\n\
           local d = items[id]\n\
           out[#out + 1] = id .. '|' .. tostring(d and d.buy or -1) .. '|' .. tostring(d and d.sell or -1) .. '|' .. tostring(d and d.effect or 'none')\n\
         end\n\
         return table.concat(out, ';')",
    )?;
    let expect_fert =
        "herb_fertilizer|20|0|quality;bone_fertilizer|25|0|growth;spirit_fertilizer|30|0|yield";
    if fert != expect_fert {
        return Err(format!("肥料条目不符：{fert:?}\n应为：{expect_fert:?}").into());
    }
    // zhao_stall 售卖 3 种肥料（价格同 §5.2）。
    let stall = query_str(
        &mut runtime,
        "local s = require('data.shops').zhao_stall\n\
         local out = {}\n\
         for _, it in ipairs(s.stock or {}) do\n\
           if tostring(it.item):find('fertilizer') then\n\
             out[#out + 1] = it.item .. '=' .. it.price .. 'x' .. it.limit\n\
           end\n\
         end\n\
         table.sort(out)\n\
         return table.concat(out, ',')",
    )?;
    let expect_stall = "bone_fertilizer=25x10,herb_fertilizer=20x10,spirit_fertilizer=30x5";
    if stall != expect_stall {
        return Err(format!("zhao_stall 肥料不符：{stall:?}，应为 {expect_stall:?}").into());
    }
    // 品质派生：mint(12) → good 15 / super 19；rice(30) → good 37 / super 48。
    let quality = query_str(
        &mut runtime,
        "local items = require('data.items')\n\
         local out = {}\n\
         for _, id in ipairs({ 'mint_good','mint_super','rice_good','rice_super' }) do\n\
           local d = items[id]\n\
           out[#out + 1] = id .. '=' .. tostring(d and d.sell or -1) .. ':' .. tostring(d and d.buy or -1) .. ':' .. tostring(d and d.kind or 'none')\n\
         end\n\
         return table.concat(out, ';')",
    )?;
    let expect_quality =
        "mint_good=15:0:crop;mint_super=19:0:crop;rice_good=37:0:crop;rice_super=48:0:crop";
    if quality != expect_quality {
        return Err(format!("品质派生条目不符：{quality:?}\n应为：{expect_quality:?}").into());
    }
    // 品质条目不进任何商店 stock/buys（上品专属用途，不是套利通道）。
    let leaked = query_str(
        &mut runtime,
        "local items = require('data.items')\n\
         local shops = require('data.shops')\n\
         local stock, buys = {}, {}\n\
         for _, s in pairs(shops) do\n\
           if type(s) == 'table' then\n\
             for _, it in ipairs(s.stock or {}) do stock[it.item] = true end\n\
             for _, id in ipairs(s.buys or {}) do buys[id] = true end\n\
           end\n\
         end\n\
         local bad = {}\n\
         for id, d in pairs(items) do\n\
           if d.kind == 'crop' and tostring(id):find('_good$') or tostring(id):find('_super$') then\n\
             if stock[id] or buys[id] then bad[#bad + 1] = id end\n\
           end\n\
         end\n\
         return table.concat(bad, ',')",
    )?;
    if !leaked.is_empty() {
        return Err(format!("品质条目混入商店 stock/buys：{leaked:?}").into());
    }
    println!("  肥料 3 级与品质派生全部与设计案一致，品质条目未泄漏进商店");

    banner("② 进山庄新档，取 2 块药田 DIRT（③④ 用），⑤ 用固定站位格 (16,4)");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    let cells = dirt_cells(&mut runtime, 2)?;
    if cells.len() < 2 {
        return Err(format!("药田区未开垦 DIRT 不足 2 格，实际 {}", cells.len()).into());
    }
    println!("  药田格：{:?}；⑤ 固定用 (16,4)", cells);

    banner("③ 骨肥：薄荷 growth 3→2 天成熟，施肥消耗 1 个肥料");
    let bone_plot = cells[0];
    // 翻土
    till_at(&mut runtime, &host, bone_plot)?;
    // 补 1 个骨肥并选中施肥（真实玩家从 zhao_stall 购买）
    add_fertilizer(&mut runtime, "bone_fertilizer")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "fertilize_bone_fertilizer",
    )?;
    assert_num(&mut runtime, "inv.count('player', 'bone_fertilizer')", 0.0)?;
    assert_plot_fertilizer(&mut runtime, bone_plot, "bone_fertilizer")?;
    // 播种薄荷（补 1 粒）
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_mint', 1)")?;
    set_seed(&mut runtime, "seed_mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_mint",
    )?;
    // 骨肥：所需浇水天数 3→2。浇水 2 天后成熟（第 2 次浇水时 watered_days=1 < 2，
    // 仍是 water_mint；睡一觉后 watered_days=2 >= need 成熟）。
    let days = grow_crop(&mut runtime, &host, "water_mint")?;
    if days != 2 {
        return Err(format!("骨肥薄荷应 2 天成熟，实际 {days} 天").into());
    }
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    let total = total_crop(&mut runtime, "mint")?;
    let documented = query_num(&mut runtime, "require('data.crops').mint.yield")?;
    if (total - documented).abs() > f64::EPSILON {
        return Err(format!("骨肥薄荷收获 {total}，yield = {documented}（无灵肥不应加产）").into());
    }
    // 薄荷是再生作物：收获保留地块与作物 ⇒ 肥料继续保留（docs/33 §4.1.3
    // "再生作物多茬同享"；非再生作物清空地块 ⇒ 肥料消失由收获分支代码覆盖）
    assert_plot_fertilizer(&mut runtime, bone_plot, "bone_fertilizer")?;
    println!("  骨肥薄荷 {days} 天成熟、收 {total} 个，收获后地块保留作物与肥料（再生多茬同享）");

    banner("④ 灵肥：薄荷单茬 2 → +floor(2×0.5)=3 个");
    let spirit_plot = cells[1];
    till_at(&mut runtime, &host, spirit_plot)?;
    add_fertilizer(&mut runtime, "spirit_fertilizer")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "fertilize_spirit_fertilizer",
    )?;
    // 薄荷籽在 ③ 已用掉 1 粒（seed_for_selected 会回退到灵芝）；补 1 粒再播种
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_mint', 1)")?;
    set_seed(&mut runtime, "seed_mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_mint",
    )?;
    // 无骨肥：薄荷仍 3 天成熟
    let days = grow_crop(&mut runtime, &host, "water_mint")?;
    if days != 3 {
        return Err(format!("灵肥薄荷应 3 天成熟（无骨肥），实际 {days} 天").into());
    }
    let before = total_crop(&mut runtime, "mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    let after = total_crop(&mut runtime, "mint")?;
    if (after - before - 3.0).abs() > f64::EPSILON {
        return Err(format!(
            "灵肥薄荷单茬应 +3（2+floor(2×0.5)），实际 +{}",
            after - before
        )
        .into());
    }
    println!("  灵肥薄荷单茬 +{}（yield 2 → 3）", after - before);

    banner("⑤ 草药肥：收获至少良好（普通档并入良好），再生第二茬同享");
    // 用 (16,4)（`till_at` 站到它的南邻 (16,3) 朝上）：最左列站位 (14,5)(14,6)(14,7) 离庄园
    // NPC 过近，按 E 会被 dialogue 候选优先拦截（dialogue 按最近距离判定，不要求面前）；
    // (16,3) 是 m24 ⑧ 验证过的安全站位（面前格可 till、无 NPC 干扰）。
    // #28 起山庄出生点在 (19,3)，本节用的是显式传送站位，不再停在出生点。
    let herb_plot = (16, 4);
    till_at(&mut runtime, &host, herb_plot)?;
    add_fertilizer(&mut runtime, "herb_fertilizer")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "fertilize_herb_fertilizer",
    )?;
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
        return Err(format!("草药肥薄荷应 3 天成熟，实际 {days} 天").into());
    }
    // 首茬：本格收获的普通档增量必须为 0（③④ 无肥薄荷的普通档已累积在背包，
    // 用增量而非绝对计数），良好+上品增量 == yield
    let mint_before_first = query_num(&mut runtime, "inv.count('player', 'mint')")?;
    let good_super_before = good_super(&mut runtime, "mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    let mint_first_gain =
        query_num(&mut runtime, "inv.count('player', 'mint')")? - mint_before_first;
    if (mint_first_gain - 0.0).abs() > f64::EPSILON {
        return Err(format!("草药肥首茬产出普通档 {mint_first_gain} 个（应 0：保底良好）").into());
    }
    let first_quality = good_super(&mut runtime, "mint")? - good_super_before;
    if (first_quality - documented).abs() > f64::EPSILON {
        return Err(format!(
            "草药肥首茬良好/上品 {first_quality}，应 == yield {documented}（保底良好）"
        )
        .into());
    }
    assert_plot_fertilizer(&mut runtime, herb_plot, "herb_fertilizer")?;
    // 再生第二茬：地块保留肥料 ⇒ 多茬同享，收获仍全为良好/上品
    let days = grow_crop(&mut runtime, &host, "water_mint")?;
    if days != 3 {
        return Err(format!("草药肥薄荷再生茬应 3 天成熟（regrow_days=3），实际 {days} 天").into());
    }
    let mint_before_second = query_num(&mut runtime, "inv.count('player', 'mint')")?;
    let second_before = good_super(&mut runtime, "mint")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_mint",
    )?;
    let mint_second_gain =
        query_num(&mut runtime, "inv.count('player', 'mint')")? - mint_before_second;
    if (mint_second_gain - 0.0).abs() > f64::EPSILON {
        return Err(format!("草药肥再生茬产出普通档 {mint_second_gain} 个（应 0）").into());
    }
    let second_quality = good_super(&mut runtime, "mint")? - second_before;
    if (second_quality - documented).abs() > f64::EPSILON {
        return Err(
            format!("草药肥再生茬良好/上品 {second_quality}，应 == yield {documented}").into(),
        );
    }
    println!(
        "  草药肥薄荷两茬均无普通档（首茬 {first_quality} 个、再生茬 {second_quality} 个全为良好/上品）"
    );

    banner("⑥ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m38_fertilizer_quality_ok project={} fert=3 stall=3 quality=4 bone=2d spirit=+1 herb=2x",
        project.display()
    );
    Ok(())
}

/// 施肥动作：补 1 个肥料进背包并选中（真实玩家从 zhao_stall 购买 + 快捷栏选中）。
fn add_fertilizer(runtime: &mut LuaRuntime, item: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "inv.add(require('core.state').inventory, '{item}', 1)\n\
         require('core.state').selected_fertilizer = '{item}'"
    ))?;
    Ok(())
}

/// 浇水直到成熟（装备水壶、钉晴天、断言水动作、sleep）；返回浇水的游戏日数。
/// 成熟判定由地块 watered_days 与 crop growth_need（含骨肥 -1）决定。
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
            return Err("作物 10 天未成熟，成长/施肥判定可能被破坏".into());
        }
    }
    unreachable!()
}

/// 断言指定格地块的 fertilizer 字段（"none" = 无肥料）。失败时 dump 全量地块辅助定位。
fn assert_plot_fertilizer(
    runtime: &mut LuaRuntime,
    cell: (i64, i64),
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "local m = map.state('farm_plots')\n\
             local p = m['{},{}']\n\
             return tostring(p and p.fertilizer or 'none')",
            cell.0, cell.1
        ),
    )?;
    if got != expected {
        let dump = query_str(
            runtime,
            "local m = map.state('farm_plots')\n\
             local out = {}\n\
             for k, p in pairs(m) do\n\
               out[#out + 1] = k .. '=' .. tostring(p and p.crop or '?') .. '|' .. tostring(p and p.fertilizer or '-') .. '|' .. tostring(p and p.watered_days or '-') .. '|' .. tostring(p and p.cycles or '-') .. '|' .. tostring(p and p.watered_today or '-') .. '\\n'\n\
             end\n\
             table.sort(out)\n\
             local pid = require('player.player').ensure()\n\
             local pos = transform.get(pid)\n\
             local cell = grid.world_to_cell(pos.x, pos.y, 32)\n\
             return table.concat(out, '') .. 'player_cell=' .. cell[1] .. ',' .. cell[2] .. ' facing=' .. require('core.state').facing",
        )?;
        return Err(format!(
            "地块 ({},{}) fertilizer = {got:?}，应为 {expected:?}\nfarm_plots 全量：\n{dump}",
            cell.0, cell.1
        )
        .into());
    }
    Ok(())
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

/// 良好 + 上品条目总量（草药肥断言用：普通档不得出现）。
fn good_super(runtime: &mut LuaRuntime, crop_id: &str) -> Result<f64, Box<dyn std::error::Error>> {
    query_num(
        runtime,
        &format!("inv.count('player', '{crop_id}_good') + inv.count('player', '{crop_id}_super')"),
    )
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

/// 移动到指定格上方一格并朝上，翻土一次；断言 last_action == till。
/// 先装备锄头（翻土要求装锄；上一步浇水可能已换成水壶）。
/// 注意引擎 `grid.facing_cell(Up)` = 当前格 y+1（测试断言 Up→(0,1)，屏幕 y 向下），
/// 所以要让"面前格"恰为 (cx,cy)，玩家须站在 (cx, cy-1)。
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
