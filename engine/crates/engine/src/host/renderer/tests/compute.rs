//! M74-03 的 GPU 门：compute 接缝（P1）与第一个消费者 GPU 粒子（P2）。
//!
//! 这些用例**真的建设备、真的 dispatch、真的回读**。没有适配器时走 [`gpu_skipped`]：
//! `RUSTLUA_REQUIRE_GPU=1` 下跳过即 panic（本仓不接受"跳过当通过"）。
//!
//! 三条判据：
//! ① 常量填充 shader → 回读值逐项正确；
//! ② 同 key 第二次 dispatch 命中管线/绑定组缓存（`pipeline_compiles` 不涨）；
//! ③ GPU 粒子与 CPU `ParticleSystem` 同初始状态、同步数后位置差 < 1e-4；
//! ④ 无 compute 档下所有入口返回 `Unsupported`，且计划里没有 `Particle`。

use super::*;
use crate::host::frame::{SceneMesh, WorldSpriteVertex};
use crate::host::{
    ComputeBinding, ComputeError, ComputeRequest, GpuParticleConfig, GpuParticleState,
};
use rustlua_camera::camera::{Camera2D, RenderView, Viewport};
use rustlua_ecs::particle::{ParticleEmitter, ParticleSystem};
use rustlua_ecs::transform::Vec2;

fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-compute-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request compute test device");
    Some((device, queue))
}

const CONSTANT_FILL_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read_write> data: array<u32>;

@compute @workgroup_size(64)
fn fill(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < arrayLength(&data)) {
        data[id.x] = id.x * 3u + 7u;
    }
}
"#;

/// P1 硬门：有能力的设备上"dispatch 写常量 → 回读校验"通过，且第二次命中两级缓存。
#[test]
fn compute_constant_fill_roundtrip_and_cache_hits() {
    let Some((device, queue)) = headless() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    if !renderer.compute_enabled() {
        return gpu_skipped("device reports no compute");
    }
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-test-constant-fill"),
        size: 64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let request = ComputeRequest {
        label: "rustlua-test-constant-fill",
        shader: CONSTANT_FILL_SHADER,
        entry_point: "fill",
        bindings: &[ComputeBinding::Storage {
            binding: 0,
            buffer: &buffer,
            read_only: false,
        }],
        workgroups: [1, 1, 1],
        bind_group_key: Some(0xC0FFEE),
    };
    let first = renderer.dispatch_compute(&request).expect("dispatch");
    assert!(first.pipeline_compiled, "第一次必须编译管线");
    assert!(first.bind_group_created, "第一次必须建绑定组");

    let bytes = renderer
        .read_storage_buffer(&buffer, 64)
        .expect("compute readback");
    let values: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    let expected: Vec<u32> = (0..16).map(|index| index * 3 + 7).collect();
    assert_eq!(values, expected, "常量填充回读必须逐项正确");

    let second = renderer
        .dispatch_compute(&request)
        .expect("second dispatch");
    assert!(!second.pipeline_compiled, "同 key 必须命中管线缓存");
    assert!(!second.bind_group_created, "同 key 必须命中绑定组缓存");
    let stats = renderer.compute_stats();
    assert_eq!(stats.dispatches, 2);
    assert_eq!(stats.pipeline_compiles, 1);
    assert_eq!(stats.pipeline_cache_hits, 1);
    assert_eq!(stats.bind_group_creates, 1);
    assert_eq!(stats.bind_group_cache_hits, 1);
}

/// P1 硬门：无能力/被关闭时 `dispatch` 与消费者入口一律 `Unsupported`，不 panic。
#[test]
fn disabled_compute_returns_unsupported_for_every_entry() {
    let Some((device, queue)) = headless() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    renderer.set_compute_enabled(false);
    assert!(!renderer.compute_enabled());

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-test-disabled"),
        size: 64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let request = ComputeRequest {
        label: "rustlua-test-disabled",
        shader: CONSTANT_FILL_SHADER,
        entry_point: "fill",
        bindings: &[ComputeBinding::Storage {
            binding: 0,
            buffer: &buffer,
            read_only: false,
        }],
        workgroups: [1, 1, 1],
        bind_group_key: None,
    };
    assert!(matches!(
        renderer.dispatch_compute(&request),
        Err(ComputeError::Unsupported { .. })
    ));
    assert!(matches!(
        renderer.step_gpu_particles(1.0 / 60.0),
        Err(ComputeError::Unsupported { .. })
    ));

    // 关掉之后重新打开仍然可用（回滚档不是单向门）。
    renderer.set_compute_enabled(true);
    if renderer.compute_enabled() {
        renderer
            .dispatch_compute(&request)
            .expect("重新打开后 dispatch");
    }
}

fn test_emitter() -> ParticleEmitter {
    ParticleEmitter {
        count: 1024,
        speed: 120.0,
        life: 5.0,
        size_start: 4.0,
        size_end: 1.0,
        color_start: [1.0, 0.5, 0.2, 1.0],
        color_end: [1.0, 0.5, 0.2, 0.0],
        gravity: Vec2::new(0.0, -50.0),
        drag: 0.5,
        rotation: 0.25,
        rotation_speed: 1.0,
        ..ParticleEmitter::default()
    }
}

/// P2 硬门：两档对拍。CPU 与 GPU 同一初始 burst、同一步数，最终位置差 < 1e-4。
#[test]
fn gpu_particles_match_the_cpu_reference() {
    let Some((device, queue)) = headless() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    if !renderer.compute_enabled() {
        return gpu_skipped("device reports no compute");
    }
    let emitter = test_emitter();
    let status = renderer
        .configure_gpu_particles(GpuParticleConfig {
            origin: [10.0, -5.0],
            emitter,
            capacity: emitter.count as u32,
        })
        .expect("configure gpu particles");
    assert_eq!(status.spawned, emitter.count as u32);
    assert_eq!(status.capacity, emitter.count as u32);

    let mut cpu = ParticleSystem::new();
    cpu.burst_with(Vec2::new(10.0, -5.0), &emitter);
    assert_eq!(cpu.len(), emitter.count);

    let dt = 1.0 / 60.0;
    for _ in 0..120 {
        cpu.step(dt);
        renderer.step_gpu_particles(dt).expect("gpu step");
    }
    assert_eq!(cpu.len(), emitter.count, "1.92s < 最短寿命 2.5s");
    let gpu = renderer.read_gpu_particles().expect("read gpu particles");
    assert_eq!(gpu.len(), cpu.len());

    let mut max_position_error = 0.0_f32;
    let mut max_velocity_error = 0.0_f32;
    let mut max_rotation_error = 0.0_f32;
    for (index, (gpu, cpu)) in gpu.iter().zip(cpu.particles()).enumerate() {
        let position_error = ((gpu.position[0] - cpu.position.x).powi(2)
            + (gpu.position[1] - cpu.position.y).powi(2))
        .sqrt();
        let velocity_error = ((gpu.velocity[0] - cpu.velocity.x).powi(2)
            + (gpu.velocity[1] - cpu.velocity.y).powi(2))
        .sqrt();
        max_position_error = max_position_error.max(position_error);
        max_velocity_error = max_velocity_error.max(velocity_error);
        max_rotation_error = max_rotation_error.max((gpu.rotation - cpu.rotation).abs());
        assert!(
            position_error < 1e-4,
            "粒子 {index} 位置差 {position_error}（gpu={:?} cpu={:?}）",
            gpu.position,
            [cpu.position.x, cpu.position.y]
        );
        assert!(
            (gpu.life - cpu.life).abs() < 1e-5,
            "粒子 {index} 寿命 {} vs {}",
            gpu.life,
            cpu.life
        );
    }
    println!(
        "gpu_particles_match_cpu max_position_error={max_position_error} \
         max_velocity_error={max_velocity_error} max_rotation_error={max_rotation_error}"
    );
    assert!(max_position_error < 1e-4);
    assert_eq!(renderer.gpu_particle_status().expect("status").steps, 120);
}

/// 参数口径：连续发射（`rate > 0`）显式拒绝，不静默忽略。
#[test]
fn continuous_emission_is_rejected_explicitly() {
    let Some((device, queue)) = headless() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, false, false).expect("offscreen renderer");
    if !renderer.compute_enabled() {
        return gpu_skipped("device reports no compute");
    }
    let emitter = ParticleEmitter {
        rate: 30.0,
        ..ParticleEmitter::default()
    };
    let result = renderer.configure_gpu_particles(GpuParticleConfig {
        origin: [0.0, 0.0],
        emitter,
        capacity: 128,
    });
    assert!(matches!(result, Err(ComputeError::InvalidConfig(_))));
}

fn empty_mesh() -> SceneMesh {
    SceneMesh {
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
    }
}

/// P2 目视门（机器判据）：粒子 pass 真的进了计划、真的画了顶点、像素上真的多了东西。
#[test]
fn gpu_particle_pass_draws_pixels() {
    let Some((device, queue)) = headless() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    if !renderer.compute_enabled() {
        return gpu_skipped("device reports no compute");
    }
    let size = 128u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-particle-target"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
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
    let viewport = Viewport::new(size as f32, size as f32).expect("viewport");
    let camera = Camera2D::new(Vec2::new(0.0, 0.0), 1.0)
        .expect("camera")
        .view_projection(viewport, 0.1, 100.0)
        .expect("camera matrix");
    renderer.set_world_sprite_camera(camera);
    renderer.set_frame_views(FrameViews::overlay_only(RenderView::main_2d(
        Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("camera"),
        viewport,
        [0.0, 0.0, 0.0, 1.0],
    )));

    let emitter = ParticleEmitter {
        count: 64,
        speed: 0.0,
        life: 10.0,
        size_start: 12.0,
        size_end: 12.0,
        color_start: [1.0, 0.0, 0.0, 1.0],
        color_end: [1.0, 0.0, 0.0, 1.0],
        ..ParticleEmitter::default()
    };
    renderer
        .configure_gpu_particles(GpuParticleConfig {
            origin: [0.0, 0.0],
            emitter,
            capacity: 64,
        })
        .expect("configure");
    renderer.step_gpu_particles(1.0 / 60.0).expect("step");

    let mesh = empty_mesh();
    renderer.render_into_texture(&mesh, None, &view, size, size);
    assert!(
        renderer.last_frame_passes().contains(&RenderPass::Particle),
        "计划里必须有 Particle：{:?}",
        renderer.last_frame_passes()
    );
    assert!(
        renderer.last_frame_metrics().instances >= 64,
        "draw 实例数不足：{}",
        renderer.last_frame_metrics().instances
    );
    let pixels = renderer
        .copy_texture_to_cpu(&texture, size, size)
        .expect("pixels");
    let background = [pixels[0], pixels[1], pixels[2]];
    let changed = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != background)
        .count();
    println!("gpu_particle_pass changed_pixels={changed} background={background:?}");
    assert!(
        changed > 100,
        "粒子应覆盖中心区域（64 个同点粒子≈一个 12px 方块的面积），实际变化像素 {changed}"
    );

    // 关掉 compute ⇒ 计划里没有 Particle，像素回到纯背景（回滚档不残留半帧）。
    renderer.set_compute_enabled(false);
    renderer.render_into_texture(&mesh, None, &view, size, size);
    assert!(!renderer.last_frame_passes().contains(&RenderPass::Particle));
    let pixels = renderer
        .copy_texture_to_cpu(&texture, size, size)
        .expect("pixels after disable");
    let changed = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != background)
        .count();
    assert_eq!(changed, 0, "关掉 compute 后不应再画 GPU 粒子");
}

/// 布局一致性：公开的 `GpuParticleState` 必须与 shader 的 48 字节结构一致。
#[test]
fn public_state_layout_is_stable() {
    assert_eq!(std::mem::size_of::<GpuParticleState>(), 48);
    assert_eq!(std::mem::size_of::<WorldSpriteVertex>(), 36);
}
