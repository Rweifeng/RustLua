//! 原 `mod sdf_tests` 里 `animation_layers` 这一域（WP12 阶段 1 纯搬运；源行段 7247–7477）。
//!
//! - `animation_clip_identity_is_stable_across_frames`
//! - `conflicting_renderables_are_reported_and_only_one_draws`
//! - `merging_adjacent_draws_never_crosses_render_layers`
//! - `explicit_render_modes_route_geometry_to_the_right_channel`

use super::*;
use rustlua_scene::scene::Scene;

/// docs/54 §4.4：**动画片段也有运行时身份**——推进动画时把片段的内容签名 intern 进与贴图/网格
/// 同一张身份表，因此"这份动画是哪一个"跨帧可比，而 `ResourceKind::Animation` 让它在诊断里
/// 一眼可辨。
#[test]
fn animation_clip_identity_is_stable_across_frames() {
    let root = rustlua_infra::games_dir().join("examples").join("game_api");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let mut scene = Scene::new("clip_identity");
    scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Sprite".into(),
            "texture=assets/textures/player.png;size=32x32".into(),
        );
        entity.components.insert(
            "Animation".into(),
            "default=idle;clip:idle:assets/textures/hero-idle.png:4:4:true".into(),
        );
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    // 第一步：推进动画时把片段身份 intern 进身份表。
    runtime
        .run_script_steps(
            1,
            1.0 / 60.0,
            &rustlua_input::input::InputSnapshot::default(),
        )
        .expect("step");
    let clip_key = |runtime: &RuntimeScene| -> String {
        let mut found = None;
        runtime.script.with_world(|world| {
            world.for_each::<rustlua_animation::animation::AnimationStateMachine>(|_, machine| {
                found = Some(machine.clip_resource_key().to_owned());
            });
        });
        found.expect("场景里的实体应当有动画状态机")
    };
    let key = clip_key(&runtime);
    let first = runtime
        .asset_handle(&key)
        .expect("推进动画后片段身份应已登记");
    let record = runtime
        .asset_entries()
        .into_iter()
        .find(|record| record.path == key)
        .expect("身份表里应有这条片段");
    assert_eq!(
        record.kind,
        rustlua_assets::resource_kind::ResourceKind::Animation,
        "片段身份必须标成动画资源"
    );
    let sizes = runtime.asset_entries().len();
    runtime
        .run_script_steps(
            1,
            1.0 / 60.0,
            &rustlua_input::input::InputSnapshot::default(),
        )
        .expect("second step");
    assert_eq!(clip_key(&runtime), key, "片段内容没变，身份键也不该变");
    assert_eq!(
        runtime.asset_handle(&key),
        Some(first),
        "同一段片段在相邻两步必须同句柄"
    );
    assert_eq!(
        runtime.asset_entries().len(),
        sizes,
        "已登记过的片段不该在第二帧重复登记"
    );
}

/// docs/54 §4.3：**一个实体只有一种可绘制表达**。
///
/// 同时挂 `Sprite` 与 `Quad` 时：① 加载期点名（否则"为什么另一个没画出来"无从解释）；
/// ② 渲染只按写下来的优先级画一种（`Sprite` 优先）——此前两条收集路径各自 push，会**画两遍**。
#[test]
fn conflicting_renderables_are_reported_and_only_one_draws() {
    let root = rustlua_infra::games_dir().join("examples").join("game_api");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let mut scene = Scene::new("renderable_conflict");
    scene.add_entity(SceneEntityId(1), "Both", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Sprite".into(),
            "texture=assets/textures/player.png;size=32x32".into(),
        );
        entity
            .components
            .insert("Quad".into(), "size=64,64;color=1,0,0,1".into());
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let conflicts = runtime.renderable_conflicts();
    assert_eq!(conflicts.len(), 1, "冲突必须被点名：{conflicts:?}");
    assert!(
        conflicts[0].contains("Both") && conflicts[0].contains("sprite + quad"),
        "诊断要点名实体与冲突项：{conflicts:?}"
    );
    runtime.set_world_sprite_depth(true);
    let mesh = runtime.vertices(640, 360).expect("vertices");
    assert_eq!(
        mesh.world_sprites.len(),
        6,
        "只应画精灵那一种（6 顶点），Quad 不许再画一遍"
    );
}

/// docs/54 §4.3：`merge_adjacent_draws` **必须把 `render_layer` 一起比**。
///
/// 回归背景：合并条件是"纹理相同 + 顶点连续"，漏了层以后，相邻的 ScreenUI 段会被并进前一段
/// 并沿用它的 `render_layer`——几何还在、层却变了，HUD 隔离（§8-⑤）会静默失效。
#[test]
fn merging_adjacent_draws_never_crosses_render_layers() {
    let mut draws = vec![
        SceneDraw::overlay(1, 0, 6),
        // 与上一段纹理相同、顶点连续，但层不同：不许并。
        SceneDraw::screen_ui(1, 6, 6),
        // 同层且连续：应当并成一段。
        SceneDraw::screen_ui(1, 12, 6),
    ];
    merge_adjacent_draws(&mut draws);
    assert_eq!(
        draws
            .iter()
            .map(|draw| (draw.render_layer, draw.start, draw.count))
            .collect::<Vec<_>>(),
        vec![
            (RenderLayer::Overlay2D, 0, 6),
            (RenderLayer::ScreenUI, 6, 12),
        ],
        "跨层不许合并，同层连续才合并"
    );
}

/// docs/54 §4.3：**三种显式 2D 显示模式**让同一场景混用世界通道与屏幕空间内容。
///
/// 判据是"几何真的走了哪条路"：默认实体进世界通道；`mode=overlay` / `mode=screen` 留在屏幕
/// 空间（分别落 `Overlay2D` / `ScreenUI` 桶，因此画在世界内容之上）；`mode=world` 请求世界
/// 通道——即使宿主开关是关的，只要场景里有实体这样声明，这一帧就走世界通道。
#[test]
fn explicit_render_modes_route_geometry_to_the_right_channel() {
    let root = rustlua_infra::games_dir().join("examples").join("game_api");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let mut scene = Scene::new("render_modes");
    for (id, name, mode) in [
        (SceneEntityId(1), "Default", None),
        (SceneEntityId(2), "Hud", Some("mode=overlay")),
        (SceneEntityId(3), "ScreenUi", Some("mode=screen")),
    ] {
        scene.add_entity(id, name, None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Sprite".into(),
            "texture=assets/textures/player.png;size=32x32".into(),
        );
        if let Some(mode) = mode {
            entity.components.insert("RenderMode".into(), mode.into());
        }
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    // 宿主打开世界通道：默认实体进世界通道，显式屏幕空间的两个留下。
    runtime.set_world_sprite_depth(true);
    let mesh = runtime.vertices(640, 360).expect("vertices");
    assert_eq!(
        runtime.render_mode_routed(),
        (0, 1, 1),
        "本帧应路由 1 个 overlay、1 个 screen，且没有显式 world"
    );
    assert_eq!(
        mesh.world_sprites.len(),
        6,
        "只有默认实体进了世界通道（6 顶点）"
    );
    let layers: Vec<RenderLayer> = mesh.draws.iter().map(|draw| draw.render_layer).collect();
    assert!(
        layers.contains(&RenderLayer::Overlay2D),
        "`mode=overlay` 的实体必须落在 Overlay2D 桶：{layers:?}"
    );
    assert!(
        layers.contains(&RenderLayer::ScreenUI),
        "`mode=screen` 的实体必须落在 ScreenUI 桶：{layers:?}"
    );
    assert_eq!(
        layers
            .iter()
            .filter(|layer| **layer == RenderLayer::World2D)
            .count(),
        0,
        "世界通道那一帧里不该再有 World2D 的 NDC 绘制"
    );

    // 宿主**关掉**世界通道，但场景里有实体声明 `mode=world` ⇒ 这一帧照样走世界通道。
    let mut scene = Scene::new("render_modes_world");
    scene.add_entity(SceneEntityId(1), "Forced", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Sprite".into(),
            "texture=assets/textures/player.png;size=32x32".into(),
        );
        entity
            .components
            .insert("RenderMode".into(), "mode=world".into());
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    assert!(!runtime.world_sprite_depth(), "宿主开关刻意保持关闭");
    let mesh = runtime.vertices(640, 360).expect("vertices");
    assert_eq!(runtime.render_mode_routed(), (1, 0, 0));
    assert_eq!(
        mesh.world_sprites.len(),
        6,
        "`mode=world` 必须让这一帧走世界通道"
    );
}
