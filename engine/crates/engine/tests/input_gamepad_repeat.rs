//! `input.action_repeat` 的**时间制**语义，输入帧由真实设备层生产。
//!
//! 这一条必须住在 engine 的集成测试里：它要的是 `engine::host::gamepad` 的
//! `GamepadPoller`/`FakeBackend`——轴按下边沿是**设备层**产生的，而
//! `rustlua-scripting` 依赖不到 engine（反向依赖会成环），所以脚本侧的 input 测试
//! 只能自己摆 `InputSnapshot`，唯独"边沿来自真实轮询"这件事只能在 engine 这一侧验。

use engine::host::gamepad::{FakeBackend, GamepadFrame, GamepadPoller, axis};
use rustlua_gameplay::settings::NavSettings;
use rustlua_input::input::InputSnapshot;
use rustlua_scripting::script::LuaRuntime;

/// 推住左摇杆上推 `steps` 帧，返回 `action_repeat("ui_up")` 为真的帧号。
fn run_repeat(steps: usize, delay_ms: u32, repeat_ms: u32) -> Vec<usize> {
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_nav_settings(NavSettings::new(0.5, delay_ms, repeat_ms));
    runtime.set_delta(1.0 / 60.0);

    // 第 1 帧空闲（建立基线），之后持续推住左摇杆上推。
    let mut idle = GamepadFrame::idle();
    idle.axes[axis::LEFT_Y] = 0.0;
    let mut held = GamepadFrame::idle();
    held.axes[axis::LEFT_Y] = 0.9;
    let mut poller = GamepadPoller::new(Box::new(FakeBackend::new(vec![idle, held])));

    let mut fired = Vec::new();
    for step in 1..=steps {
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        runtime.set_input(input);
        if runtime.action_repeat("ui_up") {
            fired.push(step);
        }
    }
    fired
}

/// M22/W3：`input.action_repeat` 是**时间制**——同一段按住时长内的步数与帧率
/// 无关（旧的帧计数规则在 60 步里只会走 6 步，这里应约 19 步）。
#[test]
fn action_repeat_is_time_based_for_analog_holds() {
    let fired = run_repeat(61, 100, 50);
    assert_eq!(
        fired.first(),
        Some(&2),
        "轴按下边沿那一帧应立即走一步：{fired:?}"
    );
    assert!(
        !fired.iter().any(|step| (3..=7).contains(step)),
        "100ms 延迟内不得连发：{fired:?}"
    );
    // 1000ms：首次 + (1000-100)/50 ≈ 19 步；旧的帧计数规则只有 6 步。
    assert!(
        (15..=22).contains(&fired.len()),
        "时间制连发步数异常：{} 步 {fired:?}",
        fired.len()
    );

    // 间隔改为 500ms 后步数显著下降（证明确实读的是毫秒设置）。
    let slow = run_repeat(61, 100, 500);
    assert!(
        (1..=4).contains(&slow.len()),
        "500ms 间隔在 1s 内只应有约 3 步，实际 {} 步 {slow:?}",
        slow.len()
    );
}
