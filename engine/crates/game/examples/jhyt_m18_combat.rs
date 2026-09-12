//! Headless acceptance drive for M18-01 《江湖有田》战斗 Hitbox 与最小武学.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M18-01 acceptance paths:
//!
//! - 纯数据注册：招式与敌人全部来自 `data/skills.lua` / `data/enemies.lua`，
//!   加载期校验通过（枚举/范围/引用错误可定位）。
//! - 普攻与命中去重：同一攻击判定窗内对同一目标只结算一次；冷却期拒绝。
//! - 武器加成：装备练习剑（damage=8）后普攻伤害真实生效。
//! - 武学：千风掌消耗内力、固定伤害。
//! - 击杀：敌人 hp 归零 → 死亡销毁，战斗日志含 kill 事件。
//! - 敌人 AI：A* 追击进入攻击距离后攻击玩家。
//! - 战败与重开：玩家 hp 归零 → 冻结输入，按 R 回出生点并复活全部敌人。
//! - 存档恢复：玩家气血与敌人账本（hp/存活/位置）随存档一致恢复。
//! - 固定步一致性：战斗逻辑只依赖固定步帧号，同输入不同推进节奏结果一致。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m18_combat [-- <game_jhyt-root>]

use engine::core::{
    input::{InputKey, InputSnapshot},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 演武场出生格 (9,5) 的世界坐标。
const ARENA_SPAWN: (f32, f32) = (304.0, 176.0);
/// 训练木人 home (14,5)；玩家站在 (13,5) 面向右即可命中。
const PLAYER_BEFORE_DUMMY: (f32, f32) = (432.0, 176.0);
/// 灰狼 home (2,9)；玩家站 (6,8) 进入其索敌半径。
const WOLF_LURE: (f32, f32) = (208.0, 272.0);

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
    let save_path = fresh_save_path(&project, "jhyt_m18_combat.sav");
    let project_arg = project.clone();

    banner("M18-01a 纯数据注册 + 加载期校验：招式/敌人来自 data，错误可定位");
    let mut runtime = new_runtime(&project_arg, &save_path)?;
    enter_title(&mut runtime, &main_script)?;
    assert_num(&mut runtime, "#(require('data.skills').ids())", 2.0)?;
    assert_str(
        &mut runtime,
        "require('data.skills').get('strike').kind",
        "basic",
    )?;
    assert_num(
        &mut runtime,
        "require('data.tools').training_sword.damage",
        8.0,
    )?;
    assert_num(&mut runtime, "#(require('data.enemies').list())", 2.0)?;
    assert_validate(&mut runtime, "data.skills")?;
    assert_validate(&mut runtime, "data.enemies")?;
    // 枚举/引用错误可定位：临时破坏数据，validate_all 必须失败并点名字段。
    assert_str(
        &mut runtime,
        "(function() local d = require('data.skills').skills.strike; local old = d.kind; \
         d.kind = 'invalid'; local ok, issues = require('data.skills').validate_all(); \
         d.kind = old; return not ok and table.concat(issues, '|'):find('kind', 1, true) ~= nil end)()",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "(function() local d = require('data.enemies').enemies[1]; local old = d.skill; \
         d.skill = 'no_such_skill'; local ok, issues = require('data.enemies').validate_all(); \
         d.skill = old; return not ok and table.concat(issues, '|'):find('unknown skill', 1, true) ~= nil end)()",
        "true",
    )?;

    banner("M18-01b 进入演武场：新档启动，敌人按数据落位，玩家满气血");
    lua(&mut runtime, "scene.load('combat_arena')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "combat_arena")?;
    assert_map_current(&mut runtime, "combat_arena")?;
    assert_mode(&mut runtime, "Playing")?;
    assert_num(&mut runtime, "require('combat').player_hp()", 100.0)?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        30.0,
    )?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').cell_x",
        14.0,
    )?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_wolf').cell_x",
        2.0,
    )?;
    assert_num(&mut runtime, "require('combat').frame()", 0.0)?;

    banner("M18-01c 普攻：徒手伤害 2，判定窗内同一攻击只结算一次，冷却期拒绝");
    teleport(&mut runtime, PLAYER_BEFORE_DUMMY)?;
    lua(&mut runtime, "require('state').facing = 'right'")?;
    frames(&mut runtime, 2)?;
    // 攻击#1：判定窗（windup 6 + active 12）内目标一直贴着 → 只掉一次血。
    press(&mut runtime, InputKey::from_char('j'))?;
    frames(&mut runtime, 30)?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        28.0,
    )?;
    // 攻击#2（#1 的 24 步冷却已过）：注册后仍在前摇，且再次攻击被冷却拒绝。
    press(&mut runtime, InputKey::from_char('j'))?;
    assert_str(
        &mut runtime,
        "select(2, require('combat').player_attack('strike'))",
        "cooldown",
    )?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        28.0,
    )?;
    frames(&mut runtime, 30)?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        26.0,
    )?;
    // 攻击#3：冷却结束后再次命中。
    press(&mut runtime, InputKey::from_char('j'))?;
    frames(&mut runtime, 30)?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        24.0,
    )?;
    // 战斗日志含 hit 事件，且携带攻击者/目标/招式/帧号（可追踪）。
    assert_str(
        &mut runtime,
        "(function() for _, e in ipairs(require('combat').log()) do \
         if e.event == 'hit' and e.attacker == 'player' and e.target ~= 'player' \
         and e.skill == 'strike' and e.frame > 0 then return 'true' end end return 'false' end)()",
        "true",
    )?;

    banner("M18-01d 武器加成：装备练习剑后普攻伤害 2+8=10");
    lua(&mut runtime, "inv.add('player', 'training_sword', 1)")?;
    lua(
        &mut runtime,
        "inv.equip('player', 'weapon', 'training_sword')",
    )?;
    assert_num(&mut runtime, "require('skills').weapon_damage()", 8.0)?;
    assert_num(
        &mut runtime,
        "require('skills').player_damage('strike')",
        10.0,
    )?;
    frames(&mut runtime, 30)?;
    press(&mut runtime, InputKey::from_char('j'))?;
    frames(&mut runtime, 30)?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        14.0,
    )?;

    banner("M18-01e 武学：千风掌消耗内力 10、固定伤害 6");
    assert_num(&mut runtime, "require('state').inner_power", 50.0)?;
    press(&mut runtime, InputKey::from_char('k'))?;
    frames(&mut runtime, 30)?;
    assert_num(&mut runtime, "require('state').inner_power", 40.0)?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        8.0,
    )?;
    // 内力不足：先推过千风掌冷却（60 步），拒绝原因才是 inner_power，且不掉血。
    frames(&mut runtime, 60)?;
    lua(&mut runtime, "require('state').inner_power = 5")?;
    assert_str(
        &mut runtime,
        "select(2, require('combat').player_attack('palm_gust'))",
        "inner_power",
    )?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        8.0,
    )?;
    lua(&mut runtime, "require('state').inner_power = 40")?;

    banner("M18-01f 击杀：hp 归零 → 死亡销毁，战斗日志含 kill");
    frames(&mut runtime, 24)?;
    press(&mut runtime, InputKey::from_char('j'))?;
    frames(&mut runtime, 30)?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        0.0,
    )?;
    assert_str(
        &mut runtime,
        "require('enemy').query('enemy_dummy').alive",
        "false",
    )?;
    assert_str(
        &mut runtime,
        "require('enemy').query('enemy_dummy').live",
        "false",
    )?;
    assert_str(
        &mut runtime,
        "(function() for _, e in ipairs(require('combat').log()) do \
         if e.event == 'kill' and e.target == '训练木人' then return 'true' end end return 'false' end)()",
        "true",
    )?;
    assert_num(&mut runtime, "require('combat').stats().kills", 1.0)?;

    banner("M18-01g 敌人 AI：A* 追击 → 进入攻击距离后攻击玩家");
    teleport(&mut runtime, WOLF_LURE)?;
    frames(&mut runtime, 60)?;
    assert_str(
        &mut runtime,
        "(function() local q = require('enemy').query('enemy_wolf'); \
         if q.state == 'chase' or q.state == 'attack' then return 'true' end \
         return 'state=' .. tostring(q.state) end)()",
        "true",
    )?;
    frames(&mut runtime, 300)?;
    let player_hp = query_num(&mut runtime, "return require('combat').player_hp()")?;
    if player_hp >= 100.0 {
        return Err(format!("wolf never damaged player: hp={player_hp}").into());
    }
    assert_num(
        &mut runtime,
        "require('combat').stats().taken > 0 and 1 or 0",
        1.0,
    )?;

    banner("M18-01h 战败与重开：hp 归零冻结输入，R 回出生点并复活全部敌人");
    lua(&mut runtime, "require('combat').set_player_hp(3)")?;
    frames(&mut runtime, 240)?;
    assert_num(&mut runtime, "require('combat').player_hp()", 0.0)?;
    assert_str(&mut runtime, "require('combat').is_player_dead()", "true")?;
    assert_mode(&mut runtime, "Playing")?;
    // 战败后敌人停手（不鞭尸），气血保持 0。
    frames(&mut runtime, 120)?;
    assert_num(&mut runtime, "require('combat').player_hp()", 0.0)?;
    // 按 R 重开：气血回满、回到出生点、敌人全部复活。
    press(&mut runtime, InputKey::from_char('r'))?;
    assert_num(&mut runtime, "require('combat').player_hp()", 100.0)?;
    assert_str(&mut runtime, "require('combat').is_player_dead()", "false")?;
    assert_num(
        &mut runtime,
        "require('player').position().x",
        ARENA_SPAWN.0 as f64,
    )?;
    assert_str(
        &mut runtime,
        "require('enemy').query('enemy_dummy').alive",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        30.0,
    )?;
    assert_str(
        &mut runtime,
        "require('enemy').query('enemy_wolf').alive",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_wolf').hp",
        20.0,
    )?;
    // 战斗日志含 respawn 与 death 事件。
    assert_str(
        &mut runtime,
        "(function() local seen = {} \
         for _, e in ipairs(require('combat').log()) do seen[e.event] = true end \
         return tostring(seen['respawn'] == true and seen['death'] == true) end)()",
        "true",
    )?;

    banner("M18-01i 存档恢复：玩家气血与敌人账本（hp/位置/装备）随存档一致恢复");
    teleport(&mut runtime, PLAYER_BEFORE_DUMMY)?;
    lua(&mut runtime, "require('state').facing = 'right'")?;
    frames(&mut runtime, 24)?;
    press(&mut runtime, InputKey::from_char('j'))?;
    frames(&mut runtime, 30)?;
    assert_num(
        &mut runtime,
        "require('enemy').query('enemy_dummy').hp",
        20.0,
    )?;
    save_game(&mut runtime)?;

    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_title(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_scene_is(&mut second, "combat_arena")?;
    assert_map_current(&mut second, "combat_arena")?;
    assert_validate(&mut second, "data.skills")?;
    assert_validate(&mut second, "data.enemies")?;
    assert_num(
        &mut second,
        "require('enemy').query('enemy_dummy').hp",
        20.0,
    )?;
    assert_str(
        &mut second,
        "require('enemy').query('enemy_dummy').alive",
        "true",
    )?;
    assert_num(&mut second, "require('combat').player_hp()", 100.0)?;
    assert_str(
        &mut second,
        "require('skills').weapon_id()",
        "training_sword",
    )?;

    banner("M18-01j 固定步一致性：帧号按 update 计数，同输入不同推进节奏同结果");
    teleport(&mut second, PLAYER_BEFORE_DUMMY)?;
    lua(&mut second, "require('state').facing = 'right'")?;
    let f0 = query_num(&mut second, "return require('combat').frame()")?;
    frames(&mut second, 5)?;
    assert_num(&mut second, "require('combat').frame()", f0 + 5.0)?;
    assert_num(&mut second, "time.delta()", 1.0 / 60.0)?;
    // 基准 A：攻击后批量推 30 帧。
    frames(&mut second, 24)?;
    let hp0 = query_num(
        &mut second,
        "return require('enemy').query('enemy_dummy').hp",
    )?;
    press(&mut second, InputKey::from_char('j'))?;
    frames(&mut second, 30)?;
    let hp_a = query_num(
        &mut second,
        "return require('enemy').query('enemy_dummy').hp",
    )?;
    // 基准 B：等待冷却后同攻击，逐帧推 30 次。
    frames(&mut second, 30)?;
    press(&mut second, InputKey::from_char('j'))?;
    for _ in 0..30 {
        frames(&mut second, 1)?;
    }
    let hp_b = query_num(
        &mut second,
        "return require('enemy').query('enemy_dummy').hp",
    )?;
    // 两次攻击各自的单次伤害增量必须一致（与推进节奏无关）。
    let delta_a = hp0 - hp_a;
    let delta_b = hp_a - hp_b;
    if !(delta_a > 0.0 && (delta_a - delta_b).abs() < 1e-6) {
        return Err(format!(
            "fixed-step mismatch: per-attack delta batch={delta_a} step={delta_b}"
        )
        .into());
    }
    assert_num(
        &mut second,
        "require('enemy').query('enemy_dummy').hp",
        hp0 - 20.0,
    )?;

    println!(
        "jhyt_m18_combat_ok project={} kills={} deaths={} taken={} frame={}",
        project.display(),
        query_num(&mut second, "return require('combat').stats().kills")?,
        query_num(&mut second, "return require('combat').stats().deaths")?,
        query_num(&mut second, "return require('combat').stats().taken")?,
        query_num(&mut second, "return require('combat').frame()")?,
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

/// Runs the project script, then loads the `title` scene so BtnStart/BtnContinue
/// are live (mirrors jhyt_m17_shop).
fn enter_title(
    runtime: &mut LuaRuntime,
    main_script: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_project_script(main_script)?;
    runtime.execute_console("scene.load('title')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(runtime, "title")?;
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
    runtime.execute_console(&format!("require('player').teleport({}, {})", pos.0, pos.1))?;
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

/// Runs `update()` for `n` frames with an empty input snapshot.
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

/// 数值断言走 f64 比较：存档 JSON 往返可能把整数读回为 `3.0`。
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

/// 加载期校验：issues 非空时把全部条目拼进错误信息。
fn assert_validate(
    runtime: &mut LuaRuntime,
    module: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "local ok, issues = require('{module}').validate_all(); \
             if not ok then return table.concat(issues, ' | ') end; return 'ok'"
        ),
    )?;
    if got != "ok" {
        return Err(format!("{module}.validate_all failed: {got}").into());
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
    let current = query_str(runtime, "require('map_manager').current()")?;
    if current != expected {
        return Err(format!("map.current() = {current:?}, want {expected:?}").into());
    }
    Ok(())
}

fn assert_mode(runtime: &mut LuaRuntime, expected: &str) -> Result<(), Box<dyn std::error::Error>> {
    assert_str(runtime, "require('state').mode", expected)
}

fn save_game(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    let ok = query_str(runtime, "return tostring(require('save_manager').save())")?;
    if ok != "true" {
        return Err(format!("save() returned {ok:?}").into());
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
