//! 严格校验：重复 id、悬空边、端口类型、必填输入、数据环、无守卫流程环。
//!
//! 所有错误一次性收齐（不在第一条就返回），编辑器能把整张图的红点一起点亮。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::error::GraphError;
use crate::graph::{Graph, Node, ValueType, is_valid_node_id, lua_var_name};
use crate::registry::{NodeSpec, PortType, node_spec};

/// 单张图的节点上限；防止坏资产把递归校验/生成拖死（正常玩法图远小于此）。
pub const MAX_NODES: usize = 256;

const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

pub fn validate(graph: &Graph) -> Result<(), Vec<GraphError>> {
    let mut errors = Vec::new();
    if graph.name.trim().is_empty() {
        errors.push(GraphError::EmptyGraphName);
    }
    if graph.nodes.len() > MAX_NODES {
        errors.push(GraphError::GraphTooLarge {
            nodes: graph.nodes.len(),
            limit: MAX_NODES,
        });
        return Err(errors);
    }

    validate_nodes(graph, &mut errors);
    validate_variables(graph, &mut errors);
    validate_edges(graph, &mut errors);
    validate_cycles(graph, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_nodes(graph: &Graph, errors: &mut Vec<GraphError>) {
    let mut ids = BTreeSet::new();
    let mut lua_names: BTreeMap<String, &str> = BTreeMap::new();
    let mut events: BTreeMap<&'static str, &str> = BTreeMap::new();
    let mut event_count = 0usize;

    for node in &graph.nodes {
        if !is_valid_node_id(&node.id) {
            errors.push(GraphError::InvalidNodeId(node.id.clone()));
        } else {
            if !ids.insert(node.id.as_str()) {
                errors.push(GraphError::DuplicateNodeId(node.id.clone()));
            }
            let name = lua_var_name(&node.id);
            if let Some(first) = lua_names.insert(name.clone(), &node.id)
                && first != node.id
            {
                errors.push(GraphError::NodeIdCollision {
                    first: first.to_owned(),
                    second: node.id.clone(),
                    name,
                });
            }
        }
        if !node.position[0].is_finite() || !node.position[1].is_finite() {
            errors.push(GraphError::InvalidPosition {
                node: node.id.clone(),
            });
        }
        let Some(spec) = node_spec(&node.kind) else {
            errors.push(GraphError::UnknownNodeType {
                node: node.id.clone(),
                kind: node.kind.clone(),
            });
            continue;
        };
        validate_params(node, spec, errors);
        if let Some(event) = spec.event {
            event_count += 1;
            let function_name = event.function_name();
            if let Some(first) = events.insert(function_name, &node.id) {
                errors.push(GraphError::DuplicateEvent {
                    node: node.id.clone(),
                    first: first.to_owned(),
                    kind: node.kind.clone(),
                });
            }
        }
    }
    if event_count == 0 {
        errors.push(GraphError::MissingEvent);
    }
}

fn validate_params(node: &Node, spec: &NodeSpec, errors: &mut Vec<GraphError>) {
    for key in node.params.keys() {
        if spec.param(key).is_none() {
            errors.push(GraphError::UnknownParam {
                node: node.id.clone(),
                param: key.clone(),
            });
        }
    }
    for param in spec.params {
        let Some(value) = node.params.get(param.name) else {
            errors.push(GraphError::MissingParam {
                node: node.id.clone(),
                param: param.name.to_owned(),
            });
            continue;
        };
        let valid = match param.ty {
            ValueType::Number => value.as_f64().is_some_and(f64::is_finite),
            ValueType::Bool => value.is_boolean(),
            ValueType::String => value
                .as_str()
                .is_some_and(|text| !param.non_empty || !text.trim().is_empty()),
            // 内置节点没有 vec2/vec3/entity 参数；Any 留给插件节点（非 null 即可）。
            ValueType::Any => !value.is_null(),
            ValueType::Vec2 | ValueType::Vec3 | ValueType::Entity => false,
        };
        if !valid {
            errors.push(GraphError::InvalidParam {
                node: node.id.clone(),
                param: param.name.to_owned(),
                message: format!("需要 {}", param.ty.label()),
            });
        }
    }
    if node.kind == "math"
        && let Some(value) = node.params.get("op")
        && let Some(op) = value.as_str()
        && !matches!(op, "add" | "sub" | "mul" | "div")
    {
        errors.push(GraphError::InvalidParam {
            node: node.id.clone(),
            param: "op".to_owned(),
            message: format!("`{op}` 不是 add/sub/mul/div"),
        });
    }
}

fn validate_variables(graph: &Graph, errors: &mut Vec<GraphError>) {
    let mut names = BTreeSet::new();
    for variable in &graph.variables {
        if !is_valid_variable_name(&variable.name) {
            errors.push(GraphError::InvalidVariableName {
                name: variable.name.clone(),
            });
        }
        if !names.insert(variable.name.as_str()) {
            errors.push(GraphError::DuplicateVariable {
                name: variable.name.clone(),
            });
        }
        if let Some(default) = &variable.default
            && !default_matches(default, variable.value_type)
        {
            errors.push(GraphError::InvalidVariableDefault {
                name: variable.name.clone(),
                value_type: variable.value_type.label(),
            });
        }
    }
}

fn is_valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !LUA_KEYWORDS.contains(&name)
}

fn default_matches(value: &Value, ty: ValueType) -> bool {
    match ty {
        ValueType::Number => value.as_f64().is_some_and(f64::is_finite),
        ValueType::Bool => value.is_boolean(),
        ValueType::String => value.is_string(),
        ValueType::Vec2 => number_array(value, 2),
        ValueType::Vec3 => number_array(value, 3),
        ValueType::Entity => value.as_u64().is_some(),
        ValueType::Any => !value.is_null(),
    }
}

fn number_array(value: &Value, len: usize) -> bool {
    value.as_array().is_some_and(|items| {
        items.len() == len
            && items
                .iter()
                .all(|item| item.as_f64().is_some_and(f64::is_finite))
    })
}

fn validate_edges(graph: &Graph, errors: &mut Vec<GraphError>) {
    let mut seen_edges = BTreeSet::new();
    let mut input_edges: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    let mut flow_fanout: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
    let mut connected_inputs: BTreeSet<(&str, &str)> = BTreeSet::new();

    for edge in &graph.edges {
        let from = &edge.from;
        let to = &edge.to;
        let Some(from_node) = graph.node(&from.node) else {
            errors.push(GraphError::DanglingEdge {
                from_node: from.node.clone().into_boxed_str(),
                from_port: from.port.clone().into_boxed_str(),
                to_node: to.node.clone().into_boxed_str(),
                to_port: to.port.clone().into_boxed_str(),
                message: "起点节点不存在".into(),
            });
            continue;
        };
        let Some(to_node) = graph.node(&to.node) else {
            errors.push(GraphError::DanglingEdge {
                from_node: from.node.clone().into_boxed_str(),
                from_port: from.port.clone().into_boxed_str(),
                to_node: to.node.clone().into_boxed_str(),
                to_port: to.port.clone().into_boxed_str(),
                message: "终点节点不存在".into(),
            });
            continue;
        };
        let (Some(from_spec), Some(to_spec)) =
            (node_spec(&from_node.kind), node_spec(&to_node.kind))
        else {
            continue;
        };

        let key = (
            from.node.clone(),
            from.port.clone(),
            to.node.clone(),
            to.port.clone(),
        );
        if !seen_edges.insert(key.clone()) {
            errors.push(GraphError::DuplicateEdge {
                from_node: key.0,
                from_port: key.1,
                to_node: key.2,
                to_port: key.3,
            });
            continue;
        }

        let Some(from_port) = from_spec.output(&from.port) else {
            errors.push(GraphError::UnknownPort {
                node: from.node.clone(),
                port: from.port.clone(),
                side: "输出",
            });
            continue;
        };
        let Some(to_port) = to_spec.input(&to.port) else {
            errors.push(GraphError::UnknownPort {
                node: to.node.clone(),
                port: to.port.clone(),
                side: "输入",
            });
            continue;
        };
        if !to_port.ty.accepts(from_port.ty) {
            errors.push(GraphError::TypeMismatch {
                from_node: from.node.clone().into_boxed_str(),
                from_port: from.port.clone().into_boxed_str(),
                from_type: from_port.ty.label(),
                to_node: to.node.clone().into_boxed_str(),
                to_port: to.port.clone().into_boxed_str(),
                to_type: to_port.ty.label(),
            });
            continue;
        }

        // 数据输入只能接一条（避免求值次序歧义）；流程输入允许多条——它就是「汇合」，
        // 也是 gate/sequence 回边的唯一合法入口。
        if matches!(to_port.ty, PortType::Value(_)) {
            *input_edges
                .entry((to.node.as_str(), to.port.as_str()))
                .or_insert(0) += 1;
        }
        connected_inputs.insert((to.node.as_str(), to.port.as_str()));
        if from_port.ty == PortType::Flow {
            flow_fanout
                .entry((from.node.as_str(), from.port.as_str()))
                .or_default()
                .push(to.node.clone());
        }
    }

    for ((node, port), count) in input_edges {
        if count > 1 {
            errors.push(GraphError::DuplicateInputEdge {
                node: node.to_owned(),
                port: port.to_owned(),
            });
        }
    }
    for ((node, port), targets) in flow_fanout {
        if targets.len() > 1 {
            errors.push(GraphError::FlowFanOut {
                node: node.to_owned(),
                port: port.to_owned(),
                targets,
            });
        }
    }
    for node in &graph.nodes {
        let Some(spec) = node_spec(&node.kind) else {
            continue;
        };
        for input in spec.inputs {
            if !connected_inputs.contains(&(node.id.as_str(), input.name)) {
                errors.push(GraphError::MissingInput {
                    node: node.id.clone(),
                    port: input.name.to_owned(),
                });
            }
        }
    }
}

fn validate_cycles(graph: &Graph, errors: &mut Vec<GraphError>) {
    let mut data: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut flow: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in &graph.edges {
        let (Some(from_node), Some(to_node)) =
            (graph.node(&edge.from.node), graph.node(&edge.to.node))
        else {
            continue;
        };
        let (Some(from_spec), Some(to_spec)) =
            (node_spec(&from_node.kind), node_spec(&to_node.kind))
        else {
            continue;
        };
        let (Some(from_port), Some(to_port)) = (
            from_spec.output(&edge.from.port),
            to_spec.input(&edge.to.port),
        ) else {
            continue;
        };
        match (from_port.ty, to_port.ty) {
            (PortType::Value(_), PortType::Value(_)) if to_port.ty.accepts(from_port.ty) => {
                data.entry(&from_node.id).or_default().push(&to_node.id);
            }
            (PortType::Flow, PortType::Flow) => {
                flow.entry(&from_node.id).or_default().push(&to_node.id);
            }
            _ => {}
        }
    }

    let mut reported = BTreeSet::new();
    let mut cycles = Vec::new();
    for node in &graph.nodes {
        let mut color = BTreeMap::new();
        let mut stack = Vec::new();
        find_cycles(&node.id, &data, &mut color, &mut stack, &mut cycles);
    }
    for cycle in cycles {
        if reported.insert(cycle.iter().cloned().collect::<BTreeSet<_>>()) {
            errors.push(GraphError::DataCycle {
                nodes: display_cycle(&cycle),
            });
        }
    }

    let mut reported = BTreeSet::new();
    let mut cycles = Vec::new();
    for node in &graph.nodes {
        let mut color = BTreeMap::new();
        let mut stack = Vec::new();
        find_cycles(&node.id, &flow, &mut color, &mut stack, &mut cycles);
    }
    for cycle in cycles {
        let key: BTreeSet<_> = cycle.iter().cloned().collect();
        if !reported.insert(key) {
            continue;
        }
        let guarded = cycle.iter().any(|id| {
            graph
                .node(id)
                .and_then(|node| node_spec(&node.kind))
                .is_some_and(|spec| spec.kind == "gate" || spec.kind == "sequence")
        });
        if !guarded {
            errors.push(GraphError::FlowCycle {
                nodes: display_cycle(&cycle),
            });
        }
    }
}

fn find_cycles<'a>(
    node: &'a str,
    adjacency: &BTreeMap<&'a str, Vec<&'a str>>,
    color: &mut BTreeMap<&'a str, u8>,
    stack: &mut Vec<&'a str>,
    cycles: &mut Vec<Vec<String>>,
) {
    color.insert(node, 1);
    stack.push(node);
    if let Some(next_nodes) = adjacency.get(node) {
        for next in next_nodes {
            match color.get(next).copied().unwrap_or(0) {
                0 => find_cycles(next, adjacency, color, stack, cycles),
                1 => {
                    if let Some(position) = stack.iter().position(|item| item == next) {
                        cycles.push(
                            stack[position..]
                                .iter()
                                .map(|id| (*id).to_owned())
                                .collect(),
                        );
                    }
                }
                _ => {}
            }
        }
    }
    stack.pop();
    color.insert(node, 2);
}

fn display_cycle(cycle: &[String]) -> Vec<String> {
    let mut path = cycle.to_vec();
    if let Some(first) = cycle.first() {
        path.push(first.clone());
    }
    path
}
