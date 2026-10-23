//! E03-1 验收：格式往返、负向门、稳定 id、确定性生成、Lua 5.4 可加载、golden。

use std::path::PathBuf;

use rustlua_visual_script::{
    Edge, Graph, GraphError, Node, Params, PortRef, compile, lua_var_name, validate,
};
use serde_json::{Value, json};

const SOURCE: &str = "assets/graphs/p3-e03-interact.vgraph";

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/p3-e03-interact.vgraph")
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/p3-e03-interact.generated.lua")
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
        name: "test-graph".to_owned(),
        variables: Vec::new(),
        nodes,
        edges,
    }
}

/// 合法最小图：on_start → set_transform(owner, 1, 2, 3)。
fn minimal_graph() -> Graph {
    graph(
        vec![
            node("start", "on_start"),
            node("put", "set_transform"),
            with_params("x", "number", &[("value", json!(1))]),
            with_params("y", "number", &[("value", json!(2))]),
            with_params("z", "number", &[("value", json!(3))]),
        ],
        vec![
            edge("start", "then", "put", "in"),
            edge("start", "self", "put", "entity"),
            edge("x", "value", "put", "x"),
            edge("y", "value", "put", "y"),
            edge("z", "value", "put", "z"),
        ],
    )
}

fn errors(graph: &Graph) -> Vec<GraphError> {
    validate(graph).expect_err("图应校验失败")
}

fn assert_has_error(graph: &Graph, needle: &str) {
    let all = errors(graph);
    assert!(
        all.iter().any(|error| error.to_string().contains(needle)),
        "期望错误包含 `{needle}`，实际：{all:#?}"
    );
}

#[test]
fn fixture_round_trips_and_matches_the_checked_in_golden() {
    let text = std::fs::read_to_string(fixture_path()).expect("fixture");
    let fixture = Graph::from_json(&text).expect("fixture 必须合法");
    let reparsed = Graph::from_json(&fixture.to_json()).expect("规范 JSON 必须可回读");
    assert_eq!(fixture, reparsed, "格式往返不得丢字段");

    let generated = compile(&fixture, SOURCE).expect("fixture 必须能生成");
    let golden = std::fs::read_to_string(golden_path()).expect("golden 必须存在");
    assert_eq!(
        generated.replace("\r\n", "\n"),
        golden.replace("\r\n", "\n"),
        "生成物与 golden 不一致：改节点集/生成器时必须同步 baseline"
    );
    assert!(
        generated
            .starts_with("-- generated from assets/graphs/p3-e03-interact.vgraph; do not edit\n")
    );

    let lua = mlua::Lua::new();
    let module: mlua::Table = lua
        .load(&generated)
        .eval()
        .expect("Lua 5.4 必须能加载生成物");
    for function in ["on_start", "on_update", "on_interact", "on_collision"] {
        assert!(module.contains_key(function).unwrap(), "缺少 {function}");
    }
}

#[test]
fn generated_variables_are_derived_from_stable_node_ids_and_are_deterministic() {
    let graph = graph(
        vec![
            node("start", "on_start"),
            with_params("seq-a", "sequence", &[]),
            with_params("put-1", "set_transform", &[]),
            with_params("num-x", "number", &[("value", json!(1.5))]),
            with_params("math-y", "math", &[("op", json!("add"))]),
            with_params("num-a", "number", &[("value", json!(2))]),
            with_params("num-b", "number", &[("value", json!(3))]),
            with_params("num-z", "number", &[("value", json!(4))]),
        ],
        vec![
            edge("start", "then", "seq-a", "in"),
            edge("seq-a", "then0", "put-1", "in"),
            edge("start", "self", "put-1", "entity"),
            edge("num-x", "value", "put-1", "x"),
            edge("math-y", "result", "put-1", "y"),
            edge("num-z", "value", "put-1", "z"),
            edge("num-a", "value", "math-y", "a"),
            edge("num-b", "value", "math-y", "b"),
        ],
    );
    let first = compile(&graph, "g.vgraph").expect("编译");
    let second = compile(&graph, "g.vgraph").expect("再编译");
    assert_eq!(first, second, "同一张图必须生成逐字节相同的 Lua");
    assert_eq!(lua_var_name("num-x"), "n_num_x");
    for expected in [
        "local n_num_x = 1.5",
        "local n_math_y = (n_num_a + n_num_b)",
        "transform3d.set(owner, n_num_x, n_math_y, n_num_z)",
    ] {
        assert!(first.contains(expected), "缺少 `{expected}`：\n{first}");
    }
    let lua = mlua::Lua::new();
    lua.load(&first).exec().expect("Lua 语法必须合法");
}

#[test]
fn random_and_all_action_calls_are_generated() {
    let graph = graph(
        vec![
            node("start", "on_start"),
            node("seq", "sequence"),
            node("seq2", "sequence"),
            node("seq3", "sequence"),
            node("seq4", "sequence"),
            node("seq5", "sequence"),
            with_params("rnd", "random", &[]),
            with_params("min", "number", &[("value", json!(0))]),
            with_params("max", "number", &[("value", json!(2))]),
            node("put", "set_transform"),
            with_params("zero", "number", &[("value", json!(0))]),
            with_params("anim", "play_animation", &[("clip", json!("spin"))]),
            with_params("sound", "play_audio", &[("name", json!("tick"))]),
            with_params("hint", "string", &[("value", json!("hi"))]),
            with_params("text", "ui_set_text", &[("element", json!("hud"))]),
            with_params("emit", "emit_event", &[("name", json!("done"))]),
            node("save", "save_game"),
        ],
        vec![
            edge("start", "then", "seq", "in"),
            edge("seq", "then0", "put", "in"),
            edge("start", "self", "put", "entity"),
            edge("rnd", "value", "put", "x"),
            edge("min", "value", "rnd", "min"),
            edge("max", "value", "rnd", "max"),
            edge("zero", "value", "put", "y"),
            edge("zero", "value", "put", "z"),
            edge("seq", "then1", "seq2", "in"),
            edge("seq2", "then0", "anim", "in"),
            edge("start", "self", "anim", "entity"),
            edge("seq2", "then1", "seq3", "in"),
            edge("seq3", "then0", "sound", "in"),
            edge("seq3", "then1", "seq4", "in"),
            edge("seq4", "then0", "text", "in"),
            edge("hint", "value", "text", "text"),
            edge("seq4", "then1", "seq5", "in"),
            edge("seq5", "then0", "emit", "in"),
            edge("seq5", "then1", "save", "in"),
        ],
    );
    let generated = compile(&graph, "actions.vgraph").expect("编译");
    for expected in [
        "math.random()",
        "animation3d.set(owner, \"spin\")",
        "audio.play(\"tick\")",
        "ui.text(\"hud\", n_hint)",
        "event.emit(\"done\")",
        "save.commit()",
    ] {
        assert!(
            generated.contains(expected),
            "缺少 `{expected}`：\n{generated}"
        );
    }
    mlua::Lua::new()
        .load(&generated)
        .exec()
        .expect("Lua 语法必须合法");
}

#[test]
fn duplicate_ids_and_sanitized_collisions_are_rejected() {
    let mut duplicate = minimal_graph();
    duplicate
        .nodes
        .push(with_params("x", "number", &[("value", json!(9))]));
    assert_has_error(&duplicate, "重复节点 id `x`");

    let mut collision = minimal_graph();
    collision
        .nodes
        .push(with_params("x-y", "number", &[("value", json!(1))]));
    collision
        .nodes
        .push(with_params("x_y", "number", &[("value", json!(2))]));
    assert_has_error(&collision, "归一化后都变成 `n_x_y`");
}

#[test]
fn dangling_edges_and_unknown_ports_are_rejected() {
    let mut dangling = minimal_graph();
    dangling.edges.push(edge("x", "value", "ghost", "x"));
    assert_has_error(&dangling, "终点节点不存在");

    let mut missing_start = minimal_graph();
    missing_start.edges.push(edge("ghost", "out", "put", "x"));
    assert_has_error(&missing_start, "起点节点不存在");

    let mut bad_port = minimal_graph();
    bad_port.edges.push(edge("x", "nope", "put", "x"));
    assert_has_error(&bad_port, "没有输出端口 `nope`");
}

#[test]
fn type_mismatch_is_rejected() {
    let mut mismatched = minimal_graph();
    // Number 接到 Entity 输入。
    mismatched
        .edges
        .retain(|edge| edge.to.node != "put" || edge.to.port != "entity");
    mismatched.edges.push(edge("x", "value", "put", "entity"));
    assert_has_error(&mismatched, "类型不匹配");
}

#[test]
fn missing_required_input_and_unknown_node_type_are_rejected() {
    let mut missing = minimal_graph();
    missing
        .edges
        .retain(|edge| edge.to.node != "put" || edge.to.port != "z");
    assert_has_error(&missing, "必填输入 `z` 未连接");

    let mut unknown = minimal_graph();
    unknown.nodes.push(node("mystery", "not_a_node"));
    assert_has_error(&unknown, "未注册");
}

#[test]
fn data_cycle_is_rejected() {
    let mut cyclic = minimal_graph();
    cyclic
        .nodes
        .push(with_params("m1", "math", &[("op", json!("add"))]));
    cyclic
        .nodes
        .push(with_params("m2", "math", &[("op", json!("add"))]));
    cyclic.edges.push(edge("m1", "result", "m2", "a"));
    cyclic.edges.push(edge("m2", "result", "m1", "a"));
    cyclic.edges.push(edge("m1", "result", "m2", "b"));
    cyclic.edges.push(edge("m2", "result", "m1", "b"));
    assert_has_error(&cyclic, "数据环");
}

#[test]
fn flow_cycles_need_a_gate_or_sequence_guard() {
    let bad = graph(
        vec![
            node("start", "on_start"),
            node("branch", "branch"),
            with_params("flag", "bool", &[("value", json!(true))]),
        ],
        vec![
            edge("start", "then", "branch", "in"),
            edge("flag", "value", "branch", "condition"),
            edge("branch", "true", "branch", "in"),
        ],
    );
    assert_has_error(&bad, "流程环没有 gate/sequence 守卫");

    let guarded = graph(
        vec![
            node("start", "on_start"),
            node("gate", "gate"),
            with_params("flag", "bool", &[("value", json!(true))]),
        ],
        vec![
            edge("start", "then", "gate", "in"),
            edge("flag", "value", "gate", "open"),
            edge("gate", "then", "gate", "in"),
        ],
    );
    let generated = compile(&guarded, "loop.vgraph").expect("gate 守卫的环必须放行");
    assert!(generated.contains("goto flow_"));
    mlua::Lua::new()
        .load(&generated)
        .exec()
        .expect("带 goto 回边的生成物必须能被 Lua 5.4 加载");
}

#[test]
fn duplicate_and_missing_events_are_rejected() {
    let mut duplicate = minimal_graph();
    duplicate.nodes.push(node("start2", "on_start"));
    assert_has_error(&duplicate, "事件入口重复");

    let only_data = graph(
        vec![with_params("n", "number", &[("value", json!(1))])],
        vec![],
    );
    assert_has_error(&only_data, "没有任何事件节点");
}

#[test]
fn flow_fanout_and_duplicate_input_edges_are_rejected() {
    let mut fanout = minimal_graph();
    fanout.nodes.push(node("save", "save_game"));
    fanout.edges.push(edge("start", "then", "save", "in"));
    assert_has_error(&fanout, "扇出");

    let mut double = minimal_graph();
    double
        .nodes
        .push(with_params("x2", "number", &[("value", json!(9))]));
    double.edges.push(edge("x2", "value", "put", "x"));
    assert_has_error(&double, "有多条入边");
}

#[test]
fn variables_are_validated_even_though_v1_has_no_get_set_nodes() {
    let mut graph = minimal_graph();
    graph.variables.push(rustlua_visual_script::Variable {
        name: "score".to_owned(),
        value_type: rustlua_visual_script::ValueType::Number,
        default: Some(json!("not a number")),
    });
    assert_has_error(&graph, "默认值与类型 number 不匹配");

    graph.variables[0].default = Some(json!(0));
    graph.variables.push(rustlua_visual_script::Variable {
        name: "end".to_owned(),
        value_type: rustlua_visual_script::ValueType::Bool,
        default: Some(json!(false)),
    });
    assert_has_error(&graph, "变量名 `end` 非法");
}

#[test]
fn generated_functions_drive_the_existing_lua_api_in_graph_order() {
    let text = std::fs::read_to_string(fixture_path()).expect("fixture");
    let graph = Graph::from_json(&text).expect("fixture");
    let generated = compile(&graph, SOURCE).expect("编译");

    let lua = mlua::Lua::new();
    lua.load(
        r#"
        calls = {}
        transform3d = { set = function(id, x, y, z)
            calls[#calls + 1] = "transform:" .. id .. ":" .. x .. "," .. y .. "," .. z
        end }
        animation3d = { set = function(id, name)
            calls[#calls + 1] = "anim:" .. id .. ":" .. name
        end }
        audio = { play = function(name) calls[#calls + 1] = "audio:" .. name end }
        event = { emit = function(name) calls[#calls + 1] = "event:" .. name end }
        ui = { text = function(id, text) calls[#calls + 1] = "ui:" .. id .. ":" .. text end }
        save = { commit = function() calls[#calls + 1] = "save" end }
        "#,
    )
    .exec()
    .expect("装 Lua stub");
    let module: mlua::Table = lua.load(&generated).eval().expect("加载生成物");
    let call = |name: &str, args: mlua::MultiValue| {
        module
            .get::<mlua::Function>(name)
            .expect("事件函数")
            .call::<()>(args)
            .expect("调用事件函数");
    };
    call(
        "on_start",
        mlua::MultiValue::from_vec(vec![mlua::Value::Integer(7)]),
    );
    call(
        "on_update",
        mlua::MultiValue::from_vec(vec![mlua::Value::Integer(7), mlua::Value::Number(0.016)]),
    );
    call(
        "on_interact",
        mlua::MultiValue::from_vec(vec![mlua::Value::Integer(7), mlua::Value::Integer(9)]),
    );
    call(
        "on_collision",
        mlua::MultiValue::from_vec(vec![mlua::Value::Integer(7), mlua::Value::Integer(9)]),
    );

    let calls: mlua::Table = lua.globals().get("calls").expect("calls");
    let recorded: Vec<String> = calls
        .sequence_values::<String>()
        .collect::<mlua::Result<_>>()
        .expect("读取调用记录");
    assert_eq!(
        recorded,
        vec![
            "transform:7:1.5,5,0",
            "save",
            "audio:tick",
            "anim:7:spin",
            "ui:hint:hello \"world\"\n",
            "event:hit",
            "save",
        ]
    );
}
