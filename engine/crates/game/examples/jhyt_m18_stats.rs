//! Headless acceptance drive for M18-04 《江湖有田》装备属性与关系声望模型.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M18-04 acceptance paths:
//!
//! - 纯数据注册：关系/礼物/档位/阵营全部来自 `data/relationships.lua`，
//!   加载期校验通过（main.validate_content 聚合，错误可定位）。
//! - 三层属性：base（attack=2）+ equipment（练习剑 8）+ buff（行气散 3）
//!   互不重复叠加，query = 2 → 10 → 13；同 buff 重复添加幂等（仍 13）。
//! - 换装变化：`skills.player_damage('strike')` 随装备/Buff 可验证变化
//!   （13 → 10 → 2 → 10），M18-01 数值锚点不破坏。
//! - 礼物：`relationship.gift` 计分/累积/拒绝路径（not_giftable/not_enough/
//!   unknown_npc）；`gift_facing` 在无 NPC 的演武场返回 `no_npc_near`（G 键同路）。
//! - 对话/任务修改关系与声望：对话 relationship effect（+5）、任务
//!   relationship/reputation 奖励（+10/+5）到账且来源入流水；重复交付幂等。
//! - reward_once 幂等：同 key 第二次 `already_rewarded`，fn 只执行一次。
//! - 边界值：+999/-999 clamp 到 0..100 与阵营 [min,max]；未知 npc/faction 拒绝。
//! - Buff 过期：`stats.on_day_start()` 一次 days 2→1 仍在，第二次移除。
//! - 存档往返：capture 后改动 → restore 恢复一致；旧档缺 stats/relationship
//!   provider 数据不报错；restore({}) 得到默认值。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m18_stats [-- <game_jhyt-root>]

use engine::core::{
    input::{InputKey, InputSnapshot},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 演武场出生格 (9,5) 的世界坐标：演武场无 NPC 实体，gift_facing 必然 no_npc_near。
const ARENA_SPAWN: (f32, f32) = (304.0, 176.0);
/// 10:00：沈青萝在云泽村医馆（复用 M17-03 的对话驱动时刻）。
const TEN_AM: i64 = 600;

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
    let save_path = fresh_save_path(&project, "jhyt_m18_stats.sav");
    let project_arg = project.clone();

    banner("M18-04a 纯数据注册 + 加载期校验：关系/礼物/档位/阵营来自 data/relationships");
    let mut runtime = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut runtime, &main_script)?;
    // 直接从标题进云泽村引导新档（entry=nil 走 new_game 分支，与 M17「继续到
    // 村庄」的 boot 路径同款）。不点 BtnStart（boot 在山庄会 farm.init，
    // 之后跨图回村时 farm.restore 的 redraw_all 会撞上已销毁的 FarmGrid 旧 id）。
    lua(&mut runtime, "scene.load('village')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "village")?;
    assert_mode(&mut runtime, "Playing")?;
    // boot_game → validate_content 已聚合 data.relationships.validate_all（失败会 error）。
    assert_validate(&mut runtime, "data.relationships")?;
    assert_num(
        &mut runtime,
        "#(require('data.relationships').npc_ids())",
        7.0,
    )?;
    assert_num(
        &mut runtime,
        "require('data.relationships').gift_points('npc_shen_qingluo','lingzhi')",
        8.0,
    )?;
    assert_num(
        &mut runtime,
        "require('data.relationships').gift_points('npc_gu_changfeng','heal_pill')",
        8.0,
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('data.relationships').gift_points('npc_shen_qingluo','seed_rice'))",
        "nil",
    )?;
    for (points, key) in [
        (0.0, "stranger"),
        (19.0, "stranger"),
        (20.0, "acquainted"),
        (45.0, "friendly"),
        (70.0, "close"),
        (90.0, "devoted"),
        (100.0, "devoted"),
    ] {
        assert_str(
            &mut runtime,
            &format!("require('data.relationships').tier_for({points}).key"),
            key,
        )?;
    }
    assert_str(
        &mut runtime,
        "require('data.relationships').faction('village').name",
        "云泽村",
    )?;
    assert_num(
        &mut runtime,
        "require('data.relationships').faction('village').min",
        -100.0,
    )?;
    assert_num(
        &mut runtime,
        "require('data.relationships').faction('village').max",
        100.0,
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('data.relationships').faction('nope'))",
        "nil",
    )?;

    banner("M18-04b 三层属性：base(2) + 装备(8) + Buff(3) 互不重复叠加，同 Buff 幂等");
    assert_num(
        &mut runtime,
        "require('player.stats').base('player').attack",
        2.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').base('player').defense",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').base('npc_unknown').attack",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','attack')",
        2.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','defense')",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','speed')",
        0.0,
    )?;
    lua(&mut runtime, "inv.add('player', 'training_sword', 1)")?;
    lua(
        &mut runtime,
        "inv.equip('player', 'weapon', 'training_sword')",
    )?;
    // 装备层：练习剑无 stats 字段，damage=8 按旧字段兼容视为 stats.attack。
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','attack')",
        10.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').equipment_bonus('player').attack",
        8.0,
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('player.stats').add_buff('player', { stat='attack', amount=3, days=2 }))",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','attack')",
        13.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').buff_bonus('player','attack')",
        3.0,
    )?;
    // 同 stat+amount 的 Buff 不叠加重复条目（幂等），query 仍 13。
    assert_str(
        &mut runtime,
        "tostring(require('player.stats').add_buff('player', { stat='attack', amount=3, days=2 }))",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','attack')",
        13.0,
    )?;
    assert_num(
        &mut runtime,
        "#(require('player.stats').list('player'))",
        1.0,
    )?;
    // 非法参数拒绝：bad_stat / bad_amount / bad_days，且不产生条目。
    assert_str(
        &mut runtime,
        "select(2, require('player.stats').add_buff('player', { stat='speed', amount=1, days=1 }))",
        "bad_stat",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('player.stats').add_buff('player', { stat='attack', amount='x', days=1 }))",
        "bad_amount",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('player.stats').add_buff('player', { stat='attack', amount=1, days=0 }))",
        "bad_days",
    )?;
    assert_num(
        &mut runtime,
        "#(require('player.stats').list('player'))",
        1.0,
    )?;

    banner("M18-04c 换装变化：player_damage 随装备/Buff 可验证变化（13→10→2→10），M18-01 锚点不变");
    assert_num(
        &mut runtime,
        "require('player.skills').player_damage('strike')",
        13.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.skills').weapon_bonus()",
        11.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.skills').weapon_damage()",
        8.0,
    )?;
    // Buff 过期铺垫：on_day_start 一次 days 2→1 仍在，第二次移除。
    assert_num(&mut runtime, "require('player.stats').on_day_start()", 0.0)?;
    assert_num(
        &mut runtime,
        "#(require('player.stats').list('player'))",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').list('player')[1].days",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.skills').player_damage('strike')",
        13.0,
    )?;
    assert_num(&mut runtime, "require('player.stats').on_day_start()", 1.0)?;
    assert_num(
        &mut runtime,
        "#(require('player.stats').list('player'))",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.skills').player_damage('strike')",
        10.0,
    )?;
    // 卸装（inv.equip 空串清槽）：伤害回到徒手 2；重装回 10（M18-01 数值一致）。
    lua(&mut runtime, "inv.equip('player', 'weapon', '')")?;
    assert_num(
        &mut runtime,
        "require('player.skills').player_damage('strike')",
        2.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','attack')",
        2.0,
    )?;
    lua(
        &mut runtime,
        "inv.equip('player', 'weapon', 'training_sword')",
    )?;
    assert_num(
        &mut runtime,
        "require('player.skills').player_damage('strike')",
        10.0,
    )?;

    banner(
        "M18-04d 礼物：gift 计分/累积 + not_giftable/not_enough/unknown_npc + gift_facing 无 NPC",
    );
    lua(&mut runtime, "inv.add('player', 'lingzhi', 2)")?;
    assert_str(
        &mut runtime,
        "(function() local ok, after = require('player.relationship').gift('npc_shen_qingluo','lingzhi'); \
         return tostring(ok) .. '|' .. tostring(after) end)()",
        "true|8",
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').points('npc_shen_qingluo')",
        8.0,
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, after = require('player.relationship').gift('npc_shen_qingluo','lingzhi'); \
         return tostring(ok) .. '|' .. tostring(after) end)()",
        "true|16",
    )?;
    assert_num(&mut runtime, "inv.count('player','lingzhi')", 0.0)?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.relationship').gift('npc_shen_qingluo','seed_rice'); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "false|not_giftable",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.relationship').gift('npc_zhao_tiesuanpan','lingzhi'); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "false|not_giftable",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.relationship').gift('npc_shen_qingluo','heal_pill'); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "false|not_enough",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.relationship').gift('npc_ghost','lingzhi'); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "false|unknown_npc",
    )?;
    // 演武场没有 NPC 实体：gift_facing 必然 no_npc_near（G 键输入路径同步验证）。
    lua(&mut runtime, "scene.load('combat_arena')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "combat_arena")?;
    teleport(&mut runtime, ARENA_SPAWN)?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.relationship').gift_facing('right'); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "false|no_npc_near",
    )?;
    press(&mut runtime, InputKey::from_char('g'))?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "no_npc_near",
    )?;

    banner("M18-04e 对话副作用：沈青萝接取选项 relationship=+5，来源入流水");
    lua(&mut runtime, "scene.load('village')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "village")?;
    lua(&mut runtime, "return require('world.weather').set('sunny')")?;
    lua(&mut runtime, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut runtime, 2)?;
    assert_num(
        &mut runtime,
        "require('player.relationship').points('npc_shen_qingluo')",
        16.0,
    )?;
    talk(&mut runtime, "npc_shen_qingluo")?;
    assert_mode(&mut runtime, "Dialogue")?;
    // 选项数量随 M19-01 追加的订单/主线入口变化；这里断言「接取」选项排第一且可见。
    assert_str(
        &mut runtime,
        "(function() for _, t in ipairs(require('world.dialogue').info().options) do \
           if t == '愿为医馆采三株灵芝。' then return 'visible' end end return 'hidden' end)()",
        "visible",
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().options[1]",
        "愿为医馆采三株灵芝。",
    )?;
    press(&mut runtime, InputKey::from_char('1'))?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_herb')",
        "in_progress",
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').points('npc_shen_qingluo')",
        21.0,
    )?;
    assert_relationship_event(
        &mut runtime,
        "npc_shen_qingluo",
        5.0,
        "dialogue:shen_qingluo_herb",
    )?;
    press(&mut runtime, InputKey::from_char('1'))?;
    assert_str(&mut runtime, "require('world.dialogue').is_open()", "false")?;
    assert_mode(&mut runtime, "Playing")?;

    banner("M18-04f 任务奖励：relationship/reputation 到账 + 重复交付幂等 + gift_facing 无可礼物");
    lua(&mut runtime, "inv.add('player', 'lingzhi', 3)")?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_herb')",
        "deliverable",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('world.quest').deliver('quest_herb'))",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').points('npc_shen_qingluo')",
        31.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').reputation('village')",
        5.0,
    )?;
    assert_num(&mut runtime, "require('core.state').money", 170.0)?;
    assert_num(&mut runtime, "inv.count('player','rice')", 4.0)?;
    // 幂等：重复交付 already_completed，关系/声望/金币不再变化。
    assert_str(
        &mut runtime,
        "select(2, require('world.quest').deliver('quest_herb'))",
        "already_completed",
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').points('npc_shen_qingluo')",
        31.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').reputation('village')",
        5.0,
    )?;
    assert_relationship_event(&mut runtime, "npc_shen_qingluo", 10.0, "quest:quest_herb")?;
    assert_reputation_event(&mut runtime, "village", 5.0, "quest:quest_herb")?;
    // no_gift 分支：清空可礼物品后站在沈青萝同格 → 背包无可送礼物。
    lua(&mut runtime, "inv.remove('player', 'rice', 4)")?;
    lua(
        &mut runtime,
        "require('player.player').teleport((require('world.npc').query('npc_shen_qingluo').cell_x + 0.5) * 32, \
         (require('world.npc').query('npc_shen_qingluo').cell_y + 0.5) * 32)",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.relationship').gift_facing('down'); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "false|no_gift",
    )?;

    banner("M18-04g reward_once：同 key 第二次 already_rewarded，fn 只执行一次");
    assert_str(
        &mut runtime,
        "(function() local calls = 0; \
         local ok1 = require('player.relationship').reward_once('m18_stats_test_key', function() calls = calls + 1 end); \
         local ok2, why = require('player.relationship').reward_once('m18_stats_test_key', function() calls = calls + 1 end); \
         return tostring(ok1) .. '|' .. tostring(ok2) .. '|' .. tostring(why) .. '|' .. tostring(calls) end)()",
        "true|false|already_rewarded|1",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('player.relationship').reward_once(123, function() end))",
        "bad_key",
    )?;

    banner("M18-04h 边界值：+999/-999 clamp（0..100 与阵营 [min,max]），未知对象拒绝");
    assert_str(
        &mut runtime,
        "(function() local ok, after = require('player.relationship').add('npc_han_tieshan', 999); \
         return tostring(ok) .. '|' .. tostring(after) end)()",
        "true|100",
    )?;
    assert_str(
        &mut runtime,
        "require('player.relationship').tier('npc_han_tieshan').key",
        "devoted",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, after = require('player.relationship').add('npc_han_tieshan', -999); \
         return tostring(ok) .. '|' .. tostring(after) end)()",
        "true|0",
    )?;
    assert_str(
        &mut runtime,
        "require('player.relationship').tier('npc_han_tieshan').key",
        "stranger",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, after = require('player.relationship').add_reputation('sect', 999); \
         return tostring(ok) .. '|' .. tostring(after) end)()",
        "true|100",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, after = require('player.relationship').add_reputation('sect', -999); \
         return tostring(ok) .. '|' .. tostring(after) end)()",
        "true|-100",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.relationship').add('npc_ghost', 1); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "false|unknown_npc",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.relationship').add_reputation('nope', 1); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "false|unknown_faction",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('player.relationship').add('npc_shen_qingluo', 'x'); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "false|bad_delta",
    )?;

    banner("M18-04i Buff 过期：on_day_start 一次 days 2→1 仍在，第二次移除");
    assert_str(
        &mut runtime,
        "tostring(require('player.stats').add_buff('player', { stat='defense', amount=2, days=2 }))",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','defense')",
        3.0,
    )?;
    assert_num(&mut runtime, "require('player.stats').on_day_start()", 0.0)?;
    assert_num(
        &mut runtime,
        "#(require('player.stats').list('player'))",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').list('player')[1].days",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','defense')",
        3.0,
    )?;
    assert_num(&mut runtime, "require('player.stats').on_day_start()", 1.0)?;
    assert_num(
        &mut runtime,
        "#(require('player.stats').list('player'))",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','defense')",
        1.0,
    )?;

    banner(
        "M18-04j 存档往返：capture→改动→restore 一致；旧档缺 provider 不报错；restore({}) 得默认值",
    );
    assert_str(
        &mut runtime,
        "tostring(require('player.stats').add_buff('player', { stat='attack', amount=1, days=5 }))",
        "true",
    )?;
    lua(
        &mut runtime,
        "__captured = require('core.save_manager').capture()",
    )?;
    assert_str(
        &mut runtime,
        "tostring(__captured.providers.stats ~= nil and __captured.providers.relationship ~= nil)",
        "true",
    )?;
    // 改动关系/声望/流水/Buff，再 restore 回快照。
    lua(
        &mut runtime,
        "require('player.relationship').add('npc_shen_qingluo', 9)",
    )?;
    lua(
        &mut runtime,
        "require('player.relationship').add_reputation('village', 5)",
    )?;
    lua(
        &mut runtime,
        "require('player.relationship').add('npc_lin_xiaoman', 7)",
    )?;
    lua(
        &mut runtime,
        "require('player.stats').add_buff('player', { stat='defense', amount=1, days=3 })",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('core.save_manager').restore(__captured); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "true|nil",
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').points('npc_shen_qingluo')",
        31.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').reputation('village')",
        5.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').reputation('sect')",
        -100.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').points('npc_lin_xiaoman')",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').points('npc_han_tieshan')",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "#(require('player.stats').list('player'))",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('player.stats').list('player')[1].stat",
        "attack",
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').list('player')[1].amount",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').list('player')[1].days",
        5.0,
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','attack')",
        11.0,
    )?;
    // 幂等奖励 keys 随档恢复：同 key 仍拒绝。
    assert_str(
        &mut runtime,
        "select(2, require('player.relationship').reward_once('m18_stats_test_key', function() end))",
        "already_rewarded",
    )?;
    // 旧档兼容①：剥离 stats/relationship provider 的旧快照整体 restore 不报错，
    // 缺失 provider 被跳过（内存态保留）。scene 摘要必须保留（restore 前置校验）。
    lua(
        &mut runtime,
        "__legacy = { version = __captured.version, format = __captured.format, \
         scene = __captured.scene, providers = {} }; \
         for k, v in pairs(__captured.providers) do \
           if k ~= 'stats' and k ~= 'relationship' then __legacy.providers[k] = v end \
         end",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('core.save_manager').restore(__legacy); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "true|nil",
    )?;
    // 旧档兼容③：version=1 的旧 blob 被 save_manager 明确拒绝（早期版本门）。
    assert_str(
        &mut runtime,
        "select(2, require('core.save_manager').restore({ version = 1, format = 'jhyt.snapshot', \
         providers = {}, scene = { name = scene.reload() } }))",
        "legacy snapshot version 1 is explicitly rejected; start a new game",
    )?;
    let rel_shen = query_num(
        &mut runtime,
        "return require('player.relationship').points('npc_shen_qingluo')",
    )?;
    let rep_village = query_num(
        &mut runtime,
        "return require('player.relationship').reputation('village')",
    )?;
    let rep_sect = query_num(
        &mut runtime,
        "return require('player.relationship').reputation('sect')",
    )?;
    let buff_days = query_num(
        &mut runtime,
        "return require('player.stats').list('player')[1].days",
    )?;
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','attack')",
        11.0,
    )?;
    // 旧档兼容②：对各自 restore 传 {}（或非表）→ 不报错且得到默认值。
    lua(&mut runtime, "require('player.relationship').restore({})")?;
    lua(&mut runtime, "require('player.stats').restore({})")?;
    lua(&mut runtime, "require('player.stats').restore(nil)")?;
    lua(&mut runtime, "require('player.stats').restore('garbage')")?;
    lua(
        &mut runtime,
        "require('player.relationship').restore(false)",
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').points('npc_shen_qingluo')",
        0.0,
    )?;
    assert_str(
        &mut runtime,
        "require('player.relationship').tier('npc_shen_qingluo').key",
        "stranger",
    )?;
    assert_num(
        &mut runtime,
        "require('player.relationship').reputation('village')",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "#(require('player.stats').list('player'))",
        0.0,
    )?;
    // Buff 清空后只剩 base+装备层：query = 2 + 8 = 10（三层模型收口锚点）。
    assert_num(
        &mut runtime,
        "require('player.stats').query('player','attack')",
        10.0,
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('player.relationship').reward_once('m18_stats_test_key', function() end))",
        "true",
    )?;

    println!(
        "jhyt_m18_stats_ok project={} rel_shen={} rep_village={} rep_sect={} buff_days={} legacy_restore_ok=true defaults_ok=true",
        project.display(),
        rel_shen,
        rep_village,
        rep_sect,
        buff_days,
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

/// Runs the project script, then loads the `main` 门面 scene so BtnStart/BtnContinue
/// are live (mirrors jhyt_m18_combat).
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

/// Teleports the player to a world position via `player.teleport`.
fn teleport(runtime: &mut LuaRuntime, pos: (f32, f32)) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "require('player.player').teleport({}, {})",
        pos.0, pos.1
    ))?;
    Ok(())
}

/// 把玩家传送到 NPC 的逻辑格心，再按 E 触发 interaction → 开对话（M17-03 同款）。
fn talk(runtime: &mut LuaRuntime, persist_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "local q = require('world.npc').query('{persist_id}'); \
             if not q or not q.live then return 'no_npc' end; \
             require('player.player').teleport((q.cell_x + 0.5) * 32, (q.cell_y + 0.5) * 32); \
             return 'ok'"
        ),
    )?;
    if got != "ok" {
        return Err(format!("npc {persist_id} not interactable: {got}").into());
    }
    press(runtime, InputKey::from_char('e'))?;
    Ok(())
}

/// Simulates a single key press: one frame with the key down, one clean frame.
fn press(runtime: &mut LuaRuntime, key: InputKey) -> Result<(), Box<dyn std::error::Error>> {
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

/// 关系流水含指定 NPC/增量/来源的记录（对话/任务副作用可回溯）。
fn assert_relationship_event(
    runtime: &mut LuaRuntime,
    npc_id: &str,
    delta: f64,
    source: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_str(
        runtime,
        &format!(
            "(function() for _, e in ipairs(require('player.relationship').events()) do \
             if e.kind == 'relationship' and e.id == '{npc_id}' and e.delta == {delta} \
             and e.source == '{source}' then return 'true' end end return 'false' end)()"
        ),
        "true",
    )
}

/// 声望流水含指定阵营/增量/来源的记录。
fn assert_reputation_event(
    runtime: &mut LuaRuntime,
    faction: &str,
    delta: f64,
    source: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_str(
        runtime,
        &format!(
            "(function() for _, e in ipairs(require('player.relationship').events()) do \
             if e.kind == 'reputation' and e.id == '{faction}' and e.delta == {delta} \
             and e.source == '{source}' then return 'true' end end return 'false' end)()"
        ),
        "true",
    )
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
