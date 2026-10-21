//! 输入域的宿主侧状态：Lua 绑定与宿主门面（[`crate::host::HostInput`]）共享的**唯一**一份。
//!
//! 从 `rustlua-scripting::ScriptState` 搬出来的四个字段（快照 / 动作映射 / 导航手感 /
//! 诊断）+ 两处运行期状态（连发计时、震动请求）。搬走的理由与 `audio` 同源：这些是**输入域
//! 的事实**，VM 只是它们的消费者；留在 40 字段的中枢里，改一次输入逻辑就要重编全部绑定。

use std::collections::HashMap;

use crate::input::{ActionMap, InputSnapshot};
use crate::nav::NavSettings;

/// 输入层诊断只保留最近这么多条（M20-07，最新在后）。
const DIAGNOSTIC_LIMIT: usize = 16;

/// `input.action_repeat` 的每动作状态（M22/W3）。
///
/// * `holding`：上一帧该动作是否处于按下状态（用于识别「首次按下 = 立即一步」）；
/// * `next_repeat_ms`：下一次允许连发的逻辑时间。
#[derive(Debug, Clone, Copy, Default)]
struct ActionRepeatState {
    holding: bool,
    next_repeat_ms: f64,
}

/// 输入域状态（绑定唯一持有者见 `Handle<InputState>`）。
#[derive(Debug)]
pub struct InputState {
    /// 本帧输入快照（宿主每步喂入）。
    pub snapshot: InputSnapshot,
    /// 命名动作映射（一份实例，玩家偏好）。
    pub action_map: ActionMap,
    /// 菜单导航手感（阈值 + 重复节奏）。
    pub nav: NavSettings,
    /// 输入层诊断（最新在后）。
    pub diagnostics: Vec<String>,
    /// v1→v2 改键迁移说明（有损换算逐条），与诊断分开。
    pub migration_notes: Vec<String>,
    /// 待宿主取走的震动请求（左右马达 0..1，毫秒）。
    pub rumble_request: Option<(f32, f32, u32)>,
    /// 逻辑时间（毫秒）：`set_input` 每个固定步累加一次，连发节奏按它计时。
    logical_ms: f64,
    /// 每个动作的连发状态。
    repeat: HashMap<String, ActionRepeatState>,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            snapshot: InputSnapshot::default(),
            action_map: ActionMap::with_defaults(),
            nav: NavSettings::DEFAULT,
            diagnostics: Vec::new(),
            migration_notes: Vec::new(),
            rumble_request: None,
            logical_ms: 0.0,
            repeat: HashMap::new(),
        }
    }
}

impl InputState {
    /// 逻辑时间推进（**每个固定步一次**，由 [`crate::host::HostInput::set_input`] 调用）。
    ///
    /// 为什么按固定步而不是按真实时间：catch-up 帧（一帧多步）也走同一条时间线，
    /// 连发节奏与帧率无关（M22/W3 的原始动机）。
    pub fn advance(&mut self, delta_seconds: f32) {
        self.logical_ms += f64::from(delta_seconds.max(0.0)) * 1000.0;
    }

    /// 当前逻辑时间（毫秒）。
    #[must_use]
    pub fn logical_ms(&self) -> f64 {
        self.logical_ms
    }

    /// 时间制连发（M22/W3）：[`crate::host::HostInput::action_repeat`] 与 Lua
    /// `input.action_repeat` 同源实现。
    ///
    /// 语义（逐条都是回归过的）：
    /// * 边沿 = 真实按下边沿，一步之内完成的「按下 + 松开」也走一步；
    /// * 只对「持续保持的模拟量绑定」连发（数字键/DPad 保持 = 一步）；
    /// * 以「当前时间 + 间隔」推进，而不是累加：长卡顿后不爆发式连发。
    pub fn repeat_step(&mut self, name: &str) -> bool {
        let now = self.logical_ms;
        let down = self.snapshot.action_is_down(&self.action_map, name);
        let edge = self.snapshot.action_just_pressed(&self.action_map, name);
        // 连发只对「持续保持的模拟量绑定」生效（数字键/DPad 保持一次按下 = 一步）。
        let analog = down && self.snapshot.action_held_by_analog(&self.action_map, name);
        let delay_ms = f64::from(self.nav.delay_ms);
        let repeat_ms = f64::from(self.nav.repeat_ms);
        let entry = self.repeat.entry(name.to_owned()).or_default();
        if !down {
            entry.holding = false;
            // 边沿 = 「刚按下」，与「此刻是否仍按住」无关：**一步之内完成的「按下 + 松开」**
            // 同样必须走一步——极短点按、编辑器 Play 预览把同帧内的按下/松开发成 down+up
            // 边沿（`game_view::bridge_preview_input`）、摇杆越过阈值后立刻回中都是这种形态。
            // 边沿在逻辑步内保留（与 `action_pressed` 同语义），宿主在每帧首个固定步之后
            // `finish_frame` 清边沿，因此不会跨步重复。
            return edge;
        }
        if !entry.holding {
            // 开始计时；边沿帧立即走一步，非边沿帧只武装计时器。
            entry.holding = true;
            entry.next_repeat_ms = now + delay_ms;
            return edge;
        }
        if !analog {
            return false;
        }
        if now >= entry.next_repeat_ms {
            // 以「当前时间 + 间隔」推进，而不是累加：长卡顿后不爆发式连发。
            entry.next_repeat_ms = now + repeat_ms;
            return true;
        }
        false
    }

    /// 记一条输入层诊断（只保留最近 `DIAGNOSTIC_LIMIT` 条）。
    pub fn push_diagnostic(&mut self, message: String) {
        tracing::warn!(target: "input", "{}", message);
        if self.diagnostics.len() >= DIAGNOSTIC_LIMIT {
            self.diagnostics.remove(0);
        }
        self.diagnostics.push(message);
    }

    /// 取走震动请求（宿主帧末交给设备层）。
    pub fn take_rumble_request(&mut self) -> Option<(f32, f32, u32)> {
        self.rumble_request.take()
    }
}

#[cfg(test)]
mod tests {
    use super::InputState;
    use crate::input::InputSnapshot;

    /// 造一个「某物理键按下」的快照（走真实事件入口，边沿与按住态一致）。
    fn snapshot_with(key: crate::input::KeyCode) -> InputSnapshot {
        let mut snapshot = InputSnapshot::default();
        snapshot.key_event(key, true);
        snapshot
    }

    #[test]
    fn repeat_fires_once_on_edge_then_holds_for_digital_bindings() {
        // 默认映射里 `ui_up` 首绑定是方向键（数字键：按住只走一步）。
        let key = crate::input::KeyCode::from_text("arrowup").unwrap();
        let mut state = InputState {
            snapshot: snapshot_with(key),
            ..InputState::default()
        };
        state.advance(0.016);
        assert!(state.repeat_step("ui_up"), "首次按下立即一步");
        state.advance(0.016);
        assert!(!state.repeat_step("ui_up"), "数字键保持不连发");
        state.advance(0.5);
        assert!(!state.repeat_step("ui_up"), "数字键再久也不连发");
        // 松开后重新按下：又是立即一步。
        state.snapshot = InputSnapshot::default();
        assert!(!state.repeat_step("ui_up"));
        state.snapshot = snapshot_with(key);
        assert!(state.repeat_step("ui_up"));
    }

    #[test]
    fn diagnostics_keep_only_tail() {
        let mut state = InputState::default();
        for index in 0..20 {
            state.push_diagnostic(format!("d{index}"));
        }
        assert_eq!(state.diagnostics.len(), 16);
        assert_eq!(state.diagnostics.last().unwrap(), "d19");
        assert_eq!(state.diagnostics.first().unwrap(), "d4");
    }
}
