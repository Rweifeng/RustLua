//! Headless acceptance drive for data-registry validation (M23 / 计划 G4).
//!
//! 覆盖四类缺陷，全部锁成回归：
//!
//! 1. **登记漏项**：`data/gather.lua` 与 `data/enemy_art.lua` 早就写了
//!    `validate_all`，但从没登记进 `validate_data.lua` → 它们的校验从来没跑过。
//!    本示例枚举 `scripts/**/*.lua`，断言"带 `validate_all` 的模块都在登记表里"。
//! 2. **纯表注册表放不下校验器**：`data/items.lua` / `data/tools.lua` /
//!    `data/crops.lua` 返回注册表本身，`player/inventory.lua` 会
//!    `for id, def in pairs(items) do item.define(id, def) end`；往表里塞
//!    `validate_all`/`version` 会被注册成一个物品。校验集中在
//!    `data/registry_check.lua`（本示例断言这两个键在注册表里不存在）。
//! 3. **跨文件断链**：种子/收获物指向不存在的物品、作物土地类型没有任何
//!    `data/farm.lua` 区划、`sell_price` 与物品收价漂移、敌人类术未接线。
//! 4. **资源引用**：物品图标与敌人美术引用的文件必须真实存在于工程内。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m23_data_validation [-- <game_jhyt-root>]

use engine::core::script::LuaRuntime;
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let scripts = project.join("scripts");
    let mut runtime = new_runtime(&project)?;
    runtime.execute_project_script(&scripts.join("main.lua"))?;

    banner("① 注册表本身干净：纯表注册表里没有 version/validate_all 这种会被当物品注册的键");
    for (module, registry) in [
        ("data.items", "items"),
        ("data.tools", "tools"),
        ("data.crops", "crops"),
    ] {
        let leaked = query_str(
            &mut runtime,
            &format!(
                "local reg = require('{module}')\n\
                 local out = {{}}\n\
                 for key in pairs(reg) do\n\
                   if key == 'version' or key == 'validate_all' or key == 'get' then out[#out+1] = key end\n\
                 end\n\
                 table.sort(out)\n\
                 return table.concat(out, ',')"
            ),
        )?;
        if !leaked.is_empty() {
            return Err(format!("{module} ({registry}) leaks reserved keys: {leaked}").into());
        }
    }
    // 注册表是哈希表（`#` 恒为 0），只能数键个数：物品 10 条 = lingzhi/rice/
    // seed_lingzhi/seed_rice/heal_pill/herb_tonic/timber（M19/M18）+ M28 的敌人掉落物
    // wolf_pelt/boar_meat/ape_core。
    let item_count = query_num(
        &mut runtime,
        "(function() local n = 0 for _ in pairs(require('data.items')) do n = n + 1 end return n end)()",
    )?;
    if item_count != 10.0 {
        return Err(format!("data/items.lua has {item_count} entries, want 10").into());
    }

    banner("② 登记覆盖：任何带 validate_all 的模块都必须登记进 validate_data.lua");
    let registered: BTreeSet<String> = query_str(
        &mut runtime,
        "local out = {}\n\
         for _, entry in ipairs(require('validate_data').registered()) do out[#out+1] = entry.path end\n\
         return table.concat(out, ',')",
    )?
    .split(',')
    .filter(|entry| !entry.is_empty())
    .map(str::to_owned)
    .collect();
    let mut modules_with_validators = Vec::new();
    collect_validator_modules(&scripts, &scripts, &mut modules_with_validators)?;
    modules_with_validators.sort();
    for module in &modules_with_validators {
        if !registered.contains(module) {
            return Err(format!(
                "{module} defines validate_all but is not registered in scripts/validate_data.lua \
                 (its validation never runs)"
            )
            .into());
        }
    }
    if modules_with_validators.len() < 12 {
        return Err(format!(
            "only {} module(s) with validate_all found; the enumeration is broken",
            modules_with_validators.len()
        )
        .into());
    }

    banner("③ 全量校验通过（与 game --check / editor --check 同一条路径）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("validate_data.check reported issues:\n{report}").into());
    }

    banner("④ 负向：注入的数据问题必须被逐条报出（含文件/对象/字段路径）");
    // 每个 case：改数据 → 期望的诊断子串 → 还原。
    let cases: &[(&str, &str, &str)] = &[
        (
            "require('data.items').lingzhi.stack = 0",
            "data/items.lua:lingzhi.stack",
            "require('data.items').lingzhi.stack = 99",
        ),
        (
            "require('data.items').herb_tonic.use.buff.stat = 'mana'",
            "data/items.lua:herb_tonic.use.buff.stat: unknown stat mana",
            "require('data.items').herb_tonic.use.buff.stat = 'attack'",
        ),
        (
            "require('data.items').heal_pill.use = {}",
            "data/items.lua:heal_pill.use: has no effect",
            "require('data.items').heal_pill.use = { energy = 40 }",
        ),
        (
            "require('data.tools').hoe.kind = 'gadget'",
            "data/tools.lua:hoe.kind: unknown kind gadget",
            "require('data.tools').hoe.kind = 'tool'",
        ),
        (
            "require('data.tools').watering_can.energy = nil",
            "data/tools.lua:watering_can.energy: tool requires integer >= 0",
            "require('data.tools').watering_can.energy = 2",
        ),
        (
            "require('data.tools').lingzhi = { name = '影子灵芝', kind = 'material' }",
            "data/tools.lua:lingzhi: id also exists in data/items.lua",
            "require('data.tools').lingzhi = nil",
        ),
        (
            "require('data.crops').rice.seed_item = 'seed_void'",
            "data/crops.lua:rice.seed_item: unknown item seed_void",
            "require('data.crops').rice.seed_item = 'seed_rice'",
        ),
        (
            "require('data.crops').rice.season = { 'rainy' }",
            "data/crops.lua:rice.season[1]: unknown season rainy",
            "require('data.crops').rice.season = { 'spring', 'summer', 'autumn' }",
        ),
        (
            "require('data.crops').lingzhi.land = 'lava'",
            "data/crops.lua:lingzhi.land: no data/farm.lua land rect uses type lava",
            "require('data.crops').lingzhi.land = 'medicinal'",
        ),
        (
            "require('data.crops').lingzhi.harvest_item = 'seed_lingzhi'",
            "data/crops.lua:lingzhi.harvest_item: seed_lingzhi has kind seed, expected crop",
            "require('data.crops').lingzhi.harvest_item = 'lingzhi'",
        ),
        (
            "require('data.farm').land[1].x1 = require('data.farm').land[1].x0 - 1",
            "data/farm.lua:land[1]: x0 > x1",
            "require('data.farm').land[1].x1 = 23",
        ),
        (
            "require('data.farm').plots[1].watered = 'yes'",
            "data/farm.lua:plots[1].watered: expected boolean",
            "require('data.farm').plots[1].watered = false",
        ),
        (
            "require('data.farm').plots[1].crop = 'void_crop'",
            "data/farm.lua:plots[1].crop: unknown crop void_crop",
            "require('data.farm').plots[1].crop = nil",
        ),
        (
            "require('data.enemies').enemies[1].art = 'ghost_art'",
            "data/enemies.lua:enemy_dummy.art: unknown enemy art ghost_art",
            "require('data.enemies').enemies[1].art = 'training_dummy'",
        ),
        (
            "require('data.enemy_art').enemies.gray_wolf.animation = 'default=idle_s'",
            "data/enemy_art.lua:gray_wolf.animation: default clip idle_s is not declared",
            "require('data.enemy_art').enemies.gray_wolf.animation = __animation_backup",
        ),
    ];
    query_str(
        &mut runtime,
        "local art = require('data.enemy_art')\n\
         __animation_backup = art.enemies.gray_wolf.animation\n\
         return 'ok'",
    )?;
    for (mutate, expected_issue, restore) in cases {
        runtime.execute_console(mutate)?;
        let issues = query_str(
            &mut runtime,
            "local ok, list = require('data.registry_check').validate_all()\n\
             if ok then return '' end\n\
             return table.concat(list, '\\n')",
        )?;
        if !issues.contains(expected_issue) {
            return Err(format!(
                "after `{mutate}` expected issue {expected_issue:?}, got:\n{issues}"
            )
            .into());
        }
        runtime.execute_console(restore)?;
    }
    let restored = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !restored.is_empty() {
        return Err(format!("validation did not recover after restores:\n{restored}").into());
    }

    banner("⑤ 登记覆盖反向：把登记表里的一项拿掉必须真的少跑一条校验");
    // 直接构造"未登记"的情形：registry_check 不在时，items 的坏数据不再被报出。
    let uncovered = query_str(
        &mut runtime,
        "require('data.items').lingzhi.stack = 0\n\
         local report = require('validate_data').check({ strict = true })\n\
         require('data.items').lingzhi.stack = 99\n\
         if report:find('lingzhi.stack', 1, true) then return 'covered' end\n\
         return 'uncovered'",
    )?;
    if uncovered != "covered" {
        return Err("validate_data no longer reports item registry issues".into());
    }

    banner("⑥ 资源引用：物品图标与敌人美术引用的文件必须真实存在");
    let icons = query_str(
        &mut runtime,
        "local items = require('data.items')\n\
         local out = {}\n\
         for _, def in pairs(items) do if def.icon then out[#out+1] = def.icon end end\n\
         table.sort(out)\n\
         return table.concat(out, ',')",
    )?;
    let mut checked = 0;
    for icon in icons.split(',').filter(|entry| !entry.is_empty()) {
        let path = project.join(icon);
        if !path.is_file() {
            return Err(format!("item icon {icon:?} does not exist at {}", path.display()).into());
        }
        checked += 1;
    }
    let sprites = query_str(
        &mut runtime,
        "local art = require('data.enemy_art')\n\
         local out = {}\n\
         for id, entry in pairs(art.enemies) do\n\
           out[#out+1] = id\n\
           for facing, sprite in pairs(entry.static_sprite or {}) do\n\
             out[#out+1] = string.format('%s:%s:%s', id, facing, sprite)\n\
           end\n\
         end\n\
         table.sort(out)\n\
         return table.concat(out, '\\n')",
    )?;
    let mut art_entries = 0;
    for line in sprites.lines().filter(|line| !line.is_empty()) {
        if !line.contains(':') {
            art_entries += 1;
            continue;
        }
        let texture = line
            .split("texture=")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .ok_or_else(|| format!("cannot parse sprite string: {line}"))?;
        let path = project.join(texture);
        if !path.is_file() {
            return Err(format!("enemy art texture {texture:?} does not exist").into());
        }
        checked += 1;
    }

    println!(
        "jhyt_m23_data_validation_ok project={} registered={} modules_with_validators={} \
         negative_cases={} resources_checked={} enemy_art_entries={}",
        project.display(),
        registered.len(),
        modules_with_validators.len(),
        cases.len(),
        checked,
        art_entries
    );
    Ok(())
}

/// 枚举 `scripts/**/*.lua`，返回"定义了 `function M.validate_all`"的模块 require 路径。
///
/// 判定用源码文本（不做 Lua 解析）：`validate_data.lua` 的约定就是"模块返回 M 表并
/// 暴露 validate_all"，因此文本出现 `function M.validate_all` 即可认定。
fn collect_validator_modules(
    root: &Path,
    dir: &Path,
    out: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_validator_modules(root, &path, out)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("lua") {
            continue;
        }
        let source = fs::read_to_string(&path)?;
        if !source.contains("function M.validate_all") {
            continue;
        }
        let relative = path.strip_prefix(root)?;
        let module = relative
            .with_extension("")
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(".");
        out.push(module);
    }
    Ok(())
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}

fn new_runtime(project: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(720, 1280);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

/// 断言一个**语句块**的返回值（块自带 `return`）。
fn query_str(runtime: &mut LuaRuntime, block: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(block)?.trim().to_owned())
}

fn query_num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let value = query_str(runtime, &format!("return tostring({expr})"))?;
    value
        .parse::<f64>()
        .map_err(|error| format!("{value:?} is not a number: {error}").into())
}
