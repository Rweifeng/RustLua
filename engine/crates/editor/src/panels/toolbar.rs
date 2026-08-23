//! 中央 Scene/Game 工作区工具栏：Play/Stop、视图切换、吸附、对齐、缩放。
//! Undo/Redo 通过快捷键 Ctrl+Z / Ctrl+Y 触发（见 main.rs）。
//! 工具按钮（Select/Move/Rotate/Scale）在 Scene 视图右上角的小面板中（见 scene_view.rs）。

use crate::EditorApp;
use crate::{AlignMode, SceneViewMode, WorkspaceTab};
use eframe::egui;

impl EditorApp {
    pub(crate) fn draw_workspace_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Play/Stop：场景与游戏运行控制。
            if self.is_playing() {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "Play");
                if ui.button("Stop").clicked() {
                    self.stop();
                }
            } else if ui.button("Play").clicked() && self.project_loaded {
                self.play();
            }
            // 预览分辨率（下次 Play 生效；可自定义增删）。
            let resolutions = self.preview_resolutions.clone();
            egui::ComboBox::from_id_salt("preview_resolution")
                .selected_text(format!(
                    "{}×{}",
                    self.preview_resolution.0, self.preview_resolution.1
                ))
                .show_ui(ui, |ui| {
                    for (w, h) in &resolutions {
                        if ui
                            .selectable_value(
                                &mut self.preview_resolution,
                                (*w, *h),
                                format!("{w}×{h}"),
                            )
                            .changed()
                        {
                            self.adding_preview_resolution = false;
                        }
                    }
                    ui.separator();
                    if ui.button("＋ 自定义…").clicked() {
                        self.custom_preview_resolution = self.preview_resolution;
                        self.adding_preview_resolution = true;
                        ui.close();
                    }
                });
            if ui.button("删除").clicked() {
                let index = self
                    .preview_resolutions
                    .iter()
                    .position(|r| *r == self.preview_resolution);
                if let Some(index) = index {
                    self.preview_resolutions.remove(index);
                }
                if self.preview_resolutions.is_empty() {
                    self.preview_resolutions.push((640, 360));
                }
                self.preview_resolution = self.preview_resolutions[0];
            }
            ui.separator();
            ui.selectable_value(&mut self.workspace_tab, WorkspaceTab::Scene, "Scene");
            ui.selectable_value(&mut self.workspace_tab, WorkspaceTab::Game, "Game");
            ui.separator();
            ui.selectable_value(&mut self.scene_view_mode, SceneViewMode::View2D, "2D");
            ui.selectable_value(&mut self.scene_view_mode, SceneViewMode::View3D, "3D");
            ui.separator();
            ui.checkbox(&mut self.snap_enabled, "吸附");
            ui.add(
                egui::DragValue::new(&mut self.snap_step)
                    .range(0.05..=10.0)
                    .speed(0.05)
                    .prefix(" "),
            );
            if ui.button("对齐网格").clicked() {
                self.align_selected(AlignMode::Grid);
            }
            if ui.button("对齐H").clicked() {
                self.align_selected(AlignMode::H);
            }
            if ui.button("对齐V").clicked() {
                self.align_selected(AlignMode::V);
            }
            ui.separator();
            ui.add(egui::Slider::new(&mut self.scene_zoom, 0.5..=2.5).text("Zoom"));
        });
        // 自定义分辨率输入行。
        if self.adding_preview_resolution {
            ui.horizontal(|ui| {
                ui.label("自定义分辨率");
                ui.add(
                    egui::DragValue::new(&mut self.custom_preview_resolution.0)
                        .range(100..=7680)
                        .prefix("宽 "),
                );
                ui.add(
                    egui::DragValue::new(&mut self.custom_preview_resolution.1)
                        .range(100..=4320)
                        .prefix("高 "),
                );
                if ui.button("添加").clicked() {
                    if !self
                        .preview_resolutions
                        .contains(&self.custom_preview_resolution)
                    {
                        self.preview_resolutions.push(self.custom_preview_resolution);
                    }
                    self.preview_resolution = self.custom_preview_resolution;
                    self.adding_preview_resolution = false;
                }
                if ui.button("取消").clicked() {
                    self.adding_preview_resolution = false;
                }
            });
        }
    }
}
