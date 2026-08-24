//! 行4 控制台：实时日志（LogSink）+ Lua 运行输出 + 诊断。

use crate::EditorApp;
use eframe::egui;
use engine::core::script::{LuaRuntime, ScriptDiagnostic};

impl EditorApp {
    /// 行4 控制台面板（全宽底部，先于左右面板显示以获得整行宽度）。
    pub(crate) fn draw_lua_console_panel(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .default_height(150.0)
            .min_height(60.0)
            .max_height(520.0)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    let project_name = self
                        .project
                        .root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("项目");
                    ui.heading(format!("控制台 — {project_name}"));
                    ui.text_edit_singleline(&mut self.console_filter);
                    if ui.button("运行选中脚本").clicked() {
                        self.run_selected_script();
                    }
                    if ui.button("清空").clicked() {
                        self.clear_console();
                        self.log_sink.clear();
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
                let log_lines = self.log_sink.snapshot();
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .show(ui, |ui| {
                        for line in &log_lines {
                            ui.monospace(line);
                        }
                        for line in &self.console_output {
                            ui.monospace(line);
                        }
                        for diagnostic in self.console_diagnostics() {
                            ui.colored_label(egui::Color32::RED, format_diagnostic(&diagnostic));
                        }
                    });
            });
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
}
