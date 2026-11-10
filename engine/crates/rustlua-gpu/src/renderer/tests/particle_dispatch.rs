//! P3-R07-2 的派发通道门：脚本侧 `mode=gpu` 的 burst 怎么走进渲染器的 GPU 粒子池，
//! 以及每一条走不通的路怎么**不丢粒子**地回到 CPU 池。
//!
//! 通道的一端是 `LuaRuntime` 的派发队列（Lua `particles.burst(..., {mode='gpu'})` 或宿主
//! `burst_particles`），另一端是 `Renderer` 的 GPU 粒子池。四件事各自一条用例钉住：
//! ① 收下的一批不进 CPU 池（`particles.count` 读不到 = 视觉层语义）；
//! ② 判定不允许（能力位为假）与预算拒绝（池满）都按同一份参数回 CPU，粒子数一个不少；
//! ③ 宿主没来接 ⇒ 队列在本帧开头倒回 CPU 池，原因如实报 `not_wired`；
//! ④ 能力位每帧从渲染器重取，启动时的下推值说错了也不会谎报。
//!
//! 这些用例要真的设备（池长在渲染器里），没有适配器时走 `gpu_skipped`：
//! `RUSTLUA_REQUIRE_GPU=1` 下跳过即 panic（本仓不接受"跳过当通过"）。

use super::*;
use crate::GpuParticleConfig;
use engine::host::runtime_scene::{ParticleDispatchReport, RuntimeScene};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::particle::{ParticleCpuReason, ParticleEmitter, ParticleMode};
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::Scene;

/// 一次要 GPU 的爆发：初速 0、寿命 10s，便于把"派发通道的账"和"模拟的账"分开读。
fn gpu_burst_emitter(count: usize) -> ParticleEmitter {
    ParticleEmitter {
        count,
        speed: 0.0,
        life: 10.0,
        size_start: 4.0,
        size_end: 4.0,
        mode: ParticleMode::Gpu,
        seed: Some(5),
        ..ParticleEmitter::default()
    }
}

fn dispatch_runtime() -> RuntimeScene {
    RuntimeScene::from_scene_edit(None, Scene::new("particle-dispatch")).expect("runtime")
}

/// 有 compute 的离屏渲染器；设备或能力缺席时按本目录口径跳过。
fn compute_renderer() -> Option<Renderer> {
    let (device, queue) = super::compute::headless()?;
    let renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    renderer.compute_enabled().then_some(renderer)
}

fn cpu_pool_count(scene: &mut RuntimeScene) -> usize {
    scene.with_script(|script| script.particle_count())
}

/// ① + ④：两批 gpu burst 被渲染器收下 ⇒ CPU 池为空、GPU 池里 2 个系统 6 个粒子，
/// 且启动时那句错误的"没有 compute"被派发口每帧重取纠正（不谎报）。
#[test]
fn gpu_bursts_reach_the_renderer_pool_and_stay_out_of_the_cpu_pool() {
    let Some(mut renderer) = compute_renderer() else {
        return gpu_skipped("device has no compute");
    };
    let mut scene = dispatch_runtime();
    // 宿主启动时把能力说成 false：派发口必须自己纠正，否则第一批会被记成 no_compute。
    scene.set_particle_compute_available(false);
    scene.with_script(|script| {
        script.burst_particles(Vec2::new(0.0, 0.0), &gpu_burst_emitter(4));
        script.burst_particles(Vec2::new(8.0, 0.0), &gpu_burst_emitter(2));
    });
    let report = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!(
        (report.requested, report.gpu, report.fallback),
        (2, 2, 0),
        "两批都要被渲染器收下：{report:?}"
    );
    assert_eq!(
        (report.deferred, report.overflow, report.fallback_reason),
        (0, 0, None)
    );
    assert_eq!((report.systems, report.gpu_particles), (2, 6));
    assert_eq!(report.steps, 1);
    assert_eq!(
        report,
        scene.last_particle_dispatch(),
        "读数要留在场景上给诊断用"
    );
    assert_eq!(
        cpu_pool_count(&mut scene),
        0,
        "渲染器收下的粒子不许同时出现在 CPU 池（否则等于双份发射）"
    );
    assert_eq!(
        scene.particle_warned_bits(),
        0,
        "全留在 GPU：一条回退日志都不该喊"
    );
    let read = renderer.read_gpu_particles().expect("dispatch readback");
    assert_eq!(read.len(), 6, "调试读回必须数得出派发出去的那 6 个粒子");

    // 空队列再派发一次：没有新爆发 ≠ 清空池，已有的系统照常步进。
    let second = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!(
        (second.requested, second.gpu, second.systems, second.steps),
        (0, 0, 2, 2)
    );
    println!(
        "dispatch_gpu requested={} gpu={} systems={} gpu_particles={} steps={}",
        report.requested, report.gpu, report.systems, report.gpu_particles, second.steps
    );
}

/// ②：能力位为假（宿主显式关掉加速那一档）⇒ 整批按同一份参数回 CPU 池，原因 `no_compute`。
#[test]
fn gpu_bursts_fall_back_to_cpu_when_compute_is_off() {
    let Some(mut renderer) = compute_renderer() else {
        return gpu_skipped("device has no compute");
    };
    renderer.set_compute_enabled(false);
    let mut scene = dispatch_runtime();
    // 下推过 true，但策略位关着 ⇒ 每帧重取只能说"这一帧没有 compute"。
    scene.set_particle_compute_available(true);
    scene.with_script(|script| {
        script.burst_particles(Vec2::new(1.0, 2.0), &gpu_burst_emitter(5));
    });
    let report = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!((report.requested, report.gpu, report.fallback), (1, 0, 1));
    assert_eq!(report.fallback_reason, Some(ParticleCpuReason::NoCompute));
    assert_eq!((report.systems, report.gpu_particles), (0, 0));
    assert_eq!(cpu_pool_count(&mut scene), 5, "回退不丢粒子");
    assert_eq!(
        scene.particle_warned_bits(),
        1 << ParticleCpuReason::NoCompute.index(),
        "每种原因只喊一层"
    );
    // 回到 CPU 池的那 5 个照常被模拟（寿命 10s，跑 1s 不该少一个）。
    scene
        .run_script_steps(60, 1.0 / 60.0, &InputSnapshot::default())
        .expect("step");
    assert_eq!(cpu_pool_count(&mut scene), 5);
    println!(
        "dispatch_no_compute fallback={:?} cpu_particles={}",
        report.fallback_reason,
        cpu_pool_count(&mut scene)
    );
}

/// ③：宿主这一帧没来接（web 宿主、忘记接线的宿主）⇒ 队列在下一帧开头倒回 CPU 池，
/// 派发读数报 `deferred` + `not_wired`；粒子只晚一帧，不静默消失。
#[test]
fn undispatched_bursts_flush_next_frame_and_report_not_wired() {
    let Some(mut renderer) = compute_renderer() else {
        return gpu_skipped("device has no compute");
    };
    let mut scene = dispatch_runtime();
    scene.set_particle_compute_available(true);
    scene.with_script(|script| {
        script.burst_particles(Vec2::new(0.0, 0.0), &gpu_burst_emitter(4));
    });
    assert_eq!(
        cpu_pool_count(&mut scene),
        0,
        "入队后、派发前不该在 CPU 池里"
    );
    // 故意跳过派发口，直接进下一帧。
    scene
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("step");
    assert_eq!(
        cpu_pool_count(&mut scene),
        4,
        "入口保底必须把没人取的爆发倒回 CPU 池"
    );
    let report = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!((report.requested, report.gpu, report.deferred), (0, 0, 1));
    assert_eq!(report.fallback, 1);
    assert_eq!(report.fallback_reason, Some(ParticleCpuReason::NotWired));
    assert!(
        scene.particle_warned_bits() & (1 << ParticleCpuReason::NotWired.index()) != 0,
        "not_wired 要喊得出来，否则'宿主没接线'在日志里是隐形的"
    );
    // 残留是一次性的：再来一次既没有新爆发，也没有新的倒回。
    let again = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!((again.requested, again.deferred, again.fallback), (0, 0, 0));
    println!(
        "dispatch_deferred deferred={} cpu_particles=4",
        report.deferred
    );
}

/// ②（预算那条）：池满时渲染器拒绝 ⇒ 派发通道把这一批原样送回 CPU 池，已有系统一个不动。
#[test]
fn pool_budget_rejection_returns_the_burst_to_cpu() {
    let Some(mut renderer) = compute_renderer() else {
        return gpu_skipped("device has no compute");
    };
    let filler = ParticleEmitter {
        count: 1,
        speed: 0.0,
        life: 100.0,
        ..ParticleEmitter::default()
    };
    for index in 0..MAX_GPU_PARTICLE_SYSTEMS {
        renderer
            .push_gpu_particle_burst(GpuParticleConfig {
                origin: [index as f32, 0.0],
                emitter: filler,
                capacity: 4,
            })
            .expect("fill pool");
    }
    let mut scene = dispatch_runtime();
    scene.with_script(|script| {
        script.burst_particles(Vec2::new(0.0, 0.0), &gpu_burst_emitter(6));
    });
    let report = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!((report.requested, report.gpu, report.fallback), (1, 0, 1));
    assert_eq!(
        report.fallback_reason,
        Some(ParticleCpuReason::DispatchRejected)
    );
    assert_eq!(
        report.systems, MAX_GPU_PARTICLE_SYSTEMS,
        "被拒的一批不许挤掉已有系统"
    );
    assert_eq!(cpu_pool_count(&mut scene), 6, "拒绝不等于丢粒子");
    println!(
        "dispatch_rejected systems={} gpu_particles={} cpu_particles=6",
        report.systems, report.gpu_particles
    );
}

/// 场景声明的 `rate > 0` 发射器：第二帧起由渲染器的连续流接管，不再逐帧落 CPU。
#[test]
fn scene_rate_emitter_becomes_a_persistent_gpu_stream() {
    let Some(mut renderer) = compute_renderer() else {
        return gpu_skipped("device has no compute");
    };
    let mut scene_asset = Scene::new("particle-stream");
    let id = rustlua_scene::scene::SceneEntityId(1);
    scene_asset.add_entity(id, "Rain", None).expect("entity");
    let entity = scene_asset.entities.get_mut(&id).expect("entity mut");
    entity.components.insert(
        "Transform".into(),
        "position=10,20,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity.components.insert(
        "ParticleEmitter".into(),
        "count=1;rate=120;speed=0;life=0.5;size=2;color=1,1,1,1;mode=gpu".into(),
    );
    let mut scene = RuntimeScene::from_scene_edit(None, scene_asset).expect("runtime");
    let input = InputSnapshot::default();

    // 第一帧：宿主还没从渲染器取到"支持连续流"的能力位，按 continuous_rate 回 CPU。
    scene.run_script_steps(1, 1.0 / 60.0, &input).expect("step");
    let first = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!(first.streams, 0, "第一帧还没有流");
    assert_eq!(cpu_pool_count(&mut scene), 2, "第一帧回退不丢粒子");

    // 第二帧：能力位已知，场景发射器变成连续流；这一帧的粒子由渲染器按 rate 抽样。
    scene.run_script_steps(1, 1.0 / 60.0, &input).expect("step");
    let placement = scene.last_particle_placement();
    assert_eq!(
        (placement.cpu, placement.gpu, placement.gpu_requested),
        (0, 1, 1),
        "连续流应如实记成 GPU，而不是折算成 not_wired"
    );
    assert_eq!(placement.fallback, None);
    let second = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!(second.streams, 1, "场景 rate 发射器应建立一个连续流");
    assert_eq!(renderer.gpu_particle_stream_count(), 1);
    assert_eq!(renderer.gpu_particle_stream_keys().len(), 1);
    assert_eq!(
        cpu_pool_count(&mut scene),
        2,
        "流接管后不再往 CPU 池追加（第一帧那 2 颗仍按寿命存在）"
    );
    let alive = renderer
        .read_gpu_particles()
        .expect("readback")
        .iter()
        .filter(|state| state.life > 0.0)
        .count();
    assert!(alive >= 1, "连续流这一帧应至少抽到 1 颗，实际 {alive}");

    // 第三帧没有新请求：流必须继续活着并继续按 rate 发射，而不是被当成 stale 删掉。
    let third = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!(third.streams, 1);
    assert_eq!(renderer.gpu_particle_stream_count(), 1);
    // 宿主两帧没来接派发：上一帧的流请求必须在下一帧入口倒回 CPU，不能静静被覆盖。
    let before = cpu_pool_count(&mut scene);
    scene.run_script_steps(1, 1.0 / 60.0, &input).expect("step");
    scene.run_script_steps(1, 1.0 / 60.0, &input).expect("step");
    assert!(
        cpu_pool_count(&mut scene) > before,
        "没接派发口时连续流请求必须回 CPU 池（{before} → {}）",
        cpu_pool_count(&mut scene)
    );
    let recovered = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!(recovered.streams, 1, "恢复派发后流应继续存在");

    // 实体消失：下一帧场景不再登记这条流，派发口必须把它删掉，而不是留一条孤儿流。
    let handle = scene.scene_entity_handles()[0].1;
    scene.with_script(|script| {
        script.with_world_mut(|world| {
            world
                .despawn(EntityId::from_lua_i64(handle as i64).expect("handle"))
                .expect("despawn");
        });
    });
    scene.run_script_steps(1, 1.0 / 60.0, &input).expect("step");
    let removed = scene.dispatch_gpu_particle_bursts(&mut renderer, 1.0 / 60.0);
    assert_eq!(removed.streams, 0, "实体消失后流必须被回收");
    assert_eq!(renderer.gpu_particle_stream_count(), 0);
    println!(
        "dispatch_stream streams={} systems={} gpu_particles={} removed_streams={}",
        third.streams, third.systems, third.gpu_particles, removed.streams
    );
}

/// 读数结构本身：默认值必须是"什么都没发生"，`Eq` 用来跟 `last_particle_dispatch` 对拍。
#[test]
fn dispatch_report_defaults_to_silence() {
    assert_eq!(
        ParticleDispatchReport::default(),
        ParticleDispatchReport {
            requested: 0,
            gpu: 0,
            fallback: 0,
            deferred: 0,
            overflow: 0,
            fallback_reason: None,
            streams: 0,
            lua_streams: 0,
            systems: 0,
            recycled: 0,
            steps: 0,
            gpu_particles: 0,
        }
    );
}

/// P3-R243：预热把两种目标格式的粒子管线都在启动期编好，第二次调用不再重编；
/// 设备没有粒子管线布局（无存储缓冲）时如实返回 0。
#[test]
fn prewarm_builds_both_formats_once() {
    let Some(mut renderer) = compute_renderer() else {
        return gpu_skipped("device has no compute");
    };
    assert!(
        renderer.gpu_particle_pipelines.is_empty(),
        "构造期不该建粒子管线（P3-R02 的启动预算）"
    );
    if renderer.gpu_particle_pipeline_layout.is_none() {
        assert_eq!(
            renderer.prewarm_gpu_particles(),
            0,
            "没有粒子布局 ⇒ 预热如实返回 0"
        );
        assert!(renderer.gpu_particle_pipelines.is_empty());
        return;
    }
    let expected = if renderer.capabilities.hdr_usable()
        && renderer.config.format != wgpu::TextureFormat::Rgba16Float
    {
        2
    } else {
        1
    };
    let started = std::time::Instant::now();
    let first = renderer.prewarm_gpu_particles();
    let first_elapsed = started.elapsed();
    assert_eq!(first, expected, "首次预热编 LDR（+ 可用时的 HDR）");
    assert_eq!(renderer.gpu_particle_pipelines.len(), expected as usize);
    let started = std::time::Instant::now();
    let second = renderer.prewarm_gpu_particles();
    let second_elapsed = started.elapsed();
    assert_eq!(second, 0, "已热 ⇒ 不重编");
    assert_eq!(renderer.gpu_particle_pipelines.len(), expected as usize);
    // 只打印不判：不同机器/后端差一个量级都正常，账上只写实测，不把计时当门。
    eprintln!("prewarm built={first} first={first_elapsed:?} second={second_elapsed:?}");
}
