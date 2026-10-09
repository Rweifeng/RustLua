//! Web 平台边界成本的**原生**基线（M73-00 WP0）。
//!
//! 为什么需要它：方案把"消除每帧源码/字符串协议、改用批量二进制 ABI"列为结构性决策，
//! 但此前**没有任何读数**说明那条协议到底值多少钱——而没有基线就谈不上"优化了"，
//! 只能凭感觉改架构。这个例子在原生 Lua 状态上量出三件事的单价（每次调用的微秒数）：
//!
//!   ① 逐帧写 `__rustlua_dt`：`lua_do_string("__rustlua_dt = …")`（编译一小段源码）
//!      对比 `globals().set(...)`（本次重构已改为后者，见 `lua_bridge.rs::lua_set_number`）；
//!   ② 逐帧注入输入快照：`web-runtime.js` 每帧把整个输入表**拼成 Lua 源码字符串**再执行，
//!      对比直接建表写值（批量 ABI 应当走的方向）；
//!   ③ 空 chunk 的编译底价：说明①②的差值里有多少是"编译"本身。
//!
//! 口径与限制（不得夸大）：
//!   * 这是 **MSVC 原生**读数，不是微信 iOS/Android 真机读数；wasm 下编译/执行的相对关系
//!     会变（JIT 缺失、内存布局不同），所以这里只用来判断**量级与方向**，不用来宣称倍数。
//!   * 计时用 `std::time::Instant`，取多轮中的**中位数**（单次最小值会被缓存效应误导）。
//!   * 输入快照的源码文本按 `web-runtime.js` 的 `luaValue()` 规则生成（键用 `["k"]=` 形式），
//!     规模取"标题画面一次真实采样"的形态：少量按键 + 鼠标 + 指针 + 滚轮。
//!
//! 用法：`cargo run --release -p rustlua-scripting --example lua_boundary_cost`

use std::time::Instant;

use mlua::Lua;
use rustlua_scripting::script::LuaRuntime;

/// 单轮迭代数：够大到让单次耗时远高于计时器噪声，又小到整轮能跑完。
const ITERATIONS: u32 = 20_000;
/// 重复轮数：取中位数以避开首次触碰缓存、后台调度等一次性效应。
const ROUNDS: usize = 7;
/// 分配压力测量的迭代数：比计时少，但足够让峰值垃圾堆起来。
const CHURN_ITERATIONS: u32 = 4_000;

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    samples[samples.len() / 2]
}

/// 测一段闭包每次调用的微秒数（多轮取中位数）。
fn measure<F: FnMut()>(mut body: F) -> f64 {
    // 预热：让 Lua 的字符串驻留、表扩容、CPU 缓存都进入稳态，否则第一轮永远偏高。
    for _ in 0..(ITERATIONS / 4) {
        body();
    }
    let mut rounds = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let started = Instant::now();
        for _ in 0..ITERATIONS {
            body();
        }
        rounds.push(started.elapsed().as_secs_f64() * 1e6 / f64::from(ITERATIONS));
    }
    median(rounds)
}

/// 一次操作的 Lua 堆分配**总量**（每调用字节数）。
///
/// 为什么**必须**单独量这个：微秒读数看不见 GC。逐帧编译一段源码会在 Lua 堆上留下一个
/// 函数原型（Proto + 常量表 + 名字），每帧一份、全部是垃圾；而"帧时间稳定性（P95/P99）"
/// 与"长跑内存稳定性"在方案里排在下载体积之前。只拿 µs 读数去论证批量 ABI，理由是不成立的。
///
/// 测法要点：**先停掉增量 GC**。不停的话回收与分配同时进行，`used_memory()` 只能看到一条
/// 锯齿的振幅（第一版就是这么写错的：编译源码那种最该昂贵的操作只量出 20 B/次）。
/// 停掉之后堆里累积的就是这段时间分配的全部垃圾，除以迭代数才是每调用的真实分配量。
struct Churn {
    /// 每调用分配的字节数（GC 停用期间堆的全部增长 / 迭代数）。
    allocated_per_call: f64,
    /// 恢复 GC 并回收之后仍留在堆上的量（真正的常驻增长，应接近 0）。
    retained_bytes: usize,
}

fn churn<F: FnMut()>(lua: &Lua, mut body: F) -> Churn {
    lua.gc_collect().expect("gc collect");
    let base = lua.used_memory();
    lua.gc_stop();
    for _ in 0..CHURN_ITERATIONS {
        body();
    }
    let allocated = lua.used_memory().saturating_sub(base);
    lua.gc_restart();
    lua.gc_collect().expect("gc collect");
    let retained = lua.used_memory().saturating_sub(base);
    Churn {
        allocated_per_call: allocated as f64 / f64::from(CHURN_ITERATIONS),
        retained_bytes: retained,
    }
}

/// 与 `web-runtime.js` 的 `luaValue()` + `webInput.snapshot()` 同形的输入快照源码。
///
/// 保持同形很重要：如果这里给一个"最小表"，量出来的成本会低估真实每帧开销，
/// 而低估的基线会让人以为这条协议不贵、不值得改。
fn input_snapshot_source() -> String {
    let mut out = String::from("__rustlua_input = {");
    out.push_str("[\"axis_x\"]=1,[\"axis_y\"]=-1,");
    out.push_str("[\"keys\"]={[\"w\"]=true,[\"d\"]=true,[\"space\"]=true},");
    out.push_str("[\"just\"]={[\"space\"]=true},");
    out.push_str("[\"released\"]={},");
    out.push_str("[\"mouse\"]={[\"left\"]=false,[\"right\"]=false},");
    out.push_str("[\"mouse_just\"]={[\"left\"]=true},");
    out.push_str("[\"mouse_just_released\"]={},");
    out.push_str("[\"cursor\"]={627,318},");
    out.push_str("[\"press_cursor\"]={627,318},");
    out.push_str("[\"cursor_delta\"]={3,-2},");
    out.push_str("[\"scroll\"]={0,0}");
    out.push('}');
    out
}

/// 常驻输入表的"目标形态"：一张表 + 常驻嵌套表，每帧只改值。
///
/// 用 `FnMut` 而不是每帧重建，是因为**重建本身就是要消除的开销**：②里"直接建表"仍要
/// 为 6 张嵌套表分配，实测 544 B/帧；这里全部复用，稳态应当是 0 B/帧。
struct LiveInput {
    root: mlua::Table,
    keys: mlua::Table,
    just: mlua::Table,
    released: mlua::Table,
    mouse: mlua::Table,
    mouse_just: mlua::Table,
    mouse_just_released: mlua::Table,
    cursor: mlua::Table,
    press_cursor: mlua::Table,
    cursor_delta: mlua::Table,
    scroll: mlua::Table,
}

impl LiveInput {
    /// 每帧刷新：内容与②的源码变体逐字段等价。
    fn flush(&self) {
        self.root.set("axis_x", 1).expect("axis_x");
        self.root.set("axis_y", -1).expect("axis_y");
        self.keys.set("w", true).expect("w");
        self.keys.set("d", true).expect("d");
        self.keys.set("space", false).expect("space");
        self.just.set("space", true).expect("just space");
        self.mouse.set("left", false).expect("mouse left");
        self.mouse.set("right", false).expect("mouse right");
        self.mouse_just.set("left", true).expect("mouse_just left");
        self.cursor.set(1, 627).expect("cursor x");
        self.cursor.set(2, 318).expect("cursor y");
        self.press_cursor.set(1, 627).expect("press x");
        self.press_cursor.set(2, 318).expect("press y");
        self.cursor_delta.set(1, 3).expect("delta x");
        self.cursor_delta.set(2, -2).expect("delta y");
        self.scroll.set(1, 0).expect("scroll x");
        self.scroll.set(2, 0).expect("scroll y");
        let _ = &self.released;
        let _ = &self.mouse_just_released;
    }
}

fn seed_live_input(lua: &Lua) -> LiveInput {
    let table = |name: &str| {
        lua.create_table()
            .unwrap_or_else(|_| panic!("table {name}"))
    };
    let live = LiveInput {
        root: table("root"),
        keys: table("keys"),
        just: table("just"),
        released: table("released"),
        mouse: table("mouse"),
        mouse_just: table("mouse_just"),
        mouse_just_released: table("mouse_just_released"),
        cursor: table("cursor"),
        press_cursor: table("press_cursor"),
        cursor_delta: table("cursor_delta"),
        scroll: table("scroll"),
    };
    // 键集固定：预先把用到的键都建出来，之后每帧只改布尔值，表不再扩容。
    for name in [
        "w",
        "a",
        "s",
        "d",
        "space",
        "arrowup",
        "arrowdown",
        "arrowleft",
        "arrowright",
    ] {
        live.keys.set(name, false).expect("seed key");
    }
    for name in ["left", "right", "middle"] {
        live.mouse.set(name, false).expect("seed mouse");
    }
    live.root.set("keys", live.keys.clone()).expect("keys");
    live.root.set("just", live.just.clone()).expect("just");
    live.root
        .set("released", live.released.clone())
        .expect("released");
    live.root.set("mouse", live.mouse.clone()).expect("mouse");
    live.root
        .set("mouse_just", live.mouse_just.clone())
        .expect("mouse_just");
    live.root
        .set("mouse_just_released", live.mouse_just_released.clone())
        .expect("mjr");
    live.root
        .set("cursor", live.cursor.clone())
        .expect("cursor");
    live.root
        .set("press_cursor", live.press_cursor.clone())
        .expect("press_cursor");
    live.root
        .set("cursor_delta", live.cursor_delta.clone())
        .expect("cursor_delta");
    live.root
        .set("scroll", live.scroll.clone())
        .expect("scroll");
    lua.globals()
        .set("__rustlua_input", live.root.clone())
        .expect("live input");
    live
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = LuaRuntime::new()?;
    let lua: &Lua = runtime.lua();

    let source = input_snapshot_source();
    println!("M73-00 原生 Lua 边界成本基线（MSVC 原生，非真机；单位 µs/次）");
    println!("输入快照源码长度 = {} 字节", source.len());
    println!("每轮 {} 次 × {} 轮取中位数\n", ITERATIONS, ROUNDS);

    // ① 逐帧写 __rustlua_dt：编译一小段源码 vs 直接写全局。
    let dt_literal = "__rustlua_dt = 0.016666666666666666";
    let dt_by_source = measure(|| {
        lua.load(dt_literal)
            .set_name("__rustlua_dt")
            .exec()
            .expect("dt source");
    });
    let dt_by_set = measure(|| {
        lua.globals()
            .set("__rustlua_dt", 1.0 / 60.0)
            .expect("dt set");
    });

    // ② 逐帧注入输入快照：编译整表源码 vs 直接建表 vs 原地复用常驻表。
    //
    // **三个变体必须建出同一份内容**，否则比较就是自欺：源码变体设了 12 个字段
    // （含 mouse/press_cursor/cursor_delta/scroll），建表变体要是只设 5 个，
    // 建表当然"更便宜"——那不是协议便宜，是我少干活了。
    let build_fresh = |lua: &Lua| {
        let table = lua.create_table().expect("table");
        table.set("axis_x", 1).expect("axis_x");
        table.set("axis_y", -1).expect("axis_y");
        let keys = lua.create_table().expect("keys");
        keys.set("w", true).expect("w");
        keys.set("d", true).expect("d");
        keys.set("space", true).expect("space");
        table.set("keys", keys).expect("keys set");
        let just = lua.create_table().expect("just");
        just.set("space", true).expect("just space");
        table.set("just", just).expect("just set");
        table
            .set("released", lua.create_table().expect("released"))
            .expect("released set");
        let mouse = lua.create_table().expect("mouse");
        mouse.set("left", false).expect("mouse left");
        mouse.set("right", false).expect("mouse right");
        table.set("mouse", mouse).expect("mouse set");
        let mouse_just = lua.create_table().expect("mouse_just");
        mouse_just.set("left", true).expect("mouse_just left");
        table.set("mouse_just", mouse_just).expect("mouse_just set");
        table
            .set("mouse_just_released", lua.create_table().expect("mjr"))
            .expect("mjr set");
        let cursor = lua.create_table().expect("cursor");
        cursor.set(1, 627).expect("cursor x");
        cursor.set(2, 318).expect("cursor y");
        table.set("cursor", cursor).expect("cursor set");
        let press = lua.create_table().expect("press_cursor");
        press.set(1, 627).expect("press x");
        press.set(2, 318).expect("press y");
        table.set("press_cursor", press).expect("press set");
        let delta = lua.create_table().expect("cursor_delta");
        delta.set(1, 3).expect("delta x");
        delta.set(2, -2).expect("delta y");
        table.set("cursor_delta", delta).expect("delta set");
        let scroll = lua.create_table().expect("scroll");
        scroll.set(1, 0).expect("scroll x");
        scroll.set(2, 0).expect("scroll y");
        table.set("scroll", scroll).expect("scroll set");
        lua.globals()
            .set("__rustlua_input", table)
            .expect("input set");
    };
    let input_by_source = measure(|| {
        lua.load(source.as_str())
            .set_name("__rustlua_input")
            .exec()
            .expect("input source");
    });
    let input_by_table = measure(|| build_fresh(lua));

    // ③ 空 chunk 的编译底价：用来判断①②的差值有多少来自"编译"这一步本身。
    let empty_by_source = measure(|| {
        lua.load("local _ = 1")
            .set_name("__rustlua_noop")
            .exec()
            .expect("noop");
    });

    println!("{:<42}{:>12}", "操作", "µs/次");
    println!("{}", "-".repeat(54));
    println!(
        "{:<42}{:>12.3}",
        "① dt：编译源码 (lua_do_string)", dt_by_source
    );
    println!(
        "{:<42}{:>12.3}",
        "① dt：直接写全局 (lua_set_number)", dt_by_set
    );
    println!(
        "{:<42}{:>12.3}",
        "② 输入：编译源码 (现行每帧协议)", input_by_source
    );
    println!(
        "{:<42}{:>12.3}",
        "② 输入：直接建表 (批量 ABI 方向)", input_by_table
    );
    println!(
        "{:<42}{:>12.3}",
        "③ 底价：空 chunk 编译+执行", empty_by_source
    );
    println!();
    println!(
        "dt 改写：{:.2}× 更快（省 {:.3} µs/步；60 步/秒 ⇒ 约 {:.2} ms/秒）",
        dt_by_source / dt_by_set,
        dt_by_source - dt_by_set,
        (dt_by_source - dt_by_set) * 60.0 / 1000.0
    );
    println!(
        "输入注入：{:.2}× 更快（省 {:.3} µs/帧；60 帧/秒 ⇒ 约 {:.2} ms/秒）",
        input_by_source / input_by_table,
        input_by_source - input_by_table,
        (input_by_source - input_by_table) * 60.0 / 1000.0
    );

    // ④ 分配压力：微秒读数看不见的那一半，也是"帧时间稳定性"真正的风险来源。
    let dt_churn_source = churn(lua, || {
        lua.load(dt_literal)
            .set_name("__rustlua_dt")
            .exec()
            .expect("dt source");
    });
    let dt_churn_set = churn(lua, || {
        lua.globals()
            .set("__rustlua_dt", 1.0 / 60.0)
            .expect("dt set");
    });
    let input_churn_source = churn(lua, || {
        lua.load(source.as_str())
            .set_name("__rustlua_input")
            .exec()
            .expect("input source");
    });
    let input_churn_table = churn(lua, || build_fresh(lua));

    // ⑤ 原地复用：一张常驻表 + 常驻嵌套表，每帧只改值（与②的源码变体同一份内容）。
    //
    // 这是④给出方向之后的**目标形态**：既不编译源码，也不每帧新建表。固定键集意味着
    // 嵌套表只在第一次调用时扩容，之后每帧零分配。真机上 JS 侧只发"变化了的那些键"，
    // 而不是像现在这样把整表拼成 326 字节源码。
    let live = seed_live_input(lua);
    let input_by_reuse = measure(|| live.flush());
    let input_churn_reuse = churn(lua, || live.flush());

    println!();
    println!(
        "④ Lua 堆分配量（停用增量 GC 后测 {} 次连续调用）",
        CHURN_ITERATIONS
    );
    println!("{:<42}{:>16}{:>14}", "操作", "每调用分配", "GC 后残留");
    println!("{}", "-".repeat(72));
    println!(
        "{:<42}{:>13.1} B{:>11} B",
        "dt：编译源码 (lua_do_string)",
        dt_churn_source.allocated_per_call,
        dt_churn_source.retained_bytes
    );
    println!(
        "{:<42}{:>13.1} B{:>11} B",
        "dt：直接写全局 (lua_set_number)",
        dt_churn_set.allocated_per_call,
        dt_churn_set.retained_bytes
    );
    println!(
        "{:<42}{:>13.1} B{:>11} B",
        "输入：编译源码 (现行每帧协议)",
        input_churn_source.allocated_per_call,
        input_churn_source.retained_bytes
    );
    println!(
        "{:<42}{:>13.1} B{:>11} B",
        "输入：直接建表 (每帧新建)",
        input_churn_table.allocated_per_call,
        input_churn_table.retained_bytes
    );
    println!(
        "{:<42}{:>13.1} B{:>11} B  ({:.3} µs)",
        "输入：原地复用常驻表 (目标形态)",
        input_churn_reuse.allocated_per_call,
        input_churn_reuse.retained_bytes,
        input_by_reuse
    );
    println!();
    // 这才是架构改良的**主要**理由：60 次/秒的分配量直接决定 GC 频率与停顿分布。
    let per_second_kb = |bytes: f64| bytes * 60.0 / 1024.0;
    let now_per_second =
        per_second_kb(dt_churn_source.allocated_per_call + input_churn_source.allocated_per_call);
    let after_per_second =
        per_second_kb(dt_churn_set.allocated_per_call + input_churn_table.allocated_per_call);
    let target_per_second =
        per_second_kb(dt_churn_set.allocated_per_call + input_churn_reuse.allocated_per_call);
    println!(
        "每秒 Lua 垃圾：现行协议 ≈ {:.0} KB/s，直接写全局+每帧建表 ≈ {:.0} KB/s，目标形态（原地复用）≈ {:.1} KB/s（{:.0}× 更少）",
        now_per_second,
        after_per_second,
        target_per_second,
        if target_per_second > 1.0 {
            now_per_second / target_per_second
        } else {
            now_per_second
        }
    );
    println!(
        "合计每秒省：约 {:.2} ms CPU、约 {:.0} KB 垃圾",
        (dt_by_source - dt_by_set) * 60.0 / 1000.0
            + (input_by_source - input_by_reuse) * 60.0 / 1000.0,
        now_per_second - target_per_second
    );
    println!();
    println!("注意：这是原生读数。wasm 下没有 JIT，编译开销占比通常更高，但必须实测才算数；");
    println!("本基线只用于判断量级与方向，不用于宣称倍数。");
    Ok(())
}
