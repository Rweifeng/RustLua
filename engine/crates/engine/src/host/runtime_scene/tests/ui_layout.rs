//! 原 `mod sdf_tests` 里 `ui_layout` 这一域（WP12 阶段 1 纯搬运；源行段 5592–5804）。
//!
//! - `nine_slice_image_renders_nine_quads`
//! - `scene_ui_image_alpha_reaches_the_vertex`
//! - `scene_ui_text_keeps_layout_origin_once`
//! - `ui_layout_preview_targets_authored_entity`
//! - `ui_layout_rects_expose_exact_preview_geometry`
//! - `ui_layout_preview_changes_rendered_mesh`
//! - `font_prewarm_from_chars_txt`

use super::*;
use rustlua_scene::scene::Scene;
use std::fs;

/// 九宫格：带 `nine_slice` 的 Image 应渲染为 3×3 共 9 格（54 顶点），而非
/// 单四边形（6 顶点）。用真实按钮纹理（192×64）验证 UV 分片路径。
#[test]
fn nine_slice_image_renders_nine_quads() {
    use rustlua_scene::scene::{Scene, SceneEntityId};

    let root = rustlua_infra::games_dir().join("examples").join("game_api");
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
    use rustlua_scene::scene::{Scene, SceneEntityId};

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
    use rustlua_scene::scene::{Scene, SceneEntityId};

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
    use rustlua_scene::scene::SceneEntityId;

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
    use rustlua_scene::scene::{Scene, SceneEntityId};

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

/// 半透明色块（M64a）：作者写的 `color` 第四段必须真的走到**顶点 alpha** 上。
///
/// 为什么钉在顶点而不是矩形：发射代码（`mesh_ui.rs` 的 `push_image_quad`）曾经把 alpha
/// 硬写成 `1.0`，那种回归矩形全对、画面全实。实测（口径：把 `image.alpha` 改回 `1.0` 跑
/// `cargo test -p engine --lib`）：159 条里只有本条红，另一条红是既有无关红（`prefab.*`
/// 缺 game_api 演示），所以面板遮罩搬进 `.prefab` 之前这条判定没有任何门会红。
/// 同一趟里钉住"三段仍是不透明"：仓库 120 行 `Image` 的 `color=` 全是三段，这条一红就是全工程变淡。
#[test]
fn scene_ui_image_alpha_reaches_the_vertex() {
    use rustlua_scene::scene::{Scene, SceneEntityId};

    let mut scene = Scene::new("image-alpha");
    for (id, name, color, offsets) in [
        (
            SceneEntityId(1),
            "Dim",
            "0.2,0.3,0.4,0.55",
            "offsets=0,0,80,60",
        ),
        (SceneEntityId(2), "Opaque", "1,1,1", "offsets=0,100,80,160"),
    ] {
        scene.add_entity(id, name, None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        // `__white__` 是引擎内置的 1×1 白纹理（`WHITE_TEXTURE_PATH`）：纯色块不需要任何
        // 项目文件，这个测试因此不依赖样例工程是否存在。
        entity.components.insert(
            "Image".to_owned(),
            format!("texture=__white__;color={color}"),
        );
        entity
            .components
            .insert("UiLayout".to_owned(), format!("anchors=0,0,0,0;{offsets}"));
    }
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).expect("runtime image-alpha");
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    // 两块白图共用同一份纹理，会被并成**一个** 12 顶点的批，而不是两个 6 顶点的批，
    // 所以这里按顶点读数、不按 draw 数找四边形。
    let colors: Vec<[f32; 4]> = mesh.vertices.iter().map(|v| v.color).collect();
    let dim: Vec<[f32; 4]> = colors
        .iter()
        .copied()
        .filter(|c| (c[0] - 0.2).abs() < 1e-4)
        .collect();
    assert_eq!(
        dim.len(),
        6,
        "半透明那块应出满一个四边形（6 顶点）：{colors:?}"
    );
    assert!(
        dim.iter().all(|c| *c == [0.2, 0.3, 0.4, 0.55]),
        "第四段必须落到顶点 alpha，rgb 不许被动过：{dim:?}"
    );
    let opaque: Vec<[f32; 4]> = colors
        .iter()
        .copied()
        .filter(|c| (c[0] - 1.0).abs() < 1e-4)
        .collect();
    assert_eq!(opaque.len(), 6, "不透明那块同样出满一个四边形：{colors:?}");
    assert!(
        opaque.iter().all(|c| *c == [1.0, 1.0, 1.0, 1.0]),
        "三段 `color=` 仍须读成不透明（仓库 120 行 `Image` 数据的形状）：{opaque:?}"
    );
}
