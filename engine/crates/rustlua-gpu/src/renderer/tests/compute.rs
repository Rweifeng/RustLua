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
use crate::{ComputeBinding, ComputeError, ComputeRequest, GpuParticleConfig, GpuParticleState};
use rustlua_camera::camera::{Camera2D, RenderView, Viewport};
use rustlua_ecs::particle::{ParticleEmitter, ParticleSystem};
use rustlua_ecs::transform::Vec2;
use rustlua_render::frame::{SceneMesh, WorldSpriteVertex};

/// 测试用的无窗设备（`force_fallback_adapter` ⇒ 本机是 WARP，其他机器可能是 llvmpipe/软件后端）。
///
/// `pub(super)` 是给同目录的派发通道测试（`particle_dispatch.rs`）复用的：两处都要"同一个
/// 适配器口径"，各写一份就会出现"一边跳过一边跑"的假绿。
pub(super) fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
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

/// P3-R07-2 硬门：池里每个 burst 用**自己**的发射参数。
///
/// 这条测试专门盯"合并成一个系统"那种省事但错误的做法：gravity/drag 在 uniform 里是
/// 按系统的，把两个发射器的粒子塞进同一系统等于用 A 的重力模拟 B。判据是初速 0、重力 0
/// 的那一批在步进后**必须还钉在原地**，而受重力那一批必须掉下去。
/// 读回按入池顺序拼接，因此用 `capacity` 的分布切片，不跨系统比较。
#[test]
fn gpu_particle_pool_keeps_each_burst_independent() {
    let Some((device, queue)) = headless() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    if !renderer.compute_enabled() {
        return gpu_skipped("device reports no compute");
    }
    let fallen = ParticleEmitter {
        count: 64,
        speed: 0.0,
        life: 10.0,
        gravity: Vec2::new(0.0, -100.0),
        drag: 0.0,
        seed: Some(11),
        ..ParticleEmitter::default()
    };
    let still = ParticleEmitter {
        count: 32,
        speed: 0.0,
        life: 10.0,
        gravity: Vec2::new(0.0, 0.0),
        drag: 0.0,
        seed: Some(12),
        ..ParticleEmitter::default()
    };
    renderer
        .push_gpu_particle_burst(GpuParticleConfig {
            origin: [0.0, 0.0],
            emitter: fallen,
            capacity: 64,
        })
        .expect("push falling burst");
    renderer
        .push_gpu_particle_burst(GpuParticleConfig {
            origin: [100.0, 0.0],
            emitter: still,
            capacity: 32,
        })
        .expect("push static burst");
    assert_eq!(renderer.gpu_particle_systems(), 2);
    let status = renderer.gpu_particle_status().expect("pool status");
    assert_eq!(status.capacity, 96, "池聚合读数必须逐项求和");
    assert_eq!(status.spawned, 96);
    assert_eq!(status.steps, 0, "还没步进");

    let dt = 1.0 / 60.0;
    for _ in 0..30 {
        let step = renderer.step_gpu_particles(dt).expect("pool step");
        assert_eq!(step.systems, 2, "两个系统都要被推进");
        assert_eq!(step.recycled, 0, "寿命 10s 还没走完");
    }
    assert_eq!(renderer.gpu_particle_status().expect("status").steps, 30);
    let read = renderer.read_gpu_particles().expect("pool readback");
    assert_eq!(read.len(), 96);
    assert!(
        read[..64]
            .iter()
            .all(|particle| particle.position[1] < -1.0),
        "第一个 burst 必须被自己的重力下拉"
    );
    for particle in &read[64..] {
        assert!(
            (particle.position[0] - 100.0).abs() < 1e-4 && particle.position[1].abs() < 1e-4,
            "第二个 burst 被别的系统的参数污染：{:?}",
            particle.position
        );
    }
    println!(
        "gpu_particle_pool_independent systems={} capacity={} spawned={} steps={}",
        renderer.gpu_particle_systems(),
        status.capacity,
        status.spawned,
        renderer.gpu_particle_status().expect("status").steps
    );
}

/// P3-R07-2：池的两条预算（系统数、总槽位）超限时**拒绝**新 burst，已有的一个不动。
///
/// 拒绝而不是排队/挤掉，是"回退不丢粒子"的前提：调用方（派发通道）拿到
/// `InvalidConfig` 就把这一批原样送回 CPU 池。
#[test]
fn gpu_particle_pool_budgets_reject_new_bursts() {
    let Some((device, queue)) = headless() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    if !renderer.compute_enabled() {
        return gpu_skipped("device reports no compute");
    }
    let tiny = ParticleEmitter {
        count: 1,
        speed: 0.0,
        life: 100.0,
        seed: Some(7),
        ..ParticleEmitter::default()
    };
    for index in 0..MAX_GPU_PARTICLE_SYSTEMS {
        renderer
            .push_gpu_particle_burst(GpuParticleConfig {
                origin: [index as f32, 0.0],
                emitter: tiny,
                capacity: 8,
            })
            .expect("填池");
    }
    assert_eq!(renderer.gpu_particle_systems(), MAX_GPU_PARTICLE_SYSTEMS);
    let rejected = renderer.push_gpu_particle_burst(GpuParticleConfig {
        origin: [999.0, 0.0],
        emitter: tiny,
        capacity: 8,
    });
    assert!(
        matches!(rejected, Err(ComputeError::InvalidConfig(_))),
        "池满必须显式拒绝：{rejected:?}"
    );
    assert_eq!(
        renderer.gpu_particle_systems(),
        MAX_GPU_PARTICLE_SYSTEMS,
        "被拒的新 burst 不许挤掉已有系统"
    );
    assert_eq!(
        renderer.gpu_particle_status().expect("status").capacity as usize,
        MAX_GPU_PARTICLE_SYSTEMS * 8
    );

    // 槽位预算是**跨系统求和**：一次大 burst 单独合法，压过上限的那一次被拒。
    renderer.clear_gpu_particles();
    assert_eq!(renderer.gpu_particle_systems(), 0);
    let big = MAX_GPU_PARTICLE_SLOTS - 5_536;
    renderer
        .push_gpu_particle_burst(GpuParticleConfig {
            origin: [0.0, 0.0],
            emitter: tiny,
            capacity: big,
        })
        .expect("预算内的单次大爆发");
    let over = renderer.push_gpu_particle_burst(GpuParticleConfig {
        origin: [0.0, 0.0],
        emitter: tiny,
        capacity: 6_000,
    });
    assert!(
        matches!(over, Err(ComputeError::InvalidConfig(_))),
        "超过槽位预算必须拒绝：{over:?}"
    );
    assert_eq!(renderer.gpu_particle_systems(), 1);
    println!(
        "gpu_particle_pool_budgets systems={} slots={} big={big}",
        renderer.gpu_particle_systems(),
        renderer.gpu_particle_status().expect("status").capacity
    );
}

/// P3-R07-2：走完寿命的系统在步进末尾被回收，且**精确**剔掉它的绑定组缓存条目。
///
/// 回收判据是 `elapsed > emitter.life`（CPU 侧 `max_life = life × 0.5..=1.0` 恒 ≤ `life`，
/// 因此不需要读回就能确定"全死了"）。绑定组不清就会随着"每帧爆发一次"无限增长。
#[test]
fn expired_gpu_particle_systems_are_recycled() {
    let Some((device, queue)) = headless() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    if !renderer.compute_enabled() {
        return gpu_skipped("device reports no compute");
    }
    let short = ParticleEmitter {
        count: 4,
        speed: 0.0,
        life: 0.1,
        seed: Some(1),
        ..ParticleEmitter::default()
    };
    let long = ParticleEmitter {
        count: 4,
        speed: 0.0,
        life: 10.0,
        seed: Some(2),
        ..ParticleEmitter::default()
    };
    renderer
        .push_gpu_particle_burst(GpuParticleConfig {
            origin: [0.0, 0.0],
            emitter: short,
            capacity: 4,
        })
        .expect("short burst");
    renderer
        .push_gpu_particle_burst(GpuParticleConfig {
            origin: [10.0, 0.0],
            emitter: long,
            capacity: 4,
        })
        .expect("long burst");
    assert_eq!(
        renderer.compute_bind_group_cache_len(),
        0,
        "步进前没有绑定组"
    );

    let first = renderer.step_gpu_particles(0.05).expect("step 1");
    assert_eq!((first.systems, first.recycled, first.steps), (2, 0, 1));
    assert_eq!(
        renderer.compute_bind_group_cache_len(),
        2,
        "两个系统各一份绑定组"
    );
    // 恰好等于寿命时**不**回收（判据是严格大于），下一帧才走。
    let second = renderer.step_gpu_particles(0.05).expect("step 2");
    assert_eq!((second.systems, second.recycled), (2, 0));
    let third = renderer.step_gpu_particles(0.05).expect("step 3");
    assert_eq!(third.recycled, 1, "elapsed=0.15 > life=0.1 的那个必须走");
    assert_eq!(third.systems, 1);
    assert_eq!(
        third.steps, 3,
        "steps 取池里最大值（活下来的那个也步了 3 次）"
    );
    assert_eq!(renderer.gpu_particle_systems(), 1, "池里只剩长寿命系统");
    assert_eq!(
        renderer.compute_bind_group_cache_len(),
        1,
        "回收必须剔掉那一份绑定组缓存"
    );
    let read = renderer.read_gpu_particles().expect("read after recycle");
    assert_eq!(read.len(), 4, "读回只剩活着的系统");
    assert!(
        read.iter()
            .all(|particle| (particle.position[0] - 10.0).abs() < 1e-4),
        "剩下的必须是 origin=(10,0) 那一批：{:?}",
        read.first().map(|particle| particle.position)
    );

    // 最后一个系统也走完寿命 ⇒ 池空、聚合读数为 None、读回 `NotConfigured`。
    let mut steps = 0u32;
    while renderer.gpu_particle_systems() > 0 {
        let step = renderer.step_gpu_particles(1.0).expect("drain pool");
        assert_eq!(step.recycled, if step.systems == 0 { 1 } else { 0 });
        steps += 1;
        assert!(steps < 32, "池必须在一小段模拟时间内排空");
    }
    assert!(renderer.gpu_particle_status().is_none());
    assert!(matches!(
        renderer.read_gpu_particles(),
        Err(ComputeError::NotConfigured)
    ));
    assert_eq!(
        renderer.compute_bind_group_cache_len(),
        0,
        "池排空后绑定组缓存也要归零"
    );
    println!("gpu_particle_pool_recycle drain_steps={steps}");
}

/// P3-R07-2 出口门：1k 一次爆发——首帧与 CPU 语义同参、状态全有限、寿命区间正确。
///
/// 首帧一致性是**真判据**而不是走过场：GPU 侧的初始状态就是按 CPU 语义生成的，
/// 所以这里量的是"48 字节槽位编码 + 回读切片"这条链路有没有把状态读花。
#[test]
fn gpu_particle_burst_first_frame_matches_cpu_at_one_thousand() {
    let Some((device, queue)) = headless() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    if !renderer.compute_enabled() {
        return gpu_skipped("device reports no compute");
    }
    let emitter = ParticleEmitter {
        count: 1_000,
        speed: 120.0,
        life: 5.0,
        size_start: 4.0,
        size_end: 1.0,
        gravity: Vec2::new(0.0, -50.0),
        drag: 0.5,
        seed: Some(2_024),
        ..ParticleEmitter::default()
    };
    let origin = [3.0, -2.0];
    let status = renderer
        .push_gpu_particle_burst(GpuParticleConfig {
            origin,
            emitter,
            capacity: 1_000,
        })
        .expect("push 1k burst");
    assert_eq!(status.spawned, 1_000);
    assert_eq!(status.steps, 0);

    let mut cpu = ParticleSystem::new();
    cpu.burst_with(Vec2::new(origin[0], origin[1]), &emitter);
    assert_eq!(cpu.len(), 1_000);
    let gpu = renderer
        .read_gpu_particles()
        .expect("first frame readback without any step");
    assert_eq!(gpu.len(), cpu.len());

    let mut max_position_error = 0.0_f32;
    for (index, (gpu, reference)) in gpu.iter().zip(cpu.particles()).enumerate() {
        assert!(
            gpu.position.iter().all(|value| value.is_finite())
                && gpu.velocity.iter().all(|value| value.is_finite())
                && gpu.life.is_finite()
                && gpu.max_life.is_finite()
                && gpu.size_start.is_finite()
                && gpu.size_end.is_finite()
                && gpu.rotation.is_finite(),
            "粒子 {index} 状态非有限：{gpu:?}"
        );
        assert!(
            gpu.life > 0.0 && gpu.max_life > 0.0 && gpu.max_life <= emitter.life,
            "粒子 {index} 寿命越界：life={} max_life={}",
            gpu.life,
            gpu.max_life
        );
        let error = ((gpu.position[0] - reference.position.x).powi(2)
            + (gpu.position[1] - reference.position.y).powi(2))
        .sqrt();
        max_position_error = max_position_error.max(error);
        assert!(error < 1e-4, "粒子 {index} 首帧位置差 {error}");
        assert!(
            (gpu.max_life - reference.max_life).abs() < 1e-6,
            "粒子 {index} 寿命差 {} vs {}",
            gpu.max_life,
            reference.max_life
        );
    }

    // 2s（< 最短寿命 2.5s）后：一个都没死、状态仍有限、系统还没被回收。
    let dt = 1.0 / 60.0;
    for _ in 0..120 {
        renderer.step_gpu_particles(dt).expect("step");
    }
    let after = renderer.read_gpu_particles().expect("read after 120 steps");
    assert_eq!(after.len(), 1_000);
    assert!(
        after.iter().all(|particle| particle.life > 0.0
            && particle.position.iter().all(|value| value.is_finite())
            && particle.velocity.iter().all(|value| value.is_finite())),
        "120 步后仍有粒子寿命归零或状态非有限"
    );
    assert_eq!(renderer.gpu_particle_systems(), 1, "elapsed=2s < life=5s");
    assert_eq!(renderer.gpu_particle_status().expect("status").steps, 120);
    println!(
        "gpu_particle_1k first_frame max_position_error={max_position_error} \
         spawned={} state_bytes={}",
        status.spawned, status.state_bytes
    );
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

/// P3-R07-3 的两条 instanced 判据：**死实例不产生片元** + **死槽位不被写**。
///
/// 为什么这两条要一起跑：instanced 路径的 draw 范围恒等于 `capacity`（不为"还活着几个"而
/// 回读），于是"死掉的粒子去哪了"完全由顶点着色器的 `life <= 0` 分支决定；而 compute 的
/// `life <= 0` 早退又决定了死槽是否被继续积分。两处都静默： shader 少一个分支不会报错，
/// 只会让画面上"早该消失的粒子还在飘"。
///
/// 时间轴是刻意选的：`dt = 1.0`、`life = 10.0`，而 CPU 侧 `max_life = life × (0.5 + rand×0.5)`
/// 恒 **< 10**，所以第 10 步之后全部粒子死亡、而池的回收判据是 `elapsed > life`（10.0 > 10.0
/// 为假）⇒ 系统仍在池里。这正是"系统在、粒子全死"的窗口，剔除与不写都只能靠这两条分支。
#[test]
fn dead_gpu_particle_instances_draw_nothing_and_stay_frozen() {
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
        label: Some("rustlua-test-particle-cull-target"),
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
    let mesh = empty_mesh();

    let count = 1024u32;
    let emitter = ParticleEmitter {
        count: count as usize,
        speed: 1.0,
        life: 10.0,
        size_start: 8.0,
        size_end: 4.0,
        gravity: Vec2::new(0.0, -1.0),
        drag: 0.1,
        rotation_speed: 0.5,
        color_start: [1.0, 0.0, 0.0, 1.0],
        color_end: [1.0, 0.0, 0.0, 1.0],
        ..ParticleEmitter::default()
    };
    renderer
        .configure_gpu_particles(GpuParticleConfig {
            origin: [0.0, 0.0],
            emitter,
            capacity: count,
        })
        .expect("configure");

    // 第 9 步（elapsed = 9.0）：max_life ∈ [5, 10) ⇒ 同时存在死粒子与活粒子。
    for _ in 0..9 {
        renderer.step_gpu_particles(1.0).expect("step 9");
    }
    let mid = renderer.read_gpu_particles().expect("read at elapsed=9");
    assert_eq!(mid.len(), count as usize);
    let dead_at_nine: Vec<usize> = mid
        .iter()
        .enumerate()
        .filter(|(_, particle)| particle.life <= 0.0)
        .map(|(index, _)| index)
        .collect();
    let alive_at_nine = count as usize - dead_at_nine.len();
    assert!(
        !dead_at_nine.is_empty() && alive_at_nine > 0,
        "第 9 步应当同时有死/活粒子（死 {} / 活 {alive_at_nine}）：判据失效，先查 max_life 分布",
        dead_at_nine.len()
    );

    // 判据一（画面侧）：还有活粒子时，同一份相机确实画出了像素。
    renderer.render_into_texture(&mesh, None, &view, size, size);
    assert!(
        renderer.last_frame_passes().contains(&RenderPass::Particle),
        "计划里必须有 Particle：{:?}",
        renderer.last_frame_passes()
    );
    let pixels = renderer
        .copy_texture_to_cpu(&texture, size, size)
        .expect("pixels while alive");
    let background = [pixels[0], pixels[1], pixels[2]];
    let changed_alive = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != background)
        .count();
    let metrics = renderer.last_frame_metrics();
    println!(
        "gpu_particle_instanced alive_frame dead={} alive={} changed_pixels={} \
         draw_calls={} instances={} vertices={} triangles={}",
        dead_at_nine.len(),
        alive_at_nine,
        changed_alive,
        metrics.draw_calls,
        metrics.instances,
        metrics.vertices,
        metrics.triangles
    );
    assert!(
        changed_alive > 100,
        "活粒子没画出来：changed={changed_alive}"
    );
    // 判据二（记账侧）：instanced 的顶点/三角形是**每实例**的量，不是 draw 范围。
    // 旧的 `record_vertex_draw` 在这里会得到 instances=0，所以这条要单独钉。
    assert_eq!(metrics.draw_calls, 1, "一个 pass 一次 draw");
    assert_eq!(metrics.instances, u64::from(count), "实例数 = 槽位数");
    assert_eq!(metrics.vertices, 4 * u64::from(count));
    assert_eq!(metrics.triangles, 2 * u64::from(count));

    // 第 10 步（elapsed = 10.0 == life）：全部粒子寿命走完，系统仍在池里。
    renderer.step_gpu_particles(1.0).expect("step 10");
    let after = renderer.read_gpu_particles().expect("read at elapsed=10");
    assert_eq!(
        renderer.gpu_particle_systems(),
        1,
        "elapsed == life 还没到回收线"
    );
    assert!(
        after.iter().all(|particle| particle.life <= 0.0),
        "max_life 恒 < life，第 10 步后不应再有活粒子"
    );
    // 判据三（写带宽侧）：第 9 步已死的槽位，状态逐位不变 ⇒ compute 对它一个字节都没写。
    for &index in &dead_at_nine {
        assert_eq!(
            after[index], mid[index],
            "死粒子槽位 {index} 被改写了：{:?} -> {:?}",
            mid[index], after[index]
        );
    }

    // 判据一（收口）：全死的一帧 draw 范围仍是 capacity，但像素回到纯背景。
    renderer.render_into_texture(&mesh, None, &view, size, size);
    assert!(
        renderer.last_frame_passes().contains(&RenderPass::Particle),
        "系统在、计划就必须在（剔除发生在着色器里）"
    );
    let pixels = renderer
        .copy_texture_to_cpu(&texture, size, size)
        .expect("pixels after all dead");
    let changed_dead = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != [pixels[0], pixels[1], pixels[2]])
        .count();
    println!("gpu_particle_instanced dead_frame changed_pixels={changed_dead}");
    assert_eq!(
        changed_dead, 0,
        "死实例必须被顶点着色器剔除（四角落在裁剪体外），实际变化像素 {changed_dead}"
    );
}

/// P3-R07-4：透视视图下 quad 必须**正对相机**（3D billboard），而不是躺在世界 XY 平面里。
///
/// 判据用"侧视相机"：相机在 +X 轴看向原点，世界 XY 平面上的 quad 恰好是一条侧影（投影面积
/// 为零）；只有把两个展开轴换成相机右/上基才画得出像素。2D（正交）情形仍走历史路径，由
/// `wp0_gpu_particles_golden` 的逐像素校验和钉住，所以这条只验透视分支。
#[test]
fn gpu_particle_quads_billboard_to_a_perspective_camera() {
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
        label: Some("rustlua-test-particle-billboard-target"),
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
    let camera3d = Camera3D::perspective(
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("3d camera");
    let overlay = Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("camera");
    renderer.set_frame_views(FrameViews::layered(
        RenderView::main_3d(camera3d, viewport, [0.0, 0.0, 0.0, 1.0]),
        RenderView::overlay_2d(overlay, viewport),
    ));
    let mesh = empty_mesh();
    let emitter = ParticleEmitter {
        count: 1,
        speed: 0.0,
        life: 10.0,
        size_start: 8.0,
        size_end: 8.0,
        gravity: Vec2::zero(),
        drag: 0.0,
        rotation: 0.0,
        rotation_speed: 0.0,
        color_start: [1.0, 0.0, 0.0, 1.0],
        color_end: [1.0, 0.0, 0.0, 1.0],
        ..ParticleEmitter::default()
    };
    renderer
        .configure_gpu_particles(GpuParticleConfig {
            origin: [0.0, 0.0],
            emitter,
            capacity: 1,
        })
        .expect("configure");
    renderer.render_into_texture(&mesh, None, &view, size, size);
    assert!(
        renderer.last_frame_passes().contains(&RenderPass::Particle),
        "计划里必须有 Particle：{:?}",
        renderer.last_frame_passes()
    );
    let pixels = renderer
        .copy_texture_to_cpu(&texture, size, size)
        .expect("pixels");
    let background = [pixels[0], pixels[1], pixels[2]];
    let changed = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != background)
        .count();
    let metrics = renderer.last_frame_metrics();
    println!(
        "gpu_particle_3d_billboard changed_pixels={changed} passes={:?} instances={} \
         vertices={} dropped={} center={:?}",
        renderer.last_frame_passes(),
        metrics.instances,
        metrics.vertices,
        metrics.dropped_draws,
        &pixels[(64 * 128 + 64) * 4..(64 * 128 + 64) * 4 + 4]
    );
    assert!(
        changed > 100,
        "侧视透视相机下粒子没画出来（changed={changed}）：quad 仍躺在世界 XY 平面里"
    );
}

/// 布局一致性：公开的 `GpuParticleState` 必须与 shader 的 48 字节结构一致。
#[test]
fn public_state_layout_is_stable() {
    assert_eq!(std::mem::size_of::<GpuParticleState>(), 48);
    assert_eq!(std::mem::size_of::<WorldSpriteVertex>(), 36);
}
