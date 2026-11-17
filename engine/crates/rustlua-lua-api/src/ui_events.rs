//! UI 事件的中立契约类型：事件种类与命中目标。
//!
//! 这两个类型是纯数据（无领域依赖），但既被 UI 域的绑定使用，也被脚本层的点击派发与
//! 宿主的命中计算使用。住在契约层（叶子 crate）让脚本层能引用它们而不必依赖整个 `rustlua-ui`，
//! 与 [`crate::PrefabHost`] 同属"绑定需要的、与具体领域无关的东西"。

/// 结构化 UI 事件的种类。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiEventType {
    PointerEnter,
    PointerExit,
    PointerDown,
    PointerUp,
    Click,
    Focus,
    Submit,
    ValueChanged,
    Drag,
}

impl UiEventType {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "pointer_enter" => Some(Self::PointerEnter),
            "pointer_exit" => Some(Self::PointerExit),
            "pointer_down" => Some(Self::PointerDown),
            "pointer_up" => Some(Self::PointerUp),
            "click" => Some(Self::Click),
            "focus" => Some(Self::Focus),
            "submit" => Some(Self::Submit),
            "value_changed" => Some(Self::ValueChanged),
            "drag" => Some(Self::Drag),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PointerEnter => "pointer_enter",
            Self::PointerExit => "pointer_exit",
            Self::PointerDown => "pointer_down",
            Self::PointerUp => "pointer_up",
            Self::Click => "click",
            Self::Focus => "focus",
            Self::Submit => "submit",
            Self::ValueChanged => "value_changed",
            Self::Drag => "drag",
        }
    }
}

/// 命中测试后的目标，携带要派发的 Lua 回调名。
#[derive(Clone, Debug, Default)]
pub struct UiClickTarget {
    pub name: String,
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub functions: Vec<String>,
}
