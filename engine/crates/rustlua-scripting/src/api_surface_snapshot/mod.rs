//! Lua API 面的**快照门**（WP11 阶段 0）。
//!
//! 事实源是运行期：探针 `crate::script::LUA_API_PROBE` 遍历 `_G` 导出「模块表 → 函数名」，
//! 与提交进仓库的基线 `lua_api_surface.json` 逐项比对。
//!
//! 它钉住的漂移是 `register_api` 的固有风险——同名函数注册两次、后者静默覆盖，或搬运时漏掉一整个
//! 模块表。这类问题**画面与样例都可能照跑**（docs/37 记录过 `input.action` 被注册两次、
//! 旧的那份成为死代码且语义变更无迁移，直到 M22 批 1 才修）。人眼 diff 12k 行抓不住，
//! 只能靠快照。
//!
//! 基线由 `cargo run -p engine --example dump_lua_api -- --write` 重生成。
//! 拆分 WP11 期间这条门必须始终绿；**只有有意增删 Lua API 时**才更新基线（等价于一次 API review）。

use std::collections::BTreeMap;

use crate::script::LUA_API_PROBE;
use crate::script::LuaRuntime;

/// 基线随源码提交（不放 `baselines/`——那个目录整体被 `.gitignore` 排除，
/// CI 拿不到文件，门就不成其为门）。
const BASELINE: &str = include_str!("lua_api_surface.json");

/// 探针输出 → `模块名 → 排序后的函数名列表`。
fn surface_tables(value: &serde_json::Value) -> BTreeMap<String, Vec<String>> {
    let mut tables = BTreeMap::new();
    for table in value["tables"].as_array().expect("tables 应为数组") {
        let name = table["name"].as_str().expect("模块名").to_owned();
        let mut funcs: Vec<String> = table["funcs"]
            .as_array()
            .expect("funcs 应为数组")
            .iter()
            .map(|func| func.as_str().expect("函数名").to_owned())
            .collect();
        funcs.sort();
        tables.insert(name, funcs);
    }
    tables
}

#[test]
fn lua_api_surface_matches_baseline() {
    let runtime = LuaRuntime::new().expect("lua vm");
    let current = surface_tables(
        &runtime
            .eval_json(LUA_API_PROBE)
            .expect("导出运行时 API 面失败"),
    );
    let baseline = surface_tables(
        &serde_json::from_str::<serde_json::Value>(BASELINE).expect("基线 JSON 解析失败"),
    );

    let mut issues: Vec<String> = Vec::new();
    for (name, funcs) in &baseline {
        match current.get(name) {
            None => issues.push(format!(
                "模块表 `{name}` 整块消失（基线 {count} 个函数）",
                count = funcs.len()
            )),
            Some(now) => {
                for func in funcs.iter().filter(|func| !now.contains(func)) {
                    issues.push(format!("`{name}.{func}` 不再注册"));
                }
                for func in now.iter().filter(|func| !funcs.contains(func)) {
                    issues.push(format!("`{name}.{func}` 是基线里没有的新增"));
                }
            }
        }
    }
    for name in current.keys().filter(|name| !baseline.contains_key(*name)) {
        issues.push(format!("多出模块表 `{name}`"));
    }

    let current_total: usize = current.values().map(Vec::len).sum();
    let baseline_total: usize = baseline.values().map(Vec::len).sum();
    assert!(
        issues.is_empty(),
        "Lua API 面与基线不一致（基线 {baseline_total} 个函数 / 当前 {current_total} 个）：\n  - {}\n\
         拆分与搬运不得改变 API 面；确属有意增删时跑\ncargo run -p engine --example dump_lua_api -- --write\
         并说明理由",
        issues.join("\n  - ")
    );
    assert_eq!(
        current.len(),
        baseline.len(),
        "模块表数量变了（基线 {}，当前 {}）",
        baseline.len(),
        current.len()
    );
}

/// L3 行为契约的**桌面侧**基线（与 `crates/engine/examples/run_lua_contract.rs` 同一份 Lua）。
const CONTRACT: &str = include_str!("lua_api_contract.lua");
const CONTRACT_TRANSCRIPT: &str = include_str!("lua_api_contract_transcript.txt");

/// 契约在活体桌面上跑出来的 transcript 必须等于提交进仓库的那份。
///
/// 为什么名字门不够：`lua_api_surface_matches_baseline` 只保证**注册层**两边同名同形，
/// 而 web 的 57 个补丁覆盖点改的是**行为**（`clock.now()` 少三个字段就是实测的一例），
/// 名字对拍一个字都抓不到。这份 transcript 把"两侧理应相同"的语义钉成文本，
/// web 侧（`node scripts/wechat-minigame-harness.js <发布目录> --check-api-contract`）
/// 对的是**同一份基线**，所以 PC↔Web 的等价是传递性的，不需要另写一套"web 期望值"。
#[test]
fn lua_api_contract_matches_transcript() {
    let runtime = LuaRuntime::new().expect("lua vm");
    runtime
        .lua()
        .load(CONTRACT)
        .set_name("lua_api_contract.lua")
        .exec()
        .expect("契约用例加载失败");
    let value = runtime.eval_json("__contract_run()").expect("契约执行失败");
    let current = value.as_str().expect("契约应返回字符串").trim_end();
    let baseline = CONTRACT_TRANSCRIPT.trim_end();
    if current == baseline {
        return;
    }
    // 逐行报差异：一行一条用例，行号即用例位置，改完能直接定位是哪条语义变了。
    let mut issues = Vec::new();
    let mut current_lines = current.lines();
    let mut baseline_lines = baseline.lines();
    for index in 1.. {
        match (current_lines.next(), baseline_lines.next()) {
            (None, None) => break,
            (lhs, rhs) => {
                if lhs != rhs {
                    issues.push(format!(
                        "L{index} 基线 {:?} / 当前 {:?}",
                        rhs.unwrap_or("<行数少>"),
                        lhs.unwrap_or("<行数多>")
                    ));
                }
            }
        }
    }
    panic!(
        "L3 行为契约与基线 transcript 不一致（重生成：cargo test -p rustlua-scripting 之后跑 \
         cargo run -q -p engine --example run_lua_contract -- --write）：\n{}",
        issues.join("\n")
    );
}
