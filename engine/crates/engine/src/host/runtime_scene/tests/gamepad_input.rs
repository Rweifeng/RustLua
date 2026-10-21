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

    // CI windows-latest 上这条用例在 `run_script_steps` 后触发 0xC0000005（本地同二进制
    // 单线程通过；相邻的 gamepad 用例也通过）。先用显式开关隔离，避免它把整个 engine
    // 测试二进制带走；本地/专门排查时取消该环境变量即可复跑。
    if std::env::var_os("RUSTLUA_SKIP_CI_FLAKY").is_some() {
        eprintln!("skip title_scene_stick_navigation_works_through_runtime_scene (RUSTLUA_SKIP_CI_FLAKY)");
        return;
    }

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
/// `input.press_position`（第 4 项）的**行为级**用例：命中测试必须用**按下点**，而不是实时光标。
///
/// 为什么这条用例存在：`ui/virtual_pad.lua` 用 `press_position` 做控件命中测试——按住摇杆时另一只手
/// 点按钮，光标仍跟着摇杆那只手，而按钮必须按**自己的按下点**命中。补丁层（web/微信）一直有这个名字
/// （它把逐指按下点塞进 Lua 表），而引擎侧此前**连字段都没有**，于是 native 上
/// `host_function(input, "press_position")` 恒假、静默退化成「按实时光标命中」——症状是
/// 「按住摇杆＋另一手点按钮」命中错位，而**任何读数都不报错**。这条用例把那个退化钉住。
///
/// 判别力来自**同一帧同一份输入下的两个答案**：`hit`（用按下点）必须是 `button`，
/// `fallback_hit`（用实时光标，即修复前的行为）必须是 `pad`。两者相等就说明这条用例在空转，
/// 所以第二个断言不是陪衬——它是「这条断言真的在判别」的自证。
#[test]
fn press_position_drives_hit_test_not_live_cursor() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-press-position-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let scripts = root.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    // 与 `ui/virtual_pad.lua` **同一套读法**（`cp.x or cp[1]`、`pcall` + `type` 探针），
    // 所以它测的是那个真实契约，而不是一个仿写的近似物。
    std::fs::write(
        scripts.join("main.lua"),
        "hit, fallback_hit = 'none', 'none'\n\
         function in_rect(x, y, x0, y0, x1, y1)\n\
           return x >= x0 and x <= x1 and y >= y0 and y <= y1\n\
         end\n\
         function classify(x, y)\n\
           if in_rect(x, y, 600, 300, 700, 400) then return 'button' end\n\
           if in_rect(x, y, 100, 300, 200, 400) then return 'pad' end\n\
           return 'none'\n\
         end\n\
         function update()\n\
           local cp = input.cursor_position()\n\
           local mx, my = cp.x or cp[1] or 0, cp.y or cp[2] or 0\n\
           local press_x, press_y = mx, my\n\
           if type(input.press_position) == 'function' then\n\
             local ok, pp = pcall(input.press_position)\n\
             if ok and pp then\n\
               press_x = pp.x or pp[1] or mx\n\
               press_y = pp.y or pp[2] or my\n\
             end\n\
           end\n\
           hit = classify(press_x, press_y)\n\
           fallback_hit = classify(mx, my)\n\
           return ''\n\
         end\n",
    )
    .unwrap();

    let mut runtime =
        RuntimeScene::from_scene(Some(&root), Scene::new("press-position")).expect("runtime scene");
    // 摇杆在左半屏、按钮在右半屏；**光标跟着「按住摇杆的那只手」**，而按下点在按钮上——
    // 这正是真机上另一只手点按钮的几何关系。
    let mut host_input = InputSnapshot::default();
    host_input.cursor_moved(150.0, 350.0);
    host_input.press_cursor_moved(650.0, 350.0);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &host_input)
        .expect("script step");

    let hit = runtime
        .script
        .execute_console("return hit")
        .expect("read hit");
    let fallback = runtime
        .script
        .execute_console("return fallback_hit")
        .expect("read fallback_hit");
    assert_eq!(
        hit, "button",
        "命中测试必须用 `input.press_position`（按下点）：光标在摇杆上、按下点在按钮上 ⇒ 应命中按钮；实际 {hit}。若这里是 pad，说明 `press_position` 退化成了实时光标（第 4 项的那个静默退步）。"
    );
    assert_eq!(
        fallback, "pad",
        "同一帧里用实时光标的答案必须是 pad，否则本用例没有判别力（两个答案相同 = 空转）；实际 {fallback}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
/// `input.press_position(i)` / `input.pointer_count()` 的**取值级**用例：脚本真的按索引读到
/// "每根手指各自的按下点"。
///
/// 为什么必须有它：多指这条链有四处（线格式 → 生产者 → 引擎快照 → Lua 注册），前三处各有用例，
/// 而"Lua 侧真的取到那一根手指"此前只有"函数在 API 面里"这一级证据——**"在面里"与"取到对的值"
/// 是两回事**：注册了却永远回主按下点，照样在面里、照样过基线门。
#[test]
fn lua_reads_each_finger_press_point() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-finger-points-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let scripts = root.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    // 用 `string.format('%.1f')` 而不是 `tostring`：后者在不同 Lua 版本上对整数值浮点的
    // 输出不一致（"100.0" vs "100"），会让这条用例变成版本敏感的门。
    std::fs::write(
        scripts.join("main.lua"),
        "count = -1\n\
         s0, s1, s2, snone = 'x', 'x', 'x', 'x'\n\
         local function fmt(pp)\n\
           if not pp then return 'nil' end\n\
           return string.format('%.1f,%.1f', pp.x or pp[1], pp.y or pp[2])\n\
         end\n\
         function update()\n\
           count = input.pointer_count()\n\
           s0 = fmt(input.press_position(0))\n\
           s1 = fmt(input.press_position(1))\n\
           s2 = fmt(input.press_position(2))\n\
           snone = fmt(input.press_position())\n\
           return ''\n\
         end\n",
    )
    .unwrap();

    let mut runtime =
        RuntimeScene::from_scene(Some(&root), Scene::new("finger-points")).expect("runtime scene");
    let mut host_input = InputSnapshot::default();
    // 两根手指各按一处（顺序即按下先后）。
    host_input.set_press_cursors(&[(100.0, 200.0), (600.0, 400.0)]);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &host_input)
        .expect("script step");

    let mut read = |name: &str| {
        runtime
            .script
            .execute_console(&format!("return tostring({name})"))
            .expect("read")
    };
    assert_eq!(read("count"), "2", "脚本必须看到两根手指");
    assert_eq!(read("s0"), "100.0,200.0", "第一根的按下点");
    assert_eq!(read("s1"), "600.0,400.0", "第二根的按下点");
    assert_eq!(
        read("s2"),
        "nil",
        "越界必须回 nil，不能拿第一根（或主按下点）冒充——命中测试里错的坐标比没有坐标更糟"
    );
    assert_eq!(
        read("snone"),
        "600.0,400.0",
        "不传参数 = 主按下点 = 最后一根（旧行为，`ui/virtual_pad.lua` 靠它）"
    );

    let _ = std::fs::remove_dir_all(&root);
}
