//! 手柄设备轮询适配层（M20-06）。
//!
//! 引擎的**抽象**手柄输入早已就位（`core::input::InputSnapshot` 的
//! `gamepad_button_event` / `set_gamepad_axis` 与 `ActionMap` 的
//! `Binding::GamepadButton` / `Binding::GamepadAxis`），但此前没有任何代码
//! 喂入真实设备事件。本模块补齐这一层：
//!
//! * [`GamepadBackend`]：平台无关的轮询接口，`poll()` 返回一帧 [`GamepadFrame`]；
//! * [`XInputBackend`]：Windows 原生 XInput（`windows-sys`，无需新增 crate）；
//! * [`FakeBackend`]：脚本化帧，供 headless 测试与验收驱动；
//! * [`GamepadPoller`]：把设备帧写进 `InputSnapshot`——按住状态与轴值**每帧写全量**
//!   （宿主可能每帧重建快照），按下/松开**边沿只在变化帧**喂事件，断连时清空，
//!   避免「粘键」也避免「按住不动等于没输入」。
//!
//! 契约（写入设置文件与验收测试，改动需同步文档）：
//!
//! | 按钮索引 | 语义 | | 轴索引 | 语义 |
//! |---|---|---|---|---|
//! | 0 | A/Cross | | 0 | 左摇杆 X（右为正） |
//! | 1 | B/Circle | | 1 | 左摇杆 Y（**上为正**，与 WASD `vertical` 一致） |
//! | 2 | X/Square | | 2 | 右摇杆 X |
//! | 3 | Y/Triangle | | 3 | 右摇杆 Y（上为正） |
//! | 4 | LB | | 4 | 左扳机 LT（0..1） |
//! | 5 | RB | | 5 | 右扳机 RT（0..1） |
//! | 6 | LT（数字） | | | |
//! | 7 | RT（数字） | | | |
//! | 8 | Back/Share | | | |
//! | 9 | Start/Options | | | |
//! | 10 | L3 | | | |
//! | 11 | R3 | | | |
//! | 12..15 | DPad 上/下/左/右 | | | |
//!
//! 死区与阈值是**两个概念**：死区（[`STICK_DEADZONE`]/[`TRIGGER_DEADZONE`]）用于
//! 消除漂移，动作判定阈值沿用 `core::input` 的 0.5。

use crate::core::input::InputSnapshot;

/// 按钮数量（见模块文档的索引契约）。
pub const GAMEPAD_BUTTON_COUNT: usize = 16;
/// 轴数量（见模块文档的索引契约）。
pub const GAMEPAD_AXIS_COUNT: usize = 6;
/// 左/右摇杆默认死区。
pub const STICK_DEADZONE: f32 = 0.15;
/// 扳机默认死区。
pub const TRIGGER_DEADZONE: f32 = 0.05;
/// 死区可配置上限：必须严格小于 1（等于 1 会把满量程也归零）。
pub const MAX_DEADZONE: f32 = 0.9;

/// 摇杆/扳机死区（M20-08：从常量升级为可在 `settings.toml` 的 `[input]` 段
/// 读写的玩家偏好；默认值仍是 [`STICK_DEADZONE`] / [`TRIGGER_DEADZONE`]）。
///
/// 语义与 [`apply_stick_deadzone`] / [`apply_trigger_deadzone`] 一致：死区内
/// 归零（消除回中漂移），死区外按比例重映射到 0..1。非有限值与超界值在构造时
/// 就被钳制，因此从设置文件读进来的脏数据不会产生非法曲线。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamepadDeadzone {
    /// 左/右摇杆死区（0..=MAX_DEADZONE）。
    pub stick: f32,
    /// 扳机死区（0..=MAX_DEADZONE）。
    pub trigger: f32,
}

impl GamepadDeadzone {
    /// 引擎默认死区（0.15 / 0.05）。
    pub const DEFAULT: Self = Self {
        stick: STICK_DEADZONE,
        trigger: TRIGGER_DEADZONE,
    };

    /// 构造并钳制到 `0..=MAX_DEADZONE`（非有限值回落默认值）。
    #[must_use]
    pub fn new(stick: f32, trigger: f32) -> Self {
        Self {
            stick: sanitize_deadzone(stick, STICK_DEADZONE),
            trigger: sanitize_deadzone(trigger, TRIGGER_DEADZONE),
        }
    }

    /// 与默认值比较（编辑器「死区是否被改过」判断用）。
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
    }
}

impl Default for GamepadDeadzone {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn sanitize_deadzone(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, MAX_DEADZONE)
    } else {
        fallback
    }
}

/// 按钮索引（契约常量，避免各处魔数）。
pub mod button {
    pub const A: usize = 0;
    pub const B: usize = 1;
    pub const X: usize = 2;
    pub const Y: usize = 3;
    pub const LEFT_SHOULDER: usize = 4;
    pub const RIGHT_SHOULDER: usize = 5;
    pub const LEFT_TRIGGER: usize = 6;
    pub const RIGHT_TRIGGER: usize = 7;
    pub const BACK: usize = 8;
    pub const START: usize = 9;
    pub const LEFT_STICK: usize = 10;
    pub const RIGHT_STICK: usize = 11;
    pub const DPAD_UP: usize = 12;
    pub const DPAD_DOWN: usize = 13;
    pub const DPAD_LEFT: usize = 14;
    pub const DPAD_RIGHT: usize = 15;
}

/// 轴索引（契约常量）。
pub mod axis {
    pub const LEFT_X: usize = 0;
    pub const LEFT_Y: usize = 1;
    pub const RIGHT_X: usize = 2;
    pub const RIGHT_Y: usize = 3;
    pub const LEFT_TRIGGER: usize = 4;
    pub const RIGHT_TRIGGER: usize = 5;
}

/// 一帧设备状态。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamepadFrame {
    pub buttons: [bool; GAMEPAD_BUTTON_COUNT],
    pub axes: [f32; GAMEPAD_AXIS_COUNT],
    /// 本帧是否有手柄连接（无设备/全部断开时为 false）。
    pub connected: bool,
}

impl GamepadFrame {
    /// 无设备/断开的一帧（全零且未连接）。
    #[must_use]
    pub fn disconnected() -> Self {
        Self {
            buttons: [false; GAMEPAD_BUTTON_COUNT],
            axes: [0.0; GAMEPAD_AXIS_COUNT],
            connected: false,
        }
    }

    /// 已连接但全未按的一帧。
    #[must_use]
    pub fn idle() -> Self {
        Self {
            connected: true,
            ..Self::disconnected()
        }
    }
}

impl Default for GamepadFrame {
    fn default() -> Self {
        Self::disconnected()
    }
}

/// 平台无关的手柄轮询接口。
pub trait GamepadBackend: Send {
    /// 采样当前设备状态（无设备时返回 [`GamepadFrame::disconnected`]）。
    fn poll(&mut self) -> GamepadFrame;

    /// 后端名称（日志与诊断用）。
    fn name(&self) -> &'static str;
}

/// 摇杆死区：死区内归零，死区外按比例映射到 0..1（保持符号）。
#[must_use]
pub fn apply_stick_deadzone(value: f32, deadzone: f32) -> f32 {
    apply_deadzone(value, deadzone)
}

/// 扳机死区（输入 0..1）。
#[must_use]
pub fn apply_trigger_deadzone(value: f32, deadzone: f32) -> f32 {
    apply_deadzone(value, deadzone)
}

fn apply_deadzone(value: f32, deadzone: f32) -> f32 {
    let clamped = value.clamp(-1.0, 1.0);
    if clamped.abs() <= deadzone {
        return 0.0;
    }
    let span = (1.0 - deadzone).max(f32::EPSILON);
    let magnitude = (clamped.abs() - deadzone) / span;
    magnitude.clamp(0.0, 1.0) * clamped.signum()
}

/// XInput 按钮位标志 → 契约按钮数组（纯函数，无需设备即可测试）。
#[must_use]
pub fn buttons_from_flags(flags: u16) -> [bool; GAMEPAD_BUTTON_COUNT] {
    // XInput 位定义（windows-sys 常量）：DPAD_UP=0x0001 ... A=0x1000 B=0x2000 X=0x4000 Y=0x8000。
    const FLAG_DPAD_UP: u16 = 0x0001;
    const FLAG_DPAD_DOWN: u16 = 0x0002;
    const FLAG_DPAD_LEFT: u16 = 0x0004;
    const FLAG_DPAD_RIGHT: u16 = 0x0008;
    const FLAG_START: u16 = 0x0010;
    const FLAG_BACK: u16 = 0x0020;
    const FLAG_LEFT_THUMB: u16 = 0x0040;
    const FLAG_RIGHT_THUMB: u16 = 0x0080;
    const FLAG_LEFT_SHOULDER: u16 = 0x0100;
    const FLAG_RIGHT_SHOULDER: u16 = 0x0200;
    const FLAG_A: u16 = 0x1000;
    const FLAG_B: u16 = 0x2000;
    const FLAG_X: u16 = 0x4000;
    const FLAG_Y: u16 = 0x8000;

    let mut buttons = [false; GAMEPAD_BUTTON_COUNT];
    let mapping = [
        (FLAG_A, button::A),
        (FLAG_B, button::B),
        (FLAG_X, button::X),
        (FLAG_Y, button::Y),
        (FLAG_LEFT_SHOULDER, button::LEFT_SHOULDER),
        (FLAG_RIGHT_SHOULDER, button::RIGHT_SHOULDER),
        (FLAG_BACK, button::BACK),
        (FLAG_START, button::START),
        (FLAG_LEFT_THUMB, button::LEFT_STICK),
        (FLAG_RIGHT_THUMB, button::RIGHT_STICK),
        (FLAG_DPAD_UP, button::DPAD_UP),
        (FLAG_DPAD_DOWN, button::DPAD_DOWN),
        (FLAG_DPAD_LEFT, button::DPAD_LEFT),
        (FLAG_DPAD_RIGHT, button::DPAD_RIGHT),
    ];
    for (flag, index) in mapping {
        buttons[index] = flags & flag != 0;
    }
    // 扳机数字按钮由模拟量派生（见 `axes_from_raw` + `triggers_to_buttons`）。
    buttons
}

/// 原始摇杆/扳机值 → 契约轴数组（含**默认**死区）。
///
/// Y 轴约定：**上为正**（XInput 原生即上为正，与 WASD 的 `vertical` 语义一致）。
#[must_use]
pub fn axes_from_raw(
    left_x: f32,
    left_y: f32,
    right_x: f32,
    right_y: f32,
    left_trigger: f32,
    right_trigger: f32,
) -> [f32; GAMEPAD_AXIS_COUNT] {
    axes_from_raw_with(
        left_x,
        left_y,
        right_x,
        right_y,
        left_trigger,
        right_trigger,
        GamepadDeadzone::DEFAULT,
    )
}

/// 同 [`axes_from_raw`]，但使用调用方给定的死区（M20-08：来自 `settings.toml`）。
#[must_use]
pub fn axes_from_raw_with(
    left_x: f32,
    left_y: f32,
    right_x: f32,
    right_y: f32,
    left_trigger: f32,
    right_trigger: f32,
    deadzone: GamepadDeadzone,
) -> [f32; GAMEPAD_AXIS_COUNT] {
    let mut axes = [0.0; GAMEPAD_AXIS_COUNT];
    axes[axis::LEFT_X] = apply_stick_deadzone(left_x, deadzone.stick);
    axes[axis::LEFT_Y] = apply_stick_deadzone(left_y, deadzone.stick);
    axes[axis::RIGHT_X] = apply_stick_deadzone(right_x, deadzone.stick);
    axes[axis::RIGHT_Y] = apply_stick_deadzone(right_y, deadzone.stick);
    axes[axis::LEFT_TRIGGER] = apply_trigger_deadzone(left_trigger, deadzone.trigger);
    axes[axis::RIGHT_TRIGGER] = apply_trigger_deadzone(right_trigger, deadzone.trigger);
    axes
}

/// 扳机模拟量超过判定阈值时同步置位数字按钮（6=LT、7=RT）。
pub fn triggers_to_buttons(
    buttons: &mut [bool; GAMEPAD_BUTTON_COUNT],
    axes: &[f32; GAMEPAD_AXIS_COUNT],
) {
    buttons[button::LEFT_TRIGGER] = axes[axis::LEFT_TRIGGER] >= 0.5;
    buttons[button::RIGHT_TRIGGER] = axes[axis::RIGHT_TRIGGER] >= 0.5;
}

/// Windows XInput 后端（`windows-sys`，无新增依赖）。
///
/// M20-08：死区可配置（`settings.toml` 的 `[input] stick_deadzone/trigger_deadzone`），
/// 但**只在这一层**应用——原始硬件值在这里归一化，`InputSnapshot` 里的轴值已经是
/// 死区后的契约值，避免二次衰减。
#[cfg(windows)]
#[derive(Debug, Default)]
pub struct XInputBackend {
    deadzone: GamepadDeadzone,
}

#[cfg(windows)]
impl XInputBackend {
    /// 默认死区后端。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定死区后端（玩家在编辑器/设置文件里改过的值）。
    #[must_use]
    pub fn with_deadzone(deadzone: GamepadDeadzone) -> Self {
        Self { deadzone }
    }

    /// 当前死区。
    #[must_use]
    pub fn deadzone(&self) -> GamepadDeadzone {
        self.deadzone
    }
}

#[cfg(windows)]
impl GamepadBackend for XInputBackend {
    fn poll(&mut self) -> GamepadFrame {
        poll_xinput(self.deadzone)
    }

    fn name(&self) -> &'static str {
        "xinput"
    }
}

/// 采样 0..3 号 XInput 用户，返回第一个已连接设备的状态。
#[cfg(windows)]
fn poll_xinput(deadzone: GamepadDeadzone) -> GamepadFrame {
    use windows_sys::Win32::UI::Input::XboxController::{
        XINPUT_GAMEPAD, XINPUT_STATE, XInputGetState, XUSER_MAX_COUNT,
    };

    const ERROR_SUCCESS: u32 = 0;
    for index in 0..XUSER_MAX_COUNT {
        let mut state = XINPUT_STATE::default();
        // SAFETY: `state` 是有效的可写结构；索引来自 XInput 规定范围。
        let result = unsafe { XInputGetState(index, &mut state) };
        if result != ERROR_SUCCESS {
            continue;
        }
        let gamepad: XINPUT_GAMEPAD = state.Gamepad;
        let mut buttons = buttons_from_flags(gamepad.wButtons);
        let axes = axes_from_raw_with(
            normalize_thumb(gamepad.sThumbLX),
            normalize_thumb(gamepad.sThumbLY),
            normalize_thumb(gamepad.sThumbRX),
            normalize_thumb(gamepad.sThumbRY),
            normalize_trigger(gamepad.bLeftTrigger),
            normalize_trigger(gamepad.bRightTrigger),
            deadzone,
        );
        triggers_to_buttons(&mut buttons, &axes);
        return GamepadFrame {
            buttons,
            axes,
            connected: true,
        };
    }
    GamepadFrame::disconnected()
}

/// XInput 摇杆原始值（-32768..32767）→ -1..1。
#[cfg(windows)]
fn normalize_thumb(raw: i16) -> f32 {
    const MAX: f32 = 32767.0;
    (f32::from(raw) / MAX).clamp(-1.0, 1.0)
}

/// XInput 扳机原始值（0..255）→ 0..1。
#[cfg(windows)]
fn normalize_trigger(raw: u8) -> f32 {
    (f32::from(raw) / 255.0).clamp(0.0, 1.0)
}

/// 非 Windows 平台：无设备（静默降级）。
#[cfg(not(windows))]
#[derive(Debug, Default)]
pub struct UnsupportedBackend;

#[cfg(not(windows))]
impl GamepadBackend for UnsupportedBackend {
    fn poll(&mut self) -> GamepadFrame {
        GamepadFrame::disconnected()
    }

    fn name(&self) -> &'static str {
        "unsupported"
    }
}

/// 脚本化后端：按序返回预置帧（帧耗尽后保持最后一帧），供 headless 测试与验收。
#[derive(Debug, Default)]
pub struct FakeBackend {
    frames: Vec<GamepadFrame>,
    cursor: usize,
}

impl FakeBackend {
    /// 用一帧序列构造。
    #[must_use]
    pub fn new(frames: Vec<GamepadFrame>) -> Self {
        Self { frames, cursor: 0 }
    }

    /// 追加一帧（测试中逐步编排设备事件）。
    pub fn push(&mut self, frame: GamepadFrame) {
        self.frames.push(frame);
    }

    /// 已消费帧数。
    #[must_use]
    pub fn consumed(&self) -> usize {
        self.cursor
    }
}

impl GamepadBackend for FakeBackend {
    fn poll(&mut self) -> GamepadFrame {
        if self.frames.is_empty() {
            return GamepadFrame::disconnected();
        }
        let index = self.cursor.min(self.frames.len() - 1);
        self.cursor = self.cursor.saturating_add(1);
        self.frames[index]
    }

    fn name(&self) -> &'static str {
        "fake"
    }
}

/// 轮询器：持有后端并把设备帧写入 [`InputSnapshot`]（保持既有边沿语义）。
pub struct GamepadPoller {
    backend: Box<dyn GamepadBackend>,
    last: GamepadFrame,
}

impl std::fmt::Debug for GamepadPoller {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GamepadPoller")
            .field("backend", &self.backend.name())
            .field("connected", &self.last.connected)
            .finish()
    }
}

impl GamepadPoller {
    /// 用指定后端构造。
    #[must_use]
    pub fn new(backend: Box<dyn GamepadBackend>) -> Self {
        Self {
            backend,
            last: GamepadFrame::disconnected(),
        }
    }

    /// 平台默认后端：Windows 用 XInput，其余平台无设备。
    #[must_use]
    pub fn platform_default() -> Self {
        Self::platform_default_with_deadzone(GamepadDeadzone::DEFAULT)
    }

    /// 平台默认后端 + 指定死区（M20-08：`settings.toml` 的 `[input]` 段）。
    ///
    /// 非 Windows 平台没有设备后端，死区参数被忽略（保持静默降级语义）。
    #[must_use]
    pub fn platform_default_with_deadzone(deadzone: GamepadDeadzone) -> Self {
        #[cfg(windows)]
        {
            Self::new(Box::new(XInputBackend::with_deadzone(deadzone)))
        }
        #[cfg(not(windows))]
        {
            let _ = deadzone;
            Self::new(Box::new(UnsupportedBackend))
        }
    }

    /// 采样一帧设备状态并**更新内部记录**，返回原始帧（M20-08：编辑器重绑定
    /// 面板的「捕获下一次手柄输入」需要直接读设备帧，而不是写进 `InputSnapshot`）。
    ///
    /// 与 [`Self::poll_into`] 不要在同一帧混用：两者都会消费后端的一帧
    /// （`FakeBackend` 的脚本序列会因此错位）。
    pub fn poll_frame(&mut self) -> GamepadFrame {
        let frame = self.backend.poll();
        self.last = frame;
        frame
    }

    /// 当前是否检测到手柄。
    #[must_use]
    pub fn connected(&self) -> bool {
        self.last.connected
    }

    /// 后端名称（诊断用）。
    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// 采样一帧并写入 `input`。
    ///
    /// 语义：
    /// * 按钮**当前状态每帧都写入**（按住为真），按下/松开**边沿只在变化的那一帧**
    ///   产生（`just_pressed`/`just_released` 由 `InputSnapshot` 保留到本逻辑帧被
    ///   消费，快速点按不会丢）。每帧写全量是关键：宿主可能每帧重建输入快照
    ///   （编辑器 Play 预览只桥接键鼠，手柄状态每次都是空的），只写变化会让
    ///   「按住不放」在第二帧起变成没有输入；反过来，把「按住」当成按下事件重复
    ///   喂给快照会让一次性动作每帧重复触发，因此按住状态走
    ///   [`InputSnapshot::set_gamepad_button_held`]、边沿才走
    ///   [`InputSnapshot::gamepad_button_event`]；
    /// * 轴值每帧写入（模拟量，设备持续上报当前杆位）；
    /// * 本帧未连接（拔线/无设备）→ 释放此前按住的按钮并清零轴，避免粘键。
    pub fn poll_into(&mut self, input: &mut InputSnapshot) {
        let frame = self.backend.poll();
        if !frame.connected {
            if self.last.connected {
                for index in 0..GAMEPAD_BUTTON_COUNT {
                    if self.last.buttons[index] {
                        input.gamepad_button_event(index as u8, false);
                    }
                }
                for index in 0..GAMEPAD_AXIS_COUNT {
                    if self.last.axes[index] != 0.0 {
                        input.set_gamepad_axis(index as u8, 0.0);
                    }
                }
                tracing::info!(target: "input", "[gamepad] disconnected; buttons and axes cleared");
            } else {
                // 持续无设备：每帧重建的快照也要看到「没有按住任何键/轴归零」。
                for index in 0..GAMEPAD_BUTTON_COUNT {
                    input.set_gamepad_button_held(index as u8, false);
                }
                for index in 0..GAMEPAD_AXIS_COUNT {
                    input.set_gamepad_axis(index as u8, 0.0);
                }
            }
            self.last = frame;
            return;
        }

        if !self.last.connected {
            tracing::info!(target: "input", backend = self.backend.name(), "[gamepad] connected");
        }
        for index in 0..GAMEPAD_BUTTON_COUNT {
            let now = frame.buttons[index];
            if now != self.last.buttons[index] {
                // 变化帧：写边沿（按下 / 松开），同时更新按住状态。
                input.gamepad_button_event(index as u8, now);
                tracing::debug!(target: "input", index, pressed = now, "[gamepad] button");
            } else if now {
                // 未变化的按住：边沿不重复，只断言按住状态。
                input.set_gamepad_button_held(index as u8, true);
            } else {
                input.set_gamepad_button_held(index as u8, false);
            }
        }
        for index in 0..GAMEPAD_AXIS_COUNT {
            input.set_gamepad_axis(index as u8, frame.axes[index]);
        }
        self.last = frame;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FakeBackend, GamepadDeadzone, GamepadFrame, GamepadPoller, STICK_DEADZONE,
        TRIGGER_DEADZONE, apply_stick_deadzone, apply_trigger_deadzone, axes_from_raw,
        axes_from_raw_with, button, buttons_from_flags, triggers_to_buttons,
    };
    use crate::core::input::{ActionMap, AxisDirection, Binding, InputSnapshot};

    fn frame_with_button(index: usize) -> GamepadFrame {
        let mut frame = GamepadFrame::idle();
        frame.buttons[index] = true;
        frame
    }

    #[test]
    fn deadzone_zeroes_drift_and_rescales_beyond_it() {
        // 死区内 → 0（摇杆回中漂移不产生输入）。
        assert_eq!(apply_stick_deadzone(0.14, 0.15), 0.0);
        assert_eq!(apply_stick_deadzone(-0.1, 0.15), 0.0);
        // 死区外按比例映射，符号保持。
        let value = apply_stick_deadzone(0.575, 0.15);
        assert!((value - 0.5).abs() < 1e-4, "unexpected rescale: {value}");
        assert!(apply_stick_deadzone(-0.575, 0.15) < 0.0);
        // 极值仍为 ±1。
        assert!((apply_stick_deadzone(1.0, 0.15) - 1.0).abs() < 1e-6);
        // 扳机死区：低于阈值归零，之上按比例。
        assert_eq!(apply_trigger_deadzone(0.04, 0.05), 0.0);
        assert!(apply_trigger_deadzone(0.5, 0.05) > 0.4);
    }

    #[test]
    fn button_flags_map_to_contract_indices() {
        let buttons = buttons_from_flags(0x1000 | 0x0001 | 0x0100);
        assert!(buttons[button::A], "A 应命中索引 0");
        assert!(buttons[button::DPAD_UP], "DPadUp 应命中索引 12");
        assert!(buttons[button::LEFT_SHOULDER], "LB 应命中索引 4");
        assert!(!buttons[button::B]);
        assert!(!buttons[button::RIGHT_TRIGGER], "扳机按钮由模拟量派生");
    }

    #[test]
    fn axes_follow_contract_and_y_is_up_positive() {
        let axes = axes_from_raw(0.0, 1.0, 0.0, -1.0, 1.0, 0.0);
        assert!(axes[super::axis::LEFT_Y] > 0.9, "左摇杆上推应为正");
        assert!(axes[super::axis::RIGHT_Y] < -0.9, "右摇杆下推应为负");
        assert!(axes[super::axis::LEFT_TRIGGER] > 0.9);
        assert_eq!(axes[super::axis::RIGHT_TRIGGER], 0.0);

        // 数字扳机按钮与模拟量一致。
        let mut buttons = [false; super::GAMEPAD_BUTTON_COUNT];
        triggers_to_buttons(&mut buttons, &axes);
        assert!(buttons[button::LEFT_TRIGGER]);
        assert!(!buttons[button::RIGHT_TRIGGER]);
    }

    #[test]
    fn poller_preserves_edges_and_held_state() {
        let mut input = InputSnapshot::default();
        let mut backend = FakeBackend::default();
        // 一帧内按下并松开 A：两个边沿都必须保留到逻辑帧被消费。
        backend.push(frame_with_button(button::A));
        let mut released = GamepadFrame::idle();
        released.buttons[button::A] = false;
        backend.push(released);
        let mut poller = GamepadPoller::new(Box::new(backend));

        poller.poll_into(&mut input);
        assert!(input.gamepad_button(0), "按下应生效");
        poller.poll_into(&mut input);
        assert!(!input.gamepad_button(0), "松开后不再按住");
        let map = ActionMap::with_defaults();
        assert!(
            input.action_just_pressed(&map, "confirm"),
            "按下配置的默认确认动作"
        );
        assert!(input.action_just_released(&map, "confirm"), "松开边沿保留");
        input.finish_frame();
        assert!(!input.action_just_pressed(&map, "confirm"));
        assert!(!input.action_just_released(&map, "confirm"));
    }

    #[test]
    fn poller_maps_axis_to_action_binding_and_defaults() {
        let mut input = InputSnapshot::default();
        let mut frame = GamepadFrame::idle();
        frame.axes[super::axis::LEFT_Y] = 0.8;
        let mut poller = GamepadPoller::new(Box::new(FakeBackend::new(vec![frame])));
        poller.poll_into(&mut input);

        // 默认映射按方向拆分：轴 1 上推 → ui_up 命中，且合成轴为正值
        // （约定：`action_axis(negative, positive)` 首参为负向动作）。
        let map = ActionMap::with_defaults();
        assert!(input.action_is_down(&map, "ui_up"));
        assert!(!input.action_is_down(&map, "ui_down"));
        assert!((input.action_axis(&map, "ui_down", "ui_up") - 0.8).abs() < 1e-6);

        // 方向语义：自定义一对动作时同样「负向在前」，上推为正、下推为负。
        let mut stick = ActionMap::with_defaults();
        stick.clear("ui_up");
        stick.clear("ui_down");
        stick
            .bind(
                "stick_down",
                Binding::GamepadAxis(1, AxisDirection::Negative),
            )
            .unwrap();
        stick
            .bind("stick_up", Binding::GamepadAxis(1, AxisDirection::Positive))
            .unwrap();
        assert!((input.action_axis(&stick, "stick_down", "stick_up") - 0.8).abs() < 1e-6);
    }

    #[test]
    fn poll_into_writes_full_state_every_frame_for_fresh_snapshots() {
        // 宿主可能每帧重建快照（编辑器 Play 预览）：设备恒定上报同一帧时，
        // 「按住」与轴值也必须每帧可见，否则推住摇杆/按住按钮从第二帧起失效。
        // 帧 1 = 已连接的空闲帧（建立基线），帧 2 起 = 设备保持按住 A + 轴 0.7，
        // 帧 4 = 松开（保持轴值，验证松开边沿只在变化帧出现）。
        let mut idle = GamepadFrame::idle();
        idle.connected = true;
        let mut held = frame_with_button(button::A);
        held.axes[super::axis::LEFT_X] = 0.7;
        let mut released = held;
        released.buttons[button::A] = false;
        let mut poller = GamepadPoller::new(Box::new(FakeBackend::new(vec![idle, held, released])));
        let map = ActionMap::with_defaults();

        for frame in 0..2 {
            let mut fresh = InputSnapshot::default();
            poller.poll_into(&mut fresh);
            if frame == 0 {
                // 基线帧：设备空闲。
                assert!(!fresh.gamepad_button(button::A as u8));
                assert_eq!(fresh.gamepad_axis(super::axis::LEFT_X as u8), 0.0);
                continue;
            }
            assert!(
                fresh.gamepad_button(button::A as u8),
                "第 {frame} 帧应看到按住的 A"
            );
            assert!(
                (fresh.gamepad_axis(super::axis::LEFT_X as u8) - 0.7).abs() < 1e-6,
                "第 {frame} 帧应看到持续的轴值"
            );
            assert!(fresh.action_is_down(&map, "confirm"), "按住持续命中动作");
            // 边沿语义不变：只有从空闲变按下的那一帧是 just_pressed
            // （按住期间每帧重建快照也不能重复触发一次性动作）。
            assert_eq!(
                frame == 1,
                fresh.action_just_pressed(&map, "confirm"),
                "第 {frame} 帧的按下边沿"
            );
        }
        // 松开：变化帧产生 just_released（每帧重建快照同样只此一帧）。
        let mut released = InputSnapshot::default();
        poller.poll_into(&mut released);
        assert!(
            !released.gamepad_button(button::A as u8),
            "松开后不应再是按住"
        );
        assert!(
            released.action_just_released(&map, "confirm"),
            "松开边沿应在变化帧出现"
        );
        // 再下一帧：仍是松开状态，但没有松开边沿。
        let mut after = InputSnapshot::default();
        poller.poll_into(&mut after);
        assert!(!after.gamepad_button(button::A as u8));
        assert!(
            !after.action_just_released(&map, "confirm"),
            "松开边沿不得每帧重复"
        );
    }

    /// 断连后每帧重建的快照也必须看到「没有按住任何按钮、轴归零」。
    #[test]
    fn disconnected_frame_clears_state_for_fresh_snapshots() {
        let mut poller = GamepadPoller::new(Box::new(FakeBackend::new(vec![
            GamepadFrame::disconnected(),
        ])));
        let map = ActionMap::with_defaults();
        for frame in 0..2 {
            let mut fresh = InputSnapshot::default();
            poller.poll_into(&mut fresh);
            assert_eq!(fresh.gamepad_axis(0), 0.0, "第 {frame} 帧轴归零");
            assert!(!fresh.gamepad_button(0), "第 {frame} 帧无按住按钮");
            assert!(!fresh.action_is_down(&map, "confirm"));
            assert!(!fresh.action_just_pressed(&map, "confirm"));
        }
    }

    #[test]
    fn disconnect_and_focus_loss_clear_device_state() {
        let mut input = InputSnapshot::default();
        let mut backend = FakeBackend::default();
        let mut held = frame_with_button(button::B);
        held.axes[super::axis::LEFT_X] = 0.9;
        backend.push(held);
        backend.push(GamepadFrame::disconnected());
        let mut poller = GamepadPoller::new(Box::new(backend));

        poller.poll_into(&mut input);
        assert!(poller.connected());
        assert!(input.gamepad_button(1));
        assert!(input.gamepad_axis(0) > 0.5);

        // 拔线：按钮与轴清空，不再粘键。
        poller.poll_into(&mut input);
        assert!(!poller.connected());
        assert!(!input.gamepad_button(1));
        assert_eq!(input.gamepad_axis(0), 0.0);

        // 失焦同理（宿主路径）：清空手柄状态。
        let mut input = InputSnapshot::default();
        input.gamepad_button_event(0, true);
        input.set_gamepad_axis(1, 0.7);
        input.set_focused(false);
        assert!(!input.gamepad_button(0));
        assert_eq!(input.gamepad_axis(1), 0.0);
    }

    #[test]
    fn no_device_produces_no_input_and_no_panic() {
        let mut input = InputSnapshot::default();
        let mut poller = GamepadPoller::new(Box::new(FakeBackend::default()));
        poller.poll_into(&mut input);
        assert!(!poller.connected());
        let map = ActionMap::with_defaults();
        assert!(!input.action_is_down(&map, "confirm"));
        assert!(!input.action_is_down(&map, "ui_up"));
        assert!(!input.action_is_down(&map, "ui_down"));
        assert_eq!(poller.backend_name(), "fake");
    }

    #[test]
    fn platform_default_backend_is_available() {
        let poller = GamepadPoller::platform_default();
        #[cfg(windows)]
        assert_eq!(poller.backend_name(), "xinput");
        #[cfg(not(windows))]
        assert_eq!(poller.backend_name(), "unsupported");
    }

    #[test]
    fn deadzone_config_keeps_defaults_and_clamps_dirty_values() {
        // 默认值与既有常量一致（M20-06 语义不变）。
        let default = GamepadDeadzone::DEFAULT;
        assert_eq!(default.stick, STICK_DEADZONE);
        assert_eq!(default.trigger, TRIGGER_DEADZONE);
        assert_eq!(GamepadDeadzone::default(), default);
        assert!(default.is_default());
        // 脏数据（非有限/负/超界）被钳制，构造后一定是合法曲线。
        let dirty = GamepadDeadzone::new(f32::NAN, 5.0);
        assert_eq!(dirty.stick, STICK_DEADZONE, "非有限值回落默认");
        assert_eq!(dirty.trigger, super::MAX_DEADZONE);
        let negative = GamepadDeadzone::new(-1.0, 0.02);
        assert_eq!(negative.stick, 0.0);
        assert!((negative.trigger - 0.02).abs() < 1e-6);
        assert!(!negative.is_default());

        // 自定义死区只影响轴映射，不改变「死区内归零、死区外重映射」的语义。
        let wide = GamepadDeadzone::new(0.5, 0.5);
        let axes = axes_from_raw_with(0.4, 0.4, 0.0, 0.0, 0.4, 0.9, wide);
        assert_eq!(axes[super::axis::LEFT_X], 0.0, "0.4 落在 0.5 死区内");
        assert_eq!(axes[super::axis::LEFT_TRIGGER], 0.0);
        assert!((axes[super::axis::RIGHT_TRIGGER] - 0.8).abs() < 1e-4);
        // 默认死区下同样的输入仍有值（证明死区确实生效）。
        let default_axes = axes_from_raw(0.4, 0.4, 0.0, 0.0, 0.4, 0.9);
        assert!(default_axes[super::axis::LEFT_X] > 0.29);
        assert!(default_axes[super::axis::LEFT_TRIGGER] > 0.36);
    }

    #[test]
    fn poll_frame_reports_raw_device_frame_for_capture_ui() {
        let mut frame = GamepadFrame::idle();
        frame.buttons[button::X] = true;
        frame.axes[super::axis::LEFT_Y] = -1.0;
        let mut poller = GamepadPoller::new(Box::new(FakeBackend::new(vec![frame])));
        let sampled = poller.poll_frame();
        assert!(sampled.connected);
        assert!(sampled.buttons[button::X]);
        assert!((sampled.axes[super::axis::LEFT_Y] + 1.0).abs() < 1e-6);
        assert!(poller.connected(), "poll_frame 同步更新连接状态");
        // 帧耗尽后保持最后一帧（FakeBackend 语义），断连帧清空连接标志。
        let mut disconnected = FakeBackend::new(vec![GamepadFrame::idle()]);
        disconnected.push(GamepadFrame::disconnected());
        let mut poller = GamepadPoller::new(Box::new(disconnected));
        assert!(poller.poll_frame().connected);
        assert!(!poller.poll_frame().connected);
        assert!(!poller.connected());
    }

    #[test]
    fn platform_default_with_deadzone_reports_same_backend() {
        let poller = GamepadPoller::platform_default_with_deadzone(GamepadDeadzone::new(0.3, 0.1));
        #[cfg(windows)]
        assert_eq!(poller.backend_name(), "xinput");
        #[cfg(not(windows))]
        assert_eq!(poller.backend_name(), "unsupported");
    }
}
