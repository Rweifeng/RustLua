//! 中央 Scene/Game 工作区工具栏（Unity 式分层）：
//! 常驻 = Play 控制、设计分辨率、3D/UI、吸附+步长、缩放；
//! 次要功能收进下拉 = 工具（对齐/等间距）、排序（Sprite）、覆盖层（辅助线等开关）。
//! Undo/Redo 通过快捷键 Ctrl+Z / Ctrl+Y 触发（见 main.rs）。
//! 工具按钮（Select/Move/Rotate/Scale）在 Scene 视图右上角的小面板中（见 scene_view.rs）。
//! Scene/Game 切换由 dock 页签与"窗口"菜单承担，工具栏不再重复放按钮。

use crate::EditorApp;
use crate::panels::scene_view::{SCENE_ZOOM_MAX, SCENE_ZOOM_MIN};
use crate::util::{icon_button, icon_toggle, pause_resume_icon, pause_resume_icon_button};
use crate::{AlignMode, SceneViewMode};
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
            // 不再用 U+275A 双竖杠这类字形——字体缺字形时会渲染成"豆腐块"。
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
            if self.scene_view_mode == SceneViewMode::Widget {
                // Unity 式分层：调色板收进「控件」下拉，不再整排常驻工具栏。
                ui.menu_button("控件", |ui| {
                    if ui.button("New Widget").clicked() {
                        self.request_document_change(crate::actions::PendingChange::CreateWidget {
                            name: "New Widget".to_owned(),
                        });
                        ui.close();
                    }
                    ui.separator();
                    // 5b：统一走 create_widget_at_center，与拖放创建共用同一路径。
                    // M27-01：拖拽载荷换成 `DropPayload::Widget`——与资源/实体的拖放同一类型，
                    // 落点矩阵（`dnd::plan`）才能对四种来源用同一张表。
                    for kind in crate::dnd::WIDGET_PALETTE {
                        let source = ui.dnd_drag_source(
                            egui::Id::new(("widget_palette", kind)),
                            crate::dnd::DropPayload::Widget(kind),
                            |ui| ui.button(*kind),
                        );
                        if source.inner.clicked() {
                            self.create_widget_at_center(kind);
                            ui.close();
                        }
                        if source.response.drag_started() {
                            // 拖拽起步就收起下拉，浮层不再挡住画布落点。
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("点击创建 / 拖到画布");
            }
            ui.separator();
            // Unity 式分层：常驻只留高频项（吸附开关），步长收进「▾」弹层（Unity Snap Settings 同构）。
            ui.checkbox(&mut self.snap_enabled, "吸附");
            // 三角是画笔画的，不是 `▾` 字形（字体缺它，会渲染成豆腐块）。
            let snap_steps_button = crate::util::chevron_down_button(
                ui,
                "吸附步长：位移（世界单位）/ 旋转（度），共用左侧开关",
            );
            egui::Popup::menu(&snap_steps_button).show(|ui| {
                ui.horizontal(|ui| {
                    ui.label("位移步长");
                    ui.add(
                        egui::DragValue::new(&mut self.snap_step)
                            .range(0.05..=10.0)
                            .speed(0.05),
                    );
                    ui.label("世界单位");
                });
                // M27-02：旋转**角度**吸附。与位移吸附共用同一个开关，但单位必须是度数——
                // 拿 0.5 的位移步长当角度用（0.5°）不叫吸附，叫抖。
                ui.horizontal(|ui| {
                    ui.label("旋转步长");
                    ui.add(
                        egui::DragValue::new(&mut self.snap_angle_step)
                            .range(1.0..=90.0)
                            .speed(1.0)
                            .suffix("°"),
                    );
                });
            });
            ui.menu_button("工具", |ui| {
                if ui.button("对齐到网格").clicked() {
                    self.align_selected(AlignMode::Grid);
                    ui.close();
                }
                if ui.button("水平对齐").clicked() {
                    self.align_selected(AlignMode::H);
                    ui.close();
                }
                if ui.button("垂直对齐").clicked() {
                    self.align_selected(AlignMode::V);
                    ui.close();
                }
                ui.separator();
                if ui.button("水平等间距").clicked() {
                    self.distribute_selected(true);
                    ui.close();
                }
                if ui.button("垂直等间距").clicked() {
                    self.distribute_selected(false);
                    ui.close();
                }
            });
            // Sprite 排序是 2D 精灵专属，Widget 画布上没有作用对象，直接不出现。
            if self.scene_view_mode != SceneViewMode::Widget {
                ui.menu_button("排序", |ui| {
                    if ui.button("Sprite 排序层级 -1").clicked() {
                        self.nudge_selected_sprite_order(-1);
                        ui.close();
                    }
                    if ui.button("Sprite 排序层级 +1").clicked() {
                        self.nudge_selected_sprite_order(1);
                        ui.close();
                    }
                    if ui.button("按 Y 重排 Sprite 排序").clicked() {
                        self.sort_selected_sprites_by_y(true);
                        ui.close();
                    }
                });
            }
            ui.menu_button("覆盖层", |ui| {
                ui.checkbox(&mut self.show_scene_guides, "辅助线");
                ui.checkbox(&mut self.show_safe_area, "Safe Area");
                ui.checkbox(&mut self.show_pixel_grid, "像素网格");
                ui.checkbox(&mut self.show_collision_overlay, "碰撞区域")
                    .on_hover_text(
                        "画出场景里所有碰撞区域：Tilemap solid 格 + 实体 Collider 轮廓（调试用）",
                    );
                ui.checkbox(&mut self.show_performance_panel, "性能")
                    .on_hover_text("性能面板（独立弹窗：帧/面板/命令耗时、缓存与队列计数）");
                // M19-03：出口/地点只读覆盖层 + 两个可开关的生产工具窗口。
                ui.checkbox(&mut self.show_domain_overlay, "出口/地点");
                ui.checkbox(&mut self.show_domain_data_panel, "领域数据面板")
                    .on_hover_text("Lua 校验 / PersistId / 出口地点");
            });
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
