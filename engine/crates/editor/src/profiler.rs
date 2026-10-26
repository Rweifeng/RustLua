//! Editor profiler: bounded timing samples, counters and gauges.
//!
//! The plan (`docs/55` P0) requires an editor profiler covering UI frame time, panel cost,
//! command cost, scan/import queues, GPU readback and cache hit rate. Recording is
//! allocation-free after warm-up: every series is a fixed-capacity ring buffer, so a long
//! session cannot grow profiler memory without limit.

use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Write as _,
    time::{Duration, Instant},
};

use engine::host::profile_trace::{
    CaptureLevel, DEFAULT_SPAN_CAPACITY, SpanRecord, SpanStat, SpanToken, SpanTrace,
    SpanTraceSummary,
};

/// Default per-frame budget (60 FPS) used to count over-budget frames.
pub const DEFAULT_FRAME_BUDGET_MS: f32 = 16.7;
/// Default number of retained samples per series (about 4 seconds at 60 FPS).
pub const DEFAULT_SAMPLE_CAPACITY: usize = 240;

/// Fixed-capacity ring of `f32` samples.
#[derive(Clone, Debug)]
struct SampleRing {
    capacity: usize,
    samples: VecDeque<f32>,
}

impl SampleRing {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            samples: VecDeque::with_capacity(capacity),
        }
    }

    fn push(&mut self, value: f32) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
    }

    fn clear(&mut self) {
        self.samples.clear();
    }

    fn len(&self) -> usize {
        self.samples.len()
    }

    fn last(&self) -> Option<f32> {
        self.samples.back().copied()
    }

    /// Nearest-rank percentile (`percentile` in 0..=100) over retained samples.
    fn percentile(&self, percentile: f32) -> Option<f32> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted: Vec<f32> = self.samples.iter().copied().collect();
        sorted.sort_by(|left, right| left.total_cmp(right));
        let clamped = percentile.clamp(0.0, 100.0);
        let rank = ((clamped / 100.0) * sorted.len() as f32).ceil() as usize;
        let index = rank.saturating_sub(1).min(sorted.len() - 1);
        Some(sorted[index])
    }

    fn max(&self) -> Option<f32> {
        self.samples.iter().copied().fold(None, |acc, value| {
            Some(acc.map_or(value, |current: f32| current.max(value)))
        })
    }
}

/// Summary of one timing sample set, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Percentiles {
    pub p50: f32,
    pub p95: f32,
    pub p99: f32,
    pub max: f32,
}

/// Nearest-rank percentiles over millisecond samples. `None` for an empty slice.
///
/// Shared by the headless baseline report and tests so recorded evidence uses exactly the
/// same percentile definition as the in-editor profiler.
pub fn percentiles_ms(samples: &[f32]) -> Option<Percentiles> {
    if samples.is_empty() {
        return None;
    }
    let mut ring = SampleRing::new(samples.len());
    for sample in samples {
        ring.push(*sample);
    }
    Some(Percentiles {
        p50: ring.percentile(50.0)?,
        p95: ring.percentile(95.0)?,
        p99: ring.percentile(99.0)?,
        max: ring.max()?,
    })
}

/// Aggregated timing for one named series (panel, command, job stage).
#[derive(Clone, Debug, Default)]
pub struct TimingSummary {
    pub name: String,
    pub samples: usize,
    pub last_ms: f32,
    pub p50_ms: f32,
    pub p95_ms: f32,
    pub max_ms: f32,
    pub total_ms: f64,
}

/// Immutable snapshot handed to the HUD, status bar or an exported report artifact.
#[derive(Clone, Debug, Default)]
pub struct ProfilerReport {
    pub enabled: bool,
    pub frame_samples: usize,
    pub frame_last_ms: f32,
    pub frame_p50_ms: f32,
    pub frame_p95_ms: f32,
    pub frame_p99_ms: f32,
    pub frame_max_ms: f32,
    pub frame_budget_ms: f32,
    pub over_budget_frames: u64,
    pub panels: Vec<TimingSummary>,
    pub commands: Vec<TimingSummary>,
    pub counters: Vec<(String, u64)>,
    pub gauges: Vec<(String, f64)>,
}

impl ProfilerReport {
    /// Human-readable multi-line report; used by the HUD and by baseline artifacts.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "editor profiler: enabled={} budget={:.1}ms samples={} over_budget={}",
            self.enabled, self.frame_budget_ms, self.frame_samples, self.over_budget_frames
        );
        let _ = writeln!(
            out,
            "frame: last={:.2}ms p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms",
            self.frame_last_ms,
            self.frame_p50_ms,
            self.frame_p95_ms,
            self.frame_p99_ms,
            self.frame_max_ms
        );
        for panel in &self.panels {
            let _ = writeln!(
                out,
                "panel {}: last={:.2}ms p95={:.2}ms max={:.2}ms n={}",
                panel.name, panel.last_ms, panel.p95_ms, panel.max_ms, panel.samples
            );
        }
        for command in &self.commands {
            let _ = writeln!(
                out,
                "command {}: last={:.2}ms p95={:.2}ms n={} total={:.1}ms",
                command.name, command.last_ms, command.p95_ms, command.samples, command.total_ms
            );
        }
        for (name, value) in &self.counters {
            let _ = writeln!(out, "counter {name} = {value}");
        }
        for (name, value) in &self.gauges {
            let _ = writeln!(out, "gauge {name} = {value:.3}");
        }
        out
    }

    pub fn frame_budget_exceeded(&self) -> bool {
        self.frame_p95_ms > self.frame_budget_ms && self.frame_samples > 0
    }
}

/// Bounded editor profiler. Cheap enough to leave enabled in Debug builds.
#[derive(Clone, Debug)]
pub struct EditorProfiler {
    enabled: bool,
    frame_budget_ms: f32,
    sample_capacity: usize,
    frames: SampleRing,
    over_budget_frames: u64,
    frame_started: Option<Instant>,
    panels: BTreeMap<String, (SampleRing, f64)>,
    commands: BTreeMap<String, (SampleRing, f64)>,
    counters: BTreeMap<String, u64>,
    gauges: BTreeMap<String, f64>,
    /// P3-E06-2：按需开启的 span 采集（默认 Off，不开销）。
    trace: SpanTrace,
    /// 当前帧的根 span 令牌；`begin_frame` 开启、`end_frame` 闭合。
    frame_span: Option<SpanToken>,
}

impl Default for EditorProfiler {
    fn default() -> Self {
        Self::new(DEFAULT_FRAME_BUDGET_MS)
    }
}

impl EditorProfiler {
    pub fn new(frame_budget_ms: f32) -> Self {
        Self {
            enabled: true,
            frame_budget_ms: if frame_budget_ms > 0.0 {
                frame_budget_ms
            } else {
                DEFAULT_FRAME_BUDGET_MS
            },
            sample_capacity: DEFAULT_SAMPLE_CAPACITY,
            frames: SampleRing::new(DEFAULT_SAMPLE_CAPACITY),
            over_budget_frames: 0,
            frame_started: None,
            panels: BTreeMap::new(),
            commands: BTreeMap::new(),
            counters: BTreeMap::new(),
            gauges: BTreeMap::new(),
            trace: SpanTrace::new(CaptureLevel::Off, DEFAULT_SPAN_CAPACITY),
            frame_span: None,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.frame_span = None;
            self.trace.abort_open();
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn frame_budget_ms(&self) -> f32 {
        self.frame_budget_ms
    }

    /// Start a frame. Repeated calls without `end_frame` restart the measurement.
    pub fn begin_frame(&mut self, now: Instant) {
        if self.enabled {
            self.frame_started = Some(now);
            // 重复 begin 时上一帧的未闭合 span 已不可信（结束时间会被算进新帧）。
            self.trace.abort_open();
            self.frame_span = self.trace.begin_at("frame", "editor", now, Vec::new());
        }
    }

    /// Finish a frame and retain its duration.
    pub fn end_frame(&mut self, now: Instant) {
        if !self.enabled {
            return;
        }
        let Some(started) = self.frame_started.take() else {
            return;
        };
        if let Some(token) = self.frame_span.take() {
            self.trace.end_at(token, now);
        }
        let elapsed_ms = now.saturating_duration_since(started).as_secs_f32() * 1000.0;
        if elapsed_ms > self.frame_budget_ms {
            self.over_budget_frames = self.over_budget_frames.saturating_add(1);
        }
        self.frames.push(elapsed_ms);
    }

    pub fn record_panel(&mut self, name: &str, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        if self.trace.level().allows_fine() {
            let now = Instant::now();
            let start = now.checked_sub(elapsed).unwrap_or(now);
            let start = self.frame_started.map_or(start, |frame| start.max(frame));
            self.trace
                .push_span_at(name, "panel", start, elapsed, 1, Vec::new());
        }
        let entry = self
            .panels
            .entry(name.to_owned())
            .or_insert_with(|| (SampleRing::new(self.sample_capacity), 0.0));
        let ms = elapsed.as_secs_f32() * 1000.0;
        entry.0.push(ms);
        entry.1 += f64::from(ms);
    }

    pub fn record_command(&mut self, name: &str, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        if self.trace.level().allows_fine() {
            let now = Instant::now();
            let start = now.checked_sub(elapsed).unwrap_or(now);
            let start = self.frame_started.map_or(start, |frame| start.max(frame));
            self.trace
                .push_span_at(name, "command", start, elapsed, 1, Vec::new());
        }
        let entry = self
            .commands
            .entry(name.to_owned())
            .or_insert_with(|| (SampleRing::new(self.sample_capacity), 0.0));
        let ms = elapsed.as_secs_f32() * 1000.0;
        entry.0.push(ms);
        entry.1 += f64::from(ms);
    }

    /// P3-E06-2：开始 span 捕获（`Off` 等价于停止）。已在捕获中时重新开始会清空
    /// 旧记录——捕获大小有界，不留两份。
    pub fn start_span_capture(&mut self, level: CaptureLevel) {
        self.trace = SpanTrace::new(level, DEFAULT_SPAN_CAPACITY);
        self.frame_span = None;
    }

    pub fn stop_span_capture(&mut self) {
        self.trace.set_level(CaptureLevel::Off);
        self.frame_span = None;
    }

    /// 清空已记录的 span（采集档位不变，继续捕获时从头累积）。
    pub fn clear_span_records(&mut self) {
        self.trace.clear();
        self.frame_span = None;
    }

    #[must_use]
    pub fn span_capture_level(&self) -> CaptureLevel {
        self.trace.level()
    }

    /// 当前档位是否记录运行时系统级 span（`System`/`Fine`）。
    #[must_use]
    pub fn capture_records_systems(&self) -> bool {
        self.trace.level().records_spans()
    }

    #[must_use]
    pub fn span_capture_summary(&self) -> SpanTraceSummary {
        self.trace.summary()
    }

    #[must_use]
    pub fn span_top_n(&self, limit: usize) -> Vec<SpanStat> {
        self.trace.top_n(limit)
    }

    /// 面板时间线的输入：最近 `limit` 条已闭合 span（捕获顺序，环形缓冲的尾部）。
    /// 拷贝上限就是 `limit`，不随 `DEFAULT_SPAN_CAPACITY` 涨——面板每帧都要取一次。
    #[must_use]
    pub fn span_records_for_timeline(&self, limit: usize) -> Vec<SpanRecord> {
        self.trace.latest_records(limit)
    }

    #[must_use]
    pub fn span_report_text(&self, top: usize) -> String {
        self.trace.to_text(top)
    }

    /// Chrome Trace Event JSON（`traceEvents` / `ph:"X"` / `ts/dur` / `pid/tid` / `args`）。
    #[must_use]
    pub fn span_trace_json(&self) -> String {
        self.trace.to_chrome_trace_json()
    }

    /// 桥接 `FrameScheduleReport` 的 per-system 耗时。报告只有耗时没有开始时间，
    /// 调用方按顺序铺开（`synthetic=true`）：trace 里看到的是"同一段调度内谁占了多少"，
    /// 而不是各系统真实的墙钟起点。
    pub fn record_runtime_span(
        &mut self,
        name: &str,
        stage: &str,
        start: Instant,
        elapsed: Duration,
    ) {
        if !self.capture_records_systems() {
            return;
        }
        self.trace.push_span_at(
            name,
            "system",
            start,
            elapsed,
            1,
            vec![
                ("stage", stage.to_owned()),
                ("synthetic", "true".to_owned()),
            ],
        );
    }

    pub fn bump(&mut self, counter: &str) {
        self.add(counter, 1);
    }

    pub fn add(&mut self, counter: &str, amount: u64) {
        if !self.enabled {
            return;
        }
        let entry = self.counters.entry(counter.to_owned()).or_insert(0);
        *entry = entry.saturating_add(amount);
    }

    pub fn counter(&self, counter: &str) -> u64 {
        self.counters.get(counter).copied().unwrap_or(0)
    }

    /// Gauges are absolute values (queue lengths, retained item counts, cache entries).
    pub fn set_gauge(&mut self, gauge: &str, value: f64) {
        if !self.enabled {
            return;
        }
        self.gauges.insert(gauge.to_owned(), value);
    }

    pub fn gauge(&self, gauge: &str) -> Option<f64> {
        self.gauges.get(gauge).copied()
    }

    pub fn frame_samples(&self) -> usize {
        self.frames.len()
    }

    pub fn over_budget_frames(&self) -> u64 {
        self.over_budget_frames
    }

    pub fn clear_samples(&mut self) {
        self.frames.clear();
        for (ring, total) in self.panels.values_mut() {
            ring.clear();
            *total = 0.0;
        }
        for (ring, total) in self.commands.values_mut() {
            ring.clear();
            *total = 0.0;
        }
        self.over_budget_frames = 0;
    }

    pub fn report(&self) -> ProfilerReport {
        ProfilerReport {
            enabled: self.enabled,
            frame_samples: self.frames.len(),
            frame_last_ms: self.frames.last().unwrap_or(0.0),
            frame_p50_ms: self.frames.percentile(50.0).unwrap_or(0.0),
            frame_p95_ms: self.frames.percentile(95.0).unwrap_or(0.0),
            frame_p99_ms: self.frames.percentile(99.0).unwrap_or(0.0),
            frame_max_ms: self.frames.max().unwrap_or(0.0),
            frame_budget_ms: self.frame_budget_ms,
            over_budget_frames: self.over_budget_frames,
            panels: self.summaries(&self.panels),
            commands: self.summaries(&self.commands),
            counters: self
                .counters
                .iter()
                .map(|(name, value)| (name.clone(), *value))
                .collect(),
            gauges: self
                .gauges
                .iter()
                .map(|(name, value)| (name.clone(), *value))
                .collect(),
        }
    }

    fn summaries(&self, series: &BTreeMap<String, (SampleRing, f64)>) -> Vec<TimingSummary> {
        series
            .iter()
            .map(|(name, (ring, total))| TimingSummary {
                name: name.clone(),
                samples: ring.len(),
                last_ms: ring.last().unwrap_or(0.0),
                p50_ms: ring.percentile(50.0).unwrap_or(0.0),
                p95_ms: ring.percentile(95.0).unwrap_or(0.0),
                max_ms: ring.max().unwrap_or(0.0),
                total_ms: *total,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_ms_matches_ring_definition() {
        assert!(percentiles_ms(&[]).is_none());
        let samples = [1.0_f32, 2.0, 3.0, 4.0];
        let stats = percentiles_ms(&samples).expect("non-empty samples");
        assert_eq!(stats.p50, 2.0);
        assert_eq!(stats.p95, 4.0);
        assert_eq!(stats.max, 4.0);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let mut ring = SampleRing::new(100);
        for value in 1..=100 {
            ring.push(value as f32);
        }
        assert_eq!(ring.percentile(50.0), Some(50.0));
        assert_eq!(ring.percentile(95.0), Some(95.0));
        assert_eq!(ring.percentile(100.0), Some(100.0));
        assert_eq!(ring.max(), Some(100.0));
    }

    #[test]
    fn empty_series_reports_nothing() {
        let ring = SampleRing::new(4);
        assert!(ring.percentile(95.0).is_none());
        assert_eq!(ring.len(), 0);
    }

    #[test]
    fn sample_rings_stay_bounded() {
        let mut profiler = EditorProfiler::new(DEFAULT_FRAME_BUDGET_MS);
        let start = Instant::now();
        for step in 0..(DEFAULT_SAMPLE_CAPACITY * 3) {
            profiler.begin_frame(start + Duration::from_millis(step as u64));
            profiler
                .end_frame(start + Duration::from_millis(step as u64) + Duration::from_millis(1));
            profiler.record_panel("hierarchy", Duration::from_micros(500));
        }
        let report = profiler.report();
        assert_eq!(report.frame_samples, DEFAULT_SAMPLE_CAPACITY);
        assert_eq!(report.panels.len(), 1);
        assert_eq!(report.panels[0].samples, DEFAULT_SAMPLE_CAPACITY);
        assert!((report.frame_last_ms - 1.0).abs() < 0.001);
    }

    #[test]
    fn over_budget_frames_are_counted() {
        let mut profiler = EditorProfiler::new(10.0);
        let start = Instant::now();
        profiler.begin_frame(start);
        profiler.end_frame(start + Duration::from_millis(4));
        profiler.begin_frame(start);
        profiler.end_frame(start + Duration::from_millis(25));
        assert_eq!(profiler.over_budget_frames(), 1);
        // Nearest-rank p95 over {4ms, 25ms} is 25ms, i.e. above the 10ms budget.
        assert!(profiler.report().frame_budget_exceeded());
        assert_eq!(profiler.report().frame_max_ms, 25.0);
    }

    #[test]
    fn disabled_profiler_records_nothing() {
        let mut profiler = EditorProfiler::new(DEFAULT_FRAME_BUDGET_MS);
        profiler.set_enabled(false);
        let start = Instant::now();
        profiler.begin_frame(start);
        profiler.end_frame(start + Duration::from_millis(5));
        profiler.record_panel("inspector", Duration::from_millis(3));
        profiler.bump("resource_scan_started");
        profiler.set_gauge("undo_len", 12.0);
        let report = profiler.report();
        assert_eq!(report.frame_samples, 0);
        assert!(report.panels.is_empty());
        assert!(report.counters.is_empty());
        assert!(report.gauges.is_empty());
    }

    #[test]
    fn counters_and_gauges_are_reported() {
        let mut profiler = EditorProfiler::new(DEFAULT_FRAME_BUDGET_MS);
        profiler.bump("hierarchy_cache_hit");
        profiler.add("hierarchy_cache_hit", 4);
        profiler.bump("hierarchy_cache_miss");
        profiler.set_gauge("undo_len", 3.0);
        profiler.record_command("undo", Duration::from_millis(2));
        let report = profiler.report();
        assert_eq!(profiler.counter("hierarchy_cache_hit"), 5);
        assert_eq!(profiler.counter("hierarchy_cache_miss"), 1);
        assert_eq!(profiler.gauge("undo_len"), Some(3.0));
        let text = report.to_text();
        assert!(text.contains("counter hierarchy_cache_hit = 5"));
        assert!(text.contains("gauge undo_len = 3.000"));
        assert!(text.contains("command undo"));
    }

    #[test]
    fn clearing_samples_keeps_counters() {
        let mut profiler = EditorProfiler::new(DEFAULT_FRAME_BUDGET_MS);
        profiler.bump("scan");
        profiler.record_panel("console", Duration::from_millis(1));
        profiler.clear_samples();
        assert_eq!(profiler.frame_samples(), 0);
        assert!(profiler.report().panels[0].samples == 0);
        assert_eq!(profiler.counter("scan"), 1);
    }

    /// E06-2：Fine 档记录帧根 span、面板与命令；导出 JSON 能被解析。
    #[test]
    fn span_capture_records_frame_panels_and_commands() {
        let mut profiler = EditorProfiler::new(DEFAULT_FRAME_BUDGET_MS);
        assert_eq!(profiler.span_capture_level(), CaptureLevel::Off);
        profiler.start_span_capture(CaptureLevel::Fine);
        let start = Instant::now();
        profiler.begin_frame(start);
        profiler.record_panel("hierarchy", Duration::from_millis(2));
        profiler.record_command("undo", Duration::from_millis(1));
        profiler.end_frame(start + Duration::from_millis(8));
        let summary = profiler.span_capture_summary();
        assert_eq!(summary.level, CaptureLevel::Fine);
        assert_eq!(summary.spans, 3, "frame + panel + command");
        assert_eq!(summary.dropped, 0);
        let names = profiler
            .span_top_n(8)
            .iter()
            .map(|stat| stat.name.clone())
            .collect::<Vec<_>>();
        assert!(names.contains(&"frame".to_owned()));
        assert!(names.contains(&"hierarchy".to_owned()));
        assert!(names.contains(&"undo".to_owned()));
        let parsed: serde_json::Value =
            serde_json::from_str(&profiler.span_trace_json()).expect("trace 必须是合法 JSON");
        let events = parsed["traceEvents"].as_array().expect("traceEvents");
        assert_eq!(events.len(), 3);
        assert!(profiler.span_report_text(5).contains("level=fine"));
        profiler.stop_span_capture();
        assert_eq!(profiler.span_capture_level(), CaptureLevel::Off);
    }

    /// System 档只留帧与系统级 span；面板/命令这类细粒度 scope 不开销。
    #[test]
    fn system_capture_level_skips_fine_scopes() {
        let mut profiler = EditorProfiler::new(DEFAULT_FRAME_BUDGET_MS);
        profiler.start_span_capture(CaptureLevel::System);
        let start = Instant::now();
        profiler.begin_frame(start);
        profiler.record_panel("hierarchy", Duration::from_millis(2));
        profiler.record_runtime_span("physics2d_step", "Physics", start, Duration::from_millis(1));
        profiler.end_frame(start + Duration::from_millis(8));
        let summary = profiler.span_capture_summary();
        assert_eq!(summary.spans, 2, "frame + runtime system");
        assert!(profiler.capture_records_systems());
        let stats = profiler.span_top_n(4);
        assert!(stats.iter().any(|stat| stat.name == "physics2d_step"));
        assert!(!stats.iter().any(|stat| stat.name == "hierarchy"));
    }

    #[test]
    fn off_capture_records_no_spans() {
        let mut profiler = EditorProfiler::new(DEFAULT_FRAME_BUDGET_MS);
        let start = Instant::now();
        profiler.begin_frame(start);
        profiler.record_panel("hierarchy", Duration::from_millis(2));
        profiler.record_runtime_span("lua_update", "Script", start, Duration::from_millis(1));
        profiler.end_frame(start + Duration::from_millis(8));
        assert!(profiler.span_capture_summary().spans == 0);
        assert!(!profiler.capture_records_systems());
        assert!(!profiler.span_trace_json().contains("physics"));
    }
}
