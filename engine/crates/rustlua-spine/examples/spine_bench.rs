//! M84 P4-1：Spine 骨骼的 CPU 侧基准（1 / 10 / 100 个角色）。
//!
//! ## 这份基准量什么、不量什么
//!
//! **量**：从"推进一帧"到"产出可提交的 draw list"的 CPU 成本，以及每帧的
//! 顶点/索引/draw 数与**堆分配次数**。这是 Spine 接入真正引入的增量成本——
//! 骨骼采样 + 顶点变换 + attachment 遍历本来就是我们实现的。
//!
//! **不量** GPU 时间：那要看具体显卡与场景，且需要 timestamp query（`gpu_pass_timings`
//! 已覆盖渲染通道）。把 CPU 与 GPU 混在一张表里会让结论无法归因。
//!
//! ## 为什么同时报分配次数
//!
//! 帧内分配是 2D 游戏掉帧的常见真凶：单次几十纳秒看着无害，但每帧几百次就会在
//! GC/分配器上抖动，表现为"平均 60fps 却有周期性卡顿"。P4-2 要把它压到稳态零分配，
//! 这份基准先给出**改动前的实测底数**。
//!
//! 用法：
//! ```text
//! cargo run -p rustlua-spine --release --example spine_bench [-- --json <输出路径>]
//! ```

use std::time::Instant;

use rustlua_spine::{SpineData, SpineSkeletonComponent};

/// 参与基准的骨架：官方 spineboy（essence 版，含 region + boundingbox，单页图集）。
///
/// 刻意用**真实导出**而不是手写夹具：手写夹具很容易让 region 名与 atlas 页对不上，
/// 那样每帧都会走到"页表查不到 → 现场拼路径"的兜底分支，基准量到的就不是生产路径。
const SKELETON: &str = include_str!("../tests/fixtures/spineboy_bench.json");
const ATLAS: &str = include_str!("../tests/fixtures/spineboy_bench.atlas");

/// 预热的帧数：前几帧有页纹理首次解析等一次性成本，计入会污染 p50。
const WARMUP_FRAMES: usize = 30;
/// 正式采样的帧数。够多才能让 p95/p99 有意义（99 分位至少需要 100 个样本）。
const SAMPLE_FRAMES: usize = 300;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json_out = args
        .iter()
        .position(|arg| arg == "--json")
        .and_then(|index| args.get(index + 1))
        .cloned();

    let data = SpineData::from_bytes(SKELETON.as_bytes(), false, ATLAS, "4.3.75-beta", "")
        .expect("夹具骨架必须能加载");

    let mut rows = Vec::new();
    for characters in [1_usize, 10, 100] {
        let row = measure(&data, characters);
        println!(
            "spine_bench characters={} draws={} vertices={} p50_us={} p95_us={} p99_us={} \
             allocs_per_frame={} (upstream_state={} upstream_drawlist={} ours={}) bytes_per_frame={}",
            row.characters,
            row.draws_per_frame,
            row.vertices_per_frame,
            row.p50_us,
            row.p95_us,
            row.p99_us,
            row.allocs_per_frame,
            row.state_only_allocs_per_frame,
            row.draw_list_allocs_per_frame,
            row.allocs_per_frame
                .saturating_sub(row.state_only_allocs_per_frame + row.draw_list_allocs_per_frame),
            row.bytes_per_frame,
        );
        rows.push(row);
    }

    if let Some(path) = json_out {
        let report = render_json(&rows);
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, report.as_bytes()).expect("写报告失败");
        println!("report: {path}");
    }
}

struct Row {
    characters: usize,
    draws_per_frame: usize,
    vertices_per_frame: usize,
    triangles_per_frame: usize,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    allocs_per_frame: u64,
    bytes_per_frame: u64,
    /// 归因：其中多少来自 spine2d 状态机/采样（上游库，我们改不了）。
    state_only_allocs_per_frame: u64,
    /// 归因：其中多少来自 `append_draw_list_with_atlas`（上游库，我们改不了）。
    draw_list_allocs_per_frame: u64,
}

/// 对给定角色数跑一轮：预热 → 采样 → 取分位数与分配统计。
fn measure(data: &SpineData, characters: usize) -> Row {
    // 每轮都新建播放器：复用会把上一轮的稳态状态带进来，测不到"冷启动后的稳态"。
    let mut players: Vec<_> = (0..characters)
        .map(|_| {
            let component = SpineSkeletonComponent {
                data: "bench".to_owned(),
                ..SpineSkeletonComponent::default()
            };
            data.instantiate(&component).expect("实例化成功")
        })
        .collect();

    for _ in 0..WARMUP_FRAMES {
        for player in &mut players {
            player.advance(1.0 / 60.0).expect("推进成功");
            std::hint::black_box(player.output());
        }
    }

    let mut samples = Vec::with_capacity(SAMPLE_FRAMES);
    let mut draws = 0usize;
    let mut vertices = 0usize;
    let mut triangles = 0usize;

    // 诊断：先把"只推进动画、不产 draw list"的分配数单独量出来，
    // 用来把总分配拆成"状态机+采样"与"draw list 生成"两块。
    let probe_start = AllocSnapshot::now();
    for _ in 0..SAMPLE_FRAMES {
        for player in &mut players {
            player.advance_state_only(1.0 / 60.0).expect("推进成功");
        }
    }
    let probe_end = AllocSnapshot::now();
    let state_only_allocs =
        probe_end.allocs.saturating_sub(probe_start.allocs) / SAMPLE_FRAMES as u64;

    // 诊断 2：只跑 append_draw_list（不跑状态机），定位 draw 生成侧的开销。
    let dl_start = AllocSnapshot::now();
    for _ in 0..SAMPLE_FRAMES {
        for player in &mut players {
            player.advance_draw_list_only().expect("生成成功");
        }
    }
    let dl_end = AllocSnapshot::now();
    let draw_list_only_allocs =
        dl_end.allocs.saturating_sub(dl_start.allocs) / SAMPLE_FRAMES as u64;

    // 诊断 3：只做"从已生成的 list 复制到 out"这一段（不含上游两段）。
    // 先预热一遍让 `out` 的 Vec 达到稳态容量，否则会把首次扩容算进来。
    for player in &mut players {
        player.advance_copy_only();
    }
    let copy_start = AllocSnapshot::now();
    for _ in 0..SAMPLE_FRAMES {
        for player in &mut players {
            player.advance_copy_only();
        }
    }
    let copy_end = AllocSnapshot::now();
    let copy_only_allocs = copy_end.allocs.saturating_sub(copy_start.allocs) / SAMPLE_FRAMES as u64;
    let _ = copy_only_allocs;
    let alloc_start = AllocSnapshot::now();
    for _ in 0..SAMPLE_FRAMES {
        let started = Instant::now();
        for player in &mut players {
            player.advance(1.0 / 60.0).expect("推进成功");
            let output = player.output();
            std::hint::black_box(output);
        }
        samples.push(started.elapsed().as_nanos() as u64 / 1_000);
    }
    let alloc_end = AllocSnapshot::now();

    // 几何口径取最后一帧（稳态下每帧相同）。
    for player in &players {
        let output = player.output();
        draws += output.draws.len();
        vertices += output.vertices.len();
        triangles += output.indices.len() / 3;
    }

    samples.sort_unstable();
    let percentile = |percent: f64| -> u64 {
        let index = ((percent / 100.0) * (samples.len() - 1) as f64).round() as usize;
        samples[index.min(samples.len() - 1)]
    };
    let frames = SAMPLE_FRAMES as u64;
    Row {
        characters,
        draws_per_frame: draws,
        vertices_per_frame: vertices,
        triangles_per_frame: triangles,
        p50_us: percentile(50.0),
        p95_us: percentile(95.0),
        p99_us: percentile(99.0),
        allocs_per_frame: alloc_end.allocs.saturating_sub(alloc_start.allocs) / frames,
        bytes_per_frame: alloc_end.bytes.saturating_sub(alloc_start.bytes) / frames,
        state_only_allocs_per_frame: state_only_allocs,
        draw_list_allocs_per_frame: draw_list_only_allocs,
    }
}

/// 全局分配器计数快照。
///
/// 用 `std::alloc` 的全局计数而不是 `dhat`/`heaptrack`：那些要单独装工具、且报告格式
/// 不便于进 `artifacts/`。这里的目的是"每帧分配几次"这一条可比较的数字，不是分析
/// 每个分配的调用栈。
struct AllocSnapshot {
    allocs: u64,
    bytes: u64,
}

impl AllocSnapshot {
    fn now() -> Self {
        Self {
            allocs: ALLOC_COUNT.load(std::sync::atomic::Ordering::Relaxed),
            bytes: ALLOC_BYTES.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

static ALLOC_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static ALLOC_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 包一层 `System`：只统计，不改变分配行为。
///
/// 注意计数是**跨线程累加**的，而本基准是单线程跑采样，所以差值就是本线程的开销。
/// 如果以后加多线程，这个口径要改成线程局部。
struct CountingAllocator;

unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, std::sync::atomic::Ordering::Relaxed);
        // SAFETY: 转交给系统分配器，契约由调用方保证。
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: 指针由 `alloc` 产出且尚未释放。
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(
            new_size.saturating_sub(layout.size()) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        // SAFETY: 同 `alloc`。
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn render_json(rows: &[Row]) -> String {
    let mut out = String::from(
        "{\n  \"report_version\": \"m84-spine-bench-v1\",\n  \"skeleton\": \"spineboy-ess (single page, region+boundingbox)\",\n  \"warmup_frames\": ",
    );
    out.push_str(&WARMUP_FRAMES.to_string());
    out.push_str(",\n  \"sample_frames\": ");
    out.push_str(&SAMPLE_FRAMES.to_string());
    out.push_str(",\n  \"cases\": [\n");
    for (index, row) in rows.iter().enumerate() {
        out.push_str(&format!(
            "    {{\"characters\": {}, \"draws\": {}, \"vertices\": {}, \"triangles\": {}, \
             \"p50_us\": {}, \"p95_us\": {}, \"p99_us\": {}, \"allocs_per_frame\": {}, \"state_only_allocs\": {}, \"draw_list_allocs\": {}, \"bytes_per_frame\": {}}}",
            row.characters,
            row.draws_per_frame,
            row.vertices_per_frame,
            row.triangles_per_frame,
            row.p50_us,
            row.p95_us,
            row.p99_us,
            row.allocs_per_frame,
            row.state_only_allocs_per_frame,
            row.draw_list_allocs_per_frame,
            row.bytes_per_frame
        ));
        if index + 1 < rows.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ]\n}\n");
    out
}
