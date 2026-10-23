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
use crate::host::GpuParticleConfig;
use crate::host::runtime_scene::{ParticleDispatchReport, RuntimeScene};
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
    assert_eq!(scene.particle_warned, 0, "全留在 GPU：一条回退日志都不该喊");
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
        scene.particle_warned,
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
        scene.particle_warned & (1 << ParticleCpuReason::NotWired.index()) != 0,
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
            systems: 0,
            recycled: 0,
            steps: 0,
            gpu_particles: 0,
        }
    );
}
