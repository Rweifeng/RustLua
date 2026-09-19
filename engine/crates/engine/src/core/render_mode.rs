//! docs/54 §4.3：**三种显式 2D 显示模式**（`World2D` / `Legacy2DOverlay` / `ScreenUI`）。
//!
//! §4.3 要求 2D 内容有三种显式模式，而不是"由宿主选择调用哪条产出路径"：
//!
//! - `World2D`：走世界相机、可选参与深度（`mode=world`）；
//! - `Legacy2DOverlay`：旧项目的默认模式，屏幕空间绘制、保持旧排序与像素行为（`mode=overlay`）；
//! - `ScreenUI`：屏幕空间布局，不受世界相机与世界 Transform 影响（`mode=screen`）。
//!
//! 此前"这段几何走哪条路"是**全局开关**（`RuntimeScene::set_world_sprite_depth`）决定的：整帧要么
//! 全部走世界通道、要么全部走 NDC overlay，同一个场景里**没法混**——"3D 场景里挂一个不受相机
//! 影响的 HUD 精灵"只能靠另开一条 UI 通道或干脆不做。本组件把这条选择下放到实体：
//!
//! ```text
//! entity  7  HudIcon
//! component  7  RenderMode  mode=screen
//! ```
//!
//! 语义边界（刻意的几条）：
//!
//! - **不声明 = 跟随全局开关**，因此既有场景一字不改、像素不变（这是回滚档）。
//! - `mode=overlay` / `mode=screen` 是**退出世界通道**：即使这一帧走了世界通道，它们也留在屏幕
//!   空间（HUD、血条、调试浮层）。
//! - `mode=world` 是**请求世界通道**：只要场景里有实体这样声明，这一帧就走世界通道（与宿主开关
//!   打开同一效果）。因此"默认实体也跟着进世界通道"——通道是帧级的，不是逐实体的；这条写进文档，
//!   避免"我只给一个实体开了 world，怎么别的也变了"这种意外。
//! - 三种模式都只影响**产出路径**，不改变排序/深度/可见遮罩的既有规则（那些仍由 `SpriteSorting`、
//!   `z` 与 `Visibility` 表达）。

use crate::core::layer::RenderLayer;

/// 一个实体的 2D 显示模式。
///
/// `Default` 取 [`Self::Overlay`]：未声明时运行时不插入这个组件，这个默认值只用于编辑器/工具的
/// 初值，落在"旧行为"那一档。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum RenderMode {
    /// 世界坐标 + 世界相机（可选深度）。
    World,
    /// 旧 2D overlay：屏幕空间、保持旧排序与像素行为。
    #[default]
    Overlay,
    /// 屏幕空间 UI：不受世界相机与世界 Transform 影响。
    Screen,
}

impl RenderMode {
    /// 稳定短名（组件串与诊断共用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::Overlay => "overlay",
            Self::Screen => "screen",
        }
    }

    /// 组件串 → 模式。未知取值**报错**：拼错 `mode=wrold` 被静默当成 overlay，会让"我明明写了
    /// world 却没进世界通道"变成没有线索的问题。
    pub fn from_component_str(value: &str) -> Result<Self, RenderModeError> {
        let mut mode: Option<Self> = None;
        let mut seen = false;
        for field in value
            .trim()
            .split(';')
            .filter(|field| !field.trim().is_empty())
        {
            seen = true;
            let (key, raw) = field
                .split_once('=')
                .map(|(key, raw)| (key.trim(), raw.trim()))
                .ok_or_else(|| RenderModeError::MalformedField(field.to_owned()))?;
            match key {
                "mode" => {
                    mode = Some(match raw {
                        "world" => Self::World,
                        "overlay" => Self::Overlay,
                        "screen" => Self::Screen,
                        other => return Err(RenderModeError::UnknownMode(other.to_owned())),
                    });
                }
                other => return Err(RenderModeError::UnknownField(other.to_owned())),
            }
        }
        if !seen {
            return Err(RenderModeError::Empty);
        }
        mode.ok_or(RenderModeError::MissingField("mode"))
    }

    /// 模式 → 规范组件串。
    #[must_use]
    pub fn to_component_str(self) -> String {
        format!("mode={}", self.as_str())
    }

    /// 这条模式是否**退出世界通道**（屏幕空间）。
    #[must_use]
    pub const fn is_screen_space(self) -> bool {
        matches!(self, Self::Overlay | Self::Screen)
    }

    /// 在 2D overlay 通道里应该落进哪个 `RenderLayer` 桶。
    ///
    /// `World` 不在 overlay 通道里绘制（`None`）：它去世界通道，桶由世界 Sprite 分段自己决定。
    #[must_use]
    pub const fn overlay_layer(self) -> Option<RenderLayer> {
        match self {
            Self::World => None,
            Self::Overlay => Some(RenderLayer::Overlay2D),
            Self::Screen => Some(RenderLayer::ScreenUI),
        }
    }
}

/// `RenderMode` 组件串的解析错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderModeError {
    Empty,
    MissingField(&'static str),
    MalformedField(String),
    UnknownField(String),
    UnknownMode(String),
}

impl std::fmt::Display for RenderModeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("render mode component is empty"),
            Self::MissingField(field) => write!(formatter, "render mode is missing field {field}"),
            Self::MalformedField(field) => {
                write!(formatter, "malformed render mode field: {field}")
            }
            Self::UnknownField(field) => write!(formatter, "unknown render mode field: {field}"),
            Self::UnknownMode(value) => write!(
                formatter,
                "unknown render mode: {value}（应为 world / overlay / screen）"
            ),
        }
    }
}

impl std::error::Error for RenderModeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_mode_and_round_trips() {
        for mode in [RenderMode::World, RenderMode::Overlay, RenderMode::Screen] {
            let encoded = mode.to_component_str();
            assert_eq!(RenderMode::from_component_str(&encoded).unwrap(), mode);
        }
        assert_eq!(
            RenderMode::from_component_str("mode=screen").unwrap(),
            RenderMode::Screen
        );
        assert!(!RenderMode::World.is_screen_space());
        assert!(RenderMode::Overlay.is_screen_space());
        assert!(RenderMode::Screen.is_screen_space());
        assert_eq!(RenderMode::World.overlay_layer(), None);
        assert_eq!(
            RenderMode::Overlay.overlay_layer(),
            Some(RenderLayer::Overlay2D)
        );
        assert_eq!(
            RenderMode::Screen.overlay_layer(),
            Some(RenderLayer::ScreenUI)
        );
    }

    /// 拼错必须报错：静默当成 overlay 会让"我写了 world 却没进世界通道"无从查起。
    #[test]
    fn rejects_malformed_components() {
        for raw in ["", "mode", "mode=", "mode=wrold", "mdoe=world"] {
            assert!(
                RenderMode::from_component_str(raw).is_err(),
                "{raw:?} 应当被拒绝"
            );
        }
        assert_eq!(
            RenderMode::from_component_str("mode=wrold"),
            Err(RenderModeError::UnknownMode("wrold".to_owned()))
        );
        assert_eq!(
            RenderMode::from_component_str("mode=world;extra=1"),
            Err(RenderModeError::UnknownField("extra".to_owned()))
        );
    }
}
