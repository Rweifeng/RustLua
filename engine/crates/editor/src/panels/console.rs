//! 行4 控制台：实时日志（LogSink）+ Lua 运行输出 + 诊断。

use crate::{ConsoleLogLevelFilter, ConsoleTab, EditorApp};
use eframe::egui;
use engine::core::script::{LuaRuntime, ScriptDiagnostic};

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
    /// Lua 控制台输出。
    Output,
    /// Game View / UI 预览诊断。
    Preview,
    /// 脚本诊断。
    Script,
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
    output_len: usize,
    preview_len: usize,
    script_len: usize,
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
    output: Vec<String>,
    preview: Vec<(String, ConsoleLogLevel)>,
    script: Vec<String>,
}

impl ConsoleSources {
    fn line(&self, source: ConsoleSource, index: usize) -> Option<&str> {
        match source {
            ConsoleSource::Sink => self.sink.get(index).map(String::as_str),
            ConsoleSource::Notice => self.notices.get(index).map(String::as_str),
            ConsoleSource::Output => self.output.get(index).map(String::as_str),
            ConsoleSource::Preview => self.preview.get(index).map(|(line, _)| line.as_str()),
            ConsoleSource::Script => self.script.get(index).map(String::as_str),
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
            ConsoleSource::Output => self
                .output
                .get(index)
                .map_or(ConsoleLogLevel::Info, |line| output_level(line)),
            ConsoleSource::Preview => self
                .preview
                .get(index)
                .map_or(ConsoleLogLevel::Info, |(_, level)| *level),
            ConsoleSource::Script => ConsoleLogLevel::Error,
        }
    }
}

impl EditorApp {
    /// 行4 控制台面板（全宽底部，先于左右面板显示以获得整行宽度）。
    ///
    /// 高度固定：滚动区填满剩余空间，面板不再随日志行数伸缩（M-固定高度）。
    pub(crate) fn draw_lua_console_panel(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            // 高度来自持久化布局（console 面板在布局里复用 PANEL_CONSOLE 的宽度槽位记录高度）。
            .default_height(self.layout.panel_width(editor::PANEL_CONSOLE))
            .min_height(60.0)
            .max_height(520.0)
            .show(context, |ui| {
                ui.horizontal_top(|ui| {
                    // 左侧：日志分类导航（图标按钮，切换编辑器日志 / 游戏 Lua 日志）。
                    self.console_tab_rail(ui);
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let project_name = self
                                .project
                                .root
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("项目");
                            ui.heading(format!("控制台 — {project_name}"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.console_filter)
                                    .desired_width(160.0)
                                    .hint_text("过滤日志…"),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("清空").clicked() {
                                    self.clear_console();
                                    self.log_sink.clear();
                                    self.console_notices.clear();
                                }
                                ui.selectable_value(
                                    &mut self.console_log_level_filter,
                                    ConsoleLogLevelFilter::Error,
                                    "Error",
                                );
                                ui.selectable_value(
                                    &mut self.console_log_level_filter,
                                    ConsoleLogLevelFilter::Warning,
                                    "Warning",
                                );
                                ui.selectable_value(
                                    &mut self.console_log_level_filter,
                                    ConsoleLogLevelFilter::Info,
                                    "Info",
                                );
                                ui.selectable_value(
                                    &mut self.console_log_level_filter,
                                    ConsoleLogLevelFilter::All,
                                    "全部",
                                );
                            });
                        });
                        // 滚动区填满剩余空间，面板高度保持固定，仅滚动内部内容。
                        egui::ScrollArea::vertical()
                            .auto_shrink([false; 2])
                            .hscroll(false)
                            .show(ui, |ui| {
                                self.draw_console_log_body(ui);
                            });
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
        // 诊断需要 &mut 解析，先收集成自有文本（数量远小于日志行）。
        let script_diagnostics: Vec<String> = self
            .console_diagnostics()
            .iter()
            .map(format_diagnostic)
            .collect();
        let preview_diagnostics: Vec<(String, ConsoleLogLevel)> = self
            .preview_diagnostics
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                let level = if diagnostic.kind
                    == engine::core::preview_diagnostics::PreviewErrorKind::Layout
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
            output: self.console_output.clone(),
            preview: preview_diagnostics,
            script: script_diagnostics,
        };
        let key = ConsoleRowKey {
            tab: Some(tab),
            filter: filter.clone(),
            level: Some(level_filter),
            sink_generation: self.log_sink.generation(),
            notice_len: sources.notices.len(),
            output_len: sources.output.len(),
            preview_len: sources.preview.len(),
            script_len: sources.script.len(),
        };
        let row_count =
            self.rebuild_console_rows_if_needed(&key, &sources, tab, &filter, level_filter);
        // 取出（而非借用）行索引：绘制期间还需要不可变借用各来源文本。
        let rows = std::mem::take(&mut self.console_rows_cache.rows);
        let row_height = console_row_height(ui);
        let body_height = ui.available_height().max(row_height);
        self.console_row_probe.reset();
        // 点击跳转在帧末统一执行：跳转可能切换文档，不能在绘制中途改状态。
        let mut pending_jump: Option<editor::DiagnosticLocation> = None;
        egui::ScrollArea::vertical()
            .id_salt("console_log_body")
            .auto_shrink([false; 2])
            .max_height(body_height)
            .show_rows(ui, row_height, row_count, |ui, visible| {
                for index in visible {
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
                    // 可定位的行可点击：解析出的 路径/行号/实体 交给统一跳转入口。
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
            });
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

    fn ensure_lua_runtime(&mut self) -> Result<&mut LuaRuntime, String> {
        if self.lua_runtime.is_none() {
            self.lua_runtime = Some(LuaRuntime::new().map_err(|error| error.to_string())?);
        }
        self.lua_runtime
            .as_mut()
            .ok_or_else(|| "Lua VM 未初始化".to_owned())
    }

    fn run_console_command(&mut self) {
        let source = self.console_input.trim().to_owned();
        if source.is_empty() {
            return;
        }
        let result = self.ensure_lua_runtime().and_then(|runtime| {
            runtime
                .execute_console(&source)
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(value) => self.push_console_output(format!("> {source}\n{value}")),
            Err(error) => {
                self.status = error.clone();
                self.push_console_output(format!("> {source}\n错误：{error}"));
            }
        }
        self.console_input.clear();
    }

    fn clear_console(&mut self) {
        self.console_output.clear();
        if let Some(runtime) = self.lua_runtime.as_mut() {
            runtime.clear_diagnostics();
        }
    }

    fn run_selected_script(&mut self) {
        let script = self.lua_script.trim().to_owned();
        if script.is_empty() {
            self.status = "当前实体没有 Lua 脚本引用".to_owned();
            return;
        }
        let path = self.project.root.join(&script);
        let result = self.ensure_lua_runtime().and_then(|runtime| {
            runtime
                .execute_project_script(&path)
                .and_then(|_| runtime.call_update_debug())
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(value) => {
                self.status = format!("Lua 脚本运行成功：{script}");
                self.push_console_output(format!("运行 {script}\n{value}"));
            }
            Err(error) => {
                self.status = error.clone();
                self.push_console_output(format!("运行 {script}\n错误：{error}"));
            }
        }
    }

    fn console_diagnostics(&mut self) -> Vec<ScriptDiagnostic> {
        let filter = self.console_filter.trim().to_owned();
        let Some(runtime) = self.lua_runtime.as_mut() else {
            return Vec::new();
        };
        runtime.set_debug_filter((!filter.is_empty()).then_some(filter));
        runtime.diagnostics()
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
        (ConsoleSource::Output, sources.output.len()),
        (ConsoleSource::Preview, sources.preview.len()),
        (ConsoleSource::Script, sources.script.len()),
    ] {
        for index in 0..count {
            let Some(line) = sources.line(source, index) else {
                continue;
            };
            // 分类过滤（与重构前逐分支语义一一对应，全部 10 种组合都显式列出）：
            // - 编辑器日志：非 Lua 的 tracing 行 + 编辑器提示 + 预览诊断；
            // - Lua 日志：Lua target 的 tracing 行 + 控制台输出 + 脚本诊断。
            let lua_line = is_lua_log_line(line);
            let included = match (tab, source) {
                (ConsoleTab::Editor, ConsoleSource::Sink) => !lua_line,
                (ConsoleTab::Lua, ConsoleSource::Sink) => lua_line,
                (ConsoleTab::Editor, ConsoleSource::Notice) => true,
                (ConsoleTab::Lua, ConsoleSource::Notice) => false,
                (ConsoleTab::Editor, ConsoleSource::Preview) => true,
                (ConsoleTab::Lua, ConsoleSource::Preview) => false,
                (ConsoleTab::Editor, ConsoleSource::Output) => false,
                (ConsoleTab::Lua, ConsoleSource::Output) => true,
                (ConsoleTab::Editor, ConsoleSource::Script) => false,
                (ConsoleTab::Lua, ConsoleSource::Script) => true,
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

fn output_level(line: &str) -> ConsoleLogLevel {
    if line.contains("错误：") || line.contains("Error") || line.contains("error") {
        ConsoleLogLevel::Error
    } else if line.contains("警告：") || line.contains("Warning") || line.contains("warning") {
        ConsoleLogLevel::Warning
    } else {
        ConsoleLogLevel::Info
    }
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
            .wrap_mode(egui::TextWrapMode::Extend)
            .selectable(false),
    );
    if line.chars().count() > 80 {
        response.clone().on_hover_text(line);
    }
    response
}

fn format_diagnostic(diagnostic: &ScriptDiagnostic) -> String {
    let location = diagnostic
        .line
        .map_or_else(|| "未知行".to_owned(), |line| format!("第 {line} 行"));
    let base = format!(
        "[{}] {} {}：{}（重复 {} 次）",
        diagnostic.phase,
        diagnostic.path.display(),
        location,
        diagnostic.message,
        diagnostic.occurrences
    );
    match suggest_action(diagnostic) {
        Some(action) => format!("{base}\n    建议：{action}"),
        None => base,
    }
}

/// M11-05 / R-09：为脚本诊断提供启发式建议动作，让错误"可定位来源 + 可采取动作"。
/// 基于诊断阶段与消息关键字给出可执行的下一步，而非仅堆叠错误文本。
fn suggest_action(diagnostic: &ScriptDiagnostic) -> Option<String> {
    let message = diagnostic.message.to_lowercase();
    let action = match diagnostic.phase.as_str() {
        "load"
            if message.contains("找不到")
                || message.contains("无法打开")
                || message.contains("no such file") =>
        {
            "检查脚本文件是否存在，及路径与项目根目录的相对关系是否正确"
        }
        "load" => "检查 Lua 语法与脚本加载路径（脚本需能单独解析）",
        "runtime" if message.contains("attempt to") || message.contains("nil") => {
            "检查实体/变量是否已初始化——避免对 nil 调用方法或索引"
        }
        "runtime"
            if message.contains("invalid")
                || message.contains("无效")
                || message.contains("缺失") =>
        {
            "检查实体引用的资源/组件路径是否存在且有效"
        }
        "runtime" => "检查脚本运行时逻辑与实体引用",
        "api" => "检查 API 名称、参数类型与返回值约定",
        _ => return None,
    };
    Some(action.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn diagnostic(phase: &str, message: &str) -> ScriptDiagnostic {
        ScriptDiagnostic {
            path: PathBuf::from("scripts/main.lua"),
            phase: phase.to_owned(),
            message: message.to_owned(),
            line: Some(7),
            stack_trace: String::new(),
            occurrences: 1,
        }
    }

    /// M11-05：加载/运行/API 各阶段诊断都能给出建议动作，且输出包含阶段、路径、行与消息。
    #[test]
    fn diagnostics_include_phase_path_line_and_suggestion() {
        let cases = [
            ("load", "无法打开文件 scripts/player.lua"),
            ("load", "unexpected symbol near 'end'"),
            ("runtime", "attempt to index a nil value"),
            ("runtime", "无效的纹理资源 assets/tex.png"),
            ("runtime", "boom at 42"),
            ("api", "unknown field 'speed'"),
        ];
        for (phase, message) in cases {
            let diagnostic = diagnostic(phase, message);
            let formatted = format_diagnostic(&diagnostic);
            assert!(formatted.contains(&format!("[{phase}]")));
            assert!(formatted.contains("scripts/main.lua"));
            assert!(formatted.contains("第 7 行"));
            assert!(formatted.contains(message), "{formatted} 应包含消息");
            assert!(
                suggest_action(&diagnostic).is_some(),
                "{phase}/{message} 应给出建议动作"
            );
        }
    }

    #[test]
    fn unknown_diagnostic_has_no_suggestion_but_stays_structured() {
        let diagnostic = diagnostic("mystery", "weird");
        assert!(suggest_action(&diagnostic).is_none());
        let formatted = format_diagnostic(&diagnostic);
        assert!(formatted.contains("[mystery] scripts/main.lua 第 7 行"));
        assert!(!formatted.contains("建议"));
    }

    /// 索引构建：分类 + 级别 + 过滤 + 物理行拆分的组合语义。
    #[test]
    fn console_row_index_respects_tab_level_filter_and_multiline() {
        let sources = ConsoleSources {
            sink: vec![
                "2026-01-01T00:00:00Z  INFO editor: [editor] ready".to_owned(),
                "2026-01-01T00:00:00Z  WARN script: Lua log.warn message=low health".to_owned(),
                "2026-01-01T00:00:01Z ERROR editor: [editor] failed".to_owned(),
            ],
            notices: vec!["Scene View 回读完成".to_owned()],
            output: vec!["> print(1)\n1".to_owned()],
            preview: vec![("layout overflow".to_owned(), ConsoleLogLevel::Warning)],
            script: vec!["[runtime] scripts/main.lua 第 3 行：boom（重复 1 次）".to_owned()],
        };

        // 编辑器分类 + 全部级别：sink 的非 Lua 行 ×2 + notice + Game View 预览诊断；
        // 控制台输出与脚本诊断不进该分类（与重构前的分类语义一致）。
        let editor =
            build_console_rows(&sources, ConsoleTab::Editor, "", ConsoleLogLevelFilter::All);
        assert_eq!(editor.len(), 4, "editor tab: 2 sink + 1 notice + 1 preview");
        assert_eq!(editor[0], ConsoleRowRef::new(ConsoleSource::Sink, 0, 0));
        assert_eq!(editor[1], ConsoleRowRef::new(ConsoleSource::Sink, 2, 0));
        assert_eq!(editor[2], ConsoleRowRef::new(ConsoleSource::Notice, 0, 0));
        assert_eq!(editor[3], ConsoleRowRef::new(ConsoleSource::Preview, 0, 0));

        // Lua 分类：script sink 行 + 输出（多行拆 2 行）+ 脚本诊断；
        // 编辑器提示与预览诊断只在编辑器分类出现（保持重构前的分类语义）。
        let lua = build_console_rows(&sources, ConsoleTab::Lua, "", ConsoleLogLevelFilter::All);
        assert_eq!(lua.len(), 4, "lua tab: 1 sink + 2 output + 1 script");
        assert_eq!(lua[0], ConsoleRowRef::new(ConsoleSource::Sink, 1, 0));
        assert_eq!(lua[1], ConsoleRowRef::new(ConsoleSource::Output, 0, 0));
        assert_eq!(lua[2], ConsoleRowRef::new(ConsoleSource::Output, 0, 1));
        assert_eq!(lua[3], ConsoleRowRef::new(ConsoleSource::Script, 0, 0));
        // 多行源行的第二行文本按子行解析。
        assert_eq!(
            console_row_text(&sources, ConsoleTab::Lua, lua[2]),
            Some("1")
        );
        // Lua sink 行显示时裁掉 tracing 前缀。
        assert_eq!(
            console_row_text(&sources, ConsoleTab::Lua, lua[0]),
            Some("low health")
        );

        // 级别过滤：Lua 分类只看 Error → 只剩脚本诊断。
        let errors =
            build_console_rows(&sources, ConsoleTab::Lua, "", ConsoleLogLevelFilter::Error);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].source, ConsoleSource::Script);

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
            output_len: 0,
            preview_len: 0,
            script_len: 0,
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
                output_len: 1,
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
