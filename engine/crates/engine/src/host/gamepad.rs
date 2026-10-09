//! 手柄设备轮询适配层（M20-06）。
//!
//! 引擎的**抽象**手柄输入早已就位（`crate::core::input::InputSnapshot` 的
//! `gamepad_button_event` / `set_gamepad_axis` 与 `ActionMap` 的
//! `Binding::GamepadButton` / `Binding::GamepadAxis`），但此前没有任何代码
//! 喂入真实设备事件。本模块补齐这一层：
//!
//! * [`GamepadBackend`]：平台无关的轮询接口，`poll()` 返回一帧 [`GamepadFrame`]；
//! * [`crate::host::gamepad::XInputBackend`]：Windows 原生 XInput（`windows-sys`，无需新增 crate）；
//! * [`crate::host::gamepad::FakeBackend`]：脚本化帧，供 headless 测试与验收驱动；
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
//! 死区与阈值是**两个概念**：死区（[`crate::core::input::STICK_DEADZONE`]/[`crate::core::input::TRIGGER_DEADZONE`]）用于
//! 消除漂移，动作判定阈值沿用 `crate::core::input` 的 0.5。

use crate::core::input::{AxisDirection, InputSnapshot};
// 时钟必须走 `web-time`（与渲染器 `passes.rs` 同一处理）：`wasm32-unknown-unknown` 的 std
// 把 `Instant::now()` 实现在 `sys/pal/unsupported/time.rs` 上，是**直接 panic**。
// 这里踩到的原因是"每帧"：`expire_rumble` 由 `poll_into` 调用，而 `poll_into` 在
// `RuntimeScene::run_script_steps` 里——wasm 上只要推进一帧就炸，而且炸在"手柄震动到期"
// 这种与画面无关的功能上（第 14 轮就是这么被拦下来的）。
// `web-time` 在原生上就是 `std::time::Instant` 的再导出，零行为变化。
use web_time::Instant;

// M22（W5）：死区值类型与纯函数已下沉到 `crate::core::input`（core 不再反向依赖 host）。
// 这里保留重导出，既有 `host::gamepad::{GamepadDeadzone, MAX_DEADZONE, ...}` 路径
// 与调用方（编辑器面板、示例、headless 测试）不受影响。
pub use crate::core::input::{
    AXIS_DIGITAL_OFF, AXIS_DIGITAL_ON, AXIS_HYSTERESIS, GamepadDeadzone, MAX_DEADZONE,
    STICK_DEADZONE, TRIGGER_DEADZONE, apply_stick_deadzone, apply_stick_deadzone_radial,
    apply_trigger_deadzone,
};

/// 按钮数量（见模块文档的索引契约）。
pub const GAMEPAD_BUTTON_COUNT: usize = 16;
/// 轴数量（见模块文档的索引契约）。
pub const GAMEPAD_AXIS_COUNT: usize = 6;
/// 手柄槽位数（XInput 固定支持 4 个用户；M22/W2）。
pub const GAMEPAD_SLOT_COUNT: usize = 4;
/// 单次震动时长上限（毫秒；更长的请求被钳制，防止「设下去忘了停」）。
pub const MAX_RUMBLE_MS: u32 = 5000;

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
    ///
    /// 语义是「第一个可用设备」（M20-06 起的行为），供 `GamepadPoller::poll_frame` 与默认
    /// [`Self::poll_all`] 使用。
    fn poll(&mut self) -> GamepadFrame;

    /// 后端名称（日志与诊断用）。
    fn name(&self) -> &'static str;

    /// 逐槽位采样（M22/W2）。
    ///
    /// 默认实现把 [`Self::poll`] 的结果放到 slot 0（未连接则全 `None`），因此既有后端与
    /// `FakeBackend` 无需改动即可继续工作。XInput 后端覆写为逐用户采样。
    fn poll_all(&mut self) -> [Option<GamepadFrame>; GAMEPAD_SLOT_COUNT] {
        let mut slots: [Option<GamepadFrame>; GAMEPAD_SLOT_COUNT] = [None; GAMEPAD_SLOT_COUNT];
        let frame = self.poll();
        if frame.connected {
            slots[0] = Some(frame);
        }
        slots
    }

    /// 设置某个槽位的震动强度（左右马达 0..1；M22/W2）。默认空实现（无震动设备）。
    fn set_rumble(&mut self, _slot: u8, _low: f32, _high: f32) {}
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
/// 摇杆走**径向**死区（M22：按模长判定，消除「斜向比正向灵敏」），扳机逐轴。
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
    let (left_x, left_y) = apply_stick_deadzone_radial(left_x, left_y, deadzone.stick);
    let (right_x, right_y) = apply_stick_deadzone_radial(right_x, right_y, deadzone.stick);
    axes[axis::LEFT_X] = left_x;
    axes[axis::LEFT_Y] = left_y;
    axes[axis::RIGHT_X] = right_x;
    axes[axis::RIGHT_Y] = right_y;
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
        poll_xinput_first(self.deadzone)
    }

    fn poll_all(&mut self) -> [Option<GamepadFrame>; GAMEPAD_SLOT_COUNT] {
        let mut slots: [Option<GamepadFrame>; GAMEPAD_SLOT_COUNT] = [None; GAMEPAD_SLOT_COUNT];
        for (index, slot) in slots.iter_mut().enumerate() {
            *slot = poll_xinput_user(index, self.deadzone);
        }
        slots
    }

    fn name(&self) -> &'static str {
        "xinput"
    }

    fn set_rumble(&mut self, slot: u8, low: f32, high: f32) {
        set_xinput_rumble(slot, low, high);
    }
}

/// 采样 0..3 号 XInput 用户，返回第一个已连接设备的状态。
#[cfg(windows)]
fn poll_xinput_first(deadzone: GamepadDeadzone) -> GamepadFrame {
    for index in 0..GAMEPAD_SLOT_COUNT {
        if let Some(frame) = poll_xinput_user(index, deadzone) {
            return frame;
        }
    }
    GamepadFrame::disconnected()
}

/// 采样单个 XInput 用户；未连接（`ERROR_DEVICE_NOT_CONNECTED`）返回 `None`（M22/W2）。
#[cfg(windows)]
fn poll_xinput_user(index: usize, deadzone: GamepadDeadzone) -> Option<GamepadFrame> {
    use windows_sys::Win32::UI::Input::XboxController::{
        XINPUT_GAMEPAD, XINPUT_STATE, XInputGetState,
    };

    const ERROR_SUCCESS: u32 = 0;
    let mut state = XINPUT_STATE::default();
    // SAFETY: `state` 是有效的可写结构；索引来自 XInput 规定范围（0..3）。
    let result = unsafe { XInputGetState(index as u32, &mut state) };
    if result != ERROR_SUCCESS {
        // 1167 = ERROR_DEVICE_NOT_CONNECTED；其余为驱动/参数错误，同样按「该槽位无设备」处理
        //（保持 M20-06 的静默降级语义）。
        return None;
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
    Some(GamepadFrame {
        buttons,
        axes,
        connected: true,
    })
}

/// 设置 XInput 震动（`XInputSetState`；M22/W2）。强度 0..1 → 0..65535。
#[cfg(windows)]
fn set_xinput_rumble(slot: u8, low: f32, high: f32) {
    use windows_sys::Win32::UI::Input::XboxController::{XINPUT_VIBRATION, XInputSetState};

    let scale =
        |value: f32| -> u16 { (value.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16 };
    let vibration = XINPUT_VIBRATION {
        wLeftMotorSpeed: scale(low),
        wRightMotorSpeed: scale(high),
    };
    // SAFETY: `vibration` 是有效的可读结构；slot 来自 XInput 规定范围。
    let _ = unsafe { XInputSetState(u32::from(slot), &vibration) };
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

/// 震动调用记录（M22/W2）：poller 持有 `Box<dyn GamepadBackend>`，因此用共享句柄让
/// 测试/示例在把后端交给 poller 之后仍能读回 `set_rumble` 调用序列。
#[derive(Debug, Default, Clone)]
pub struct RumbleLog(std::sync::Arc<std::sync::Mutex<Vec<(u8, f32, f32)>>>);

impl RumbleLog {
    /// 已记录的调用（槽位、左马达、右马达），按发生顺序。
    #[must_use]
    pub fn calls(&self) -> Vec<(u8, f32, f32)> {
        self.0.lock().map(|calls| calls.clone()).unwrap_or_default()
    }

    fn record(&self, slot: u8, low: f32, high: f32) {
        if let Ok(mut calls) = self.0.lock() {
            calls.push((slot, low, high));
        }
    }
}

/// 多槽位脚本化后端（M22/W2）：每帧给出 4 个槽位的状态，并记录 `set_rumble` 调用。
///
/// 与 [`crate::host::gamepad::FakeBackend`] 并列：`FakeBackend` 用于单设备脚本帧（经默认 `poll_all` 落在 slot 0），
/// 本类型用于验证槽位切换/热插拔/震动（headless 测试与验收示例共用）。
#[derive(Debug, Default)]
pub struct SlotBackend {
    frames: Vec<[Option<GamepadFrame>; GAMEPAD_SLOT_COUNT]>,
    cursor: usize,
    log: RumbleLog,
}

impl SlotBackend {
    /// 用逐槽位的帧序列构造。
    #[must_use]
    pub fn new(frames: Vec<[Option<GamepadFrame>; GAMEPAD_SLOT_COUNT]>) -> Self {
        Self {
            frames,
            cursor: 0,
            log: RumbleLog::default(),
        }
    }

    /// 追加一帧（4 个槽位）。
    pub fn push(&mut self, frame: [Option<GamepadFrame>; GAMEPAD_SLOT_COUNT]) {
        self.frames.push(frame);
    }

    /// 震动调用记录句柄（可在后端被 move 进 poller 之后继续读取）。
    #[must_use]
    pub fn rumble_log(&self) -> RumbleLog {
        self.log.clone()
    }

    /// 当前帧（帧耗尽后保持最后一帧）。
    fn current(&self) -> [Option<GamepadFrame>; GAMEPAD_SLOT_COUNT] {
        if self.frames.is_empty() {
            return [None; GAMEPAD_SLOT_COUNT];
        }
        self.frames[self.cursor.min(self.frames.len() - 1)]
    }
}

impl GamepadBackend for SlotBackend {
    fn poll(&mut self) -> GamepadFrame {
        let slots = self.poll_all();
        slots
            .iter()
            .flatten()
            .next()
            .copied()
            .unwrap_or_else(GamepadFrame::disconnected)
    }

    fn poll_all(&mut self) -> [Option<GamepadFrame>; GAMEPAD_SLOT_COUNT] {
        let slots = self.current();
        self.cursor = self.cursor.saturating_add(1);
        slots
    }

    fn name(&self) -> &'static str {
        "slots"
    }

    fn set_rumble(&mut self, slot: u8, low: f32, high: f32) {
        self.log.record(slot, low, high);
    }
}

/// 轮询器：持有后端并把设备帧写入 [`InputSnapshot`]（保持既有边沿语义）。
///
/// M22/W3：轴除模拟量外还维护**滞回数字态与边沿**（`axis_digital`），这样
/// `input.action_pressed("ui_up")` 对摇杆也成立，菜单导航不再需要在 Lua 里按帧计数。
/// M22/W2：逐槽位跟踪设备（`slots`），并用**粘性**活动槽位（连着的设备不会被后来接入的
/// 低编号设备抢走，切换/断连时释放旧槽位仍按住的按钮与轴），另支持震动到期归零。
pub struct GamepadPoller {
    backend: Box<dyn GamepadBackend>,
    /// 每个槽位最近一次采样（未连接为 disconnected 帧）。
    slots: [GamepadFrame; GAMEPAD_SLOT_COUNT],
    /// 每个槽位的轴方向数字态（`[槽位][轴][0=负方向, 1=正方向]`）。
    axis_digital: [[[bool; 2]; GAMEPAD_AXIS_COUNT]; GAMEPAD_SLOT_COUNT],
    /// 当前活动槽位（`None` = 无设备）。
    active: Option<usize>,
    /// 宿主/脚本显式指定的优先槽位（未连接时回落到自动选择）。
    preferred: Option<usize>,
    /// 轴判定进入阈值（M22/W3，可由 `[input] nav_threshold` 配置）。
    axis_threshold: f32,
    /// 每个槽位的震动到期时间（M22/W2）。
    rumble_until: [Option<Instant>; GAMEPAD_SLOT_COUNT],
    /// 槽位是否**出现过真实输入**（按钮按下或轴偏移；M22/W2）。
    ///
    /// 用于避开「连接着但从未有输入」的槽位：XInput 下可能存在占位/虚拟设备（或手柄处于
    /// 待机），此时「第一个已连接」会选错设备，表现为「手柄明明按了却没反应」。
    slot_activity: [bool; GAMEPAD_SLOT_COUNT],
}

/// 轴的数字态槽位顺序（与 `axis_digital` 的第二维对应）。
const AXIS_DIRECTIONS: [AxisDirection; 2] = [AxisDirection::Negative, AxisDirection::Positive];

/// 方向 → 数字态槽位下标。
const fn axis_direction_slot(direction: AxisDirection) -> usize {
    match direction {
        AxisDirection::Negative => 0,
        AxisDirection::Positive => 1,
    }
}

/// 滞回：已按下时用退出阈值保持，未按下时用进入阈值触发。
fn axis_digital_next(previous: bool, signed_value: f32, on: f32, off: f32) -> bool {
    if previous {
        signed_value > off
    } else {
        signed_value >= on
    }
}

/// 进入阈值 → (进入, 退出)：退出阈值比进入低 [`AXIS_HYSTERESIS`]，钳制到 ≥ 0。
fn axis_digital_thresholds(on: f32) -> (f32, f32) {
    let on = if on.is_finite() {
        on.clamp(0.0, 1.0)
    } else {
        AXIS_DIGITAL_ON
    };
    (on, (on - AXIS_HYSTERESIS).max(0.0))
}

/// 该帧是否**出现过真实输入**（任一按钮按下，或任一轴偏移超过噪声容限）。
///
/// 用于槽位选择（M22/W2）：占位/虚拟 XInput 设备会「已连接但永远全零」，
/// 用它把活动槽位让给真正在操作的手柄。轴容限刻意小于动作阈值（0.25），
/// 这样轻轻动一下摇杆就能让设备被认到，而不必推过 0.5。
fn slot_shows_input(frame: &GamepadFrame) -> bool {
    const ACTIVITY_AXIS: f32 = 0.25;
    frame.buttons.iter().any(|pressed| *pressed)
        || frame.axes.iter().any(|value| value.abs() >= ACTIVITY_AXIS)
}

impl std::fmt::Debug for GamepadPoller {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GamepadPoller")
            .field("backend", &self.backend.name())
            .field("active_slot", &self.active)
            .field("connected", &self.connected())
            .finish()
    }
}

impl GamepadPoller {
    /// 用指定后端构造。
    #[must_use]
    pub fn new(backend: Box<dyn GamepadBackend>) -> Self {
        Self {
            backend,
            slots: [GamepadFrame::disconnected(); GAMEPAD_SLOT_COUNT],
            axis_digital: [[[false; 2]; GAMEPAD_AXIS_COUNT]; GAMEPAD_SLOT_COUNT],
            active: None,
            preferred: None,
            axis_threshold: AXIS_DIGITAL_ON,
            rumble_until: [None; GAMEPAD_SLOT_COUNT],
            slot_activity: [false; GAMEPAD_SLOT_COUNT],
        }
    }

    /// 设置轴判定进入阈值（M22/W3；来自 `[input] nav_threshold`，缺省
    /// [`AXIS_DIGITAL_ON`]）。非有限/越界值回落到默认，退出阈值 = 进入 −
    /// [`AXIS_HYSTERESIS`]。
    pub fn set_axis_threshold(&mut self, threshold: f32) {
        self.axis_threshold = axis_digital_thresholds(threshold).0;
    }

    /// 当前轴判定进入阈值。
    #[must_use]
    pub fn axis_threshold(&self) -> f32 {
        self.axis_threshold
    }

    /// 显式指定优先使用的槽位（M22/W2；`None` = 自动：粘住当前设备，否则取第一个已连接）。
    ///
    /// 本地双人/多设备时由宿主或 Lua 指定；指定的槽位未连接时回落到自动选择。
    pub fn set_active_slot(&mut self, slot: Option<usize>) {
        self.preferred = slot.filter(|slot| *slot < GAMEPAD_SLOT_COUNT);
    }

    /// 当前活动槽位（没有设备时为 `None`）。
    #[must_use]
    pub fn active_slot(&self) -> Option<usize> {
        self.active
    }

    /// 触发震动（M22/W2）：`low`/`high` 是左右马达强度 0..1，`duration_ms` 后自动停止
    /// （`0` = 立即停止）。无设备时静默忽略。
    ///
    /// XInput 的震动是「设置即持续」，因此这里记下到期时间，由 [`Self::poll_into`] 每帧
    /// 检查归零（不依赖调用方记得清除）。
    pub fn rumble(&mut self, low: f32, high: f32, duration_ms: u32) {
        let Some(slot) = self.active else {
            return;
        };
        let low = if low.is_finite() {
            low.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let high = if high.is_finite() {
            high.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let millis = duration_ms.min(MAX_RUMBLE_MS);
        self.backend.set_rumble(slot as u8, low, high);
        if millis == 0 {
            self.backend.set_rumble(slot as u8, 0.0, 0.0);
            self.rumble_until[slot] = None;
            return;
        }
        self.rumble_until[slot] =
            Some(Instant::now() + std::time::Duration::from_millis(u64::from(millis)));
        tracing::debug!(target: "input", slot, low, high, millis, "[gamepad] rumble");
    }

    /// 到期归零（每帧由 [`Self::poll_into`] 调用）。
    fn expire_rumble(&mut self) {
        let now = Instant::now();
        for slot in 0..GAMEPAD_SLOT_COUNT {
            if let Some(deadline) = self.rumble_until[slot]
                && now >= deadline
            {
                self.backend.set_rumble(slot as u8, 0.0, 0.0);
                self.rumble_until[slot] = None;
                tracing::debug!(target: "input", slot, "[gamepad] rumble stopped");
            }
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

    /// 采样一帧设备状态并**更新内部记录**，返回活动槽位的帧。
    ///
    /// 用途：**自己持有 poller、且没有别的采样者**的场景（编辑器重绑定面板的
    /// 「捕获下一次手柄输入」就是这种：它有自己的 poller，与 Play 预览的 poller 相互独立）。
    ///
    /// 与 [`Self::poll_into`] 的分工（M22/W4）：本方法会**消费后端的一帧**，因此同一个 poller
    /// 上不要与 `poll_into` 同帧混用（`FakeBackend` 的脚本序列会错位）。需要「读取当前逻辑帧
    /// 所用的那帧设备状态」请用不消费的 [`Self::last_frame`]。
    pub fn poll_frame(&mut self) -> GamepadFrame {
        let frames = self.backend.poll_all();
        for (slot, frame) in frames.iter().enumerate() {
            self.slots[slot] = frame.unwrap_or_else(GamepadFrame::disconnected);
        }
        self.active = self
            .preferred
            .filter(|slot| frames[*slot].is_some())
            .or_else(|| {
                self.active
                    .filter(|slot| frames[*slot].is_some())
                    .or_else(|| (0..GAMEPAD_SLOT_COUNT).find(|slot| frames[*slot].is_some()))
            });
        self.expire_rumble();
        self.last_frame()
    }

    /// 活动槽位最近一次采样到的设备帧（**不再采样**，M22/W4）。
    ///
    /// 与 [`Self::poll_frame`] 的区别：只读、不消费后端的一帧，因此可以与 [`Self::poll_into`]
    /// 在同一帧同时使用——运行时调试/捕获面板读到的正是当前逻辑帧所用的设备状态。
    /// 尚未采样过（或无设备）时返回未连接帧。
    #[must_use]
    pub fn last_frame(&self) -> GamepadFrame {
        self.active
            .map(|slot| self.slots[slot])
            .unwrap_or_else(GamepadFrame::disconnected)
    }

    /// 当前活动槽位是否有手柄。
    #[must_use]
    pub fn connected(&self) -> bool {
        self.active.is_some()
    }

    /// 指定槽位是否连接（M22/W2；诊断与多设备选择用）。
    ///
    /// 状态来自最近一次采样（`poll_into`/`poll_frame`），因此调用前需要先采样。
    #[must_use]
    pub fn slot_connected(&self, slot: usize) -> bool {
        self.slots.get(slot).is_some_and(|frame| frame.connected)
    }

    /// 已连接的槽位列表（升序；M22/W2）。
    #[must_use]
    pub fn connected_slots(&self) -> Vec<usize> {
        (0..GAMEPAD_SLOT_COUNT)
            .filter(|slot| self.slot_connected(*slot))
            .collect()
    }

    /// 指定槽位最近一次采样到的帧（诊断用；未连接返回未连接帧）。
    #[must_use]
    pub fn slot_frame(&self, slot: usize) -> GamepadFrame {
        self.slots
            .get(slot)
            .copied()
            .unwrap_or_else(GamepadFrame::disconnected)
    }

    /// 指定槽位是否出现过真实输入（M22/W2；诊断与「占位设备」排查用）。
    #[must_use]
    pub fn slot_has_input(&self, slot: usize) -> bool {
        self.slot_activity.get(slot).copied().unwrap_or(false)
    }

    /// 后端名称（诊断用）。
    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// 采样一帧并写入 `input`（使用活动槽位；M22/W2 起逐槽位跟踪）。
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
    /// * 轴值每帧写入（模拟量，设备持续上报当前杆位）；轴的**数字态与边沿**
    ///   （M22/W3）用同一套约定：越过阈值（滞回 `AXIS_DIGITAL_ON/OFF`）才产生
    ///   `action_pressed`，按住期间每帧只断言数字态；
    /// * **活动槽位**（M22/W2）：显式指定 → 粘住当前设备 → 第一个已连接；切换槽位或拔线时
    ///   释放旧槽位仍按住的按钮与轴方向（不粘键）；
    /// * 无设备时每帧写「没有按住任何按钮、轴归零」，每帧重建的快照也一致。
    pub fn poll_into(&mut self, input: &mut InputSnapshot) {
        let frames = self.backend.poll_all();

        // 1) 逐槽位记录连接变化（诊断）并更新状态；切换前先留住旧活动槽位的帧，用于释放。
        let previous_active = self.active;
        let previous_frame = previous_active.map(|slot| self.slots[slot]);
        for (slot, frame) in frames.iter().enumerate() {
            let frame = frame.unwrap_or_else(GamepadFrame::disconnected);
            if frame.connected != self.slots[slot].connected {
                if frame.connected {
                    tracing::info!(target: "input", slot, backend = self.backend.name(), "[gamepad] connected");
                } else {
                    tracing::info!(target: "input", slot, "[gamepad] disconnected; buttons and axes cleared");
                }
            }
            self.slots[slot] = frame;
        }

        // 2) 选择活动槽位：显式指定 → 有输入活动的槽位 → 粘住当前 → 第一个已连接。
        //
        // 「有输入活动」优先于「第一个已连接」是为了避开占位/虚拟 XInput 槽位（M22/W2）：
        // 若 0 号槽位挂着从不产生输入的设备，而玩家真正用的是 1 号，那么按下任意键的那一帧
        // 就应切到 1 号（切槽位会释放旧槽位的按住状态，因此不会留下假输入）。
        for (slot, frame) in frames.iter().enumerate() {
            if frame.as_ref().is_some_and(slot_shows_input) {
                self.slot_activity[slot] = true;
            }
        }
        let any_activity = (0..GAMEPAD_SLOT_COUNT).any(|slot| self.slot_activity[slot]);
        let first_with_activity = (0..GAMEPAD_SLOT_COUNT)
            .find(|slot| frames[*slot].is_some() && self.slot_activity[*slot]);
        let next_active = self
            .preferred
            .filter(|slot| frames[*slot].is_some())
            .or_else(|| {
                previous_active.filter(|slot| {
                    frames[*slot].is_some() && (self.slot_activity[*slot] || !any_activity)
                })
            })
            .or(first_with_activity)
            .or_else(|| (0..GAMEPAD_SLOT_COUNT).find(|slot| frames[*slot].is_some()));

        // 3) 切换/断连：释放旧槽位仍按住的按钮与轴方向。
        if previous_active != next_active {
            if let Some(old) = previous_active {
                let old_frame = previous_frame.unwrap_or_else(GamepadFrame::disconnected);
                for index in 0..GAMEPAD_BUTTON_COUNT {
                    if old_frame.buttons[index] {
                        input.gamepad_button_event(index as u8, false);
                    }
                }
                for index in 0..GAMEPAD_AXIS_COUNT {
                    if old_frame.axes[index] != 0.0 {
                        input.set_gamepad_axis(index as u8, 0.0);
                    }
                    for direction in AXIS_DIRECTIONS {
                        let slot = axis_direction_slot(direction);
                        if self.axis_digital[old][index][slot] {
                            input.gamepad_axis_event(index as u8, direction, false);
                        }
                        self.axis_digital[old][index][slot] = false;
                    }
                }
            }
            self.active = next_active;
        }

        let Some(active) = self.active else {
            // 持续无设备：每帧重建的快照也要看到「没有按住任何键/轴归零」。
            for index in 0..GAMEPAD_BUTTON_COUNT {
                input.set_gamepad_button_held(index as u8, false);
            }
            for index in 0..GAMEPAD_AXIS_COUNT {
                input.set_gamepad_axis(index as u8, 0.0);
                for direction in AXIS_DIRECTIONS {
                    input.set_gamepad_axis_digital(index as u8, direction, false);
                }
            }
            self.expire_rumble();
            return;
        };

        let frame = self.slots[active];
        let previous = previous_frame.unwrap_or_else(GamepadFrame::disconnected);
        for index in 0..GAMEPAD_BUTTON_COUNT {
            let now = frame.buttons[index];
            if now != previous.buttons[index] {
                // 变化帧：写边沿（按下 / 松开），同时更新按住状态。
                input.gamepad_button_event(index as u8, now);
                tracing::debug!(target: "input", slot = active, index, pressed = now, "[gamepad] button");
            } else if now {
                // 未变化的按住：边沿不重复，只断言按住状态。
                input.set_gamepad_button_held(index as u8, true);
            } else {
                input.set_gamepad_button_held(index as u8, false);
            }
        }
        let (on, off) = axis_digital_thresholds(self.axis_threshold);
        for index in 0..GAMEPAD_AXIS_COUNT {
            let value = frame.axes[index];
            input.set_gamepad_axis(index as u8, value);
            for direction in AXIS_DIRECTIONS {
                let slot = axis_direction_slot(direction);
                let was = self.axis_digital[active][index][slot];
                let now = axis_digital_next(was, value * direction.sign(), on, off);
                if now != was {
                    // 变化帧：写边沿（越阈值 / 回中），同时更新数字态。
                    input.gamepad_axis_event(index as u8, direction, now);
                    tracing::debug!(
                        target: "input",
                        slot = active,
                        index,
                        direction = direction.to_text(),
                        pressed = now,
                        "[gamepad] axis"
                    );
                } else {
                    input.set_gamepad_axis_digital(index as u8, direction, now);
                }
                self.axis_digital[active][index][slot] = now;
            }
        }
        self.expire_rumble();
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
        // M22/W3：摇杆走**径向**死区（按模长判定），因此 (0.4, 0.4) 的模长 0.566
        // 已越过 0.5 —— 换一个模长落在死区内的点来验证归零。
        let wide = GamepadDeadzone::new(0.5, 0.5);
        let inside = axes_from_raw_with(0.3, 0.3, 0.0, 0.0, 0.4, 0.9, wide);
        assert_eq!(
            inside[super::axis::LEFT_X],
            0.0,
            "模长 0.424 落在 0.5 死区内"
        );
        assert_eq!(inside[super::axis::LEFT_Y], 0.0);
        let axes = axes_from_raw_with(0.4, 0.4, 0.0, 0.0, 0.4, 0.9, wide);
        assert!(axes[super::axis::LEFT_X] > 0.0, "模长 0.566 已越过 0.5");
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

    /// 构造一帧：左摇杆 X 推到 `value`（已连接）。
    fn frame_with_stick_x(value: f32) -> GamepadFrame {
        let mut frame = GamepadFrame::idle();
        frame.axes[super::axis::LEFT_X] = value;
        frame
    }

    /// M22/W4：`last_frame()` 与 `poll_into` 共享同一帧，且**不消费**后端序列。
    ///
    /// 场景：运行时调试/捕获面板想看到「当前逻辑帧用的设备状态」，同时 Play 循环仍在
    /// 用同一个 poller 采样。只读的 `last_frame` 不会打扰边沿语义，也不会推进 `FakeBackend`。
    #[test]
    fn last_frame_shares_sampled_frame_without_consuming() {
        let map = ActionMap::with_defaults();
        let mut held = frame_with_button(button::A);
        held.axes[super::axis::LEFT_X] = 0.0;
        let mut poller = GamepadPoller::new(Box::new(FakeBackend::new(vec![
            GamepadFrame::idle(),
            held,
            GamepadFrame::disconnected(),
        ])));

        // 尚未采样：未连接帧。
        assert!(!poller.last_frame().connected, "未采样时为未连接帧");

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(poller.last_frame().connected, "采样后共享同一帧");
        assert!(!poller.last_frame().buttons[button::A]);

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        // 连续读取多次不消费后端，也不会重复/丢失边沿。
        for _ in 0..3 {
            assert!(poller.last_frame().buttons[button::A], "按住帧应稳定");
        }
        assert!(input.action_just_pressed(&map, "confirm"));

        // 第三个脚本帧是断连帧：若 `last_frame` 消费了帧，这里会错位。
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(!poller.last_frame().connected, "断连帧未被 last_frame 吃掉");
        assert!(input.action_just_released(&map, "confirm"));
    }

    /// M22/W3：轴越过阈值产生**按下边沿**，动作层可直接用 `action_pressed`。
    #[test]
    fn poller_synthesizes_axis_press_edges_with_hysteresis() {
        let map = ActionMap::with_defaults();
        let mut poller = GamepadPoller::new(Box::new(FakeBackend::new(vec![
            GamepadFrame::idle(),
            frame_with_stick_x(0.6),
            // 0.45：进入滞回区（> AXIS_DIGITAL_OFF）→ 仍算按住，不产生松开边沿。
            frame_with_stick_x(0.45),
            // 0.3：低于 OFF → 松开边沿。
            frame_with_stick_x(0.3),
            // 0.45：高于 OFF 但低于 ON → 不重新按下（滞回）。
            frame_with_stick_x(0.45),
            frame_with_stick_x(0.7),
        ])));

        // 基线帧：空闲。
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(!input.action_is_down(&map, "ui_right"));

        // 推过 0.5 → 按下边沿 + 按住。
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(
            input.action_just_pressed(&map, "ui_right"),
            "轴应产生按下边沿"
        );
        assert!(input.action_is_down(&map, "ui_right"));

        // 0.45：滞回区内保持按住，且不重复按下边沿。
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(input.action_is_down(&map, "ui_right"), "0.45 仍在滞回区内");
        assert!(!input.action_just_pressed(&map, "ui_right"), "不得重复按下");
        assert!(!input.action_just_released(&map, "ui_right"));

        // 0.3：低于 OFF → 松开边沿。
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(input.action_just_released(&map, "ui_right"));
        assert!(!input.action_is_down(&map, "ui_right"));

        // 0.45：不重新按下（ON=0.5 才是进入阈值）。
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(
            !input.action_is_down(&map, "ui_right"),
            "0.45 不足以重新进入"
        );
        assert!(!input.action_just_pressed(&map, "ui_right"));

        // 0.7：重新按下。
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(input.action_just_pressed(&map, "ui_right"));
    }

    /// M22/W3：轴按住期间每帧重建快照也不重复产生进入边沿（与按钮同一套约定）。
    #[test]
    fn poller_axis_hold_does_not_repeat_edges_across_frames() {
        let map = ActionMap::with_defaults();
        let mut poller = GamepadPoller::new(Box::new(FakeBackend::new(vec![
            GamepadFrame::idle(),
            frame_with_stick_x(0.9),
            frame_with_stick_x(0.9),
            frame_with_stick_x(0.9),
        ])));

        let mut edges = 0;
        let mut held = 0;
        for _ in 0..4 {
            let mut input = InputSnapshot::default();
            poller.poll_into(&mut input);
            if input.action_just_pressed(&map, "ui_right") {
                edges += 1;
            }
            if input.action_is_down(&map, "ui_right") {
                held += 1;
            }
        }
        assert_eq!(edges, 1, "按住期间只应有一次按下边沿");
        assert_eq!(held, 3, "推住的三帧都应看到按住");
    }

    /// M22/W3：拔线时释放仍在数字按下的轴方向（不粘杆）。
    #[test]
    fn poller_releases_axis_directions_on_disconnect() {
        let map = ActionMap::with_defaults();
        let mut poller = GamepadPoller::new(Box::new(FakeBackend::new(vec![
            frame_with_stick_x(0.9),
            GamepadFrame::disconnected(),
        ])));

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(input.action_is_down(&map, "ui_right"));

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(
            input.action_just_released(&map, "ui_right"),
            "拔线应产生松开边沿"
        );
        assert!(!input.action_is_down(&map, "ui_right"));
        assert_eq!(input.gamepad_axis(super::axis::LEFT_X as u8), 0.0);

        // 持续无设备：快照里不得残留数字态（回落判定也不会命中）。
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert_eq!(
            input.gamepad_axis_digital(super::axis::LEFT_X as u8, AxisDirection::Positive),
            Some(false)
        );
        assert!(!input.action_is_down(&map, "ui_right"));
    }

    /// M22/W3：摇杆映射走径向死区——斜向与同模长单轴同幅度，且不越界。
    #[test]
    fn axes_use_radial_stick_deadzone() {
        let diagonal = axes_from_raw(0.6, 0.6, 0.0, 0.0, 0.0, 0.0);
        let single = axes_from_raw(0.849, 0.0, 0.0, 0.0, 0.0, 0.0);
        let diagonal_magnitude =
            (diagonal[super::axis::LEFT_X].powi(2) + diagonal[super::axis::LEFT_Y].powi(2)).sqrt();
        let delta = (diagonal_magnitude - single[super::axis::LEFT_X]).abs();
        assert!(delta < 2e-3, "斜向模长与同模长单轴差 {delta}，应一致");

        // 斜向满量程归一化到单位圆内（不因两轴各自 1.0 而越界）。
        let extremes = axes_from_raw(1.0, 1.0, -1.0, -1.0, 0.0, 0.0);
        let magnitude =
            (extremes[super::axis::LEFT_X].powi(2) + extremes[super::axis::LEFT_Y].powi(2)).sqrt();
        assert!(magnitude <= 1.0 + 1e-6, "模长 {magnitude} 不得越界");
        // 死区内的小位移（含斜向）归零。
        let drift = axes_from_raw(0.1, 0.1, -0.1, -0.1, 0.0, 0.0);
        assert_eq!(drift[super::axis::LEFT_X], 0.0);
        assert_eq!(drift[super::axis::RIGHT_X], 0.0);
    }

    /// M22/W2：活动槽位跟随**真正有输入**的设备。
    ///
    /// 场景：0 号槽位挂着一个「已连接但从不产生输入」的占位/虚拟设备（XInput 下真实存在），
    /// 玩家真正用的是 1 号。旧规则「第一个已连接」会一直用 0 号 → 表现为「手柄按了没反应」。
    /// 现在一旦某个槽位出现输入，就切到它（并释放旧槽位的按住状态）。
    #[test]
    fn active_slot_follows_the_device_that_shows_input() {
        let map = ActionMap::with_defaults();
        let idle = GamepadFrame::idle();
        let mut slot1_a = GamepadFrame::idle();
        slot1_a.buttons[button::A] = true;

        let frames: Vec<[Option<GamepadFrame>; super::GAMEPAD_SLOT_COUNT]> = vec![
            // ① 两个槽位都「已连接」，都没有输入。
            [Some(idle), Some(idle), None, None],
            // ② 1 号出现输入（按 A）。
            [Some(idle), Some(slot1_a), None, None],
        ];
        let mut poller = GamepadPoller::new(Box::new(super::SlotBackend::new(frames)));

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert_eq!(poller.active_slot(), Some(0), "无输入时取第一个已连接槽位");
        assert!(!poller.slot_has_input(0));

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert_eq!(
            poller.active_slot(),
            Some(1),
            "有输入的槽位应接管（避开占位设备）"
        );
        assert!(poller.slot_has_input(1));
        assert!(
            input.action_just_pressed(&map, "confirm"),
            "接管后 1 号的 A 应直接生效"
        );
    }

    /// M22/W2：槽位切换释放旧槽位状态（不产生假动作/粘键），新槽位立即可用。
    #[test]
    fn slot_switch_releases_old_slot_state_and_adopts_new_device() {
        let map = ActionMap::with_defaults();
        let idle = GamepadFrame::idle();
        let mut held_a = GamepadFrame::idle();
        held_a.buttons[button::A] = true;
        let mut held_b = GamepadFrame::idle();
        held_b.buttons[button::B] = true;

        // ① slot 0 已连接但空闲；② slot 0 按住 A；③ slot 0 拔线、slot 1 接入（空闲）；
        // ④ slot 1 按住 B。
        let frames: Vec<[Option<GamepadFrame>; super::GAMEPAD_SLOT_COUNT]> = vec![
            [Some(idle), None, None, None],
            [Some(held_a), None, None, None],
            [None, Some(idle), None, None],
            [None, Some(held_b), None, None],
        ];
        let mut poller = GamepadPoller::new(Box::new(super::SlotBackend::new(frames)));

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert_eq!(poller.active_slot(), Some(0));
        assert!(!input.action_is_down(&map, "confirm"));

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(
            input.action_just_pressed(&map, "confirm"),
            "slot 0 的 A 应产生按下"
        );

        // 切换设备：旧槽位仍按住的 A 必须被释放，且不得出现「新设备按下」的假动作。
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert_eq!(poller.active_slot(), Some(1), "应接管已连接的 slot 1");
        assert!(
            input.action_just_released(&map, "confirm"),
            "切换槽位应释放旧槽位的按住状态"
        );
        assert!(!input.action_is_down(&map, "confirm"));
        assert!(
            !input.action_just_pressed(&map, "cancel"),
            "不得凭空产生新按下"
        );

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert!(
            input.action_just_pressed(&map, "cancel"),
            "slot 1 的 B 应生效"
        );
    }

    /// M22/W2：活动槽位是**粘性**的（连着的设备不会被后来接入的低编号槽位抢走），
    /// 显式指定优先于自动选择。
    #[test]
    fn active_slot_is_sticky_and_explicit_selection_wins() {
        let idle = GamepadFrame::idle();
        // ① 只有 slot 1；② slot 0 也接入（不应抢走）；③ 只有 slot 0（slot 1 拔线 → 接管）。
        let frames: Vec<[Option<GamepadFrame>; super::GAMEPAD_SLOT_COUNT]> = vec![
            [None, Some(idle), None, None],
            [Some(idle), Some(idle), None, None],
            [Some(idle), None, None, None],
        ];
        let mut poller = GamepadPoller::new(Box::new(super::SlotBackend::new(frames)));

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert_eq!(poller.active_slot(), Some(1), "自动选择第一个已连接槽位");

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert_eq!(
            poller.active_slot(),
            Some(1),
            "粘住当前设备，不被 slot 0 抢走"
        );

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert_eq!(
            poller.active_slot(),
            Some(0),
            "原槽位消失后接管仍连接的槽位"
        );

        // 显式指定：优先使用的槽位连上后立即生效。
        let mut poller = GamepadPoller::new(Box::new(super::SlotBackend::new(vec![[
            Some(idle),
            Some(idle),
            None,
            None,
        ]])));
        poller.set_active_slot(Some(1));
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert_eq!(poller.active_slot(), Some(1), "显式槽位优先");
        assert_eq!(poller.backend_name(), "slots");
    }

    /// M22/W2：震动设置强度并在到期后归零（`duration_ms = 0` 立即停止）。
    #[test]
    fn rumble_sets_levels_then_expires() {
        let idle = GamepadFrame::idle();
        let backend = super::SlotBackend::new(vec![[Some(idle), None, None, None]]);
        let log = backend.rumble_log();
        let mut poller = GamepadPoller::new(Box::new(backend));

        // 尚无设备：静默忽略（不 panic、不记录）。
        poller.rumble(1.0, 1.0, 100);
        assert!(log.calls().is_empty(), "无设备时不应调用后端");

        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        poller.rumble(0.5, 0.25, 1000);
        assert_eq!(log.calls().last().copied(), Some((0, 0.5, 0.25)));

        // 立即停止。
        poller.rumble(0.5, 0.5, 0);
        assert_eq!(log.calls().last().copied(), Some((0, 0.0, 0.0)));

        // 到期归零：1ms 后下一帧采样时归零。
        poller.rumble(0.8, 0.8, 1);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut input = InputSnapshot::default();
        poller.poll_into(&mut input);
        assert_eq!(
            log.calls().last().copied(),
            Some((0, 0.0, 0.0)),
            "到期应由 poll_into 归零"
        );
    }
}
