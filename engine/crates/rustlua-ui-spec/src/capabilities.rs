use crate::spec::StyleState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// 运行时能力矩阵：AI 只能生成这里声明支持的东西。
///
/// `core()` 与当前运行时逐位对齐（M76-02 落地的指针/焦点事件、四个状态覆盖、
/// 绑定与裁剪已打开；转场、富文本、虚拟列表尚未接进运行时，保持关闭）。未打开的位
/// 会被校验器拒绝，避免“编辑器能编、运行时不动”。
///
/// 可序列化：Provider 请求与录制回放都携带同一份能力矩阵，避免模型侧与校验侧
/// 对“现在支持什么”各说各话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub node_types: BTreeSet<String>,
    pub events: BTreeSet<String>,
    pub style_states: BTreeSet<StyleState>,
    pub bindings: bool,
    pub transitions: bool,
    pub clipping: bool,
    pub rich_text: bool,
    pub virtual_list: bool,
}

impl Capabilities {
    pub fn core() -> Self {
        Self {
            node_types: [
                "panel",
                "container",
                "text",
                "image",
                "button",
                "scroll_view",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            events: [
                "pointer_enter",
                "pointer_exit",
                "pointer_down",
                "pointer_up",
                "click",
                "focus",
                "submit",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            style_states: [
                StyleState::Hover,
                StyleState::Pressed,
                StyleState::Disabled,
                StyleState::Focused,
            ]
            .into_iter()
            .collect(),
            bindings: true,
            transitions: false,
            clipping: true,
            rich_text: false,
            virtual_list: false,
        }
    }

    /// 方案 v2 的目标能力面，供编译器/测试描述“完成后应支持什么”。
    pub fn planned() -> Self {
        Self {
            node_types: [
                "panel",
                "container",
                "text",
                "image",
                "button",
                "toggle",
                "slider",
                "progress",
                "input",
                "scroll_view",
                "list",
                "grid",
                "tabs",
                "modal",
                "tooltip",
                "spacer",
                "safe_area",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            events: [
                "pointer_enter",
                "pointer_exit",
                "pointer_down",
                "pointer_up",
                "click",
                "focus",
                "submit",
                "value_changed",
                "drag",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            style_states: [
                StyleState::Hover,
                StyleState::Pressed,
                StyleState::Disabled,
                StyleState::Focused,
            ]
            .into_iter()
            .collect(),
            bindings: true,
            transitions: true,
            clipping: true,
            rich_text: true,
            virtual_list: true,
        }
    }

    pub fn supports_node(&self, type_name: &str) -> bool {
        self.node_types.contains(type_name)
    }

    pub fn supports_event(&self, event: &str) -> bool {
        self.events.contains(event)
    }

    pub fn supports_state(&self, state: StyleState) -> bool {
        self.style_states.contains(&state)
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::core()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_is_honest_about_current_runtime() {
        let caps = Capabilities::core();
        assert!(caps.supports_node("button"));
        assert!(caps.supports_node("scroll_view"));
        assert!(!caps.supports_node("toggle"));
        assert!(caps.supports_event("click"));
        assert!(caps.supports_event("pointer_enter"));
        assert!(caps.supports_event("submit"));
        assert!(!caps.supports_event("value_changed"));
        assert!(caps.supports_state(StyleState::Hover));
        assert!(caps.supports_state(StyleState::Focused));
        assert!(caps.bindings);
        assert!(caps.clipping);
        assert!(!caps.transitions);
        assert!(!caps.rich_text);
        assert!(!caps.virtual_list);
    }

    #[test]
    fn planned_covers_all_contract_nodes() {
        let caps = Capabilities::planned();
        for node in [
            "panel",
            "container",
            "text",
            "image",
            "button",
            "toggle",
            "slider",
            "progress",
            "input",
            "scroll_view",
            "list",
            "grid",
            "tabs",
            "modal",
            "tooltip",
            "spacer",
            "safe_area",
        ] {
            assert!(caps.supports_node(node), "missing {node}");
        }
    }
}
