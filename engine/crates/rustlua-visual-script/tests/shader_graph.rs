use rustlua_visual_script::shader_graph::{ShaderGraphError, ShaderTemplate};

#[test]
fn every_builtin_template_has_valid_graph_and_deterministic_wgsl() {
    for template in ShaderTemplate::ALL {
        let name = template.name_zh();
        let graph = template.graph();
        graph.validate().expect(name);
        let source = graph.compile_wgsl().expect(name);
        assert_eq!(source, graph.compile_wgsl().expect("repeat compile"));
        assert!(source.contains("fn shader_graph_color"));
        assert!(!source.contains("$"));
    }
}

#[test]
fn json_round_trip_preserves_template_graph() {
    let graph = ShaderTemplate::Hologram.graph();
    let decoded = rustlua_visual_script::shader_graph::ShaderGraph::from_json(&graph.to_json())
        .expect("valid graph JSON");
    assert_eq!(graph, decoded);
}

#[test]
fn arithmetic_and_texture_nodes_emit_valid_wgsl() {
    use rustlua_visual_script::shader_graph::{
        ShaderEdge, ShaderGraph, ShaderNode, ShaderNodeKind, ShaderPort,
    };
    let node = |id: &str, kind| ShaderNode {
        id: id.into(),
        kind,
        position: [0.0, 0.0],
        params: Default::default(),
    };
    let port = |node: &str, port: &str| ShaderPort {
        node: node.into(),
        port: port.into(),
    };
    let edge = |a, ap, b, bp| ShaderEdge {
        from: port(a, ap),
        to: port(b, bp),
    };
    let graph = ShaderGraph {
        version: 1,
        name: "nodes".into(),
        nodes: vec![
            node("uv", ShaderNodeKind::Uv),
            node("tex", ShaderNodeKind::Texture),
            node("add", ShaderNodeKind::Add),
            node("out", ShaderNodeKind::Output),
        ],
        edges: vec![
            edge("uv", "value", "tex", "uv"),
            edge("tex", "value", "add", "a"),
            edge("tex", "value", "add", "b"),
            edge("add", "value", "out", "color"),
        ],
    };
    let wgsl = graph.compile_wgsl().expect("supported nodes compile");
    assert!(wgsl.contains("textureSample"));
    assert!(wgsl.contains("sg_add = (sg_tex) + (sg_tex)"));
}

#[test]
fn mismatched_port_types_are_rejected() {
    let mut graph = ShaderTemplate::Dissolve.graph();
    graph
        .edges
        .push(rustlua_visual_script::shader_graph::ShaderEdge {
            from: rustlua_visual_script::shader_graph::ShaderPort {
                node: "uv".into(),
                port: "value".into(),
            },
            to: rustlua_visual_script::shader_graph::ShaderPort {
                node: "out".into(),
                port: "alpha".into(),
            },
        });
    assert!(matches!(
        graph.validate(),
        Err(ShaderGraphError::TypeMismatch { .. })
    ));
}

#[test]
fn bad_edges_and_cycles_are_rejected() {
    let mut graph = ShaderTemplate::FresnelOutline.graph();
    graph.edges[0].from.port = "missing".into();
    assert!(matches!(
        graph.validate(),
        Err(ShaderGraphError::InvalidPort { .. })
    ));

    let mut graph = ShaderTemplate::FresnelOutline.graph();
    graph.edges.push(graph.edges[0].clone());
    assert!(matches!(
        graph.validate(),
        Err(ShaderGraphError::DuplicateInput { .. })
    ));
}

/// 模板扩充后的核心质量门：**每个模板必须生成不同的 WGSL**。
///
/// 只加名字不加图会很便宜，但对作者没有价值——20 个模板编译出同一份着色器，
/// 等于 19 个假模板。这条测试把"模板之间真的不同"钉成回归门。
#[test]
fn templates_compile_to_pairwise_distinct_wgsl() {
    let mut seen: Vec<(&str, String)> = Vec::new();
    for template in ShaderTemplate::ALL {
        let name = template.name_zh();
        let source = template.graph().compile_wgsl().expect(name);
        if let Some((other, _)) = seen.iter().find(|(_, text)| *text == source) {
            panic!(
                "模板 `{}`（{}）与 `{}` 生成的 WGSL 完全相同：模板之间必须真正不同",
                template.name_zh(),
                template.id(),
                other
            );
        }
        seen.push((template.name_zh(), source));
    }
    assert_eq!(seen.len(), ShaderTemplate::ALL.len());
}

/// 面向非程序员的元数据必须齐全：分类、性能等级、先调什么。
#[test]
fn templates_expose_category_perf_and_tuning_guidance() {
    let mut categories = std::collections::BTreeSet::new();
    for template in ShaderTemplate::ALL {
        assert!(
            !template.category_zh().is_empty(),
            "{} 缺分类",
            template.name_zh()
        );
        assert!(
            !template.tune_zh().is_empty(),
            "{} 缺调参提示",
            template.name_zh()
        );
        assert!(
            template.perf_level() <= 3,
            "{} 性能等级越界",
            template.name_zh()
        );
        categories.insert(template.category_zh());
    }
    assert!(
        categories.len() >= 3,
        "模板应覆盖角色/技能/环境/UI 等多类场景，实测只有 {categories:?}"
    );
}

/// 默认图必须真的"接上"输入节点，而不是只有输出节点。
#[test]
fn every_template_graph_wires_inputs_into_its_output() {
    for template in ShaderTemplate::ALL {
        let graph = template.graph();
        assert!(
            graph.nodes.len() >= 2,
            "{} 的默认图只有输出节点",
            template.name_zh()
        );
        assert!(
            !graph.edges.is_empty(),
            "{} 的默认图没有任何连线",
            template.name_zh()
        );
        // 每个图最终都要把值送进输出节点的 tint 或 color 端口。
        let out = graph
            .nodes
            .iter()
            .find(|node| node.kind == rustlua_visual_script::shader_graph::ShaderNodeKind::Output)
            .expect("output");
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.to.node == out.id
                    && matches!(edge.to.port.as_str(), "tint" | "color")),
            "{} 的颜色没有接到输出节点",
            template.name_zh()
        );
    }
}

#[test]
fn builtin_templates_have_complete_unique_metadata() {
    use std::collections::BTreeSet;

    assert!(
        ShaderTemplate::ALL.len() >= 20,
        "at least twenty built-in effects"
    );
    let ids: BTreeSet<_> = ShaderTemplate::ALL
        .iter()
        .map(|template| template.id())
        .collect();
    let names: BTreeSet<_> = ShaderTemplate::ALL
        .iter()
        .map(|template| template.name_zh())
        .collect();
    assert_eq!(
        ids.len(),
        ShaderTemplate::ALL.len(),
        "template ids must be unique"
    );
    assert_eq!(
        names.len(),
        ShaderTemplate::ALL.len(),
        "Chinese names must be unique"
    );
    for template in ShaderTemplate::ALL {
        assert!(!template.id().is_empty());
        assert!(!template.name_zh().is_empty());
        assert!(!template.hint_zh().is_empty());
        assert_eq!(template.graph().name, template.id());
    }
}

/// 运行时叠加层要把 `binding 0` 留给自己的 uniform，纹理/采样器因此整体后移。
#[test]
fn binding_base_moves_texture_and_sampler_bindings() {
    let graph = ShaderTemplate::Hologram.graph();
    let normal = graph.compile_wgsl().expect("base 0");
    let shifted = graph.compile_wgsl_with_binding_base(1).expect("base 1");
    assert!(
        normal.contains("@group(0) @binding(0) var shader_graph_texture"),
        "默认纹理绑定必须是 0：{normal}"
    );
    assert!(
        normal.contains("@group(0) @binding(1) var shader_graph_sampler"),
        "默认采样器绑定必须是 1：{normal}"
    );
    assert!(
        shifted.contains("@group(0) @binding(1) var shader_graph_texture"),
        "移位后纹理绑定必须是 1：{shifted}"
    );
    assert!(
        shifted.contains("@group(0) @binding(2) var shader_graph_sampler"),
        "移位后采样器绑定必须是 2：{shifted}"
    );
}

/// 工程内 `.sgraph` → 运行时叠加层源码（game 与编辑器共用的加载路径）。
#[test]
fn load_runtime_overlay_reads_project_sgraph() {
    let root = std::env::temp_dir().join(format!("rustlua-sgraph-runtime-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("assets/fx")).unwrap();
    let graph = ShaderTemplate::Scanlines.graph();
    std::fs::write(root.join("assets/fx/scan.sgraph"), graph.to_json()).unwrap();
    let (source, compiled) =
        rustlua_visual_script::shader_graph::ShaderGraph::load_runtime_overlay(
            &root,
            "assets/fx/scan.sgraph",
        )
        .expect("load runtime overlay");
    assert!(source.contains("@group(0) @binding(0) var<uniform> effect"));
    assert!(source.contains("@group(0) @binding(3) var<uniform> graph_params"));
    assert!(source.contains("fn vs_main") && source.contains("fn fs_main"));
    assert_eq!(compiled.slots.len(), 16);

    // 路径逃逸必须被拒绝（与 `.sgraph` 保存同一口径）。
    assert!(
        rustlua_visual_script::shader_graph::ShaderGraph::load_runtime_overlay(
            &root,
            "../escape.sgraph"
        )
        .is_err()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn uniform_params_emit_slots_instead_of_baked_literals() {
    use rustlua_visual_script::shader_graph::{MAX_PARAM_SLOTS, ShaderNodeKind};
    use std::collections::BTreeSet;
    let graph = ShaderTemplate::Fire.graph();
    let literal = graph.compile_wgsl().expect("literal compile");
    let compiled = graph.compile_wgsl_uniform(0, 3).expect("uniform compile");

    // 频率 16.0 是火焰模板的默认值：uniform 版不应再把它烘焙进源码。
    assert!(
        !compiled.source.contains("16.0000"),
        "uniform 版不应把频率烘焙成字面量：\n{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("graph_params.p["),
        "uniform 版应引用参数槽：\n{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("@group(0) @binding(3) var<uniform> graph_params"),
        "参数表应绑在调用方指定的 binding 3：\n{}",
        compiled.source
    );
    assert!(literal.contains("16.0000"), "字面量版保留烘焙值");
    assert!(!literal.contains("graph_params"), "字面量版不声明参数表");

    assert_eq!(compiled.slots.len(), MAX_PARAM_SLOTS, "槽位数量必须固定");
    assert!(
        compiled
            .params
            .iter()
            .any(|param| param.param == "frequency"),
        "频率参数必须出现在槽位表里：{:?}",
        compiled.params
    );
    // 常量颜色的三个分量应落在同一个 vec4 槽里。
    let color_slots: BTreeSet<usize> = compiled
        .params
        .iter()
        .filter(|param| param.param == "color")
        .map(|param| param.slot)
        .collect();
    assert_eq!(color_slots.len(), 1, "颜色三通道应共享一个 vec4 槽");

    // 默认值与图上的参数一致：改图上的频率，槽位值跟着变。
    let mut edited = graph.clone();
    if let Some(node) = edited
        .nodes
        .iter_mut()
        .find(|node| node.kind == ShaderNodeKind::Sine)
    {
        node.params.insert("frequency".into(), 42.0);
    }
    let values = edited.param_values(&compiled);
    let slot = compiled
        .params
        .iter()
        .find(|param| param.param == "frequency")
        .map(|param| param.slot)
        .expect("frequency slot");
    assert_eq!(values[slot][0], 42.0, "改图参数后槽位值必须跟着变");
}

/// 超出 16 个槽位要明确报错，而不是悄悄截断。
#[test]
fn too_many_params_are_rejected() {
    use rustlua_visual_script::shader_graph::{
        ShaderEdge, ShaderGraph, ShaderNode, ShaderNodeKind, ShaderPort,
    };
    let node = |id: &str, kind| ShaderNode {
        id: id.into(),
        kind,
        position: [0.0, 0.0],
        params: Default::default(),
    };
    let edge = |a: &str, ap: &str, b: &str, bp: &str| ShaderEdge {
        from: ShaderPort {
            node: a.into(),
            port: ap.into(),
        },
        to: ShaderPort {
            node: b.into(),
            port: bp.into(),
        },
    };
    // 17 个常量节点 + 输出：每个常量占一个槽 → 超限。
    let mut nodes = vec![node("out", ShaderNodeKind::Output)];
    let mut edges = Vec::new();
    for index in 0..17 {
        let id = format!("c{index}");
        nodes.push(node(&id, ShaderNodeKind::Constant));
        if index == 0 {
            edges.push(edge(&id, "value", "out", "tint"));
        }
    }
    let graph = ShaderGraph {
        version: 1,
        name: "many".into(),
        nodes,
        edges,
    };
    let error = graph
        .compile_wgsl_uniform(0, 3)
        .expect_err("超过槽位上限必须报错");
    assert!(matches!(error, ShaderGraphError::TooManyParams(_)));
}

/// 命名参数表：唯一参数直接用名字，颜色展开 `_r/_g/_b`，默认值与槽位一致。
#[test]
fn named_params_are_unique_and_expand_colors() {
    let graph = ShaderTemplate::Fire.graph();
    let compiled = graph.compile_wgsl_uniform(0, 3).expect("uniform compile");
    let named = compiled.named_params();
    let names: Vec<&str> = named.iter().map(|param| param.name.as_str()).collect();
    assert!(
        names.contains(&"frequency"),
        "唯一参数直接用名字：{names:?}"
    );
    assert!(
        names.iter().any(|name| name.starts_with("color_")),
        "颜色应展开：{names:?}"
    );
    assert_eq!(names.len(), compiled.params.len(), "每个槽分量都应有名字");
    let frequency = named
        .iter()
        .find(|param| param.name == "frequency")
        .expect("frequency named");
    assert!(frequency.default > 0.0, "默认值应来自槽位默认");
}
