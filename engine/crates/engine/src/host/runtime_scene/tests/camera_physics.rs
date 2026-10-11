//! 原 `mod sdf_tests` 里 `camera_physics` 这一域（WP12 阶段 1 纯搬运；源行段 7784–8273）。
//!
//! - `scene_declared_camera2d_becomes_the_initial_runtime_camera`
//! - `three_d_physics_authority_syncs_before_step_and_skips_writeback`
//! - `legacy_stage_order_actually_changes_the_pipeline`
//! - `document_runtime_and_lua_agree_on_the_world_transform`
//! - `loading_a_scene_reports_broken_parent_edges`
//! - `view_visibility_mask_filters_world_sprites`
//! - `view_visibility_mask_filters_3d_meshes`
//! - `view_visibility_mask_filters_entity_backed_ui_only`
//! - `collider_debug_draw_3d_projects_the_live_physics_pose`

use super::*;
use rustlua_scene::scene::Scene;

/// docs/54 §8-①：场景声明的**正交** `Camera` 成为**初始** 2D 相机；没声明时保持历史默认。
#[test]
fn scene_declared_camera2d_becomes_the_initial_runtime_camera() {
    let root = game_root();
    let mut scene = Scene::new("scene-camera2d");
    let id = rustlua_scene::scene::SceneEntityId(1);
    scene.add_entity(id, "Main2D", None).unwrap();
    scene.entities.get_mut(&id).unwrap().components.insert(
        "Camera".into(),
        "projection=orthographic;center=100,50;zoom=2".into(),
    );
    let runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
    let camera = runtime.camera_2d();
    assert_eq!(camera.center, Vec2::new(100.0, 50.0));
    assert_eq!(camera.zoom, 2.0);

    let plain = Scene::new("no-camera");
    let default_runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), plain).expect("runtime");
    assert_eq!(
        default_runtime.camera_2d(),
        Camera2D::new(rustlua_camera::camera::DEFAULT_2D_CAMERA_CENTER, 1.0).unwrap(),
        "没有声明相机时必须是历史默认（既有工程逐字节不变）"
    );
}

/// docs/54 §8-⑥：**3D 也有脚本权威**（此前只有 2D 接线，3D 的"authority"字段对 3D 无效）。
///
/// 两步都断言：`Script` 下步前把 ECS 位置推给 3D 模拟、步后不回写（脚本设的位置留在 ECS）；
/// 默认 `Physics` 下模拟结果回写 ECS（脚本的移动被模拟覆盖）。
#[test]
fn three_d_physics_authority_syncs_before_step_and_skips_writeback() {
    let root = game_root();
    let build = || {
        let mut scene = Scene::new("authority-3d");
        let ball = rustlua_scene::scene::SceneEntityId(1);
        scene.add_entity(ball, "Ball", None).unwrap();
        let entity = scene.entities.get_mut(&ball).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,5,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Collider3d".into(),
            "shape=ball;radius=0.5;dynamic=true".into(),
        );
        scene
    };
    let input = rustlua_input::input::InputSnapshot::default();
    let drop_one_step = 1.0 / 60.0;

    // ① authority = Script：步前同步（3D 计数为 1），步后不回写 ⇒ ECS 保留脚本设的位置。
    let mut scripted =
        RuntimeScene::from_scene_edit(Some(root.as_path()), build()).expect("runtime");
    scripted.set_physics_authority(PhysicsAuthority::Script);
    let handle = scripted
        .with_script(|script| script.scene_entity_handle(rustlua_scene::scene::SceneEntityId(1)))
        .expect("handle");
    scripted.with_script(|script| {
        script.with_world_mut(|world| {
            let transform = world
                .get_mut::<Transform3D>(handle)
                .expect("get")
                .expect("has transform3d");
            transform.position = Vec3::new(0.0, 9.0, 0.0);
        });
    });
    scripted
        .run_script_steps(1, drop_one_step, &input)
        .expect("step");
    assert_eq!(
        scripted.last_physics_sync(),
        (0, 1),
        "脚本权威下必须在物理步之前同步 3D 刚体（2D 场景里没有 2D 刚体，因此是 (0, 1)）"
    );
    let position = scripted
        .world_transform_3d_of(rustlua_scene::scene::SceneEntityId(1))
        .expect("world")
        .position;
    assert!(
        (position.y - 9.0).abs() < 1e-4,
        "脚本权威下 ECS 位置不得被回写覆盖，实际 {position:?}"
    );

    // ② 默认 authority = Physics：模拟是权威 ⇒ ECS 回到模拟结果（脚本那次移动被覆盖）。
    let mut simulated =
        RuntimeScene::from_scene_edit(Some(root.as_path()), build()).expect("runtime");
    let handle = simulated
        .with_script(|script| script.scene_entity_handle(rustlua_scene::scene::SceneEntityId(1)))
        .expect("handle");
    simulated.with_script(|script| {
        script.with_world_mut(|world| {
            let transform = world
                .get_mut::<Transform3D>(handle)
                .expect("get")
                .expect("has transform3d");
            transform.position = Vec3::new(0.0, 9.0, 0.0);
        });
    });
    simulated
        .run_script_steps(1, drop_one_step, &input)
        .expect("step");
    assert_eq!(
        simulated.last_physics_sync(),
        (0, 0),
        "默认权威不做步前同步"
    );
    let position = simulated
        .world_transform_3d_of(rustlua_scene::scene::SceneEntityId(1))
        .expect("world")
        .position;
    assert!(
        (position.y - 5.0).abs() < 0.05,
        "模拟权威下 ECS 应被回写成模拟结果（≈ 5 起落一步），实际 {position:?}"
    );
}

/// docs/54 §8-⑥ / §7：`PhysicsStageOrder::Legacy` 是**真回滚档**（此前是个没接线、没人读的字段）。
#[test]
fn legacy_stage_order_actually_changes_the_pipeline() {
    let root = game_root();
    let mut scene = Scene::new("legacy-order");
    let ball = rustlua_scene::scene::SceneEntityId(1);
    scene.add_entity(ball, "Ball", None).unwrap();
    let entity = scene.entities.get_mut(&ball).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=0,5,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity
        .components
        .insert("Collider3d".into(), "shape=ball;radius=0.5".into());

    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
    runtime.set_physics_authority(PhysicsAuthority::Script);
    runtime.set_physics_stage_order(PhysicsStageOrder::Legacy);
    runtime
        .run_script_steps(
            1,
            1.0 / 60.0,
            &rustlua_input::input::InputSnapshot::default(),
        )
        .expect("step");
    assert_eq!(
        runtime.last_physics_sync(),
        (0, 0),
        "回滚档没有统一 Sync 格：脚本权威不做步前同步（与收敛前的单系统顺序一致）"
    );
}

/// docs/54 §8-⑦：**文档 / 运行时 / Lua** 三条路必须对同一个实体给出同一个世界位置。
///
/// 这是本轮"同一空间语义"的收官断言：编辑态用 `scene_space`（文档），运行时用
/// `RuntimeScene::world_transform_3d_of`（World + GlobalTransform），Lua 用 `transform3d.world`
/// ——三处各算一次，任何一处退回局部值都会被这条抓住。
#[test]
fn document_runtime_and_lua_agree_on_the_world_transform() {
    let root = game_root();
    let rig = rustlua_scene::scene::SceneEntityId(1);
    let child = rustlua_scene::scene::SceneEntityId(2);
    let mut scene = Scene::new("three-way");
    // docs/54 §4.1：这条测试断言的是"父子参与空间组合"，因此场景必须**显式声明**它
    // （未声明 ⇒ 组织关系，运行时不会组合）。
    scene.metadata.insert("spatial".into(), "local".into());
    scene.add_entity(rig, "Rig", None).unwrap();
    scene.add_entity(child, "Child", Some(rig)).unwrap();
    // 统一 `Transform` 之后，空间种类由实体携带的**其它组件**推断
    //（`spatial_transform::entity_space`）：这条断言读的是 3D 世界变换（运行时 `Transform3D`
    // 与 Lua `transform3d.world`），因此父子都必须带 3D 专属组件，否则运行时会按 2D 落库。
    for id in [rig, child] {
        scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
    }
    scene.entities.get_mut(&rig).unwrap().components.insert(
        "Transform".into(),
        "position=10,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&child).unwrap().components.insert(
        "Transform".into(),
        "position=0,5,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );

    // ① 文档侧（编辑态，没有 ECS）。
    let document_world = rustlua_scene::scene_space::world_transform_3d(&scene, child)
        .expect("document world transform");
    assert_eq!(document_world.position, Vec3::new(10.0, 5.0, 0.0));

    // ② 运行时侧（World + 传播）。
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
    runtime.vertices(1280, 720).expect("vertices");
    let runtime_world = runtime
        .world_transform_3d_of(child)
        .expect("runtime world transform");
    assert_eq!(
        runtime_world.position, document_world.position,
        "运行时与文档的世界位置必须一致"
    );

    // ③ Lua 侧（同一实体，走 `transform3d.world`）。
    let handle = runtime
        .with_script(|script| script.scene_entity_handle(child))
        .map(rustlua_scripting::script::entity_lua_id)
        .expect("runtime entity handle");
    let source = format!(
        "function update()\n\
             local world = transform3d.world({handle})\n\
             assert(math.abs(world.x - 10) < 0.001, 'x')\n\
             assert(math.abs(world.y - 5) < 0.001, 'y')\n\
             assert(math.abs(world.z) < 0.001, 'z')\n\
             local local_pose = transform3d.get({handle})\n\
             assert(math.abs(local_pose.x) < 0.001, 'get 仍是局部值')\n\
             return 'ok' end",
    );
    let path = std::env::temp_dir().join(format!(
        "rustlua-three-way-{}-{}.lua",
        std::process::id(),
        handle
    ));
    std::fs::write(&path, source).expect("write lua script");
    runtime
        .with_script(|script| script.execute_project_script(&path))
        .expect("execute script");
    assert_eq!(
        runtime
            .with_script(|script| script.call_update())
            .expect("lua update"),
        "ok"
    );
    let _ = std::fs::remove_file(path);
}

/// docs/54 §8-②：「运行时只有一份权威 LocalTransform」在加载期就要可见：
/// 坏 `parent` 边（悬空/自环）在加载期就被点名——不该只在"开启继承后逐帧传播"时才浮现。
///
/// （第 57 轮起"一个实体同时带 `Transform` 与 `Transform3D`"这件事**无法再表达**：
/// 变换只剩一个组件名，因此那条点位随之删除，见 `scene_space`。）
#[test]
fn loading_a_scene_reports_broken_parent_edges() {
    let root = game_root();
    let mut scene = Scene::new("parent-audit");
    let dangling = rustlua_scene::scene::SceneEntityId(2);
    scene.add_entity(dangling, "Dangling", None).unwrap();
    scene
        .entities
        .get_mut(&dangling)
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    // 手改成悬空父：`Scene::add_entity` 会拒绝这种边，因此只能直接写字段。
    scene.entities.get_mut(&dangling).unwrap().parent =
        Some(rustlua_scene::scene::SceneEntityId(999));

    let runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
    assert_eq!(
        runtime.scene_parent_issues(),
        vec!["Dangling → 不存在的父 999".to_owned()],
        "悬空父必须点名"
    );
}

/// docs/54 §4.2：`RenderView::visibility` 的实体侧**真的被消费**——被遮掉的实体不产顶点，
/// 且计数能区分"被遮罩拦住"与"根本没提取到"。
#[test]
fn view_visibility_mask_filters_world_sprites() {
    use rustlua_render::visibility::ALL_LAYERS;
    let root = game_root();
    let masked = rustlua_scene::scene::SceneEntityId(1);
    let plain = rustlua_scene::scene::SceneEntityId(2);
    let mut scene = Scene::new("visibility-sprite");
    for (id, name) in [(masked, "Masked"), (plain, "Plain")] {
        scene.add_entity(id, name, None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        // 放在默认 2D 相机中心（`DEFAULT_2D_CAMERA_CENTER`）附近：否则它们会被
        // **视口外剔除**挡掉，测出来的 0 顶点就不是遮罩造成的了。
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Sprite".into(),
            "texture=assets/textures/red-1x1.png;size=2x2;pivot=0.500,0.500;\
                 tint=1.000,1.000,1.000,1.000;sorting_layer=default;order=0"
                .into(),
        );
    }
    scene
        .entities
        .get_mut(&masked)
        .unwrap()
        .components
        .insert("Visibility".into(), "layers=0b10".into());
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");

    // 不声明视图遮罩 ⇒ 全可见，且没有任何实体被拦下（既有行为与像素不变）。
    assert_eq!(runtime.view_visibility(), (ALL_LAYERS, ALL_LAYERS));
    assert_eq!(
        runtime.vertices(320, 180).expect("vertices").vertices.len(),
        12,
        "两个精灵各 6 个顶点"
    );
    assert_eq!(runtime.view_filtered(), 0);

    // 世界视图只收 0b01：`layers=0b10` 的精灵被拦下，**没有声明 Visibility 的实体照画**。
    runtime.set_view_visibility(0b01, ALL_LAYERS);
    assert_eq!(
        runtime.vertices(320, 180).expect("vertices").vertices.len(),
        6,
        "被遮罩拦下的精灵不该产顶点"
    );
    assert_eq!(runtime.take_view_filtered(), 1, "被拦下的实体数要能报出来");
    assert_eq!(runtime.take_view_filtered(), 0, "取走即清空");

    // 只收 0b10：声明为 0b10 的实体可见；未声明的实体（全可见）同样可见。
    runtime.set_view_visibility(0b10, ALL_LAYERS);
    assert_eq!(
        runtime.vertices(320, 180).expect("vertices").vertices.len(),
        12
    );
}

/// docs/54 §4.2：3D 网格吃**世界**视图的遮罩（与 2D 世界内容同一条规则）。
#[test]
fn view_visibility_mask_filters_3d_meshes() {
    use rustlua_render::visibility::ALL_LAYERS;
    let root = game_root();
    let camera = rustlua_scene::scene::SceneEntityId(1);
    let mesh = rustlua_scene::scene::SceneEntityId(2);
    let mut scene = Scene::new("visibility-mesh");
    scene.add_entity(camera, "Cam", None).unwrap();
    scene.add_entity(mesh, "Box", None).unwrap();
    scene.entities.get_mut(&camera).unwrap().components.insert(
        "Camera".into(),
        "projection=perspective;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100".into(),
    );
    scene.entities.get_mut(&mesh).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene
        .entities
        .get_mut(&mesh)
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
    scene
        .entities
        .get_mut(&mesh)
        .unwrap()
        .components
        .insert("Visibility".into(), "layers=0b10".into());
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");

    let visible = runtime
        .scene_3d(Viewport::new(1.0, 1.0).unwrap())
        .expect("mesh visible by default");
    assert_eq!(visible.drawables.len(), 1);

    runtime.set_view_visibility(0b01, ALL_LAYERS);
    assert!(
        runtime.scene_3d(Viewport::new(1.0, 1.0).unwrap()).is_none(),
        "被世界遮罩排除的网格不该出现在提取结果里"
    );
    assert_eq!(runtime.take_view_filtered(), 1);

    runtime.set_view_visibility(0b10, ALL_LAYERS);
    assert_eq!(
        runtime
            .scene_3d(Viewport::new(1.0, 1.0).unwrap())
            .expect("mesh visible again")
            .drawables
            .len(),
        1
    );
}

/// docs/54 §4.2：实体身份的 UI 吃 **overlay** 遮罩（世界遮罩管不着它），
/// 而按名字注册的画布 UI 不是实体、不受遮罩影响。
#[test]
fn view_visibility_mask_filters_entity_backed_ui_only() {
    use rustlua_render::visibility::ALL_LAYERS;
    let root = game_root();
    let panel = rustlua_scene::scene::SceneEntityId(1);
    let mut scene = Scene::new("visibility-ui");
    scene.add_entity(panel, "Panel", None).unwrap();
    let entity = scene.entities.get_mut(&panel).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity.components.insert(
        "Image".into(),
        "texture=__white__;color=1.000,0.000,1.000".into(),
    );
    entity.components.insert(
        "UiLayout".into(),
        "anchors=0.5,0.5,0.5,0.5;offsets=-32.00,-32.00,32.00,32.00;\
             min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.5000,0.5000"
            .into(),
    );
    entity
        .components
        .insert("Visibility".into(), "layers=0b01".into());
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");

    let all = runtime.vertices(320, 180).expect("vertices");
    assert_eq!(all.vertices.len(), 6, "一个 UI 面板 = 一个四边形");

    // 世界遮罩排除它、overlay 遮罩收它 ⇒ 仍然要画（UI 归 overlay 管）。
    runtime.set_view_visibility(0b10, ALL_LAYERS);
    assert_eq!(
        runtime.vertices(320, 180).expect("vertices").vertices.len(),
        6,
        "UI 不受世界遮罩影响"
    );

    // overlay 遮罩排除它 ⇒ 不产顶点，并被计数。
    runtime.set_view_visibility(ALL_LAYERS, 0b10);
    assert!(
        runtime
            .vertices(320, 180)
            .expect("vertices")
            .vertices
            .is_empty()
    );
    assert_eq!(runtime.take_view_filtered(), 1);
}

/// docs/54 §8-⑥：3D collider 调试绘制必须真的把**物理世界当前姿态**投影成 overlay 线框，
/// 且用与 3D 主视图同一份投影数学（`picking::screen_from_world_unclipped`）。
#[test]
fn collider_debug_draw_3d_projects_the_live_physics_pose() {
    let root = game_root();
    let mut scene = Scene::new("collider-debug-3d");
    let cam = rustlua_scene::scene::SceneEntityId(1);
    let wall = rustlua_scene::scene::SceneEntityId(2);
    scene.add_entity(cam, "Cam", None).unwrap();
    scene.add_entity(wall, "Wall", None).unwrap();
    scene.entities.get_mut(&cam).unwrap().components.insert(
        "Camera".into(),
        "projection=perspective;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100".into(),
    );
    scene.entities.get_mut(&wall).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&wall).unwrap().components.insert(
        "Collider3d".into(),
        "shape=cuboid;half_width=1;half_height=1;half_z=1;dynamic=false".into(),
    );
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
    let overlay_draws = |mesh: &SceneMesh| -> usize {
        mesh.draws
            .iter()
            .filter(|draw| draw.render_layer == RenderLayer::Overlay2D)
            .count()
    };

    // 关着的时候一条调试线都没有（这个场景里没有精灵/UI，因此顶点缓冲整体为空）。
    let off = runtime.vertices(320, 180).expect("vertices");
    assert_eq!(overlay_draws(&off), 0, "默认不画调试线框");
    assert!(off.vertices.is_empty(), "该场景没有其它 2D 内容");

    // 打开：固定体的 12 条棱各 6 个顶点，合成**一个** draw range（同一张白纹理）。
    runtime.set_collider_debug_draw_3d(true);
    let on = runtime.vertices(320, 180).expect("vertices");
    assert_eq!(overlay_draws(&on), 1, "调试线框合成一个 draw range");
    assert_eq!(on.vertices.len(), 12 * 6, "cuboid = 12 条棱 × 6 顶点");
    for vertex in &on.vertices {
        let [x, y] = vertex.position;
        assert!(x.is_finite() && y.is_finite(), "投影必须有限");
        assert!(
            x.abs() <= 1.0 && y.abs() <= 1.0,
            "居中的单位立方体必须投到视口内：({x}, {y})"
        );
    }
    // 关掉之后立刻回到"没有线"——开关是每帧的，不留残影。
    runtime.set_collider_debug_draw_3d(false);
    let off_again = runtime.vertices(320, 180).expect("vertices");
    assert_eq!(overlay_draws(&off_again), 0);
}
