//! 中央 Scene/Game 工作区工具栏：Play/Stop、视图切换、吸附、对齐、缩放。
//! Undo/Redo 通过快捷键 Ctrl+Z / Ctrl+Y 触发（见 main.rs）。
//! 工具按钮（Select/Move/Rotate/Scale）在 Scene 视图右上角的小面板中（见 scene_view.rs）。

use crate::EditorApp;
use crate::panels::scene_view::{SCENE_ZOOM_MAX, SCENE_ZOOM_MIN};
use crate::util::{icon_button, icon_toggle, pause_resume_icon, pause_resume_icon_button};
use crate::{AlignMode, SceneViewMode, WorkspaceTab};
use eframe::egui;

impl EditorApp {
    pub(crate) fn draw_workspace_toolbar(&mut self, ui: &mut egui::Ui) {
        // 自动换行：窗口/面板较窄时控件折行显示，避免溢出到右侧属性面板区域。
        ui.horizontal_wrapped(|ui| {
            // Play/Stop：场景与游戏运行控制，同一按钮切换，不显示 Play 字样。
            let playing = self.is_playing();
            let (icon, tooltip) = if playing {
                ("■", "停止运行")
            } else {
                ("▶", "开始运行")
            };
            if icon_button(ui, icon, tooltip).clicked() && (playing || self.project_loaded) {
                // typed 命令入口：Play/Stop 的状态机与提交边界统一在 dispatch 内。
                self.dispatch(if playing {
                    editor::EditorCommand::StopPlay
                } else {
                    editor::EditorCommand::StartPlay
                });
            }
            // ER-10：暂停/继续 + 单帧步进 + 重启（仅运行/暂停时可用）。
            // 按钮只发 typed 命令，真正的状态迁移在 `PlayController` 内完成。
            let play_state = self.play_state();
            let paused = play_state == editor::PlayState::Paused;
            let in_play = play_state.is_in_play_mode();
            // 暂停/继续只在 Playing ↔ Paused 之间有定义；启动中/停止中禁用而不静默失败。
            let pausable = matches!(
                play_state,
                editor::PlayState::Playing | editor::PlayState::Paused
            );
            // 暂停/继续：图标由画笔绘制（`pause_resume_icon_button`），
            // 不再用 `❚❚` 这类字形——字体缺字形时会渲染成"豆腐块"。
            let pause_tooltip = if paused {
                "继续运行"
            } else {
                "暂停运行（逻辑时间冻结）"
            };
            if pause_resume_icon_button(ui, pause_resume_icon(paused), pausable, pause_tooltip)
                .clicked()
            {
                self.dispatch(if paused {
                    editor::EditorCommand::ResumePlay
                } else {
                    editor::EditorCommand::PausePlay
                });
            }
            if ui
                .add_enabled(in_play, egui::Button::new("⏭").small())
                .on_hover_text("单帧步进（推进一个逻辑帧后保持暂停）")
                .clicked()
            {
                self.dispatch(editor::EditorCommand::StepPlay { frames: 1 });
            }
            if ui
                .add_enabled(in_play, egui::Button::new("⟲").small())
                .on_hover_text("重新开始运行（丢弃运行态后按编辑器快照重启）")
                .clicked()
            {
                self.dispatch(editor::EditorCommand::RestartPlay);
            }
            // 设计分辨率：Game 视图渲染目标 / Scene 视图包围框尺寸（下次 Play 生效；可自定义增删）。
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
                            self.scene_view_fit_pending = true;
                        }
                    }
                    ui.separator();
                    if ui.button("＋ 自定义…").clicked() {
                        self.custom_preview_resolution = self.preview_resolution;
                        self.adding_preview_resolution = true;
                        ui.close();
                    }
                });
            if icon_button(ui, "×", "删除当前分辨率预设").clicked() {
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
                self.scene_view_fit_pending = true;
            }
            ui.separator();
            if icon_toggle(
                ui,
                self.workspace_tab == WorkspaceTab::Scene,
                "◫",
                "场景工作区",
            )
            .clicked()
            {
                self.workspace_tab = WorkspaceTab::Scene;
            }
            if icon_toggle(
                ui,
                self.workspace_tab == WorkspaceTab::Game,
                "▣",
                "游戏工作区",
            )
            .clicked()
            {
                self.workspace_tab = WorkspaceTab::Game;
            }
            ui.separator();
            // 单一 3D 切换：默认 2D，点亮进入 3D，取消回到 2D。
            if icon_toggle(
                ui,
                self.scene_view_mode == SceneViewMode::View3D,
                "3D",
                "3D 视图（点亮时显示，取消回到 2D）",
            )
            .clicked()
            {
                let was_3d = self.scene_view_mode == SceneViewMode::View3D;
                self.scene_view_mode = if was_3d {
                    SceneViewMode::View2D
                } else {
                    SceneViewMode::View3D
                };
                // docs/55 阶段 2：换模式后重新校验工具（Tile 画笔在 3D 里没有落点）。
                self.revalidate_scene_tool_for_view_mode();
                // 从 3D 切回 2D：把视图适配到设计分辨率包围框。
                if was_3d {
                    self.scene_view_fit_pending = true;
                }
            }
            if icon_toggle(
                ui,
                self.scene_view_mode == SceneViewMode::Widget,
                "UI",
                "Widget Mode（UI 画布）",
            )
            .clicked()
            {
                self.scene_view_mode = if self.scene_view_mode == SceneViewMode::Widget {
                    SceneViewMode::View2D
                } else {
                    SceneViewMode::Widget
                };
                self.revalidate_scene_tool_for_view_mode();
                self.scene_view_fit_pending = true;
            }
            if icon_toggle(
                ui,
                self.scene_view_mode == SceneViewMode::Prefab,
                "Prefab",
                "Prefab Mode",
            )
            .clicked()
            {
                self.scene_view_mode = if self.scene_view_mode == SceneViewMode::Prefab {
                    SceneViewMode::View2D
                } else {
                    SceneViewMode::Prefab
                };
                self.revalidate_scene_tool_for_view_mode();
                self.scene_view_fit_pending = true;
            }
            if self.scene_view_mode == SceneViewMode::Widget {
                ui.separator();
                if ui.button("New Widget").clicked() {
                    self.request_document_change(crate::actions::PendingChange::CreateWidget {
                        name: "New Widget".to_owned(),
                    });
                }
                ui.label("控件");
                if ui.button("Text").clicked() {
                    self.add_entity_with_component("New Text", "Text");
                }
                ui.dnd_drag_source(
                    egui::Id::new("widget_palette_text"),
                    "widget:Text".to_owned(),
                    |ui| ui.small("↕"),
                );
                if ui.button("Image").clicked() {
                    self.add_entity_with_component("New Image", "Image");
                }
                ui.dnd_drag_source(
                    egui::Id::new("widget_palette_image"),
                    "widget:Image".to_owned(),
                    |ui| ui.small("↕"),
                );
                if ui.button("Button").clicked() {
                    self.add_entity_with_component("New Button", "Image");
                    if let Err(error) = self.add_component("UiEvents") {
                        self.status = error;
                    }
                }
                ui.dnd_drag_source(
                    egui::Id::new("widget_palette_button"),
                    "widget:Button".to_owned(),
                    |ui| ui.small("↕"),
                );
            }
            ui.separator();
            ui.checkbox(&mut self.snap_enabled, "吸附");
            ui.checkbox(&mut self.show_scene_guides, "辅助线");
            ui.checkbox(&mut self.show_safe_area, "Safe Area");
            ui.checkbox(&mut self.show_pixel_grid, "像素网格");
            ui.checkbox(&mut self.show_performance_hud, "性能");
            // M19-03：出口/地点只读覆盖层 + 两个可开关的生产工具窗口。
            ui.checkbox(&mut self.show_domain_overlay, "出口/地点");
            if icon_toggle(
                ui,
                self.show_domain_data_panel,
                "域",
                "领域数据面板（Lua 校验 / PersistId / 出口地点）",
            )
            .clicked()
            {
                self.show_domain_data_panel = !self.show_domain_data_panel;
            }
            if icon_toggle(
                ui,
                self.show_runtime_debug_panel,
                "调",
                "运行时调试面板（只读：地图/NPC/任务/农田/建筑/存档）",
            )
            .clicked()
            {
                self.show_runtime_debug_panel = !self.show_runtime_debug_panel;
            }
            ui.add(
                egui::DragValue::new(&mut self.snap_step)
                    .range(0.05..=10.0)
                    .speed(0.05)
                    .prefix(" "),
            );
            if icon_button(ui, "⊞", "对齐到网格").clicked() {
                self.align_selected(AlignMode::Grid);
            }
            if icon_button(ui, "↔", "水平对齐").clicked() {
                self.align_selected(AlignMode::H);
            }
            if icon_button(ui, "↕", "垂直对齐").clicked() {
                self.align_selected(AlignMode::V);
            }
            if icon_button(ui, "⇔", "水平等间距").clicked() {
                self.distribute_selected(true);
            }
            if icon_button(ui, "⇕", "垂直等间距").clicked() {
                self.distribute_selected(false);
            }
            ui.separator();
            if icon_button(ui, "−", "Sprite 排序层级 -1").clicked() {
                self.nudge_selected_sprite_order(-1);
            }
            if icon_button(ui, "＋", "Sprite 排序层级 +1").clicked() {
                self.nudge_selected_sprite_order(1);
            }
            if icon_button(ui, "⇅", "按 Y 重排 Sprite 排序").clicked() {
                self.sort_selected_sprites_by_y(true);
            }
            ui.separator();
            let zoom_before_slider = self.scene_zoom;
            ui.add(
                egui::Slider::new(&mut self.scene_zoom, SCENE_ZOOM_MIN..=SCENE_ZOOM_MAX)
                    .text("Zoom"),
            );
            self.set_scene_zoom_centered(zoom_before_slider, self.scene_zoom);
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
                        self.preview_resolutions
                            .push(self.custom_preview_resolution);
                    }
                    self.preview_resolution = self.custom_preview_resolution;
                    self.scene_view_fit_pending = true;
                    self.adding_preview_resolution = false;
                }
                if ui.button("取消").clicked() {
                    self.adding_preview_resolution = false;
                }
            });
        }
    }
}
