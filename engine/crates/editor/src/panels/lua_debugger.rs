//! C8：Lua 断点调试面板（v1）。
//!
//! 内核的调试回调是**同步**的（`rustlua-scripting` 的 `DebugHandler` 在 Lua 行事件里直接调用），
//! 且不能借编辑器状态、不能阻塞等 UI——阻塞会冻死整个窗口（Lua 与 egui 同一线程）。
//! 所以本面板的语义是：
//!
//! * **断点**：编辑器持有集合（跨 Play 保留），进入 Play 时同步给 VM；
//! * **暂停**：命中时下帧边界冻结 Play（复用 `play.pause()`，与崩溃现场同一套）；
//! * **继续**：恢复 Play；
//! * **单步**：前进**一帧**（Unity 的 Step 同样是一帧）；真正的"第 N 行"单步要等内核支持
//!   原地挂起 VM，那是下一个工作包。
//!
//! v1 不做：条件断点 / hit count、暂停期求值（watch）、真·按行步进、断点持久化到磁盘。

use crate::EditorApp;
use eframe::egui;
use rustlua_scripting::script::{DebugAction, PauseSnapshot};
use std::collections::BTreeSet;

/// 一条断点：工程内相对路径（正斜杠）+ 1 基行号。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct LuaBreakpoint {
    pub(crate) path: String,
    pub(crate) line: u32,
}

impl LuaBreakpoint {
    pub(crate) fn label(&self) -> String {
        format!("{}:{}", self.path, self.line)
    }
}

/// 调试面板状态：编辑器是断点的**事实源**（VM 只是被同步的一方）。
#[derive(Debug, Default)]
pub(crate) struct LuaDebuggerState {
    pub(crate) open: bool,
    pub(crate) breakpoints: BTreeSet<LuaBreakpoint>,
    /// `路径:行` 的输入缓冲。
    pub(crate) input: String,
    /// 最近一次命中的现场（面板展示用；取走后仍在面板里留着，直到下一次命中/清空）。
    pub(crate) last_pause: Option<PauseSnapshot>,
    /// 本局是否已经把回调/断点装进 VM。
    ///
    /// 为什么要有这个开关：内核只要"有断点**或**有回调"就会逐行触发钩子，即使断点集合为空
    /// 也会每行调一次 Rust 回调。Lua 密集的游戏（每帧几万个行事件）这是实打实的开销，
    /// 所以**只有当用户真的在用调试器**（有断点，或面板打开）时才装。
    pub(crate) armed: bool,
    /// 面板顶部的一行结果（添加失败/同步失败都说清楚）。
    pub(crate) status: String,
}

impl LuaDebuggerState {
    /// 解析断点输入：`scripts/main.lua:12`，行号必须是正整数。
    ///
    /// 也接受 `路径 行`（复制粘贴时更常见）。路径统一成正斜杠，与工程内引用口径一致。
    pub(crate) fn parse_breakpoint(input: &str) -> Result<LuaBreakpoint, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err("先填写 路径:行".to_owned());
        }
        let (path, line_text) = if let Some((path, line)) = input.rsplit_once(':') {
            (path, line)
        } else if let Some((path, line)) = input.rsplit_once(char::is_whitespace) {
            (path, line)
        } else {
            return Err("格式是 路径:行（例如 scripts/main.lua:12）".to_owned());
        };
        let path = path.trim().replace('\\', "/");
        if path.is_empty() {
            return Err("路径不能为空".to_owned());
        }
        let line: u32 = line_text
            .trim()
            .parse()
            .map_err(|_| format!("行号不是正整数：{}", line_text.trim()))?;
        if line == 0 {
            return Err("行号从 1 开始".to_owned());
        }
        Ok(LuaBreakpoint { path, line })
    }
}

impl EditorApp {
    /// 进入 Play 时按需把编辑器侧断点装进 VM。
    ///
    /// "按需"很关键：内核只要装了回调/断点就**逐行**回调，没有断点时也照付这份开销。
    pub(crate) fn arm_lua_debugger_for_play(&mut self) {
        self.lua_debug.armed = false;
        if self.lua_debug.breakpoints.is_empty() && !self.lua_debug.open {
            return;
        }
        self.ensure_lua_debugger_armed();
    }

    /// 确保本局已装好回调与断点（Play 中打开面板/加断点时调用）。
    pub(crate) fn ensure_lua_debugger_armed(&mut self) {
        let Some(preview) = self.preview.as_mut() else {
            return;
        };
        if self.lua_debug.armed {
            self.sync_lua_breakpoints_to_preview();
            return;
        }
        if let Err(error) = preview.arm_lua_debugger() {
            self.lua_debug.status = format!("调试回调安装失败：{error}");
            return;
        }
        self.lua_debug.armed = true;
        self.sync_lua_breakpoints_to_preview();
    }

    /// 把编辑器侧断点集合同步给 VM（Play 中改断点走这里）。
    ///
    /// 未装回调时会先装（[`Self::ensure_lua_debugger_armed`]），否则 Play 中新加的断点不会生效。
    pub(crate) fn sync_lua_breakpoints_to_preview(&mut self) {
        if self.preview.is_none() {
            return;
        }
        if !self.lua_debug.armed {
            self.ensure_lua_debugger_armed();
            return;
        }
        let breakpoints: Vec<rustlua_scripting::script::Breakpoint> = self
            .lua_debug
            .breakpoints
            .iter()
            .map(|breakpoint| {
                rustlua_scripting::script::Breakpoint::new(
                    std::path::PathBuf::from(&breakpoint.path),
                    breakpoint.line,
                )
            })
            .collect();
        let Some(preview) = self.preview.as_mut() else {
            return;
        };
        match preview.set_lua_breakpoints(&breakpoints) {
            Ok(()) => {
                self.lua_debug.status = if breakpoints.is_empty() {
                    "断点已清空（Play 中即时生效）".to_owned()
                } else {
                    format!("{} 个断点已同步到运行时", breakpoints.len())
                };
            }
            Err(error) => self.lua_debug.status = format!("断点同步失败：{error}"),
        }
    }

    /// 每帧轮询：有命中的暂停现场就**冻结 Play**（帧边界），并把它留在面板里。
    pub(crate) fn poll_lua_debugger(&mut self) {
        let Some(preview) = self.preview.as_mut() else {
            return;
        };
        let Some(snapshot) = preview.take_lua_pause() else {
            return;
        };
        self.lua_debug.last_pause = Some(snapshot);
        if !self.play.is_paused() {
            let _ = self.play.pause();
        }
        self.status = "Lua 断点命中：已暂停（可继续或单帧步进）".to_owned();
    }

    /// 继续：恢复 Play，并把下一次命中后的动作设为 Continue。
    pub(crate) fn debug_continue(&mut self) {
        if let Some(preview) = self.preview.as_mut() {
            preview.request_debug_action(DebugAction::Continue);
        }
        match self.play.resume() {
            Ok(()) => self.status = "已继续运行".to_owned(),
            Err(error) => self.status = error.to_string(),
        }
    }

    /// 单帧步进：排队 StepOver 后让 Play 前进一帧（下一帧边界再次冻结）。
    pub(crate) fn debug_step_frame(&mut self) {
        if let Some(preview) = self.preview.as_mut() {
            preview.request_debug_action(DebugAction::StepOver);
        }
        self.step_play_frames(1);
    }

    /// 面板：断点列表 + 继续/单帧步进 + 最近一次命中的栈与局部变量。
    pub(crate) fn draw_lua_debugger(&mut self, context: &egui::Context) {
        // Play 中第一次打开面板：这时才把回调装进 VM（见 `armed` 的注释）。
        if self.lua_debug.open && self.preview.is_some() && !self.lua_debug.armed {
            self.ensure_lua_debugger_armed();
        }
        let mut open = self.lua_debug.open;
        egui::Window::new("Lua 调试")
            .id(egui::Id::new("lua_debugger"))
            .open(&mut open)
            .default_pos(egui::pos2(320.0, 96.0))
            .default_width(460.0)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.lua_debug.input)
                            .hint_text("scripts/main.lua:12")
                            .desired_width(240.0),
                    );
                    if ui.button("添加断点").clicked() {
                        self.add_lua_breakpoint_from_input();
                    }
                    if !self.lua_debug.breakpoints.is_empty() && ui.button("全部清空").clicked()
                    {
                        self.lua_debug.breakpoints.clear();
                        self.sync_lua_breakpoints_to_preview();
                    }
                });
                if !self.lua_debug.status.is_empty() {
                    ui.small(self.lua_debug.status.clone());
                }
                ui.separator();
                if self.lua_debug.breakpoints.is_empty() {
                    ui.weak("还没有断点。添加后进入 Play 会在这些行停住。");
                }
                let mut remove: Option<LuaBreakpoint> = None;
                // 抄一份再迭代：循环里要 `&mut self`（打开源码 / 删除断点）。
                let breakpoints: Vec<LuaBreakpoint> =
                    self.lua_debug.breakpoints.iter().cloned().collect();
                for breakpoint in &breakpoints {
                    ui.horizontal(|ui| {
                        ui.monospace(breakpoint.label());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("删除这个断点").clicked() {
                                remove = Some(breakpoint.clone());
                            }
                            if ui.small_button("打开").clicked() {
                                self.open_lua_source(
                                    std::path::Path::new(&breakpoint.path),
                                    Some(breakpoint.line),
                                );
                            }
                        });
                    });
                }
                if let Some(breakpoint) = remove {
                    self.lua_debug.breakpoints.remove(&breakpoint);
                    self.sync_lua_breakpoints_to_preview();
                }
                ui.separator();
                let playing = self.play.is_playing();
                let paused = self.play.is_paused();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(paused, egui::Button::new("继续"))
                        .on_disabled_hover_text("只有暂停时才能继续")
                        .clicked()
                    {
                        self.debug_continue();
                    }
                    if ui
                        .add_enabled(playing, egui::Button::new("单帧步进"))
                        .on_disabled_hover_text("进入 Play 后才能步进")
                        .clicked()
                    {
                        self.debug_step_frame();
                    }
                    if ui.add_enabled(playing, egui::Button::new("停止")).clicked() {
                        self.dispatch(editor::EditorCommand::StopPlay);
                    }
                });
                ui.separator();
                match &self.lua_debug.last_pause {
                    None => {
                        ui.weak("尚未命中。断点在下一帧的脚本行上生效。");
                    }
                    Some(pause) => {
                        ui.horizontal(|ui| {
                            ui.label(match pause.reason {
                                rustlua_scripting::script::PauseReason::Breakpoint => "断点",
                                rustlua_scripting::script::PauseReason::Step => "单步",
                            });
                            ui.monospace(format!(
                                "{}:{}",
                                pause.location.path.display(),
                                pause.location.line
                            ));
                        });
                        ui.collapsing(format!("调用栈（{} 帧）", pause.frames.len()), |ui| {
                            for (index, frame) in pause.frames.iter().enumerate() {
                                ui.monospace(format!(
                                    "{index:>2} {}  {}:{}",
                                    frame.function,
                                    frame.location.path.display(),
                                    frame.location.line
                                ));
                            }
                        });
                        ui.collapsing(
                            format!("局部变量（{} 个）", pause.locals.len()),
                            |ui| {
                                if pause.locals.is_empty() {
                                    ui.weak("这一帧没有局部变量");
                                }
                                for local in &pause.locals {
                                    ui.monospace(format!(
                                        "{} = {}  ({})",
                                        local.name, local.value, local.kind
                                    ));
                                }
                            },
                        );
                    }
                }
            });
        self.lua_debug.open = open;
    }

    /// 从输入框添加断点（解析失败只改面板里的一行，不弹窗）。
    pub(crate) fn add_lua_breakpoint_from_input(&mut self) {
        match LuaDebuggerState::parse_breakpoint(&self.lua_debug.input) {
            Ok(breakpoint) => {
                self.lua_debug.breakpoints.insert(breakpoint.clone());
                self.lua_debug.status = format!("已添加 {}", breakpoint.label());
                self.lua_debug.input.clear();
                self.sync_lua_breakpoints_to_preview();
            }
            Err(error) => self.lua_debug.status = error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoint_input_accepts_colon_and_space_and_normalizes_slashes() {
        assert_eq!(
            LuaDebuggerState::parse_breakpoint("scripts\\main.lua:12"),
            Ok(LuaBreakpoint {
                path: "scripts/main.lua".to_owned(),
                line: 12
            })
        );
        assert_eq!(
            LuaDebuggerState::parse_breakpoint("  assets/ai.lua 7 "),
            Ok(LuaBreakpoint {
                path: "assets/ai.lua".to_owned(),
                line: 7
            })
        );
        // Windows 盘符式路径：按**最后一个**冒号切，盘符不算行号分隔。
        assert_eq!(
            LuaDebuggerState::parse_breakpoint("D:/p/scripts/a.lua:3"),
            Ok(LuaBreakpoint {
                path: "D:/p/scripts/a.lua".to_owned(),
                line: 3
            })
        );
    }

    #[test]
    fn breakpoint_input_rejects_what_cannot_become_a_breakpoint() {
        assert!(LuaDebuggerState::parse_breakpoint("").is_err());
        assert!(LuaDebuggerState::parse_breakpoint("scripts/main.lua").is_err());
        assert!(LuaDebuggerState::parse_breakpoint("scripts/main.lua:0").is_err());
        assert!(LuaDebuggerState::parse_breakpoint("scripts/main.lua:abc").is_err());
        assert!(LuaDebuggerState::parse_breakpoint(":12").is_err());
        // 错误说明要能指导用户（不是"invalid input"）。
        let error = LuaDebuggerState::parse_breakpoint("x:0").unwrap_err();
        assert!(error.contains("行号"), "{error}");
    }

    /// 没有预览会话时（未进入 Play / headless）调试器的所有入口都必须是**空操作**：
    /// 不 panic、不改断点集合、不把 `armed` 置真。
    #[test]
    fn debugger_entries_are_no_ops_without_a_play_session() {
        let mut app = crate::EditorApp::new(
            editor::ProjectInfo {
                root: std::env::temp_dir()
                    .join(format!("rustlua-lua-debugger-{}", std::process::id())),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            rustlua_infra::logging::LogSink::new(16),
        );
        app.lua_debug.input = "scripts/main.lua:3".to_owned();
        app.add_lua_breakpoint_from_input();
        assert_eq!(app.lua_debug.breakpoints.len(), 1);
        assert_eq!(app.lua_debug.breakpoints.iter().next().unwrap().line, 3);

        app.sync_lua_breakpoints_to_preview();
        app.poll_lua_debugger();
        app.ensure_lua_debugger_armed();
        assert_eq!(app.lua_debug.breakpoints.len(), 1, "同步失败不得丢断点");
        assert!(!app.lua_debug.armed, "没有会话时不能声称已装好回调");
        assert!(app.lua_debug.last_pause.is_none());

        // 同一个断点再写一次（反斜杠形态）是幂等的。
        app.lua_debug.input = "scripts\\main.lua:3".to_owned();
        app.add_lua_breakpoint_from_input();
        assert_eq!(app.lua_debug.breakpoints.len(), 1);
    }
}
