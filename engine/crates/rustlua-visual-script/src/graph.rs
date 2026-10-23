//! `.vgraph` 资产模型：JSON 文本，节点 id 稳定、位置与参数原样保存。
//!
//! 解析层只做**语法**（serde + 版本号）；语义（重复 id、悬空边、类型、环）全部在
//! [`crate::validate`] 里，这样编辑器能拿到结构化的 [`GraphError`] 而不是 serde 字符串。

use serde::{Deserialize, Serialize};

use crate::GraphError;

/// 当前格式版本。读到别的版本直接拒绝，不做隐式迁移。
pub const VGRAPH_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub variables: Vec<Variable>,
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// 稳定 id：`[A-Za-z0-9_-]{1,64}`。生成 Lua 的变量名由它派生，改名即改生成物。
    pub id: String,
    /// 节点类型名，必须在 `crate::registry` 里注册。
    pub kind: String,
    /// 编辑器画布坐标；无 UI 时也照样读写，保证往返不丢。
    #[serde(default)]
    pub position: [f32; 2],
    #[serde(default)]
    pub params: Params,
}

pub type Params = serde_json::Map<String, serde_json::Value>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: PortRef,
    pub to: PortRef,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortRef {
    pub node: String,
    pub port: String,
}

/// 图级变量。v1 只落盘与校验，还没有 get/set 变量节点；解析器严格校验默认值类型，
/// 避免坏资产在将来接线时才炸。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    pub name: String,
    pub value_type: ValueType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

/// 端口值类型。`Any` 只能由将来的插件节点使用；内置节点不产出 `Any`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Number,
    Bool,
    String,
    Vec2,
    Vec3,
    Entity,
    Any,
}

impl ValueType {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Vec2 => "vec2",
            Self::Vec3 => "vec3",
            Self::Entity => "entity",
            Self::Any => "any",
        }
    }

    /// 赋值兼容性：同类型，或任一侧是 `Any`。
    #[must_use]
    pub fn accepts(self, source: Self) -> bool {
        self == source || self == Self::Any || source == Self::Any
    }
}

impl Graph {
    /// 解析 `.vgraph`。serde 错误保留行/列，编辑器错误列表直接可点。
    pub fn from_json(text: &str) -> Result<Self, GraphError> {
        let graph: Self = serde_json::from_str(text).map_err(|error| GraphError::Parse {
            line: error.line(),
            column: error.column(),
            message: error.to_string(),
        })?;
        if graph.version != VGRAPH_VERSION {
            return Err(GraphError::UnsupportedVersion {
                found: graph.version,
                expected: VGRAPH_VERSION,
            });
        }
        Ok(graph)
    }

    /// 规范 JSON（2 空格缩进 + 末尾换行）。写盘前先 `from_json` 回读由调用方负责。
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("vgraph 序列化不会失败");
        text.push('\n');
        text
    }

    #[must_use]
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

/// 由稳定 id 派生 Lua 局部变量名。校验阶段已保证字符集与改名后不冲突。
#[must_use]
pub fn lua_var_name(id: &str) -> String {
    let mut name = String::with_capacity(id.len() + 2);
    name.push_str("n_");
    for ch in id.chars() {
        name.push(if ch == '-' { '_' } else { ch });
    }
    name
}

/// 节点 id 允许的字符集；`n_` 前缀保证不会撞 Lua 关键字。
#[must_use]
pub fn is_valid_node_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_keeps_ids_positions_params_and_variables() {
        let text = r#"{
          "version": 1,
          "name": "demo",
          "variables": [{"name": "score", "value_type": "number", "default": 1.5}],
          "nodes": [
            {"id": "a-1", "kind": "on_start", "position": [10.5, -20.0]},
            {"id": "b", "kind": "number", "params": {"value": 2.0}}
          ],
          "edges": []
        }"#;
        let graph = Graph::from_json(text).expect("parse");
        let again = Graph::from_json(&graph.to_json()).expect("reparse");
        assert_eq!(graph, again);
        assert_eq!(again.node("a-1").unwrap().position, [10.5, -20.0]);
        assert_eq!(lua_var_name("a-1"), "n_a_1");
    }

    #[test]
    fn unknown_version_is_rejected_before_semantic_checks() {
        let error =
            Graph::from_json(r#"{"version": 2, "name": "x", "nodes": []}"#).expect_err("version 2");
        assert!(matches!(
            error,
            GraphError::UnsupportedVersion {
                found: 2,
                expected: 1
            }
        ));
    }

    #[test]
    fn unknown_fields_are_parse_errors_with_line_numbers() {
        let error = Graph::from_json(
            "{\n  \"version\": 1,\n  \"name\": \"x\",\n  \"nodes\": [],\n  \"typo\": 1\n}",
        )
        .expect_err("unknown field");
        match error {
            GraphError::Parse { line, .. } => assert_eq!(line, 5),
            other => panic!("expected parse error, got {other:?}"),
        }
    }
}
