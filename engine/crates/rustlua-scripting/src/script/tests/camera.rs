use super::*;

/// WP4 ⑧：屏幕↔世界换算必须和渲染走**同一份**投影，否则"点到哪"会与"看到哪"错位。
/// 这里断言三件事：视口中心 → 相机中心；往返一致；没渲染过就报错而不是猜。
#[test]
fn lua_camera_conversions_use_the_rendered_projection() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let camera = Camera2D::new(Vec2::new(100.0, 50.0), 2.0).unwrap();
    runtime.set_camera_view(camera, [400.0, 300.0]);

    let path = script(
        "function update()\n\
         local center = camera2d.screen_to_world(200, 150)\n\
         assert(math.abs(center[1] - 100) < 0.001, 'viewport centre maps to camera centre x')\n\
         assert(math.abs(center[2] - 50) < 0.001, 'viewport centre maps to camera centre y')\n\
         local back = camera2d.world_to_screen(center[1], center[2])\n\
         assert(math.abs(back[1] - 200) < 0.001 and math.abs(back[2] - 150) < 0.001)\n\
         local corner = camera2d.screen_to_world(0, 0)\n\
         assert(math.abs(corner[1] - 0) < 0.001 and math.abs(corner[2] - 125) < 0.001)\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");

    // 还没渲染过任何一帧：必须报错。
    let cold = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let mut cold = cold;
    let cold_path = script(
        "function update()\n\
         local ok = pcall(camera2d.screen_to_world, 1, 1)\n\
         assert(not ok, 'no rendered frame yet must be an error')\n\
         return 'guarded' end",
    );
    cold.execute_project_script(&cold_path).unwrap();
    assert_eq!(cold.call_update().unwrap(), "guarded");
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(cold_path);
}

/// docs/54 §8-⑦：3D 侧的屏↔世界投影链。断言四件事：屏幕中心 → 指向相机目标的射线；
/// `world_to_screen` 把目标投回屏幕中心；相机背后的点 `visible=false`；没渲染过就报错。
#[test]
fn lua_camera3d_ray_and_screen_conversions_share_the_rendered_projection() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 5.0, 10.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .unwrap();
    runtime.set_camera3d_view(camera, [400.0, 300.0]);

    let path = script(
        "function update()\n\
         local ray = camera3d.ray(200, 150)\n\
         assert(ray.oz > 9.9, 'origin sits on the camera')\n\
         assert(ray.dy < -0.3 and ray.dz < -0.8, 'centre ray points down-forward toward the target')\n\
         local screen = camera3d.world_to_screen(0, 0, 0)\n\
         assert(math.abs(screen.x - 200) < 0.01 and math.abs(screen.y - 150) < 0.01, 'target maps to viewport centre')\n\
         assert(screen.visible, 'target must be visible')\n\
         local behind = camera3d.world_to_screen(0, 0, 20)\n\
         assert(not behind.visible, 'point behind the camera must not be visible')\n\
         local ground = camera3d.pick_plane(200, 150, 0)\n\
         assert(ground ~= nil, 'centre ray must hit the ground plane')\n\
         assert(math.abs(ground.x) < 0.01 and math.abs(ground.z) < 0.01, 'ground hit is the origin')\n\
         local pose = camera3d.get()\n\
         assert(math.abs(pose.pz - 10) < 0.001 and math.abs(pose.width - 400) < 0.001)\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");

    // 还没渲染过任何一帧：必须报错（猜一台相机做射线会让"点不中"变成无头案）。
    let mut cold = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let cold_path = script(
        "function update()\n\
         local ok = pcall(camera3d.ray, 1, 1)\n\
         assert(not ok, 'no rendered frame yet must be an error')\n\
         return 'guarded' end",
    );
    cold.execute_project_script(&cold_path).unwrap();
    assert_eq!(cold.call_update().unwrap(), "guarded");
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(cold_path);
}

#[test]
fn camera2d_lua_api_requests_and_validates() {
    // 相机数据在宿主，Lua 侧只登记请求（set/set_zoom）与控制器配置（follow/shake）；
    // 非法参数必须报错，不能悄悄写进去。
    let first = script(
        "function on_create()\n\
         assert(camera2d.set(120, 80))\n\
         assert(camera2d.set_zoom(2))\n\
         assert(camera2d.follow(nil, {offset_x=0, offset_y=-16, dead_zone_x=8, dead_zone_y=8, smoothing=5}))\n\
         assert(camera2d.shake(6, 0.2))\n\
         return 'ok' end\n\
         function update() return 'ok' end",
    );
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.execute_project_script(&first).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    runtime.camera2d_controller(|controller| {
        assert_eq!(
            controller.offset(),
            rustlua_ecs::transform::Vec2::new(0.0, -16.0)
        );
        assert_eq!(
            controller.dead_zone(),
            rustlua_ecs::transform::Vec2::new(8.0, 8.0)
        );
        assert!((controller.smoothing() - 5.0).abs() < 1e-6);
        assert!(controller.is_shaking());
    });
    assert_eq!(
        runtime.camera2d_controller_mut(|controller| controller.take_pending_center()),
        Some(rustlua_ecs::transform::Vec2::new(120.0, 80.0))
    );
    assert_eq!(
        runtime.camera2d_controller_mut(|controller| controller.take_pending_zoom()),
        Some(2.0)
    );
    // 取走一次即清空，避免宿主每帧重复应用同一次请求。
    assert_eq!(
        runtime.camera2d_controller_mut(|controller| controller.take_pending_center()),
        None
    );
    let second = script(
        "function update()\n\
         assert(not pcall(camera2d.set, 0/0, 0), 'nan center')\n\
         assert(not pcall(camera2d.set_zoom, 0), 'zoom must be > 0')\n\
         assert(not pcall(camera2d.shake, 4, 0), 'duration must be > 0')\n\
         assert(not pcall(camera2d.follow, nil, {dead_zone_x=-1}), 'dead zone >= 0')\n\
         assert(not pcall(camera2d.follow, 999999), 'dead entity')\n\
         return 'guarded' end",
    );
    runtime.execute_project_script(&second).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "guarded");
    let _ = fs::remove_file(first);
    let _ = fs::remove_file(second);
}
#[test]
fn lua_3d_api_moves_mesh_and_switches_camera() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let mut scene = Scene::new("3d");
    scene.add_entity(SceneEntityId(1), "Camera", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Camera".into(),
            "projection=perspective;position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100".into(),
        );
    scene.add_entity(SceneEntityId(2), "Sun", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Light".into(),
            "direction=0.4,0.8,0.5;color=1,1,1;intensity=1".into(),
        );
    scene.add_entity(SceneEntityId(3), "Hero", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(3))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(3))
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=assets/models/hero.glb".into());

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 3);

    let path = script(
        "function update()\n\
         local h = entity.find('Hero')\n\
         assert(h ~= nil)\n\
         assert(mesh.get(h) == 'assets/models/hero.glb')\n\
         transform3d.move(h, 2, 3, 4)\n\
         local t = transform3d.get(h)\n\
         assert(t.x == 2 and t.y == 3 and t.z == 4)\n\
         local cam = entity.find('Camera')\n\
         assert(camera.get_active() == nil)\n\
         assert(camera.set_active(cam))\n\
         assert(camera.get_active() == cam)\n\
         local c = camera.get(cam)\n\
         assert(math.abs(c.fov - 90) < 0.001 and math.abs(c.near - 0.1) < 0.001 and c.far == 100)\n\
         local sun = entity.find('Sun')\n\
         light.set_active(sun)\n\
         light.set(sun, 0, 1, 0, 1, 0.5, 0.25, 2)\n\
         return '3d-ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "3d-ok");

    assert_eq!(
        runtime.active_camera(),
        Some(
            runtime
                .with_world(|world| { world.entities_with::<rustlua_camera::camera::Camera>()[0] })
        )
    );
    assert_eq!(
        runtime.active_light(),
        Some(runtime.with_world(|world| world.entities_with::<Light>()[0]))
    );
    let moved = runtime.with_world(|world| {
        let hero = world.entities_with::<MeshRenderer>()[0];
        *world.get::<Transform3D>(hero).unwrap().unwrap()
    });
    assert_eq!(moved.position, Vec3::new(2.0, 3.0, 4.0));
    let light = runtime.with_world(|world| {
        let sun = world.entities_with::<Light>()[0];
        *world.get::<Light>(sun).unwrap().unwrap()
    });
    assert_eq!(light.direction, Vec3::new(0.0, 1.0, 0.0));
    assert_eq!(light.intensity, 2.0);
    let _ = fs::remove_file(path);
}

/// 拆分批 9：`camera.set_active` 是**唯一**的选择链——认任何带 `Camera` 的实体，由它的
/// `projection` 决定服务哪个 pass（透视 → 3D world，正交 → 2D overlay）。
///
/// 这条测试同时钉住**契约变更**的两条（CHANGELOG「兼容契约（变更）」）：
/// ① 选中正交相机不再报错；② 对完全没有相机的实体，错误串是 `entity has no camera`。
/// 而 `camera.get` / `camera.set`（透视 payload 的读写）仍保持原来的 `entity has no camera3d`。
#[test]
fn camera_set_active_takes_any_projection() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let mut scene = Scene::new("camera-selection-unified");
    for (id, name) in [
        (SceneEntityId(1), "Cam2d"),
        (SceneEntityId(2), "Cam3d"),
        (SceneEntityId(3), "Prop"),
    ] {
        scene.add_entity(id, name, None).unwrap();
    }
    let insert = |scene: &mut Scene, id, payload: &str| {
        scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert("Camera".into(), payload.into());
    };
    insert(
        &mut scene,
        SceneEntityId(1),
        "projection=orthographic;center=640,360;zoom=1.5",
    );
    insert(
        &mut scene,
        SceneEntityId(2),
        "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100",
    );

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 3);

    let path = script(
        "function update()\n\
         local c2 = entity.find('Cam2d')\n\
         local c3 = entity.find('Cam3d')\n\
         local prop = entity.find('Prop')\n\
         assert(camera.get_active() == nil)\n\
         -- 正交相机也能被选中（批 9 之前这里报 entity has no camera3d）\n\
         assert(camera.set_active(c2))\n\
         assert(camera.get_active() == c2)\n\
         -- 透视相机照旧\n\
         assert(camera.set_active(c3))\n\
         assert(camera.get_active() == c3)\n\
         -- 完全没有相机的实体：错误串在一条选择链下是 entity has no camera\n\
         local ok, err = pcall(camera.set_active, prop)\n\
         assert(not ok)\n\
         local text = tostring(err)\n\
         assert(string.find(text, 'entity has no camera', 1, true) ~= nil, text)\n\
         -- 透视为 payload 的读写仍是 3D 专属：正交相机在这里照旧被拒\n\
         local ok_get = pcall(camera.get, c2)\n\
         assert(not ok_get)\n\
         local ok_set = pcall(camera.set, c2, 0, 0, 1, 0, 0, 0)\n\
         assert(not ok_set)\n\
         return 'unified-selection-ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "unified-selection-ok");

    let expected = runtime.with_world(|world| {
        world.entities_with::<rustlua_camera::camera::Camera>()[1] // Cam3d（id 升序第二台）
    });
    assert_eq!(runtime.active_camera(), Some(expected));
    let _ = fs::remove_file(path);
}

#[test]
fn camera_set_moves_position_target_and_fov() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let mut scene = Scene::new("cam");
    scene.add_entity(SceneEntityId(1), "Cam", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Camera".into(),
            "projection=perspective;position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100".into(),
        );
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

    let path = script(
        "function update()\n\
         local cam = entity.find('Cam')\n\
         assert(camera.set(cam, 1, 2, 3, 4, 5, 6, 45))\n\
         local c = camera.get(cam)\n\
         assert(c.x == 1 and c.y == 2 and c.z == 3)\n\
         assert(c.tx == 4 and c.ty == 5 and c.tz == 6)\n\
         assert(math.abs(c.fov - 45) < 0.001)\n\
         local bad = pcall(function() camera.set(cam, 0, 0, 0, 0, 0, 0, 0) end)\n\
         assert(not bad, 'fov=0 must be rejected')\n\
         return 'cam-ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "cam-ok");
    let cam = runtime.with_world(|world| {
        let id = world.entities_with::<rustlua_camera::camera::Camera>()[0];
        world
            .get::<rustlua_camera::camera::Camera>(id)
            .unwrap()
            .unwrap()
            .as_perspective()
            .expect("这台相机是透视的")
    });
    assert_eq!(cam.position, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(cam.target, Vec3::new(4.0, 5.0, 6.0));
    assert!((cam.fov_y.to_degrees() - 45.0).abs() < 0.001);
    let _ = fs::remove_file(path);
}
