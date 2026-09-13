//! Headless acceptance drive for M20-08 手柄重绑定与游戏手柄垂直验收.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and drives
//! a **scripted gamepad backend**（`GamepadBackend` + `GamepadPoller`）through the
//! whole M20-08 acceptance path:
//!
//! - ①设备契约与边沿：按钮/轴按 `host/gamepad.rs` 的 0..15 / 0..5 契约注入，
//!   按下走边沿语义（`input.action_pressed('confirm')` 只在按下那一帧为真）；
//! - ②默认映射与改绑：默认表里玩法动作只有键盘绑定 → `input.rebind` 把
//!   移动/交互/攻击/武学/背包/暂停/重开绑到手柄，**旧键失效、新键生效**；
//! - ③摇杆移动与朝向：左摇杆轴 1（上为正）驱动 `move_up`，死区内（原始 0.10 <
//!   设备死区 0.15）不动，0.6 生效并给出模拟量速度；
//! - ④A 交互（药田翻土）与 B 取消（打开背包后按 B 关闭）；
//! - ⑤肩键 LB/RB 开背包/暂停，且面板期间 UI 输入不下沉世界（速度保持 0）；
//! - ⑥对话与商店的手柄导航（DPad = ui_up/ui_down + 摇杆按住重复）与 A 确认、
//!   B 取消；
//! - ⑦战斗：手柄 X 普攻、Y 武学（内力消耗）、战败后 Back 键重开；
//! - ⑧`settings.toml` 往返：改绑 → `input.save_settings()`（保留 `[audio]` 段与
//!   注释）→ 新 runtime 启动时 `input.load_settings()` 生效；跑完仓库不残留；
//! - ⑨断连（`GamepadFrame::disconnected()`）清空按钮与轴、不粘键、可重连；
//! - ⑩编辑器预览等价路径：**每帧全新 `InputSnapshot`**（只桥接键鼠，不跨帧保留
//!   手柄）时，摇杆持续推住仍每帧都有输入，松杆即停；
//! - ⑪UI 操作说明两套：键鼠显示键位说明、手柄显示手柄说明，按最近使用的设备切换。
//!
//! 本示例用 `SettingsGuard` 备份/恢复 `<project>/settings.toml`（示例必须写入才
//! 能验证启动应用与保序写回），并在成功/失败路径下都还原仓库状态。
//!
//! 真实手柄的「设备 ↔ 动作层」由 M20-06 的 XInput 后端与设备层单测覆盖，本示例
//! 覆盖「动作层 ↔ 玩法」；实机插拔步骤见任务 M20-08 的未验证项。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m20_gamepad [-- <game_jhyt-root>]

use engine::core::{
    input::{InputKey, InputSnapshot},
    script::LuaRuntime,
};
use engine::host::gamepad::{
    GAMEPAD_BUTTON_COUNT, GamepadBackend, GamepadDeadzone, GamepadFrame, GamepadPoller,
    axes_from_raw_with, button, triggers_to_buttons,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// 示例写入的设置文件模板：注释 + `[audio]` 段 + **部分** `[input]` 映射
/// （缺的动作由引擎默认值补齐，验证「旧设置文件仍可玩」）。
const SETTINGS_TEMPLATE: &str = "# M20-08 手柄验收设置（示例结束时按原样恢复）\n[audio]\nmaster = 0.5\nbgm = 0.25\n\n[input]\nmap = \"rustlua-actions-v1\\ninteract=key:e\"\n";

/// 药田起步点（与 `jhyt_smoke` 一致）：格 (16,3)，朝上面前 (16,4) 落在药田内。
const FARM_START: (f32, f32) = (528.0, 112.0);
/// 第二块未耕作药田（格 (17,3)，朝上面前 (17,4)）：断连重连后再劳作一次。
const FARM_START_2: (f32, f32) = (560.0, 112.0);
/// 演武场：玩家站位（格 13,5）面向右即命中训练木人（格 14,5）。
const PLAYER_BEFORE_DUMMY: (f32, f32) = (432.0, 176.0);
/// 摇杆原始值 0.6 经默认死区 0.15 重映射后的模拟量：`(0.6-0.15)/0.85`。
const EXPECTED_STICK_06: f64 = (0.6 - 0.15) / 0.85;
/// 移动速度（`input.lua` 的 MOVE_SPEED，世界单位/秒）。
const MOVE_SPEED: f64 = 140.0;
/// 玩家刚体速度查询（`physics.get_velocity` 返回数组式表：1=x、2=y）。
const VELOCITY_X: &str =
    "(function() local v = physics.get_velocity(require('core.state').player); return v[1] end)()";
const VELOCITY_Y: &str =
    "(function() local v = physics.get_velocity(require('core.state').player); return v[2] end)()";

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
    let settings_path = project.join("settings.toml");
    let settings_existed = settings_path.exists();
    // 备份/恢复 settings.toml：示例写入的绑定不得污染仓库（与 jhyt_m20_audio 同套路）。
    let guard = SettingsGuard::capture(&settings_path)?;
    fs::write(&settings_path, SETTINGS_TEMPLATE)?;

    let save_path = fresh_save_path(&project, "jhyt_m20_gamepad.sav");
    let mut runtime = new_runtime(&project, &save_path)?;
    enter_playable(&mut runtime, &main_script)?;
    // 脚本化手柄：`Arc<Mutex<RawPad>>` 保存当前设备帧，测试随时改按钮/轴。
    let pad = Pad::new(GamepadDeadzone::DEFAULT);
    let mut poller = GamepadPoller::new(Box::new(ScriptedPad {
        state: Arc::clone(&pad.state),
        deadzone: GamepadDeadzone::DEFAULT,
    }));
    let mut snapshot = InputSnapshot::default();

    banner("M20-08a 设备契约与边沿：按钮/轴按 0..15 / 0..5 契约注入，按下走边沿");
    assert_str(&mut runtime, "input.pad_button(0)", "false")?;
    assert_num(&mut runtime, "input.pad_axis(1)", 0.0)?;
    pad.press(button::A);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    assert_str(&mut runtime, "input.pad_button(0)", "true")?;
    assert_str(&mut runtime, "input.action('confirm')", "true")?;
    assert_str(&mut runtime, "input.action_pressed('confirm')", "true")?;
    // 持续按住：held 为真、边沿只在第一帧（快速点按不丢由设备层单测覆盖）。
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    assert_str(&mut runtime, "input.action('confirm')", "true")?;
    assert_str_why(
        &mut runtime,
        "input.action_pressed('confirm')",
        "false",
        "按住不应重复触发按下边沿",
    )?;
    pad.release(button::A);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    assert_str(&mut runtime, "input.action_released('confirm')", "true")?;
    assert_str(&mut runtime, "input.pad_button(0)", "false")?;
    // 扳机模拟量 ≥0.5 时派生数字按钮 6/7（设备契约）。
    pad.stick_trigger(1.0, 0.0);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    assert_str(&mut runtime, "input.pad_button(6)", "true")?;
    assert_str(&mut runtime, "input.pad_button(7)", "false")?;
    assert_range(&mut runtime, "input.pad_axis(4)", 0.9, 1.0)?;
    pad.stick_trigger(0.0, 0.0);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    assert_str(&mut runtime, "input.pad_button(6)", "false")?;
    assert_str(&mut runtime, "input.pad_axis(4)", "0.0")?;
    println!("device backend={} buttons=16 axes=6", poller.backend_name());
    assert_eq!(poller.backend_name(), "scripted");

    banner("M20-09 UI 操作说明两套：键鼠显示键位说明、手柄显示手柄说明（按最近使用的设备切换）");
    // 顶部提示（HUD `hud:keys`）与 `input.key_hints()` 同源，直接断言渲染出的文案。
    let hint_reader =
        "(function() local t = ui.get('hud:keys'); return t and tostring(t.text) or 'nil' end)()";
    // ① 键鼠一套：WASD / E / J…（从当前绑定推导）。
    assert_str_why(
        &mut runtime,
        "tostring(require('ui.input').set_device('keyboard'))",
        "keyboard",
        "可显式指定当前说明设备（测试用）",
    )?;
    lua(&mut runtime, "require('ui.input').update()")?;
    let keyboard_hint = query_str(&mut runtime, hint_reader)?;
    for needle in ["W/A/S/D 移动", "E 交互", "J 攻击", "K 武学", "I 背包"] {
        if !keyboard_hint.contains(needle) {
            return Err(format!("键鼠说明应含 {needle:?}：{keyboard_hint}").into());
        }
    }
    if keyboard_hint.contains("左摇杆") || keyboard_hint.contains("手柄") {
        return Err(format!("键鼠说明里不应出现手柄字样：{keyboard_hint}").into());
    }
    // ② 手柄一套：中文字柄名（左摇杆/DPad、A、B…），且不混入键盘标签。
    assert_str(
        &mut runtime,
        "tostring(require('ui.input').set_device('gamepad'))",
        "gamepad",
    )?;
    lua(&mut runtime, "require('ui.input').update()")?;
    let gamepad_hint = query_str(&mut runtime, hint_reader)?;
    for needle in ["左摇杆/DPad 移动", "A 确认", "B 取消"] {
        if !gamepad_hint.contains(needle) {
            return Err(format!("手柄说明应含 {needle:?}：{gamepad_hint}").into());
        }
    }
    if gamepad_hint.contains("W/A/S/D 移动") || gamepad_hint.contains("E 交互") {
        return Err(format!("手柄说明里不应出现键盘标签：{gamepad_hint}").into());
    }
    // ③ 设备自动检测：推左摇杆 → 切到手柄说明；按键盘 W → 切回键鼠说明。
    lua(&mut runtime, "require('ui.input').set_device(nil)")?;
    pad.stick(0.0, 0.9);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    lua(&mut runtime, "require('ui.input').update()")?;
    assert_str_why(
        &mut runtime,
        "tostring(require('ui.input').device())",
        "gamepad",
        "推摇杆后应自动切到手柄说明",
    )?;
    pad.stick(0.0, 0.0);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    // 键盘侧同理：把 W 喂在输入快照里跑一帧（`input.update()` 同帧看到按键）。
    let mut held_w = InputSnapshot::default();
    held_w.key_event(InputKey::from_char('w'), true);
    runtime.set_input(held_w);
    lua(&mut runtime, "require('ui.input').update()")?;
    assert_str_why(
        &mut runtime,
        "tostring(require('ui.input').device())",
        "keyboard",
        "按键盘后应自动切回键鼠说明",
    )?;
    runtime.set_input(InputSnapshot::default());
    println!("keyboard hint: {keyboard_hint}");
    println!("gamepad  hint: {gamepad_hint}");

    banner("M20-09b 面板提示两套 + 导航手感可配：对话/商店/背包/任务/暂停按设备换文案");
    // 面板底部提示不能 require `ui.input`（`input → dialogue` 成环），统一走
    // `ui.hints`：这里断言渲染出的实际文案，而不是断言函数返回值。
    let panel_reader =
        "(function() local t = ui.get('dlg:hint'); return t and tostring(t.text) or 'nil' end)()";
    lua(
        &mut runtime,
        "require('world.dialogue').start('gu_changfeng_teach')",
    )?;
    lua(&mut runtime, "require('ui.hints').set_device('keyboard')")?;
    lua(&mut runtime, "require('world.dialogue').render()")?;
    let keyboard_panel = query_str(&mut runtime, panel_reader)?;
    for needle in [
        "W/S 或 方向键 选择",
        "1/2/3/4/5 直选",
        "空格 确认",
        "Esc 结束",
    ] {
        if !keyboard_panel.contains(needle) {
            return Err(format!("键鼠面板提示应含 {needle:?}：{keyboard_panel}").into());
        }
    }
    lua(&mut runtime, "require('ui.hints').set_device('gamepad')")?;
    lua(&mut runtime, "require('world.dialogue').render()")?;
    let gamepad_panel = query_str(&mut runtime, panel_reader)?;
    for needle in ["左摇杆/DPad 选择", "A 确认", "B 结束"] {
        if !gamepad_panel.contains(needle) {
            return Err(format!("手柄面板提示应含 {needle:?}：{gamepad_panel}").into());
        }
    }
    if gamepad_panel.contains("W/S") || gamepad_panel.contains("空格") {
        return Err(format!("手柄面板提示里不应出现键盘文案：{gamepad_panel}").into());
    }
    println!("dialogue hint (keyboard): {keyboard_panel}");
    println!("dialogue hint (gamepad):  {gamepad_panel}");
    // 切换设备只影响下一帧绘制，收尾前恢复自动检测。
    lua(&mut runtime, "require('ui.hints').set_device(nil)")?;
    lua(&mut runtime, "require('world.dialogue').close(true)")?;
    // 导航手感：settings.toml 的 [input] 三行会被启动 `apply_nav_settings()` 应用。
    assert_num(&mut runtime, "input.nav_settings().threshold", 0.5)?;
    assert_num(&mut runtime, "input.nav_settings().delay_frames", 16.0)?;
    assert_num(&mut runtime, "input.nav_settings().repeat_frames", 8.0)?;
    fs::write(
        &settings_path,
        "# M20-09 导航手感（示例结束时按原样恢复）\n[input]\nnav_threshold = 0.25\nnav_delay_frames = 4\nnav_repeat_frames = 2\n",
    )?;
    lua(&mut runtime, "require('ui.input').apply_nav_settings()")?;
    assert_num(
        &mut runtime,
        "require('ui.input').nav_settings().threshold",
        0.25,
    )?;
    assert_num(
        &mut runtime,
        "require('ui.input').nav_settings().delay_frames",
        4.0,
    )?;
    assert_num(
        &mut runtime,
        "require('ui.input').nav_settings().repeat_frames",
        2.0,
    )?;
    // 脏值由引擎钳制（间隔至少 1 帧、帧数上限 120、阈值 0..1）。
    fs::write(
        &settings_path,
        "[input]\nnav_threshold = 5.0\nnav_delay_frames = 999\nnav_repeat_frames = 0\n",
    )?;
    lua(&mut runtime, "require('ui.input').apply_nav_settings()")?;
    assert_num(
        &mut runtime,
        "require('ui.input').nav_settings().threshold",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('ui.input').nav_settings().delay_frames",
        120.0,
    )?;
    assert_num(
        &mut runtime,
        "require('ui.input').nav_settings().repeat_frames",
        1.0,
    )?;
    // 恢复示例模板（后续章节仍按 M20-08 的 settings.toml 预期断言）。
    fs::write(&settings_path, SETTINGS_TEMPLATE)?;
    lua(&mut runtime, "require('ui.input').apply_nav_settings()")?;

    banner("M20-08b 默认映射与改绑：默认玩法动作只有键盘绑定，改绑后旧键失效新键生效");
    // 默认表：玩法动作是键盘绑定（手柄默认只覆盖 confirm/cancel/ui_*）。
    assert_num(&mut runtime, "#input.bindings('interact')", 1.0)?;
    assert_str(&mut runtime, "input.bindings('interact')[1]", "key:e")?;
    assert_num(&mut runtime, "#input.bindings('move_up')", 1.0)?;
    assert_str(&mut runtime, "input.bindings('move_up')[1]", "key:w")?;
    // 玩家改键（settings.toml 的 [input] 段就是这份映射）：手柄按钮/轴 + 摇杆。
    for (action, binding) in [
        ("move_up", "padaxis:1+"),
        ("move_down", "padaxis:1-"),
        ("move_left", "padaxis:0-"),
        ("move_right", "padaxis:0+"),
        ("interact", "pad:0"),
        ("attack", "pad:2"),
        ("skill", "pad:3"),
        ("inventory", "pad:4"),
        ("pause", "pad:5"),
        ("respawn", "pad:8"),
    ] {
        assert_str(
            &mut runtime,
            &format!("select(1, input.rebind('{action}', '{binding}'))"),
            "true",
        )?;
    }
    // 摇杆轴已改绑给移动 → 解除 ui_* 上重复的轴绑定（保留方向键与 DPad），
    // 让映射保持无冲突（编辑器「项目设置 → 输入」的冲突校验即看这个）。
    for (action, binding) in [
        ("ui_up", "padaxis:1+"),
        ("ui_down", "padaxis:1-"),
        ("ui_left", "padaxis:0-"),
        ("ui_right", "padaxis:0+"),
    ] {
        assert_str(
            &mut runtime,
            &format!("tostring(input.unbind('{action}', '{binding}'))"),
            "true",
        )?;
    }
    assert_str(&mut runtime, "input.bindings('interact')[1]", "pad:0")?;
    assert_num(&mut runtime, "#input.bindings('interact')", 1.0)?;
    // 旧键失效：键盘 E 不再触发交互，W 不再移动。
    press_key(&mut runtime, InputKey::from_char('e'))?;
    assert_ne_str(&mut runtime, "require('core.state').last_action", "till")?;
    assert_num(&mut runtime, VELOCITY_X, 0.0)?;
    // 新键生效：手柄 A 触发交互（见下一章的药田断言）。
    assert_str(&mut runtime, "input.action('interact')", "false")?;

    banner("M20-08c 摇杆移动与朝向：死区内不动、越死区生效且保留模拟量");
    lua(
        &mut runtime,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'down'",
            FARM_START.0, FARM_START.1
        ),
    )?;
    // 回中：速度归零（无输入时 stop）。
    pad.stick(0.0, 0.0);
    pad_ticks(&mut runtime, &mut poller, &mut snapshot, 2)?;
    assert_num(&mut runtime, VELOCITY_X, 0.0)?;
    assert_num(&mut runtime, VELOCITY_Y, 0.0)?;
    // 死区内：原始 0.10 < 设备死区 0.15 → 轴归零 → 不移动、不改朝向。
    pad.stick(0.0, 0.10);
    pad_ticks(&mut runtime, &mut poller, &mut snapshot, 3)?;
    assert_str(&mut runtime, "input.pad_axis(1)", "0.0")?;
    assert_str(&mut runtime, "input.action('move_up')", "false")?;
    assert_num(&mut runtime, VELOCITY_Y, 0.0)?;
    assert_str(&mut runtime, "require('core.state').facing", "down")?;
    // 越过死区：原始 0.6 → (0.6-0.15)/0.85 ≈ 0.529 → 上移（轴 1 上为正）。
    pad.stick(0.0, 0.6);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    assert_str(&mut runtime, "input.action('move_up')", "true")?;
    assert_str(&mut runtime, "require('core.state').facing", "up")?;
    assert_str(&mut runtime, "require('core.state').last_action", "move")?;
    let velocity = player_velocity(&mut runtime)?;
    let expected_y = EXPECTED_STICK_06 * MOVE_SPEED;
    if (velocity.1 - expected_y).abs() > 0.5 {
        return Err(format!(
            "摇杆纵向速度 {:.3} 应约等于 {expected_y:.3}（模拟量未被保留？）",
            velocity.1
        )
        .into());
    }
    if velocity.0.abs() > 1e-6 {
        return Err(format!("纵向推杆不应产生横向速度：{velocity:?}").into());
    }
    println!(
        "stick raw=0.60 -> axis={} velocity_y={:.2}",
        query_str(&mut runtime, "return input.pad_axis(1)")?,
        velocity.1
    );
    // 左推：轴 0 为负 → 左移并改朝向。
    pad.stick(-0.6, 0.0);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    assert_str(&mut runtime, "input.action('move_left')", "true")?;
    assert_str(&mut runtime, "require('core.state').facing", "left")?;
    let velocity = player_velocity(&mut runtime)?;
    if velocity.0 >= 0.0 {
        return Err(format!("左推摇杆应产生负向速度：{velocity:?}").into());
    }
    // 斜向：合成长度 >1 时缩放到 1（单位向量 → 各分量约 0.707×速度）。
    pad.stick(-1.0, 1.0);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    let diagonal = player_velocity(&mut runtime)?;
    let component = 0.5_f64.sqrt() * MOVE_SPEED;
    if (diagonal.0 + component).abs() > 1.0 || (diagonal.1 - component).abs() > 1.0 {
        return Err(format!("斜向速度未归一化到单位长度：{diagonal:?}").into());
    }
    // 松杆：停机（不滑行）。
    pad.stick(0.0, 0.0);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    assert_num(&mut runtime, VELOCITY_X, 0.0)?;
    assert_num(&mut runtime, VELOCITY_Y, 0.0)?;

    banner("M20-08d A 交互（药田翻土）与 B 取消（打开背包后按 B 关闭）");
    lua(
        &mut runtime,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'",
            FARM_START.0, FARM_START.1
        ),
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 100.0)?;
    pad_press(&mut runtime, &mut poller, &mut snapshot, &pad, button::A)?;
    assert_str(&mut runtime, "require('core.state').last_action", "till")?;
    assert_num(&mut runtime, "require('core.state').energy", 92.0)?;
    // B 取消：LB 开背包 → B 关闭（cancel = Esc/B）。
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::LEFT_SHOULDER,
    )?;
    assert_mode(&mut runtime, "Inventory")?;
    pad_press(&mut runtime, &mut poller, &mut snapshot, &pad, button::B)?;
    assert_mode(&mut runtime, "Playing")?;
    assert_str(
        &mut runtime,
        "require('ui.inventory_ui').ui_active()",
        "false",
    )?;

    banner("M20-08e 肩键 LB/RB：背包/暂停开关 + 面板期间 UI 输入不下沉世界");
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::LEFT_SHOULDER,
    )?;
    assert_mode(&mut runtime, "Inventory")?;
    // 面板期间推摇杆：世界速度必须保持 0（update_movement 未被调用）。
    pad.stick(0.0, 0.9);
    pad_ticks(&mut runtime, &mut poller, &mut snapshot, 3)?;
    assert_num(&mut runtime, VELOCITY_Y, 0.0)?;
    assert_str(&mut runtime, "require('core.state').facing", "up")?;
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::LEFT_SHOULDER,
    )?;
    assert_mode(&mut runtime, "Playing")?;
    pad.stick(0.0, 0.0);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::RIGHT_SHOULDER,
    )?;
    assert_mode(&mut runtime, "Pause")?;
    // 暂停菜单里 DPad 下移 → 选中项变化（走 ui_down 动作）。
    assert_num(&mut runtime, "require('ui.pause_menu').index()", 0.0)?;
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::DPAD_DOWN,
    )?;
    assert_num(&mut runtime, "require('ui.pause_menu').index()", 1.0)?;
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::DPAD_UP,
    )?;
    assert_num(&mut runtime, "require('ui.pause_menu').index()", 0.0)?;
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::RIGHT_SHOULDER,
    )?;
    assert_mode(&mut runtime, "Playing")?;

    banner("M20-08f 对话与商店：DPad/摇杆导航 + A 确认 + B 取消");
    assert_str(
        &mut runtime,
        "tostring(require('world.dialogue').start('gu_changfeng_teach'))",
        "true",
    )?;
    assert_mode(&mut runtime, "Dialogue")?;
    assert_str(&mut runtime, "require('world.dialogue').node()", "greet")?;
    assert_num(&mut runtime, "require('world.dialogue').selected()", 1.0)?;
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::DPAD_DOWN,
    )?;
    assert_num(&mut runtime, "require('world.dialogue').selected()", 2.0)?;
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::DPAD_UP,
    )?;
    assert_num(&mut runtime, "require('world.dialogue').selected()", 1.0)?;
    // 摇杆导航：模拟摇杆没有按下边沿，玩法层用「首次立即 + 按住延迟 16 帧 +
    // 每 8 帧重复」（`input.lua` 的 ui_step/stick_step），避免每帧滚动。
    pad.stick(0.0, -0.9);
    pad_ticks(&mut runtime, &mut poller, &mut snapshot, 2)?;
    assert_num_why(
        &mut runtime,
        "require('world.dialogue').selected()",
        2.0,
        "摇杆下推应首次立即走一步",
    )?;
    // 按住 13 帧（累计 15 帧）仍未到重复延迟 → 停在原处。
    pad_ticks(&mut runtime, &mut poller, &mut snapshot, 13)?;
    assert_num_why(
        &mut runtime,
        "require('world.dialogue').selected()",
        2.0,
        "未到按住重复延迟不应继续移动",
    )?;
    let options = query_num(
        &mut runtime,
        "return require('world.dialogue').option_limit()",
    )?;
    let next = if 2.0 >= options { 1.0 } else { 3.0 };
    pad_ticks(&mut runtime, &mut poller, &mut snapshot, 1)?;
    assert_num_why(
        &mut runtime,
        "require('world.dialogue').selected()",
        next,
        "第 16 帧到达重复延迟应再走一步",
    )?;
    let after_repeat = if next >= options { 1.0 } else { next + 1.0 };
    pad_ticks(&mut runtime, &mut poller, &mut snapshot, 8)?;
    assert_num_why(
        &mut runtime,
        "require('world.dialogue').selected()",
        after_repeat,
        "按住后每 8 帧重复一步",
    )?;
    pad.stick(0.0, 0.0);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;
    lua(&mut runtime, "require('world.dialogue').select(1)")?;
    pad_press(&mut runtime, &mut poller, &mut snapshot, &pad, button::A)?;
    assert_str(&mut runtime, "require('world.dialogue').node()", "accept")?;
    assert_mode(&mut runtime, "Dialogue")?;
    pad_press(&mut runtime, &mut poller, &mut snapshot, &pad, button::B)?;
    assert_mode(&mut runtime, "Playing")?;
    assert_str(&mut runtime, "require('world.dialogue').is_open()", "false")?;

    // 商店：进入营业时间 → 打开面板 → DPad 选择 → A 购买 → B 关闭。
    lua(
        &mut runtime,
        "clock.set(1, 600); require('core.state').money = 500",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('systems.shop').open_ui('zhao_stall'))",
        "true",
    )?;
    assert_mode(&mut runtime, "Shop")?;
    assert_num(&mut runtime, "require('systems.shop').selected()", 1.0)?;
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::DPAD_DOWN,
    )?;
    assert_num(&mut runtime, "require('systems.shop').selected()", 2.0)?;
    pad_press(
        &mut runtime,
        &mut poller,
        &mut snapshot,
        &pad,
        button::DPAD_UP,
    )?;
    assert_num(&mut runtime, "require('systems.shop').selected()", 1.0)?;
    let money_before = query_num(&mut runtime, "return require('core.state').money")?;
    pad_press(&mut runtime, &mut poller, &mut snapshot, &pad, button::A)?;
    let money_after = query_num(&mut runtime, "return require('core.state').money")?;
    if money_after >= money_before {
        return Err(
            format!("手柄确认应完成一次购买：money {money_before} -> {money_after}").into(),
        );
    }
    assert_str_why(
        &mut runtime,
        "require('core.state').last_action:sub(1, 4)",
        "buy_",
        "购买结果写入 last_action（HUD 反馈同源）",
    )?;
    // F 键（place）在商店里切换买卖页：手柄侧用同一动作（X 已绑到 attack 无关）。
    assert_str(&mut runtime, "require('systems.shop').tab()", "buy")?;
    press_key(&mut runtime, InputKey::from_char('f'))?;
    assert_str_why(
        &mut runtime,
        "require('systems.shop').tab()",
        "sell",
        "键盘 F 仍可切换买卖页（改绑未影响上下文动作）",
    )?;
    pad_press(&mut runtime, &mut poller, &mut snapshot, &pad, button::B)?;
    assert_mode(&mut runtime, "Playing")?;

    banner("M20-08g 战斗：手柄 X 普攻、Y 武学（内力消耗）、战败后 Back 重开");
    lua(&mut runtime, "scene.load('combat_arena')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "combat_arena")?;
    assert_mode(&mut runtime, "Playing")?;
    lua(
        &mut runtime,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'right'",
            PLAYER_BEFORE_DUMMY.0, PLAYER_BEFORE_DUMMY.1
        ),
    )?;
    frames(&mut runtime, 2)?;
    assert_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').hp",
        30.0,
    )?;
    // X = attack：徒手伤害 2。
    pad_press(&mut runtime, &mut poller, &mut snapshot, &pad, button::X)?;
    frames(&mut runtime, 30)?;
    assert_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').hp",
        28.0,
    )?;
    // Y = skill（千风掌）：内力 -10、固定伤害 6。
    assert_num(&mut runtime, "require('core.state').inner_power", 50.0)?;
    pad_press(&mut runtime, &mut poller, &mut snapshot, &pad, button::Y)?;
    frames(&mut runtime, 30)?;
    assert_num(&mut runtime, "require('core.state').inner_power", 40.0)?;
    assert_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').hp",
        22.0,
    )?;
    // 战败 → Back（respawn）重开：气血回满、回出生点、敌人复活。
    lua(&mut runtime, "require('systems.combat').set_player_hp(0)")?;
    frames(&mut runtime, 2)?;
    assert_str(
        &mut runtime,
        "require('systems.combat').is_player_dead()",
        "true",
    )?;
    // 战败态冻结移动：推杆不产生速度（只认 respawn）。
    pad.stick(0.0, 0.9);
    pad_ticks(&mut runtime, &mut poller, &mut snapshot, 2)?;
    assert_num(&mut runtime, VELOCITY_Y, 0.0)?;
    pad_press(&mut runtime, &mut poller, &mut snapshot, &pad, button::BACK)?;
    assert_num(&mut runtime, "require('systems.combat').player_hp()", 100.0)?;
    assert_str(
        &mut runtime,
        "require('systems.combat').is_player_dead()",
        "false",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').alive",
        "true",
    )?;
    pad.stick(0.0, 0.0);
    pad_tick(&mut runtime, &mut poller, &mut snapshot)?;

    banner("M20-08h settings.toml 往返：保存绑定（保序）→ 新 runtime 启动应用");
    assert_str_why(
        &mut runtime,
        "select(1, input.save_settings())",
        "true",
        "保存到 <project>/settings.toml",
    )?;
    let written = fs::read_to_string(&settings_path)?;
    for needle in [
        "# M20-08 手柄验收设置（示例结束时按原样恢复）",
        "[audio]",
        "master = 0.5",
        "interact=pad:0",
        "move_up=padaxis:1+",
        "attack=pad:2",
        "skill=pad:3",
        "inventory=pad:4",
        "pause=pad:5",
        "respawn=pad:8",
    ] {
        if !written.contains(needle) {
            return Err(format!("settings.toml 缺少 {needle:?}:\n{written}").into());
        }
    }
    if written.contains("ui_up=padaxis:1+") {
        return Err("解绑的 ui_up 轴绑定不应被写回".into());
    }
    println!("settings.toml after save:\n{written}");
    // 新 runtime：main.lua 的 boot_game → input.load_settings() 应用玩家改键，
    // 因此无需再 rebind，手柄 A 直接就能在药田翻土。
    let save_path2 = fresh_save_path(&project, "jhyt_m20_gamepad_boot.sav");
    let mut booted = new_runtime(&project, &save_path2)?;
    enter_playable(&mut booted, &main_script)?;
    let pad2 = Pad::new(GamepadDeadzone::DEFAULT);
    let mut poller2 = GamepadPoller::new(Box::new(ScriptedPad {
        state: Arc::clone(&pad2.state),
        deadzone: GamepadDeadzone::DEFAULT,
    }));
    let mut snapshot2 = InputSnapshot::default();
    assert_str(&mut booted, "input.bindings('interact')[1]", "pad:0")?;
    assert_str(&mut booted, "input.bindings('move_up')[1]", "padaxis:1+")?;
    assert_num(&mut booted, "#input.bindings('move_up')", 1.0)?;
    assert_str(&mut booted, "input.bindings('ui_up')[1]", "key:up")?;
    assert_str(&mut booted, "input.diagnostics()[1]", "nil")?;
    lua(
        &mut booted,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'",
            FARM_START.0, FARM_START.1
        ),
    )?;
    assert_num(&mut booted, "require('core.state').energy", 100.0)?;
    pad_press(&mut booted, &mut poller2, &mut snapshot2, &pad2, button::A)?;
    assert_str(&mut booted, "require('core.state').last_action", "till")?;
    assert_num(&mut booted, "require('core.state').energy", 92.0)?;

    banner("M20-08i 断连不粘键：拔线清空按钮与轴，重连后仍可操作");
    pad2.press(button::A);
    pad2.stick(0.0, 0.9);
    pad_tick(&mut booted, &mut poller2, &mut snapshot2)?;
    assert_str(&mut booted, "input.pad_button(0)", "true")?;
    assert_str(&mut booted, "input.action('move_up')", "true")?;
    pad2.disconnect();
    pad_tick(&mut booted, &mut poller2, &mut snapshot2)?;
    assert_str(&mut booted, "input.pad_button(0)", "false")?;
    assert_str(&mut booted, "input.pad_axis(1)", "0.0")?;
    assert_str(&mut booted, "input.action('confirm')", "false")?;
    assert_str(&mut booted, "input.action('move_up')", "false")?;
    assert_num(&mut booted, VELOCITY_Y, 0.0)?;
    // 断连期间点按不产生输入（没有设备）。
    pad_press(&mut booted, &mut poller2, &mut snapshot2, &pad2, button::A)?;
    assert_str(&mut booted, "input.action('confirm')", "false")?;
    // 重连：同一按键重新按下即生效（不粘键、不丢输入）。
    pad2.connect();
    pad2.stick(0.0, 0.0);
    pad_press(&mut booted, &mut poller2, &mut snapshot2, &pad2, button::A)?;
    assert_str(&mut booted, "input.pad_button(0)", "false")?;
    lua(
        &mut booted,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'",
            FARM_START_2.0, FARM_START_2.1
        ),
    )?;
    lua(&mut booted, "require('core.state').energy = 100")?;
    pad_press(&mut booted, &mut poller2, &mut snapshot2, &pad2, button::A)?;
    assert_str_why(
        &mut booted,
        "require('core.state').last_action",
        "till",
        "重连后手柄仍可交互",
    )?;

    banner("M20-08j 编辑器预览等价路径：每帧全新快照 + 摇杆持续推住仍持续有输入");
    // 编辑器 Play 预览的宿主语义（`preview_session` + `game_view::bridge_preview_input`）：
    // 每帧重建输入快照、只把键鼠写进去，运行时再把设备状态合进自己那份快照。
    // 因此「按住不放」必须**每帧都可见**，不能只写在轴值变化的那一帧——否则预览里
    // 推住摇杆从第二帧起就等于没有输入。
    pad2.release(button::A);
    lua(
        &mut booted,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'down'",
            FARM_START_2.0, FARM_START_2.1
        ),
    )?;
    // 原始 0.6 → 死区重映射后的模拟量，与 M20-08c 同一口径。
    pad2.stick(0.0, 0.6);
    for frame in 0..8 {
        let mut fresh = InputSnapshot::default();
        poller2.poll_into(&mut fresh);
        booted.set_input(fresh);
        booted.call_update()?;
        assert_str_why(
            &mut booted,
            "input.action('move_up')",
            "true",
            &format!("每帧全新快照：第 {} 帧仍应看到被推住的摇杆", frame + 1),
        )?;
        let velocity = player_velocity(&mut booted)?;
        let expected = EXPECTED_STICK_06 * MOVE_SPEED;
        if (velocity.1 - expected).abs() > 0.5 {
            return Err(format!(
                "第 {} 帧纵向速度应为模拟量速度 {expected:.3}，实际 {:.3}（按住状态未被持久化？）",
                frame + 1,
                velocity.1
            )
            .into());
        }
    }
    // 松杆：设备回中 → 下一帧立即停机（无残留按住状态）。
    pad2.stick(0.0, 0.0);
    let mut fresh = InputSnapshot::default();
    poller2.poll_into(&mut fresh);
    booted.set_input(fresh);
    booted.call_update()?;
    assert_num_why(
        &mut booted,
        VELOCITY_Y,
        0.0,
        "松杆后应停机（无残留按住状态）",
    )?;

    banner("M20-09c 标题界面手柄导航：DPad/摇杆选按钮 + A 确认走 on_click");
    // `main.scene` 的按钮原本只有鼠标 click 通路（`UiEvents event=click`），标题界面的
    // 手柄/键盘导航由 `ui.title_menu` 补上：焦点环选按钮、confirm 触发同一个 on_click。
    let title_save = fresh_save_path(&project, "jhyt_m20_gamepad_title.sav");
    let mut title = new_runtime(&project, &title_save)?;
    title.execute_project_script(&main_script)?;
    // 启动场景由包/工程声明，headless 显式切到门面 main（等同启动流程）。
    lua(&mut title, "scene.load('main')")?;
    title.process_scene_transition()?;
    assert_scene_is(&mut title, "main")?;
    assert_num_why(
        &mut title,
        "(function() local m = require('ui.title_menu') m.reset() return m.index() end)()",
        1.0,
        "标题菜单默认选中第一项「开始新游戏」",
    )?;
    let mut title_snapshot = InputSnapshot::default();
    let mut title_poller = GamepadPoller::new(Box::new(ScriptedPad {
        state: Arc::clone(&pad.state),
        deadzone: GamepadDeadzone::DEFAULT,
    }));
    pad.stick(0.0, 0.0);
    // 空闲一帧：建立焦点环与设备基线。
    pad_tick(&mut title, &mut title_poller, &mut title_snapshot)?;
    // DPad 下移一次 → 第二项（摇杆/方向键同理，都经 `ui.input.ui_step()`）。
    pad.press(button::DPAD_DOWN);
    pad_tick(&mut title, &mut title_poller, &mut title_snapshot)?;
    assert_str_why(
        &mut title,
        "require('ui.title_menu').selected_id()",
        "BtnContinue",
        "DPad 下移一次应对齐第二项「继续江湖」",
    )?;
    pad.release(button::DPAD_DOWN);
    pad_tick(&mut title, &mut title_poller, &mut title_snapshot)?;
    // 再下移一次 → 第三项「退出江湖」。
    pad.press(button::DPAD_DOWN);
    pad_tick(&mut title, &mut title_poller, &mut title_snapshot)?;
    assert_str(
        &mut title,
        "require('ui.title_menu').selected_id()",
        "BtnExit",
    )?;
    pad.release(button::DPAD_DOWN);
    pad_tick(&mut title, &mut title_poller, &mut title_snapshot)?;
    // A（confirm）落在「退出江湖」→ 同一个 on_click → app.quit()（不切场景）。
    // 手动推进（先喂设备帧再取方向），使断言与消费边沿的顺序确定。
    // 注意：Lua 侧 `app` 表只暴露 `quit()`；退出的可观测状态在宿主侧
    // `LuaRuntime::quit_requested()`，因此这里用 Rust 断言而不是 Lua 表达式。
    pad.press(button::A);
    title_poller.poll_into(&mut title_snapshot);
    title.set_input(title_snapshot.clone());
    let activated = query_str(
        &mut title,
        "(function() local id = require('ui.title_menu').update() \
         if id then on_click(id) end \
         return tostring(id) end)()",
    )?;
    println!("title trace (A on BtnExit): {activated}");
    assert_str_why(
        &mut title,
        "require('ui.title_menu').selected_id()",
        "BtnExit",
        "A 确认前应停在「退出江湖」",
    )?;
    assert!(
        title.quit_requested(),
        "A 确认应触发 on_click('BtnExit') → app.quit()（宿主侧退出标记）"
    );
    assert_scene_is(&mut title, "main")?;
    title_snapshot.finish_frame();
    title.set_input(InputSnapshot::default());
    // B（cancel）在标题界面 = 退出项。同时验证数字直选 2 → 「继续江湖」。
    let mut digit_input = InputSnapshot::default();
    digit_input.key_event(InputKey::from_char('2'), true);
    title.set_input(digit_input);
    title.call_update()?;
    assert_str_why(
        &mut title,
        "require('ui.title_menu').selected_id()",
        "BtnContinue",
        "数字键 2 直选第二项",
    )?;
    title.set_input(InputSnapshot::default());
    title.call_update()?;
    let _ = fs::remove_file(&title_save);

    // 收尾：恢复 settings.toml（原本不存在则删除），删除临时存档。
    drop(guard);
    if settings_path.exists() != settings_existed {
        return Err(format!(
            "settings.toml 残留状态不符：存在={} 期望={settings_existed}",
            settings_path.exists()
        )
        .into());
    }
    let _ = fs::remove_file(&save_path);
    let _ = fs::remove_file(&save_path2);

    println!(
        "jhyt_m20_gamepad_ok project={} backend={} bindings={} interact={} move_up={} chapters=13",
        project.display(),
        poller.backend_name(),
        query_num(&mut runtime, "return #input.actions()")?,
        query_str(&mut runtime, "return input.bindings('interact')[1]")?,
        query_str(&mut booted, "return input.bindings('move_up')[1]")?,
    );
    Ok(())
}

// ---------- 脚本化手柄后端 ----------

/// 设备原始状态（未过死区）：测试改这里，`poll()` 按 XInput 后端的同一套归一化
/// + 死区规则组装契约帧，因此死区/扳机派生行为与真机一致。
#[derive(Debug, Clone, Copy)]
struct RawPad {
    buttons: [bool; GAMEPAD_BUTTON_COUNT],
    left_x: f32,
    left_y: f32,
    right_x: f32,
    right_y: f32,
    left_trigger: f32,
    right_trigger: f32,
    connected: bool,
}

impl RawPad {
    fn connected_idle() -> Self {
        Self {
            buttons: [false; GAMEPAD_BUTTON_COUNT],
            left_x: 0.0,
            left_y: 0.0,
            right_x: 0.0,
            right_y: 0.0,
            left_trigger: 0.0,
            right_trigger: 0.0,
            connected: true,
        }
    }
}

/// `GamepadBackend` 实现：始终返回「当前」原始帧（按住/松开由测试显式切换）。
struct ScriptedPad {
    state: Arc<Mutex<RawPad>>,
    deadzone: GamepadDeadzone,
}

impl GamepadBackend for ScriptedPad {
    fn poll(&mut self) -> GamepadFrame {
        let raw = *self.state.lock().expect("pad state lock");
        if !raw.connected {
            return GamepadFrame::disconnected();
        }
        let mut buttons = raw.buttons;
        let axes = axes_from_raw_with(
            raw.left_x,
            raw.left_y,
            raw.right_x,
            raw.right_y,
            raw.left_trigger,
            raw.right_trigger,
            self.deadzone,
        );
        triggers_to_buttons(&mut buttons, &axes);
        GamepadFrame {
            buttons,
            axes,
            connected: true,
        }
    }

    fn name(&self) -> &'static str {
        "scripted"
    }
}

/// 测试侧的手柄控制器（与后端共享同一份原始状态）。
struct Pad {
    state: Arc<Mutex<RawPad>>,
}

impl Pad {
    fn new(_deadzone: GamepadDeadzone) -> Self {
        Self {
            state: Arc::new(Mutex::new(RawPad::connected_idle())),
        }
    }

    fn with<R>(&self, visit: impl FnOnce(&mut RawPad) -> R) -> R {
        visit(&mut self.state.lock().expect("pad state lock"))
    }

    fn press(&self, index: usize) {
        self.with(|pad| pad.buttons[index] = true);
    }

    fn release(&self, index: usize) {
        self.with(|pad| pad.buttons[index] = false);
    }

    /// 设置左右摇杆原始值（未过死区）。
    fn stick(&self, x: f32, y: f32) {
        self.with(|pad| {
            pad.left_x = x;
            pad.left_y = y;
        });
    }

    /// 设置扳机原始值（0..1）。
    fn stick_trigger(&self, left: f32, right: f32) {
        self.with(|pad| {
            pad.left_trigger = left;
            pad.right_trigger = right;
        });
    }

    fn disconnect(&self) {
        self.with(|pad| pad.connected = false);
    }

    fn connect(&self) {
        self.with(|pad| pad.connected = true);
    }
}

/// 一帧手柄驱动：轮询设备 → 写入快照 → 调用 Lua `update()` → 清边沿（保留按住）。
fn pad_tick(
    runtime: &mut LuaRuntime,
    poller: &mut GamepadPoller,
    snapshot: &mut InputSnapshot,
) -> Result<(), Box<dyn std::error::Error>> {
    poller.poll_into(snapshot);
    runtime.set_input(snapshot.clone());
    runtime.call_update()?;
    snapshot.finish_frame();
    Ok(())
}

fn pad_ticks(
    runtime: &mut LuaRuntime,
    poller: &mut GamepadPoller,
    snapshot: &mut InputSnapshot,
    count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..count {
        pad_tick(runtime, poller, snapshot)?;
    }
    Ok(())
}

/// 手柄按键点按：按下 1 帧 + 松开 1 帧（与键盘 `press` 的边沿语义等价）。
fn pad_press(
    runtime: &mut LuaRuntime,
    poller: &mut GamepadPoller,
    snapshot: &mut InputSnapshot,
    pad: &Pad,
    index: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    pad.press(index);
    pad_tick(runtime, poller, snapshot)?;
    pad.release(index);
    pad_tick(runtime, poller, snapshot)?;
    Ok(())
}

// ---------- helpers ----------

/// 备份 `<project>/settings.toml`，drop 时恢复（原本不存在则删除）。
struct SettingsGuard {
    path: PathBuf,
    original: Option<String>,
}

impl SettingsGuard {
    fn capture(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let original = match fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path: path.to_path_buf(),
            original,
        })
    }
}

impl Drop for SettingsGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(text) => {
                let _ = fs::write(&self.path, text);
            }
            None => {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // 音频目录固定为 <project>/assets/audio（core/audio.rs）。
    runtime.set_audio_asset_dir(project.join("assets").join("audio"));
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

/// 运行工程脚本并进入山庄（新档），与 jhyt_m20_audio 的 headless 范式一致。
fn enter_playable(
    runtime: &mut LuaRuntime,
    main_script: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_project_script(main_script)?;
    runtime.execute_console("scene.load('main')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(runtime, "main")?;
    lua(runtime, "scene.load('manor')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(runtime, "manor")?;
    assert_mode(runtime, "Playing")?;
    Ok(())
}

/// 键盘点按（用于验证「改绑后旧键失效」与上下文动作仍可用）。
fn press_key(runtime: &mut LuaRuntime, key: InputKey) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
}

fn frames(runtime: &mut LuaRuntime, count: usize) -> Result<(), Box<dyn std::error::Error>> {
    runtime.set_input(InputSnapshot::default());
    for _ in 0..count {
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

/// 玩家刚体速度（`physics.get_velocity` 返回数组式表：1=x、2=y）。
fn player_velocity(runtime: &mut LuaRuntime) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let x = query_num(runtime, &format!("return {VELOCITY_X}"))?;
    let y = query_num(runtime, &format!("return {VELOCITY_Y}"))?;
    Ok((x, y))
}

fn assert_str(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_str_why(runtime, expr, expected, "")
}

fn assert_str_why(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: &str,
    why: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(runtime, &format!("return tostring({expr})"))?;
    if got != expected {
        return Err(format!("{expr} = {got:?}, want {expected:?} {why}").into());
    }
    Ok(())
}

fn assert_ne_str(
    runtime: &mut LuaRuntime,
    expr: &str,
    unexpected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(runtime, &format!("return tostring({expr})"))?;
    if got == unexpected {
        return Err(format!("{expr} = {got:?}, 不应等于 {unexpected:?}").into());
    }
    Ok(())
}

fn assert_num(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_num_why(runtime, expr, expected, "")
}

fn assert_num_why(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: f64,
    why: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_num(runtime, &format!("return {expr}"))?;
    if (got - expected).abs() > 1e-6 {
        return Err(format!("{expr} = {got}, want {expected} {why}").into());
    }
    Ok(())
}

fn assert_range(
    runtime: &mut LuaRuntime,
    expr: &str,
    low: f64,
    high: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_num(runtime, &format!("return {expr}"))?;
    if got < low || got > high {
        return Err(format!("{expr} = {got}, want in [{low}, {high}]").into());
    }
    Ok(())
}

fn assert_mode(runtime: &mut LuaRuntime, expected: &str) -> Result<(), Box<dyn std::error::Error>> {
    assert_str(runtime, "require('core.state').mode", expected)
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
