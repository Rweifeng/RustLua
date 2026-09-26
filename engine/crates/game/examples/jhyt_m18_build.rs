//! Headless acceptance drive for M18-02 《江湖有田》建筑占用、制作与升级.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M18-02 acceptance paths:
//!
//! - 纯数据注册：建筑/配方全部来自 `data/buildings.lua` + `data/recipes.lua`，
//!   加载期校验通过；运行期动态追加一条建筑+配方即可放置与制作（无需改 Rust）。
//! - 占用校验：逐格冲突枚举 out_of_bounds / ground / solid / exit / road /
//!   farm_plot / building，失败无副作用（不写快照、不生成实体、不改阻挡）。
//! - 放置/拆除/重载：footprint 全格阻挡、Sprite 实体、map.state 版本化快照；
//!   拆除后阻挡与占用清空；重放 on_enter 后三者一致。
//! - 制作设施：药庐消耗材料 → 按 clock 分钟制作 → 收取产出；busy/材料不足/
//!   等级不足/背包满各有明确原因，且失败不产生副作用。
//! - 升级：消耗木料升到 level2（换贴图）并解锁 min_level=2 配方；满级/材料
//!   不足幂等拒绝。
//! - 存档往返：capture→改动→restore 后建筑与制作任务一致；validate_long_run
//!   在含新 provider 的快照上仍稳定。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m18_build [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

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
    let save_path = fresh_save_path(&project, "jhyt_m18_build.sav");
    let project_arg = project.clone();

    banner("M18-02a 纯数据注册 + 加载期校验：建筑/配方来自 data/buildings + data/recipes");
    let mut runtime = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut runtime, &main_script)?;
    // 山庄是建造地图（manor.scene，FarmGrid 存在）；不点 BtnStart，直接加载场景。
    lua(&mut runtime, "scene.load('manor')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_mode(&mut runtime, "Playing")?;
    assert_validate(&mut runtime, "data.buildings")?;
    assert_validate(&mut runtime, "data.recipes")?;
    assert_num(&mut runtime, "#(require('data.buildings').ids())", 3.0)?;
    assert_num(
        &mut runtime,
        "require('data.buildings').get('herb_hut').w",
        2.0,
    )?;
    assert_num(
        &mut runtime,
        "require('data.buildings').max_level(require('data.buildings').get('storehouse'))",
        2.0,
    )?;
    assert_num(
        &mut runtime,
        "#(require('data.recipes').for_station('herb'))",
        2.0,
    )?;
    assert_str(
        &mut runtime,
        "require('data.recipes').for_station('herb')[1]",
        "pill_refine",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.crafting').query('0,0').has_job and 1 or 0",
        0.0,
    )?;

    banner("M18-02b 占用校验：逐格冲突原因 + 预览 footprint");
    // 扫描一个可放置锚点（对场景几何不敏感，确定性扫描）。
    let legal = find_legal_anchor(&mut runtime, "herb_hut")?;
    let (lx, ly) = legal;
    println!("legal anchor: {lx},{ly}");
    lua(
        &mut runtime,
        "require('systems.building').set_selected('herb_hut')",
    )?;
    // 预览：已选建筑时 footprint = 4 格（与朝向/位置无关的几何量）。
    assert_num(
        &mut runtime,
        "(function() local p = require('systems.building').preview('up'); return #p.cells end)()",
        4.0,
    )?;
    // 直接对准合法锚点的逐格校验（用 place 前的 can_place，避免朝向耦合）。
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.building').can_place('herb_hut', {lx}, {ly})))"),
        "true",
    )?;
    // out_of_bounds：越界锚点。
    assert_str(
        &mut runtime,
        "select(2, require('systems.building').can_place('herb_hut', -3, -3))[1].reason",
        "out_of_bounds",
    )?;
    // 未知建筑。
    assert_str(
        &mut runtime,
        "select(2, require('systems.building').can_place('nope', 5, 5))[1].reason",
        "unknown_building",
    )?;
    // ground：夯土（tile 2）不在 ground_tiles={1,3}。
    assert_str(
        &mut runtime,
        "(function() local b = require('systems.building'); for y = 0, 21 do for x = 0, 39 do \
           if tilemap.get_cell(entity.find('GroundMap'), x, y) == 2 then \
             local _, c = b.can_place('herb_hut', x, y); \
             if c[1] and c[1].reason == 'ground' then return 'ground' end end end end \
         return 'none' end)()",
        "ground",
    )?;
    // road：石板路（tile 3）且非出口。
    assert_str(
        &mut runtime,
        "(function() local b = require('systems.building'); local exits = {}; \
           for _, e in ipairs(require('data.maps').get('manor').exits) do \
             exits[e.cell.x .. ',' .. e.cell.y] = true end; \
           for y = 0, 21 do for x = 0, 39 do \
             local g = entity.find('GroundMap'); \
             if tilemap.get_cell(g, x, y) == 3 and not tilemap.is_solid(g, x, y) \
                and not exits[x .. ',' .. y] then \
               local _, c = b.can_place('herb_hut', x, y); \
               if c[1] and c[1].reason == 'road' then return 'road' end end end end \
         return 'none' end)()",
        "road",
    )?;
    // solid：允许地表但已阻挡（道具/院墙石材等）。
    assert_str(
        &mut runtime,
        "(function() local b = require('systems.building'); local g = entity.find('GroundMap'); \
           for y = 0, 21 do for x = 0, 39 do \
             if tilemap.get_cell(g, x, y) == 3 and tilemap.is_solid(g, x, y) then \
               local _, c = b.can_place('herb_hut', x, y); \
               if c[1] and c[1].reason == 'solid' then return 'solid' end end end end \
         return 'none' end)()",
        "solid",
    )?;
    // exit：出口触发格（manor 出口 (16,1)）。
    assert_str(
        &mut runtime,
        "(function() local b = require('systems.building'); local e = require('data.maps').get('manor').exits[1].cell; \
           local _, c = b.can_place('herb_hut', e.x, e.y); \
           for _, cell in ipairs(c) do if cell.reason == 'exit' then return 'exit' end end \
           return 'none' end)()",
        "exit",
    )?;
    // farm_plot：先在同一格翻土，再对该锚点求冲突。
    let plot_cell = find_farm_cell(&mut runtime)?;
    let (fx, fy) = plot_cell;
    till_plot(&mut runtime, fx, fy)?;
    assert_str(
        &mut runtime,
        &format!(
            "(function() local b = require('systems.building'); local _, c = b.can_place('herb_hut', {fx}, {fy}); \
             for _, cell in ipairs(c) do if cell.reason == 'farm_plot' then return 'farm_plot' end end \
             return 'none' end)()"
        ),
        "farm_plot",
    )?;

    banner("M18-02c 放置：footprint 阻挡 + Sprite 实体 + 版本化快照");
    lua(
        &mut runtime,
        &format!("require('systems.building').place('herb_hut', {lx}, {ly})"),
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "build_herb_hut",
    )?;
    assert_num(&mut runtime, "#(require('systems.building').list())", 1.0)?;
    // footprint 4 格全部阻挡。
    assert_num(
        &mut runtime,
        &format!(
            "(function() local g = entity.find('GroundMap'); local n = 0; \
             for i = 0, 1 do for j = 0, 1 do \
               if tilemap.is_solid(g, {lx} + i, {ly} + j) then n = n + 1 end end end return n end)()"
        ),
        4.0,
    )?;
    // 快照版本化 + 记录。
    assert_num(&mut runtime, "map.state('buildings').version", 1.0)?;
    assert_num(&mut runtime, "#(map.state('buildings').list)", 1.0)?;
    // 再次同锚点 → building 冲突（无副作用，仍是 1 座）。
    assert_str(
        &mut runtime,
        &format!(
            "select(2, require('systems.building').can_place('herb_hut', {lx}, {ly}))[1].reason"
        ),
        "building",
    )?;
    assert_num(&mut runtime, "#(require('systems.building').list())", 1.0)?;

    banner("M18-02d 拆除与重载一致性：占用/实体/快照三方一致");
    lua(
        &mut runtime,
        &format!("require('systems.building').remove({lx}, {ly})"),
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "demolish_herb_hut",
    )?;
    assert_num(&mut runtime, "#(require('systems.building').list())", 0.0)?;
    assert_num(
        &mut runtime,
        &format!(
            "(function() local g = entity.find('GroundMap'); local n = 0; \
             for i = 0, 1 do for j = 0, 1 do \
               if tilemap.is_solid(g, {lx} + i, {ly} + j) then n = n + 1 end end end return n end)()"
        ),
        0.0,
    )?;
    assert_num(&mut runtime, "#(map.state('buildings').list)", 0.0)?;
    // 重放（切图/读档路径）：先重建再重放，三方一致。
    lua(
        &mut runtime,
        &format!("require('systems.building').place('herb_hut', {lx}, {ly})"),
    )?;
    lua(
        &mut runtime,
        "require('systems.building').on_enter('manor')",
    )?;
    assert_num(&mut runtime, "#(require('systems.building').list())", 1.0)?;
    assert_num(
        &mut runtime,
        &format!(
            "(function() local g = entity.find('GroundMap'); local n = 0; \
             for i = 0, 1 do for j = 0, 1 do \
               if tilemap.is_solid(g, {lx} + i, {ly} + j) then n = n + 1 end end end return n end)()"
        ),
        4.0,
    )?;
    // 重放不重复生成实体：list 仍为 1（幂等）。
    lua(
        &mut runtime,
        "require('systems.building').on_enter('manor')",
    )?;
    assert_num(&mut runtime, "#(require('systems.building').list())", 1.0)?;

    banner("M18-02e 制作链路：消耗材料 → 按分钟制作 → 收取产出");
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'lingzhi', 4)",
    )?;
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'rice', 4)",
    )?;
    let key = format!("{lx},{ly}");
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').start('pill_refine', '{key}')))"),
        "true",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'lingzhi')",
        2.0,
    )?;
    let remaining = query_num(
        &mut runtime,
        &format!("return require('systems.crafting').query('{key}').remaining"),
    )?;
    if remaining <= 0.0 {
        return Err(format!("expected positive remaining, got {remaining}").into());
    }
    // busy：进行中重复 start 拒绝。
    assert_str(
        &mut runtime,
        &format!("select(2, require('systems.crafting').start('pill_refine', '{key}'))"),
        "busy",
    )?;
    // 拆除保护：制作中不可拆。
    assert_str(
        &mut runtime,
        &format!("select(2, require('systems.building').remove({lx}, {ly}))"),
        "crafting_pending",
    )?;
    // 未到点收取 → crafting。
    assert_str(
        &mut runtime,
        &format!("select(2, require('systems.crafting').collect('{key}'))[1]"),
        "crafting",
    )?;
    // 推到次日：done 已过 → 收取成功。
    lua(&mut runtime, "clock.set(2, 30)")?;
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').collect('{key}')))"),
        "true",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'heal_pill')",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        &format!("require('systems.crafting').query('{key}').has_job and 1 or 0"),
        0.0,
    )?;

    banner("M18-02f 制作原子性：材料不足 / 等级不足 / 背包满");
    // 清空材料后 start → not_enough 且无副作用。
    lua(
        &mut runtime,
        "inv.remove(require('core.state').inventory, 'lingzhi', 99)",
    )?;
    lua(
        &mut runtime,
        "inv.remove(require('core.state').inventory, 'rice', 99)",
    )?;
    assert_str(
        &mut runtime,
        &format!("select(2, require('systems.crafting').start('pill_refine', '{key}'))"),
        "not_enough",
    )?;
    assert_num(
        &mut runtime,
        &format!("require('systems.crafting').query('{key}').has_job and 1 or 0"),
        0.0,
    )?;
    // 等级不足：level1 药庐做 tonic_refine（min_level=2）→ station_level。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'lingzhi', 2)",
    )?;
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'rice', 4)",
    )?;
    assert_str(
        &mut runtime,
        &format!("select(2, require('systems.crafting').start('tonic_refine', '{key}'))"),
        "station_level",
    )?;
    // 背包满：塞满 20 格后收取 → full 且 job 保留。
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').start('pill_refine', '{key}')))"),
        "true",
    )?;
    lua(&mut runtime, "clock.set(3, 30)")?;
    lua(
        &mut runtime,
        "(function() local inv_id = require('core.state').inventory; \
         for i = 1, 20 do inv.set(inv_id, i, 'rice', 99) end end)()",
    )?;
    assert_str(
        &mut runtime,
        &format!("tostring((select(2, require('systems.crafting').collect('{key}'))))"),
        "full",
    )?;
    assert_num(
        &mut runtime,
        &format!("require('systems.crafting').query('{key}').has_job and 1 or 0"),
        1.0,
    )?;
    lua(
        &mut runtime,
        "(function() local inv_id = require('core.state').inventory; \
         for i = 1, 20 do inv.set(inv_id, i, nil, nil) end end)()",
    )?;
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').collect('{key}')))"),
        "true",
    )?;

    banner("M18-02g 升级：消耗木料 → level2 换贴图并解锁高阶配方");
    lua(
        &mut runtime,
        "inv.remove(require('core.state').inventory, 'timber', 99)",
    )?;
    assert_str(
        &mut runtime,
        &format!("select(2, require('systems.building').upgrade({lx}, {ly}))"),
        "not_enough",
    )?;
    assert_num(
        &mut runtime,
        "(function() for _, e in ipairs(require('systems.building').list()) do return e.level end end)()",
        1.0,
    )?;
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'timber', 4)",
    )?;
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.building').upgrade({lx}, {ly})))"),
        "true",
    )?;
    assert_num(
        &mut runtime,
        "(function() for _, e in ipairs(require('systems.building').list()) do return e.level end end)()",
        2.0,
    )?;
    // level2 解锁 tonic_refine，且材料齐备时可开工。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'lingzhi', 2)",
    )?;
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'rice', 4)",
    )?;
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').start('tonic_refine', '{key}')))"),
        "true",
    )?;
    lua(&mut runtime, "clock.set(4, 30)")?;
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').collect('{key}')))"),
        "true",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'herb_tonic')",
        1.0,
    )?;
    // 满级：清材料后 upgrade → max_level（先验满级优先于材料）。
    assert_str(
        &mut runtime,
        &format!("select(2, require('systems.building').upgrade({lx}, {ly}))"),
        "max_level",
    )?;

    banner("M18-02h 建造模式与输入：B 开关 / 预览冲突文案（非法锚点）");
    // 把玩家放在合法锚点旁，面向合法锚点，B 开建造模式 → HUD 预览显示可放置。
    lua(
        &mut runtime,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'",
            (lx as f32 + 0.5) * 32.0,
            (ly as f32 - 0.5) * 32.0
        ),
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('b').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "build_mode_on",
    )?;
    // 面向格已被既有建筑占用 → 预览给出 building 冲突（HUD 文案可解释）。
    assert_str(
        &mut runtime,
        "require('systems.building').preview('up').conflicts[1].reason",
        "building",
    )?;
    // M26：HUD 状态行不再拼英文 reason（`solid`/`farm_plot`/`building`…），改走
    // `hud.lua` 的 BUILD_REASON 中文表 —— 断言随之改查中文文案。
    assert_str(
        &mut runtime,
        "require('ui.input').update():match('已有建筑') ~= nil and 'ok' or 'none'",
        "ok",
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('b').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "build_mode_off",
    )?;

    banner("M18-02i 纯数据扩展：运行期新增建筑 + 配方，无需改 Rust 核心");
    lua(
        &mut runtime,
        "(function() \
           local bd = require('data.buildings'); \
           local rd = require('data.recipes'); \
           bd.buildings.workshop = { \
             name = '工坊', w = 1, h = 1, ground_tiles = { 1, 3 }, \
             levels = { { sprite = 'assets/art/buildings/build_scaffold.png', size = '128x96' } }, \
             upgrade_cost = {}, station = 'workshop' }; \
           rd.recipes.plank_make = { \
             name = '制板', station = 'workshop', \
             inputs = { { item = 'timber', count = 1 } }, \
             output = { item = 'timber', count = 2 }, minutes = 30, min_level = 1 }; \
         end)()",
    )?;
    assert_validate(&mut runtime, "data.buildings")?;
    assert_validate(&mut runtime, "data.recipes")?;
    let workshop_anchor = find_legal_anchor(&mut runtime, "workshop")?;
    let (wx, wy) = workshop_anchor;
    lua(
        &mut runtime,
        &format!("require('systems.building').place('workshop', {wx}, {wy})"),
    )?;
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'timber', 1)",
    )?;
    let wkey = format!("{wx},{wy}");
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').start('plank_make', '{wkey}')))"),
        "true",
    )?;
    lua(&mut runtime, "clock.set(6, 30)")?;
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.crafting').collect('{wkey}')))"),
        "true",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'timber')",
        2.0,
    )?;

    banner("M18-02j 存档往返：建筑/制作随 provider 恢复 + 长跑快照稳定");
    lua(
        &mut runtime,
        "__b_captured = require('core.save_manager').capture()",
    )?;
    lua(
        &mut runtime,
        &format!("require('systems.building').remove({wx}, {wy})"),
    )?;
    assert_num(&mut runtime, "#(require('systems.building').list())", 1.0)?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('core.save_manager').restore(__b_captured); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "true|nil",
    )?;
    assert_num(&mut runtime, "#(require('systems.building').list())", 2.0)?;
    assert_num(&mut runtime, "map.state('buildings').version", 1.0)?;
    assert_str(
        &mut runtime,
        "(function() local ok, report = require('core.save_manager').validate_long_run(2, 5); \
         return tostring(ok) end)()",
        "true",
    )?;
    let buildings = query_num(&mut runtime, "return #require('systems.building').list()")?;
    let level = query_num(
        &mut runtime,
        &format!(
            "(function() for _, e in ipairs(require('systems.building').list()) do \
             if e.key == '{key}' then return e.level end end return -1 end)()"
        ),
    )?;

    println!(
        "jhyt_m18_build_ok project={} legal={},{} buildings={} herb_level={} recipes={}",
        project.display(),
        lx,
        ly,
        buildings,
        level,
        query_num(&mut runtime, "return #require('data.recipes').ids()")?,
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers ----------

/// Creates a bare engine runtime pointing at the project scripts & save file.
fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

/// Runs the project script, then loads the `main` 门面 scene so scene.load works
/// (mirrors jhyt_m18_stats).
fn enter_main_menu(
    runtime: &mut LuaRuntime,
    main_script: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_project_script(main_script)?;
    runtime.execute_console("scene.load('main')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(runtime, "main")?;
    Ok(())
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

/// 药田矩形（farm.lua FARM）内找一个未开垦地块，供 farm_plot 冲突构造。
fn find_farm_cell(runtime: &mut LuaRuntime) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        "(function() for y = 4, 13 do for x = 14, 33 do \
           local p = require('systems.farm'); \
           if not p.is_plot(x, y) then return x .. ',' .. y end end end return 'none' end)()",
    )?;
    let mut parts = got.split(',');
    let x = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no farm cell: {got}"))?;
    let y = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no farm cell: {got}"))?;
    Ok((x, y))
}

/// 让玩家站到目标格下方、面向该格，用锄头翻土（走 farm.act 真实路径）。
fn till_plot(runtime: &mut LuaRuntime, cx: i64, cy: i64) -> Result<(), Box<dyn std::error::Error>> {
    lua(
        runtime,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'; \
             inv.equip(require('core.state').inventory, 'tool', 'hoe')",
            (cx as f32 + 0.5) * 32.0,
            (cy as f32 - 0.5) * 32.0
        ),
    )?;
    let action = query_str(runtime, "return require('systems.farm').act('up')")?;
    if action != "till" {
        return Err(format!("till failed: {action}").into());
    }
    assert_str(
        runtime,
        &format!("tostring(require('systems.farm').is_plot({cx}, {cy}))"),
        "true",
    )?;
    Ok(())
}

/// Simulates a single key press: one frame with the key down, one clean frame.
fn press(runtime: &mut LuaRuntime, key: KeyCode) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
}

fn lua(runtime: &mut LuaRuntime, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(code)?;
    Ok(())
}

fn query_str(runtime: &mut LuaRuntime, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value = runtime.execute_console(expr)?;
    Ok(value.trim().to_owned())
}

fn query_num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let value = query_str(runtime, expr)?;
    value
        .parse::<f64>()
        .map_err(|e| format!("{value:?} not a number: {e}").into())
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

fn assert_validate(
    runtime: &mut LuaRuntime,
    module: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "local ok, issues = require('{module}').validate_all(); \
             if not ok then return table.concat(issues, ' | ') end; return 'ok'"
        ),
    )?;
    if got != "ok" {
        return Err(format!("{module}.validate_all failed: {got}").into());
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
    println!("\n=== {title} ===");
}

fn fresh_save_path(project: &Path, name: &str) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(name);
    let _ = fs::remove_file(&path);
    path
}
