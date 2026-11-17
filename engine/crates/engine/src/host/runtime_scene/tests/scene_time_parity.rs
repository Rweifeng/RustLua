//! 跨 crate 门：**脚本读回的程序化姿态**必须等于**引擎画出来的姿态**。
//!
//! ## 这条门补的是什么
//!
//! `InstanceBatch3D` 的实例可以带 motion（沿路径流动）。它的姿态由**场景时钟**
//! 求值：`instance_matrix_at(index, time)`。同一个时钟被两处消费：
//!
//! | 消费方 | 入口 |
//! |---|---|
//! | 引擎渲染提取 | `frame_data.rs` → `renderable_state().scene_time()` |
//! | 脚本 | `mesh.batch_pose(id, index)` → 同一个 `scene_time()` |
//!
//! 两处读的是同一个 `Cell<f32>`，设计上没问题。但**没有任何测试**：
//! `renderable_lua.rs` 里 3 条测试全部关于 cells3d/terrain/instance_table，
//! `engine` 侧的 `gpu_motion.rs` 只测"批次有没有被提取"，从不看时钟。
//!
//! `scene_time` 一旦不同步（例如宿主忘了在 `step` 里推进、或推进了两次），
//! 症状是"脚本以为蚂蚁在 A 点、画面把它画在 B 点"——玩家侧表现为穿模、点击判定偏移。
//! 这类错位不会崩溃，只是所有交互都差半个身位，极难归因。
//!
//! ## 为什么用"脚本读回 vs 引擎求值"对照
//!
//! 不直接断言"时钟等于 N 秒"（那只是把实现抄一遍）。断言的是**两个消费方看到的一致**：
//! 让脚本读 `batch_pose`，再让引擎自己对同一时刻求值，两边必须逐位相同。
//! 这才是"同一个时刻"这个承诺的真正含义。

use super::*;
use rustlua_input::input::InputSnapshot;
use rustlua_math::math3d::Vec3;
use rustlua_render::instance_batch::{
    BatchMotion3D, Instance3D, InstanceBatch3D, InstanceMotion3D, MotionKind,
};
use rustlua_scene::scene::{Scene, SceneEntityId};

/// 两只实例、一条沿 +X 的路径（与 `gpu_motion.rs` 的辅助同形，那边只关心"有没有被求值"）。
fn motion_batch() -> InstanceBatch3D {
    let instances = (0..2)
        .map(|index| Instance3D {
            position: [index as f32, 0.0, 0.0],
            motion: Some(InstanceMotion3D {
                offset: index as f32,
                speed: 1.0,
                align: false,
                ..InstanceMotion3D::default()
            }),
            ..Instance3D::default()
        })
        .collect();
    InstanceBatch3D {
        mesh: "assets/models/ant.glb".to_owned(),
        material: None,
        instances,
        motion: Some(BatchMotion3D {
            kind: MotionKind::Path,
            path: vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0]],
            closed: false,
            speed: 1.0,
        }),
    }
}

/// 建一个只有"一群会动的实例"的场景，返回运行时与它的场景实体 id。
fn runtime_with_batch() -> (RuntimeScene, SceneEntityId) {
    let mut scene = Scene::new("scene_time_parity");
    scene
        .add_entity(SceneEntityId(1), "Crowd", None)
        .expect("add entity");
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(game_root().as_path()), scene).expect("runtime scene");
    let handle = runtime
        .with_script(|script| script.scene_entity_handle(SceneEntityId(1)))
        .expect("scene entity handle");
    runtime.with_script(|script| {
        script.with_world_mut(|world| {
            world
                .insert(handle, std::sync::Arc::new(motion_batch()))
                .expect("insert batch");
        });
    });
    (runtime, SceneEntityId(1))
}

/// 脚本 `mesh.batch_pose` 读到的第 `index` 只实例的位置。
fn pose_via_lua(runtime: &mut RuntimeScene, index: usize) -> [f32; 3] {
    // 场景实体 → Lua 句柄：两边编码不同（宿主 `encode_handle` vs Lua `to_lua_i64`），
    // 所以走名字（`entity.find`）而不是自己算。
    let raw = runtime
        .run_lua_console(&format!(
            "local e = entity.find('Crowd')\n\
             if not e then return 'no-entity' end\n\
             local p = mesh.batch_pose(e, {index})\n\
             if not p then return 'no-pose' end\n\
             return string.format('%.6f,%.6f,%.6f', p.x, p.y, p.z)"
        ))
        .unwrap_or_else(|error| panic!("batch_pose({index}) 调用失败：{error}"));
    let parts: Vec<f32> = raw
        .trim()
        .split(',')
        .filter_map(|part| part.parse().ok())
        .collect();
    assert_eq!(
        parts.len(),
        3,
        "batch_pose({index}) 应返回三个坐标，实际 `{}`",
        raw.trim()
    );
    [parts[0], parts[1], parts[2]]
}

/// 引擎侧对同一时刻的求值（渲染提取走的就是这一条）。
///
/// 整段求值放在**一个** `with_script` 闭包内完成：`with_world` 的返回值是借用，
/// 拿不出闭包（生命周期绑在句柄上）。在闭包里算出 `[f32; 3]` 拷出来才是干净的。
fn pose_via_engine(runtime: &mut RuntimeScene, index: usize) -> [f32; 3] {
    runtime.with_script(|script| {
        // 与渲染提取同一处读数：`renderable_state().scene_time()`
        // （见 `frame_data.rs`，那里也是 `script.renderable_state().with(...)`）。
        let time = script.renderable_state().with(|state| state.scene_time());
        let handle = script
            .scene_entity_handle(SceneEntityId(1))
            .expect("scene entity handle");
        let mut out = [0.0_f32; 3];
        script.with_world(|world| {
            let Some(batch) = world
                .get::<std::sync::Arc<InstanceBatch3D>>(handle)
                .ok()
                .flatten()
            else {
                panic!("batch 应当还在世界里");
            };
            let matrix = batch
                .instance_matrix_at(index, time)
                .unwrap_or_else(|| panic!("实例 #{index} 应当能求出矩阵"));
            // 与脚本侧 `mesh.batch_pose` 同一个取法：把原点变换过去，得到平移分量。
            // 不直接读矩阵列——`Mat4` 是列主序，硬编码下标会把轴向读错。
            let origin = matrix
                .transform_point(Vec3::new(0.0, 0.0, 0.0))
                .unwrap_or(Vec3::new(0.0, 0.0, 0.0));
            out = [origin.x, origin.y, origin.z];
        });
        out
    })
}

/// 脚本读回的姿态与引擎求值的姿态，在**同一个时刻**必须逐位一致。
#[test]
fn lua_and_engine_agree_on_instance_pose_at_the_same_time() {
    let (mut runtime, _) = runtime_with_batch();

    // 推进半步，让时钟离开 0（0 秒处两边都取默认值，测不出错位）。
    runtime
        .run_script_steps(3, 1.0 / 60.0, &InputSnapshot::default())
        .expect("步进");

    let lua = pose_via_lua(&mut runtime, 0);
    let engine = pose_via_engine(&mut runtime, 0);

    for (axis, (a, b)) in lua.iter().zip(engine.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-4,
            "两只消费方对同一时刻的姿态不一致（轴 {axis}）：脚本读 {:?}，引擎算 {:?}。\
             场景时钟被两条路径读到了不同的值",
            lua,
            engine
        );
    }
}

/// 时钟必须**随模拟步推进**，且**0 步帧不推进**。
///
/// 后半句是真实的坑：固定步档下"帧号 ⇒ 时刻一一对应"依赖这条，
/// 如果 0 步帧也推进时钟，程序化运动在卡顿帧里会悄悄跑快。
#[test]
fn the_scene_clock_advances_by_simulated_time_only() {
    let (mut runtime, _) = runtime_with_batch();
    let read = |runtime: &mut RuntimeScene| {
        runtime.with_script(|script| script.renderable_state().with(|state| state.scene_time()))
    };

    let start = read(&mut runtime);
    assert_eq!(start, 0.0, "新建运行时场景时钟应当从 0 开始");

    runtime
        .run_script_steps(3, 1.0 / 60.0, &InputSnapshot::default())
        .expect("步进");
    let after = read(&mut runtime);
    assert!(
        after > start,
        "3 个 1/60 秒的步之后时钟应当推进，实际仍为 {after}"
    );

    // 0 步帧：时钟必须原地不动。
    runtime
        .run_script_steps(0, 1.0 / 60.0, &InputSnapshot::default())
        .expect("0 步帧");
    let after_idle = read(&mut runtime);
    assert_eq!(
        after_idle, after,
        "0 步帧不该推进时钟（卡顿帧里程序化运动否则会跑快）：{after} → {after_idle}"
    );
}

/// 时钟推进后，姿态**真的变了**——否则前两条可能只是"两边都返回同一个常量"。
///
/// 这条是前两条的守卫：如果 `instance_matrix_at` 忽略了 `time` 参数（比如被优化掉、
/// 或 motion 没生效），第一条仍然会绿（两边都读常量），但它会红。
#[test]
fn the_pose_actually_moves_when_the_clock_advances() {
    let (mut runtime, _) = runtime_with_batch();

    runtime
        .run_script_steps(2, 1.0 / 60.0, &InputSnapshot::default())
        .expect("步进");
    let early = pose_via_lua(&mut runtime, 0);

    runtime
        .run_script_steps(30, 1.0 / 60.0, &InputSnapshot::default())
        .expect("再步进");
    let late = pose_via_lua(&mut runtime, 0);

    let moved = early
        .iter()
        .zip(late.iter())
        .any(|(a, b)| (a - b).abs() > 1e-3);
    assert!(
        moved,
        "时钟推进了 0.5 秒，实例姿态却没变：{early:?} → {late:?}。\
         motion 没被求值，或 `instance_matrix_at` 忽略了 time 参数"
    );
}
