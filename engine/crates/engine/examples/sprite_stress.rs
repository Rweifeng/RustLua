//! M7-04 / M15-00 精灵批处理 CPU 基准：可配置精灵数的 push + build 吞吐（无窗口）。
//!
//! 运行：`cargo run --release -p engine --example sprite_stress -- --sprites 1000 --cycles 60`

use engine::core::{
    math2d::{Camera2D, Viewport},
    sprite::Sprite,
    sprite_batch::{SpriteBatch, SpriteBatchItem},
    transform::{Transform, Vec2},
};
use std::{env, time::Instant};

const DEFAULT_SPRITES: usize = 1000;
const DEFAULT_CYCLES: usize = 60;

#[derive(Debug)]
struct Config {
    sprites: usize,
    cycles: usize,
}

fn parse_config() -> Config {
    let mut config = Config {
        sprites: DEFAULT_SPRITES,
        cycles: DEFAULT_CYCLES,
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
            "--help" | "-h" => {
                println!("Usage: sprite_stress [--sprites N] [--cycles N]");
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
    let push_start = Instant::now();
    for _ in 0..config.cycles {
        batch.clear();
        fill(&mut batch, config.sprites);
    }
    let push_ms = push_start.elapsed().as_secs_f64() * 1000.0 / config.cycles as f64;

    // build（排序 + 顶点生成）单轮耗时。
    batch.clear();
    fill(&mut batch, config.sprites);
    let build_start = Instant::now();
    let mesh = batch.build(camera, viewport).expect("build");
    let build_ms = build_start.elapsed().as_secs_f64() * 1000.0;

    println!(
        "sprite_stress_ok sprites={} cycles={} push_ms={push_ms:.3} build_ms={build_ms:.3} vertices={} draw_batches={}",
        config.sprites,
        config.cycles,
        mesh.vertices.len(),
        mesh.draws.len()
    );
}
