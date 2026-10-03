//! 编辑器性能面板：一个**独立弹窗**，展示帧/面板/命令耗时、布局缓存命中率与队列计数。
//!
//! 这份数字原先画在 Scene 视图**右上角的覆盖层**（`painter.text`）里，而那个角落正好是
//! 工具小面板（选择/移动/旋转/缩放）的位置——两段文字叠在一起：工具按钮被盖住、数字也读不全。
//! 现在数字搬进本窗口（可拖动、可缩放、可关闭），视口里不再有性能文字，工具条独占那个角落。
//! 工具栏的「性能」勾选框与「窗口 ▸ 性能面板」菜单项只负责**开关这个窗口**。
//!
//! 分工：`profiler.rs` 负责有界采样与报告，本文件只负责"把报告排成能读的表"与画窗口。
//! 因此排版逻辑（`performance_sections`）是纯函数，可以脱离 egui 断言。

use crate::EditorApp;
use editor::{ProfilerReport, TimingSummary};
use eframe::egui;
use engine::core::ui_layout_cache::LayoutCacheStats;

/// 面板耗时表最多列出的行数（其余只给计数，完整清单见「复制报告」）。
const MAX_PANEL_ROWS: usize = 12;
/// 命令耗时表最多列出的行数（命令种类随功能增长，不能无限占屏）。
const MAX_COMMAND_ROWS: usize = 8;
/// 性能面板默认内容区尺寸（`egui::Window::default_size` 定的是**内容区**）。
const DEFAULT_PANEL_WIDTH: f32 = 420.0;
const DEFAULT_PANEL_HEIGHT: f32 = 340.0;
/// 内容区高度的下限：再矮就连「帧」这一节都读不全。
const MIN_PANEL_HEIGHT: f32 = 200.0;
/// 窗口装饰（标题栏 + 边框内边距）的竖向余量，单位是点。
///
/// `default_size` 只管内容区，外面还有标题栏与边框（实测约 46 点）。定位时**必须**把它减掉：
/// 少减了这一块，算好的"右下角"会把窗口底边顶出屏幕，`egui` 再把整窗推回上方——
/// 于是又压回 Scene 工具条（这正是第一版矮窗口用例判红的原因）。
/// 这里取 56 而不是 46：宁可把窗口算矮一点（多出的空间留在底部），也不要吃掉工具条那道间距。
const WINDOW_CHROME_HEIGHT: f32 = 56.0;
/// 工具条下沿到面板上沿的最小间距（点）。
const TOOL_STRIP_GAP: f32 = 12.0;

/// 一行性能数据。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PerformanceRow {
    /// 中文标签（已登记的量）或原始键名（扩展量）。
    pub label: String,
    /// 已格式化的值。
    pub value: String,
    /// profiler 里的原始键名：悬停显示，便于与报告/基线逐条对照。
    pub raw_key: String,
    /// 是否越过预算（用警示色显示，而不是让用户自己比数字）。
    pub warn: bool,
}

/// 一节性能数据（一个可折叠分组）。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PerformanceSection {
    pub title: &'static str,
    /// 默认展开：帧/缓存/编辑器状态展开；长清单（计数器、gauge）收起。
    pub default_open: bool,
    pub rows: Vec<PerformanceRow>,
    /// 行数被截断时的补充说明。
    pub note: Option<String>,
}

/// 面板需要、但 `profiler` 里没有现成 gauge 的编辑器侧计数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EditorPerfCounts {
    pub undo_len: usize,
    pub undo_dropped: u64,
    pub hierarchy_rows: usize,
    pub resource_scans: u64,
}

impl EditorPerfCounts {
    /// 从编辑器状态读出本帧的计数（与旧 HUD 显示的同一批数字）。
    pub(crate) fn capture(app: &EditorApp) -> Self {
        Self {
            undo_len: app.history.undo_len(),
            undo_dropped: app.history.dropped_entries(),
            hierarchy_rows: app.hierarchy_visible_cache.len(),
            resource_scans: app.profiler.counter("resource_refresh_applied"),
        }
    }
}

fn row(
    label: impl Into<String>,
    value: impl Into<String>,
    raw_key: impl Into<String>,
    warn: bool,
) -> PerformanceRow {
    PerformanceRow {
        label: label.into(),
        value: value.into(),
        raw_key: raw_key.into(),
        warn,
    }
}

fn gauge(report: &ProfilerReport, name: &str) -> f64 {
    report
        .gauges
        .iter()
        .find(|(key, _)| key == name)
        .map_or(0.0, |(_, value)| *value)
}

/// 整数量按整数显示、比例量保留两位——避免"实体 12.000"这种噪声。
fn format_scalar(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// 面板/命令耗时行：`最近 / p95 / 最大 ms · 采样 N`。
fn timing_value(summary: &TimingSummary) -> String {
    format!(
        "{:.2} / {:.2} / {:.2} ms · n={}",
        summary.last_ms, summary.p95_ms, summary.max_ms, summary.samples
    )
}

/// 把一帧的 profiler 报告 + 布局缓存 + 编辑器计数排成可读的分组表。
///
/// 纯函数：不碰 `&mut self`，因此"面板该显示什么"可以在无 GPU、无窗口的测试里逐条断言。
pub(crate) fn performance_sections(
    report: &ProfilerReport,
    layout: &LayoutCacheStats,
    counts: EditorPerfCounts,
) -> Vec<PerformanceSection> {
    let exceeded = report.frame_budget_exceeded();
    let over_percent = if report.frame_samples == 0 {
        0.0
    } else {
        report.over_budget_frames as f64 / report.frame_samples as f64 * 100.0
    };

    let mut sections = Vec::new();

    // ① 帧：预算是否守住，是性能面板要回答的第一个问题。
    sections.push(PerformanceSection {
        title: "帧",
        default_open: true,
        rows: vec![
            row(
                "最近一帧",
                format!("{:.2} ms", report.frame_last_ms),
                "frame_last_ms",
                exceeded,
            ),
            row(
                "p50",
                format!("{:.2} ms", report.frame_p50_ms),
                "frame_p50_ms",
                false,
            ),
            row(
                "p95",
                format!("{:.2} ms", report.frame_p95_ms),
                "frame_p95_ms",
                exceeded,
            ),
            row(
                "p99",
                format!("{:.2} ms", report.frame_p99_ms),
                "frame_p99_ms",
                exceeded,
            ),
            row(
                "最大",
                format!("{:.2} ms", report.frame_max_ms),
                "frame_max_ms",
                false,
            ),
            row(
                "预算",
                format!("{:.1} ms（60 FPS）", report.frame_budget_ms),
                "frame_budget_ms",
                false,
            ),
            row(
                "超预算帧",
                format!(
                    "{} / {}（{:.1}%）",
                    report.over_budget_frames, report.frame_samples, over_percent
                ),
                "over_budget_frames",
                exceeded,
            ),
            row(
                "采样窗口",
                format!("{} 帧", report.frame_samples),
                "frame_samples",
                false,
            ),
            row(
                "记录中",
                if report.enabled { "是" } else { "否" },
                "enabled",
                !report.enabled,
            ),
        ],
        note: None,
    });

    // ② 布局缓存：命中率是"UI 布局重算是否被收敛"的直接证据（E25）。
    let lookups = layout.hits + layout.misses;
    let hit_rate = if lookups == 0 {
        0.0
    } else {
        layout.hits as f64 / lookups as f64 * 100.0
    };
    sections.push(PerformanceSection {
        title: "布局缓存",
        default_open: true,
        rows: vec![
            row(
                "条目",
                layout.entries.to_string(),
                "layout_cache_entries",
                false,
            ),
            row("命中", layout.hits.to_string(), "layout_cache_hits", false),
            row(
                "未命中",
                layout.misses.to_string(),
                "layout_cache_misses",
                false,
            ),
            row(
                "命中率",
                format!("{hit_rate:.1}%"),
                "layout_cache_hit_rate",
                false,
            ),
            row(
                "失效",
                layout.invalidations.to_string(),
                "layout_cache_invalidations",
                false,
            ),
        ],
        note: None,
    });

    // ③ 编辑器状态：历史深度、层级/场景/资源规模与队列（旧 HUD 那一行的等价物）。
    let pending_imports = gauge(report, "resource_pending_imports");
    sections.push(PerformanceSection {
        title: "编辑器状态",
        default_open: true,
        rows: vec![
            row(
                "撤销栈深度",
                counts.undo_len.to_string(),
                "history_undo_len",
                false,
            ),
            row(
                "丢弃的历史条数",
                counts.undo_dropped.to_string(),
                "history_dropped_entries",
                false,
            ),
            row(
                "层级可见行",
                counts.hierarchy_rows.to_string(),
                "hierarchy_visible_rows",
                false,
            ),
            row(
                "场景实体",
                format_scalar(gauge(report, "scene_entities")),
                "scene_entities",
                false,
            ),
            row(
                "资源条目",
                format_scalar(gauge(report, "resource_entries")),
                "resource_entries",
                false,
            ),
            row(
                "待导入资源",
                format_scalar(pending_imports),
                "resource_pending_imports",
                pending_imports > 0.0,
            ),
            row(
                "资源扫描应用次数",
                counts.resource_scans.to_string(),
                "resource_refresh_applied",
                false,
            ),
        ],
        note: None,
    });

    // ④ 面板耗时：按 p95 降序——"谁最贵"必须一眼看出来，而不是按字母序找。
    let mut panels = report.panels.clone();
    panels.sort_by(|left, right| right.p95_ms.total_cmp(&left.p95_ms));
    let panel_total = panels.len();
    panels.truncate(MAX_PANEL_ROWS);
    sections.push(PerformanceSection {
        title: "面板耗时",
        default_open: true,
        rows: panels
            .iter()
            .map(|panel| {
                row(
                    panel.name.clone(),
                    timing_value(panel),
                    format!("panel.{}", panel.name),
                    panel.p95_ms > report.frame_budget_ms && panel.samples > 0,
                )
            })
            .collect(),
        note: (panel_total > MAX_PANEL_ROWS)
            .then(|| format!("其余 {} 个面板见「复制报告」", panel_total - MAX_PANEL_ROWS)),
    });

    // ⑤ 命令耗时：按累计耗时降序（一次慢命令比一次慢面板更值得追）。
    let mut commands = report.commands.clone();
    commands.sort_by(|left, right| right.total_ms.total_cmp(&left.total_ms));
    let command_total = commands.len();
    commands.truncate(MAX_COMMAND_ROWS);
    sections.push(PerformanceSection {
        title: "命令耗时",
        default_open: false,
        rows: commands
            .iter()
            .map(|command| {
                row(
                    command.name.clone(),
                    format!(
                        "{} · 累计 {:.1} ms",
                        timing_value(command),
                        command.total_ms
                    ),
                    format!("command.{}", command.name),
                    false,
                )
            })
            .collect(),
        note: (command_total > MAX_COMMAND_ROWS).then(|| {
            format!(
                "其余 {} 个命令见「复制报告」",
                command_total - MAX_COMMAND_ROWS
            )
        }),
    });

    // ⑥⑦ 计数器 / gauge：profiler 的扩展面。名字可能随功能增长，未登记的原样显示。
    sections.push(PerformanceSection {
        title: "计数器（累计）",
        default_open: false,
        rows: report
            .counters
            .iter()
            .map(|(name, value)| row(counter_label(name), value.to_string(), name.clone(), false))
            .collect(),
        note: None,
    });
    sections.push(PerformanceSection {
        title: "Gauge（本帧快照）",
        default_open: false,
        rows: report
            .gauges
            .iter()
            .map(|(name, value)| {
                row(
                    gauge_label(name),
                    format_scalar(*value),
                    name.clone(),
                    false,
                )
            })
            .collect(),
        note: None,
    });

    sections
}

/// 计数器中文名（`profiler.bump/add` 的调用点里已登记的键）。
///
/// 未登记的名字原样显示——新增计数器不改这里也能读，悬停仍能看到原始键名。
fn counter_label(name: &str) -> String {
    match name {
        "hierarchy_cache_hit" => "层级缓存命中",
        "hierarchy_cache_miss" => "层级缓存未命中",
        "hierarchy_rows_drawn" => "层级实际绘制行",
        "console_index_cache_hit" => "控制台索引缓存命中",
        "console_index_cache_miss" => "控制台索引缓存未命中",
        "console_diagnostic_jump" => "控制台诊断跳转",
        "resource_refresh_started" => "资源扫描启动",
        "resource_refresh_applied" => "资源扫描应用",
        "resource_refresh_cancelled" => "资源扫描取消",
        "resource_refresh_failed" => "资源扫描失败",
        "resource_refresh_stale_dropped" => "过期扫描结果丢弃",
        "file_watch_events" => "文件监视事件",
        "file_watch_changed_files" => "变化文件数",
        "layout_saved" => "布局保存",
        "layout_auto_saved" => "布局自动保存",
        "layout_save_failed" => "布局保存失败",
        "layout_reset" => "布局重置",
        other => other,
    }
    .to_owned()
}

/// Gauge 中文名（`EditorApp::update_profiler_gauges` 写入的那批）。
fn gauge_label(name: &str) -> String {
    match name {
        "history_undo_len" => "撤销栈深度",
        "history_redo_len" => "重做栈深度",
        "history_dropped_entries" => "丢弃的历史条数",
        "log_sink_lines" => "日志行数",
        "console_notice_lines" => "控制台提示行数",
        "console_rows_indexed" => "控制台索引行数",
        "resource_entries" => "资源条目",
        "resource_pending_imports" => "待导入资源",
        "resource_refresh_in_flight" => "资源刷新进行中",
        "scene_entities" => "场景实体",
        "hierarchy_visible_rows" => "层级可见行",
        "hierarchy_row_content_height" => "层级行内容高度",
        "console_row_content_height" => "控制台行内容高度",
        "thumbnail_cache_entries" => "缩略图缓存条目",
        other => other,
    }
    .to_owned()
}

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
        let sections = performance_sections(&report, self.ui_layout_cache.stats(), counts);

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
        let response = egui::Window::new("性能（Editor Profiler）")
            .open(&mut open)
            .default_pos(default_pos)
            .default_size(default_size)
            .resizable(true)
            .vscroll(true)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("清空采样")
                        .on_hover_text("只清空帧/面板/命令的采样窗口；计数器与 gauge 保留")
                        .clicked()
                    {
                        clear_samples = true;
                    }
                    if ui
                        .button("复制报告")
                        .on_hover_text("把完整 profiler 文本报告放进剪贴板（可直接归档做证据）")
                        .clicked()
                    {
                        copy_report = true;
                    }
                    if report.frame_budget_exceeded() {
                        ui.colored_label(
                            egui::Color32::from_rgb(240, 180, 90),
                            format!("超预算（p95 {:.1} ms）", report.frame_p95_ms),
                        );
                    } else {
                        ui.weak(format!("预算内（p95 {:.1} ms）", report.frame_p95_ms));
                    }
                });
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
                                            ui.label(value);
                                            ui.end_row();
                                        }
                                    });
                            }
                            if let Some(note) = &section.note {
                                ui.weak(note);
                            }
                        });
                }
            });
        self.perf_panel_rect = response.map_or(egui::Rect::NOTHING, |inner| inner.response.rect);
        self.show_performance_panel = open;
        if clear_samples {
            self.profiler.clear_samples();
            self.status = "已清空性能采样（计数器与 gauge 保留）".to_owned();
        }
        if copy_report {
            context.copy_text(report.to_text());
            self.status = "性能报告已复制到剪贴板".to_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framegate::GuiSmoke;
    use editor::{DEFAULT_FRAME_BUDGET_MS, EditorProfiler};
    use std::time::{Duration, Instant};

    fn section<'a>(sections: &'a [PerformanceSection], title: &str) -> &'a PerformanceSection {
        sections
            .iter()
            .find(|section| section.title == title)
            .unwrap_or_else(|| panic!("缺少分组 {title}"))
    }

    /// 采样一批帧（其中一帧超预算）+ 一次面板/命令耗时 + 计数器/gauge。
    fn sampled_report() -> ProfilerReport {
        let mut profiler = EditorProfiler::new(DEFAULT_FRAME_BUDGET_MS);
        let start = Instant::now();
        profiler.begin_frame(start);
        profiler.end_frame(start + Duration::from_millis(4));
        profiler.begin_frame(start);
        profiler.end_frame(start + Duration::from_millis(25));
        profiler.record_panel("hierarchy", Duration::from_millis(3));
        profiler.record_panel("inspector", Duration::from_millis(1));
        profiler.record_command("undo", Duration::from_millis(2));
        profiler.bump("resource_refresh_applied");
        profiler.bump("hierarchy_cache_hit");
        profiler.set_gauge("scene_entities", 7.0);
        profiler.set_gauge("resource_pending_imports", 2.0);
        profiler.report()
    }

    #[test]
    fn sections_cover_frame_cache_and_editor_state() {
        let report = sampled_report();
        let layout = LayoutCacheStats {
            hits: 3,
            misses: 1,
            invalidations: 2,
            entries: 4,
        };
        let counts = EditorPerfCounts {
            undo_len: 5,
            undo_dropped: 1,
            hierarchy_rows: 6,
            resource_scans: 1,
        };
        let sections = performance_sections(&report, &layout, counts);

        let frame = section(&sections, "帧");
        assert!(frame.default_open);
        let p95 = frame
            .rows
            .iter()
            .find(|row| row.raw_key == "frame_p95_ms")
            .expect("帧分组必须有 p95");
        assert_eq!(p95.value, "25.00 ms");
        assert!(p95.warn, "p95 越过预算必须标警示：{p95:?}");

        let cache = section(&sections, "布局缓存");
        assert_eq!(
            cache
                .rows
                .iter()
                .find(|row| row.raw_key == "layout_cache_hit_rate")
                .expect("必须有命中率")
                .value,
            "75.0%"
        );

        // 旧 HUD 那一行的数字必须一个不少地出现在「编辑器状态」里。
        let state = section(&sections, "编辑器状态");
        for (key, value) in [
            ("history_undo_len", "5"),
            ("history_dropped_entries", "1"),
            ("hierarchy_visible_rows", "6"),
            ("scene_entities", "7"),
            ("resource_entries", "0"),
            ("resource_pending_imports", "2"),
            ("resource_refresh_applied", "1"),
        ] {
            let row = state
                .rows
                .iter()
                .find(|row| row.raw_key == key)
                .unwrap_or_else(|| panic!("编辑器状态缺少 {key}"));
            assert_eq!(row.value, value, "{key} 的值不对：{row:?}");
        }
        assert!(
            state
                .rows
                .iter()
                .find(|row| row.raw_key == "resource_pending_imports")
                .expect("待导入行")
                .warn,
            "有待导入资源时必须标警示"
        );
    }

    #[test]
    fn panel_and_command_tables_are_sorted_and_bounded() {
        let mut report = sampled_report();
        // 造 20 个面板与 12 个命令：验证"按耗时排序 + 截断 + 给出余量说明"。
        report.panels = (0..20)
            .map(|index| TimingSummary {
                name: format!("panel{index:02}"),
                samples: 1,
                last_ms: index as f32,
                p50_ms: index as f32,
                p95_ms: index as f32,
                max_ms: index as f32,
                total_ms: f64::from(index),
            })
            .collect();
        report.commands = (0..12)
            .map(|index| TimingSummary {
                name: format!("cmd{index:02}"),
                samples: 1,
                last_ms: index as f32,
                p50_ms: index as f32,
                p95_ms: index as f32,
                max_ms: index as f32,
                total_ms: f64::from(index),
            })
            .collect();
        let sections =
            performance_sections(&report, &LayoutCacheStats::default(), Default::default());

        let panels = section(&sections, "面板耗时");
        assert_eq!(panels.rows.len(), MAX_PANEL_ROWS);
        assert_eq!(panels.rows[0].label, "panel19", "必须按 p95 降序");
        assert!(
            panels
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("其余 8 个"),
            "截断必须给出余量：{:?}",
            panels.note
        );

        let commands = section(&sections, "命令耗时");
        assert_eq!(commands.rows.len(), MAX_COMMAND_ROWS);
        assert_eq!(commands.rows[0].label, "cmd11", "必须按累计耗时降序");
        assert!(!commands.default_open, "长清单默认收起");
    }

    #[test]
    fn empty_series_report_placeholders_not_panics() {
        let report = ProfilerReport {
            frame_budget_ms: DEFAULT_FRAME_BUDGET_MS,
            ..Default::default()
        };
        let sections =
            performance_sections(&report, &LayoutCacheStats::default(), Default::default());
        assert_eq!(sections.len(), 7, "七个分组固定存在");
        for title in [
            "面板耗时",
            "命令耗时",
            "计数器（累计）",
            "Gauge（本帧快照）",
        ] {
            assert!(
                section(&sections, title).rows.is_empty(),
                "{title} 在空报告下应为空（由绘制层显示占位）"
            );
        }
        // `enabled=false` 的报告必须自曝"没在记录"，而不是显示一排 0 让人误解。
        let disabled = ProfilerReport::default();
        let sections =
            performance_sections(&disabled, &LayoutCacheStats::default(), Default::default());
        let row = section(&sections, "帧")
            .rows
            .iter()
            .find(|row| row.raw_key == "enabled")
            .expect("帧分组必须有记录开关");
        assert_eq!(row.value, "否");
        assert!(row.warn);
    }

    /// 用户报的那个重叠：性能数字与 Scene 右上角工具条**不能**再叠在一起。
    ///
    /// 走的是整帧生产路径（合成 `egui` 帧），断言的是两个面板本帧真实的矩形。
    #[test]
    fn opening_the_panel_does_not_cover_the_scene_tool_strip() {
        let mut smoke = GuiSmoke::new("perf-panel", egui::vec2(1280.0, 720.0), 1.0);
        smoke.idle(4);
        let tools = smoke.app.scene_tools_rect;
        assert!(
            tools.width() > 1.0 && tools.height() > 1.0,
            "Scene 工具小面板必须已画出：{tools:?}"
        );
        // 关着的时候不允许留下"上一帧的矩形"，否则证据会撒谎。
        assert_eq!(smoke.app.perf_panel_rect, egui::Rect::NOTHING);

        smoke.app.show_performance_panel = true;
        smoke.idle(3);
        let panel = smoke.app.perf_panel_rect;
        assert!(
            panel.width() > 1.0 && panel.height() > 1.0,
            "打开后必须真的画出窗口：{panel:?}"
        );
        assert!(
            !panel.intersects(tools),
            "性能面板默认位置压住了 Scene 工具条：panel={panel:?} tools={tools:?}"
        );

        // 反向对照：关掉后又回到 NOTHING（矩形不是"最后一次拖动的残留"）。
        smoke.app.show_performance_panel = false;
        smoke.idle(2);
        assert_eq!(smoke.app.perf_panel_rect, egui::Rect::NOTHING);
        smoke.cleanup();

        // 矮窗口：右下角放不下默认高度时，按"工具条下沿以下的可用空间"收矮，
        // 而不是被 egui 顶回屏幕上方、又压回用户报的那个角落。
        let tall_height = panel.height();
        let mut short = GuiSmoke::new("perf-panel-short", egui::vec2(1024.0, 600.0), 1.0);
        short.idle(4);
        short.app.show_performance_panel = true;
        short.idle(3);
        let panel = short.app.perf_panel_rect;
        let tools = short.app.scene_tools_rect;
        assert!(
            panel.width() > 1.0 && tools.width() > 1.0,
            "矮窗口下两者都必须画出来：panel={panel:?} tools={tools:?}"
        );
        assert!(
            panel.height() < tall_height,
            "矮窗口下默认高度必须收到可用空间内：矮={:.1} 常规={tall_height:.1}",
            panel.height()
        );
        assert!(
            !panel.intersects(tools),
            "矮窗口下性能面板压住了 Scene 工具条：panel={panel:?} tools={tools:?}"
        );
        short.cleanup();
    }
}
