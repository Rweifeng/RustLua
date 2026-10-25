//! 中央 Scene/Game 工作区工具栏（Unity 式分层）：
//! 常驻 = Play 控制、设计分辨率、3D/UI、吸附+步长、缩放；
//! 次要功能收进下拉 = 工具（对齐/等间距）、排序（Sprite）、覆盖层（辅助线等开关）。
//! Undo/Redo 通过快捷键 Ctrl+Z / Ctrl+Y 触发（见 main.rs）。
//! 工具按钮（Select/Move/Rotate/Scale）在 Scene 视图右上角的小面板中（见 panels/scene_view/）。
//! Scene/Game 切换由 dock 页签与"窗口"菜单承担，工具栏不再重复放按钮。

use crate::EditorApp;
use crate::panels::scene_view::{SCENE_ZOOM_MAX, SCENE_ZOOM_MIN};
use crate::util::{icon_button, icon_toggle, pause_resume_icon, pause_resume_icon_button};
use crate::{AlignMode, SceneViewMode};
use eframe::egui;

/// 缩放滑条的轨道宽度：egui 默认 100pt 在工具栏里太占位，收窄到能稳定拖动即可。
const ZOOM_SLIDER_WIDTH: f32 = 60.0;

/// E18：比例快捷组（顺序即工具栏顺序），比例为宽/高。
pub(crate) const PREVIEW_ASPECTS: [(&str, f32); 5] = [
    ("16:9", 16.0 / 9.0),
    ("16:10", 16.0 / 10.0),
    ("21:9", 21.0 / 9.0),
    ("4:3", 4.0 / 3.0),
    ("9:16", 9.0 / 16.0),
];

/// 保持当前长边、按目标比例换掉另一条边（四舍五入，至少 1px）。
/// 例如 1280×720 → 16:10 得到 1280×800，9:16 得到 720×1280。
pub(crate) fn resolution_for_aspect(current: (u32, u32), ratio: f32) -> (u32, u32) {
    let long = current.0.max(current.1).max(1);
    if ratio >= 1.0 {
        (long, ((long as f32) / ratio).round().max(1.0) as u32)
    } else {
        (((long as f32) * ratio).round().max(1.0) as u32, long)
    }
}

/// 当前分辨率是否已落在该比例上（0.5% 容差，覆盖 1919×1080 这类取整结果）。
pub(crate) fn resolution_matches_aspect(size: (u32, u32), ratio: f32) -> bool {
    if size.0 == 0 || size.1 == 0 {
        return false;
    }
    ((size.0 as f32 / size.1 as f32) - ratio).abs() <= ratio * 0.005
}

impl EditorApp {
    pub(crate) fn draw_workspace_toolbar(&mut self, ui: &mut egui::Ui) {
        // 工具栏保持单行；窄面板通过横向滚动访问末尾控件，避免换行挤压 Scene 画布。
        egui::ScrollArea::horizontal()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Play/Stop：场景与游戏运行控制，同一按钮切换，不显示 Play 字样。
                    let playing = self.is_playing();
                    let (icon, tooltip) = if playing {
                        ("■", "停止运行")
                    } else {
                        ("▶", "开始运行")
                    };
                    if icon_button(ui, icon, tooltip).clicked() && (playing || self.project_loaded)
                    {
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
                    if pause_resume_icon_button(
                        ui,
                        pause_resume_icon(paused),
                        pausable,
                        pause_tooltip,
                    )
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
                    // E18：比例快捷组——改的是同一份 `preview_resolution`（不动场景数据）。
                    ui.separator();
                    for (label, ratio) in PREVIEW_ASPECTS {
                        let selected = resolution_matches_aspect(self.preview_resolution, ratio);
                        if ui
                            .selectable_label(selected, label)
                            .on_hover_text(format!("切换为 {label} 比例（保持当前长边）"))
                            .clicked()
                        {
                            let size = resolution_for_aspect(self.preview_resolution, ratio);
                            if size != self.preview_resolution {
                                self.preview_resolution = size;
                                self.adding_preview_resolution = false;
                                self.scene_view_fit_pending = true;
                            }
                        }
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
                        "控件模式（界面画布）",
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
                            if ui.button("新建控件").clicked() {
                                self.request_document_change(
                                    crate::actions::PendingChange::CreateWidget {
                                        name: "新建控件".to_owned(),
                                    },
                                );
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
                                    |ui| ui.button(crate::util::widget_display_name(kind)),
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
                            if ui.button("精灵排序层级 -1").clicked() {
                                self.nudge_selected_sprite_order(-1);
                                ui.close();
                            }
                            if ui.button("精灵排序层级 +1").clicked() {
                                self.nudge_selected_sprite_order(1);
                                ui.close();
                            }
                            if ui.button("按 Y 重排精灵排序").clicked() {
                                self.sort_selected_sprites_by_y(true);
                                ui.close();
                            }
                        });
                    }
                    ui.menu_button("覆盖层", |ui| {
                        ui.checkbox(&mut self.show_scene_guides, "辅助线");
                        ui.checkbox(&mut self.show_safe_area, "安全区");
                        ui.checkbox(&mut self.show_pixel_grid, "像素网格");
                        ui.checkbox(&mut self.show_layout_debug, "布局调试")
                            .on_hover_text(
                                "控件模式下画出所有界面元素的布局边界、容器内边距与间距，溢出元素标红",
                            );
                        ui.checkbox(&mut self.show_collision_overlay, "碰撞区域")
                    .on_hover_text(
                        "画出场景里所有碰撞区域：瓦片地图碰撞格 + 实体碰撞体轮廓（调试用）",
                    );
                        ui.checkbox(&mut self.show_performance_panel, "性能")
                            .on_hover_text(
                                "性能分析面板（独立弹窗：播放运行时帧率 / 绘制调用 / 设置通道 / 顶点面数 / CPU/GPU / 内存 / 资源缓存，以及编辑器各面板耗时）",
                            );
                        // M19-03：出口/地点只读覆盖层 + 两个可开关的生产工具窗口。
                        ui.checkbox(&mut self.show_domain_overlay, "出口/地点");
                        ui.checkbox(&mut self.show_domain_data_panel, "领域数据面板")
                            .on_hover_text("Lua 校验 / 持久化 ID / 出口地点");
                    });
                    ui.separator();
                    let zoom_before_slider = self.scene_zoom;
                    // spacing 改动只在 scope 内生效，否则会连带改窄本面板其它滑条。
                    ui.scope(|ui| {
                        ui.spacing_mut().slider_width = ZOOM_SLIDER_WIDTH;
                        ui.add(egui::Slider::new(
                            &mut self.scene_zoom,
                            SCENE_ZOOM_MIN..=SCENE_ZOOM_MAX,
                        ))
                        .on_hover_text("场景缩放");
                    });
                    self.set_scene_zoom_centered(zoom_before_slider, self.scene_zoom);
                });
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

#[cfg(test)]
mod tests {
    use super::{resolution_for_aspect, resolution_matches_aspect};
    use crate::EditorApp;
    use eframe::egui;
    use rustlua_infra::logging::LogSink;

    /// E18：比例切换保持长边。1280×720 换 21:9 时高变为 549（1280×9/21 取整），
    /// 换 9:16 时长宽互换为 720×1280。
    #[test]
    fn aspect_quick_switch_keeps_long_edge() {
        assert_eq!(resolution_for_aspect((1280, 720), 16.0 / 9.0), (1280, 720));
        assert_eq!(resolution_for_aspect((1280, 720), 16.0 / 10.0), (1280, 800));
        assert_eq!(resolution_for_aspect((1280, 720), 21.0 / 9.0), (1280, 549));
        assert_eq!(resolution_for_aspect((1280, 720), 4.0 / 3.0), (1280, 960));
        assert_eq!(resolution_for_aspect((1280, 720), 9.0 / 16.0), (720, 1280));
        assert_eq!(resolution_for_aspect((720, 1280), 16.0 / 9.0), (1280, 720));
    }

    #[test]
    fn aspect_match_tolerates_rounding_but_rejects_wrong_ratio() {
        assert!(resolution_matches_aspect((1920, 1080), 16.0 / 9.0));
        assert!(resolution_matches_aspect((1919, 1080), 16.0 / 9.0));
        assert!(!resolution_matches_aspect((1920, 1080), 16.0 / 10.0));
        assert!(resolution_matches_aspect((1080, 1920), 9.0 / 16.0));
        assert!(!resolution_matches_aspect((0, 0), 1.0));
    }

    #[test]
    fn workspace_toolbar_stays_on_one_row_in_narrow_view() {
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/game_api");
        let project = editor::open_project(&root).expect("打开样板工程");
        let mut app = EditorApp::new(project, None, LogSink::new(16));
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(768.0, 500.0),
            )),
            ..Default::default()
        };
        let _ = context.run(raw, |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let top = ui.next_widget_position().y;
                app.draw_workspace_toolbar(ui);
                let bottom = ui.next_widget_position().y;
                assert!(
                    bottom - top < 45.0,
                    "窄视图工具栏应保持单行，高度为 {:.1}",
                    bottom - top
                );
            });
        });
    }
}
