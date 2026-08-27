//! 通用进度面板：后台任务 + 进度/日志 + 非阻塞窗口。
//! 可复用于导入、发布等任何耗时操作（任务在线程中执行，UI 不阻塞）。

use crate::util::open_in_explorer;
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
};

/// 进度面板事件（任务线程 → 主线程）。
pub(crate) enum ProgressEvent {
    Line(String),
    // 通用组件 API：供后续进度任务发送确定进度（当前任务未用，保留）。
    #[allow(dead_code)]
    Progress(f32),
    Finished {
        success: bool,
        result: Option<Box<dyn std::any::Any + Send>>,
    },
}

/// 进度任务的类型，决定完成时如何应用结果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ProgressKind {
    #[default]
    ImportProject,
    Publish,
    ResourceImport,
}

#[derive(Default)]
pub(crate) struct ProgressPanel {
    pub(crate) kind: ProgressKind,
    title: String,
    rx: Option<Receiver<ProgressEvent>>,
    pub(crate) output: String,
    /// 确定进度 `0..=1`；`None` 表示不定进度（动画条）。
    progress: Option<f32>,
    pub(crate) running: bool,
    pub(crate) done: bool,
    pub(crate) success: bool,
    pub(crate) show: bool,
    /// 任务结果负载（任务线程发送，主线程 poll 完成后按需取用）。
    pub(crate) result: Option<Box<dyn std::any::Any + Send>>,
    /// 完成时展示的目录（如发布包目录），可「打开目录」。
    pub(crate) output_dir: Option<PathBuf>,
    /// 用户关闭进度窗时置 true，让后台任务（如资源导入）尽快停止。
    pub(crate) cancel: Option<Arc<AtomicBool>>,
}

impl ProgressPanel {
    pub(crate) fn start(
        &mut self,
        kind: ProgressKind,
        title: impl Into<String>,
        rx: Receiver<ProgressEvent>,
        output_dir: Option<PathBuf>,
        cancel: Option<Arc<AtomicBool>>,
    ) {
        self.kind = kind;
        self.title = title.into();
        self.rx = Some(rx);
        self.output.clear();
        self.progress = None;
        self.running = true;
        self.done = false;
        self.success = false;
        self.show = true;
        self.result = None;
        self.output_dir = output_dir;
        self.cancel = cancel;
    }

    /// 每帧排空事件；返回本帧是否刚完成。
    pub(crate) fn poll(&mut self) -> bool {
        if !self.running {
            return false;
        }
        let events: Vec<ProgressEvent> = if let Some(rx) = self.rx.as_ref() {
            std::iter::from_fn(|| rx.try_recv().ok()).collect()
        } else {
            Vec::new()
        };
        let mut finished = false;
        for event in events {
            match event {
                ProgressEvent::Line(text) => self.output.push_str(&text),
                ProgressEvent::Progress(value) => self.progress = Some(value),
                ProgressEvent::Finished { success, result } => {
                    finished = true;
                    self.success = success;
                    self.result = result;
                }
            }
        }
        if finished {
            self.running = false;
            self.done = true;
            self.rx = None;
        }
        finished
    }

    pub(crate) fn window(&mut self, context: &egui::Context) {
        if !self.show {
            return;
        }
        let mut open = true;
        egui::Window::new(&self.title)
            .open(&mut open)
            .show(context, |ui| {
                let status = if self.running {
                    "进行中..."
                } else if self.success {
                    "✔ 完成"
                } else {
                    "✘ 失败"
                };
                ui.colored_label(
                    if self.success {
                        egui::Color32::LIGHT_GREEN
                    } else {
                        egui::Color32::LIGHT_RED
                    },
                    status,
                );
                if let Some(progress) = self.progress {
                    ui.add(egui::ProgressBar::new(progress).show_percentage());
                } else if self.running {
                    ui.add(egui::ProgressBar::new(0.0).animate(true));
                }
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.monospace(&self.output);
                    });
                if self.done
                    && let Some(dir) = &self.output_dir
                {
                    ui.label(format!("输出：{}", dir.display()));
                    if ui.button("打开目录").clicked() {
                        open_in_explorer(dir);
                    }
                }
            });
        if !open {
            // 用户关闭进度窗：通知后台任务取消（如资源导入）并隐藏窗口。
            if let Some(cancel) = &self.cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            self.show = false;
        }
    }
}
