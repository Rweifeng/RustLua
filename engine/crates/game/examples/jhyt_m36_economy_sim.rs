//! Headless acceptance drive for the crop economy curve (改造计划 §5.3 / docs/33 §5.3, M36).
//!
//! 把"经济曲线"变成机器断言（纯数据层，不跑真实耕作——真实链路由
//! `jhyt_m36_regrow` / `jhyt_m24_numeric_alignment` 覆盖代表作物）：
//!   1. 作物总数 = 24，每季可种 ≥6（冬季 ≥6）；
//!   2. 每种作物首茬利润/天 = `(yield × 售价 − 种子买价) / 成长天数` ∈ [3, 15]
//!      （下限 3 是原料豁免：草药 3.3 为制药原料定位，非收入作物）；
//!   3. 无套利：收获物售价 > 种子买价；种子卖价 < 种子买价（种子理论残值如
//!      lingzhi=5 < 买价 25，不构成"买入再卖出"通道），且收获物不进任何商店 stock、
//!      种子不进任何商店 buys（herb.buy=8 是既有"商店参考价"，不进 stock——套利
//!      检查按 stock/buys 集合而不是按 buy 字段是否为 0）；
//!   4. 再生作物恰为 6 种（mint/mulberry/cucumber/gouqi/xueli/evergreen），
//!      `regrow_days` 全部为正整数。
//!
//! 数据口径：docs/33 §4.1 作物表 == scripts/data/crops.lua + data/items.lua
//! （与 `jhyt_m24_numeric_alignment` 同一份数据源）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m36_economy_sim [-- <game_jhyt-root>]

use rustlua_scripting::script::LuaRuntime;
use std::{env, path::PathBuf};

/// 利润/天下限与上限（docs/33 §5.3 经济曲线）。
const PROFIT_MIN: f64 = 3.0;
const PROFIT_MAX: f64 = 15.0;
/// 每季可种作物下限。
const PER_SEASON_MIN: i64 = 6;
/// 再生作物总数（docs/33 §4.1.1）。
const REGROW_COUNT: i64 = 6;

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

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())?;
    runtime.set_project_root(project.clone());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    runtime.execute_project_script(&main_script)?;

    banner("① 作物总数 24，每季可种 ≥6（冬季 ≥6），再生作物 6 种");
    let summary = query_str(
        &mut runtime,
        "local crops = require('data.crops')\n\
         local items = require('data.items')\n\
         local seasons = { spring = 0, summer = 0, autumn = 0, winter = 0 }\n\
         local regrow = 0\n\
         local total = 0\n\
         local violations = {}\n\
         for _, c in pairs(crops) do\n\
           -- 果树（docs/33 §4.1.5，tree=true）不属于四季作物矩阵：不浇水、
           -- 每年一季收成、不走首茬利润锁——经济模拟只统计常规作物。
           if c.tree then goto continue end\n\
           total = total + 1\n\
           for _, s in ipairs(c.season or {}) do seasons[s] = seasons[s] + 1 end\n\
           if c.regrow_days then regrow = regrow + 1 end\n\
           ::continue::\n\
         end\n\
         return string.format('total=%d spring=%d summer=%d autumn=%d winter=%d regrow=%d',\n\
           total, seasons.spring, seasons.summer, seasons.autumn, seasons.winter, regrow)",
    )?;
    println!("  {summary}");
    let parsed = parse_kv(&summary)?;
    if parsed.get("total") != Some(&24.0) {
        return Err(format!("作物总数应为 24，实际 {}", parsed["total"]).into());
    }
    for season in ["spring", "summer", "autumn", "winter"] {
        let count = *parsed
            .get(season)
            .ok_or_else(|| format!("缺 {season} 计数"))? as i64;
        if count < PER_SEASON_MIN {
            return Err(format!(
                "{season} 季可种作物 {count} < {PER_SEASON_MIN}（docs/33 §5.3 每季 ≥6）"
            )
            .into());
        }
    }
    if *parsed.get("regrow").ok_or("缺 regrow 计数")? as i64 != REGROW_COUNT {
        return Err(format!("再生作物应为 {REGROW_COUNT} 种，实际 {}", parsed["regrow"]).into());
    }

    banner("② 逐作物断言：利润/天 ∈ [3,15]、售价 > 买价、种子卖价 < 买价（不构成回卖通道）");
    let lines = query_str(
        &mut runtime,
        "local crops = require('data.crops')\n\
         local items = require('data.items')\n\
         local out = {}\n\
         for id, c in pairs(crops) do\n\
           -- 果树（tree=true）不走首茬利润锁（docs/33 §4.1.5：长线投资、一年一季收成）\n\
           if c.tree then goto skip end\n\
           local seed = items[c.seed_item]\n\
           local hv = items[c.harvest_item]\n\
           local profit = (c.yield * hv.sell - seed.buy) / c.growth_days\n\
           out[#out + 1] = string.format('%s|%.2f|%d|%d|%d|%d',\n\
             id, profit, seed.buy, hv.sell, seed.sell or 0, hv.buy or 0)\n\
           ::skip::\n\
         end\n\
         table.sort(out)\n\
         return table.concat(out, '\\n')",
    )?;
    let mut checked = 0;
    for line in lines.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 6 {
            return Err(format!("作物行格式不符: {line:?}").into());
        }
        let crop_id = parts[0];
        let profit: f64 = parts[1].parse()?;
        let seed_buy: i64 = parts[2].parse()?;
        let harvest_sell: i64 = parts[3].parse()?;
        let seed_sell: i64 = parts[4].parse()?;
        if !(PROFIT_MIN..=PROFIT_MAX).contains(&profit) {
            return Err(format!(
                "{crop_id} 首茬利润/天 = {profit:.2}，应在 [{PROFIT_MIN}, {PROFIT_MAX}]（docs/33 §5.3）"
            )
            .into());
        }
        if harvest_sell <= seed_buy {
            return Err(format!(
                "{crop_id} 无套利约束被破坏：收获物售价 {harvest_sell} <= 种子买价 {seed_buy}"
            )
            .into());
        }
        // 种子允许保留理论残值（seed_lingzhi=5 等既有数据），但残值绝不能 >= 买价，
        // 否则"买种子→卖种子"本身就成了稳定收益（真正的通道由 ②′ 的 buys 集合把关）。
        if seed_sell > 0 && seed_sell >= seed_buy {
            return Err(format!(
                "{crop_id} 种子卖价 {seed_sell} >= 买价 {seed_buy}，买入种子再卖出即可套利"
            )
            .into());
        }
        println!(
            "  {crop_id}: 利润/天={profit:.2} 种子买价={seed_buy} 售价={harvest_sell}（无套利 ✓）"
        );
        checked += 1;
    }
    if checked != 24 {
        return Err(format!("应断言 24 种作物，实际 {checked}").into());
    }

    banner("②′ 无套利（商店不反向买卖）：收获物不进任何 stock、种子不进任何 buys");
    // 套利的真实条件是"商店同时可卖与可收同一物品"；herb.buy=8 是既有商店参考价
    // （不进 stock，docs/33 §5.2 草药转正口径），因此这里按 stock/buys 集合断言，
    // 而不是要求收获物 buy 一律为 0。
    let bad = query_str(
        &mut runtime,
        "local shops = require('data.shops')\n\
         local crops = require('data.crops')\n\
         local items = require('data.items')\n\
         local stock, buys = {}, {}\n\
         for _, s in pairs(shops) do\n\
           if type(s) == 'table' then\n\
             for _, it in ipairs(s.stock or {}) do stock[it.item] = true end\n\
             for _, it in ipairs(s.buys or {}) do buys[it] = true end\n\
           end\n\
         end\n\
         local bad = {}\n\
         for id, c in pairs(crops) do\n\
           local seed = items[c.seed_item]\n\
           if stock[c.harvest_item] then bad[#bad + 1] = id .. ':harvest_in_stock' end\n\
           if buys[c.seed_item] then bad[#bad + 1] = id .. ':seed_in_buys' end\n\
         end\n\
         return table.concat(bad, ',')",
    )?;
    if !bad.is_empty() {
        return Err(format!("存在商店套利通道: {bad:?}").into());
    }
    println!("  全部 24 种作物的种子/收获物在商店买卖方向分离");

    banner("③ 再生作物 regrow_days 全部为正整数（registry_check 之外再锁一遍口径）");
    let bad = query_str(
        &mut runtime,
        "local crops = require('data.crops')\n\
         local bad = {}\n\
         for id, c in pairs(crops) do\n\
           if c.regrow_days and (type(c.regrow_days) ~= 'number' or c.regrow_days < 1 or math.floor(c.regrow_days) ~= c.regrow_days) then\n\
             bad[#bad + 1] = id\n\
           end\n\
         end\n\
         return table.concat(bad, ',')",
    )?;
    if !bad.is_empty() {
        return Err(format!("非法 regrow_days 的作物: {bad:?}").into());
    }
    println!("  全部再生作物 regrow_days 合法");

    println!(
        "jhyt_m36_economy_sim_ok project={} crops=24 checked={checked} profit=[{PROFIT_MIN},{PROFIT_MAX}]",
        project.display()
    );
    Ok(())
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}

/// 解析 `k=v k2=v2 ...` 形式的摘要行。
fn parse_kv(
    line: &str,
) -> Result<std::collections::HashMap<String, f64>, Box<dyn std::error::Error>> {
    let mut map = std::collections::HashMap::new();
    for token in line.split_whitespace() {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| format!("token {token:?} 不是 k=v"))?;
        map.insert(key.to_owned(), value.parse()?);
    }
    Ok(map)
}

fn query_str(runtime: &mut LuaRuntime, block: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(block)?.trim().to_owned())
}
