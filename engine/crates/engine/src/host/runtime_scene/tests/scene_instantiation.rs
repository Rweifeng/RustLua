//! 原 `mod sdf_tests` 里 `scene_instantiation` 这一域（WP12 阶段 1 纯搬运；源行段 8705–8951）。
//!
//! - `unmarked_v2_scenes_treat_parent_as_organizational_only`
//! - `explicit_and_invalid_spatial_declarations_are_reported`
//! - `scene_parents_become_runtime_parent_components`
//! - `render_view_2d_shares_the_runtime_camera`
//! - `runtime_drawables_use_scene_names`
//! - `sample_project_ui_scene_instantiates_ui`
//! - `sample_project_gameplay_scene_instantiates_gameplay_entities`

use super::*;
use rustlua_scene::scene::Scene;

/// docs/54 §4.1：**未声明空间语义的 v2 场景里，`parent` 只是组织关系**。
///
/// 这条是本批的核心判据：此前运行时开关默认是开的，于是任何带 `parent` 边的 v2 场景都会被
/// 自动按父子组合解释——旧场景（`parent` 只用于编辑器分组）会因此**静默改变画面**。
/// 现在默认值来自场景自身：未声明 ⇒ 不组合，子实体读到的就是它自己的局部值。
#[test]
fn unmarked_v2_scenes_treat_parent_as_organizational_only() {
    let root = game_root();
    let parent = rustlua_scene::scene::SceneEntityId(1);
    let child = rustlua_scene::scene::SceneEntityId(2);
    let mut scene = rustlua_scene::scene::Scene::new("organizational");
    scene.add_entity(parent, "Group", None).unwrap();
    scene.add_entity(child, "Member", Some(parent)).unwrap();
    scene.entities.get_mut(&parent).unwrap().components.insert(
        "Transform".into(),
        "position=10,5,0;rotation=0,0,0,1;scale=2,2,1".into(),
    );
    scene.entities.get_mut(&child).unwrap().components.insert(
        "Transform".into(),
        "position=1,2,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    let runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");

    assert_eq!(
        runtime.spatial_mode(),
        rustlua_scene::scene_space::SpatialMode::Organizational,
        "未声明时场景语义应是组织关系"
    );
    assert!(
        !runtime.spatial_inheritance(),
        "未声明的 v2 场景默认不该建立父子空间继承"
    );
    let child_world = runtime
        .world_transform_2d_of(child)
        .expect("child world transform");
    assert!(
        (child_world.position.x - 1.0).abs() < 1e-6
            && (child_world.position.y - 2.0).abs() < 1e-6
            && (child_world.scale.x - 1.0).abs() < 1e-6,
        "组织关系下子实体的世界值就是它自己的局部值：{child_world:?}"
    );
    // 但 `parent` 边本身仍然落到 ECS 上（层级信息不丢，只是不参与空间组合）。
    let has_parent = runtime.script.with_world(|world| {
        world
            .entities_with::<rustlua_ecs::hierarchy::Parent>()
            .len()
    });
    assert_eq!(has_parent, 1, "组织关系不等于丢掉父子边");
}

/// docs/54 §4.1：显式声明与非法声明各有一条出口——`spatial=organizational` 等同未声明，
/// 认不出的值按默认处理但**会被点名**（不静默吞掉）。
#[test]
fn explicit_and_invalid_spatial_declarations_are_reported() {
    let mut scene = rustlua_scene::scene::Scene::new("declared");
    scene
        .metadata
        .insert("spatial".into(), "organizational".into());
    assert_eq!(
        rustlua_scene::scene_space::spatial_mode(&scene),
        rustlua_scene::scene_space::SpatialMode::Organizational
    );
    assert!(rustlua_scene::scene_space::spatial_mode_issue(&scene).is_none());
    scene.metadata.insert("spatial".into(), "inherit".into());
    assert_eq!(
        rustlua_scene::scene_space::spatial_mode(&scene),
        rustlua_scene::scene_space::SpatialMode::Inherit
    );
    scene.metadata.insert("spatial".into(), "world".into());
    assert_eq!(
        rustlua_scene::scene_space::spatial_mode(&scene),
        rustlua_scene::scene_space::SpatialMode::Organizational,
        "认不出的值按 v2 默认处理"
    );
    let issue = rustlua_scene::scene_space::spatial_mode_issue(&scene).expect("非法取值必须被点名");
    assert!(issue.contains("spatial=world"), "{issue}");
}

/// WP2：运行时的 `Parent` 组件来自场景的 `parent`（父子关系真的落到了 ECS 上）。
#[test]
fn scene_parents_become_runtime_parent_components() {
    let root = game_root();
    let mut scene = rustlua_scene::scene::Scene::new("parents");
    let parent = rustlua_scene::scene::SceneEntityId(1);
    let child = rustlua_scene::scene::SceneEntityId(2);
    let orphan = rustlua_scene::scene::SceneEntityId(3);
    scene.add_entity(parent, "Parent", None).unwrap();
    scene.add_entity(child, "Child", Some(parent)).unwrap();
    scene.add_entity(orphan, "Orphan", None).unwrap();
    for (id, x) in [(parent, 0.0_f32), (child, 4.0), (orphan, 8.0)] {
        scene.entities.get_mut(&id).unwrap().components.insert(
            "Transform".into(),
            format!("position={x},0,0;rotation=0,0,0,1;scale=1,1,1"),
        );
    }
    let runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let with_parent = runtime.script.with_world(|world| {
        world
            .entities_with::<rustlua_ecs::hierarchy::Parent>()
            .len()
    });
    assert_eq!(with_parent, 1, "只有子实体带 Parent");
}

/// WP5：宿主交给渲染器的 2D overlay 视图必须与 `vertices()` 用**同一个相机**——
/// 收敛前"取景"（相机）与"清屏/视口"（渲染器写死）是两套说法，这里把它钉住。
#[test]
fn render_view_2d_shares_the_runtime_camera() {
    let mut scene = Scene::new("render-view");
    scene.add_entity(SceneEntityId(1), "Empty", None).unwrap();
    let runtime = RuntimeScene::from_scene_edit(None, scene).expect("runtime");
    let mut runtime = runtime;
    runtime.set_camera_2d(Camera2D::new(Vec2::new(120.0, 80.0), 2.0).expect("camera"));

    let view = runtime.render_view_2d(640, 360);
    assert_eq!(view.viewport, Viewport::new(640.0, 360.0).unwrap());
    assert_eq!(
        view.projection,
        rustlua_camera::camera::Projection::Orthographic { zoom: 2.0 },
        "视图的投影必须来自运行时相机的 zoom"
    );
    // 2D overlay 是叠加层：不清屏（3D 主视图才清屏）。
    assert_eq!(view.clear, rustlua_camera::camera::ClearPolicy::None);
    assert!((view.aspect() - 640.0 / 360.0).abs() < 1e-6);
    assert_eq!(
        runtime.camera_2d().center,
        runtime.camera_center_2d(),
        "视图相机与宿主相机是同一份状态"
    );

    // 0 尺寸（最小化窗口）不能造出非法视口。
    assert_eq!(
        runtime.render_view_2d(0, 0).viewport,
        Viewport::new(1.0, 1.0).unwrap()
    );
}

/// 运行态 3D drawable 使用序列化场景的实体名称（而非 `Entity {index}`）；
/// 未命名的脚本实体才回退到索引名。
#[test]
fn runtime_drawables_use_scene_names() {
    let root = game_root();
    if !root.join("scenes/demo3d.scene").is_file() {
        return;
    }
    let path = root.join("scenes/demo3d.scene");
    let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load demo3d: {e}"));
    let runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime demo3d");
    let scene3d = runtime
        .scene_3d(Viewport::new(16.0, 9.0).unwrap())
        .expect("3d scene");
    let mut names: Vec<&str> = scene3d.drawables.iter().map(|d| d.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["Ground", "ShadowTriangle", "SkeletonWarrior"]);
    assert!(
        scene3d
            .drawables
            .iter()
            .all(|d| !d.name.starts_with("Entity ")),
        "drawable 名称应来自场景实体名，而非索引"
    );
}

/// 示例工程的 **场景 UI 场景**（`ui_test.scene`）要真的产出可见 UI draw，
/// 并且场景侧的 UI 组件（`UiEvents` / `Image` / `Text`）都在运行时世界里。
#[test]
fn sample_project_ui_scene_instantiates_ui() {
    let root = production_sample_root();
    if !root.join("scenes/ui_test.scene").is_file() {
        return;
    }
    let path = root.join("scenes/ui_test.scene");
    let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load ui_test: {e}"));
    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime ui_test");
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    let counts = runtime.script.with_world(|world| {
        (
            world
                .entities_with::<rustlua_ui::ui_events::UiEvents>()
                .len(),
            world.entities_with::<UiImage>().len(),
            world.entities_with::<UiText>().len(),
        )
    });
    assert!(
        counts.0 >= 2,
        "ui_test 场景的按钮是 UiEvents 实体，实际 {}",
        counts.0
    );
    assert!(counts.1 >= 1, "ui_test 场景应有 Image 实体");
    assert!(counts.2 >= 3, "ui_test 场景应有标题/按钮/HUD 文本");
    assert!(
        !mesh.draws.is_empty(),
        "ui_test 场景应产出可见的 2D/UI draw"
    );
}

/// 示例工程的 **玩法场景**（`api_gameplay.scene`）要带着它的地块、演员与世界文字
/// 实例化出来，并真的渲染：这是"场景 + 脚本 + 渲染"三层的端到端样本。
#[test]
fn sample_project_gameplay_scene_instantiates_gameplay_entities() {
    let root = production_sample_root();
    if !root.join("scenes/api_gameplay.scene").is_file() {
        return;
    }
    let path = root.join("scenes/api_gameplay.scene");
    let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load api_gameplay: {e}"));
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime api_gameplay");
    let input = InputSnapshot::default();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("script step");
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    let counts = runtime.script.with_world(|world| {
        (
            world.entities_with::<rustlua_ecs::tilemap::Tilemap>().len(),
            world
                .entities_with::<rustlua_render::sprite::SpriteAsset>()
                .len(),
            world
                .entities_with::<rustlua_ui::world_text::WorldText>()
                .len(),
            world
                .entities_with::<rustlua_ui::ui_events::UiEvents>()
                .len(),
        )
    });
    assert_eq!(counts.0, 1, "玩法场景应有一块地块（Tilemap）");
    assert!(counts.1 >= 3, "玩法场景应有玩家/箱子/商人等演员");
    assert!(counts.2 >= 1, "玩法场景应有世界文字（FieldSign）");
    assert_eq!(counts.3, 0, "玩法场景不声明场景 UI 事件");
    assert!(
        runtime.texture_paths.len() >= 3,
        "玩法场景应加载地块/玩家/史莱姆纹理，实际 {:?}",
        runtime.texture_paths
    );
    assert!(
        mesh.draws.len() >= 3,
        "玩法场景应渲染地块与演员，实际 {} 个 draw",
        mesh.draws.len()
    );
}
