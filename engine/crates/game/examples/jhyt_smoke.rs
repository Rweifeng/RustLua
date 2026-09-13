//! Headless acceptance drive for the `game_jhyt`（《江湖有田》）project.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` (no
//! window / no host `RuntimeScene`), then reproduces the M16 acceptance paths:
//!
//! - M16-01: gameplay context state machine — movement / E / Q / T are ignored
//!   while the game is not in `Playing` mode (Pause / Inventory / Transition).
//! - M16-02: E reaches farm labour through `interaction.interact`, with clean
//!   reasons for unreachable farmland.
//! - M16-03: till -> plant -> water -> already_watered -> rain_watered -> daily
//!   growth across day sleeps -> 7-day lingzhi maturity -> harvest. Also proves
//!   plots are persisted under crop short ids (regression guard for the fix that
//!   previously stored `seed_item`).
//! - M16-04: full save round-trip in a *second* runtime via `on_click('BtnContinue')`
//!   (state/inventory/clock/farm/scene restored), plus `validate_long_run(30, 100)`.
//!
//! Driven by `InputSnapshot` edge frames the same way `farm_smoke.rs` does:
//! down-frame + clean-frame per key. `day.sleep()` is the only clock advance,
//! so nothing can accidentally roll past midnight while setting up.
//!
//! Usage:
//!   cargo run -p game --example jhyt_smoke [-- <game_jhyt-root>]

use engine::core::{
    input::{InputKey, InputSnapshot},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// Tile size from `main.scene` meta.
const TILE: f32 = 32.0;
/// 《江湖有田》药田/稻田矩形（mirrors farm.lua FARM / data/farm.lua land）。
/// X: 药田 14..23，稻田 24..33；Y: 4..13（0-index，Y 向上）。
const FARM: (i32, i32, i32, i32) = (14, 33, 4, 13);
/// 灵芝成长期数（scripts/data/crops.lua）。
const LINGZHI_GROWTH: i64 = 7;

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
    let save_path = fresh_save_path(&project);
    let project_arg = project.clone();

    // 第一步：真实「开始新游戏」流程 —— 标题按钮 BtnStart -> 山庄 on_create。
    let mut runtime = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut runtime, &main_script)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;

    banner("M16-01 输入上下文状态机：非 Playing 抑制");
    // 起步能量 100、内力 50。
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    assert_num(&mut runtime, "require('core.state').inner_power", 50.0)?;
    // Q 吐纳 +5 内力。
    press(&mut runtime, InputKey::from_char('q'))?;
    assert_num(&mut runtime, "require('core.state').inner_power", 55.0)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "practice_breath",
    )?;

    // Pause：E/Q/T 均被屏蔽。
    press(&mut runtime, InputKey::from_char('p'))?; // -> Pause
    assert_str(&mut runtime, "require('core.state').mode", "Pause")?;
    press(&mut runtime, InputKey::from_char('q'))?;
    press(&mut runtime, InputKey::from_char('e'))?;
    press(&mut runtime, InputKey::from_char('t'))?;
    assert_num(&mut runtime, "require('core.state').inner_power", 55.0)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "practice_breath",
    )?;
    assert_day(&mut runtime, 1)?;
    press(&mut runtime, InputKey::from_char('p'))?; // -> Playing

    // Inventory：E/Q/T 均被屏蔽。
    press(&mut runtime, InputKey::from_char('i'))?; // -> Inventory
    assert_str(&mut runtime, "require('core.state').mode", "Inventory")?;
    press(&mut runtime, InputKey::from_char('q'))?;
    press(&mut runtime, InputKey::from_char('e'))?;
    press(&mut runtime, InputKey::from_char('t'))?;
    assert_num(&mut runtime, "require('core.state').inner_power", 55.0)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "practice_breath",
    )?;
    assert_day(&mut runtime, 1)?;
    press(&mut runtime, InputKey::from_char('i'))?; // -> Playing

    // Transition：E/Q/T 均被屏蔽（模拟切场景瞬间）。
    set_mode(&mut runtime, "Transition")?;
    press(&mut runtime, InputKey::from_char('q'))?;
    press(&mut runtime, InputKey::from_char('e'))?;
    press(&mut runtime, InputKey::from_char('t'))?;
    assert_num(&mut runtime, "require('core.state').inner_power", 55.0)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "practice_breath",
    )?;
    assert_day(&mut runtime, 1)?;
    set_mode(&mut runtime, "Playing")?;

    banner("M16-02 交互框架：E 定位农田并在其面前劳作");
    // 玩家出生在 (528,112) → 格 (16,3)。朝上（state.facing = "up"）面前即
    // (16,4)，落在药田矩形 x14..23/y4..13 内 —— 与文档"种灵芝朝上"一致。
    set_facing(&mut runtime, "up")?;
    let (_, _, ax, ay) = facing_cell_above(528.0, 112.0)?;
    assert!(in_farm(ax, ay), "起步上方必须是农田格 (got {ax},{ay})");
    // 耕作前需装备锄头（inventory.setup 已装 hoe）。
    press(&mut runtime, InputKey::from_char('e'))?; // 翻土
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    assert_num(&mut runtime, "require('core.state').energy", 92.0)?; // 翻土消耗 8 (100->92)

    banner("M16-03 农田规则：翻土→播种→浇水→成长→收获");
    // 选定灵芝并确保种在药田（当前朝上在药田行上）。
    set_seed(&mut runtime, "seed_lingzhi")?;
    press(&mut runtime, InputKey::from_char('e'))?; // 播种
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_lingzhi",
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 89.0)?; // 播种消耗 3 (92->89)

    // 种在药田格上的灵芝，地块应记录 short id "lingzhi"（而非 seed_item）。
    let stored = query_str(
        &mut runtime,
        &format!(
            "local m = map.state('farm_plots'); local p = m['{},{}']; return p and p.crop or 'nil'",
            ax, ay
        ),
    )?;
    assert_eq!(stored, "lingzhi", "地块作物应存 short id");

    // 浇水（需要浇水壶）。
    equip_tool(&mut runtime, "watering_can")?;
    press(&mut runtime, InputKey::from_char('e'))?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "water_lingzhi",
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 85.0)?; // 浇水消耗 4 (89->85)
    // 同日重复浇水 → already_watered 且不扣体力。
    press(&mut runtime, InputKey::from_char('e'))?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "already_watered",
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 85.0)?;

    // 睡到第 2 天清晨：settle_day 把昨日的 watered_today 记为成长 +1，
    // 同时 on_day_start 把体力重置回 100。
    sleep(&mut runtime)?; // -> day 2
    assert_day(&mut runtime, 2)?;
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    let watered_days = query_num(
        &mut runtime,
        &format!(
            "local p = map.state('farm_plots')['{},{}']; return tostring((p and p.watered_days) or -1)",
            ax, ay
        ),
    )?;
    assert_eq!(watered_days, 1.0, "day2 watered_days 应为 1");

    // 雨天验证：设置 rainy，未成熟作物按下 E 应免费 rain_watered 且不耗体力。
    // （灌溉当天不计成长 —— 成长永远发生在“浇水次日”的日结算。）
    set_weather(&mut runtime, "rainy")?;
    press(&mut runtime, InputKey::from_char('e'))?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "rain_watered",
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;

    // 灵芝 7 天成熟 = 7 次“当日已浇”跨夜。day2 已浇，还需浇 day3..day7 并
    // 睡过 day2→day8 共 6 晚，day8 清晨 watered_days 才到 7。
    for _ in 0..5 {
        sleep(&mut runtime)?; // -> day3..day7，各 +1 成长
        press(&mut runtime, InputKey::from_char('e'))?; // 当天再浇（雨天免费）
    }
    assert_day(&mut runtime, 7)?;
    let watered_days = query_num(
        &mut runtime,
        &format!(
            "local p = map.state('farm_plots')['{},{}']; return tostring((p and p.watered_days) or -1)",
            ax, ay
        ),
    )?;
    assert_eq!(watered_days, 6.0, "day7 watered_days 应为 6（未成熟）");

    // 最后一晚：day7 已浇 → day8 清晨 watered_days = 7 → 成熟。
    sleep(&mut runtime)?;
    assert_day(&mut runtime, 8)?;
    let mature = query_str(
        &mut runtime,
        &format!(
            "local p = map.state('farm_plots')['{},{}']; return p and p.watered_days >= {} and 'true' or 'false'",
            ax, ay, LINGZHI_GROWTH
        ),
    )?;
    assert_eq!(mature, "true", "day8 灵芝应成熟（watered_days>=7）");

    // 换上锄头收获（day8 清晨体力 100，收获消耗 2）。
    equip_tool(&mut runtime, "hoe")?;
    press(&mut runtime, InputKey::from_char('e'))?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "harvest_lingzhi",
    )?;
    let lingzhi = query_num(&mut runtime, "inv.count('player', 'lingzhi')")?;
    assert_eq!(lingzhi, 1.0, "收获 1 灵芝");
    assert_num(&mut runtime, "require('core.state').energy", 98.0)?;
    // 收获后地块恢复空地（crop=nil）。
    let after_harvest = query_str(
        &mut runtime,
        &format!(
            "local m = map.state('farm_plots'); local p = m['{},{}']; return p and p.crop or 'nil'",
            ax, ay
        ),
    )?;
    assert_eq!(after_harvest, "nil", "收获后地块应为空地");

    banner("M16-04 存档：第二 runtime 通过 BtnContinue 读档恢复");
    // 当前是 day8、灵芝已收获。显式存档一次，作为 continue 档基座。
    save_game(&mut runtime)?;

    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_scene_is(&mut second, "manor")?;
    // 读档恢复 day（应为存档的 day8）与内力/金钱。
    assert_day(&mut second, 8)?;
    assert_num(&mut second, "require('core.state').inner_power", 55.0)?;
    assert_num(&mut second, "require('core.state').money", 120.0)?;
    // 读档后农田格仍在，且已是空地（灵芝已收）。
    let restored = query_str(
        &mut second,
        &format!(
            "local m = map.state('farm_plots'); local p = m['{},{}']; return p and p.crop or 'nil'",
            ax, ay
        ),
    )?;
    assert_eq!(restored, "nil", "读档后灵芝格应保持空地（已收获）");

    // validate_long_run: 纯 Lua 快照稳定性。
    let ok = query_str(
        &mut second,
        "return tostring(select(1, require('core.save_manager').validate_long_run(30, 100)))",
    )?;
    assert_eq!(ok, "true", "validate_long_run(30,100) 应成功");

    println!(
        "jhyt_smoke_ok project={} day={} inner_power={} lingzhi={} energy_after={}",
        project.display(),
        8,
        55,
        lingzhi as i64,
        query_num(&mut second, "require('core.state').energy")? as i64
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
/// are live. `execute_project_script` already fired `on_create` while
/// `scene.reload()` was `nil` (the title branch of `main.lua`); switching into
/// the `title` scene makes `on_click` dispatch valid.
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

fn query_i64(runtime: &mut LuaRuntime, expr: &str) -> Result<i64, Box<dyn std::error::Error>> {
    Ok(query_num(runtime, expr)? as i64)
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
    let got = query_str(runtime, &format!("return tostring({expr})"))?;
    let value: f64 = got.parse().map_err(|e| format!("{expr} = {got:?}: {e}"))?;
    if (value - expected).abs() > f64::EPSILON {
        return Err(format!("{expr} = {got:?}, want {expected}").into());
    }
    Ok(())
}

fn assert_day(runtime: &mut LuaRuntime, expected: i64) -> Result<(), Box<dyn std::error::Error>> {
    let day = query_i64(runtime, "clock.now().day")?;
    if day != expected {
        return Err(format!("clock.now().day = {day}, want {expected}").into());
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

fn set_mode(runtime: &mut LuaRuntime, mode: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('core.state').set_mode('{mode}')"))?;
    Ok(())
}

fn set_facing(runtime: &mut LuaRuntime, facing: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('core.state').facing = '{facing}'"))?;
    Ok(())
}

fn set_seed(runtime: &mut LuaRuntime, seed: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('core.state').selected_seed = '{seed}'"))?;
    Ok(())
}

fn equip_tool(runtime: &mut LuaRuntime, tool: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "inv.equip(require('core.state').inventory, 'tool', '{tool}')"
    ))?;
    Ok(())
}

fn set_weather(runtime: &mut LuaRuntime, weather: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('world.weather').set('{weather}')"))?;
    Ok(())
}

fn sleep(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console("return day.sleep()")?;
    Ok(())
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

/// Computes the cell directly above the player's starting position, returning
/// the ground truth world position and the target farm cell `(px, py, cx, cy)`.
fn facing_cell_above(px: f32, py: f32) -> Result<(f32, f32, i32, i32), Box<dyn std::error::Error>> {
    let (cx, cy) = ((px / TILE).floor() as i32, (py / TILE).floor() as i32);
    Ok((px, py, cx, cy + 1))
}

fn in_farm(cx: i32, cy: i32) -> bool {
    let (x0, x1, y0, y1) = FARM;
    cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1
}

fn fresh_save_path(project: &Path) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("jhyt_smoke.sav");
    let _ = fs::remove_file(&path);
    path
}
