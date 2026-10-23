use super::*;

use rustlua_visual_script::{Edge, Graph, Node, PortRef, VGRAPH_VERSION};
use serde_json::json;

/// 4 个事件各连一个 `ui.text`：不改世界，观测点直接是脚本侧 UI 状态。
fn behavior_graph(prefix: &str) -> String {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (index, (kind, element)) in [
        ("on_start", "start"),
        ("on_update", "update"),
        ("on_interact", "interact"),
        ("on_collision", "collision"),
    ]
    .into_iter()
    .enumerate()
    {
        let event = format!("event-{element}");
        let string = format!("string-{element}");
        let action = format!("action-{element}");
        nodes.push(Node {
            id: event.clone(),
            kind: kind.to_owned(),
            position: [0.0, index as f32 * 100.0],
            params: serde_json::Map::new(),
        });
        nodes.push(Node {
            id: string.clone(),
            kind: "string".to_owned(),
            position: [0.0, index as f32 * 100.0 + 40.0],
            params: json!({ "value": format!("{prefix}:{element}") })
                .as_object()
                .unwrap()
                .clone(),
        });
        nodes.push(Node {
            id: action.clone(),
            kind: "ui_set_text".to_owned(),
            position: [200.0, index as f32 * 100.0],
            params: json!({ "element": element }).as_object().unwrap().clone(),
        });
        edges.push(Edge {
            from: PortRef {
                node: event,
                port: "then".to_owned(),
            },
            to: PortRef {
                node: action.clone(),
                port: "in".to_owned(),
            },
        });
        edges.push(Edge {
            from: PortRef {
                node: string,
                port: "value".to_owned(),
            },
            to: PortRef {
                node: action,
                port: "text".to_owned(),
            },
        });
    }
    Graph {
        version: VGRAPH_VERSION,
        name: "runtime-behavior".to_owned(),
        variables: Vec::new(),
        nodes,
        edges,
    }
    .to_json()
}

fn text(runtime: &LuaRuntime, element: &str) -> Option<String> {
    runtime.with_ui(|texts, _| texts.get(element).map(|text| text.text.clone()))
}

/// E03-3：`.vgraph` 直接挂到实体 `Script` 上，4 类事件走现有实体脚本钩子；
/// 改图后热重载换掉行为，坏图重载失败时旧 VM 原封不动。
#[test]
fn vgraph_entity_runs_four_events_and_hot_reloads() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-vgraph-runtime-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let scripts = root.join("scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(scripts.join("behavior.vgraph"), behavior_graph("v1")).unwrap();

    let mut scene = rustlua_scene::scene::Scene::new("vgraph-runtime");
    let actor = rustlua_scene::scene::SceneEntityId(1);
    scene.add_entity(actor, "Actor", None).unwrap();
    scene.entities.get_mut(&actor).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene
        .entities
        .get_mut(&actor)
        .unwrap()
        .components
        .insert("Script".into(), "file=scripts/behavior.vgraph".into());
    scene
        .entities
        .get_mut(&actor)
        .unwrap()
        .components
        .insert("Collider".into(), "shape=ball;radius=6".into());
    let wall = rustlua_scene::scene::SceneEntityId(2);
    scene.add_entity(wall, "Wall", None).unwrap();
    scene.entities.get_mut(&wall).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&wall).unwrap().components.insert(
        "Collider".into(),
        "shape=ball;radius=6;dynamic=false".into(),
    );

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.set_project_root(root.clone());
    runtime.instantiate_scene(&scene).unwrap();
    // on_spawn 映射到图上 on_start；同一次 sync 不执行 on_update。
    assert_eq!(runtime.call_entity_scripts(0.016).unwrap(), 1);
    assert_eq!(text(&runtime, "start").as_deref(), Some("v1:start"));
    assert_eq!(text(&runtime, "update").as_deref(), Some("v1:update"));
    assert!(
        runtime.vgraph_trace_last().is_none(),
        "E03-4b 探针默认关闭：不为性能付埋点成本"
    );

    // 打开探针后再跑一次 update：事件节点与动作节点各报一次，最近一次是动作。
    runtime.set_vgraph_trace_enabled(true);
    runtime.call_entity_scripts(0.016).unwrap();
    let trace = runtime
        .vgraph_trace_last()
        .expect("update 执行后应有节点事件");
    assert_eq!(trace.script, "scripts/behavior.vgraph", "脚本路径统一用 /");
    assert_eq!(trace.node, "action-update");
    assert_eq!(runtime.vgraph_trace_sequence(), 2, "事件 + 动作各一次");

    // clear 只清最近事件，不动序号；下一次执行重新填上。
    runtime.clear_vgraph_trace();
    assert!(runtime.vgraph_trace_last().is_none());
    runtime.call_entity_scripts(0.016).unwrap();
    assert_eq!(
        runtime.vgraph_trace_last().map(|event| event.node),
        Some("action-update".to_owned())
    );

    // on_interact 走真实 UI 点击分发（目标名 → 实体 → 该实体的脚本）。
    runtime
        .process_ui_clicks(
            Some((5.0, 5.0)),
            &[UiClickTarget {
                name: "Actor".to_owned(),
                origin: [0.0, 0.0],
                size: [10.0, 10.0],
                functions: Vec::new(),
            }],
        )
        .unwrap();
    assert_eq!(text(&runtime, "interact").as_deref(), Some("v1:interact"));

    // on_collision 走真实物理接触：重叠的两个球在第一步产生 started 边沿，
    // 独立事件队列把它投给 Actor（Wall 没有脚本）。
    runtime.step_physics(1.0 / 60.0);
    assert!(runtime.dispatch_collision_contacts().unwrap() >= 1);
    assert_eq!(text(&runtime, "collision").as_deref(), Some("v1:collision"));

    // 改图 → 热重载：仅 on_update 会重新执行（on_spawn 不重复），行为必须变成 v2。
    fs::write(scripts.join("behavior.vgraph"), behavior_graph("v2")).unwrap();
    assert_eq!(runtime.reload_entity_scripts().unwrap(), 1);
    runtime.call_entity_scripts(0.016).unwrap();
    assert_eq!(text(&runtime, "update").as_deref(), Some("v2:update"));
    assert_eq!(
        text(&runtime, "start").as_deref(),
        Some("v1:start"),
        "热重载不重复 on_spawn"
    );

    // 坏图：重载报错且旧 VM 保持可用（不能停在半替换状态）。
    fs::write(scripts.join("behavior.vgraph"), "{ not json").unwrap();
    assert!(runtime.reload_entity_scripts().is_err());
    runtime.call_entity_scripts(0.016).unwrap();
    assert_eq!(text(&runtime, "update").as_deref(), Some("v2:update"));

    let _ = fs::remove_dir_all(root);
}

/// `on_start` → `set_transform` 的可观测量必须是运行时 `Transform3D`：
/// 图生成的 `transform3d.set(owner, x, y, z)` 直接写组件，而不是只碰 UI 文本。
/// 实体带 `Light`（3D 标记），场景装配才会建 `Transform3D` 而不是 2D 快路径。
#[test]
fn vgraph_on_start_sets_the_scene_transform() {
    use rustlua_ecs::transform::Transform3D;
    use rustlua_scene::scene::{Scene, SceneEntityId};

    let root = std::env::temp_dir().join(format!(
        "rustlua-vgraph-transform-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let scripts = root.join("scripts");
    fs::create_dir_all(&scripts).unwrap();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    nodes.push(Node {
        id: "start".to_owned(),
        kind: "on_start".to_owned(),
        position: [0.0, 0.0],
        params: serde_json::Map::new(),
    });
    for (index, (id, value)) in [("x", 1.5), ("y", 2.5), ("z", 3.5)].into_iter().enumerate() {
        nodes.push(Node {
            id: id.to_owned(),
            kind: "number".to_owned(),
            position: [0.0, 140.0 + index as f32 * 100.0],
            params: json!({ "value": value }).as_object().unwrap().clone(),
        });
        edges.push(Edge {
            from: PortRef {
                node: id.to_owned(),
                port: "value".to_owned(),
            },
            to: PortRef {
                node: "move".to_owned(),
                port: id.to_owned(),
            },
        });
    }
    nodes.push(Node {
        id: "move".to_owned(),
        kind: "set_transform".to_owned(),
        position: [360.0, 0.0],
        params: serde_json::Map::new(),
    });
    edges.push(Edge {
        from: PortRef {
            node: "start".to_owned(),
            port: "then".to_owned(),
        },
        to: PortRef {
            node: "move".to_owned(),
            port: "in".to_owned(),
        },
    });
    edges.push(Edge {
        from: PortRef {
            node: "start".to_owned(),
            port: "self".to_owned(),
        },
        to: PortRef {
            node: "move".to_owned(),
            port: "entity".to_owned(),
        },
    });
    let graph = Graph {
        version: VGRAPH_VERSION,
        name: "runtime-transform".to_owned(),
        variables: Vec::new(),
        nodes,
        edges,
    };
    fs::write(scripts.join("move.vgraph"), graph.to_json()).unwrap();

    let mut scene = Scene::new("vgraph-transform");
    let actor = SceneEntityId(1);
    scene.add_entity(actor, "Actor", None).unwrap();
    {
        let components = &mut scene.entities.get_mut(&actor).unwrap().components;
        components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        components.insert("Script".into(), "file=scripts/move.vgraph".into());
        components.insert(
            "Light".into(),
            "direction=0,-1,0;color=1,1,1;intensity=1".into(),
        );
    }

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.set_project_root(root.clone());
    runtime.instantiate_scene(&scene).unwrap();
    assert_eq!(runtime.call_entity_scripts(0.016).unwrap(), 1);

    let ids = runtime.with_world(|world| world.entities_with::<Transform3D>());
    assert_eq!(ids.len(), 1, "3D 标记实体必须装配出 Transform3D");
    let transform = runtime
        .with_world(|world| world.get::<Transform3D>(ids[0]).unwrap().copied())
        .unwrap();
    for (actual, expected) in [
        (transform.position.x, 1.5),
        (transform.position.y, 2.5),
        (transform.position.z, 3.5),
    ] {
        assert!(
            (actual - expected).abs() < 1e-5,
            "position={actual} expected {expected}"
        );
    }
    assert!((transform.rotation.w - 1.0).abs() < 1e-5);
    assert!((transform.scale.x - 1.0).abs() < 1e-5);

    let _ = fs::remove_dir_all(root);
}
