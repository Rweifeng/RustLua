//! 帮助 ▸ 快捷键…：编辑器快捷键的查看与改键。
//!
//! 键位表在 `crate::shortcuts`（分发匹配、菜单提示、这里三处共用同一张表）；
//! 改键属于**用户级**偏好（`~/.rustlua/editor.json` 的 `shortcuts` 段），与工程布局
//! 分开。本面板不碰文档与历史，只调 `EditorApp` 的改键入口。

use eframe::egui;

use crate::shortcuts::{self, EditorShortcut};

impl crate::EditorApp {
    /// 快捷键窗口（帮助 ▸ 快捷键…）。关闭窗口即取消进行中的捕获。
    pub(crate) fn draw_shortcut_settings(&mut self, context: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        let mut open = self.show_shortcuts;
        let mut request_capture = None;
        let mut request_reset = None;
        let mut request_reset_all = false;
        let capturing = self.shortcut_capture;
        egui::Window::new("快捷键")
            .open(&mut open)
            .default_size([560.0, 440.0])
            .resizable(true)
            .show(context, |ui| {
                ui.label("改键属于这台机器的这个用户（存 ~/.rustlua/editor.json），所有工程共用。");
                ui.label(
                    "「改键」后按下组合键即可；Esc 取消。裸字母/数字必须搭配 Ctrl、Alt 或 Shift。",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.shortcuts.is_empty(),
                            egui::Button::new("全部恢复默认"),
                        )
                        .on_hover_text("清空本机改键，全部动作回到默认键位")
                        .clicked()
                    {
                        request_reset_all = true;
                    }
                    if let Some(action) = capturing {
                        ui.colored_label(
                            egui::Color32::from_rgb(230, 170, 60),
                            format!("正在为「{}」捕获新键位…（Esc 取消）", action.label()),
                        );
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        egui::Grid::new("shortcut_grid")
                            .num_columns(4)
                            .striped(true)
                            .show(ui, |ui| {
                                for action in EditorShortcut::ALL {
                                    let spec = shortcuts::binding(&self.shortcuts, action);
                                    let overridden = self.shortcuts.get(action.id()).is_some();
                                    ui.label(action.label());
                                    let mut binding_text =
                                        egui::RichText::new(spec.display()).monospace();
                                    if overridden {
                                        binding_text = binding_text
                                            .color(egui::Color32::from_rgb(110, 190, 240));
                                    }
                                    ui.label(binding_text);
                                    let this_capture = capturing == Some(action);
                                    if ui
                                        .add_enabled(
                                            !this_capture,
                                            egui::Button::new(if this_capture {
                                                "请按键…"
                                            } else {
                                                "改键"
                                            }),
                                        )
                                        .clicked()
                                    {
                                        request_capture = Some(action);
                                    }
                                    if ui
                                        .add_enabled(overridden, egui::Button::new("恢复默认"))
                                        .on_hover_text(format!(
                                            "回到默认键位 {}",
                                            action.default_spec()
                                        ))
                                        .clicked()
                                    {
                                        request_reset = Some(action);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            });
        self.show_shortcuts = open;
        if !open {
            self.shortcut_capture = None;
        } else if let Some(action) = request_capture {
            self.shortcut_capture = Some(action);
            self.status = format!("请为「{}」按下新键位（Esc 取消）", action.label());
        }
        if let Some(action) = request_reset {
            self.reset_shortcut(action);
        }
        if request_reset_all {
            self.reset_all_shortcuts();
        }
    }
}
