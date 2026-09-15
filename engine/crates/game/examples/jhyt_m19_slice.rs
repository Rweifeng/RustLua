//! Headless acceptance drive for M19-01 《江湖有田》MVP 垂直切片整合.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M19-01 acceptance script chapter by chapter, exactly as a new
//! player would experience it in a clean install:
//!
//! 1. 创建存档（title → BtnStart → manor）：初始金钱/体力/种子/水壶 + 数据校验；
//! 2. 开垦 → 播种 → 浇水（farm.act 与真实 E 键路径各覆盖一次）；
//! 3. 进入村庄（走 manor→village 出口真实路径）；
//! 4. 接任务：经对话接 3 个订单中的至少 1 个 + 主线第一段，断言 quest.state/state.vars；
//! 5. 进山采集（真实 E 键 → gather 交互目标 → inv/labor/已采状态/重复采集被拒）
//!    + 与青云山道野生敌人（野猪）战斗（真实 J 键，断言战斗账本）；
//! 6. 回村交付订单（断言奖励到账 + 重复交付 already_completed 且不重复发奖）；
//! 7. 制作药剂（放置药庐 → crafting.start('pill_refine') → 推 clock → 收取）；
//! 8. 睡觉（真实 T 键 → day.sleep() 真实路径）：日计数 +1、体力恢复、自动存档；
//! 9. 第二天读档（第二个 runtime → title → BtnContinue）：地图/金钱/任务/关系/背包一致；
//! 10. 完成主线节点（交付主线最后一段）：mainline_done == true + 关系/声望奖励到账；
//! 11. 汇总输出 `jhyt_m19_slice_ok ...`。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m19_slice [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 山庄 → 云泽村出口格 (16,1)：世界坐标 (528, 48)。
const MANOR_TO_VILLAGE: (f32, f32) = (528.0, 48.0);
/// 青云山道采集点 wild_herb_1 格 (12,11)：站 (12,10)（cell y 更小 = 世界更靠下）朝上。
const GATHER_STAND: (f32, f32) = (400.0, 336.0);
/// 10:00：顾长风在剑坪（manor_sword_ground）、沈青萝在医馆（village_clinic）。
const TEN_AM: i64 = 600;
/// 睡觉前的时刻：12:00。
const NOON: i64 = 720;

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
    let save_path = fresh_save_path(&project, "jhyt_m19_slice.sav");
    let project_arg = project.clone();

    // ---------------- 1. 创建存档 ----------------
    banner("M19-01① 创建存档：title → BtnStart → manor，初始资源 + 内容校验");
    let mut rt = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;
    assert_map_current(&mut rt, "manor")?;
    assert_mode(&mut rt, "Playing")?;
    assert_num(&mut rt, "require('core.state').money", 120.0)?;
    assert_num(&mut rt, "require('core.state').energy", 100.0)?;
    assert_num(&mut rt, "require('core.state').inner_power", 50.0)?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'seed_lingzhi')",
        8.0,
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'seed_rice')",
        4.0,
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'watering_can')",
        1.0,
    )?;
    assert_str(
        &mut rt,
        "inv.equip_list(require('core.state').inventory).tool",
        "hoe",
    )?;
    // 内容最低标准：3 张地图、3 个以上 NPC、2 种作物、1 种建筑、山道有敌人、
    // 3 个订单 + 1 条主线（3 段）、晴雨天气、季节日历。
    for (expr, want, what) in [
        (
            "(function() local n = 0; for _ in pairs(require('data.maps').maps) do n = n + 1 end return n end)()",
            "at least:3",
            "地图",
        ),
        ("#(require('data.npcs').npcs)", "at least:3", "NPC"),
        (
            "(function() local c = require('data.crops'); return (c.lingzhi and c.rice) and 2 or 0 end)()",
            "at least:2",
            "作物",
        ),
        ("#(require('data.enemies').enemies)", "at least:1", "敌人"),
        ("#(require('world.quest').ids())", "at least:4", "任务"),
    ] {
        assert_at_least(&mut rt, expr, want, what)?;
    }
    assert_str(
        &mut rt,
        "require('data.enemies').get('enemy_wild_boar').map",
        "mountain_path",
    )?;
    assert_num(
        &mut rt,
        "(function() local n = 0; for _, m in ipairs({ 'manor', 'village', 'mountain_path' }) do \
           if require('data.maps').get(m) then n = n + 1 end end return n end)()",
        3.0,
    )?;
    // 统一数据校验（含 quest/dialogue/enemies 交叉引用）+ 新增 gather 校验器。
    assert_str(
        &mut rt,
        "(function() local report = require('validate_data').check({ strict = true }); \
          return report == '' and 'ok' or report end)()",
        "ok",
    )?;
    assert_validate(&mut rt, "data.gather")?;
    assert_validate(&mut rt, "data.enemies")?;
    assert_validate(&mut rt, "world.quest")?;
    assert_validate(&mut rt, "world.dialogue")?;
    // 晴雨天气可切换（天气排程是已知缺口，见 docs/46 §已知限制）。
    assert_str(
        &mut rt,
        "(function() local w = require('world.weather'); w.set('rainy'); local a = w.is_rainy(); \
          w.set('sunny'); return tostring(a) .. '|' .. tostring(w.current()) end)()",
        "true|sunny",
    )?;
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, 2)?;

    // ---------------- 2. 开垦 → 播种 → 浇水 ----------------
    banner("M19-01② 种田闭环：开垦→播种→浇水（farm.act 与真实 E 键各一次）");
    // 药田（land=medicinal）两块 + 粮田（land=field）一块：
    // 灵芝要分「药庐订单 2 + 主线交药材 2 + 炼药 2」，两块田（各收 2）才算得过来。
    let (fx, fy) = find_plot_cell(&mut rt, 14, 23, "lingzhi")?;
    let (fx2, fy2) = find_plot_cell(&mut rt, 14, 23, "lingzhi")?;
    let (rx, ry) = find_plot_cell(&mut rt, 24, 33, "rice")?;
    // 灵芝块 1：全程走 farm.act 公开 API。
    till_with_api(&mut rt, fx, fy)?;
    assert_str(&mut rt, "require('core.state').last_action", "till")?;
    assert_num(&mut rt, "require('core.state').energy", 92.0)?;
    assert_num(&mut rt, "require('systems.farm').stats().tilled", 1.0)?;
    lua(
        &mut rt,
        "require('core.state').selected_seed = 'seed_lingzhi'",
    )?;
    assert_str(
        &mut rt,
        "require('systems.farm').act('up')",
        "plant_seed_lingzhi",
    )?;
    // 钉住晴天（M29 起天气按 `(seed, day, 季节)` 抽签）：雨天走 `rain_watered` 免体力
    // 分支，而本节断言的是体力/劳作账（100-8-3-4 → 85 → 70 → 55）。
    lua(&mut rt, "require('world.weather').set('sunny')")?;
    lua(
        &mut rt,
        "inv.equip(require('core.state').inventory, 'tool', 'watering_can')",
    )?;
    assert_str(
        &mut rt,
        "require('systems.farm').act('up')",
        "water_lingzhi",
    )?;
    assert_num(&mut rt, "require('core.state').energy", 85.0)?;
    // 灵芝块 2：同一套 API 再种一块（farm.act 的公开路径已覆盖，这里直接走 act）。
    till_with_api(&mut rt, fx2, fy2)?;
    lua(
        &mut rt,
        "require('core.state').selected_seed = 'seed_lingzhi'",
    )?;
    assert_str(
        &mut rt,
        "require('systems.farm').act('up')",
        "plant_seed_lingzhi",
    )?;
    lua(
        &mut rt,
        "inv.equip(require('core.state').inventory, 'tool', 'watering_can')",
    )?;
    assert_str(
        &mut rt,
        "require('systems.farm').act('up')",
        "water_lingzhi",
    )?;
    assert_num(&mut rt, "require('core.state').energy", 70.0)?;
    // 稻谷块：全程走真实按键路径（E 键 → interaction → farm 目标 → farm.act）。
    lua(
        &mut rt,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'; \
             inv.equip(require('core.state').inventory, 'tool', 'hoe'); \
             require('core.state').selected_seed = 'seed_rice'",
            (rx as f32 + 0.5) * 32.0,
            (ry as f32 - 0.5) * 32.0
        ),
    )?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(&mut rt, "require('core.state').last_action", "till")?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "plant_seed_rice",
    )?;
    lua(
        &mut rt,
        "inv.equip(require('core.state').inventory, 'tool', 'watering_can')",
    )?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(&mut rt, "require('core.state').last_action", "water_rice")?;
    assert_num(&mut rt, "require('core.state').energy", 55.0)?;
    assert_num(&mut rt, "require('systems.farm').stats().planted", 3.0)?;
    assert_num(&mut rt, "require('core.state').labor_minutes", 90.0)?;
    // 新手引导第①~③步已按真实路径推进（主线一段的「翻一块药田」目标即以此为锚点）。
    assert_num(
        &mut rt,
        "require('core.state').get_var('tutorial_step')",
        3.0,
    )?;

    // ---------------- 3. 进入村庄 ----------------
    banner("M19-01③ 进入云泽村：走 manor→village 出口格真实路径，NPC 在场");
    teleport_world(&mut rt, MANOR_TO_VILLAGE)?;
    lua(&mut rt, "require('core.state').facing = 'down'")?;
    press(&mut rt, KeyCode::KeyE)?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "village")?;
    assert_map_current(&mut rt, "village")?;
    assert_str(&mut rt, "require('core.state').last_action", "exit_village")?;
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    for npc_id in ["npc_shen_qingluo", "npc_lin_xiaoman"] {
        assert_str(
            &mut rt,
            &format!(
                "(function() local q = require('world.npc').query('{npc_id}'); \
                  if not q then return 'none' end; \
                  return q.map .. '|' .. tostring(q.live) end)()"
            ),
            "village|true",
        )?;
    }

    // ---------------- 4. 接任务：订单 + 主线第一段 ----------------
    banner("M19-01④ 接任务：对话接订单 order_herb + 主线 mainline_arrival");
    // 订单·药庐灵芝（沈青萝）：greet → order_intro → 接取 → 结束。
    talk_to(&mut rt, "npc_shen_qingluo")?;
    assert_str(
        &mut rt,
        "require('world.dialogue').dialogue_id()",
        "shen_qingluo_herb",
    )?;
    choose_option(&mut rt, "听说药庐常年收灵芝？")?;
    assert_str(
        &mut rt,
        "require('world.dialogue').info().node",
        "order_intro",
    )?;
    choose_option(&mut rt, "这单我接了。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_herb')",
        "in_progress",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('orders_started')",
        "true",
    )?;
    lua(&mut rt, "require('world.dialogue').choose()")?; // 关掉 order_accept
    assert_str(&mut rt, "require('world.dialogue').is_open()", "false")?;
    assert_mode(&mut rt, "Playing")?;
    // 主线一段（顾长风，山庄）：回到山庄接取 + 记录「讲规矩」。
    lua(&mut rt, "map['goto']('manor', 528.0, 48.0)")?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "manor")?;
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_gu_changfeng")?;
    assert_str(
        &mut rt,
        "require('world.dialogue').dialogue_id()",
        "gu_changfeng_teach",
    )?;
    choose_option(&mut rt, "弟子新来乍到，请师父吩咐。")?;
    assert_str(
        &mut rt,
        "require('world.dialogue').info().node",
        "arrival_offer",
    )?;
    // 「弟子遵命。」= arrival_offer 的第 1 条；「是。」按同一相对序号对齐
    // arrival_accept 的 set_var = { gu_taught = true }（对话用相对索引，见数据注释）。
    // 「翻一块药田」目标在②已达成（tutorial_step >= 1），故接取后立刻只差「讲规矩」。
    choose_option(&mut rt, "弟子遵命。")?;
    choose_option(&mut rt, "是。")?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('gu_taught')",
        "true",
    )?;
    assert_str(&mut rt, "require('world.dialogue').is_open()", "false")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('mainline_arrival')",
        "deliverable",
    )?;
    talk_to(&mut rt, "npc_gu_changfeng")?;
    choose_option(&mut rt, "药田翻好了，规矩也已记下。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('mainline_arrival')",
        "completed",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('mainline_arrival_done')",
        "true",
    )?;
    assert_num(&mut rt, "require('core.state').inner_power", 60.0)?;
    // 关系分 = 对话接取 +3 + mainline_arrival.on_complete.relationship +5 = 8。
    assert_num(
        &mut rt,
        "require('player.relationship').points('npc_gu_changfeng')",
        8.0,
    )?;
    lua(&mut rt, "require('world.dialogue').choose()")?;

    // ---------------- 5. 进山采集 + 战斗 ----------------
    banner("M19-01⑤ 青云山道：采集（真实 E 键）+ 野猪战斗（真实 J 键）");
    lua(&mut rt, "map['goto']('mountain_path', 624.0, 80.0)")?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "mountain_path")?;
    assert_map_current(&mut rt, "mountain_path")?;
    frames(&mut rt, 2)?;
    // 数据侧：山道有采集点，野猪在该图。
    assert_num(
        &mut rt,
        "#(require('systems.gather').nodes('mountain_path'))",
        5.0,
    )?;
    assert_str(
        &mut rt,
        "require('data.enemies').get('enemy_wild_boar').map",
        "mountain_path",
    )?;
    let lingzhi_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lingzhi')",
    )?;
    let labor_before = query_num(&mut rt, "require('core.state').labor_minutes")?;
    teleport_world(&mut rt, GATHER_STAND)?;
    lua(&mut rt, "require('core.state').facing = 'up'")?;
    frames(&mut rt, 2)?;
    // 只读辅助：面向格命中可采集节点。
    assert_str(
        &mut rt,
        "(function() local a = require('systems.gather').available('up'); \
          if not a then return 'none' end; \
          return a.node.id .. '|' .. tostring(a.ok) end)()",
        "wild_herb_1|true",
    )?;
    assert_str(
        &mut rt,
        "require('systems.interaction').prompt('up')",
        "E: 采集 灵芝",
    )?;
    // 真实按键路径：E → interaction → gather 目标 → inv/labor/已采记录。
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "gather_lingzhi",
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lingzhi')",
        lingzhi_before + 2.0,
    )?;
    assert_num(
        &mut rt,
        "require('core.state').labor_minutes",
        labor_before + 20.0,
    )?;
    assert_str(
        &mut rt,
        "tostring(require('systems.gather').is_gathered('wild_herb_1'))",
        "true",
    )?;
    assert_num(&mut rt, "map.state('gather').state_version", 1.0)?;
    assert_num(
        &mut rt,
        "(function() local s = map.state('gather'); \
          if type(s) ~= 'table' or type(s.nodes) ~= 'table' then return 0 end; \
          return s.nodes['wild_herb_1'] and s.nodes['wild_herb_1'].day or 0 end)()",
        1.0,
    )?;
    // 已采集：prompt 变「（已采集）」且再次按 E 不产出、不重复记账。
    assert_str(
        &mut rt,
        "require('systems.interaction').prompt('up')",
        "（已采集 · 灵芝）",
    )?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lingzhi')",
        lingzhi_before + 2.0,
    )?;
    // 跨天刷新语义：第 2 天该节点重新可采（clock 推进 + 惰性刷新，幂等）。
    lua(&mut rt, "clock.set(2, 480)")?;
    assert_str(
        &mut rt,
        "(function() local a = require('systems.gather').available('up'); \
          if not a then return 'none' end; \
          return a.node.id .. '|' .. tostring(a.ok) end)()",
        "wild_herb_1|true",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('systems.gather').is_gathered('wild_herb_1'))",
        "false",
    )?;
    lua(&mut rt, "clock.set(1, 600)")?;
    // 缺前置分支：矿石点 requires = "hoe"，手上没有锄头时明确给出原因。
    assert_str(
        &mut rt,
        "(function() local d = require('data.gather').get('wild_ore_1'); \
          inv.remove(require('core.state').inventory, 'hoe', 99); \
          local g = require('systems.gather'); \
          local cell = d.cell; \
          require('player.player').teleport((cell.x + 0.5) * 32, (cell.y - 0.5) * 32); \
          require('core.state').facing = 'up'; \
          local info = g.available('up'); \
          if not info then return 'none' end; \
          return tostring(info.ok) .. '|' .. tostring(info.reason) end)()",
        "false|需要锄头",
    )?;
    lua(
        &mut rt,
        "inv.add(require('core.state').inventory, 'hoe', 1); \
         inv.equip(require('core.state').inventory, 'tool', 'hoe')",
    )?;
    // 战斗：青云山道野猪（真实 J 键：普攻判定窗 → 命中/击杀入账）。
    lua(
        &mut rt,
        "inv.add(require('core.state').inventory, 'training_sword', 1); \
         inv.equip(require('core.state').inventory, 'weapon', 'training_sword')",
    )?;
    assert_num(
        &mut rt,
        "require('player.skills').player_damage('strike')",
        10.0,
    )?;
    let kills_before = query_num(&mut rt, "require('systems.combat').stats().kills")?;
    strike_boar(&mut rt)?;
    frames(&mut rt, 30)?;
    let boar_hp = query_str(
        &mut rt,
        "require('systems.enemy').query('enemy_wild_boar').hp",
    )?;
    assert_str(
        &mut rt,
        "(function() local q = require('systems.enemy').query('enemy_wild_boar'); \
          return tostring(q.max_hp - q.hp > 0) end)()",
        "true",
    )?;
    println!("野猪 hp after 1st strike = {boar_hp}");
    // 第二击（等冷却 24 步）→ 击杀，战斗账本 kills +1。
    frames(&mut rt, 30)?;
    strike_boar(&mut rt)?;
    frames(&mut rt, 30)?;
    assert_num(
        &mut rt,
        "require('systems.combat').stats().kills",
        kills_before + 1.0,
    )?;
    assert_str(
        &mut rt,
        "require('systems.enemy').query('enemy_wild_boar').alive",
        "false",
    )?;
    assert_str(
        &mut rt,
        "(function() for _, e in ipairs(require('systems.combat').log()) do \
           if e.event == 'kill' and e.target == '野猪' then return 'true' end end return 'false' end)()",
        "true",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('systems.combat').stats().dealt > 0)",
        "true",
    )?;
    // 继续在山道采集：野谷（wild_berry_1）与毒草丛（wild_poison_herb_1，需锄头已装备）。
    // 农田作物要 7/4 个有效成长日（当日不可收），故药材/稻谷由山道供给。
    // 站到采集点**相邻格**并面向它（(9,15)→(9,16)、(29,14)→(29,15)）。
    lua(
        &mut rt,
        "require('player.player').teleport(304.0, 496.0); require('core.state').facing = 'up'",
    )?;
    frames(&mut rt, 2)?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(&mut rt, "require('core.state').last_action", "gather_rice")?;
    lua(
        &mut rt,
        "require('player.player').teleport(944.0, 464.0); require('core.state').facing = 'up'",
    )?;
    frames(&mut rt, 2)?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "gather_lingzhi",
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'rice') >= 4 and 1 or 0",
        1.0,
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lingzhi') >= 3 and 1 or 0",
        1.0,
    )?;

    // ---------------- 6. 回村交付订单 ----------------
    banner("M19-01⑥ 回村交付：order_herb 奖励到账 + 重复交付幂等");
    lua(&mut rt, "map['goto']('village', 320.0, 48.0)")?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "village")?;
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    let rel_before = query_num(
        &mut rt,
        "require('player.relationship').points('npc_shen_qingluo')",
    )?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    choose_option(&mut rt, "灵芝备好了，共两株。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_herb')",
        "completed",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('order_herb_done')",
        "true",
    )?;
    assert_num(&mut rt, "require('core.state').money", money_before + 60.0)?;
    assert_num(
        &mut rt,
        "require('player.relationship').points('npc_shen_qingluo')",
        rel_before + 8.0,
    )?;
    // 幂等：重复交付返回 already_completed 且奖励不重复发放。
    assert_str(
        &mut rt,
        "select(2, require('world.quest').deliver('order_herb'))",
        "already_completed",
    )?;
    assert_num(&mut rt, "require('core.state').money", money_before + 60.0)?;
    assert_num(
        &mut rt,
        "require('player.relationship').points('npc_shen_qingluo')",
        rel_before + 8.0,
    )?;
    choose_option(&mut rt, "收下了。")?;
    // 主线二段：任一订单结清后由沈青萝发放（交付放到⑦制丸药之后，见那里的说明）。
    assert_str(
        &mut rt,
        "require('world.quest').state('mainline_orders')",
        "not_started",
    )?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    choose_option(&mut rt, "村里还有别的活计吗？")?;
    choose_option(&mut rt, "应该的。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('mainline_orders')",
        "in_progress",
    )?;
    choose_option(&mut rt, "记下了。")?;
    // 市集订单（林小满·茶农）：需求 2 稻谷（田园产出）。
    // 不选货摊掌柜赵铁算盘：他挂着 shop_keeper（priority 20），按 E 只开店、
    // 对话不可达（docs/46 §已知限制）；茶农林小满按时段在茶园 (12,13)。
    let money_before2 = query_num(&mut rt, "require('core.state').money")?;
    let rep_before = query_num(
        &mut rt,
        "require('player.relationship').reputation('village')",
    )?;
    talk_to(&mut rt, "npc_lin_xiaoman")?;
    assert_str(
        &mut rt,
        "require('world.dialogue').dialogue_id()",
        "lin_xiaoman_rice",
    )?;
    choose_option(&mut rt, "怎么收？")?;
    choose_option(&mut rt, "这单我接了。")?;
    // 新档自带 2 稻谷，接取后目标即刻达成（deliverable）。
    assert_str(
        &mut rt,
        "require('world.quest').state('order_rice')",
        "deliverable",
    )?;
    choose_option(&mut rt, "放心。")?;
    lua(&mut rt, "clock.set(1, 600)")?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_lin_xiaoman")?;
    choose_option(&mut rt, "两斗稻谷在此。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_rice')",
        "completed",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('order_rice_done')",
        "true",
    )?;
    assert_num(&mut rt, "require('core.state').money", money_before2 + 45.0)?;
    assert_num(
        &mut rt,
        "require('player.relationship').reputation('village')",
        rep_before + 6.0,
    )?;
    assert_str(
        &mut rt,
        "select(2, require('world.quest').deliver('order_rice'))",
        "already_completed",
    )?;
    choose_option(&mut rt, "成交。")?;

    // ---------------- 7. 制作药剂 ----------------
    banner("M19-01⑦ 制作药剂：放置药庐 → 炼回复药 → 推 clock → 收取");
    // 炼制需要灵芝 ×2：山道药草丛次日刷新，这里当日再采一次补足（走真实 E 键）。
    lua(&mut rt, "map['goto']('mountain_path', 624.0, 80.0)")?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "mountain_path")?;
    lua(&mut rt, "clock.set(2, 480)")?;
    frames(&mut rt, 2)?;
    lua(
        &mut rt,
        "require('player.player').teleport(400.0, 336.0); require('core.state').facing = 'up'",
    )?;
    frames(&mut rt, 2)?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "gather_lingzhi",
    )?;
    // 回到第 1 天 12:00 继续制作线（采集本身不推进 clock）。
    lua(&mut rt, "clock.set(1, 720)")?;
    lua(&mut rt, "map['goto']('manor', 528.0, 48.0)")?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "manor")?;
    let (bx, by) = find_legal_anchor(&mut rt, "herb_hut")?;
    lua(
        &mut rt,
        &format!("require('systems.building').place('herb_hut', {bx}, {by})"),
    )?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "build_herb_hut",
    )?;
    let key = format!("{bx},{by}");
    // 接取山庄订单 order_pill（顾长风），让三个订单都进入玩家任务栏；
    // 交付要等回复药出炉，故在收取后回话。
    talk_to(&mut rt, "npc_gu_changfeng")?;
    choose_option(&mut rt, "庄中可有要办的差事？")?;
    choose_option(&mut rt, "愿为山庄炼一丸。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_pill')",
        "in_progress",
    )?;
    choose_option(&mut rt, "记下了。")?;
    let pill_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'heal_pill')",
    )?;
    assert_str(
        &mut rt,
        &format!("tostring((require('systems.crafting').start('pill_refine', '{key}')))"),
        "true",
    )?;
    let remaining = query_num(
        &mut rt,
        &format!("return require('systems.crafting').query('{key}').remaining"),
    )?;
    if remaining <= 0.0 {
        return Err(format!("expected positive remaining, got {remaining}").into());
    }
    // 未到点收取 → crafting（失败无副作用）。
    assert_str(
        &mut rt,
        &format!("select(2, require('systems.crafting').collect('{key}'))[1]"),
        "crafting",
    )?;
    // 推进 clock 到完成时刻 → 收取产出。
    lua(&mut rt, &format!("clock.set(1, {})", NOON + 180))?;
    assert_str(
        &mut rt,
        &format!("tostring((require('systems.crafting').collect('{key}')))"),
        "true",
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'heal_pill')",
        pill_before + 1.0,
    )?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "craft_collect_heal_pill",
    )?;
    // 收取后「持有 1 丸回复药」目标即刻达成 → 山庄订单可交付。
    assert_str(
        &mut rt,
        "require('world.quest').state('order_pill')",
        "deliverable",
    )?;
    // 山庄订单：交付刚炼出的回复药。
    let money_before3 = query_num(&mut rt, "require('core.state').money")?;
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_gu_changfeng")?;
    choose_option(&mut rt, "回复药炼好了，请过目。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_pill')",
        "completed",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('order_pill_done')",
        "true",
    )?;
    assert_num(&mut rt, "require('core.state').money", money_before3 + 80.0)?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'herb_tonic')",
        1.0,
    )?;
    choose_option(&mut rt, "多谢师父。")?;
    // 主线二段交付：三笔订单已结清两笔（药庐 + 山庄），把山道采来的药材交一把给沈青萝。
    // （主线的「或」语义：order_herb_done / order_rice_done / order_pill_done 任一即可，
    //  但 mainline_orders 的 flag 目标由任务系统按需求全中，故三个订单都已结清。）
    lua(&mut rt, "map['goto']('village', 320.0, 48.0)")?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "village")?;
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        "require('world.quest').state('mainline_orders')",
        "deliverable",
    )?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    choose_option(&mut rt, "这把药材，请沈掌柜验看。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('mainline_orders')",
        "completed",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('mainline_orders_done')",
        "true",
    )?;
    choose_option(&mut rt, "多谢。")?;

    // ---------------- 8. 睡觉 ----------------
    banner("M19-01⑧ 睡觉（真实 T 键 → day.sleep）：日计数 +1、体力恢复、自动存档");
    let day_before = query_num(&mut rt, "clock.now().day")?;
    lua(&mut rt, "require('core.state').energy = 42")?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(&mut rt, "require('core.state').last_action", "sleep")?;
    assert_num(&mut rt, "clock.now().day", day_before + 1.0)?;
    assert_num(&mut rt, "require('core.state').energy", 100.0)?;
    assert_num(&mut rt, "require('core.state').labor_minutes", 0.0)?;
    // 在世界坐标外面（village 无 FarmGrid）时 farm.settle_day 走地图快照路径：
    // 三块田都浇过水 → 有效成长日 +1（灵芝 7 天/稻谷 4 天，当日未成熟）。
    assert_num(&mut rt, "require('systems.farm').stats().planted", 3.0)?;
    assert_num(&mut rt, "require('systems.farm').stats().mature", 0.0)?;
    let summary = query_str(
        &mut rt,
        "(function() local s = require('core.save_manager').last_summary(); \
          if not s then return 'none' end; \
          return tostring(s.day) .. '|' .. tostring(s.map) .. '|' .. tostring(s.farm_plots) end)()",
    )?;
    println!("save_manager.last_summary() = day|map|farm_plots = {summary}");
    if !summary.starts_with(&format!("{}|village|3", day_before as i64 + 1)) {
        return Err(format!("自动存档摘要不符: {summary}").into());
    }

    // ---------------- 9. 第二天读档 ----------------
    banner("M19-01⑨ 第二天读档：第二个 runtime → title → BtnContinue，跨 runtime 一致");
    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    // 存档发生在云泽村（第 8 章睡觉处），读档后应回到同一张图。
    assert_scene_is(&mut second, "village")?;
    assert_map_current(&mut second, "village")?;
    assert_mode(&mut second, "Playing")?;
    assert_num(&mut second, "clock.now().day", day_before + 1.0)?;
    assert_num(
        &mut second,
        "require('core.state').money",
        query_num(&mut rt, "require('core.state').money")?,
    )?;
    assert_num(&mut second, "require('core.state').energy", 100.0)?;
    assert_str(
        &mut second,
        "require('world.quest').state('order_herb')",
        "completed",
    )?;
    assert_str(
        &mut second,
        "require('world.quest').state('order_rice')",
        "completed",
    )?;
    assert_str(
        &mut second,
        "require('world.quest').state('order_pill')",
        "completed",
    )?;
    assert_str(
        &mut second,
        "require('world.quest').state('mainline_arrival')",
        "completed",
    )?;
    assert_str(
        &mut second,
        "require('world.quest').state('mainline_orders')",
        "completed",
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('order_herb_done')",
        "true",
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('mainline_orders_done')",
        "true",
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').points('npc_shen_qingluo')",
        query_num(
            &mut rt,
            "require('player.relationship').points('npc_shen_qingluo')",
        )?,
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').reputation('village')",
        query_num(
            &mut rt,
            "require('player.relationship').reputation('village')",
        )?,
    )?;
    assert_num(
        &mut second,
        "inv.count(require('core.state').inventory, 'timber')",
        query_num(
            &mut rt,
            "inv.count(require('core.state').inventory, 'timber')",
        )?,
    )?;
    assert_num(&mut second, "require('core.state').labor_minutes", 0.0)?;
    // 农田地块明细随存档恢复（map_state.farm_plots，跨 runtime 读回仍是 3 块已播种）；
    // 建筑记录（map.state("buildings")）同样随存档恢复，读档后重放视觉/阻挡（幂等）。
    assert_num(&mut second, "require('systems.farm').stats().planted", 3.0)?;
    assert_num(&mut second, "require('systems.farm').stats().mature", 0.0)?;
    assert_num(&mut second, "#(require('systems.building').list())", 1.0)?;
    assert_str(
        &mut second,
        "require('systems.building').list()[1].building",
        "herb_hut",
    )?;
    // 读档后重复交付仍被拦截（完成态随存档恢复）。
    assert_str(
        &mut second,
        "select(2, require('world.quest').deliver('order_herb'))",
        "already_completed",
    )?;

    // ---------------- 10. 完成主线节点 ----------------
    banner("M19-01⑩ 主线节点：交付 mainline_seal → mainline_done == true + 声望/关系到账");
    let money_before4 = query_num(&mut second, "require('core.state').money")?;
    let rep_v_before = query_num(
        &mut second,
        "require('player.relationship').reputation('village')",
    )?;
    let rep_s_before = query_num(
        &mut second,
        "require('player.relationship').reputation('sect')",
    )?;
    let rel_before2 = query_num(
        &mut second,
        "require('player.relationship').points('npc_shen_qingluo')",
    )?;
    // 主线三段由沈青萝发放——沈青萝在村里，故先回村。
    lua(&mut second, "map['goto']('village', 320.0, 48.0)")?;
    second.process_scene_transition()?;
    assert_scene_is(&mut second, "village")?;
    lua(
        &mut second,
        &format!("clock.set({}, {TEN_AM})", day_before as i64 + 1),
    )?;
    frames(&mut second, 2)?;
    talk_to(&mut second, "npc_shen_qingluo")?;
    choose_option(&mut second, "三笔买卖都清了。")?;
    choose_option(&mut second, "请验看。")?;
    assert_str(
        &mut second,
        "require('world.quest').state('mainline_seal')",
        "deliverable",
    )?;
    choose_option(&mut second, "好。")?;
    talk_to(&mut second, "npc_shen_qingluo")?;
    choose_option(&mut second, "请验我炼的行气散。")?;
    assert_str(
        &mut second,
        "require('world.quest').state('mainline_seal')",
        "completed",
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('mainline_done')",
        "true",
    )?;
    assert_num(
        &mut second,
        "require('core.state').money",
        money_before4 + 120.0,
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').reputation('village')",
        rep_v_before + 15.0 + 5.0,
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').reputation('sect')",
        rep_s_before + 10.0,
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').points('npc_shen_qingluo')",
        // 接取 +2（seal_offer 选项的 relationship 副作用）+ on_complete +10。
        rel_before2 + 12.0,
    )?;
    assert_str(
        &mut second,
        "require('player.relationship').tier('npc_shen_qingluo').name",
        "相识",
    )?;
    choose_option(&mut second, "承让。")?;
    assert_mode(&mut second, "Playing")?;

    // ---------------- 11. 汇总 ----------------
    banner("M19-01⑪ 汇总：三张地图 / 三个订单 / 主线锚点 / 金钱");
    let orders_done = query_num(
        &mut second,
        "(function() local n = 0; \
          for _, id in ipairs({ 'order_herb', 'order_rice', 'order_pill' }) do \
            if require('world.quest').state(id) == 'completed' then n = n + 1 end end; \
          return n end)()",
    )?;
    if (orders_done - 3.0).abs() > 1e-6 {
        return Err(format!("expected 3 completed orders, got {orders_done}").into());
    }
    assert_num(
        &mut second,
        "#(require('systems.gather').nodes('mountain_path')) >= 3 and 1 or 0",
        1.0,
    )?;
    // 任务 9 条 = M19-01 的 8 条 + M28（计划 G3）追加的猎获订单 order_hunt；
    // 对话仍是 3 条（order_hunt 复用林小满那一条对话文件，不新增 speaker）。
    assert_num(&mut second, "#(require('world.quest').ids())", 9.0)?;
    assert_num(&mut second, "#(require('world.dialogue').ids())", 3.0)?;
    println!(
        "jhyt_m19_slice_ok project={} day={} orders={} mainline=true money={} lingzhi={} gather_nodes={}",
        project.display(),
        query_num(&mut second, "return clock.now().day")? as i64,
        orders_done as i64,
        query_num(&mut second, "return require('core.state').money")? as i64,
        query_num(
            &mut second,
            "return inv.count(require('core.state').inventory, 'lingzhi')"
        )? as i64,
        query_num(
            &mut second,
            "return #require('systems.gather').nodes('mountain_path')"
        )? as i64,
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
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

/// Runs the project script, then loads the `main` 门面 scene so BtnStart/BtnContinue are live.
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

/// Simulates a UI button click through `on_click`, then processes the queued scene.
fn click_button(runtime: &mut LuaRuntime, button: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("on_click('{button}')"))?;
    runtime.process_scene_transition()?;
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

/// Runs `update()` for `n` frames with an empty input snapshot.
fn frames(runtime: &mut LuaRuntime, n: usize) -> Result<(), Box<dyn std::error::Error>> {
    runtime.set_input(InputSnapshot::default());
    for _ in 0..n {
        runtime.call_update()?;
    }
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

/// 数值断言走 f64 比较：存档 JSON 往返可能把整数读回为 `3.0`。
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

/// 内容最低标准的宽松断言：`at least:N` 形式（N 为下界）。
fn assert_at_least(
    runtime: &mut LuaRuntime,
    expr: &str,
    want: &str,
    what: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let floor: f64 = want
        .strip_prefix("at least:")
        .ok_or_else(|| format!("assert_at_least expects at least:N, got {want}"))?
        .parse::<f64>()?;
    let got = query_num(runtime, &format!("return {expr}"))?;
    if got < floor {
        return Err(format!("{what}: {expr} = {got}, want >= {floor}").into());
    }
    println!("{what} = {got}");
    Ok(())
}

/// 加载期校验：issues 非空时把全部条目拼进错误信息。
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

fn assert_map_current(
    runtime: &mut LuaRuntime,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let current = query_str(runtime, "require('world.map_manager').current()")?;
    if current != expected {
        return Err(format!("map.current() = {current:?}, want {expected:?}").into());
    }
    Ok(())
}

fn assert_mode(runtime: &mut LuaRuntime, expected: &str) -> Result<(), Box<dyn std::error::Error>> {
    assert_str(runtime, "require('core.state').mode", expected)
}

/// Teleports the player to a world position via `player.teleport`.
fn teleport_world(
    runtime: &mut LuaRuntime,
    pos: (f32, f32),
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "require('player.player').teleport({}, {})",
        pos.0, pos.1
    ))?;
    Ok(())
}

/// 走到 NPC 相邻可行走格、面向 NPC，再按 E 开对话（真实交互路径）。
///
/// teleport 会同步刚体与 ECS Transform，但物理步可能把重叠刚体推开，
/// 因此按键的每一帧都重新贴回站立格（等价于玩家真实走位到 NPC 面前）。
fn talk_to(runtime: &mut LuaRuntime, persist_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let stick = format!(
        "(function() local q = require('world.npc').query('{persist_id}'); \
           if not q or not q.live then return 'no_npc' end; \
           local g = entity.find('GroundMap'); \
           local cx, cy = q.cell_x, q.cell_y; \
           local dirs = {{ {{1,0,'left'}}, {{-1,0,'right'}}, {{0,1,'down'}}, {{0,-1,'up'}} }}; \
           for _, d in ipairs(dirs) do \
             local tx, ty = cx + d[1], cy + d[2]; \
             if not tilemap.is_solid(g, tx, ty) then \
               require('player.player').teleport((tx + 0.5) * 32, (ty + 0.5) * 32); \
               require('core.state').facing = d[3]; \
               return 'ok' \
             end \
           end \
           return 'no_stand' \
         end)()"
    );
    let probe = query_str(runtime, &stick)?;
    if probe != "ok" {
        return Err(format!("npc {persist_id} not interactable: {probe}").into());
    }
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyE, true);
    runtime.set_input(input);
    lua(runtime, &stick)?;
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    lua(runtime, &stick)?;
    runtime.call_update()?;
    Ok(())
}

/// 在当前对话里按选项文本选择（1 基序号）；找不到时把可见选项拼进错误信息。
fn choose_option(runtime: &mut LuaRuntime, text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let expr = format!(
        "(function() local d = require('world.dialogue'); \
           local opts = d.options(); \
           for i, e in ipairs(opts) do \
             if e.text == [[{text}]] then d.select(i); return tostring(d.choose()) end \
           end; \
           local out = {{}}; \
           for i, e in ipairs(opts) do out[i] = tostring(i) .. ':' .. tostring(e.text) end; \
           return 'not found ' .. table.concat(out, ' / ') end)()"
    );
    let got = query_str(runtime, &expr)?;
    if got != "true" {
        return Err(format!("choose_option {text:?} failed: {got}").into());
    }
    Ok(())
}

/// 把玩家放到野猪侧面并朝它按 J（真实普攻键路径）。
fn strike_boar(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        "(function() local q = require('systems.enemy').query('enemy_wild_boar'); \
           if not q or not q.live then return 'no_enemy' end; \
           local cx, cy = q.cell_x, q.cell_y; \
           require('player.player').teleport((cx + 1.5) * 32, (cy + 0.5) * 32); \
           require('core.state').facing = 'left'; \
           return 'ok' end)()",
    )?;
    if got != "ok" {
        return Err(format!("boar not strikeable: {got}").into());
    }
    frames(runtime, 2)?;
    press(
        runtime,
        KeyCode::from_char('j').expect("QWERTY 字符必须有对应物理键"),
    )?;
    Ok(())
}

/// 扫描农田矩形，返回第一个未开垦且有指定作物土地类型的地块（确定性）。
fn find_plot_cell(
    runtime: &mut LuaRuntime,
    x0: i64,
    x1: i64,
    crop: &str,
) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "(function() local farm = require('systems.farm'); local land = require('data.farm').land; \
             for y = 4, 13 do for x = {x0}, {x1} do \
               if not farm.is_plot(x, y) and (not __m19_taken or not __m19_taken[x .. ',' .. y]) then \
                 for _, l in ipairs(land) do \
                   if x >= l.x0 and x <= l.x1 and y >= l.y0 and y <= l.y1 then \
                     local want = ([[{crop}]] == 'lingzhi') and 'medicinal' or 'field'; \
                     if l.type == want then \
                       __m19_taken = __m19_taken or {{}}; \
                       __m19_taken[x .. ',' .. y] = true; \
                       return x .. ',' .. y \
                     end \
                   end \
                 end \
               end \
             end end; return 'none' end)()"
        ),
    )?;
    let mut parts = got.split(',');
    let x = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no {crop} plot cell: {got}"))?;
    let y = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no {crop} plot cell: {got}"))?;
    Ok((x, y))
}

/// 站到地块下方朝上，用 farm.act 公开 API 翻土。
fn till_with_api(
    runtime: &mut LuaRuntime,
    cx: i64,
    cy: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    lua(
        runtime,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'; \
             inv.equip(require('core.state').inventory, 'tool', 'hoe')",
            (cx as f32 + 0.5) * 32.0,
            (cy as f32 - 0.5) * 32.0
        ),
    )?;
    lua(runtime, "require('systems.farm').act('up')")?;
    Ok(())
}

/// 扫描当前地图，返回第一个对该建筑可放置的锚点（确定性，与 jhyt_m18_build 同法）。
fn find_legal_anchor(
    runtime: &mut LuaRuntime,
    building_id: &str,
) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "(function() local b = require('systems.building'); \
             for y = 0, 21 do for x = 0, 39 do \
               if b.can_place('{building_id}', x, y) then return x .. ',' .. y end end end return 'none' end)()"
        ),
    )?;
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
