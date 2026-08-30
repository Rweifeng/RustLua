//! 行4 控制台：实时日志（LogSink）+ Lua 运行输出 + 诊断。

use crate::{ConsoleLogLevelFilter, ConsoleTab, EditorApp};
use eframe::egui;
use engine::core::script::{LuaRuntime, ScriptDiagnostic};

/// 左侧分类导航图标（egui 内嵌 emoji 字体渲染）。
const EDITOR_LOG_ICON: &str = "⚙";
const LUA_LOG_ICON: &str = "🕹";

impl EditorApp {
    /// 行4 控制台面板（全宽底部，先于左右面板显示以获得整行宽度）。
    ///
    /// 高度固定：滚动区填满剩余空间，面板不再随日志行数伸缩（M-固定高度）。
    pub(crate) fn draw_lua_console_panel(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .default_height(150.0)
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
                            ui.text_edit_singleline(&mut self.console_filter);
                            ui.separator();
                            ui.selectable_value(
                                &mut self.console_log_level_filter,
                                ConsoleLogLevelFilter::All,
                                "全部",
                            );
                            ui.selectable_value(
                                &mut self.console_log_level_filter,
                                ConsoleLogLevelFilter::Info,
                                "Info",
                            );
                            ui.selectable_value(
                                &mut self.console_log_level_filter,
                                ConsoleLogLevelFilter::Warning,
                                "Warning",
                            );
                            ui.selectable_value(
                                &mut self.console_log_level_filter,
                                ConsoleLogLevelFilter::Error,
                                "Error",
                            );
                            if ui.button("运行选中脚本").clicked() {
                                self.run_selected_script();
                            }
                            if ui.button("清空").clicked() {
                                self.clear_console();
                                self.log_sink.clear();
                                self.console_notices.clear();
                            }
                        });
                        ui.horizontal(|ui| {
                            let response = ui.text_edit_singleline(&mut self.console_input);
                            if response.lost_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                self.run_console_command();
                            }
                            if ui.button("执行").clicked() {
                                self.run_console_command();
                            }
                        });
                        // 滚动区填满剩余空间，面板高度保持固定，仅滚动内部内容。
                        egui::ScrollArea::vertical()
                            .auto_shrink([false; 2])
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

    /// 按左侧分类绘制日志主体：
    /// 编辑器日志 = 非 Lua（script target）的 tracing 行 + 编辑器提示；
    /// Lua 日志 = Lua `log.*`（script target）+ 控制台输出 + 脚本诊断。
    fn draw_console_log_body(&mut self, ui: &mut egui::Ui) {
        let filter = self.console_filter.trim().to_lowercase();
        let log_lines = self.log_sink.snapshot();
        match self.console_tab {
            ConsoleTab::Editor => {
                for line in &log_lines {
                    if is_lua_log_line(line) {
                        continue;
                    }
                    let level = log_level(line);
                    if self.console_log_level_filter.matches(level)
                        && text_matches_filter(line, &filter)
                    {
                        draw_log_line(ui, line, level);
                    }
                }
                for line in &self.console_notices {
                    let level = log_level(line);
                    if self.console_log_level_filter.matches(level)
                        && text_matches_filter(line, &filter)
                    {
                        draw_log_line(ui, line, level);
                    }
                }
            }
            ConsoleTab::Lua => {
                for line in &log_lines {
                    if !is_lua_log_line(line) {
                        continue;
                    }
                    let level = log_level(line);
                    if self.console_log_level_filter.matches(level)
                        && text_matches_filter(line, &filter)
                    {
                        draw_log_line(ui, lua_log_message(line), level);
                    }
                }
                for line in &self.console_output {
                    let level = output_level(line);
                    if self.console_log_level_filter.matches(level)
                        && text_matches_filter(line, &filter)
                    {
                        draw_log_line(ui, line, level);
                    }
                }
                for diagnostic in self.console_diagnostics() {
                    let line = format_diagnostic(&diagnostic);
                    if self
                        .console_log_level_filter
                        .matches(ConsoleLogLevel::Error)
                        && text_matches_filter(&line, &filter)
                    {
                        ui.colored_label(egui::Color32::RED, line);
                    }
                }
            }
        }
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
            Ok(value) => self.console_output.push(format!("> {source}\n{value}")),
            Err(error) => {
                self.status = error.clone();
                self.console_output
                    .push(format!("> {source}\n错误：{error}"));
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
                self.console_output.push(format!("运行 {script}\n{value}"));
            }
            Err(error) => {
                self.status = error.clone();
                self.console_output
                    .push(format!("运行 {script}\n错误：{error}"));
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

fn draw_log_line(ui: &mut egui::Ui, line: &str, level: ConsoleLogLevel) {
    let color = match level {
        ConsoleLogLevel::Info => ui.visuals().text_color(),
        ConsoleLogLevel::Warning => egui::Color32::YELLOW,
        ConsoleLogLevel::Error => egui::Color32::RED,
    };
    ui.colored_label(color, egui::RichText::new(line).monospace());
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
