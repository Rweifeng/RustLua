//! P3-R244：GPU 路径位姿求值（`batch_motion.wgsl`）与 CPU `matrices_at` 的 1e-4 对账门。
//!
//! 钉三件事：
//!
//! * **同式**：同一条路径、同一批实例、同一时刻，GPU 写出的 N 份世界矩阵与 CPU
//!   `InstanceBatch3D::matrices_at` 逐元素相差 ≤ 1e-4（相对 1.0 的最坏绝对值）——允许
//!   `atan2/sin/cos` 的末位差，不允许"位姿规则不同"。
//! * **开口/闭合两种弧长口径都覆盖**：闭合路径的取模与末→首段最容易和 CPU 分叉。
//! * **落位**：给一个带前导空格的缓冲，GPU 只写 `[offset, offset + N×64)`，其它字节不动。

use super::*;
use crate::renderer::batch_motion_gpu::BATCH_MOTION_MATRIX_BYTES;
use rustlua_render::instance_batch::{
    BatchMotion3D, ForwardAxis, Instance3D, InstanceBatch3D, InstanceMotion3D, MotionKind,
};

fn headless_renderer() -> Option<Renderer> {
    let (device, queue) = super::compute::headless()?;
    let renderer =
        Renderer::new_offscreen(device, queue, None, false, false).expect("offscreen renderer");
    renderer.compute_enabled().then_some(renderer)
}

/// 64 只实例：align / 前向轴 / 速度倍率 / 起始偏移 / 缩放都各不相同，覆盖两个前向轴分支。
fn motion_batch(closed: bool) -> InstanceBatch3D {
    // 第四个点与前一个重合（退化点）：CPU 建弧长表时会剔除，GPU 拿到的节点表必须一致。
    let path = vec![
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 0.0, 10.0],
        [10.0, 0.0, 10.0],
        [-5.0, 2.0, 4.0],
    ];
    let mut instances = Vec::with_capacity(64);
    for index in 0..64usize {
        let angle = index as f32 * 0.1;
        instances.push(Instance3D {
            position: [index as f32, 0.0, 0.0],
            // 静态姿态：align=false 的实例用的是这一份四元数（含非单位长度，走归一化分支）。
            rotation: [0.0, angle.sin() * 2.0, 0.0, angle.cos() * 2.0],
            scale: [0.1 + index as f32 * 0.01, 0.2, 0.05],
            motion: Some(InstanceMotion3D {
                offset: index as f32 * 0.7,
                speed: 0.5 + (index % 5) as f32 * 0.25,
                align: index % 3 != 0,
                forward: if index % 2 == 0 {
                    ForwardAxis::X
                } else {
                    ForwardAxis::Z
                },
            }),
        });
    }
    InstanceBatch3D {
        mesh: "__ground_plane__".to_owned(),
        material: None,
        instances,
        motion: Some(BatchMotion3D {
            kind: MotionKind::Path,
            path,
            closed,
            speed: 1.7,
        }),
    }
}

fn assert_matrices_close(cpu: &[Mat4], gpu: &[Mat4], label: &str) {
    assert_eq!(cpu.len(), gpu.len(), "{label}: 实例数不一致");
    for (index, (cpu, gpu)) in cpu.iter().zip(gpu).enumerate() {
        for column in 0..4 {
            for row in 0..4 {
                let expected = cpu.columns[column][row];
                let actual = gpu.columns[column][row];
                let tolerance = 1e-4 * expected.abs().max(1.0);
                assert!(
                    (expected - actual).abs() <= tolerance,
                    "{label}: 实例 {index} 列 {column} 行 {row} CPU={expected} GPU={actual} \
                     （容差 {tolerance}）"
                );
            }
        }
    }
}

/// 开口与闭合两条路径、三个时刻：GPU 求值与 CPU `matrices_at` 同式。
#[test]
fn gpu_bake_matches_cpu_matrices_at() {
    let Some(mut renderer) = headless_renderer() else {
        return gpu_skipped("device has no compute");
    };
    for (label, batch) in [("开口", motion_batch(false)), ("闭合", motion_batch(true))] {
        for time in [0.0f32, 1.5, 17.25] {
            let cpu = batch.matrices_at(time);
            assert_eq!(cpu.len(), 64, "{label} t={time}: CPU 侧 64 只都该合法");
            let gpu = renderer
                .bake_batch_motion_readback(&batch, time)
                .unwrap_or_else(|error| panic!("{label} t={time}: {error}"));
            assert_matrices_close(&cpu, &gpu, &format!("{label} t={time}"));
        }
    }
}

/// 落位：目标缓冲前面留一格，GPU 只写 `[64, 64 + N×64)`，第 0 格保持原值。
#[test]
fn gpu_bake_writes_only_the_reserved_slots() {
    let Some(mut renderer) = headless_renderer() else {
        return gpu_skipped("device has no compute");
    };
    let batch = motion_batch(false);
    let count = batch.instances.len() as u64;
    let size = (count + 1) * 64;
    let destination = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-batch-motion-offset-target"),
        size,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // 第 0 格显式写零：`mapped_at_creation` 的初值未定义，不能拿它当"没被写过"的证据。
    renderer.queue.write_buffer(&destination, 0, &[0u8; 64]);
    let time = 2.25f32;
    let written = renderer
        .bake_batch_motion(&batch, time, &destination, 64)
        .expect("bake into offset");
    assert_eq!(written, count as u32);
    let bytes = renderer
        .read_storage_buffer(&destination, size)
        .expect("readback");
    assert!(
        bytes[..64].iter().all(|byte| *byte == 0),
        "第 0 格是调用方预留区，GPU 不许写它"
    );
    let mut columns = Vec::with_capacity(count as usize);
    for chunk in bytes[64..].chunks_exact(64) {
        let mut values = [0.0f32; 16];
        for (slot, raw) in values.iter_mut().zip(chunk.chunks_exact(4)) {
            *slot = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        }
        let mut matrix = [[0.0f32; 4]; 4];
        for (column, slot) in matrix.iter_mut().enumerate() {
            slot.copy_from_slice(&values[column * 4..column * 4 + 4]);
        }
        columns.push(Mat4 { columns: matrix });
    }
    assert_matrices_close(&batch.matrices_at(time), &columns, "offset=64");
}

/// 没有运动 / 路径不合法 ⇒ `Ok(0)`，调用方回退 CPU（不是错误，也不是静默半批）。
#[test]
fn bake_without_motion_or_with_bad_path_is_a_no_op() {
    let Some(mut renderer) = headless_renderer() else {
        return gpu_skipped("device has no compute");
    };
    let destination = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-batch-motion-noop-target"),
        size: 64 * 8,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let mut static_batch = motion_batch(false);
    static_batch.motion = None;
    assert_eq!(
        renderer
            .bake_batch_motion(&static_batch, 0.0, &destination, 0)
            .expect("no motion"),
        0
    );
    let mut broken_batch = motion_batch(false);
    broken_batch.motion = Some(BatchMotion3D {
        kind: MotionKind::Path,
        path: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
        closed: false,
        speed: 1.0,
    });
    assert_eq!(
        renderer
            .bake_batch_motion(&broken_batch, 0.0, &destination, 0)
            .expect("degenerate path"),
        0
    );
}

/// 目标缓冲太小 / 偏移不对齐 / 时钟非有限：三条都显式报错，不静默写坏半批。
#[test]
fn bake_rejects_bad_targets_and_non_finite_time() {
    let Some(mut renderer) = headless_renderer() else {
        return gpu_skipped("device has no compute");
    };
    let batch = motion_batch(false);
    let destination = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-batch-motion-small-target"),
        size: 64 * 8,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let too_small = renderer
        .bake_batch_motion(&batch, 0.0, &destination, 0)
        .expect_err("8 格放不下 64 份矩阵");
    assert!(
        matches!(too_small, BatchMotionGpuError::DestinationTooSmall { .. }),
        "{too_small}"
    );
    let misaligned = renderer
        .bake_batch_motion(&batch, 0.0, &destination, 32)
        .expect_err("32 不是 64 的倍数");
    assert!(
        matches!(
            misaligned,
            BatchMotionGpuError::DestinationMisaligned { .. }
        ),
        "{misaligned}"
    );
    let destination = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-batch-motion-time-target"),
        size: 64 * 64,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let non_finite = renderer
        .bake_batch_motion(&batch, f32::NAN, &destination, 0)
        .expect_err("NaN 时钟");
    assert!(
        matches!(non_finite, BatchMotionGpuError::InvalidInput(_)),
        "{non_finite}"
    );
}

/// P3-R250：逐实例距离剔除——超出半径的实例写**零矩阵**（不压缩，实例数仍为 N），
/// 半径内的实例与 CPU `matrices_at` 同式；`None`/半径 0 档逐矩阵不变；法线入口同一口径。
///
/// 期望集合不是手写的：从 CPU `matrices_at` 的平移列算距离，GPU 的零/非零必须与之逐实例
/// 一致 —— 这样"夹具刚好全在同侧"或"两侧都绿"的假绿都过不了。
#[test]
fn gpu_bake_cull_distance_zeroes_far_instances() {
    let Some(mut renderer) = headless_renderer() else {
        return gpu_skipped("device has no compute");
    };
    let batch = motion_batch(false);
    let time = 0.0f32;
    let cpu = batch.matrices_at(time);
    assert_eq!(cpu.len(), 64, "夹具里 64 只都该合法");
    let origin = rustlua_math::math3d::Vec3::new(0.0, 0.0, 0.0);
    let radius = 12.0f32;
    let expected_culled: Vec<bool> = cpu
        .iter()
        .map(|model| {
            let [x, y, z] = [
                model.columns[3][0],
                model.columns[3][1],
                model.columns[3][2],
            ];
            x * x + y * y + z * z > radius * radius
        })
        .collect();
    let culled_count = expected_culled.iter().filter(|culled| **culled).count();
    assert!(
        culled_count > 0 && culled_count < 64,
        "夹具必须同时覆盖剔除/保留两侧，实际剔除 {culled_count}/64"
    );

    let gpu = renderer
        .bake_batch_motion_readback_with_cull(&batch, time, Some((origin, radius)))
        .unwrap_or_else(|error| panic!("距离剔除烘焙失败：{error}"));
    assert_eq!(gpu.len(), 64, "剔除只写零矩阵，不许压缩实例数");
    for (index, (&culled, matrix)) in expected_culled.iter().zip(&gpu).enumerate() {
        let zero = matrix
            .columns
            .iter()
            .all(|column| column.iter().all(|value| *value == 0.0));
        assert_eq!(
            zero, culled,
            "实例 {index}: CPU 距离判定剔除={culled}，GPU 零矩阵={zero}（平移列 {:?}）",
            matrix.columns[3]
        );
        if !culled {
            for column in 0..4 {
                for row in 0..4 {
                    let expected = cpu[index].columns[column][row];
                    let actual = matrix.columns[column][row];
                    let tolerance = 1e-4 * expected.abs().max(1.0);
                    assert!(
                        (expected - actual).abs() <= tolerance,
                        "保留实例 {index} 列 {column} 行 {row} CPU={expected} GPU={actual}"
                    );
                }
            }
        }
    }

    // 关闭档（None）：一只都不许剔，且与 CPU 同式。
    let off = renderer
        .bake_batch_motion_readback(&batch, time)
        .unwrap_or_else(|error| panic!("关闭档烘焙失败：{error}"));
    assert!(
        off.iter().all(|matrix| matrix
            .columns
            .iter()
            .any(|column| column.iter().any(|value| *value != 0.0))),
        "关闭档出现零矩阵：默认路径被剔除分支污染了"
    );
    assert_matrices_close(&cpu, &off, "关闭档");

    // 法线入口（`main_normals`）必须同一口径：远实例的法线矩阵也全零。
    let size = 64 * 64;
    let destination = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-batch-motion-cull-normals-target"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let normals = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-batch-motion-cull-normals"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    renderer
        .bake_batch_motion_with_normals_and_cull(
            &batch,
            time,
            &destination,
            0,
            Some((&normals, 0)),
            1.0,
            Some((origin, radius)),
        )
        .unwrap_or_else(|error| panic!("带法线的距离剔除烘焙失败：{error}"));
    let normal_bytes = renderer
        .read_storage_buffer(&normals, size)
        .unwrap_or_else(|error| panic!("法线回读失败：{error}"));
    for (index, &culled) in expected_culled.iter().enumerate() {
        let chunk = &normal_bytes[index * 64..(index + 1) * 64];
        let zero = chunk.iter().all(|byte| *byte == 0);
        assert_eq!(
            zero, culled,
            "实例 {index}: 法线矩阵剔除口径与模型矩阵不一致"
        );
    }

    // 参数护栏：非有限半径必须报错，不许静默关档。
    let non_finite = renderer
        .bake_batch_motion_readback_with_cull(&batch, time, Some((origin, f32::NAN)))
        .expect_err("NaN 半径必须报错");
    assert!(
        matches!(non_finite, BatchMotionGpuError::InvalidInput(_)),
        "{non_finite}"
    );
}

#[test]
fn gpu_compact_bake_packs_survivors_and_reports_indirect_count() {
    let Some(mut renderer) = headless_renderer() else {
        return gpu_skipped("device has no compute");
    };
    let batch = motion_batch(false);
    let time = 0.0f32;
    let cpu = batch.matrices_at(time);
    let origin = rustlua_math::math3d::Vec3::new(0.0, 0.0, 0.0);
    let radius = 12.0f32;
    let translation = |model: &Mat4| {
        [
            model.columns[3][0],
            model.columns[3][1],
            model.columns[3][2],
        ]
    };
    let expected: Vec<Mat4> = cpu
        .iter()
        .filter(|model| {
            let [x, y, z] = translation(model);
            x * x + y * y + z * z <= radius * radius
        })
        .cloned()
        .collect();
    assert!(
        !expected.is_empty() && expected.len() < cpu.len(),
        "夹具必须同时覆盖保留/剔除两侧，实际保留 {}/{}",
        expected.len(),
        cpu.len()
    );

    let size = cpu.len() as u64 * BATCH_MOTION_MATRIX_BYTES;
    let destination = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-batch-motion-compact-target"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let index_count = 321u32;
    let args = renderer
        .bake_batch_motion_compact(
            &batch,
            time,
            &destination,
            0,
            None,
            1.0,
            Some((origin, radius)),
            index_count,
            None,
            0.0,
            None,
            None,
        )
        .unwrap_or_else(|error| panic!("压缩烘焙失败：{error}"))
        .expect("剔除档必须给出 indirect 参数缓冲");
    assert!(
        args.usage().contains(wgpu::BufferUsages::INDIRECT),
        "参数缓冲必须带 INDIRECT 用法，否则 draw_indexed_indirect 校验会拒"
    );
    let args_bytes = renderer
        .read_storage_buffer(&args, 20)
        .unwrap_or_else(|error| panic!("参数回读失败：{error}"));
    let word = |index: usize| {
        u32::from_le_bytes([
            args_bytes[index * 4],
            args_bytes[index * 4 + 1],
            args_bytes[index * 4 + 2],
            args_bytes[index * 4 + 3],
        ])
    };
    assert_eq!(
        word(0),
        index_count,
        "indirect 参数第 1 项必须是 index_count"
    );
    assert_eq!(
        word(1) as usize,
        expected.len(),
        "存活数必须与 CPU 距离判定一致（压缩后实例数）"
    );
    assert_eq!(
        [word(2), word(3), word(4)],
        [0, 0, 0],
        "first_index/base_vertex/first_instance 必须为 0"
    );

    let bytes = renderer
        .read_storage_buffer(&destination, size)
        .unwrap_or_else(|error| panic!("压缩结果回读失败：{error}"));
    let packed: Vec<Mat4> = bytes
        .chunks_exact(BATCH_MOTION_MATRIX_BYTES as usize)
        .take(expected.len())
        .map(|chunk| {
            let mut columns = [[0.0f32; 4]; 4];
            for (column, slot) in columns.iter_mut().enumerate() {
                for (row, value) in slot.iter_mut().enumerate() {
                    let start = (column * 4 + row) * 4;
                    *value = f32::from_le_bytes([
                        chunk[start],
                        chunk[start + 1],
                        chunk[start + 2],
                        chunk[start + 3],
                    ]);
                }
            }
            Mat4 { columns }
        })
        .collect();
    assert_eq!(packed.len(), expected.len());
    // 原子加的顺序不稳定 ⇒ 按平移列排序后逐个对账（画面与顺序无关）。
    let mut packed_sorted = packed.clone();
    packed_sorted.sort_by(|a, b| {
        let (a, b) = (translation(a), translation(b));
        a[0].total_cmp(&b[0])
            .then(a[1].total_cmp(&b[1]))
            .then(a[2].total_cmp(&b[2]))
    });
    let mut expected_sorted = expected.clone();
    expected_sorted.sort_by(|a, b| {
        let (a, b) = (translation(a), translation(b));
        a[0].total_cmp(&b[0])
            .then(a[1].total_cmp(&b[1]))
            .then(a[2].total_cmp(&b[2]))
    });
    assert_matrices_close(&expected_sorted, &packed_sorted, "压缩前缀");

    // 压缩前缀之后不许再写：尾部保持缓冲初值（全 0）。
    for slot in expected.len()..cpu.len() {
        let start = slot * BATCH_MOTION_MATRIX_BYTES as usize;
        let chunk = &bytes[start..start + BATCH_MOTION_MATRIX_BYTES as usize];
        assert!(
            chunk.iter().all(|byte| *byte == 0),
            "第 {slot} 槽位超出存活数却被写入，压缩没有真正缩短 draw 范围"
        );
    }

    // 关剔除档 + 压缩：全部实例都可见 ⇒ 参数里的实例数等于批次大小。
    let off = renderer
        .bake_batch_motion_compact(
            &batch,
            time,
            &destination,
            0,
            None,
            1.0,
            None,
            index_count,
            None,
            0.0,
            None,
            None,
        )
        .unwrap_or_else(|error| panic!("关闭剔除的压缩烘焙失败：{error}"))
        .expect("压缩档必须给出参数缓冲");
    let off_bytes = renderer
        .read_storage_buffer(&off, 20)
        .unwrap_or_else(|error| panic!("关闭档参数回读失败：{error}"));
    let off_count = u32::from_le_bytes([off_bytes[4], off_bytes[5], off_bytes[6], off_bytes[7]]);
    assert_eq!(
        off_count as usize,
        cpu.len(),
        "关闭剔除时不许压缩掉任何实例"
    );
}

/// P3-R255 夹具：x∈[-8, 8] 的直路，四只实例在冻结时刻正好落在 `SLOT_SLOTS` 上。
const SLOT_FROZEN: f32 = 4.2;
const SLOT_SLOTS: [f32; 4] = [-2.7, -0.9, 0.9, 2.7];
const SLOT_SCALE: f32 = 0.15;

fn slot_batch() -> InstanceBatch3D {
    let path = vec![[-8.0, 0.0, 0.0], [8.0, 0.0, 0.0]];
    let instances = SLOT_SLOTS
        .iter()
        .map(|slot| Instance3D {
            position: [*slot, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [SLOT_SCALE; 3],
            motion: Some(InstanceMotion3D {
                offset: *slot + 8.0 - SLOT_FROZEN,
                speed: 1.0,
                align: false,
                ..InstanceMotion3D::default()
            }),
        })
        .collect();
    InstanceBatch3D {
        mesh: "__ground_plane__".to_owned(),
        material: None,
        instances,
        motion: Some(BatchMotion3D {
            kind: MotionKind::Path,
            path,
            closed: false,
            speed: 1.0,
        }),
    }
}

fn translation_of(model: &Mat4) -> [f32; 3] {
    [
        model.columns[3][0],
        model.columns[3][1],
        model.columns[3][2],
    ]
}

/// 原子加决定压缩顺序 ⇒ 比对前按平移列排序（与 `gpu_compact_bake_packs_survivors_*` 同口径）。
fn sort_by_translation(mut models: Vec<Mat4>) -> Vec<Mat4> {
    models.sort_by(|a, b| {
        let (a, b) = (translation_of(a), translation_of(b));
        a[0].total_cmp(&b[0])
            .then(a[1].total_cmp(&b[1]))
            .then(a[2].total_cmp(&b[2]))
    });
    models
}

fn matrix_from_chunk(chunk: &[u8]) -> Mat4 {
    let mut columns = [[0.0f32; 4]; 4];
    for (column, slot) in columns.iter_mut().enumerate() {
        for (row, value) in slot.iter_mut().enumerate() {
            let start = (column * 4 + row) * 4;
            *value = f32::from_le_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }
    }
    Mat4 { columns }
}

/// P3-R255：视锥六面 + 保守局部半径的逐实例剔除。
///
/// 期望存活集合由 CPU `matrices_at` 的平移列 + **同一份平面语义**（正 = 内侧；球整颗落在
/// 某面外侧一个半径之外 ⇒ 剔）独立算出，不抄 shader 的判定式。四档互为对照：
///
/// * 收窄视锥（半宽 1.8）⇒ 两端的实例被剔，只剩 [1, 2]；
/// * 平面放宽到 1e2 ⇒ 四只全留（抓"永远剔"的假实现）；
/// * 局部半径 1 → 8 ⇒ 两端回到存活集（抓"忽略半径、只看球心"）；
/// * 半径 8 + `model_scale 0.1` ⇒ 半径被缩回去，两端重新被剔（抓"漏乘 model_scale"）；
/// * 平面整体推到球心外侧 ⇒ 一只不剩，且目标缓冲一个字节都没被写。
#[test]
fn gpu_bake_frustum_cull_keeps_only_instances_whose_sphere_reaches_the_view() {
    let Some(mut renderer) = headless_renderer() else {
        return gpu_skipped("device has no compute");
    };
    let batch = slot_batch();
    let cpu = batch.matrices_at(SLOT_FROZEN);
    assert_eq!(cpu.len(), SLOT_SLOTS.len(), "夹具四只都该合法");
    // 六面里只有左右两面可能剔人；上下前后放到 100 个单位外。
    let planes = |half_width: f32| {
        [
            [1.0, 0.0, 0.0, half_width],
            [-1.0, 0.0, 0.0, half_width],
            [0.0, 1.0, 0.0, 100.0],
            [0.0, -1.0, 0.0, 100.0],
            [0.0, 0.0, 1.0, 100.0],
            [0.0, 0.0, -1.0, 100.0],
        ]
    };
    let expected = |half_width: f32, local_radius: f32, model_scale: f32| -> Vec<usize> {
        let radius = local_radius * SLOT_SCALE * model_scale;
        cpu.iter()
            .enumerate()
            .filter(|(_, model)| {
                let x = translation_of(model)[0];
                x + half_width >= -radius && -x + half_width >= -radius
            })
            .map(|(index, _)| index)
            .collect()
    };
    // 先自证夹具两侧都覆盖得到（否则"全留"和"全剔"两档没有判别力）。
    assert_eq!(expected(1.8, 1.0, 1.0), vec![1, 2], "收窄视锥应剔掉两端");
    assert_eq!(
        expected(1.8, 8.0, 1.0),
        vec![0, 1, 2, 3],
        "大半径应让两端回到画面"
    );
    assert_eq!(
        expected(1.8, 8.0, 0.1),
        vec![1, 2],
        "model_scale 应缩回半径"
    );

    let size = cpu.len() as u64 * BATCH_MOTION_MATRIX_BYTES;
    let destination = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-batch-motion-frustum-target"),
        size,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let zeros = vec![0u8; size as usize];
    let index_count = 96u32;
    let run = |renderer: &mut Renderer,
               half_width: f32,
               local_radius: f32,
               model_scale: f32|
     -> (usize, Vec<Mat4>, Vec<u8>) {
        // 每档先清零：全剔档要能看出"一个槽位都没领"。
        renderer.queue.write_buffer(&destination, 0, &zeros);
        let args = renderer
            .bake_batch_motion_compact(
                &batch,
                SLOT_FROZEN,
                &destination,
                0,
                None,
                model_scale,
                None,
                index_count,
                Some(planes(half_width)),
                local_radius,
                None,
                None,
            )
            .unwrap_or_else(|error| panic!("视锥剔除烘焙失败：{error}"))
            .expect("视锥档必须给出 indirect 参数缓冲");
        let args_bytes = renderer
            .read_storage_buffer(&args, 20)
            .unwrap_or_else(|error| panic!("参数回读失败：{error}"));
        let count = u32::from_le_bytes([args_bytes[4], args_bytes[5], args_bytes[6], args_bytes[7]])
            as usize;
        let bytes = renderer
            .read_storage_buffer(&destination, size)
            .unwrap_or_else(|error| panic!("压缩结果回读失败：{error}"));
        let packed = bytes[..count * BATCH_MOTION_MATRIX_BYTES as usize]
            .chunks_exact(BATCH_MOTION_MATRIX_BYTES as usize)
            .map(matrix_from_chunk)
            .collect();
        (count, packed, bytes)
    };

    for (label, half_width, local_radius, model_scale) in [
        ("收窄视锥", 1.8, 1.0, 1.0),
        ("放宽平面", 1.0e2, 1.0, 1.0),
        ("半径 8", 1.8, 8.0, 1.0),
        ("半径 8 + model_scale 0.1", 1.8, 8.0, 0.1),
    ] {
        let want = expected(half_width, local_radius, model_scale);
        let (count, packed, _) = run(&mut renderer, half_width, local_radius, model_scale);
        assert_eq!(
            count,
            want.len(),
            "{label}: 存活数必须与 CPU 期望一致（实测 {count}，期望 {}）",
            want.len()
        );
        // GPU 烘焙把 `model_scale` 乘进 3x3（与 `scaled_model` 同式），CPU `matrices_at` 不乘。
        let want_models: Vec<Mat4> = want
            .iter()
            .map(|index| {
                let mut model = cpu[*index];
                for column in &mut model.columns[..3] {
                    for value in &mut column[..3] {
                        *value *= model_scale;
                    }
                }
                model
            })
            .collect();
        assert_matrices_close(
            &sort_by_translation(want_models),
            &sort_by_translation(packed),
            label,
        );
    }

    // 全剔档：平面推到球心外侧（半径 0.15 不够救），一只都不许领槽位。
    let (count, _, bytes) = run(&mut renderer, -20.0, 1.0, 1.0);
    assert_eq!(count, 0, "全在视锥外时实例数必须是 0（实测 {count}）");
    assert!(
        bytes.iter().all(|byte| *byte == 0),
        "全剔档不许写目标缓冲（一字节非零 = 有实例偷偷领了槽位）"
    );

    // 参数护栏：视锥/半径非有限、半径为负都必须报错，不许静默关档。
    let mut nan_planes = planes(1.8);
    nan_planes[2][0] = f32::NAN;
    let nan_plane = renderer
        .bake_batch_motion_compact(
            &batch,
            SLOT_FROZEN,
            &destination,
            0,
            None,
            1.0,
            None,
            index_count,
            Some(nan_planes),
            1.0,
            None,
            None,
        )
        .expect_err("NaN 平面必须报错");
    assert!(
        matches!(nan_plane, BatchMotionGpuError::InvalidInput(_)),
        "{nan_plane}"
    );
    for (label, radius) in [("NaN", f32::NAN), ("负", -1.0)] {
        let error = renderer
            .bake_batch_motion_compact(
                &batch,
                SLOT_FROZEN,
                &destination,
                0,
                None,
                1.0,
                None,
                index_count,
                Some(planes(1.8)),
                radius,
                None,
                None,
            )
            .expect_err("非法局部半径必须报错");
        assert!(
            matches!(error, BatchMotionGpuError::InvalidInput(_)),
            "{label} 半径：{error}"
        );
    }

    // 关档时未使用的参数不参与校验：`frustum = None` + NaN 半径不报错，且一只都不剔。
    let off_args = renderer
        .bake_batch_motion_compact(
            &batch,
            SLOT_FROZEN,
            &destination,
            0,
            None,
            1.0,
            None,
            index_count,
            None,
            f32::NAN,
            None,
            None,
        )
        .expect("关档不该校验 local_radius")
        .expect("压缩档必须给出参数缓冲");
    let off_bytes = renderer
        .read_storage_buffer(&off_args, 20)
        .unwrap_or_else(|error| panic!("关档参数回读失败：{error}"));
    let off_count = u32::from_le_bytes([off_bytes[4], off_bytes[5], off_bytes[6], off_bytes[7]]);
    assert_eq!(
        off_count as usize,
        cpu.len(),
        "关档一只都不许剔（实测 {off_count}）"
    );
}
