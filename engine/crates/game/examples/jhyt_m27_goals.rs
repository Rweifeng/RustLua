//! Headless acceptance drive for M27 / 计划 G1：七日目标与日结算联动
//! （`data/goals.lua` + `systems/goals.lua` + HUD 目标行）。
//!
//! 目标系统存在的理由：MVP 剧本（docs/46）是一条线性流程，但玩家进游戏后
//! **不知道今天该干什么** —— 只有一次性新手引导（10 步），引导走完 HUD 只剩一行
//! `last_action` 反馈。本样例把"七日目标"当作可验收行为来驱动：
//!
//! 1. 目标表结构与校验（含负例：缺 `day_start`、id 重复、非法 kind、奖励越界）；
//! 2. 第 1–7 天**全部用真实玩法**推进（E 键种田/采集、J 键战斗、T 键睡觉、
//!    对话接任务、商铺购买、盖建筑、炼药收取），逐条断言目标达成；
//! 3. 日结算联动：睡觉跨天 → 当天结算结果 + 奖励入账（幂等，不重复发）；
//! 4. 七日里程碑：第 7 天走完 → HUD 换收尾文案 + 里程碑奖励发一次；
//! 5. 稳健性：漏做目标的那天记为 missed 且不发奖、存档往返、新档复位到第 1 天。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m27_goals [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 山庄出生点（也是村庄回山庄的落点）。
const MANOR_POS: (f32, f32) = (528.0, 48.0);
/// 云泽村入口落点。
const VILLAGE_POS: (f32, f32) = (528.0, 48.0);
/// 青云山道入口落点。
const MOUNTAIN_POS: (f32, f32) = (624.0, 80.0);
/// 演武场落地坐标。
const ARENA_POS: (f32, f32) = (304.0, 176.0);
/// 演武场木人桩左侧站位（朝右）。
const BEFORE_DUMMY: (f32, f32) = (432.0, 176.0);
/// 10:00：顾长风在剑坪、沈青萝在医馆（NPC 日程）。
const TEN_AM: i64 = 600;
/// 中午（睡觉前的时刻）。
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
    let save_path = fresh_save_path(&project, "jhyt_m27_goals.sav");
    let project_arg = project.clone();

    // ---------------- ① 目标表结构与校验 ----------------
    banner("M27-01① 七日目标表：7 天 × 每天 3–4 条，结构与交叉引用全部可校验");
    let mut rt = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;
    assert_num(&mut rt, "require('data.goals').total_days()", 7.0)?;
    assert_str(
        &mut rt,
        "(function() local g = require('data.goals'); \
           for day = 1, g.total_days() do \
             local d = g.day(day); \
             if type(d) ~= 'table' or type(d.goals) ~= 'table' then return 'day' .. day .. ':missing' end; \
             if #d.goals < 3 then return 'day' .. day .. ':too_few' end; \
             local last = d.goals[#d.goals]; \
             if last.match.kind ~= 'day_start' then return 'day' .. day .. ':no_day_start' end; \
           end; return 'ok' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local n = 0; local seen = {}; local g = require('data.goals'); \
           for day = 1, g.total_days() do \
             for _, goal in ipairs(g.day(day).goals) do \
               if seen[goal.id] then return 'dup:' .. goal.id end; \
               seen[goal.id] = true; n = n + 1 \
             end \
           end; return n >= 21 and 'ok' or ('n=' .. n) end)()",
        "ok",
    )?;
    // 校验器已登记（`jhyt_m23_data_validation` 的覆盖断言也覆盖它）。
    assert_str(
        &mut rt,
        "(function() for _, e in ipairs(require('validate_data').registered()) do \
           if e.path == 'data.goals' then return 'ok' end end return 'missing' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local report = require('validate_data').check({ strict = true }); \
           return report == '' and 'ok' or report end)()",
        "ok",
    )?;
    // 负例：结构不变量必须真的被检查（不是写着好看）。
    assert_str(
        &mut rt,
        "(function() local g = require('data.goals'); local saved = g.days[2].goals[#g.days[2].goals]; \
           g.days[2].goals[#g.days[2].goals] = { id = 'tmp_bad', text = '缺 day_start', \
             match = { kind = 'action', value = 'till' } }; \
           local ok, issues = g.validate_all(); \
           g.days[2].goals[#g.days[2].goals] = saved; \
           for _, i in ipairs(issues or {}) do \
             if i:find('day_start', 1, true) then return 'ok' end \
           end; \
           return 'missing:not_reported' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local g = require('data.goals'); local saved = g.days[1].goals[2].id; \
           g.days[1].goals[2].id = g.days[1].goals[1].id; \
           local ok, issues = g.validate_all(); \
           g.days[1].goals[2].id = saved; \
           for _, i in ipairs(issues or {}) do \
             if i:find('重复', 1, true) then return 'ok' end \
           end; \
           return 'missing:not_reported' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local g = require('data.goals'); local saved = g.days[3].goals[1].match.kind; \
           g.days[3].goals[1].match.kind = 'teleport'; \
           local ok, issues = g.validate_all(); \
           g.days[3].goals[1].match.kind = saved; \
           for _, i in ipairs(issues or {}) do \
             if i:find('kind', 1, true) then return 'ok' end \
           end; \
           return 'missing:not_reported' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local g = require('data.goals'); local saved = g.days[1].reward; \
           g.days[1].reward = { item = 'seed_rice', count = 1, money = 10 }; \
           local ok, issues = g.validate_all(); \
           g.days[1].reward = saved; \
           for _, i in ipairs(issues or {}) do \
             if i:find('只能给出', 1, true) then return 'ok' end \
           end; \
           return 'missing:not_reported' end)()",
        "ok",
    )?;
    // 新档：第 1 天、零进度、无结算、无里程碑。
    assert_num(&mut rt, "require('systems.goals').day()", 1.0)?;
    assert_num(&mut rt, "require('systems.goals').completed_days()", 0.0)?;
    assert_str(
        &mut rt,
        "require('systems.goals').milestone_complete()",
        "false",
    )?;
    assert_str(
        &mut rt,
        "require('systems.goals').prompt():find('第 1 天', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;
    // HUD 目标行与目标系统同一来源。
    assert_str(
        &mut rt,
        "require('ui.hud').goals_line():find('目标', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;
    let day1_goals = query_num(&mut rt, "require('systems.goals').goal_count(1)")?;
    println!("第 1 天目标数 = {day1_goals}");

    // ---------------- ② 第 1 天：真实 E 键走完动作目标 ----------------
    banner("M27-01② 第 1 天：翻土→播种→浇水（真实 E 键）+ 睡觉结算");
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    let (fx, fy) = find_plot_cell(&mut rt, 14, 23, "lingzhi")?;
    // 站到地块下方朝上（farm 的朝向约定：up = cell y 更小）。
    lua(
        &mut rt,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'; \
             inv.equip(require('core.state').inventory, 'tool', 'hoe')",
            (fx as f32 + 0.5) * 32.0,
            (fy as f32 + 1.5) * 32.0
        ),
    )?;
    frames(&mut rt, 2)?;
    assert_goal(&mut rt, "d1_till", false)?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(&mut rt, "require('core.state').last_action", "till")?;
    assert_goal(&mut rt, "d1_till", true)?;

    lua(
        &mut rt,
        "require('core.state').selected_seed = 'seed_lingzhi'",
    )?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "plant_seed_lingzhi",
    )?;
    assert_goal(&mut rt, "d1_plant", true)?;
    // 钉住晴天（M29 起天气按 `(seed, day, 季节)` 抽签）：雨天是 `rain_watered`，
    // 而这里的目标判定与断言按 `water_lingzhi` 写。
    lua(&mut rt, "require('world.weather').set('sunny')")?;
    lua(
        &mut rt,
        "inv.equip(require('core.state').inventory, 'tool', 'watering_can')",
    )?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "water_lingzhi",
    )?;
    assert_goal(&mut rt, "d1_water", true)?;
    assert_num(
        &mut rt,
        "require('systems.goals').status().done_count",
        day1_goals - 1.0,
    )?;
    // 睡觉前：跨天目标仍未完成（它由日结算事件本身达成，不能提前变绿）。
    assert_goal(&mut rt, "d1_sleep", false)?;
    let seed_rice_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'seed_rice')",
    )?;
    lua(&mut rt, &format!("clock.set(1, {NOON})"))?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(&mut rt, "require('core.state').last_action", "sleep")?;
    assert_num(&mut rt, "clock.now().day", 2.0)?;
    // 日结算：第 1 天 completed + 奖励 seed_rice ×2；目标日与游戏日同步推进。
    assert_num(&mut rt, "require('systems.goals').day()", 2.0)?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('goals_day_1')",
        "completed",
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'seed_rice')",
        seed_rice_before + 2.0,
    )?;
    assert_str(
        &mut rt,
        "require('ui.hud').goals_line():find('第 2 天', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;
    // 幂等：重复结算同一天不再发奖（奖励锚点变量）。
    let seed_rice_after = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'seed_rice')",
    )?;
    assert_str(
        &mut rt,
        "(function() local s = require('core.state'); s.set_var('goals_day', 1, 'test', 'replay'); \
           require('systems.goals').on_day_start(); \
           s.set_var('goals_day', 2, 'test', 'resync'); return 'ok' end)()",
        "ok",
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'seed_rice')",
        seed_rice_after,
    )?;
    assert_num(&mut rt, "require('systems.goals').completed_days()", 1.0)?;

    // ---------------- ③ 第 2 天：进村 + 采集 ----------------
    banner("M27-01③ 第 2 天：进云泽村（状态类目标进图即判定）+ 回山庄采集");
    lua(&mut rt, &format!("clock.set(2, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    teleport_world(&mut rt, MANOR_POS)?;
    lua(&mut rt, "require('core.state').facing = 'down'")?;
    press(&mut rt, KeyCode::KeyE)?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "village")?;
    assert_map_current(&mut rt, "village")?;
    // enter_map 类目标是状态查询：进图那一刻就已经达成，不需要再往返一次。
    assert_goal(&mut rt, "d2_village", true)?;
    assert_goal(&mut rt, "d2_gather", false)?;
    // 采集目标不限定地图（云泽村没有采集点，山庄药圃西侧有一处一次性野采）。
    goto_map(&mut rt, "manor", MANOR_POS)?;
    gather_once(&mut rt, "manor")?;
    assert_goal(&mut rt, "d2_gather", true)?;
    lua(&mut rt, &format!("clock.set(2, {NOON})"))?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "clock.now().day", 3.0)?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('goals_day_2')",
        "completed",
    )?;
    assert_num(&mut rt, "require('systems.goals').completed_days()", 2.0)?;

    // ---------------- ④ 第 3 天：主线复命 + 购买 ----------------
    banner("M27-01④ 第 3 天：向顾长风复命（对话交付主线）+ 商铺购买");
    goto_map(&mut rt, "manor", MANOR_POS)?;
    assert_scene_is(&mut rt, "manor")?;
    lua(&mut rt, &format!("clock.set(3, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_goal(&mut rt, "d3_mainline", false)?;
    talk_to(&mut rt, "npc_gu_changfeng")?;
    choose_option(&mut rt, "弟子新来乍到，请师父吩咐。")?;
    choose_option(&mut rt, "弟子遵命。")?;
    choose_option(&mut rt, "是。")?;
    talk_to(&mut rt, "npc_gu_changfeng")?;
    choose_option(&mut rt, "药田翻好了，规矩也已记下。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('mainline_arrival')",
        "completed",
    )?;
    // 主线交付是"轮询类"目标：下一帧轮询即变绿（不依赖 last_action）。
    frames(&mut rt, 2)?;
    assert_goal(&mut rt, "d3_mainline", true)?;
    // 购买：走 shop.buy 真实入口（同一入口写出 last_action = buy_seed_rice）。
    goto_map(&mut rt, "village", VILLAGE_POS)?;
    assert_scene_is(&mut rt, "village")?;
    lua(&mut rt, &format!("clock.set(3, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    lua(&mut rt, "require('core.state').money = 500")?;
    assert_goal(&mut rt, "d3_buy", false)?;
    assert_str(
        &mut rt,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_rice', 1))",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "buy_seed_rice",
    )?;
    frames(&mut rt, 1)?;
    assert_goal(&mut rt, "d3_buy", true)?;
    let power_before = query_num(&mut rt, "require('core.state').inner_power")?;
    lua(&mut rt, &format!("clock.set(3, {NOON})"))?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "clock.now().day", 4.0)?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('goals_day_3')",
        "completed",
    )?;
    assert_num(
        &mut rt,
        "require('core.state').inner_power",
        power_before + 10.0,
    )?;

    // ---------------- ⑤ 第 4 天：入山采药两次 ----------------
    banner("M27-01⑤ 第 4 天：入青云山道 + 采集 2 次（计数目标需两次不同事件）");
    goto_map(&mut rt, "mountain_path", MOUNTAIN_POS)?;
    assert_scene_is(&mut rt, "mountain_path")?;
    assert_map_current(&mut rt, "mountain_path")?;
    lua(&mut rt, &format!("clock.set(4, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_goal(&mut rt, "d4_mountain", true)?;
    assert_goal(&mut rt, "d4_gather", false)?;
    assert_num(
        &mut rt,
        "require('systems.goals').status().goals[2].count",
        2.0,
    )?;
    gather_once(&mut rt, "mountain_path")?;
    assert_num(
        &mut rt,
        "require('systems.goals').status().goals[2].progress",
        1.0,
    )?;
    assert_goal(&mut rt, "d4_gather", false)?;
    gather_once(&mut rt, "mountain_path")?;
    assert_goal(&mut rt, "d4_gather", true)?;
    let lingzhi_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lingzhi')",
    )?;
    // 回山庄睡觉（真实 T 键）。
    goto_map(&mut rt, "manor", MANOR_POS)?;
    assert_scene_is(&mut rt, "manor")?;
    lua(&mut rt, &format!("clock.set(4, {NOON})"))?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "clock.now().day", 5.0)?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('goals_day_4')",
        "completed",
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lingzhi')",
        lingzhi_before + 1.0,
    )?;

    // ---------------- ⑥ 第 5 天：接订单 + 盖药庐 ----------------
    banner("M27-01⑥ 第 5 天：沈青萝的订单（对话接取）+ 盖起药庐");
    goto_map(&mut rt, "village", VILLAGE_POS)?;
    assert_scene_is(&mut rt, "village")?;
    lua(&mut rt, &format!("clock.set(5, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_goal(&mut rt, "d5_order", false)?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    choose_option(&mut rt, "听说药庐常年收灵芝？")?;
    choose_option(&mut rt, "这单我接了。")?;
    // 第 4 天已经采到灵芝，订单目标可能当场就满足 → 状态可能是 in_progress 或
    // deliverable；这里只断言"已接取"（目标自身的判定口径）。
    assert_str(
        &mut rt,
        "require('world.quest').state('order_herb') ~= 'not_started' and 'ok' or 'none'",
        "ok",
    )?;
    frames(&mut rt, 2)?;
    assert_goal(&mut rt, "d5_order", true)?;
    lua(&mut rt, "require('world.dialogue').choose()")?;
    // 回山庄盖药庐（真实建筑入口：building.place 写出 last_action = build_<id>）。
    goto_map(&mut rt, "manor", MANOR_POS)?;
    assert_scene_is(&mut rt, "manor")?;
    lua(&mut rt, &format!("clock.set(5, {NOON})"))?;
    frames(&mut rt, 2)?;
    let (bx, by) = find_legal_anchor(&mut rt, "herb_hut")?;
    assert_goal(&mut rt, "d5_build", false)?;
    lua(
        &mut rt,
        &format!("require('systems.building').place('herb_hut', {bx}, {by})"),
    )?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "build_herb_hut",
    )?;
    frames(&mut rt, 1)?;
    assert_goal(&mut rt, "d5_build", true)?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "clock.now().day", 6.0)?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('goals_day_5')",
        "completed",
    )?;
    assert_num(&mut rt, "require('core.state').money", money_before + 50.0)?;

    // ---------------- ⑦ 第 6 天：演武场试手 ----------------
    banner("M27-01⑦ 第 6 天：去演武场 + 击败木人桩（真实 J 键）");
    goto_map(&mut rt, "combat_arena", ARENA_POS)?;
    assert_scene_is(&mut rt, "combat_arena")?;
    assert_map_current(&mut rt, "combat_arena")?;
    lua(&mut rt, &format!("clock.set(6, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_goal(&mut rt, "d6_arena", true)?;
    assert_goal(&mut rt, "d6_kill", false)?;
    assert_num(&mut rt, "require('systems.combat').stats().kills", 0.0)?;
    teleport_world(&mut rt, BEFORE_DUMMY)?;
    lua(&mut rt, "require('core.state').facing = 'right'")?;
    frames(&mut rt, 2)?;
    let mut killed = false;
    for _ in 0..30 {
        press(
            &mut rt,
            KeyCode::from_char('j').expect("QWERTY 字符必须有对应物理键"),
        )?;
        frames(&mut rt, 30)?;
        if query_num(&mut rt, "require('systems.combat').stats().kills")? >= 1.0 {
            killed = true;
            break;
        }
    }
    if !killed {
        return Err("30 次普攻都没能击败木人桩".into());
    }
    assert_goal(&mut rt, "d6_kill", true)?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "clock.now().day", 7.0)?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('goals_day_6')",
        "completed",
    )?;
    assert_num(&mut rt, "require('core.state').money", money_before + 60.0)?;

    // ---------------- ⑧ 第 7 天：炼药 + 七日里程碑 ----------------
    banner("M27-01⑧ 第 7 天：炼成回气散 + 睡满第七夜 → 七日之约");
    goto_map(&mut rt, "manor", MANOR_POS)?;
    assert_scene_is(&mut rt, "manor")?;
    lua(&mut rt, &format!("clock.set(7, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_goal(&mut rt, "d7_collect", false)?;
    assert_goal(&mut rt, "d7_pill", false)?;
    let key = format!("{bx},{by}");
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
    lua(&mut rt, &format!("clock.set(7, {})", TEN_AM + 180))?;
    assert_str(
        &mut rt,
        &format!("tostring((require('systems.crafting').collect('{key}')))"),
        "true",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "craft_collect_heal_pill",
    )?;
    frames(&mut rt, 1)?;
    assert_goal(&mut rt, "d7_collect", true)?;
    assert_goal(&mut rt, "d7_pill", true)?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    lua(&mut rt, &format!("clock.set(7, {NOON})"))?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "clock.now().day", 8.0)?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('goals_day_7')",
        "completed",
    )?;
    assert_num(&mut rt, "require('systems.goals').completed_days()", 7.0)?;
    assert_str(
        &mut rt,
        "require('systems.goals').milestone_complete()",
        "true",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('goals_milestone')",
        "completed",
    )?;
    // 第 7 天奖励（80）+ 里程碑奖励（200）都已入账。
    assert_num(
        &mut rt,
        "require('core.state').money",
        money_before + 80.0 + 200.0,
    )?;
    assert_str(&mut rt, "require('systems.goals').arc_done()", "true")?;
    assert_str(
        &mut rt,
        "require('ui.hud').goals_line():find('七日之约已达成', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;
    println!(
        "七日汇总：{}",
        query_str(&mut rt, "require('systems.goals').prompt()")?
    );
    // 里程碑奖励幂等：再次跨天不再发钱，达标天数也不变。
    let money_final = query_num(&mut rt, "require('core.state').money")?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "require('core.state').money", money_final)?;
    assert_num(&mut rt, "require('systems.goals').completed_days()", 7.0)?;
    let day_after_arc = query_num(&mut rt, "clock.now().day")?;
    // 目标日封顶在 7+1=8（"目标线结束"），游戏日继续走；两者只在 1..7 内同步。
    assert_str(&mut rt, "require('systems.goals').arc_done()", "true")?;
    assert_num(&mut rt, "require('systems.goals').day()", 8.0)?;

    // ---------------- ⑨ 存档往返 / 新档隔离 / 遗漏一天 ----------------
    banner("M27-01⑨ 存档往返：目标进度与结算结果跟着 state provider 一起回来");
    lua(&mut rt, "require('core.save_manager').save()")?;
    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_num(&mut second, "clock.now().day", day_after_arc)?;
    assert_str(&mut second, "require('systems.goals').arc_done()", "true")?;
    assert_num(&mut second, "require('systems.goals').day()", 8.0)?;
    assert_num(
        &mut second,
        "require('systems.goals').completed_days()",
        7.0,
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('goals_day_7')",
        "completed",
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('goals_milestone_reward')",
        "true",
    )?;
    assert_str(
        &mut second,
        "require('ui.hud').goals_line():find('七日之约已达成', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;
    // 遗漏的一天：回标题开新档，什么都不做直接睡 → 第 1 天 missed、不发奖。
    // （标题面板的 BtnExit 是"退出游戏"，返回标题走 pause/门面共用的
    // `jianghu_return_to_title()` —— 与 jhyt_m22_save_isolation 同一路径。）
    lua(&mut second, "assert(jianghu_return_to_title())")?;
    second.process_scene_transition()?;
    assert_scene_is(&mut second, "main")?;
    click_button(&mut second, "BtnStart")?;
    assert_scene_is(&mut second, "manor")?;
    assert_num(&mut second, "require('systems.goals').day()", 1.0)?;
    assert_num(
        &mut second,
        "require('systems.goals').completed_days()",
        0.0,
    )?;
    assert_num(&mut second, "require('systems.goals').missed_days()", 0.0)?;
    let seed_rice_start = query_num(
        &mut second,
        "inv.count(require('core.state').inventory, 'seed_rice')",
    )?;
    lua(&mut second, &format!("clock.set(1, {NOON})"))?;
    press(
        &mut second,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut second, "clock.now().day", 2.0)?;
    assert_num(&mut second, "require('systems.goals').day()", 2.0)?;
    assert_str(
        &mut second,
        "require('core.state').get_var('goals_day_1')",
        "missed",
    )?;
    assert_num(&mut second, "require('systems.goals').missed_days()", 1.0)?;
    assert_num(
        &mut second,
        "inv.count(require('core.state').inventory, 'seed_rice')",
        seed_rice_start,
    )?;
    // 第 2 天的目标照常给出（漏一天不会让整条目标线失效）。
    assert_str(
        &mut second,
        "require('systems.goals').prompt():find('第 2 天', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;

    println!(
        "jhyt_m27_goals_ok project={} days={} completed={} missed={} money={} heal_pill={} milestone=true",
        project.display(),
        query_num(&mut second, "require('systems.goals').total_days()")? as i64,
        query_num(&mut second, "require('systems.goals').completed_days()")? as i64,
        query_num(&mut second, "require('systems.goals').missed_days()")? as i64,
        query_num(&mut rt, "require('core.state').money")? as i64,
        query_num(
            &mut rt,
            "inv.count(require('core.state').inventory, 'heal_pill')"
        )? as i64,
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- 目标断言 ----------

/// 断言当前目标日某条目标的达成状态（读 `systems.goals` 的公开状态）。
fn assert_goal(
    runtime: &mut LuaRuntime,
    goal_id: &str,
    expected: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let expr = format!(
        "(function() for _, g in ipairs(require('systems.goals').status().goals) do \
           if g.id == '{goal_id}' then return tostring(g.done) end end return 'missing' end)()"
    );
    let got = query_str(runtime, &expr)?;
    let want = if expected { "true" } else { "false" };
    if got != want {
        let status = query_str(runtime, "require('systems.goals').prompt()")?;
        return Err(format!("目标 {goal_id} done = {got:?}, want {want:?}（{status}）").into());
    }
    Ok(())
}

/// 通过引擎的地图跳转入口切图（与 `map_manager` 出口一致的真实路径）。
fn goto_map(
    runtime: &mut LuaRuntime,
    map_id: &str,
    pos: (f32, f32),
) -> Result<(), Box<dyn std::error::Error>> {
    lua(
        runtime,
        &format!("map['goto']('{map_id}', {}, {})", pos.0, pos.1),
    )?;
    runtime.process_scene_transition()?;
    Ok(())
}

/// 在当前地图采集一次（真实 E 键）：找一个尚未采集的采集点，站到它旁边按 E。
fn gather_once(runtime: &mut LuaRuntime, map_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let probe = query_str(
        runtime,
        &format!(
            "(function() local g = require('systems.gather'); \
               local grid = entity.find('GroundMap'); \
               for _, node in ipairs(g.nodes('{map_id}')) do \
                 if not g.is_gathered(node.id) then \
                   local cx, cy = node.cell.x, node.cell.y; \
                   local dirs = {{ {{0,-1,'up'}}, {{0,1,'down'}}, {{1,0,'left'}}, {{-1,0,'right'}} }}; \
                   for _, delta in ipairs(dirs) do \
                     local sx, sy = cx + delta[1], cy + delta[2]; \
                     if not tilemap.is_solid(grid, sx, sy) then \
                       require('player.player').teleport((sx + 0.5) * 32, (sy + 0.5) * 32); \
                       require('core.state').facing = delta[3]; \
                       return 'ready:' .. node.id \
                     end \
                   end \
                 end \
               end; return 'none' end)()"
        ),
    )?;
    if !probe.starts_with("ready:") {
        return Err(format!("{map_id} 没有可采集的采集点（{probe}）").into());
    }
    frames(runtime, 2)?;
    press(runtime, KeyCode::KeyE)?;
    let action = query_str(runtime, "require('core.state').last_action")?;
    if !action.starts_with("gather_") {
        return Err(format!("采集失败：last_action = {action:?}（{probe}）").into());
    }
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

/// 在当前对话里按选项文本选择；找不到时把可见选项拼进错误信息。
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
               if not farm.is_plot(x, y) and (not __m27_taken or not __m27_taken[x .. ',' .. y]) then \
                 for _, l in ipairs(land) do \
                   if x >= l.x0 and x <= l.x1 and y >= l.y0 and y <= l.y1 then \
                     local want = ([[{crop}]] == 'lingzhi') and 'medicinal' or 'field'; \
                     if l.type == want then \
                       __m27_taken = __m27_taken or {{}}; \
                       __m27_taken[x .. ',' .. y] = true; \
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
