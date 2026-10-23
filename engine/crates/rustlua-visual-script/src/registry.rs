//! v1 节点注册表（18 个内置节点），同时是校验与代码生成的唯一事实来源。
//! E03-4 起插件节点也注册到同一张查找表（[`register_plugin_node`]），
//! 因此编辑器调色板、校验与生成器不需要区分内置/插件。
//!
//! 范围取舍（P3 §6.10 风险项的「砍到 12 节点」预案只触发了一半）：
//! - 保留 4 个事件、3 个流程、5 个数据、6 个动作；
//! - **暂不做** `spawn_prefab` 与 `get_component`：当前 Lua API 面没有对应的稳定函数
//!   （`prefab.*` 只有 despawn/node/root/set_rect，实体也没有组件查询），生成出来只能
//!   在运行时炸。等 E03-3 补绑定后再作为新节点类型加入；
//! - `vec2` 节点也暂缓：v1 没有任何动作消费 Vec2（唯一的位置动作是 `transform3d.set`，
//!   收三个 Number），留着只会是摆不上桌的节点；`Vec2/Vec3/Any` 类型仍保留给变量与插件节点。

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use crate::ValueType;
use crate::graph::ValueType as VT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Event,
    Flow,
    Data,
    Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Start,
    Update,
    Interact,
    Collision,
}

impl EventKind {
    #[must_use]
    pub fn function_name(self) -> &'static str {
        match self {
            Self::Start => "on_start",
            Self::Update => "on_update",
            Self::Interact => "on_interact",
            Self::Collision => "on_collision",
        }
    }

    /// 事件节点在生成函数里的参数（顺序固定，决定签名）。
    #[must_use]
    pub fn parameters(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Start => &[("self", "owner")],
            Self::Update => &[("self", "owner"), ("dt", "dt")],
            Self::Interact => &[("self", "owner"), ("target", "target")],
            Self::Collision => &[("self", "owner"), ("other", "other")],
        }
    }

    /// 事件数据输出端口的 Lua 名字（`self` 映射到 `owner`，其余同名）。
    #[must_use]
    pub fn lua_binding(self, port: &str) -> Option<&'static str> {
        match (self, port) {
            (_, "self") => Some("owner"),
            (Self::Update, "dt") => Some("dt"),
            (Self::Interact, "target") => Some("target"),
            (Self::Collision, "other") => Some("other"),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortType {
    Flow,
    Value(VT),
}

impl PortType {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Flow => "flow",
            Self::Value(ty) => ty.label(),
        }
    }

    #[must_use]
    pub fn accepts(self, source: Self) -> bool {
        match (self, source) {
            (Self::Flow, Self::Flow) => true,
            (Self::Value(target), Self::Value(source)) => target.accepts(source),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PortSpec {
    pub name: &'static str,
    pub ty: PortType,
}

#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub ty: ValueType,
    /// 字符串参数是否必须非空（动作路径/事件名/clip 名）。
    pub non_empty: bool,
}

const fn flow(name: &'static str) -> PortSpec {
    PortSpec {
        name,
        ty: PortType::Flow,
    }
}

const fn value(name: &'static str, ty: VT) -> PortSpec {
    PortSpec {
        name,
        ty: PortType::Value(ty),
    }
}

const fn param(name: &'static str, ty: VT) -> ParamSpec {
    ParamSpec {
        name,
        ty,
        non_empty: true,
    }
}

const fn param_allow_empty(name: &'static str, ty: VT) -> ParamSpec {
    ParamSpec {
        name,
        ty,
        non_empty: false,
    }
}

pub struct NodeSpec {
    pub kind: &'static str,
    pub title: &'static str,
    pub category: Category,
    pub event: Option<EventKind>,
    pub inputs: &'static [PortSpec],
    pub outputs: &'static [PortSpec],
    pub params: &'static [ParamSpec],
}

impl NodeSpec {
    #[must_use]
    pub fn input(&self, name: &str) -> Option<&'static PortSpec> {
        self.inputs.iter().find(|port| port.name == name)
    }

    #[must_use]
    pub fn output(&self, name: &str) -> Option<&'static PortSpec> {
        self.outputs.iter().find(|port| port.name == name)
    }

    #[must_use]
    pub fn param(&self, name: &str) -> Option<&'static ParamSpec> {
        self.params.iter().find(|spec| spec.name == name)
    }
}

// --- 事件 ---------------------------------------------------------------

static ON_START: NodeSpec = NodeSpec {
    kind: "on_start",
    title: "开始",
    category: Category::Event,
    event: Some(EventKind::Start),
    inputs: &[],
    outputs: &[flow("then"), value("self", VT::Entity)],
    params: &[],
};

static ON_UPDATE: NodeSpec = NodeSpec {
    kind: "on_update",
    title: "每帧",
    category: Category::Event,
    event: Some(EventKind::Update),
    inputs: &[],
    outputs: &[
        flow("then"),
        value("self", VT::Entity),
        value("dt", VT::Number),
    ],
    params: &[],
};

static ON_INTERACT: NodeSpec = NodeSpec {
    kind: "on_interact",
    title: "交互",
    category: Category::Event,
    event: Some(EventKind::Interact),
    inputs: &[],
    outputs: &[
        flow("then"),
        value("self", VT::Entity),
        value("target", VT::Entity),
    ],
    params: &[],
};

static ON_COLLISION: NodeSpec = NodeSpec {
    kind: "on_collision",
    title: "碰撞",
    category: Category::Event,
    event: Some(EventKind::Collision),
    inputs: &[],
    outputs: &[
        flow("then"),
        value("self", VT::Entity),
        value("other", VT::Entity),
    ],
    params: &[],
};

// --- 流程 ---------------------------------------------------------------

static BRANCH: NodeSpec = NodeSpec {
    kind: "branch",
    title: "分支",
    category: Category::Flow,
    event: None,
    inputs: &[flow("in"), value("condition", VT::Bool)],
    outputs: &[flow("true"), flow("false")],
    params: &[],
};

static SEQUENCE: NodeSpec = NodeSpec {
    kind: "sequence",
    title: "顺序",
    category: Category::Flow,
    event: None,
    inputs: &[flow("in")],
    outputs: &[flow("then0"), flow("then1")],
    params: &[],
};

static GATE: NodeSpec = NodeSpec {
    kind: "gate",
    title: "闸门",
    category: Category::Flow,
    event: None,
    inputs: &[flow("in"), value("open", VT::Bool)],
    outputs: &[flow("then")],
    params: &[],
};

// --- 数据 ---------------------------------------------------------------

static NUMBER: NodeSpec = NodeSpec {
    kind: "number",
    title: "数字",
    category: Category::Data,
    event: None,
    inputs: &[],
    outputs: &[value("value", VT::Number)],
    params: &[param("value", VT::Number)],
};

static BOOL: NodeSpec = NodeSpec {
    kind: "bool",
    title: "布尔",
    category: Category::Data,
    event: None,
    inputs: &[],
    outputs: &[value("value", VT::Bool)],
    params: &[param("value", VT::Bool)],
};

static STRING: NodeSpec = NodeSpec {
    kind: "string",
    title: "字符串",
    category: Category::Data,
    event: None,
    inputs: &[],
    outputs: &[value("value", VT::String)],
    params: &[param_allow_empty("value", VT::String)],
};

static MATH: NodeSpec = NodeSpec {
    kind: "math",
    title: "四则运算",
    category: Category::Data,
    event: None,
    inputs: &[value("a", VT::Number), value("b", VT::Number)],
    outputs: &[value("result", VT::Number)],
    params: &[param("op", VT::String)],
};

static RANDOM: NodeSpec = NodeSpec {
    kind: "random",
    title: "随机数",
    category: Category::Data,
    event: None,
    inputs: &[value("min", VT::Number), value("max", VT::Number)],
    outputs: &[value("value", VT::Number)],
    params: &[],
};

// --- 动作 ---------------------------------------------------------------

static SET_TRANSFORM: NodeSpec = NodeSpec {
    kind: "set_transform",
    title: "设置位置",
    category: Category::Action,
    event: None,
    inputs: &[
        flow("in"),
        value("entity", VT::Entity),
        value("x", VT::Number),
        value("y", VT::Number),
        value("z", VT::Number),
    ],
    outputs: &[],
    params: &[],
};

static PLAY_ANIMATION: NodeSpec = NodeSpec {
    kind: "play_animation",
    title: "播放动画",
    category: Category::Action,
    event: None,
    inputs: &[flow("in"), value("entity", VT::Entity)],
    outputs: &[],
    params: &[param("clip", VT::String)],
};

static PLAY_AUDIO: NodeSpec = NodeSpec {
    kind: "play_audio",
    title: "播放音频",
    category: Category::Action,
    event: None,
    inputs: &[flow("in")],
    outputs: &[],
    params: &[param("name", VT::String)],
};

static EMIT_EVENT: NodeSpec = NodeSpec {
    kind: "emit_event",
    title: "派发事件",
    category: Category::Action,
    event: None,
    inputs: &[flow("in")],
    outputs: &[],
    params: &[param("name", VT::String)],
};

static UI_SET_TEXT: NodeSpec = NodeSpec {
    kind: "ui_set_text",
    title: "设置文本",
    category: Category::Action,
    event: None,
    inputs: &[flow("in"), value("text", VT::String)],
    outputs: &[],
    params: &[param("element", VT::String)],
};

static SAVE_GAME: NodeSpec = NodeSpec {
    kind: "save_game",
    title: "保存游戏",
    category: Category::Action,
    event: None,
    inputs: &[flow("in")],
    outputs: &[],
    params: &[],
};

static NODES: &[&NodeSpec] = &[
    &ON_START,
    &ON_UPDATE,
    &ON_INTERACT,
    &ON_COLLISION,
    &BRANCH,
    &SEQUENCE,
    &GATE,
    &NUMBER,
    &BOOL,
    &STRING,
    &MATH,
    &RANDOM,
    &SET_TRANSFORM,
    &PLAY_ANIMATION,
    &PLAY_AUDIO,
    &EMIT_EVENT,
    &UI_SET_TEXT,
    &SAVE_GAME,
];

/// E03-4：插件节点。内置节点与插件节点共用 [`node_spec`] 查找、校验与生成路径。
///
/// `template` 是 Lua 语句（动作）或表达式（数据）模板：
/// - `{port}` → 该输入端口的上游表达式；
/// - `{param:name}` → 参数的字面量（字符串已按 Lua 规则加引号）。
#[derive(Clone, Copy)]
pub struct PluginNode {
    pub spec: &'static NodeSpec,
    pub template: &'static str,
    pub returns_value: bool,
}

static PLUGIN_NODES: OnceLock<RwLock<BTreeMap<&'static str, PluginNode>>> = OnceLock::new();

fn plugin_nodes() -> &'static RwLock<BTreeMap<&'static str, PluginNode>> {
    PLUGIN_NODES.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// 注册一个插件节点（进程级、一次性；重复注册会被拒绝）。
///
/// 规则：kind 不得与内置/已注册节点重复；不允许事件与流程节点；`returns_value = true`
/// 必须是 `Category::Data`，否则必须是 `Category::Action`；参数只支持 Number/Bool/String；
/// 模板里的占位符必须在声明的输入/参数里。错误在注册期暴露，不拖到生成期。
pub fn register_plugin_node(
    spec: &'static NodeSpec,
    template: &'static str,
    returns_value: bool,
) -> Result<(), String> {
    if spec.kind.trim().is_empty() {
        return Err("插件节点 kind 不能为空".to_owned());
    }
    if spec.event.is_some() || spec.category == Category::Event {
        return Err(format!("插件节点 `{}` 不能是事件节点", spec.kind));
    }
    if spec.category == Category::Flow {
        return Err(format!("插件节点 `{}` 不能是流程节点", spec.kind));
    }
    match (returns_value, spec.category) {
        (true, Category::Data) | (false, Category::Action) => {}
        (true, _) => {
            return Err(format!(
                "插件数据节点 `{}` 的 category 必须是 Data",
                spec.kind
            ));
        }
        (false, _) => {
            return Err(format!(
                "插件动作节点 `{}` 的 category 必须是 Action",
                spec.kind
            ));
        }
    }
    for param in spec.params {
        if !matches!(param.ty, VT::Number | VT::Bool | VT::String) {
            return Err(format!(
                "插件节点 `{}` 的参数 `{}` 只支持 Number/Bool/String",
                spec.kind, param.name
            ));
        }
    }
    for token in template_tokens(template)? {
        if let Some(name) = token.strip_prefix("param:") {
            if spec.param(name).is_none() {
                return Err(format!(
                    "插件节点 `{}` 的模板引用了未声明参数 `{name}`",
                    spec.kind
                ));
            }
        } else if spec.input(token).is_none() {
            return Err(format!(
                "插件节点 `{}` 的模板引用了未声明输入 `{token}`",
                spec.kind
            ));
        }
    }
    if NODES.iter().any(|builtin| builtin.kind == spec.kind) {
        return Err(format!("节点类型 `{}` 已被内置节点占用", spec.kind));
    }
    let mut registry = plugin_nodes()
        .write()
        .map_err(|_| "插件节点注册表已中毒".to_owned())?;
    if registry.contains_key(spec.kind) {
        return Err(format!("节点类型 `{}` 已注册", spec.kind));
    }
    registry.insert(
        spec.kind,
        PluginNode {
            spec,
            template,
            returns_value,
        },
    );
    Ok(())
}

#[must_use]
pub fn plugin_node(kind: &str) -> Option<PluginNode> {
    plugin_nodes().read().ok()?.get(kind).copied()
}

fn template_tokens(template: &str) -> Result<Vec<&str>, String> {
    let mut tokens = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open + 1..].find('}') else {
            return Err(format!("模板占位符未闭合：`{template}`"));
        };
        let token = &rest[open + 1..open + 1 + close];
        if token.is_empty() || token.contains('{') {
            return Err(format!("模板占位符非法：`{template}`"));
        }
        tokens.push(token);
        rest = &rest[open + close + 2..];
    }
    if rest.contains('}') {
        return Err(format!("模板有孤立的 `}}`：`{template}`"));
    }
    Ok(tokens)
}

#[must_use]
pub fn node_spec(kind: &str) -> Option<&'static NodeSpec> {
    NODES
        .iter()
        .copied()
        .find(|spec| spec.kind == kind)
        .or_else(|| plugin_node(kind).map(|plugin| plugin.spec))
}

/// 内置节点 + 已注册插件节点（编辑器调色板与注册表测试共用）。
#[must_use]
pub fn all_nodes() -> Vec<&'static NodeSpec> {
    let mut nodes = NODES.to_vec();
    if let Ok(registry) = plugin_nodes().read() {
        nodes.extend(registry.values().map(|plugin| plugin.spec));
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_unique_kinds_and_at_least_one_event_per_kind() {
        let mut kinds = std::collections::BTreeSet::new();
        for spec in all_nodes() {
            assert!(kinds.insert(spec.kind), "重复节点类型 {}", spec.kind);
        }
        for kind in [
            EventKind::Start,
            EventKind::Update,
            EventKind::Interact,
            EventKind::Collision,
        ] {
            assert_eq!(
                all_nodes()
                    .iter()
                    .filter(|spec| spec.event == Some(kind))
                    .count(),
                1,
                "{kind:?} 必须有且仅有一个事件节点"
            );
        }
        assert_eq!(all_nodes().len(), 18);
    }

    #[test]
    fn every_event_output_has_a_lua_binding() {
        for spec in all_nodes() {
            let Some(event) = spec.event else { continue };
            for port in spec.outputs {
                if let PortType::Value(_) = port.ty {
                    assert!(
                        event.lua_binding(port.name).is_some(),
                        "{} 的输出 {} 没有 Lua 绑定",
                        spec.kind,
                        port.name
                    );
                }
            }
        }
    }
}
