//! P3-E06-2 headless 证据：固定场景 120 帧，量 trace 开销并导出 Chrome Trace。
//!
//! 为什么要在 headless 里做 A/B：编辑器开窗后的帧时间受 GUI、系统调度与显示垂直同步
//! 影响，量不出"多记 25 个 span 到底贵多少"。这里用同一个 `RuntimeScene`、同一场景、
//! 同进程**交替**跑基线帧与带 trace 帧（不是先跑完 A 再跑 B——CPU 频率/缓存漂移会
//! 把差值算到 profiler 头上），p50 的差值就是细粒度 scope 的净开销。
//!
//! 判据（计划 §6.13 / docs/55）：主线程 p50 开销 ≤2%。Fine 超预算时再量一遍
//! **System 档**（只记帧/脚本/系统级），报告 `recommended_level=system`——这就是
//! "超过则默认关闭细粒度 scope"的降级路径，而不是把超预算的档位留在默认配置里。
//!
//! 运行：
//! ```powershell
//! cd engine
//! cargo run --release -p engine --example wp0_profile_capture -- --frames 120 `
//!   --output target\reports\p3\profile-capture
//! ```
//!
//! 产物：`trace.json`（Chrome Trace Event，可在 `chrome://tracing` / Perfetto 打开）、
//! `report.json`（span_count/overhead_pct/systems/passes 等读数）。

use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use engine::host::RuntimeScene;
use engine::host::profile_trace::{
    CaptureLevel, DEFAULT_SPAN_CAPACITY, SpanToken, SpanTrace, TRACE_OVERHEAD_BUDGET_PCT,
    TRACE_REPORT_VERSION, capture_level_for_overhead, overhead_pct, overhead_within_budget,
};
use rustlua_camera::camera::Viewport;
use rustlua_infra::games_dir;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::{Scene, SceneEntityId};

const REPORT_VERSION: &str = "p3-profile-capture-v1";
const VIEWPORT: (u32, u32) = (1280, 720);
const DEFAULT_FRAMES: usize = 120;
const WARMUP_FRAMES: usize = 5;
/// 5k 精灵让单帧稳定在毫秒量级：帧越短，固定 25 个 span 的相对开销越容易被噪声放大。
const SPRITES: usize = 5_000;

struct Config {
    output: PathBuf,
    frames: usize,
}

struct PhaseMetrics {
    baseline_p50_us: f64,
    profiled_p50_us: f64,
    overhead_pct: f64,
}

fn main() {
    let config = parse_config();
    let examples = games_dir().join("examples");
    let root = examples.join("game_api");
    let input = InputSnapshot::default();
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), sprite_scene(SPRITES))
        .expect("runtime scene for profile capture");

    for _ in 0..WARMUP_FRAMES {
        run_frame(&mut runtime, &input, None, false, 0);
    }

    let fine = measure_phase(&mut runtime, &input, config.frames, CaptureLevel::Fine);
    let system = measure_phase(&mut runtime, &input, config.frames, CaptureLevel::System);

    let mut trace = SpanTrace::new(CaptureLevel::Fine, DEFAULT_SPAN_CAPACITY);
    let mut fine_frames = Vec::with_capacity(config.frames);
    let mut system_names = Vec::new();
    for frame in 0..config.frames {
        fine_frames.push(run_frame(
            &mut runtime,
            &input,
            Some(&mut trace),
            true,
            frame,
        ));
        if system_names.is_empty()
            && let Some(report) = runtime.last_schedule_report()
        {
            system_names = report
                .timings
                .iter()
                .map(|timing| timing.name.clone())
                .collect();
        }
    }
    let summary = trace.summary();
    let trace_json = trace.to_chrome_trace_json();
    let trace_valid = validate_trace(&trace_json, summary.spans);

    fs::create_dir_all(&config.output).expect("create capture output directory");
    let trace_path = config.output.join("trace.json");
    fs::write(&trace_path, trace_json.as_bytes()).expect("write chrome trace");
    let report_path = config.output.join("report.json");

    let verdict = if overhead_within_budget(fine.overhead_pct) {
        "pass"
    } else if overhead_within_budget(system.overhead_pct) {
        "degraded"
    } else {
        "fail"
    };
    let recommended = capture_level_for_overhead(fine.overhead_pct);
    let trace_ok = trace_valid.is_ok();
    let report = format!(
        "{{\n  \"report_version\": \"{REPORT_VERSION}\",\n  \
         \"target\": {{\"scene\": \"wp0-profile-capture\", \"sprites\": {SPRITES},\
         \"frames\": {}, \"warmup\": {WARMUP_FRAMES}, \"viewport\": [{}, {}]}},\n  \
         \"capture\": {{\"level\": \"fine\", \"span_count\": {}, \"dropped_spans\": {},\
         \"capacity\": {}, \"systems\": {}, \"passes\": 0, \"passes_available\": false,\
         \"trace_file\": \"{}\", \"trace_bytes\": {}, \"trace_json_ok\": {}}},\n  \
         \"overhead\": {{\"fine\": {{\"baseline_p50_us\": {:.3}, \"profiled_p50_us\": {:.3},\
         \"overhead_pct\": {:.3}}}, \"system\": {{\"baseline_p50_us\": {:.3},\
         \"profiled_p50_us\": {:.3}, \"overhead_pct\": {:.3}}}, \"budget_pct\": {:.1},\
         \"verdict\": \"{}\", \"recommended_level\": \"{}\"}},\n  \
         \"frames\": {{\"fine_profiled_p50_us\": {:.3}, \"fine_profiled_p95_us\": {:.3}}},\n  \
         \"top_spans\": [{}]\n}}\n",
        config.frames,
        VIEWPORT.0,
        VIEWPORT.1,
        summary.spans,
        summary.dropped,
        summary.capacity,
        system_names.len(),
        trace_path.display().to_string().replace('\\', "/"),
        trace_json.len(),
        trace_ok,
        fine.baseline_p50_us,
        fine.profiled_p50_us,
        fine.overhead_pct,
        system.baseline_p50_us,
        system.profiled_p50_us,
        system.overhead_pct,
        TRACE_OVERHEAD_BUDGET_PCT,
        verdict,
        recommended.as_str(),
        percentile_us(&fine_frames, 50),
        percentile_us(&fine_frames, 95),
        top_spans_json(&trace, 8)
    );
    fs::write(&report_path, report.as_bytes()).expect("write capture report");

    println!(
        "wp0_profile_capture_ok report_version={REPORT_VERSION} frames={} span_count={} \
         dropped_spans={} systems={} passes=0 fine_overhead_pct={:.3} \
         system_overhead_pct={:.3} budget_pct={:.1} verdict={} recommended_level={} \
         trace_json_ok={} trace={} trace_bytes={} report={}",
        config.frames,
        summary.spans,
        summary.dropped,
        system_names.len(),
        fine.overhead_pct,
        system.overhead_pct,
        TRACE_OVERHEAD_BUDGET_PCT,
        verdict,
        recommended.as_str(),
        trace_ok,
        trace_path.display(),
        trace_json.len(),
        report_path.display()
    );
    if let Err(error) = trace_valid {
        eprintln!("trace 结构校验失败：{error}");
        std::process::exit(1);
    }
    if verdict == "fail" {
        eprintln!(
            "开销门失败：Fine {:.3}% / System {:.3}% 均超过 {:.1}%",
            fine.overhead_pct, system.overhead_pct, TRACE_OVERHEAD_BUDGET_PCT
        );
        std::process::exit(1);
    }
}

fn parse_config() -> Config {
    let mut output = PathBuf::from("target/reports/p3/profile-capture");
    let mut frames = DEFAULT_FRAMES;
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--output" => {
                output = PathBuf::from(args.next().expect("--output requires a directory"));
            }
            "--frames" => {
                frames = args
                    .next()
                    .expect("--frames requires an integer")
                    .parse()
                    .expect("--frames must be an integer");
            }
            "--help" | "-h" => {
                println!("Usage: wp0_profile_capture [--frames N] [--output DIR]");
                std::process::exit(0);
            }
            other => panic!("unknown argument: {other}"),
        }
    }
    assert!(frames > 0, "--frames must be greater than zero");
    Config { output, frames }
}

/// 交替 A/B：每轮先跑一帧无 trace 的基线，再跑一帧带 trace 的同一场景。
fn measure_phase(
    runtime: &mut RuntimeScene,
    input: &InputSnapshot,
    frames: usize,
    level: CaptureLevel,
) -> PhaseMetrics {
    let mut trace = SpanTrace::new(level, DEFAULT_SPAN_CAPACITY);
    let mut baseline = Vec::with_capacity(frames);
    let mut profiled = Vec::with_capacity(frames);
    let fine = level == CaptureLevel::Fine;
    for frame in 0..frames {
        baseline.push(run_frame(runtime, input, None, false, frame));
        profiled.push(run_frame(runtime, input, Some(&mut trace), fine, frame));
    }
    let baseline_p50_us = percentile_us(&baseline, 50);
    let profiled_p50_us = percentile_us(&profiled, 50);
    PhaseMetrics {
        baseline_p50_us,
        profiled_p50_us,
        overhead_pct: overhead_pct(baseline_p50_us, profiled_p50_us),
    }
}

/// 一帧：脚本步进 → 2D 提取 → 3D 提取。`fine=false` 时只记帧/脚本/系统级（System 档）。
fn run_frame(
    runtime: &mut RuntimeScene,
    input: &InputSnapshot,
    trace: Option<&mut SpanTrace>,
    fine: bool,
    frame_index: usize,
) -> Duration {
    let frame_start = Instant::now();
    let mut trace = trace;
    let frame_token = open_frame(&mut trace, frame_start, frame_index);

    let script_start = Instant::now();
    runtime
        .run_script_steps(1, 1.0 / 60.0, input)
        .expect("script step");
    let script_elapsed = script_start.elapsed();
    record_span(
        &mut trace,
        "script",
        "script",
        script_start,
        script_elapsed,
        1,
        Vec::new(),
    );
    if let Some(report) = runtime.last_schedule_report() {
        let mut offset = Duration::ZERO;
        for timing in &report.timings {
            record_span(
                &mut trace,
                &timing.name,
                "system",
                script_start + offset,
                timing.elapsed,
                2,
                vec![
                    ("stage", timing.stage.to_string()),
                    ("synthetic", "true".to_owned()),
                ],
            );
            offset += timing.elapsed;
        }
    }

    let extract_start = Instant::now();
    let mesh = runtime
        .vertices(VIEWPORT.0, VIEWPORT.1)
        .expect("2D extraction");
    let mesh_elapsed = extract_start.elapsed();
    if fine {
        record_span(
            &mut trace,
            "extract2d",
            "extract",
            extract_start,
            mesh_elapsed,
            1,
            Vec::new(),
        );
    }

    let scene_start = Instant::now();
    let scene3d =
        runtime.scene_3d(Viewport::new(VIEWPORT.0 as f32, VIEWPORT.1 as f32).expect("viewport"));
    let scene_elapsed = scene_start.elapsed();
    if fine {
        record_span(
            &mut trace,
            "extract3d",
            "extract",
            scene_start,
            scene_elapsed,
            1,
            Vec::new(),
        );
    }

    drop(scene3d);
    drop(mesh);
    close_frame(&mut trace, frame_token);
    frame_start.elapsed()
}

fn open_frame(
    trace: &mut Option<&mut SpanTrace>,
    start: Instant,
    frame_index: usize,
) -> Option<SpanToken> {
    trace.as_mut()?.begin_at(
        "frame",
        "frame",
        start,
        vec![("frame", frame_index.to_string())],
    )
}

fn record_span(
    trace: &mut Option<&mut SpanTrace>,
    name: &str,
    category: &'static str,
    start: Instant,
    elapsed: Duration,
    depth: u16,
    args: Vec<(&'static str, String)>,
) {
    if let Some(trace) = trace.as_mut() {
        trace.push_span_at(name, category, start, elapsed, depth, args);
    }
}

fn close_frame(trace: &mut Option<&mut SpanTrace>, token: Option<SpanToken>) {
    if let (Some(trace), Some(token)) = (trace.as_mut(), token) {
        trace.end_at(token, Instant::now());
    }
}

fn percentile_us(samples: &[Duration], percent: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut values = samples
        .iter()
        .map(|duration| duration.as_secs_f64() * 1_000_000.0)
        .collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    let at = ((values.len() * percent).saturating_sub(1) / 100).min(values.len() - 1);
    values[at]
}

/// 固定场景：`count` 个 Sprite 的网格排布（与 `wp0_scene_frame` 的阶梯同族）。
fn sprite_scene(count: usize) -> Scene {
    let mut scene = Scene::new(format!("wp0-profile-capture-{count}"));
    for index in 0..count {
        let id = SceneEntityId(index as u64 + 1);
        scene
            .add_entity(id, format!("Sprite{index}"), None)
            .expect("add sprite entity");
        let x = (index % 40) as f32 * 34.0 - 600.0;
        let y = (index / 40) as f32 * 34.0 - 300.0;
        let entity = scene.entities.get_mut(&id).expect("sprite entity");
        entity.components.insert(
            "Transform".into(),
            format!("position={x},{y},0;rotation=0,0,0,1;scale=1,1,1"),
        );
        entity.components.insert(
            "Sprite".into(),
            format!(
                "texture=assets/textures/player.png;size=32x32;pivot=0.500,0.500;\
                 tint=1.000,1.000,1.000,1.000;sorting_layer=default;order={}",
                index / 500
            ),
        );
    }
    scene
}

fn top_spans_json(trace: &SpanTrace, limit: usize) -> String {
    trace
        .top_n(limit)
        .iter()
        .map(|stat| {
            format!(
                "{{\"name\":\"{}\",\"category\":\"{}\",\"count\":{},\"total_us\":{:.3},\
                 \"p50_us\":{:.3},\"p95_us\":{:.3},\"max_us\":{:.3}}}",
                stat.name,
                stat.category,
                stat.count,
                stat.total_us,
                stat.p50_us,
                stat.p95_us,
                stat.max_us
            )
        })
        .collect::<Vec<_>>()
        .join(",\n    ")
}

/// 用 serde_json 真解析一遍，并检查 Chrome Trace 的必需字段。格式错误在这里就判红，
/// 而不是等人工打开浏览器才发现。
fn validate_trace(json: &str, expected_spans: usize) -> Result<(), String> {
    let parsed: serde_json::Value =
        serde_json::from_str(json).map_err(|error| format!("JSON 解析失败：{error}"))?;
    let events = parsed
        .get("traceEvents")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "缺少 traceEvents 数组".to_owned())?;
    if events.len() != expected_spans {
        return Err(format!(
            "事件数 {} 与 span_count {expected_spans} 不一致",
            events.len()
        ));
    }
    for (index, event) in events.iter().enumerate() {
        for field in ["name", "cat", "ph", "ts", "dur", "pid", "tid", "args"] {
            if event.get(field).is_none() {
                return Err(format!("第 {index} 个事件缺字段 {field}"));
            }
        }
        if event["ph"] != "X" {
            return Err(format!("第 {index} 个事件 ph 不是 X"));
        }
        if !event["ts"].is_number() || !event["dur"].is_number() {
            return Err(format!("第 {index} 个事件 ts/dur 不是数值"));
        }
    }
    let version = parsed
        .get("metadata")
        .and_then(|metadata| metadata.get("report_version"))
        .and_then(serde_json::Value::as_str);
    if version != Some(TRACE_REPORT_VERSION) {
        return Err(format!("metadata.report_version={version:?} 不符"));
    }
    Ok(())
}
