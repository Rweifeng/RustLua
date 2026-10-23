//! P3-R06 验收门（`wp0-compute-readback-v1`）：buffer → dispatch → 读回的逐值/容差对拍。
//!
//! 这条样例只回答一个问题：**compute 接缝是不是真的把数据送进去、又真的把数据拿回来**。
//! 它不像 M74-03 那样测消费者（GPU 粒子有自己的对拍门），而是测接缝本身，四步各钉一条硬要求：
//!
//! 1. 整数核：256 个槽位写成 `i*3+7` → 读回**逐项相等**（`mismatches=0`）；
//! 2. 浮点核：同一批槽位写成 `x*1.5+0.25` → 读回最大绝对误差 < 1e-6（证明不是"看着像 0"）；
//! 3. 同 key 第二次 dispatch 命中管线与绑定组缓存（`pipeline_compiled=false`）；
//! 4. 关掉 compute 后所有入口返回可读的 `Unsupported`，**不 panic、不静默空转**。
//!
//! 能力读数（`backend`/`compute`/workgroup 限制/存储上限）一并进报告，无 compute 的机器上
//! 本样例以 BLOCKED 结束而不是"全 0 通过"。
//!
//! 运行：`RUSTLUA_REQUIRE_GPU=1 cargo run -p engine --example wp0_compute_readback -- --output artifacts/p3/compute-readback`
//! 没跑就是没跑：这台机器没有可用适配器时以 **exit 3 / BLOCKED** 结束，绝不报成功。

use std::{env, fs, path::PathBuf, process::ExitCode};

use engine::host::{ComputeBinding, ComputeError, ComputeRequest, Renderer};
use serde_json::json;
use wgpu::util::DeviceExt;

const SLOTS: u32 = 256;
const WORKGROUP: u32 = 64;

/// 整数核：`data[i] = i*3 + 7`。逐值相等，没有任何容差可言。
const INT_FILL_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read_write> data: array<u32>;

@compute @workgroup_size(64)
fn fill(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < arrayLength(&data)) {
        data[id.x] = id.x * 3u + 7u;
    }
}
"#;

/// 浮点核：`data[i] = src[i] * 1.5 + 0.25`，两个 storage 绑定（读 + 写）。
const FLOAT_SCALE_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(64)
fn scale(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < arrayLength(&dst)) {
        dst[id.x] = src[id.x] * 1.5 + 0.25;
    }
}
"#;

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
        eprintln!("BLOCKED: no GPU adapter available; compute readback cannot run (没跑 ≠ 通过)");
        return ExitCode::from(3);
    };
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-compute-readback"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request golden device");
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, None, false, false).expect("offscreen");
    let capabilities = renderer.capabilities();
    if !capabilities.compute {
        eprintln!(
            "BLOCKED: adapter {} {:?} reports compute=false; the readback gate cannot run (没跑 ≠ 通过)",
            info.name, info.backend
        );
        return ExitCode::from(3);
    }

    let storage_bytes = SLOTS * 4;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wp0-compute-readback-int"),
        size: u64::from(storage_bytes),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let int_request = ComputeRequest {
        label: "wp0-compute-readback-int",
        shader: INT_FILL_SHADER,
        entry_point: "fill",
        bindings: &[ComputeBinding::Storage {
            binding: 0,
            buffer: &buffer,
            read_only: false,
        }],
        workgroups: [SLOTS / WORKGROUP, 1, 1],
        bind_group_key: Some(0x1_E7),
    };
    let first = match renderer.dispatch_compute(&int_request) {
        Ok(dispatch) => dispatch,
        Err(error) => return blocked_dispatch("整数核首次 dispatch", &error),
    };
    if !first.pipeline_compiled {
        eprintln!("FAIL: 首次 dispatch 没有编译管线（缓存命中了不存在的东西？）");
        return ExitCode::FAILURE;
    }
    let bytes = match renderer.read_storage_buffer(&buffer, u64::from(storage_bytes)) {
        Ok(bytes) => bytes,
        Err(error) => return blocked_dispatch("整数核读回", &error),
    };
    let values: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    let mismatches: Vec<usize> = (0..SLOTS as usize)
        .filter(|index| values.get(*index).copied().unwrap_or(u32::MAX) != (*index as u32) * 3 + 7)
        .collect();
    if !mismatches.is_empty() {
        eprintln!(
            "FAIL: 整数核逐值对拍失败 {} 项（首三处 {:?}），实际值 {:?}",
            mismatches.len(),
            &mismatches[..mismatches.len().min(3)],
            mismatches
                .iter()
                .take(3)
                .map(|index| (*index, values.get(*index).copied().unwrap_or(u32::MAX)))
                .collect::<Vec<_>>()
        );
        return ExitCode::FAILURE;
    }

    // ③ 同 key 第二次必须命中两级缓存。
    let second = match renderer.dispatch_compute(&int_request) {
        Ok(dispatch) => dispatch,
        Err(error) => return blocked_dispatch("整数核第二次 dispatch", &error),
    };
    let stats = renderer.compute_stats();
    if second.pipeline_compiled || second.bind_group_created {
        eprintln!(
            "FAIL: 同 key 第二次 dispatch 没有命中缓存（pipeline_compiled={} bind_group_created={}）",
            second.pipeline_compiled, second.bind_group_created
        );
        return ExitCode::FAILURE;
    }

    // ② 浮点核：两个绑定、一次写回，容差 1e-6。
    let source = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("wp0-compute-readback-float-src"),
        contents: bytemuck::cast_slice(
            &(0..SLOTS)
                .map(|index| index as f32 / 8.0 - 4.0)
                .collect::<Vec<f32>>(),
        ),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let sink = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wp0-compute-readback-float-dst"),
        size: u64::from(storage_bytes),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let float_request = ComputeRequest {
        label: "wp0-compute-readback-float",
        shader: FLOAT_SCALE_SHADER,
        entry_point: "scale",
        bindings: &[
            ComputeBinding::Storage {
                binding: 0,
                buffer: &source,
                read_only: true,
            },
            ComputeBinding::Storage {
                binding: 1,
                buffer: &sink,
                read_only: false,
            },
        ],
        workgroups: [SLOTS / WORKGROUP, 1, 1],
        bind_group_key: Some(0xF10A7),
    };
    if let Err(error) = renderer.dispatch_compute(&float_request) {
        return blocked_dispatch("浮点核 dispatch", &error);
    }
    let bytes = match renderer.read_storage_buffer(&sink, u64::from(storage_bytes)) {
        Ok(bytes) => bytes,
        Err(error) => return blocked_dispatch("浮点核读回", &error),
    };
    let floats: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    let mut max_abs_error = 0.0_f32;
    for index in 0..SLOTS as usize {
        let expected = (index as f32 / 8.0 - 4.0) * 1.5 + 0.25;
        let actual = floats.get(index).copied().unwrap_or(f32::NAN);
        if !actual.is_finite() {
            eprintln!("FAIL: 浮点核读回槽位 {index} 非有限（{actual}）");
            return ExitCode::FAILURE;
        }
        max_abs_error = max_abs_error.max((actual - expected).abs());
    }
    if max_abs_error >= 1e-6 {
        eprintln!("FAIL: 浮点核最大绝对误差 {max_abs_error} >= 1e-6");
        return ExitCode::FAILURE;
    }

    // ④ 关掉 compute：两个入口都必须返回可读的 `Unsupported`，且不能 panic。
    renderer.set_compute_enabled(false);
    let disabled = renderer.dispatch_compute(&int_request);
    match disabled {
        Err(ComputeError::Unsupported { reason }) => {
            eprintln!("note: compute 关闭后 dispatch 被拒（{reason}）");
        }
        Err(other) => {
            eprintln!("FAIL: 关闭 compute 后 dispatch 返回的不是 Unsupported：{other}");
            return ExitCode::FAILURE;
        }
        Ok(_) => {
            eprintln!("FAIL: 关闭 compute 后 dispatch 竟然成功（回滚档没生效）");
            return ExitCode::FAILURE;
        }
    }
    if !matches!(
        renderer.read_storage_buffer(&buffer, u64::from(storage_bytes)),
        Err(ComputeError::Unsupported { .. })
    ) {
        eprintln!("FAIL: 关闭 compute 后读回没有返回 Unsupported");
        return ExitCode::FAILURE;
    }

    let report = json!({
        "report_version": "wp0-compute-readback-v1",
        "adapter": { "backend": format!("{:?}", info.backend), "name": info.name },
        "capabilities": {
            "compute": capabilities.compute,
            "storage_buffers": capabilities.storage_buffers,
            "max_compute_workgroup_size": capabilities.max_compute_workgroup_size,
            "max_compute_invocations_per_workgroup": capabilities.max_compute_invocations_per_workgroup,
            "max_compute_workgroups_per_dimension": capabilities.max_compute_workgroups_per_dimension,
            "max_storage_buffer_binding_size": capabilities.max_storage_buffer_binding_size,
        },
        "gates": {
            "slots": SLOTS,
            "workgroup_size": WORKGROUP,
            "int_exact_mismatches": 0,
            "float_max_abs_error": max_abs_error,
            "float_tolerance": 1e-6,
            "second_dispatch_pipeline_compiled": second.pipeline_compiled,
            "second_dispatch_bind_group_created": second.bind_group_created,
            "disabled_dispatch_unsupported": true,
            "disabled_readback_unsupported": true,
        },
        "stats": {
            "dispatches": stats.dispatches,
            "pipeline_compiles": stats.pipeline_compiles,
            "pipeline_cache_hits": stats.pipeline_cache_hits,
            "bind_group_creates": stats.bind_group_creates,
            "bind_group_cache_hits": stats.bind_group_cache_hits,
        },
    });
    match output {
        Some(path) => {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            fs::write(
                &path,
                serde_json::to_string_pretty(&report).expect("serialize"),
            )
            .expect("write report");
            println!("report: {}", path.display());
        }
        None => println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("serialize")
        ),
    }
    println!(
        "wp0_compute_readback_ok int_exact=true float_max_abs_error={max_abs_error} \
         cache_hit=true rollback_unsupported=true backend={:?} workgroup={}",
        info.backend, capabilities.max_compute_workgroup_size[0]
    );
    ExitCode::SUCCESS
}

/// dispatch/读回失败：本机算得出来的门不该失败，但无 compute 的后端确实会——两种都要说清。
fn blocked_dispatch(stage: &str, error: &ComputeError) -> ExitCode {
    match error {
        ComputeError::Unsupported { reason } => {
            eprintln!("BLOCKED: {stage} 被后端拒绝（{reason}）(没跑 ≠ 通过)");
            ExitCode::from(3)
        }
        other => {
            eprintln!("FAIL: {stage} 返回 {other}");
            ExitCode::FAILURE
        }
    }
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
