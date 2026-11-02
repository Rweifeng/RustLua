use super::*;

/// 面板耗时表最多列出的行数（其余只给计数，完整清单见「复制报告」）。
pub(crate) const MAX_PANEL_ROWS: usize = 12;
/// 命令耗时表最多列出的行数（命令种类随功能增长，不能无限占屏）。
pub(crate) const MAX_COMMAND_ROWS: usize = 8;
/// 调度系统表最多列出的行数（系统数随玩法增长，完整清单仍可复制报告）。
pub(crate) const MAX_SYSTEM_ROWS: usize = 12;
/// 性能面板默认内容区尺寸（`egui::Window::default_size` 定的是**内容区**）。
pub(crate) const DEFAULT_PANEL_WIDTH: f32 = 460.0;
pub(crate) const DEFAULT_PANEL_HEIGHT: f32 = 520.0;
/// 内容区高度的下限：再矮就连「帧」这一节都读不全。
pub(crate) const MIN_PANEL_HEIGHT: f32 = 240.0;
/// 窗口装饰（标题栏 + 边框内边距）的竖向余量，单位是点。
///
/// `default_size` 只管内容区，外面还有标题栏与边框（实测约 46 点）。定位时**必须**把它减掉：
/// 少减了这一块，算好的"右下角"会把窗口底边顶出屏幕，`egui` 再把整窗推回上方——
/// 于是又压回 Scene 工具条（这正是第一版矮窗口用例判红的原因）。
/// 这里取 56 而不是 46：宁可把窗口算矮一点（多出的空间留在底部），也不要吃掉工具条那道间距。
pub(crate) const WINDOW_CHROME_HEIGHT: f32 = 56.0;
/// 工具条下沿到面板上沿的最小间距（点）。
pub(crate) const TOOL_STRIP_GAP: f32 = 12.0;

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
    /// E06-4：0..=1 的占比条（逐 GPU 通道占整帧的比例）；普通行是 `None`。
    pub bar: Option<f32>,
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

/// 一帧调度里的单个系统耗时（`FrameScheduleReport` 的只读快照）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SystemFrameRow {
    pub name: String,
    pub elapsed: Duration,
}

/// 最近一帧的调度系统表；`steps` 与 `total` 与 `FrameScheduleReport` 同口径。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SystemFrameSnapshot {
    pub steps: u32,
    pub total: Duration,
    pub systems: Vec<SystemFrameRow>,
    /// 运行时已注册、但报告里没有计时的系统（E06-1 的"缺一个判红"）。
    pub missing: Vec<String>,
}

impl SystemFrameSnapshot {
    /// 从 Play 预览的运行时读最近一帧调度报告；没在运行时返回 `None`。
    pub(crate) fn capture(app: &EditorApp) -> Option<Self> {
        let runtime = app.preview.as_ref()?.runtime();
        let report = runtime.last_schedule_report()?;
        let (step_names, frame_names) = runtime.registered_system_names();
        Some(Self::from_report(report, &step_names, &frame_names))
    }

    /// 报告 + 注册表 → 面板快照；注册了但报告没计时的系统进 `missing`。
    pub(crate) fn from_report(
        report: &FrameScheduleReport,
        step_names: &[String],
        frame_names: &[String],
    ) -> Self {
        let missing = if report.steps == 0 {
            Vec::new()
        } else {
            step_names
                .iter()
                .chain(frame_names.iter())
                .filter(|name| !report.timings.iter().any(|timing| &timing.name == *name))
                .cloned()
                .collect()
        };
        Self {
            steps: report.steps,
            total: report.total_elapsed,
            systems: report
                .timings
                .iter()
                .map(|timing| SystemFrameRow {
                    name: timing.name.clone(),
                    elapsed: timing.elapsed,
                })
                .collect(),
            missing,
        }
    }

    /// 完整清单（不截断）：面板只列耗时前 N 个，复制报告时仍然给全。
    pub(crate) fn to_text(&self) -> String {
        let mut text = String::from("运行时调度系统（最近一次 run_script_steps）\n");
        text.push_str(&format!("  固定步: {}\n", self.steps));
        text.push_str(&format!(
            "  调度总耗时: {:.3} ms\n",
            self.total.as_secs_f64() * 1000.0
        ));
        if self.systems.is_empty() {
            text.push_str("  （报告没有 per-system 计时）\n");
        }
        for system in &self.systems {
            text.push_str(&format!(
                "  {:<32} {:>10.3} ms\n",
                system.name,
                system.elapsed.as_secs_f64() * 1000.0
            ));
        }
        text
    }
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

pub(crate) fn row(
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
        bar: None,
    }
}

/// 带占比条的行（E06-4：逐通道 GPU 时间的条形图）。
pub(crate) fn row_with_bar(
    label: impl Into<String>,
    value: impl Into<String>,
    raw_key: impl Into<String>,
    fraction: f32,
) -> PerformanceRow {
    PerformanceRow {
        label: label.into(),
        value: value.into(),
        raw_key: raw_key.into(),
        warn: false,
        bar: Some(fraction.clamp(0.0, 1.0)),
    }
}

pub(crate) fn gauge(report: &ProfilerReport, name: &str) -> f64 {
    report
        .gauges
        .iter()
        .find(|(key, _)| key == name)
        .map_or(0.0, |(_, value)| *value)
}

/// 整数量按整数显示、比例量保留两位——避免"实体 12.000"这种噪声。
pub(crate) fn format_scalar(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// 面板/命令耗时行：`最近 / p95 / 最大 ms · 采样 N`。
pub(crate) fn timing_value(summary: &TimingSummary) -> String {
    format!(
        "{:.2} / {:.2} / {:.2} ms · n={}",
        summary.last_ms, summary.p95_ms, summary.max_ms, summary.samples
    )
}
