//! 原 `mod sdf_tests` 里 `scenes_and_clicks` 这一域（WP12 阶段 1 纯搬运；源行段 6174–6409）。
//!
//! - `tetris_project_runs_and_renders`
//! - `compute_click_targets_resolves_image_text_sprite`
//! - `ui_layout_component_defines_click_rect`
//! - `load_runtime_scene_uses_project_startup_scene`
//! - `ui_test_scene_emits_sdf_text_draws`

use super::*;
use rustlua_scene::scene::Scene;

/// 俄罗斯方块工程冒烟：加载场景 + 脚本，跑几步（输入驱动），验证精灵/UI
/// 渲染与 Lua 脚本不报错。
#[test]
fn tetris_project_runs_and_renders() {
    use rustlua_input::input::KeyCode;
    use rustlua_scene::scene::Scene;

    let root = rustlua_infra::fixture_games_dir().join("tetris");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let path = root.join("scenes/main.scene");
    assert!(path.is_file(), "tetris 场景缺失: {}", path.display());
    assert!(root.join("scripts/main.lua").is_file(), "tetris 脚本缺失");
    let scene = Scene::load(&path).unwrap();
    let mut runtime = RuntimeScene::from_scene(Some(&root), scene).unwrap();
    // on_create 已执行：背景面板 + 首块精灵 + 文字应渲染出 draw。
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    assert!(
        !mesh.draws.is_empty(),
        "tetris 应渲染出 draw（UI 面板/精灵/文字）"
    );
    // 关键：必须存在 white.png 的精灵 draw（方块），否则只渲染了 UI。
    let has_sprite_draw = mesh.draws.iter().any(|d| {
        d.texture > 0
            && mesh.paths.get(d.texture as usize - 1).map(|p| p.as_str())
                == Some("assets/textures/white.png")
    });
    assert!(
        has_sprite_draw,
        "应存在 white.png 精灵 draw（方块）：{:#?}",
        mesh.paths
    );
    // UI 合批：全部方块用同一纹理（white.png），应被合并成极少量 draw。
    let white_draws = mesh
        .draws
        .iter()
        .filter(|d| {
            d.texture > 0
                && mesh.paths.get(d.texture as usize - 1).map(|p| p.as_str())
                    == Some("assets/textures/white.png")
        })
        .count();
    assert!(
        white_draws <= 2,
        "同纹理 UI 方块应合批为 ≤2 个 draw（初始 4 块），实际 {white_draws}"
    );
    // 模拟按键：右移 → 旋转 → 硬降，逐帧驱动。
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyD, true);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step d");
    input.key_event(KeyCode::KeyW, true);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step w");
    input.key_event(KeyCode::Space, true);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("step space");
    // 多步后仍能渲染。
    let mesh = runtime.vertices(1280, 720).expect("vertices after steps");
    assert!(!mesh.draws.is_empty(), "多步后仍应渲染");
}

/// UiEvents 命中矩形：Image 用 UI 矩形、Text 用 SDF 字形 bbox、Sprite 用
/// 相机投影到屏幕的 AABB。验证三类实体都能得到正确的 `(名字, origin, size)`。
#[test]
fn compute_click_targets_resolves_image_text_sprite() {
    use rustlua_scene::scene::{Scene, SceneEntityId};

    let root = rustlua_infra::games_dir().join("examples");
    let mut scene = Scene::new("click");
    let add = |scene: &mut Scene, id: u64, name: &str, components: &[(&str, &str)]| {
        scene.add_entity(SceneEntityId(id), name, None).unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
        for (key, value) in components {
            entity.components.insert(key.to_string(), value.to_string());
        }
    };
    add(
        &mut scene,
        1,
        "ImgClick",
        &[
            ("Image", "texture=assets/textures/coin.png;color=1,1,1"),
            ("UiLayout", "anchors=0,0,0,0;offsets=10,20,74,52"),
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    add(
        &mut scene,
        2,
        "TxtClick",
        &[
            ("Text", "text=Go;font_size=24;color=1,1,1"),
            ("UiLayout", "anchors=0,0,0,0;offsets=5,5,5,5"),
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    add(
        &mut scene,
        3,
        "SprClick",
        &[
            (
                "Transform",
                "position=100,50,0;rotation=0,0,0,1;scale=1,1,1",
            ),
            (
                "Sprite",
                "texture=assets/textures/coin.png;size=32x32;pivot=0.5,0.5;tint=1,1,1,1",
            ),
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
    runtime.set_camera_2d(Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("default camera"));
    let targets = runtime.compute_click_targets();
    let img = targets
        .iter()
        .find(|target| target.name == "ImgClick")
        .expect("image click target");
    assert_eq!(img.origin, [10.0, 20.0], "Image origin 来自 UiLayout");
    assert_eq!(img.size, [64.0, 32.0], "Image size 来自 UiLayout");
    assert!(
        targets.iter().any(|target| target.name == "TxtClick"),
        "Text click target 存在"
    );
    let spr = targets
        .iter()
        .find(|target| target.name == "SprClick")
        .expect("sprite click target");
    // 相机在原点、viewport 1280×720：sprite(100,50) 32×32 pivot 0.5
    // → 屏幕 AABB 左上 (724, 294)、尺寸 (32, 32)。
    assert_eq!(spr.origin, [724.0, 294.0]);
    assert_eq!(spr.size, [32.0, 32.0]);
}

/// E16：命中矩形完全来自四边 Anchor+Offset 布局；Image 只携带视觉属性。
#[test]
fn ui_layout_component_defines_click_rect() {
    use rustlua_scene::scene::{Scene, SceneEntityId};

    let root = rustlua_infra::games_dir().join("examples");
    let mut scene = Scene::new("layout");
    scene
        .add_entity(SceneEntityId(1), "StretchImg", None)
        .unwrap();
    let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
    entity.components.insert(
        "Image".to_owned(),
        "texture=assets/textures/coin.png".to_owned(),
    );
    entity.components.insert(
        "UiLayout".to_owned(),
        rustlua_ui::ui_layout::UiRectLayout {
            anchors: [0.0, 0.0, 1.0, 0.0],
            offsets: [10.0, 20.0, -10.0, 80.0],
            ..rustlua_ui::ui_layout::UiRectLayout::default()
        }
        .to_component_str(),
    );
    entity.components.insert(
        "UiEvents".to_owned(),
        rustlua_ui::ui_events::UiEvents::click("on_click").to_component_str(),
    );
    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
    let targets = runtime.compute_click_targets();
    let img = targets
        .iter()
        .find(|target| target.name == "StretchImg")
        .expect("layout click target");
    // viewport 1280×720：stretch 水平 → (10, 20, 1260, 60)。
    assert_eq!(img.origin, [10.0, 20.0]);
    assert_eq!(img.size, [1260.0, 60.0]);
}

#[test]
fn load_runtime_scene_uses_project_startup_scene() {
    let root = std::env::temp_dir().join(format!("rustlua-startup-scene-{}", std::process::id()));
    let scenes = root.join("scenes");
    std::fs::create_dir_all(&scenes).unwrap();
    std::fs::write(
        root.join("project.toml"),
        "[project]\nname = \"test\"\nstartup_scene = \"scenes/custom.scene\"\n",
    )
    .unwrap();
    Scene::new("custom-startup")
        .save(scenes.join("custom.scene"))
        .unwrap();
    let scene = load_runtime_scene(Some(&root));
    assert_eq!(scene.name, "custom-startup");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn ui_test_scene_emits_sdf_text_draws() {
    let root = game_root();
    if !root.join("scenes/ui_test.scene").is_file() {
        return;
    }
    let path = root.join("scenes/ui_test.scene");
    assert!(path.is_file(), "测试场景资源缺失: {}", path.display());
    let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load ui_test: {e}"));
    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime");
    // 本用例断言的是 **SDF 字形**，因此自己钉住渲染模式，不继承工程配置
    // （`game_api/project.toml` 现为 `text_render_mode = "vector"`）。
    runtime.set_text_render_mode(TextRenderMode::Sdf);
    let mesh = runtime.vertices(640, 360).expect("vertices");
    let sdf_count = mesh.draws.iter().filter(|d| d.sdf).count();
    eprintln!(
        "ui_test: draws={} sdf={} text_entities={}",
        mesh.draws.len(),
        sdf_count,
        runtime
            .script
            .with_world(|w| w.entities_with::<UiText>().len())
    );
    eprintln!(
        "ui_test: draws={} sdf={} text_entities={}",
        mesh.draws.len(),
        sdf_count,
        runtime
            .script
            .with_world(|w| w.entities_with::<UiText>().len())
    );
    assert!(sdf_count > 0, "ui_test scene should emit SDF text draws");
    assert!(
        mesh.vertices.iter().any(|v| v.sdf == 1),
        "SDF vertices present"
    );
}
