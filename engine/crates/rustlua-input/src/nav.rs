//! 菜单导航手感（左摇杆推杆阈值 + 按住重复节奏）。
//!
//! 为什么住在这里而不是 `rustlua-gameplay`：它是**输入域的值类型**——`input.action_repeat`、
//! `input.nav_settings` 与引擎设备层（轴判定阈值）读的都是它。P1 阶段 3 把 `input` 的 Lua
//! 绑定下沉到本 crate 之后，设置文件（`[input]` 段）那一侧只剩"文本 ↔ 结构"的转换留在
//! `rustlua-gameplay`，结构本身必须跟着绑定走，否则两边各定义一份、手感字段迟早漂移。
//!
//! 路径兼容：`rustlua_gameplay::settings::NavSettings` 仍然可用（那边是 `pub use` 转发）。

/// 菜单导航手感（左摇杆推杆阈值 + 按住重复节奏）。
///
/// 与死区一样属于**玩家偏好**：只影响面板选中的滚动节奏，不影响玩法移动。
///
/// **M22/W3 起重复节奏用毫秒**（原 M20-09 是帧数，导致手感随刷新率变化：144Hz 的
/// 连发速度是 60Hz 的 2.4 倍）。旧的 `nav_delay_frames` / `nav_repeat_frames` 仍可读
/// （按 60Hz 换算），写入统一用 `nav_delay_ms` / `nav_repeat_ms`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavSettings {
    /// 摇杆推杆阈值（0..=1）：低于它不算导航输入（引擎轴判定阈值同源）。
    pub threshold: f32,
    /// 首次重复前的按住时长（毫秒，上限 [`Self::MAX_MS`]）。
    pub delay_ms: u32,
    /// 连发间隔（毫秒，下限 [`Self::MIN_REPEAT_MS`]，上限 [`Self::MAX_MS`]）。
    pub repeat_ms: u32,
}

impl Default for NavSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl NavSettings {
    /// 默认手感：阈值 0.5、延迟 350ms、间隔 90ms。
    ///
    /// 与 M20-09 的「16 帧 / 8 帧 @60Hz」几乎一致（267ms / 133ms → 350ms / 90ms，
    /// 首次延迟略长、连发略快），这样键盘/手柄在任意帧率下手感一致。
    pub const DEFAULT: Self = Self {
        threshold: 0.5,
        delay_ms: 350,
        repeat_ms: 90,
    };

    /// 时长上限：再长也没有实用意义，同时防脏数据把导航变成「永不重复」。
    pub const MAX_MS: u32 = 5000;

    /// 连发间隔下限（约一个 60Hz 逻辑帧，防止退化成每步连发）。
    pub const MIN_REPEAT_MS: u32 = 16;

    /// 旧「帧」键换算成毫秒所用的固定步长（60Hz，与引擎默认固定步一致）。
    pub const LEGACY_FRAME_MS: f32 = 1000.0 / 60.0;

    /// 唯一构造入口：非有限/超界值统一钳制（阈值 0..=1、延迟 ≤ 上限、间隔 ≥ 下限）。
    #[must_use]
    pub fn new(threshold: f32, delay_ms: u32, repeat_ms: u32) -> Self {
        let threshold = if threshold.is_finite() {
            threshold.clamp(0.0, 1.0)
        } else {
            Self::DEFAULT.threshold
        };
        Self {
            threshold,
            delay_ms: delay_ms.min(Self::MAX_MS),
            repeat_ms: repeat_ms.clamp(Self::MIN_REPEAT_MS, Self::MAX_MS),
        }
    }

    /// 旧帧数 → 毫秒（兼容读取用）。非有限/负值按 0 处理（与 M20-09 的
    /// `v.max(0.0) as u32` 一致），上限由 [`Self::new`] 统一钳制。
    #[must_use]
    pub fn frames_to_ms(frames: f32) -> u32 {
        (frames.max(0.0) * Self::LEGACY_FRAME_MS).round() as u32
    }

    /// 是否与默认手感一致（编辑器/诊断用）。
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::NavSettings;

    #[test]
    fn clamp_bounds_match_settings_contract() {
        // 与 `rustlua-gameplay` 的设置解析共用同一份钳制：数值越界不得让导航卡死。
        let nav = NavSettings::new(f32::NAN, 4, 1);
        assert_eq!(nav.threshold, NavSettings::DEFAULT.threshold);
        assert_eq!(nav.delay_ms, 4);
        assert_eq!(nav.repeat_ms, NavSettings::MIN_REPEAT_MS);
        assert_eq!(
            NavSettings::new(0.9, 99_999, 99_999).delay_ms,
            NavSettings::MAX_MS
        );
        assert!(!nav.is_default());
        assert!(NavSettings::DEFAULT.is_default());
    }

    #[test]
    fn legacy_frame_keys_convert_at_60hz() {
        assert_eq!(NavSettings::frames_to_ms(16.0), 267);
        assert_eq!(NavSettings::frames_to_ms(-3.0), 0);
        assert_eq!(NavSettings::frames_to_ms(f32::NAN), 0);
    }
}
