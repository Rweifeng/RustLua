//! P3-R07-3 出口门（`wp0-gpu-particles-golden-v1`）：instanced GPU 粒子的画面与状态对拍。
//!
//! ## 这条样例回答什么
//!
//! R07-2 只证明了"派发与池语义正确"，画面仍走 CPU 档；R07-3 把粒子搬进真正的 instanced
//! 管线（4 顶点 triangle strip、无顶点缓冲、实例流就是状态缓冲）。于是需要一条**场景规模**
//! 的门：10k 粒子、固定 seed、固定 dt，首帧与第 N 帧都真的回读、真的渲染、真的读像素。
//!
//! 1. **首帧逐位相同**：GPU 的初始状态由 CPU 语义生成，回读必须与 CPU burst 完全相等
//!    （`first_frame_mismatches=0`）——这一步出现任何误差都不是精度问题，而是接线问题；
//! 2. **第 30 帧对拍**：同一积分公式跑 0.5 s，位置差 < 1e-4（`max_position_error`）；
//! 3. **第 540 帧仍然有限且在界**：`life ∈ [-dt, max_life]`，位置/速度无 NaN——"越界"在
//!    粒子这里是静默的（NaN 顶点被裁剪掉，画面上只是"少了一批"），所以必须逐槽位查；
//! 4. **instanced 记账**：`instances == capacity`、`vertices == 4×capacity`、
//!    `triangles == 2×capacity`、`draw_calls == 1`、`dropped_draws == 0`。旧路径在这里是
//!    `6×capacity` 顶点 + 每帧 216 B/粒子的写带宽，instanced 之后实例数与顶点数是两个量；
//! 5. **画面非空且可复现**：三次渲染的像素校验和进报告（`pixels_checksum_*`），baseline
//!    比对靠它，而不是靠"看着差不多"；
//! 6. **回滚档**：`set_compute_enabled(false)` 后 configure 返回可读的 `Unsupported`，
//!    计划里没有 `Particle` 通道，像素回到纯背景。
//!
//! ## 无 compute 的机器
//!
//! 以 **exit 3 / BLOCKED** 收场并打印原因，绝不报成功（`没跑 ≠ 通过`）。这条门的判据全部
//! 需要真实 dispatch 与真实附件读回，软件光栅化（WARP）能跑通但不等于真实 GPU——台账里
//! 记明本机后端。
//!
//! 运行：
//! ```text
//! RUSTLUA_REQUIRE_GPU=1 cargo run -p engine --release --example wp0_gpu_particles_golden \
//!   -- --output artifacts/p3/gpu-particles
//! ```
//! 首次落地后把报告复制为 `baselines/p3/gpu-particles` 候选，人工评审后入库。

use std::{env, fs, path::PathBuf, process::ExitCode, time::Instant};

use engine::host::{
    ComputeError, GpuParticleConfig, GpuParticleSnapshot, RenderOutcome, Renderer, SceneMesh,
};
use rustlua_camera::camera::{Camera2D, FrameViews, RenderView, Viewport};
use rustlua_ecs::particle::{ParticleEmitter, ParticleMode, ParticleSystem};
use rustlua_ecs::transform::Vec2;
use rustlua_render::render_plan::RenderPass;
use serde_json::json;

/// 单次 burst 的粒子数：R07-3 的验收规模（R07-4 才上 50k）。
const COUNT: u32 = 10_000;
/// 固定步长与固定 seed：报告可复现的前提，任何一项变了都必须重新评审 baseline。
const DT: f32 = 1.0 / 60.0;
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;
/// 发射寿命。CPU 侧 `max_life = life × (0.5 + rand×0.5)` **恒 < life**，所以
/// `elapsed == life` 的那一帧全部粒子已死而系统尚未回收（回收判据是 `elapsed > life`）。
const LIFE: f32 = 10.0;
/// 对拍帧：0.5 s 时 `max_life ≥ 5` 的粒子全都还活着 ⇒ 与 CPU 参考**逐位对齐**可比。
const NEAR_STEPS: u32 = 30;
/// 画面帧：9.0 s，`max_life < 9` 的那批（≈80%）已经死亡，剩下的仍在屏内。
/// 刻意不到 10.0：f32 累加的 `elapsed` 一旦越过 `life`，池会回收整个系统，那条门的
/// 判据就从"剔除"变成"通道消失"，两回事。
const FAR_STEPS: u32 = 540;
const TARGET: u32 = 256;

fn main() -> ExitCode {
    let output = parse_output();
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return blocked("没有可用的 GPU 适配器，instanced 粒子门无法运行");
    };
    let info = adapter.get_info();
    let (device, queue) =
        match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("wp0-gpu-particles-golden"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })) {
            Ok(pair) => pair,
            Err(error) => return blocked(&format!("默认限制的设备建不出来：{error}")),
        };
    let renderer = match Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
    {
        Ok(renderer) => renderer,
        Err(error) => return blocked(&format!("离屏渲染器建不出来：{error}")),
    };
    let capabilities = renderer.capabilities();
    if !capabilities.compute || !renderer.compute_enabled() {
        let reason = capabilities
            .compute_unavailable_reason()
            .unwrap_or_else(|| "宿主关闭了 compute".to_owned());
        return blocked(&reason);
    }

    let emitter = ParticleEmitter {
        count: COUNT as usize,
        // 只按 burst 发射（rate=0）：连续发射会让每帧的槽位追加与 CPU 参考的步进顺序耦合，
        // 对拍门要的是"一次注入、两条积分线各自跑"。
        rate: 0.0,
        // 无重力、无阻力：位置 = 原点 + 速度 × t，屏内范围可由 `speed × 寿命` 直接算出来，
        // "越界" 检查才有意义（有重力时 9 s 的位移会飘出视口，画面门就退化成"没画"）。
        speed: 8.0,
        life: LIFE,
        size_start: 8.0,
        size_end: 2.0,
        gravity: Vec2::zero(),
        drag: 0.0,
        rotation: 0.0,
        rotation_speed: 0.5,
        color_start: [1.0, 0.35, 0.1, 1.0],
        color_end: [0.6, 0.05, 0.02, 1.0],
        mode: ParticleMode::Gpu,
        seed: Some(SEED),
    };
    let mut renderer = renderer;
    let status = match renderer.configure_gpu_particles(GpuParticleConfig {
        origin: [0.0, 0.0],
        emitter,
        capacity: COUNT,
    }) {
        Ok(status) => status,
        Err(error) => return blocked(&format!("GPU 粒子配置被拒绝：{error}")),
    };

    // ── ① 首帧：GPU 的初始状态必须与 CPU burst **逐位**相同 ──────────────────────────
    let first = match renderer.read_gpu_particles() {
        Ok(snapshot) => snapshot,
        Err(error) => return blocked(&format!("首帧回读失败：{error}")),
    };
    let mut cpu = ParticleSystem::new();
    cpu.burst_with(Vec2::new(0.0, 0.0), &emitter);
    let first_mismatches = count_first_frame_mismatches(&first, &cpu);
    let cpu_near_started = Instant::now();
    for _ in 0..NEAR_STEPS {
        cpu.step(DT);
    }
    let cpu_near_ms = cpu_near_started.elapsed().as_secs_f64() * 1_000.0;

    // ── ② 对拍帧（0.5 s）：与 CPU 参考逐项比，同时留一份画面 ────────────────────────
    let gpu_near_started = Instant::now();
    for step in 0..NEAR_STEPS {
        match renderer.step_gpu_particles(DT) {
            Ok(_) => {}
            Err(error) => return blocked(&format!("第 {step} 步 dispatch 失败：{error}")),
        }
    }
    let gpu_near_ms = gpu_near_started.elapsed().as_secs_f64() * 1_000.0;
    let near = renderer.read_gpu_particles().expect("near-frame readback");
    let (max_position_error, max_velocity_error, near_bounds) = compare_with_cpu(&near, &cpu);
    let near_frame = render_frame(&mut renderer, &device);

    // ── ③ 画面帧（9.0 s）：多数粒子已死，剔除与冻结都必须仍然成立 ───────────────────
    for step in NEAR_STEPS..FAR_STEPS {
        match renderer.step_gpu_particles(DT) {
            Ok(_) => {}
            Err(error) => return blocked(&format!("第 {step} 步 dispatch 失败：{error}")),
        }
    }
    let far = renderer.read_gpu_particles().expect("far-frame readback");
    let far_bounds = state_bounds(&far);
    let dead_far = far.iter().filter(|p| p.life <= 0.0).count();
    let far_frame = render_frame(&mut renderer, &device);
    // 关闭 compute 之后的同一帧：通道消失、像素回到背景（回滚档不残留半帧）。
    renderer.set_compute_enabled(false);
    let rollback = render_frame(&mut renderer, &device);
    let rollback_rejected = matches!(
        renderer.configure_gpu_particles(GpuParticleConfig {
            origin: [0.0, 0.0],
            emitter,
            capacity: COUNT,
        }),
        Err(ComputeError::Unsupported { .. })
    );

    // ── 硬门 ────────────────────────────────────────────────────────────────────────
    let mut failures: Vec<String> = Vec::new();
    let mut gate = |name: &str, ok: bool| {
        println!("  [{}] {name}", if ok { "ok" } else { "FAIL" });
        if !ok {
            failures.push(name.to_owned());
        }
    };
    println!("\n=== ① 首帧逐位相同（接线正确性） ===");
    gate(
        &format!("首帧状态与 CPU burst 逐位相同（mismatches={first_mismatches}）"),
        first_mismatches == 0,
    );
    gate(
        &format!("槽位数 = capacity（{} == {COUNT}）", first.len()),
        first.len() == COUNT as usize,
    );
    println!("\n=== ② 0.5 s 对拍（同一积分公式） ===");
    gate(
        &format!("位置差 < 1e-4（max_position_error={max_position_error:.3e}）"),
        max_position_error < 1e-4,
    );
    gate(
        &format!("速度差 < 1e-4（max_velocity_error={max_velocity_error:.3e}）"),
        max_velocity_error < 1e-4,
    );
    println!("\n=== ③ 9.0 s 状态在界（无 NaN/越界） ===");
    gate(
        &format!("全部状态有限（non_finite={})", near_bounds.3 + far_bounds.3),
        near_bounds.3 == 0 && far_bounds.3 == 0,
    );
    gate(
        &format!(
            "寿命落在 [-dt, max_life]（min_life={:.6} max_life={:.6}）",
            far_bounds.0, far_bounds.1
        ),
        far_bounds.0 >= -(DT + 1e-5) && far_bounds.1 <= LIFE,
    );
    gate(
        &format!("9.0 s 时确有粒子死亡（dead={dead_far}）"),
        dead_far > 0,
    );
    println!("\n=== ④ instanced 记账 ===");
    gate(
        &format!(
            "每帧一次 draw、实例数恒为 capacity（draws={}/instances={}/vertices={}/triangles={}）",
            near_frame.draw_calls, near_frame.instances, near_frame.vertices, near_frame.triangles
        ),
        near_frame.draw_calls == 1
            && near_frame.instances == u64::from(COUNT)
            && near_frame.vertices == 4 * u64::from(COUNT)
            && near_frame.triangles == 2 * u64::from(COUNT),
    );
    gate(
        &format!("本帧没有丢弃的 draw（dropped={})", near_frame.dropped_draws),
        near_frame.dropped_draws == 0,
    );
    gate(
        &format!("状态缓冲 = capacity×48 B（{} B）", status.state_bytes),
        status.state_bytes == u64::from(COUNT) * 48,
    );
    println!("\n=== ⑤ 画面非空且与粒子数同向 ===");
    gate(
        &format!("0.5 s 帧有内容（content={}）", near_frame.content_pixels),
        near_frame.content_pixels > 200,
    );
    gate(
        &format!("9.0 s 帧仍有内容（content={}）", far_frame.content_pixels),
        far_frame.content_pixels > 200,
    );
    gate(
        "两帧画面校验和不同（粒子在动，不是同一张图交差）",
        near_frame.checksum != far_frame.checksum,
    );
    println!("\n=== ⑥ 回滚档（关闭 compute） ===");
    gate(
        &format!(
            "通道消失（Particle 不在计划里，content={}）",
            rollback.content_pixels
        ),
        !rollback.passes_particle && rollback.content_pixels == 0,
    );
    gate(
        &format!(
            "configure 返回可读的 Unsupported（rollback_unsupported={rollback_rejected}）",
            rollback_rejected = rollback_rejected
        ),
        rollback_rejected,
    );

    let report = json!({
        "report_version": "wp0-gpu-particles-golden-v1",
        "scene": "renderer-level instanced GPU particle burst (mode=gpu, fixed seed)",
        "target": { "width": TARGET, "height": TARGET },
        "adapter": { "name": info.name, "backend": format!("{:?}", info.backend) },
        "capabilities": {
            "compute": capabilities.compute,
            "storage_buffers": capabilities.storage_buffers,
            "backend": capabilities.backend_name(),
            "max_compute_workgroup_size": capabilities.max_compute_workgroup_size,
            "max_storage_buffer_binding_size": capabilities.max_storage_buffer_binding_size,
        },
        "semantic": {
            "mode": "gpu",
            "fallback": "none",
            "count": COUNT,
            "capacity": status.capacity,
            "spawned": status.spawned,
            "seed": SEED,
            "delta_seconds": DT,
            "emitter_life": LIFE,
            "near_steps": NEAR_STEPS,
            "far_steps": FAR_STEPS,
            "first_frame_mismatches": first_mismatches,
            "max_position_error": max_position_error,
            "max_velocity_error": max_velocity_error,
            "near_non_finite": near_bounds.3,
            "far_non_finite": far_bounds.3,
            "far_min_life": far_bounds.0,
            "far_max_life_seen": far_bounds.1,
            "far_dead_count": dead_far,
            "state_bytes": status.state_bytes,
            "draw_calls": near_frame.draw_calls,
            "instances": near_frame.instances,
            "vertices": near_frame.vertices,
            "triangles": near_frame.triangles,
            "dropped_draws": near_frame.dropped_draws,
            "content_pixels_near": near_frame.content_pixels,
            "content_pixels_far": far_frame.content_pixels,
            "pixels_checksum_near": near_frame.checksum,
            "pixels_checksum_far": far_frame.checksum,
            "rollback_particle_pass": rollback.passes_particle,
            "rollback_content_pixels": rollback.content_pixels,
            "rollback_unsupported": rollback_rejected,
            // 计时字段带 `_ms` 后缀：compare-golden.ps1 的噪声白名单放行它们。
            "cpu_ms": cpu_near_ms,
            "gpu_ms": gpu_near_ms,
        },
    });
    let text = serde_json::to_string_pretty(&report).expect("serialize golden") + "\n";
    match output {
        Some(path) => {
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                fs::create_dir_all(parent).expect("create golden output directory");
            }
            fs::write(&path, &text).expect("write golden report");
            println!("report: {}", path.display());
        }
        None => print!("{text}"),
    }
    if !failures.is_empty() {
        for failure in &failures {
            eprintln!("FAIL: {failure}");
        }
        return ExitCode::FAILURE;
    }
    println!(
        "wp0_gpu_particles_golden_ok count={COUNT} first_frame_mismatches={first_mismatches} \
         max_position_error={max_position_error:.3e} instances={} draw_calls=1 \
         content_near={} content_far={} backend={:?}",
        near_frame.instances, near_frame.content_pixels, far_frame.content_pixels, info.backend
    );
    ExitCode::SUCCESS
}

/// 一帧的读数：记账 + 像素校验和 + 内容像素数 + 计划里有没有 Particle 通道。
struct FrameReading {
    draw_calls: u64,
    instances: u64,
    vertices: u64,
    triangles: u64,
    dropped_draws: u64,
    content_pixels: u64,
    checksum: u64,
    passes_particle: bool,
}

/// 渲染一帧到离屏附件并读回全部像素。`renderer` 必须是**已经步进过**的那一份。
fn render_frame(renderer: &mut Renderer, device: &wgpu::Device) -> FrameReading {
    let viewport = Viewport::new(TARGET as f32, TARGET as f32).expect("viewport");
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
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("wp0-gpu-particles-golden-target"),
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
    let outcome = renderer.render_into_texture(&empty_mesh(), None, &view, TARGET, TARGET);
    assert!(
        matches!(outcome, RenderOutcome::Presented),
        "离屏渲染没有产出画面"
    );
    let metrics = renderer.last_frame_metrics();
    let pixels = renderer
        .copy_texture_to_cpu(&texture, TARGET, TARGET)
        .expect("pixel readback");
    let background = [pixels[0], pixels[1], pixels[2]];
    FrameReading {
        draw_calls: metrics.draw_calls,
        instances: metrics.instances,
        vertices: metrics.vertices,
        triangles: metrics.triangles,
        dropped_draws: metrics.dropped_draws,
        content_pixels: count_non_background(&pixels, &background),
        checksum: fnv64(&pixels),
        passes_particle: renderer.last_frame_passes().contains(&RenderPass::Particle),
    }
}

/// 首帧逐位比较：GPU 的初始状态**来自** CPU burst，任何不一致都不是精度问题。
fn count_first_frame_mismatches(gpu: &[GpuParticleSnapshot], cpu: &ParticleSystem) -> usize {
    if gpu.len() != cpu.len() {
        return usize::MAX;
    }
    gpu.iter()
        .zip(cpu.particles())
        .filter(|(gpu, cpu)| {
            gpu.position != [cpu.position.x, cpu.position.y]
                || gpu.velocity != [cpu.velocity.x, cpu.velocity.y]
                || gpu.life != cpu.life
                || gpu.max_life != cpu.max_life
                || gpu.size_start != cpu.size_start
                || gpu.size_end != cpu.size_end
                || gpu.rotation != cpu.rotation
        })
        .count()
}

/// 第 N 帧对拍：返回（最大位置差、最大速度差、状态界读数）。
///
/// 只在**没有任何粒子死亡**的那一帧调用（CPU 侧会压缩掉死粒子、槽位就此错位），所以
/// 两个序列按索引一一对应。
fn compare_with_cpu(
    gpu: &[GpuParticleSnapshot],
    cpu: &ParticleSystem,
) -> (f32, f32, (f32, f32, f32, usize)) {
    let mut max_position_error = 0.0f32;
    let mut max_velocity_error = 0.0f32;
    for (index, (gpu, cpu)) in gpu.iter().zip(cpu.particles()).enumerate() {
        let position_error = ((gpu.position[0] - cpu.position.x).abs())
            .max((gpu.position[1] - cpu.position.y).abs());
        let velocity_error = ((gpu.velocity[0] - cpu.velocity.x).abs())
            .max((gpu.velocity[1] - cpu.velocity.y).abs());
        max_position_error = max_position_error.max(position_error);
        max_velocity_error = max_velocity_error.max(velocity_error);
        assert!(
            position_error < 1e-4,
            "粒子 {index} 位置差 {position_error}（gpu={:?} cpu={:?}）",
            gpu.position,
            [cpu.position.x, cpu.position.y]
        );
    }
    let bounds = state_bounds(gpu);
    (max_position_error, max_velocity_error, bounds)
}

/// 状态界读数：（最小 `life`、最大 `life`、最大坐标绝对值、非有限值计数）。
///
/// "最大坐标"这条在 256×256 + 无重力的设定下必然 < 128，写进报告是为了让**将来**给发射器
/// 加重力/阻力时，越界一眼可见，而不是等画面变空才发现。
fn state_bounds(gpu: &[GpuParticleSnapshot]) -> (f32, f32, f32, usize) {
    let mut min_life = f32::INFINITY;
    let mut max_life = f32::NEG_INFINITY;
    let mut max_coordinate = 0.0f32;
    let mut non_finite = 0usize;
    for particle in gpu {
        let finite = particle
            .position
            .iter()
            .chain(particle.velocity.iter())
            .copied()
            .chain([particle.life, particle.max_life])
            .all(|value| value.is_finite());
        if !finite {
            non_finite += 1;
            continue;
        }
        min_life = min_life.min(particle.life);
        max_life = max_life.max(particle.life);
        max_coordinate = max_coordinate
            .max(particle.position[0].abs())
            .max(particle.position[1].abs());
    }
    if non_finite == 0 {
        assert!(
            max_coordinate < 128.0,
            "粒子飘出视口（max_coordinate={max_coordinate}）：发射参数或积分变了，画面门会退化成空帧"
        );
    }
    (min_life, max_life, max_coordinate, non_finite)
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

/// 与背景（第一像素）通道差之和超过阈值的像素数。
fn count_non_background(pixels: &[u8], background: &[u8]) -> u64 {
    let mut count = 0_u64;
    for chunk in pixels.chunks(4) {
        let difference = (i16::from(chunk[0]) - i16::from(background[0])).unsigned_abs()
            + (i16::from(chunk[1]) - i16::from(background[1])).unsigned_abs()
            + (i16::from(chunk[2]) - i16::from(background[2])).unsigned_abs();
        if difference > 24 {
            count += 1;
        }
    }
    count
}

/// FNV-1a/64：整帧像素的校验和。
///
/// 用它而不是"逐字节相等"：跨机器（WARP ↔ 真实 GPU）的混合精度本来就可能差一两个 LSB，
/// 校验和能把"画面结构变了"与"最后一位抖动"分开——差一个量级就先查内容像素。
fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn parse_output() -> Option<PathBuf> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--output" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

/// 环境受限的出口：3 = 跳过（不记通过）；`RUSTLUA_REQUIRE_GPU=1` 时升级为失败。
fn blocked(reason: &str) -> ExitCode {
    eprintln!("BLOCKED: {reason}（没跑 ≠ 通过；CPU 粒子仍是默认与回滚档）");
    if std::env::var("RUSTLUA_REQUIRE_GPU").as_deref() == Ok("1") {
        eprintln!("FAIL: RUSTLUA_REQUIRE_GPU=1 要求这台机器提供 compute");
        return ExitCode::FAILURE;
    }
    ExitCode::from(3)
}
