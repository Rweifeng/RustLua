use super::*;

/// `docs/60` §10.3 第 2 条：宿主每帧要判断"spawn 出来的 UI 布局节点变没变"。原实现是**整表 clone
/// 再 `==`**——克隆发生在比较**之前**，而这张表在 UI 预设化之后是非空的（每个面板的布局节点都在里头），
/// 于是那份拷贝每帧都付。改成比修订号之后，唯一的风险换成**修订号漏增**：漏一次，画面与命中框就停在
/// 旧版上。这条用例钉两件相反的事：三个改动入口**各加一次**，而**只读快照不加**。
#[test]
fn prefab_ui_nodes_revision_bumps_on_every_mutation_and_only_then() {
    let mut nodes = PrefabUiNodes::default();
    let start = nodes.revision();

    // 只读不许改修订号——否则宿主每帧都误判"变了"、白克隆一次，正好退回被修掉的那条。
    let snapshot = nodes.snapshot();
    assert!(snapshot.is_empty());
    assert_eq!(nodes.revision(), start, "只读快照不许改修订号");

    // 三个改动入口各加一次。删一个不存在的键也算"动过"：宁多一次同步，不漏一次。
    nodes.remove(&7);
    assert_eq!(nodes.revision(), start + 1, "remove 必须自增");
    nodes.extend(BTreeMap::new());
    assert_eq!(nodes.revision(), start + 2, "extend 必须自增");
    nodes.clear();
    assert_eq!(nodes.revision(), start + 3, "clear 必须自增");
}

#[test]
fn instantiated_scene_entities_are_shared_with_lua_movement() {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let mut scene = Scene::new("test");
    scene.add_entity(SceneEntityId(1), "Player", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Sprite".into(),
            "texture=assets/textures/red-1x1.png;size=32x32".into(),
        );

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

    let path = script(
        "function update()\nlocal p = entity.find('Player')\nassert(p ~= nil)\ntransform.move(p, 10, 5)\nreturn 'moved' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "moved");

    let player = runtime.with_world(|world| world.entities_with::<Transform>());
    assert_eq!(player.len(), 1);
    let player = player[0];
    assert!(runtime.with_world(|world| world.has::<Sprite>(player).unwrap()));
    assert!(runtime.with_world(|world| world.has::<SpriteAsset>(player).unwrap()));
    let asset =
        runtime.with_world(|world| world.get::<SpriteAsset>(player).unwrap().unwrap().clone());
    assert_eq!(asset.path(), "assets/textures/red-1x1.png");
    let moved = runtime.with_world(|world| *world.get::<Transform>(player).unwrap().unwrap());
    assert_eq!(moved.position, Vec2::new(10.0, 5.0));
    let _ = fs::remove_file(path);
}

/// **Lua 桥接读统一组件**（第 53 轮）：统一的 `Transform` 与统一的 `Camera`（两种投影）都必须能
/// 实例化，且落进世界的是对应的类型化组件（2D → `Transform` / `Camera2D`，3D → `Transform3D` / `Camera3D`；
/// 2D 还是 3D 由实体携带的其它组件推断）。坏串仍然报错——解析失败不得被静默吞掉。
#[test]
fn lua_bridge_accepts_unified_transform_and_camera_components() {
    use rustlua_camera::camera::Camera2D;
    use rustlua_camera::camera::Camera3D;
    use rustlua_ecs::transform::Transform3D;
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;

    let instantiate = |components: &[(&str, &str)]| {
        let mut scene = Scene::new("unified");
        scene.add_entity(SceneEntityId(1), "Cam", None).unwrap();
        for (name, value) in components {
            scene
                .entities
                .get_mut(&SceneEntityId(1))
                .unwrap()
                .components
                .insert((*name).to_owned(), (*value).to_owned());
        }
        LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())
            .unwrap()
            .instantiate_scene(&scene)
    };

    // 2D 实体（无 3D 专属组件）+ 统一 `Camera`（正交）。
    assert_eq!(
        instantiate(&[
            ("Transform", "position=1,2,0;rotation=0,0,0,1;scale=1,1,1"),
            (
                "Camera",
                "projection=orthographic;center=0.0000,0.0000;zoom=1.000000"
            ),
        ])
        .unwrap(),
        1
    );
    // 统一 3D 形态 + 统一 Camera（透视）。
    assert_eq!(
        instantiate(&[
            (
                "Transform",
                "position=1.000000000,2.000000000,3.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000",
            ),
            (
                "Camera",
                "projection=perspective;position=0.000000000,1.000000000,5.000000000;target=0.000000000,0.000000000,0.000000000;fov=60.0000;near=0.100000000;far=100.000000000",
            ),
        ])
        .unwrap(),
        1
    );
    // 统一 Camera（正交）。
    assert_eq!(
        instantiate(&[(
            "Camera",
            "projection=orthographic;center=1.0000,2.0000;zoom=4.000000",
        )])
        .unwrap(),
        1
    );
    // `projection` 是**必填判别字段**：缺字段、或字段体与判别不匹配，一律报错（没有"兼容写法"）。
    assert!(
        instantiate(&[("Camera", "center=1.0000,2.0000;zoom=4.000000")]).is_err(),
        "缺 projection 的 Camera 必须被拒"
    );
    assert!(
        instantiate(&[(
            "Camera",
            "projection=orthographic;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100",
        )])
        .is_err(),
        "写 orthographic 却给透视字段体必须被拒"
    );

    // 世界里的取值确实是 3D 那份（不是空壳），并且相机落到了 3D 分支。
    let mut scene = Scene::new("unified");
    scene.add_entity(SceneEntityId(1), "Cam", None).unwrap();
    {
        let components = &mut scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components;
        components.insert(
            "Transform".to_owned(),
            "position=1.000000000,2.000000000,3.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000".to_owned(),
        );
        components.insert(
            "Camera".to_owned(),
            "projection=perspective;position=0.000000000,0.000000000,5.000000000;target=0.000000000,0.000000000,0.000000000;fov=60.0000;near=0.100000000;far=100.000000000".to_owned(),
        );
    }
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);
    let entity = runtime.with_world(|world| {
        world
            .entities_with::<Transform3D>()
            .first()
            .copied()
            .expect("统一 Transform 应作为 Transform3D 落进世界")
    });
    let transform = runtime.with_world(|world| *world.get::<Transform3D>(entity).unwrap().unwrap());
    assert_eq!(
        transform.position,
        rustlua_math::math3d::Vec3::new(1.0, 2.0, 3.0)
    );
    // 世界存储里只有**一个**相机组件：投影由判别分支表达，而不是两个不同的组件类型。
    let stored = runtime
        .with_world(|world| {
            world
                .get::<rustlua_camera::camera::Camera>(entity)
                .unwrap()
                .copied()
        })
        .expect("统一 Camera 必须落进世界");
    assert_eq!(
        stored.projection(),
        rustlua_camera::camera::CameraProjection::Perspective
    );
    assert!(!runtime.with_world(|world| world.has::<Camera3D>(entity).unwrap()));
    assert!(!runtime.with_world(|world| world.has::<Camera2D>(entity).unwrap()));

    // 坏串必须报错。
    assert!(instantiate(&[("Transform", "x=oops")]).is_err());
    assert!(instantiate(&[("Transform", "position=1,2")]).is_err());
    assert!(instantiate(&[("Camera", "projection=orthographic;fov=60")]).is_err());
}

#[test]
fn runtime_spawned_entity_can_attach_world_text() {
    // 与 animation.attach 同理：`entity.spawn` 只插 Transform，运行期创建的实体
    // （伤害数字、名牌）否则无法带世界文字。
    let first = script(
        "function on_create()\n\
         local id = entity.spawn('Label', 'wt_probe')\n\
         assert(world_text.attach(id, 'text=HP;font_size=18;color=1,1,1;offset=0,-24'))\n\
         return 'ok' end\n\
         function update() return 'ok' end",
    );
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.execute_project_script(&first).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let texts =
        runtime.with_world(|world| world.entities_with::<rustlua_ui::world_text::WorldText>());
    assert_eq!(texts.len(), 1);
    let attached = runtime
        .with_world(|world| {
            world
                .get::<rustlua_ui::world_text::WorldText>(texts[0])
                .unwrap()
                .cloned()
        })
        .expect("world text component");
    assert_eq!(attached.text.text, "HP");
    assert_eq!(attached.offset, [0.0, -24.0]);
    assert!(!attached.centered);
    // 非法串、失效实体与重复 detach 都必须有确定行为。
    let second = script(
        "function update()\n\
         local id = entity.find_id('wt_probe')\n\
         local bad = pcall(world_text.attach, id, 'text=x;centered=maybe')\n\
         assert(not bad, 'malformed component must be rejected')\n\
         local gone = pcall(world_text.attach, 999999, 'text=x;font_size=12')\n\
         assert(not gone, 'dead entity must be rejected')\n\
         assert(world_text.detach(id), 'first detach removes the component')\n\
         assert(not world_text.detach(id), 'second detach is a no-op')\n\
         return 'guarded' end",
    );
    runtime.execute_project_script(&second).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "guarded");
    let _ = fs::remove_file(first);
    let _ = fs::remove_file(second);
}

/// 批 C 门：Lua 侧 `sprite.set` 的 `图集路径#区域名` 引用必须与场景装配同一解析语义
/// （sprite.get 读回纹理路径、世界组件落区域 UV、坏引用显式报错）。
#[test]
fn lua_sprite_set_resolves_atlas_region_reference() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-atlas-lua-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("assets").join("atlases")).unwrap();
    let png = root.join("assets").join("atlases").join("hero-0.png");
    let mut img = image::RgbaImage::new(4, 2);
    for (x, _y, pixel) in img.enumerate_pixels_mut() {
        *pixel = if x < 2 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 255, 0, 255])
        };
    }
    img.save(&png).unwrap();
    // 真实项目形状：`.atlas` 与 Lua 里的引用都写工程根相对路径（正斜杠）。
    fs::write(
        root.join("assets").join("atlases").join("hero.atlas"),
        "atlas_version=1\ntexture=assets/atlases/hero-0.png\nregion\twalk\t2\t0\t2\t2\n",
    )
    .unwrap();

    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.set_project_root(root.clone());
    let id = runtime.with_world_mut(|world| world.spawn());
    let lua_id = super::super::values::entity_lua_id(id);
    let texture = runtime
        .execute_console(&format!(
            "sprite.set({lua_id}, 'texture=assets/atlases/hero.atlas#walk;size=1x1') return sprite.get({lua_id}).texture"
        ))
        .unwrap();
    assert_eq!(
        texture.trim(),
        "assets/atlases/hero-0.png",
        "sprite.get 必须读回 .atlas 里写的工程根相对纹理路径"
    );
    let sprite = runtime.with_world(|world| *world.get::<Sprite>(id).unwrap().unwrap());
    assert_eq!(
        sprite.uv_min,
        rustlua_math::math2d::Vec2::new(0.5, 0.0),
        "区域 UV 必须来自 .atlas 的实际像素矩形"
    );
    assert_eq!(sprite.uv_max, rustlua_math::math2d::Vec2::new(1.0, 1.0));

    // 未知区域：显式报错，绝不静默回退成整图。
    let broken = runtime.execute_console(&format!(
        "sprite.set({lua_id}, 'texture=assets/atlases/hero.atlas#fly;size=1x1') return 'unreachable'"
    ));
    assert!(broken.is_err(), "未知区域必须显式失败");
    let _ = fs::remove_dir_all(&root);
}
