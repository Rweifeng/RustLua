//! 导出运行期 Lua API 面清单，用于重生成快照门基线。
//!
//! 用法：
//! - `cargo run -q -p engine --example dump_lua_api` —— 打印到 stdout（人读/临时核对）
//! - `cargo run -q -p engine --example dump_lua_api -- --write` —— 写回
//!   `crates/rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json`
//!   （WP11 拆分期间每改一处 API 都要跑）
//!
//! 事实源是运行期 `_G`，不是任何手抄清单：见 `crate::core::script::LUA_API_PROBE`。

use engine::core::script::{LUA_API_PROBE, LuaRuntime};

/// 基线相对 `crates/engine` 清单文件的路径。
///
/// 必须与 `api_surface_snapshot` 门 `include_str!` 的那一份指向同一个文件——WP11 拆分把基线
/// 挪进了 `rustlua-scripting`，这里的 `src/core/...` 没跟着改，于是 `--write` 一直在
/// panic（目录不存在），基线只能手改。
const BASELINE_REL: &str = "../rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json";

fn main() {
    let write = std::env::args().skip(1).any(|arg| arg == "--write");
    let runtime = LuaRuntime::new().expect("lua vm");
    let surface = runtime
        .eval_json(LUA_API_PROBE)
        .expect("导出运行时 API 面失败");

    let tables = surface["tables"].as_array().expect("tables 应为数组");
    let funcs: usize = tables
        .iter()
        .map(|table| table["funcs"].as_array().expect("funcs").len())
        .sum();
    eprintln!("模块表 {} 个，函数 {funcs} 个", tables.len());

    let json = serde_json::to_string_pretty(&surface).expect("序列化失败");
    if write {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BASELINE_REL);
        std::fs::write(&path, format!("{json}\n")).expect("写入基线失败");
        eprintln!("已写入 {}", path.display());
    } else {
        println!("{json}");
    }
}
