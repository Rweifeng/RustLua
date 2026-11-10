//! P3-R246：GPU 位姿批次档在**提取层**的口径。
//!
//! 渲染器侧的同档像素门在 `rustlua-gpu`（`batch_motion_frame`，含"宿主一个 CPU 矩阵都
//! 不给"的那一条）。这里钉的是宿主这一段：开档后 `scene_3d` **跳过** `matrices_at`
//! （drawable 的 `instances` 为空）并把批次登记进暂存表；关档保持旧路径（drawable 带
//! N 份 CPU 矩阵、暂存为空）。两者必须严格互补——开档却仍带矩阵 = 白付一份求值，
//! 关档却登记了批次 = 默认路径被偷偷改动。

use super::*;
use rustlua_render::instance_batch::{
    BatchMotion3D, Instance3D, InstanceBatch3D, InstanceMotion3D, MotionKind,
};
use rustlua_scene::scene::Scene;

/// 两只实例、一条沿 +X 的路径（`matrices_at` 的模板值不重要，只看"有没有被求值"）。
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

fn runtime_with_batch() -> RuntimeScene {
    let mut scene = Scene::new("gpu_motion");
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
            world.insert(handle, motion_batch()).expect("insert batch");
        });
    });
    runtime
}

/// 开档跳 CPU 求值并登记批次；关档保持旧路径。两层口径严格互补。
#[test]
fn gpu_motion_extraction_stages_batches_and_skips_cpu_matrices() {
    let runtime = runtime_with_batch();
    let viewport = Viewport::new(320.0, 180.0).expect("viewport");

    // 关档（默认）：drawable 带 2 份 CPU 矩阵、暂存为空。
    let off = runtime.scene_3d(viewport).expect("3d frame");
    assert_eq!(off.drawables.len(), 1, "批次实体必须提出一条 drawable");
    assert_eq!(
        off.drawables[0].instances.len(),
        2,
        "关档必须照旧求 CPU 矩阵（matrices_at）"
    );
    assert!(
        runtime.take_staged_motion_batches().is_empty(),
        "关档不许登记批次（默认路径一个字节都不碰）"
    );

    // 开档：drawable 一个矩阵都不带，批次进暂存表（实体 1、场景时刻）。
    let mut runtime = runtime;
    runtime.set_gpu_motion_batches(true);
    assert!(runtime.gpu_motion_batches(), "开档后开关要读得回来");
    let on = runtime.scene_3d(viewport).expect("3d frame");
    assert_eq!(on.drawables.len(), 1);
    assert!(
        on.drawables[0].instances.is_empty(),
        "开档必须跳过 matrices_at（drawable 的 instances 为空）"
    );
    let staged = runtime.take_staged_motion_batches();
    assert_eq!(staged.len(), 1, "每个带运动的批次登记一条");
    // 登记键必须是**渲染器认领用的那个键**（`FrameDraw` 来自 drawable 的实体 id），
    // 不是场景里的 `SceneEntityId` 序号——两者一旦分叉，GPU 批永远认领不上、静默回退 CPU。
    assert_eq!(
        staged[0].0, on.drawables[0].entity.0,
        "登记键必须与 drawable 的实体 id 一致"
    );
    assert_eq!(staged[0].1.instances.len(), 2, "登记的是整批（含运动参数）");
    assert!(
        runtime.take_staged_motion_batches().is_empty(),
        "取走即清空：同一个批次不许跨帧重复推给渲染器"
    );
}
