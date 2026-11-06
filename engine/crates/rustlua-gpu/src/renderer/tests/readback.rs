//! 原 `renderer/mod.rs` 的 `mod tests`（WP12 阶段 4a 纯搬运）。非阻塞回读：轮询出像素、映射失败清槽并报错。

use super::*;
use crate::renderer::write_capture_placeholder;

/// Non-blocking readback (`readback_frame`) eventually yields the rendered
/// pixels without a blocking `device.poll(Wait)` on the first call. Skips
/// when no GPU is available.
#[test]
fn non_blocking_readback_eventually_yields_pixels() {
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
        label: Some("rustlua-test-readback"),
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
    renderer.render_into_texture(&mesh, None, &view, 64, 64, true);
    // 首帧提交；随后的调用非阻塞轮询，直到返回就绪像素（不应无限阻塞）。
    let mut result = renderer.readback_frame(&texture, 64, 64);
    for _ in 0..120 {
        match &result {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            } => {
                assert_eq!((*width, *height), (64, 64));
                assert_eq!(pixels.len(), 64 * 64 * 4);
                assert!(
                    pixels.iter().any(|&byte| byte > 0),
                    "offscreen frame is blank"
                );
                return;
            }
            ReadbackStatus::Pending => {
                // 未就绪：等 GPU 完成拷贝（真实编辑器帧间隔远大于此），再轮询。
                std::thread::sleep(std::time::Duration::from_millis(2));
                result = renderer.readback_frame(&texture, 64, 64);
            }
            ReadbackStatus::Failed(error) => panic!("readback failed: {error}"),
        }
    }
    panic!("readback never became ready within 120 polls");
}

/// §21.256 抓帧失败也要落一张可显示的占位 PNG。
#[test]
fn capture_placeholder_writes_png() {
    let path = std::env::temp_dir().join(format!(
        "rustlua-capture-placeholder-{}.png",
        std::process::id()
    ));
    write_capture_placeholder(&path).expect("write placeholder png");
    let decoded = image::open(&path)
        .expect("decode placeholder png")
        .to_rgba8();
    assert_eq!(decoded.dimensions(), (320, 180));
    assert_eq!(decoded.get_pixel(0, 0).0, [178, 142, 74, 255]);
    let _ = std::fs::remove_file(path);
}

#[test]
fn capture_surface_png_writes_a_png() {
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
        label: Some("rustlua-test-capture-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, false, false).expect("offscreen renderer");
    renderer.config.width = 64;
    renderer.config.height = 64;
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
    let path =
        std::env::temp_dir().join(format!("rustlua-capture-test-{}.png", std::process::id()));
    renderer
        .capture_surface_png(&mesh, None, &path, 2, false)
        .expect("capture png");
    let bytes = std::fs::read(&path).expect("read captured png");
    assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']), "not a PNG");
    // 相框 2 档是黄铜色；四角必须被烧进 PNG。
    let decoded = image::open(&path).expect("decode captured png").to_rgba8();
    assert_eq!(decoded.get_pixel(0, 0).0, [178, 142, 74, 255]);
    let mark = [240, 232, 210, 255];
    let marked = decoded.pixels().filter(|px| px.0 == mark).count();
    assert!(
        marked > 20,
        "MYRMEX watermark not burned in: {marked} pixels"
    );
    // 整屏抓图（include_ui=true）已经由 UI 层提供相框，不再重复烧像素框。
    let ui_path = std::env::temp_dir().join(format!(
        "rustlua-capture-ui-test-{}.png",
        std::process::id()
    ));
    renderer
        .capture_surface_png(&mesh, None, &ui_path, 2, true)
        .expect("capture png with ui");
    let ui = image::open(&ui_path).expect("decode ui png").to_rgba8();
    assert_ne!(ui.get_pixel(0, 0).0, [178, 142, 74, 255]);
    let _ = std::fs::remove_file(ui_path);
    let _ = std::fs::remove_file(path);
}

/// M11-03（R-04）：映射失败的 readback 槽上报 `Failed` 而非当作就绪返回脏数据，
/// 且失败槽被清空，后续轮询可重试（不再永久卡在旧状态）。
#[test]
fn failed_readback_mapping_reports_error_and_clears_slot() {
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
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    // 提交一个空 encoder 拿到合法的 SubmissionIndex（无需真实回读流程）。
    let empty = device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default())
        .finish();
    let submission = queue.submit([empty]);
    // 注入一个"映射失败"的槽（模拟 map_async 回调返回 Err）。
    renderer.readback_slots.push(ReadbackSlot {
        buffer: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-test-readback-failed"),
            size: 256 * 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }),
        submission: Some(submission),
        map_result: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(2)),
        map_error: std::sync::Arc::new(std::sync::Mutex::new(Some("注入的映射失败".to_owned()))),
        width: 8,
        height: 8,
        row_bytes: 32,
        bytes_per_row: 256,
        pixels: Vec::new(),
        generation: 0,
    });
    match renderer.readback_pending() {
        ReadbackStatus::Failed(error) => assert!(error.contains("注入的映射失败")),
        _ => panic!("映射失败应上报 Failed 而非脏数据"),
    }
    // 失败槽已清空：再次轮询进入 NotReady → Pending，不再重复报错。
    assert!(matches!(
        renderer.readback_pending(),
        ReadbackStatus::Pending
    ));
    assert!(!renderer.readback_in_flight(), "失败槽应被释放以便重试");
}
