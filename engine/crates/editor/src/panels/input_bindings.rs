//! 「项目设置 → 输入」小节的**纯逻辑**（M20-08）：动作列表、捕获式改绑、
//! 冲突诊断、恢复默认、死区往返。
//!
//! 与 `project_settings.rs` 的分工：本模块不绘制控件，只维护缓冲状态与读写
//! 规则，因此可以脱离 GPU/egui 上下文做单测（`cargo test -p editor` 覆盖改绑、
//! 冲突、恢复默认、死区往返）。
//!
//! 语义要点：
//! * **改绑 = 覆盖**该动作的全部旧绑定（与引擎 `input.rebind` 一致，单一主绑定）；
//! * **捕获模式**：面板进入捕获后只接受*新*的按下/推动边沿（避免按住不放立即
//!   改绑），摇杆/扳机要越过 [`AXIS_CAPTURE_THRESHOLD`] 才算一次输入（去抖），
//!   `Esc` 保留为「取消捕获」（要绑 Esc 请用「手动绑定」文本框）；
//! * **冲突**：同一物理输入绑到多个动作时给出可读提示，并且**阻止保存**
//!   （沿用引擎 [`ActionConflict`] / `ActionMapError::DuplicateBinding`）；
//! * **保存**沿用引擎 `upsert_input_settings` 的保序读改写：`[audio]` 段、注释、
//!   未知键与未知段原样保留。

use crate::util::strip_verbatim_prefix;
use eframe::egui;
use engine::host::gamepad::{
    GAMEPAD_AXIS_COUNT, GAMEPAD_BUTTON_COUNT, GamepadDeadzone, GamepadFrame, GamepadPoller,
};
use rustlua_gameplay::settings::{
    ProjectSettings, load_deadzone_section, load_input_section, save_input_settings,
};
use rustlua_input::input::{ActionConflict, ActionMap, AxisDirection, Binding};
use std::path::Path;

/// 轴捕获阈值：明显高于设备死区（0.15）与动作判定阈值（0.5），使摇杆回中漂移
/// 不会触发改绑（「一次性去抖」）。
pub(crate) const AXIS_CAPTURE_THRESHOLD: f32 = 0.75;

/// 手柄捕获去抖器：只在「上一帧未按下 / 未越过阈值」时产出一次候选绑定。
///
/// 第一次观察只做基线（`primed`），因此**捕获开始前就按住**的键不会被立即绑定，
/// 玩家必须松开再按一次——这就是捕获模式的「明确确认动作」。
#[derive(Debug, Default)]
pub(crate) struct GamepadCaptureTracker {
    last: GamepadFrame,
    primed: bool,
}

impl GamepadCaptureTracker {
    /// 清空基线（进入新一轮捕获时调用）。
    pub(crate) fn reset(&mut self) {
        self.last = GamepadFrame::default();
        self.primed = false;
    }

    /// 观察一帧设备状态；命中新的按下/推动边沿时返回候选绑定。
    ///
    /// 断连帧（拔线/无设备）**不产出候选**，并把基线复位为「什么都没按」：
    /// 这样重连后按住的键算一次新输入，而拔线瞬间残留的按下不会误触发。
    pub(crate) fn observe(&mut self, frame: &GamepadFrame) -> Option<Binding> {
        if !frame.connected {
            self.last = GamepadFrame::default();
            self.primed = true;
            return None;
        }
        if !self.primed {
            self.last = *frame;
            self.primed = true;
            return None;
        }
        let candidate = self.candidate(frame);
        self.last = *frame;
        candidate
    }

    fn candidate(&self, frame: &GamepadFrame) -> Option<Binding> {
        if !frame.connected {
            return None;
        }
        for index in 0..GAMEPAD_BUTTON_COUNT {
            if frame.buttons[index] && !self.last.buttons[index] {
                return Some(Binding::GamepadButton(index as u8));
            }
        }
        for index in 0..GAMEPAD_AXIS_COUNT {
            let value = frame.axes[index];
            if value.abs() >= AXIS_CAPTURE_THRESHOLD
                && self.last.axes[index].abs() < AXIS_CAPTURE_THRESHOLD
            {
                let direction = if value > 0.0 {
                    AxisDirection::Positive
                } else {
                    AxisDirection::Negative
                };
                return Some(Binding::GamepadAxis(index as u8, direction));
            }
        }
        None
    }
}

/// `egui::Key` → 引擎 [`Binding`]（捕获键盘输入用）。
///
/// M22/W1：键身份来自**唯一**的转换表 [`crate::panels::keymap::egui_input_key`]
/// （与 Play 预览桥接同一份），因此捕获面板能绑定的键**恰好**是引擎键域里 egui
/// 表达得了的那些——字母/数字/符号/F1..F12/方向键/空格/Tab/Enter/Backspace 等。
///
/// `Esc` 返回 `None`：它留给「取消捕获」；要绑 Esc 用下方「手动绑定文本」输入
/// `key:escape`（M20-08 既有隔离顺序不变，W1 只替换键身份）。
#[must_use]
pub(crate) fn binding_from_egui_key(key: egui::Key) -> Option<Binding> {
    if key == egui::Key::Escape {
        return None;
    }
    crate::panels::keymap::egui_input_key(key).map(Binding::Key)
}

/// 手柄按键名的**抽象**通用名（M20-06 设备契约的 0..15 索引；不做 ABXY 机型
/// 图标适配，见任务 M20-08 的已知限制）。
#[must_use]
pub(crate) fn gamepad_button_name(index: u8) -> &'static str {
    match index {
        0 => "A",
        1 => "B",
        2 => "X",
        3 => "Y",
        4 => "LB",
        5 => "RB",
        6 => "LT",
        7 => "RT",
        8 => "Back",
        9 => "Start",
        10 => "L3",
        11 => "R3",
        12 => "DPad↑",
        13 => "DPad↓",
        14 => "DPad←",
        15 => "DPad→",
        _ => "未知按键",
    }
}

/// 手柄轴名（M20-06 设备契约的 0..5 索引）。
#[must_use]
pub(crate) fn gamepad_axis_name(index: u8) -> &'static str {
    match index {
        0 => "左摇杆X",
        1 => "左摇杆Y",
        2 => "右摇杆X",
        3 => "右摇杆Y",
        4 => "LT",
        5 => "RT",
        _ => "未知轴",
    }
}

/// 动作的中文名（未知动作回落到动作名本身，新增动作不必改这里也能显示）。
#[must_use]
pub(crate) fn action_label(action: &str) -> &str {
    match action {
        "move_up" => "上移",
        "move_down" => "下移",
        "move_left" => "左移",
        "move_right" => "右移",
        "interact" => "交互",
        "attack" => "普攻",
        "skill" => "武学",
        "gift" => "送礼",
        "sleep" => "睡觉",
        "practice" => "修炼/切换建筑",
        "inventory" => "背包",
        "pause" => "暂停",
        "build" => "建造模式",
        "place" => "放置/商店页切换",
        "demolish" => "拆除",
        "use" => "使用/升级",
        "quest" => "任务日志",
        "respawn" => "战败重开",
        "seed" => "选择种子",
        "volume_up" => "音量 +",
        "volume_down" => "音量 -",
        "confirm" => "确认",
        "cancel" => "取消/关闭",
        "ui_up" => "菜单上移",
        "ui_down" => "菜单下移",
        "ui_left" => "菜单左移",
        "ui_right" => "菜单右移",
        other => other,
    }
}

/// 绑定 → 界面上更易读的短标签（`key:keye` → `E`，`pad:0` → `手柄 A（0）`）。
///
/// M22/W1：键盘/鼠标标签**完全交给引擎**（[`Binding::display_label`] → 键位的
/// `KeyCode::display_label`）——旧实现拿 v1 的**字符文本**（`key:space`、`key:up`）
/// 在这里再翻一遍，既与引擎的键身份逻辑构成第二份实现，也无法表达 v2 的物理键名
/// （`key:keye` 会被 `to_ascii_uppercase` 显示成「KEYE」）。
///
/// 手柄两个分支保留编辑器自己的设备命名（[`gamepad_button_name`] /
/// [`gamepad_axis_name`] 是 M20-06 的抽象通用名，引擎侧只有索引），因此不走引擎标签。
#[must_use]
pub(crate) fn binding_label(binding: Binding) -> String {
    match binding {
        Binding::GamepadButton(index) => {
            format!("手柄 {}（{}）", gamepad_button_name(index), index)
        }
        Binding::GamepadAxis(index, direction) => {
            let sign = match direction {
                AxisDirection::Positive => "+",
                AxisDirection::Negative => "−",
            };
            format!("手柄 {}{}（轴{}）", gamepad_axis_name(index), sign, index)
        }
        Binding::Key(_) | Binding::Mouse(_) => binding.display_label(),
    }
}

/// 冲突列表 → 一行可读中文（面板提示与保存失败原因共用）。
#[must_use]
pub(crate) fn describe_conflicts(conflicts: &[ActionConflict]) -> String {
    conflicts
        .iter()
        .map(|conflict| {
            let actions = conflict
                .actions
                .iter()
                .map(|action| format!("{}（{}）", action_label(action), action))
                .collect::<Vec<_>>()
                .join("、");
            format!("{} 同时绑定到 {}", binding_label(conflict.binding), actions)
        })
        .collect::<Vec<_>>()
        .join("；")
}

/// 「项目设置 → 输入」的缓冲状态（挂在 `ProjectSettingsUi` 上，打开窗口时从
/// `<project>/project.toml` 读入，保存时写回）。
#[derive(Debug)]
pub(crate) struct InputBindingsUi {
    /// 当前编辑中的动作映射（打开面板时读入；缺失文件 = 引擎默认表）。
    pub(crate) map: ActionMap,
    /// 摇杆/扳机死区（默认 0.15 / 0.05）。
    pub(crate) deadzone: GamepadDeadzone,
    /// 正在捕获的动作名（`None` = 不在捕获模式）。
    capture: Option<String>,
    tracker: GamepadCaptureTracker,
    /// 手动绑定文本框内容（`key:e` / `pad:0` / `padaxis:1-`）。
    pub(crate) manual_binding: String,
    /// 「恢复默认」的二次确认是否处于待确认状态。
    pub(crate) confirm_reset: bool,
    /// 最近一次操作提示（加载诊断 / 改绑结果 / 冲突 / 保存结果）。
    pub(crate) message: String,
    /// 手柄设备轮询器：**只在捕获期间采样**（避免编辑器常态空转 XInput）。
    poller: GamepadPoller,
    /// 缓存的跨动作冲突（改绑/加载/恢复默认后刷新）。
    conflicts: Vec<ActionConflict>,
}

impl Default for InputBindingsUi {
    fn default() -> Self {
        Self {
            map: ActionMap::with_defaults(),
            deadzone: GamepadDeadzone::DEFAULT,
            capture: None,
            tracker: GamepadCaptureTracker::default(),
            manual_binding: String::new(),
            confirm_reset: false,
            message: String::new(),
            poller: GamepadPoller::platform_default(),
            conflicts: Vec::new(),
        }
    }
}

impl InputBindingsUi {
    /// 是否处于捕获模式（编辑器快捷键隔离用）。
    #[must_use]
    pub(crate) fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// 正在捕获的动作名。
    #[must_use]
    pub(crate) fn capture_target(&self) -> Option<&str> {
        self.capture.as_deref()
    }

    /// 缓存的冲突列表。
    #[must_use]
    pub(crate) fn conflicts(&self) -> &[ActionConflict] {
        &self.conflicts
    }

    /// 进入捕获模式（点击某一行的「捕获」按钮）。
    pub(crate) fn begin_capture(&mut self, action: &str) {
        self.capture = Some(action.to_owned());
        self.tracker.reset();
        self.message = format!(
            "正在为「{}」捕获输入：按键盘按键 / 手柄按钮，或推动摇杆、扳机；Esc 取消。",
            action_label(action)
        );
    }

    /// 退出捕获模式（Esc 或「取消」按钮）。
    pub(crate) fn cancel_capture(&mut self) {
        if self.capture.take().is_some() {
            self.message = "已取消改绑。".to_owned();
        }
    }

    /// 采样一帧手柄并返回捕获到的候选绑定（未在捕获模式时恒为 `None`）。
    ///
    /// 只有捕获期间才真的轮询设备：编辑器常态不碰手柄（不额外占用设备）。
    pub(crate) fn poll_capture(&mut self) -> Option<Binding> {
        self.capture.as_ref()?;
        let frame = self.poller.poll_frame();
        self.tracker.observe(&frame)
    }

    /// 替换手柄轮询器（headless 单测注入 `FakeBackend`；编辑器用平台默认）。
    #[cfg(test)]
    pub(crate) fn set_poller(&mut self, poller: GamepadPoller) {
        self.poller = poller;
    }

    /// 当前是否检测到手柄设备（面板状态提示用）。
    #[must_use]
    pub(crate) fn gamepad_connected(&self) -> bool {
        self.poller.connected()
    }

    /// 设备后端名（诊断；headless 注入 Fake 时为 `fake`）。
    #[must_use]
    pub(crate) fn backend_name(&self) -> &'static str {
        self.poller.backend_name()
    }

    /// 从 `<project>/project.toml` 读入映射与死区；返回需要提示的诊断（可空）。
    ///
    /// 语义沿用引擎：文件缺失 → 默认表 / 默认死区（无提示）；`[input]` 段缺失 →
    /// 默认表且**没有** `map` 键提示（首次运行）；版本/绑定非法 → 回退默认并给出
    /// 可定位诊断。读取失败也走诊断而不是 panic。
    pub(crate) fn load(&mut self, root: &Path) -> Option<String> {
        let root = strip_verbatim_prefix(root);
        let (map, map_diagnostic) = load_input_section(&root);
        let (deadzone, deadzone_diagnostic) = load_deadzone_section(&root);
        self.map = map;
        self.deadzone = deadzone;
        self.capture = None;
        self.confirm_reset = false;
        self.refresh_conflicts();
        // `[input]` 段整体缺失时的 `missing_key` 不是问题（首次运行/尚未保存过），
        // 不打扰用户；只有内容非法/读取失败才提示。
        let map_diagnostic =
            map_diagnostic.filter(|reason| !reason.contains("missing_key: input.map"));
        match (map_diagnostic, deadzone_diagnostic) {
            (None, None) => {
                self.message.clear();
                None
            }
            (Some(map), None) => {
                self.message = format!("输入设置已回退默认：{map}");
                Some(self.message.clone())
            }
            (None, Some(deadzone)) => {
                self.message = format!("手柄死区读取失败，使用默认值：{deadzone}");
                Some(self.message.clone())
            }
            (Some(map), Some(deadzone)) => {
                self.message = format!("输入设置已回退默认：{map}；手柄死区：{deadzone}");
                Some(self.message.clone())
            }
        }
    }

    /// 保存映射与死区到 `<project>/project.toml`（保序写回）。
    ///
    /// **保存前校验**：存在跨动作冲突时拒绝写入并返回可读原因（不静默落盘坏配置）；
    /// 写盘失败同样返回原因。
    pub(crate) fn save(&self, root: &Path) -> Result<(), String> {
        let root = strip_verbatim_prefix(root);
        if let Err(error) = self.map.validate_bindings() {
            return Err(format!(
                "输入绑定存在冲突，未写入设置文件：{}（{}）",
                describe_conflicts(&self.conflicts),
                error
            ));
        }
        let deadzone = GamepadDeadzone::new(self.deadzone.stick, self.deadzone.trigger);
        save_input_settings(&root, &self.map, deadzone)
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "保存 {} 失败：{error}",
                    ProjectSettings::path_in(&root).display()
                )
            })
    }

    /// 把 `binding` 绑到 `action`（**覆盖**旧绑定，与 `input.rebind` 同语义）。
    ///
    /// 允许产生跨动作重复（先写入、后提示冲突），这样玩家可以先改后清；真正的
    /// 拦截点在 [`Self::save`]。
    pub(crate) fn apply_binding_to(
        &mut self,
        action: &str,
        binding: Binding,
    ) -> Result<(), String> {
        if action.trim().is_empty() {
            return Err("动作名不能为空".to_owned());
        }
        let previous = self.map.bindings(action).to_vec();
        self.map.clear(action);
        if let Err(error) = self.map.bind(action, binding) {
            // 回滚：任何失败都不能让动作变成「无绑定」。
            self.map.clear(action);
            for binding in previous {
                let _ = self.map.bind(action, binding);
            }
            return Err(format!("绑定「{}」失败：{error}", binding_label(binding)));
        }
        self.refresh_conflicts();
        self.message = format!(
            "已把「{}」改绑到 {}。",
            action_label(action),
            binding_label(binding)
        );
        if !self.conflicts.is_empty() {
            self.message
                .push_str(&format!(" 冲突：{}", describe_conflicts(&self.conflicts)));
        }
        Ok(())
    }

    /// 面板「捕获」按钮的落点：把捕获到的绑定应用到当前捕获目标。
    pub(crate) fn apply_capture(&mut self, binding: Binding) -> Result<(), String> {
        let Some(action) = self.capture.clone() else {
            return Err("当前没有正在捕获的动作".to_owned());
        };
        self.apply_binding_to(&action, binding)?;
        // 保留改绑成功的提示：只结束捕获，不覆盖 message。
        self.capture = None;
        self.tracker.reset();
        Ok(())
    }

    /// 解析并应用「手动绑定」文本框（`key:e` / `pad:0` / `padaxis:1-`）。
    pub(crate) fn apply_manual_to(&mut self, action: &str) -> Result<(), String> {
        let text = self.manual_binding.trim().to_owned();
        let binding = Binding::parse(&text).map_err(|error| {
            format!(
                "绑定文本「{text}」无法解析（示例：key:e、key:space、pad:0、padaxis:1-）：{error}"
            )
        })?;
        self.apply_binding_to(action, binding)
    }

    /// 清空某个动作的全部绑定（玩家留空的按键就是「无绑定」）。
    pub(crate) fn clear_action(&mut self, action: &str) {
        self.map.clear(action);
        self.refresh_conflicts();
        self.message = format!("已清空「{}」的绑定。", action_label(action));
    }

    /// 「恢复默认」= [`ActionMap::with_defaults`]（调用方负责二次确认）。
    pub(crate) fn reset_defaults(&mut self) {
        self.map = ActionMap::with_defaults();
        self.confirm_reset = false;
        self.capture = None;
        self.refresh_conflicts();
        self.message = "已恢复默认绑定（尚未保存到设置文件）。".to_owned();
    }

    /// 重新计算并缓存冲突列表。
    pub(crate) fn refresh_conflicts(&mut self) {
        self.conflicts = self.map.conflicts();
    }

    /// 死区是否已被改成非默认值（面板提示用）。
    #[must_use]
    pub(crate) fn deadzone_changed(&self) -> bool {
        !GamepadDeadzone::new(self.deadzone.stick, self.deadzone.trigger).is_default()
    }

    /// 写入死区缓冲（越界/非有限值由 [`GamepadDeadzone::new`] 钳制）。
    pub(crate) fn set_deadzone(&mut self, stick: f32, trigger: f32) {
        self.deadzone = GamepadDeadzone::new(stick, trigger);
    }
}

/// 输入设置文件路径（面板标题行显示用）。
#[must_use]
pub(crate) fn settings_path(root: &Path) -> String {
    let root = strip_verbatim_prefix(root);
    ProjectSettings::path_in(&root)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::host::gamepad::{FakeBackend, axis, button};
    use rustlua_input::input::{InputSnapshot, KeyCode, MouseButton};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_project(tag: &str) -> std::path::PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustlua-editor-input-{tag}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn frame_with_button(index: usize) -> GamepadFrame {
        let mut frame = GamepadFrame::idle();
        frame.buttons[index] = true;
        frame
    }

    #[test]
    fn egui_keys_map_to_engine_identities() {
        // 字母/数字与游戏窗口、Play 预览同源（W/A/S/D 是专用变体）。
        assert_eq!(
            binding_from_egui_key(egui::Key::E),
            Some(Binding::Key(KeyCode::KeyE))
        );
        assert_eq!(
            binding_from_egui_key(egui::Key::W),
            Some(Binding::Key(KeyCode::KeyW))
        );
        assert_eq!(
            binding_from_egui_key(egui::Key::Num3),
            Some(Binding::Key(
                KeyCode::from_char('3').expect("QWERTY 字符必须有对应物理键")
            ))
        );
        assert_eq!(
            binding_from_egui_key(egui::Key::Space),
            Some(Binding::Key(KeyCode::Space))
        );
        assert_eq!(
            binding_from_egui_key(egui::Key::ArrowUp),
            Some(Binding::Key(KeyCode::ArrowUp))
        );
        assert_eq!(
            binding_from_egui_key(egui::Key::Minus),
            Some(Binding::Key(
                KeyCode::from_char('-').expect("QWERTY 字符必须有对应物理键")
            ))
        );
        assert_eq!(
            binding_from_egui_key(egui::Key::Plus),
            Some(Binding::Key(
                KeyCode::from_char('+').expect("QWERTY 字符必须有对应物理键")
            ))
        );
        // 符号键（egui 的 `name()` 是符号名）也要能捕获，并翻成**物理键**身份。
        // `Colon` 就是 `;` 那个键的带 Shift 写法 → 落回 `Semicolon`（同一个物理键，
        // 不能因为玩家按住 Shift 就变成另一个可绑定的键）。
        assert_eq!(
            crate::panels::keymap::egui_input_key(egui::Key::Quote),
            Some(KeyCode::Quote)
        );
        assert_eq!(
            crate::panels::keymap::egui_input_key(egui::Key::Backslash),
            Some(KeyCode::Backslash)
        );
        assert_eq!(
            crate::panels::keymap::egui_input_key(egui::Key::Colon),
            Some(KeyCode::Semicolon)
        );
        // M22/W1 改进：Tab/Enter/Backspace/F5/Delete 现在**在引擎键域内**（旧
        // `InputKey` 按字符编码，这些键根本无法表达），因此捕获面板能绑定它们。
        // 只有 Esc 例外——它留给「取消捕获」。
        assert_eq!(binding_from_egui_key(egui::Key::Escape), None);
        assert_eq!(
            binding_from_egui_key(egui::Key::Tab),
            Some(Binding::Key(KeyCode::Tab))
        );
        assert_eq!(
            binding_from_egui_key(egui::Key::Enter),
            Some(Binding::Key(KeyCode::Enter))
        );
        assert_eq!(
            binding_from_egui_key(egui::Key::Backspace),
            Some(Binding::Key(KeyCode::Backspace))
        );
        assert_eq!(
            binding_from_egui_key(egui::Key::F5),
            Some(Binding::Key(KeyCode::F5))
        );
        assert_eq!(
            binding_from_egui_key(egui::Key::Delete),
            Some(Binding::Key(KeyCode::Delete))
        );
        // 捕获结果的**落盘文本**一律是 v2 物理键名（字母/数字带前缀、符号用变体名，
        // `+` 与 `=` 收敄为同一个 `key:equal`）。
        for (egui_key, expected) in [
            (egui::Key::E, "key:keye"),
            (egui::Key::Num3, "key:digit3"),
            (egui::Key::Space, "key:space"),
            (egui::Key::ArrowUp, "key:arrowup"),
            (egui::Key::Minus, "key:minus"),
            (egui::Key::Plus, "key:equal"),
            (egui::Key::Quote, "key:quote"),
            (egui::Key::Backslash, "key:backslash"),
            (egui::Key::Colon, "key:semicolon"),
        ] {
            let binding = binding_from_egui_key(egui_key).expect("上述键都必须可捕获");
            assert_eq!(binding.to_text(), expected, "{egui_key:?} 的稳定文本");
        }
    }

    #[test]
    fn capture_tracker_requires_edge_and_rejects_stick_drift() {
        let mut tracker = GamepadCaptureTracker::default();
        // 第一帧只建立基线：捕获开始前就按住的键不会立刻改绑。
        assert_eq!(
            tracker.observe(&frame_with_button(button::A)),
            None,
            "按住不放不得立即捕获"
        );
        assert_eq!(tracker.observe(&GamepadFrame::idle()), None);
        assert_eq!(
            tracker.observe(&frame_with_button(button::A)),
            Some(Binding::GamepadButton(0)),
            "松开后重新按下 = 一次捕获"
        );
        // 持续按住不重复触发（边沿语义）。
        assert_eq!(tracker.observe(&frame_with_button(button::A)), None);
        // 摇杆漂移（远低于阈值）不算输入；越过阈值才产出带方向的轴绑定。
        let mut tracker = GamepadCaptureTracker::default();
        let mut drift = GamepadFrame::idle();
        drift.axes[axis::LEFT_Y] = 0.3;
        assert_eq!(tracker.observe(&drift), None);
        assert_eq!(tracker.observe(&drift), None, "漂移持续存在也不触发");
        let mut push_up = GamepadFrame::idle();
        push_up.axes[axis::LEFT_Y] = 0.9;
        assert_eq!(
            tracker.observe(&push_up),
            Some(Binding::GamepadAxis(1, AxisDirection::Positive)),
            "上推 = padaxis:1+（上为正）"
        );
        // 回中一帧后才算新的边沿（推着不放不会连续改绑）。
        assert_eq!(tracker.observe(&GamepadFrame::idle()), None);
        let mut push_down = GamepadFrame::idle();
        push_down.axes[axis::LEFT_Y] = -0.9;
        assert_eq!(
            tracker.observe(&push_down),
            Some(Binding::GamepadAxis(1, AxisDirection::Negative))
        );
        // 扳机同理（轴 4/5）。
        let mut trigger = GamepadFrame::idle();
        trigger.axes[axis::LEFT_TRIGGER] = 1.0;
        assert_eq!(
            tracker.observe(&trigger),
            Some(Binding::GamepadAxis(4, AxisDirection::Positive))
        );
        // 断连帧不产出候选（拔线瞬间的残留按下被忽略）；重连后重新按下才算一次。
        let mut tracker = GamepadCaptureTracker::default();
        assert_eq!(tracker.observe(&GamepadFrame::idle()), None);
        let mut disconnected_held = GamepadFrame::disconnected();
        disconnected_held.buttons[button::B] = true;
        assert_eq!(
            tracker.observe(&disconnected_held),
            None,
            "断连帧不产出候选"
        );
        assert_eq!(
            tracker.observe(&frame_with_button(button::B)),
            Some(Binding::GamepadButton(1)),
            "重连后按下 = 一次新输入"
        );
    }

    #[test]
    fn capture_flow_binds_fake_device_without_touching_other_actions() {
        let mut ui = InputBindingsUi::default();
        ui.set_poller(GamepadPoller::new(Box::new(FakeBackend::new(vec![
            GamepadFrame::idle(),
            frame_with_button(button::X),
        ]))));
        assert_eq!(ui.backend_name(), "fake");
        // 未进入捕获模式 → 不轮询、不产出。
        assert_eq!(ui.poll_capture(), None);
        ui.begin_capture("interact");
        assert!(ui.is_capturing());
        assert_eq!(ui.capture_target(), Some("interact"));
        assert_eq!(ui.poll_capture(), None, "第一帧只建立基线");
        let binding = ui.poll_capture().expect("第二帧捕获到手柄 X");
        assert_eq!(binding, Binding::GamepadButton(2));
        ui.apply_capture(binding).unwrap();
        assert!(!ui.is_capturing());
        assert_eq!(ui.map.bindings("interact"), [Binding::GamepadButton(2)]);
        // 其它动作用默认绑定补齐（只覆盖捕获目标）。
        assert_eq!(ui.map.bindings("move_up"), [Binding::Key(KeyCode::KeyW)]);
        assert!(ui.message.contains("交互"), "{}", ui.message);
        // 取消捕获不改绑定。
        ui.begin_capture("interact");
        assert!(ui.is_capturing());
        ui.cancel_capture();
        assert!(!ui.is_capturing());
        assert_eq!(ui.map.bindings("interact"), [Binding::GamepadButton(2)]);
        assert!(ui.message.contains("取消"), "{}", ui.message);
    }

    #[test]
    fn rebind_replaces_old_binding_and_engine_queries_follow_it() {
        let mut ui = InputBindingsUi::default();
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyE, true);
        assert!(input.action_is_down(&ui.map, "interact"));
        // 改绑到 F：旧键失效、新键生效（复用引擎动作查询，证明「改绑真的生效」）。
        ui.apply_binding_to("interact", Binding::Key(KeyCode::KeyF))
            .unwrap();
        assert!(!input.action_is_down(&ui.map, "interact"));
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyF, true);
        assert!(input.action_is_down(&ui.map, "interact"));
        assert!(input.action_just_pressed(&ui.map, "interact"));
        assert_eq!(
            ui.map.bindings("interact"),
            [Binding::Key(KeyCode::KeyF)],
            "改绑 = 覆盖旧绑定（与 input.rebind 同语义）"
        );
        // 非法绑定文本被拒绝且不改变现有映射。
        ui.manual_binding = "joystick:1".to_owned();
        assert!(ui.apply_manual_to("interact").is_err());
        assert_eq!(ui.map.bindings("interact"), [Binding::Key(KeyCode::KeyF)]);
        // 手动绑定生效。
        ui.manual_binding = "padaxis:1-".to_owned();
        ui.apply_manual_to("interact").unwrap();
        assert_eq!(
            ui.map.bindings("interact"),
            [Binding::GamepadAxis(1, AxisDirection::Negative)]
        );
        // 清空绑定 → 该动作不再响应任何输入。
        ui.clear_action("interact");
        assert!(ui.map.bindings("interact").is_empty());
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyF, true);
        assert!(!input.action_is_down(&ui.map, "interact"));
    }

    #[test]
    fn conflicts_are_reported_readably_and_block_save() {
        let root = temp_project("conflict");
        let mut ui = InputBindingsUi::default();
        // 把 attack 也绑到 E（与 interact 撞车）→ 冲突必须被列出。
        ui.apply_binding_to("attack", Binding::Key(KeyCode::KeyE))
            .unwrap();
        assert_eq!(ui.conflicts().len(), 1);
        assert_eq!(ui.conflicts()[0].actions, vec!["attack", "interact"]);
        let describe = describe_conflicts(ui.conflicts());
        assert!(
            describe.contains("普攻") && describe.contains("交互"),
            "{describe}"
        );
        assert!(describe.contains("E"), "{describe}");
        // 保存被拒绝，且设置文件不产生残留。
        let error = ui.save(&root).unwrap_err();
        assert!(error.contains("冲突"), "{error}");
        assert!(
            error.contains("attack") || error.contains("普攻"),
            "{error}"
        );
        assert!(!root.join("project.toml").exists(), "校验失败不得落盘");
        // 解绑冲突项后保存成功。
        ui.apply_binding_to(
            "attack",
            Binding::Key(KeyCode::from_char('j').expect("QWERTY 字符必须有对应物理键")),
        )
        .unwrap();
        assert!(ui.conflicts().is_empty());
        ui.save(&root).unwrap();
        let text = std::fs::read_to_string(root.join("project.toml")).unwrap();
        // v2 文本：键位是**物理键名**而不是字符（v1 的 `key:j` 已不再写出）。
        assert!(text.contains("attack=key:keyj"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reset_defaults_restores_engine_default_map() {
        let mut ui = InputBindingsUi::default();
        ui.apply_binding_to("interact", Binding::Key(KeyCode::KeyF))
            .unwrap();
        ui.set_deadzone(0.4, 0.3);
        ui.clear_action("pause");
        assert_ne!(ui.map, ActionMap::with_defaults());
        // 二次确认语义：进入待确认状态并不改数据。
        ui.confirm_reset = true;
        assert_ne!(ui.map, ActionMap::with_defaults());
        ui.reset_defaults();
        assert_eq!(ui.map, ActionMap::with_defaults());
        assert_eq!(ui.map.bindings("interact"), [Binding::Key(KeyCode::KeyE)]);
        assert_eq!(
            ui.map.bindings("pause"),
            [Binding::Key(
                KeyCode::from_char('p').expect("QWERTY 字符必须有对应物理键")
            )]
        );
        assert!(!ui.confirm_reset, "恢复默认后清除待确认状态");
        assert!(ui.conflicts().is_empty());
        // 死区**不**被恢复默认改动（它是设备参数，与绑定表分开配置）。
        assert!((ui.deadzone.stick - 0.4).abs() < 1e-6);
    }

    #[test]
    fn deadzone_and_bindings_round_trip_through_settings_file() {
        let root = temp_project("round-trip");
        std::fs::write(
            root.join("project.toml"),
            "# 玩家设置\n[audio]\nmaster = 0.5\n# 音量注释\nbgm = 0.25\n",
        )
        .unwrap();
        let mut ui = InputBindingsUi::default();
        assert_eq!(
            ui.load(&root),
            None,
            "缺失 [input] 段不是需要打扰用户的错误"
        );
        assert_eq!(ui.map, ActionMap::with_defaults());
        assert_eq!(ui.deadzone, GamepadDeadzone::DEFAULT);
        // V 未被任何默认动作占用（F 是 `place` 的默认键）。
        ui.apply_binding_to(
            "interact",
            Binding::Key(KeyCode::from_char('v').expect("QWERTY 字符必须有对应物理键")),
        )
        .unwrap();
        ui.set_deadzone(0.25, 0.08);
        ui.save(&root).unwrap();

        let text = std::fs::read_to_string(root.join("project.toml")).unwrap();
        assert!(text.contains("# 玩家设置"), "保序：注释保留\n{text}");
        assert!(text.contains("# 音量注释"), "{text}");
        assert!(
            text.contains("master = 0.5"),
            "保序：[audio] 段保留\n{text}"
        );
        // v2 文本：键位是**物理键名**而不是字符（v1 的 `key:v` 已不再写出）。
        assert!(text.contains("interact=key:keyv"), "{text}");
        assert!(text.contains("stick_deadzone = 0.25"), "{text}");
        assert!(text.contains("trigger_deadzone = 0.08"), "{text}");

        // 读回（新面板实例）→ 绑定与死区都还原。
        let mut reloaded = InputBindingsUi::default();
        assert_eq!(reloaded.load(&root), None);
        assert_eq!(
            reloaded.map.bindings("interact"),
            [Binding::Key(
                KeyCode::from_char('v').expect("QWERTY 字符必须有对应物理键")
            )]
        );
        assert!((reloaded.deadzone.stick - 0.25).abs() < 1e-6);
        assert!((reloaded.deadzone.trigger - 0.08).abs() < 1e-6);
        assert!(reloaded.deadzone_changed());
        // 幂等：再次保存不产生重复键/丢段。
        reloaded.save(&root).unwrap();
        let again = std::fs::read_to_string(root.join("project.toml")).unwrap();
        assert_eq!(text, again);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_settings_fall_back_to_defaults_with_readable_diagnostic() {
        let root = temp_project("invalid");
        std::fs::write(
            root.join("project.toml"),
            "[audio]\nmaster = 0.5\n[input]\nmap = \"rustlua-actions-v0\\ninteract=key:f\"\nstick_deadzone = 9\n",
        )
        .unwrap();
        let mut ui = InputBindingsUi::default();
        let diagnostic = ui.load(&root).expect("非法版本必须给诊断");
        assert!(diagnostic.contains("project.toml"), "{diagnostic}");
        assert!(diagnostic.contains("rustlua-actions-v0"), "{diagnostic}");
        assert_eq!(ui.map, ActionMap::with_defaults(), "回退默认映射");
        // 死区超界被钳制（0.9 上限），不会污染面板/后续保存。
        assert_eq!(ui.deadzone.stick, engine::host::gamepad::MAX_DEADZONE);
        assert!((ui.deadzone.trigger - 0.05).abs() < 1e-6);
        // 写盘失败（project.toml 是目录）→ 可读错误而不是 panic。
        let blocked = temp_project("blocked");
        std::fs::create_dir_all(blocked.join("project.toml")).unwrap();
        let error = ui.save(&blocked).unwrap_err();
        assert!(error.contains("project.toml"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&blocked);
    }

    #[test]
    fn labels_and_settings_path_are_stable() {
        assert_eq!(action_label("interact"), "交互");
        assert_eq!(action_label("slot_4"), "slot_4", "未知动作回落到动作名");
        assert_eq!(binding_label(Binding::Key(KeyCode::KeyE)), "E");
        assert_eq!(binding_label(Binding::Key(KeyCode::Space)), "空格");
        assert_eq!(binding_label(Binding::Key(KeyCode::Escape)), "Esc");
        assert_eq!(binding_label(Binding::Key(KeyCode::ArrowUp)), "↑");
        assert_eq!(binding_label(Binding::GamepadButton(0)), "手柄 A（0）");
        assert_eq!(
            binding_label(Binding::GamepadAxis(1, AxisDirection::Negative)),
            "手柄 左摇杆Y−（轴1）"
        );
        assert_eq!(gamepad_button_name(12), "DPad↑");
        assert_eq!(gamepad_axis_name(5), "RT");
        assert!(settings_path(Path::new("C:\\Games\\Demo")).ends_with("project.toml"));
    }

    /// M22/W1：绑定的**稳定文本**（进设置文件）与**展示标签**（进界面）刻意分开。
    ///
    /// 文本是物理键名（`key:keye`/`key:digit3`/`key:equal`），标签是人读短名
    /// （`E`/`1`/`=`）；旧实现把两者混成一份「字符文本 + `to_ascii_uppercase`」，
    /// 于是 v2 的 `key:keye` 会显示成「KEYE」，而 `+` 与 `=` 会变成两个不同的键。
    #[test]
    fn key_binding_text_and_label_follow_physical_key_identity() {
        for (binding, text, label) in [
            (Binding::Key(KeyCode::KeyE), "key:keye", "E"),
            (Binding::Key(KeyCode::Digit3), "key:digit3", "3"),
            (Binding::Key(KeyCode::Minus), "key:minus", "-"),
            // `+` 与 `=` 是**同一个物理键**：v1 的两个字符写法在 v2 里收敄成一项。
            (Binding::Key(KeyCode::Equal), "key:equal", "="),
            (Binding::Key(KeyCode::Quote), "key:quote", "'"),
            (Binding::Key(KeyCode::Backslash), "key:backslash", "\\"),
            (Binding::Key(KeyCode::Semicolon), "key:semicolon", ";"),
            (Binding::Key(KeyCode::ArrowUp), "key:arrowup", "↑"),
            (Binding::Key(KeyCode::Space), "key:space", "空格"),
            (Binding::Key(KeyCode::Escape), "key:escape", "Esc"),
            (
                Binding::Key(KeyCode::ShiftLeft),
                "key:shiftleft",
                "Shift 左",
            ),
            // 鼠标标签同样由引擎给（编辑器不再自己拼字符串）。
            (Binding::Mouse(MouseButton::Left), "mouse:left", "鼠标左键"),
        ] {
            assert_eq!(binding.to_text(), text, "{binding:?} 的稳定文本");
            assert_eq!(binding_label(binding), label, "{binding:?} 的展示标签");
        }
        // `Binding::parse` 同时接受 v2 物理键名与 v1 字符写法（既有 Lua 调用/旧设置文件）。
        assert_eq!(
            Binding::parse("key:keye").unwrap(),
            Binding::Key(KeyCode::KeyE)
        );
        assert_eq!(
            Binding::parse("key:e").unwrap(),
            Binding::Key(KeyCode::KeyE),
            "v1 的字符写法仍可读入"
        );
    }
}
