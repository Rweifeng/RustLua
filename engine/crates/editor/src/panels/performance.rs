//! 编辑器性能分析面板：一个**独立弹窗**，分两层展示读数。
//!
//! - **运行时层**（Play 预览）：帧率、帧间隔、CPU/GPU 渲染、Lua/更新耗时、draw call、
//!   set pass、实例/顶点/三角形、内存、贴图/网格缓存与加载队列（见 `runtime_perf.rs`）；
//! - **编辑器层**：编辑器帧/面板/命令耗时、布局缓存命中率、资源扫描与队列计数。
//!
//! 这份数字原先画在 Scene 视图**右上角的覆盖层**（`painter.text`）里，而那个角落正好是
//! 工具小面板（选择/移动/旋转/缩放）的位置——两段文字叠在一起：工具按钮被盖住、数字也读不全。
//! 现在数字搬进本窗口（可拖动、可缩放、可关闭），视口里不再有性能文字，工具条独占那个角落。
//! 工具栏的「性能」勾选框与「窗口 ▸ 性能分析面板」菜单项只负责**开关这个窗口**。
//!
//! 分工：`profiler.rs` 负责编辑器有界采样与报告，`runtime_perf.rs` 负责运行时快照与趋势，
//! 本文件只负责"把报告排成能读的表 / 把趋势画成折线"与画窗口。因此排版逻辑
//! （`performance_sections`）是纯函数，可以脱离 egui 断言。

use crate::EditorApp;
use crate::runtime_perf::{RuntimePerfHistory, RuntimePerfSnapshot, RuntimeSeries, bytes_to_mib};
use editor::{ProfilerReport, TimingSummary};
use eframe::egui;
use rustlua_ui::ui_layout_cache::LayoutCacheStats;

/// 面板耗时表最多列出的行数（其余只给计数，完整清单见「复制报告」）。
const MAX_PANEL_ROWS: usize = 12;
/// 命令耗时表最多列出的行数（命令种类随功能增长，不能无限占屏）。
const MAX_COMMAND_ROWS: usize = 8;
/// 性能面板默认内容区尺寸（`egui::Window::default_size` 定的是**内容区**）。
const DEFAULT_PANEL_WIDTH: f32 = 460.0;
const DEFAULT_PANEL_HEIGHT: f32 = 520.0;
/// 内容区高度的下限：再矮就连「帧」这一节都读不全。
const MIN_PANEL_HEIGHT: f32 = 240.0;
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
    runtime: Option<&RuntimePerfSnapshot>,
) -> Vec<PerformanceSection> {
    let exceeded = report.frame_budget_exceeded();
    let over_percent = if report.frame_samples == 0 {
        0.0
    } else {
        report.over_budget_frames as f64 / report.frame_samples as f64 * 100.0
    };

    let mut sections = Vec::new();

    // ⓪ 运行时：Play 预览的实时读数（帧率 / draw / setpass / 顶点面数 / 资源 / 内存）。
    // 未 Play 时只给一个说明分组，不把上一局的过期数字留在这里。
    sections.extend(runtime_perf_sections(runtime));

    // ① 编辑器帧：预算是否守住，是编辑器自身性能要回答的第一个问题。
    sections.push(PerformanceSection {
        title: "编辑器帧",
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

/// 运行时（Play 预览）的四个分组；没有运行时快照时给一个明确的"未运行"分组。
///
/// 只读 `RuntimePerfSnapshot`，不碰 `EditorApp`——与编辑器分组一样保持可单测。
fn runtime_perf_sections(snapshot: Option<&RuntimePerfSnapshot>) -> Vec<PerformanceSection> {
    let Some(snapshot) = snapshot else {
        return vec![PerformanceSection {
            title: "运行时（Play 预览）",
            default_open: true,
            rows: vec![row(
                "状态",
                "未运行（点击 Play 开始实时采样）",
                "runtime.status",
                false,
            )],
            note: Some(
                "Play 后本组显示帧率 / draw call / set pass / 顶点面数 / CPU/GPU / 内存 / \
                 资源加载与缓存。停止 Play 会清空，不保留上一局的过期读数。"
                    .to_owned(),
            ),
        }];
    };

    let texture_lookups = snapshot
        .texture_identity_hits
        .saturating_add(snapshot.texture_identity_misses);
    let texture_hit_rate = if texture_lookups == 0 {
        0.0
    } else {
        snapshot.texture_identity_hits as f64 / texture_lookups as f64 * 100.0
    };
    let mesh_lookups = snapshot
        .mesh_identity_hits
        .saturating_add(snapshot.mesh_identity_misses);
    let mesh_hit_rate = if mesh_lookups == 0 {
        0.0
    } else {
        snapshot.mesh_identity_hits as f64 / mesh_lookups as f64 * 100.0
    };
    let pass_list = if snapshot.passes.is_empty() {
        "（无）".to_owned()
    } else {
        snapshot.passes.join(" → ")
    };
    let short_pass_list = truncate_chars(&pass_list, 48);
    let known_gpu_bytes = snapshot
        .texture_bytes
        .saturating_add(snapshot.mesh_bytes)
        .saturating_add(snapshot.vertex_buffer_capacity);
    let process_memory = snapshot.process_memory_mib.map_or_else(
        || "不可用（仅 Windows）".to_owned(),
        |mib| format!("{mib:.1} MiB"),
    );

    vec![
        PerformanceSection {
            title: "运行时 · 帧",
            default_open: true,
            rows: vec![
                row("目标", snapshot.target, "runtime.target", false),
                row(
                    "分辨率",
                    format!("{}×{}", snapshot.resolution.0, snapshot.resolution.1),
                    "runtime.resolution",
                    false,
                ),
                row(
                    "帧率",
                    format!("{:.1} FPS", snapshot.fps),
                    "runtime.fps",
                    snapshot.fps_below_target(),
                ),
                row(
                    "帧间隔",
                    format!("{:.2} ms", snapshot.frame_ms),
                    "runtime.frame_ms",
                    snapshot.frame_ms_over_budget(),
                ),
                row(
                    "CPU 渲染",
                    format!("{:.2} ms", snapshot.render_cpu_ms),
                    "runtime.render_cpu_ms",
                    snapshot.render_cpu_ms > 16.7,
                ),
                row(
                    "GPU 渲染",
                    snapshot.gpu_timing_label(),
                    "runtime.render_gpu_ms",
                    false,
                ),
                row(
                    "运行时更新",
                    format!("{:.2} ms", snapshot.update_ms),
                    "runtime.update_ms",
                    snapshot.update_ms > 16.7,
                ),
                row(
                    "Lua 耗时",
                    format!("{:.3} ms", snapshot.lua_ms),
                    "runtime.lua_ms",
                    false,
                ),
                row(
                    "累计帧数",
                    snapshot.frames.to_string(),
                    "runtime.frames",
                    false,
                ),
            ],
            note: (snapshot.render_gpu_ms.is_none() && snapshot.gpu_timing_note.is_some())
                .then(|| snapshot.gpu_timing_label()),
        },
        PerformanceSection {
            title: "运行时 · 渲染",
            default_open: true,
            rows: vec![
                row(
                    "Draw Calls",
                    snapshot.draw_calls.to_string(),
                    "runtime.draw_calls",
                    snapshot.draw_calls > 2000,
                ),
                row(
                    "Set Pass",
                    snapshot.set_pass_calls.to_string(),
                    "runtime.set_pass_calls",
                    snapshot.set_pass_calls > 200,
                ),
                row(
                    "实例数",
                    snapshot.instances.to_string(),
                    "runtime.instances",
                    false,
                ),
                row(
                    "三角形（面数）",
                    snapshot.triangles.to_string(),
                    "runtime.triangles",
                    false,
                ),
                row(
                    "顶点数",
                    snapshot.vertices.to_string(),
                    "runtime.vertices",
                    false,
                ),
                row(
                    "丢弃 Draw",
                    snapshot.dropped_draws.to_string(),
                    "runtime.dropped_draws",
                    snapshot.dropped_draws > 0,
                ),
                row(
                    "渲染通道数",
                    snapshot.passes.len().to_string(),
                    "runtime.pass_count",
                    false,
                ),
                row(
                    "渲染通道",
                    format!("{} 条：{short_pass_list}", snapshot.passes.len()),
                    "runtime.passes",
                    false,
                ),
                row(
                    "2D 顶点缓冲",
                    format_bytes(snapshot.vertex_buffer_capacity),
                    "runtime.vertex_buffer_capacity",
                    false,
                ),
                row(
                    "顶点缓冲峰值",
                    format_bytes(snapshot.vertex_bytes_peak),
                    "runtime.vertex_bytes_peak",
                    false,
                ),
                row(
                    "缓冲扩容次数",
                    snapshot.vertex_buffer_growths.to_string(),
                    "runtime.vertex_buffer_growths",
                    snapshot.vertex_buffer_growths > 8,
                ),
                row(
                    "回读槽 / 丢弃",
                    format!(
                        "{} / {}",
                        snapshot.readback.slot_count, snapshot.readback.dropped
                    ),
                    "runtime.readback_slots",
                    snapshot.readback.dropped > 0,
                ),
                row(
                    "回读待处理",
                    snapshot.readback.pending.to_string(),
                    "runtime.readback_pending",
                    false,
                ),
            ],
            note: Some(format!("完整通道顺序：{pass_list}")),
        },
        PerformanceSection {
            title: "运行时 · 资源与缓存",
            default_open: true,
            rows: vec![
                row(
                    "2D/3D 贴图",
                    snapshot.loaded_textures.to_string(),
                    "runtime.loaded_textures",
                    false,
                ),
                row(
                    "字体图集",
                    snapshot.font_atlas_textures.to_string(),
                    "runtime.font_atlas_textures",
                    false,
                ),
                row(
                    "贴图显存（估算）",
                    format_bytes(snapshot.texture_bytes),
                    "runtime.texture_bytes",
                    false,
                ),
                row(
                    "贴图身份命中",
                    snapshot.texture_identity_hits.to_string(),
                    "runtime.texture_identity_hits",
                    false,
                ),
                row(
                    "贴图身份未命中",
                    snapshot.texture_identity_misses.to_string(),
                    "runtime.texture_identity_misses",
                    false,
                ),
                row(
                    "贴图身份命中率",
                    format!("{texture_hit_rate:.1}%"),
                    "runtime.texture_identity_hit_rate",
                    false,
                ),
                row(
                    "网格缓存",
                    snapshot.cached_meshes.to_string(),
                    "runtime.cached_meshes",
                    false,
                ),
                row(
                    "网格显存（估算）",
                    format_bytes(snapshot.mesh_bytes),
                    "runtime.mesh_bytes",
                    false,
                ),
                row(
                    "网格身份命中 / 未命中",
                    format!(
                        "{} / {}（{:.1}%）",
                        snapshot.mesh_identity_hits,
                        snapshot.mesh_identity_misses,
                        mesh_hit_rate
                    ),
                    "runtime.mesh_identity_stats",
                    false,
                ),
                row(
                    "加载模式",
                    if snapshot.async_mode {
                        "异步（后台 worker）"
                    } else {
                        "同步（编辑器预览 / 确定性输出）"
                    },
                    "runtime.load_mode",
                    false,
                ),
                row(
                    "加载线程",
                    snapshot.async_workers.to_string(),
                    "runtime.async_workers",
                    false,
                ),
                row(
                    "待加载 / 已完成",
                    format!(
                        "{} / {}",
                        snapshot.async_pending, snapshot.async_completed
                    ),
                    "runtime.async_loads",
                    snapshot.async_pending > 0,
                ),
                row(
                    "资源身份条目",
                    snapshot.asset_entries.to_string(),
                    "runtime.asset_entries",
                    false,
                ),
                row(
                    "场景实体 / 运行时实体",
                    format!(
                        "{} / {}",
                        snapshot.scene_entities, snapshot.runtime_entities
                    ),
                    "runtime.entities",
                    false,
                ),
            ],
            note: Some(
                "身份命中率按 draw 的资源句柄解析计，不等于磁盘缓存命中率；显存是上传时记录的估算值。"
                    .to_owned(),
            ),
        },
        PerformanceSection {
            title: "运行时 · 内存",
            default_open: true,
            rows: vec![
                row(
                    "进程工作集",
                    process_memory,
                    "runtime.process_memory_mib",
                    false,
                ),
                row(
                    "GPU 贴图",
                    format_bytes(snapshot.texture_bytes),
                    "runtime.gpu_texture_bytes",
                    false,
                ),
                row(
                    "GPU 网格",
                    format_bytes(snapshot.mesh_bytes),
                    "runtime.gpu_mesh_bytes",
                    false,
                ),
                row(
                    "2D 顶点缓冲",
                    format_bytes(snapshot.vertex_buffer_capacity),
                    "runtime.gpu_vertex_buffer_bytes",
                    false,
                ),
                row(
                    "回读 CPU 缓冲",
                    format_bytes(snapshot.readback.cpu_bytes as u64),
                    "runtime.readback_cpu_bytes",
                    false,
                ),
                row(
                    "已知 GPU 合计",
                    format_bytes(known_gpu_bytes),
                    "runtime.known_gpu_bytes",
                    false,
                ),
            ],
            note: Some(
                "进程工作集是真实 RSS（Windows GetProcessMemoryInfo）；GPU 字节是上传统计的估算值，\
                 不含驱动对齐与不可见的管线/绑定组开销。"
                    .to_owned(),
            ),
        },
    ]
}

/// 字节 → 人读单位（面板表格用）。
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes_to_mib(bytes))
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}

/// 按字符截断长文本（通道列表一行放不下；完整顺序放在分组说明里换行显示）。
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
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

/// 趋势图单格高度（点）。
const TREND_HEIGHT: f32 = 52.0;
/// 趋势图两列之间的间距（点）。
const TREND_GAP: f32 = 8.0;

/// 运行时趋势：把最近 240 帧画成 2 列折线图（帧率 / 帧间隔 / CPU/GPU / draw / 顶点面数 / 内存）。
///
/// 用 `egui::Painter` 直接画折线，不引入 `egui_plot` 依赖；每格只保留最近值与
/// 最小/最大刻度，趋势细节靠折线形状表达。
fn draw_runtime_trends(ui: &mut egui::Ui, history: &RuntimePerfHistory) {
    if history.is_empty() {
        ui.weak("（Play 运行后开始记录；窗口容量 240 帧）");
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
                    draw_sparkline(ui, series.label(), &values, series_color(series), width);
                }
                if index % 2 == 1 {
                    ui.end_row();
                }
            }
        });
}

/// 一格折线图：背景 + 中线 + 折线 + 左上标签 + 右上最近值。
fn draw_sparkline(
    ui: &mut egui::Ui,
    label: &str,
    values: &[f32],
    color: egui::Color32,
    width: f32,
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

fn series_color(series: RuntimeSeries) -> egui::Color32 {
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
        let sections = performance_sections(
            &report,
            self.ui_layout_cache.stats(),
            counts,
            runtime_snapshot,
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
        let response = egui::Window::new("性能分析（Editor + Runtime）")
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
                            ui.weak("运行时不采样（Play 后开始）");
                        }
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
            self.runtime_perf.clear_history();
            self.status = "已清空性能采样（计数器、gauge 与当前运行时快照保留）".to_owned();
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
        if copy_report {
            let mut text = report.to_text();
            if let Some(snapshot) = self.runtime_perf.snapshot() {
                text.push('\n');
                text.push_str(&snapshot.to_text());
            }
            context.copy_text(text);
            self.status = "性能报告已复制到剪贴板".to_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framegate::GuiSmoke;
    use editor::{DEFAULT_FRAME_BUDGET_MS, EditorProfiler};
    use engine::host::ReadbackMetrics;
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
        let sections = performance_sections(&report, &layout, counts, None);

        let frame = section(&sections, "编辑器帧");
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
        let sections = performance_sections(
            &report,
            &LayoutCacheStats::default(),
            Default::default(),
            None,
        );

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
        let sections = performance_sections(
            &report,
            &LayoutCacheStats::default(),
            Default::default(),
            None,
        );
        assert_eq!(sections.len(), 8, "七个编辑器分组 + 一个运行时占位分组");
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
        let runtime = section(&sections, "运行时（Play 预览）");
        assert_eq!(runtime.rows[0].raw_key, "runtime.status");
        assert_eq!(
            runtime.rows[0].value, "未运行（点击 Play 开始实时采样）",
            "未 Play 时必须明确说没在采样"
        );
        // `enabled=false` 的报告必须自曝"没在记录"，而不是显示一排 0 让人误解。
        let disabled = ProfilerReport::default();
        let sections = performance_sections(
            &disabled,
            &LayoutCacheStats::default(),
            Default::default(),
            None,
        );
        let row = section(&sections, "编辑器帧")
            .rows
            .iter()
            .find(|row| row.raw_key == "enabled")
            .expect("帧分组必须有记录开关");
        assert_eq!(row.value, "否");
        assert!(row.warn);
    }

    /// 运行时分组必须覆盖用户点名的全部关键读数，并给出可断言的原始键。
    #[test]
    fn runtime_sections_cover_frame_render_resource_and_memory() {
        let snapshot = RuntimePerfSnapshot {
            target: "Play 预览",
            resolution: (1280, 720),
            frames: 120,
            fps: 58.5,
            frame_ms: 17.1,
            update_ms: 4.2,
            lua_ms: 1.1,
            render_cpu_ms: 2.3,
            render_gpu_ms: Some(3.4),
            gpu_timing_note: None,
            draw_calls: 42,
            set_pass_calls: 6,
            instances: 18,
            triangles: 1200,
            vertices: 3600,
            dropped_draws: 0,
            passes: vec!["clear", "legacy_2d_overlay"],
            loaded_textures: 12,
            font_atlas_textures: 1,
            texture_bytes: 4 * 1024 * 1024,
            texture_identity_hits: 100,
            texture_identity_misses: 2,
            cached_meshes: 3,
            mesh_bytes: 2 * 1024 * 1024,
            mesh_identity_hits: 50,
            mesh_identity_misses: 1,
            async_mode: false,
            async_workers: 0,
            async_pending: 0,
            async_completed: 15,
            vertex_buffer_capacity: 1024 * 1024,
            vertex_buffer_growths: 1,
            vertex_bytes_peak: 900 * 1024,
            readback: ReadbackMetrics {
                slot_count: 3,
                dropped: 1,
                ..Default::default()
            },
            process_memory_mib: Some(512.0),
            asset_entries: 20,
            scene_entities: 40,
            runtime_entities: 43,
        };
        let report = sampled_report();
        let sections = performance_sections(
            &report,
            &LayoutCacheStats::default(),
            Default::default(),
            Some(&snapshot),
        );
        assert_eq!(sections.len(), 11, "七个编辑器分组 + 四个运行时分组");

        let frame = section(&sections, "运行时 · 帧");
        for (key, value) in [
            ("runtime.fps", "58.5 FPS"),
            ("runtime.frame_ms", "17.10 ms"),
            ("runtime.render_cpu_ms", "2.30 ms"),
            ("runtime.render_gpu_ms", "3.40 ms"),
            ("runtime.update_ms", "4.20 ms"),
            ("runtime.lua_ms", "1.100 ms"),
            ("runtime.frames", "120"),
        ] {
            let row = frame
                .rows
                .iter()
                .find(|row| row.raw_key == key)
                .unwrap_or_else(|| panic!("运行时帧分组缺少 {key}"));
            assert_eq!(row.value, value);
        }

        let render = section(&sections, "运行时 · 渲染");
        for (key, value) in [
            ("runtime.draw_calls", "42"),
            ("runtime.set_pass_calls", "6"),
            ("runtime.instances", "18"),
            ("runtime.triangles", "1200"),
            ("runtime.vertices", "3600"),
            ("runtime.dropped_draws", "0"),
            ("runtime.pass_count", "2"),
            ("runtime.passes", "2 条：clear → legacy_2d_overlay"),
            ("runtime.readback_slots", "3 / 1"),
        ] {
            let row = render
                .rows
                .iter()
                .find(|row| row.raw_key == key)
                .unwrap_or_else(|| panic!("运行时渲染分组缺少 {key}"));
            assert_eq!(row.value, value, "{key} 的值不对");
        }
        assert!(
            render
                .rows
                .iter()
                .find(|row| row.raw_key == "runtime.readback_slots")
                .expect("回读槽")
                .warn,
            "回读丢弃必须标警示"
        );

        let resources = section(&sections, "运行时 · 资源与缓存");
        for (key, value) in [
            ("runtime.loaded_textures", "12"),
            ("runtime.texture_bytes", "4.00 MiB"),
            ("runtime.texture_identity_hit_rate", "98.0%"),
            ("runtime.cached_meshes", "3"),
            ("runtime.mesh_bytes", "2.00 MiB"),
            ("runtime.load_mode", "同步（编辑器预览 / 确定性输出）"),
            ("runtime.async_loads", "0 / 15"),
            ("runtime.asset_entries", "20"),
            ("runtime.entities", "40 / 43"),
        ] {
            let row = resources
                .rows
                .iter()
                .find(|row| row.raw_key == key)
                .unwrap_or_else(|| panic!("运行时资源分组缺少 {key}"));
            assert_eq!(row.value, value, "{key} 的值不对");
        }

        let memory = section(&sections, "运行时 · 内存");
        for (key, value) in [
            ("runtime.process_memory_mib", "512.0 MiB"),
            ("runtime.gpu_texture_bytes", "4.00 MiB"),
            ("runtime.gpu_mesh_bytes", "2.00 MiB"),
            ("runtime.gpu_vertex_buffer_bytes", "1.00 MiB"),
            ("runtime.known_gpu_bytes", "7.00 MiB"),
        ] {
            let row = memory
                .rows
                .iter()
                .find(|row| row.raw_key == key)
                .unwrap_or_else(|| panic!("运行时内存分组缺少 {key}"));
            assert_eq!(row.value, value, "{key} 的值不对");
        }
    }

    /// 低帧率/丢 draw/超预算必须标黄；GPU 不可用只给原因，不冒充数据。
    #[test]
    fn runtime_warnings_are_explicit() {
        let mut snapshot = RuntimePerfSnapshot {
            frames: 60,
            fps: 30.0,
            frame_ms: 33.0,
            draw_calls: 5000,
            set_pass_calls: 400,
            dropped_draws: 2,
            ..Default::default()
        };
        snapshot.gpu_timing_note = Some("不可用（设备未启用时间戳）".to_owned());
        let report = sampled_report();
        let sections = performance_sections(
            &report,
            &LayoutCacheStats::default(),
            Default::default(),
            Some(&snapshot),
        );
        let frame = section(&sections, "运行时 · 帧");
        assert!(
            frame
                .rows
                .iter()
                .find(|row| row.raw_key == "runtime.fps")
                .expect("FPS 行")
                .warn
        );
        assert!(
            frame
                .rows
                .iter()
                .find(|row| row.raw_key == "runtime.frame_ms")
                .expect("帧间隔行")
                .warn
        );
        assert_eq!(
            frame
                .rows
                .iter()
                .find(|row| row.raw_key == "runtime.render_gpu_ms")
                .expect("GPU 行")
                .value,
            "不可用（设备未启用时间戳）"
        );
        let render = section(&sections, "运行时 · 渲染");
        for key in [
            "runtime.draw_calls",
            "runtime.set_pass_calls",
            "runtime.dropped_draws",
        ] {
            assert!(
                render
                    .rows
                    .iter()
                    .find(|row| row.raw_key == key)
                    .unwrap_or_else(|| panic!("缺少 {key}"))
                    .warn,
                "{key} 越过阈值必须标警示"
            );
        }
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
