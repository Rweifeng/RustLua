//! Headless acceptance drive for save-data integrity & new-game isolation.
//!
//! 覆盖三类缺陷（历史复核已复现，本示例把它们锁成回归）：
//!
//! 1. **存档可被一次多行文本整档损坏**：`save.write` 的值含 `\n` 会写出伪行，
//!    随后 `decode` 整档失败。现在写入端转义（v2）、读取端兼容 v1，且
//!    `save.load` 区分「没有存档」与「存档已损坏」。
//! 2. **「开始新游戏」继承上一局进度**：`__map` / `__mapstate:*` / `__blob:*`
//!    不属于任何 Lua provider，只清 Lua 表会读回旧档的农田/建筑/采集状态。
//!    现在 `new_game()` 先 `save.clear()` + `map.state_clear()`，再让各模块写空快照。
//! 3. **provider 循环中途 abort 造成半恢复**：现在全部应用并收集失败，
//!    失败即回滚到读档前的一致性状态，并返回失败清单。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m22_save_isolation [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 新档初始灵芝菌种数（`scripts/player/inventory.lua` 的 `inventory.setup()`）。
const START_SEED_LINGZHI: f64 = 8.0;
/// 山庄药田第一格（与 jhyt_m17 相同：出生朝上，面前 (16,4) 是 DIRT）。
const PLOT_CELL: &str = "16,4";
const PLOT_X: i64 = 16;
const PLOT_Y: i64 = 4;

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
    let save_path = fresh_save_path(&project);
    let project_arg = project.clone();

    // -----------------------------------------------------------------
    banner("① 注册中心：provider 名单完整（防止重构时静默少注册）");
    let mut runtime = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut runtime, &main_script)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    for name in [
        "state",
        "inventory",
        "clock",
        "player",
        "scene",
        "farm",
        "npc",
        "quest",
        "shop",
        "combat",
        "enemy",
        "stats",
        "relationship",
        "building",
        "crafting",
        "audio",
    ] {
        let present = query_str(
            &mut runtime,
            &format!(
                "local names = require('core.save_manager').provider_names()\n\
                 for _, n in ipairs(names) do if n == '{name}' then return 'yes' end end\n\
                 return 'no'"
            ),
        )?;
        if present != "yes" {
            return Err(format!("save provider {name:?} is not registered").into());
        }
    }

    banner("② 多行/制表符/CJK 值不再整档损坏（真实写盘 → 新 runtime 读回）");
    assert_num(
        &mut runtime,
        "inv.count('player', 'seed_lingzhi')",
        START_SEED_LINGZHI,
    )?;
    runtime.execute_console(
        "save.write('note', 'line one\\nline two')\n\
         save.write('tabbed', 'a\\tb')\n\
         save.write('草药名', '灵芝 x2')",
    )?;
    runtime.execute_console("assert(require('core.save_manager').save(), 'commit multiline')")?;
    let text = fs::read_to_string(&save_path)?;
    if !text.starts_with("save_version=2\n") {
        return Err(format!("save header should be v2:\n{text}").into());
    }
    // 版本头 + 每个键值对一行：多行值绝不能产生伪行。
    let data_rows = text
        .lines()
        .filter(|line| line.contains('\t') && !line.starts_with("save_version="))
        .count();
    if data_rows != text.lines().count() - 1 {
        return Err(format!("multiline value wrote extra rows:\n{text}").into());
    }
    let mut reader = new_runtime(&project_arg, &save_path)?;
    reader.execute_console(
        "local ok, reason = save.load()\n\
         assert(ok, tostring(reason))\n\
         assert(save.read('note') == 'line one\\nline two', 'note')\n\
         assert(save.read('tabbed') == 'a\\tb', 'tabbed')\n\
         assert(save.read('草药名') == '灵芝 x2', 'cjk')",
    )?;

    banner("③ v1 旧存档仍可读（零数据丢失）");
    let legacy = "save_version=1\nhp\t75\nnote\tlegacy\n";
    fs::write(&save_path, legacy)?;
    let mut legacy_reader = new_runtime(&project_arg, &save_path)?;
    legacy_reader.execute_console(
        "local ok, reason = save.load()\n\
         assert(ok, tostring(reason))\n\
         assert(save.read('hp') == '75', 'legacy hp')\n\
         assert(save.read('note') == 'legacy', 'legacy note')",
    )?;
    // 读档不影响原文件；只有下次 commit 才升级为 v2。
    if fs::read_to_string(&save_path)? != legacy {
        return Err("loading a legacy save must not rewrite the file".into());
    }

    banner("④ 损坏存档：保留原文件并给出 corrupt 原因（与 missing 区分）");
    let corrupt = "save_version=1\nhp\t75\nbroken row\n";
    fs::write(&save_path, corrupt)?;
    let mut corrupt_reader = new_runtime(&project_arg, &save_path)?;
    corrupt_reader.execute_console(
        "local ok, reason = save.load()\n\
         assert(ok == false, 'corrupt save must not load')\n\
         assert(tostring(reason):sub(1, 8) == 'corrupt:', tostring(reason))",
    )?;
    if fs::read_to_string(&save_path)? != corrupt {
        return Err("a corrupt save must be preserved, never overwritten".into());
    }
    // 没有存档文件时必须报 "missing" 而不是 "corrupt"：玩家进度"丢了"和
    // "从没存过"在 UI 与诊断里是两件事。
    let _ = fs::remove_file(&save_path);
    let mut missing_reader = new_runtime(&project_arg, &save_path)?;
    missing_reader.execute_console(
        "local ok, reason = save.load()\n\
         assert(ok == false)\n\
         assert(reason == 'missing', tostring(reason))",
    )?;

    // -----------------------------------------------------------------
    banner("⑤ 新游戏隔离：继续 → 玩 → 返回标题 → 开始新游戏 不继承上一局");
    let _ = fs::remove_file(&save_path);
    let mut session = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut session, &main_script)?;
    click_button(&mut session, "BtnStart")?;
    assert_scene_is(&mut session, "manor")?;

    // 开垦 + 播种 + 浇水：制造"上一局的农田进度"。
    set_facing(&mut session, "up")?;
    press(&mut session, KeyCode::KeyE)?;
    assert_str(&mut session, "require('core.state').last_action", "till")?;
    set_seed(&mut session, "seed_lingzhi")?;
    press(&mut session, KeyCode::KeyE)?;
    assert_str(
        &mut session,
        "require('core.state').last_action",
        "plant_seed_lingzhi",
    )?;
    // 钉住晴天（M29 起天气按 `(seed, day, 季节)` 抽签）：雨天是 `rain_watered`。
    session.execute_console("require('world.weather').set('sunny')")?;
    equip_tool(&mut session, "watering_can")?;
    press(&mut session, KeyCode::KeyE)?;
    assert_str(
        &mut session,
        "require('core.state').last_action",
        "water_lingzhi",
    )?;
    assert_plot_crop(&mut session, PLOT_CELL, "lingzhi")?;
    // M72-08 分层：土面 = 犁地(1)（当日浇过水），作物 = 灵芝图集第 1 行苗期(entry 1)。
    assert_farm_tile(&mut session, "FarmGrid", 1)?;
    assert_farm_tile(&mut session, "CropGrid", 1)?;
    assert_num(
        &mut session,
        "inv.count('player', 'seed_lingzhi')",
        START_SEED_LINGZHI - 1.0,
    )?;
    // 采一份药材（gather 记录）与睡一天（日期推进），让"上一局"有更多可继承状态。
    session.execute_console(
        "require('systems.gather').reset()\n\
         map.state('gather', { state_version = 1, day = 1, nodes = { wild_herb_1 = { day = 1 } } })",
    )?;
    sleep(&mut session)?;
    assert_day(&mut session, 2)?;
    assert_plot_days(&mut session, PLOT_CELL, 1.0)?;
    session.execute_console("assert(require('core.save_manager').save(), 'save before title')")?;

    // 返回门面 → 再次开始新游戏（同一个 runtime，即缺陷的复现路径）。
    session.execute_console("assert(jianghu_return_to_title())")?;
    session.process_scene_transition()?;
    assert_scene_is(&mut session, "main")?;
    click_button(&mut session, "BtnStart")?;
    assert_scene_is(&mut session, "manor")?;

    assert_day(&mut session, 1)?;
    assert_num(&mut session, "require('core.state').energy", 100.0)?;
    assert_num(
        &mut session,
        "inv.count('player', 'seed_lingzhi')",
        START_SEED_LINGZHI,
    )?;
    assert_num(&mut session, "inv.count('player', 'lingzhi')", 0.0)?;
    assert_plot_crop(&mut session, PLOT_CELL, "nil")?;
    assert_farm_tile(&mut session, "FarmGrid", 0)?; // 土面抹回空
    assert_farm_tile(&mut session, "CropGrid", 0)?; // 作物层不留幽灵苗
    assert_num(&mut session, "require('systems.farm').stats().tilled", 0.0)?;
    let gather_nodes = query_str(
        &mut session,
        "local s = map.state('gather')\n\
         local n = 0\n\
         if type(s) == 'table' and type(s.nodes) == 'table' then\n\
           for _ in pairs(s.nodes) do n = n + 1 end\n\
         end\n\
         return tostring(n)",
    )?;
    if gather_nodes != "0" {
        return Err(format!("gather nodes survived a new game: {gather_nodes}").into());
    }
    // 引擎侧键（地图/出生点）也必须被丢弃。
    assert_str(&mut session, "map.spawn()", "nil")?;
    assert_str(&mut session, "save.read('__map')", "nil")?;

    // -----------------------------------------------------------------
    banner("⑥ 读档原子性：一个 provider 失败 → 整体失败并回滚（不留半恢复状态）");
    assert_day(&mut session, 1)?;
    session.execute_console(
        "local sm = require('core.save_manager')\n\
         sm.register('zzz_fault', function() return { marker = true } end,\n\
           function() error('injected restore failure') end)\n\
         assert(sm.save(), 'save with faulty provider')",
    )?;
    // 读档前先推进到一个可识别状态。
    sleep(&mut session)?;
    assert_day(&mut session, 2)?;
    session.execute_console("assert(require('core.save_manager').save(), 'save stable state')")?;
    assert_num(
        &mut session,
        "inv.count('player', 'seed_lingzhi')",
        START_SEED_LINGZHI,
    )?;
    let load_result = query_str(
        &mut session,
        "local ok, reason = require('core.save_manager').load()\n\
         return tostring(ok) .. '|' .. tostring(reason)",
    )?;
    if !load_result.starts_with("false|") {
        return Err(format!("faulty provider must fail the load, got {load_result:?}").into());
    }
    if !load_result.contains("zzz_fault") {
        return Err(format!("failure list must name the provider: {load_result:?}").into());
    }
    // 回滚证据：日期/背包/地块与读档前完全一致（半恢复会破坏其中至少一项）。
    assert_day(&mut session, 2)?;
    assert_num(
        &mut session,
        "inv.count('player', 'seed_lingzhi')",
        START_SEED_LINGZHI,
    )?;
    // provider 恢复正常后读档必须再次成功（失败不是永久中毒）。
    session.execute_console(
        "require('core.save_manager').register('zzz_fault', function() return { marker = true } end, function() end)\n\
         assert(require('core.save_manager').load(), 'load after provider repaired')",
    )?;
    assert_day(&mut session, 2)?;

    // -----------------------------------------------------------------
    banner("⑦ continue 正常路径仍可用（回归保护）");
    session.execute_console("assert(jianghu_return_to_title())")?;
    session.process_scene_transition()?;
    click_button(&mut session, "BtnContinue")?;
    assert_scene_is(&mut session, "manor")?;
    assert_day(&mut session, 2)?;

    // -----------------------------------------------------------------
    banner("⑧ 游戏层读档状态：loaded / missing 可区分（诊断面板与 UI 依赖它）");
    assert_str(
        &mut session,
        "require('core.save_manager').last_load_status().status",
        "loaded",
    )?;
    let _ = fs::remove_file(&save_path);
    assert_block(
        &mut session,
        "local ok, reason = require('core.save_manager').load()\n\
         return tostring(ok) .. '/' .. tostring(reason) .. '/' .. tostring(require('core.save_manager').last_load_status().status)",
        "false/missing/missing",
    )?;

    // -----------------------------------------------------------------
    banner("⑨ 读档补发：初始工具缺件即死档，restore 必须补齐（旧档的锄头形状）");
    // 老档里锄头只有 tool 槽标签、没有堆叠（`inv.equip` 是纯标签写），而背包面板的
    // 行来自槽位 ⇒ 玩家选不到锄头。两种坏形状都要自愈：①槽上还指着锄头；
    // ②槽已被水壶覆盖，锄头连引用都不剩（玩家实际报的那一种）。
    assert_block(
        &mut session,
        "local st = require('core.state')\n\
         local bag = require('player.inventory')\n\
         local snap = bag.capture()\n\
         snap.capacity = 20\n\
         snap.slots = {}\n\
         for index = 1, snap.capacity do snap.slots[index] = { item = '', count = 0 } end\n\
         snap.equipment = { tool = 'hoe' }\n\
         bag.restore(snap)\n\
         return tostring(inv.count(st.inventory, 'hoe')) .. '/' \
           .. tostring(inv.equip_list(st.inventory).tool)",
        "1/hoe",
    )?;
    assert_block(
        &mut session,
        "local st = require('core.state')\n\
         local bag = require('player.inventory')\n\
         local snap = { capacity = 20, slots = {}, equipment = { tool = 'watering_can' } }\n\
         for index = 1, snap.capacity do snap.slots[index] = { item = '', count = 0 } end\n\
         snap.slots[1] = { item = 'watering_can', count = 1 }\n\
         bag.restore(snap)\n\
         return tostring(inv.count(st.inventory, 'hoe')) .. '/' \
           .. tostring(inv.count(st.inventory, 'watering_can'))",
        "1/1",
    )?;

    println!(
        "jhyt_m22_save_isolation_ok project={} providers=16 rows={} day={}",
        project.display(),
        data_rows,
        query_i64(&mut session, "clock.now().day")?
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers ----------

fn banner(title: &str) {
    println!("\n== {title} ==");
}

fn fresh_save_path(project: &Path) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!(
        "save-acceptance-{}-{}.sav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ))
}

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
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

fn press(runtime: &mut LuaRuntime, key: KeyCode) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
}

/// 真实按键路径：方向键转向由 `input.update` 消费，这里直接设朝向避免依赖按键序列。
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

fn sleep(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console("day.sleep()")?;
    Ok(())
}

fn query_str(runtime: &mut LuaRuntime, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(expr)?.trim().to_owned())
}

fn query_num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let value = query_str(runtime, &format!("return tostring({expr})"))?;
    value
        .parse::<f64>()
        .map_err(|error| format!("{value:?} is not a number: {error}").into())
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

fn assert_plot_crop(
    runtime: &mut LuaRuntime,
    cell: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "local m = map.state('farm_plots')\n\
             local p = type(m) == 'table' and m['{cell}'] or nil\n\
             return p and tostring(p.crop) or 'nil'"
        ),
    )?;
    if got != expected {
        return Err(format!("plot {cell} crop = {got:?}, want {expected:?}").into());
    }
    Ok(())
}

fn assert_plot_days(
    runtime: &mut LuaRuntime,
    cell: &str,
    expected: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = query_str(
        runtime,
        &format!(
            "local m = map.state('farm_plots')\n\
             local p = type(m) == 'table' and m['{cell}'] or nil\n\
             return tostring((p and p.watered_days) or -1)"
        ),
    )?;
    let got: f64 = raw
        .parse()
        .map_err(|error| format!("{raw:?} is not a number: {error}"))?;
    if (got - expected).abs() > f64::EPSILON {
        return Err(format!("plot {cell} watered_days = {got:?}, want {expected}").into());
    }
    Ok(())
}

/// 农田图层（FarmGrid 土面 / CropGrid 作物）某格的 tile 值（0 = 空）。这是玩家真正
/// 看见的东西：只清 map.state 而不重绘会留下"看得见但存不下"的幽灵农田。
fn assert_farm_tile(
    runtime: &mut LuaRuntime,
    layer: &str,
    expected: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = query_str(
        runtime,
        &format!(
            "local grid = entity.find('{layer}')\n\
             if not grid then return '-1' end\n\
             return tostring(tilemap.get_cell(grid, {PLOT_X}, {PLOT_Y}))"
        ),
    )?;
    let got: i64 = raw
        .parse()
        .map_err(|error| format!("{raw:?} is not an integer: {error}"))?;
    if got != expected {
        return Err(format!("{layer}({PLOT_X},{PLOT_Y}) = {got}, want {expected}").into());
    }
    Ok(())
}

/// 断言一个**语句块**的返回值（块自带 `return`），避免把多语句块塞进表达式包装。
fn assert_block(
    runtime: &mut LuaRuntime,
    block: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(runtime, block)?;
    if got != expected {
        return Err(format!("{block} = {got:?}, want {expected:?}").into());
    }
    Ok(())
}
