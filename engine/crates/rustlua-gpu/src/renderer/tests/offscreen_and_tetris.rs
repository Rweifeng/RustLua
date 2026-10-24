//! 原 `renderer/mod.rs` 的 `mod tests`（WP12 阶段 4a 纯搬运）。离屏构造、俄罗斯方块示例工程像素、编辑器预览路径。

use super::*;

/// Renders an empty frame to an offscreen texture and reads it back,
/// proving the offscreen path (`render_into_texture` +
/// `copy_texture_to_cpu`) produces real pixels. Skips when no GPU is
/// available.
#[test]
fn offscreen_renderer_produces_pixels() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return gpu_skipped("request_adapter");
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-preview"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
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
    let mesh = rustlua_render::frame::SceneMesh {
        vertices: Vec::new(),
        draws: Vec::new(),
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: Vec::new(),
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };
    let outcome = renderer.render_into_texture(&mesh, None, &view, 64, 64);
    assert!(matches!(outcome, RenderOutcome::Presented));
    let pixels = renderer
        .copy_texture_to_cpu(&texture, 64, 64)
        .expect("preview readback");
    assert_eq!(pixels.len(), 64 * 64 * 4);
    // Clear color is (0.04, 0.07, 0.12); the frame must not be all zero.
    assert!(
        pixels.iter().any(|&byte| byte > 0),
        "offscreen frame is blank"
    );
}

/// 俄罗斯方块离屏渲染：方块（white.png 精灵）应实际可见——I 方块起始
/// 位于 col 4-7/row 0，其红色调应出现在字段区域的像素上。
#[test]
fn tetris_blocks_are_visible_pixels() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return gpu_skipped("request_adapter");
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-tetris-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let root = rustlua_infra::fixture_games_dir().join("tetris");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let mut renderer = Renderer::new_offscreen(
        device.clone(),
        queue.clone(),
        Some(root.clone()),
        false,
        false,
    )
    .expect("offscreen renderer");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-tetris-preview"),
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
    let scene = rustlua_scene::scene::Scene::load(root.join("scenes/main.scene")).unwrap();
    let mut runtime = engine::host::RuntimeScene::from_scene(Some(&root), scene).unwrap();
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    renderer.render_into_texture(&mesh, None, &view, 1280, 720);
    let pixels = renderer
        .copy_texture_to_cpu(&texture, 1280, 720)
        .expect("readback");
    // 扫描全帧找 I 方块（红色调 r 显著大于 g）的可见像素。
    let mut found = 0usize;
    let mut first = (0usize, 0usize);
    for y in 0..720usize {
        for x in 0..1280usize {
            let idx = (y * 1280 + x) * 4;
            let (r, g, b) = (pixels[idx], pixels[idx + 1], pixels[idx + 2]);
            // I 色 (1,0.45,0.45) 线性 → sRGB (255,179,179)：红主导即方块。
            let _ = b;
            if r > 200 && i32::from(r) > i32::from(g) {
                if found == 0 {
                    first = (x, y);
                }
                found += 1;
            }
        }
    }
    assert!(
        found > 0,
        "未找到红色方块像素（I 方块应可见），首像素位置样本：{}x{}",
        first.0,
        first.1
    );
    // 首块 I 方块 cell(5,0) 世界 (655,75) 附近应能看到红色。
    eprintln!("red block pixels={found} first at {}x{}", first.0, first.1);
}

/// 重开（R）应清屏：初始 4 块 → 硬降后 8 块 → 重开后只剩新方块 4 块。
#[test]
fn tetris_restart_clears_field() {
    use rustlua_input::input::{InputSnapshot, KeyCode};
    use rustlua_scene::scene::Scene;

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return; // no GPU; skip
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-tetris-restart-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("device");
    let root = rustlua_infra::fixture_games_dir().join("tetris");
    let mut renderer = Renderer::new_offscreen(
        device.clone(),
        queue.clone(),
        Some(root.clone()),
        false,
        false,
    )
    .expect("offscreen renderer");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-tetris-restart"),
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
    let root = rustlua_infra::fixture_games_dir().join("tetris");
    if !root.join("scenes/main.scene").is_file() {
        return;
    }
    let scene = Scene::load(root.join("scenes/main.scene")).unwrap();
    let mut runtime = engine::host::RuntimeScene::from_scene(Some(&root), scene).unwrap();
    // 统计字段区域（屏幕 486..794 × 56..664）内的亮块像素（方块）。
    let mut count_blocks = |runtime: &mut engine::host::RuntimeScene| -> usize {
        let mesh = runtime.vertices(1280, 720).unwrap();
        renderer.render_into_texture(&mesh, None, &view, 1280, 720);
        let pixels = renderer.copy_texture_to_cpu(&texture, 1280, 720).unwrap();
        let mut n = 0;
        for y in 56..664usize {
            for x in 486..794usize {
                let idx = (y * 1280 + x) * 4;
                let m = pixels[idx].max(pixels[idx + 1]).max(pixels[idx + 2]);
                if m > 150 {
                    n += 1;
                }
            }
        }
        n
    };
    let initial = count_blocks(&mut runtime);
    assert_eq!(initial, 4 * 28 * 28, "初始应是 4 块，got {initial}");
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::Space, true); // 硬降：放置当前块 + 生成新块
    runtime.run_script_steps(1, 1.0 / 60.0, &input).unwrap();
    let after_drop = count_blocks(&mut runtime);
    assert_eq!(
        after_drop,
        8 * 28 * 28,
        "硬降后应 8 块（4 放置 + 4 新），got {after_drop}"
    );
    input.key_event(KeyCode::Space, false);
    input.key_event(
        KeyCode::from_char('r').expect("QWERTY 字符必须有对应物理键"),
        true,
    ); // 重开
    runtime.run_script_steps(1, 1.0 / 60.0, &input).unwrap();
    let after_restart = count_blocks(&mut runtime);
    assert_eq!(
        after_restart,
        4 * 28 * 28,
        "重开应清屏只剩 4 块，got {after_restart}"
    );
}

/// Reproduces the editor's `create_preview` path against the real game
/// project: instantiate `main.scene`, step, build frames, render offscreen
/// and read back. Requires the game project beside the engine workspace.
#[test]
fn editor_preview_path_renders_game_scene() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        panic!("无法获取用于编辑器预览测试的 wgpu adapter");
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let game = rustlua_infra::games_dir().join("examples").join("game_api");
    if !game.join("scenes/main.scene").is_file() {
        return;
    }
    let scene_text =
        std::fs::read_to_string(game.join("scenes/main.scene")).expect("read main.scene");
    let scene = rustlua_scene::scene::Scene::from_text(&scene_text).expect("parse main.scene");
    let mut runtime = engine::host::runtime_scene::RuntimeScene::from_scene(Some(&game), scene)
        .expect("instantiate runtime");
    runtime
        .run_script_steps(
            1,
            1.0 / 60.0,
            &rustlua_input::input::InputSnapshot::default(),
        )
        .expect("step runtime");
    let mesh = runtime.vertices(640, 360).expect("build 2D mesh");
    let scene3d = runtime.scene_3d(Viewport::new(640.0, 360.0).unwrap());
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-preview"),
        size: wgpu::Extent3d {
            width: 640,
            height: 360,
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
    renderer.render_into_texture(&mesh, scene3d.as_ref(), &view, 640, 360);
    let pixels = renderer
        .copy_texture_to_cpu(&texture, 640, 360)
        .expect("readback");
    assert_eq!(pixels.len(), 640 * 360 * 4);
    assert!(
        pixels.iter().any(|&byte| byte > 0),
        "preview frame is blank"
    );
}
