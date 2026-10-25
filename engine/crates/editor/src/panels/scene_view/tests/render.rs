//! 离屏渲染：UI 与场景网格在没有窗口时也要能画出图（无 GPU 时跳过）。

use super::*;

/// 复现 examples/ui_test.scene 的 GPU 离屏渲染（含 font_size=75 的 SDF 文本、
/// Image/Button 覆盖层），若 render_into_texture panic 则测试失败。
#[test]
fn ui_test_scene_renders_offscreen_with_gpu() {
    let Some((device, queue)) = test_device() else {
        return;
    };
    let source = rustlua_infra::games_dir().join("examples").join("game_api");
    let root = temp_root("ui-test-gpu");
    fs::create_dir_all(&root).unwrap();
    fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
    copy_dir(&source.join("scripts"), &root.join("scripts"));
    copy_dir(&source.join("scenes"), &root.join("scenes"));
    copy_dir(&source.join("assets"), &root.join("assets"));
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    let scene = app.document.current_scene.as_ref().unwrap().scene.clone();
    // from_scene_edit 默认 render_ui_overlay=true：vertices 渲染 Text/Image/Button。
    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
    let mesh = runtime.vertices(1280, 720).expect("build ui_test mesh");
    let mut renderer = Renderer::new_offscreen(
        device.clone(),
        queue.clone(),
        Some(root.clone()),
        false,
        false,
    )
    .expect("offscreen renderer");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-ui-test"),
        size: wgpu::Extent3d {
            width: 1280,
            height: 720,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    // GPU 渲染 ui_test 网格：若 panic/校验失败则测试失败。
    renderer.render_into_texture(&mesh, None, &view, 1280, 720);
    let _ = fs::remove_dir_all(&root);
}

/// 有 3D 时游戏画面**不得**声明 2D 主视图：渲染器把声明的 `RenderView` 优先当成
/// 3D 的投影矩阵（`passes.rs` 的 `main_matrix`），3D 会被 2D 正交矩阵拍扁。
#[test]
fn game_screen_skips_2d_main_view_when_scene_has_3d() {
    let camera = Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("camera");
    let viewport = render_viewport(1280, 720);
    let flat = super::super::render::game_screen_frame_views(camera, viewport, false);
    let main = flat.main.expect("纯 2D 用游戏相机作主视图");
    assert_eq!(main.viewport, viewport);

    let with_3d = super::super::render::game_screen_frame_views(camera, viewport, true);
    assert!(
        with_3d.main.is_none(),
        "有 3D 时必须不声明，让渲染器回退到 Scene3d::view"
    );
    assert!(with_3d.overlay.is_none());
}

/// 未 Play 时游戏画面也要能画出 3D：`render_scene_screen` 走与 Play 同一条
/// `scene_3d` 提取（Game 视图的编辑预览复用它）。
#[test]
fn edit_game_screen_renders_3d_scene_without_play() {
    let Some((device, queue)) = test_device() else {
        return;
    };
    let root = copy_sample_project("game-screen-3d");
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, Some((device, queue)), LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/demo3d.scene"));
    app.ensure_scene_runtime();
    app.ensure_scene_renderer_instance();
    let runtime = app.scene_runtime.as_ref().expect("edit runtime");
    let viewport = render_viewport(app.preview_resolution.0, app.preview_resolution.1);
    assert!(
        runtime.scene_3d(viewport).is_some(),
        "样例 3D 场景必须提取出 drawable，否则本用例没有覆盖 3D 分支"
    );

    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput::default());
    app.render_scene_screen(&ctx);
    let _ = ctx.end_pass();

    assert!(!app.is_playing(), "编辑预览不得进入 Play");
    assert!(
        app.scene_screen_texture.is_some(),
        "3D 场景的游戏画面纹理应已建出"
    );
    assert!(
        app.scene_screen_cache_key.is_some(),
        "渲染路径应真的跑过（不是提前返回）"
    );
    let _ = fs::remove_dir_all(root);
}
