//! 原 `mod sdf_tests` 里 `custom_fonts` 这一域（WP12 阶段 1 纯搬运；源行段 9812–10020）。
//!
//! - `scene_text_custom_font_surfaces_in_mesh`
//! - `text_render_mode_forces_vector_or_sdf_for_custom_fonts`
//! - `text_render_mode_parsing_uses_auto_fallback`
//! - `sourcehan_sdf_mode_emits_default_font_draws`

use super::*;
use crate::core::scene::Scene;
use std::fs;

/// 场景 Text 指定自定义字体时：该字体 key 出现在 SceneMesh 的自定义图集
/// 上报与纹理路径里；默认文本在短文本场景改走矢量路径。用临时项目根存放
/// 两个字体文件（同源，仅验证管线/命名空间，不验证字形差异）。
#[test]
fn scene_text_custom_font_surfaces_in_mesh() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
    if !source.is_file() {
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "rustlua-custom-font-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let fonts_dir = root.join("assets").join("fonts");
    let _ = fs::create_dir_all(&fonts_dir);
    fs::copy(&source, fonts_dir.join("NotoSansSC-VF.ttf")).expect("copy default font");
    fs::copy(&source, fonts_dir.join("Custom.ttf")).expect("copy custom font");

    let mut scene = Scene::new("custom-fonts");
    scene
        .add_entity(SceneEntityId(1), "DefaultText", None)
        .unwrap();
    scene
        .add_entity(SceneEntityId(2), "CustomText", None)
        .unwrap();
    let layout = "anchors=0,0,0,0;offsets=0,0,64,16".to_owned();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("UiLayout".into(), layout.clone());
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Text".into(), "text=Default;font_size=24".into());
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert("UiLayout".into(), layout);
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Text".into(),
            "text=Custom;font_size=24;font=assets/fonts/Custom.ttf".into(),
        );

    let result = RuntimeScene::from_scene_edit(Some(&root), scene)
        .and_then(|mut runtime| runtime.vertices(320, 240));
    let _ = fs::remove_dir_all(&root);
    let mesh = result.expect("vertices");
    assert!(
        mesh.custom_font_atlases
            .iter()
            .any(|font| font.key == "font://assets/fonts/Custom.ttf"),
        "custom font key should be reported to the renderer"
    );
    let paths: Vec<&str> = mesh.paths.iter().map(|path| path.as_str()).collect();
    assert!(
        paths.contains(&"font://assets/fonts/Custom.ttf"),
        "custom font path should be in mesh paths: {paths:?}"
    );
    assert!(
        !paths.contains(&FONT_ATLAS_TEXTURE_PATH),
        "default short text should use the vector path: {paths:?}"
    );
    assert!(
        mesh.vector_vertices.len() >= 4,
        "default short text should emit vector vertices"
    );
}

#[test]
fn text_render_mode_forces_vector_or_sdf_for_custom_fonts() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
    if !source.is_file() {
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "rustlua-text-render-mode-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let fonts_dir = root.join("assets").join("fonts");
    fs::create_dir_all(&fonts_dir).unwrap();
    fs::copy(&source, fonts_dir.join("Custom.ttf")).unwrap();

    let mut scene = Scene::new("render-mode");
    scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "UiLayout".into(),
            "anchors=0,0,0,0;offsets=20,20,240,80".into(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Text".into(),
            "text=Mode;font_size=48;font=assets/fonts/Custom.ttf".into(),
        );

    let project = |mode: &str| {
        fs::write(
                root.join("project.toml"),
                format!(
                    "[project]\nfont = \"assets/fonts/NotoSansSC-VF.ttf\"\ntext_render_mode = \"{mode}\"\n"
                ),
            )
            .unwrap();
    };
    project("vector");
    let vector = RuntimeScene::from_scene_edit(Some(&root), scene.clone())
        .and_then(|mut runtime| runtime.vertices(320, 240))
        .expect("vector vertices");
    assert!(!vector.vector_vertices.is_empty());
    assert!(
        vector.custom_font_atlases.is_empty(),
        "forced vector mode should not build custom SDF atlases"
    );

    project("sdf");
    let sdf = RuntimeScene::from_scene_edit(Some(&root), scene)
        .and_then(|mut runtime| runtime.vertices(320, 240))
        .expect("sdf vertices");
    assert!(sdf.vector_vertices.is_empty());
    assert!(
        sdf.custom_font_atlases
            .iter()
            .any(|font| font.key == "font://assets/fonts/Custom.ttf")
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn text_render_mode_parsing_uses_auto_fallback() {
    assert_eq!(TextRenderMode::parse("auto"), TextRenderMode::Auto);
    assert_eq!(TextRenderMode::parse(" Vector "), TextRenderMode::Vector);
    assert_eq!(TextRenderMode::parse("MSDF"), TextRenderMode::Sdf);
    assert_eq!(TextRenderMode::parse("bad"), TextRenderMode::Auto);
}

#[test]
fn sourcehan_sdf_mode_emits_default_font_draws() {
    let root = crate::core::games_dir()
        .join("examples")
        .join("sourcehan_test");
    if !root.join("scenes/font_check.scene").is_file() {
        return;
    }
    let path = root.join("scenes/font_check.scene");
    let scene = Scene::load(&path).unwrap();
    let font_atlas = load_project_font_atlas(Some(&root)).expect("project font atlas");
    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
    runtime.set_font_atlas(Some(font_atlas));
    runtime.set_text_render_mode(TextRenderMode::Sdf);
    runtime.set_render_ui_overlay(false);
    let hidden = runtime.vertices(1280, 720).unwrap();
    runtime.set_render_ui_overlay(true);
    let mesh = runtime.vertices(1280, 720).unwrap();
    let sdf_draws = mesh.draws.iter().filter(|draw| draw.sdf).count();
    let font_paths = mesh
        .paths
        .iter()
        .filter(|path| path.as_str() == FONT_ATLAS_TEXTURE_PATH)
        .count();
    assert!(
        sdf_draws > 0 && font_paths > 0,
        "sdf draws={sdf_draws}, font paths={font_paths}, all={:?}",
        mesh.paths
    );
    // 空网格帧（overlay=false）不得消费版本号：首个可绘制帧必须携带全量
    // 像素（否则新渲染器会以空数据 write_texture 整张图集 → wgpu 崩溃）。
    assert!(
        hidden
            .font_atlas
            .as_ref()
            .is_some_and(|font| font.pixels.is_empty()),
        "empty-mesh frame must not consume the atlas version (deferred to drawable frame)"
    );
    assert!(
        mesh.font_atlas
            .as_ref()
            .is_some_and(|font| !font.pixels.is_empty()),
        "first drawable frame must carry full atlas pixels"
    );
}
