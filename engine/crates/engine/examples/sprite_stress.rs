//! M7-04 / M15-00 精灵批处理 CPU 基准：可配置精灵数的 push + build 吞吐（无窗口）。
//!
//! 运行：`cargo run --release -p engine --example sprite_stress -- --sprites 1000 --cycles 60`

use engine::core::{
    math2d::{Camera2D, Viewport},
    sprite::Sprite,
    sprite_batch::{SpriteBatch, SpriteBatchItem},
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
}

fn parse_config() -> Config {
    let mut config = Config {
        sprites: DEFAULT_SPRITES,
        cycles: DEFAULT_CYCLES,
        output: None,
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

    println!(
        "sprite_stress_ok report_version={REPORT_VERSION} sprites={} cycles={} push_p50_us={} push_p95_us={} push_p99_us={} build_p50_us={} build_p95_us={} build_p99_us={} vertices={} draw_batches={} instances={} triangles={}",
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
        config.sprites * 2
    );
    if let Some(path) = config.output.as_ref() {
        write_report(path, &config, push, build, mesh.draws.len());
    }
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

fn write_report(path: &PathBuf, config: &Config, push: Stats, build: Stats, draws: usize) {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).expect("create report directory");
    }
    let json = format!(
        "{{\n  \"report_version\": \"{REPORT_VERSION}\",\n  \"benchmark\": \"sprite_stress\",\n  \"sprites\": {},\n  \"cycles\": {},\n  \"metrics\": {{\n    \"push_us\": {},\n    \"build_us\": {},\n    \"render\": {{\"draw_calls\":{},\"instances\":{},\"triangles\":{},\"cpu_frame_us\":null,\"gpu_frame_us\":null}}\n  }}\n}}\n",
        config.sprites,
        config.cycles,
        stats_json(push),
        stats_json(build),
        draws,
        config.sprites,
        config.sprites * 2
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
