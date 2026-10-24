//! P3-R07-4 出口门（`wp0-gpu-particles-50k-v1`）：50k 粒子的两档帧成本读数 + 默认档判定。
//!
//! ## 这条样例回答什么
//!
//! R07-3 证明了 instanced 路径在 10k 规模**正确**；R07-4 要回答的是"值不值得默认开"。
//! 本样例用同一 emitter（固定 seed/dt）跑两条路径，各记三笔账：
//!
//! 1. **CPU 档每帧成本** = `ParticleSystem::step` + 逐粒子发射（`push_particles` 的同一套
//!    `Sprite::new`/`with_tint`/`Transform`/`SpriteBatchItem`）+ `build_world_into` 展开成
//!    6 顶点/粒子的世界 Sprite 顶点。这是 GPU 档要替换掉的那部分工作；
//! 2. **GPU 档每帧 CPU 成本** = `step_gpu_particles`（encode + dispatch submit）+
//!    `render_into_texture`（instanced draw 的 encode），不含 GPU 执行；另记 `gpu_wall`
//!    （每帧提交后 `device.poll(Wait)`）作为**总帧时间**的下界读数；
//! 3. **内存**：50k × 48 B 的状态缓冲 = 2.4 MB，远低于 16 MB 预算；帧间不增长（状态缓冲只在
//!    configure 时分配一次，`gpu_particle_status().state_bytes` 前后必须相等）。
//!
//! ## 默认档判定
//!
//! 计划 §6.7 的门槛：GPU 档 CPU 帧时间 ≤ CPU 档的 50%，**或**总帧时间 ≤ CPU 档的 80%；
//! 达不到就保留 `cpu` 默认档并在报告里写明原因。本机是 **WARP**（软件光栅化），
//! `gpu_wall` 不代表真实 GPU，所以只要总帧时间没有优于 CPU 档，判定就保守地留在 `cpu`；
//! 真实 GPU 上的复测是未完成项，不能拿 WARP 的 CPU 侧收益去翻默认档。
//!
//! 无 compute 的机器：以 **exit 3 / BLOCKED** 收场并打印原因（没跑 ≠ 通过）。
//!
//! 运行：
//! ```text
//! RUSTLUA_REQUIRE_GPU=1 cargo run -p engine --release --example wp0_gpu_particles_50k \
//!   -- --output artifacts/p3/gpu-particles-50k
//! ```

// 报告的 `json!` 字面量字段多，默认宏展开深度不够。
#![recursion_limit = "256"]

use std::{
    env, fs,
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

use engine::host::SceneMesh;
use rustlua_camera::camera::{Camera2D, FrameViews, RenderView, Viewport};
use rustlua_ecs::particle::{ParticleEmitter, ParticleMode, ParticleSystem};
use rustlua_ecs::transform::{Transform, Vec2};
use rustlua_gpu::{GpuParticleConfig, OffscreenRequest, OffscreenTarget, Renderer};
use rustlua_infra::par::Parallelism;
use rustlua_render::frame::WorldSpriteVertex;
use rustlua_render::sprite::Sprite;
use rustlua_render::sprite_batch::{SpriteBatch, SpriteBatchItem, SpriteDrawBatch};
use serde_json::json;

/// R07-4 的验收规模。
const COUNT: u32 = 50_000;
/// 固定步长与 seed：两档对拍与复现的前提。
const DT: f32 = 1.0 / 60.0;
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;
const LIFE: f32 = 10.0;
/// 计时帧数与预热帧数：预热吃掉首帧的管线编译/缓冲首次触碰，不混进均值。
const FRAMES: u32 = 10;
const WARMUP_FRAMES: u32 = 2;
/// 预算：50k 状态缓冲 2.4 MB，远低于 16 MB 上限（计划 §6.7 内存门）。
const MEMORY_BUDGET_BYTES: u64 = 16 * 1024 * 1024;
const TARGET: u32 = 256;

fn main() -> ExitCode {
    let output = parse_output();
    let fixture = match OffscreenTarget::acquire(&OffscreenRequest::new(
        "wp0-gpu-particles-50k",
        TARGET,
        TARGET,
    )) {
        Ok(fixture) => fixture,
        Err(error) => return blocked(&format!("离屏设备与目标纹理起不来：{error}")),
    };
    let device = fixture.device();
    let info = fixture.adapter_info();
    let mut renderer = match Renderer::new_offscreen(
        device.clone(),
        fixture.queue().clone(),
        None,
        false,
        false,
    ) {
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
        // 只按 burst 发射（rate=0）；场景侧连续发射不进 GPU 档。
        rate: 0.0,
        // 与 R07-3 同一组积分参数（无重力 ⇒ 10 帧内全部存活，两档样本量一致）；速度调大是为了
        // 10 帧（0.167 s）的窗口里粒子已经铺开——否则 50k 全叠在中心十几像素内，像素门
        // 只剩"画没画"的信息量，看不出粒子数。
        speed: 40.0,
        life: LIFE,
        size_start: 8.0,
        size_end: 2.0,
        gravity: Vec2::zero(),
        drag: 0.1,
        rotation: 0.0,
        rotation_speed: 0.5,
        color_start: [1.0, 0.35, 0.1, 1.0],
        color_end: [0.6, 0.05, 0.02, 1.0],
        mode: ParticleMode::Gpu,
        seed: Some(SEED),
    };
    let status = match renderer.configure_gpu_particles(GpuParticleConfig {
        origin: [0.0, 0.0],
        emitter,
        capacity: COUNT,
    }) {
        Ok(status) => status,
        Err(error) => return blocked(&format!("GPU 粒子配置被拒绝：{error}")),
    };

    let camera = Camera2D::new(Vec2::new(0.0, 0.0), 1.0)
        .expect("camera")
        .view_projection(
            Viewport::new(TARGET as f32, TARGET as f32).expect("viewport"),
            0.1,
            100.0,
        )
        .expect("camera matrix");
    renderer.set_world_sprite_camera(camera);
    renderer.set_frame_views(FrameViews::overlay_only(RenderView::main_2d(
        Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("camera"),
        Viewport::new(TARGET as f32, TARGET as f32).expect("viewport"),
        [0.0, 0.0, 0.0, 1.0],
    )));
    let texture = fixture.texture();
    let view = fixture.view();
    let mesh = empty_mesh();

    // ── CPU 档：同一套积分 + 逐粒子发射 + 世界 Sprite 顶点展开 ───────────────────────
    let mut cpu = ParticleSystem::new();
    cpu.burst_with(Vec2::new(0.0, 0.0), &emitter);
    let mut batch = SpriteBatch::new();
    let mut vertices: Vec<WorldSpriteVertex> = Vec::with_capacity(COUNT as usize * 6);
    let mut draws: Vec<SpriteDrawBatch> = Vec::new();
    let mut cpu_step = Duration::ZERO;
    let mut cpu_emit = Duration::ZERO;
    for frame in 0..(WARMUP_FRAMES + FRAMES) {
        let started = Instant::now();
        cpu.step(DT);
        cpu_step += started.elapsed();
        let started = Instant::now();
        batch.clear();
        vertices.clear();
        draws.clear();
        for particle in cpu.particles() {
            let size = particle.size();
            if size <= 0.0 {
                continue;
            }
            let color = particle.color();
            let sprite = Sprite::new(Vec2::new(size, size))
                .and_then(|sprite| sprite.with_tint([color[0], color[1], color[2], color[3]]))
                .expect("particle sprite");
            let transform = Transform {
                rotation: particle.rotation,
                ..Transform::from_position(particle.position)
            };
            batch.push(SpriteBatchItem::new(0, 1, sprite, transform));
        }
        batch
            .build_world_into(&mut vertices, &mut draws, None, Parallelism::SEQUENTIAL)
            .expect("particle world vertices");
        cpu_emit += started.elapsed();
        if frame + 1 == WARMUP_FRAMES {
            // 预热结束：把预热成本从累计里扣掉（后续计时只算正式帧）。
            cpu_step = Duration::ZERO;
            cpu_emit = Duration::ZERO;
        }
    }
    // 上面把预热后的累计清零，所以这里除以正式帧数得到每帧均值；顶点/批次数是最后一帧的读数。
    let cpu_step_ms = cpu_step.as_secs_f64() * 1_000.0 / f64::from(FRAMES);
    let cpu_emit_ms = cpu_emit.as_secs_f64() * 1_000.0 / f64::from(FRAMES);
    let cpu_frame_ms = cpu_step_ms + cpu_emit_ms;
    let cpu_live = cpu.len();

    // ── GPU 档：dispatch + instanced draw；CPU 时间不含 GPU 执行，wall 含 ─────────────
    let mut gpu_dispatch = Duration::ZERO;
    let mut gpu_draw = Duration::ZERO;
    let mut gpu_wall = Duration::ZERO;
    for frame in 0..(WARMUP_FRAMES + FRAMES) {
        let frame_started = Instant::now();
        let started = Instant::now();
        match renderer.step_gpu_particles(DT) {
            Ok(_) => {}
            Err(error) => return blocked(&format!("第 {frame} 步 dispatch 失败：{error}")),
        }
        gpu_dispatch += started.elapsed();
        let started = Instant::now();
        renderer.render_into_texture(&mesh, None, view, TARGET, TARGET);
        gpu_draw += started.elapsed();
        if let Err(error) = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        }) {
            return blocked(&format!("等待第 {frame} 帧 GPU 完成失败：{error}"));
        }
        gpu_wall += frame_started.elapsed();
        if frame + 1 == WARMUP_FRAMES {
            gpu_dispatch = Duration::ZERO;
            gpu_draw = Duration::ZERO;
            gpu_wall = Duration::ZERO;
        }
    }
    let gpu_dispatch_ms = gpu_dispatch.as_secs_f64() * 1_000.0 / f64::from(FRAMES);
    let gpu_draw_ms = gpu_draw.as_secs_f64() * 1_000.0 / f64::from(FRAMES);
    let gpu_cpu_frame_ms = gpu_dispatch_ms + gpu_draw_ms;
    let gpu_wall_ms = gpu_wall.as_secs_f64() * 1_000.0 / f64::from(FRAMES);
    let metrics = renderer.last_frame_metrics();
    let final_status = renderer
        .gpu_particle_status()
        .expect("GPU 粒子系统应当还在池里（10 帧 < 10 s 寿命）");
    let snapshot = match renderer.read_gpu_particles() {
        Ok(snapshot) => snapshot,
        Err(error) => return blocked(&format!("最终回读失败：{error}")),
    };
    let non_finite = snapshot
        .iter()
        .filter(|particle| {
            !particle.position.iter().all(|value| value.is_finite())
                || !particle.velocity.iter().all(|value| value.is_finite())
                || !particle.life.is_finite()
                || !particle.max_life.is_finite()
        })
        .count();
    let gpu_alive = snapshot
        .iter()
        .filter(|particle| particle.life > 0.0)
        .count();
    let pixels = renderer
        .copy_texture_to_cpu(texture, TARGET, TARGET)
        .expect("final pixel readback");
    let background = [pixels[0], pixels[1], pixels[2]];
    let content_pixels = pixels
        .chunks_exact(4)
        .filter(|pixel| {
            (i16::from(pixel[0]) - i16::from(background[0])).unsigned_abs()
                + (i16::from(pixel[1]) - i16::from(background[1])).unsigned_abs()
                + (i16::from(pixel[2]) - i16::from(background[2])).unsigned_abs()
                > 24
        })
        .count() as u64;
    let particle_buffer_bytes = final_status.state_bytes + 64;
    let memory_ok = particle_buffer_bytes <= MEMORY_BUDGET_BYTES;
    let buffer_stable = final_status.state_bytes == status.state_bytes;
    let cpu_side_ratio = gpu_cpu_frame_ms / cpu_frame_ms;
    let total_ratio = gpu_wall_ms / cpu_frame_ms;
    let cpu_side_gate = cpu_side_ratio <= 0.5;
    let total_gate = total_ratio <= 0.8;
    // 计划门槛是「CPU 侧 ≤50% 或 总帧 ≤80%」；但 WARP 的总帧读数不代表真实 GPU，只要总帧
    // 没赢过 CPU 档就不翻默认值——宁可少赚，也不拿软件光栅化的收支去改默认行为。
    let default_mode = if (cpu_side_gate || total_gate) && total_ratio <= 1.0 {
        "gpu"
    } else {
        "cpu"
    };
    let default_mode_reason = if default_mode == "cpu" {
        format!(
            "总帧时间未优于 CPU 档（gpu_wall={gpu_wall_ms:.3} ms vs cpu_frame={cpu_frame_ms:.3} ms，ratio={total_ratio:.3}）；本机是 WARP（{}），总帧读数不代表真实 GPU，保留 cpu 默认档",
            info.name
        )
    } else {
        format!(
            "CPU 侧 ratio={cpu_side_ratio:.3}、总帧 ratio={total_ratio:.3}，两条门槛至少一条成立"
        )
    };

    let mut failures: Vec<String> = Vec::new();
    let mut gate = |name: &str, ok: bool| {
        println!("  [{}] {name}", if ok { "ok" } else { "FAIL" });
        if !ok {
            failures.push(name.to_owned());
        }
    };
    println!("\n=== ① 规模与状态（50k） ===");
    gate(
        &format!(
            "capacity/spawned = {COUNT}（{} / {}）",
            status.capacity, status.spawned
        ),
        status.capacity == COUNT && status.spawned == COUNT,
    );
    gate(
        &format!("GPU 回读槽位 = capacity（{}）", snapshot.len()),
        snapshot.len() == COUNT as usize,
    );
    gate(
        &format!("全部状态有限（non_finite={non_finite}）"),
        non_finite == 0,
    );
    gate(
        &format!("10 帧后仍有活粒子（alive={gpu_alive} / cpu_live={cpu_live}）"),
        gpu_alive == COUNT as usize && cpu_live == COUNT as usize,
    );
    println!("\n=== ② instanced 记账（50k 一次 draw） ===");
    gate(
        &format!(
            "draws={} instances={} vertices={} triangles={} dropped={}",
            metrics.draw_calls,
            metrics.instances,
            metrics.vertices,
            metrics.triangles,
            metrics.dropped_draws
        ),
        metrics.draw_calls == 1
            && metrics.instances == u64::from(COUNT)
            && metrics.vertices == 4 * u64::from(COUNT)
            && metrics.triangles == 2 * u64::from(COUNT)
            && metrics.dropped_draws == 0,
    );
    gate(
        &format!("画面有内容（content={content_pixels}）"),
        content_pixels > 200,
    );
    println!("\n=== ③ 内存门（≤ 16 MB、帧间不增长） ===");
    gate(
        &format!(
            "粒子缓冲 = {particle_buffer_bytes} B（state={} + params=64），预算 {MEMORY_BUDGET_BYTES} B",
            final_status.state_bytes
        ),
        memory_ok && final_status.state_bytes == u64::from(COUNT) * 48,
    );
    gate(
        &format!(
            "状态缓冲帧间不变（{} == {}）",
            final_status.state_bytes, status.state_bytes
        ),
        buffer_stable,
    );
    println!("\n=== ④ 两档读数（CPU 侧 / 总帧） ===");
    gate(
        &format!(
            "CPU 档 step={cpu_step_ms:.3} ms emit={cpu_emit_ms:.3} ms frame={cpu_frame_ms:.3} ms",
        ),
        cpu_frame_ms > 0.0,
    );
    gate(
        &format!(
            "GPU 档 dispatch={gpu_dispatch_ms:.3} ms draw={gpu_draw_ms:.3} ms cpu_frame={gpu_cpu_frame_ms:.3} ms wall={gpu_wall_ms:.3} ms"
        ),
        gpu_cpu_frame_ms > 0.0 && gpu_wall_ms > 0.0,
    );
    gate(
        &format!(
            "门槛读数：cpu_side_ratio={cpu_side_ratio:.3}（≤0.5? {cpu_side_gate}） total_ratio={total_ratio:.3}（≤0.8? {total_gate}） ⇒ default_mode={default_mode}"
        ),
        default_mode == "cpu" || (cpu_side_gate || total_gate),
    );

    let report = json!({
        "report_version": "wp0-gpu-particles-50k-v1",
        "scene": "renderer-level 50k GPU particle burst vs CPU particle emission (fixed seed/dt)",
        "target": { "width": TARGET, "height": TARGET },
        "adapter": { "name": info.name, "backend": format!("{:?}", info.backend) },
        "capabilities": {
            "compute": capabilities.compute,
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
            "frames": FRAMES,
            "warmup_frames": WARMUP_FRAMES,
            "cpu_step_ms": cpu_step_ms,
            "cpu_emit_ms": cpu_emit_ms,
            "cpu_frame_ms": cpu_frame_ms,
            "gpu_dispatch_ms": gpu_dispatch_ms,
            "gpu_draw_ms": gpu_draw_ms,
            "gpu_cpu_frame_ms": gpu_cpu_frame_ms,
            "gpu_wall_ms": gpu_wall_ms,
            "cpu_side_ratio": cpu_side_ratio,
            "total_ratio": total_ratio,
            "cpu_side_gate": cpu_side_gate,
            "total_gate": total_gate,
            "default_mode": default_mode,
            "default_mode_reason": default_mode_reason,
            "draw_calls": metrics.draw_calls,
            "instances": metrics.instances,
            "vertices": metrics.vertices,
            "triangles": metrics.triangles,
            "dropped_draws": metrics.dropped_draws,
            "state_bytes": final_status.state_bytes,
            "particle_buffer_bytes": particle_buffer_bytes,
            "memory_budget_bytes": MEMORY_BUDGET_BYTES,
            "memory_ok": memory_ok,
            "buffer_stable": buffer_stable,
            "steps": final_status.steps,
            "gpu_alive": gpu_alive,
            "cpu_live": cpu_live,
            "non_finite": non_finite,
            "content_pixels": content_pixels,
        },
    });
    let text = serde_json::to_string_pretty(&report).expect("serialize 50k report") + "\n";
    match output {
        Some(path) => {
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                fs::create_dir_all(parent).expect("create 50k output directory");
            }
            fs::write(&path, &text).expect("write 50k report");
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
        "wp0_gpu_particles_50k_ok count={COUNT} cpu_frame_ms={cpu_frame_ms:.3} \
         gpu_cpu_frame_ms={gpu_cpu_frame_ms:.3} gpu_wall_ms={gpu_wall_ms:.3} \
         state_bytes={} content={content_pixels} default_mode={default_mode} backend={:?}",
        final_status.state_bytes, info.backend
    );
    ExitCode::SUCCESS
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

fn parse_output() -> Option<PathBuf> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--output" {
            return args.next().map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix("--output=") {
            return Some(PathBuf::from(value));
        }
    }
    None
}

fn blocked(reason: &str) -> ExitCode {
    eprintln!("wp0_gpu_particles_50k BLOCKED: {reason}");
    ExitCode::from(3)
}
