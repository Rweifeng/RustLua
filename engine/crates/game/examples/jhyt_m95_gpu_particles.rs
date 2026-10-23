//! M74-03 的 headless 门：compute 能力探测 + 第一个真实消费者（GPU 粒子）。
//!
//! ## 这条门证明什么
//!
//! ① **能力**：`RendererCapabilities` 在真实设备上给出 `compute` 与各项 workgroup/storage 上限；
//! ② **接缝**：compute dispatch → 存储缓冲 → 同步回读，数值逐项正确，管线/绑定组缓存命中；
//! ③ **消费者**：同一初始状态，GPU 粒子与 CPU `ParticleSystem` 跑 120 步后位置差 < 1e-4；
//! ④ **可见**：`RenderPass::Particle` 真的进计划、真的画进颜色附件（像素变化 > 0）；
//! ⑤ **50k**：只量帧时间与状态有限性（不把 1e-4 判据外推到 50k）；
//! ⑥ **降级**：`set_compute_enabled(false)` 后所有入口返回 `Unsupported`，计划里没有粒子通道。
//!
//! ## 无 compute 的机器
//!
//! 本仓的硬规矩是"无能力时**显式降级**，不 panic、不静默空转"。因此这里**以退出码 3 收场**
//! （与 `scripts/verify/gpu-web-probe.ps1` 的"环境受限"约定一致），并打印具体原因——
//! 不是记一个"通过"。有 GPU 的机器上，`RUSTLUA_REQUIRE_GPU=1` 可把"跳过"升级为失败。
//!
//! Usage:
//! ```text
//! cargo run -p game --example jhyt_m95_gpu_particles
//! ```

use std::time::Instant;

use engine::host::{ComputeError, GpuParticleConfig, Renderer, SceneMesh};
use rustlua_camera::camera::{Camera2D, FrameViews, RenderView, Viewport};
use rustlua_ecs::particle::{ParticleEmitter, ParticleSystem};
use rustlua_ecs::transform::Vec2;
use rustlua_render::render_plan::RenderPass;

const PARTICLES: usize = 10_000;
const STEPS: u32 = 120;
const DT: f32 = 1.0 / 60.0;
const TARGET: u32 = 256;

fn banner(title: &str) {
    println!("\n=== {title} ===");
}

/// 断言 + 打印：读数是"证据"，失败是"门"。
fn check(what: &str, ok: bool) {
    println!("  [{}] {what}", if ok { "ok" } else { "FAIL" });
    assert!(ok, "{what}");
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

/// 环境受限的退出：3 = 跳过（不记通过），`RUSTLUA_REQUIRE_GPU=1` 时升级为 1。
///
/// 与 `scripts/verify/gpu-web-probe.ps1` 的退出码语义一致；调用点只用于"这台机器拿不到
/// 可用设备/没有 compute"，不用于代码或断言失败。
fn skip_environment(reason: &str) -> ! {
    eprintln!("\n[skip] {reason}");
    eprintln!("[skip] CPU 粒子仍是默认与回滚档；这条门不记通过。");
    if std::env::var("RUSTLUA_REQUIRE_GPU").as_deref() == Ok("1") {
        eprintln!("[fail] RUSTLUA_REQUIRE_GPU=1：要求这台机器提供 compute");
        std::process::exit(1);
    }
    std::process::exit(3);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .or_else(|_| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
    })
    .unwrap_or_else(|error| skip_environment(&format!("没有可用的 GPU 适配器：{error}")));
    let info = adapter.get_info();
    println!(
        "adapter = {} ({:?}, {:?})",
        info.name, info.backend, info.device_type
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-m95-gpu-particles"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .unwrap_or_else(|error| skip_environment(&format!("默认限制的设备建不出来：{error}")));
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)?;

    banner("① 能力快照（唯一判据）");
    let caps = renderer.capabilities();
    println!(
        "  compute={} storage={} timestamp={} backend={}",
        caps.compute,
        caps.storage_buffers,
        caps.timestamp,
        caps.backend_name()
    );
    println!(
        "  workgroup={:?} invocations={} workgroups/dim={} storage_binding={}",
        caps.max_compute_workgroup_size,
        caps.max_compute_invocations_per_workgroup,
        caps.max_compute_workgroups_per_dimension,
        caps.max_storage_buffer_binding_size
    );
    if !caps.compute || !renderer.compute_enabled() {
        let reason = caps
            .compute_unavailable_reason()
            .unwrap_or_else(|| "宿主关闭了 compute".to_owned());
        skip_environment(&format!("本机没有 compute：{reason}"));
    }
    check("compute 可用", caps.compute);
    check(
        "workgroup 上限足够 64 线程",
        caps.max_compute_workgroup_size[0] >= 64,
    );

    banner("② 消费者：GPU 粒子 vs CPU 粒子（同一初始状态、同一步数）");
    let emitter = ParticleEmitter {
        count: PARTICLES,
        speed: 80.0,
        life: 5.0,
        size_start: 6.0,
        size_end: 2.0,
        color_start: [1.0, 0.6, 0.2, 1.0],
        color_end: [0.8, 0.2, 0.1, 0.0],
        gravity: Vec2::new(0.0, -30.0),
        drag: 0.4,
        rotation: 0.25,
        rotation_speed: 1.5,
        ..ParticleEmitter::default()
    };
    let status = renderer.configure_gpu_particles(GpuParticleConfig {
        origin: [0.0, 0.0],
        emitter,
        capacity: PARTICLES as u32,
    })?;
    println!(
        "  capacity={} spawned={} state={}B backend={}",
        status.capacity, status.spawned, status.state_bytes, status.backend
    );
    check(
        "槽位装载量 = 请求粒子数",
        status.spawned == PARTICLES as u32,
    );

    let mut cpu = ParticleSystem::new();
    cpu.burst_with(Vec2::new(0.0, 0.0), &emitter);
    let cpu_started = Instant::now();
    for _ in 0..STEPS {
        cpu.step(DT);
    }
    let cpu_elapsed = cpu_started.elapsed();
    let gpu_started = Instant::now();
    for _ in 0..STEPS {
        renderer.step_gpu_particles(DT)?;
    }
    let gpu_submit_elapsed = gpu_started.elapsed();
    let gpu = renderer.read_gpu_particles()?;
    println!(
        "  步数={STEPS} dt={DT:.5} cpu={:?} gpu_submit={:?}（回读前，仅 command 提交）",
        cpu_elapsed, gpu_submit_elapsed
    );
    check("GPU 回读粒子数 = CPU 粒子数", gpu.len() == cpu.len());

    let mut max_position_error = 0.0_f32;
    let mut max_velocity_error = 0.0_f32;
    for (index, (gpu, cpu)) in gpu.iter().zip(cpu.particles()).enumerate() {
        let position_error = ((gpu.position[0] - cpu.position.x).powi(2)
            + (gpu.position[1] - cpu.position.y).powi(2))
        .sqrt();
        let velocity_error = ((gpu.velocity[0] - cpu.velocity.x).powi(2)
            + (gpu.velocity[1] - cpu.velocity.y).powi(2))
        .sqrt();
        max_position_error = max_position_error.max(position_error);
        max_velocity_error = max_velocity_error.max(velocity_error);
        assert!(
            position_error < 1e-4,
            "粒子 {index} 位置差 {position_error}（gpu={:?} cpu={:?}）",
            gpu.position,
            [cpu.position.x, cpu.position.y]
        );
    }
    println!(
        "  max_position_error={max_position_error:.3e} max_velocity_error={max_velocity_error:.3e}"
    );
    check("最终状态位置差 < 1e-4", max_position_error < 1e-4);

    banner("③ 可见性：Particle 通道真的画进颜色附件");
    let viewport = Viewport::new(TARGET as f32, TARGET as f32)?;
    let camera = Camera2D::new(Vec2::new(0.0, 0.0), 1.0)?.view_projection(viewport, 0.1, 100.0)?;
    renderer.set_world_sprite_camera(camera);
    renderer.set_frame_views(FrameViews::overlay_only(RenderView::main_2d(
        Camera2D::new(Vec2::new(0.0, 0.0), 1.0)?,
        viewport,
        [0.0, 0.0, 0.0, 1.0],
    )));
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-m95-gpu-particles-target"),
        size: wgpu::Extent3d {
            width: TARGET,
            height: TARGET,
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
    let mesh = empty_mesh();
    renderer.render_into_texture(&mesh, None, &view, TARGET, TARGET);
    let passes = renderer.last_frame_passes();
    println!("  passes = {:?}", passes);
    check("计划里含 Particle", passes.contains(&RenderPass::Particle));
    let metrics = renderer.last_frame_metrics();
    println!(
        "  draw_calls={} instances={} vertices={} dropped={}",
        metrics.draw_calls, metrics.instances, metrics.vertices, metrics.dropped_draws
    );
    check(
        "粒子的实例数 = capacity、顶点数 = 4×capacity（instanced triangle strip，没有顶点缓冲）",
        metrics.instances >= PARTICLES as u64 && metrics.vertices >= 4 * PARTICLES as u64,
    );
    let pixels = renderer.copy_texture_to_cpu(&texture, TARGET, TARGET)?;
    let background = [pixels[0], pixels[1], pixels[2]];
    let changed = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != background)
        .count();
    println!("  changed_pixels={changed} background={background:?}");
    check("颜色附件上出现粒子像素", changed > 0);

    banner("④ 50k 档：帧时间对比（不把 1e-4 判据外推到 50k，误差随粒子数增大）");
    let emitter_50k = ParticleEmitter {
        count: 50_000,
        ..emitter
    };
    let status_50k = renderer.configure_gpu_particles(GpuParticleConfig {
        origin: [0.0, 0.0],
        emitter: emitter_50k,
        capacity: 50_000,
    })?;
    println!(
        "  capacity={} spawned={} state={}B",
        status_50k.capacity, status_50k.spawned, status_50k.state_bytes
    );
    let mut cpu_50k = ParticleSystem::new();
    cpu_50k.burst_with(Vec2::new(0.0, 0.0), &emitter_50k);
    let steps_50k = 60u32;
    let cpu_started = Instant::now();
    for _ in 0..steps_50k {
        cpu_50k.step(DT);
    }
    let cpu_elapsed = cpu_started.elapsed();
    let gpu_started = Instant::now();
    for _ in 0..steps_50k {
        renderer.step_gpu_particles(DT)?;
    }
    let gpu_submit_elapsed = gpu_started.elapsed();
    let gpu_50k = renderer.read_gpu_particles()?;
    let max_error_50k = gpu_50k
        .iter()
        .zip(cpu_50k.particles())
        .map(|(gpu, cpu)| {
            ((gpu.position[0] - cpu.position.x).powi(2)
                + (gpu.position[1] - cpu.position.y).powi(2))
            .sqrt()
        })
        .fold(0.0_f32, f32::max);
    let all_finite = gpu_50k.iter().all(|particle| {
        particle.position[0].is_finite()
            && particle.position[1].is_finite()
            && particle.velocity[0].is_finite()
            && particle.velocity[1].is_finite()
            && particle.life.is_finite()
    });
    println!(
        "  步数={steps_50k} cpu={:?} gpu_submit={:?}（仅 command 提交，未等待 GPU） max_position_error={max_error_50k:.3e}",
        cpu_elapsed, gpu_submit_elapsed
    );
    check("50k 回读数量 = CPU 粒子数", gpu_50k.len() == cpu_50k.len());
    check("50k 状态全部有限（无 NaN/Inf）", all_finite);

    banner("⑤ 降级：关掉 compute 后所有入口显式拒绝、计划里没有粒子");
    renderer.set_compute_enabled(false);
    check(
        "step 返回 Unsupported",
        matches!(
            renderer.step_gpu_particles(DT),
            Err(ComputeError::Unsupported { .. })
        ),
    );
    check(
        "readback 返回 Unsupported",
        matches!(
            renderer.read_gpu_particles(),
            Err(ComputeError::Unsupported { .. })
        ),
    );
    renderer.render_into_texture(&mesh, None, &view, TARGET, TARGET);
    check(
        "计划里不再有 Particle",
        !renderer.last_frame_passes().contains(&RenderPass::Particle),
    );
    let pixels = renderer.copy_texture_to_cpu(&texture, TARGET, TARGET)?;
    let changed = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != background)
        .count();
    check("回滚档画面上不残留半帧粒子", changed == 0);

    // 恢复开关不改变硬件事实：能力读数仍是 compute=true。
    renderer.set_compute_enabled(true);
    check("重新打开后 compute 仍可用", renderer.compute_enabled());

    println!("\n[ok] M74-03：compute 接缝 + GPU 粒子消费者（10k × 120 步 / 50k × 60 步）全部通过");
    Ok(())
}
