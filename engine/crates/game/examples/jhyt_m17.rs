//! Headless acceptance drive for M17-01 《江湖有田》多地图与场景生命周期.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M17-01 acceptance paths:
//!
//! - 新档从山庄出生，农田耕作链路照常（till/plant/water）。
//! - 走到山庄南出口格按 E → 切到云泽村，并在入口格 (19,2)=(624,80) 出生；
//!   背包内容跨图不被清空（inventory.setup 只跑一次 / booted 语义）。
//! - 云泽村内睡觉推日：on_day_start 在 village 上跑（体力重置 + 自动存档），
//!   且不触碰 main 的农田日结算（无 FarmGrid 也不崩）。
//! - 从云泽村南门按 E → 回到山庄：农田 overlay / plots 恢复，作物仍在。
//! - 存档记录所在地图；**第二 runtime** 点 BtnContinue 回到 village 而非固定
//!   山庄，恢复存档坐标（覆盖出生点），并保留农田状态；validate_long_run
//!   证明 village 档的快照恢复稳定。
//!
//! 移动用 player.teleport 落位（引擎无窗口，physics.set_position 同步刚体与
//! ECS Transform），交互按 jhyt_smoke 的按键帧语义驱动。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m17 [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 山庄出生格 (16,3) 世界坐标。
const MAIN_SPAWN: (f32, f32) = (528.0, 112.0);
/// 山庄 → 云泽村出口格 (16,1) 世界坐标。
const MAIN_EXIT: (f32, f32) = (528.0, 48.0);
/// 云泽村出生/入口格 (19,2) 世界坐标。
const VILLAGE_SPAWN: (f32, f32) = (624.0, 80.0);
/// 云泽村 → 山庄出口格 (19,1) 世界坐标。
const VILLAGE_EXIT: (f32, f32) = (624.0, 48.0);
/// 存档用非出生点（验证 restore 以存档坐标覆盖出生点）。
const VILLAGE_SAVE_SPOT: (f32, f32) = (624.0, 208.0);
/// 新档初始灵芝菌种数（`scripts/player/inventory.lua` 的 `inventory.setup()`）。
/// M19-01 按 MVP 剧本把 5 调到 8；改动新档载荷必须同步本常量与下方断言。
const START_SEED_LINGZHI: f64 = 8.0;
/// 种一茬灵芝消耗 1 颗，这是剧本走完后的剩余量。
const SEED_LINGZHI_AFTER_PLANT: f64 = START_SEED_LINGZHI - 1.0;

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

    banner("M17-01a 新档：山庄出生 + 农田链路");
    let mut runtime = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut runtime, &main_script)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_mode(&mut runtime, "Playing")?;
    assert_day(&mut runtime, 1)?;
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    assert_player_at(&mut runtime, MAIN_SPAWN)?;

    // 出生朝上，面前 (16,4) 是药田 DIRT：翻土 → 种灵芝 → 浇水。
    set_facing(&mut runtime, "up")?;
    press(&mut runtime, KeyCode::KeyE)?; // till
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    set_seed(&mut runtime, "seed_lingzhi")?;
    press(&mut runtime, KeyCode::KeyE)?; // plant
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "plant_seed_lingzhi",
    )?;
    // 钉住晴天（M29 起天气按 `(seed, day, 季节)` 抽签）：雨天会走 `rain_watered`
    // 免体力分支，而这里断言的是体力账（100-8-3-4）。
    runtime.execute_console("require('world.weather').set('sunny')")?;
    equip_tool(&mut runtime, "watering_can")?;
    press(&mut runtime, KeyCode::KeyE)?; // water
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "water_lingzhi",
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 85.0)?; // 100-8-3-4
    assert_plot_crop(&mut runtime, "16,4", "lingzhi")?;
    // 种地耗 1 颗灵芝种子。
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_lingzhi')",
        SEED_LINGZHI_AFTER_PLANT,
    )?;

    // 山庄睡到 day2：main 日结算成长 +1，能量回满。
    sleep(&mut runtime)?;
    assert_day(&mut runtime, 2)?;
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    assert_plot_days(&mut runtime, "16,4", 1.0)?;

    banner("M17-01b 出口交互：山庄 → 云泽村，背包跨图不清空");
    teleport(&mut runtime, MAIN_EXIT)?;
    assert_mode(&mut runtime, "Playing")?;
    press(&mut runtime, KeyCode::KeyE)?; // exit main_to_village
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "exit_village",
    )?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "village")?;
    assert_map_current(&mut runtime, "village")?;
    assert_mode(&mut runtime, "Playing")?;
    // 入口落点 (19,2)，非山庄出生点。
    assert_player_at(&mut runtime, VILLAGE_SPAWN)?;
    // 背包仍是「新档载荷 - 已播种数」：未二次 inventory.setup 清空。
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_lingzhi')",
        SEED_LINGZHI_AFTER_PLANT,
    )?;
    assert_num(&mut runtime, "inv.count('player', 'lingzhi')", 0.0)?;
    // 农田明细在切图后仍可从 map.state 读到（全量持久键保留）。
    assert_plot_crop(&mut runtime, "16,4", "lingzhi")?;

    banner("M17-01c village 日结算：睡觉推日 + 自动存档不碰农田");
    // 存档点：玩家停在不触发出口的格，睡到 day3。
    teleport(&mut runtime, VILLAGE_SAVE_SPOT)?;
    sleep(&mut runtime)?; // day2 -> day3
    assert_day(&mut runtime, 3)?;
    assert_scene_is(&mut runtime, "village")?;
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    save_game(&mut runtime)?; // 显式存档：map=village day3 pos=VILLAGE_SAVE_SPOT
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_lingzhi')",
        SEED_LINGZHI_AFTER_PLANT,
    )?;

    banner("M17-01d 回山庄：农田 overlay/plots 恢复，作物仍在");
    teleport(&mut runtime, VILLAGE_EXIT)?;
    press(&mut runtime, KeyCode::KeyE)?; // exit village_to_manor
    // 山庄地图键/场景在示例重组后为 manor（M18-02 收口同步断言）。
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "exit_manor",
    )?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_map_current(&mut runtime, "manor")?;
    assert_player_at(&mut runtime, MAIN_SPAWN)?;
    assert_plot_crop(&mut runtime, "16,4", "lingzhi")?;
    assert_plot_days(&mut runtime, "16,4", 1.0)?; // day3 未浇水不成长
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_lingzhi')",
        SEED_LINGZHI_AFTER_PLANT,
    )?;

    // 再次往返验证稳定：main -> village 回村不丢状态。
    teleport(&mut runtime, MAIN_EXIT)?;
    press(&mut runtime, KeyCode::KeyE)?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "village")?;
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_lingzhi')",
        SEED_LINGZHI_AFTER_PLANT,
    )?;
    assert_plot_crop(&mut runtime, "16,4", "lingzhi")?;

    banner("M17-01e continue：第二 runtime 按存档地图回到云泽村");
    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    // 存档在 village —— continue 应切回 village 而非固定山庄。
    assert_scene_is(&mut second, "village")?;
    assert_map_current(&mut second, "village")?;
    assert_day(&mut second, 3)?;
    assert_mode(&mut second, "Playing")?;
    // restore 以存档坐标覆盖出生点 (624,80) -> (624,208)。
    assert_player_at(&mut second, VILLAGE_SAVE_SPOT)?;
    assert_num(
        &mut second,
        "inv.count('player', 'seed_lingzhi')",
        SEED_LINGZHI_AFTER_PLANT,
    )?;
    assert_num(&mut second, "inv.count('player', 'lingzhi')", 0.0)?;
    // village 档同样携带 main 农田明细；查 map.state 校验 restore 写回。
    assert_plot_crop(&mut second, "16,4", "lingzhi")?;
    assert_plot_days(&mut second, "16,4", 1.0)?;
    // 长跑：快照在 village 场景名下反复恢复稳定。
    let ok = query_str(
        &mut second,
        "return tostring(select(1, require('core.save_manager').validate_long_run(30, 100)))",
    )?;
    assert_eq!(ok, "true", "validate_long_run(30,100) 应成功");

    banner("M17-01f continue 后回到山庄农田依旧");
    teleport(&mut second, VILLAGE_EXIT)?;
    press(&mut second, KeyCode::KeyE)?;
    second.process_scene_transition()?;
    assert_scene_is(&mut second, "manor")?;
    assert_plot_crop(&mut second, "16,4", "lingzhi")?;

    println!(
        "jhyt_m17_ok project={} day={} scene={} plot=16,4:lingzhi seeds={} pos={},{}",
        project.display(),
        query_i64(&mut second, "clock.now().day")?,
        query_str(&mut second, "scene.reload()")?,
        query_i64(&mut second, "inv.count('player', 'seed_lingzhi')")?,
        VILLAGE_SAVE_SPOT.0 as i64,
        VILLAGE_SAVE_SPOT.1 as i64
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
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(720, 1280);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
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

/// Teleports the player to a world position via `player.teleport`
/// (physics.set_position + zero velocity, kept in sync with ECS Transform).
fn teleport(runtime: &mut LuaRuntime, pos: (f32, f32)) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "require('player.player').teleport({}, {})",
        pos.0, pos.1
    ))?;
    Ok(())
}

/// Simulates a single key press: one frame with the key down, one clean frame.
fn press(runtime: &mut LuaRuntime, key: KeyCode) -> Result<(), Box<dyn std::error::Error>> {
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
    let value = query_num(runtime, expr)?;
    if (value - expected).abs() > f64::EPSILON {
        return Err(format!("{expr} = {value:?}, want {expected}").into());
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

fn assert_player_at(
    runtime: &mut LuaRuntime,
    expected: (f32, f32),
) -> Result<(), Box<dyn std::error::Error>> {
    let pos = query_str(
        runtime,
        "local p = entity.find('JianghuPlayer'); if not p then return 'nil' end; local t = transform.get(p); return string.format('%.1f,%.1f', t.x, t.y)",
    )?;
    let (wx, wy) = expected;
    let want = format!("{wx:.1},{wy:.1}");
    if pos != want {
        return Err(format!("player position = {pos:?}, want {want:?}").into());
    }
    Ok(())
}

/// 查询山庄药田某格作物（map.state 持久键，跨图/读档均可读）。
fn plot_crop(runtime: &mut LuaRuntime, cell: &str) -> Result<String, Box<dyn std::error::Error>> {
    query_str(
        runtime,
        &format!(
            "local m = map.state('farm_plots'); local p = m and m['{cell}']; return p and tostring(p.crop) or 'nil'"
        ),
    )
}

fn assert_plot_crop(
    runtime: &mut LuaRuntime,
    cell: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = plot_crop(runtime, cell)?;
    if got != expected {
        return Err(format!("plot {cell} crop = {got:?}, want {expected:?}").into());
    }
    Ok(())
}

fn plot_days(runtime: &mut LuaRuntime, cell: &str) -> Result<f64, Box<dyn std::error::Error>> {
    query_num(
        runtime,
        &format!(
            "local m = map.state('farm_plots'); local p = m and m['{cell}']; return tostring((p and p.watered_days) or -1)"
        ),
    )
}

fn assert_plot_days(
    runtime: &mut LuaRuntime,
    cell: &str,
    expected: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = plot_days(runtime, cell)?;
    if (got - expected).abs() > f64::EPSILON {
        return Err(format!("plot {cell} watered_days = {got:?}, want {expected}").into());
    }
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

fn fresh_save_path(project: &Path) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("jhyt_m17.sav");
    let _ = fs::remove_file(&path);
    path
}
