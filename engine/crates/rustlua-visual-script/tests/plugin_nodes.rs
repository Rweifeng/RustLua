//! E03-4：插件节点与内置节点走同一张注册表，注册期挡住坏模板/撞名/错误分类；
//! 校验错误仍带节点 id（编辑器依赖它描红到节点）。

use std::sync::Once;

use rustlua_visual_script::{
    Category, Edge, EventKind, Graph, GraphError, Node, NodeSpec, ParamSpec, Params, PortRef,
    PortSpec, PortType, ValueType, all_nodes, compile, node_spec, plugin_node,
    register_plugin_node, validate,
};
use serde_json::{Value, json};

const SHOW_TEXT: NodeSpec = NodeSpec {
    kind: "plugin.demo.show_text",
    title: "插件·显示文本",
    category: Category::Action,
    event: None,
    inputs: &[
        PortSpec {
            name: "in",
            ty: PortType::Flow,
        },
        PortSpec {
            name: "text",
            ty: PortType::Value(ValueType::String),
        },
    ],
    outputs: &[],
    params: &[ParamSpec {
        name: "channel",
        ty: ValueType::String,
        non_empty: true,
    }],
};

const DOUBLE: NodeSpec = NodeSpec {
    kind: "plugin.demo.double",
    title: "插件·翻倍",
    category: Category::Data,
    event: None,
    inputs: &[PortSpec {
        name: "value",
        ty: PortType::Value(ValueType::Number),
    }],
    outputs: &[PortSpec {
        name: "result",
        ty: PortType::Value(ValueType::Number),
    }],
    params: &[],
};

const BAD_EVENT: NodeSpec = NodeSpec {
    kind: "plugin.demo.bad_event",
    title: "插件·坏事件",
    category: Category::Event,
    event: Some(EventKind::Start),
    inputs: &[],
    outputs: &[],
    params: &[],
};

const BAD_TEMPLATE: NodeSpec = NodeSpec {
    kind: "plugin.demo.bad_template",
    title: "插件·坏模板",
    category: Category::Action,
    event: None,
    inputs: &[PortSpec {
        name: "in",
        ty: PortType::Flow,
    }],
    outputs: &[],
    params: &[],
};

const BUILTIN_COLLISION: NodeSpec = NodeSpec {
    kind: "number",
    title: "插件·撞名",
    category: Category::Action,
    event: None,
    inputs: &[PortSpec {
        name: "in",
        ty: PortType::Flow,
    }],
    outputs: &[],
    params: &[],
};

const WRONG_CATEGORY: NodeSpec = NodeSpec {
    kind: "plugin.demo.wrong_category",
    title: "插件·错分类",
    category: Category::Data,
    event: None,
    inputs: &[],
    outputs: &[],
    params: &[],
};

fn ensure_plugins() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        register_plugin_node(
            &SHOW_TEXT,
            "plugin_demo.show({text}, {param:channel})",
            false,
        )
        .expect("注册示例动作节点");
        register_plugin_node(&DOUBLE, "({value} * 2)", true).expect("注册示例数据节点");
    });
}

fn node(id: &str, kind: &str) -> Node {
    Node {
        id: id.to_owned(),
        kind: kind.to_owned(),
        position: [0.0, 0.0],
        params: Params::new(),
    }
}

fn with_params(id: &str, kind: &str, params: &[(&str, Value)]) -> Node {
    let mut built = node(id, kind);
    for (name, value) in params {
        built.params.insert((*name).to_owned(), value.clone());
    }
    built
}

fn edge(from: &str, from_port: &str, to: &str, to_port: &str) -> Edge {
    Edge {
        from: PortRef {
            node: from.to_owned(),
            port: from_port.to_owned(),
        },
        to: PortRef {
            node: to.to_owned(),
            port: to_port.to_owned(),
        },
    }
}

fn graph(nodes: Vec<Node>, edges: Vec<Edge>) -> Graph {
    Graph {
        version: rustlua_visual_script::VGRAPH_VERSION,
        name: "plugin-node-test".to_owned(),
        variables: Vec::new(),
        nodes,
        edges,
    }
}

#[test]
fn registered_plugin_nodes_are_first_class() {
    ensure_plugins();
    assert_eq!(
        plugin_node(SHOW_TEXT.kind).map(|plugin| plugin.returns_value),
        Some(false)
    );
    assert!(
        all_nodes().iter().any(|spec| spec.kind == SHOW_TEXT.kind),
        "插件动作节点必须出现在编辑器调色板的同一份节点表里"
    );
    assert_eq!(
        node_spec(DOUBLE.kind).map(|spec| spec.title),
        Some("插件·翻倍")
    );

    let action_graph = graph(
        vec![
            node("start", "on_start"),
            with_params("text", "string", &[("value", json!("hi"))]),
            with_params("show", SHOW_TEXT.kind, &[("channel", json!("hud"))]),
        ],
        vec![
            edge("start", "then", "show", "in"),
            edge("text", "value", "show", "text"),
        ],
    );
    validate(&action_graph).expect("插件动作节点应通过校验");
    let lua = compile(&action_graph, "memory").expect("插件动作节点应能生成");
    assert!(
        lua.contains("plugin_demo.show(n_text, \"hud\")"),
        "动作模板应按输入表达式与参数渲染：\n{lua}"
    );

    let data_graph = graph(
        vec![
            node("start", "on_start"),
            node("put", "set_transform"),
            with_params("three", "number", &[("value", json!(3))]),
            with_params("zero", "number", &[("value", json!(0))]),
            node("double", DOUBLE.kind),
        ],
        vec![
            edge("start", "then", "put", "in"),
            edge("start", "self", "put", "entity"),
            edge("three", "value", "double", "value"),
            edge("double", "result", "put", "x"),
            edge("zero", "value", "put", "y"),
            edge("zero", "value", "put", "z"),
        ],
    );
    validate(&data_graph).expect("插件数据节点应通过校验");
    let lua = compile(&data_graph, "memory").expect("插件数据节点应能生成表达式");
    assert!(
        lua.contains("local n_double = (n_three * 2)"),
        "数据模板应生成为局部表达式：\n{lua}"
    );
    assert!(
        lua.contains("transform3d.set(owner, n_double, n_zero, n_zero)"),
        "下游动作应消费插件节点的表达式：\n{lua}"
    );
}

#[test]
fn plugin_node_errors_point_at_the_node() {
    ensure_plugins();
    let missing_input = graph(
        vec![
            node("start", "on_start"),
            with_params("show", SHOW_TEXT.kind, &[("channel", json!("hud"))]),
        ],
        vec![edge("start", "then", "show", "in")],
    );
    let errors = validate(&missing_input).unwrap_err();
    assert!(
        errors.iter().any(|error| matches!(
            error,
            GraphError::MissingInput { node, port } if node == "show" && port == "text"
        )),
        "插件节点的缺输入必须定位到节点：{errors:?}"
    );

    let unknown = graph(
        vec![
            node("start", "on_start"),
            node("ghost", "plugin.not_registered"),
        ],
        Vec::new(),
    );
    let errors = validate(&unknown).unwrap_err();
    assert!(
        errors.iter().any(|error| matches!(
            error,
            GraphError::UnknownNodeType { node, kind }
                if node == "ghost" && kind == "plugin.not_registered"
        )),
        "未注册插件节点必须定位到节点：{errors:?}"
    );
}

#[test]
fn registration_rejects_bad_plugins_at_register_time() {
    ensure_plugins();
    assert!(
        register_plugin_node(
            &SHOW_TEXT,
            "plugin_demo.show({text}, {param:channel})",
            false
        )
        .unwrap_err()
        .contains("已注册")
    );
    assert!(
        register_plugin_node(&BAD_EVENT, "", false)
            .unwrap_err()
            .contains("事件")
    );
    assert!(
        register_plugin_node(&BAD_TEMPLATE, "do({nope})", false)
            .unwrap_err()
            .contains("未声明输入")
    );
    assert!(
        register_plugin_node(&BUILTIN_COLLISION, "-- collision", false)
            .unwrap_err()
            .contains("内置")
    );
    assert!(
        register_plugin_node(&WRONG_CATEGORY, "-- wrong", false)
            .unwrap_err()
            .contains("Action")
    );
}
