//! E03-1 golden：固定 `.vgraph` → 逐字节比对 `tests/fixtures/*.generated.lua` → Lua 5.4 加载。
//!
//! `--write` 只用于生成器/节点集**明确变更**后重刷 baseline；验收跑不带参数。
//! 生成失败（校验错误）时不会碰 baseline。

use std::path::PathBuf;

use rustlua_visual_script::{Graph, compile};

const SOURCE: &str = "assets/graphs/p3-e03-interact.vgraph";
const GOLDEN: &str = "tests/fixtures/p3-e03-interact.generated.lua";

fn main() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/p3-e03-interact.vgraph");
    let text = std::fs::read_to_string(&fixture).expect("读取 vgraph fixture");
    let graph = Graph::from_json(&text).expect("解析 vgraph");
    let generated = match compile(&graph, SOURCE) {
        Ok(lua) => lua,
        Err(errors) => {
            eprintln!("vgraph_golden_failed errors={errors:#?}");
            std::process::exit(2);
        }
    };

    let lua = mlua::Lua::new();
    let module: mlua::Table = lua.load(&generated).eval().expect("Lua 5.4 加载生成物");
    let mut functions = Vec::new();
    for name in ["on_start", "on_update", "on_interact", "on_collision"] {
        assert!(module.contains_key(name).unwrap(), "缺少 {name}");
        functions.push(name);
    }

    let golden_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    if std::env::args().any(|arg| arg == "--write") {
        std::fs::write(&golden_path, &generated).expect("写 golden");
        println!(
            "vgraph_golden_written path={GOLDEN} bytes={}",
            generated.len()
        );
        return;
    }
    let golden = std::fs::read_to_string(&golden_path).expect("golden 缺失；先跑 --write");
    let golden = golden.replace("\r\n", "\n");
    let generated_normalized = generated.replace("\r\n", "\n");
    if golden != generated_normalized {
        first_difference(&golden, &generated_normalized);
        eprintln!("vgraph_golden_failed reason=golden_mismatch path={GOLDEN}");
        std::process::exit(1);
    }

    println!(
        "{{\"gate\":\"wp0_vgraph_golden\",\"graph\":\"{}\",\"nodes\":{},\"edges\":{},\"functions\":{},\"bytes\":{},\"lua_load\":\"ok\",\"golden\":\"match\"}}",
        graph.name,
        graph.nodes.len(),
        graph.edges.len(),
        functions.len(),
        generated.len()
    );
}

fn first_difference(expected: &str, actual: &str) {
    for (index, (left, right)) in expected.lines().zip(actual.lines()).enumerate() {
        if left != right {
            eprintln!("first difference at line {}", index + 1);
            eprintln!("  golden: {left}");
            eprintln!("  generated: {right}");
            return;
        }
    }
    eprintln!(
        "line count differs: golden={} generated={}",
        expected.lines().count(),
        actual.lines().count()
    );
}
