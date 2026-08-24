//! M7-04 精灵批处理 CPU 基准：1000 精灵的 push + build 吞吐（无窗口）。
//!
//! 运行：`cargo run --release -p engine --example sprite_stress`

use engine::core::{
    math2d::{Camera2D, Viewport},
    sprite::Sprite,
    sprite_batch::{SpriteBatch, SpriteBatchItem},
    transform::{Transform, Vec2},
};
use std::time::Instant;

const SPRITES: usize = 1000;
const CYCLES: usize = 60;

fn fill(batch: &mut SpriteBatch) {
    let texture_count = 4;
    for i in 0..SPRITES {
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
    let camera = Camera2D::new(Vec2::new(1280.0, 720.0), 1.0).expect("camera");
    let viewport = Viewport::new(1280.0, 720.0).expect("viewport");
    let mut batch = SpriteBatch::with_capacity(SPRITES);

    // 预热，避免首轮分配抖动。
    for _ in 0..10 {
        batch.clear();
        fill(&mut batch);
        let _ = batch.build(camera, viewport);
    }

    // push 吞吐。
    batch.clear();
    let push_start = Instant::now();
    for _ in 0..CYCLES {
        batch.clear();
        fill(&mut batch);
    }
    let push_ms = push_start.elapsed().as_secs_f64() * 1000.0 / CYCLES as f64;

    // build（排序 + 顶点生成）单轮耗时。
    batch.clear();
    fill(&mut batch);
    let build_start = Instant::now();
    let mesh = batch.build(camera, viewport).expect("build");
    let build_ms = build_start.elapsed().as_secs_f64() * 1000.0;

    println!(
        "sprite_stress_ok sprites={SPRITES} push_ms={push_ms:.3} build_ms={build_ms:.3} vertices={} draw_batches={}",
        mesh.vertices.len(),
        mesh.draws.len()
    );
}
