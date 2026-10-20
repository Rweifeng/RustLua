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
