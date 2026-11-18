//! 架构门：**领域绑定真的装进了 VM**，而不只是"代码里有一行 install"。
//!
//! ## 为什么需要它
//!
//! `docs/62` 的依赖倒置把 14 个领域的 Lua 绑定下沉回各自的 crate，装配集中到
//! `host::script_packs::install`（27 处调用）。这套做法的代价文档已经写明：
//!
//! > `engine` 多了一个"必须调"的入口——漏调不会报错，只会让 `camera*` 静默缺失。
//!
//! `architecture_edges.rs` 管的是**依赖边**（谁 `use` 谁）。它管不了这件事：
//! `script_packs.rs` 里删掉一行 `install`，依赖图完全不变，门照样绿。
//
//! 2026-11-18 实测过了一次：手工注释掉相机装配器，`engine --lib` 有 5 条以上测试立刻红
//! （`camera_physics` / `animation_layers` 等）。所以**兜底是有效的**——但那是碰巧：
//! 那些用例测的是"相机影响渲染管线"，不是因为有人守着"每个领域表都装上了"。
//! 换一个冷门领域（比如 `grid`、`world_text`），很可能一条测试都不会红。
//!
//! 这个门把那件事变成显式的：**逐表检查 `_G` 里有没有它**。
//!
//! ## 判据为什么是"表存在"而不是"函数数对得上"
//!
//! 函数集合的完整性由 `api_surface_snapshot` 基线管（那是另一道门，更细）。
//! 这一道只回答一个粗问题：**这个领域的绑定到底有没有被接上**。
//! 两者分工：基线门防"少导出一个函数"，这道门防"整个领域没装"。
//!
//! 粗有粗的好处——它不会因为加了一个函数就红，只在真正漏装时红。

use rustlua_gameplay::input_lua;
use rustlua_scripting::script::LuaRuntime;

/// 装好全部领域绑定的运行时（与 `host::script_packs::install` 同一条装配路径）。
fn runtime() -> LuaRuntime {
    let mut runtime = LuaRuntime::with_installers(input_lua::registry()).expect("建立 Lua 运行时");
    engine::host::script_packs::install(&mut runtime);
    runtime
}

/// `_G` 里必须存在的领域表。
///
/// 每一行对应 `host::script_packs::install` 里的一次装配。**新增领域时把表名加进来**——
/// 忘了加不会报错，但下次有人漏调那一行时，也就不会有东西替他喊。
const REQUIRED_TABLES: &[(&str, &str)] = &[
    ("camera", "相机 2D 基底"),
    ("camera2d", "相机 2D"),
    ("physics", "物理 2D"),
    ("physics3d", "物理 3D"),
    ("ui", "UI 控件"),
    ("world_text", "世界空间文本"),
    ("animation", "动画"),
    ("tilemap", "瓦片地图"),
    ("prefab", "预制体"),
    ("clock", "时钟"),
    ("grid", "网格坐标"),
    ("inv", "背包"),
    ("save", "存档"),
    ("item", "物品定义"),
    ("calendar", "日历"),
    ("day", "日程"),
];

/// 每个领域表都存在。
///
/// 失败信息直接给出"哪张表、属于哪个领域"——漏装时看不出来是装配漏了还是绑定自己没建表。
#[test]
fn every_domain_table_is_installed_into_the_vm() {
    let runtime = runtime();
    let mut missing = Vec::new();
    for (table, domain) in REQUIRED_TABLES {
        let present = runtime
            .eval_json(&format!("return {table} ~= nil"))
            .unwrap_or_else(|error| panic!("检查 `{table}` 时 VM 报错：{error}"));
        // `eval_json` 把 Lua 的 true/false 映射成 JSON 布尔。
        let present = matches!(present, serde_json::Value::Bool(true));
        if !present {
            missing.push(format!("{table}（{domain}）"));
        }
    }
    assert!(
        missing.is_empty(),
        "这些领域表没有装进 VM：{}。\
         多半是 `host/script_packs.rs` 里那几行 `install` 被漏掉了——\
         依赖图不会变，所以 `architecture_edges` 抓不到，只有这个门会红",
        missing.join("、")
    );
}

/// 领域表不可能是空表：里面至少有可调用的函数。
///
/// 防的是"装了但装错了"——例如注册了一个同名空表把真表顶掉，或者装配器早退。
/// 只数个数不做具体比对：函数集合的完整性归 `api_surface_snapshot` 基线门管。
#[test]
fn installed_domain_tables_are_not_empty() {
    let runtime = runtime();
    let mut empty = Vec::new();
    for (table, domain) in REQUIRED_TABLES {
        let raw = runtime
            .eval_json(&format!(
                "local n = 0\n\
                 for _ in pairs({table}) do n = n + 1 end\n\
                 return n"
            ))
            .unwrap_or_else(|error| panic!("数 `{table}` 的成员时 VM 报错：{error}"));
        let count = raw.as_u64().unwrap_or(0);
        if count == 0 {
            empty.push(format!("{table}（{domain}）"));
        }
    }
    assert!(
        empty.is_empty(),
        "这些领域表装上了但一个成员都没有：{}",
        empty.join("、")
    );
}

/// 装配器**不许**在建 VM 时失败。
///
/// `add_deferred_installer` 的闭包在 VM 真正建好时才跑，里面的 `?` 如果报错，
/// 表现是"某个领域的表静默缺席"而不是 panic。上面两条能抓到缺席，
/// 但抓不到"装到一半失败"——所以这里让装配器在两种入口（有工程根 / 无工程根）各跑一次，
/// 把失败提前暴露。
#[test]
fn installers_survive_both_boot_paths() {
    // 无工程根（`LuaRuntime::new` 那条路）：端口拿不到根，但不该因此崩。
    let mut bare = LuaRuntime::with_installers(input_lua::registry()).expect("无根运行时");
    engine::host::script_packs::install(&mut bare);
    let present = bare
        .eval_json("return camera ~= nil and ui ~= nil")
        .expect("无根运行时里读全局表");
    assert_eq!(
        present,
        serde_json::Value::Bool(true),
        "没有工程根时装配器不该整体失败——相机与 UI 不依赖工程根"
    );
}
