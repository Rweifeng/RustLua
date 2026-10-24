//! E06-3：逐通道 GPU timestamp 的真实回读门。
//!
//! 两条用例分别覆盖门的两半：
//! - 设备**支持** timestamp 时，同一查询集解析出整帧与各通道时间，且各通道之和与整帧差 ≤5%；
//! - 设备**不支持**时，报告是 `supported=false` + 原因，`frame_us` 为 `None`——不拿 0 冒充。

use super::*;
use rustlua_render::frame::OverlayVertex;
use rustlua_render::frame::{SceneDraw, SceneMesh};

const SIZE: u32 = 64;
const TIMESTAMP_FEATURES: wgpu::Features =
    wgpu::Features::TIMESTAMP_QUERY.union(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);

fn request_device(label: &str, features: wgpu::Features) -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some(label),
        required_features: features,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    Some((device, queue))
}

fn quad() -> Vec<OverlayVertex> {
    let corner = |x: f32, y: f32| OverlayVertex {
        position: [x, y],
        uv: [0.0, 0.0],
        color: [1.0; 4],
        sdf: 0,
    };
    vec![
        corner(-0.9, -0.9),
        corner(0.9, -0.9),
        corner(0.9, 0.9),
        corner(-0.9, -0.9),
        corner(0.9, 0.9),
        corner(-0.9, 0.9),
    ]
}

fn overlay_mesh() -> SceneMesh {
    SceneMesh {
        vertices: quad(),
        draws: vec![SceneDraw::screen_ui(0, 0, 6)],
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: Vec::new(),
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    }
}

fn target(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-pass-timestamps"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// 支持 timestamp 的设备：整帧与逐通道来自同一次提交，通道和与整帧差 ≤5%。
#[test]
fn per_pass_timestamps_cover_the_frame_and_agree_within_five_percent() {
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
    if !adapter.features().contains(TIMESTAMP_FEATURES) {
        gpu_skipped("adapter lacks TIMESTAMP_QUERY features");
        return;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-pass-timestamps-device"),
        required_features: TIMESTAMP_FEATURES,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request timestamp device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    assert!(
        renderer.gpu_timestamp_supported(),
        "设备声明了 timestamp 查询，渲染器必须启用它"
    );
    let texture = target(&device);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mesh = overlay_mesh();
    assert!(matches!(
        renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE),
        RenderOutcome::Presented
    ));
    let frame_us = renderer
        .wait_for_gpu_timestamp(std::time::Duration::from_secs(5))
        .expect("timestamp readback");
    let report = renderer.gpu_pass_report();
    assert!(report.supported, "报告：{}", report.to_text());
    assert_eq!(report.frame_us, Some(frame_us));
    assert!(
        report.pass_us(RenderPass::Clear).is_some(),
        "清屏通道必须被计时：{}",
        report.to_text()
    );
    assert!(
        report.pass_us(RenderPass::Legacy2DOverlay).is_some(),
        "2D overlay 通道必须被计时：{}",
        report.to_text()
    );
    assert_eq!(
        report.passes.iter().map(|p| p.gpu_us).sum::<u64>(),
        report.pass_sum_us
    );
    let delta = report.delta_pct.expect("整帧非零 ⇒ 有百分比");
    assert!(
        delta.abs() <= 5.0,
        "通道和与整帧相差 {delta:+.2}% 超出 5%：{}",
        report.to_text()
    );
    assert_eq!(report.consistent, Some(true));
    assert!(report.to_text().contains("consistent"));
    println!("{}", report.to_text());
}

/// 没请求 timestamp 特性的设备：报告必须是 `unavailable`，不是 0 µs。
#[test]
fn device_without_timestamp_features_reports_unsupported_not_zero() {
    let Some((device, queue)) =
        request_device("rustlua-test-no-timestamp-device", wgpu::Features::empty())
    else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    assert!(!renderer.gpu_timestamp_supported());
    assert_eq!(renderer.gpu_timestamp_reason(), Some("unsupported"));
    let texture = target(&device);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    assert!(matches!(
        renderer.render_into_texture(&overlay_mesh(), None, &view, SIZE, SIZE),
        RenderOutcome::Presented
    ));
    assert_eq!(
        renderer.wait_for_gpu_timestamp(std::time::Duration::from_millis(50)),
        None
    );
    let report = renderer.gpu_pass_report();
    assert!(!report.supported);
    assert_eq!(report.reason.as_deref(), Some("unsupported"));
    assert_eq!(report.frame_us, None);
    assert!(report.passes.is_empty());
    assert_eq!(report.pass_sum_us, 0);
    assert_eq!(report.delta_pct, None);
    assert_eq!(report.consistent, None);
    assert!(report.to_text().contains("unavailable (unsupported)"));
}
