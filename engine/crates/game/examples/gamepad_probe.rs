//! 手柄自检（M22）：用**真实平台后端**（Windows = XInput）采样若干秒，打印每个槽位的
//! 连接状态、按钮/轴活动与一次震动脉冲，用来区分「设备层看不到手柄」与「游戏侧逻辑没接线」。
//!
//! 用法（工作目录 `engine/`）：
//!
//! ```powershell
//! cargo run -p game --example gamepad_probe            # 默认采样 8 秒
//! cargo run -p game --example gamepad_probe -- 15      # 采样 15 秒
//! ```
//!
//! 判读：
//!
//! * 打印 `已连接槽位: [0]` 并能看到 `按钮`/`轴` 变化 → 设备层正常，问题在游戏/编辑器接线；
//! * 一直打印 `已连接槽位: []` → **XInput 看不到这个设备**（本示例与游戏用的是同一条
//!   `XInputGetState` 路径）。常见原因：DualSense/DualShock 走蓝牙或 DS4 模式、Switch Pro
//!   未切到 XInput、被 Steam Input 独占、或使用了非 XInput 的第三方驱动。此时游戏里任何
//!   手柄功能都不会响应，需要切到 XInput 模式（或加一层 DS4Windows/Steam 映射）。
//! * 连上了但**某个按钮/轴没反应** → 按契约表核对索引（`docs/33` §2）。
//!
//! 结束时还会触发一次 300ms 震动（左强右弱），能感觉到即说明 `XInputSetState` 通路可用。

use engine::host::gamepad::{
    GAMEPAD_AXIS_COUNT, GAMEPAD_BUTTON_COUNT, GamepadFrame, GamepadPoller, axis, button,
};
use std::time::{Duration, Instant};

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(8);

    let mut poller = GamepadPoller::platform_default();
    println!(
        "手柄自检：后端={} 采样={seconds}s（请按动按钮/推动摇杆；结束时会有一次震动脉冲）",
        poller.backend_name()
    );

    let started = Instant::now();
    let mut last: Option<GamepadFrame> = None;
    let mut changes = 0_u32;
    while started.elapsed() < Duration::from_secs(seconds) {
        let frame = poller.poll_frame();
        if frame.connected != last.is_some_and(|previous| previous.connected) {
            println!(
                "[{:>5.2}s] 连接状态变化：{}",
                started.elapsed().as_secs_f32(),
                frame.connected
            );
        }
        if let Some(previous) = last
            && previous.connected == frame.connected
        {
            for index in 0..GAMEPAD_BUTTON_COUNT {
                if previous.buttons[index] != frame.buttons[index] {
                    changes += 1;
                    println!(
                        "[{:>5.2}s] 按钮 {} ({}) {}",
                        started.elapsed().as_secs_f32(),
                        index,
                        button_name(index),
                        if frame.buttons[index] {
                            "按下"
                        } else {
                            "松开"
                        }
                    );
                }
            }
            for index in 0..GAMEPAD_AXIS_COUNT {
                let before = previous.axes[index];
                let now = frame.axes[index];
                if (now - before).abs() >= 0.2 {
                    changes += 1;
                    println!(
                        "[{:>5.2}s] 轴 {} ({}) {:.3} -> {:.3}",
                        started.elapsed().as_secs_f32(),
                        index,
                        axis_name(index),
                        before,
                        now
                    );
                }
            }
        }
        last = Some(frame);
        std::thread::sleep(Duration::from_millis(16));
    }

    let slots = poller.connected_slots();
    println!(
        "已连接槽位: {slots:?}（活动槽位 {:?}）",
        poller.active_slot()
    );
    for slot in &slots {
        let frame = poller.slot_frame(*slot);
        let held = (0..GAMEPAD_BUTTON_COUNT)
            .filter(|index| frame.buttons[*index])
            .map(|index| format!("{index}"))
            .collect::<Vec<_>>();
        println!(
            "  slot {slot}: 出现过输入={} 按住按钮 {held:?} 轴 {:?}",
            poller.slot_has_input(*slot),
            frame.axes
        );
    }
    if slots.is_empty() {
        println!(
            "=> **未检测到任何手柄**：设备没有出现在 XInput 的 0..3 槽位里（见文件头部判读说明）"
        );
    } else if changes == 0 {
        println!("=> 设备已连接，但本次采样没有捕捉到输入变化（可能没按到按键）");
    } else {
        let active = poller.active_slot();
        let active_seen = active.is_some_and(|slot| poller.slot_has_input(slot));
        println!(
            "=> 设备层正常：捕捉到 {changes} 次输入变化（活动槽位 {active:?}，该槽位出现过输入={active_seen}）"
        );
        if !active_seen && slots.len() > 1 {
            println!(
                "   注意：有多个槽位已连接但活动槽位没有输入 —— 若有占位/虚拟设备占用低编号槽位，\n\
                 引擎会在你按下手柄的那一帧自动切到真正有输入的槽位（M22/W2）。"
            );
        }
    }

    // 震动脉冲（左强右弱 300ms）——能感觉到即说明震动通路可用。
    poller.rumble(0.6, 0.3, 300);
    println!("已发送震动脉冲（左 0.6 / 右 0.3 / 300ms）；到期由引擎自动归零。");
    // 到期归零由 poll_into 驱动；这里手动采样几帧让状态机走完。
    let mut input = rustlua_input::input::InputSnapshot::default();
    let stopped = Instant::now();
    while stopped.elapsed() < Duration::from_millis(400) {
        poller.poll_into(&mut input);
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// 契约按钮索引 → 可读名（与编辑器面板同源口径）。
fn button_name(index: usize) -> &'static str {
    match index {
        button::A => "A",
        button::B => "B",
        button::X => "X",
        button::Y => "Y",
        button::LEFT_SHOULDER => "LB",
        button::RIGHT_SHOULDER => "RB",
        button::LEFT_TRIGGER => "LT",
        button::RIGHT_TRIGGER => "RT",
        button::BACK => "Back",
        button::START => "Start",
        button::LEFT_STICK => "L3",
        button::RIGHT_STICK => "R3",
        button::DPAD_UP => "DPad↑",
        button::DPAD_DOWN => "DPad↓",
        button::DPAD_LEFT => "DPad←",
        button::DPAD_RIGHT => "DPad→",
        _ => "?",
    }
}

/// 契约轴索引 → 可读名。
fn axis_name(index: usize) -> &'static str {
    match index {
        axis::LEFT_X => "左摇杆 X",
        axis::LEFT_Y => "左摇杆 Y",
        axis::RIGHT_X => "右摇杆 X",
        axis::RIGHT_Y => "右摇杆 Y",
        axis::LEFT_TRIGGER => "LT",
        axis::RIGHT_TRIGGER => "RT",
        _ => "?",
    }
}
