//! 原 `mod sdf_tests` 里 `render_channels` 这一域（WP12 阶段 1 纯搬运；源行段 6410–6863）。
//!
//! - `world_text_anchors_to_world_position_and_follows_camera`
//! - `camera2d_controller_follows_scripted_target`
//! - `world_channel_is_sorted_far_to_near_within_a_layer`
//! - `quad_is_a_first_class_renderable_sharing_the_sprite_path`
//! - `entity_script_component_runs_inside_the_step_pipeline`
//! - `mesh_identity_is_stable_across_frames`
//! - `texture_identity_is_stable_across_frames`

use super::*;
use crate::core::scene::Scene;

/// 世界文字（WP4）：锚点跟随实体的世界位置与相机。
///
/// 场景里只有 WorldText 实体，因此产出的顶点全部来自文字；相机右移 100 世界单位后，
/// 同一个世界锚点必须投影到更靠左的屏幕位置。
#[test]
fn world_text_anchors_to_world_position_and_follows_camera() {
    let mut scene = Scene::new("world_text");
    scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "WorldText".into(),
            "text=HP;font_size=24;color=1,1,1;offset=0,-20;centered=true".into(),
        );
    }
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).expect("runtime");
    let base = Camera2D::new(Vec2::new(320.0, 180.0), 1.0).expect("camera");
    runtime.set_camera_2d(base);
    let first = runtime.vertices(640, 360).expect("vertices");
    assert!(!first.vertices.is_empty(), "世界文字应产出顶点");

    runtime.set_camera_2d(Camera2D {
        center: Vec2::new(base.center.x + 100.0, base.center.y),
        ..base
    });
    let second = runtime.vertices(640, 360).expect("vertices");
    let leftmost = |mesh: &SceneMesh| {
        mesh.vertices
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::INFINITY, f32::min)
    };
    assert!(
        leftmost(&second) < leftmost(&first),
        "相机右移 100 世界单位后文字应左移：{} vs {}",
        leftmost(&first),
        leftmost(&second)
    );
}

/// 2D 相机跟随（WP4）：脚本登记跟随目标后，宿主每帧按控制器结果移动相机，并把真实值
/// 回填给 `camera2d.get`（读到的是宿主在用的值，不是脚本的自述）。
#[test]
fn camera2d_controller_follows_scripted_target() {
    let mut scene = Scene::new("camera2d");
    scene.add_entity(SceneEntityId(1), "Player", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=400,200,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).expect("runtime");
    let player = runtime
        .script
        .with_world(|world| world.entities_with::<Transform>().first().copied())
        .expect("player entity");
    runtime
        .script
        .camera2d_controller_mut(|controller| controller.follow(Some(player)));
    runtime.set_camera_2d(Camera2D::new(Vec2::zero(), 1.0).expect("camera"));

    let input = InputSnapshot::default();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("one frame");
    let center = runtime.camera_center_2d();
    assert!(
        (center.x - 400.0).abs() < 1e-3 && (center.y - 200.0).abs() < 1e-3,
        "无平滑时相机应在第一帧直接到位：{center:?}"
    );
    // 回填一致性：Lua 侧 `camera2d.get` 必须等于宿主实际中心。
    assert_eq!(
        runtime.script.camera2d_controller(|c| c.center()),
        center,
        "camera2d.get 应读到宿主真实值"
    );

    // 平滑跟随：目标移动后不应一步到位。
    runtime
        .script
        .camera2d_controller_mut(|controller| controller.set_smoothing(6.0));
    runtime.script.with_world_mut(|world| {
        world
            .get_mut::<Transform>(player)
            .unwrap()
            .unwrap()
            .position = Vec2::new(600.0, 200.0);
    });
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("one frame");
    let smoothed = runtime.camera_center_2d();
    assert!(
        smoothed.x > 400.0 && smoothed.x < 600.0,
        "平滑跟随应逐步推进：{smoothed:?}"
    );
}

/// WP6 排序契约（"不写深度 + 按 z 排序"）：世界内容必须按**画家顺序**交给渲染器——
/// 同层内远的先画、近的后画，且层的先后压过 z。这条钉的是"宿主把顺序排好"这件事：
/// 渲染器不写深度，顺序错了就会画错（渲染器侧有自己的像素级测试）。
#[test]
fn world_channel_is_sorted_far_to_near_within_a_layer() {
    let root = game_root();
    let mut scene = crate::core::scene::Scene::new("world-sort");
    // 三个精灵：刻意让"插入顺序"与 z 顺序相反，并且给两个不同的排序层。
    let specs = [
        (1_u64, "Near", "-0.5", "default"),
        (2, "Far", "0.5", "default"),
        (3, "BackLayer", "0.9", "background"),
    ];
    for (id, name, z, layer) in specs {
        let entity_id = crate::core::scene::SceneEntityId(id);
        scene.add_entity(entity_id, name, None).unwrap();
        let entity = scene.entities.get_mut(&entity_id).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Sprite".into(),
            format!(
                "texture=assets/textures/player.png;size=16x16;pivot=0.5,0.5;\
                     sorting_layer={layer};z={z}"
            ),
        );
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    runtime.set_world_sprite_depth(true);
    let mesh = runtime.vertices(640, 360).expect("world vertices");
    assert_eq!(mesh.world_sprites.len(), 18, "三个精灵各 6 个顶点");

    // 每个四边形第一个顶点的 z，按绘制顺序列出。
    let quad_starts: Vec<usize> = mesh
        .world_sprite_draws
        .iter()
        .flat_map(|draw| (0..draw.vertex_count / 6).map(move |quad| draw.vertex_start + quad * 6))
        .collect();
    let quad_zs: Vec<f32> = quad_starts
        .iter()
        .map(|start| mesh.world_sprites[*start].position[2])
        .collect();
    assert_eq!(
        quad_zs,
        vec![0.9, 0.5, -0.5],
        "应先是 background 层（z=0.9，层优先），再是同层内远→近（0.5 → -0.5）"
    );
    // 分段必须深度一致：同一分段内的 z 只能有一个值（否则"分段一个 z"的前提不成立）。
    for draw in &mesh.world_sprite_draws {
        let first = mesh.world_sprites[draw.vertex_start].position[2];
        assert!(
            (draw.vertex_start..draw.vertex_start + draw.vertex_count)
                .all(|index| mesh.world_sprites[index].position[2] == first),
            "分段内混了不同 z"
        );
    }
}

/// docs/54 §4.3：**一等 Quad** 走的是与精灵完全相同的那条路。
///
/// 判据有三条，缺一条"Quad 是一等可绘制对象"就不成立：
/// ① 一个没有贴图的 Quad 也能产几何（`texture` 省略 ⇒ 内置白纹理，纯色矩形不再需要内部哨兵）；
/// ② 几何/材质/深度/排序与精灵同源（同一个批处理 ⇒ 顶点数、z、分段规则一致）；
/// ③ `Quad` 组件真的留在 ECS 里（不是实例化时被悄悄降级成 `Sprite`），因此运行时可查可改。
#[test]
fn quad_is_a_first_class_renderable_sharing_the_sprite_path() {
    let root = crate::core::games_dir().join("examples").join("game_api");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let mut scene = Scene::new("quad_primitives");
    // ① 纯色矩形：连 texture 都不写。
    scene
        .add_entity(SceneEntityId(1), "Backdrop", None)
        .unwrap();
    // ② 带贴图 + 显式排序层的矩形。
    scene.add_entity(SceneEntityId(2), "Bar", None).unwrap();
    {
        let backdrop = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        backdrop.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        backdrop
            .components
            .insert("Quad".into(), "size=64,32;color=0.25,0.5,1,1".into());
        let bar = scene.entities.get_mut(&SceneEntityId(2)).unwrap();
        bar.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        bar.components.insert(
            "Quad".into(),
            "size=120,8;color=1,1,1,1;texture=assets/textures/ui.png;z=-0.5;layer=actor;order=2"
                .into(),
        );
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    runtime.set_world_sprite_depth(true);
    let mesh = runtime.vertices(640, 360).expect("world vertices");
    assert_eq!(
        mesh.world_sprites.len(),
        12,
        "两个 Quad 各 6 个顶点（与精灵同一套几何）"
    );
    // 没有贴图的 Quad 采样内置白纹理；有贴图的注册进同一张纹理表（因此两者能合段）。
    assert!(
        mesh.paths
            .iter()
            .any(|path| path == crate::core::quad::QUAD_DEFAULT_TEXTURE),
        "缺省贴图必须解析到内置白纹理：{:?}",
        mesh.paths
    );
    assert!(
        mesh.paths
            .iter()
            .any(|path| path == "assets/textures/ui.png"),
        "显式贴图必须注册：{:?}",
        mesh.paths
    );
    // ②/③ 组件留在 ECS 里，且几何/材质按组件串生效。
    runtime.script.with_world(|world| {
        let mut quads: Vec<Quad> = Vec::new();
        world.for_each2::<Transform, Quad>(|id, _, quad| {
            assert!(
                world.get::<Sprite>(id).expect("world read").is_none(),
                "Quad 不该同时被写成 Sprite（否则会画两遍）"
            );
            quads.push(quad.clone());
        });
        assert_eq!(
            quads.len(),
            2,
            "两个 Quad 都必须留在 ECS 里（不能被降级成 Sprite）"
        );
        let plain = quads
            .iter()
            .find(|quad| quad.texture.is_none())
            .expect("应有那个没写 texture 的 Quad");
        assert_eq!(plain.color, [0.25, 0.5, 1.0, 1.0]);
        assert_eq!(plain.size, Vec2::new(64.0, 32.0));
    });
    // 分段按**层升序**绘制（层越大越靠上，与精灵同一套规则）：默认层的矩形先画，
    // `layer=actor;order=2` 的那个后画，因此压在上面。
    assert!(
        mesh.world_sprite_draws.len() >= 2,
        "两个不同层的 Quad 应落在不同分段：{:?}",
        mesh.world_sprite_draws.len()
    );
    let last = mesh.world_sprite_draws.last().expect("至少一个分段");
    let last_z = mesh.world_sprites[last.vertex_start].position[2];
    assert!(
        (last_z - -0.5).abs() < 1e-6,
        "显式排序层的 Quad 应画在后面（更靠上），实际末段 z={last_z}"
    );
}

/// docs/54 §4.1：每实体脚本必须**真的在固定步流水线里跑**——只断言"能手动调用"是不够的，
/// 组件化脚本要替代的正是"工程脚本自己遍历实体"的那套手工分派。
#[test]
fn entity_script_component_runs_inside_the_step_pipeline() {
    let root =
        std::env::temp_dir().join(format!("rustlua-rs-entity-script-{}", std::process::id()));
    let scripts = root.join("scripts");
    std::fs::create_dir_all(&scripts).expect("create scripts dir");
    std::fs::write(
        scripts.join("spin.lua"),
        "function on_spawn(id) spawn_seen = (spawn_seen or 0) + 1 end\n\
             function on_update(id, dt)\n\
               local t = transform.get(id)\n\
               transform.set(id, t.x, t.y + 20 * dt)\n\
             end\n",
    )
    .expect("write script");

    let mut scene = Scene::new("entity_script_pipeline");
    scene.add_entity(SceneEntityId(1), "Spinner", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity
            .components
            .insert("Script".into(), "file=scripts/spin.lua".into());
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let scripted_position = |runtime: &RuntimeScene| -> Vec2 {
        runtime.script.with_world(|world| {
            let mut position = None;
            world.for_each2::<Transform, crate::core::entity_script::EntityScript>(
                |_, transform, _| position = Some(transform.position),
            );
            position.expect("带 Script 的实体必须有 Transform")
        })
    };
    assert!(
        scripted_position(&runtime).y.abs() < 1e-6,
        "脚本不该在实例化时就动实体：{:?}",
        scripted_position(&runtime)
    );
    runtime
        .run_script_steps(1, 0.5, &crate::core::input::InputSnapshot::default())
        .expect("step runtime");
    assert_eq!(
        runtime.last_entity_script_calls(),
        1,
        "这一步应有 1 个实体脚本被调用"
    );
    let after = scripted_position(&runtime);
    assert!(
        (after.y - 10.0).abs() < 1e-4,
        "脚本应按 20*dt 移动实体（dt=0.5）：实际 {after:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// docs/54 §4.4：**网格也走同一套运行时身份**——3D 提取是 `&self`，因此身份表在 `RefCell`
/// 里；同一网格跨帧恒同句柄、不同网格不同句柄，且身份表记录的 `ResourceKind` 是 `Model`。
#[test]
fn mesh_identity_is_stable_across_frames() {
    let root = crate::core::games_dir().join("examples").join("game_api");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let mut scene = Scene::new("mesh_identity");
    for (id, mesh) in [
        (SceneEntityId(1), crate::core::mesh3d::GROUND_PLANE_SENTINEL),
        (SceneEntityId(2), "assets/models/missing.glb"),
    ] {
        scene.add_entity(id, format!("Mesh{}", id.0), None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity
            .components
            .insert("MeshRenderer".into(), format!("mesh={mesh}"));
    }
    let runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let first = runtime
        .scene_3d(Viewport::new(16.0, 9.0).unwrap())
        .expect("first frame 3d");
    assert_eq!(first.drawables.len(), 2, "两个网格都应在 drawable 里");
    let handles: Vec<_> = first
        .drawables
        .iter()
        .map(|drawable| drawable.mesh_handle.expect("运行时必须给网格身份"))
        .collect();
    assert_ne!(handles[0], handles[1], "不同网格必须是不同句柄");
    let second = runtime
        .scene_3d(Viewport::new(16.0, 9.0).unwrap())
        .expect("second frame 3d");
    let second_handles: Vec<_> = second
        .drawables
        .iter()
        .map(|drawable| drawable.mesh_handle.expect("身份"))
        .collect();
    assert_eq!(handles, second_handles, "同一网格在相邻两帧必须同句柄");
    let record = runtime
        .asset_entries()
        .into_iter()
        .find(|record| record.path == crate::core::mesh3d::GROUND_PLANE_SENTINEL)
        .expect("网格身份应在表里");
    assert_eq!(record.kind, crate::core::resource_kind::ResourceKind::Model);
}

/// docs/54 §4.4：运行时资源身份**跨帧稳定**——同一个路径在相邻两帧必须是同一个句柄，
/// 这正是"这一帧的 draw 引用的是哪个资源"可以跨帧比较的前提（`texture_paths` 的下标每帧
/// 重建，本身不是身份）。
#[test]
fn texture_identity_is_stable_across_frames() {
    let root = crate::core::games_dir().join("examples").join("game_api");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let mut scene = Scene::new("asset_identity");
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
    }
    scene.add_entity(SceneEntityId(2), "Bar", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(2)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,300,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        // 第二个资源（内置白纹理）：必须与贴图拿到**不同**的句柄。
        entity
            .components
            .insert("Quad".into(), "size=64,8;color=1,1,1,1".into());
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let first = runtime.vertices(640, 360).expect("first frame");
    assert_eq!(
        first.path_handles.len(),
        first.paths.len(),
        "句柄必须与路径同序同长"
    );
    assert_eq!(first.paths.len(), 2, "本帧应注册两个资源：贴图 + 白纹理");
    let second = runtime.vertices(640, 360).expect("second frame");
    assert_eq!(
        first.path_handles, second.path_handles,
        "同一路径在相邻两帧必须拿到同一个句柄"
    );
    assert_eq!(first.paths, second.paths);
    assert_ne!(
        first.path_handles[0], first.path_handles[1],
        "不同资源必须是不同句柄"
    );
    // 身份表里的记录确实是路径本体（不是"每帧重新编号"的副本）。
    let entries = runtime.asset_entries();
    assert_eq!(entries.len(), 2, "身份表应恰好登记两个资源");
    let mut paths: Vec<&str> = entries.iter().map(|record| record.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        vec!["__white__", "assets/textures/player.png"],
        "身份表记录的就是资源路径"
    );
    assert_eq!(
        entries
            .iter()
            .find(|record| record.path == "assets/textures/player.png")
            .map(|record| record.kind),
        Some(ResourceKind::Texture)
    );
}
