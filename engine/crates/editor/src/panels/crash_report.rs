//! 运行期错误的**现场面板**：列表 + 详情 + 一键复现。
//!
//! ## 分工
//!
//! - **抓现场**：脚本运行时的 `xpcall` 守卫（`rustlua-scripting::script::crash`）；
//! - **落盘与上报**：`PreviewSession`（每条新快照写 `.cache/crash/`，失败也保留内存副本）；
//! - **展示与复现**：本模块。面板只读快照，不碰运行时。
//!
//! ## 弹窗口径
//!
//! **全抓、只对致命弹窗**：每个未捕获错误都留一条现场（并落盘），但只有致命
//! （回调因连续报错被停用、或错误直接中止了这一步）才弹模态并冻结 Play。日常的
//! 偶发容错错误不该打断作者的操作——它们安静地待在列表里。
//!
//! ## 一键复现
//!
//! v1 的语义是「**重跑到出错行并停住**」：重启 Play 时在快照的出错行装一个断点，
//! 命中断点后把 Play 暂停（复用既有 Pause/Step 状态机），并打开本面板显示那一刻的
//! 栈与局部变量。不录放逐帧输入——复现依赖脚本自身的确定性，这一点在面板上写明。

use std::path::PathBuf;

use eframe::egui;
use rustlua_scripting::script::PauseSnapshot;

use crate::EditorApp;
use crate::preview_session::CrashReport;

/// 一键复现目标：重跑 Play 并在这一行**之前**停住。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReplayTarget {
    pub(crate) path: PathBuf,
    pub(crate) line: u32,
}

impl ReplayTarget {
    /// 从一条现场里取复现目标；没有位置（例如纯 C 侧错误）时返回 `None`。
    pub(crate) fn from_report(report: &CrashReport) -> Option<Self> {
        let location = report.snapshot.location.as_ref()?;
        Some(Self {
            path: location.path.clone(),
            line: location.line,
        })
    }
}

/// 错误现场面板的进程内状态（不持久化：现场本身已经在磁盘上）。
#[derive(Default)]
pub(crate) struct CrashPanelState {
    pub(crate) show: bool,
    pub(crate) records: Vec<CrashReport>,
    pub(crate) selected: usize,
    /// 待用户确认的致命错误弹窗（快照序号）。
    pub(crate) fatal_modal: Option<u64>,
    /// 下一次 Play 要挂的复现断点。
    replay: Option<ReplayTarget>,
    /// 一键复现命中断点后的暂停现场（断点那一刻的栈与局部变量）。
    pub(crate) pause: Option<PauseSnapshot>,
}

impl CrashPanelState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取走待挂的复现断点（`play()` 建好会话后调用，只消费一次）。
    pub(crate) fn take_replay_target(&mut self) -> Option<ReplayTarget> {
        self.replay.take()
    }
}

/// 面板里不能立刻执行的动作（避免在遍历记录时再借 `self`）。
enum CrashAction {
    OpenSource(PathBuf, u32),
    Replay(ReplayTarget),
}

impl EditorApp {
    /// 每帧一次：把预览会话里的新错误现场收进来，必要时弹窗并冻结 Play。
    ///
    /// 调用点在 `keep_preview_running_without_game_view` 之后——本帧所有可能的步进
    /// （Game 视图内 / 没画 Game 视图的补步进）都已经发生。
    pub(crate) fn poll_crash_events(&mut self, context: &egui::Context) {
        let reports = self
            .preview
            .as_mut()
            .map(crate::preview_session::PreviewSession::take_crash_reports)
            .unwrap_or_default();
        let replay_hit = self
            .preview
            .as_mut()
            .and_then(crate::preview_session::PreviewSession::take_replay_hit);

        if let Some(pause) = replay_hit {
            self.crash_panel.pause = Some(pause);
            self.crash_panel.show = true;
            self.freeze_play_for_crash("已运行到出错行，Play 停在断点上");
        }
        if reports.is_empty() {
            return;
        }
        let mut fatal_seen = None;
        for report in reports {
            if report.snapshot.fatal && fatal_seen.is_none() {
                fatal_seen = Some(report.snapshot.sequence);
            }
            self.crash_panel.records.push(report);
        }
        self.crash_panel.selected = self.crash_panel.records.len().saturating_sub(1);
        if let Some(sequence) = fatal_seen {
            self.crash_panel.fatal_modal = Some(sequence);
            self.crash_panel.show = true;
            self.freeze_play_for_crash("运行期致命错误：Play 已冻结，现场已保存");
        }
        // 没有弹窗时也要让列表刷新出来（面板可能正开着）。
        context.request_repaint();
    }

    /// 把 Play 从运行中冻到暂停；不在运行中只更新状态栏。
    fn freeze_play_for_crash(&mut self, note: &str) {
        if self.play.state().is_running()
            && let Err(error) = self.play.pause()
        {
            self.status = error.to_string();
            return;
        }
        self.status = note.to_owned();
    }

    /// 错误现场窗口（列表 + 详情）。
    pub(crate) fn draw_crash_panel(&mut self, context: &egui::Context) {
        if !self.crash_panel.show {
            return;
        }
        let mut open = self.crash_panel.show;
        let mut action = None;
        egui::Window::new("崩溃现场（运行期错误快照）")
            .id(egui::Id::new("crash_report_panel"))
            .open(&mut open)
            .default_size([760.0, 560.0])
            .resizable(true)
            .show(context, |ui| {
                action = self.draw_crash_records(ui);
            });
        self.crash_panel.show = open;
        self.apply_crash_action(action);
        let modal_action = self.draw_crash_fatal_modal(context);
        self.apply_crash_action(modal_action);
    }

    /// 致命错误的模态：标题行 + 三个出口（复现 / 查看现场 / 关闭）。
    ///
    /// 用 `egui::Modal` 而不是普通窗口：致命错误要求作者先做一个决定（复现、查看或
    /// 明确关掉），否则它会被后面的面板挡住、悄悄错过。
    fn draw_crash_fatal_modal(&mut self, context: &egui::Context) -> Option<CrashAction> {
        let sequence = self.crash_panel.fatal_modal?;
        let record = self
            .crash_panel
            .records
            .iter()
            .find(|record| record.snapshot.sequence == sequence)?
            .clone();
        let snapshot = &record.snapshot;
        let callback = snapshot.callback.as_deref().unwrap_or("<未知回调>");
        let location = snapshot
            .location
            .as_ref()
            .map(|location| format!("{}:{}", location.path.display(), location.line))
            .unwrap_or_else(|| "<无源码位置>".to_owned());
        let mut action = None;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("crash_fatal_modal")).show(context, |ui| {
            ui.set_max_width(620.0);
            ui.heading("脚本运行期致命错误");
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::from_rgb(235, 120, 110), snapshot.headline());
            ui.monospace(format!("{callback} @ {location}"));
            ui.add_space(6.0);
            ui.weak(
                "现场（栈 + 局部变量 + 实体摘要）已保存到工程 .cache/crash/。\
                 一键复现会重启 Play 并在出错行之前停住——它依赖脚本自己的确定性，\
                 不录放逐帧输入。",
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_replay = ReplayTarget::from_report(&record).is_some();
                if ui
                    .add_enabled(can_replay, egui::Button::new("一键复现"))
                    .on_disabled_hover_text("这条现场没有源码位置，无法挂断点")
                    .clicked()
                    && let Some(target) = ReplayTarget::from_report(&record)
                {
                    action = Some(CrashAction::Replay(target));
                }
                if ui.button("查看现场").clicked() {
                    self.crash_panel.show = true;
                    close = true;
                }
                if ui.button("关闭").clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.crash_panel.fatal_modal = None;
        }
        action
    }

    /// 记录列表 + 选中条目的详情。返回需要在本函数外执行的动作。
    fn draw_crash_records(&mut self, ui: &mut egui::Ui) -> Option<CrashAction> {
        let mut action = None;
        let mut clear_all = false;
        if self.crash_panel.records.is_empty() {
            ui.weak("还没有捕获到运行期错误。");
            ui.weak("脚本里未被 pcall 接住的错误会自动出现在这里，并落盘到工程 .cache/crash/。");
            return None;
        }
        let last = self.crash_panel.records.len() - 1;
        let mut selected = self.crash_panel.selected.min(last);
        ui.horizontal(|ui| {
            ui.weak(format!("共 {} 条", self.crash_panel.records.len()));
            if let Some(pause) = &self.crash_panel.pause {
                ui.separator();
                ui.strong(format!(
                    "断点停住：{} @ {}:{}",
                    pause.function(),
                    pause.location.path.display(),
                    pause.location.line
                ));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("清空列表").clicked() {
                    clear_all = true;
                }
            });
        });
        ui.separator();
        ui.horizontal_top(|ui| {
            egui::ScrollArea::vertical()
                .id_salt("crash_report_list")
                .max_width(250.0)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    for (index, report) in self.crash_panel.records.iter().enumerate() {
                        let tag = if report.snapshot.fatal {
                            "致命"
                        } else {
                            "容错"
                        };
                        let label = format!(
                            "#{} [{}] {}\n{}",
                            report.snapshot.sequence,
                            tag,
                            report.snapshot.callback.as_deref().unwrap_or("<未知回调>"),
                            report.snapshot.headline()
                        );
                        if ui.selectable_label(index == selected, label).clicked() {
                            selected = index;
                        }
                    }
                });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("crash_report_detail")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    action = self.draw_crash_detail(ui, selected);
                });
        });
        if clear_all {
            self.crash_panel.records.clear();
            self.crash_panel.selected = 0;
        } else {
            // 清空后旧的选中下标要收回有效范围。
            self.crash_panel.selected =
                selected.min(self.crash_panel.records.len().saturating_sub(1));
        }
        action
    }

    /// 一条现场的详情（只读）。
    fn draw_crash_detail(&self, ui: &mut egui::Ui, selected: usize) -> Option<CrashAction> {
        let report = self.crash_panel.records.get(selected)?;
        let snapshot = &report.snapshot;
        let mut action = None;
        ui.label(
            egui::RichText::new(snapshot.headline())
                .strong()
                .color(egui::Color32::from_rgb(235, 120, 110)),
        );
        ui.weak(format!(
            "抓取于 {} · 回调 {} · {}",
            format_capture_ms(snapshot.captured_at_ms),
            snapshot.callback.as_deref().unwrap_or("<未知回调>"),
            if snapshot.fatal {
                "致命"
            } else {
                "已容错"
            }
        ));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if let Some(location) = &snapshot.location {
                ui.monospace(format!("{}:{}", location.path.display(), location.line));
                if ui.button("查看源码").clicked() {
                    action = Some(CrashAction::OpenSource(
                        location.path.clone(),
                        location.line,
                    ));
                }
            } else {
                ui.weak("没有源码位置");
            }
        });
        ui.horizontal(|ui| match &report.file {
            Some(file) => {
                ui.weak("现场文件：");
                ui.monospace(file.display().to_string());
                if ui.button("复制路径").clicked() {
                    ui.ctx().copy_text(file.display().to_string());
                }
            }
            None => {
                ui.colored_label(
                    egui::Color32::from_rgb(230, 170, 60),
                    report.write_error.as_deref().unwrap_or("现场未落盘"),
                );
            }
        });
        ui.horizontal(|ui| {
            let can_replay = ReplayTarget::from_report(report).is_some();
            if ui
                .add_enabled(
                    can_replay,
                    egui::Button::new("一键复现（重跑到出错行并停住）"),
                )
                .on_disabled_hover_text("这条现场没有源码位置，无法挂断点")
                .clicked()
                && let Some(target) = ReplayTarget::from_report(report)
            {
                action = Some(CrashAction::Replay(target));
            }
            if ui.button("复制 JSON").clicked() {
                let json = serde_json::to_string_pretty(&snapshot.to_json())
                    .unwrap_or_else(|_| "{}".to_owned());
                ui.ctx().copy_text(json);
            }
        });
        ui.separator();
        egui::CollapsingHeader::new("错误文本（含 traceback）")
            .default_open(true)
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(&snapshot.message).monospace())
                        .selectable(true),
                );
            });
        egui::CollapsingHeader::new(format!("调用栈（{} 帧）", snapshot.frames.len()))
            .default_open(true)
            .show(ui, |ui| {
                if snapshot.frames.is_empty() {
                    ui.weak("没有可用的栈帧");
                }
                for (index, frame) in snapshot.frames.iter().enumerate() {
                    ui.monospace(format!(
                        "{index:>2} {}  {}:{}",
                        frame.function,
                        frame.location.path.display(),
                        frame.location.line
                    ));
                }
            });
        egui::CollapsingHeader::new(format!("局部变量（{} 个）", snapshot.locals.len()))
            .default_open(true)
            .show(ui, |ui| {
                if snapshot.locals.is_empty() {
                    ui.weak("出错帧没有局部变量");
                }
                for local in &snapshot.locals {
                    ui.monospace(format!(
                        "{} = {}  ({})",
                        local.name, local.value, local.kind
                    ));
                }
            });
        if let Some(progress) = &snapshot.progress {
            egui::CollapsingHeader::new("错误时刻的心跳").show(ui, |ui| {
                ui.monospace(format!(
                    "指令数下界 {} · {}:{}",
                    progress.instructions, progress.source, progress.line
                ));
            });
        }
        egui::CollapsingHeader::new("实体摘要")
            .default_open(!snapshot.entities.scripted.is_empty())
            .show(ui, |ui| {
                ui.monospace(format!(
                    "场景实体 {} · 运行期实体 {} · 脚本实体 {}",
                    snapshot.entities.scene_entities,
                    snapshot.entities.runtime_entities,
                    snapshot.entities.scripted.len()
                ));
                for entity in &snapshot.entities.scripted {
                    let position = entity
                        .position
                        .map(|position| format!(" @ ({:.1}, {:.1})", position[0], position[1]))
                        .unwrap_or_default();
                    ui.monospace(format!(
                        "{} {} ({}){}",
                        entity.id,
                        entity.name.as_deref().unwrap_or("<未命名>"),
                        entity.file,
                        position
                    ));
                }
            });
        if let Some(pause) = &self.crash_panel.pause {
            ui.separator();
            egui::CollapsingHeader::new(format!(
                "复现断点现场：{}（{}）",
                pause.function(),
                if pause.reason == rustlua_scripting::script::PauseReason::Breakpoint {
                    "断点"
                } else {
                    "单步"
                }
            ))
            .default_open(true)
            .show(ui, |ui| {
                for (index, frame) in pause.frames.iter().enumerate() {
                    ui.monospace(format!(
                        "{index:>2} {}  {}:{}",
                        frame.function,
                        frame.location.path.display(),
                        frame.location.line
                    ));
                }
                if !pause.locals.is_empty() {
                    ui.separator();
                    for local in &pause.locals {
                        ui.monospace(format!(
                            "{} = {}  ({})",
                            local.name, local.value, local.kind
                        ));
                    }
                }
            });
        }
        action
    }

    fn apply_crash_action(&mut self, action: Option<CrashAction>) {
        match action {
            Some(CrashAction::OpenSource(path, line)) => {
                self.open_lua_source(&path, Some(line));
            }
            Some(CrashAction::Replay(target)) => self.replay_crash(target),
            None => {}
        }
    }

    /// 一键复现：重启 Play，并在出错行之前停住。
    ///
    /// 会话创建后由 `play()` 取走 `replay` 目标装断点（见 `PreviewSession::arm_crash_replay`）；
    /// 命中后 `poll_crash_events` 冻结 Play 并把暂停现场交给面板。
    pub(crate) fn replay_crash(&mut self, target: ReplayTarget) {
        if !self.project_loaded {
            self.status = "请先打开工程".to_owned();
            return;
        }
        self.status = format!(
            "一键复现：重跑并在 {}:{} 之前停住",
            target.path.display(),
            target.line
        );
        self.crash_panel.fatal_modal = None;
        self.crash_panel.pause = None;
        self.crash_panel.replay = Some(target);
        if self.play.state().is_in_play_mode() {
            self.stop();
        }
        self.play();
    }
}

/// 抓取时刻的可读形态（UTC）。只用于面板展示，不做时区换算。
fn format_capture_ms(ms: u64) -> String {
    let seconds = ms / 1000;
    format!(
        "{:02}:{:02}:{:02} UTC",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::{ReplayTarget, format_capture_ms};
    use crate::preview_session::CrashReport;
    use rustlua_scripting::script::{CrashSnapshot, EntitySummary, SourceLocation, StackFrame};
    use std::path::PathBuf;

    fn snapshot(location: Option<SourceLocation>) -> CrashSnapshot {
        CrashSnapshot {
            sequence: 3,
            captured_at_ms: 1_700_000_000_000,
            callback: Some("update()".to_owned()),
            fatal: true,
            message: "boom\nstack traceback:\n\t...".to_owned(),
            location,
            frames: Vec::new(),
            locals: Vec::new(),
            progress: None,
            entities: EntitySummary::default(),
        }
    }

    #[test]
    fn replay_target_comes_from_the_error_location() {
        let report = CrashReport {
            snapshot: snapshot(Some(SourceLocation {
                path: PathBuf::from("scripts/main.lua"),
                line: 42,
            })),
            file: None,
            write_error: None,
        };
        assert_eq!(
            ReplayTarget::from_report(&report),
            Some(ReplayTarget {
                path: PathBuf::from("scripts/main.lua"),
                line: 42
            })
        );
        // 没有位置就没有可挂断点的目标（按钮会被禁用，而不是挂到第 0 行）。
        let report = CrashReport {
            snapshot: snapshot(None),
            file: None,
            write_error: None,
        };
        assert_eq!(ReplayTarget::from_report(&report), None);
    }

    #[test]
    fn stack_frames_are_only_decorative_here() {
        // 面板自己不解析栈：它只展示引擎给的结构化帧（共享解析在 scripting 侧）。
        let mut snap = snapshot(Some(SourceLocation {
            path: PathBuf::from("scripts/main.lua"),
            line: 7,
        }));
        snap.frames = vec![StackFrame {
            function: "deep".to_owned(),
            location: SourceLocation {
                path: PathBuf::from("scripts/main.lua"),
                line: 7,
            },
        }];
        assert_eq!(snap.frames[0].function, "deep");
    }

    #[test]
    fn capture_time_is_formatted_in_utc() {
        assert_eq!(format_capture_ms(0), "00:00:00 UTC");
        assert_eq!(format_capture_ms(3_723_000), "01:02:03 UTC");
        // 跨过一天：小时按 24 取模（日期不在这里展示）。
        assert_eq!(format_capture_ms(86_400_000 + 5_000), "00:00:05 UTC");
    }
}
