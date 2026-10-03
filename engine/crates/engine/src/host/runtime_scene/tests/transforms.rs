//! 原 `mod sdf_tests` 里 `transforms` 这一域（WP12 阶段 1 纯搬运；源行段 8274–8704）。
//!
//! - `camera_pose_follows_the_parent_chain`
//! - `migrated_child_renders_at_the_same_place_with_inheritance_enabled`
//! - `draw_ranges_carry_the_render_layer_of_their_producer`
//! - `scene_entity_world_transform_follows_the_parent_chain`
//! - `physics_authority_decides_who_owns_the_transform`
//! - `spatial_inheritance_switches_between_local_and_world_positions`

use super::*;
use crate::core::scene::Scene;

#[test]
fn camera_pose_follows_the_parent_chain() {
    let root = game_root();
    let mut scene = Scene::new("camera-under-parent");
    let rig = crate::core::scene::SceneEntityId(1);
    let cam = crate::core::scene::SceneEntityId(2);
    let ground = crate::core::scene::SceneEntityId(3);
    scene.add_entity(rig, "Rig", None).unwrap();
    scene.add_entity(cam, "Cam", Some(rig)).unwrap();
    scene.add_entity(ground, "Ground", None).unwrap();
    // 相机姿态取的是**世界**变换（`world_transform_3d_lazy` 沿父链组合），父实体因此也必须
    // 落成 3D：空间种类由实体携带的其它组件推断（`spatial_transform::entity_space`），
    // 没有 3D 专属组件的父会按 2D 落库，组合会在这里断掉。
    scene
        .entities
        .get_mut(&rig)
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
    scene.entities.get_mut(&rig).unwrap().components.insert(
        "Transform".into(),
        "position=10,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&cam).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,6;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&cam).unwrap().components.insert(
            "Camera".into(),
            "projection=perspective;position=0,0,99;target=0,0,0;fov=60;near=0.1;far=100;pose=transform".into(),
        );
    // `scene_3d` 需要至少一个 drawable 才会返回 Some。
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene
        .entities
        .get_mut(&ground)
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());

    let runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
    let camera = runtime
        .scene_3d(Viewport::new(16.0, 9.0).unwrap())
        .expect("3d scene")
        .camera;
    assert!(
        (camera.position.x - 10.0).abs() < 1e-4 && (camera.position.z - 6.0).abs() < 1e-4,
        "相机姿态应取世界变换（父 +10 x 再局部 z=6），实际 {:?}",
        camera.position
    );
}

/// WP2 的关键验收：**迁移无损**——对同一份"子按世界坐标写"的场景跑真实迁移
/// （`scene_migration::migrate_scene_file`，含写回与读回校验），再把**迁移后的文件**
/// 读回来在**开启空间继承**下渲染；两帧顶点必须一致。这条把"迁移 + 继承"的效果钉在
/// **顶点级**证据上，并且钉的是**落盘之后**的数据——内存里换一组数值的等价不算数。
#[test]
fn migrated_child_renders_at_the_same_place_with_inheritance_enabled() {
    let root = game_root();
    let dir = std::env::temp_dir().join(format!(
        "rustlua-migration-vertex-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("hierarchy.scene");

    let mut scene = crate::core::scene::Scene::new("hierarchy-render");
    let parent = crate::core::scene::SceneEntityId(1);
    let child = crate::core::scene::SceneEntityId(2);
    scene.add_entity(parent, "Parent", None).unwrap();
    scene.add_entity(child, "Child", Some(parent)).unwrap();
    // 相机默认中心是 (640, 360)，精灵放这附近才不会被视口剔除掉（剔除是渲染路径的一部分）。
    for (id, x, scale, texture) in [
        (parent, "640", "2", "assets/textures/player.png"),
        (child, "650", "1", "assets/textures/coin.png"),
    ] {
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            format!("position={x},360,0;rotation=0,0,0,1;scale={scale},{scale},1"),
        );
        entity.components.insert(
            "Sprite".into(),
            format!("texture={texture};size=16x16;pivot=0.5,0.5"),
        );
    }
    scene.save(&path).expect("save legacy scene");

    // A：旧数据（子按世界坐标写）+ 关闭继承 = 迁移前的画面。
    let mut legacy =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene.clone()).expect("legacy");
    // A 档刻意关掉继承：它代表"迁移前的数据 + 迁移前的语义"。
    legacy.set_spatial_inheritance(false);
    assert!(!legacy.spatial_active(), "关掉后不应再有传播");
    let legacy_mesh = legacy.vertices(640, 360).expect("legacy vertices");
    assert!(
        !legacy_mesh.vertices.is_empty(),
        "样例场景应画出精灵（否则这条测试是空转）"
    );

    // B：跑真实**文件级**迁移（写回 + 读回校验），再把迁移后的文件读回来开继承渲染。
    let outcome = crate::core::scene_migration::migrate_scene_file(
        &path,
        crate::core::scene_migration::MigrationOptions {
            write: true,
            backup: false,
        },
    )
    .expect("migrate");
    assert!(
        outcome.written && outcome.verified,
        "必须真的写盘并读回校验：{outcome:?}"
    );
    assert!(outcome.is_lossless(), "这份两层场景应无损迁移：{outcome:?}");
    assert_eq!(
        outcome.converted, 1,
        "只有一个子实体需要改成局部坐标：{outcome:?}"
    );
    let migrated_scene = crate::core::scene::Scene::load(&path).expect("load migrated scene file");
    let mut migrated =
        RuntimeScene::from_scene_edit(Some(root.as_path()), migrated_scene).expect("migrated");
    migrated.set_spatial_inheritance(true);
    assert!(
        migrated.last_spatial_report().diagnostics.is_empty(),
        "{:?}",
        migrated.last_spatial_report()
    );
    let migrated_mesh = migrated.vertices(640, 360).expect("migrated vertices");

    let positions = |mesh: &SceneMesh| {
        mesh.vertices
            .iter()
            .map(|vertex| (vertex.position, vertex.uv))
            .collect::<Vec<_>>()
    };
    let legacy_positions = positions(&legacy_mesh);
    let migrated_positions = positions(&migrated_mesh);
    assert_eq!(
        legacy_positions.len(),
        migrated_positions.len(),
        "两帧顶点数必须相同（迁移不得多画/少画）"
    );
    for (index, (left, right)) in legacy_positions.iter().zip(&migrated_positions).enumerate() {
        assert!(
            (left.0[0] - right.0[0]).abs() < 1e-4 && (left.0[1] - right.0[1]).abs() < 1e-4,
            "第 {index} 个顶点位置不一致：{:?} vs {:?}",
            left.0,
            right.0
        );
        assert_eq!(left.1, right.1, "第 {index} 个顶点 UV 不一致");
    }

    std::fs::remove_dir_all(&dir).expect("clean temp dir");
}

/// WP6：绘制分段必须带上**正确的渲染层**——世界文字/精灵/瓦片是世界内容，
/// 屏幕 UI 文本是 `ScreenUI`。这条把"哪段几何属于哪一层"从"看谁调了哪个函数"
/// 变成可断言的事实（世界 Sprite 深度通道只吃世界层，靠的就是这个区分）。
#[test]
fn draw_ranges_carry_the_render_layer_of_their_producer() {
    let root = game_root();
    let build = |name: &str, component: (&str, &str)| {
        let mut scene = crate::core::scene::Scene::new(name);
        let id = crate::core::scene::SceneEntityId(1);
        scene.add_entity(id, "E", None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity
            .components
            .insert(component.0.into(), component.1.into());
        scene
    };

    // 世界文字：世界内容。
    let mut world_runtime = RuntimeScene::from_scene_edit(
        Some(root.as_path()),
        build(
            "world-text-layer",
            ("WorldText", "text=HP;font_size=16;color=1,1,1;offset=0,0"),
        ),
    )
    .expect("runtime");
    // 本用例断言的是 **SDF 字形**分段，因此自己钉住渲染模式：`game_api/project.toml`
    // 现为 `text_render_mode = "vector"`（示例工程刻意走矢量轮廓），`auto` 也会因
    // 字号 < 2.0 之外的启发式而漂移。依赖工程配置会让这条测试随工程设置变红。
    world_runtime.set_text_render_mode(TextRenderMode::Sdf);
    let world_mesh = world_runtime.vertices(640, 360).expect("vertices");
    assert!(!world_mesh.draws.is_empty(), "世界文字应产出绘制分段");
    assert!(
        world_mesh
            .draws
            .iter()
            .all(|draw| draw.render_layer == crate::core::layer::RenderLayer::World2D),
        "世界文字必须标成 World2D：{:?}",
        world_mesh
            .draws
            .iter()
            .map(|draw| draw.render_layer)
            .collect::<Vec<_>>()
    );

    // 屏幕 UI 文本：ScreenUI（不受世界相机影响、不参与深度）。
    // 屏幕 UI 要参与布局才有绘制分段，因此这里显式给一个 `UiLayout`。
    let mut ui_scene = crate::core::scene::Scene::new("ui-text-layer");
    let ui_id = crate::core::scene::SceneEntityId(1);
    ui_scene.add_entity(ui_id, "E", None).unwrap();
    let ui_entity = ui_scene.entities.get_mut(&ui_id).unwrap();
    ui_entity.components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    ui_entity.components.insert(
        "UiLayout".into(),
        "anchors=0,0,0,0;offsets=0,0,160,24".into(),
    );
    ui_entity
        .components
        .insert("Text".into(), "text=Menu;font_size=16;color=1,1,1".into());
    let mut ui_runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), ui_scene).expect("runtime");
    // 同上：本用例断言 SDF 路径，不继承工程配置的 `vector`。
    ui_runtime.set_text_render_mode(TextRenderMode::Sdf);
    let ui_mesh = ui_runtime.vertices(640, 360).expect("vertices");
    assert!(!ui_mesh.draws.is_empty(), "屏幕 UI 文本应产出绘制分段");
    assert!(
        ui_mesh
            .draws
            .iter()
            .all(|draw| draw.render_layer == crate::core::layer::RenderLayer::ScreenUI),
        "屏幕 UI 文本必须标成 ScreenUI：{:?}",
        ui_mesh
            .draws
            .iter()
            .map(|draw| draw.render_layer)
            .collect::<Vec<_>>()
    );
}

/// WP2 ④：编辑器拾取用的"按场景实体 id 读世界变换"必须与渲染同源——带父子关系时给出
/// **组合后**的位置，没有父子关系时给出局部值（两种情形都不需要调用方分支）。
#[test]
fn scene_entity_world_transform_follows_the_parent_chain() {
    let root = game_root();
    let mut scene = crate::core::scene::Scene::new("editor-picking");
    // docs/54 §4.1：断言的是父链组合，所以场景显式声明局部空间语义。
    scene.metadata.insert("spatial".into(), "local".into());
    let parent = crate::core::scene::SceneEntityId(1);
    let child = crate::core::scene::SceneEntityId(2);
    let lone = crate::core::scene::SceneEntityId(3);
    scene.add_entity(parent, "Parent", None).unwrap();
    scene.add_entity(child, "Child", Some(parent)).unwrap();
    scene.add_entity(lone, "Lone", None).unwrap();
    scene.entities.get_mut(&parent).unwrap().components.insert(
        "Transform".into(),
        "position=10,5,0;rotation=0,0,0,1;scale=2,2,1".into(),
    );
    scene.entities.get_mut(&child).unwrap().components.insert(
        "Transform".into(),
        "position=-3.5,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&lone).unwrap().components.insert(
        "Transform".into(),
        "position=7,9,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    runtime.propagate_spatial_now();

    let world = runtime
        .world_transform_2d_of(child)
        .expect("child world transform");
    assert!(
        (world.position.x - 3.0).abs() < 1e-3 && (world.position.y - 5.0).abs() < 1e-3,
        "子实体应给出组合后的世界位置 (3, 5)：{world:?}"
    );
    assert!((world.scale.x - 2.0).abs() < 1e-3, "父缩放应作用到子");
    // 没有父的实体：世界值 = 局部值（编辑器因此不需要分支）。
    let alone = runtime
        .world_transform_2d_of(lone)
        .expect("lone world transform");
    assert!((alone.position.x - 7.0).abs() < 1e-3 && alone.scale.x == 1.0);
    // 未知场景实体：`None`（而不是编一个 0 位置的框）。
    assert!(
        runtime
            .world_transform_2d_of(crate::core::scene::SceneEntityId(99))
            .is_none()
    );
}

/// WP7：`PhysicsAuthority` 决定"谁有权写 Transform"。
///
/// 场景：动态刚体在原点、零重力、无速度；**ECS 侧**把 Transform 改成 (100, 0)（等价于脚本
/// 直接改 ECS，而不是改刚体）。两种权威下的结果必须不同：
/// - `Physics`（默认）：模拟是权威 ⇒ 回写把 ECS 改回刚体的位置 0（脚本的位置被覆盖）；
/// - `Script`：ECS 是权威 ⇒ 步前同步把 100 推给刚体、步后不回写 ⇒ ECS 保持 100。
///
/// 这条同时证明「同步」与「回写」两格真的接在流水线上（而不只是注释里的顺序）。
#[test]
fn physics_authority_decides_who_owns_the_transform() {
    let root = game_root();
    let mut scene = crate::core::scene::Scene::new("authority");
    let id = crate::core::scene::SceneEntityId(1);
    scene.add_entity(id, "Crate", None).unwrap();
    let entity = scene.entities.get_mut(&id).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity
        .components
        .insert("Collider".into(), "shape=ball;radius=4".into());
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let moved = runtime.script.with_world(|world| {
        world
            .entities_with::<Transform>()
            .into_iter()
            .next()
            .expect("crate entity")
    });
    let input = InputSnapshot::default();

    // 默认：模拟是权威 ⇒ 脚本直接改 ECS 的位置会被回写覆盖。
    assert_eq!(runtime.physics_authority(), PhysicsAuthority::Physics);
    runtime.script.with_world_mut(|world| {
        world.get_mut::<Transform>(moved).unwrap().unwrap().position = Vec2::new(100.0, 0.0);
    });
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("steps");
    let after_physics = runtime
        .script
        .with_world(|world| world.get::<Transform>(moved).unwrap().unwrap().position);
    assert!(
        after_physics.x.abs() < 1e-3,
        "Physics 权威下模拟结果应写回 ECS（刚体仍在原点）：{after_physics:?}"
    );

    // Script 权威：ECS 是权威 ⇒ 步前同步、步后不回写，位置保持脚本写下的值。
    runtime.set_physics_authority(PhysicsAuthority::Script);
    runtime.script.with_world_mut(|world| {
        world.get_mut::<Transform>(moved).unwrap().unwrap().position = Vec2::new(100.0, 0.0);
    });
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("steps");
    let after_script = runtime
        .script
        .with_world(|world| world.get::<Transform>(moved).unwrap().unwrap().position);
    assert!(
        (after_script.x - 100.0).abs() < 1e-3,
        "Script 权威下 ECS 位置必须保留（步前同步、步后不回写）：{after_script:?}"
    );
}

/// WP2 + docs/54 §4.1：空间继承**默认由场景声明决定**，宿主开关只是覆盖。
///
/// 场景声明 `spatial=local` ⇒ 默认就组合父子；显式关掉 ⇒ 退回局部值（回滚档）；再打开 ⇒
/// 与第一次完全一致。未声明的场景默认是**组织关系**（另一条测试钉住）。
#[test]
fn spatial_inheritance_switches_between_local_and_world_positions() {
    let root = game_root();
    let mut scene = crate::core::scene::Scene::new("hierarchy");
    scene.metadata.insert("spatial".into(), "local".into());
    let parent = crate::core::scene::SceneEntityId(1);
    let child = crate::core::scene::SceneEntityId(2);
    scene.add_entity(parent, "Parent", None).unwrap();
    scene.add_entity(child, "Child", Some(parent)).unwrap();
    scene.entities.get_mut(&parent).unwrap().components.insert(
        "Transform".into(),
        "position=10,5,0;rotation=0,0,0,1;scale=2,2,1".into(),
    );
    // 迁移后的局部值：世界 x=3 ⇒ 局部 (3-10)/2 = -3.5。
    scene.entities.get_mut(&child).unwrap().components.insert(
        "Transform".into(),
        "position=-3.5,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let child_entity = runtime.script.with_world(|world| {
        world.entities_with::<Transform>().into_iter().find(|id| {
            world
                .get::<crate::core::hierarchy::Parent>(*id)
                .ok()
                .flatten()
                .is_some()
        })
    });
    let child_entity = child_entity.expect("child entity has a Parent component");

    let read_world = |runtime: &RuntimeScene| {
        runtime
            .script
            .with_world(|world| crate::core::hierarchy::world_transform_2d(world, child_entity))
            .unwrap()
            .expect("child transform")
    };

    // 场景声明了 `spatial=local`：默认就开，传播真的在跑，世界位置 = 父 ∘ 子 = (3, 5)。
    assert!(runtime.spatial_inheritance());
    runtime.propagate_spatial_now();
    assert!(runtime.spatial_active());
    assert!(runtime.last_spatial_report().diagnostics.is_empty());
    let composed = read_world(&runtime);
    assert!(
        (composed.position.x - 3.0).abs() < 1e-3 && (composed.position.y - 5.0).abs() < 1e-3,
        "开启继承后世界位置应为 (3, 5)：{composed:?}"
    );
    assert!((composed.scale.x - 2.0).abs() < 1e-3, "父缩放应作用到子");

    // 回滚档：关掉继承 ⇒ 世界值退回局部值（-3.5, 0），且不再有传播成本。
    runtime.set_spatial_inheritance(false);
    assert!(!runtime.spatial_active());
    let local = read_world(&runtime);
    assert!(
        (local.position.x + 3.5).abs() < 1e-3,
        "关掉继承后应退回局部值：{local:?}"
    );

    // 再打开：值与第一次开启时完全一致（开关可反复切换）。
    runtime.set_spatial_inheritance(true);
    assert_eq!(read_world(&runtime), composed);
}
