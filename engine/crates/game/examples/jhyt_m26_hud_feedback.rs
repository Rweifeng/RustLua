//! Headless acceptance drive for M26 / 计划 G6：HUD 反馈闭环
//! （`docs/37` 4-13 —— 缺失文案、前缀误命中、未知值不再裸露英文键）。
//!
//! HUD 左下角只显示一行 `state.last_action` 的中文反馈。此前它有三类问题：
//!
//! 1. **缺条目**：`missing_map`、`gather_blocked`、`unknown_recipe`、`unknown_shop`、
//!    `no_station`、`station_level`、`busy`、`no_job`、`gathered` … 一批取值没有中文，
//!    玩家直接看到英文键。
//! 2. **前缀误命中**：`select_seed_lingzhi`（选种子）会被 `select_`（已选建筑）吃掉，
//!    显示成「已选建筑：灵芝」。
//! 3. **未知值原样返回**：表里没有的取值直接把英文键摆给玩家。
//!
//! 本样例用"扫描源码 + 断言覆盖"的方式守住这三条：任何未来新增的 `state.last_action`
//! 取值或 reason 词，只要没有中文映射就会失败——而不是等玩家看见一个英文单词。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m26_hud_feedback [-- <game_jhyt-root>]

use engine::core::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 会通过变量写进 `state.last_action` 的模块（它们 `return false, "<reason>"` /
/// `reason = "<key>"` 里的取值就是玩家可能看到的反馈）。
const REASON_SOURCES: &[&str] = &[
    "systems/interaction.lua",
    "systems/farm.lua",
    "systems/gather.lua",
    "systems/crafting.lua",
    "systems/shop.lua",
    "systems/building.lua",
    "systems/combat.lua",
    "player/relationship.lua",
    "player/inventory.lua",
    "ui/inventory_ui.lua",
    "ui/tutorial.lua",
    "world/map_manager.lua",
    "world/quest.lua",
    "world/dialogue.lua",
];

/// 内部状态名：出现在 `return` 里但**不会**进 `state.last_action`（调用方自己消费）。
const INTERNAL_KEYS: &[&str] = &["ok", "available", "move", "none", "true", "false"];

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
    let main_script = scripts.join("main.lua");

    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.clone());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(720, 1280);
    runtime.set_delta(1.0 / 60.0);
    runtime.execute_project_script(&main_script)?;

    banner("① 扫描 scripts/**：每个 state.last_action 字面量都必须有中文反馈");
    let files = collect_lua_files(&scripts)?;
    let mut literals: Vec<(String, String)> = Vec::new(); // (值, 文件)
    let mut prefixes: Vec<(String, String)> = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file)?;
        let relative = file
            .strip_prefix(&project)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        for (value, prefix) in scan_last_action(&source) {
            match prefix {
                Some(prefix) => prefixes.push((prefix, relative.clone())),
                None => literals.push((value, relative.clone())),
            }
        }
    }
    literals.sort();
    literals.dedup();
    prefixes.sort();
    prefixes.dedup();
    if literals.is_empty() {
        return Err("没有扫到任何 state.last_action 字面量（扫描规则失效？）".into());
    }
    for (value, file) in &literals {
        assert_feedback_mapped(&mut runtime, value, &format!("{file} 的 last_action"))?;
    }
    println!(
        "  字面量 {} 个：{}",
        literals.len(),
        literals
            .iter()
            .map(|(value, _)| value.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    banner("② 扫描动态前缀（`\"prefix_\" .. x`）：前缀必须已登记，否则只剩英文 id");
    if prefixes.is_empty() {
        return Err("没有扫到任何动态 last_action 前缀".into());
    }
    for (prefix, file) in &prefixes {
        let probe = format!("{prefix}probe_item");
        let text = query_str(
            &mut runtime,
            &format!("require('ui.hud').feedback('{probe}')"),
        )?;
        if text == probe || !has_cjk(&text) {
            return Err(format!(
                "{file} 的 last_action 前缀 {prefix:?} 没有中文映射：feedback({probe:?}) = {text:?}"
            )
            .into());
        }
    }
    println!(
        "  前缀 {} 个：{}",
        prefixes.len(),
        prefixes
            .iter()
            .map(|(prefix, _)| prefix.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    banner("③ reason 词表：会经变量写进 last_action 的取值也必须全部有中文");
    let mut reasons: Vec<(String, String)> = Vec::new();
    for relative in REASON_SOURCES {
        let path = scripts.join(relative);
        if !path.is_file() {
            return Err(format!("reason 来源模块不存在：{relative}（模块被移动/改名？）").into());
        }
        let source = fs::read_to_string(&path)?;
        for key in scan_reason_keys(&source) {
            if INTERNAL_KEYS.contains(&key.as_str()) {
                continue;
            }
            reasons.push((key, relative.to_string()));
        }
    }
    reasons.sort();
    reasons.dedup();
    if reasons.len() < 20 {
        return Err(format!(
            "只扫到 {} 个 reason 词，期望 ≥20（扫描规则或模块列表失效？）",
            reasons.len()
        )
        .into());
    }
    for (key, file) in &reasons {
        assert_feedback_mapped(&mut runtime, key, &format!("{file} 的 reason"))?;
    }
    println!("  reason 词 {} 个，全部有中文映射", reasons.len());

    banner("④ 前缀优先级：选种子不能被「已选建筑」吃掉，exit_ 要出地图名");
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback('select_seed_lingzhi')",
        "已选种子：灵芝",
    )?;
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback('select_seed_rice')",
        "已选种子：稻谷",
    )?;
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback('equip_watering_can')",
        "已装备：水壶",
    )?;
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback('gather_timber')",
        "已采集：木料",
    )?;
    // `exit_<map>` 用地图短名（`data/maps.lua` 的 `title` 取 `·` 之前一段），不再是 map id。
    let manor_title = query_str(&mut runtime, "require('data.maps').get('manor').title")?;
    let manor_short = manor_title
        .split('·')
        .next()
        .unwrap_or(&manor_title)
        .trim()
        .to_owned();
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback('exit_manor')",
        &format!("前往：{manor_short}"),
    )?;
    let exit_text = format!("前往：{manor_short}");
    println!("  exit_ → {exit_text}");

    banner("⑤ 未知值兜底：不再裸露英文键，但保留原值便于诊断");
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback('totally_unknown_action')",
        "未知反馈（totally_unknown_action）",
    )?;
    assert_str(&mut runtime, "require('ui.hud').feedback('')", "")?;
    // 未知值经 feedback_line 渲染时同样带中文标签。
    lua(
        &mut runtime,
        "require('core.state').last_action = 'totally_unknown_action'",
    )?;
    let line = query_str(&mut runtime, "require('ui.hud').feedback_line()")?;
    if !line.contains("未知反馈（totally_unknown_action）") {
        return Err(format!("feedback_line 对未知值没有兜底文案: {line:?}").into());
    }

    banner("⑥ 真实玩法路径：实际动作的 last_action 经 HUD 出中文");
    lua(&mut runtime, "scene.load('manor')")?;
    runtime.process_scene_transition()?;
    lua(&mut runtime, "require('core.state').facing = 'up'")?;
    press(&mut runtime, 'e')?;
    assert_feedback_chinese(&mut runtime, "翻土")?;
    lua(
        &mut runtime,
        "require('core.state').selected_seed = 'seed_lingzhi'",
    )?;
    press(&mut runtime, 'e')?;
    assert_feedback_chinese(&mut runtime, "播种")?;
    // 钉住晴天（M29 起天气按 `(seed, day, 季节)` 抽签）：雨天浇水的反馈文案是
    // 「雨水浇灌」（`rain_watered`），本节要验的是「浇水」这条映射。
    lua(&mut runtime, "require('world.weather').set('sunny')")?;
    lua(
        &mut runtime,
        "inv.equip(require('core.state').inventory, 'tool', 'watering_can')",
    )?;
    press(&mut runtime, 'e')?;
    assert_feedback_chinese(&mut runtime, "浇水")?;
    // 体力耗尽 → tired 的中文反馈（换个没浇过水的田块，走真实劳作路径：
    // 已浇过水的地块会先返回 already_watered，测不到体力检查）。
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
    let (fx, fy) = field_cell
        .split_once(',')
        .ok_or_else(|| format!("bad cell {field_cell:?}"))?;
    let (fx, fy): (i64, i64) = (fx.trim().parse()?, fy.trim().parse()?);
    lua(
        &mut runtime,
        &format!(
            "require('player.player').teleport({}, {})",
            (fx as f64 + 0.5) * 32.0,
            (fy as f64 + 1.5) * 32.0
        ),
    )?;
    lua(&mut runtime, "require('core.state').facing = 'up'")?;
    lua(
        &mut runtime,
        "inv.equip(require('core.state').inventory, 'tool', 'hoe')",
    )?;
    lua(&mut runtime, "require('core.state').energy = 0")?;
    press(&mut runtime, 'e')?;
    assert_str(&mut runtime, "require('core.state').last_action", "tired")?;
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback(require('core.state').last_action)",
        "体力不足",
    )?;

    println!(
        "jhyt_m26_hud_feedback_ok project={} literals={} prefixes={} reasons={}",
        project.display(),
        literals.len(),
        prefixes.len(),
        reasons.len()
    );
    Ok(())
}

// ---------- scanning ----------

fn collect_lua_files(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    collect_lua_files_into(dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_lua_files_into(
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_lua_files_into(&path, out)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some("lua") {
            out.push(path);
        }
    }
    Ok(())
}

/// 从源码里取出 `state.last_action = <...>` 的取值：
/// 返回 (字面量, Some(前缀)) —— 字面量赋值给第一个元素，动态拼接给第二个。
fn scan_last_action(source: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        // 跳过注释行：`-- state.last_action = "till"` 这类说明不该被当成赋值。
        if line.starts_with("--") {
            continue;
        }
        let Some(index) = line.find("last_action") else {
            continue;
        };
        let rest = &line[index..];
        let Some(eq) = rest.find('=') else {
            continue;
        };
        // 排除 `==` 比较与 `M.last_action = M.last_action` 这类自赋值。
        let after = rest[eq + 1..].trim();
        if after.starts_with('=') {
            continue;
        }
        if !after.starts_with('"') {
            continue;
        }
        let Some(end) = after[1..].find('"') else {
            continue;
        };
        let value = &after[1..1 + end];
        if value.is_empty() {
            continue;
        }
        // 动态拼接：`"plant_" .. seed_id`（含前缀 + 变量）。
        if after[end + 2..].trim_start().starts_with("..") {
            out.push((value.to_owned(), Some(value.to_owned())));
        } else {
            out.push((value.to_owned(), None));
        }
    }
    out
}

/// 从源码里取出会被当作 reason 返回的字符串键：
/// `return false, "x"` / `return nil, "x"` / `reason = "x"` / `return "x", "y"`。
fn scan_reason_keys(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        if line.starts_with("--") {
            continue;
        }
        for (marker, offset) in [
            ("return false, \"", 15),
            ("return nil, \"", 13),
            ("reason = \"", 10),
        ] {
            let mut search = line;
            while let Some(index) = search.find(marker) {
                let start = index + offset;
                let candidate = &search[start..];
                if let Some(end) = candidate.find('"') {
                    let key = &candidate[..end];
                    if is_key_like(key) {
                        out.push(key.to_owned());
                    }
                }
                search = &search[start..];
            }
        }
    }
    out
}

/// 只接受 `^[a-z][a-z0-9_]*$` 形式的键（中文/提示语/路径都不算 reason 键）。
fn is_key_like(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn has_cjk(text: &str) -> bool {
    text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

// ---------- assertions ----------

/// 断言某个取值在 HUD 里有**中文**反馈（不是原样返回英文键）。
fn assert_feedback_mapped(
    runtime: &mut LuaRuntime,
    value: &str,
    origin: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let text = query_str(runtime, &format!("require('ui.hud').feedback('{value}')"))?;
    if text == value {
        return Err(format!("{origin}: {value:?} 没有中文映射（HUD 会显示英文键）").into());
    }
    if !has_cjk(&text) {
        return Err(format!("{origin}: {value:?} 的反馈缺失中文: {text:?}").into());
    }
    Ok(())
}

fn assert_feedback_chinese(
    runtime: &mut LuaRuntime,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let action = query_str(runtime, "require('core.state').last_action")?;
    let text = query_str(
        runtime,
        "require('ui.hud').feedback(require('core.state').last_action)",
    )?;
    if !has_cjk(&text) || text == action {
        return Err(format!("{label} 后 last_action={action:?} 的反馈不是中文: {text:?}").into());
    }
    Ok(())
}

fn query_str(runtime: &mut LuaRuntime, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(expr)?.trim().to_owned())
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

fn lua(runtime: &mut LuaRuntime, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(code)?;
    Ok(())
}

fn press(runtime: &mut LuaRuntime, key: char) -> Result<(), Box<dyn std::error::Error>> {
    use engine::core::input::{InputSnapshot, KeyCode};
    let mut input = InputSnapshot::default();
    input.key_event(
        KeyCode::from_char(key).ok_or_else(|| format!("字符 {key:?} 没有对应物理键"))?,
        true,
    );
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}
