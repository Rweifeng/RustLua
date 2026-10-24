//! M76-02：`UiBindings` 运行时接线门。
//!
//! 用程序化场景钉住四件事：
//! - 视图模型 → `Text.text` / `Image` 可见性 / 动态进度条比例的单向 resolve；
//! - 源缺失时走 `default`，目标实体或组件缺失时给出诊断而不是 panic；
//! - 只在源变化时写目标（手动改过的文字不会被静止帧覆盖）；
//! - 双向绑定经 [`RuntimeScene::push_ui_binding`] 写回源，单向绑定拒绝写回。

use super::*;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::{Scene, SceneEntityId};
use rustlua_ui::ui_binding::{BindingValue, BoundProperty};

const BINDINGS: &str = concat!(
    "property=text.text;target=Score;source=data.score;default=0",
    "|property=image.visible;target=Icon;source=data.icon_visible",
    "|property=progress.value;target=HpBar;source=data.hp",
    "|property=text.text;target=NameInput;source=data.name;mode=two_way",
    "|property=text.text;target=NameLabel;source=data.name",
    "|property=text.text;target=Missing;source=data.none;default=fallback",
    "|property=progress.value;target=NoBar;source=data.nobar;default=0.5",
);

fn binding_runtime() -> RuntimeScene {
    let mut scene = Scene::new("ui_bindings");
    let add = |scene: &mut Scene, id: u64, name: &str, components: &[(&str, &str)]| {
        scene.add_entity(SceneEntityId(id), name, None).unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
        for (key, value) in components {
            entity.components.insert((*key).into(), (*value).into());
        }
    };
    add(&mut scene, 1, "View", &[("UiBindings", BINDINGS)]);
    add(
        &mut scene,
        2,
        "Score",
        &[
            ("Text", "text=Waiting;font_size=18;color=1,1,1"),
            ("UiLayout", "anchors=0,0,0,0;offsets=10,20,180,24"),
        ],
    );
    add(
        &mut scene,
        3,
        "Icon",
        &[
            ("Image", "texture=assets/textures/coin.png;color=1,1,1"),
            ("UiLayout", "anchors=0,0,0,0;offsets=10,60,32,32"),
            // 命中集只收带 `UiEvents`/`ButtonStates` 的实体，绑定可见性门要能观测到 Icon。
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    add(
        &mut scene,
        4,
        "Missing",
        &[
            ("Text", "text=old;font_size=18;color=1,1,1"),
            ("UiLayout", "anchors=0,0,0,0;offsets=10,100,180,24"),
        ],
    );
    add(
        &mut scene,
        5,
        "NameInput",
        &[
            ("Text", "text=;font_size=18;color=1,1,1"),
            ("UiLayout", "anchors=0,0,0,0;offsets=10,140,180,24"),
        ],
    );
    add(
        &mut scene,
        6,
        "NameLabel",
        &[
            ("Text", "text=;font_size=18;color=1,1,1"),
            ("UiLayout", "anchors=0,0,0,0;offsets=10,180,180,24"),
        ],
    );
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
    runtime
        .script
        .execute_console("ui.bar('HpBar', 10, 10, 100, 10, 1, 1, 1, 1)")
        .expect("创建动态进度条");
    runtime
}

fn text_of(runtime: &RuntimeScene, name: &str) -> Option<String> {
    let id = runtime.script.entity_handle(name)?;
    runtime.script.with_world(|world| {
        world
            .get::<UiText>(id)
            .ok()
            .flatten()
            .map(|text| text.text.clone())
    })
}

fn set_text(runtime: &mut RuntimeScene, name: &str, value: &str) {
    let id = runtime.script.entity_handle(name).expect("实体存在");
    runtime.script.with_world_mut(|world| {
        if let Ok(Some(text)) = world.get_mut::<UiText>(id) {
            text.text = value.to_owned();
        }
    });
}

fn set_view_model(runtime: &mut RuntimeScene, lua: &str) {
    runtime.script.execute_console(lua).expect("写视图模型");
}

fn step(runtime: &mut RuntimeScene) {
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .unwrap();
}

#[test]
fn one_way_bindings_write_text_visibility_and_progress() {
    let mut runtime = binding_runtime();
    set_view_model(
        &mut runtime,
        "ui.view_model_set('data.score', 12.5)\n\
         ui.view_model_set('data.icon_visible', true)\n\
         ui.view_model_set('data.hp', 0.4)",
    );
    step(&mut runtime);

    assert_eq!(text_of(&runtime, "Score").as_deref(), Some("12.5"));
    assert!(
        runtime.ui_hit_data().iter().any(|item| item.name == "Icon"),
        "visible=true 时图像仍参与命中"
    );
    runtime.script.with_ui(|_, bars| {
        assert_eq!(bars["HpBar"].ratio, 0.4, "progress.value 写到动态进度条");
    });
    // 夹具里 `NoBar` 是故意缺进度条的目标，诊断只应有它一条。
    let report = runtime.ui_binding_report();
    assert!(
        report.iter().all(|line| line.contains("NoBar")),
        "其他绑定不应产生诊断：{report:?}"
    );
}

#[test]
fn unchanged_sources_do_not_overwrite_targets() {
    let mut runtime = binding_runtime();
    set_view_model(&mut runtime, "ui.view_model_set('data.score', 7)");
    step(&mut runtime);
    assert_eq!(text_of(&runtime, "Score").as_deref(), Some("7"));

    // 目标被别处改掉、源没变：绑定不得把它改回去（只在变化时写）。
    set_text(&mut runtime, "Score", "manual");
    step(&mut runtime);
    assert_eq!(
        text_of(&runtime, "Score").as_deref(),
        Some("manual"),
        "源未变化的帧不能覆盖目标"
    );

    set_view_model(&mut runtime, "ui.view_model_set('data.score', 8)");
    step(&mut runtime);
    assert_eq!(text_of(&runtime, "Score").as_deref(), Some("8"));
}

#[test]
fn missing_source_uses_default_and_missing_progress_bar_is_reported() {
    let mut runtime = binding_runtime();
    set_view_model(&mut runtime, "ui.view_model_set('data.nobar', 0.2)");
    step(&mut runtime);

    assert_eq!(
        text_of(&runtime, "Missing").as_deref(),
        Some("fallback"),
        "源缺失必须走 default"
    );
    let report = runtime.ui_binding_report();
    assert!(
        report
            .iter()
            .any(|line| line.contains("NoBar") && line.contains("没有 ui.bar")),
        "缺少进度条要进诊断：{report:?}"
    );
}

#[test]
fn image_binding_hides_and_shows_the_target() {
    let mut runtime = binding_runtime();
    set_view_model(
        &mut runtime,
        "ui.view_model_set('data.icon_visible', false)",
    );
    step(&mut runtime);
    assert!(
        !runtime.ui_hit_data().iter().any(|item| item.name == "Icon"),
        "隐藏的图像不能再被点中"
    );

    set_view_model(&mut runtime, "ui.view_model_set('data.icon_visible', true)");
    step(&mut runtime);
    assert!(
        runtime.ui_hit_data().iter().any(|item| item.name == "Icon"),
        "重新可见后回到命中集合"
    );
}

#[test]
fn ui_animation_tracks_publish_to_view_model_and_feed_bindings() {
    let root = std::env::temp_dir().join(format!(
        "rustlua-ui-anim-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("assets/anim")).expect("temp assets");
    std::fs::write(
        root.join("assets/anim/fade.anim"),
        "\
version=1
name=fade
duration=1
looped=false
track=UiAnimation.opacity;interp=linear;keys=0:0;1:1
",
    )
    .expect("write clip");

    let mut scene = Scene::new("ui_animation");
    let add = |scene: &mut Scene, id: u64, name: &str, components: &[(&str, &str)]| {
        scene.add_entity(SceneEntityId(id), name, None).unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
        for (key, value) in components {
            entity.components.insert((*key).into(), (*value).into());
        }
    };
    add(
        &mut scene,
        1,
        "Panel",
        &[
            ("AnimationPlayer", "clip=assets/anim/fade.anim"),
            ("Image", "texture=assets/textures/coin.png;color=1,1,1"),
            ("UiLayout", "anchors=0,0,0,0;offsets=10,60,32,32"),
        ],
    );
    add(
        &mut scene,
        2,
        "Label",
        &[
            ("Text", "text=;font_size=18;color=1,1,1"),
            ("UiLayout", "anchors=0,0,0,0;offsets=10,20,180,24"),
            (
                "UiBindings",
                "property=text.text;target=Label;source=anim.Panel.opacity",
            ),
        ],
    );
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).unwrap();

    runtime
        .run_script_steps(1, 0.5, &InputSnapshot::default())
        .expect("step 0.5s");
    runtime.script.with_ui_view_model(|view_model| {
        assert_eq!(
            view_model["anim.Panel.opacity"],
            BindingValue::Number(0.5),
            "AnimationPlayer 采样后发布到 anim.<实体>.<属性>"
        );
    });

    // 下一步先跑 resolve 读上一帧发布的视图模型值。
    step(&mut runtime);
    assert_eq!(text_of(&runtime, "Label").as_deref(), Some("0.5"));
}

#[test]
fn two_way_binding_writes_back_and_one_way_rejects() {
    let mut runtime = binding_runtime();
    set_view_model(&mut runtime, "ui.view_model_set('data.name', 'Ann')");
    step(&mut runtime);
    assert_eq!(text_of(&runtime, "NameInput").as_deref(), Some("Ann"));
    assert_eq!(text_of(&runtime, "NameLabel").as_deref(), Some("Ann"));

    runtime
        .push_ui_binding(
            "NameInput",
            BoundProperty::TextText,
            BindingValue::Text("Bob".to_owned()),
        )
        .unwrap();
    runtime.script.with_ui_view_model(|view_model| {
        assert_eq!(
            view_model["data.name"],
            BindingValue::Text("Bob".to_owned()),
            "双向绑定写回源"
        );
    });
    assert_eq!(text_of(&runtime, "NameLabel").as_deref(), Some("Bob"));
    assert_eq!(text_of(&runtime, "NameInput").as_deref(), Some("Bob"));

    let error = runtime
        .push_ui_binding(
            "NameLabel",
            BoundProperty::TextText,
            BindingValue::Text("x".to_owned()),
        )
        .unwrap_err();
    assert!(
        matches!(error, rustlua_ui::ui_binding::BindingError::NotTwoWay(_)),
        "单向绑定必须拒绝写回，实际 {error:?}"
    );
}
