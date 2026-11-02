use super::*;

/// 趋势图单格高度（点）。
pub(crate) const TREND_HEIGHT: f32 = 52.0;
/// 趋势图两列之间的间距（点）。
pub(crate) const TREND_GAP: f32 = 8.0;

/// 运行时趋势：把最近 240 帧画成 2 列折线图（帧率 / 帧间隔 / CPU/GPU / draw / 顶点面数 / 内存）。
///
/// 用 `egui::Painter` 直接画折线，不引入 `egui_plot` 依赖；每格只保留最近值与
/// 最小/最大刻度，趋势细节靠折线形状表达。
pub(crate) fn draw_runtime_trends(ui: &mut egui::Ui, history: &RuntimePerfHistory) {
    if history.is_empty() {
        ui.weak("（开始播放后记录；窗口容量 240 帧）");
        return;
    }
    ui.weak(format!(
        "最近 {} / {} 帧（旧样本自动淘汰）",
        history.len(),
        history.capacity()
    ));
    let width = ((ui.available_width() - TREND_GAP) * 0.5).max(120.0);
    let series = [
        RuntimeSeries::Fps,
        RuntimeSeries::FrameMs,
        RuntimeSeries::RenderCpuMs,
        RuntimeSeries::RenderGpuMs,
        RuntimeSeries::DrawCalls,
        RuntimeSeries::SetPassCalls,
        RuntimeSeries::Triangles,
        RuntimeSeries::Vertices,
        RuntimeSeries::MemoryMib,
    ];
    egui::Grid::new("perf_runtime_trends")
        .num_columns(2)
        .spacing([TREND_GAP, 6.0])
        .show(ui, |ui| {
            for (index, series) in series.into_iter().enumerate() {
                let values = history.series(series);
                if !values.is_empty() {
                    // E06-4：帧间隔图叠加尖峰标记（>20 ms，即跌破 60 FPS 预算）。
                    let spike_over = (series == RuntimeSeries::FrameMs).then_some(20.0);
                    draw_sparkline(
                        ui,
                        series.label(),
                        &values,
                        series_color(series),
                        width,
                        spike_over,
                    );
                }
                if index % 2 == 1 {
                    ui.end_row();
                }
            }
        });
}

/// 一格折线图：背景 + 中线 + 折线 + 左上标签 + 右上最近值；可选尖峰竖线标记。
pub(crate) fn draw_sparkline(
    ui: &mut egui::Ui,
    label: &str,
    values: &[f32],
    color: egui::Color32,
    width: f32,
    spike_over: Option<f32>,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, TREND_HEIGHT), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let inner = rect.shrink2(egui::vec2(4.0, 14.0));
    painter.line_segment(
        [
            egui::pos2(inner.left(), inner.center().y),
            egui::pos2(inner.right(), inner.center().y),
        ],
        egui::Stroke::new(1.0_f32, ui.visuals().weak_text_color().gamma_multiply(0.25)),
    );
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for value in values {
        if value.is_finite() {
            min = min.min(*value);
            max = max.max(*value);
        }
    }
    if !min.is_finite() || !max.is_finite() {
        return;
    }
    if (max - min).abs() <= f32::EPSILON {
        // 一条水平线也要占满中间高度，不能贴边。
        let pad = max.abs().max(1.0) * 0.05;
        min -= pad;
        max += pad;
    }
    let span = (max - min).max(f32::EPSILON);
    let points: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .filter(|(_, value)| value.is_finite())
        .map(|(index, value)| {
            let x = if values.len() <= 1 {
                inner.center().x
            } else {
                inner.left() + inner.width() * index as f32 / (values.len() - 1) as f32
            };
            let y = inner.bottom() - (*value - min) / span * inner.height();
            egui::pos2(x, y)
        })
        .collect();
    if points.len() >= 2 {
        painter.add(egui::Shape::line(points, egui::Stroke::new(1.4_f32, color)));
    } else if let Some(point) = points.first() {
        painter.circle_filled(*point, 2.0, color);
    }
    if let Some(threshold) = spike_over {
        let spike_color = egui::Color32::from_rgb(240, 120, 90);
        let mut spikes = 0usize;
        for (index, value) in values.iter().enumerate() {
            if !value.is_finite() || *value <= threshold {
                continue;
            }
            spikes += 1;
            let x = if values.len() <= 1 {
                inner.center().x
            } else {
                inner.left() + inner.width() * index as f32 / (values.len() - 1) as f32
            };
            painter.line_segment(
                [egui::pos2(x, inner.top()), egui::pos2(x, inner.bottom())],
                egui::Stroke::new(1.0_f32, spike_color.gamma_multiply(0.6)),
            );
        }
        if spikes > 0 {
            painter.text(
                rect.left_top() + egui::vec2(4.0, 12.0),
                egui::Align2::LEFT_TOP,
                format!("尖峰 {spikes}"),
                egui::FontId::monospace(9.0),
                spike_color,
            );
        }
    }
    painter.text(
        rect.left_top() + egui::vec2(4.0, 1.0),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::proportional(10.0),
        ui.visuals().weak_text_color(),
    );
    painter.text(
        rect.right_top() + egui::vec2(-4.0, 1.0),
        egui::Align2::RIGHT_TOP,
        format!("{:.2}", values.last().copied().unwrap_or(0.0)),
        egui::FontId::monospace(10.0),
        color,
    );
    painter.text(
        rect.left_bottom() + egui::vec2(4.0, -1.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{min:.1}"),
        egui::FontId::monospace(9.0),
        ui.visuals().weak_text_color(),
    );
    painter.text(
        rect.right_bottom() + egui::vec2(-4.0, -1.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{max:.1}"),
        egui::FontId::monospace(9.0),
        ui.visuals().weak_text_color(),
    );
}

pub(crate) fn series_color(series: RuntimeSeries) -> egui::Color32 {
    match series {
        RuntimeSeries::Fps => egui::Color32::from_rgb(110, 210, 120),
        RuntimeSeries::FrameMs => egui::Color32::from_rgb(240, 180, 90),
        RuntimeSeries::RenderCpuMs => egui::Color32::from_rgb(240, 130, 90),
        RuntimeSeries::RenderGpuMs => egui::Color32::from_rgb(110, 170, 240),
        RuntimeSeries::DrawCalls => egui::Color32::from_rgb(120, 210, 220),
        RuntimeSeries::SetPassCalls => egui::Color32::from_rgb(180, 200, 110),
        RuntimeSeries::Triangles => egui::Color32::from_rgb(200, 150, 240),
        RuntimeSeries::Vertices => egui::Color32::from_rgb(160, 180, 250),
        RuntimeSeries::MemoryMib => egui::Color32::from_rgb(240, 140, 160),
    }
}
