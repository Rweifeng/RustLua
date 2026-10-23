//! P3-R04-4 变体缓存 golden（`wp0-variant-golden-v1`）：启动期预编译的**数量、耗时、命中**。
//!
//! 这里不重复 `wp0_3d_golden` 的像素读数（那条已覆盖蒙皮/静态绘制，且要求零 diff）；
//! 本报告只回答三个问题：
//!
//! - 基础变体矩阵是否编译通过（网格/阴影 × 静态/蒙皮 ≥ 4 条）；
//! - 是否存在重复管线（`cache_misses == variant_count`，构造期没有同一键请求两次）；
//! - 启动预编译是否在 250 ms 预算内（`within_budget`）。
//!
//! `normal_map` 轴的像素对比要等后续材质 golden；`ibl` 轴已在 R03-2 接入 key（按需编译，
//! 不影响启动期 6 条基础变体），其像素 golden 由 R03-4 的 `wp0_ibl_golden` 覆盖。
//!
//! 运行：`cargo run -p engine --example wp0_variant_golden -- --output artifacts/p3/variants`

use std::{env, fs, path::PathBuf, process::ExitCode};

use engine::host::Renderer;
use engine::host::renderer::pipeline_cache::VARIANT_COMPILE_BUDGET_MS;
use serde_json::json;

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
        eprintln!("BLOCKED: no GPU adapter available; variant golden cannot run (没跑 ≠ 通过)");
        return ExitCode::from(3);
    };
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-variant-golden"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request golden device");
    let renderer =
        Renderer::new_offscreen(device, queue, None, false, false).expect("offscreen renderer");
    let stats = renderer.pipeline_cache_stats();
    let resources = renderer.resource_stats();
    let compile_ms = stats.compile_ms();

    let report = json!({
        "report_version": "wp0-variant-golden-v1",
        "adapter": {
            "backend": format!("{:?}", info.backend),
            "name": info.name,
        },
        "budget_ms": VARIANT_COMPILE_BUDGET_MS,
        "variant_count": stats.variants,
        "compile_ms": compile_ms,
        "cache_hits": stats.hits,
        "cache_misses": stats.misses,
        "within_budget": stats.within_budget(),
        "resource_stats": {
            "pipeline_variants": resources.pipeline_variants,
            "pipeline_variant_hits": resources.pipeline_variant_hits,
            "pipeline_variant_misses": resources.pipeline_variant_misses,
            "pipeline_compile_ms": resources.pipeline_compile_ms,
        },
        "deferred_axes": ["normal_map"],
    });
    let text = serde_json::to_string_pretty(&report).expect("serialize variant golden") + "\n";
    if let Some(path) = output {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create output dir");
        }
        fs::write(&path, text).expect("write variant golden");
    } else {
        print!("{text}");
    }

    assert!(
        stats.variants >= 4,
        "基础变体矩阵不完整：{} < 4",
        stats.variants
    );
    assert_eq!(
        stats.misses as usize, stats.variants,
        "重复管线：{} 条变体却有 {} 次编译",
        stats.variants, stats.misses
    );
    assert_eq!(stats.hits, 0, "构造期不应重复请求同一变体");
    assert_eq!(
        resources.pipeline_variants, stats.variants,
        "资源快照与缓存统计口径不一致"
    );
    if !stats.within_budget() {
        eprintln!(
            "FAIL: variant compile {compile_ms:.2} ms > budget {VARIANT_COMPILE_BUDGET_MS:.0} ms"
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn parse_output() -> Option<PathBuf> {
    let mut output = None;
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().expect("--output requires a path"),
                ))
            }
            "--help" | "-h" => {
                println!("Usage: wp0_variant_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
