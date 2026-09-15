//! Headless acceptance drive for M17-02 《江湖有田》NPC 日程与路径执行.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M17-02 acceptance paths:
//!
//! - 纯数据注册：7 个 NPC 全来自 `data/npcs.lua` + `data/schedules/*`，日程解析成功。
//! - 山庄 NPC 按时间在活动点间切换，并用 A* 路径逐帧走到目标（顾长风：厢房 → 剑坪）。
//! - 跨图：玄鹤 10:00 逻辑上离开山庄；玩家进入云泽村后按 PersistId 重建。
//! - 不可达诊断：把返家目标格设为实心后切时段，输出 NPC/时间段/地图/目标点。
//! - 连续 7 天日程运行状态稳定；读档后各 NPC 位置与活动一致。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m17_npc [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 山庄 → 云泽村出口格 (16,1) 世界坐标。
const MAIN_EXIT: (f32, f32) = (528.0, 48.0);
/// 云泽村出生/入口格 (19,2) 世界坐标。
const VILLAGE_SPAWN: (f32, f32) = (624.0, 80.0);

/// 读档一致性抽样的 NPC（含跨图玄鹤）。
const SAMPLE: [&str; 4] = [
    "npc_gu_changfeng",
    "npc_shen_qingluo",
    "npc_han_tieshan",
    "npc_xuanhe",
];

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

    banner("M17-02a 纯数据注册：7 个 NPC 全部由 data/npcs.lua + schedule 解析");
    let mut runtime = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut runtime, &main_script)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_mode(&mut runtime, "Playing")?;
    // 批次 4：新增阿七 / 司徒魁 / 陆归藏 三名 NPC（原 7 名 → 10 名）
    assert_str(&mut runtime, "#(require('data.npcs').list())", "10")?;
    lua(&mut runtime, "return require('world.weather').set('sunny')")?;
    let resolve_check = query_str(
        &mut runtime,
        "local npcs = require('data.npcs'); local sched = require('world.schedule'); \
         for _, n in ipairs(npcs.list()) do \
           local r = sched.resolve(n.schedule, 600, false); \
           if not r then return 'fail:' .. n.persist_id end \
         end; return 'ok'",
    )?;
    if resolve_check != "ok" {
        return Err(format!("data-driven schedule resolve failed: {resolve_check}").into());
    }
    // 00:00（睡觉时段）：山庄 NPC 全部落在住所。
    lua(&mut runtime, "clock.set(1, 0)")?;
    frames(&mut runtime, 5)?;
    assert_npc(&mut runtime, "npc_gu_changfeng", "map", "manor")?;
    assert_npc(&mut runtime, "npc_gu_changfeng", "live", "true")?;
    assert_npc(&mut runtime, "npc_gu_changfeng", "place", "manor_home")?;
    assert_npc(&mut runtime, "npc_gu_changfeng", "activity", "sleep")?;
    assert_npc(&mut runtime, "npc_gu_changfeng", "cell_x", "12")?;
    assert_npc(&mut runtime, "npc_gu_changfeng", "cell_y", "6")?;
    // 玄鹤 00:00 也在山庄（厢房）。
    assert_npc(&mut runtime, "npc_xuanhe", "map", "manor")?;
    assert_npc(&mut runtime, "npc_xuanhe", "live", "true")?;

    banner("M17-02b 时间驱动 + A* 路径跟随：顾长风 厢房 → 剑坪");
    lua(&mut runtime, "clock.set(1, 600)")?; // 10:00
    frames(&mut runtime, 1)?;
    assert_npc(
        &mut runtime,
        "npc_gu_changfeng",
        "place",
        "manor_sword_ground",
    )?;
    assert_npc(&mut runtime, "npc_gu_changfeng", "activity", "work")?;
    assert_npc(&mut runtime, "npc_gu_changfeng", "live", "true")?;
    // 从 (12,6) 沿 A* 路径走到 (26,12)。
    run_until(
        &mut runtime,
        "local q = require('world.npc').query('npc_gu_changfeng'); return q.cell_x == 26 and q.cell_y == 12",
        3600,
    )?;
    assert_npc(&mut runtime, "npc_gu_changfeng", "cell_x", "26")?;
    assert_npc(&mut runtime, "npc_gu_changfeng", "cell_y", "12")?;

    banner("M17-02c 跨图：玄鹤 10:00 离开山庄，进入云泽村后重建");
    // 10:00 玄鹤目标在云泽村 → 山庄销毁其实体（逻辑上离开本图）。
    frames(&mut runtime, 2)?;
    assert_npc(&mut runtime, "npc_xuanhe", "map", "village")?;
    assert_npc(&mut runtime, "npc_xuanhe", "live", "false")?;
    // 玩家从山庄南出口进入云泽村。
    teleport(&mut runtime, MAIN_EXIT)?;
    press(&mut runtime, KeyCode::KeyE)?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "village")?;
    assert_map_current(&mut runtime, "village")?;
    assert_player_at(&mut runtime, VILLAGE_SPAWN)?;
    // on_enter 按 PersistId 重建：玄鹤在云泽村土地庙（10:00 目标）。
    assert_npc(&mut runtime, "npc_xuanhe", "map", "village")?;
    assert_npc(&mut runtime, "npc_xuanhe", "live", "true")?;
    assert_npc(&mut runtime, "npc_xuanhe", "place", "village_shrine")?;
    assert_npc(&mut runtime, "npc_xuanhe", "cell_x", "24")?;
    assert_npc(&mut runtime, "npc_xuanhe", "cell_y", "19")?;

    banner("M17-02d 不可达诊断：输出 NPC/时间段/地图/目标点");
    assert_npc(&mut runtime, "npc_shen_qingluo", "map", "village")?;
    assert_npc(&mut runtime, "npc_shen_qingluo", "live", "true")?;
    // 沈青萝 10:00 在医馆；把 21:00 的返家目标格 (12,8) 设为实心 → 路径端点受阻。
    lua(
        &mut runtime,
        "local g = entity.find('GroundMap'); tilemap.set_solid(g, 12, 8, true)",
    )?;
    lua(&mut runtime, "clock.set(1, 1260)")?; // 21:00 return_home → village_home_west
    frames(&mut runtime, 3)?;
    let diag = query_str(
        &mut runtime,
        "return table.concat(require('world.npc').diagnostics(), ' | ')",
    )?;
    for needle in [
        "npc=沈青萝",
        "persist_id=npc_shen_qingluo",
        "window=21:00-23:00",
        "map=village",
        "place=village_home_west",
        "cell=12,8",
        "reason=",
    ] {
        if !diag.contains(needle) {
            return Err(format!("diagnostics missing {needle:?}: {diag}").into());
        }
    }
    println!("diagnostics => {diag}");
    // 复原地形。
    lua(
        &mut runtime,
        "local g = entity.find('GroundMap'); tilemap.set_solid(g, 12, 8, false)",
    )?;

    banner("M17-02e 连续 7 天日程运行稳定");
    let start_day = query_i64(&mut runtime, "clock.now().day")?;
    for _ in 0..7 {
        for minute in [0, 360, 480, 720, 1080, 1260, 1380] {
            lua(
                &mut runtime,
                &format!("clock.set(clock.now().day, {minute})"),
            )?;
            frames(&mut runtime, 3)?;
        }
        lua(&mut runtime, "return day.sleep()")?;
        frames(&mut runtime, 2)?;
    }
    assert_day(&mut runtime, start_day + 7)?;
    let ids = lua_id_list(&SAMPLE);
    let stable = query_str(
        &mut runtime,
        &format!(
            "local npc = require('world.npc'); \
             for _, id in ipairs({{{ids}}}) do \
               local q = npc.query(id); \
               if not q then return 'nocall:' .. id end; \
               if type(q.map) ~= 'string' or q.map == '' then return 'nomap:' .. id end; \
               if type(q.place) ~= 'string' or q.place == '' then return 'noplace:' .. id end \
             end; return 'ok'"
        ),
    )?;
    if stable != "ok" {
        return Err(format!("7-day NPC state unstable: {stable}").into());
    }

    banner("M17-02f 读档后位置与活动一致");
    // 停到午夜（全部落回住所）后再存档抽样，比较 map/place/activity/cell/live。
    lua(&mut runtime, "clock.set(clock.now().day, 0)")?;
    frames(&mut runtime, 4)?;
    let mut expected = Vec::new();
    for pid in SAMPLE {
        expected.push((pid, sample(&mut runtime, pid)?));
    }
    save_game(&mut runtime)?;

    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_scene_is(&mut second, "village")?;
    assert_map_current(&mut second, "village")?;
    assert_day(&mut second, start_day + 7)?;
    for (pid, want) in &expected {
        let got = sample(&mut second, pid)?;
        if got != *want {
            return Err(format!("{pid} restored state = {got:?}, want {want:?}").into());
        }
    }

    println!(
        "jhyt_m17_npc_ok project={} npcs=7 day={} scene={} sample={}",
        project.display(),
        query_i64(&mut second, "clock.now().day")?,
        query_str(&mut second, "scene.reload()")?,
        expected
            .iter()
            .map(|(pid, state)| format!("{pid}:{state}"))
            .collect::<Vec<_>>()
            .join(",")
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

/// Teleports the player to a world position via `player.teleport`.
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

/// Runs `update()` for `n` frames with an empty input snapshot.
fn frames(runtime: &mut LuaRuntime, n: usize) -> Result<(), Box<dyn std::error::Error>> {
    runtime.set_input(InputSnapshot::default());
    for _ in 0..n {
        runtime.call_update()?;
    }
    Ok(())
}

/// Steps frames until the Lua expression evaluates to `true` (checked before each step).
fn run_until(
    runtime: &mut LuaRuntime,
    expr: &str,
    max_frames: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..max_frames {
        if query_str(runtime, expr)? == "true" {
            return Ok(());
        }
        runtime.call_update()?;
    }
    Err(format!("condition not met within {max_frames} frames: {expr}").into())
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

/// 单个 NPC 的运行时状态字段（nil 渲染为 "nil"）。
fn npc_field(
    runtime: &mut LuaRuntime,
    persist_id: &str,
    field: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    query_str(
        runtime,
        &format!(
            "local q = require('world.npc').query('{persist_id}'); return q and tostring(q.{field}) or 'nil'"
        ),
    )
}

fn assert_npc(
    runtime: &mut LuaRuntime,
    persist_id: &str,
    field: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = npc_field(runtime, persist_id, field)?;
    if got != expected {
        return Err(format!("npc {persist_id}.{field} = {got:?}, want {expected:?}").into());
    }
    Ok(())
}

/// 读档一致性抽样：map|place|activity|cell_x,cell_y|live。
fn sample(
    runtime: &mut LuaRuntime,
    persist_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    query_str(
        runtime,
        &format!(
            "local q = require('world.npc').query('{persist_id}'); \
             if not q then return 'missing' end; \
             return string.format('%s|%s|%s|%s,%s|%s', \
               tostring(q.map), tostring(q.place), tostring(q.activity), \
               tostring(q.cell_x), tostring(q.cell_y), tostring(q.live))"
        ),
    )
}

fn lua_id_list(ids: &[&str]) -> String {
    ids.iter()
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(",")
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
    let path = dir.join("jhyt_m17_npc.sav");
    let _ = fs::remove_file(&path);
    path
}
