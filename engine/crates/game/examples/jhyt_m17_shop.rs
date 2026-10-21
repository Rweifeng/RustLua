//! Headless acceptance drive for M17-04 《江湖有田》商店经济与物品使用闭环.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M17-04 acceptance paths:
//!
//! - 纯数据注册：商店全部来自 `data/shops.lua`，加载期校验通过（价格/限购/收购套利）。
//! - 营业时间：`clock.now().minute_of_day` 落在 `hours` 窗口内才可交易（左闭右开）。
//! - 购买/出售：金币与库存同步变化，限购按日累计。
//! - 原子性：背包满 / 余额不足 / 未营业 / 超限购 / 收购外 全部拒绝且无半完成状态。
//! - 商店 UI：E 交互开面板，数字键选择、+/- 数量、F 切页、空格确认、Esc 关闭。
//! - 物品使用：效果真实作用于 state，无效果/已满则拒绝而不消耗（硬风险③）。
//! - 每日刷新：`day.sleep()` → `on_day_start` 清零当日限购计数。
//! - 存档恢复：任务奖励与商店交易（含当日限购计数）随存档一致恢复。
//! - 重复交易：连续/重复调用结果确定，不产生重复计账。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m17_shop [-- <game_jhyt-root>]

use rustlua_ecs::entity::EntityId;
use rustlua_input::input::{InputSnapshot, KeyCode};
use rustlua_scripting::script::LuaRuntime;
use rustlua_ui::ui::UiText;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 山庄 → 云泽村出口格 (16,1) 世界坐标。
const MAIN_EXIT: (f32, f32) = (528.0, 48.0);
/// 10:00：赵铁算盘在云泽村货摊（schedule 480..720）。
const TEN_AM: i64 = 600;
/// 新档初始灵芝菌种数（`scripts/player/inventory.lua` 的 `inventory.setup()`）。
/// M19-01 按 MVP 剧本把 5 调到 8；改动新档载荷必须同步本常量与下方断言。
const START_SEED_LINGZHI: f64 = 8.0;
/// 新档初始稻种数（同上，M19-01 由 3 调到 4）。
const START_SEED_RICE: f64 = 4.0;
/// `data/shops.lua` 的商店条数。`1cd624b`（2026-10-01）把示例工程从单店扩到三家
/// （云泽村货摊 / 韩氏铁铺 / 阿七食摊），本常量随之从 1 调到 3。
/// 新增商店时必须同步本常量并补该店自己的营业时间/库存断言——下面第 ② 条把常量
/// 钉在 `data/shops.lua` 的实际条数上，改了数据不改用例就会红（与 m24 的白名单同思路）。
const SHOP_COUNT: f64 = 3.0;
/// 数 `data/shops.lua` 的顶层商店键（排除 `version` 字段）。
const SHOP_DATA_COUNT_LUA: &str = "(function() local n = 0 \
     for k in pairs(require('data.shops')) do if k ~= 'version' then n = n + 1 end end \
     return n end)()";

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
    let save_path = fresh_save_path(&project, "jhyt_m17_shop.sav");
    let project_arg = project.clone();

    banner("M17-04a 纯数据注册 + 加载期校验：商店来自 data/shops.lua");
    let (mut runtime, host) = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut runtime, &main_script)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_mode(&mut runtime, "Playing")?;
    assert_num(&mut runtime, "#(require('systems.shop').ids())", SHOP_COUNT)?;
    // ② 把上一条的常量钉回数据源：注册数必须等于 `data/shops.lua` 的实际条数，
    // 否则"加了商店但用例不知道"就会悄悄绿着过去。
    assert_num(&mut runtime, SHOP_DATA_COUNT_LUA, SHOP_COUNT)?;
    assert_str(
        &mut runtime,
        "require('systems.shop').get('zhao_stall').name",
        "云泽村·货摊",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.shop').get('zhao_stall').keeper",
        "npc_zhao_tiesuanpan",
    )?;
    // 价格对齐设计案 §5.2：种子买价、回复药买/卖价。
    assert_num(&mut runtime, "require('data.items').seed_rice.buy", 12.0)?;
    assert_num(&mut runtime, "require('data.items').seed_lingzhi.buy", 25.0)?;
    assert_num(&mut runtime, "require('data.items').heal_pill.buy", 60.0)?;
    assert_num(&mut runtime, "require('data.items').heal_pill.sell", 20.0)?;
    assert_num(
        &mut runtime,
        "require('data.items').heal_pill.use.energy",
        40.0,
    )?;
    // M18-02 起货摊额外上架 timber；2026-09-27 Alpha 作物扩展再上架 4 种新种子
    // （seed_radish/seed_herb/seed_polygonum/seed_ginseng），共 8 项；buys 同步 +4
    // （radish/herb/polygonum/ginseng）→ 6 项，其后 `1cd624b` 又把 `iron_ore` 加进收购
    // 列表 → 7 项（该 commit 扩到三家店时漏更新了这里）。
    // 阶段2（docs/33 §5.2，1.0 起）：四季种子 24+6、肥料 3、果树苗 5 全部由货摊供应
    // → stock 34；buys 同步覆盖全部收获物与树果 → 30。新增项都在列表末尾，[2] 不受影响。
    assert_num(
        &mut runtime,
        "#(require('systems.shop').stock('zhao_stall'))",
        34.0,
    )?;
    assert_num(
        &mut runtime,
        "#(require('systems.shop').buys('zhao_stall'))",
        30.0,
    )?;
    assert_validate(&mut runtime, "systems.shop")?;
    assert_validate(&mut runtime, "world.quest")?;
    assert_validate(&mut runtime, "world.dialogue")?;
    assert_str(
        &mut runtime,
        "(function() local d = require('systems.shop').get('zhao_stall'); local old = d.hours; d.hours = nil; local open = require('systems.shop').is_open('zhao_stall'); d.hours = old; return open end)()",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "(function() local d = require('world.quest').get('quest_herb').objectives[1]; local old = d.kind; d.kind = 'invalid'; local ok, issues = require('world.quest').validate_all(); d.kind = old; return not ok and table.concat(issues, '|'):find('kind 非法', 1, true) ~= nil end)()",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "(function() local d = require('systems.shop').get('zhao_stall'); local old = d.hours.close; d.hours.close = 1500; local ok, issues = require('systems.shop').validate_all(); d.hours.close = old; return not ok and table.concat(issues, '|'):find('hours 窗口非法', 1, true) ~= nil end)()",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "(function() local d = require('systems.shop').get('zhao_stall'); local old = d.keeper; d.keeper = 'npc_missing'; local ok, issues = require('systems.shop').validate_all(); d.keeper = old; return not ok and table.concat(issues, '|'):find('keeper 引用未知 NPC', 1, true) ~= nil end)()",
        "true",
    )?;

    banner("M17-04b 营业时间：左闭右开，窗口外拒绝交易");
    lua(&mut runtime, "clock.set(1, 480)")?;
    assert_str(
        &mut runtime,
        "require('systems.shop').is_open('zhao_stall')",
        "true",
    )?;
    lua(&mut runtime, "clock.set(1, 1079)")?;
    assert_str(
        &mut runtime,
        "require('systems.shop').is_open('zhao_stall')",
        "true",
    )?;
    lua(&mut runtime, "clock.set(1, 1080)")?;
    assert_str(
        &mut runtime,
        "require('systems.shop').is_open('zhao_stall')",
        "false",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').is_open('zhao_stall'))",
        "closed",
    )?;
    lua(&mut runtime, "clock.set(1, 300)")?;
    assert_str(
        &mut runtime,
        "require('systems.shop').is_open('zhao_stall')",
        "false",
    )?;
    // 未营业时购买被拒，金币与库存不变。
    lua(&mut runtime, "require('core.state').money = 500")?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_rice', 1))",
        "closed",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 500.0)?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_rice')",
        START_SEED_RICE,
    )?;

    banner("M17-04c API 购买：金币与库存同步变化");
    lua(&mut runtime, "clock.set(1, 600)")?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_rice', 2))",
        "ok",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 476.0)?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_rice')",
        START_SEED_RICE + 2.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shop').used('zhao_stall', 'seed_rice')",
        2.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "buy_seed_rice",
    )?;

    banner("M17-04d 商店 UI：E 交互开面板 + 键盘选择/数量/确认/切页/关闭");
    teleport(&mut runtime, MAIN_EXIT)?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "village")?;
    assert_map_current(&mut runtime, "village")?;
    lua(&mut runtime, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut runtime, &host, 2)?;
    goto_npc(&mut runtime, "npc_zhao_tiesuanpan")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_mode(&mut runtime, "Shop")?;
    assert_str(&mut runtime, "require('systems.shop').ui_active()", "true")?;
    assert_str(&mut runtime, "require('systems.shop').tab()", "buy")?;
    // buy 页行数 = zhao_stall stock 数（34，含阶段2 四季种子/肥料/果树苗）。
    assert_num(&mut runtime, "#(require('systems.shop').rows())", 34.0)?;
    assert_str(&mut runtime, "require('core.state').last_action", "shop")?;
    // 选第 2 行（seed_lingzhi），数量 +1。
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('2').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut runtime, "require('systems.shop').selected()", 2.0)?;
    assert_num(&mut runtime, "require('systems.shop').quantity()", 1.0)?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('+').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut runtime, "require('systems.shop').quantity()", 2.0)?;
    let money_before = query_num(&mut runtime, "return require('core.state').money")?;
    press(&mut runtime, &host, KeyCode::Space)?;
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_before - 50.0,
    )?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_lingzhi')",
        START_SEED_LINGZHI + 2.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shop').used('zhao_stall', 'seed_lingzhi')",
        2.0,
    )?;
    // M17-04d 的商店 UI 在 M73 已迁进 `assets/prefabs/ui/shop.prefab`：反馈文案由
    // `prefab.set_text(handle, "ShopMsg", ...)` 写进 `ShopMsg` 的 `Text` 组件，
    // 不再有动态层 `ui.get('shp:msg')`（旧断言会直接 index nil）。
    let shop_msg = prefab_text(&mut runtime, "ShopMsg")?;
    if shop_msg != "购买成功" {
        return Err(format!("ShopMsg = {shop_msg:?}, want \"购买成功\"").into());
    }
    // F 切到出售页：只列出本店收购且背包持有的物品（rice 2 件）。
    press(&mut runtime, &host, KeyCode::KeyF)?;
    assert_str(&mut runtime, "require('systems.shop').tab()", "sell")?;
    assert_num(&mut runtime, "#(require('systems.shop').rows())", 1.0)?;
    assert_str(
        &mut runtime,
        "require('systems.shop').rows()[1].item",
        "rice",
    )?;
    // Esc 关闭面板，模式复位。
    press(&mut runtime, &host, KeyCode::Escape)?;
    assert_str(&mut runtime, "require('systems.shop').ui_active()", "false")?;
    assert_mode(&mut runtime, "Playing")?;

    banner("M17-04e 拒绝路径与原子性：未营业 / 余额 / 限购 / 背包满 / 收购外");
    // 未营业时 E 交互：M19-01 给店主加了订单对话（priority 10 且可用），而未营业的
    // 商店目标 can_interact=false 会被 interaction 排到可用候选之后 → 按键先进入
    // 对话而不是「closed」反馈（内容变化，非商店逻辑回归）。这里断言**没有打开
    // 商店面板**；「未营业拒绝交易」由上面的 is_open/buy API 断言覆盖。
    lua(&mut runtime, "clock.set(1, 300)")?;
    goto_npc(&mut runtime, "npc_zhao_tiesuanpan")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    if query_str(&mut runtime, "require('core.state').mode")? == "Dialogue" {
        press(&mut runtime, &host, KeyCode::Escape)?;
    }
    assert_str(
        &mut runtime,
        "tostring(require('systems.shop').ui_active())",
        "false",
    )?;
    assert_mode(&mut runtime, "Playing")?;
    lua(&mut runtime, "clock.set(1, 600)")?;
    frames(&mut runtime, &host, 1)?;

    // 余额不足：拒绝且不改金币/背包。
    lua(&mut runtime, "require('core.state').money = 10")?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'heal_pill', 1))",
        "poor",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 10.0)?;
    assert_num(&mut runtime, "inv.count('player', 'heal_pill')", 0.0)?;

    // 每日限购：seed_lingzhi limit=5，已购 2 → 再买 3 成功、第 4 件被拒。
    lua(&mut runtime, "require('core.state').money = 1000")?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_lingzhi', 3))",
        "ok",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 925.0)?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_lingzhi')",
        START_SEED_LINGZHI + 5.0,
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_lingzhi', 1))",
        "limit",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 925.0)?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_lingzhi')",
        START_SEED_LINGZHI + 5.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shop').stock('zhao_stall')[2].remaining",
        0.0,
    )?;

    // 背包满：容量 3 且三格非空 → 购买被拒，且不产生「扣款/半入包」。
    lua(&mut runtime, "require('core.state').money = 500")?;
    lua(&mut runtime, "inv.create('player', 3)")?;
    lua(&mut runtime, "inv.set('player', 1, 'seed_rice', 1)")?;
    lua(&mut runtime, "inv.set('player', 2, 'seed_lingzhi', 1)")?;
    lua(&mut runtime, "inv.set('player', 3, 'rice', 1)")?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'heal_pill', 1))",
        "full",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 500.0)?;
    assert_num(&mut runtime, "inv.count('player', 'heal_pill')", 0.0)?;
    assert_num(&mut runtime, "inv.count('player', 'seed_rice')", 1.0)?;
    // 恢复标准背包（含锄头装备槽）。
    lua(&mut runtime, "require('player.inventory').setup()")?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_rice')",
        START_SEED_RICE,
    )?;

    // 出售类拒绝：不收购 / 持有不足 / 数量非法 / 未知商店与物品。
    lua(&mut runtime, "require('core.state').money = 100")?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').sell('zhao_stall', 'seed_rice', 1))",
        "not_bought",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').sell('zhao_stall', 'rice', 5))",
        "not_enough",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').sell('zhao_stall', 'heal_pill', 1))",
        "not_bought",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_rice', 0))",
        "bad_count",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('no_such_shop', 'seed_rice', 1))",
        "unknown_shop",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'no_such_item', 1))",
        "not_sold",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 100.0)?;
    assert_num(&mut runtime, "inv.count('player', 'rice')", 2.0)?;

    banner("M17-04f 出售与重复交易：金币库存同步，计数不重复");
    lua(&mut runtime, "inv.add('player', 'rice', 4)")?;
    assert_num(&mut runtime, "inv.count('player', 'rice')", 6.0)?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').sell('zhao_stall', 'rice', 2))",
        "ok",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 160.0)?;
    assert_num(&mut runtime, "inv.count('player', 'rice')", 4.0)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "sell_rice",
    )?;
    // 重复同一笔交易：逐次等量结算，不翻倍也不丢失。
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').sell('zhao_stall', 'rice', 2))",
        "ok",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 220.0)?;
    assert_num(&mut runtime, "inv.count('player', 'rice')", 2.0)?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').sell('zhao_stall', 'rice', 2))",
        "ok",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 280.0)?;
    assert_num(&mut runtime, "inv.count('player', 'rice')", 0.0)?;
    // 卖空后再卖：拒绝且不进账。
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').sell('zhao_stall', 'rice', 1))",
        "not_enough",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 280.0)?;
    // 重复购买：每次各扣一件的价钱、各加一件。
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_rice', 1))",
        "ok",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 268.0)?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_rice')",
        START_SEED_RICE + 1.0,
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_rice', 1))",
        "ok",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 256.0)?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_rice')",
        START_SEED_RICE + 2.0,
    )?;

    banner("M17-04g 物品使用：效果真实生效，无效果/已满则拒绝且不消耗");
    lua(&mut runtime, "require('core.state').energy = 20")?;
    lua(&mut runtime, "inv.add('player', 'heal_pill', 2)")?;
    assert_num(&mut runtime, "inv.count('player', 'heal_pill')", 2.0)?;
    assert_str(
        &mut runtime,
        "select(2, require('player.inventory').use('heal_pill'))",
        "ok",
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 60.0)?;
    assert_num(&mut runtime, "inv.count('player', 'heal_pill')", 1.0)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "use_heal_pill",
    )?;
    // 效果写入 state 快照：capture 与运行时一致（硬风险③）。
    assert_num(&mut runtime, "require('core.state').capture().energy", 60.0)?;
    // 无 use 字段的物品：拒绝且不消耗。
    assert_str(
        &mut runtime,
        "select(2, require('player.inventory').use('seed_rice'))",
        "no_effect",
    )?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_rice')",
        START_SEED_RICE + 2.0,
    )?;
    // 已满上限：同样拒绝且不消耗。
    lua(&mut runtime, "require('core.state').energy = 100")?;
    assert_str(
        &mut runtime,
        "select(2, require('player.inventory').use('heal_pill'))",
        "no_effect",
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    assert_num(&mut runtime, "inv.count('player', 'heal_pill')", 1.0)?;

    banner("M17-04h 每日刷新：day.sleep → on_day_start 清零当日限购");
    assert_num(
        &mut runtime,
        "require('systems.shop').used('zhao_stall', 'seed_lingzhi')",
        5.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shop').stock('zhao_stall')[2].remaining",
        0.0,
    )?;
    lua(&mut runtime, "day.sleep()")?;
    assert_num(&mut runtime, "clock.now().day", 2.0)?;
    assert_num(
        &mut runtime,
        "require('systems.shop').used('zhao_stall', 'seed_lingzhi')",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.shop').stock('zhao_stall')[2].remaining",
        5.0,
    )?;
    // 日结算重置体力（main.on_day_start）。
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    lua(&mut runtime, "clock.set(2, 600)")?;
    frames(&mut runtime, &host, 1)?;

    banner("M17-04i 存档恢复：任务奖励 + 商店交易（含限购计数）");
    lua(&mut runtime, "require('core.state').money = 1000")?;
    assert_str(
        &mut runtime,
        "tostring(require('world.quest').accept('quest_herb'))",
        "true",
    )?;
    lua(&mut runtime, "inv.add('player', 'lingzhi', 3)")?;
    // 交付成功路径只返回单值 true（失败路径才带 reason），故断言返回值而非 select(2)。
    assert_str(
        &mut runtime,
        "tostring(require('world.quest').deliver('quest_herb'))",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_herb')",
        "completed",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 1050.0)?;
    assert_num(&mut runtime, "inv.count('player', 'rice')", 2.0)?;
    // 商店交易：继续买入 2 枚灵芝菌种（当日限购已刷新）。
    assert_str(
        &mut runtime,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_lingzhi', 2))",
        "ok",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 1000.0)?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_lingzhi')",
        START_SEED_LINGZHI + 2.0,
    )?;
    lua(&mut runtime, "require('core.state').energy = 30")?;
    save_game(&mut runtime)?;

    let (mut second, _host) = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_scene_is(&mut second, "village")?;
    assert_map_current(&mut second, "village")?;
    assert_validate(&mut second, "systems.shop")?;
    assert_num(&mut second, "require('core.state').money", 1000.0)?;
    assert_num(&mut second, "require('core.state').energy", 30.0)?;
    assert_num(
        &mut second,
        "inv.count('player', 'seed_lingzhi')",
        START_SEED_LINGZHI + 2.0,
    )?;
    assert_num(&mut second, "inv.count('player', 'rice')", 2.0)?;
    assert_num(&mut second, "inv.count('player', 'heal_pill')", 1.0)?;
    assert_str(
        &mut second,
        "require('world.quest').state('quest_herb')",
        "completed",
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('herb_delivered')",
        "true",
    )?;
    // 当日限购计数随存档恢复：剩余 3，再买 3 成功、第 4 件被拒。
    assert_num(
        &mut second,
        "require('systems.shop').used('zhao_stall', 'seed_lingzhi')",
        2.0,
    )?;
    assert_num(
        &mut second,
        "require('systems.shop').stock('zhao_stall')[2].remaining",
        3.0,
    )?;
    lua(&mut second, &format!("clock.set(2, {TEN_AM})"))?;
    assert_str(
        &mut second,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_lingzhi', 3))",
        "ok",
    )?;
    assert_str(
        &mut second,
        "select(2, require('systems.shop').buy('zhao_stall', 'seed_lingzhi', 1))",
        "limit",
    )?;
    // 读档后重复交付仍被拦截（完成态随存档恢复）。
    assert_str(
        &mut second,
        "select(2, require('world.quest').deliver('quest_herb'))",
        "already_completed",
    )?;

    println!(
        // `shops` 必须是运行期读回来的实际注册数，不能写死——上一版这里硬编 `shops=1`，
        // 工程扩到三家店之后，这行**通过日志**给出的读数与断言的事实相反（假读数）。
        "jhyt_m17_shop_ok project={} shops={} money={} seed_lingzhi={} rice={} energy={}",
        project.display(),
        query_num(&mut second, "return #(require('systems.shop').ids())")? as i64,
        query_num(&mut second, "return require('core.state').money")?,
        query_num(&mut second, "return inv.count('player', 'seed_lingzhi')")?,
        query_num(&mut second, "return inv.count('player', 'rice')")?,
        query_num(&mut second, "return require('core.state').energy")?,
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers ----------

/// Creates a bare engine runtime pointing at the project scripts & save file.
fn new_runtime(
    project: &Path,
    save_path: &Path,
) -> Result<(LuaRuntime, rustlua_input::host::HostInput), Box<dyn std::error::Error>> {
    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let host = handles.input;
    let mut runtime = LuaRuntime::with_installers(installers)?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok((runtime, host))
}

/// Runs the project script, then loads the `main` 门面 scene so BtnStart/BtnContinue
/// are live (mirrors jhyt_smoke).
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

/// Teleports the player to a world position via `player.teleport`.
fn teleport(runtime: &mut LuaRuntime, pos: (f32, f32)) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "require('player.player').teleport({}, {})",
        pos.0, pos.1
    ))?;
    Ok(())
}

/// 把玩家传送到 NPC 的逻辑格心，供随后按 E 触发 interaction。
fn goto_npc(runtime: &mut LuaRuntime, persist_id: &str) -> Result<(), Box<dyn std::error::Error>> {
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
    Ok(())
}

/// Simulates a single key press: one frame with the key down, one clean frame.
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

/// Runs `update()` for `n` frames with an empty input snapshot.
fn frames(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    n: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    host.set_input(InputSnapshot::default());
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

/// 读预设实例节点的 `Text` 组件。预设节点与场景实体共用同一张全局名字表（`entity.find`），
/// 所以 `ShopMsg` 这个名字拿到的就是 `prefab.spawn` 出来的那个实例节点。
fn prefab_text(runtime: &mut LuaRuntime, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let raw = query_str(runtime, &format!("return tostring(entity.find('{name}'))"))?;
    let raw: u64 = raw
        .parse()
        .map_err(|error| format!("entity.find('{name}') = {raw:?} 不是实体 id：{error}"))?;
    let id = EntityId::from_raw((raw >> 32) as u32, raw as u32);
    let text = runtime.with_world(|world| {
        world
            .get::<UiText>(id)
            .ok()
            .flatten()
            .map(|text| text.text.clone())
    });
    text.ok_or_else(|| format!("预设节点 {name:?} 不存在或没有 Text 组件").into())
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

fn save_game(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    let ok = query_str(
        runtime,
        "return tostring(require('core.save_manager').save())",
    )?;
    if ok != "true" {
        return Err(format!("save() returned {ok:?}").into());
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
