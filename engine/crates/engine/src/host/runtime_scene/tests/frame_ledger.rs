//! 原 `mod sdf_tests` 里 `frame_ledger` 这一域（WP12 阶段 1 纯搬运；源行段 9432–9811）。
//!
//! - `scene_3d_without_a_frame_still_resolves_on_demand`
//! - `frame_bake_ledger_is_pinned`
//! - `camera2d_selection_scene`
//! - `camera_set_active_on_an_orthographic_camera_adopts_the_2d_payload`
//! - `camera_set_active_on_a_disabled_2d_camera_falls_back`
//! - `camera_selection_serves_each_pass_by_projection`
//! - `runtime_scene_create_step_drop_100_cycles`

use super::*;
use rustlua_camera::camera::ledger;
use rustlua_scene::scene::Scene;

/// 拆分批 8：宿主**没走** `vertices()` 就先调 `scene_3d`（示例/golden 里有这种顺序）时，
/// 没有定帧值 ⇒ 按需解析 + 烘焙，行为与定帧前逐字节相同。
#[test]
fn scene_3d_without_a_frame_still_resolves_on_demand() {
    let root = game_root();
    let scene = frame_view_scene(
        "frame-view-on-demand",
        None,
        "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100",
    );
    let runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");

    ledger::reset();
    let viewport = Viewport::new(16.0, 9.0).unwrap();
    let extracted = runtime.scene_3d(viewport).expect("3d scene");
    let counts = ledger::snapshot();
    assert_eq!(counts.camera3d_resolutions, 1);
    assert_eq!(counts.view_3d, 1);
    assert_eq!(
        extracted.view.matrix(),
        Some(
            extracted
                .camera
                .view_projection_matrix(viewport)
                .expect("matrix")
        )
    );
    assert!((extracted.view.aspect() - 16.0 / 9.0).abs() < 1e-6);
}

/// 拆分批 8 的**账本门**：把"一帧里相机被投影/烘焙了几次"变成确定的数字。
///
/// 这不是性能断言（测试里的时间不可信），而是**口径断言**：docs/54 批 3 写的是"宿主每帧
/// 烘焙一次"，现实是"每个消费者每帧各烘焙一次"（overlay 批、瓦片层、世界文字、宿主的
/// overlay 视图各自一份 2D 预算项）。数字写在这里，任何增减都得有人来解释：是有意，还是回退。
#[test]
fn frame_bake_ledger_is_pinned() {
    let root = game_root();
    let mut scene = rustlua_scene::scene::Scene::new("frame-bake-ledger");
    let sprite = rustlua_scene::scene::SceneEntityId(1);
    let tiles = rustlua_scene::scene::SceneEntityId(2);
    let text = rustlua_scene::scene::SceneEntityId(3);
    let camera = rustlua_scene::scene::SceneEntityId(4);
    let mesh = rustlua_scene::scene::SceneEntityId(5);
    for (id, name) in [
        (sprite, "Sprite"),
        (tiles, "Tiles"),
        (text, "Text"),
        (camera, "Camera"),
        (mesh, "Mesh"),
    ] {
        scene.add_entity(id, name, None).unwrap();
    }
    let payload = |scene: &mut rustlua_scene::scene::Scene, id, key: &str, value: &str| {
        scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert(key.into(), value.into());
    };
    payload(
        &mut scene,
        sprite,
        "Transform",
        "position=100,100,0;rotation=0,0,0,1;scale=1,1,1",
    );
    payload(
        &mut scene,
        sprite,
        "Sprite",
        "texture=assets/textures/red-1x1.png;size=2x2;pivot=0.5,0.5;z=0",
    );
    payload(
        &mut scene,
        tiles,
        "Transform",
        "position=640,360,0;rotation=0,0,0,1;scale=1,1,1",
    );
    payload(
        &mut scene,
        tiles,
        "Tilemap",
        "tile_size=32x32;width=2;height=2;texture=assets/textures/tileset.png;tiles=1111;solid=0000",
    );
    payload(
        &mut scene,
        text,
        "Transform",
        "position=640,400,0;rotation=0,0,0,1;scale=1,1,1",
    );
    payload(
        &mut scene,
        text,
        "WorldText",
        "text=HP;font_size=24;color=1,1,1;offset=0,-20;centered=false",
    );
    payload(
        &mut scene,
        camera,
        "Camera",
        "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100",
    );
    payload(
        &mut scene,
        mesh,
        "Transform",
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1",
    );
    payload(
        &mut scene,
        mesh,
        "MeshRenderer",
        "mesh=assets/models/test.glb",
    );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let viewport = Viewport::new(640.0, 360.0).unwrap();

    // 一帧典型路径：宿主提取 2D 顶点 → 声明 2D overlay 视图 → 提取 3D 场景。
    ledger::reset();
    let _ = runtime.vertices(640, 360).expect("vertices");
    let _ = runtime.render_view_2d(640, 360);
    let _ = runtime.scene_3d(viewport).expect("3d scene");
    let counts = ledger::snapshot();

    // 3D 侧（拆分批 8 的成果）：相机解析 1 次、视图烘焙 1 次——渲染提取复用定帧值。
    assert_eq!(counts.camera3d_resolutions, 1);
    assert_eq!(counts.view_3d, 1);
    // 2D 侧：每个消费者各烘焙一次自己的预算项——① overlay 批 ② 瓦片层 ③ 世界文字
    // ④ 宿主的 overlay 视图（`render_view_2d`，它还会烘焙一份完整 2D 视图）。
    // 这 4 次是**有意保留**的：单次烘焙是常数级（validate + sin_cos + 几次乘法），4 次远低于
    // 一帧预算，改签名做帧级共享的收益不成比例（docs/54 §12 批 8 有记录）。
    assert_eq!(counts.view_2d, 1, "宿主的 overlay 视图（render_view_2d）");
    assert_eq!(
        counts.ortho_terms, 4,
        "2D 预算项：overlay 批 + 瓦片层 + 世界文字 + render_view_2d 内部各一次，实际 {}",
        counts.ortho_terms
    );
}

/// 拆分批 9 的夹具：两台正交相机（`Cam2dA` 在前）+ 一个精灵，可选把 A 设为 `enabled=false`；
/// `with_3d` 时再加一台透视相机（`Cam3d`）与一个网格，让 `scene_3d` 有内容。
fn camera2d_selection_scene(
    name: &str,
    disable_a: bool,
    with_3d: bool,
) -> rustlua_scene::scene::Scene {
    let mut scene = rustlua_scene::scene::Scene::new(name);
    let a = rustlua_scene::scene::SceneEntityId(1);
    let b = rustlua_scene::scene::SceneEntityId(2);
    let sprite = rustlua_scene::scene::SceneEntityId(3);
    for (id, entity) in [(a, "Cam2dA"), (b, "Cam2dB"), (sprite, "Sprite")] {
        scene.add_entity(id, entity, None).unwrap();
    }
    let enabled = if disable_a { ";enabled=false" } else { "" };
    scene.entities.get_mut(&a).unwrap().components.insert(
        "Camera".into(),
        format!("projection=orthographic;center=100,100;zoom=2{enabled}"),
    );
    scene.entities.get_mut(&b).unwrap().components.insert(
        "Camera".into(),
        "projection=orthographic;center=500,500;zoom=0.5".into(),
    );
    let entity = scene.entities.get_mut(&sprite).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=100,100,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity.components.insert(
        "Sprite".into(),
        "texture=assets/textures/red-1x1.png;size=16x16;pivot=0.5,0.5;z=0".into(),
    );
    if with_3d {
        let camera3d = rustlua_scene::scene::SceneEntityId(4);
        let mesh = rustlua_scene::scene::SceneEntityId(5);
        scene.add_entity(camera3d, "Cam3d", None).unwrap();
        scene.add_entity(mesh, "Mesh", None).unwrap();
        scene
            .entities
            .get_mut(&camera3d)
            .unwrap()
            .components
            .insert(
                "Camera".into(),
                "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
            );
        let entity = scene.entities.get_mut(&mesh).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity
            .components
            .insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
    }
    scene
}

/// 拆分批 9：`camera.set_active` 是**一条**选择链——选中的正交相机就是 2D 相机的来源，
/// 宿主在**选择变化**时采用一次它的载荷（center/zoom）；同一台不重复采用（否则每帧会把
/// 控制器算出的取景抹平），而选中**透视**相机时 2D 取景必须原地不动（切 3D 相机是常见操作）。
#[test]
fn camera_set_active_on_an_orthographic_camera_adopts_the_2d_payload() {
    let root = game_root();
    let mut runtime = RuntimeScene::from_scene_edit(
        Some(root.as_path()),
        camera2d_selection_scene("camera2d-select", false, true),
    )
    .expect("runtime scene");
    // 默认（无选中）：排序序第一台启用的正交相机 ⇒ Cam2dA。
    assert_eq!(runtime.camera.center, Vec2::new(100.0, 100.0));
    assert_eq!(runtime.camera.zoom, 2.0);

    // 选中 Cam2dB（正交）⇒ 采用它的载荷。
    runtime
        .with_script(|script| {
            script.execute_console("return camera.set_active(entity.find('Cam2dB'))")
        })
        .expect("select Cam2dB");
    runtime.step_camera2d(1.0 / 60.0);
    assert_eq!(
        runtime.camera.center,
        Vec2::new(500.0, 500.0),
        "选中正交相机后必须采用它的载荷"
    );
    assert_eq!(runtime.camera.zoom, 0.5);

    // 同一台再"选中"一次不重复采用：把相机挪走（控制器或宿主写入）之后不得被盖回去。
    runtime.set_camera_2d(Camera2D {
        center: Vec2::new(42.0, 24.0),
        ..runtime.camera
    });
    runtime
        .with_script(|script| {
            script.execute_console("return camera.set_active(entity.find('Cam2dB'))")
        })
        .expect("re-select Cam2dB");
    for _ in 0..3 {
        runtime.step_camera2d(1.0 / 60.0);
    }
    assert_eq!(
        runtime.camera.center,
        Vec2::new(42.0, 24.0),
        "同一台重复选中不该把控制器算出的取景抹平"
    );

    // 选中**透视**相机（切 3D 取景）：2D 取景必须原地不动。
    runtime
        .with_script(|script| {
            script.execute_console("return camera.set_active(entity.find('Cam3d'))")
        })
        .expect("select Cam3d");
    for _ in 0..2 {
        runtime.step_camera2d(1.0 / 60.0);
    }
    assert_eq!(
        runtime.camera.center,
        Vec2::new(42.0, 24.0),
        "切 3D 相机不该把 2D 相机弹回作者值"
    );

    // 再选回 Cam2dB ⇒ 来源变了，重新采用它的载荷（这就是"把 2D 取景复位回场景声明"的表达）。
    runtime
        .with_script(|script| {
            script.execute_console("return camera.set_active(entity.find('Cam2dB'))")
        })
        .expect("select Cam2dB again");
    runtime.step_camera2d(1.0 / 60.0);
    assert_eq!(runtime.camera.center, Vec2::new(500.0, 500.0));
}

/// 拆分批 9：选中的正交相机**被禁用**时告警并回退到下一台可用相机（与 3D 侧选中禁用相机
/// 同一规则、同一告警形状）。
#[test]
fn camera_set_active_on_a_disabled_2d_camera_falls_back() {
    let root = game_root();
    let mut runtime = RuntimeScene::from_scene_edit(
        Some(root.as_path()),
        camera2d_selection_scene("camera2d-disabled", true, false),
    )
    .expect("runtime scene");
    // A 被禁用 ⇒ 初始取景来自 B（"第一台被禁用不该挡掉后面启用的那台"）。
    assert_eq!(runtime.camera.center, Vec2::new(500.0, 500.0));

    // 显式选中被禁用的 A：必须回退（而不是采用它的载荷）。
    runtime
        .with_script(|script| {
            script.execute_console("return camera.set_active(entity.find('Cam2dA'))")
        })
        .expect("select disabled Cam2dA");
    runtime.step_camera2d(1.0 / 60.0);
    assert_eq!(
        runtime.camera.center,
        Vec2::new(500.0, 500.0),
        "选中被禁用的 2D 相机必须回退到下一台可用相机（B）"
    );
    assert_eq!(runtime.camera.zoom, 0.5);
}

/// 拆分批 9（负向）：一条选择链上，**投影决定服务哪个 pass**——选中正交相机不得改变 3D pass
/// 用的相机，反之选中透视相机也不得改变 2D 取景。
#[test]
fn camera_selection_serves_each_pass_by_projection() {
    let root = game_root();
    let mut runtime = RuntimeScene::from_scene_edit(
        Some(root.as_path()),
        camera2d_selection_scene("camera2d-vs-3d", false, true),
    )
    .expect("runtime scene");
    let viewport = Viewport::new(640.0, 360.0).unwrap();
    let before = runtime.scene_3d(viewport).expect("3d scene").camera;
    let framing_before = runtime.camera;

    // 选中正交相机：3D pass 不受影响（候选按投影过滤）。
    runtime
        .with_script(|script| {
            script.execute_console("return camera.set_active(entity.find('Cam2dB'))")
        })
        .expect("select Cam2dB");
    runtime.step_camera2d(1.0 / 60.0);
    let after = runtime.scene_3d(viewport).expect("3d scene").camera;
    assert_eq!(after, before, "选中正交相机不得改变 3D pass 用的相机");
    assert_eq!(
        runtime.camera.center,
        Vec2::new(500.0, 500.0),
        "2D 侧确实换了相机（否则这条测试没有区分力）"
    );

    // 反过来：选中透视相机，2D 取景必须保持（不是回到作者值）。
    runtime
        .with_script(|script| {
            script.execute_console("return camera.set_active(entity.find('Cam3d'))")
        })
        .expect("select Cam3d");
    for _ in 0..3 {
        runtime.step_camera2d(1.0 / 60.0);
    }
    assert_eq!(runtime.camera.center, Vec2::new(500.0, 500.0));
    assert_ne!(framing_before.center, runtime.camera.center);
    let after_3d = runtime.scene_3d(viewport).expect("3d scene").camera;
    assert_eq!(after_3d, before, "Cam3d 是唯一透视相机，3D pass 仍用它");
}

/// RuntimeScene 无 GPU 长会话：重复 create → run_script_steps → vertices → drop，
/// 断言无 panic、帧顶点数/纹理路径容器有界（含字体图集反复加载/释放）。
#[test]
fn runtime_scene_create_step_drop_100_cycles() {
    let root = game_root();
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let path = root.join("scenes/main.scene");
    assert!(path.is_file(), "测试场景资源缺失: {}", path.display());
    let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load main: {e}"));
    let input = InputSnapshot::default();
    for cycle in 0..100 {
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(&root), scene.clone()).expect("runtime main");
        for _ in 0..3 {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &input)
                .expect("script step");
        }
        let mesh = runtime.vertices(640, 360).expect("vertices");
        assert!(
            mesh.vertices.len() < 1_000_000,
            "cycle {cycle}: 顶点数应有界: {}",
            mesh.vertices.len()
        );
        assert!(
            runtime.texture_paths.len() < 512,
            "cycle {cycle}: 纹理路径容器应有界: {}",
            runtime.texture_paths.len()
        );
        // drop 释放 Lua/世界/字体资源。
    }
}
