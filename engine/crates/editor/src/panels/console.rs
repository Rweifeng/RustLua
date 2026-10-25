//! 行4 控制台：实时日志（LogSink）+ 编辑器提示 + Play 预览诊断。

use crate::{ConsoleLogLevelFilter, ConsoleTab, EditorApp};
use eframe::egui;

/// 左侧分类导航图标（egui 内嵌 emoji 字体渲染）。
const EDITOR_LOG_ICON: &str = "⚙";
const LUA_LOG_ICON: &str = "🕹";

/// Base console row height (single line, no wrapping): a uniform height is what lets
/// `show_rows` draw only the rows inside the viewport, which is what keeps 100k-entry logs
/// from freezing the main thread.
const CONSOLE_ROW_HEIGHT: f32 = 16.0;

/// Effective console row height for the current style: the monospace line height plus a
/// small margin, so raised zoom / larger fonts cannot make text taller than its row.
pub(crate) fn console_row_height(ui: &egui::Ui) -> f32 {
    let text_height = ui.text_style_height(&egui::TextStyle::Monospace);
    (text_height + 2.0).max(CONSOLE_ROW_HEIGHT * 0.5)
}

/// Layout probe: tallest console row content produced in a frame (see
/// [`crate::panels::hierarchy::RowHeightProbe`]).
pub(crate) type RowHeightProbe = crate::panels::hierarchy::RowHeightProbe;

/// 控制台行来源。索引只记来源与序号，不复制文本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConsoleSource {
    /// tracing `LogSink` 行。
    Sink,
    /// 编辑器侧提示。
    Notice,
    /// Game View / UI 预览诊断。
    Preview,
}

/// 一行控制台内容：来源 + 源行号 + 该源行内的子行号（源行含换行时按物理行拆开）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ConsoleRowRef {
    source: ConsoleSource,
    index: usize,
    sub: usize,
}

impl ConsoleRowRef {
    fn new(source: ConsoleSource, index: usize, sub: usize) -> Self {
        Self { source, index, sub }
    }
}

/// 控制台行索引缓存键：任一影响可见集合的输入变化都要重建。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ConsoleRowKey {
    tab: Option<ConsoleTab>,
    filter: String,
    level: Option<ConsoleLogLevelFilter>,
    sink_generation: u64,
    notice_len: usize,
    preview_len: usize,
}

/// 缓存的行索引。文本本身仍留在各自的有界缓冲里，这里只持有 (来源, 序号)。
#[derive(Default)]
pub(crate) struct ConsoleRowCache {
    key: ConsoleRowKey,
    rows: Vec<ConsoleRowRef>,
    valid: bool,
}

impl ConsoleRowCache {
    /// 上次构建的行数（远端行数为 0 表示还没构建过）。
    pub(crate) fn len(&self) -> usize {
        if self.valid { self.rows.len() } else { 0 }
    }
}

/// 控制台本帧的文本来源快照（诊断需要 `&mut` 解析，故先收集为自有字符串）。
#[derive(Default)]
struct ConsoleSources {
    sink: Vec<String>,
    notices: Vec<String>,
    preview: Vec<(String, ConsoleLogLevel)>,
}

impl ConsoleSources {
    fn line(&self, source: ConsoleSource, index: usize) -> Option<&str> {
        match source {
            ConsoleSource::Sink => self.sink.get(index).map(String::as_str),
            ConsoleSource::Notice => self.notices.get(index).map(String::as_str),
            ConsoleSource::Preview => self.preview.get(index).map(|(line, _)| line.as_str()),
        }
    }

    fn level(&self, source: ConsoleSource, index: usize) -> ConsoleLogLevel {
        match source {
            ConsoleSource::Sink => self
                .sink
                .get(index)
                .map_or(ConsoleLogLevel::Info, |line| log_level(line)),
            ConsoleSource::Notice => self
                .notices
                .get(index)
                .map_or(ConsoleLogLevel::Info, |line| log_level(line)),
            ConsoleSource::Preview => self
                .preview
                .get(index)
                .map_or(ConsoleLogLevel::Info, |(_, level)| *level),
        }
    }
}

impl EditorApp {
    /// 行4 控制台面板（全宽底部，先于左右面板显示以获得整行宽度）。
    ///
    /// 高度固定：滚动区填满剩余空间，面板不再随日志行数伸缩（M-固定高度）。
    pub(crate) fn draw_lua_console_panel(&mut self, context: &egui::Context) {
        // 高度来自持久化布局（console 面板在布局里复用 PANEL_CONSOLE 的宽度槽位记录高度），
        // 并且用 `exact_height` **钉死**。
        //
        // 为什么不用 `default_height`：egui 的 `TopBottomPanel` 把「内容矩形」存成下一帧的
        // 高度。控制台内部是「表头行 + 日志滚动区」两级布局，滚动区会把内容矩形顶出分配高度，
        // 于是每帧放大 ~2px（`内容变高 → 面板变高 → 可用高度变大`），实测 60 帧后画布被吃掉
        // 一百多像素。`exact_height` 切断这条正反馈；代价是面板不再可拖动改高（面板尺寸本来
        // 也不参与布局持久化——`capture_layout` 只写可见性，所以重启后本来也回默认高度）。
        let panel_height = self
            .layout
            .panel_width(editor::PANEL_CONSOLE)
            .clamp(60.0, 520.0);
        egui::TopBottomPanel::bottom("console")
            .exact_height(panel_height)
            .show(context, |ui| {
                self.draw_console_body(ui);
            });
    }

    /// 控制台内容（M27-00 薄切片：从 `draw_lua_console_panel` 抽出的 `&mut Ui` 入口，
    /// 供 egui_dock `PanelUi` 回调复用；原 TopBottomPanel 外壳绘制路径不变）。
    pub(crate) fn draw_console_body(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            // 左侧：日志分类导航（图标按钮，切换编辑器日志 / 游戏 Lua 日志）。
            self.console_tab_rail(ui);
            ui.separator();
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.console_filter)
                            .desired_width(160.0)
                            .hint_text("过滤日志…"),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("清空").clicked() {
                            self.log_sink.clear();
                            self.console_notices.clear();
                        }
                        ui.selectable_value(
                            &mut self.console_log_level_filter,
                            ConsoleLogLevelFilter::Error,
                            "错误",
                        );
                        ui.selectable_value(
                            &mut self.console_log_level_filter,
                            ConsoleLogLevelFilter::Warning,
                            "警告",
                        );
                        ui.selectable_value(
                            &mut self.console_log_level_filter,
                            ConsoleLogLevelFilter::Info,
                            "信息",
                        );
                        ui.selectable_value(
                            &mut self.console_log_level_filter,
                            ConsoleLogLevelFilter::All,
                            "全部",
                        );
                    });
                });
                // 滚动区填满剩余空间，面板高度保持固定，仅滚动内部内容。
                let log_height = ui.available_height();
                egui::ScrollArea::vertical()
                    .id_salt("console_log_body")
                    .auto_shrink([false; 2])
                    .hscroll(false)
                    .max_height(log_height)
                    .show(ui, |ui| {
                        self.draw_console_log_body(ui);
                    });
            });
        });
    }

    /// 左侧日志分类导航（图标按钮 + tooltip）。
    fn console_tab_rail(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.set_min_width(28.0);
            let editor = egui::Button::new(egui::RichText::new(EDITOR_LOG_ICON).size(20.0))
                .selected(self.console_tab == ConsoleTab::Editor);
            if ui.add(editor).on_hover_text("编辑器日志").clicked() {
                self.console_tab = ConsoleTab::Editor;
            }
            ui.add_space(4.0);
            let lua = egui::Button::new(egui::RichText::new(LUA_LOG_ICON).size(20.0))
                .selected(self.console_tab == ConsoleTab::Lua);
            if ui.add(lua).on_hover_text("游戏 Lua 日志").clicked() {
                self.console_tab = ConsoleTab::Lua;
            }
        });
    }

    /// 按左侧分类绘制日志主体。
    ///
    /// 做法与 Hierarchy 一致：先算出**行索引**（按来源+序号，不复制文本），再按固定行高
    /// 虚拟滚动，只绘制视口内的行。索引按 (分类, 过滤词, 级别, 各来源代数/长度) 缓存，
    /// 因此 100k 级日志下滚动与过滤都不会每帧重建整表。
    fn draw_console_log_body(&mut self, ui: &mut egui::Ui) {
        let filter = self.console_filter.trim().to_lowercase();
        let tab = self.console_tab;
        let level_filter = self.console_log_level_filter;
        // 预览诊断需要按 kind 定级，先收集成自有文本（数量远小于日志行）。
        let preview_diagnostics: Vec<(String, ConsoleLogLevel)> = self
            .preview_diagnostics
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                let level = if diagnostic.kind
                    == rustlua_infra::preview_diagnostics::PreviewErrorKind::Layout
                {
                    ConsoleLogLevel::Warning
                } else {
                    ConsoleLogLevel::Error
                };
                (diagnostic.summary(), level)
            })
            .collect();
        let sources = ConsoleSources {
            sink: self.log_sink.snapshot(),
            notices: self.console_notices.clone(),
            preview: preview_diagnostics,
        };
        let key = ConsoleRowKey {
            tab: Some(tab),
            filter: filter.clone(),
            level: Some(level_filter),
            sink_generation: self.log_sink.generation(),
            notice_len: sources.notices.len(),
            preview_len: sources.preview.len(),
        };
        let row_count =
            self.rebuild_console_rows_if_needed(&key, &sources, tab, &filter, level_filter);
        // 取出（而非借用）行索引：绘制期间还需要不可变借用各来源文本。
        let rows = std::mem::take(&mut self.console_rows_cache.rows);
        self.console_row_probe.reset();
        // 点击跳转在帧末统一执行：跳转可能切换文档，不能在绘制中途改状态。
        let mut pending_jump: Option<editor::DiagnosticLocation> = None;
        for index in 0..row_count {
            if pending_jump.is_some() {
                break;
            }
            let Some(row) = rows.get(index).copied() else {
                continue;
            };
            let Some(text) = console_row_text(&sources, tab, row) else {
                continue;
            };
            let level = sources.level(row.source, row.index);
            let response = draw_log_line(ui, text, level);
            self.console_row_probe.observe(response.rect.height());
            if let Some(location) = editor::parse_location(text) {
                let response = response.interact(egui::Sense::click());
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() {
                    pending_jump = Some(location);
                }
            }
        }
        self.console_rows_cache.rows = rows;
        if let Some(location) = pending_jump {
            self.profiler.bump("console_diagnostic_jump");
            self.jump_to_diagnostic(&location);
        }
    }

    /// 当索引键变化时重建控制台行索引（过滤 + 级别 + 分类 + 物理行拆分）。
    fn rebuild_console_rows_if_needed(
        &mut self,
        key: &ConsoleRowKey,
        sources: &ConsoleSources,
        tab: ConsoleTab,
        filter: &str,
        level_filter: ConsoleLogLevelFilter,
    ) -> usize {
        if self.console_rows_cache.valid && self.console_rows_cache.key == *key {
            self.profiler.bump("console_index_cache_hit");
            return self.console_rows_cache.rows.len();
        }
        self.profiler.bump("console_index_cache_miss");
        let rows = build_console_rows(sources, tab, filter, level_filter);
        let row_count = rows.len();
        self.console_rows_cache.key = key.clone();
        self.console_rows_cache.rows = rows;
        self.console_rows_cache.valid = true;
        row_count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConsoleLogLevel {
    Info,
    Warning,
    Error,
}

impl ConsoleLogLevelFilter {
    fn matches(self, level: ConsoleLogLevel) -> bool {
        match self {
            Self::All => true,
            Self::Info => level == ConsoleLogLevel::Info,
            Self::Warning => level == ConsoleLogLevel::Warning,
            Self::Error => level == ConsoleLogLevel::Error,
        }
    }
}

/// tracing fmt 默认格式：`<timestamp> <LEVEL> <target>: <message>`，第三段即 target。
/// Lua 游戏逻辑经 `log.info/warn/error` 以 `target: "script"` 输出，据此归入 Lua 日志分类。
fn is_lua_log_line(line: &str) -> bool {
    line.split_whitespace().nth(2) == Some("script:")
}

/// 精简 Lua `log.*` 的 tracing 行：取 `message=` 字段值作为纯消息；
/// 非 `log.*` 消息（如引擎用 script target 的告警）没有该字段则原样返回。
fn lua_log_message(line: &str) -> &str {
    if let Some(index) = line.rfind("message=") {
        &line[index + "message=".len()..]
    } else {
        line
    }
}

fn log_level(line: &str) -> ConsoleLogLevel {
    if line.contains(" ERROR ") || line.contains("ERROR") {
        ConsoleLogLevel::Error
    } else if line.contains(" WARN ") || line.contains("WARN") || line.contains("WARNING") {
        ConsoleLogLevel::Warning
    } else {
        ConsoleLogLevel::Info
    }
}

/// 构建控制台行索引：按分类与级别筛选来源行，按过滤词匹配，并把多行源行拆成物理行。
///
/// 生产路径（`rebuild_console_rows_if_needed`）与单元测试共用这一个实现，
/// 避免"测试里复刻一份索引逻辑"造成的假绿灯。
fn build_console_rows(
    sources: &ConsoleSources,
    tab: ConsoleTab,
    filter: &str,
    level_filter: ConsoleLogLevelFilter,
) -> Vec<ConsoleRowRef> {
    let mut rows = Vec::new();
    for (source, count) in [
        (ConsoleSource::Sink, sources.sink.len()),
        (ConsoleSource::Notice, sources.notices.len()),
        (ConsoleSource::Preview, sources.preview.len()),
    ] {
        for index in 0..count {
            let Some(line) = sources.line(source, index) else {
                continue;
            };
            // 分类过滤（逐分支语义显式列出）：
            // - 编辑器日志：非 Lua 的 tracing 行 + 编辑器提示 + 预览诊断；
            // - Lua 日志：Lua target 的 tracing 行。
            let lua_line = is_lua_log_line(line);
            let included = match (tab, source) {
                (ConsoleTab::Editor, ConsoleSource::Sink) => !lua_line,
                (ConsoleTab::Lua, ConsoleSource::Sink) => lua_line,
                (ConsoleTab::Editor, ConsoleSource::Notice) => true,
                (ConsoleTab::Lua, ConsoleSource::Notice) => false,
                (ConsoleTab::Editor, ConsoleSource::Preview) => true,
                (ConsoleTab::Lua, ConsoleSource::Preview) => false,
            };
            if !included || !level_filter.matches(sources.level(source, index)) {
                continue;
            }
            // Lua sink 行按 `message=` 后的纯消息匹配与显示（与旧行为一致）。
            let searchable = if tab == ConsoleTab::Lua && source == ConsoleSource::Sink {
                lua_log_message(line)
            } else {
                line
            };
            if !text_matches_filter(searchable, filter) {
                continue;
            }
            let multiline = searchable.contains('\n');
            for (sub, physical) in searchable.split('\n').enumerate() {
                if multiline && physical.trim().is_empty() {
                    continue;
                }
                rows.push(ConsoleRowRef::new(source, index, sub));
            }
        }
    }
    rows
}

/// 解析一行控制台行的显示文本（含分类相关的裁剪与物理行拆分）。
fn console_row_text(sources: &ConsoleSources, tab: ConsoleTab, row: ConsoleRowRef) -> Option<&str> {
    let line = sources.line(row.source, row.index)?;
    let text = if tab == ConsoleTab::Lua && row.source == ConsoleSource::Sink {
        lua_log_message(line)
    } else {
        line
    };
    text.split('\n').nth(row.sub)
}

fn text_matches_filter(line: &str, filter: &str) -> bool {
    filter.is_empty() || line.to_lowercase().contains(filter)
}

/// 绘制一行日志；返回该行响应（`rect.height()` 供布局探针，`interact` 供点击跳转）。
fn draw_log_line(ui: &mut egui::Ui, line: &str, level: ConsoleLogLevel) -> egui::Response {
    let color = match level {
        ConsoleLogLevel::Info => ui.visuals().text_color(),
        ConsoleLogLevel::Warning => egui::Color32::YELLOW,
        ConsoleLogLevel::Error => egui::Color32::RED,
    };
    // 单行行高固定：不换行（超出宽度裁切），完整内容经悬停提示查看。
    // 这是虚拟滚动的前提——换行会让行高不一致，无法按固定行高跳转绘制。
    let response = ui.add(
        egui::Label::new(egui::RichText::new(line).monospace().color(color))
            .wrap_mode(egui::TextWrapMode::Truncate)
            .selectable(true)
            .sense(egui::Sense::click()),
    );
    if response.clicked() {
        ui.ctx().copy_text(line.to_string());
    }
    if line.chars().count() > 80 {
        response.clone().on_hover_text(line);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 索引构建：分类 + 级别 + 过滤 + 物理行拆分的组合语义。
    #[test]
    fn console_row_index_respects_tab_level_filter_and_multiline() {
        let sources = ConsoleSources {
            sink: vec![
                "2026-01-01T00:00:00Z  INFO editor: [editor] ready\nsecond line".to_owned(),
                "2026-01-01T00:00:00Z ERROR script: Lua log.error message=low health".to_owned(),
                "2026-01-01T00:00:01Z ERROR editor: [editor] failed".to_owned(),
            ],
            notices: vec!["Scene View 回读完成".to_owned()],
            preview: vec![("layout overflow".to_owned(), ConsoleLogLevel::Warning)],
        };

        // 编辑器分类 + 全部级别：sink 的非 Lua 行（首行多行拆 2 物理行）+ notice + 预览诊断。
        let editor =
            build_console_rows(&sources, ConsoleTab::Editor, "", ConsoleLogLevelFilter::All);
        assert_eq!(
            editor.len(),
            5,
            "editor tab: 3 sink physical + 1 notice + 1 preview"
        );
        assert_eq!(editor[0], ConsoleRowRef::new(ConsoleSource::Sink, 0, 0));
        assert_eq!(editor[1], ConsoleRowRef::new(ConsoleSource::Sink, 0, 1));
        assert_eq!(editor[2], ConsoleRowRef::new(ConsoleSource::Sink, 2, 0));
        assert_eq!(editor[3], ConsoleRowRef::new(ConsoleSource::Notice, 0, 0));
        assert_eq!(editor[4], ConsoleRowRef::new(ConsoleSource::Preview, 0, 0));
        // 多行源行的第二行文本按子行解析。
        assert_eq!(
            console_row_text(&sources, ConsoleTab::Editor, editor[1]),
            Some("second line")
        );

        // Lua 分类：只有 Lua target 的 sink 行（提示与预览诊断只进编辑器分类）。
        let lua = build_console_rows(&sources, ConsoleTab::Lua, "", ConsoleLogLevelFilter::All);
        assert_eq!(lua.len(), 1, "lua tab: 1 sink");
        assert_eq!(lua[0], ConsoleRowRef::new(ConsoleSource::Sink, 1, 0));
        // Lua sink 行显示时裁掉 tracing 前缀。
        assert_eq!(
            console_row_text(&sources, ConsoleTab::Lua, lua[0]),
            Some("low health")
        );

        // 级别过滤：Lua 分类只看 Error → 只剩那条 ERROR sink 行。
        let errors =
            build_console_rows(&sources, ConsoleTab::Lua, "", ConsoleLogLevelFilter::Error);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].source, ConsoleSource::Sink);

        // 过滤词命中 Lua 消息体（而不是整条 tracing 前缀）。
        let filtered = build_console_rows(
            &sources,
            ConsoleTab::Lua,
            "health",
            ConsoleLogLevelFilter::All,
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0], ConsoleRowRef::new(ConsoleSource::Sink, 1, 0));
    }

    /// 缓存键覆盖所有影响可见集合的输入。
    #[test]
    fn console_row_key_tracks_every_input() {
        let base = ConsoleRowKey {
            tab: Some(ConsoleTab::Editor),
            filter: String::new(),
            level: Some(ConsoleLogLevelFilter::All),
            sink_generation: 1,
            notice_len: 0,
            preview_len: 0,
        };
        assert_eq!(
            base,
            ConsoleRowKey {
                sink_generation: 1,
                ..base.clone()
            }
        );
        for changed in [
            ConsoleRowKey {
                sink_generation: 2,
                ..base.clone()
            },
            ConsoleRowKey {
                tab: Some(ConsoleTab::Lua),
                ..base.clone()
            },
            ConsoleRowKey {
                filter: "x".to_owned(),
                ..base.clone()
            },
            ConsoleRowKey {
                level: Some(ConsoleLogLevelFilter::Error),
                ..base.clone()
            },
            ConsoleRowKey {
                notice_len: 1,
                ..base.clone()
            },
        ] {
            assert_ne!(base, changed, "任一输入变化都必须视为缓存失效");
        }
    }

    #[test]
    fn console_row_height_never_collapses_below_control_needs() {
        let context = egui::Context::default();
        let raw = egui::RawInput::default();
        let mut height = 0.0_f32;
        let _ = context.run(raw, |context| {
            egui::CentralPanel::default().show(context, |ui| {
                height = console_row_height(ui);
            });
        });
        assert!(
            height >= CONSOLE_ROW_HEIGHT * 0.5,
            "控制台行高不得塌陷：{height}"
        );
        assert!(height >= 14.0, "行高必须容纳 monospace 文本：{height}");
    }

    /// 窄面板下日志行必须裁切而不是折行：虚拟滚动按固定行高跳转，
    /// 折成两行会让内容高度（30pt）超过行高（~17pt），相邻行重叠。
    #[test]
    fn console_log_line_truncates_at_narrow_width_instead_of_wrapping() {
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(160.0, 640.0),
            )),
            ..Default::default()
        };
        let long = "2026-01-01T00:00:00Z  INFO editor: a long console line ".repeat(6);
        let mut content_height = 0.0_f32;
        let mut row_height = 0.0_f32;
        let _ = context.run(raw, |context| {
            egui::CentralPanel::default().show(context, |ui| {
                ui.set_max_width(160.0);
                row_height = console_row_height(ui);
                content_height = draw_log_line(ui, &long, ConsoleLogLevel::Info)
                    .rect
                    .height();
            });
        });
        assert!(content_height > 0.0, "测试行必须真的被绘制");
        assert!(
            content_height <= row_height + 0.01,
            "窄宽日志行折行了：内容 {content_height:.2} > 行高 {row_height:.2}"
        );
    }

    /// 布局探针语义：取最大值、可重置（门槛判定依赖这三点）。
    #[test]
    fn console_row_probe_tracks_max_and_resets() {
        let mut probe = RowHeightProbe::default();
        probe.observe(15.0);
        probe.observe(18.25);
        assert_eq!(probe.max_content_height(), 18.25);
        assert_eq!(probe.rows(), 2);
        probe.reset();
        assert_eq!(probe.max_content_height(), 0.0);
        assert_eq!(probe.rows(), 0);
    }

    /// 分类：Lua `log.*`（target `script`）进 Lua 日志；编辑器/引擎日志排除在外。
    #[test]
    fn lua_log_lines_are_classified_by_script_target() {
        assert!(is_lua_log_line(
            "2026-08-21T15:35:23.670753Z  INFO script: Lua log.info project script loaded"
        ));
        assert!(is_lua_log_line(
            "2026-08-21T15:49:57.358650Z  INFO script: Lua log.info tilemap loaded 6x4"
        ));
        assert!(!is_lua_log_line(
            "2026-08-21T15:35:23.668576Z  INFO game: [render] initialized"
        ));
        assert!(!is_lua_log_line(
            "2026-08-21T15:35:23.669980Z  INFO ecs: [ecs] world initialized"
        ));
    }

    /// Lua `log.*` 行取 `message=` 字段值；其他 script target 行原样保留。
    #[test]
    fn lua_log_message_extracts_field_value() {
        assert_eq!(
            lua_log_message(
                "2026-08-21T15:35:23.670753Z  INFO script: Lua log.info message=ui_demo scene ready"
            ),
            "ui_demo scene ready"
        );
        let callback_failure =
            "2026-08-21T15:35:23.670753Z  WARN script: on_day_start callback failed error=boom";
        assert_eq!(lua_log_message(callback_failure), callback_failure);
    }
}
