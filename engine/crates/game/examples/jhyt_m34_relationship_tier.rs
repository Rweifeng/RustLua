//! Headless acceptance drive for M34-01：关系与声望档位接线
//! （`data/relationships.lua` 的 `tiers`/`reputation_tiers` + 商店折扣 + 结局门槛 + 对话分支）。
//!
//! 为什么要这一门：§8.2 的声望档位表在 M18-04 之后一直是"只有分数、没有档位消费方"的
//! 死数据——存档里躺着 reputation 数字，商店按标价卖、结局按物品交付，谁也没读过它。
//! 本样例把"到档"变成可验收行为：
//!
//! 1. 档位表自身的结构校验（含 6 类负例：空洞、首末档越界、折扣倒挂、未知效果键、
//!    未知阵营引用、任务/对话里写错档位名）；
//! 2. 商店**购买价**按店主阵营档位逐档打折（0/5/8/12/15%），边界值 19/20/39/40/59/60/
//!    79/80/100 与负声望都钉住；出售价与标价数据不受折扣影响；
//! 3. 结局交付的声望门槛（`kind = "reputation"` 目标）：不到档对话里没有交付选项，
//!    到档即放行；同时钉住"断念不设门槛"这条保底路径；
//! 4. 对话的关系档位分支：陌生时看不见，真实送礼刷到"友善"才出现，说过一次由锚点变量
//!    不再重复；每日限次与"不收此物"走同一入口；
//! 5. 任务日志的进度行（声望目标显示 1/1 而不是分数）；
//! 6. 存档往返：档位是派生值，分数回来折扣/门槛/分支就回来。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m34_relationship_tier [-- <game_jhyt-root>]

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
/// 10:00：沈青萝在医馆、顾长风在剑坪、阿七在村口（NPC 日程）。
const TEN_AM: i64 = 600;
/// 货摊与阿七食摊都营业的时刻（480..1080 / 420..1200）。
const SHOP_HOUR: i64 = 600;
/// 体己话分支的选项文本（`data/dialogues/lu_guizang_truth.lua`）。
const SECRET_OPTION: &str = "师叔可还有体己话？";

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
    let save_path = fresh_save_path(&project, "jhyt_m34_tier.sav");
    let project_arg = project.clone();

    // ---------------- ① 档位表结构与校验（含负例） ----------------
    banner("M34-01① 五档关系 / 五档声望：结构与交叉引用可校验，负例必须真的报出来");
    let mut rt = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;
    assert_num(&mut rt, "#require('data.relationships').tiers", 5.0)?;
    assert_num(
        &mut rt,
        "#require('data.relationships').reputation_tiers",
        5.0,
    )?;
    // 折扣效果键的白名单只有 price_off：新增键必须先有消费方，否则这里红。
    assert_str(
        &mut rt,
        "tostring(require('data.relationships').effect_keys.price_off)",
        "true",
    )?;
    assert_str(
        &mut rt,
        "require('validate_data').check({ strict = true })",
        "",
    )?;
    let negative_cases: [(&str, &str, &str); 6] = [
        (
            "空洞",
            "local t = require('data.relationships').reputation_tiers; local saved = t[3].min; \
             t[3].min = 41; local issues = table.concat((select(2, require('data.relationships').validate_all())) or {}, '\\n'); \
             t[3].min = saved; return issues",
            "contiguous",
        ),
        (
            "末档不覆盖声望上限",
            "local t = require('data.relationships').reputation_tiers; local saved = t[5].max; \
             t[5].max = 99; local issues = table.concat((select(2, require('data.relationships').validate_all())) or {}, '\\n'); \
             t[5].max = saved; return issues",
            "must end at max reputation",
        ),
        (
            "折扣倒挂",
            "local t = require('data.relationships').reputation_tiers; local saved = t[4].effects.price_off; \
             t[4].effects.price_off = 3; local issues = table.concat((select(2, require('data.relationships').validate_all())) or {}, '\\n'); \
             t[4].effects.price_off = saved; return issues",
            "non-decreasing",
        ),
        (
            "未知效果键",
            "local t = require('data.relationships').reputation_tiers; \
             t[2].effects.gift_off = 1; local issues = table.concat((select(2, require('data.relationships').validate_all())) or {}, '\\n'); \
             t[2].effects.gift_off = nil; return issues",
            "unknown effect key",
        ),
        (
            "NPC 引用未知阵营",
            "local npc = require('data.npcs').npcs[1]; local saved = npc.faction; npc.faction = 'heaven_bank'; \
             local issues = table.concat((select(2, require('data.relationships').validate_all())) or {}, '\\n'); \
             npc.faction = saved; return issues",
            "unknown faction",
        ),
        (
            "档位下限写成正数",
            "local t = require('data.relationships').reputation_tiers; local saved = t[1].min; \
             t[1].min = 1; local issues = table.concat((select(2, require('data.relationships').validate_all())) or {}, '\\n'); \
             t[1].min = saved; return issues",
            "must start at 0",
        ),
    ];
    for (label, script, needle) in negative_cases {
        let report = query_str(&mut rt, &format!("(function() {script} end)()"))?;
        if !report.contains(needle) {
            return Err(format!(
                "负例「{label}」没被 validate_all 报出（期望含 {needle:?}）:\n{report}"
            )
            .into());
        }
        println!("  负例 {label}: 报出 {needle}");
    }
    // 任务与对话里的档位名同样是引用完整性的一部分。
    let quest_bad = query_str(
        &mut rt,
        "(function() local q = require('data.quests.ending_field'); \
           local objective = q.objectives[3]; local saved_tier = objective.tier; local saved_count = objective.count; \
           objective.tier = 'legend'; objective.count = 3; \
           local issues = table.concat((select(2, require('world.quest').validate_all())) or {}, '\\n'); \
           objective.tier = saved_tier; objective.count = saved_count; return issues end)()",
    )?;
    if !quest_bad.contains("未知声望档位") || !quest_bad.contains("不接受 count") {
        return Err(format!(
            "写错声望档位名/多写 count 未被 quest.validate_all 报出:\n{quest_bad}"
        )
        .into());
    }
    let dialogue_bad = query_str(
        &mut rt,
        "(function() local d = require('data.dialogues.lu_guizang_truth'); \
           local options = d.nodes.greet.options; local option = nil; \
           for _, e in ipairs(options) do if e.cond and e.cond.tier_gte then option = e end end; \
           if not option then return 'missing:cond' end; \
           local saved = option.cond.tier_gte; local saved_npc = option.cond.tier_npc; \
           option.cond.tier_gte = 'bestie'; option.cond.tier_npc = nil; \
           local issues = table.concat((select(2, require('world.dialogue').validate_all())) or {}, '\\n'); \
           option.cond.tier_gte = saved; option.cond.tier_npc = saved_npc; return issues end)()",
    )?;
    if !dialogue_bad.contains("未知关系档位") || !dialogue_bad.contains("必须成对出现")
    {
        return Err(format!(
            "写错关系档位名/落单 tier_gte 未被 dialogue.validate_all 报出:\n{dialogue_bad}"
        )
        .into());
    }
    println!("  负例 任务/对话档位名: 报出 未知声望档位 + 未知关系档位 + 成对出现");
    assert_str(
        &mut rt,
        "require('validate_data').check({ strict = true })",
        "",
    )?;

    // ---------------- ② 商店购买折扣逐档 ----------------
    banner("M34-01② 店主阵营声望档位 → 购买价折扣（heal_pill 标价 60：60/57/55/53/51）");
    lua(&mut rt, "require('core.state').money = 2000")?;
    let base_price = query_num(
        &mut rt,
        "(function() for _, e in ipairs(require('systems.shop').stock('zhao_stall')) do \
           if e.item == 'heal_pill' then return e.price end end return -1 end)()",
    )?;
    assert_num(
        &mut rt,
        "require('data.shops').zhao_stall.stock[3].price",
        base_price,
    )?;
    // 边界值：档位下限处立刻变价，下界前一格保持原价。
    let ladder: [(f64, f64, f64); 10] = [
        (0.0, 0.0, 60.0),
        (19.0, 0.0, 60.0),
        (20.0, 5.0, 57.0),
        (39.0, 5.0, 57.0),
        (40.0, 8.0, 55.0),
        (59.0, 8.0, 55.0),
        (60.0, 12.0, 53.0),
        (79.0, 12.0, 53.0),
        (80.0, 15.0, 51.0),
        (100.0, 15.0, 51.0),
    ];
    let mut reputation = 0.0;
    for (target, expected_off, expected_price) in ladder {
        reputation = set_reputation(&mut rt, "village", target, reputation)?;
        assert_num(
            &mut rt,
            "require('systems.shop').price_off('zhao_stall')",
            expected_off,
        )?;
        let price = query_num(
            &mut rt,
            "(function() for _, e in ipairs(require('systems.shop').stock('zhao_stall')) do \
               if e.item == 'heal_pill' then return e.price end end return -1 end)()",
        )?;
        if (price - expected_price).abs() > 1e-6 {
            return Err(format!(
                "云泽村声望 {target:.0} 时 heal_pill 成交价 {price}，期望 {expected_price}"
            )
            .into());
        }
        let tier_name = query_str(
            &mut rt,
            "return require('player.relationship').reputation_tier('village').name",
        )?;
        println!(
            "  声望 {target:.0} → {tier_name} 档，折扣 {expected_off:.0}%，heal_pill {price:.0}"
        );
    }
    // 负声望按第一档算：没有正向效果，也不是惩罚。
    reputation = set_reputation(&mut rt, "village", -60.0, reputation)?;
    assert_num(
        &mut rt,
        "require('systems.shop').price_off('zhao_stall')",
        0.0,
    )?;
    assert_str(
        &mut rt,
        "require('player.relationship').reputation_tier('village').name",
        "陌生",
    )?;
    println!("  声望 -60 也落陌生档（原价，无惩罚）");
    // 折扣只作用在购买价：出售价仍取物品 sell。
    reputation = set_reputation(&mut rt, "village", 80.0, reputation)?;
    let heal_pill_sell = query_num(&mut rt, "require('data.items').heal_pill.sell")?;
    let rice_sell = query_num(&mut rt, "require('data.items').rice.sell")?;
    let offered_rice = query_num(
        &mut rt,
        "(function() for _, e in ipairs(require('systems.shop').buys('zhao_stall')) do \
           if e.item == 'rice' then return e.price end end return -1 end)()",
    )?;
    assert_num(&mut rt, "require('data.items').rice.sell", offered_rice)?;
    if rice_sell != offered_rice {
        return Err(format!("出售价被折扣污染：{offered_rice} != sell {rice_sell}").into());
    }
    println!(
        "  核心档（让 15%）下 rice 出售价仍为 sell = {offered_rice:.0}（heal_pill sell = {heal_pill_sell:.0}）"
    );
    // 实付金额 = 折扣价（走 shop.buy 真实入口；货摊营业窗口 480..1080）。
    lua(&mut rt, &format!("clock.set(1, {SHOP_HOUR})"))?;
    frames(&mut rt, 2)?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    assert_str(
        &mut rt,
        "select(2, require('systems.shop').buy('zhao_stall', 'heal_pill', 1))",
        "ok",
    )?;
    assert_num(&mut rt, "require('core.state').money", money_before - 51.0)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "buy_heal_pill",
    )?;
    // 无阵营/未知店铺的折扣读数为 0（`keeper_price_off` 的两条兜底）。
    assert_num(
        &mut rt,
        "require('systems.shop').price_off('no_such_shop')",
        0.0,
    )?;
    assert_str(
        &mut rt,
        "tostring(require('player.relationship').faction_of('npc_situ_kui'))",
        "nil",
    )?;

    // ---------------- ③ 结局交付的声望门槛 ----------------
    banner("M34-01③ 结局·归田：不到档没有交付选项，到档即放行（真实对话交付）");
    reputation = set_reputation(&mut rt, "village", 0.0, reputation)?;
    goto_map(&mut rt, "village", VILLAGE_POS)?;
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    // 主线五段的锚点由 quest.on_complete 写入，这里按同一入口写锚点（本样例不重跑主线）。
    lua(
        &mut rt,
        "require('core.state').set_var('mainline_truth_done', true, 'test', 'gate')",
    )?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    choose_option(&mut rt, "沈掌柜，我想回田里种药。")?;
    choose_option(&mut rt, "便依掌柜所言。")?;
    choose_option(&mut rt, "记下了。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('ending_field')",
        "in_progress",
    )?;
    lua(
        &mut rt,
        "inv.add(require('core.state').inventory, 'rice', 20)",
    )?;
    // 稻谷已够、真相已明，只差声望：状态仍必须是 in_progress（门槛真的在挡）。
    assert_str(
        &mut rt,
        "require('world.quest').state('ending_field')",
        "in_progress",
    )?;
    assert_num(
        &mut rt,
        "require('world.quest').objectives_met('ending_field') and 1 or 0",
        0.0,
    )?;
    // 任务日志的进度行：声望目标显示 1/1，与 flag 目标同一口径（不是分数）。
    let log_line = query_str(
        &mut rt,
        "(function() local rows = require('world.quest').progress('ending_field'); \
           for _, row in ipairs(rows) do \
             if row.id == 'village_welcome' then \
               return row.label .. '|' .. row.current .. '/' .. row.target .. '|' .. tostring(row.done) \
             end \
           end; return 'missing' end)()",
    )?;
    if !log_line.starts_with("云泽村声望到熟识|0/1|false") {
        return Err(format!("声望门槛的进度行不对: {log_line}").into());
    }
    println!("  任务日志：{log_line}");
    talk_to(&mut rt, "npc_shen_qingluo")?;
    assert_str(&mut rt, &option_visible("二十份稻谷已入囤。"), "false")?;
    choose_option(&mut rt, "稻谷还在晒。")?;
    choose_option(&mut rt, "好。")?;
    // 19 分仍然挡住，20 分放行：档位下限就是交付下限。
    reputation = set_reputation(&mut rt, "village", 19.0, reputation)?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    assert_str(&mut rt, &option_visible("二十份稻谷已入囤。"), "false")?;
    choose_option(&mut rt, "改日再说。")?;
    // 这一章之后不再改声望，返回值无需接住（门槛放行只读数据侧的分数）。
    set_reputation(&mut rt, "village", 20.0, reputation)?;
    assert_str(
        &mut rt,
        "require('world.quest').state('ending_field')",
        "deliverable",
    )?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    talk_to(&mut rt, "npc_shen_qingluo")?;
    assert_str(&mut rt, &option_visible("二十份稻谷已入囤。"), "true")?;
    choose_option(&mut rt, "二十份稻谷已入囤。")?;
    choose_option(&mut rt, "多谢掌柜。")?;
    assert_str(
        &mut rt,
        "require('world.quest').state('ending_field')",
        "completed",
    )?;
    assert_str(&mut rt, "require('core.state').get_var('ending')", "field")?;
    assert_num(&mut rt, "require('core.state').money", money_before + 500.0)?;
    // 交付奖励里的 village +30 也入账（20 → 50，落受信档）。
    assert_num(
        &mut rt,
        "require('player.relationship').reputation('village')",
        50.0,
    )?;
    assert_num(
        &mut rt,
        "require('systems.shop').price_off('zhao_stall')",
        8.0,
    )?;
    // 保底第四条路：断念不挂声望门槛，零门派声望也能交付。
    assert_num(
        &mut rt,
        "require('player.relationship').reputation('sect')",
        0.0,
    )?;
    goto_map(&mut rt, "manor", MANOR_POS)?;
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_gu_changfeng")?;
    choose_option(&mut rt, "弟子想放下这一切。")?;
    choose_option(&mut rt, "弟子遵命。")?;
    lua(
        &mut rt,
        "inv.add(require('core.state').inventory, 'herb_tonic', 5)",
    )?;
    assert_str(
        &mut rt,
        "require('world.quest').state('ending_renounce')",
        "deliverable",
    )?;
    println!("  断念无门槛：门派声望 0 也能交付（ending_renounce = deliverable）");

    // ---------------- ④ 对话的关系档位分支 ----------------
    banner("M34-01④ 体己话分支：陌生看不见，真实送礼到「友善」才出现，且只说一次");
    goto_map(&mut rt, "village", VILLAGE_POS)?;
    lua(&mut rt, &format!("clock.set(1, {SHOP_HOUR})"))?;
    frames(&mut rt, 2)?;
    // 茶由阿七食摊买（每日限购 5，分两天）——真实购买入口，顺便验证折扣不碰限购。
    assert_str(
        &mut rt,
        "select(2, require('systems.shop').buy('a_qi_provisions', 'tea_brew', 5))",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "select(2, require('systems.shop').buy('a_qi_provisions', 'tea_brew', 1))",
        "limit",
    )?;
    lua(&mut rt, "day.sleep()")?;
    assert_num(&mut rt, "clock.now().day", 2.0)?;
    // 睡到清晨（< 07:00）会落在食摊营业窗口外，把时刻钉回上午再买第二批。
    lua(&mut rt, &format!("clock.set(2, {SHOP_HOUR})"))?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        "select(2, require('systems.shop').buy('a_qi_provisions', 'tea_brew', 4))",
        "ok",
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'tea_brew')",
        9.0,
    )?;
    let relationship = "require('player.relationship')";
    assert_str(
        &mut rt,
        &format!("select(2, {relationship}.gift('npc_zhao_tiesuanpan', 'rice'))"),
        "not_giftable",
    )?;
    assert_str(
        &mut rt,
        &format!("select(2, {relationship}.gift('npc_lu_guizang', 'tea_brew'))"),
        "5",
    )?;
    assert_str(
        &mut rt,
        &format!("select(2, {relationship}.gift('npc_lu_guizang', 'tea_brew'))"),
        "gift_today",
    )?;
    assert_num(
        &mut rt,
        &format!("{relationship}.points('npc_lu_guizang')"),
        5.0,
    )?;
    assert_str(
        &mut rt,
        &format!("tostring({relationship}.affinity_met('npc_lu_guizang', 'friendly'))"),
        "false",
    )?;
    // 再送 7 次到 40 分（相识档上界仍在友善下限之下），先证明分支还不肯露出来。
    for gift_index in 2..=8 {
        lua(&mut rt, "day.sleep()")?;
        assert_num(&mut rt, "clock.now().day", gift_index as f64 + 1.0)?;
        let expected = format!("{}", gift_index * 5);
        assert_str(
            &mut rt,
            &format!("select(2, {relationship}.gift('npc_lu_guizang', 'tea_brew'))"),
            &expected,
        )?;
    }
    assert_num(
        &mut rt,
        &format!("{relationship}.points('npc_lu_guizang')"),
        40.0,
    )?;
    goto_map(&mut rt, "manor", MANOR_POS)?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_lu_guizang")?;
    assert_str(&mut rt, &option_visible(SECRET_OPTION), "false")?;
    choose_option(&mut rt, "改日再来请教。")?;
    println!("  关系分 40（相识档）：体己话选项仍不出现");
    // 第 9 次送礼到 45 = 友善档下限，分支即放行。
    lua(&mut rt, "day.sleep()")?;
    assert_str(
        &mut rt,
        &format!("select(2, {relationship}.gift('npc_lu_guizang', 'tea_brew'))"),
        "45",
    )?;
    println!("  连送 9 日：陆归藏关系分 = 45（友善档下限）");
    assert_str(
        &mut rt,
        &format!("tostring({relationship}.affinity_met('npc_lu_guizang', 'friendly'))"),
        "true",
    )?;
    goto_map(&mut rt, "manor", MANOR_POS)?;
    frames(&mut rt, 2)?;
    talk_to(&mut rt, "npc_lu_guizang")?;
    assert_str(&mut rt, &option_visible(SECRET_OPTION), "true")?;
    choose_option(&mut rt, SECRET_OPTION)?;
    choose_option(&mut rt, "徒儿收好。")?;
    choose_option(&mut rt, "谢师叔。")?;
    assert_str(
        &mut rt,
        "require('core.state').get_var('lu_guizang_secret')",
        "true",
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'herb_tonic')",
        6.0,
    )?;
    // 锚点之后不再重复（falsy 条件把分支收掉）。
    talk_to(&mut rt, "npc_lu_guizang")?;
    assert_str(&mut rt, &option_visible(SECRET_OPTION), "false")?;
    choose_option(&mut rt, "改日再来请教。")?;
    println!("  分支只说一次：锚点 lu_guizang_secret 已写入，选项收回");

    // ---------------- ⑤ 存档往返 ----------------
    banner("M34-01⑤ 档位是派生值：分数随存档回来，折扣/门槛/分支结论一起回来");
    lua(&mut rt, "require('core.save_manager').save()")?;
    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_num(
        &mut second,
        "require('player.relationship').reputation('village')",
        50.0,
    )?;
    assert_num(
        &mut second,
        "require('systems.shop').price_off('zhao_stall')",
        8.0,
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').points('npc_lu_guizang')",
        45.0,
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('lu_guizang_secret')",
        "true",
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('ending')",
        "field",
    )?;
    assert_str(
        &mut second,
        "require('world.quest').state('ending_field')",
        "completed",
    )?;
    let restored_price = query_num(
        &mut second,
        "(function() for _, e in ipairs(require('systems.shop').stock('zhao_stall')) do \
           if e.item == 'heal_pill' then return e.price end end return -1 end)()",
    )?;
    assert_num(&mut second, "require('data.items').heal_pill.buy", 60.0)?;
    if (restored_price - 55.0).abs() > 1e-6 {
        return Err(
            format!("读档后 heal_pill 成交价 {restored_price}，期望 55（受信档 8%）").into(),
        );
    }
    println!("  读档：云泽村 50 → 受信 8% → heal_pill {restored_price:.0}（原价 60）");

    println!(
        "jhyt_m34_relationship_tier_ok project={} tiers=5 reputation_tiers=5 ladder=60/57/55/53/51 \
         ending_field=completed ending_renounce=deliverable affinity=45 secret=true price_restored={:.0}",
        project.display(),
        restored_price
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- 档位驱动 ----------

/// 把某阵营声望调到 `target`（走 `add_reputation` 真实入口，只给增量）。
/// `current` 是调用方手里的读数基准，用来算增量；返回新的基准。
fn set_reputation(
    runtime: &mut LuaRuntime,
    faction: &str,
    target: f64,
    current: f64,
) -> Result<f64, Box<dyn std::error::Error>> {
    let delta = target - current;
    if delta.abs() > 1e-9 {
        lua(
            runtime,
            &format!(
                "require('player.relationship').add_reputation('{faction}', {delta}, 'test', 'tier')"
            ),
        )?;
    }
    let got = query_num(
        runtime,
        &format!("require('player.relationship').reputation('{faction}')"),
    )?;
    if (got - target).abs() > 1e-6 {
        return Err(format!("{faction} 声望调到 {target} 失败，实际 {got}").into());
    }
    Ok(target)
}

/// 当前对话里是否看得见某个选项文本。
fn option_visible(text: &str) -> String {
    format!(
        "(function() for _, e in ipairs(require('world.dialogue').options()) do \
           if e.text == [[{text}]] then return true end end return false end)()"
    )
}

// ---------- helpers（与 jhyt_m27_goals 同一套） ----------

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
