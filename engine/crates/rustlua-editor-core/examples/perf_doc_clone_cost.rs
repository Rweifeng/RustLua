//! P0-3 诊断：量化 `History::execute` 里"全文档快照"的成本。
//!
//! 为什么需要这个读数：`--perf-trace` 的 trace 显示 `dispatch:SetComponentField`
//! 单次约 941 µs（10k 实体档），但 `dispatch` 的 span 内部没有子划分。
//! `rustlua-editor-core` 不持有 `Profiler`，所以在这里单独量一次"文档克隆 / 比较"
//! 的净成本，用来判断 941 µs 里有多少是 undo 快照造成的。
//!
//! 运行：`cargo run --release -p rustlua-editor-core --example perf_doc_clone_cost`

use std::time::Instant;

use rustlua_editor_core::perf_fixture::{FixtureSize, build_scene_fixture};

fn main() {
    for size in [FixtureSize::Small, FixtureSize::Medium, FixtureSize::Large] {
        let spec = size.spec();
        let fixture = build_scene_fixture(spec);
        let entities = fixture.document.scene.entities.len();
        // 与 `History::execute` 同一条路径：clone → 比较 → clone。
        let rounds = 20;
        let mut samples = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let started = Instant::now();
            let before = fixture.document.clone();
            let after = fixture.document.clone();
            let equal = fixture.document == before;
            std::hint::black_box(&after);
            std::hint::black_box(equal);
            samples.push(started.elapsed().as_secs_f64() * 1_000_000.0);
        }
        samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        let p50 = samples[samples.len() / 2];
        println!(
            "{:>8?} entities={:<6} 两次clone+比较 p50 = {:>9.1} us",
            size, entities, p50
        );
    }
}
