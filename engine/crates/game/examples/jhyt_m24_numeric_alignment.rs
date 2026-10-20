//! Headless acceptance drive for numeric alignment (M23 / 计划 G5).
//!
//! 把"数值设计案 ↔ Lua 数据 ↔ 实际行为"三者锁成一条断言链，覆盖三类缺陷：
//!
//! 1. **文档与数据各说各话**：`docs/33-江湖有田-数值设计案.md` 的作物表与商店价格表直接
//!    被本示例解析出来逐格比对 `data/crops.lua` / `data/items.lua`——不再靠人工核对。
//! 2. **数据缺字段导致数值静默走兜底**：`systems/farm.lua` 收获时读 `crop.yield`，
//!    但 `data/crops.lua` 长期没有 `yield` 字段 ⇒ `crop.yield or 1` 让**每茬恒收 1**
//!    （稻谷按设计案应为 2，等于少收一半，而且没有任何报错）。现在 `yield` 是必填字段
//!    （`data/registry_check.lua` 强制正整数），并由本示例用**真实耕作链路**断言
//!    "收获数量 == 数据里的 yield"，以及"改 yield 立即改变收获数量"。
//! 3. **文档里的行没有对应数据**：商店价格表中尚未落地的商品必须在显式白名单里，
//!    新增一行而不补数据会让本示例失败，逼出"补数据或移出白名单"的决定。
//!
//! ③–⑦ 把同一套"文档 ↔ 数据"锁扩展到剩余项（M24 续，docs/37 4-14）：
//!   - ③ §2.2 初始属性（铜钱 120 / 内力 50 / 上限 100）↔ core/state.lua + player/inventory.lua；
//!   - ④ §3 打坐效果单元格（恢复 5 点内力）↔ ui/input.lua practice 分支口径；
//!   - ⑤ §6.1 三行招式（基础刀法 2/0/0.4s=24 步、追风步 0/15/2s=120 步+位移 3 格+无敌 15 步、
//!     守心诀 0/10 每秒/持续+正面减伤 70%）↔ data/skills.lua，未实现行只剩回风掌白名单；
//!   - ⑥ §7.2 建筑升级成本（仅木材 4/6/3）↔ data/buildings.lua 的 upgrade_cost；
//!   - ⑦ §9.1 普通订单奖励区间 [40,100] ⊇ order_herb 60 / order_pill 80 / order_hunt 90。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m24_numeric_alignment [-- <game_jhyt-root>]

use rustlua_input::input::{InputSnapshot, KeyCode};
use rustlua_scripting::script::LuaRuntime;
use std::{env, fs, path::PathBuf};

/// 设计案里"尚未进入 MVP 数据"的商店商品（显式白名单，新增行必须做决定）。
const SHOP_ROWS_WITHOUT_DATA: &[&str] = &["草药", "清水", "石材"];

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
    let design_doc = project.join("docs/33-江湖有田-数值设计案.md");
    let design = fs::read_to_string(&design_doc)
        .map_err(|error| format!("cannot read {}: {error}", design_doc.display()))?;
    let main_script = project.join("scripts/main.lua");

    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let host = handles.input;
    let mut runtime = LuaRuntime::with_installers(installers)?;
    runtime.set_project_root(project.clone());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    // 先执行工程入口：`scripts/` 由此进入 require 搜索路径，①② 两节才能在
    // **进入玩法之前**直接读数据（与 `game --check` 同一条路径）。
    runtime.execute_project_script(&main_script)?;

    banner("① 设计案 §4.1 作物表 == data/crops.lua + data/items.lua（逐格解析文档比对）");
    let crop_rows = markdown_table(&design, "作物")?;
    let expected_crops = [
        ("稻谷", "rice"),
        ("灵芝", "lingzhi"),
        ("萝卜", "radish"),
        ("草药", "herb"),
        ("何首乌", "polygonum"),
        ("人参", "ginseng"),
        // 四季作物矩阵扩展（2026-10-05，docs/33 §4.1 0.2 同步 +18）：
        // 新增作物必须同时出现在这里与 docs/33 作物表，否则行数/逐格比对失败。
        ("薄荷", "mint"),
        ("桑葚", "mulberry"),
        ("青豌豆", "pea"),
        ("青瓜", "cucumber"),
        ("绿豆", "mung_bean"),
        ("夏枯草", "prunella"),
        ("茄子", "eggplant"),
        ("南瓜", "pumpkin"),
        ("枸杞", "gouqi"),
        ("白菜", "cabbage"),
        ("菊花", "chrysanthemum"),
        ("韭葱", "leek"),
        ("冬麦", "winter_wheat"),
        ("雪里蕻", "xueli"),
        ("冬参", "winter_shen"),
        ("腊梅", "plum"),
        ("冬白菜", "winter_cabbage"),
        ("冬青", "evergreen"),
    ];
    if crop_rows.len() != expected_crops.len() {
        return Err(format!(
            "设计案作物表有 {} 行，期望 {} 行（新增作物必须同时更新本示例的映射）",
            crop_rows.len(),
            expected_crops.len()
        )
        .into());
    }
    for (row, (doc_name, crop_id)) in crop_rows.iter().zip(expected_crops) {
        if row.len() < 6 {
            return Err(format!("作物表行格式不符（期望 6 列）: {row:?}").into());
        }
        let seed_price = number_from_cell(&row[1])?;
        let growth_days = number_from_cell(&row[2])?;
        let yield_count = number_from_cell(&row[3])?;
        let sell_price = number_from_cell(&row[4])?;
        let land = backticked(&row[5])
            .ok_or_else(|| format!("作物表『推荐土地』列必须用反引号写出土地 id：{}", row[5]))?;
        if row[0] != doc_name {
            return Err(format!("作物表行顺序变化：{} != {doc_name}", row[0]).into());
        }
        let seed_item = query_str(
            &mut runtime,
            &format!("return require('data.crops').{crop_id}.seed_item"),
        )?;
        let harvest_item = query_str(
            &mut runtime,
            &format!("return require('data.crops').{crop_id}.harvest_item"),
        )?;
        let checks = [
            (
                "种子价".to_owned(),
                seed_price,
                query_num(
                    &mut runtime,
                    &format!("require('data.items').{seed_item}.buy"),
                )?,
            ),
            (
                "成长天数".to_owned(),
                growth_days,
                query_num(
                    &mut runtime,
                    &format!("require('data.crops').{crop_id}.growth_days"),
                )?,
            ),
            (
                "基础产量".to_owned(),
                yield_count,
                query_num(
                    &mut runtime,
                    &format!("require('data.crops').{crop_id}.yield"),
                )?,
            ),
            (
                "基础售价".to_owned(),
                sell_price,
                query_num(
                    &mut runtime,
                    &format!("require('data.items').{harvest_item}.sell"),
                )?,
            ),
            (
                "基础售价(crops.sell_price)".to_owned(),
                sell_price,
                query_num(
                    &mut runtime,
                    &format!("require('data.crops').{crop_id}.sell_price"),
                )?,
            ),
        ];
        for (field, doc_value, data_value) in checks {
            if (doc_value - data_value).abs() > f64::EPSILON {
                return Err(format!(
                    "设计案作物表 {doc_name} 的{field} = {doc_value}，Lua 数据为 {data_value}"
                )
                .into());
            }
        }
        let data_land = query_str(
            &mut runtime,
            &format!("return require('data.crops').{crop_id}.land"),
        )?;
        if land != data_land {
            return Err(format!(
                "设计案作物表 {doc_name} 的推荐土地 = {land:?}，Lua 数据为 {data_land:?}"
            )
            .into());
        }
        println!(
            "  {doc_name}({crop_id}): 种子价={seed_price} 成长={growth_days}天 产量={yield_count} 售价={sell_price} 土地={land}"
        );
    }

    banner("② 设计案 §5.2 商店价格表 == data/items.lua（未落地商品必须显式白名单）");
    let shop_rows = markdown_table(&design, "商品")?;
    let mapped = [
        ("稻谷种子", "seed_rice"),
        ("灵芝孢子", "seed_lingzhi"),
        ("木材", "timber"),
        ("回复药", "heal_pill"),
        // Alpha 作物扩展（2026-09-27，docs/33 §5.2）：草药白名单转正 + 4 种新种子。
        ("草药", "herb"),
        ("萝卜籽", "seed_radish"),
        ("草药籽", "seed_herb"),
        ("何首乌苗", "seed_polygonum"),
        ("人参籽", "seed_ginseng"),
        // 四季作物矩阵扩展（2026-10-05，docs/33 §5.2 0.2 同步 +18）：种子名与
        // 商品表逐行对应，改任一名称必须同时改本表与文档。
        ("薄荷籽", "seed_mint"),
        ("桑葚籽", "seed_mulberry"),
        ("青豌豆籽", "seed_pea"),
        ("青瓜籽", "seed_cucumber"),
        ("绿豆籽", "seed_mung_bean"),
        ("夏枯草籽", "seed_prunella"),
        ("茄子籽", "seed_eggplant"),
        ("南瓜籽", "seed_pumpkin"),
        ("枸杞籽", "seed_gouqi"),
        ("白菜籽", "seed_cabbage"),
        ("菊花籽", "seed_chrysanthemum"),
        ("韭葱籽", "seed_leek"),
        ("冬麦籽", "seed_winter_wheat"),
        ("雪里蕻籽", "seed_xueli"),
        ("冬参籽", "seed_winter_shen"),
        ("腊梅籽", "seed_plum"),
        ("冬白菜籽", "seed_winter_cabbage"),
        ("冬青籽", "seed_evergreen"),
        // 肥料（2026-10-05，docs/33 §5.2 0.4 同步 +3）：货摊售卖，不进 buys。
        ("草药肥", "herb_fertilizer"),
        ("骨肥", "bone_fertilizer"),
        ("灵肥", "spirit_fertilizer"),
        // 果树苗（2026-10-05，docs/33 §5.2 0.6 同步 +5）：货摊售卖，sell=0。
        ("桃树苗", "seed_peach"),
        ("青梅树苗", "seed_mume"),
        ("枇杷树苗", "seed_loquat"),
        ("枣树苗", "seed_jujube"),
        ("银杏树苗", "seed_ginkgo"),
    ];
    let mut checked_rows = 0;
    for row in &shop_rows {
        if row.len() < 2 {
            return Err(format!("商店价格表行格式不符（期望 ≥2 列）: {row:?}").into());
        }
        match mapped.iter().find(|(name, _)| *name == row[0]) {
            Some((_, item_id)) => {
                let doc_price = number_from_cell(&row[1])?;
                let data_price = query_num(
                    &mut runtime,
                    &format!("require('data.items').{item_id}.buy"),
                )?;
                if (doc_price - data_price).abs() > f64::EPSILON {
                    return Err(format!(
                        "设计案商店价 {} = {doc_price}，data/items.lua:{item_id}.buy = {data_price}",
                        row[0]
                    )
                    .into());
                }
                checked_rows += 1;
            }
            None => {
                if !SHOP_ROWS_WITHOUT_DATA.contains(&row[0].as_str()) {
                    return Err(format!(
                        "设计案商店价表新增了「{}」但 data/items.lua 里没有对应物品：\
                         要么补数据并登记映射，要么加入 SHOP_ROWS_WITHOUT_DATA 白名单",
                        row[0]
                    )
                    .into());
                }
            }
        }
    }
    if checked_rows != mapped.len() {
        return Err(format!(
            "只比对了 {checked_rows} 行商店价，期望 {} 行（文档行被删/改名时必须同步本示例）",
            mapped.len()
        )
        .into());
    }

    banner("③ 设计案 §2.2 玩家初始属性 == core/state.lua + player/inventory.lua");
    let attr_rows = markdown_table(&design, "属性")?;
    let expected_attrs = [
        (
            "铜钱",
            query_num(&mut runtime, "require('core.state').money")?,
        ),
        (
            "内力",
            query_num(&mut runtime, "require('core.state').inner_power")?,
        ),
        (
            "最大内力",
            query_num(
                &mut runtime,
                "require('player.inventory').max_inner_power()",
            )?,
        ),
        (
            "最大体力",
            query_num(&mut runtime, "require('player.inventory').max_energy()")?,
        ),
    ];
    for (doc_name, data_value) in expected_attrs {
        let row = attr_rows
            .iter()
            .find(|row| row.len() >= 2 && row[0] == doc_name)
            .ok_or_else(|| format!("§2.2 找不到「{doc_name}」行：{attr_rows:?}"))?;
        let doc_value = number_from_cell(&row[1])?;
        if (doc_value - data_value).abs() > f64::EPSILON {
            return Err(
                format!("设计案 §2.2 {doc_name} = {doc_value}，Lua 数据为 {data_value}").into(),
            );
        }
        println!("  {doc_name}: 初始/上限 = {doc_value}");
    }

    banner("④ 设计案 §3 打坐行 == 实现口径（恢复 5 点内力，ui/input.lua）");
    let action_rows = markdown_table(&design, "行为")?;
    let meditate = action_rows
        .iter()
        .find(|row| row.first().map(String::as_str) == Some("打坐"))
        .ok_or_else(|| format!("§3 找不到「打坐」行：{action_rows:?}"))?;
    if meditate.len() < 4 {
        return Err(format!("打坐行格式不符（期望 ≥4 列）: {meditate:?}").into());
    }
    // 实现口径：`ui/input.lua` 的 practice 分支
    //   `state.inner_power = math.min(require("systems.meridian").max_inner_power(),
    //                                  (state.inner_power or 0) + 5)`
    // 所以文档单元格必须同时写两件事：**+5 内力** 与 **钳经脉动态上限**。
    //
    // 2026-10-20：单元格被改写成「恢复5点内力，钳经脉动态上限」（去掉旧文案里的空格、补上
    // 钳制那一半），旧断言按带空格的「恢复 5 点内力」逐字比对，于是把一次**更准确**的
    // 文档修订判成了漂移。现在按语义比对（去空白）并把「钳经脉」也纳入判据——它和 §10
    // 的「体力恢复」行、§7.4 的跨天恢复池说的是同一件事，这条门守的就是这一致。
    let cell = meditate[3].replace([' ', '\u{3000}'], "");
    for needle in ["恢复5点内力", "钳经脉动态上限"] {
        if !cell.contains(needle) {
            return Err(format!(
                "设计案 §3 打坐效果 = {:?}，实现口径为「恢复5点内力，钳经脉动态上限」\
                 （`ui/input.lua` practice 分支：math.min(meridian.max_inner_power(), +5)）",
                meditate[3]
            )
            .into());
        }
    }
    // 文档写了「钳经脉动态上限」，代码侧那条钳制也必须真的在：只查文档不查代码的话，
    // 把 `math.min(...)` 拆掉（退回硬上限 100）这条路一个断言都碰不到。
    let input_source = fs::read_to_string(project.join("scripts").join("ui").join("input.lua"))?;
    let mut clamped = false;
    let mut cursor = 0usize;
    while let Some(found) = input_source[cursor..].find("action_pressed(\"practice\")") {
        let at = cursor + found;
        let tail = &input_source[at..(at + 320).min(input_source.len())];
        if tail.contains("max_inner_power()") && tail.contains("+ 5") {
            clamped = true;
            break;
        }
        cursor = at + 1;
    }
    if !clamped {
        return Err(
            "`ui/input.lua` 的 practice 分支不再按 meridian.max_inner_power() 钳制 +5 内力：\
                   文档 §3 写的「钳经脉动态上限」在代码里已经没有对应实现"
                .into(),
        );
    }
    println!(
        "  打坐: 效果单元格 = {:?}（+5 内力 ∧ 钳经脉动态上限，两半都有代码对应）",
        meditate[3]
    );

    banner("⑤ 设计案 §6.1 玩家招式 == data/skills.lua（基础刀法/追风步/守心诀；白名单只剩回风掌）");
    let skill_rows = markdown_table(&design, "招式")?;
    // 白名单的语义是"这一行还没有数据可对"——M34-04 把追风步/守心诀补成了 `data/skills.lua`
    // 的 movement/guard 两行，所以它们必须从这里**删掉**（留着的分支是静默跳过的）。
    const SKILL_ROWS_WITHOUT_DATA: &[&str] = &["回风掌"];
    let mut skill_checked = 0;
    for row in &skill_rows {
        if row.len() < 4 {
            return Err(format!("招式表行格式不符（期望 ≥4 列）: {row:?}").into());
        }
        match row[0].as_str() {
            "基础刀法" => {
                let doc_damage = number_from_cell(&row[1])?;
                let data_damage =
                    query_num(&mut runtime, "require('data.skills').skills.strike.damage")?;
                if (doc_damage - data_damage).abs() > f64::EPSILON {
                    return Err(format!(
                        "设计案 §6.1 基础刀法伤害 = {doc_damage}，data/skills.lua:strike.damage = {data_damage}"
                    )
                    .into());
                }
                let doc_inner = number_from_cell(&row[2])?;
                let data_inner = query_num(
                    &mut runtime,
                    "require('data.skills').skills.strike.inner_cost",
                )?;
                if (doc_inner - data_inner).abs() > f64::EPSILON {
                    return Err(format!(
                        "设计案 §6.1 基础刀法内力消耗 = {doc_inner}，实现为 {data_inner}"
                    )
                    .into());
                }
                // 冷却：文档写秒（0.4 秒），数据写 60Hz 固定步（24）→ 秒×60 == 步。
                let doc_cooldown_sec = number_from_cell(&row[3])?;
                let data_cooldown_steps = query_num(
                    &mut runtime,
                    "require('data.skills').skills.strike.cooldown",
                )?;
                if (doc_cooldown_sec * 60.0 - data_cooldown_steps).abs() > f64::EPSILON {
                    return Err(format!(
                        "设计案 §6.1 基础刀法冷却 = {doc_cooldown_sec}s，实现 {data_cooldown_steps} 步（60Hz）"
                    )
                    .into());
                }
                // 装剑后 10：由 jhyt_m18_stats 的 player_damage 行为断言覆盖，这里只锁数据行。
                skill_checked += 1;
                println!("  基础刀法: 伤害={doc_damage} 内力={doc_inner} 冷却={doc_cooldown_sec}s");
            }
            "追风步" => {
                // movement 类：伤害 0 + 内力 + 冷却（文档秒 → 固定步）与基础刀法同一读法，
                // 「其他」列的位移格数与无敌秒数是这一招**唯一**的行为参数，也必须逐格对上。
                let doc_damage = number_from_cell(&row[1])?;
                let data_damage = query_num(
                    &mut runtime,
                    "require('data.skills').skills.chase_wind.damage",
                )?;
                if (doc_damage - data_damage).abs() > f64::EPSILON {
                    return Err(format!(
                        "设计案 §6.1 追风步伤害 = {doc_damage}，data/skills.lua:chase_wind.damage = {data_damage}"
                    )
                    .into());
                }
                let doc_inner = number_from_cell(&row[2])?;
                let data_inner = query_num(
                    &mut runtime,
                    "require('data.skills').skills.chase_wind.inner_cost",
                )?;
                if (doc_inner - data_inner).abs() > f64::EPSILON {
                    return Err(format!(
                        "设计案 §6.1 追风步内力消耗 = {doc_inner}，实现为 {data_inner}"
                    )
                    .into());
                }
                let doc_cooldown_sec = number_from_cell(&row[3])?;
                let data_cooldown_steps = query_num(
                    &mut runtime,
                    "require('data.skills').skills.chase_wind.cooldown",
                )?;
                if (doc_cooldown_sec * 60.0 - data_cooldown_steps).abs() > f64::EPSILON {
                    return Err(format!(
                        "设计案 §6.1 追风步冷却 = {doc_cooldown_sec}s，实现 {data_cooldown_steps} 步（60Hz）"
                    )
                    .into());
                }
                let data_tiles = query_num(
                    &mut runtime,
                    "require('data.skills').skills.chase_wind.dash_tiles",
                )?;
                let want_tiles = format!("位移 {:.0} 格", data_tiles);
                if !row[4].contains(&want_tiles) {
                    return Err(format!(
                        "设计案 §6.1 追风步「其他」= {:?}，实现位移 {data_tiles:.0} 格（应含「{want_tiles}」）",
                        row[4]
                    )
                    .into());
                }
                let data_iframe_steps = query_num(
                    &mut runtime,
                    "require('data.skills').skills.chase_wind.invincible_steps",
                )?;
                let want_iframes = format!("无敌 {:.2} 秒", data_iframe_steps / 60.0);
                if !row[4].contains(&want_iframes) {
                    return Err(format!(
                        "设计案 §6.1 追风步「其他」= {:?}，实现无敌 {data_iframe_steps:.0} 固定步 = {want_iframes}",
                        row[4]
                    )
                    .into());
                }
                skill_checked += 1;
                println!(
                    "  追风步: 伤害={doc_damage} 内力={doc_inner} 冷却={doc_cooldown_sec}s 位移={data_tiles:.0}格 无敌={data_iframe_steps:.0}固定步（{want_iframes}）"
                );
            }
            "守心诀" => {
                // guard 类：冷却那一格写的是「持续」（= 数据里 cooldown = 0），内力写「10/秒」
                // （= 每 inner_period 固定步扣一次 inner_cost），减伤比例在「其他」列。
                let doc_damage = number_from_cell(&row[1])?;
                let data_damage = query_num(
                    &mut runtime,
                    "require('data.skills').skills.guard_heart.damage",
                )?;
                if (doc_damage - data_damage).abs() > f64::EPSILON {
                    return Err(format!(
                        "设计案 §6.1 守心诀伤害 = {doc_damage}，data/skills.lua:guard_heart.damage = {data_damage}"
                    )
                    .into());
                }
                let doc_inner = number_from_cell(&row[2])?;
                let data_inner = query_num(
                    &mut runtime,
                    "require('data.skills').skills.guard_heart.inner_cost",
                )?;
                if (doc_inner - data_inner).abs() > f64::EPSILON {
                    return Err(format!(
                        "设计案 §6.1 守心诀内力消耗 = {doc_inner}，实现为每期内力 {data_inner}"
                    )
                    .into());
                }
                if !row[2].contains("/秒") {
                    return Err(format!(
                        "设计案 §6.1 守心诀内力那一格 = {:?}，不带「/秒」就无法与「每 inner_period 步扣一次」对齐",
                        row[2]
                    )
                    .into());
                }
                let data_period = query_num(
                    &mut runtime,
                    "require('data.skills').skills.guard_heart.inner_period",
                )?;
                if (data_period - 60.0).abs() > f64::EPSILON {
                    return Err(format!(
                        "§6.1 那一格写「{doc_inner}/秒」，实现必须每 60 固定步扣一笔，实际 inner_period = {data_period}"
                    )
                    .into());
                }
                if !row[3].contains("持续") {
                    return Err(format!(
                        "设计案 §6.1 守心诀冷却/恢复那一格 = {:?}，实现口径是「持续」（guard_heart.cooldown = 0）",
                        row[3]
                    )
                    .into());
                }
                let data_cooldown = query_num(
                    &mut runtime,
                    "require('data.skills').skills.guard_heart.cooldown",
                )?;
                if data_cooldown.abs() > f64::EPSILON {
                    return Err(format!(
                        "§6.1 写「持续」的招式在数据里有冷却 {data_cooldown} 步（守心诀应该是 0）"
                    )
                    .into());
                }
                let data_absorb = query_num(
                    &mut runtime,
                    "require('data.skills').skills.guard_heart.absorb",
                )?;
                let want_absorb = format!("正面减伤 {:.0}%", data_absorb * 100.0);
                if !row[4].contains(&want_absorb) {
                    return Err(format!(
                        "设计案 §6.1 守心诀「其他」= {:?}，实现 absorb = {data_absorb}（应含「{want_absorb}」）",
                        row[4]
                    )
                    .into());
                }
                skill_checked += 1;
                println!(
                    "  守心诀: 伤害={doc_damage} 内力={doc_inner}/秒（每 {data_period:.0} 步） 恢复={want_absorb}"
                );
            }
            name if SKILL_ROWS_WITHOUT_DATA.contains(&name) => {}
            name => {
                return Err(format!(
                    "设计案 §6.1 新增了「{name}」但 data/skills.lua 没有对应招式：\
                     要么补数据并登记映射，要么加入 SKILL_ROWS_WITHOUT_DATA 白名单"
                )
                .into());
            }
        }
    }
    if skill_checked != 3 {
        return Err(
            format!("§6.1 应比对 3 行（基础刀法/追风步/守心诀），实际 {skill_checked} 行").into(),
        );
    }

    banner("⑥ 设计案 §7.2 建筑升级成本 == data/buildings.lua（仅木材口径）");
    let upgrade_lines = markdown_bullets_after(&design, "升级成本")?;
    let expected_costs = [
        ("药庐 1→2 级", "herb_hut", 4.0),
        ("库房 1→2 级", "storehouse", 6.0),
        ("茶棚 1→2 级", "tea_shed", 3.0),
        ("地热药圃 1→2 级", "greenhouse", 6.0),
    ];
    if upgrade_lines.len() < expected_costs.len() {
        return Err(format!(
            "§7.2 升级成本只找到 {} 行，期望 ≥{} 行",
            upgrade_lines.len(),
            expected_costs.len()
        )
        .into());
    }
    for (doc_label, building_id, expected_timber) in expected_costs {
        if !upgrade_lines.iter().any(|line| {
            line.contains(doc_label) && line.contains(&format!("木材 {}", expected_timber as i64))
        }) {
            return Err(format!(
                "§7.2 找不到「{doc_label}：木材 {}」行：{upgrade_lines:?}",
                expected_timber as i64
            )
            .into());
        }
        let data_cost = query_str(
            &mut runtime,
            &format!(
                "(function() local b = require('data.buildings').buildings.{building_id}; \
                 local out = {{}}; for _, c in ipairs(b.upgrade_cost or {{}}) do out[#out+1] = c.item .. '=' .. tostring(c.count) end; \
                 return table.concat(out, ',') end)()"
            ),
        )?;
        if data_cost != format!("timber={}", expected_timber as i64) {
            return Err(format!(
                "设计案 §7.2 {doc_label} 木材 {}，data/buildings.lua:{building_id}.upgrade_cost = {data_cost}",
                expected_timber as i64
            )
            .into());
        }
        println!("  {doc_label}: upgrade_cost = {data_cost}");
    }

    banner("⑦ 设计案 §9.1 普通订单奖励区间 ⊇ data/quests/order_*.lua 的显式金额");
    let reward_rows = markdown_table(&design, "难度")?;
    let ordinary = reward_rows
        .iter()
        .find(|row| row.first().map(String::as_str) == Some("普通"))
        .ok_or_else(|| format!("§9.1 找不到「普通」行：{reward_rows:?}"))?;
    if ordinary.len() < 3 {
        return Err(format!("§9.1 普通行格式不符（期望 ≥3 列）: {ordinary:?}").into());
    }
    // 铜钱奖励单元格形如 `40–100`（en dash）或 `40-100`，取首尾数字为区间。
    let (reward_min, reward_max) = range_from_cell(&ordinary[2])?;
    let order_rewards = [
        ("order_herb", 60.0),
        ("order_pill", 80.0),
        ("order_hunt", 90.0),
        ("order_tea", 90.0),
    ];
    for (quest_id, expected) in order_rewards {
        let data_money = query_num(
            &mut runtime,
            &format!(
                "(function() local q = require('data.quests.{quest_id}'); \
                 for _, r in ipairs(q.rewards or {{}}) do if r.kind == 'money' then return r.amount end end; \
                 return -1 end)()"
            ),
        )?;
        if (data_money - expected).abs() > f64::EPSILON {
            return Err(format!(
                "设计案 §9.2 订单 {quest_id} 奖励应为 {expected}（文档口径），实现为 {data_money}"
            )
            .into());
        }
        if data_money < reward_min || data_money > reward_max {
            return Err(format!(
                "订单 {quest_id} 奖励 {data_money} 不在 §9.1 普通任务区间 [{reward_min},{reward_max}]"
            )
            .into());
        }
        println!("  {quest_id}: 铜钱奖励={data_money}（区间 [{reward_min},{reward_max}] 内）");
    }

    banner("⑧ 灵芝：真实耕作 7 天收获，数量 == data/crops.lua:lingzhi.yield");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 山庄出生点正前方就是药田格（与 jhyt_m17/jhyt_m22 同一格）。
    set_facing(&mut runtime, "up")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    set_seed(&mut runtime, "seed_lingzhi")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_lingzhi",
    )?;
    let lingzhi_before = total_crop(&mut runtime, "lingzhi")?;
    grow_and_harvest(&mut runtime, &host, "watering_can", "water_lingzhi")?;
    let lingzhi_yield = total_crop(&mut runtime, "lingzhi")? - lingzhi_before;
    let documented_lingzhi = query_num(&mut runtime, "require('data.crops').lingzhi.yield")?;
    if (lingzhi_yield - documented_lingzhi).abs() > f64::EPSILON {
        return Err(format!(
            "灵芝实际收获 {lingzhi_yield}，data/crops.lua:lingzhi.yield = {documented_lingzhi}"
        )
        .into());
    }

    banner("④ 稻谷：换到 `field` 地块，真实耕作 4 天收获，数量 == yield");
    let field_cell = query_str(
        &mut runtime,
        "local g = entity.find('GroundMap')\n\
         local land = require('data.farm').land\n\
         for cy = 3, 13 do\n\
           for cx = 14, 33 do\n\
             if (tilemap.get_cell(g, cx, cy) or 0) == 2 then\n\
               local kind = nil\n\
               for _, rect in ipairs(land) do\n\
                 if cx >= rect.x0 and cx <= rect.x1 and cy >= rect.y0 and cy <= rect.y1 then kind = rect.type end\n\
               end\n\
               if kind == 'field' then return cx .. ',' .. cy end\n\
             end\n\
           end\n\
         end\n\
         return 'none'",
    )?;
    if field_cell == "none" {
        return Err("manor 场景里找不到 field 类型的可开垦地块（地点/地表数据变化？）".into());
    }
    let (fx, fy) = parse_cell(&field_cell)?;
    // 站到目标格下方一格、朝上，正对目标格（facing_cell 取"面前一格"）。
    runtime.execute_console(&format!(
        "local pid = require('player.player').ensure()\n\
         transform.set(pid, {}, {})",
        (fx as f64 + 0.5) * 32.0,
        (fy as f64 + 1.5) * 32.0
    ))?;
    set_facing(&mut runtime, "up")?;
    println!("  field 地块 = {field_cell}（玩家已就位）");
    equip_tool(&mut runtime, "hoe")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    set_seed(&mut runtime, "seed_rice")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_rice",
    )?;
    let rice_before = total_crop(&mut runtime, "rice")?;
    grow_and_harvest(&mut runtime, &host, "watering_can", "water_rice")?;
    let rice_yield = total_crop(&mut runtime, "rice")? - rice_before;
    let documented_rice = query_num(&mut runtime, "require('data.crops').rice.yield")?;
    if (rice_yield - documented_rice).abs() > f64::EPSILON {
        return Err(format!(
            "稻谷实际收获 {rice_yield}，data/crops.lua:rice.yield = {documented_rice}"
        )
        .into());
    }

    banner("⑤ 数值真的由数据驱动：把 rice.yield 改成 3，同样流程就该收 3");
    runtime.execute_console("require('data.crops').rice.yield = 3")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_rice",
    )?;
    let rice_before_two = total_crop(&mut runtime, "rice")?;
    grow_and_harvest(&mut runtime, &host, "watering_can", "water_rice")?;
    let mutated_yield = total_crop(&mut runtime, "rice")? - rice_before_two;
    if (mutated_yield - 3.0).abs() > f64::EPSILON {
        return Err(format!(
            "yield=3 时实际收获 {mutated_yield}；收获数量没有走 data/crops.lua（硬编码？）"
        )
        .into());
    }
    runtime.execute_console("require('data.crops').rice.yield = 2")?;
    let recovered = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !recovered.is_empty() {
        return Err(format!("还原 yield 后校验不通过:\n{recovered}").into());
    }

    banner(
        "⑨ 萝卜：复用 ④ 的 field 格（⑤ 已收空为 tilled 空地），真实耕作 2 天收获 == yield（Alpha 快熟）",
    );
    // ⑤ 的 grow_and_harvest 已把 field_cell 收空（plot.crop=nil、可播种），且玩家仍在原位
    // 面向 up（④/⑤ 均未移动）。till 步骤已由④ 断言覆盖。
    // 测试角色没有萝卜籽（初始背包只有稻种/菌种；真实玩家从 zhao_stall 购买）——
    // 先补 1 个到背包，否则 seed_for_selected 会回退选中背包里第一个有库存的种子。
    runtime.execute_console("inv.add(require('core.state').inventory, 'seed_radish', 1)")?;
    set_facing(&mut runtime, "up")?;
    set_seed(&mut runtime, "seed_radish")?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_radish",
    )?;
    let radish_before = total_crop(&mut runtime, "radish")?;
    grow_and_harvest(&mut runtime, &host, "watering_can", "water_radish")?;
    let radish_yield = total_crop(&mut runtime, "radish")? - radish_before;
    let documented_radish = query_num(&mut runtime, "require('data.crops').radish.yield")?;
    if (radish_yield - documented_radish).abs() > f64::EPSILON {
        return Err(format!(
            "萝卜实际收获 {radish_yield}，data/crops.lua:radish.yield = {documented_radish}"
        )
        .into());
    }

    banner("⑥ yield 是必填字段：抹掉它必须在校验期报出（不能再静默按 1 结算）");
    let missing = query_str(
        &mut runtime,
        "require('data.crops').rice.yield = nil\n\
         local ok, issues = require('data.registry_check').validate_all()\n\
         require('data.crops').rice.yield = 2\n\
         if ok then return '' end\n\
         return table.concat(issues, '\\n')",
    )?;
    if !missing.contains("data/crops.lua:rice.yield") {
        return Err(format!("缺少 yield 时未报出问题，得到:\n{missing}").into());
    }

    banner("⑩ 设计案 §8.1/§8.2 档位表 == data/relationships.lua 的 tiers / reputation_tiers");
    // 两张表都逐格比对：区间下界直接取自数据，区间上界由"下一档下界 - 1"推出、
    // 末档取值域上界（关系分 0..100 由 relationship.add 的 clamp 定，声望上界取 faction.max）。
    // 文档改一个数、加一行或删一行都会在这里红——§8.2 的折扣列与 effects.price_off 也是同一口径。
    let affinity_rows = markdown_table(&design, "关系分区间")?;
    let affinity_tiers = query_num(&mut runtime, "#require('data.relationships').tiers")? as usize;
    if affinity_rows.len() != affinity_tiers {
        return Err(format!(
            "§8.1 关系档位表 {} 行，data.relationships.tiers {} 档",
            affinity_rows.len(),
            affinity_tiers
        )
        .into());
    }
    for (index, row) in affinity_rows.iter().enumerate() {
        if row.len() < 2 {
            return Err(format!("§8.1 第 {} 行格式不符（期望 ≥2 列）: {row:?}", index + 1).into());
        }
        let (doc_min, doc_max) = range_from_cell(&row[0])?;
        let data_min = query_num(
            &mut runtime,
            &format!("require('data.relationships').tiers[{}].min", index + 1),
        )?;
        let next_min = if index + 1 < affinity_tiers {
            query_num(
                &mut runtime,
                &format!("require('data.relationships').tiers[{}].min", index + 2),
            )? - 1.0
        } else {
            100.0
        };
        let data_name = query_str(
            &mut runtime,
            &format!(
                "return require('data.relationships').tiers[{}].name",
                index + 1
            ),
        )?;
        if doc_min != data_min || doc_max != next_min {
            return Err(format!(
                "§8.1 第 {} 档区间写的是「{}」，data.relationships.tiers 推出的是 {data_min:.0}–{next_min:.0}（{data_name}）",
                index + 1,
                row[0]
            )
            .into());
        }
        if row[1] != data_name {
            return Err(format!(
                "§8.1 第 {} 档称号是「{}」，data.relationships.tiers 写的是「{data_name}」",
                index + 1,
                row[1]
            )
            .into());
        }
        println!("  关系 {data_min:.0}–{next_min:.0}: {data_name}");
    }

    let reputation_rows = markdown_table(&design, "声望区间")?;
    let reputation_tiers = query_num(
        &mut runtime,
        "#require('data.relationships').reputation_tiers",
    )? as usize;
    if reputation_rows.len() != reputation_tiers {
        return Err(format!(
            "§8.2 声望档位表 {} 行，data.relationships.reputation_tiers {} 档",
            reputation_rows.len(),
            reputation_tiers
        )
        .into());
    }
    for (index, row) in reputation_rows.iter().enumerate() {
        if row.len() < 3 {
            return Err(format!("§8.2 第 {} 行格式不符（期望 ≥3 列）: {row:?}", index + 1).into());
        }
        let (doc_min, doc_max) = range_from_cell(&row[0])?;
        let doc_off = number_from_cell(&row[2])?;
        let path = format!(
            "require('data.relationships').reputation_tiers[{}]",
            index + 1
        );
        let data_min = query_num(&mut runtime, &format!("{path}.min"))?;
        let data_max = query_num(&mut runtime, &format!("{path}.max"))?;
        let data_name = query_str(&mut runtime, &format!("return {path}.name"))?;
        let data_off = query_num(
            &mut runtime,
            &format!("({path}.effects and {path}.effects.price_off) or 0"),
        )?;
        if doc_min != data_min || doc_max != data_max {
            return Err(format!(
                "§8.2 第 {} 档区间写的是 {}，data.relationships.reputation_tiers 是 {data_min:.0}–{data_max:.0}",
                index + 1,
                row[0]
            )
            .into());
        }
        if row[1] != data_name {
            return Err(format!(
                "§8.2 第 {} 档称号是「{}」，数据里是「{data_name}」",
                index + 1,
                row[1]
            )
            .into());
        }
        if doc_off != data_off {
            return Err(format!(
                "§8.2 「{data_name}」档折扣文档写 {doc_off:.0}%，effects.price_off = {data_off:.0}"
            )
            .into());
        }
        println!("  声望 {data_min:.0}–{data_max:.0}: {data_name}，购买折扣 {data_off:.0}%");
    }

    banner("⑪ 策划案 docs/30 §12 节气表 == data/festivals.lua（名称/季节/季内天数逐格比对）");
    // M34-02：节气表是"今天说得出这句话、采得到两倍、走不走得了山口"的唯一规格来源，
    // 于是按 docs/30 §12 逐格比：行数、顺序、季节、季内天数闭区间都要对上。
    // 「实现落点」列同样必填——新增一行而不交代它接在哪个消费方，本示例直接红。
    let plan_doc = project.join("docs/30-江湖有田-总策划案.md");
    let plan = fs::read_to_string(&plan_doc)
        .map_err(|error| format!("cannot read {}: {error}", plan_doc.display()))?;
    let festival_rows = markdown_table(&plan, "节气/事件")?;
    let festival_count = query_num(&mut runtime, "#require('data.festivals').festivals")? as usize;
    if festival_rows.len() != festival_count {
        return Err(format!(
            "docs/30 §12 节气表 {} 行，data/festivals.lua {} 条",
            festival_rows.len(),
            festival_count
        )
        .into());
    }
    for (index, row) in festival_rows.iter().enumerate() {
        if row.len() < 5 || row[4].is_empty() {
            return Err(format!(
                "docs/30 §12 第 {} 行缺「实现落点」列（期望 5 列且最后一列非空）: {row:?}",
                index + 1
            )
            .into());
        }
        let path = format!("require('data.festivals').festivals[{}]", index + 1);
        let data_name = query_str(&mut runtime, &format!("return {path}.name"))?;
        if row[0] != data_name {
            return Err(format!(
                "docs/30 §12 第 {} 行是「{}」，data/festivals.lua 第 {} 条是「{data_name}」——名称或顺序对不上",
                index + 1,
                row[0],
                index + 1
            )
            .into());
        }
        let data_season = query_str(&mut runtime, &format!("return {path}.season"))?;
        if row[1] != season_cn(&data_season) {
            return Err(format!(
                "docs/30 §12「{data_name}」季节写「{}」，data/festivals.lua 是 {data_season}",
                row[1]
            )
            .into());
        }
        let (doc_from, doc_to) = range_from_cell(&row[2])?;
        let data_from = query_num(&mut runtime, &format!("{path}.from"))?;
        let data_to = query_num(&mut runtime, &format!("{path}.to"))?;
        if doc_from != data_from || doc_to != data_to {
            return Err(format!(
                "docs/30 §12「{data_name}」季内天数写「{}」，数据是 {data_from:.0}–{data_to:.0}",
                row[2]
            )
            .into());
        }
        println!("  {data_name}: {data_season} {data_from:.0}–{data_to:.0} 天");
    }

    println!(
        "jhyt_m24_numeric_alignment_ok project={} crops={} shop_rows={} lingzhi_yield={} \
         rice_yield={} mutated_yield={} field_cell={} radish_yield={} affinity_tiers={} \
         reputation_tiers={} festivals={}",
        project.display(),
        crop_rows.len(),
        checked_rows,
        lingzhi_yield,
        rice_yield,
        mutated_yield,
        field_cell,
        radish_yield,
        affinity_tiers,
        reputation_tiers,
        festival_rows.len()
    );
    Ok(())
}

/// 走完"浇水 → 逐日睡觉 → 换锄头收获"，返回时地块已收空。
///
/// 作物成熟需要 `growth_days` 天浇水：每天浇一次。
///
/// **每一天都要重新钉住晴天**（M29 起天气按 `(seed, day, 季节)` 抽签）：雨天走的是
/// `rain_watered`（免体力）分支，本样例断言的是"数值由数据驱动"的体力/产量口径，
/// 因此把天气这个变量固定下来（`weather.set` 只钉当天，跨天会重抽）。
fn grow_and_harvest(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    water_tool: &str,
    water_action: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    equip_tool(runtime, water_tool)?;
    loop {
        runtime.execute_console("require('world.weather').set('sunny')")?;
        press(runtime, &host, KeyCode::KeyE)?;
        let action = query_str(runtime, "require('core.state').last_action")?;
        if action != water_action {
            return Err(format!("浇水失败，last_action = {action:?}").into());
        }
        runtime.execute_console("day.sleep()")?;
        let watered = query_num(
            runtime,
            "(function() local m = map.state('farm_plots')\n\
              for _, p in pairs(m) do if p.crop then return p.watered_days end end\n\
              return -1 end)()",
        )?;
        if watered < 0.0 {
            return Err("睡觉后地块上的作物消失了".into());
        }
        let mature = query_str(
            runtime,
            "local m = map.state('farm_plots')\n\
             local crops = require('data.crops')\n\
             for _, p in pairs(m) do\n\
               if p.crop then\n\
                 local crop = crops[p.crop]\n\
                 if crop and p.watered_days >= crop.growth_days then return 'yes' end\n\
               end\n\
             end\n\
             return 'no'",
        )?;
        if mature == "yes" {
            break;
        }
        if watered > 30.0 {
            return Err("作物 30 天还没成熟，生长判定可能被破坏".into());
        }
    }
    equip_tool(runtime, "hoe")?;
    press(runtime, &host, KeyCode::KeyE)?;
    let action = query_str(runtime, "require('core.state').last_action")?;
    if !action.starts_with("harvest_") {
        return Err(format!("收获失败，last_action = {action:?}").into());
    }
    Ok(())
}

fn parse_cell(cell: &str) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let (x, y) = cell
        .split_once(',')
        .ok_or_else(|| format!("cell {cell:?} is not `x,y`"))?;
    Ok((x.trim().parse()?, y.trim().parse()?))
}

/// 季节标识 → 策划案表格里的中文列（docs/30 §12 用中文写季节）。
fn season_cn(season: &str) -> &'static str {
    match season {
        "spring" => "春",
        "summer" => "夏",
        "autumn" => "秋",
        "winter" => "冬",
        _ => "?",
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
        // 分隔行（|---|---|）
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

/// 取设计案 markdown 里以 `needle` 为标题下方的"`- `"清单行（直到第一个非清单非空行）。
fn markdown_bullets_after(
    markdown: &str,
    needle: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut bullets = Vec::new();
    let mut found = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if !found {
            // 标题行必须是段落/标题（不能是 `- ` 列表行），避免命中"沿革/备注"里的同词。
            if trimmed.contains(needle) && !trimmed.starts_with("- ") {
                found = true;
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            bullets.push(rest.to_owned());
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('|') || trimmed.starts_with("```") {
            continue;
        }
        break;
    }
    if bullets.is_empty() {
        return Err(format!("设计案里找不到「{needle}」下方的清单行").into());
    }
    Ok(bullets)
}

/// 区间单元格：`40–100` / `40-100` / `40—100` → (40, 100)。
fn range_from_cell(cell: &str) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let trimmed = cell.replace(' ', "");
    let (left, right) = trimmed
        .split_once(['–', '-', '—'])
        .ok_or_else(|| format!("cell {cell:?} 不是区间（如 40–100）"))?;
    Ok((left.parse()?, right.parse()?))
}

/// 单元格里的数值：`12`、`30/个`、`4` 都要能解析出前面的数字。
fn number_from_cell(cell: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let digits: String = cell
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    digits
        .parse::<f64>()
        .map_err(|error| format!("cell {cell:?} has no leading number: {error}").into())
}

/// 取反引号里的第一个标识符：`普通耕地（\`field\`）` → `field`。
fn backticked(cell: &str) -> Option<String> {
    let start = cell.find('`')? + 1;
    let rest = &cell[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_owned())
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}

/// 打开门面场景（`main.lua` 已在 main 开头执行过一次，这里只切场景）。
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

/// 统计某收获物的背包总量 = 普通 + 良好 + 上品三条目之和（品质系统
/// docs/33 §4.1.2：收获可能产出 `<id>_good` / `<id>_super`，数量断言必须合并统计）。
fn total_crop(runtime: &mut LuaRuntime, crop_id: &str) -> Result<f64, Box<dyn std::error::Error>> {
    query_num(
        runtime,
        &format!(
            "inv.count('player', '{crop_id}') + inv.count('player', '{crop_id}_good') + inv.count('player', '{crop_id}_super')"
        ),
    )
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
