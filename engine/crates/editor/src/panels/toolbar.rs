//! 中央 Scene/Game 工作区工具栏（Unity 式分层）：
//! 常驻 = Play/Stop、3D/2D 切换；其余按`编辑 ▸ 工具栏`里的勾选按需出现。
//! 默认只开「暂停/继续 + 设计分辨率 + 场景缩放」三项，窄面板也能一行放下；
//! 需要设备模拟/吸附/工具/排序/覆盖层的人自己勾出来，不再为所有人常驻一排。
//! Undo/Redo 通过快捷键 Ctrl+Z / Ctrl+Y 触发（见 main.rs）。
//! 工具按钮（Select/Move/Rotate/Scale）在 Scene 视图右上角的小面板中（见 panels/scene_view/）。
//! Scene/Game 切换由 dock 页签与"窗口"菜单承担，工具栏不再重复放按钮。

use crate::EditorApp;
use crate::panels::scene_view::{SCENE_ZOOM_MAX, SCENE_ZOOM_MIN};
use crate::util::{icon_button, icon_toggle, pause_resume_icon, pause_resume_icon_button};
use crate::{AlignMode, SceneViewMode};
use eframe::egui;
use std::collections::BTreeMap;

/// 缩放滑条的轨道宽度：egui 默认 100pt 在工具栏里太占位，收窄到能稳定拖动即可。
const ZOOM_SLIDER_WIDTH: f32 = 60.0;
/// 可勾选项的数量（可见性位图的长度）。
const TOOLBAR_ITEM_COUNT: usize = 11;

/// 工具栏里**可以勾选**的项目。Play/Stop 与 3D 切换是核心入口，恒常驻、不列在这里。
///
/// 标签同时用作「编辑 ▸ 工具栏」的复选框文案，`key` 是 `layout.json` 的持久化键
/// （与 `panel_visible` 同一套 BTreeMap 机制，未知键原样保留）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolbarItem {
    PauseResume,
    /// 单帧步进 + 重新开始运行。
    PlaybackExtras,
    ApplyPlayEdits,
    Resolution,
    DevicePreset,
    GameViewZoom,
    /// 吸附开关 + 步长下拉。
    Snap,
    Tools,
    Sort,
    Overlays,
    SceneZoom,
}

impl ToolbarItem {
    pub(crate) const ALL: [Self; TOOLBAR_ITEM_COUNT] = [
        Self::PauseResume,
        Self::PlaybackExtras,
        Self::ApplyPlayEdits,
        Self::Resolution,
        Self::DevicePreset,
        Self::GameViewZoom,
        Self::Snap,
        Self::Tools,
        Self::Sort,
        Self::Overlays,
        Self::SceneZoom,
    ];

    const fn index(self) -> usize {
        match self {
            Self::PauseResume => 0,
            Self::PlaybackExtras => 1,
            Self::ApplyPlayEdits => 2,
            Self::Resolution => 3,
            Self::DevicePreset => 4,
            Self::GameViewZoom => 5,
            Self::Snap => 6,
            Self::Tools => 7,
            Self::Sort => 8,
            Self::Overlays => 9,
            Self::SceneZoom => 10,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::PauseResume => "暂停 / 继续",
            Self::PlaybackExtras => "单帧 / 重启",
            Self::ApplyPlayEdits => "应用到场景",
            Self::Resolution => "设计分辨率",
            Self::DevicePreset => "设备模拟",
            Self::GameViewZoom => "Game 视图缩放",
            Self::Snap => "吸附与步长",
            Self::Tools => "工具菜单",
            Self::Sort => "排序菜单",
            Self::Overlays => "覆盖层菜单",
            Self::SceneZoom => "场景缩放",
        }
    }

    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::PauseResume => "pause_resume",
            Self::PlaybackExtras => "playback_extras",
            Self::ApplyPlayEdits => "apply_play_edits",
            Self::Resolution => "resolution",
            Self::DevicePreset => "device_preset",
            Self::GameViewZoom => "game_view_zoom",
            Self::Snap => "snap",
            Self::Tools => "tools",
            Self::Sort => "sort",
            Self::Overlays => "overlays",
            Self::SceneZoom => "scene_zoom",
        }
    }

    /// 默认精简集：播放暂停 + 设计分辨率 + 场景缩放。其余项都在编辑菜单里按需打开。
    const fn default_visible(self) -> bool {
        matches!(self, Self::PauseResume | Self::Resolution | Self::SceneZoom)
    }
}

/// 工具栏项的可见性。运行时只认这一份状态，`layout.json` 只是它的持久化镜像。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ToolbarVisibility {
    flags: [bool; TOOLBAR_ITEM_COUNT],
}

impl Default for ToolbarVisibility {
    fn default() -> Self {
        let mut flags = [false; TOOLBAR_ITEM_COUNT];
        for (index, item) in ToolbarItem::ALL.iter().enumerate() {
            flags[index] = item.default_visible();
        }
        Self { flags }
    }
}

impl ToolbarVisibility {
    pub(crate) fn visible(self, item: ToolbarItem) -> bool {
        self.flags[item.index()]
    }

    pub(crate) fn set(&mut self, item: ToolbarItem, visible: bool) {
        self.flags[item.index()] = visible;
    }

    /// 写进 `EditorLayout::toolbar_visible`（全量写，缺项即回落默认）。
    ///
    /// 按值取 `self`（`ToolbarVisibility` 是 `Copy`，只有几十字节的位图）：clippy 的
    /// `wrong_self_convention` 会判 `to_*` 配 `&self` 为误用——`to_` 约定表示"转成另一个
    /// 类型"，本就该消费/复制自值，而不是借一个引用再复制。
    pub(crate) fn to_layout(self) -> BTreeMap<String, bool> {
        ToolbarItem::ALL
            .iter()
            .map(|item| (item.key().to_owned(), self.visible(*item)))
            .collect()
    }

    /// 从布局读回；未知键忽略，缺键保留默认精简集。
    pub(crate) fn from_layout(map: &BTreeMap<String, bool>) -> Self {
        let mut visibility = Self::default();
        for item in ToolbarItem::ALL {
            if let Some(visible) = map.get(item.key()) {
                visibility.set(item, *visible);
            }
        }
        visibility
    }
}

/// 只有前面已经画过控件才插分隔线，避免默认精简集里出现一串空 `|`。
fn separator_if_drawn(ui: &mut egui::Ui, drawn: &mut bool) {
    if *drawn {
        ui.separator();
    }
}

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

/// 当前分辨率命中的比例标签（如 `16:9`）：进分辨率下拉的选中文本，
/// 不必再常驻一排比例按钮。
pub(crate) fn aspect_label_for(size: (u32, u32)) -> Option<&'static str> {
    PREVIEW_ASPECTS
        .iter()
        .find(|(_, ratio)| resolution_matches_aspect(size, *ratio))
        .map(|(label, _)| *label)
}

impl EditorApp {
    /// 工具栏项当前是否显示（菜单与绘制共用同一份运行时状态）。
    pub(crate) fn toolbar_item_visible(&self, item: ToolbarItem) -> bool {
        self.toolbar_visibility.visible(item)
    }

    /// 「编辑 ▸ 工具栏」复选框写入口：这里是可见性的唯一写入点。
    pub(crate) fn set_toolbar_item_visible(&mut self, item: ToolbarItem, visible: bool) {
        self.toolbar_visibility.set(item, visible);
    }

    pub(crate) fn draw_workspace_toolbar(&mut self, ui: &mut egui::Ui) {
        // 默认精简集一行放得下；只有用户把「编辑 ▸ 工具栏」全勾上时才需要横向滚动。
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
                    let mut drawn = true;
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
                    if self.toolbar_item_visible(ToolbarItem::PauseResume) {
                        separator_if_drawn(ui, &mut drawn);
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
                    }
                    if self.toolbar_item_visible(ToolbarItem::PlaybackExtras) {
                        separator_if_drawn(ui, &mut drawn);
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
                    }
                    // P1：Play 期调参的出口。按钮只在运行中出现，角标是"记下来几个字段值"；
                    // 不点它、直接 Stop 就是 Unity 的默认语义（回滚到编辑快照）。
                    if self.toolbar_item_visible(ToolbarItem::ApplyPlayEdits) && in_play {
                        separator_if_drawn(ui, &mut drawn);
                        let pending = self.pending_play_edit_count();
                        let label = if pending == 0 {
                            "应用到场景".to_owned()
                        } else {
                            format!("应用到场景（{pending}）")
                        };
                        if ui
                            .add_enabled(pending > 0, egui::Button::new(label).small())
                            .on_hover_text(
                                "把这次运行里改过的取值写回场景（一条撤销）；不点则 Stop 后丢弃",
                            )
                            .clicked()
                        {
                            self.dispatch(editor::EditorCommand::ApplyPlayEdits);
                        }
                    }
                    if self.toolbar_item_visible(ToolbarItem::Resolution) {
                        separator_if_drawn(ui, &mut drawn);
                        // 设计分辨率：Game 视图渲染目标 / Scene 视图包围框尺寸（下次 Play 生效；可自定义增删）。
                        // 比例快捷组收进同一菜单，选中文本带上当前比例，顶上不再常驻一排按钮。
                        let resolutions = self.preview_resolutions.clone();
                        let resolution_text = match aspect_label_for(self.preview_resolution) {
                            Some(label) => format!(
                                "{}×{} · {label}",
                                self.preview_resolution.0, self.preview_resolution.1
                            ),
                            None => format!(
                                "{}×{}",
                                self.preview_resolution.0, self.preview_resolution.1
                            ),
                        };
                        egui::ComboBox::from_id_salt("preview_resolution")
                            .selected_text(resolution_text)
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
                                        // 手动选择 = 退出机型模拟，再按竖屏锁定收敛方向。
                                        self.set_custom_resolution((*w, *h));
                                        self.adding_preview_resolution = false;
                                        self.scene_view_fit_pending = true;
                                    }
                                }

                                ui.separator();
                                ui.label(egui::RichText::new("按比例切换（保持长边）").weak().size(11.0));
                                ui.horizontal(|ui| {
                                    for (label, ratio) in PREVIEW_ASPECTS {
                                        let selected =
                                            resolution_matches_aspect(self.preview_resolution, ratio);
                                        if ui
                                            .selectable_label(selected, label)
                                            .on_hover_text(format!(
                                                "切换为 {label} 比例（保持当前长边）"
                                            ))
                                            .clicked()
                                        {
                                            let size =
                                                resolution_for_aspect(self.preview_resolution, ratio);
                                            if size != self.preview_resolution {
                                                self.set_custom_resolution(size);
                                                self.adding_preview_resolution = false;
                                                self.scene_view_fit_pending = true;
                                            }
                                            ui.close();
                                        }
                                    }
                                });

                                ui.separator();
                                if ui.button("＋ 自定义…").clicked() {
                                    self.custom_preview_resolution = self.preview_resolution;
                                    self.adding_preview_resolution = true;
                                    ui.close();
                                }
                            });
                    }
                    if self.toolbar_item_visible(ToolbarItem::DevicePreset) {
                        separator_if_drawn(ui, &mut drawn);
                        // 设备模拟：按机型预设一次性给设计分辨率 + 显示缩放档(DPI) + 安全区
                        // （刘海/底部条）+ 竖屏锁定。三端（PC/Web/微信）验收多分辨率不再靠
                        // 人肉改窗口再截图：切预设 = 切真机。
                        let device_text = match self.device_sim.active() {
                            Some(preset) => format!(
                                "{} · {}×{}",
                                preset.name, preset.resolution.0, preset.resolution.1
                            ),
                            None => "自定义".to_owned(),
                        };
                        egui::ComboBox::from_id_salt("device_preset")
                            .selected_text(device_text)
                            .width(170.0)
                            .show_ui(ui, |ui| {
                                for (index, preset) in crate::device_sim::DEVICE_PRESETS
                                    .iter()
                                    .enumerate()
                                {
                                    if ui
                                        .selectable_label(
                                            self.device_sim.preset == Some(index),
                                            format!(
                                                "{}（{}×{}）",
                                                preset.name, preset.resolution.0, preset.resolution.1
                                            ),
                                        )
                                        .clicked()
                                    {
                                        self.apply_device_preset(Some(index));
                                        self.scene_view_fit_pending = true;
                                        ui.close();
                                    }
                                }
                                ui.separator();
                                if ui
                                    .selectable_label(
                                        self.device_sim.preset.is_none(),
                                        "自定义（不模拟）",
                                    )
                                    .clicked()
                                {
                                    self.apply_device_preset(None);
                                    self.scene_view_fit_pending = true;
                                    ui.close();
                                }
                            })
                            .response
                            .on_hover_text(
                                "设备模拟：机型设计分辨率 / 显示缩放档(DPI) / 安全区（刘海·底部条）/ 竖屏锁定",
                            );
                    }
                    if self.toolbar_item_visible(ToolbarItem::GameViewZoom) {
                        separator_if_drawn(ui, &mut drawn);
                        // 想看字就切 `1:1`（1 设计像素 = 1 物理像素，超出部分裁剪 + 中键拖动）。
                    egui::ComboBox::from_id_salt("game_view_zoom")
                        .selected_text(self.game_view_zoom.label())
                            .width(66.0)
                            .show_ui(ui, |ui| {
                                for zoom in crate::panels::game_view::GameViewZoom::ALL {
                                    if ui
                                        .selectable_value(
                                            &mut self.game_view_zoom,
                                            zoom,
                                            zoom.label(),
                                        )
                                        .changed()
                                    {
                                        self.game_view_pan = egui::Vec2::ZERO;
                                    }
                                }
                            })
                            .response
                            .on_hover_text(
                                "Game 视图缩放：适应窗口 / 1:1（1 设计像素 = 1 物理像素，可中键拖动）",
                            );
                    }
                    // 3D/2D 是核心入口，恒常驻。
                    separator_if_drawn(ui, &mut drawn);
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
                    if self.toolbar_item_visible(ToolbarItem::Snap) {
                        separator_if_drawn(ui, &mut drawn);
                        // Unity 式分层：吸附开关 + 步长收进「▾」弹层（Unity Snap Settings 同构）。
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
                    }
                    if self.toolbar_item_visible(ToolbarItem::Tools) {
                        separator_if_drawn(ui, &mut drawn);
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
                            ui.separator();
                            if ui.button("程序化素材…").clicked() {
                                self.procedural.show = true;
                                ui.close();
                            }
                        });
                    }
                    if self.toolbar_item_visible(ToolbarItem::Sort) {
                        separator_if_drawn(ui, &mut drawn);
                            // Sprite 排序是 2D 精灵专属。
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
                    if self.toolbar_item_visible(ToolbarItem::Overlays) {
                        separator_if_drawn(ui, &mut drawn);
                            ui.menu_button("覆盖层", |ui| {
                            ui.checkbox(&mut self.show_scene_guides, "辅助线");
                            ui.checkbox(&mut self.show_safe_area, "安全区");
                            ui.checkbox(&mut self.show_pixel_grid, "像素网格");
                            ui.checkbox(&mut self.show_entity_labels, "标记与名字")
                                .on_hover_text(
                                    "3D 场景视图里每个实体的菱形标记与名称；关闭后场景不再被文字遮挡（点选与 Gizmo 照常可用）",
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
                    }
                    if self.toolbar_item_visible(ToolbarItem::SceneZoom) {
                        separator_if_drawn(ui, &mut drawn);
                            let zoom_before_slider = self.scene_zoom;
                        // spacing 改动只在 scope 内生效，否则会连带改窄本面板其它滑条。
                        ui.scope(|ui| {
                            ui.spacing_mut().slider_width = ZOOM_SLIDER_WIDTH;
                            ui.add(
                                egui::Slider::new(
                                    &mut self.scene_zoom,
                                    SCENE_ZOOM_MIN..=SCENE_ZOOM_MAX,
                                )
                                .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
                            )
                            .on_hover_text("场景缩放");
                        });
                    self.set_scene_zoom_centered(zoom_before_slider, self.scene_zoom);
                    }
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
                    self.set_custom_resolution(self.custom_preview_resolution);
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
    use super::{
        ToolbarItem, ToolbarVisibility, aspect_label_for, resolution_for_aspect,
        resolution_matches_aspect,
    };
    use crate::EditorApp;
    use eframe::egui;
    use rustlua_infra::logging::LogSink;
    use std::collections::{BTreeMap, BTreeSet};

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
        // 选中文本用比例标签：命中预设给标签，非预设比例不给。
        assert_eq!(aspect_label_for((1280, 720)), Some("16:9"));
        assert_eq!(aspect_label_for((1280, 800)), Some("16:10"));
        assert_eq!(aspect_label_for((1000, 1000)), None);
    }

    /// 默认精简集：只有高频三项；低频项一个都不出现（否则窄面板又要滚动）。
    #[test]
    fn default_toolbar_keeps_only_primary_items() {
        let visibility = ToolbarVisibility::default();
        for item in ToolbarItem::ALL {
            let expected = matches!(
                item,
                ToolbarItem::PauseResume | ToolbarItem::Resolution | ToolbarItem::SceneZoom
            );
            assert_eq!(visibility.visible(item), expected, "{item:?}");
        }
        // 持久化键必须一一对应：撞名会让两个复选框互相踩。
        let keys: BTreeSet<_> = ToolbarItem::ALL.iter().map(|item| item.key()).collect();
        assert_eq!(keys.len(), ToolbarItem::ALL.len());
    }

    /// 勾选结果随 layout.json 往返；缺键回落精简默认，未知键忽略。
    #[test]
    fn toolbar_visibility_round_trips_through_layout_map() {
        let mut visibility = ToolbarVisibility::default();
        visibility.set(ToolbarItem::DevicePreset, true);
        visibility.set(ToolbarItem::Overlays, true);
        visibility.set(ToolbarItem::Resolution, false);
        let stored = visibility.to_layout();
        assert_eq!(stored.get(ToolbarItem::DevicePreset.key()), Some(&true));
        assert_eq!(ToolbarVisibility::from_layout(&stored), visibility);

        let mut partial = BTreeMap::new();
        partial.insert(ToolbarItem::DevicePreset.key().to_owned(), true);
        partial.insert("unknown_item".to_owned(), true);
        let restored = ToolbarVisibility::from_layout(&partial);
        assert!(restored.visible(ToolbarItem::DevicePreset));
        assert!(!restored.visible(ToolbarItem::Overlays));
        assert!(restored.visible(ToolbarItem::PauseResume), "缺键回落默认");
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
                egui::vec2(520.0, 500.0),
            )),
            ..Default::default()
        };
        // 精简集不画设备模拟/Game 缩放/三个菜单，520px 的窄叶子也无需滚动。
        assert!(!app.toolbar_item_visible(ToolbarItem::DevicePreset));
        assert!(!app.toolbar_item_visible(ToolbarItem::GameViewZoom));
        assert!(!app.toolbar_item_visible(ToolbarItem::Tools));
        assert!(!app.toolbar_item_visible(ToolbarItem::Sort));
        assert!(!app.toolbar_item_visible(ToolbarItem::Overlays));
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
