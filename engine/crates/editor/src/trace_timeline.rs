//! 性能面板的 span 时间线：把 `SpanTrace` 的记录画成"泳道 = 嵌套深度、横轴 = 时间"的条形。
//!
//! 导出 Chrome Trace 早就有了（docs/68 §6.4），但"面板里看一眼这一帧谁套在谁里面"仍然要
//! 离开编辑器。这一层补的是那一段：**几何全是纯函数**（进纳秒、出像素），面板只负责把
//! `egui::Ui` 递进来。于是"哪条 span 落在哪个像素、哪条不是真实宽度、刻度为什么这么密"
//! 都有门可钉，而不是只有导出那条路被证明过。
//!
//! 刻意不做的事：不做缩放与拖拽（要钉的是口径，不是交互手感）、不做真实 flame 树
//! （`SpanTrace` 只有 `depth`，没有父子 id）、调度 system 那一段本来就是**合成**时间线
//! （见 `SpanRecord.args` 里的 `synthetic=true`），条形会画暗一档并写进悬停说明。

use eframe::egui;
use engine::host::profile_trace::SpanRecord;

/// 一次时间线能画的**最多**条形数。`SpanTrace` 容量是 8192，逐条画矩形会把面板自己变成
/// 热点；这里只画最近的那一段（尾部），其余计进 [`Timeline::dropped`] 并写在面板上。
pub(crate) const MAX_BARS: usize = 512;

/// 刻度标签占的那一行高度（像素）。
pub(crate) const TICK_LABEL_HEIGHT: f32 = 12.0;
/// 相邻刻度至少隔这么宽，否则宁可不画：刻度密到每像素一根与没有刻度一样没用。
pub(crate) const TICK_MIN_SPACING_PX: f32 = 56.0;
/// 时间线底色（门的认领手段之一：按颜色把画出来的矩形归给背景）。
pub(crate) const BACKGROUND: egui::Color32 = egui::Color32::from_rgb(18, 18, 21);
/// 被抬到最小宽度的那条用这个描边——它**不按比例**，必须让读者看出来。
pub(crate) const WIDENED_STROKE: egui::Color32 = egui::Color32::from_rgb(0xe6, 0x4a, 0x4a);
/// 合成 span 的暗化系数（255 分制的整数运算，不依赖 `Color32` 的浮点乘法）。
const SYNTHETIC_DIM: u16 = 184;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TimelineOptions {
    /// 可用宽度（像素）。
    pub width: f32,
    pub row_height: f32,
    pub row_gap: f32,
    /// 再窄也至少画这么宽，否则 1 µs 的 span 在 16 ms 窗口里只有 0.05 px，根本看不见。
    pub min_bar_px: f32,
    pub max_bars: usize,
}

impl TimelineOptions {
    #[must_use]
    pub(crate) fn new(width: f32) -> Self {
        Self {
            width,
            row_height: 12.0,
            row_gap: 3.0,
            min_bar_px: 2.0,
            max_bars: MAX_BARS,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineBar {
    /// 相对时间线原点的像素矩形。
    pub rect: [f32; 4],
    pub name: String,
    pub category: &'static str,
    pub depth: u16,
    pub start_ns: u64,
    pub dur_ns: u64,
    /// 调度 system 那一段是帧末合成的顺序 span，不是真实起止时刻。
    pub synthetic: bool,
    /// 真实宽度不足 `min_bar_px`，被抬到下限：这根**不按比例**，画描边。
    pub widened: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Timeline {
    pub bars: Vec<TimelineBar>,
    /// 横轴窗口（纳秒）：保留下来的那些 span 里最晚的结束时间，至少 1。
    pub window_ns: u64,
    /// 泳道数 = 最大深度 + 1。
    pub lanes: usize,
    /// 因为 `max_bars` 没画的条数（画的是**尾部**，也就是最近那一段）。
    pub dropped: usize,
    /// 布局用的宽度，`paint` 按它原样落笔。
    pub width: f32,
    /// 条形区高度（不含刻度那一行）。
    pub height: f32,
}

/// 记录 → 像素。排序与 Chrome Trace 导出**同一条**（起点、深度、名字），于是面板里看到的
/// 顺序与 Perfetto 里对得上。
#[must_use]
pub(crate) fn layout(records: &[SpanRecord], options: &TimelineOptions) -> Timeline {
    if records.is_empty() || !(options.width.is_finite()) || options.width <= 0.0 {
        return Timeline {
            width: options.width.max(0.0),
            ..Default::default()
        };
    }
    let mut ordered = records.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.start_ns
            .cmp(&right.start_ns)
            .then_with(|| left.depth.cmp(&right.depth))
            .then_with(|| left.name.cmp(&right.name))
    });
    let dropped = ordered.len().saturating_sub(options.max_bars);
    let kept = &ordered[dropped..];
    let window_ns = kept
        .iter()
        .map(|span| span.start_ns.saturating_add(span.dur_ns))
        .max()
        .unwrap_or(0)
        .max(1);
    let per_ns = options.width / window_ns as f32;
    let mut bars = Vec::with_capacity(kept.len());
    let mut lanes = 1usize;
    for span in kept {
        let lane = usize::from(span.depth);
        lanes = lanes.max(lane + 1);
        let x = span.start_ns as f32 * per_ns;
        let raw = span.dur_ns as f32 * per_ns;
        let y = lane as f32 * (options.row_height + options.row_gap);
        // 下限会把条推出右边界；贴边的记录宁可少给宽度，也不画到框外。
        let width = raw
            .max(options.min_bar_px)
            .min((options.width - x).max(0.0));
        if width <= 0.0 {
            continue;
        }
        bars.push(TimelineBar {
            rect: [x, y, width, options.row_height],
            name: span.name.clone(),
            category: span.category,
            depth: span.depth,
            start_ns: span.start_ns,
            dur_ns: span.dur_ns,
            synthetic: is_synthetic(span),
            widened: width > raw + 1e-6,
        });
    }
    let height = (lanes as f32) * (options.row_height + options.row_gap) - options.row_gap;
    Timeline {
        bars,
        window_ns,
        lanes,
        dropped,
        width: options.width,
        height,
    }
}

fn is_synthetic(span: &SpanRecord) -> bool {
    span.args
        .iter()
        .any(|(key, value)| *key == "synthetic" && value == "true")
}

/// 悬停在哪一条上：后画的（同一泳道里更晚开始的）赢。
#[must_use]
pub(crate) fn bar_at(timeline: &Timeline, point: [f32; 2]) -> Option<&TimelineBar> {
    timeline.bars.iter().rev().find(|bar| {
        let [x, y, w, h] = bar.rect;
        point[0] >= x && point[0] <= x + w && point[1] >= y && point[1] <= y + h
    })
}

/// [`bar_at`] 的面板版：输入是**分配矩形**内的坐标（鼠标相对矩形左上角），
/// 减掉刻度那一行才是泳道坐标。面板不必自己记这个偏移。
#[must_use]
pub(crate) fn hover_at(timeline: &Timeline, point: [f32; 2]) -> Option<&TimelineBar> {
    bar_at(timeline, [point[0], point[1] - TICK_LABEL_HEIGHT])
}

/// 一条 bar 的最终底色（合成档暗一档）。门按这个颜色把矩形认领回 bar。
#[must_use]
pub(crate) fn bar_fill(bar: &TimelineBar) -> egui::Color32 {
    let base = category_color(bar.category);
    if !bar.synthetic {
        return base;
    }
    egui::Color32::from_rgb(
        (u16::from(base.r()) * SYNTHETIC_DIM / 255) as u8,
        (u16::from(base.g()) * SYNTHETIC_DIM / 255) as u8,
        (u16::from(base.b()) * SYNTHETIC_DIM / 255) as u8,
    )
}

/// 画一条时间线：背景、刻度竖线与标签、条形。**按布局时的像素原样落笔**——面板每帧用
/// 当帧的 `available_width` 重算 [`layout`]，所以两者总是相等；宽度变了必须先重算。
/// 宽度不可用（零宽 / NaN）时什么都不画，返回 `None`。
pub(crate) fn paint(ui: &mut egui::Ui, timeline: &Timeline) -> Option<egui::Response> {
    if !(timeline.width.is_finite()) || timeline.width <= 0.0 || !(timeline.height.is_finite()) {
        return None;
    }
    let size = egui::vec2(timeline.width, timeline.height + TICK_LABEL_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, BACKGROUND);
    let plot = egui::Rect::from_min_size(
        egui::pos2(rect.min.x, rect.min.y + TICK_LABEL_HEIGHT),
        egui::vec2(timeline.width, timeline.height),
    );
    for (x, label) in axis_ticks(timeline.window_ns, timeline.width, TICK_MIN_SPACING_PX) {
        let x = plot.min.x + x;
        painter.line_segment(
            [egui::pos2(x, plot.min.y), egui::pos2(x, plot.max.y)],
            egui::Stroke::new(1.0_f32, egui::Color32::from_gray(46)),
        );
        painter.text(
            egui::pos2(x + 2.0, rect.min.y + 1.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(9.0),
            egui::Color32::from_gray(150),
        );
    }
    for bar in &timeline.bars {
        let rect = egui::Rect::from_min_size(
            egui::pos2(plot.min.x + bar.rect[0], plot.min.y + bar.rect[1]),
            egui::vec2(bar.rect[2], bar.rect[3]),
        );
        painter.rect_filled(rect, 1.0, bar_fill(bar));
        if bar.widened {
            painter.rect_stroke(
                rect,
                1.0,
                egui::Stroke::new(1.0_f32, WIDENED_STROKE),
                egui::StrokeKind::Inside,
            );
        }
    }
    Some(response)
}

/// 横轴刻度：`1,2,5,10,20,50,…` 纳秒里第一个让相邻刻度隔至少 `min_spacing_px` 的档位。
#[must_use]
pub(crate) fn axis_ticks(window_ns: u64, width: f32, min_spacing_px: f32) -> Vec<(f32, String)> {
    if window_ns == 0 || !(width.is_finite()) || width <= 0.0 {
        return Vec::new();
    }
    let Some(step_ns) = tick_step_ns(window_ns, width, min_spacing_px) else {
        return Vec::new();
    };
    let per_ns = width / window_ns as f32;
    let mut out = Vec::new();
    let mut at = 0u64;
    while at <= window_ns {
        out.push((at as f32 * per_ns, format_duration_ns(at)));
        at = match at.checked_add(step_ns) {
            Some(next) if next > at => next,
            _ => break,
        };
    }
    out
}

fn tick_step_ns(window_ns: u64, width: f32, min_spacing_px: f32) -> Option<u64> {
    for exponent in 0..13u32 {
        let power = 10u64.checked_pow(exponent)?;
        for mantissa in [1u64, 2, 5] {
            let step = mantissa.checked_mul(power)?;
            if step == 0 || window_ns / step < 1 {
                continue;
            }
            if f64::from(width) * step as f64 / window_ns as f64 >= f64::from(min_spacing_px) {
                return Some(step);
            }
        }
    }
    None
}

#[must_use]
pub(crate) fn format_duration_ns(ns: u64) -> String {
    if ns == 0 {
        return "0".to_owned();
    }
    let ms = ns as f64 / 1_000_000.0;
    let mut text = format!("{ms:.2}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text.push_str(" ms");
    text
}

#[must_use]
pub(crate) fn category_color(category: &str) -> egui::Color32 {
    match category {
        "frame" => egui::Color32::from_rgb(0x5b, 0x8f, 0xd0),
        "script" => egui::Color32::from_rgb(0xd0, 0x8a, 0x3f),
        "system" => egui::Color32::from_rgb(0x5f, 0xa8, 0x6b),
        "extract" => egui::Color32::from_rgb(0x9a, 0x6f, 0xc2),
        "panel" => egui::Color32::from_rgb(0xc2, 0x5f, 0x7a),
        "command" => egui::Color32::from_rgb(0x4f, 0xb3, 0xb0),
        _ => egui::Color32::from_rgb(0x8a, 0x8a, 0x8a),
    }
}

/// 悬停到的那条怎么说（面板直接把这行写在时间线下面，省一套 tooltip 机制）。
#[must_use]
pub(crate) fn bar_hint(bar: &TimelineBar) -> String {
    let mut note = String::new();
    if bar.synthetic {
        note.push_str(" · 合成");
    }
    if bar.widened {
        note.push_str(" · 不足最小宽度");
    }
    format!(
        "{} · {} · {:.3} ms · 深度 {}{note}",
        bar.name,
        bar.category,
        bar.dur_ns as f64 / 1_000_000.0,
        bar.depth
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    /// 像素判据：纳秒→像素走的是 f32 乘法，尾数误差在 1e-5 量级；真正的错位（错泳道、
    /// 错比例、用了头一段而不是尾一段）都是几个像素以上的量级，所以 0.01 的容差不放水。
    fn near(actual: f32, expected: f32, tag: &str) {
        assert!(
            (actual - expected).abs() < 0.01,
            "{tag}：期望 {expected}，实际 {actual}"
        );
    }

    fn near_rect(actual: [f32; 4], expected: [f32; 4], tag: &str) {
        for (index, value) in expected.iter().enumerate() {
            near(actual[index], *value, &format!("{tag}[{index}]"));
        }
    }

    fn span(
        name: &str,
        category: &'static str,
        start_ns: u64,
        dur_ns: u64,
        depth: u16,
    ) -> SpanRecord {
        SpanRecord {
            name: name.to_owned(),
            category,
            start_ns,
            dur_ns,
            depth,
            args: Vec::new(),
        }
    }

    #[test]
    fn bars_map_nanoseconds_onto_the_window_in_pixels() {
        // 窗口 = 最晚结束 = 16 ms；宽 800 px ⇒ 50 px/ms；泳道高 12 + 间隔 3。
        let records = vec![
            span("frame", "frame", 0, 16 * MS, 0),
            span("lua", "script", 2 * MS, 4 * MS, 1),
            span("extract", "extract", 6 * MS, 500_000, 2),
        ];
        let timeline = layout(&records, &TimelineOptions::new(800.0));
        assert_eq!(timeline.window_ns, 16 * MS);
        assert_eq!(timeline.lanes, 3);
        assert_eq!(timeline.dropped, 0);
        near_rect(timeline.bars[0].rect, [0.0, 0.0, 800.0, 12.0], "frame");
        near_rect(timeline.bars[1].rect, [100.0, 15.0, 200.0, 12.0], "lua");
        // 0.5 ms 在 50 px/ms 下是 25 px，仍大于下限，所以按真实宽度画。
        near_rect(timeline.bars[2].rect, [300.0, 30.0, 25.0, 12.0], "extract");
        assert!(!timeline.bars[2].widened);
        // 总高 = 3 泳道 × (12 + 3) − 3。
        near(timeline.height, 42.0, "height");
    }

    #[test]
    fn a_span_too_thin_to_see_is_widened_and_says_so() {
        let records = vec![
            span("frame", "frame", 0, 16 * MS, 0),
            span("tick", "system", 8 * MS, 1_000, 1),
        ];
        let timeline = layout(&records, &TimelineOptions::new(800.0));
        let tick = &timeline.bars[1];
        assert_eq!(tick.dur_ns, 1_000);
        assert_eq!(tick.rect[2], 2.0, "1 µs 只有 0.05 px，必须抬到下限");
        assert!(tick.widened, "被抬宽的必须标出来，否则读者会拿它当真实耗时");
        assert!(!timeline.bars[0].widened);
        assert!(
            bar_hint(tick).contains("不足最小宽度"),
            "{}",
            bar_hint(tick)
        );
    }

    #[test]
    fn depth_is_the_lane_and_a_tail_bar_clamps_instead_of_leaking_out() {
        let records = vec![
            span("frame", "frame", 0, 10 * MS, 0),
            span("deep", "panel", 5 * MS, 5 * MS, 3),
            // 零时长且正好贴在窗口右端：给不出宽度，只能不画。
            span("zero", "command", 10 * MS, 0, 1),
        ];
        let timeline = layout(&records, &TimelineOptions::new(400.0));
        assert_eq!(timeline.window_ns, 10 * MS);
        assert_eq!(timeline.lanes, 4);
        near_rect(timeline.bars[1].rect, [200.0, 45.0, 200.0, 12.0], "deep");
        assert_eq!(
            timeline
                .bars
                .iter()
                .filter(|bar| bar.name == "zero")
                .count(),
            0,
            "贴边的零时长记录不该画出一条 2 px 的假条形"
        );
        // 贴右端的窄条：下限被右边可用宽度夹住，于是它不再是"按比例"的，但也不出界。
        let timeline = layout(
            &[
                span("frame", "frame", 0, 16 * MS, 0),
                span("tail", "command", 16 * MS - 1_000, 1_000, 1),
            ],
            &TimelineOptions::new(400.0),
        );
        let tail = &timeline.bars[1];
        assert!(tail.rect[0] + tail.rect[2] <= 400.0 + 1e-3, "{tail:?}");
        assert!(tail.widened, "{tail:?}");
    }

    #[test]
    fn only_the_newest_tail_is_drawn_once_it_exceeds_the_budget() {
        let records = (0..600)
            .map(|index| {
                span(
                    &format!("s{index:03}"),
                    "system",
                    index * MS,
                    MS,
                    (index % 2) as u16,
                )
            })
            .collect::<Vec<_>>();
        let timeline = layout(&records, &TimelineOptions::new(800.0));
        assert_eq!(timeline.dropped, 88);
        assert_eq!(timeline.bars.len(), MAX_BARS);
        assert_eq!(timeline.bars[0].name, "s088", "画的必须是最近那一段");
        assert_eq!(timeline.bars[511].name, "s599");
        assert_eq!(timeline.window_ns, 600 * MS);
    }

    #[test]
    fn synthetic_schedule_spans_are_flagged_from_their_args() {
        let mut real = span("render", "system", 0, 4 * MS, 1);
        let mut fake = span("physics", "system", 4 * MS, 2 * MS, 1);
        fake.args.push(("synthetic", "true".to_owned()));
        let timeline = layout(&[real.clone(), fake], &TimelineOptions::new(400.0));
        assert!(!timeline.bars[0].synthetic);
        assert!(timeline.bars[1].synthetic);
        assert!(bar_hint(&timeline.bars[1]).contains("合成"));
        real.args.push(("stage", "update".to_owned()));
        let timeline = layout(&[real], &TimelineOptions::new(400.0));
        assert!(!timeline.bars[0].synthetic, "别的 args 不能被当成合成标记");
    }

    #[test]
    fn hover_picks_the_last_drawn_bar_covering_the_point() {
        let records = vec![
            span("frame", "frame", 0, 16 * MS, 0),
            span("lua", "script", 2 * MS, 4 * MS, 1),
        ];
        let timeline = layout(&records, &TimelineOptions::new(800.0));
        assert_eq!(
            bar_at(&timeline, [400.0, 6.0]).map(|bar| bar.name.as_str()),
            Some("frame")
        );
        assert_eq!(
            bar_at(&timeline, [150.0, 21.0]).map(|bar| bar.name.as_str()),
            Some("lua")
        );
        assert_eq!(
            bar_at(&timeline, [150.0, 40.0]),
            None,
            "泳道之间的空隙不该命中"
        );
        assert_eq!(bar_at(&timeline, [900.0, 6.0]), None);
        // 面板拿到的是"相对分配矩形"的鼠标点：刻度那一行要先减掉才对得上泳道。
        let frame = &timeline.bars[0];
        assert_eq!(
            hover_at(&timeline, [frame.rect[0] + 1.0, TICK_LABEL_HEIGHT + 6.0])
                .map(|bar| bar.name.as_str()),
            Some("frame")
        );
        assert_eq!(
            hover_at(&timeline, [150.0, TICK_LABEL_HEIGHT + 21.0]).map(|bar| bar.name.as_str()),
            Some("lua")
        );
        assert_eq!(
            hover_at(&timeline, [150.0, 6.0]),
            None,
            "指针还在刻度行上，不该命中任何一条"
        );
    }

    #[test]
    fn axis_ticks_pick_a_one_two_five_step_that_stays_readable() {
        // 16 ms / 800 px：1 ms 一档只有 50 px（< 56）不够宽，必须跳到 2 ms ⇒ 100 px。
        let ticks = axis_ticks(16 * MS, 800.0, TICK_MIN_SPACING_PX);
        assert_eq!(ticks.len(), 9, "{ticks:?}");
        assert_eq!(ticks[0].1, "0");
        near(ticks[0].0, 0.0, "刻度 0 的 x");
        assert_eq!(ticks[1].1, "2 ms");
        near(ticks[1].0, 100.0, "刻度 2 ms 的 x");
        assert_eq!(ticks[8].1, "16 ms");
        near(ticks[8].0, 800.0, "最右刻度的 x");
        for pair in ticks.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= 56.0 - 1e-2, "{ticks:?}");
        }
        // 亚毫秒窗口：900 µs / 450 px ⇒ 0.1 ms 一档正好 50 px ≥ 40。
        let ticks = axis_ticks(900_000, 450.0, 40.0);
        assert_eq!(ticks[1].1, "0.1 ms", "{ticks:?}");
        near(ticks[1].0, 50.0, "亚毫秒刻度的 x");
        assert!(axis_ticks(0, 400.0, 56.0).is_empty());
        assert!(axis_ticks(16 * MS, 0.0, 56.0).is_empty());
        assert!(axis_ticks(16 * MS, f32::NAN, 56.0).is_empty());
    }

    #[test]
    fn an_empty_or_broken_window_draws_nothing_instead_of_dividing_by_zero() {
        let empty = layout(&[], &TimelineOptions::new(800.0));
        assert!(empty.bars.is_empty());
        assert_eq!((empty.lanes, empty.dropped, empty.window_ns), (0, 0, 0));
        near(empty.width, 800.0, "空捕获也保留当帧宽度");
        // 宽度不可用时归零，这样面板拿到的就是一个明确的"什么都没有"。
        assert_eq!(layout(&[], &TimelineOptions::new(0.0)), Timeline::default());
        let timeline = layout(
            &[span("frame", "frame", 0, 0, 0)],
            &TimelineOptions::new(800.0),
        );
        assert_eq!(timeline.window_ns, 1, "零时长记录不能把窗口压成 0");
        assert_eq!(timeline.bars.len(), 1);
        assert!(
            layout(&[span("a", "frame", 0, MS, 0)], &TimelineOptions::new(0.0))
                .bars
                .is_empty()
        );
        let nan = TimelineOptions {
            width: f32::NAN,
            ..TimelineOptions::new(800.0)
        };
        assert!(
            layout(&[span("a", "frame", 0, MS, 0)], &nan)
                .bars
                .is_empty()
        );
    }

    #[test]
    fn the_paint_emits_one_rectangle_per_bar_at_the_laid_out_pixels() {
        let records = vec![
            span("frame", "frame", 0, 24 * MS, 0),
            span("lua", "script", 2 * MS, 4 * MS, 1),
            span("late", "panel", 15 * MS, 9 * MS, 2),
            {
                let mut tick = span("tick", "system", 20 * MS, 1_000, 3);
                tick.args.push(("synthetic", "true".to_owned()));
                tick
            },
        ];
        let timeline = layout(&records, &TimelineOptions::new(800.0));
        assert_eq!(timeline.bars.len(), 4);
        let output = egui::Context::default().run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 200.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    assert!(paint(ui, &timeline).is_some());
                });
            },
        );
        let filled = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(shape) if shape.stroke.is_empty() => {
                    Some((shape.fill, shape.rect))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        // 认领靠底色：夹具里四种 category 的底色互不相同，也不等于背景色。
        let drawn_of = |fill: egui::Color32, tag: &str| -> egui::Rect {
            let hits = filled
                .iter()
                .filter(|(shape_fill, _)| *shape_fill == fill)
                .map(|(_, rect)| *rect)
                .collect::<Vec<_>>();
            assert_eq!(hits.len(), 1, "{tag} 的底色应当只被画一次：{hits:?}");
            hits[0]
        };
        let background = drawn_of(BACKGROUND, "背景");
        near(background.width(), timeline.width, "背景宽");
        near(
            background.height(),
            timeline.height + TICK_LABEL_HEIGHT,
            "背景高（含刻度行）",
        );
        for bar in &timeline.bars {
            let drawn = drawn_of(bar_fill(bar), &bar.name);
            near(
                drawn.min.x - background.min.x,
                bar.rect[0],
                &format!("{} 的 x", bar.name),
            );
            near(
                drawn.min.y - background.min.y - TICK_LABEL_HEIGHT,
                bar.rect[1],
                &format!("{} 的泳道 y", bar.name),
            );
            near(drawn.width(), bar.rect[2], &format!("{} 的宽", bar.name));
            near(drawn.height(), bar.rect[3], &format!("{} 的高", bar.name));
        }
        // 被抬宽的那条必须有红色描边，否则"画到最小宽度"与"画到真实宽度"看不出区别。
        let stroked = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(shape) if shape.stroke.color == WIDENED_STROKE => {
                    Some(shape.rect)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(stroked.len(), 1, "{stroked:?}");
        // 刻度标签确实进了形状表（窗口 24 ms ⇒ 2 ms 一档）。
        assert!(
            output.shapes.iter().any(|clipped| matches!(
                &clipped.shape,
                egui::Shape::Text(text) if text.galley.text() == "2 ms"
            )),
            "刻度标签必须画出来"
        );
    }

    #[test]
    fn paint_draws_nothing_when_the_width_is_unusable() {
        for width in [0.0, -8.0, f32::NAN] {
            let timeline = Timeline {
                bars: vec![TimelineBar {
                    rect: [0.0, 0.0, width, 12.0],
                    name: "frame".to_owned(),
                    category: "frame",
                    depth: 0,
                    start_ns: 0,
                    dur_ns: MS,
                    synthetic: false,
                    widened: false,
                }],
                window_ns: MS,
                lanes: 1,
                dropped: 0,
                width,
                height: 12.0,
            };
            let output = egui::Context::default().run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 200.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        assert!(paint(ui, &timeline).is_none(), "{width} 宽度不该分配矩形");
                    });
                },
            );
            assert!(
                !output.shapes.iter().any(|clipped| matches!(
                    &clipped.shape,
                    egui::Shape::Rect(shape) if shape.fill == BACKGROUND
                )),
                "{width} 宽度仍然画出了时间线背景"
            );
        }
    }
}
