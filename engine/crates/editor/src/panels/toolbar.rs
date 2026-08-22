//! 中央 Scene/Game 工作区工具栏：Play/Stop、视图切换、工具、吸附、对齐、缩放、Undo/Redo。

use crate::EditorApp;
use crate::{AlignMode, SceneTool, SceneViewMode, WorkspaceTab};
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
            ui.separator();
            ui.selectable_value(&mut self.workspace_tab, WorkspaceTab::Scene, "Scene");
            ui.selectable_value(&mut self.workspace_tab, WorkspaceTab::Game, "Game");
            ui.separator();
            ui.selectable_value(&mut self.scene_view_mode, SceneViewMode::View2D, "2D");
            ui.selectable_value(&mut self.scene_view_mode, SceneViewMode::View3D, "3D");
            ui.separator();
            ui.selectable_value(&mut self.scene_tool, SceneTool::Select, "Select");
            ui.selectable_value(&mut self.scene_tool, SceneTool::Move, "Move");
            ui.selectable_value(&mut self.scene_tool, SceneTool::Rotate, "Rotate");
            ui.selectable_value(&mut self.scene_tool, SceneTool::Scale, "Scale");
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
            ui.separator();
            if ui.button("Undo").clicked() && self.project_loaded {
                self.undo();
            }
            if ui.button("Redo").clicked() && self.project_loaded {
                self.redo();
            }
        });
    }
}
