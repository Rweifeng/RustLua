//! 原 `mod sdf_tests` 里 `gamepad_input` 这一域（WP12 阶段 1 纯搬运；源行段 5805–6173）。
//!
//! - `title_scene_gamepad_navigation_works_through_runtime_scene`
//! - `title_scene_stick_navigation_works_through_runtime_scene`
//! - `held_gamepad_stick_reaches_script_with_fresh_input_snapshot_per_frame`
//! - `virtual_pad_key_injection_survives_fresh_input_snapshot_per_frame`
//! - `zero_step_frame_does_not_drop_gamepad_edges`
//! - `lua_rumble_reaches_device_layer`

use super::*;
use rustlua_scene::scene::Scene;

/// 标题场景（`main.scene`）的手柄导航在**真实 RuntimeScene 路径**上可用。
///
/// 编辑器 Play 预览走的就是这条路径（每帧全新键鼠快照 + `run_script_steps` 合并手柄），
/// 而 M20-08 的验收示例是直接驱动 `LuaRuntime`，覆盖不到 `input_apply` 这一步。
#[test]
fn title_scene_gamepad_navigation_works_through_runtime_scene() {
    use crate::host::gamepad::{GamepadFrame, SlotBackend, button};

    let root = rustlua_infra::games_dir()
        .join("examples")
        .join("game_jhyt");
    let scene_path = root.join("scenes/main.scene");
    if !scene_path.is_file() {
        return;
    }
    let scene = Scene::load(&scene_path).expect("load main.scene");
    let mut runtime =
        RuntimeScene::from_scene(Some(&root), scene).expect("runtime from main.scene");

    let idle = GamepadFrame::idle();
    let mut dpad_down = GamepadFrame::idle();
    dpad_down.buttons[button::DPAD_DOWN] = true;
    // ① 空闲帧（建立设备基线）→ ② 起按住 DPad 下。
    runtime.set_gamepad_backend(Box::new(SlotBackend::new(vec![
        [Some(idle), None, None, None],
        [Some(dpad_down), None, None, None],
    ])));

    let probe = |runtime: &mut RuntimeScene| -> String {
        runtime
            .script
            .execute_console("return tostring(require('ui.title_menu').selected_id())")
            .expect("probe title menu")
            .trim()
            .to_owned()
    };

    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("idle step");
    let before = probe(&mut runtime);
    assert_ne!(before, "nil", "标题菜单应在 main.scene 上初始化");
    for _ in 0..3 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("dpad step");
    }
    let after = probe(&mut runtime);
    println!("title selected: before={before} after={after}");
    assert_ne!(after, before, "DPad 下应改变标题菜单选中项");
}

/// 标题场景（`main.scene`）的**摇杆**菜单导航在真实 RuntimeScene 路径上可用。
///
/// 项目 `settings.toml` 把左摇杆绑给了 `move_*`（实机测试用），`ui_*` 未列在 `map` 里
/// → `load_settings` 用默认绑定补齐（含 `padaxis:1±`），因此摇杆仍能选菜单。这条路径
/// 依赖 **`ui_*` 的轴绑定 + 轴边沿**（M22/W3），与 M20-08 的「Lua 直接读 `pad_axis`」不同。
#[test]
fn title_scene_stick_navigation_works_through_runtime_scene() {
    use crate::host::gamepad::{GamepadFrame, SlotBackend};

    let root = rustlua_infra::games_dir()
        .join("examples")
        .join("game_jhyt");
    let scene_path = root.join("scenes/main.scene");
    if !scene_path.is_file() {
        return;
    }
    let scene = Scene::load(&scene_path).expect("load main.scene");
    let mut runtime =
        RuntimeScene::from_scene(Some(&root), scene).expect("runtime from main.scene");

    let mut stick_down = GamepadFrame::idle();
    stick_down.axes[crate::host::gamepad::axis::LEFT_Y] = -0.9;
    runtime.set_gamepad_backend(Box::new(SlotBackend::new(vec![
        [Some(GamepadFrame::idle()), None, None, None],
        [Some(stick_down), None, None, None],
    ])));

    let probe = |runtime: &mut RuntimeScene| -> String {
        runtime
            .script
            .execute_console("return tostring(require('ui.title_menu').selected_id())")
            .expect("probe title menu")
            .trim()
            .to_owned()
    };

    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("idle step");
    let before = probe(&mut runtime);
    for _ in 0..3 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("stick step");
    }
    let after = probe(&mut runtime);
    println!("title selected (stick): before={before} after={after}");
    assert_ne!(after, before, "摇杆下推应改变标题菜单选中项");
}

/// 编辑器 Play 预览每帧重建输入快照（`bridge_preview_input` 只桥接键鼠），
/// 而独立 game 宿主跨帧复用同一快照。设备在被按住期间持续上报同一轴值时，
/// 运行时必须把**当前**手柄状态写进本帧输入，而不是只写「与上一帧不同」的轴——
/// 否则编辑器预览里摇杆推住不动就等于没有输入（回归：编辑器内摇杆无反应）。
#[test]
fn held_gamepad_stick_reaches_script_with_fresh_input_snapshot_per_frame() {
    /// 恒定设备帧：始终「按住」左摇杆上推（值不变 → 无变化边沿）。
    struct HeldStickBackend {
        frame: crate::host::gamepad::GamepadFrame,
    }

    impl crate::host::gamepad::GamepadBackend for HeldStickBackend {
        fn poll(&mut self) -> crate::host::gamepad::GamepadFrame {
            self.frame
        }

        fn name(&self) -> &'static str {
            "held-stick"
        }
    }

    let root = std::env::temp_dir().join(format!(
        "rustlua-preview-gamepad-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let scripts = root.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(
        scripts.join("main.lua"),
        "args = {}\n\
             function update()\n\
               local s = input.pad_axis(1)\n\
               if type(s) == 'number' and s == s and s ~= 0 then args[#args + 1] = s end\n\
               return ''\n\
             end\n",
    )
    .unwrap();

    let mut runtime = RuntimeScene::from_scene(Some(&root), Scene::new("preview-gamepad"))
        .expect("runtime scene");
    let mut frame = crate::host::gamepad::GamepadFrame::idle();
    frame.axes[crate::host::gamepad::axis::LEFT_Y] = 0.8;
    runtime.set_gamepad_backend(Box::new(HeldStickBackend { frame }));

    // 与编辑器预览逐帧一致：每帧都是全新快照（不跨帧保留手柄状态）。
    for _ in 0..3 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("script step");
    }
    let sampled = runtime
        .script
        .execute_console("return tostring(#args) .. '|' .. tostring(args[1])")
        .expect("probe lua");
    let (count, value) = sampled.trim().split_once('|').expect("count|value");
    assert_eq!(
        count, "3",
        "每帧都应读到被按住摇杆的轴值（编辑器预览逐帧重建快照）"
    );
    let value: f32 = value.parse().expect("axis value");
    assert!(
        (value - 0.8).abs() < 1e-3,
        "轴值应保持设备上报值 0.8：{value}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// 触屏虚拟键（`input.virtual_key`，`ui/virtual_pad.lua` 的摇杆与虚拟按钮）
/// 必须**跨帧保持**，因为宿主每帧都用设备层重新采样的快照整份替换脚本输入。
///
/// 回归（实机症状）：摇杆头能动、`stick=true`，但 `input.action_axis('move_left',
/// 'move_right')` 恒为 `0.00`，角色不动——虚拟键在帧内注入后，被下一帧的
/// `set_input` 整份替换清空，`update_movement` 永远读到 0。
#[test]
fn virtual_pad_key_injection_survives_fresh_input_snapshot_per_frame() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-virtual-pad-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let scripts = root.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    // 与触屏摇杆同构：按住期间每帧注入 `true`，抬起时注入 `false`。
    std::fs::write(
        scripts.join("main.lua"),
        "sticks = {}\n\
             held = false\n\
             function hold() held = true end\n\
             function release() held = false end\n\
             function update()\n\
               local before = input.action_axis('move_down', 'move_up')\n\
               input.virtual_key('keyw', held)\n\
               local after = input.action_axis('move_down', 'move_up')\n\
               if held then sticks[#sticks + 1] = tostring(before) .. '/' .. tostring(after) end\n\
               return ''\n\
             end\n",
    )
    .unwrap();

    let mut runtime =
        RuntimeScene::from_scene(Some(&root), Scene::new("virtual-pad")).expect("runtime scene");
    // 与 `game_app.rs` / 编辑器预览逐帧一致：宿主持有一份**自己的**快照，每帧 clone
    // 后交给运行时；脚本对虚拟键的注入只落在运行时那一份克隆里，下一帧又被宿主的
    // 快照整份覆盖——这正是实机上「摇杆能动、角色不动」的帧边界。
    let mut host_input = InputSnapshot::default();
    let step = |runtime: &mut RuntimeScene, host_input: &mut InputSnapshot| {
        let frame = host_input.clone();
        runtime
            .run_script_steps(1, 1.0 / 60.0, &frame)
            .expect("script step");
        // 宿主侧只推进边沿/增量（`game_app.rs` 帧末），按住态由设备层重新采样。
        host_input.finish_frame();
    };
    step(&mut runtime, &mut host_input);
    runtime
        .script
        .execute_console("hold() return ''")
        .expect("hold stick");
    for _ in 0..3 {
        step(&mut runtime, &mut host_input);
    }
    let held = runtime
        .script
        .execute_console(
            "return tostring(#sticks) .. '|' .. tostring(sticks[1]) .. '|' .. tostring(sticks[3])",
        )
        .expect("probe held");
    runtime
        .script
        .execute_console("release() return ''")
        .expect("release stick");
    step(&mut runtime, &mut host_input);
    let released = runtime
        .script
        .execute_console("return tostring(#sticks)")
        .expect("probe released");
    let (count, first, last) = {
        let probe = held.trim().to_owned();
        let mut parts = probe.split('|');
        (
            parts.next().unwrap_or_default().to_owned(),
            parts.next().unwrap_or_default().to_owned(),
            parts.next().unwrap_or_default().to_owned(),
        )
    };
    // `before/after`：注入前的轴值 / 注入后的轴值。首帧注入前为 0（此前没有任何注入），
    // 但**第二帧起**注入前必须已是 +1——虚拟键要跨帧保持住，否则 `update_movement`
    // 在下一帧只会读到 0（实机「摇杆能动、角色不动」）。
    assert_eq!(count, "3", "按住期间每帧都应读到虚拟键轴值");
    assert_eq!(first, "0.0/1.0", "首帧：注入前无虚拟键、注入后立即可读");
    assert_eq!(
        last, "1.0/1.0",
        "第二帧起：注入前虚拟键必须仍被按住（跨帧保持）"
    );
    assert_eq!(released, "3", "释放后不再计入按住帧");
    let _ = std::fs::remove_dir_all(&root);
}

/// M22/W4：**0 步帧不得吞掉手柄边沿**。
///
/// 宿主侧键鼠边沿只在 `steps > 0` 时被 `finish_frame` 清掉（`game_app.rs`），所以 0 步帧
/// （帧率高于逻辑步，关 vsync 时常见）里发生的按键会保留到下一有效帧。手柄此前在 0 步帧
/// 也会 poll：边沿被写进那个随后丢弃的快照克隆，而 `poller.last` 已推进 → 下一帧不再产生
/// 同一变化，边沿**永久丢失**。修复后 0 步帧不采样，两条路径语义一致。
#[test]
fn zero_step_frame_does_not_drop_gamepad_edges() {
    use crate::host::gamepad::{FakeBackend, GamepadFrame, button};

    let root = std::env::temp_dir().join(format!(
        "rustlua-zero-step-gamepad-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let scripts = root.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(
        scripts.join("main.lua"),
        "presses = 0\n\
             function update()\n\
               if input.action_pressed('confirm') then presses = presses + 1 end\n\
               return ''\n\
             end\n",
    )
    .unwrap();

    let mut runtime = RuntimeScene::from_scene(Some(&root), Scene::new("zero-step-gamepad"))
        .expect("runtime scene");
    // 设备从第一帧起就按住 A：唯一一次按下边沿必须被某个**有效**帧看到。
    let mut held = GamepadFrame::idle();
    held.buttons[button::A] = true;
    runtime.set_gamepad_backend(Box::new(FakeBackend::new(vec![held])));

    // 0 步帧（此前会在这里 poll 并丢掉边沿）。
    runtime
        .run_script_steps(0, 1.0 / 60.0, &InputSnapshot::default())
        .expect("zero-step frame");
    // 第一个有效帧：应看到按下边沿。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("first script step");
    // 第二个有效帧：按住不重复边沿。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("second script step");

    let count = runtime
        .script
        .execute_console("return tostring(presses)")
        .expect("probe lua");
    assert_eq!(
        count.trim(),
        "1",
        "0 步帧不得吞掉手柄按下边沿；按住也不得重复边沿"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// M22/W2：Lua `input.rumble` 的请求在帧末到达设备层（含槽位与强度）。
#[test]
fn lua_rumble_reaches_device_layer() {
    use crate::host::gamepad::{GamepadFrame, SlotBackend};

    let root = std::env::temp_dir().join(format!(
        "rustlua-rumble-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let scripts = root.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(
        scripts.join("main.lua"),
        "function update()\n\
               input.rumble(0.4, 0.6, 120)\n\
               return ''\n\
             end\n",
    )
    .unwrap();

    let mut runtime =
        RuntimeScene::from_scene(Some(&root), Scene::new("rumble")).expect("runtime scene");
    let backend = SlotBackend::new(vec![[Some(GamepadFrame::idle()), None, None, None]]);
    let log = backend.rumble_log();
    runtime.set_gamepad_backend(Box::new(backend));

    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("script step");
    assert_eq!(
        log.calls().last().copied(),
        Some((0, 0.4, 0.6)),
        "Lua 震动请求应到达设备层"
    );
    // 请求取走后不重复触发（下一帧没有新请求）。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("script step");
    assert_eq!(log.calls().len(), 2, "每帧一次请求，不做额外重发");
    let _ = std::fs::remove_dir_all(&root);
}
