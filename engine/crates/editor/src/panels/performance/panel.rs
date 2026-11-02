use super::*;

impl EditorApp {
    /// 性能面板（独立弹窗）。
    ///
    /// 关闭时只读一次布尔量。打开时默认落在**工作区右下角**（`workspace_rect`：工具栏 +
    /// Scene/Game 视图那块，2D/3D/Game 三个工作区一致），并额外钉一条下界——窗口上沿
    /// **不得高于 Scene 工具条下沿**。于是两个角落各有其主：右上角是工具条
    /// （选择/移动/旋转/缩放），右下角是性能数字。位置之后由 `egui` 记忆，用户可以拖走。
    pub(crate) fn draw_performance_panel(&mut self, context: &egui::Context) {
        if !self.show_performance_panel {
            self.perf_panel_rect = egui::Rect::NOTHING;
            return;
        }
        let report = self.profiler.report();
        let counts = EditorPerfCounts::capture(self);
        let runtime_snapshot = self.runtime_perf.snapshot();
        let runtime_history = self.runtime_perf.history();
        let schedule = SystemFrameSnapshot::capture(self);
        let trace_level = self.profiler.span_capture_level();
        let trace_summary = self.profiler.span_capture_summary();
        let trace_stats = self.profiler.span_top_n(8);
        let trace_active = trace_level != CaptureLevel::Off;
        // 时间线的输入：只取面板画得起的那一段（`MAX_BARS` 条），不在这里拷满 8192。
        let trace_records = if trace_summary.spans > 0 {
            self.profiler
                .span_records_for_timeline(crate::trace_timeline::MAX_BARS)
        } else {
            Vec::new()
        };
        let sections = performance_sections(
            &report,
            self.ui_layout_cache.stats(),
            counts,
            runtime_snapshot,
            schedule.as_ref(),
        );
        let runtime_paused = self.runtime_perf.paused();

        // 工作区矩形本帧一定画过（`CentralPanel` 每帧都在）；退化值只是防御。
        let workspace = self.workspace_rect;
        let anchor = if workspace.width() > 1.0 && workspace.height() > 1.0 {
            workspace
        } else {
            context.content_rect()
        };
        // `Rect::NOTHING` 的 `bottom()` 是 -inf，因此"本帧没画工具条"时这一项自动失效。
        let floor = self.scene_tools_rect.bottom() + TOOL_STRIP_GAP;
        // 竖向按"工具条下沿以下还剩多少"来定：矮窗口不硬塞 340（塞不下时整窗会被 egui
        // 顶回屏幕内，那正是"又压回工具条"的路径）。
        let available_outer = (anchor.bottom() - 12.0) - floor;
        let inner_height =
            (available_outer - WINDOW_CHROME_HEIGHT).clamp(MIN_PANEL_HEIGHT, DEFAULT_PANEL_HEIGHT);
        let default_size = egui::vec2(DEFAULT_PANEL_WIDTH, inner_height);
        let default_pos = egui::pos2(
            (anchor.right() - default_size.x - 12.0).max(anchor.left() + 8.0),
            (anchor.bottom() - (inner_height + WINDOW_CHROME_HEIGHT) - 12.0)
                .max(anchor.top() + 8.0)
                .max(floor),
        );

        let mut open = self.show_performance_panel;
        let mut clear_samples = false;
        let mut copy_report = false;
        let mut toggle_pause = false;
        let mut toggle_trace = false;
        let mut export_trace = false;
        let mut start_capture = false;
        let mut finish_capture = false;
        let mut cancel_capture = false;
        let capturing = self.runtime_perf.is_capturing();
        let capture_frames = self.runtime_perf.capture_len();
        let capture_status = self.capture_status.clone();
        let response = egui::Window::new("性能分析（编辑器 + 运行时）")
            .open(&mut open)
            .default_pos(default_pos)
            .default_size(default_size)
            .resizable(true)
            .vscroll(true)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("清空采样")
                        .on_hover_text(
                            "清空编辑器帧/面板/命令采样与运行时趋势；计数器、gauge 与当前运行时快照保留",
                        )
                        .clicked()
                    {
                        clear_samples = true;
                    }
                    if ui
                        .button(if runtime_paused {
                            "继续采样"
                        } else {
                            "冻结采样"
                        })
                        .on_hover_text("冻结后面板保留最后一次运行时快照与趋势，便于逐项核对")
                        .clicked()
                    {
                        toggle_pause = true;
                    }
                    if ui
                        .button("复制报告")
                        .on_hover_text(
                            "把编辑器 profiler 与运行时性能文本报告放进剪贴板（可直接归档做证据）",
                        )
                        .clicked()
                    {
                        copy_report = true;
                    }
                    if !capturing {
                        if ui
                            .add_enabled(
                                runtime_snapshot.is_some(),
                                egui::Button::new("开始采集"),
                            )
                            .on_hover_text(
                                "E06-4：采集逐帧帧时间、调度系统与 GPU 通道样本，\
                                 结束后导出 p3-capture-v1 JSON（编辑器采集不带输入日志，不用于跨进程重放）",
                            )
                            .clicked()
                        {
                            start_capture = true;
                        }
                    } else {
                        ui.weak(format!("采集中 {capture_frames} 帧"));
                        if ui
                            .add_enabled(capture_frames >= 2, egui::Button::new("结束并导出"))
                            .on_hover_text("导出到项目的 target/reports/editor-capture.json；有上一份采集时同时写 editor-capture-diff.json")
                            .clicked()
                        {
                            finish_capture = true;
                        }
                        if ui.button("取消采集").clicked() {
                            cancel_capture = true;
                        }
                    }
                    if ui
                        .button(if trace_active { "停止追踪" } else { "开始追踪" })
                        .on_hover_text(
                            "E06-2：捕获帧/面板/命令/调度系统的区间记录（有界 8192，重开会清空）；\
                             用「导出追踪」在 chrome://tracing 或 Perfetto 打开",
                        )
                        .clicked()
                    {
                        toggle_trace = true;
                    }
                    if ui
                        .add_enabled(
                            trace_summary.spans > 0,
                            egui::Button::new("导出追踪"),
                        )
                        .on_hover_text("写 Chrome Trace Event JSON 到项目的 target/reports/editor-profile-trace.json")
                        .clicked()
                    {
                        export_trace = true;
                    }
                    if trace_active {
                        ui.weak(format!(
                            "追踪 {} 个区间（丢弃 {}）",
                            trace_summary.spans, trace_summary.dropped
                        ));
                    }
                    if report.frame_budget_exceeded() {
                        ui.colored_label(
                            egui::Color32::from_rgb(240, 180, 90),
                            format!("编辑器超预算（p95 {:.1} ms）", report.frame_p95_ms),
                        );
                    } else {
                        ui.weak(format!("编辑器预算内（p95 {:.1} ms）", report.frame_p95_ms));
                    }
                    match runtime_snapshot {
                        Some(snapshot) if snapshot.fps_below_target() => {
                            ui.colored_label(
                                egui::Color32::from_rgb(240, 180, 90),
                                format!("运行时 {:.1} FPS", snapshot.fps),
                            );
                        }
                        Some(snapshot) => {
                            ui.weak(format!("运行时 {:.1} FPS", snapshot.fps));
                        }
                        None => {
                            ui.weak("运行时不采样（开始播放后记录）");
                        }
                    }
                });
                if let Some(status) = &capture_status {
                    ui.weak(status);
                }
                ui.separator();
                for section in &sections {
                    egui::CollapsingHeader::new(section.title)
                        .id_salt(section.title)
                        .default_open(section.default_open)
                        .show(ui, |ui| {
                            if section.rows.is_empty() {
                                ui.weak("（暂无数据）");
                            } else {
                                egui::Grid::new(("perf_rows", section.title))
                                    .num_columns(2)
                                    .striped(true)
                                    .spacing([12.0, 2.0])
                                    .show(ui, |ui| {
                                        for entry in &section.rows {
                                            ui.label(&entry.label).on_hover_text(format!(
                                                "profiler 键：{}",
                                                entry.raw_key
                                            ));
                                            let value =
                                                egui::RichText::new(&entry.value).monospace();
                                            let value = if entry.warn {
                                                value.color(egui::Color32::from_rgb(240, 180, 90))
                                            } else {
                                                value
                                            };
                                            ui.vertical(|ui| {
                                                ui.label(value);
                                                if let Some(fraction) = entry.bar {
                                                    ui.add(
                                                        egui::ProgressBar::new(fraction)
                                                            .desired_width(160.0)
                                                            .text(format!("{:.0}%", fraction * 100.0)),
                                                    );
                                                }
                                            });
                                            ui.end_row();
                                        }
                                    });
                            }
                            if let Some(note) = &section.note {
                                ui.weak(note);
                            }
                        });
                }
                if trace_active || trace_summary.spans > 0 {
                    egui::CollapsingHeader::new("追踪捕获")
                        .id_salt("trace_capture")
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.weak(format!(
                                "档位 {} · 区间 {}/{} · 丢弃 {} · 覆盖 {:.3} ms",
                                trace_level_label(trace_level),
                                trace_summary.spans,
                                trace_summary.capacity,
                                trace_summary.dropped,
                                trace_summary.covered_ns as f64 / 1_000_000.0
                            ));
                            if trace_stats.is_empty() {
                                ui.weak("（暂无区间）");
                            } else {
                                egui::Grid::new("trace_top_spans")
                                    .num_columns(3)
                                    .striped(true)
                                    .spacing([12.0, 2.0])
                                    .show(ui, |ui| {
                                        ui.label("区间");
                                        ui.label("合计");
                                        ui.label("p95");
                                        ui.end_row();
                                        for stat in &trace_stats {
                                            ui.label(&stat.name);
                                            ui.label(format!(
                                                "{:.3} ms × {}",
                                                stat.total_us / 1000.0,
                                                stat.count
                                            ));
                                            ui.label(format!("{:.3} ms", stat.p95_us / 1000.0));
                                            ui.end_row();
                                        }
                                    });
                            }
                            ui.add_space(4.0);
                            draw_trace_timeline(ui, &trace_records);
                            ui.weak(
                                "面板里这条是尾部粗时间线（只画最近若干条，合成区间画暗一档、\
                                 被抬到最小宽度的那条描红边）；完整时间线用「导出追踪」在\
                                 chrome://tracing / Perfetto 打开，Fine 档的开销门见\
                                 wp0_profile_capture（≤2%）。",
                            );
                        });
                }
                egui::CollapsingHeader::new("运行时趋势")
                    .id_salt("runtime_trends")
                    .default_open(runtime_snapshot.is_some())
                    .show(ui, |ui| {
                        draw_runtime_trends(ui, runtime_history);
                    });
            });
        self.perf_panel_rect = response.map_or(egui::Rect::NOTHING, |inner| inner.response.rect);
        self.show_performance_panel = open;
        if clear_samples {
            self.profiler.clear_samples();
            self.profiler.clear_span_records();
            self.last_trace_report = None;
            self.runtime_perf.clear_history();
            self.status = "已清空性能采样与追踪记录（计数器、仪表与当前运行时快照保留）".to_owned();
        }
        if toggle_pause {
            let next = !self.runtime_perf.paused();
            self.runtime_perf.set_paused(next);
            self.status = if next {
                "运行时性能采样已冻结".to_owned()
            } else {
                "运行时性能采样已继续".to_owned()
            };
        }
        if start_capture {
            if self.runtime_perf.start_capture() {
                self.capture_status = Some("采集中：播放预览的逐帧样本已开始记录".to_owned());
            } else {
                self.status = "采集需要先开始播放（当前没有运行时快照）".to_owned();
            }
        }
        if cancel_capture {
            self.runtime_perf.cancel_capture();
            self.capture_status = Some("采集已取消".to_owned());
        }
        if finish_capture {
            self.export_editor_capture();
        }
        if copy_report {
            let mut text = report.to_text();
            if let Some(snapshot) = self.runtime_perf.snapshot() {
                text.push('\n');
                text.push_str(&snapshot.to_text());
            }
            if let Some(schedule) = schedule.as_ref() {
                text.push('\n');
                text.push_str(&schedule.to_text());
            }
            if self.profiler.span_capture_summary().spans > 0 {
                text.push('\n');
                text.push_str(&self.profiler.span_report_text(8));
            }
            context.copy_text(text);
            self.status = "性能报告已复制到剪贴板".to_owned();
        }
        if toggle_trace {
            if trace_active {
                self.profiler.stop_span_capture();
                self.status = format!("追踪已停止（保留 {} 个区间，可导出）", trace_summary.spans);
            } else {
                self.profiler.start_span_capture(CaptureLevel::Fine);
                self.last_trace_report = None;
                self.status = "追踪已开始（精细：帧/面板/命令/调度系统）".to_owned();
            }
        }
        if export_trace {
            let json = self.profiler.span_trace_json();
            let path = trace_export_path(&self.project.root);
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, json.as_bytes()));
            self.status = match written {
                Ok(()) => format!("追踪已导出：{}（{} 字节）", path.display(), json.len()),
                Err(error) => format!("追踪导出失败：{error}"),
            };
        }
    }
}

/// 「追踪捕获」里的尾部粗时间线：泳道 = 嵌套深度，横轴 = 相对捕获起点。
///
/// 几何与落笔都在 `crate::trace_timeline`（那层有像素门），这里只负责三件面板才有的事：
/// 把**当帧**的 `available_width` 交给布局（宽度变了必须重算，否则落笔像素与布局对不上）、
/// 把鼠标点从"相对分配矩形"换成"相对泳道原点"（差一个刻度行高）、以及没有悬停时给一行
/// 概况。合成区间与"被抬到最小宽度"的区间由那层自己标色，这里不重复判断。
pub(crate) fn draw_trace_timeline(ui: &mut egui::Ui, records: &[SpanRecord]) {
    use crate::trace_timeline as tl;

    let timeline = tl::layout(records, &tl::TimelineOptions::new(ui.available_width()));
    if timeline.bars.is_empty() {
        ui.weak("（没有可画的区间：未捕获，或留下的全是零时长）");
        return;
    }
    let Some(response) = tl::paint(ui, &timeline) else {
        return;
    };
    // 用当帧的指针位置，而不是 `response.hovered()`：后者要等交互解算完的那一帧才为真，
    // 鼠标刚划进来时会慢一拍。
    let hovered = ui.input(|input| input.pointer.hover_pos()).and_then(|pos| {
        tl::hover_at(
            &timeline,
            [pos.x - response.rect.min.x, pos.y - response.rect.min.y],
        )
    });
    // 两种文案占同一行，避免鼠标划过时面板高度跳动。
    if let Some(bar) = hovered {
        ui.label(tl::bar_hint(bar));
    } else {
        let mut caption = format!(
            "时间线：{} 条 · 窗口 {:.2} ms · {} 条泳道（悬停看单条）",
            timeline.bars.len(),
            timeline.window_ns as f64 / 1_000_000.0,
            timeline.lanes,
        );
        if timeline.dropped > 0 {
            caption += &format!(" · 另有 {} 条超出绘制预算", timeline.dropped);
        }
        ui.weak(caption);
    }
}

/// 追踪档位的中文名（面板只显示这个，不上屏 `off/system/fine` 这类取值）。
pub(crate) fn trace_level_label(level: CaptureLevel) -> &'static str {
    match level {
        CaptureLevel::Off => "关闭",
        CaptureLevel::System => "系统",
        CaptureLevel::Fine => "精细",
    }
}

/// Chrome trace 的落盘路径。放在项目 `target/reports/` 下：与编辑器其它报告产物
/// 同一处，且不会污染工程资源目录。
pub(crate) fn trace_export_path(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root
        .join("target")
        .join("reports")
        .join("editor-profile-trace.json")
}
