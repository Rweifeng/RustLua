//! 导出 Web↔PC API 一致性契约的**桌面侧**规范化输出（基线 transcript）。
//!
//! 用法：
//! - `cargo run -q -p engine --example run_lua_contract` —— 打印到 stdout（人读/临时核对）
//! - `cargo run -q -p engine --example run_lua_contract -- --write` —— 写回
//!   `crates/rustlua-scripting/src/api_surface_snapshot/lua_api_contract_transcript.txt`
//!
//! 同一份 `lua_api_contract.lua` 还会在 web 侧（打完宿主补丁的运行时）跑一遍：
//! `node scripts/wechat-minigame-harness.js <发布目录> --check-api-contract`。
//! 两侧都对齐这份 transcript，PC↔Web 的行为等价就是传递性的——不需要再维护一份"web 期望值"。
//!
//! 事实源是运行期：桌面注册层与 wasm 注册层已实测逐字节相同（见 L1 门），所以这里跑出来的
//! 就是 web 应当达到的行为，而不是另一套手抄规格。

use std::{path::PathBuf, process};

use rustlua_scripting::script::LuaRuntime;

/// 契约用例（两侧共用的同一份 Lua）与 transcript 基线，相对 `crates/engine` 清单文件的路径。
const CONTRACT_REL: &str = "../rustlua-scripting/src/api_surface_snapshot/lua_api_contract.lua";
const TRANSCRIPT_REL: &str =
    "../rustlua-scripting/src/api_surface_snapshot/lua_api_contract_transcript.txt";

fn manifest_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// 在桌面 VM 上跑契约，返回规范化 transcript 文本。
///
/// 走 `eval_json` 而不是直接摸 mlua：`engine` crate 不把 mlua 列成自己的依赖，
/// example 里 `use mlua` 会 E0433；契约返回值本来就是纯字符串，经 JSON 桥无损。
pub fn transcript_of(runtime: &LuaRuntime, source: &str) -> Result<String, String> {
    runtime
        .lua()
        .load(source)
        .set_name("lua_api_contract.lua")
        .exec()
        .map_err(|e| format!("契约用例加载失败: {e}"))?;
    let value = runtime
        .eval_json("__contract_run()")
        .map_err(|e| format!("契约执行失败: {e}"))?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "契约 __contract_run() 未返回字符串".to_owned())
}

fn main() {
    let write = std::env::args().skip(1).any(|arg| arg == "--write");
    let source = std::fs::read_to_string(manifest_path(CONTRACT_REL)).expect("读契约用例失败");

    let runtime = LuaRuntime::new().expect("lua vm");
    let transcript = transcript_of(&runtime, &source).expect("契约执行失败");

    if write {
        std::fs::write(manifest_path(TRANSCRIPT_REL), format!("{transcript}\n"))
            .expect("写入 transcript 失败");
        eprintln!(
            "已写入 {}（{} 行）",
            TRANSCRIPT_REL,
            transcript.lines().count()
        );
        return;
    }
    println!("{transcript}");
    // 非 --write 模式下顺带做一次"与基线一致"的检查，方便本地改完直接看。
    let committed = std::fs::read_to_string(manifest_path(TRANSCRIPT_REL))
        .unwrap_or_default()
        .trim_end()
        .to_owned();
    if committed != transcript.trim_end() {
        eprintln!("注意：当前输出与已提交 transcript 不一致（跑 --write 更新基线）");
        process::exit(1);
    }
}
