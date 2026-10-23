//! P3-E01-2：`AnimationPlayer` 的运行时接线——装配时读 `.anim`、每帧采样写回、
//! Sprite 区域只在提取时覆盖、事件进 Lua 队列、`play/pause/stop/seek` 生效。

use super::*;
use rustlua_animation::animation_player::AnimationPlayer;
use rustlua_ecs::transform::Transform;
use rustlua_scene::scene::Scene;

fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rustlua-anim-player-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("assets/anim")).expect("temp assets");
    root
}

fn player_entity(runtime: &RuntimeScene) -> rustlua_ecs::entity::EntityId {
    let mut found = None;
    runtime.script.with_world(|world| {
        world.for_each::<AnimationPlayer>(|id, _| found = Some(id));
    });
    found.expect("scene entity should carry AnimationPlayer")
}

fn player_time(runtime: &RuntimeScene) -> f32 {
    let mut time = None;
    runtime.script.with_world(|world| {
        world.for_each::<AnimationPlayer>(|_, player| time = Some(player.time()));
    });
    time.expect("player")
}

/// 2D：位置轨写回 `Transform`，`Sprite.region` 只在提取时改 UV（组件保持作者数据），
/// 命名事件进 Lua 队列；`pause/play/seek` 从 Lua 侧真的改变采样结果。
#[test]
fn two_d_player_samples_transform_regions_and_events() {
    let root = temp_root("2d");
    std::fs::write(
        root.join("assets/anim/walk.anim"),
        "\
version=1
name=walk
duration=1
looped=true
track=Transform.position;interp=linear;keys=0:640,360;1:700,360
track=Sprite.region;interp=step;keys=0:0;0.5:1
event=0.5:step
",
    )
    .expect("write clip");

    let mut scene = Scene::new("player_2d");
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
            "AnimationPlayer".into(),
            "clip=assets/anim/walk.anim;frames=2".into(),
        );
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let id = player_entity(&runtime);

    runtime
        .run_script_steps(1, 0.25, &rustlua_input::input::InputSnapshot::default())
        .expect("step");
    let (position, sprite) = runtime.script.with_world(|world| {
        (
            world
                .get::<Transform>(id)
                .unwrap()
                .expect("2d transform")
                .position,
            *world
                .get::<rustlua_render::sprite::Sprite>(id)
                .unwrap()
                .unwrap(),
        )
    });
    assert!((position.x - 655.0).abs() < 1e-4 && (position.y - 360.0).abs() < 1e-4);
    assert_eq!(
        (sprite.uv_min.x, sprite.uv_max.x),
        (0.0, 1.0),
        "作者写的 UV 不能被播放器改写"
    );

    let mesh = runtime.vertices(640, 360).expect("vertices");
    let us: Vec<f32> = mesh.vertices.iter().map(|vertex| vertex.uv[0]).collect();
    assert!(us.contains(&0.0) && us.contains(&0.5), "帧 0 的 UV：{us:?}");
    assert!(!us.contains(&1.0), "帧 0 不该用到右半张图：{us:?}");

    let events = runtime
        .script
        .eval_json("animation.player_events()")
        .expect("player events");
    let empty = match events.as_array() {
        Some(array) => array.is_empty(),
        None => events.as_object().is_some_and(|object| object.is_empty()),
    };
    assert!(empty, "0.25s 不该有事件：{events}");

    // pause → 时间不再前进。
    let lua_id = id.to_lua_i64();
    runtime
        .script
        .eval_json(&format!("animation.pause({lua_id})"))
        .expect("pause");
    runtime
        .run_script_steps(1, 0.25, &rustlua_input::input::InputSnapshot::default())
        .expect("paused step");
    assert!(
        (player_time(&runtime) - 0.25).abs() < 1e-4,
        "暂停后时间不动"
    );

    // play → 继续推进，跨过 0.5 触发事件并切到帧 1。
    runtime
        .script
        .eval_json(&format!("animation.play({lua_id})"))
        .expect("play");
    runtime
        .run_script_steps(1, 0.25, &rustlua_input::input::InputSnapshot::default())
        .expect("resumed step");
    let events = runtime
        .script
        .eval_json("animation.player_events()")
        .expect("player events");
    let entries = events.as_array().expect("event array");
    assert_eq!(entries.len(), 1, "{events}");
    assert_eq!(entries[0]["name"], serde_json::json!("step"));
    assert_eq!(entries[0]["time"], serde_json::json!(0.5));
    let mesh = runtime.vertices(640, 360).expect("vertices");
    let us: Vec<f32> = mesh.vertices.iter().map(|vertex| vertex.uv[0]).collect();
    assert!(us.contains(&0.5) && us.contains(&1.0), "帧 1 的 UV：{us:?}");

    // seek 直接定位采样（不要求正在播放）。
    runtime
        .script
        .eval_json(&format!("animation.seek({lua_id}, 0.75)"))
        .expect("seek");
    runtime
        .run_script_steps(1, 0.0, &rustlua_input::input::InputSnapshot::default())
        .expect("seek step");
    let position = runtime.script.with_world(|world| {
        world
            .get::<Transform>(id)
            .unwrap()
            .expect("transform")
            .position
    });
    assert!((position.x - 685.0).abs() < 1e-4, "seek 0.75 → x=685");

    let _ = std::fs::remove_dir_all(&root);
}

/// 3D：`Transform3D.*` 与 `Light.intensity` 写回；循环折回；`looped=false` 覆盖片段后
/// 停在末帧并置 `ended`。
#[test]
fn three_d_player_samples_transform_and_light_and_respects_loop_override() {
    let root = temp_root("3d");
    std::fs::write(
        root.join("assets/anim/spin.anim"),
        "\
version=1
name=spin
duration=1
looped=true
track=Transform3D.position;interp=linear;keys=0:0,0,0;1:2,0,0
track=Light.intensity;interp=step;keys=0:1;0.5:3
",
    )
    .expect("write clip");

    let mut scene = Scene::new("player_3d");
    scene.add_entity(SceneEntityId(1), "Sun", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Light".into(),
            "direction=0.4,-0.8,0.5;color=1,1,1;intensity=1".into(),
        );
        entity.components.insert(
            "AnimationPlayer".into(),
            "clip=assets/anim/spin.anim;autoplay=true".into(),
        );
    }
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let id = player_entity(&runtime);

    runtime
        .run_script_steps(1, 0.6, &rustlua_input::input::InputSnapshot::default())
        .expect("step");
    let (position, intensity) = runtime.script.with_world(|world| {
        (
            world
                .get::<Transform3D>(id)
                .unwrap()
                .expect("3d transform")
                .position,
            world
                .get::<rustlua_render::light::Light>(id)
                .unwrap()
                .expect("light")
                .intensity,
        )
    });
    assert!(
        (position.x - 1.2).abs() < 1e-4,
        "0.6s → x=1.2：{position:?}"
    );
    assert!((intensity - 3.0).abs() < 1e-4, "0.5s 后灯强 3");

    // 再推 0.6s：1.2 折回 0.2，灯强回到 1，位置 x=0.4。
    runtime
        .run_script_steps(1, 0.6, &rustlua_input::input::InputSnapshot::default())
        .expect("loop step");
    let (position, intensity) = runtime.script.with_world(|world| {
        (
            world.get::<Transform3D>(id).unwrap().unwrap().position,
            world
                .get::<rustlua_render::light::Light>(id)
                .unwrap()
                .unwrap()
                .intensity,
        )
    });
    assert!((position.x - 0.4).abs() < 1e-4, "折回 0.2s：{position:?}");
    assert!((intensity - 1.0).abs() < 1e-4, "折回后灯强回到段首");

    // `looped=false` 覆盖片段自身的 looped=true：停在 1.0s、ended 置位。
    let mut scene = Scene::new("player_3d_once");
    scene.add_entity(SceneEntityId(1), "Sun", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Light".into(),
            "direction=0.4,-0.8,0.5;color=1,1,1;intensity=1".into(),
        );
        entity.components.insert(
            "AnimationPlayer".into(),
            "clip=assets/anim/spin.anim;looped=false".into(),
        );
    }
    let mut once =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
    let once_id = player_entity(&once);
    once.run_script_steps(1, 2.0, &rustlua_input::input::InputSnapshot::default())
        .expect("once step");
    let (time, playing, ended, position) = once.script.with_world(|world| {
        let player = world.get::<AnimationPlayer>(once_id).unwrap().unwrap();
        let position = world.get::<Transform3D>(once_id).unwrap().unwrap().position;
        (player.time(), player.is_playing(), player.ended(), position)
    });
    assert_eq!(time, 1.0);
    assert!(!playing && ended, "非循环片段播完即停");
    assert!((position.x - 2.0).abs() < 1e-4, "停在末键");

    let _ = std::fs::remove_dir_all(&root);
}
