//! Headless acceptance drive for M17-03 《江湖有田》对话系统与任务状态机.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M17-03 acceptance paths:
//!
//! - 纯数据注册：任务/对话全部来自 `data/quests/*` + `data/dialogues/*`，加载期校验通过。
//! - NPC 交互开对话：E 键经 interaction 目标命中 NPC，进入 Dialogue 模式并渲染选项。
//! - 条件过滤：选项按任务五态/变量/物品条件出现或消失（不满足即不可见）。
//! - 选项副作用：接取任务、`add_var` 推进目标、`deliver_quest` 交付。
//! - 五态覆盖：not_started / in_progress / deliverable / completed / failed。
//! - 幂等奖励：重复交付返回 `already_completed`，奖励不重复发放。
//! - 读档恢复：任务记录、全局变量、金钱/背包随存档一致。
//! - 副作用可回溯：变量写入带来源与事件流水。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m17_dialogue [-- <game_jhyt-root>]

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
/// 10:00：顾长风在剑坪、沈青萝在医馆。
const TEN_AM: i64 = 600;

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
    let save_path = fresh_save_path(&project, "jhyt_m17_dialogue.sav");
    let fail_save_path = fresh_save_path(&project, "jhyt_m17_dialogue_fail.sav");
    let project_arg = project.clone();

    banner("M17-03a 纯数据注册 + 加载期校验：任务/对话均来自 data/*");
    let mut runtime = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut runtime, &main_script)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_mode(&mut runtime, "Playing")?;
    // 运行时注册表条目数 == 纯数据注册表条目数（数据驱动，内容增长不会顶掉断言）。
    let quest_registry = query_num(&mut runtime, "return #require('data.quests.index').ids")?;
    assert_num(
        &mut runtime,
        "#(require('world.quest').ids())",
        quest_registry,
    )?;
    let dialogue_registry = query_num(&mut runtime, "return #require('data.dialogues.index').ids")?;
    assert_num(
        &mut runtime,
        "#(require('world.dialogue').ids())",
        dialogue_registry,
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').get('gu_changfeng_teach').speaker",
        "npc_gu_changfeng",
    )?;
    assert_str(
        &mut runtime,
        "require('world.quest').get('quest_herb').giver",
        "npc_shen_qingluo",
    )?;
    // 数据条件未被运行时裁剪：顾长风 greet 节点仍保留 M17-03 的 5 个数据选项
    // （M19-01 只追加条件选项，故这里断言「不少于 5」而不是精确数量）。
    assert_str(
        &mut runtime,
        "tostring(#(require('world.dialogue').get('gu_changfeng_teach').nodes.greet.options) >= 5)",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').find_for_npc('npc_shen_qingluo')",
        "shen_qingluo_herb",
    )?;
    assert_validate(&mut runtime, "world.quest")?;
    assert_validate(&mut runtime, "world.dialogue")?;

    banner("M17-03b NPC 交互开对话 + 条件过滤：quest_spar 未接取");
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_spar')",
        "not_started",
    )?;
    lua(&mut runtime, "return require('world.weather').set('sunny')")?;
    lua(&mut runtime, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut runtime, 1)?;
    talk(&mut runtime, "npc_gu_changfeng")?;
    assert_mode(&mut runtime, "Dialogue")?;
    assert_str(&mut runtime, "require('world.dialogue').is_open()", "true")?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').dialogue_id()",
        "gu_changfeng_teach",
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().node",
        "greet",
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().speaker_name",
        "顾长风",
    )?;
    // 未接取态：「接取」仍排第一（脚本按数字键 1 接取），「告辞」可见，
    // in_progress/deliverable/completed 选项不可见。数量与第 2 项随 M19-01 追加的
    // 条件选项（主线/订单入口）变化，故只做语义断言。
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().options[1]",
        "愿随师父走一遍基础三式。",
    )?;
    assert_option(&mut runtime, "改日再来请教。", true)?;
    assert_option(&mut runtime, "三式已经走完。", false)?;

    banner("M17-03c 选项副作用：接取任务 + add_var 推进目标（in_progress）");
    press(
        &mut runtime,
        KeyCode::from_char('1').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_spar')",
        "in_progress",
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().node",
        "accept",
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('1').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().node",
        "practice",
    )?;
    assert_num(
        &mut runtime,
        "require('core.state').get_var('sword_drills')",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_spar')",
        "in_progress",
    )?;

    banner("M17-03d 目标达成 → deliverable → 交付 completed + 奖励 + on_complete");
    press(
        &mut runtime,
        KeyCode::from_char('1').expect("QWERTY 字符必须有对应物理键"),
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('1').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(
        &mut runtime,
        "require('core.state').get_var('sword_drills')",
        3.0,
    )?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_spar')",
        "deliverable",
    )?;
    // 可交付态：in_progress 选项消失，交付选项上浮。
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().options[1]",
        "三式已经走完。",
    )?;
    assert_option(&mut runtime, "愿随师父走一遍基础三式。", false)?;
    press(
        &mut runtime,
        KeyCode::from_char('1').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_spar')",
        "completed",
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().node",
        "done",
    )?;
    assert_num(&mut runtime, "require('core.state').inner_power", 60.0)?;
    assert_str(
        &mut runtime,
        "require('core.state').get_var('sword_basics_learned')",
        "true",
    )?;

    banner("M17-03e 幂等：重复交付返回 already_completed，奖励不重复发放");
    assert_str(
        &mut runtime,
        "select(2, require('world.quest').deliver('quest_spar'))",
        "already_completed",
    )?;
    assert_num(&mut runtime, "require('core.state').inner_power", 60.0)?;
    assert_num(&mut runtime, "require('core.state').money", 120.0)?;
    // Esc 结束对话，回到 Playing。
    press(&mut runtime, KeyCode::Escape)?;
    assert_str(&mut runtime, "require('world.dialogue').is_open()", "false")?;
    assert_mode(&mut runtime, "Playing")?;

    banner("M17-03f item 任务：条件不满足时交付选项不出现 / objectives_unmet");
    // 山庄南口 → 云泽村（复用 M17-02 的跨图路径）。
    teleport(&mut runtime, MAIN_EXIT)?;
    press(&mut runtime, KeyCode::KeyE)?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "village")?;
    assert_map_current(&mut runtime, "village")?;
    lua(&mut runtime, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut runtime, 2)?;
    assert_num(&mut runtime, "inv.count('player', 'lingzhi')", 0.0)?;
    talk(&mut runtime, "npc_shen_qingluo")?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').dialogue_id()",
        "shen_qingluo_herb",
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().node",
        "greet",
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().options[1]",
        "愿为医馆采三株灵芝。",
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('1').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_herb')",
        "in_progress",
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().node",
        "accept",
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('1').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(&mut runtime, "require('world.dialogue').is_open()", "false")?;
    // 空背包：目标未达成 → 交付被拒，且对话里只有「催问」与「告辞」。
    assert_str(
        &mut runtime,
        "select(2, require('world.quest').deliver('quest_herb'))",
        "objectives_unmet",
    )?;
    talk(&mut runtime, "npc_shen_qingluo")?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().options[1]",
        "灵芝还在采，稍待。",
    )?;
    assert_option(&mut runtime, "愿为医馆采三株灵芝。", false)?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().node",
        "greet",
    )?;

    banner("M17-03g item 交付：消耗目标物品 + item/money 奖励 + on_complete");
    press(&mut runtime, KeyCode::Escape)?;
    assert_mode(&mut runtime, "Playing")?;
    lua(&mut runtime, "inv.add('player', 'lingzhi', 3)")?;
    assert_num(&mut runtime, "inv.count('player', 'lingzhi')", 3.0)?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_herb')",
        "deliverable",
    )?;
    talk(&mut runtime, "npc_shen_qingluo")?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().options[1]",
        "三株灵芝已备齐。",
    )?;
    assert_option(&mut runtime, "灵芝还在采，稍待。", false)?;
    press(
        &mut runtime,
        KeyCode::from_char('1').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_herb')",
        "completed",
    )?;
    assert_str(
        &mut runtime,
        "require('world.dialogue').info().node",
        "done",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 170.0)?;
    assert_num(&mut runtime, "inv.count('player', 'lingzhi')", 0.0)?;
    assert_num(&mut runtime, "inv.count('player', 'rice')", 4.0)?;
    assert_str(
        &mut runtime,
        "require('core.state').get_var('herb_delivered')",
        "true",
    )?;
    // 幂等：奖励不因再次交付而翻倍。
    assert_str(
        &mut runtime,
        "select(2, require('world.quest').deliver('quest_herb'))",
        "already_completed",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 170.0)?;
    assert_num(&mut runtime, "inv.count('player', 'rice')", 4.0)?;
    press(&mut runtime, KeyCode::Escape)?;
    assert_mode(&mut runtime, "Playing")?;

    banner("M17-03h 副作用可回溯：变量写入带来源与事件流水");
    let events = query_str(
        &mut runtime,
        "local s = require('core.state'); local out = {}; \
         for i, e in ipairs(s.var_events) do \
           out[i] = tostring(e.var) .. '@' .. tostring(e.source) .. '#' .. tostring(e.event) \
         end; return #s.var_events .. '|' .. table.concat(out, ';')",
    )?;
    for needle in [
        "sword_drills@dialogue:gu_changfeng_teach#",
        "sword_basics_learned@quest:quest_spar#on_complete",
        "herb_delivered@quest:quest_herb#on_complete",
    ] {
        if !events.contains(needle) {
            return Err(format!("var_events missing {needle:?}: {events}").into());
        }
    }
    println!("var_events => {events}");

    banner("M17-03i 读档恢复：任务记录 / 全局变量 / 奖励一致");
    save_game(&mut runtime)?;
    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_scene_is(&mut second, "village")?;
    assert_map_current(&mut second, "village")?;
    assert_validate(&mut second, "world.quest")?;
    assert_validate(&mut second, "world.dialogue")?;
    assert_str(
        &mut second,
        "require('world.quest').state('quest_spar')",
        "completed",
    )?;
    assert_str(
        &mut second,
        "require('world.quest').state('quest_herb')",
        "completed",
    )?;
    assert_num(&mut second, "require('core.state').money", 170.0)?;
    assert_num(&mut second, "require('core.state').inner_power", 60.0)?;
    assert_num(
        &mut second,
        "require('core.state').get_var('sword_drills')",
        3.0,
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('sword_basics_learned')",
        "true",
    )?;
    assert_str(
        &mut second,
        "require('core.state').get_var('herb_delivered')",
        "true",
    )?;
    assert_num(&mut second, "inv.count('player', 'lingzhi')", 0.0)?;
    assert_num(&mut second, "inv.count('player', 'rice')", 4.0)?;
    // 读档后重复交付仍被拦截（完成态随存档恢复）。
    assert_str(
        &mut second,
        "select(2, require('world.quest').deliver('quest_spar'))",
        "already_completed",
    )?;
    let restored_events = query_str(
        &mut second,
        "return tostring(#require('core.state').var_events > 0)",
    )?;
    if restored_events != "true" {
        return Err(format!("var_events not restored: {restored_events}").into());
    }

    banner("M17-03j 五态覆盖：failed 态由任务状态机与对话条件共同生效");
    let mut third = new_runtime(&project_arg, &fail_save_path)?;
    enter_main_menu(&mut third, &main_script)?;
    click_button(&mut third, "BtnStart")?;
    assert_scene_is(&mut third, "manor")?;
    assert_str(
        &mut third,
        "require('world.quest').state('quest_spar')",
        "not_started",
    )?;
    lua(&mut third, "return require('world.weather').set('sunny')")?;
    lua(&mut third, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut third, 1)?;
    talk(&mut third, "npc_gu_changfeng")?;
    press(
        &mut third,
        KeyCode::from_char('1').expect("QWERTY 字符必须有对应物理键"),
    )?; // 接取
    assert_str(
        &mut third,
        "require('world.quest').state('quest_spar')",
        "in_progress",
    )?;
    press(
        &mut third,
        KeyCode::from_char('1').expect("QWERTY 字符必须有对应物理键"),
    )?; // 起手：sword_drills = 1
    assert_str(
        &mut third,
        "require('world.dialogue').info().node",
        "practice",
    )?;
    assert_str(
        &mut third,
        "tostring(require('world.quest').fail('quest_spar'))",
        "true",
    )?;
    assert_str(
        &mut third,
        "require('world.quest').state('quest_spar')",
        "failed",
    )?;
    // 失败态：practice 节点只剩「今日先到这里。」这一条兜底选项（数量随数据增长
    // 可能变化，故断言首选项文本而不锁死数量）。
    assert_str(
        &mut third,
        "require('world.dialogue').info().options[1]",
        "今日先到这里。",
    )?;
    assert_option(&mut third, "再练一遍。", false)?;
    assert_str(
        &mut third,
        "select(2, require('world.quest').deliver('quest_spar'))",
        "quest_failed",
    )?;
    assert_num(&mut third, "require('core.state').inner_power", 50.0)?;
    assert_str(
        &mut third,
        "select(2, require('world.quest').fail('quest_spar'))",
        "already_failed",
    )?;

    println!(
        "jhyt_m17_dialogue_ok project={} quests={} dialogues={} spar={} herb={} money={} inner_power={}",
        project.display(),
        query_num(&mut second, "return #require('world.quest').ids()")?,
        query_num(&mut second, "return #require('world.dialogue').ids()")?,
        query_str(&mut second, "require('world.quest').state('quest_spar')")?,
        query_str(&mut second, "require('world.quest').state('quest_herb')")?,
        query_num(&mut second, "return require('core.state').money")?,
        query_num(&mut second, "return require('core.state').inner_power")?,
    );
    let _ = fs::remove_file(&save_path);
    let _ = fs::remove_file(&fail_save_path);
    Ok(())
}

// ---------- helpers ----------

/// Creates a bare engine runtime pointing at the project scripts & save file.
fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
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

/// 把玩家传送到 NPC 的逻辑格心，再按 E 触发 interaction → 开对话。
fn talk(runtime: &mut LuaRuntime, persist_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "local q = require('world.npc').query('{persist_id}'); \
             if not q or not q.live then return 'no_npc' end; \
             require('player.player').teleport((q.cell_x + 0.5) * 32, (q.cell_y + 0.5) * 32); \
             return 'ok'"
        ),
    )?;
    if got != "ok" {
        return Err(format!("npc {persist_id} not interactable: {got}").into());
    }
    press(runtime, KeyCode::KeyE)?;
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

/// 对话选项语义断言：`text` 是否（不）在当前可选项里。
///
/// 选项数量随内容增长（M19-01 追加条件选项）变化，锁死数量会让示例被内容
/// 增长顶掉；这里只断言「目标选项可见 / 已消失」。
fn assert_option(
    runtime: &mut LuaRuntime,
    text: &str,
    expected: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let expr = format!(
        "(function() for _, t in ipairs(require('world.dialogue').info().options) do \
           if t == '{text}' then return 'visible' end end return 'hidden' end)()"
    );
    let got = query_str(runtime, &expr)?;
    let want = if expected { "visible" } else { "hidden" };
    if got != want {
        return Err(format!("dialogue option {text:?} = {got}, want {want}").into());
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
    let current = query_str(runtime, "require('world.map_manager').current()")?;
    if current != expected {
        return Err(format!("map.current() = {current:?}, want {expected:?}").into());
    }
    Ok(())
}

fn assert_mode(runtime: &mut LuaRuntime, expected: &str) -> Result<(), Box<dyn std::error::Error>> {
    assert_str(runtime, "require('core.state').mode", expected)
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

fn fresh_save_path(project: &Path, name: &str) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(name);
    let _ = fs::remove_file(&path);
    path
}
