//! P0-5 诊断：拆解 `atomic_write_scene` 里各段的成本。
//!
//! `save_scene` 一次约 36 ms，其中写盘段 27 ms。该段包含：序列化 `to_text()`、
//! 写临时文件、**重新 `Scene::load` 校验**、两次 rename、删 backup。
//! 这里在进程内单独量"序列化"与"回读校验"两段，判断 27 ms 里有多少是纯 CPU。

use std::time::Instant;

use rustlua_editor_core::perf_fixture::{FixtureSize, build_scene_fixture};

fn main() {
    for size in [FixtureSize::Small, FixtureSize::Medium, FixtureSize::Large] {
        let fixture = build_scene_fixture(size.spec());
        let scene = &fixture.document.scene;
        let entities = scene.entities.len();
        // 序列化
        let rounds = 5;
        let mut ser = Vec::new();
        let mut text_len = 0;
        for _ in 0..rounds {
            let t = Instant::now();
            let text = scene.to_text();
            ser.push(t.elapsed().as_secs_f64() * 1000.0);
            text_len = text.len();
        }
        // 回读校验（parse 一遍刚序列化的文本）
        let text = scene.to_text();
        let mut parse = Vec::new();
        for _ in 0..rounds {
            let t = Instant::now();
            let _ = rustlua_scene::scene::Scene::from_text(&text);
            parse.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        ser.sort_by(|a, b| a.partial_cmp(b).unwrap());
        parse.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "{:>8?} entities={:<6} text={:>9} B  to_text p50={:>7.2} ms  回读校验 p50={:>7.2} ms",
            size,
            entities,
            text_len,
            ser[rounds / 2],
            parse[rounds / 2]
        );
    }
}
