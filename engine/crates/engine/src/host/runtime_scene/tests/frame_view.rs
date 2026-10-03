//! 原 `mod sdf_tests` 里 `frame_view` 这一域（WP12 阶段 1 纯搬运；源行段 8952–9431）。
//!
//! - `serialized_and_runtime_consistency_demo3d`
//! - `runtime_camera_pose_follows_transform_when_declared`
//! - `scene_3d_with_camera_override_keeps_scene_data_intact`
//! - `frame_view_scene`
//! - `one_frame_resolves_the_active_3d_camera_once`
//! - `a_different_viewport_rebakes_without_resolving_the_camera_again`
//! - `local_transform_write_invalidates_the_frame_view`
//! - `script_steps_invalidate_the_frame_view`

use super::*;
use crate::core::camera::ledger;
use crate::core::scene::Scene;

/// 同一输入 scene 在静态 serialized extractor 与初始 runtime world 上：
/// 每个 serialized 3D 实体都能找到同名、同 mesh path、同 transform 的
/// runtime drawable；活动相机/光源与 serialized 的首个相机/光源一致
/// （无 Lua 时 runtime 取首个组件实体）。物理/脚本/动画 palette 属运行态
/// 差异，不在本测试断言逐帧相等。
#[test]
fn serialized_and_runtime_consistency_demo3d() {
    let root = game_root();
    if !root.join("scenes/demo3d.scene").is_file() {
        return;
    }
    let path = root.join("scenes/demo3d.scene");
    let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load demo3d: {e}"));
    let serialized = crate::core::scene_extract::extract_serialized_scene_3d(&scene);
    let runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime demo3d");
    let runtime3d = runtime
        .scene_3d(Viewport::new(16.0, 9.0).unwrap())
        .expect("3d scene");

    // serialized extractor 应有可断言的有效实体；合法性一致时无诊断。
    assert_eq!(serialized.entities.len(), runtime3d.drawables.len());
    assert!(serialized.diagnostics.is_empty());
    for serialized_entity in &serialized.entities {
        let drawable = runtime3d
            .drawables
            .iter()
            .find(|d| d.name == serialized_entity.name)
            .unwrap_or_else(|| panic!("runtime 缺少同名实体 {}", serialized_entity.name));
        assert_eq!(
            drawable.mesh_path, serialized_entity.mesh.mesh,
            "{} 的 mesh path 应一致",
            serialized_entity.name
        );
        for (row, col) in (0..4).flat_map(|r| (0..4).map(move |c| (r, c))) {
            let runtime_value = drawable.model.columns[col][row];
            let serialized_value = serialized_entity.model.columns[col][row];
            assert!(
                (runtime_value - serialized_value).abs() < 1e-4,
                "{} model[{col}][{row}] 不一致: {runtime_value} vs {serialized_value}",
                serialized_entity.name
            );
        }
    }

    // 活动相机 = 首个 Camera3D 实体（from_scene_edit 无 Lua 选择），与 serialized 首个相机
    // **逐字段相同**：拆分批 7 之后宿主不往相机里写任何量（此前会把传入的 16:9 写回
    // `aspect_ratio`，于是"组件里存的比例"与"渲染/拾取实际用的比例"成了两份状态）。
    assert!(!serialized.cameras.is_empty(), "demo3d 应有相机");
    let serialized_camera = &serialized.cameras[0].1;
    let runtime_camera = &runtime3d.camera;
    assert_eq!(runtime_camera.position, serialized_camera.position);
    assert_eq!(runtime_camera.target, serialized_camera.target);
    assert_eq!(runtime_camera.up, serialized_camera.up);
    assert!((runtime_camera.fov_y - serialized_camera.fov_y).abs() < 1e-6);
    assert!((runtime_camera.near - serialized_camera.near).abs() < 1e-6);
    assert!((runtime_camera.far - serialized_camera.far).abs() < 1e-6);
    assert_eq!(
        runtime_camera.pose_from_transform,
        serialized_camera.pose_from_transform
    );
    assert_eq!(runtime_camera.enabled, serialized_camera.enabled);
    // 比例的唯一去处是**本帧视图**：传入 16:9 视口 ⇒ 视图比例就是 16:9，相机载荷一字未改。
    assert!(
        (runtime3d.view.aspect() - 16.0 / 9.0).abs() < 1e-6,
        "视图比例必须来自视口：{}",
        runtime3d.view.aspect()
    );

    // 活动光源 = 首个 Light 实体，与 serialized 首个光源一致。
    assert!(!serialized.lights.is_empty(), "demo3d 应有光源");
    let serialized_light = &serialized.lights[0].1;
    let runtime_light = &runtime3d.light;
    assert_eq!(runtime_light.direction, serialized_light.direction);
    assert_eq!(runtime_light.color, serialized_light.color);
    assert_eq!(runtime_light.intensity, serialized_light.intensity);
}

/// WP5：运行时的相机姿态也可以来自实体的 `Transform3D`（`pose=transform`）。
/// 同一场景里放两台相机（一台跟随变换、一台 look-at），验证**只有声明的那台**改姿态。
#[test]
fn runtime_camera_pose_follows_transform_when_declared() {
    let root = game_root();
    let mut scene = crate::core::scene::Scene::new("camera-pose");
    let posed = crate::core::scene::SceneEntityId(1);
    let plain = crate::core::scene::SceneEntityId(2);
    let mesh = crate::core::scene::SceneEntityId(3);
    scene.add_entity(posed, "PosedCamera", None).unwrap();
    scene.add_entity(plain, "PlainCamera", None).unwrap();
    scene.add_entity(mesh, "Mesh", None).unwrap();
    // `scene_3d` 以 drawable 为准：没有网格实体时直接返回 None，放一个网格当锚点。
    scene.entities.get_mut(&mesh).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene
        .entities
        .get_mut(&mesh)
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
    scene.entities.get_mut(&posed).unwrap().components.insert(
        "Transform".into(),
        "position=20,5,0;rotation=0,0.7071068,0,0.7071068;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&posed).unwrap().components.insert(
        "Camera".into(),
        "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=transform"
            .into(),
    );
    scene.entities.get_mut(&plain).unwrap().components.insert(
        "Camera".into(),
        "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
    );
    let runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let extracted = runtime
        .scene_3d(Viewport::new(16.0, 9.0).unwrap())
        .expect("3d scene");
    // 活动相机 = 排序序第一个（实体 1，即声明了 pose=transform 的那台）。
    let camera = extracted.camera;
    assert!(
        (camera.position.x - 20.0).abs() < 1e-3,
        "姿态应来自 Transform3D 的位置：{:?}",
        camera.position
    );
    // 绕 Y 转 90° ⇒ 视线朝 -X。
    let forward = (camera.target - camera.position).normalized().unwrap();
    assert!(
        (forward - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-3,
        "视线应来自变换的 -Z：{forward:?}"
    );
    // 未声明的那台（实体 2）不在 `Scene3d` 里——运行帧只带活动相机；它的 look-at 姿态
    // 由 `scene_extract` 的对拍测试覆盖（那里两台相机都在列表中，只有声明的那台被改姿态）。
    assert_eq!(extracted.drawables.len(), 1, "只有网格实体是 drawable");

    // 对照组：同样的 `Transform3D`，但相机没有声明 `pose=transform`，姿态必须保持组件值。
    let control_camera = crate::core::scene::SceneEntityId(1);
    let control_mesh = crate::core::scene::SceneEntityId(2);
    let mut control = crate::core::scene::Scene::new("camera-look-at");
    control
        .add_entity(control_camera, "PlainCamera", None)
        .unwrap();
    control.add_entity(control_mesh, "Mesh", None).unwrap();
    control
        .entities
        .get_mut(&control_camera)
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=20,5,0;rotation=0,0.7071068,0,0.7071068;scale=1,1,1".into(),
        );
    control
        .entities
        .get_mut(&control_camera)
        .unwrap()
        .components
        .insert(
            "Camera".into(),
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
        );
    control
        .entities
        .get_mut(&control_mesh)
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    control
        .entities
        .get_mut(&control_mesh)
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
    let control_runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), control).expect("runtime scene");
    let control_camera = control_runtime
        .scene_3d(Viewport::new(16.0, 9.0).unwrap())
        .expect("3d scene")
        .camera;
    assert!(
        (control_camera.position - Vec3::new(0.0, 0.0, 6.0)).length() < 1e-3,
        "未声明 pose=transform 时位置必须保持组件值：{:?}",
        control_camera.position
    );
    assert!(
        control_camera.target.length() < 1e-3,
        "未声明 pose=transform 时 target 必须保持组件值：{:?}",
        control_camera.target
    );
}

/// M11-04 / R-05：编辑器 3D 视图相机覆盖（`scene_3d_with_camera`）不修改场景
/// 数据——传 `Some` 仅覆盖渲染视角，传 `None` 走 `active_camera3d`；覆盖前后
/// drawables 一致，且覆盖后 `scene_3d` 仍回退到场景相机（证明覆盖是瞬时的视图参数，
/// 未写入 ECS / 脚本状态）。
///
/// 拆分批 7：覆盖相机也**不被改写**——比例只进本帧视图（`view.aspect()`），相机载荷
/// 与传入值逐字段相同（此前 `aspect_ratio` 会被覆写成传入比例）。
#[test]
fn scene_3d_with_camera_override_keeps_scene_data_intact() {
    let root = game_root();
    if !root.join("scenes/demo3d.scene").is_file() {
        return;
    }
    let path = root.join("scenes/demo3d.scene");
    let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load demo3d: {e}"));
    let runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime demo3d");

    let before = runtime
        .scene_3d(Viewport::new(16.0, 9.0).unwrap())
        .expect("3d scene");
    let override_camera = Camera3D {
        position: Vec3::new(10.0, 20.0, 30.0),
        target: Vec3::new(-5.0, -6.0, -7.0),
        up: Vec3::Z,
        fov_y: 0.9,
        near: 0.5,
        far: 200.0,
        pose_from_transform: false,
        enabled: true,
    };
    let overridden = runtime
        .scene_3d_with_camera(Viewport::new(4.0, 3.0).unwrap(), Some(override_camera))
        .expect("3d scene");
    // 覆盖仅作用于**本帧视图**：相机载荷逐字段保持覆盖值，比例来自视口而不是相机。
    assert_eq!(overridden.camera.position, Vec3::new(10.0, 20.0, 30.0));
    assert_eq!(overridden.camera.target, Vec3::new(-5.0, -6.0, -7.0));
    assert!((overridden.camera.fov_y - 0.9).abs() < 1e-6);
    assert_eq!(
        overridden.camera, override_camera,
        "宿主不得改写覆盖相机（拆分批 7 之前它会在这里被写回 aspect）"
    );
    assert!((overridden.view.aspect() - 4.0 / 3.0).abs() < 1e-6);
    assert_ne!(
        overridden.view.matrix(),
        before.view.matrix(),
        "不同视口比例必须给出不同的投影矩阵"
    );
    assert_eq!(
        overridden.view.matrix(),
        Some(
            override_camera
                .view_projection_matrix(Viewport::new(4.0, 3.0).unwrap())
                .unwrap()
        ),
        "视图矩阵必须等于按视口比例现算的那一份"
    );
    // drawables 不受相机覆盖影响：数量与每项名称/mesh path 均一致。
    assert_eq!(overridden.drawables.len(), before.drawables.len());
    for (a, b) in overridden.drawables.iter().zip(before.drawables.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.mesh_path, b.mesh_path);
    }
    // 无覆盖路径 = scene_3d 路径（相机/光源/资源/视图一致），覆盖后场景数据未变。
    let after = runtime
        .scene_3d(Viewport::new(16.0, 9.0).unwrap())
        .expect("3d scene");
    assert_eq!(after.camera, before.camera);
    assert_eq!(after.light.direction, before.light.direction);
    assert_eq!(after.drawables.len(), before.drawables.len());
    assert_eq!(
        after.view.matrix(),
        before.view.matrix(),
        "覆盖是瞬时的：回到无覆盖路径必须给出同一份视图矩阵"
    );
}

/// 拆分批 8：**一帧只解析一次生效 3D 相机**，渲染提取与 Lua 回填因此是同一次解析的产物。
///
/// 这是"同源靠构造、不靠调用顺序"的账：批 8 之前同一帧里 `vertices()`（回填 Lua）与
/// `scene_3d()`（渲染提取）各解析一次，两者一致靠的是"两步之间没有代码改过世界"这个隐含
/// 约定；现在账本上的数字是 1。
#[test]
fn one_frame_resolves_the_active_3d_camera_once() {
    let root = game_root();
    let scene = frame_view_scene(
        "frame-view-once",
        None,
        "projection=perspective;position=1,2,3;target=0,0,0;fov=60;near=0.1;far=100",
    );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let viewport = Viewport::new(640.0, 360.0).unwrap();

    ledger::reset();
    let _mesh = runtime.vertices(640, 360).expect("vertices");
    let frame = runtime.frame_view_3d().expect("vertices() 应已定帧");
    let scene3d = runtime.scene_3d(viewport).expect("3d scene");

    let counts = ledger::snapshot();
    assert_eq!(
        counts.camera3d_resolutions, 1,
        "一帧只该解析一次生效相机（定帧），实际 {}",
        counts.camera3d_resolutions
    );
    assert_eq!(
        counts.view_3d, 1,
        "3D 视图只该烘焙一次（定帧），实际 {}",
        counts.view_3d
    );
    // 定帧值就是渲染提取用的那一份：相机逐字段相同、矩阵逐位相同、比例来自视口。
    assert_eq!(scene3d.camera, runtime.active_camera3d());
    assert_eq!(scene3d.view.matrix(), frame.matrix());
    assert!((scene3d.view.aspect() - viewport.aspect()).abs() < 1e-6);
    assert_eq!(scene3d.view.viewport, viewport);
    // 相机确实是场景里声明的那台，而不是回退默认相机。
    assert!((scene3d.camera.position - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-6);
}

/// 拆分批 8：视口不同 ⇒ **换烘焙、不换相机**（相机不依赖视口，再解析一次只会多一趟遍历）。
#[test]
fn a_different_viewport_rebakes_without_resolving_the_camera_again() {
    let root = game_root();
    let scene = frame_view_scene(
        "frame-view-viewport",
        None,
        "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100",
    );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");

    ledger::reset();
    let _ = runtime.vertices(640, 360).expect("vertices");
    let square = Viewport::new(480.0, 480.0).unwrap();
    let scene3d = runtime.scene_3d(square).expect("3d scene");
    let counts = ledger::snapshot();
    assert_eq!(counts.camera3d_resolutions, 1, "换视口不该再解析一次相机");
    assert_eq!(
        counts.view_3d, 2,
        "第二个视口要单独烘焙一次（1 次定帧 + 1 次重烘焙）"
    );
    assert!(
        (scene3d.view.aspect() - 1.0).abs() < 1e-6,
        "比例必须来自传入视口"
    );
    assert_eq!(scene3d.view.viewport, square);
    assert_eq!(
        scene3d.view.matrix(),
        Some(
            scene3d
                .camera
                .view_projection_matrix(square)
                .expect("matrix")
        ),
        "重烘焙必须等价于按该视口现算"
    );
}

/// 拆分批 8（负向）：定帧值**不能盖过**此后发生的相机相关世界变更。
///
/// 这是引入缓存后最危险的一条路径：宿主若在 `vertices()` 之后用 `set_local_transform` 挪动
/// 带 `pose=transform` 的相机（编辑器属性面板就是这么做的），渲染必须看到**新**姿态。
#[test]
fn local_transform_write_invalidates_the_frame_view() {
    let root = game_root();
    let mut scene = frame_view_scene(
        "frame-view-stale-transform",
        None,
        "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=transform",
    );
    // `pose=transform`：姿态来自实体自己的 `Transform`（组件名 `Transform` 解出的就是 3D 变换）。
    scene
        .entities
        .get_mut(&crate::core::scene::SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,1,6;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let viewport = Viewport::new(640.0, 360.0).unwrap();

    ledger::reset();
    let _ = runtime.vertices(640, 360).expect("vertices");
    let pinned = runtime.scene_3d(viewport).expect("3d scene");
    assert!(
        (pinned.camera.position - Vec3::new(0.0, 1.0, 6.0)).length() < 1e-3,
        "定帧时相机应取 Transform 的 (0,1,6)：{:?}",
        pinned.camera.position
    );

    // 帧内改姿态：定帧值必须作废。
    let moved = runtime
        .set_local_transform(
            crate::core::scene::SceneEntityId(1),
            crate::core::spatial_transform::LocalTransform {
                translation: Vec3::new(0.0, 1.0, 60.0),
                rotation: crate::core::math3d::Quat::from_axis_angle(Vec3::Y, 0.0).unwrap(),
                scale: Vec3::new(1.0, 1.0, 1.0),
            },
        )
        .expect("local transform write");
    assert!(moved, "写局部变换应成功");
    assert!(
        runtime.frame_view_3d().is_none(),
        "写局部变换之后定帧值必须作废"
    );
    let after = runtime.scene_3d(viewport).expect("3d scene");
    assert!(
        (after.camera.position - Vec3::new(0.0, 1.0, 60.0)).length() < 1e-3,
        "渲染必须看到新姿态，而不是定帧时的旧值：{:?}",
        after.camera.position
    );
}

/// 拆分批 8（负向）：脚本步进之后定帧值作废——脚本可能换掉生效相机或移动相机父链。
#[test]
fn script_steps_invalidate_the_frame_view() {
    let root = game_root();
    let scene = frame_view_scene(
        "frame-view-stale-steps",
        Some("projection=perspective;position=0,0,9;target=0,0,0;fov=45;near=0.1;far=100"),
        "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100",
    );
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");

    ledger::reset();
    let _ = runtime.vertices(640, 360).expect("vertices");
    assert!(runtime.frame_view_3d().is_some(), "定帧后应有本帧视图");
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("script step");
    assert!(
        runtime.frame_view_3d().is_none(),
        "步进之后定帧值必须作废（脚本可能换掉生效相机）"
    );
    let counts = ledger::snapshot();
    assert_eq!(counts.frame_view_invalidations, 1);
    // 作废之后的消费重新定帧：这一帧因此是 2 次解析（定帧 1 次 + 作废后 1 次），这是
    // "宁可多解析一次，也不拿步进前的相机去渲染步进后的世界"的代价，账本上看得见。
    let _ = runtime
        .scene_3d(Viewport::new(640.0, 360.0).unwrap())
        .expect("3d scene");
    assert_eq!(ledger::snapshot().camera3d_resolutions, 2);
}
