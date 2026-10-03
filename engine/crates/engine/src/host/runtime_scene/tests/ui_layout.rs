//! 原 `mod sdf_tests` 里 `ui_layout` 这一域（WP12 阶段 1 纯搬运；源行段 5592–5804）。
//!
//! - `nine_slice_image_renders_nine_quads`
//! - `scene_ui_text_keeps_layout_origin_once`
//! - `ui_layout_preview_targets_authored_entity`
//! - `ui_layout_rects_expose_exact_preview_geometry`
//! - `ui_layout_preview_changes_rendered_mesh`
//! - `font_prewarm_from_chars_txt`

use super::*;
use crate::core::scene::Scene;
use std::fs;

/// 九宫格：带 `nine_slice` 的 Image 应渲染为 3×3 共 9 格（54 顶点），而非
/// 单四边形（6 顶点）。用真实按钮纹理（192×64）验证 UV 分片路径。
#[test]
fn nine_slice_image_renders_nine_quads() {
    use crate::core::scene::{Scene, SceneEntityId};

    let root = crate::core::games_dir().join("examples").join("game_api");
    if !root.join("assets/textures/ui/button_green.png").is_file() {
        return;
    }
    let mut scene = Scene::new("nine-slice");
    scene.add_entity(SceneEntityId(1), "Panel", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Image".to_owned(),
            "texture=assets/textures/ui/button_green.png;color=1,1,1;nine_slice=16,16,16,16"
                .to_owned(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "UiLayout".to_owned(),
            "anchors=0,0,0,0;offsets=0,0,192,64".to_owned(),
        );
    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    assert!(
        mesh.draws.iter().any(|d| d.count == 54),
        "切片 Image 应产生 54 顶点（9 格 × 6）：{:#?}",
        mesh.draws.iter().map(|d| d.count).collect::<Vec<_>>()
    );
}

/// Scene `Text` layouts already contain absolute screen coordinates. A
/// regression used to add the `UiLayout` origin a second time, pushing
/// labels below/off their buttons.
#[test]
fn scene_ui_text_keeps_layout_origin_once() {
    use crate::core::scene::{Scene, SceneEntityId};

    let font_source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
    if !font_source.is_file() {
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "rustlua-scene-text-origin-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let fonts_dir = root.join("assets").join("fonts");
    fs::create_dir_all(&fonts_dir).unwrap();
    fs::copy(&font_source, fonts_dir.join("NotoSansSC-VF.ttf")).unwrap();

    let mut scene = Scene::new("scene-text-origin");
    scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
    let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
    entity.components.insert(
        "UiLayout".to_owned(),
        "anchors=0,0,0,0;offsets=100,600,100,600".to_owned(),
    );
    entity.components.insert(
        "Text".to_owned(),
        "text=Press;font_size=48;color=1,1,1".to_owned(),
    );
    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    let _ = fs::remove_dir_all(&root);
    let text_y: Vec<f32> = mesh
        .vector_vertices
        .iter()
        .map(|vertex| vertex.position[1])
        .collect();
    assert!(!text_y.is_empty(), "scene text should emit vector vertices");
    let minimum_y = text_y.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(
        minimum_y > -1.0,
        "layout text must stay on screen, min NDC y={minimum_y}"
    );
}

/// Runtime UI layout nodes are keyed by visible-entity order, not by the
/// sparse authoring ID. Dragging entity 2 must never rewrite entity 1.
#[test]
fn ui_layout_preview_targets_authored_entity() {
    use crate::core::scene::{Scene, SceneEntityId};

    let mut scene = Scene::new("drag-preview");
    for id in [1, 2] {
        scene
            .add_entity(SceneEntityId(id), format!("Entity{id}"), None)
            .unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
        entity.components.insert(
            "Text".to_owned(),
            format!("text=Text{id};font_size=24").to_owned(),
        );
        entity.components.insert(
            "UiLayout".to_owned(),
            format!("anchors=0,0,0,0;offsets={id}0,{id}0,{id}0,{id}0"),
        );
    }
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
    assert!(runtime.set_scene_entity_ui_layout_preview(SceneEntityId(2), [50.0, 60.0, 70.0, 80.0]));
    let first = runtime.ui_scene_nodes[&0].layout.as_ref().unwrap().offsets;
    let second = runtime.ui_scene_nodes[&1].layout.as_ref().unwrap().offsets;
    assert_eq!(first, [10.0, 10.0, 10.0, 10.0]);
    assert_eq!(second, [50.0, 60.0, 70.0, 80.0]);
}

#[test]
fn ui_layout_rects_expose_exact_preview_geometry() {
    use crate::core::scene::SceneEntityId;

    let mut scene = Scene::new("drag-preview-geometry");
    scene.add_entity(SceneEntityId(1), "Image", None).unwrap();
    let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
    entity.components.insert(
        "Image".to_owned(),
        "texture=__white__;color=1,1,1".to_owned(),
    );
    entity.components.insert(
        "UiLayout".to_owned(),
        "anchors=0,0,0,0;offsets=10,20,30,40".to_owned(),
    );
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
    assert!(
        runtime.set_scene_entity_ui_layout_preview(SceneEntityId(1), [50.0, 60.0, 90.0, 100.0])
    );
    let rects = runtime.scene_entity_ui_layout_rects([SceneEntityId(1)], [1000.0, 1000.0]);
    assert_eq!(rects.get(&1), Some(&UiRect::new(50.0, 60.0, 40.0, 40.0)));
}

/// The editor drag preview must affect the next rendered mesh, not only the
/// internal layout snapshot.
#[test]
fn ui_layout_preview_changes_rendered_mesh() {
    use crate::core::scene::{Scene, SceneEntityId};

    let mut scene = Scene::new("drag-preview-render");
    for id in [1, 2] {
        scene
            .add_entity(SceneEntityId(id), format!("Entity{id}"), None)
            .unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
        entity.components.insert(
            "Image".to_owned(),
            "texture=__white__;color=1,1,1".to_owned(),
        );
        entity.components.insert(
            "UiLayout".to_owned(),
            format!("anchors=0,0,0,0;offsets={id}0,{id}0,{id}0,{id}0"),
        );
    }
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
    runtime.set_render_ui_overlay(true);
    let before = runtime.vertices(1000, 1000).unwrap();
    assert!(
        runtime.set_scene_entity_ui_layout_preview(SceneEntityId(2), [700.0, 700.0, 740.0, 740.0])
    );
    let after = runtime.vertices(1000, 1000).unwrap();
    let before_x = before
        .vertices
        .iter()
        .map(|vertex| vertex.position[0])
        .fold(f32::NEG_INFINITY, f32::max);
    let after_x = after
        .vertices
        .iter()
        .map(|vertex| vertex.position[0])
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(after_x > before_x);
}

/// 首包预热：`load_project_font_atlas` 读到项目根 `chars.txt` 时预生成字形
/// （图集 version 增加）；无 chars.txt 不预热。
#[test]
fn font_prewarm_from_chars_txt() {
    let root = std::env::temp_dir().join(format!("rustlua_font_prewarm_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let font_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/editor/fonts/NotoSansSC-VF.ttf");
    std::fs::create_dir_all(root.join("assets/fonts")).unwrap();
    std::fs::copy(&font_src, root.join("assets/fonts/font.ttf")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        "[project]\nfont = \"assets/fonts/font.ttf\"\n",
    )
    .unwrap();
    // 无 chars.txt：不预热，version 保持 0。
    let atlas = load_project_font_atlas(Some(&root)).expect("atlas");
    assert_eq!(atlas.lock().unwrap().version(), 0, "无 chars.txt 不应预热");
    // 写 chars.txt：预热后 version > 0（字形已生成）。
    std::fs::write(root.join("chars.txt"), "测试字体AB").unwrap();
    let atlas = load_project_font_atlas(Some(&root)).expect("atlas");
    assert!(atlas.lock().unwrap().version() > 0, "chars.txt 应触发预热");
    let _ = std::fs::remove_dir_all(&root);
}
