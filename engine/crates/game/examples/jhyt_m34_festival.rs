//! Headless acceptance drive for M34-02：节气与节日事件
//! （`data/festivals.lua` + `world/festival.lua` 的四类消费方：对话条件、采集倍率、
//! 药材行情、出口封山）。
//!
//! 为什么要这一门：节气是**按天派生**的内容开关，写错一个数字不会崩，只会让这句话
//! 永远不出现、或者每一天都出现。所以本样例把三件事钉死：
//!
//! 1. 112 天（四季 × 28 天）逐日扫描：21 个事件日、91 个空日、七日循环不错位；
//! 2. 四类消费方各走**真实玩法路径**（按 E 开对话、按 E 采集、真实出售、门口按 E）：
//!    春分的茶叶单子只在会期听得见、且**茶棚建到 3 级才听得见**（M34-05 的设施门，
//!    本样例真在山庄盖一座茶棚并连升两级）、清明山道采集翻倍、端午药材行情 +20%、
//!    夏至演武给门派声望、中秋灯会给好感与莲子、重阳让一次性采集点按日再刷、
//!    冬至只封"继续往山里走"那一格（回村方向永远可用，否则就是把玩家关在山上）；
//! 3. 节气不入存档：跨天/读档后结论随 `day` 一起回来（拨到别的日子不会"记住"上一个节气）。
//!
//! 另有 10 条负例（窗口重叠、未知效果键、未知地图/出口/物品、无出售价的物品、倍率 <2、缺参数、
//! 某季无事件、对话引用未知节气），全部要求**在校验期真的报出来**。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m34_festival [-- <game_jhyt-root>]

use rustlua_input::input::{InputSnapshot, KeyCode};
use rustlua_scripting::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 山庄出生点（顾长风、苏晚晴、陆归藏都在山庄）。
const MANOR_POS: (f32, f32) = (528.0, 48.0);
/// 云泽村入口落点。
const VILLAGE_POS: (f32, f32) = (528.0, 48.0);
/// 青云山道入口落点（南山门内）。
const MOUNTAIN_POS: (f32, f32) = (624.0, 80.0);
/// 10:00：医馆/剑坪/演武场均有人，且三家店都在营业窗口内（480..1080）。
const TEN_AM: i64 = 600;
/// 山道采集点（灵芝，respawn_days = 1）：清明倍率与平日基线都用它。
const HERB_NODE: &str = "wild_herb_1";
/// 山道一次性采集点（稻穗，respawn_days = 0）：重阳"按日再刷"的载体。
const BERRY_NODE: &str = "wild_berry_1";
/// 山道另一处一次性采集点（柴堆）：重阳窗口**之外**的对照组。
const FIREWOOD_NODE: &str = "wild_firewood_1";
/// 春分茶会的设施门槛（`data/festivals.lua` 里 teafair 的 `unlock.level`）。
const TEA_TARGET_LEVEL: i64 = 3;
/// 春分茶会的接单选项（`data/dialogues/shen_qingluo_herb.lua` ⑤）。
const TEA_INTRO: &str = "沈掌柜，春分茶会可要备茶？";
/// 夏至演武的登台选项（`data/dialogues/gu_changfeng_teach.lua` ⑥）。
const DRILL_OFFER: &str = "夏至演武，弟子愿上台。";
/// 中秋灯会的搭手选项（`data/dialogues/su_wanqing_sect.lua` ②）。
const LANTERN_OFFER: &str = "灯会可要人手？弟子来搭把手。";

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
    let save_path = fresh_save_path(&project, "jhyt_m34_festival.sav");
    let project_arg = project.clone();

    let mut rt = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;

    // ---------------- ① 112 天逐日扫描 ----------------
    banner("M34-02① 112 天（四季 × 28 天）逐日：24 个事件日、88 个空日、循环不错位");
    assert_str(&mut rt, "require('data.festivals').season_length", "28")?;
    assert_num(&mut rt, "#require('data.festivals').festivals", 8.0)?;
    // 事件日的**完整清单**：季内窗口写死在数据里，这里按绝对日数逐日扫出来比对。
    // 少一天、多一天、错一天都会在这条断言上红（不是"大概对上了"）。
    let event_days = query_str(
        &mut rt,
        "(function() local f = require('world.festival'); local out = {}; \
           for d = 1, 112 do local id = f.id(d); if id ~= '' then out[#out + 1] = tostring(d) end end; \
           return table.concat(out, ',') end)()",
    )?;
    let expected_days =
        "8,9,10,15,16,17,36,37,38,43,44,45,64,65,66,71,72,73,99,100,101,110,111,112";
    if event_days != expected_days {
        return Err(
            format!("节气覆盖的日数清单不符：数据给出 {event_days}，期望 {expected_days}").into(),
        );
    }
    // 每个节气恰好三天，且没有第八个 id 冒出来。
    let per_id = query_str(
        &mut rt,
        "(function() local f = require('world.festival'); local seen, order = {}, {}; \
           for d = 1, 112 do local id = f.id(d); \
             if id ~= '' then \
               if seen[id] == nil then seen[id] = 0; order[#order + 1] = id end; \
               seen[id] = seen[id] + 1 \
             end \
           end; table.sort(order); \
           local out = {}; for _, id in ipairs(order) do out[#out + 1] = id .. '=' .. tostring(seen[id]) end; \
           return table.concat(out, ',') end)()",
    )?;
    let expected_per_id =
        "chongyang=3,chuxi=3,dongzhi=3,duanwu=3,midautumn=3,qingming=3,teafair=3,xiazhi=3";
    if per_id != expected_per_id {
        return Err(format!("每个节气的天数分布不符：{per_id}，期望 {expected_per_id}").into());
    }
    println!("  112 天里 24 天有事件（每节气 3 天），88 天无事件：{per_id}");
    // 边界：窗口首尾各错一格。
    for (day, expected) in [
        (7i64, ""),
        (8, "teafair"),
        (10, "teafair"),
        (11, ""),
        (14, ""),
        (15, "qingming"),
        (18, ""),
        (35, ""),
        (36, "duanwu"),
        (39, ""),
        (42, ""),
        (43, "xiazhi"),
        (46, ""),
        (63, ""),
        (64, "midautumn"),
        (67, ""),
        (70, ""),
        (71, "chongyang"),
        (74, ""),
        (98, ""),
        (99, "dongzhi"),
        (102, ""),
        (109, ""),
        (110, "chuxi"),
        (111, "chuxi"),
        (112, "chuxi"),
    ] {
        assert_str(
            &mut rt,
            &format!("require('world.festival').id({day})"),
            expected,
        )?;
    }
    println!("  窗口边界钉死：7/11/14/18/…/102/109 无事件，8/15/36/43/64/71/99/110 起事");
    // 循环：第 113 天回到春 1；第 120 天 == 第 8 天、第 128 天 == 第 16 天（同判）。
    assert_str(&mut rt, "require('world.festival').id(113)", "")?;
    assert_str(&mut rt, "require('world.festival').id(120)", "teafair")?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').id(120) == require('world.festival').id(8))",
        "true",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').id(128) == require('world.festival').id(16))",
        "true",
    )?;
    println!("  113 天 = 春 1（循环复位），120/128 天与 8/16 天同判（四季循环不错位）");
    // 名称（玩家看到的中文称号）与 docs/30 §12 同词。
    assert_str(&mut rt, "require('world.festival').name(8)", "春分茶会")?;
    assert_str(&mut rt, "require('world.festival').name(99)", "冬至封山")?;
    assert_str(&mut rt, "require('world.festival').name(11)", "")?;

    // ---------------- ② 春分茶会：会期听得到、过会听不到、已接的单照常能交 ----------------
    banner("M34-02② 春分（day 8–10）：茶叶单子只在会期听得见；交付不带节气条件（不做死单）");
    assert_str(
        &mut rt,
        "require('validate_data').check({ strict = true })",
        "",
    )?;
    goto_map(&mut rt, "village", VILLAGE_POS)?;
    lua(&mut rt, &format!("clock.set(5, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    assert_str(&mut rt, &option_visible(TEA_INTRO), "false")?;
    choose_option(&mut rt, "改日再说。")?;
    println!("  春 5（会期外）：沈青萝处听不到茶叶单子");
    // M34-05 给春分茶会加的设施门（docs/33 §7.2 茶棚第三档"解锁节气事件"）：
    // **会期里但没有茶棚，这单生意照样听不到**。"今天算不算春分"与"茶会办不办得成"
    // 是两件事，前者永远为真、后者要过 data/festivals.lua 里那条 unlock。
    lua(&mut rt, &format!("clock.set(8, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    assert_str(&mut rt, &option_visible(TEA_INTRO), "false")?;
    choose_option(&mut rt, "改日再说。")?;
    assert_str(&mut rt, "require('world.festival').id()", "teafair")?;
    // 门槛写在**事件**这一侧（不是建筑的等级行里）：这里把 TEA_TARGET_LEVEL 钉回数据，
    // 否则数据改了门槛、本门还在按 3 级盖茶棚，红点会落在错误的一章。
    assert_str(
        &mut rt,
        "(function() local e = require('data.festivals').get('teafair'); \
           return tostring(e.unlock.building) .. ',' .. tostring(e.unlock.level) end)()",
        "tea_shed,3",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').available('teafair'))",
        "false",
    )?;
    println!("  春 8、茶棚未建：日历仍是 teafair，但 available=false，选项不出现");
    // 回山庄真实盖一座茶棚并连升两级（place/upgrade 都走 systems/building.lua 的入口）。
    unlock_teafair(&mut rt, MANOR_POS)?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').available('teafair'))",
        "true",
    )?;
    println!("  茶棚三级之后：available=true（等级记在 state.vars，站在山庄也查得到）");
    goto_map(&mut rt, "village", VILLAGE_POS)?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    assert_str(&mut rt, &option_visible(TEA_INTRO), "true")?;
    assert_str(&mut rt, "require('world.festival').id()", "teafair")?;
    choose_option(&mut rt, TEA_INTRO)?;
    choose_option(&mut rt, "这单我接了。")?;
    choose_option(&mut rt, "记下了。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_tea')",
        "in_progress",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('teafair_started')",
        "true",
    )?;
    println!("  春 8：接单成功，order_tea = in_progress，锚点 teafair_started 已写");
    // 交齐六两茶叶（真实 inv 路径），但先把时钟拨出窗口：交付选项必须照样在。
    lua(
        &mut rt,
        "inv.add(require('core.state').inventory, 'tea_leaf', 6)",
    )?;
    lua(&mut rt, &format!("clock.set(12, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "")?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    assert_str(&mut rt, &option_visible(TEA_INTRO), "false")?;
    assert_str(&mut rt, &option_visible("六两茶叶已备好。"), "true")?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    choose_option(&mut rt, "六两茶叶已备好。")?;
    choose_option(&mut rt, "收下了。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('order_tea')",
        "completed",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('order_tea_done')",
        "true",
    )?;
    assert_num(&mut rt, "require('core.state').money", money_before + 90.0)?;
    assert_num(
        &mut rt,
        "require('player.relationship').reputation('village')",
        8.0,
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'tea_leaf')",
        0.0,
    )?;
    println!("  春 12（会期已过）：交付选项照样在 → +90 文、云泽村声望 +8、六两茶叶已消耗");

    // ---------------- ③ 清明采集倍率 + 端午药材行情 ----------------
    banner("M34-02③ 清明山道采集 ×2（真实按 E）；端午药材出售价 +20%（真实 shop.sell）");
    goto_map(&mut rt, "mountain_path", MOUNTAIN_POS)?;
    lua(&mut rt, "require('systems.gather').reset()")?;
    // 平日基线：春 13（12/14 都在窗口外）采一次 = 数据里的 count。
    let base_count = query_num(&mut rt, "require('data.gather').get('wild_herb_1').count")?;
    lua(&mut rt, &format!("clock.set(13, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "")?;
    let lingzhi_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lingzhi')",
    )?;
    gather_with(&mut rt, HERB_NODE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "gather_lingzhi",
    )?;
    let gained = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lingzhi')",
    )? - lingzhi_before;
    if (gained - base_count).abs() > 1e-6 {
        return Err(format!(
            "平日采集应得 {base_count:.0} 份，实际 {gained:.0}（节气倍率漏到了无事件日）"
        )
        .into());
    }
    println!("  春 13（无事件）：采灵芝 +{gained:.0} = 数据基线 count");
    // 清明：同一节点、同一动作，产物翻倍。
    lua(&mut rt, &format!("clock.set(15, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "qingming")?;
    let mult = query_num(
        &mut rt,
        "require('world.festival').gather_rule('mountain_path').mult",
    )?;
    if (mult - 2.0).abs() > 1e-6 {
        return Err(format!("清明山道采集倍率应为 2，数据给出 {mult}").into());
    }
    // 别的图不受影响（倍率按节点所在图查，不是全局开关）。
    assert_str(
        &mut rt,
        "tostring(require('world.festival').gather_rule('village'))",
        "nil",
    )?;
    let lingzhi_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lingzhi')",
    )?;
    gather_with(&mut rt, HERB_NODE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "gather_lingzhi",
    )?;
    let doubled = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lingzhi')",
    )? - lingzhi_before;
    if (doubled - base_count * mult).abs() > 1e-6 {
        return Err(format!("清明采集应得 {} 份，实际 {doubled:.0}", base_count * mult).into());
    }
    println!("  春 15（清明）：同一节点采一次 +{doubled:.0} = 基线 {base_count:.0} × 2");
    // 端午：药材行情 +20%，走真实出售入口。
    let lingzhi_sell = query_num(&mut rt, "require('data.items').lingzhi.sell")?;
    lua(&mut rt, &format!("clock.set(35, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_num(
        &mut rt,
        "(function() for _, e in ipairs(require('systems.shop').buys('zhao_stall')) do \
           if e.item == 'lingzhi' then return e.price end end return -1 end)()",
        lingzhi_sell,
    )?;
    lua(&mut rt, &format!("clock.set(36, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "duanwu")?;
    let bonus = query_num(&mut rt, "require('world.festival').sell_bonus('lingzhi')")?;
    let offered = query_num(
        &mut rt,
        "(function() for _, e in ipairs(require('systems.shop').buys('zhao_stall')) do \
           if e.item == 'lingzhi' then return e.price end end return -1 end)()",
    )?;
    let expected_offer = lingzhi_sell + (lingzhi_sell * bonus / 100.0).round();
    if (offered - expected_offer).abs() > 1e-6 {
        return Err(format!(
            "端午灵芝收购价 {offered:.0}，期望 {expected_offer:.0}（sell {lingzhi_sell:.0} +{bonus:.0}%）"
        )
        .into());
    }
    // 不在行情表里的东西不受影响（阿七摊收莲子，端午没有莲子行情）。
    let lotus_sell = query_num(&mut rt, "require('data.items').lotus_seed.sell")?;
    assert_num(
        &mut rt,
        "require('world.festival').sell_bonus('lotus_seed')",
        0.0,
    )?;
    assert_num(&mut rt, "require('data.items').lotus_seed.sell", lotus_sell)?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    assert_str(
        &mut rt,
        "select(2, require('systems.shop').sell('zhao_stall', 'lingzhi', 1))",
        "ok",
    )?;
    assert_num(
        &mut rt,
        "require('core.state').money",
        money_before + offered,
    )?;
    println!(
        "  夏 8（端午）：灵芝 sell {lingzhi_sell:.0} → 收购价 {offered:.0}（+{bonus:.0}%），真实卖出到账 +{offered:.0}"
    );

    // ---------------- ④ 夏至演武：门派声望 ----------------
    banner("M34-02④ 夏至（day 43–45）：演武给门派声望 +10、好感 +6、饭团 ×2，锚点只办一次");
    goto_map(&mut rt, "manor", MANOR_POS)?;
    lua(&mut rt, &format!("clock.set(42, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_gu_changfeng")?;
    assert_str(&mut rt, &option_visible(DRILL_OFFER), "false")?;
    choose_option(&mut rt, "改日再来请教。")?;
    println!("  夏 14（会期外）：顾长风不提演武");
    lua(&mut rt, &format!("clock.set(43, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_gu_changfeng")?;
    assert_str(&mut rt, &option_visible(DRILL_OFFER), "true")?;
    assert_num(
        &mut rt,
        "require('player.relationship').reputation('sect')",
        0.0,
    )?;
    // ② 章盖的三级茶棚在这里还在起作用：乘子 = 1 + 0.1（茶棚档位）+ 0（今天没饮茶）。
    assert_num(
        &mut rt,
        "require('player.relationship').social_multiplier()",
        1.1,
    )?;
    let rice_ball_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'rice_ball')",
    )?;
    choose_option(&mut rt, DRILL_OFFER)?;
    choose_option(&mut rt, "弟子登台。")?;
    choose_option(&mut rt, "谢师父。")?;
    // 门派声望走 add_reputation，**不**过社交乘子（docs/33 §8.1 的加成只给好感）：
    // 于是这一章同时钉住"乘了哪一份"和"没乘哪一份"。
    assert_num(
        &mut rt,
        "require('player.relationship').reputation('sect')",
        10.0,
    )?;
    assert_num(
        &mut rt,
        "require('player.relationship').points('npc_gu_changfeng')",
        7.0,
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'rice_ball')",
        rice_ball_before + 2.0,
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('xiazhi_drill_done')",
        "true",
    )?;
    // 锚点生效：会期内再找师父，这句话已经收回（不重复给声望）。
    lua(&mut rt, &format!("clock.set(44, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_gu_changfeng")?;
    assert_str(&mut rt, &option_visible(DRILL_OFFER), "false")?;
    choose_option(&mut rt, "改日再来请教。")?;
    assert_num(
        &mut rt,
        "require('player.relationship').reputation('sect')",
        10.0,
    )?;
    println!(
        "  夏 15：声望 +10（不过乘子）/ 好感 6×1.1 → +7 / 饭团 +2 到账；夏 16 再谈选项已收回（不重复结算）"
    );

    // ---------------- ⑤ 中秋灯会：好感与莲子，不动结局指针 ----------------
    banner("M34-02⑤ 中秋（day 64–66）：点灯给好感 +8 与莲子 ×3，且**不**写 ending 变量");
    goto_map(&mut rt, "manor", MANOR_POS)?;
    lua(&mut rt, &format!("clock.set(63, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_su_wanqing")?;
    assert_str(&mut rt, &option_visible(LANTERN_OFFER), "false")?;
    choose_option(&mut rt, "改日再谈。")?;
    println!("  秋 7（会期外）：苏晚晴不提灯会");
    lua(&mut rt, &format!("clock.set(64, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_su_wanqing")?;
    assert_str(&mut rt, &option_visible(LANTERN_OFFER), "true")?;
    let lotus_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lotus_seed')",
    )?;
    choose_option(&mut rt, LANTERN_OFFER)?;
    choose_option(&mut rt, "今晚我来点灯。")?;
    choose_option(&mut rt, "多谢晚晴。")?;
    assert_num(
        &mut rt,
        "require('player.relationship').points('npc_su_wanqing')",
        9.0,
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'lotus_seed')",
        lotus_before + 3.0,
    )?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('midautumn_lantern')",
        "true",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('core.state').get_var('ending'))",
        "nil",
    )?;
    assert_str(
        &mut rt,
        "require('world.quest').state('ending_sect')",
        "not_started",
    )?;
    println!(
        "  秋 8：好感 8×1.1 → +9、莲子 +3；ending 仍为 nil、ending_sect 未启动（节日不代管结局）"
    );

    // ---------------- ⑥ 重阳：一次性采集点按日再刷 ----------------
    banner("M34-02⑥ 重阳（day 71–73）：一次性采集点按日再刷；出了窗口立刻恢复原语义");
    goto_map(&mut rt, "mountain_path", MOUNTAIN_POS)?;
    lua(&mut rt, "require('systems.gather').reset()")?;
    lua(&mut rt, &format!("clock.set(71, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "chongyang")?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').gather_rule('mountain_path').one_time_daily)",
        "true",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').gather_rule('manor'))",
        "nil",
    )?;
    let rice_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'rice')",
    )?;
    let berry_count = query_num(&mut rt, "require('data.gather').get('wild_berry_1').count")?;
    gather_with(&mut rt, BERRY_NODE)?;
    assert_str(&mut rt, "require('core.state').last_action", "gather_rice")?;
    // 同日再按：仍然拒绝（"按日再刷"不等于"无限刷"）。
    gather_with(&mut rt, BERRY_NODE)?;
    assert_str(&mut rt, "require('core.state').last_action", "gathered")?;
    let once = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'rice')",
    )? - rice_before;
    if (once - berry_count).abs() > 1e-6 {
        return Err(format!(
            "重阳当天首采应得 {berry_count:.0} 份稻穗，实际 {once:.0}（一次性点被节气改了倍率）"
        )
        .into());
    }
    // 次日（仍在窗口内）：一次性点重新可采。
    lua(&mut rt, &format!("clock.set(72, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        "tostring(require('systems.gather').is_gathered('wild_berry_1'))",
        "false",
    )?;
    let rice_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'rice')",
    )?;
    gather_with(&mut rt, BERRY_NODE)?;
    assert_str(&mut rt, "require('core.state').last_action", "gather_rice")?;
    let again = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'rice')",
    )? - rice_before;
    if (again - berry_count).abs() > 1e-6 {
        return Err(format!("重阳次日再采应照样给 {berry_count:.0} 份，实际 {again:.0}").into());
    }
    println!(
        "  秋 15 采 +{once:.0}、同日再采被拒、秋 16 又能采 +{again:.0}（每格一天一次，不是无限刷）"
    );
    // 对照：窗口外的一次性点维持原语义（采完永久消失）。
    lua(&mut rt, &format!("clock.set(74, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "")?;
    let timber_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'timber')",
    )?;
    gather_with(&mut rt, FIREWOOD_NODE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "gather_timber",
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'timber')",
        timber_before + 1.0,
    )?;
    lua(&mut rt, &format!("clock.set(75, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        "tostring(require('systems.gather').is_gathered('wild_firewood_1'))",
        "true",
    )?;
    gather_with(&mut rt, FIREWOOD_NODE)?;
    assert_str(&mut rt, "require('core.state').last_action", "gathered")?;
    println!("  秋 18（出窗口）：柴堆照旧一次性——重阳的再刷没有把语义改成天天刷新");

    // ---------------- ⑦ 冬至封山 ----------------
    banner("M34-02⑦ 冬至封山：只拒'往山里走'那一格，回村方向永远可用（平日对照）");
    // 平日（冬 12 = day 96）：洞口可用，按 E 进寒泉洞。
    goto_map(&mut rt, "mountain_path", MOUNTAIN_POS)?;
    lua(&mut rt, &format!("clock.set(96, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "")?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').exit_closed('mountain_to_cave'))",
        "false",
    )?;
    exit_with(&mut rt, "mountain_to_cave")?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "cold_spring_cave")?;
    println!("  冬 12（无封山）：山道洞口按 E 进寒泉洞");
    // 封山日（冬 15 = day 99）：同一格、同一个键，拒绝并给出中文理由。
    goto_map(&mut rt, "mountain_path", MOUNTAIN_POS)?;
    lua(&mut rt, &format!("clock.set(99, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "dongzhi")?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').exit_closed('mountain_to_cave'))",
        "true",
    )?;
    let cave_stick = exit_stick("mountain_to_cave");
    assert_str(&mut rt, &cave_stick, "ok")?;
    assert_str(
        &mut rt,
        "require('systems.interaction').prompt('up')",
        "E: 进入寒泉洞",
    )?;
    press_e_holding(&mut rt, &cave_stick)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "closed_festival",
    )?;
    assert_scene_is(&mut rt, "mountain_path")?;
    // HUD 必须出中文（不是裸露英文键，也不能走 "exit_" 前缀被念成"前往：…"）。
    assert_str(
        &mut rt,
        "require('ui.hud').feedback('closed_festival')",
        "山道封了，进不得",
    )?;
    assert_str(
        &mut rt,
        "require('ui.hud').feedback(require('core.state').last_action)",
        "山道封了，进不得",
    )?;
    println!(
        "  冬 15：门口按 E 仍是出口命中（提示行照给），但拒绝通行，HUD 出「山道封了，进不得」"
    );
    // 同一天、同一张图：回村那一格必须可用（否则玩家被封在山上）。
    exit_with(&mut rt, "mountain_to_village")?;
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "village")?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').exit_closed('mountain_to_village'))",
        "false",
    )?;
    println!("  同一天回村方向按 E → 云泽村（封山只封数据点名的那一条）");

    // ---------------- ⑧ 存档往返：节气随 day 一起回来 ----------------
    banner("M34-02⑧ 节气不入存档：拨到别的日子不会记住上一个节气，读档后判定一致");
    assert_str(&mut rt, "require('world.festival').id()", "dongzhi")?;
    lua(&mut rt, "require('core.save_manager').save()")?;
    lua(&mut rt, &format!("clock.set(105, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "")?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').exit_closed('mountain_to_cave'))",
        "false",
    )?;
    println!("  冬 21（105 天）：同一个运行期里封山自动解除（没有残留状态要清）");
    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_num(&mut second, "clock.now().day", 99.0)?;
    assert_str(&mut second, "require('world.festival').id()", "dongzhi")?;
    assert_str(
        &mut second,
        "tostring(require('world.festival').exit_closed('mountain_to_cave'))",
        "true",
    )?;
    assert_str(&mut second, "require('world.festival').name()", "冬至封山")?;
    assert_str(
        &mut second,
        "require('world.quest').state('order_tea')",
        "completed",
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').reputation('sect')",
        10.0,
    )?;
    // 存档文件里没有任何"今天是什么节气"的字段——只有分数、锚点与日数。
    let save_text = fs::read_to_string(&save_path).unwrap_or_default();
    for needle in ["jieqi", "festival", "chongyang", "dongzhi"] {
        if save_text.contains(needle) {
            return Err(format!(
                "存档里出现了 {needle:?}——节气应当是 day 的派生值，不入档（派生值入档会漂移）"
            )
            .into());
        }
    }
    println!("  读档：day = 99 → 冬至封山自己回来了；存档正文里没有 festival/jieqi 任何字段");

    // ---------------- ⑨ 负例 ----------------
    banner("M34-02⑨ 十条负例：数据写错必须在校验期报出，而不是静默少一个事件");
    for (label, expr, needle) in NEGATIVE_CASES {
        let issues = query_str(&mut rt, expr)?;
        if !issues.contains(needle) {
            return Err(format!("负例「{label}」未被报出，得到:\n{issues}").into());
        }
        println!("  负例 {label}: 报出 {needle:?}");
    }
    assert_str(
        &mut rt,
        "require('validate_data').check({ strict = true })",
        "",
    )?;
    println!("  十条负例跑完，登记表再查一遍仍然干净");

    println!(
        "jhyt_m34_festival_ok project={} event_days=24/112 windows=8x3 \
         teafair_door=locked_when_unbuilt->open_at_level3 tea_order=completed \
         gather=2/4 sell_lingzhi={:.0} drill_sect_rep=10 social_multiplier=1.1 \
         drill_affinity=7 lantern_affinity=9 \
         chongyang_refresh=daily closed_exit=refused open_exit=village save_festival_fields=0",
        project.display(),
        offered
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- 负例（每条先改坏数据、跑校验、再改回来） ----------

/// `(标题, Lua 表达式, 期望出现在报错文案里的片段)`。
const NEGATIVE_CASES: [(&str, &str, &str); 10] = [
    (
        "窗口重叠",
        "local f = require('data.festivals').festivals; local saved = f[1].to; f[1].to = 15; \
         local issues = table.concat((select(2, require('data.festivals').validate_all())) or {}, '\\n'); \
         f[1].to = saved; return issues",
        "同一天不能有两个节气事件",
    ),
    (
        "未知效果键",
        "local f = require('data.festivals').festivals; f[1].effects.gift_off = 1; \
         local issues = table.concat((select(2, require('data.festivals').validate_all())) or {}, '\\n'); \
         f[1].effects.gift_off = nil; return issues",
        "unknown effect key",
    ),
    (
        "采集倍率 <2",
        "local f = require('data.festivals').festivals; local saved = f[2].effects.gather.mult; \
         f[2].effects.gather.mult = 1; \
         local issues = table.concat((select(2, require('data.festivals').validate_all())) or {}, '\\n'); \
         f[2].effects.gather.mult = saved; return issues",
        "expected integer >= 2",
    ),
    (
        "采集规则两个键都缺",
        "local f = require('data.festivals').festivals; local saved = f[2].effects.gather; \
         f[2].effects.gather = { maps = { 'mountain_path' } }; \
         local issues = table.concat((select(2, require('data.festivals').validate_all())) or {}, '\\n'); \
         f[2].effects.gather = saved; return issues",
        "needs either mult or one_time_daily",
    ),
    (
        "采集引用未知地图",
        "local f = require('data.festivals').festivals; local saved = f[2].effects.gather.maps; \
         f[2].effects.gather.maps = { 'north_sea' }; \
         local issues = table.concat((select(2, require('data.festivals').validate_all())) or {}, '\\n'); \
         f[2].effects.gather.maps = saved; return issues",
        "unknown map",
    ),
    (
        "行情引用未知物品",
        "local f = require('data.festivals').festivals; local saved = f[3].effects.sell_bonus.items; \
         f[3].effects.sell_bonus.items = { 'ghost_herb' }; \
         local issues = table.concat((select(2, require('data.festivals').validate_all())) or {}, '\\n'); \
         f[3].effects.sell_bonus.items = saved; return issues",
        "unknown item",
    ),
    (
        "行情引用没有出售价的物品",
        "local f = require('data.festivals').festivals; local saved = f[3].effects.sell_bonus.items; \
         f[3].effects.sell_bonus.items = { 'ape_core' }; \
         local issues = table.concat((select(2, require('data.festivals').validate_all())) or {}, '\\n'); \
         f[3].effects.sell_bonus.items = saved; return issues",
        "no sell price",
    ),
    (
        "封山引用未知出口",
        "local f = require('data.festivals').festivals; local saved = f[7].effects.exit_closed; \
         f[7].effects.exit_closed = { 'mountain_to_moon' }; \
         local issues = table.concat((select(2, require('data.festivals').validate_all())) or {}, '\\n'); \
         f[7].effects.exit_closed = saved; return issues",
        "unknown exit",
    ),
    (
        "对话引用未知节气",
        "local d = require('data.dialogues.shen_qingluo_herb'); local option; \
         for _, e in ipairs(d.nodes.greet.options) do \
           if e.cond and e.cond.festival then option = e; break end \
         end; \
         if not option then return 'missing:cond.festival' end; \
         local saved = option.cond.festival; option.cond.festival = 'teafay'; \
         local issues = table.concat((select(2, require('world.dialogue').validate_all())) or {}, '\\n'); \
         option.cond.festival = saved; return issues",
        "引用未知节气",
    ),
    (
        "某一季一个事件都没有",
        // 阶段2（1.5）补足除夕后冬季有两个事件（冬至 f[7] + 除夕 f[8]）：要造出"某季
        // 一个事件都没有"，必须把冬季两个都挪走——f[7]→秋 20-22、f[8]→秋 23-25（不重叠）。
        "local f = require('data.festivals').festivals; \
         local s0, s1, s2 = f[7].season, f[7].from, f[7].to; \
         local t0, t1, t2 = f[8].season, f[8].from, f[8].to; \
         f[7].season = 'autumn'; f[7].from = 20; f[7].to = 22; \
         f[8].season = 'autumn'; f[8].from = 23; f[8].to = 25; \
         local issues = table.concat((select(2, require('data.festivals').validate_all())) or {}, '\\n'); \
         f[7].season, f[7].from, f[7].to = s0, s1, s2; \
         f[8].season, f[8].from, f[8].to = t0, t1, t2; return issues",
        "has no festival event",
    ),
];

// ---------- helpers（与 jhyt_m34_relationship_tier 同一套） ----------

/// 当前对话里是否看得见某个选项文本。
fn option_visible(text: &str) -> String {
    format!(
        "(function() for _, e in ipairs(require('world.dialogue').options()) do \
           if e.text == [[{text}]] then return true end end return false end)()"
    )
}

/// 站到某个采集点正下方一格、面朝 up 的 Lua 表达式（返回命中的节点 id，供确认）。
fn node_stick(node_id: &str) -> String {
    format!(
        "(function() local d = require('data.gather').get('{node_id}'); \
           if not d then return 'no_def' end; \
           require('player.player').teleport((d.cell.x + 0.5) * 32, (d.cell.y - 0.5) * 32); \
           require('core.state').facing = 'up'; \
           local a = require('systems.gather').available('up'); \
           if not a or not a.node then return 'no_node' end; \
           return tostring(a.node.id) end)()"
    )
}

/// 站到某个出口的触发格正中（`map_manager` 命中条件①）的 Lua 表达式。
fn exit_stick(exit_id: &str) -> String {
    format!(
        "(function() local exit = require('data.maps').exits['{exit_id}']; \
           if not exit then return 'no_exit' end; \
           require('player.player').teleport((exit.cell.x + 0.5) * 32, (exit.cell.y + 0.5) * 32); \
           require('core.state').facing = 'up'; \
           local cx, cy = require('world.map_manager').player_cell(); \
           if cx ~= exit.cell.x or cy ~= exit.cell.y then \
             return 'cell_' .. tostring(cx) .. '_' .. tostring(cy) \
           end; \
           return 'ok' end)()"
    )
}

/// 贴住站位按一次 E：每帧重新贴回（物理步会把重叠刚体推开，与 `talk_to` 同一手法）。
fn press_e_holding(
    runtime: &mut LuaRuntime,
    stick: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyE, true);
    runtime.set_input(input);
    lua(runtime, stick)?;
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    lua(runtime, stick)?;
    runtime.call_update()?;
    Ok(())
}

/// 站到采集点前并按 E（真实交互路径）。
fn gather_with(runtime: &mut LuaRuntime, node_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let stick = node_stick(node_id);
    let got = query_str(runtime, &stick)?;
    if got != node_id {
        return Err(format!("采集点 {node_id} 的面向格未命中：{got}").into());
    }
    press_e_holding(runtime, &stick)
}

/// 站到出口触发格上并按 E（真实交互路径）。
fn exit_with(runtime: &mut LuaRuntime, exit_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let stick = exit_stick(exit_id);
    let got = query_str(runtime, &stick)?;
    if got != "ok" {
        return Err(format!("出口 {exit_id} 未站到触发格上：{got}").into());
    }
    // 提示行确认交互解析器选中的就是这个出口（priority 20 抢在采集/对话之前）。
    let prompt = query_str(runtime, "require('systems.interaction').prompt('up')")?;
    if !prompt.starts_with("E: ") {
        return Err(format!("出口 {exit_id} 的提示行异常：{prompt}").into());
    }
    press_e_holding(runtime, &stick)
}

/// 扫描当前地图，返回第一个可放置该建筑的锚点（确定性，与 jhyt_m27_goals 同法）。
fn first_legal_anchor(
    runtime: &mut LuaRuntime,
    building_id: &str,
) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "(function() local b = require('systems.building'); \
             for y = 0, 21 do for x = 0, 39 do \
               if b.can_place('{building_id}', x, y) then return x .. ',' .. y end end end \
             return 'none' end)()"
        ),
    )?;
    let mut parts = got.split(',');
    let x = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("山庄没有可放置 {building_id} 的格子: {got}"))?;
    let y = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("山庄没有可放置 {building_id} 的格子: {got}"))?;
    Ok((x, y))
}

/// 在山庄真盖一座茶棚并连升两级（M34-05 的春分茶会设施门）。
///
/// 走 `systems.building` 的 place/upgrade 入口，不用后门：升级要真的扣木材（每级 3），
/// 等级要真的经 `persist()` 写进 `building_level_tea_shed`（历史最高，随 state 进存档）。
fn unlock_teafair(
    runtime: &mut LuaRuntime,
    spawn: (f32, f32),
) -> Result<(), Box<dyn std::error::Error>> {
    goto_map(runtime, "manor", spawn)?;
    frames(runtime, 2)?;
    // 每级收一份 upgrade_cost（docs/33 §7.2 口径）：木材消耗从数据里读，不写死。
    let step_cost = query_num(
        runtime,
        "(function() local t = 0; \
           for _, c in ipairs(require('data.buildings').get('tea_shed').upgrade_cost or {}) do \
             if c.item == 'timber' then t = t + c.count end end return t end)()",
    )?;
    if step_cost <= 0.0 {
        return Err("茶棚升级不再收木材：设施门的扣料路径失效".into());
    }
    let needed = step_cost * ((TEA_TARGET_LEVEL - 1) as f64);
    let timber_before = query_num(
        runtime,
        "inv.count(require('core.state').inventory, 'timber')",
    )?;
    lua(
        runtime,
        &format!(
            "inv.add(require('core.state').inventory, 'timber', {})",
            needed as i64
        ),
    )?;
    let (bx, by) = first_legal_anchor(runtime, "tea_shed")?;
    assert_str(
        runtime,
        &format!("select(1, require('systems.building').place('tea_shed', {bx}, {by}))"),
        "true",
    )?;
    assert_str(
        runtime,
        "require('core.state').get_var('building_level_tea_shed')",
        "1",
    )?;
    for level in 2..=TEA_TARGET_LEVEL {
        assert_str(
            runtime,
            &format!("select(1, require('systems.building').upgrade({bx}, {by}))"),
            "true",
        )?;
        assert_str(
            runtime,
            &format!("tostring(require('systems.building').at({bx}, {by}).level)"),
            &level.to_string(),
        )?;
        assert_str(
            runtime,
            "require('core.state').get_var('building_level_tea_shed')",
            &level.to_string(),
        )?;
    }
    assert_str(
        runtime,
        "require('core.state').last_action",
        &format!("upgrade_tea_shed_{TEA_TARGET_LEVEL}"),
    )?;
    // 木材被连升吃光：等级确实走的是扣料路径，不是白来的。
    assert_num(
        runtime,
        "inv.count(require('core.state').inventory, 'timber')",
        timber_before,
    )?;
    Ok(())
}

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

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

fn click_button(runtime: &mut LuaRuntime, button: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("on_click('{button}')"))?;
    runtime.process_scene_transition()?;
    Ok(())
}

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
