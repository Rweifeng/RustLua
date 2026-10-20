//! Headless acceptance drive for M34-05：茶棚茶饮与社交收益
//! （`data/buildings.lua` 的 tea station 三档 + `data/recipes.lua` 的冲山泉茶 +
//! `player/relationship.lua` 的当日社交乘子）。
//!
//! 为什么要这一门：M34-05 的三格收益（1 级出茶饮 / 2 级社交 +10% / 3 级解锁春分茶会）
//! 全是**数字**，写错不会崩，只会让加成悄悄失效或者每一天都失效。所以本样例把四件事钉死：
//!
//! 1. 配方跟着设施走：茶饮改挂 `station = "tea"` 之后，**膳房必须拒收**（`no_station`），
//!    而且理由不能顶包——`no_station` / `no_building` / `station_level` 三种理由各钉一格
//!    （HUD 的中文按理由查，念错就等于把"你没盖茶棚"说成"你没材料"）；
//! 2. 跨天进位是真的：23:50 开工、20 分钟，完成时刻落在**第二天** 00:10；
//!    没到点收不到、到了点一次收走，茶叶 2 → 茶汤 1 的消耗/产出按实数断言；
//! 3. 社交乘子是**当日**的：茶棚 2 级 +10%、当天喝过茶再 +10%（合计 ×1.2），
//!    第二杯 `tea_today` 拒收且不扣物；跨天后饮茶那半边自动失效（锚点记日期，无重置钩子）；
//! 4. 三样东西存读档一致：茶汤库存、`tea_day` 锚点、`building_level_tea_shed` 历史最高等级
//!    （拆了不回收，与节气解锁同一口径）。
//!
//! 另有 7 条负例（station 拼错、`social_bonus` 越界、等级行未知键、`unlock.level` 超上限、
//! `unlock.building` 未知、`use.social_day` 非 true、`social` 未知键），全部走
//! `validate_data.check()` ——与 `cargo run -p game -- --check` 同一批校验器。
//!
//! Usage:
//!   cargo run -q -p game --example jhyt_m34_teahouse [-- <game_jhyt-root>]

use rustlua_scripting::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 山庄出生点（建筑与制作都在山庄）。
const MANOR_POS: (f32, f32) = (528.0, 48.0);
/// 10:00。
const TEN_AM: i64 = 600;
/// 23:50：茶饮 20 分钟从这里跨到次日 00:10。
const LATE_NIGHT: i64 = 1430;
/// 茶棚配方 id（`data/recipes.lua`，M34-05 从膳房改挂到茶棚）。
const TEA_RECIPE: &str = "tea_spring_brew";
/// 每杯茶的内力回复量（`data/items.lua` 的 tea_brew.use.inner_power）。
const TEA_INNER_POWER: f64 = 15.0;
/// 内力上限（走 `player/inventory.lua` 的只读访问口，不写死 100）。
const INNER_CAP: &str = "require('player.inventory').max_inner_power()";
/// 饮茶前把内力压到这一格：跨天会把它拉回上限，撞上上限就测不出 +15。
const INNER_PROBE: f64 = 50.0;

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
    let save_path = fresh_save_path(&project, "jhyt_m34_teahouse.sav");
    let project_arg = project.clone();

    let mut rt = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;

    // ---------------- ① 配方跟着设施走：膳房拒收茶饮 ----------------
    banner("M34-05① 茶饮改挂茶棚之后：膳房 no_station，三种拒绝理由各钉一格");
    assert_str(
        &mut rt,
        "require('validate_data').check({ strict = true })",
        "",
    )?;
    assert_str(
        &mut rt,
        "require('data.buildings').get('tea_shed').station",
        "tea",
    )?;
    assert_str(
        &mut rt,
        &format!("require('data.recipes').get('{TEA_RECIPE}').station"),
        "tea",
    )?;
    assert_str(
        &mut rt,
        "require('data.recipes').for_station('tea')[1]",
        TEA_RECIPE,
    )?;
    // 膳房的配方清单里不能再有它（留两条同产出无差异的配方 = 第二条数据源）。
    assert_str(
        &mut rt,
        "(function() for _, id in ipairs(require('data.recipes').for_station('kitchen')) do \
           if id == 'tea_spring_brew' then return 'still_there' end end return 'gone' end)()",
        "gone",
    )?;
    goto_map(&mut rt, "manor", MANOR_POS)?;
    lua(&mut rt, &format!("clock.set(5, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    let (kx, ky) = place(&mut rt, "kitchen")?;
    let kitchen = format!("{kx},{ky}");
    assert_str(
        &mut rt,
        &format!("select(2, require('systems.crafting').start('{TEA_RECIPE}', [[{kitchen}]]))"),
        "no_station",
    )?;
    println!("  膳房 key={kitchen}：start('tea_spring_brew') → no_station（配方不认这栋建筑）");
    assert_str(
        &mut rt,
        "require('ui.hud').feedback('no_station')",
        "没有对应设施",
    )?;
    assert_str(
        &mut rt,
        "require('ui.hud').feedback('station_level')",
        "设施等级不足",
    )?;
    // 同一入口的另两格理由：建筑 key 不存在 → no_building；膳房 1 级炖不了 2 级汤 → station_level。
    assert_str(
        &mut rt,
        &format!("select(2, require('systems.crafting').start('{TEA_RECIPE}', [[99,99]]))"),
        "no_building",
    )?;
    assert_str(
        &mut rt,
        &format!(
            "select(2, require('systems.crafting').start('kitchen_herb_soup', [[{kitchen}]]))"
        ),
        "station_level",
    )?;
    println!(
        "  理由枚举钉死：no_building（无此建筑）/ station_level（膳房 1 级 < 配方 min_level 2）"
    );
    // 茶棚还没盖：等级 var 不存在，乘子 = 1（没有任何来源在偷偷加分）。
    assert_str(
        &mut rt,
        "tostring(require('core.state').get_var('building_level_tea_shed'))",
        "nil",
    )?;
    assert_num(
        &mut rt,
        "require('player.relationship').social_multiplier()",
        1.0,
    )?;
    assert_str(
        &mut rt,
        "tostring(require('player.relationship').tea_today())",
        "false",
    )?;

    // ---------------- ② 建茶棚 → 真实出茶，完成时刻跨天进位 ----------------
    banner("M34-05② 23:50 开工 20 分钟：完成时刻落在次日 00:10，未到点收不到");
    let (tx, ty) = place(&mut rt, "tea_shed")?;
    let shed = format!("{tx},{ty}");
    assert_str(
        &mut rt,
        "require('core.state').get_var('building_level_tea_shed')",
        "1",
    )?;
    // 1 级只解锁茶饮，不给社交加成（docs/33 §7.2 的三档各有各的格子）。
    assert_num(
        &mut rt,
        "require('player.relationship').social_multiplier()",
        1.0,
    )?;
    // 茶叶要先在手边：available_recipe 连"买不买得起"一起判（没料就返回 not_enough）。
    lua(
        &mut rt,
        "inv.add(require('core.state').inventory, 'tea_leaf', 4)",
    )?;
    assert_str(
        &mut rt,
        &format!("require('systems.crafting').available_recipe([[{shed}]])"),
        TEA_RECIPE,
    )?;
    let leaf_before = query_num(&mut rt, TEA_LEAF)?;
    let brew_before = query_num(&mut rt, TEA_BREW)?;
    lua(&mut rt, &format!("clock.set(5, {LATE_NIGHT})"))?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        &format!("select(1, require('systems.crafting').start('{TEA_RECIPE}', [[{shed}]]))"),
        "true",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        &format!("craft_start_{TEA_RECIPE}"),
    )?;
    assert_num(&mut rt, TEA_LEAF, leaf_before - 2.0)?;
    println!(
        "  开工即扣茶叶 2（{leaf_before:.0} → {:.0}），产出要等 20 游戏分钟",
        leaf_before - 2.0
    );
    assert_num(
        &mut rt,
        &format!("require('systems.crafting').query([[{shed}]]).remaining"),
        20.0,
    )?;
    // 同一分钟立刻收：还在做 → `false, {"crafting", 剩余分钟}`，背包没动。
    assert_str(&mut rt, &collect_probe(&shed), "false,crafting,20")?;
    assert_num(&mut rt, TEA_BREW, brew_before)?;
    // 次日 00:05：还差 5 分钟——20 分钟的活儿不会被"换个日子"提前。
    lua(&mut rt, "clock.set(6, 5)")?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, &collect_probe(&shed), "false,crafting,5")?;
    assert_num(
        &mut rt,
        &format!("require('systems.crafting').query([[{shed}]]).remaining"),
        5.0,
    )?;
    // 次日 00:10：到点，一次收走 1 杯。
    lua(&mut rt, "clock.set(6, 10)")?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, &collect_probe(&shed), "true,tea_brew,-")?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "craft_collect_tea_brew",
    )?;
    assert_num(&mut rt, TEA_BREW, brew_before + 1.0)?;
    assert_str(
        &mut rt,
        &format!("tostring(require('systems.crafting').has_job([[{shed}]]))"),
        "false",
    )?;
    assert_str(
        &mut rt,
        &format!("tostring(require('systems.crafting').query([[{shed}]]).ready)"),
        "nil",
    )?;
    println!("  春 5 23:50 开工 → 春 6 00:05 仍差 5 分钟 → 00:10 收到茶汤 1 杯（跨天进位）");

    // ---------------- ③ 饮茶与当日社交乘子 ----------------
    banner("M34-05③ 平日 ×1 → 茶棚 2 级 ×1.1 → 当日饮茶 ×1.2；第二杯 tea_today 不扣物");
    lua(&mut rt, &format!("clock.set(7, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        "tostring(require('player.relationship').tea_today())",
        "false",
    )?;
    // 基线：茶棚还是 1 级，送礼拿的是表里的原始分值（8 点）。
    give(&mut rt, "lingzhi", 3)?;
    assert_str(
        &mut rt,
        "select(1, require('player.relationship').gift('npc_shen_qingluo', 'lingzhi'))",
        "true",
    )?;
    assert_num(
        &mut rt,
        "require('player.relationship').points('npc_shen_qingluo')",
        8.0,
    )?;
    println!("  平日无加成：沈青萝收灵芝 = 表里 8 点（乘子 1.0）");
    // 第二档：2 级茶棚给社交 +10%（docs/33 §7.2），乘子只作用在**正增量**上。
    give(&mut rt, "timber", 3)?;
    upgrade(&mut rt, tx, ty, 2)?;
    assert_num(
        &mut rt,
        "require('player.relationship').social_multiplier()",
        1.1,
    )?;
    let brew_in_hand = query_num(&mut rt, TEA_BREW)?;
    assert_str(
        &mut rt,
        "select(1, require('player.relationship').gift('npc_lu_guizang', 'tea_brew'))",
        "true",
    )?;
    assert_num(&mut rt, TEA_BREW, brew_in_hand - 1.0)?;
    // 5 × 1.1 = 5.5 → 6：取整用四舍五入，向下取整会让这条加成在 5 分礼上完全看不见。
    assert_num(
        &mut rt,
        "require('player.relationship').points('npc_lu_guizang')",
        6.0,
    )?;
    println!("  茶棚 2 级：陆归藏收山泉清茶 5 点 → 6 点（×1.1 后四舍五入）");
    // 第三档：今天喝一杯再 +10%（合计 ×1.2）。
    give(&mut rt, "tea_brew", 2)?;
    give(&mut rt, "heal_pill", 2)?;
    let before_drink = query_num(&mut rt, TEA_BREW)?;
    // 先把内力压到离上限有距离的一格：跨天会把内力拉回上限，撞上界就测不出 +15。
    assert_str(
        &mut rt,
        &format!("tostring({INNER_PROBE} + {TEA_INNER_POWER} <= {INNER_CAP})"),
        "true",
    )?;
    lua(
        &mut rt,
        &format!("require('core.state').inner_power = {INNER_PROBE}"),
    )?;
    assert_str(
        &mut rt,
        "select(1, require('player.inventory').use('tea_brew'))",
        "true",
    )?;
    assert_str(&mut rt, "require('core.state').last_action", "use_tea_brew")?;
    assert_num(&mut rt, TEA_BREW, before_drink - 1.0)?;
    assert_str(
        &mut rt,
        "tostring(require('player.relationship').tea_today())",
        "true",
    )?;
    assert_str(&mut rt, "require('core.state').get_var('tea_day')", "7")?;
    assert_num(
        &mut rt,
        "require('player.relationship').social_multiplier()",
        1.2,
    )?;
    assert_num(
        &mut rt,
        "require('core.state').inner_power",
        INNER_PROBE + TEA_INNER_POWER,
    )?;
    // ×1.2 落在 8 点礼上 = 9.6 → 10（docs/33 §8.1 的期望读数）。
    assert_str(
        &mut rt,
        "select(1, require('player.relationship').gift('npc_gu_changfeng', 'heal_pill'))",
        "true",
    )?;
    assert_num(
        &mut rt,
        "require('player.relationship').points('npc_gu_changfeng')",
        10.0,
    )?;
    println!("  饮茶当日：×1.2 生效，顾长收复气丹 8 点 → 10 点、内力 +15");
    // 第二杯：当日限次，**不扣物品**也不重复加分（与"同一 NPC 每日一次礼物"同族语义）。
    let kept = query_num(&mut rt, TEA_BREW)?;
    assert_str(
        &mut rt,
        "select(2, require('player.inventory').use('tea_brew'))",
        "tea_today",
    )?;
    assert_str(&mut rt, "require('core.state').last_action", "tea_today")?;
    assert_str(
        &mut rt,
        "require('ui.hud').feedback('tea_today')",
        "今日已饮过茶",
    )?;
    assert_num(&mut rt, TEA_BREW, kept)?;
    assert_num(
        &mut rt,
        "require('core.state').inner_power",
        INNER_PROBE + TEA_INNER_POWER,
    )?;
    println!("  第二杯：reason=tea_today，茶汤仍 {kept:.0} 杯、内力没变（HUD「今日已饮过茶」）");
    // 每日限次没有被乘子绕开：同日同 NPC 再送仍是 gift_today，分数不再涨。
    assert_str(
        &mut rt,
        "select(2, require('player.relationship').gift('npc_gu_changfeng', 'heal_pill'))",
        "gift_today",
    )?;
    assert_num(
        &mut rt,
        "require('player.relationship').points('npc_gu_changfeng')",
        10.0,
    )?;

    // ---------------- ④ 存读档：三样东西一起回来 ----------------
    banner("M34-05④ 存档往返：茶汤库存 / tea_day 锚点 / 茶棚历史最高等级三者一致");
    let tea_brew_saved = query_num(&mut rt, TEA_BREW)?;
    lua(&mut rt, "require('core.save_manager').save()")?;
    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_scene_is(&mut second, "manor")?;
    assert_num(&mut second, "clock.now().day", 7.0)?;
    assert_num(&mut second, TEA_BREW, tea_brew_saved)?;
    assert_str(&mut second, "require('core.state').get_var('tea_day')", "7")?;
    assert_str(
        &mut second,
        "require('core.state').get_var('building_level_tea_shed')",
        "2",
    )?;
    assert_str(
        &mut second,
        "tostring(require('player.relationship').tea_today())",
        "true",
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').social_multiplier()",
        1.2,
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').points('npc_gu_changfeng')",
        10.0,
    )?;
    // 建筑记录也要回来：等级 var 记的是"建到过几级"，记录才是"现在杵在哪、几级"。
    assert_str(
        &mut second,
        &format!("tostring(require('systems.building').at({tx}, {ty}).level)"),
        "2",
    )?;
    assert_str(
        &mut second,
        &format!("require('systems.crafting').query([[{shed}]]).station"),
        "tea",
    )?;
    assert_str(
        &mut second,
        &format!("tostring(require('systems.crafting').has_job([[{shed}]]))"),
        "false",
    )?;
    println!(
        "  读档：茶汤 {tea_brew_saved:.0} 杯 / tea_day=7 / 等级 var=2（记录 level=2）/ 乘子仍 1.2"
    );

    // ---------------- ⑤ 第三档：解锁春分茶会（与 M34-02 的门一致） ----------------
    banner("M34-05⑤ 茶棚 2 级 → 3 级：春分茶会从听不到变成可用；拆了不再锁回去");
    lua(&mut second, "clock.set(9, 600)")?;
    frames(&mut second, 2)?;
    assert_str(&mut second, "require('world.festival').id()", "teafair")?;
    // 跨天：饮茶那半边自动失效（锚点记的是日期，没有每日重置钩子要挂）。
    assert_str(
        &mut second,
        "tostring(require('player.relationship').tea_today())",
        "false",
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').social_multiplier()",
        1.1,
    )?;
    assert_str(
        &mut second,
        "tostring(require('world.festival').available('teafair'))",
        "false",
    )?;
    println!("  春 9、茶棚 2 级：日历仍是 teafair，available=false（第三档还没到）");
    give(&mut second, "timber", 3)?;
    upgrade(&mut second, tx, ty, 3)?;
    assert_str(
        &mut second,
        "tostring(require('world.festival').available('teafair'))",
        "true",
    )?;
    // 加成是**累加各档**的：3 级那行没写 social_bonus，所以乘子仍是 1.1 而不是 1.2。
    assert_num(
        &mut second,
        "require('data.buildings').social_bonus_at(require('data.buildings').get('tea_shed'), 3)",
        0.1,
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').social_multiplier()",
        1.1,
    )?;
    println!("  升到 3 级：available=true；只有 2 级那行写了 +0.1 → 乘子仍是 1.1（不重复计）");
    // 拆了不回收：等级 var 取历史最高，事件门不会随建筑消失而重新锁上。
    assert_str(
        &mut second,
        &format!("select(1, require('systems.building').remove({tx}, {ty}))"),
        "true",
    )?;
    assert_str(
        &mut second,
        "require('core.state').last_action",
        "demolish_tea_shed",
    )?;
    assert_str(
        &mut second,
        &format!("tostring(require('systems.building').at({tx}, {ty}))"),
        "nil",
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('building_level_tea_shed')",
        "3",
    )?;
    assert_str(
        &mut second,
        "tostring(require('world.festival').available('teafair'))",
        "true",
    )?;
    assert_num(
        &mut second,
        "require('player.relationship').social_multiplier()",
        1.1,
    )?;
    println!("  拆掉茶棚：格子空了，但 var 仍是 3 / available 仍 true / 乘子仍 1.1（拆了不回收）");

    // ---------------- ⑥ 负例 ----------------
    banner("M34-05⑥ 七条负例：新数据写错必须被 validate_data 报出（与 game --check 同一批校验器）");
    for (label, expr, needle) in NEGATIVE_CASES {
        let issues = query_str(&mut second, expr)?;
        if !issues.contains(needle) {
            return Err(format!("负例「{label}」未被报出，得到:\n{issues}").into());
        }
        println!("  负例 {label}: 报出 {needle:?}");
    }
    assert_str(
        &mut second,
        "require('validate_data').check({ strict = true })",
        "",
    )?;
    println!("  七条负例跑完，登记表再查一遍仍然干净");

    println!(
        "jhyt_m34_teahouse_ok project={} tea_station=tea kitchen_reject=no_station \
         reasons=no_station/no_building/station_level brew=2leaf->1brew@20min cross_day=00:10 \
         multiplier=1/1.1/1.2 gift_8=8_then_10 second_cup=tea_today tea_brew_kept={:.0} \
         save_roundtrip=brew+tea_day+level_var demolish_keeps_door=true negatives=7",
        project.display(),
        tea_brew_saved
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- 背包查询表达式（同一处只写一遍，避免串里数字飘） ----------

const TEA_LEAF: &str = "inv.count(require('core.state').inventory, 'tea_leaf')";
const TEA_BREW: &str = "inv.count(require('core.state').inventory, 'tea_brew')";

/// 收一次并把结果压成 `ok,产出或reason[1],reason[2]` 一行（拒绝路径无副作用，可重复调）。
fn collect_probe(key: &str) -> String {
    format!(
        "(function() local c = require('systems.crafting'); \
           local ok, err = c.collect([[{key}]]); \
           if type(err) == 'table' then \
             return tostring(ok) .. ',' .. tostring(err[1]) .. ',' .. tostring(err[2]) \
           end \
           return tostring(ok) .. ',' .. tostring(err) .. ',-' end)()"
    )
}

// ---------- 负例（每条先改坏数据、跑一遍 game --check 用的校验器、再改回来） ----------

/// `(标题, Lua 表达式, 期望出现在报错文案里的片段)`。
const NEGATIVE_CASES: [(&str, &str, &str); 7] = [
    (
        "配方 station 拼错",
        "(function() local r = require('data.recipes').recipes.tea_spring_brew; \
           local saved = r.station; r.station = 'teaa'; \
           local issues = require('validate_data').check({ strict = true }); \
           r.station = saved; return issues end)()",
        "station: unknown station teaa",
    ),
    (
        "social_bonus 越界",
        "(function() local l = require('data.buildings').buildings.tea_shed.levels[2]; \
           local saved = l.social_bonus; l.social_bonus = 2.5; \
           local issues = require('validate_data').check({ strict = true }); \
           l.social_bonus = saved; return issues end)()",
        "social_bonus: expected number in (0, 1]",
    ),
    (
        "等级行未知键",
        "(function() local b = require('data.buildings').buildings.tea_shed; \
           b.levels[2].tea_bonus = 0.1; \
           local issues = require('validate_data').check({ strict = true }); \
           b.levels[2].tea_bonus = nil; return issues end)()",
        "unknown level key",
    ),
    (
        "事件门槛超出建筑最高等级",
        "(function() local u = require('data.festivals').get('teafair').unlock; \
           local saved = u.level; u.level = 9; \
           local issues = require('validate_data').check({ strict = true }); \
           u.level = saved; return issues end)()",
        "exceeds max level",
    ),
    (
        "事件门槛引用未知建筑",
        "(function() local u = require('data.festivals').get('teafair').unlock; \
           local saved = u.building; u.building = 'tea_shedd'; \
           local issues = require('validate_data').check({ strict = true }); \
           u.building = saved; return issues end)()",
        "unlock.building: unknown building",
    ),
    (
        "use.social_day 写成非 true",
        "(function() local u = require('data.items').tea_brew.use; \
           local saved = u.social_day; u.social_day = 'yes'; \
           local issues = require('validate_data').check({ strict = true }); \
           u.social_day = saved; return issues end)()",
        "social_day: expected true or absent",
    ),
    (
        "social 未知键",
        "(function() local s = require('data.relationships').social; \
           s.hello = 0.1; \
           local issues = require('validate_data').check({ strict = true }); \
           s.hello = nil; return issues end)()",
        "unknown social key",
    ),
];

// ---------- helpers（与 jhyt_m34_festival 同一套） ----------

/// 往背包放材料（升级与送礼都要实数材料，走 `inv.add` 这条真实入口）。
fn give(
    runtime: &mut LuaRuntime,
    item_id: &str,
    count: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    lua(
        runtime,
        &format!("inv.add(require('core.state').inventory, '{item_id}', {count})"),
    )
}

/// 扫描当前地图找第一个可放置锚点并盖下去，返回锚点格。
fn place(
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
    assert_str(
        runtime,
        &format!("select(1, require('systems.building').place('{building_id}', {x}, {y}))"),
        "true",
    )?;
    assert_str(
        runtime,
        "require('core.state').last_action",
        &format!("build_{building_id}"),
    )?;
    Ok((x, y))
}

/// 升一级并确认"记录等级 + 历史最高 var + 音效/提示"三者一起跟上。
fn upgrade(
    runtime: &mut LuaRuntime,
    x: i64,
    y: i64,
    level: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_str(
        runtime,
        &format!("select(1, require('systems.building').upgrade({x}, {y}))"),
        "true",
    )?;
    assert_str(
        runtime,
        &format!("tostring(require('systems.building').at({x}, {y}).level)"),
        &level.to_string(),
    )?;
    assert_str(
        runtime,
        "require('core.state').get_var('building_level_tea_shed')",
        &level.to_string(),
    )?;
    assert_str(
        runtime,
        "require('core.state').last_action",
        &format!("upgrade_tea_shed_{level}"),
    )?;
    Ok(())
}

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())?;
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

/// 通过引擎的地图跳转入口切图。
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
