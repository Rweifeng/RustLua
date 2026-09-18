//! M7-04 / M15-00 精灵批处理 CPU 基准：可配置精灵数的 push + build 吞吐（无窗口）。
//!
//! 运行：`cargo run --release -p engine --example sprite_stress -- --sprites 1000 --cycles 60`

use engine::core::{
    camera::{Camera2D, Viewport},
    par::Parallelism,
    sprite::Sprite,
    sprite_batch::{SpriteBatch, SpriteBatchItem, SpriteBatchMesh},
    transform::{Transform, Vec2},
};
use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

const DEFAULT_SPRITES: usize = 1000;
const DEFAULT_CYCLES: usize = 60;
const REPORT_VERSION: &str = "wp0-v2";

#[derive(Debug, Clone, Copy)]
struct Stats {
    count: usize,
    min_us: u128,
    average_us: u128,
    p50_us: u128,
    p95_us: u128,
    p99_us: u128,
    max_us: u128,
}

#[derive(Debug)]
struct Config {
    sprites: usize,
    cycles: usize,
    output: Option<PathBuf>,
    /// 并行线程数覆盖（`None` = 用检测到的核数）。用于验证"线程越多不一定越快"。
    parallel_threads: Option<usize>,
}

fn parse_config() -> Config {
    let mut config = Config {
        sprites: DEFAULT_SPRITES,
        cycles: DEFAULT_CYCLES,
        output: None,
        parallel_threads: None,
    };
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--sprites" => {
                config.sprites = args
                    .next()
                    .unwrap_or_else(|| panic!("{flag} requires a value"))
                    .parse()
                    .expect("--sprites must be an integer")
            }
            "--parallel-threads" => {
                config.parallel_threads = Some(
                    args.next()
                        .unwrap_or_else(|| panic!("{flag} requires a value"))
                        .parse()
                        .expect("--parallel-threads must be an integer"),
                );
            }
            "--cycles" => {
                config.cycles = args
                    .next()
                    .unwrap_or_else(|| panic!("{flag} requires a value"))
                    .parse()
                    .expect("--cycles must be an integer")
            }
            "--output" => {
                config.output = Some(PathBuf::from(
                    args.next()
                        .unwrap_or_else(|| panic!("{flag} requires a value")),
                ))
            }
            "--help" | "-h" => {
                println!("Usage: sprite_stress [--sprites N] [--cycles N] [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    assert!(config.sprites > 0, "--sprites must be greater than zero");
    assert!(config.cycles > 0, "--cycles must be greater than zero");
    config
}

fn fill(batch: &mut SpriteBatch, sprites: usize) {
    let texture_count = 4;
    for i in 0..sprites {
        let sprite = Sprite::new(Vec2::new(32.0, 32.0)).expect("sprite");
        let texture_id = (i % texture_count) as u32;
        let layer = (i / 500) as i32;
        let transform = Transform {
            position: Vec2::new(
                (i % 40) as f32 * 34.0 - 600.0,
                (i / 40) as f32 * 34.0 - 300.0,
            ),
            ..Transform::default()
        };
        batch.push(SpriteBatchItem::new(texture_id, layer, sprite, transform));
    }
}

fn main() {
    let config = parse_config();
    let camera = Camera2D::new(Vec2::new(1280.0, 720.0), 1.0).expect("camera");
    let viewport = Viewport::new(1280.0, 720.0).expect("viewport");
    let mut batch = SpriteBatch::with_capacity(config.sprites);

    // 预热，避免首轮分配抖动。
    for _ in 0..10 {
        batch.clear();
        fill(&mut batch, config.sprites);
        let _ = batch.build(camera, viewport);
    }

    // push 吞吐。
    batch.clear();
    let mut push_samples = Vec::with_capacity(config.cycles);
    for _ in 0..config.cycles {
        let start = Instant::now();
        batch.clear();
        fill(&mut batch, config.sprites);
        push_samples.push(start.elapsed());
    }

    // build（排序 + 顶点生成）单轮耗时。
    batch.clear();
    fill(&mut batch, config.sprites);
    let mut build_samples = Vec::with_capacity(config.cycles);
    let mut mesh = None;
    for _ in 0..config.cycles {
        let start = Instant::now();
        mesh = Some(batch.build(camera, viewport).expect("build"));
        build_samples.push(start.elapsed());
    }
    let mesh = mesh.expect("mesh");
    let push = summarize(push_samples);
    let build = summarize(build_samples);

    // WP8 A/B：同一批次、同一进程内交替测串行与并行两种顶点生成，并用字节指纹证明
    // 两者输出完全相同。交替采样是为了抵消机器负载漂移；指纹不同就直接判失败——
    // 并行只允许改变"在哪个线程算"，不允许改变结果。
    let parallelism = config
        .parallel_threads
        .map_or_else(Parallelism::detected, Parallelism::with_threads)
        // 基准刻意覆盖阈值：它要对比的是同一批次的两种实现，而不是运行时默认策略。
        .with_min_items(1);
    let mut sequential_samples = Vec::with_capacity(config.cycles);
    let mut parallel_samples = Vec::with_capacity(config.cycles);
    let mut sequential_digest = 0u64;
    let mut parallel_digest = 0u64;
    let mut parallel_used = false;
    for _ in 0..config.cycles {
        let start = Instant::now();
        let sequential_mesh = batch
            .build_with_parallelism(camera, viewport, Parallelism::SEQUENTIAL)
            .expect("sequential build");
        sequential_samples.push(start.elapsed());
        let start = Instant::now();
        let (parallel_mesh, used) =
            match batch.build_with_parallelism(camera, viewport, parallelism) {
                Ok(mesh) => (mesh, parallelism.effective_threads(config.sprites) > 1),
                Err(error) => panic!("parallel build failed: {error}"),
            };
        parallel_samples.push(start.elapsed());
        parallel_used = used;
        sequential_digest = digest_mesh(&sequential_mesh);
        parallel_digest = digest_mesh(&parallel_mesh);
    }
    let sequential = summarize(sequential_samples);
    let concurrent = summarize(parallel_samples);
    let identical = sequential_digest == parallel_digest && sequential_digest != 0;

    println!(
        "sprite_stress_ok report_version={REPORT_VERSION} sprites={} cycles={} push_p50_us={} push_p95_us={} push_p99_us={} build_p50_us={} build_p95_us={} build_p99_us={} vertices={} draw_batches={} instances={} triangles={} parallel_threads={} parallel_used={} parallel_identical={} sequential_build_p50_us={} parallel_build_p50_us={}",
        config.sprites,
        config.cycles,
        push.p50_us,
        push.p95_us,
        push.p99_us,
        build.p50_us,
        build.p95_us,
        build.p99_us,
        mesh.vertices.len(),
        mesh.draws.len(),
        config.sprites,
        config.sprites * 2,
        parallelism.threads(),
        parallel_used,
        identical,
        sequential.p50_us,
        concurrent.p50_us,
    );
    if let Some(path) = config.output.as_ref() {
        write_report(
            path,
            &config,
            push,
            build,
            mesh.draws.len(),
            ParallelAb {
                threads: parallelism.threads(),
                used: parallel_used,
                identical,
                sequential,
                parallel: concurrent,
                digest: parallel_digest,
            },
        );
    }
    assert!(
        identical,
        "并行与串行的顶点输出必须逐字节相同（sequential={sequential_digest:#x} parallel={parallel_digest:#x}）"
    );
}

/// 同一批次串行/并行的对拍结果（WP8）。
struct ParallelAb {
    threads: usize,
    used: bool,
    identical: bool,
    sequential: Stats,
    parallel: Stats,
    digest: u64,
}

/// FNV-1a：只用来证明两条路径的输出**相同**，不用于跨机器比较。
fn digest_mesh(mesh: &SpriteBatchMesh) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |byte: u8| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for vertex in &mesh.vertices {
        for value in vertex.position {
            for byte in value.to_bits().to_le_bytes() {
                mix(byte);
            }
        }
        for value in vertex.uv {
            for byte in value.to_bits().to_le_bytes() {
                mix(byte);
            }
        }
        for value in vertex.color {
            for byte in value.to_bits().to_le_bytes() {
                mix(byte);
            }
        }
    }
    for draw in &mesh.draws {
        for byte in draw.texture_id.to_le_bytes() {
            mix(byte);
        }
        for byte in draw.layer.to_le_bytes() {
            mix(byte);
        }
        for byte in draw.vertex_start.to_le_bytes() {
            mix(byte);
        }
        for byte in draw.vertex_count.to_le_bytes() {
            mix(byte);
        }
    }
    hash
}

fn summarize(mut values: Vec<Duration>) -> Stats {
    values.sort_unstable();
    let percentile =
        |percent: usize| ((values.len() * percent).saturating_sub(1) / 100).min(values.len() - 1);
    Stats {
        count: values.len(),
        min_us: values[0].as_micros(),
        average_us: values.iter().map(Duration::as_micros).sum::<u128>() / values.len() as u128,
        p50_us: values[percentile(50)].as_micros(),
        p95_us: values[percentile(95)].as_micros(),
        p99_us: values[percentile(99)].as_micros(),
        max_us: values[values.len() - 1].as_micros(),
    }
}

fn write_report(
    path: &PathBuf,
    config: &Config,
    push: Stats,
    build: Stats,
    draws: usize,
    ab: ParallelAb,
) {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).expect("create report directory");
    }
    let json = format!(
        "{{\n  \"report_version\": \"{REPORT_VERSION}\",\n  \"benchmark\": \"sprite_stress\",\n  \"sprites\": {},\n  \"cycles\": {},\n  \"metrics\": {{\n    \"push_us\": {},\n    \"build_us\": {},\n    \"render\": {{\"draw_calls\":{},\"instances\":{},\"triangles\":{},\"cpu_frame_us\":null,\"gpu_frame_us\":null}}\n  }},\n  \"parallel_ab\": {{\n    \"threads\": {},\n    \"used\": {},\n    \"identical\": {},\n    \"digest\": \"{:#018x}\",\n    \"sequential_build_us\": {},\n    \"parallel_build_us\": {}\n  }}\n}}\n",
        config.sprites,
        config.cycles,
        stats_json(push),
        stats_json(build),
        draws,
        config.sprites,
        config.sprites * 2,
        ab.threads,
        ab.used,
        ab.identical,
        ab.digest,
        stats_json(ab.sequential),
        stats_json(ab.parallel),
    );
    fs::write(path, json).expect("write sprite report");
}

fn stats_json(stat: Stats) -> String {
    format!(
        "{{\"count\":{},\"min_us\":{},\"average_us\":{},\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"max_us\":{}}}",
        stat.count,
        stat.min_us,
        stat.average_us,
        stat.p50_us,
        stat.p95_us,
        stat.p99_us,
        stat.max_us
    )
}
