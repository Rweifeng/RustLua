//! Headless acceptance drive for M28 / 计划 G3：战斗 → 掉落 → 死亡回庄 → 任务端到端可达。
//!
//! 阶段四此前只剩这一项：「战斗、掉落、死亡回庄、任务和主线订单端到端可达」。落地内容：
//!
//! 1. `data/drops.lua`：敌人掉落表（铜钱区间 + 物品区间/概率），结构可校验（含负例）；
//!    并与 `docs/33` §6.2「MVP 落地口径」表逐格比对（文档漂移会直接失败）；
//! 2. `systems/loot.lua`：击杀掉落结算 —— 铜钱当场入账、物品进"当日待入库"袋，
//!    跨天 `bank()` 入库；随机序列持久化，同一存档同一击杀顺序 → 同一掉落；
//! 3. `systems/death.lua`：战败结算（损失待入库掉落 50% + 当前铜钱 5%）→ 回山庄 →
//!    生命/体力/内力回满（docs/33 §6.3）；
//! 4. 新订单 `order_hunt`（山道猎获）：消费掉落物，把"打猎"接进经济与任务链。
//!
//! 验收策略：**全部走真实玩法路径**——真实 J 键击杀、真实 T 键睡觉跨天、真实被敌人
//! 咬死（先把气血压到 3 点，再让玄冥会徒的第一次攻击打死）、真实 R 键战败重开、
//! 真实对话交付订单。只有 NPC 日程/掉落种子这类"环境变量"由测试显式固定。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m28_loop [-- <game_jhyt-root>]

use rustlua_input::input::{InputSnapshot, KeyCode};
use rustlua_scripting::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 山庄出生点（也是战败回庄的落点）。
const MANOR_POS: (f32, f32) = (528.0, 48.0);
/// 云泽村入口落点。
const VILLAGE_POS: (f32, f32) = (528.0, 48.0);
/// 青云山道入口落点。
const MOUNTAIN_POS: (f32, f32) = (624.0, 80.0);
/// 演武场落地坐标。
const ARENA_POS: (f32, f32) = (304.0, 176.0);
/// 10:00：NPC 日程里的"在岗"时段（顾长风剑坪、沈青萝医馆、林小满茶园）。
const TEN_AM: i64 = 600;
/// 中午（睡觉前的时刻）。
const NOON: i64 = 720;
/// 灰狼（演武场）：掉落狼皮 ×1，用种子 1 时同时掉 2 文铜钱。
const WOLF_SEED: i64 = 1;
/// 野猪（青云山道）：种子 58 → 铜钱 6、兽肉 ×2。
const BOAR_SEED: i64 = 58;
/// 山贼喽啰（青云山道）：种子 95 → 铜钱 14、木料 ×2。
const BANDIT_SEED: i64 = 95;

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
    if !main_script.is_file() {
        return Err(format!("missing {}", main_script.display()).into());
    }
    let save_path = fresh_save_path(&project, "jhyt_m28_loop.sav");
    let project_arg = project.clone();

    // ---------------- ① 掉落表结构与校验 ----------------
    banner("M28-01① 掉落表：敌人/物品交叉引用闭合，结构与区间可校验（含负例）");
    let mut rt = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;
    assert_num(&mut rt, "require('data.drops').version", 1.0)?;
    assert_str(
        &mut rt,
        "(function() local list = require('data.drops').list(); \
           for i = 2, #list do \
             if tostring(list[i - 1].enemy) >= tostring(list[i].enemy) then return 'unsorted' end \
           end; return #list >= 5 and 'ok' or ('n=' .. #list) end)()",
        "ok",
    )?;
    // 校验器已登记，且整份数据通过严格校验。
    assert_str(
        &mut rt,
        "(function() for _, e in ipairs(require('validate_data').registered()) do \
           if e.path == 'data.drops' then return 'ok' end end return 'missing' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local report = require('validate_data').check({ strict = true }); \
           return report == '' and 'ok' or report end)()",
        "ok",
    )?;
    // 负例①：未知敌人
    assert_str(
        &mut rt,
        "(function() local d = require('data.drops'); local saved = d.drops[1].enemy; \
           d.drops[1].enemy = 'enemy_not_here'; local ok, issues = d.validate_all(); \
           d.drops[1].enemy = saved; \
           for _, i in ipairs(issues or {}) do \
             if i:find('unknown enemy', 1, true) then return 'ok' end end; \
           return 'missing:not_reported' end)()",
        "ok",
    )?;
    // 负例②：未知物品
    assert_str(
        &mut rt,
        "(function() local d = require('data.drops'); local saved = d.drops[1].items[1].id; \
           d.drops[1].items[1].id = 'unobtainium'; local ok, issues = d.validate_all(); \
           d.drops[1].items[1].id = saved; \
           for _, i in ipairs(issues or {}) do \
             if i:find('unknown item', 1, true) then return 'ok' end end; \
           return 'missing:not_reported' end)()",
        "ok",
    )?;
    // 负例③：区间倒置
    assert_str(
        &mut rt,
        "(function() local d = require('data.drops'); local saved = d.drops[1].money; \
           d.drops[1].money = { min = 5, max = 1 }; local ok, issues = d.validate_all(); \
           d.drops[1].money = saved; \
           for _, i in ipairs(issues or {}) do \
             if i:find('> max', 1, true) then return 'ok' end end; \
           return 'missing:not_reported' end)()",
        "ok",
    )?;
    // 负例④：概率越界
    assert_str(
        &mut rt,
        "(function() local d = require('data.drops'); local drop = d.drops[1].items[1]; \
           local saved = drop.chance; drop.chance = 1.5; local ok, issues = d.validate_all(); \
           drop.chance = saved; \
           for _, i in ipairs(issues or {}) do \
             if i:find('chance', 1, true) then return 'ok' end end; \
           return 'missing:not_reported' end)()",
        "ok",
    )?;
    // 负例⑤：多写的键（写了但不生效是最坏的情况）
    assert_str(
        &mut rt,
        "(function() local d = require('data.drops'); local saved = d.drops[1].money; \
           d.drops[1].money = { min = 0, max = 3, bonus = 99 }; local ok, issues = d.validate_all(); \
           d.drops[1].money = saved; \
           for _, i in ipairs(issues or {}) do \
             if i:find('unknown key', 1, true) then return 'ok' end end; \
           return 'missing:not_reported' end)()",
        "ok",
    )?;
    // 负例⑥：一条"什么都不产出"的记录
    assert_str(
        &mut rt,
        "(function() local d = require('data.drops'); local saved = d.drops[1].money; \
           d.drops[1].money = { min = 0, max = 0 }; \
           local before = #d.drops[1].items; local keep = d.drops[1].items; d.drops[1].items = {}; \
           local ok, issues = d.validate_all(); \
           d.drops[1].money = saved; d.drops[1].items = keep; \
           local hit = false; \
           for _, i in ipairs(issues or {}) do \
             if i:find('produces nothing', 1, true) then hit = true end end; \
           return (hit and before == 1) and 'ok' or 'missing:not_reported' end)()",
        "ok",
    )?;
    // 训练木人没有掉落记录（练功靶不该产出资源）；掉落物都是真实物品。
    assert_str(
        &mut rt,
        "require('data.drops').get('enemy_dummy') == nil and 'ok' or 'has_dummy_drop'",
        "ok",
    )?;
    for item in ["wolf_pelt", "boar_meat", "ape_core"] {
        assert_str(
            &mut rt,
            &format!("require('data.items').{item} ~= nil and 'ok' or 'missing'"),
            "ok",
        )?;
    }
    // 猎获订单已登记，且目标就是掉落物。
    assert_str(
        &mut rt,
        "(function() local p = require('world.quest').progress('order_hunt'); \
           if type(p) ~= 'table' then return 'no_quest' end; \
           local items = {}; \
           for _, o in ipairs(p) do items[#items + 1] = tostring(o.id) .. ':' .. tostring(o.target) end; \
           return table.concat(items, ',') end)()",
        "bring_pelt:1,bring_meat:2",
    )?;

    // ---------------- ①b 设计案 §6.2 与实现逐格一致 ----------------
    banner("M28-01①b 设计案 §6.2「MVP 落地口径」表 == data/enemies.lua + data/drops.lua");
    let doc_rows = check_design_enemy_table(&mut rt, &project)?;
    println!("  §6.2 表逐格比对通过：{doc_rows} 个敌人");

    // ---------------- ② 真实击杀 → 掉落入待入库 ----------------
    banner("M28-01② 击杀灰狼（真实 J 键）→ 狼皮进待入库袋，铜钱当场入账");
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    goto_map(&mut rt, "combat_arena", ARENA_POS)?;
    assert_scene_is(&mut rt, "combat_arena")?;
    assert_num(&mut rt, "require('systems.loot').pending_count()", 0.0)?;
    lua(
        &mut rt,
        &format!("require('systems.loot').set_seed({WOLF_SEED})"),
    )?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    kill_enemy(&mut rt, "enemy_wolf", 40)?;
    // 掉落表：灰狼 money 0..3（种子 1 → 2）+ 狼皮 ×1。
    assert_num(&mut rt, "require('core.state').money", money_before + 2.0)?;
    assert_num(&mut rt, "require('systems.loot').pending('wolf_pelt')", 1.0)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "loot_wolf_pelt",
    )?;
    // HUD：掉落必须给出中文（前缀 loot_ 按物品名解析）。
    assert_str(
        &mut rt,
        "require('ui.hud').feedback('loot_wolf_pelt')",
        "战利品：狼皮",
    )?;
    // "未入库"是字面意思：此刻背包里还没有这张狼皮。
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'wolf_pelt')",
        0.0,
    )?;
    assert_str(
        &mut rt,
        "require('systems.loot').pending_text()",
        "wolf_pelt x1",
    )?;
    // 战斗日志里能看到 kill 事件（掉落接在击杀结算之后）。
    assert_str(
        &mut rt,
        "(function() for _, e in ipairs(require('systems.combat').log()) do \
           if e.event == 'kill' and e.target == '灰狼' then return 'ok' end end return 'none' end)()",
        "ok",
    )?;
    // 训练木人没有掉落记录 → 打它不产生任何掉落（负例的真实路径版本）。
    let pending_before = query_num(&mut rt, "require('systems.loot').pending_count()")?;
    let money_before_dummy = query_num(&mut rt, "require('core.state').money")?;
    kill_enemy(&mut rt, "enemy_dummy", 40)?;
    assert_num(
        &mut rt,
        "require('systems.loot').pending_count()",
        pending_before,
    )?;
    assert_num(&mut rt, "require('core.state').money", money_before_dummy)?;

    // ---------------- ③ 跨天入库 ----------------
    banner("M28-01③ 睡觉跨天 → 待入库袋搬进背包（未入库只到跨天为止）");
    goto_map(&mut rt, "manor", MANOR_POS)?;
    assert_scene_is(&mut rt, "manor")?;
    lua(&mut rt, &format!("clock.set(1, {NOON})"))?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "clock.now().day", 2.0)?;
    assert_num(&mut rt, "require('systems.loot').pending_count()", 0.0)?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'wolf_pelt')",
        1.0,
    )?;

    // ---------------- ④ 山道打猎：多笔掉落用于验证"损失一半" ----------------
    banner("M28-01④ 青云山道：野猪（兽肉 ×2）与山贼（木料 ×2）各一笔掉落");
    goto_map(&mut rt, "mountain_path", MOUNTAIN_POS)?;
    assert_scene_is(&mut rt, "mountain_path")?;
    lua(&mut rt, &format!("clock.set(2, {TEN_AM})"))?;
    lua(
        &mut rt,
        &format!("require('systems.loot').set_seed({BOAR_SEED})"),
    )?;
    kill_enemy(&mut rt, "enemy_wild_boar", 40)?;
    assert_num(&mut rt, "require('systems.loot').pending('boar_meat')", 2.0)?;
    lua(
        &mut rt,
        &format!("require('systems.loot').set_seed({BANDIT_SEED})"),
    )?;
    kill_enemy(&mut rt, "enemy_bandit", 40)?;
    assert_num(&mut rt, "require('systems.loot').pending('timber')", 2.0)?;
    let pending_money = query_num(&mut rt, "require('core.state').money")?;
    // 打猎收入 = 野猪 6 + 山贼 14（种子固定）。
    println!("山道打猎后铜钱 = {pending_money}");
    assert_num(&mut rt, "require('systems.loot').pending_count()", 4.0)?;
    // 接取猎获订单（真对话）：交付前先接单，战败后仍要能交付（任务进度不属于惩罚范围）。
    goto_map(&mut rt, "village", VILLAGE_POS)?;
    assert_scene_is(&mut rt, "village")?;
    lua(&mut rt, &format!("clock.set(2, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_lin_xiaoman")?;
    choose_option(&mut rt, "村里还缺什么野味？")?;
    choose_option(&mut rt, "这单我接了。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_hunt')",
        "in_progress",
    )?;
    lua(&mut rt, "require('world.dialogue').choose()")?;

    // ---------------- ⑤ 真实被咬死 ----------------
    banner("M28-01⑤ 战败：气血压到 3 点后让玄冥会徒打死（真实敌人攻击路径）");
    goto_map(&mut rt, "mountain_path", MOUNTAIN_POS)?;
    assert_scene_is(&mut rt, "mountain_path")?;
    lua(&mut rt, &format!("clock.set(2, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_num(&mut rt, "require('systems.combat').stats().deaths", 0.0)?;
    let money_at_death = query_num(&mut rt, "require('core.state').money")?;
    let pending_at_death = query_num(&mut rt, "require('systems.loot').pending_count()")?;
    if pending_at_death < 2.0 {
        return Err(format!("待入库掉落太少，无法验证减半：{pending_at_death}").into());
    }
    // 先站到玄冥会徒身边再压血：避免"压到 3 点后一路被别的敌人打死"的不确定性。
    stand_next_to(&mut rt, "enemy_xuanming")?;
    lua(&mut rt, "require('systems.combat').set_player_hp(3)")?;
    let dead = wait_for_death(&mut rt, 900)?;
    if !dead {
        return Err("900 帧内没有被敌人打死（战败路径未触发）".into());
    }
    assert_str(
        &mut rt,
        "require('systems.combat').is_player_dead()",
        "true",
    )?;
    assert_num(&mut rt, "require('systems.combat').stats().deaths", 1.0)?;
    // 死亡预算是只读的：此刻还没结算，袋里还是原样。
    assert_num(
        &mut rt,
        "require('systems.death').penalty().money",
        (money_at_death * 0.05).floor(),
    )?;

    // ---------------- ⑥ R 键战败重开：惩罚 + 回庄 + 状态回满 ----------------
    banner("M28-01⑥ R 键：损失待入库 50% + 铜钱 5% → 回山庄出生点 → 状态回满");
    let spawn = query_str(
        &mut rt,
        "(function() local d = require('data.maps').get('manor'); \
           local wx, wy = require('data.maps').to_world({ to_cell = d.spawn }); \
           return tostring(math.floor(wx + 0.5)) .. ',' .. tostring(math.floor(wy + 0.5)) end)()",
    )?;
    press(
        &mut rt,
        KeyCode::from_char('r').expect("QWERTY 字符必须有对应物理键"),
    )?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "manor")?;
    assert_map_current(&mut rt, "manor")?;
    assert_str(
        &mut rt,
        "require('systems.combat').is_player_dead()",
        "false",
    )?;
    // 落点 = 山庄出生点（回庄，而不是"原地复活"）。
    assert_str(
        &mut rt,
        "(function() local pos = require('player.player').position(); \
           return tostring(math.floor(pos.x + 0.5)) .. ',' .. tostring(math.floor(pos.y + 0.5)) end)()",
        &spawn,
    )?;
    // 状态回满（docs/33 §6.3）。
    assert_num(&mut rt, "require('systems.combat').player_hp()", 100.0)?;
    assert_num(&mut rt, "require('core.state').energy", 100.0)?;
    assert_num(&mut rt, "require('core.state').inner_power", 100.0)?;
    // 惩罚①：待入库掉落减半（向下取整）。
    assert_num(&mut rt, "require('systems.loot').pending('boar_meat')", 1.0)?;
    assert_num(&mut rt, "require('systems.loot').pending('timber')", 1.0)?;
    assert_num(&mut rt, "require('systems.loot').pending_count()", 2.0)?;
    // 惩罚②：铜钱损失 floor(5%)。
    let money_after_death = query_num(&mut rt, "require('core.state').money")?;
    assert_num(
        &mut rt,
        "require('core.state').money",
        money_at_death - (money_at_death * 0.05).floor(),
    )?;
    println!("战败损失：铜钱 {money_at_death} → {money_after_death}，待入库 4 → 2");
    // 已入库物品与任务进度不在惩罚范围（§6.3：保留已入库物品与任务进度）。
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'wolf_pelt')",
        1.0,
    )?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_hunt')",
        "in_progress",
    )?;
    // HUD 反馈：战败损失明细（前缀 death_penalty_ 原样拼接）。
    let penalty_text = query_str(&mut rt, "require('systems.death').last_penalty().text")?;
    if penalty_text.trim().is_empty() {
        return Err("战败明细为空".into());
    }
    assert_str(
        &mut rt,
        "(function() local a = require('core.state').last_action; \
           return a:sub(1, 14) == 'death_penalty_' and 'ok' or a end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "require('ui.hud').feedback(require('core.state').last_action):find('战败回庄', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;
    // 战斗日志：death + respawn 事件都在（死亡不是静默传送）。
    assert_str(
        &mut rt,
        "(function() local d, r = false, false; \
           for _, e in ipairs(require('systems.combat').log()) do \
             if e.event == 'death' then d = true end; \
             if e.event == 'respawn' then r = true end \
           end; return (d and r) and 'ok' or 'missing' end)()",
        "ok",
    )?;
    // 敌人随复活重置（否则玩家回庄后山道会一直是空的）。
    assert_str(
        &mut rt,
        "require('systems.enemy').query('enemy_xuanming').alive and 'ok' or 'dead'",
        "ok",
    )?;

    // ---------------- ⑦ 战败后仍能打猎 → 入库 → 交付订单 ----------------
    banner("M28-01⑦ 战败后重打野猪 → 跨天入库 → 向林小满交付猎获订单");
    goto_map(&mut rt, "mountain_path", MOUNTAIN_POS)?;
    assert_scene_is(&mut rt, "mountain_path")?;
    lua(&mut rt, &format!("clock.set(2, {TEN_AM})"))?;
    lua(
        &mut rt,
        &format!("require('systems.loot').set_seed({BOAR_SEED})"),
    )?;
    kill_enemy(&mut rt, "enemy_wild_boar", 40)?;
    assert_num(&mut rt, "require('systems.loot').pending('boar_meat')", 3.0)?;
    // 跨天入库（真实 T 键）。
    goto_map(&mut rt, "manor", MANOR_POS)?;
    lua(&mut rt, &format!("clock.set(2, {NOON})"))?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "clock.now().day", 3.0)?;
    assert_num(&mut rt, "require('systems.loot').pending_count()", 0.0)?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'boar_meat')",
        3.0,
    )?;
    // 交付订单：狼皮 ×1 + 兽肉 ×2（目标物品被消耗）。
    goto_map(&mut rt, "village", VILLAGE_POS)?;
    assert_scene_is(&mut rt, "village")?;
    lua(&mut rt, &format!("clock.set(3, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_hunt')",
        "deliverable",
    )?;
    let money_before_delivery = query_num(&mut rt, "require('core.state').money")?;
    talk_to(&mut rt, "npc_lin_xiaoman")?;
    choose_option(&mut rt, "狼皮一张、兽肉两份，都在这里。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_hunt')",
        "completed",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('order_hunt_done')",
        "true",
    )?;
    assert_num(
        &mut rt,
        "require('core.state').money",
        money_before_delivery + 90.0,
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'wolf_pelt')",
        0.0,
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'boar_meat')",
        1.0,
    )?;
    assert_num(
        &mut rt,
        "require('player.relationship').points('npc_lin_xiaoman')",
        6.0,
    )?;

    // ---------------- ⑧ 存档往返：待入库袋与随机序列一起回来 ----------------
    banner("M28-01⑧ 存档往返：待入库袋、铜钱、随机序列与已结订单全部还原");
    goto_map(&mut rt, "combat_arena", ARENA_POS)?;
    assert_scene_is(&mut rt, "combat_arena")?;
    lua(
        &mut rt,
        &format!("require('systems.loot').set_seed({WOLF_SEED})"),
    )?;
    // 灰狼在复活时已重置 → 可以再打一次（掉落序列用固定种子，结果确定）。
    kill_enemy(&mut rt, "enemy_wolf", 40)?;
    assert_num(&mut rt, "require('systems.loot').pending('wolf_pelt')", 1.0)?;
    let seed_before = query_num(&mut rt, "require('systems.loot').seed()")?;
    let money_before_save = query_num(&mut rt, "require('core.state').money")?;
    lua(&mut rt, "require('core.save_manager').save()")?;
    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_map_current(&mut second, "combat_arena")?;
    assert_num(
        &mut second,
        "require('systems.loot').pending('wolf_pelt')",
        1.0,
    )?;
    assert_num(&mut second, "require('systems.loot').seed()", seed_before)?;
    assert_num(
        &mut second,
        "require('core.state').money",
        money_before_save,
    )?;
    assert_str(
        &mut second,
        "require('world.quest').state('order_hunt')",
        "completed",
    )?;
    assert_num(
        &mut second,
        "inv.count(require('core.state').inventory, 'wolf_pelt')",
        0.0,
    )?;

    // ---------------- ⑨ 新档隔离 ----------------
    banner("M28-01⑨ 新档：待入库袋、战败明细与掉落序列全部复位");
    lua(&mut second, "assert(jianghu_return_to_title())")?;
    second.process_scene_transition()?;
    click_button(&mut second, "BtnStart")?;
    assert_scene_is(&mut second, "manor")?;
    assert_num(&mut second, "require('systems.loot').pending_count()", 0.0)?;
    assert_num(&mut second, "require('core.state').money", 120.0)?;
    assert_num(
        &mut second,
        "inv.count(require('core.state').inventory, 'wolf_pelt')",
        0.0,
    )?;
    assert_str(
        &mut second,
        "require('systems.death').last_penalty() == nil and 'ok' or 'stale'",
        "ok",
    )?;
    assert_num(&mut second, "require('systems.loot').seed()", 20260914.0)?;

    println!(
        "jhyt_m28_loop_ok project={} drops={} design_rows={} pending_after_death=2 order_hunt=completed \
         money={} deaths={}",
        project.display(),
        query_num(
            &mut second,
            "(function() local n = 0; \
           for _ in ipairs(require('data.drops').list()) do n = n + 1 end; return n end)()"
        )? as i64,
        doc_rows,
        query_num(&mut second, "require('core.state').money")? as i64,
        query_num(&mut second, "require('systems.combat').stats().deaths")? as i64,
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- 设计案比对 ----------

/// 逐格比对 `docs/33` §6.2 的「MVP 落地口径」表与 `data/enemies.lua` + `data/drops.lua`。
///
/// 为什么要把文档也纳入验收：这张表此前写的是 Alpha 目标数值与"草药/木材"这类**注册表里不存在**
/// 的掉落物（grep 一遍就能发现文档与实现各说各话）。现在口径只在文档里写一次，改数据不改表
/// 会直接失败——文档漂移不再是"以后再说"的事。
fn check_design_enemy_table(
    runtime: &mut LuaRuntime,
    project: &Path,
) -> Result<usize, Box<dyn std::error::Error>> {
    let doc_path = project.join("docs/33-江湖有田-数值设计案.md");
    let doc = fs::read_to_string(&doc_path)
        .map_err(|e| format!("读取 {} 失败：{e}", doc_path.display()))?;
    let rows = markdown_table(&doc, "敌人id")?;
    let enemy_total = query_num(runtime, "#(require('data.enemies').list())")? as usize;
    if rows.len() != enemy_total {
        return Err(format!(
            "§6.2 MVP 表有 {} 行，data/enemies.lua 有 {enemy_total} 个敌人——新增敌人必须同步表格",
            rows.len()
        )
        .into());
    }
    let mut checked = 0;
    for row in &rows {
        if row.len() < 6 {
            return Err(format!("§6.2 行字段不足（{row:?}）").into());
        }
        let id = row[0].clone();
        let name = row[1].clone();
        let hp = number_from_cell(&row[2])?;
        let damage = number_from_cell(&row[3])?;
        let cooldown = number_from_cell(&row[4])?;
        assert_str(
            runtime,
            &format!("require('data.enemies').get('{id}').name"),
            &name,
        )?;
        for (field, want) in [
            ("hp", hp),
            ("damage", damage),
            ("attack_cooldown", cooldown),
        ] {
            assert_num(
                runtime,
                &format!("require('data.enemies').get('{id}').{field}"),
                want,
            )?;
        }
        let doc_drops = canonical_drops_cell(&row[5])?;
        let data_drops = query_str(
            runtime,
            &format!(
                "(function() local d = require('data.drops').get('{id}'); \
                   if not d then return 'none' end; \
                   local parts = {{}}; \
                   if d.money then \
                     parts[#parts + 1] = 'money=' .. string.format('%d-%d', d.money.min, d.money.max) \
                   end; \
                   for _, it in ipairs(d.items or {{}}) do \
                     parts[#parts + 1] = it.id .. '=' .. string.format('%d-%d', it.min, it.max) \
                   end; \
                   return table.concat(parts, ';') end)()"
            ),
        )?;
        if doc_drops != data_drops {
            return Err(format!(
                "§6.2「{name}」掉落口径不一致：设计案 = {doc_drops:?}，data/drops.lua = {data_drops:?}"
            )
            .into());
        }
        checked += 1;
    }
    if checked < 5 {
        return Err(format!("§6.2 MVP 表只比对了 {checked} 行，表格被删改？").into());
    }
    Ok(checked)
}

/// 把表格里的掉落单元格规范成 `money=8-15;timber=0-2`（无掉落为 `none`），便于与数据比对。
fn canonical_drops_cell(cell: &str) -> Result<String, Box<dyn std::error::Error>> {
    let text = cell.trim();
    if text.is_empty() || text == "无" {
        return Ok("none".to_owned());
    }
    let mut parts = Vec::new();
    for raw in text.split('、') {
        let entry = raw.trim();
        if let Some(range) = entry.strip_prefix("铜钱") {
            let (min, max) = parse_range(range.trim())?;
            parts.push(format!("money={min}-{max}"));
            continue;
        }
        let (id, range) = entry
            .split_once('×')
            .ok_or_else(|| format!("§6.2 掉落项无法解析：{entry:?}"))?;
        let (min, max) = parse_range(range.trim())?;
        parts.push(format!("{}={min}-{max}", id.trim()));
    }
    Ok(parts.join(";"))
}

/// 解析 `8–15`（也接受 `8-15` 与单值 `1` = `1–1`）。
fn parse_range(text: &str) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let normalized = text.replace(['–', '—'], "-");
    match normalized.split_once('-') {
        Some((min, max)) => Ok((min.trim().parse()?, max.trim().parse()?)),
        None => {
            let value: i64 = normalized.trim().parse()?;
            Ok((value, value))
        }
    }
}

/// 从设计案 markdown 里取出以 `first_cell` 为表头的表格行（去掉表头与分隔行）。
fn markdown_table(
    markdown: &str,
    first_cell: &str,
) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>> {
    let mut rows = Vec::new();
    let mut inside = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            if inside {
                break;
            }
            continue;
        }
        let cells: Vec<String> = trimmed
            .trim_matches('|')
            .split('|')
            .map(|cell| cell.trim().to_owned())
            .collect();
        if !inside {
            if cells.first().map(String::as_str) == Some(first_cell) {
                inside = true;
            }
            continue;
        }
        if cells
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':'))
        {
            continue;
        }
        rows.push(cells);
    }
    if rows.is_empty() {
        return Err(format!("设计案里找不到以「{first_cell}」为表头的表格").into());
    }
    Ok(rows)
}

fn number_from_cell(cell: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let digits: String = cell
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    Ok(digits.parse()?)
}

// ---------- 战斗 helpers ----------

/// 站到敌人相邻格并朝它（真实移动不可控，用 teleport 复现"走到它面前"）。
fn stand_next_to(
    runtime: &mut LuaRuntime,
    enemy_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "(function() local q = require('systems.enemy').query('{enemy_id}'); \
               if not q or not q.live then return 'no_enemy' end; \
               local cx, cy = q.cell_x, q.cell_y; \
               require('player.player').teleport((cx + 1.5) * 32, (cy + 0.5) * 32); \
               require('core.state').facing = 'left'; \
               return 'ok' end)()"
        ),
    )?;
    if got != "ok" {
        return Err(format!("{enemy_id} not reachable: {got}").into());
    }
    frames(runtime, 2)?;
    Ok(())
}

/// 用真实 J 键把敌人打死（走到它旁边 → 普攻，直到 `query().alive == false`）。
fn kill_enemy(
    runtime: &mut LuaRuntime,
    enemy_id: &str,
    tries: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..tries {
        let alive = query_str(
            runtime,
            &format!("require('systems.enemy').query('{enemy_id}').alive and 'yes' or 'no'"),
        )?;
        if alive == "no" {
            return Ok(());
        }
        stand_next_to(runtime, enemy_id)?;
        press(
            runtime,
            KeyCode::from_char('j').expect("QWERTY 字符必须有对应物理键"),
        )?;
        frames(runtime, 30)?;
    }
    Err(format!("{tries} 次普攻没能击败 {enemy_id}（或目标不可达）").into())
}

/// 等玩家被敌人打死（真实受击路径）；返回是否在 `max_frames` 内死亡。
fn wait_for_death(
    runtime: &mut LuaRuntime,
    max_frames: usize,
) -> Result<bool, Box<dyn std::error::Error>> {
    runtime.set_input(InputSnapshot::default());
    for _ in 0..max_frames {
        if query_str(
            runtime,
            "require('systems.combat').is_player_dead() and 'y' or 'n'",
        )? == "y"
        {
            return Ok(true);
        }
        runtime.call_update()?;
    }
    Ok(query_str(
        runtime,
        "require('systems.combat').is_player_dead() and 'y' or 'n'",
    )? == "y")
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
