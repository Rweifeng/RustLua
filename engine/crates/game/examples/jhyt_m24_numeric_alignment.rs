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
//! Usage:
//!   cargo run -p game --example jhyt_m24_numeric_alignment [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{env, fs, path::PathBuf};

/// 设计案里"尚未进入 MVP 数据"的商店商品（显式白名单，新增行必须做决定）。
const SHOP_ROWS_WITHOUT_DATA: &[&str] = &["草药", "清水", "石材"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let design_doc = project.join("docs/33-江湖有田-数值设计案.md");
    let design = fs::read_to_string(&design_doc)
        .map_err(|error| format!("cannot read {}: {error}", design_doc.display()))?;
    let main_script = project.join("scripts/main.lua");

    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.clone());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(720, 1280);
    runtime.set_delta(1.0 / 60.0);
    // 先执行工程入口：`scripts/` 由此进入 require 搜索路径，①② 两节才能在
    // **进入玩法之前**直接读数据（与 `game --check` 同一条路径）。
    runtime.execute_project_script(&main_script)?;

    banner("① 设计案 §4.1 作物表 == data/crops.lua + data/items.lua（逐格解析文档比对）");
    let crop_rows = markdown_table(&design, "作物")?;
    let expected_crops = [("稻谷", "rice"), ("灵芝", "lingzhi")];
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

    banner("③ 灵芝：真实耕作 7 天收获，数量 == data/crops.lua:lingzhi.yield");
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    // 山庄出生点正前方就是药田格（与 jhyt_m17/jhyt_m22 同一格）。
    set_facing(&mut runtime, "up")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    set_seed(&mut runtime, "seed_lingzhi")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_lingzhi",
    )?;
    let lingzhi_before = query_num(&mut runtime, "inv.count('player', 'lingzhi')")?;
    grow_and_harvest(&mut runtime, "watering_can", "water_lingzhi")?;
    let lingzhi_yield = query_num(&mut runtime, "inv.count('player', 'lingzhi')")? - lingzhi_before;
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
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    set_seed(&mut runtime, "seed_rice")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_rice",
    )?;
    let rice_before = query_num(&mut runtime, "inv.count('player', 'rice')")?;
    grow_and_harvest(&mut runtime, "watering_can", "water_rice")?;
    let rice_yield = query_num(&mut runtime, "inv.count('player', 'rice')")? - rice_before;
    let documented_rice = query_num(&mut runtime, "require('data.crops').rice.yield")?;
    if (rice_yield - documented_rice).abs() > f64::EPSILON {
        return Err(format!(
            "稻谷实际收获 {rice_yield}，data/crops.lua:rice.yield = {documented_rice}"
        )
        .into());
    }

    banner("⑤ 数值真的由数据驱动：把 rice.yield 改成 3，同样流程就该收 3");
    runtime.execute_console("require('data.crops').rice.yield = 3")?;
    press(&mut runtime, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_rice",
    )?;
    let rice_before_two = query_num(&mut runtime, "inv.count('player', 'rice')")?;
    grow_and_harvest(&mut runtime, "watering_can", "water_rice")?;
    let mutated_yield = query_num(&mut runtime, "inv.count('player', 'rice')")? - rice_before_two;
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

    println!(
        "jhyt_m24_numeric_alignment_ok project={} crops={} shop_rows={} lingzhi_yield={} \
         rice_yield={} mutated_yield={} field_cell={}",
        project.display(),
        crop_rows.len(),
        checked_rows,
        lingzhi_yield,
        rice_yield,
        mutated_yield,
        field_cell
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
    water_tool: &str,
    water_action: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    equip_tool(runtime, water_tool)?;
    loop {
        runtime.execute_console("require('world.weather').set('sunny')")?;
        press(runtime, KeyCode::KeyE)?;
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
    press(runtime, KeyCode::KeyE)?;
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

fn press(runtime: &mut LuaRuntime, key: KeyCode) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
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
