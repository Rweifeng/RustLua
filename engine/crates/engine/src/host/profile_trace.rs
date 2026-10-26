//! 有界 span 环形缓冲与 Chrome Trace Event 导出（P3-E06-2）。
//!
//! 为什么不是扩展 [`crate::host::frame_profile::FrameProfile`]：那个模块只保留**分布**
//! （p50/p95/max），导出 trace 需要每个 span 的**开始时间、耗时、嵌套深度与参数**；
//! 二者回答的问题不同——FrameProfile 回答"稳态分布"，trace 回答"这一帧到底按什么
//! 顺序花了多久"。本模块只管采集与导出，不解析场景、不认识渲染器，编辑器与 headless
//! 证据程序共用同一份实现。
//!
//! **有界**：缓冲容量固定（默认 [`DEFAULT_SPAN_CAPACITY`]），满了丢最旧并累加
//! [`SpanTrace::dropped`]；长会话不会无限吃内存（计划 §6.13「内存有界」）。
//!
//! **开销门**：细粒度 scope 的预算按 docs/55 是主线程 ≤2%。[`overhead_pct`] 只做
//! 纯计算，判定与降级由调用方写进报告（[`capture_level_for_overhead`]）：超预算时
//! 默认只保留系统级，细粒度按需再开。
//!
//! **Chrome/Perfetto**：输出顶层 `traceEvents`，每个 span 是 `ph:"X"` 的 complete
//! event（`ts`/`dur` 微秒、`pid`/`tid`、`args`）。同一 `tid` 上按时间自然嵌套，
//! Chrome 的 timeline 与 Perfetto 都能直接打开，无需本仓自绘 flame chart 才能看。

use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Write as _,
    time::Duration,
};

// wasm32-unknown-unknown 的 std `Instant::now()` 直接 panic；`web_time` 在原生上是同一类型。
use web_time::Instant;

/// 默认缓冲容量。按 120 帧、每帧约 25 个 span（frame + script + 20 systems + 提取）
/// 估算约 3k span；8k 留出余量，也能覆盖编辑器约 10 秒的面板/命令 scope。
pub const DEFAULT_SPAN_CAPACITY: usize = 8_192;

/// docs/55：细粒度 profiler scope 的主线程开销预算（百分比）。
pub const TRACE_OVERHEAD_BUDGET_PCT: f64 = 2.0;

/// trace 报告的版本号（脚本与断言读它，避免把旧格式当成新格式）。
pub const TRACE_REPORT_VERSION: &str = "p3-profile-trace-v1";

/// 采集档位。`Off` 不记任何 span；`System` 只记帧与运行时系统级；`Fine` 额外记
/// panel/command/提取这类细粒度 scope。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureLevel {
    #[default]
    Off,
    System,
    Fine,
}

impl CaptureLevel {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::System => "system",
            Self::Fine => "fine",
        }
    }

    #[must_use]
    pub fn records_spans(self) -> bool {
        self != Self::Off
    }

    #[must_use]
    pub fn allows_fine(self) -> bool {
        self == Self::Fine
    }
}

/// 一个已完成的 span。时间一律相对 `SpanTrace` 的捕获起点，单位纳秒。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanRecord {
    pub name: String,
    pub category: &'static str,
    pub start_ns: u64,
    pub dur_ns: u64,
    /// 嵌套深度（0 = 帧根）。Chrome 自己按时间嵌套，深度只用于面板文本与断言。
    pub depth: u16,
    pub args: Vec<(&'static str, String)>,
}

/// `begin_at` 返回的配对令牌；`end_at` 用它找回开始时间。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpanToken(u64);

#[derive(Clone, Debug)]
struct OpenSpan {
    token: u64,
    name: String,
    category: &'static str,
    start_ns: u64,
    depth: u16,
    args: Vec<(&'static str, String)>,
}

/// 采集概况（面板/报告用）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpanTraceSummary {
    pub level: CaptureLevel,
    pub capacity: usize,
    pub spans: usize,
    pub dropped: u64,
    /// 最后一个 span 的结束时间相对捕获起点（只覆盖被记录的部分，不是整帧墙钟）。
    pub covered_ns: u64,
}

/// 同名 span 的聚合行（top-N 表）。
#[derive(Clone, Debug, PartialEq)]
pub struct SpanStat {
    pub name: String,
    pub category: &'static str,
    pub count: usize,
    pub total_us: f64,
    pub p50_us: f64,
    pub p95_us: f64,
    pub max_us: f64,
}

/// 有界 span 采集器。所有记录接口在 `Off` 档下都是 no-op。
#[derive(Clone, Debug)]
pub struct SpanTrace {
    level: CaptureLevel,
    capacity: usize,
    origin: Instant,
    spans: VecDeque<SpanRecord>,
    dropped: u64,
    open: Vec<OpenSpan>,
    next_token: u64,
}

impl SpanTrace {
    #[must_use]
    pub fn new(level: CaptureLevel, capacity: usize) -> Self {
        Self {
            level,
            capacity: capacity.max(1),
            origin: Instant::now(),
            spans: VecDeque::new(),
            dropped: 0,
            open: Vec::new(),
            next_token: 0,
        }
    }

    #[must_use]
    pub fn level(&self) -> CaptureLevel {
        self.level
    }

    /// 切档。切到 `Off` 时丢弃仍在栈上的未闭合 span（半截 span 不能进 trace）。
    pub fn set_level(&mut self, level: CaptureLevel) {
        self.level = level;
        if !level.records_spans() {
            self.open.clear();
        }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 最近 `count` 条已闭合记录的快照（**捕获顺序**，未按起点排序；排序由调用方决定，
    /// Chrome 导出与面板时间线用的是同一条序）。面板只画得起像素预算内的那一段，所以
    /// 这里就把它截掉，而不是先拷 8192 条再让调用方丢弃。
    #[must_use]
    pub fn latest_records(&self, count: usize) -> Vec<SpanRecord> {
        let skip = self.spans.len().saturating_sub(count);
        self.spans.iter().skip(skip).cloned().collect()
    }

    pub fn clear(&mut self) {
        self.spans.clear();
        self.dropped = 0;
        self.open.clear();
    }

    /// 丢弃未闭合的 span（例如采集被中途关闭）。已闭合的记录不受影响。
    pub fn abort_open(&mut self) {
        self.open.clear();
    }

    #[must_use]
    pub fn summary(&self) -> SpanTraceSummary {
        SpanTraceSummary {
            level: self.level,
            capacity: self.capacity,
            spans: self.spans.len(),
            dropped: self.dropped,
            covered_ns: self
                .spans
                .iter()
                .map(|span| span.start_ns.saturating_add(span.dur_ns))
                .max()
                .unwrap_or(0),
        }
    }

    /// 以 `start` 为起点开一个 span；返回的令牌必须交给 [`Self::end_at`]。
    pub fn begin_at(
        &mut self,
        name: &str,
        category: &'static str,
        start: Instant,
        args: Vec<(&'static str, String)>,
    ) -> Option<SpanToken> {
        if !self.level.records_spans() {
            return None;
        }
        self.next_token = self.next_token.saturating_add(1);
        let token = self.next_token;
        let depth = u16::try_from(self.open.len()).unwrap_or(u16::MAX);
        self.open.push(OpenSpan {
            token,
            name: name.to_owned(),
            category,
            start_ns: self.relative_ns(start),
            depth,
            args,
        });
        Some(SpanToken(token))
    }

    pub fn begin(
        &mut self,
        name: &str,
        category: &'static str,
        args: Vec<(&'static str, String)>,
    ) -> Option<SpanToken> {
        self.begin_at(name, category, Instant::now(), args)
    }

    /// 闭合令牌对应的 span。令牌不在栈上（例如已被 `set_level(Off)` 清掉）时返回
    /// `false`，不 panic、不补一条假 span。
    pub fn end_at(&mut self, token: SpanToken, now: Instant) -> bool {
        let Some(index) = self.open.iter().rposition(|span| span.token == token.0) else {
            return false;
        };
        // 父 span 先结束时，栈上更深的未闭合 span 只能丢弃：它们的起点已不可信。
        self.open.truncate(index + 1);
        let Some(open) = self.open.pop() else {
            return false;
        };
        let end_ns = self.relative_ns(now);
        let dur_ns = end_ns.saturating_sub(open.start_ns);
        self.push_record(
            open.name,
            open.category,
            open.start_ns,
            dur_ns,
            open.depth,
            open.args,
        );
        true
    }

    pub fn end(&mut self, token: SpanToken) -> bool {
        self.end_at(token, Instant::now())
    }

    /// 直接登记一个已完成 span（合成/桥接用，例如 `FrameScheduleReport` 只有耗时
    /// 没有开始时间，需要在父 span 里顺序铺开）。`start` 与 `SpanTrace` 起点比较。
    pub fn push_span_at(
        &mut self,
        name: &str,
        category: &'static str,
        start: Instant,
        elapsed: Duration,
        depth: u16,
        args: Vec<(&'static str, String)>,
    ) -> bool {
        let start_ns = self.relative_ns(start);
        self.push_span_ns(name, category, start_ns, nanos(elapsed), depth, args)
    }

    /// 以捕获起点的相对纳秒登记 span（测试与确定性证据用，避免依赖墙钟）。
    pub fn push_span_ns(
        &mut self,
        name: &str,
        category: &'static str,
        start_ns: u64,
        dur_ns: u64,
        depth: u16,
        args: Vec<(&'static str, String)>,
    ) -> bool {
        if !self.level.records_spans() {
            return false;
        }
        self.push_record(name.to_owned(), category, start_ns, dur_ns, depth, args);
        true
    }

    /// 同名（且同 category）聚合，按总耗时降序。分位数用 nearest-rank，与
    /// `frame_profile::TimingStats` / 编辑器 profiler 同一口径。
    #[must_use]
    pub fn top_n(&self, limit: usize) -> Vec<SpanStat> {
        let mut groups: BTreeMap<(String, &'static str), Vec<u64>> = BTreeMap::new();
        for span in &self.spans {
            groups
                .entry((span.name.clone(), span.category))
                .or_default()
                .push(span.dur_ns);
        }
        let mut stats = groups
            .into_iter()
            .map(|((name, category), mut samples)| {
                samples.sort_unstable();
                let at = |percent: usize| {
                    ((samples.len() * percent).saturating_sub(1) / 100).min(samples.len() - 1)
                };
                let total_ns = samples.iter().map(|value| u128::from(*value)).sum::<u128>();
                SpanStat {
                    name,
                    category,
                    count: samples.len(),
                    total_us: total_ns as f64 / 1000.0,
                    p50_us: samples[at(50)] as f64 / 1000.0,
                    p95_us: samples[at(95)] as f64 / 1000.0,
                    max_us: samples[samples.len() - 1] as f64 / 1000.0,
                }
            })
            .collect::<Vec<_>>();
        stats.sort_by(|left, right| {
            right
                .total_us
                .total_cmp(&left.total_us)
                .then_with(|| left.name.cmp(&right.name))
        });
        stats.truncate(limit);
        stats
    }

    /// 面板/复制报告用的文本 top-N。
    #[must_use]
    pub fn to_text(&self, top: usize) -> String {
        let summary = self.summary();
        let mut out = String::new();
        let _ = writeln!(
            out,
            "trace: level={} spans={}/{} dropped={} covered={:.3}ms",
            summary.level.as_str(),
            summary.spans,
            summary.capacity,
            summary.dropped,
            summary.covered_ns as f64 / 1_000_000.0
        );
        for stat in self.top_n(top) {
            let _ = writeln!(
                out,
                "  {:<28} total={:>8.3}ms p50={:>7.3}ms p95={:>7.3}ms max={:>7.3}ms n={}",
                stat.name,
                stat.total_us / 1000.0,
                stat.p50_us / 1000.0,
                stat.p95_us / 1000.0,
                stat.max_us / 1000.0,
                stat.count
            );
        }
        out
    }

    /// Chrome Trace Event JSON。`ts`/`dur` 是微秒（Chrome 约定），`pid`/`tid` 固定
    /// 1（当前只在主线程采集）；事件按开始时间排序，便于人读，也便于外部校验器
    /// 检查单调性。
    #[must_use]
    pub fn to_chrome_trace_json(&self) -> String {
        let mut ordered = self.spans.iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| {
            left.start_ns
                .cmp(&right.start_ns)
                .then_with(|| left.depth.cmp(&right.depth))
                .then_with(|| left.name.cmp(&right.name))
        });
        let mut out = String::with_capacity(ordered.len() * 160 + 256);
        out.push_str("{\"traceEvents\":[");
        for (index, span) in ordered.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"name\":\"{}\",\"cat\":\"{}\",\"ph\":\"X\",\"ts\":{:.3},\"dur\":{:.3},\
                 \"pid\":1,\"tid\":1,\"args\":{{\"depth\":{},\"seq\":{}",
                escape_json(&span.name),
                escape_json(span.category),
                span.start_ns as f64 / 1000.0,
                span.dur_ns as f64 / 1000.0,
                span.depth,
                index
            );
            for (key, value) in &span.args {
                let _ = write!(out, ",\"{}\":\"{}\"", escape_json(key), escape_json(value));
            }
            out.push_str("}}");
        }
        let summary = self.summary();
        let _ = write!(
            out,
            "],\"displayTimeUnit\":\"ms\",\"metadata\":{{\"source\":\"rustlua\",\
             \"report_version\":\"{}\",\"level\":\"{}\",\"span_count\":{},\
             \"dropped_spans\":{},\"capacity\":{}}}}}",
            TRACE_REPORT_VERSION,
            summary.level.as_str(),
            summary.spans,
            summary.dropped,
            summary.capacity
        );
        out
    }

    fn push_record(
        &mut self,
        name: String,
        category: &'static str,
        start_ns: u64,
        dur_ns: u64,
        depth: u16,
        args: Vec<(&'static str, String)>,
    ) {
        if self.spans.len() == self.capacity {
            self.spans.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.spans.push_back(SpanRecord {
            name,
            category,
            start_ns,
            dur_ns,
            depth,
            args,
        });
    }

    fn relative_ns(&self, instant: Instant) -> u64 {
        nanos(instant.saturating_duration_since(self.origin))
    }
}

/// 开启/关闭细粒度采集后的主线程 p50 相对开销（百分比）。基线为 0 时返回 0
/// （没有可比较的读数，不伪造无穷大）。
#[must_use]
pub fn overhead_pct(baseline_p50_us: f64, profiled_p50_us: f64) -> f64 {
    if baseline_p50_us <= 0.0 {
        0.0
    } else {
        (profiled_p50_us - baseline_p50_us) * 100.0 / baseline_p50_us
    }
}

/// docs/55 的硬判据：≤2% 才算在预算内（恰好 2.0 通过，与"不超过"一致）。
#[must_use]
pub fn overhead_within_budget(pct: f64) -> bool {
    pct <= TRACE_OVERHEAD_BUDGET_PCT
}

/// 超预算时降级为系统级（计划 §6.13「超过则默认关闭细粒度 scope」）。
#[must_use]
pub fn capture_level_for_overhead(pct: f64) -> CaptureLevel {
    if overhead_within_budget(pct) {
        CaptureLevel::Fine
    } else {
        CaptureLevel::System
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn escape_json(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        CaptureLevel, SpanTrace, TRACE_OVERHEAD_BUDGET_PCT, capture_level_for_overhead,
        overhead_pct, overhead_within_budget,
    };

    #[test]
    fn ring_is_bounded_and_counts_dropped() {
        let mut trace = SpanTrace::new(CaptureLevel::Fine, 4);
        for index in 0..10 {
            assert!(trace.push_span_ns(
                &format!("span{index}"),
                "test",
                index * 1000,
                500,
                0,
                Vec::new()
            ));
        }
        assert_eq!(trace.len(), 4);
        assert_eq!(trace.dropped(), 6);
        let json = trace.to_chrome_trace_json();
        assert!(json.contains("\"span9\""));
        assert!(json.contains("\"span6\""));
        assert!(!json.contains("\"span5\""), "最旧的 span 必须被淘汰");
    }

    /// 面板时间线的入口：`latest_records(count)` 给的是**被留下的那段尾部**里最近的
    /// `count` 条，顺序是捕获顺序而不是起点顺序（嵌套 span 的子总比父早闭合），字段一个
    /// 都不加工——排序、合成判定都归消费方。
    #[test]
    fn latest_records_gives_the_newest_tail_in_capture_order() {
        let mut trace = SpanTrace::new(CaptureLevel::System, 3);
        trace.push_span_ns("outer", "frame", 40_000, 20_000, 0, Vec::new());
        trace.push_span_ns(
            "inner",
            "script",
            10_000,
            5_000,
            1,
            vec![("synthetic", "true".to_owned())],
        );
        trace.push_span_ns("last", "system", 30_000, 1_000, 1, Vec::new());
        trace.push_span_ns("dropped", "command", 0, 1, 0, Vec::new());
        let all = trace.latest_records(8);
        assert_eq!(trace.dropped(), 1);
        assert_eq!(
            all.iter()
                .map(|span| span.name.as_str())
                .collect::<Vec<_>>(),
            ["inner", "last", "dropped"],
            "留下的必须是最新那段，且按捕获顺序"
        );
        assert_eq!(all[0].category, "script");
        assert_eq!(all[0].depth, 1);
        assert_eq!(all[0].args, vec![("synthetic", "true".to_owned())]);
        assert_eq!(all[1].start_ns, 30_000);
        assert_eq!(all[1].dur_ns, 1_000);
        assert_eq!(
            trace
                .latest_records(2)
                .iter()
                .map(|span| span.name.as_str())
                .collect::<Vec<_>>(),
            ["last", "dropped"],
            "按像素预算截断时丢的是更旧的那些"
        );
        assert!(trace.latest_records(0).is_empty());
        let mut off = SpanTrace::new(CaptureLevel::Off, 4);
        assert!(
            !off.push_span_ns("nope", "test", 0, 1, 0, Vec::new()),
            "Off 档登记必须失败"
        );
        assert!(off.latest_records(8).is_empty());
    }

    #[test]
    fn spans_keep_every_field_in_the_export() {
        let mut trace = SpanTrace::new(CaptureLevel::Fine, 16);
        trace.push_span_ns(
            "physics2d_step",
            "system",
            120_000,
            75_500,
            2,
            vec![
                ("stage", "Physics".to_owned()),
                ("synthetic", "true".to_owned()),
            ],
        );
        let json = trace.to_chrome_trace_json();
        assert!(json.contains("\"name\":\"physics2d_step\""));
        assert!(json.contains("\"cat\":\"system\""));
        assert!(json.contains("\"ph\":\"X\""));
        assert!(json.contains("\"ts\":120.000"));
        assert!(json.contains("\"dur\":75.500"));
        assert!(json.contains("\"pid\":1"));
        assert!(json.contains("\"tid\":1"));
        assert!(json.contains("\"depth\":2"));
        assert!(json.contains("\"stage\":\"Physics\""));
        assert!(json.contains("\"synthetic\":\"true\""));
        assert!(json.contains("\"report_version\":\"p3-profile-trace-v1\""));
        // 顶层必须是合法 JSON 结构（serde_json 是 dev-dependency）。
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("trace 必须是合法 JSON");
        let events = parsed["traceEvents"].as_array().expect("traceEvents 数组");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["ph"], "X");
        assert!(events[0]["ts"].is_number());
        assert!(events[0]["dur"].is_number());
        assert!(events[0]["pid"].is_number());
        assert!(events[0]["tid"].is_number());
        assert_eq!(events[0]["args"]["depth"], 2);
        assert_eq!(parsed["metadata"]["level"], "fine");
    }

    #[test]
    fn begin_end_nests_spans_by_depth() {
        let mut trace = SpanTrace::new(CaptureLevel::Fine, 16);
        let start = Instant::now();
        let outer = trace
            .begin_at("frame", "editor", start, Vec::new())
            .expect("span 开启");
        let inner = trace
            .begin_at(
                "panel",
                "panel",
                start + Duration::from_micros(1),
                Vec::new(),
            )
            .expect("嵌套 span 开启");
        assert!(trace.end_at(inner, start + Duration::from_micros(2)));
        assert!(trace.end_at(outer, start + Duration::from_micros(3)));
        assert_eq!(trace.len(), 2);
        let stats = trace.top_n(4);
        let frame = stats
            .iter()
            .find(|stat| stat.name == "frame")
            .expect("frame");
        let panel = stats
            .iter()
            .find(|stat| stat.name == "panel")
            .expect("panel");
        assert!((frame.p50_us - 3.0).abs() < 0.001);
        assert!((panel.p50_us - 1.0).abs() < 0.001);
        let depths = trace
            .spans
            .iter()
            .map(|span| (span.name.as_str(), span.depth))
            .collect::<Vec<_>>();
        assert!(depths.contains(&("frame", 0)));
        assert!(depths.contains(&("panel", 1)));
        // 已闭合的令牌不能重复闭合，也不能 panic。
        assert!(!trace.end_at(inner, start));
    }

    #[test]
    fn top_n_aggregates_with_nearest_rank() {
        let mut trace = SpanTrace::new(CaptureLevel::Fine, 256);
        for index in 0..100u64 {
            trace.push_span_ns(
                "script",
                "script",
                index * 1000,
                (index + 1) * 1000,
                1,
                Vec::new(),
            );
        }
        trace.push_span_ns("extract2d", "extract", 0, 10_000, 1, Vec::new());
        let stats = trace.top_n(2);
        assert_eq!(stats[0].name, "script");
        assert_eq!(stats[0].count, 100);
        assert!((stats[0].total_us - 5050.0).abs() < 0.001);
        assert!((stats[0].p50_us - 50.0).abs() < 0.001);
        assert!((stats[0].p95_us - 95.0).abs() < 0.001);
        assert!((stats[0].max_us - 100.0).abs() < 0.001);
        assert_eq!(stats[1].name, "extract2d");
        assert_eq!(trace.top_n(1).len(), 1);
    }

    #[test]
    fn off_level_records_nothing_and_clear_resets() {
        let mut trace = SpanTrace::new(CaptureLevel::Off, 8);
        assert!(
            !trace.push_span_ns("hidden", "test", 0, 1000, 0, Vec::new()),
            "Off 档不得记录"
        );
        assert!(trace.begin("hidden", "test", Vec::new()).is_none());
        trace.set_level(CaptureLevel::Fine);
        trace.push_span_ns("shown", "test", 0, 1000, 0, Vec::new());
        assert_eq!(trace.len(), 1);
        trace.clear();
        assert!(trace.is_empty());
        assert_eq!(trace.dropped(), 0);
    }

    #[test]
    fn overhead_gate_uses_two_percent_boundary() {
        assert!((overhead_pct(1000.0, 1015.0) - 1.5).abs() < 1e-9);
        assert_eq!(overhead_pct(0.0, 10.0), 0.0);
        assert!(overhead_within_budget(TRACE_OVERHEAD_BUDGET_PCT));
        assert!(!overhead_within_budget(TRACE_OVERHEAD_BUDGET_PCT + 0.001));
        assert_eq!(
            capture_level_for_overhead(2.0),
            CaptureLevel::Fine,
            "恰好 2% 仍在预算内"
        );
        assert_eq!(capture_level_for_overhead(2.1), CaptureLevel::System);
    }

    #[test]
    fn json_escapes_names_and_args() {
        let mut trace = SpanTrace::new(CaptureLevel::Fine, 4);
        trace.push_span_ns(
            "quote\"name",
            "test",
            0,
            1000,
            0,
            vec![("path", "a\\b\nc".to_owned())],
        );
        let json = trace.to_chrome_trace_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("转义后仍是合法 JSON");
        assert_eq!(parsed["traceEvents"][0]["name"], "quote\"name");
        assert_eq!(parsed["traceEvents"][0]["args"]["path"], "a\\b\nc");
    }
}
