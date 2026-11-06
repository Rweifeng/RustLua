//! Lua 脚本层性能分析：纯 Lua 执行、Lua→Rust API 调用、Rust→Lua 回调三组基线。
//!
//! 目的不是再造一个通用 benchmark，而是回答两个具体问题：
//!
//! 1. **Lua 游戏脚本本身**在 PUC-Rio Lua 5.4.8（无 JIT）上每步/每调用要花多少 ns；
//! 2. **跨语言边界**（mlua 的 `create_function` 闭包、表参数/返回值、RefCell 借用、
//!    `entity.find` 的字符串转换）相对纯 Lua 调用贵多少。
//!
//! 口径与仓库其它 perf 例子一致：Release、固定循环、预热、多个样本取 p50/p95/p99；
//! 时间用 `Instant`，Lua 堆增量用 `Lua::used_memory`（采样窗口内 `gc_stop`，因此
//! `lua_bytes_per_op` 是真实分配字节而不是净留存），Rust 侧分配用 `wp0_alloc` 计数。
//!
//! 运行：
//! ```powershell
//! cd engine
//! cargo run --release -p engine --example lua_perf -- --cycles 11 `
//!   --output ..\metrics\baselines\lua-perf.json
//! ```
//!
//! **读法**：`rust_api/*` 的 ns/op 包含 Lua 循环与调用开销；减掉 `rust_api/noop`
//! 才是"这个 API 比一次空 Rust 调用多出来的部分"。`derived` 段已经算好这些差值。

#[path = "support/wp0_alloc.rs"]
mod wp0_alloc;

use std::{
    env, fmt, fs,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use mlua::{Function, Lua};
use rustlua_scripting::script::LuaRuntime;

const REPORT_VERSION: &str = "lua-perf-v1";
const DEFAULT_CYCLES: usize = 11;
const WARMUP_ROUNDS: usize = 2;
/// 单个样本的目标时长。太短会被计时噪声淹没，太长会让采样窗口里的 Lua 堆涨得太大。
const TARGET_SAMPLE_NS: f64 = 15_000_000.0;
const MIN_OPS: usize = 2_000;

struct Config {
    cycles: usize,
    output: Option<PathBuf>,
}

#[derive(Clone, Copy)]
struct Scenario {
    group: &'static str,
    name: &'static str,
    setup: &'static str,
    body: &'static str,
    max_ops: usize,
}

/// 公共 fixture：一个带全部领域绑定的 VM + 一个 2D 实体 `bench` + 一个 3D 实体 `bench3d`。
const API_SETUP: &str = r#"
local id = __bench_id
local deep = __bench_deep
local name = __bench_name
local noop = bench.noop
local add = bench.add
local tbl = bench.table
local tbl6 = bench.table6
local tbl6_default = bench.table6_default
local delta = time.delta
local alive = entity.is_alive
local find = entity.find
local get = transform.get
local set = transform.set
local move = transform.move
local space = transform.space
local pos = transform.position
local wpos = transform.world_position
local pos3 = transform3d.position
local wpos3 = transform3d.world_position
local emit = event.emit
"#;

const SCENARIOS: &[Scenario] = &[
    // ---------------------------------------------------------------- 纯 Lua
    Scenario {
        group: "pure_lua",
        name: "loop_add",
        setup: "",
        body: "acc = acc + i",
        max_ops: 2_000_000,
    },
    Scenario {
        group: "pure_lua",
        name: "function_call",
        setup: "local f = function(x) return x + 1 end",
        body: "acc = f(acc)",
        max_ops: 2_000_000,
    },
    Scenario {
        group: "pure_lua",
        name: "table_get",
        setup: "local t = {1, 2, 3, 4, 5, 6, 7, 8}",
        body: "acc = acc + t[(i & 7) + 1]",
        max_ops: 1_000_000,
    },
    Scenario {
        group: "pure_lua",
        name: "table_set",
        setup: "local t = {1, 2, 3, 4, 5, 6, 7, 8}",
        body: "t[(i & 7) + 1] = i; acc = acc + i",
        max_ops: 1_000_000,
    },
    Scenario {
        group: "pure_lua",
        name: "table_new",
        setup: "",
        body: "local t = {x = i, y = i + 1}; acc = acc + t.x",
        max_ops: 50_000,
    },
    Scenario {
        group: "pure_lua",
        name: "method_call",
        setup: "local obj = {v = 0}\nfunction obj:add(x) self.v = self.v + x; return self.v end",
        body: "acc = obj:add(1)",
        max_ops: 1_000_000,
    },
    Scenario {
        group: "pure_lua",
        name: "string_concat",
        setup: "",
        body: r#"local s = "v" .. i; acc = acc + #s"#,
        max_ops: 100_000,
    },
    // ---------------------------------------------------------------- Lua → Rust
    Scenario {
        group: "rust_api",
        name: "noop",
        setup: API_SETUP,
        body: "acc = acc + noop()",
        max_ops: 1_000_000,
    },
    Scenario {
        group: "rust_api",
        name: "add_numbers",
        setup: API_SETUP,
        body: "acc = add(acc, 1.0)",
        max_ops: 1_000_000,
    },
    Scenario {
        group: "rust_api",
        name: "return_table",
        setup: API_SETUP,
        body: "local t = tbl(); acc = acc + t.x",
        max_ops: 50_000,
    },
    Scenario {
        group: "rust_api",
        name: "return_table6_capacity",
        setup: API_SETUP,
        body: "local t = tbl6(); acc = acc + t.x",
        max_ops: 50_000,
    },
    Scenario {
        group: "rust_api",
        name: "return_table6_default",
        setup: API_SETUP,
        body: "local t = tbl6_default(); acc = acc + t.x",
        max_ops: 50_000,
    },
    Scenario {
        group: "rust_api",
        name: "time_delta",
        setup: API_SETUP,
        body: "acc = acc + delta()",
        max_ops: 1_000_000,
    },
    Scenario {
        group: "rust_api",
        name: "entity_is_alive",
        setup: API_SETUP,
        body: "if alive(id) then acc = acc + 1 end",
        max_ops: 1_000_000,
    },
    Scenario {
        group: "rust_api",
        name: "entity_find",
        setup: API_SETUP,
        body: "local e = find(name); if e then acc = acc + 1 end",
        max_ops: 300_000,
    },
    Scenario {
        group: "rust_api",
        name: "entity_find_global",
        setup: API_SETUP,
        body: "local e = entity.find(name); if e then acc = acc + 1 end",
        max_ops: 300_000,
    },
    Scenario {
        group: "rust_api",
        name: "transform_get",
        setup: API_SETUP,
        body: "local t = get(id); acc = acc + t.x",
        max_ops: 50_000,
    },
    Scenario {
        group: "rust_api",
        name: "transform_get_global",
        setup: API_SETUP,
        body: "local t = transform.get(id); acc = acc + t.x",
        max_ops: 50_000,
    },
    Scenario {
        group: "rust_api",
        name: "transform_position",
        setup: API_SETUP,
        body: "local x, y = pos(id); acc = acc + x + y",
        max_ops: 300_000,
    },
    Scenario {
        group: "rust_api",
        name: "transform_world_position",
        setup: API_SETUP,
        body: "local x, y = wpos(id); acc = acc + x + y",
        max_ops: 300_000,
    },
    Scenario {
        group: "rust_api",
        name: "transform3d_position",
        setup: API_SETUP,
        body: "local x, y, z = pos3(deep); acc = acc + x + y + z",
        max_ops: 300_000,
    },
    Scenario {
        group: "rust_api",
        name: "transform3d_world_position",
        setup: API_SETUP,
        body: "local x, y, z = wpos3(deep); acc = acc + x + y + z",
        max_ops: 300_000,
    },
    Scenario {
        group: "rust_api",
        name: "transform_set",
        setup: API_SETUP,
        body: "set(id, acc, 0); acc = acc + 1",
        max_ops: 300_000,
    },
    Scenario {
        group: "rust_api",
        name: "transform_move",
        setup: API_SETUP,
        body: "move(id, 0.001, 0); acc = acc + 1",
        max_ops: 300_000,
    },
    Scenario {
        group: "rust_api",
        name: "transform_space",
        setup: API_SETUP,
        body: r#"if space(id) == "2d" then acc = acc + 1 end"#,
        max_ops: 300_000,
    },
    Scenario {
        group: "rust_api",
        name: "gameplay_mix",
        setup: API_SETUP,
        body: "local t = get(id); set(id, t.x + 0.001, t.y); acc = acc + t.x",
        max_ops: 50_000,
    },
    Scenario {
        group: "round_trip",
        name: "event_emit",
        setup: API_SETUP,
        body: r#"emit("bench", 1); acc = acc + 1"#,
        max_ops: 300_000,
    },
];

struct Stats {
    ops: usize,
    cycles: usize,
    per_op_ns: f64,
    p50_op_ns: f64,
    p95_op_ns: f64,
    p99_op_ns: f64,
    min_op_ns: f64,
    max_op_ns: f64,
    lua_bytes_per_op: f64,
    rust_alloc_count_per_op: f64,
    rust_alloc_bytes_per_op: f64,
    samples_ns: Vec<u64>,
}

struct ScenarioResult {
    group: &'static str,
    name: &'static str,
    stats: Stats,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_config();
    let runtime = build_fixture()?;
    let lua = runtime.lua();
    let mut results = Vec::new();

    for scenario in SCENARIOS {
        // 基准在**一次入口**里会跑几十亿条指令，必须按 scenario 重置预算周期。
        runtime.reset_instruction_budget();
        let probe_source = source_for(2_000, scenario.setup, scenario.body);
        let probe: Function = lua
            .load(&probe_source)
            .set_name(scenario.name)
            .into_function()?;
        let ops = calibrate_chunk(&probe, 2_000, scenario.max_ops)?;
        drop(probe);

        let source = source_for(ops, scenario.setup, scenario.body);
        let function: Function = lua.load(&source).set_name(scenario.name).into_function()?;
        let stats = measure_with(lua, ops, 1, config.cycles, true, || {
            function.call::<f64>(())?;
            Ok(())
        })?;
        results.push(ScenarioResult {
            group: scenario.group,
            name: scenario.name,
            stats,
        });
    }

    // Rust → Lua：空回调与每帧 `update` 各一条，后者包含全局查找、预算/守卫与错误快照桥。
    {
        runtime.reset_instruction_budget();
        let callback: Function = lua.globals().get("bench_tick")?;
        let ops = calibrate_calls(
            || {
                callback.call::<f64>(())?;
                Ok(())
            },
            500_000,
        )?;
        let stats = measure_with(lua, ops, ops, config.cycles, false, || {
            callback.call::<f64>(())?;
            Ok(())
        })?;
        results.push(ScenarioResult {
            group: "rust_to_lua",
            name: "bench_tick",
            stats,
        });
    }
    {
        let ops = calibrate_calls(
            || {
                runtime.call_update()?;
                Ok(())
            },
            100_000,
        )?;
        let stats = measure_with(lua, ops, ops, config.cycles, false, || {
            runtime.call_update()?;
            Ok(())
        })?;
        results.push(ScenarioResult {
            group: "rust_to_lua",
            name: "update_callback",
            stats,
        });
    }

    let report = build_report(&config, &results);
    if let Some(path) = &config.output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("report: {}", path.display());
    }

    print_table(&results);
    let n = |name: &str| {
        results
            .iter()
            .find(|result| result.name == name)
            .map_or(0.0, |result| result.stats.per_op_ns)
    };
    println!(
        "lua_perf_ok report_version={REPORT_VERSION} scenarios={} cycles={} \
         loop_add_ns={:.3} lua_call_ns={:.3} rust_noop_ns={:.3} \
         transform_get_ns={:.3} transform_position_ns={:.3} \
         transform_set_ns={:.3} entity_find_ns={:.3} \
         update_callback_ns={:.3}",
        results.len(),
        config.cycles,
        n("loop_add"),
        n("function_call"),
        n("noop"),
        n("transform_get"),
        n("transform_position"),
        n("transform_set"),
        n("entity_find"),
        n("update_callback"),
    );
    Ok(())
}

fn build_fixture() -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())?;
    runtime.set_delta(1.0 / 60.0);

    // 一个带 Transform 的 2D 实体；名字进 `EntityState.names`，供 `entity.find` 使用。
    let id: i64 = runtime
        .lua()
        .load(r#"return entity.spawn("bench")"#)
        .eval()?;

    let lua = runtime.lua();
    lua.globals().set("__bench_id", id)?;
    lua.globals().set("__bench_name", "bench")?;

    // 标量/批量位置 API 的夹具：一个 3D 实体 + 两批各 16 个实体。
    // 位置写成不同的值，避免优化器把重复读取折叠掉（Lua 没有这种优化，但读数更可信）。
    let deep: i64 = lua
        .load(
            r#"
            local deep = entity.spawn("bench3d")
            transform3d.attach(deep)
            transform3d.set(deep, 1, 2, 3)
            return deep
            "#,
        )
        .eval()?;
    lua.globals().set("__bench_deep", deep)?;

    let bench = lua.create_table()?;
    bench.set("noop", lua.create_function(|_, ()| Ok(0.0_f64))?)?;
    bench.set(
        "add",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(a + b))?,
    )?;
    bench.set(
        "table",
        lua.create_function(|lua, ()| {
            let table = lua.create_table_with_capacity(0, 2)?;
            table.set("x", 1.0_f64)?;
            table.set("y", 2.0_f64)?;
            Ok(table)
        })?,
    )?;
    // 6 个字符串键的返回值，用来隔离 `transform.get` 里"建表 + 填字段"的成本：
    // 一组预分配哈希槽，一组用默认 `create_table`，差值就是 `create_table_with_capacity` 的收益。
    bench.set("table6", lua.create_function(|lua, ()| table6(lua, true))?)?;
    bench.set(
        "table6_default",
        lua.create_function(|lua, ()| table6(lua, false))?,
    )?;
    lua.globals().set("bench", bench)?;

    // 事件总线回环：Lua → Rust `event.emit` → 再调回这个 Lua 监听器。
    lua.load(
        r#"
        bench_events = 0
        event.on("bench", function(v) bench_events = bench_events + v end)
        "#,
    )
    .exec()?;

    // Rust → Lua 的两条入口：裸函数与每帧 `update`（后者走 call_update 的完整守卫）。
    lua.load(
        r#"
        function bench_tick() return 1 end
        function update() return "" end
        "#,
    )
    .exec()?;
    lua.gc_collect()?;
    Ok(runtime)
}

fn table6(lua: &Lua, with_capacity: bool) -> mlua::Result<mlua::Table> {
    let table = if with_capacity {
        lua.create_table_with_capacity(0, 6)?
    } else {
        lua.create_table()?
    };
    table.set("space", "2d")?;
    table.set("x", 1.0_f64)?;
    table.set("y", 2.0_f64)?;
    table.set("rotation", 0.0_f64)?;
    table.set("scale_x", 1.0_f64)?;
    table.set("scale_y", 1.0_f64)?;
    Ok(table)
}

fn source_for(ops: usize, setup: &str, body: &str) -> String {
    format!("local N = {ops}\n{setup}\nlocal acc = 0\nfor i = 1, N do\n{body}\nend\nreturn acc\n")
}

/// 标定一个"一次调用跑 `probe_ops` 条 Lua 语句"的 chunk：返回一个样本应包含的语句数。
fn calibrate_chunk(
    function: &Function,
    probe_ops: usize,
    max_ops: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    let started = Instant::now();
    function.call::<f64>(())?;
    let per_op = started.elapsed().as_nanos() as f64 / probe_ops.max(1) as f64;
    Ok(target_ops(per_op, max_ops))
}

/// 标定一次 Rust→Lua 调用：返回一个样本里应重复的调用次数。
fn calibrate_calls<F>(mut call: F, max_ops: usize) -> Result<usize, Box<dyn std::error::Error>>
where
    F: FnMut() -> Result<(), Box<dyn std::error::Error>>,
{
    let probe = max_ops.clamp(1, 2_000);
    let started = Instant::now();
    for _ in 0..probe {
        call()?;
    }
    let per_op = started.elapsed().as_nanos() as f64 / probe as f64;
    Ok(target_ops(per_op, max_ops))
}

fn target_ops(per_op_ns: f64, max_ops: usize) -> usize {
    let target = (TARGET_SAMPLE_NS / per_op_ns.max(0.1)) as usize;
    target.clamp(MIN_OPS.min(max_ops), max_ops)
}

fn measure_with<F>(
    lua: &Lua,
    lua_ops_per_sample: usize,
    repeats: usize,
    cycles: usize,
    stop_gc: bool,
    mut call: F,
) -> Result<Stats, Box<dyn std::error::Error>>
where
    F: FnMut() -> Result<(), Box<dyn std::error::Error>>,
{
    for _ in 0..WARMUP_ROUNDS {
        for _ in 0..repeats {
            call()?;
        }
    }
    lua.gc_collect()?;
    if stop_gc {
        lua.gc_stop();
    }
    let before = lua.used_memory();
    wp0_alloc::reset_window();

    let mut samples_ns = Vec::with_capacity(cycles);
    for _ in 0..cycles {
        let started = Instant::now();
        for _ in 0..repeats {
            call()?;
        }
        samples_ns.push(started.elapsed().as_nanos() as u64);
    }

    let after = lua.used_memory();
    let rust_alloc_count = wp0_alloc::window_count();
    let rust_alloc_bytes = wp0_alloc::window_bytes();
    if stop_gc {
        lua.gc_restart();
        lua.gc_collect()?;
    }

    Ok(Stats::new(
        lua_ops_per_sample,
        cycles,
        samples_ns,
        after.saturating_sub(before) as u64,
        rust_alloc_count,
        rust_alloc_bytes,
    ))
}

impl Stats {
    fn new(
        ops: usize,
        cycles: usize,
        samples_ns: Vec<u64>,
        lua_bytes: u64,
        rust_alloc_count: u64,
        rust_alloc_bytes: u64,
    ) -> Self {
        let total_ops = (ops.saturating_mul(cycles)).max(1) as f64;
        let mut per_op: Vec<f64> = samples_ns
            .iter()
            .map(|elapsed| *elapsed as f64 / ops as f64)
            .collect();
        per_op.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let total_ns: u64 = samples_ns.iter().sum();
        Self {
            ops,
            cycles,
            per_op_ns: total_ns as f64 / total_ops,
            p50_op_ns: percentile(&per_op, 0.50),
            p95_op_ns: percentile(&per_op, 0.95),
            p99_op_ns: percentile(&per_op, 0.99),
            min_op_ns: per_op.first().copied().unwrap_or(0.0),
            max_op_ns: per_op.last().copied().unwrap_or(0.0),
            lua_bytes_per_op: lua_bytes as f64 / total_ops,
            rust_alloc_count_per_op: rust_alloc_count as f64 / total_ops,
            rust_alloc_bytes_per_op: rust_alloc_bytes as f64 / total_ops,
            samples_ns,
        }
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn build_report(config: &Config, results: &[ScenarioResult]) -> serde_json::Value {
    let scenarios: Vec<serde_json::Value> = results
        .iter()
        .map(|result| {
            serde_json::json!({
                "group": result.group,
                "name": result.name,
                "lua_ops_per_sample": result.stats.ops,
                "cycles": result.stats.cycles,
                "per_op_ns": round(result.stats.per_op_ns, 3),
                "p50_op_ns": round(result.stats.p50_op_ns, 3),
                "p95_op_ns": round(result.stats.p95_op_ns, 3),
                "p99_op_ns": round(result.stats.p99_op_ns, 3),
                "min_op_ns": round(result.stats.min_op_ns, 3),
                "max_op_ns": round(result.stats.max_op_ns, 3),
                "lua_bytes_per_op": round(result.stats.lua_bytes_per_op, 3),
                "rust_alloc_count_per_op": round(result.stats.rust_alloc_count_per_op, 6),
                "rust_alloc_bytes_per_op": round(result.stats.rust_alloc_bytes_per_op, 6),
                "samples_ns": result.stats.samples_ns,
            })
        })
        .collect();

    let ns = |name: &str| {
        results
            .iter()
            .find(|result| result.name == name)
            .map_or(0.0, |result| result.stats.per_op_ns)
    };
    let loop_add = ns("loop_add");
    let lua_call = ns("function_call");
    let noop = ns("noop");
    let table_return = ns("return_table");
    let transform_get = ns("transform_get");
    let transform_set = ns("transform_set");
    let entity_find = ns("entity_find");
    let update_callback = ns("update_callback");

    serde_json::json!({
        "report_version": REPORT_VERSION,
        "generated_at_unix": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
        "config": {
            "cycles": config.cycles,
            "warmup_rounds": WARMUP_ROUNDS,
            "target_sample_ms": round(TARGET_SAMPLE_NS / 1_000_000.0, 3),
        },
        "environment": {
            "os": env::consts::OS,
            "arch": env::consts::ARCH,
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "lua": "PUC-Rio Lua 5.4.8 (vendored, no JIT)",
            "engine": env!("CARGO_PKG_VERSION"),
        },
        "fixture": {
            "entities": 2,
            "entity": "bench (2d) + bench3d (3d)",
        },
        "baseline": {
            "loop_add_ns": round(loop_add, 3),
            "lua_function_call_ns": round(lua_call, 3),
            "rust_noop_call_ns": round(noop, 3),
            "rust_to_lua_bench_tick_ns": round(ns("bench_tick"), 3),
            "update_callback_ns": round(update_callback, 3),
        },
        "derived": {
            // 负值说明该调用不比空循环/空 Rust 调用贵，读数属噪声内。
            "cross_language_call_ns": round(noop - lua_call, 3),
            "return_table_overhead_ns": round(table_return - noop, 3),
            "transform_get_overhead_ns": round(transform_get - noop, 3),
            "transform_position_overhead_ns": round(ns("transform_position") - noop, 3),
            "transform_world_position_overhead_ns": round(
                ns("transform_world_position") - noop,
                3,
            ),
            "transform_set_overhead_ns": round(transform_set - noop, 3),
            "entity_find_overhead_ns": round(entity_find - noop, 3),
            "update_callback_overhead_ns": round(update_callback - ns("bench_tick"), 3),
        },
        "scenarios": scenarios,
    })
}

fn print_table(results: &[ScenarioResult]) {
    println!(
        "{:<10} {:<24} {:>10} {:>10} {:>10} {:>12} {:>12}",
        "group", "scenario", "mean", "p50", "p95", "lua B/op", "rust alloc/op"
    );
    for result in results {
        println!(
            "{:<10} {:<24} {:>10.2} {:>10.2} {:>10.2} {:>12.2} {:>12.4}",
            result.group,
            result.name,
            result.stats.per_op_ns,
            result.stats.p50_op_ns,
            result.stats.p95_op_ns,
            result.stats.lua_bytes_per_op,
            result.stats.rust_alloc_count_per_op,
        );
    }
    println!();
}

fn parse_config() -> Config {
    let mut cycles = DEFAULT_CYCLES;
    let mut output = None;
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--cycles" => {
                cycles = args
                    .next()
                    .expect("--cycles requires a value")
                    .parse()
                    .expect("--cycles must be an integer");
            }
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().expect("--output requires a path"),
                ))
            }
            "--help" | "-h" => {
                println!("Usage: lua_perf [--cycles N] [--output PATH]");
                std::process::exit(0);
            }
            other => panic!("unknown argument: {other}"),
        }
    }
    Config {
        cycles: cycles.max(1),
        output,
    }
}

fn round(value: f64, digits: u32) -> f64 {
    let factor = 10_f64.powi(digits as i32);
    (value * factor).round() / factor
}

impl fmt::Display for ScenarioResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}/{} {:.2} ns",
            self.group, self.name, self.stats.per_op_ns
        )
    }
}
