//! P3-E01-3/4：`.anim` 时间线面板（预览 + 编辑）。
//!
//! 面板把「当前选中实体的 `AnimationPlayer`」接到一段 `.anim` 上：播放/暂停/停止、拖动播放头、
//! 选择轨道与关键帧、在播放头插入/删除关键帧、在时间条上拖动关键帧、增删轨道与事件、帧吸附、
//! 撤销/重做，以及把规范文本原子写回工程内的 `.anim` 文件。
//!
//! 边界：
//!
//! - **不写场景文档**：播放头与选中状态只活在 [`TimelineState`] 里；编辑只改
//!   [`AnimEditSession`] 持有的片段；面板不调用 `EditorCommand`，也不碰 `document` / `history`
//!   （架构棘轮 `panels_only_emit_actions` 盯着这一点）。
//! - **不写绝对路径**：`.anim` 的读写都经 `editor::anim_ops`，只接受工程内相对路径；保存先写
//!   临时文件再原子替换，失败保留旧文件。
//! - **未保存修改有保护**：切换选中实体时若有脏片段，面板停在原片段并提示先保存/还原；
//!   「重新加载」在有未保存修改时禁用。
//! - 磁盘上的片段被外部改动后点「重新加载」显式重读（不做每帧 stat，避免把面板变成 I/O 热点）。
//!
//! 明确不做：多选/复制粘贴关键帧、拖动事件标记、编辑关键帧值、改片段名/时长、
//! 以及把采样结果叠到 Scene 视图。

use std::path::{Path, PathBuf};

use eframe::egui;
use rustlua_animation::anim_asset::{AnimClip, AnimPath, AnimValue, TransformChannel};
use rustlua_animation::anim_edit::{AnimEditError, AnimEditSession};
use rustlua_animation::animation_player::{AnimationPlayer, AnimationPlayerEvent};
use rustlua_animation::animation_track::Interpolation;
use rustlua_math::math2d::Vec2;
use rustlua_math::math3d::{Quat, Vec3};
use rustlua_scene::scene::SceneEntityId;

use crate::EditorApp;

/// `.anim` 播放器在场景组件表里的键名（与 schema 注册名一致）。
const COMPONENT: &str = "AnimationPlayer";

/// 预览事件队列上限：面板只展示最近几次，防止长播放把内存拖大。
const MAX_RECENT_EVENTS: usize = 8;

/// 关键帧在时间条上的命中半径（像素）。
const KEY_HIT_RADIUS: f32 = 7.0;

/// 面板要跟踪的选中目标（从文档里摘出来的纯数据，避免把 `&SceneDocument` 借进面板状态）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct TimelineTarget {
    entity: SceneEntityId,
    name: String,
    /// `AnimationPlayer` 组件的原文；`None` = 实体存在但没有这个组件。
    payload: Option<String>,
}

/// 决定「要不要重新读盘」的键：实体 + 组件原文。改名不重载，只更新显示名。
#[derive(Clone, Debug, PartialEq, Eq)]
struct TimelineKey {
    entity: SceneEntityId,
    payload: String,
}

/// 面板状态机：没有可预览的东西时给出**具体原因**，而不是一块空白。
#[derive(Debug, Default)]
pub(crate) enum TimelineStatus {
    #[default]
    NoSelection,
    /// 选中了实体，但它没有 `AnimationPlayer` 组件。
    NoPlayer { name: String },
    /// 组件/片段装配失败（配置串、文件缺失、`.anim` 解析、region/frames 不自洽）。
    Error(String),
    /// 可以预览/编辑。
    Ready(Box<TimelinePreview>),
}

/// 时间条上正在拖动的关键帧。提交放在松手时，拖动过程不产生撤销步。
#[derive(Clone, Copy, Debug)]
struct KeyDrag {
    track: usize,
    key: usize,
    time: f32,
}

/// 一次成功装配的预览：持有播放器与编辑会话，绝不反过来写文档。
#[derive(Debug)]
pub(crate) struct TimelinePreview {
    entity: SceneEntityId,
    entity_name: String,
    clip_path: String,
    project_root: PathBuf,
    player: AnimationPlayer,
    session: AnimEditSession,
    selected_track: Option<usize>,
    selected_key: Option<usize>,
    snap: bool,
    fps: f32,
    new_track: String,
    new_event: String,
    /// `(是否成功, 文案)`：编辑拒绝/保存结果都落在面板里，不弹窗。
    message: Option<(bool, String)>,
    /// 选中已切走，但当前片段有未保存修改，暂不重载。
    switch_blocked: bool,
    drag: Option<KeyDrag>,
    reload_requested: bool,
    recent_events: Vec<AnimationPlayerEvent>,
}

/// 时间线面板的会话状态（不落盘：布局文件只记面板位置，不记播放头）。
#[derive(Debug, Default)]
pub(crate) struct TimelineState {
    key: Option<TimelineKey>,
    status: TimelineStatus,
}

impl TimelineState {
    /// 跟随选中实体同步预览。只在键变化时读盘；有未保存修改时暂缓切换。
    fn sync(&mut self, target: Option<TimelineTarget>, project_root: &Path) {
        let Some(target) = target else {
            self.key = None;
            self.status = TimelineStatus::NoSelection;
            return;
        };
        let next_key = target.payload.as_ref().map(|payload| TimelineKey {
            entity: target.entity,
            payload: payload.clone(),
        });
        if let Some(next_key) = &next_key
            && self.key.as_ref() == Some(next_key)
        {
            // 同一个预览：只刷新显示名（重命名不该触发重新读盘）。
            match &mut self.status {
                TimelineStatus::NoPlayer { name } => {
                    *name = target.name;
                    return;
                }
                TimelineStatus::Ready(preview) => {
                    preview.entity_name = target.name;
                    preview.switch_blocked = false;
                    return;
                }
                // 上次装配失败：同一载荷也允许重试（用户可能刚修好文件）。
                _ => {}
            }
        }
        // 脏会话不能因为选中变化被静默丢弃；保存/还原后下一帧再切。
        if let TimelineStatus::Ready(preview) = &mut self.status
            && preview.session.is_dirty()
        {
            preview.switch_blocked = true;
            return;
        }
        // 没有组件时没有可缓存的载荷；先于键比较返回，否则初始的 `key == None`
        // 会把"选中了一个没有 AnimationPlayer 的实体"当成同一预览而停在 NoSelection。
        let Some(payload) = target.payload else {
            self.key = None;
            self.status = TimelineStatus::NoPlayer { name: target.name };
            return;
        };
        self.key = Some(TimelineKey {
            entity: target.entity,
            payload: payload.clone(),
        });
        self.status = match load_preview(target.entity, &target.name, &payload, project_root) {
            Ok(preview) => TimelineStatus::Ready(Box::new(preview)),
            Err(error) => TimelineStatus::Error(error),
        };
    }

    /// 推进播放（只在面板被画出来的帧里调用；暂停/无片段时是 no-op）。
    fn advance(&mut self, delta: f32) {
        let TimelineStatus::Ready(preview) = &mut self.status else {
            return;
        };
        // 拖动关键帧时播放会移动时间条，先冻结，避免"边拖边跑"。
        if preview.drag.is_some() {
            return;
        }
        let events = preview.player.advance(delta);
        if events.is_empty() {
            return;
        }
        preview.recent_events.extend(events);
        let overflow = preview
            .recent_events
            .len()
            .saturating_sub(MAX_RECENT_EVENTS);
        if overflow > 0 {
            preview.recent_events.drain(..overflow);
        }
    }
}

fn load_preview(
    entity: SceneEntityId,
    entity_name: &str,
    payload: &str,
    project_root: &Path,
) -> Result<TimelinePreview, String> {
    let mut player = AnimationPlayer::from_component_str(payload)
        .map_err(|error| format!("AnimationPlayer 配置无效：{error}"))?;
    let clip_path = player.clip_path().to_owned();
    let absolute = editor::anim_ops::resolve_asset_path(project_root, &clip_path)
        .map_err(|error| format!("动画片段路径无效：{error}"))?;
    let text = std::fs::read_to_string(&absolute).map_err(|error| {
        format!(
            "读不到动画片段 `{clip_path}`（{}）：{error}",
            absolute.display()
        )
    })?;
    let clip = AnimClip::from_text(&clip_path, &text)
        .map_err(|error| format!("解析动画片段 `{clip_path}` 失败：{error}"))?;
    let session = AnimEditSession::new(clip.clone());
    player
        .attach_clip(clip)
        .map_err(|error| format!("片段与播放器不匹配：{error}"))?;
    // 面板打开时先停住：预览的播放必须由用户显式点「播放」，不能因为选中实体就动起来。
    player.set_playing(false);
    Ok(TimelinePreview {
        entity,
        entity_name: entity_name.to_owned(),
        clip_path,
        project_root: project_root.to_path_buf(),
        player,
        selected_track: (!session.clip().tracks.is_empty()).then_some(0),
        session,
        selected_key: None,
        snap: false,
        fps: 30.0,
        new_track: String::new(),
        new_event: String::new(),
        message: None,
        switch_blocked: false,
        drag: None,
        reload_requested: false,
        recent_events: Vec::new(),
    })
}

impl EditorApp {
    /// 从当前选中实体摘出预览目标（只读文档）。
    fn sync_timeline_from_selection(&mut self) {
        let target = self.selection.primary().and_then(|id| {
            let document = self.current_scene()?;
            let entity = document.scene.entities.get(&id)?;
            Some(TimelineTarget {
                entity: id,
                name: entity.name.clone(),
                payload: entity.components.get(COMPONENT).cloned(),
            })
        });
        self.timeline.sync(target, &self.project.root);
    }

    /// 时间线面板 body（dock 标签 `DockTab::Timeline`）。
    pub(crate) fn draw_timeline_panel(&mut self, ui: &mut egui::Ui) {
        self.sync_timeline_from_selection();
        let delta = ui.ctx().input(|input| input.stable_dt).min(0.1);
        self.timeline.advance(delta);
        self.timeline.draw(ui);
    }
}

impl TimelineState {
    fn draw(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("时间线");
            ui.weak("编辑片段；不改场景文档");
        });
        ui.separator();
        match &mut self.status {
            TimelineStatus::NoSelection => {
                ui.weak("在层级或 Scene 视图里选中一个带 AnimationPlayer 的实体。");
            }
            TimelineStatus::NoPlayer { name } => {
                ui.label(format!("实体 `{name}` 没有 AnimationPlayer 组件。"));
                ui.weak(
                    "在属性面板添加 AnimationPlayer（clip=assets/anim/<name>.anim）后即可预览。",
                );
            }
            TimelineStatus::Error(error) => {
                ui.colored_label(egui::Color32::from_rgb(226, 148, 92), error);
            }
            TimelineStatus::Ready(preview) => {
                preview.draw(ui);
                if preview.reload_requested {
                    preview.reload_requested = false;
                    self.key = None;
                }
            }
        }
    }
}

impl TimelinePreview {
    fn draw(&mut self, ui: &mut egui::Ui) {
        self.draw_header(ui);
        self.draw_toolbar(ui);
        self.draw_playhead(ui);
        self.draw_tracks(ui);
        self.draw_events(ui);
        if let Some((ok, message)) = &self.message {
            ui.separator();
            let color = if *ok {
                egui::Color32::from_rgb(120, 190, 130)
            } else {
                egui::Color32::from_rgb(226, 148, 92)
            };
            ui.colored_label(color, message);
        }
    }

    fn draw_header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong(&self.entity_name);
            ui.weak(format!("· #{}", self.entity.0));
            ui.weak(format!(
                "· `{}`（{clip_path}）",
                self.session.clip().name,
                clip_path = self.clip_path
            ));
            ui.weak(format!("· 时长 {:.3}s", self.session.clip().duration));
            ui.weak(if self.player.effective_looped() {
                "· 循环"
            } else {
                "· 单次"
            });
            ui.weak(format!("· 速度 {:.2}", self.player.speed()));
            if self.session.is_dirty() {
                ui.colored_label(egui::Color32::from_rgb(226, 148, 92), "· ● 未保存");
            }
        });
        if self.switch_blocked {
            ui.colored_label(
                egui::Color32::from_rgb(226, 148, 92),
                "选中已变化：当前片段有未保存修改，保存或还原后才会切换。",
            );
        }
    }

    fn draw_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let playing = self.player.is_playing();
            if ui.button(if playing { "暂停" } else { "播放" }).clicked() {
                self.player.set_playing(!playing);
            }
            if ui.button("停止").clicked() {
                self.player.stop();
            }
            if ui
                .add_enabled(!self.session.is_dirty(), egui::Button::new("重新加载"))
                .on_disabled_hover_text("有未保存修改：先保存或还原")
                .clicked()
            {
                self.reload_requested = true;
            }
            ui.separator();
            if ui
                .add_enabled(self.session.can_undo(), egui::Button::new("撤销"))
                .clicked()
            {
                self.undo_edit();
            }
            if ui
                .add_enabled(self.session.can_redo(), egui::Button::new("重做"))
                .clicked()
            {
                self.redo_edit();
            }
            if ui
                .add_enabled(self.session.is_dirty(), egui::Button::new("保存"))
                .clicked()
            {
                self.save_to_disk();
            }
            if ui
                .add_enabled(self.session.is_dirty(), egui::Button::new("还原"))
                .on_disabled_hover_text("还原到最近一次保存/加载的内容")
                .clicked()
            {
                self.revert_edit();
            }
            ui.separator();
            ui.checkbox(&mut self.snap, "吸附");
            ui.add(
                egui::DragValue::new(&mut self.fps)
                    .range(1.0..=240.0)
                    .speed(1.0)
                    .suffix(" fps"),
            );
            if !self.fps.is_finite() {
                self.fps = 30.0;
            }
            ui.separator();
            if ui
                .add_enabled(self.selected_track.is_some(), egui::Button::new("＋键"))
                .on_hover_text("在播放头为选中轨道插入关键帧（值取当前采样值）")
                .clicked()
            {
                self.add_key_at_playhead();
            }
            if ui
                .add_enabled(
                    self.selected_track.is_some() && self.selected_key.is_some(),
                    egui::Button::new("删除键"),
                )
                .clicked()
            {
                self.remove_selected_key();
            }
            if playing {
                // 反应式重绘：不主动请求的话，用户松手后界面停在这一帧、时间不再推进。
                ui.ctx().request_repaint();
            }
        });
    }

    fn draw_playhead(&mut self, ui: &mut egui::Ui) {
        let duration = self.session.clip().duration.max(f32::EPSILON);
        let mut time = self.player.time();
        let slider = ui.add(egui::Slider::new(&mut time, 0.0..=duration).text("播放头"));
        if slider.changed() {
            self.player.set_playing(false);
            let snapped = self.snap_time(time);
            let _ = self.player.seek(snapped);
        }
        self.draw_strip(ui, duration);
    }

    fn draw_strip(&mut self, ui: &mut egui::Ui, duration: f32) {
        let events = self.session.clip().events.clone();
        let keys: Vec<f32> = self
            .selected_track
            .and_then(|index| self.session.clip().tracks.get(index))
            .map(|track| {
                track
                    .keyframes
                    .iter()
                    .map(|keyframe| keyframe.time)
                    .collect()
            })
            .unwrap_or_default();
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 46.0),
            egui::Sense::click_and_drag(),
        );
        let x_of = |time: f32| {
            egui::lerp(
                rect.left()..=rect.right(),
                (time / duration).clamp(0.0, 1.0),
            )
        };
        let time_at =
            |x: f32| ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) * duration;
        let key_hit = |x: f32| {
            keys.iter()
                .enumerate()
                .map(|(index, time)| (index, *time, (x_of(*time) - x).abs()))
                .filter(|(_, _, distance)| *distance <= KEY_HIT_RADIUS)
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .map(|(index, time, _)| (index, time))
        };

        if response.drag_started()
            && let Some(position) = response.interact_pointer_pos()
            && let Some((key, time)) = key_hit(position.x)
            && let Some(track) = self.selected_track
        {
            self.selected_key = Some(key);
            self.player.set_playing(false);
            self.drag = Some(KeyDrag { track, key, time });
        }
        if response.dragged()
            && let Some(position) = response.interact_pointer_pos()
        {
            let snapped = self.snap_time(time_at(position.x));
            if let Some(drag) = &mut self.drag {
                drag.time = snapped;
            }
        }
        if response.drag_stopped()
            && let Some(drag) = self.drag.take()
        {
            let current = self
                .session
                .clip()
                .tracks
                .get(drag.track)
                .and_then(|track| track.keyframes.get(drag.key))
                .map(|keyframe| keyframe.time);
            if current != Some(drag.time) {
                let result = self.session.move_keyframe(drag.track, drag.key, drag.time);
                if let Some(index) = self.applied(result) {
                    self.selected_track = Some(drag.track);
                    self.selected_key = Some(index);
                }
            }
        } else if response.clicked()
            && let Some(position) = response.interact_pointer_pos()
        {
            if let Some((key, time)) = key_hit(position.x) {
                self.selected_key = Some(key);
                self.player.set_playing(false);
                let _ = self.player.seek(time);
            } else {
                self.player.set_playing(false);
                let snapped = self.snap_time(time_at(position.x));
                let _ = self.player.seek(snapped);
            }
        }

        if !ui.is_rect_visible(rect) {
            return;
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let event_color = egui::Color32::from_rgb(120, 170, 220);
        let text_color = ui.visuals().weak_text_color();
        for event in &events {
            let x = x_of(event.time);
            painter.line_segment(
                [
                    egui::pos2(x, rect.top() + 4.0),
                    egui::pos2(x, rect.bottom() - 4.0),
                ],
                egui::Stroke::new(1.0_f32, event_color),
            );
            painter.text(
                egui::pos2(x + 2.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                &event.name,
                egui::FontId::proportional(10.0),
                text_color,
            );
        }
        let key_color = egui::Color32::from_rgb(235, 200, 120);
        let selected_color = egui::Color32::from_rgb(235, 140, 90);
        for (index, time) in keys.iter().enumerate() {
            let selected = self.selected_key == Some(index);
            painter.circle_filled(
                egui::pos2(x_of(*time), rect.center().y),
                if selected { 4.5 } else { 3.5 },
                if selected { selected_color } else { key_color },
            );
        }
        if let Some(drag) = self.drag {
            let position = egui::pos2(x_of(drag.time), rect.center().y);
            painter.circle_stroke(position, 6.0, egui::Stroke::new(1.5_f32, selected_color));
            painter.text(
                egui::pos2(position.x + 6.0, position.y),
                egui::Align2::LEFT_CENTER,
                format!("{:.3}s", drag.time),
                egui::FontId::monospace(10.0),
                ui.visuals().text_color(),
            );
        }
        let current = self
            .session
            .clip()
            .wrap_time(self.player.time())
            .clamp(0.0, duration);
        let x = x_of(current);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0_f32, selected_color),
        );
        painter.text(
            egui::pos2(x + 3.0, rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{current:.3}s"),
            egui::FontId::monospace(10.0),
            ui.visuals().text_color(),
        );
    }

    fn draw_tracks(&mut self, ui: &mut egui::Ui) {
        let samples = self.player.sample_all();
        let rows: Vec<(String, Interpolation, bool, usize, String)> = self
            .session
            .clip()
            .tracks
            .iter()
            .map(|track| {
                let sample = samples
                    .iter()
                    .find(|(path, _)| path == &track.path)
                    .map_or_else(|| "—".to_owned(), |(_, value)| format_value(*value));
                (
                    track.path.to_string(),
                    track.interpolation,
                    track.path.is_discrete(),
                    track.keyframes.len(),
                    sample,
                )
            })
            .collect();
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("轨道");
            ui.weak(format!("{} 条", rows.len()));
            ui.weak("点击选择；选中后可在时间条上拖动关键帧");
        });
        for (index, (path_text, interpolation, discrete, key_count, sample)) in
            rows.into_iter().enumerate()
        {
            let selected = self.selected_track == Some(index);
            let mut choose: Option<Interpolation> = None;
            ui.horizontal(|ui| {
                if ui.selectable_label(selected, &path_text).clicked() {
                    self.selected_track = Some(index);
                    self.selected_key = None;
                }
                ui.weak(format!("{key_count} 键"));
                let mut chosen = interpolation;
                egui::ComboBox::from_id_salt(("anim-interp", index))
                    .selected_text(interpolation_label(chosen))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut chosen, Interpolation::Step, "阶跃");
                        if discrete {
                            ui.add_enabled(false, egui::Button::new("线性（离散轨道不可用）"));
                        } else {
                            ui.selectable_value(&mut chosen, Interpolation::Linear, "线性");
                        }
                    });
                if chosen != interpolation {
                    choose = Some(chosen);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.monospace(&sample)
                });
            });
            if let Some(interpolation) = choose {
                let result = self.session.set_track_interpolation(index, interpolation);
                self.applied(result);
            }
            if selected {
                self.draw_key_chips(ui, index);
            }
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_track)
                    .hint_text("Transform.position / UiAnimation.opacity…")
                    .desired_width(280.0),
            );
            if ui.button("＋轨道").clicked() {
                self.add_track_from_input();
            }
            if ui
                .add_enabled(self.selected_track.is_some(), egui::Button::new("删除轨道"))
                .clicked()
            {
                self.remove_selected_track();
            }
        });
    }

    fn draw_key_chips(&mut self, ui: &mut egui::Ui, track: usize) {
        let Some(track) = self.session.clip().tracks.get(track) else {
            return;
        };
        let times: Vec<f32> = track
            .keyframes
            .iter()
            .map(|keyframe| keyframe.time)
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.weak("关键帧：");
            for (index, time) in times.iter().enumerate() {
                let selected = self.selected_key == Some(index);
                if ui
                    .selectable_label(selected, format!("{time:.3}s"))
                    .clicked()
                {
                    self.selected_key = Some(index);
                    self.player.set_playing(false);
                    let _ = self.player.seek(*time);
                }
            }
        });
    }

    fn draw_events(&mut self, ui: &mut egui::Ui) {
        let events = self.session.clip().events.clone();
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("事件");
            ui.weak(format!("{} 个", events.len()));
            ui.add(
                egui::TextEdit::singleline(&mut self.new_event)
                    .hint_text("事件名")
                    .desired_width(120.0),
            );
            if ui.button("＋事件").clicked() {
                self.add_event_from_input();
            }
        });
        let mut remove = None;
        for (index, event) in events.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{:.3}s", event.time))
                    .on_hover_text("跳到该时间")
                    .clicked()
                {
                    self.player.set_playing(false);
                    let _ = self.player.seek(event.time);
                }
                ui.monospace(&event.name);
                if ui.small_button("删除").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            let result = self.session.remove_event(index);
            self.applied(result);
        }
        if !self.recent_events.is_empty() {
            ui.separator();
            let names: Vec<&str> = self
                .recent_events
                .iter()
                .map(|event| event.name.as_str())
                .collect();
            ui.weak(format!("最近触发：{}", names.join("、")));
        }
    }

    /// 记录一次编辑结果；成功就重建预览（暂停并保留播放头）。
    fn applied<T>(&mut self, result: Result<T, AnimEditError>) -> Option<T> {
        match result {
            Ok(value) => {
                self.message = None;
                self.refresh_after_edit();
                Some(value)
            }
            Err(error) => {
                self.message = Some((false, format!("编辑被拒绝：{error}")));
                None
            }
        }
    }

    fn refresh_after_edit(&mut self) {
        self.message = None;
        if let Err(error) = self.rebuild_player() {
            self.message = Some((false, format!("预览更新失败：{error}")));
        }
    }

    fn rebuild_player(&mut self) -> Result<(), String> {
        let time = self.player.time();
        let clip = self.session.clip().clone();
        self.player
            .attach_clip(clip)
            .map_err(|error| error.to_string())?;
        self.player.set_playing(false);
        let _ = self.player.seek(time.min(self.player.duration()));
        Ok(())
    }

    fn snap_time(&self, time: f32) -> f32 {
        if !self.snap {
            return time.clamp(0.0, self.session.clip().duration);
        }
        self.session
            .snap_time(time, self.fps)
            .unwrap_or_else(|_| time.clamp(0.0, self.session.clip().duration))
    }

    fn add_key_at_playhead(&mut self) {
        let Some(track) = self.selected_track else {
            self.message = Some((false, "先选中一条轨道".to_owned()));
            return;
        };
        let Some(path) = self
            .session
            .clip()
            .tracks
            .get(track)
            .map(|track| track.path.clone())
        else {
            self.message = Some((false, "选中的轨道已不存在".to_owned()));
            return;
        };
        let time = self.snap_time(self.player.time());
        let value = self
            .session
            .clip()
            .sample(&path, time)
            .unwrap_or(AnimValue::Scalar(0.0));
        let result = self.session.add_keyframe(track, time, value);
        if let Some(index) = self.applied(result) {
            self.selected_key = Some(index);
        }
    }

    fn remove_selected_key(&mut self) {
        let (Some(track), Some(key)) = (self.selected_track, self.selected_key) else {
            return;
        };
        let result = self.session.remove_keyframe(track, key);
        if self.applied(result).is_some() {
            self.selected_key = None;
        }
    }

    fn add_track_from_input(&mut self) {
        let raw = self.new_track.trim().to_owned();
        if raw.is_empty() {
            self.message = Some((false, "轨道路径为空".to_owned()));
            return;
        }
        let path = match AnimPath::parse(&raw) {
            Ok(path) => path,
            Err(error) => {
                self.message = Some((false, error));
                return;
            }
        };
        let interpolation = if path.is_discrete() {
            Interpolation::Step
        } else {
            Interpolation::Linear
        };
        let time = self.snap_time(self.player.time());
        let value = default_value(&path);
        let result = self.session.add_track(path, interpolation, time, value);
        if let Some(index) = self.applied(result) {
            self.selected_track = Some(index);
            self.selected_key = Some(0);
            self.new_track.clear();
        }
    }

    fn remove_selected_track(&mut self) {
        let Some(index) = self.selected_track else {
            return;
        };
        let result = self.session.remove_track(index);
        if self.applied(result).is_some() {
            self.selected_track = None;
            self.selected_key = None;
        }
    }

    fn add_event_from_input(&mut self) {
        let name = self.new_event.trim().to_owned();
        if name.is_empty() {
            self.message = Some((false, "事件名为空".to_owned()));
            return;
        }
        let time = self.snap_time(self.player.time());
        let result = self.session.add_event(time, name);
        if self.applied(result).is_some() {
            self.new_event.clear();
        }
    }

    fn undo_edit(&mut self) {
        if self.session.undo() {
            self.selected_key = None;
            self.refresh_after_edit();
        }
    }

    fn redo_edit(&mut self) {
        if self.session.redo() {
            self.selected_key = None;
            self.refresh_after_edit();
        }
    }

    fn revert_edit(&mut self) {
        self.session.revert_to_saved();
        self.selected_key = None;
        self.switch_blocked = false;
        self.refresh_after_edit();
    }

    fn save_to_disk(&mut self) -> bool {
        let text = self.session.text();
        match editor::anim_ops::save_clip(&self.project_root, &self.clip_path, &text) {
            Ok(path) => {
                self.session.mark_saved();
                self.switch_blocked = false;
                self.message = Some((true, format!("已保存 {}", path.display())));
                true
            }
            Err(error) => {
                self.message = Some((false, format!("保存失败：{error}")));
                false
            }
        }
    }
}

fn default_value(path: &AnimPath) -> AnimValue {
    match path {
        AnimPath::Transform(TransformChannel::Position | TransformChannel::Scale) => {
            AnimValue::Vec2(Vec2::new(0.0, 0.0))
        }
        AnimPath::Transform(TransformChannel::Rotation) => AnimValue::Scalar(0.0),
        AnimPath::Transform3D(TransformChannel::Position | TransformChannel::Scale) => {
            AnimValue::Vec3(Vec3::new(0.0, 0.0, 0.0))
        }
        AnimPath::Transform3D(TransformChannel::Rotation) => AnimValue::Quat(Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }),
        AnimPath::SpriteRegion => AnimValue::Region(0),
        AnimPath::UiAnimation(_) => AnimValue::Scalar(0.0),
        AnimPath::LightIntensity => AnimValue::Scalar(1.0),
    }
}

fn interpolation_label(interpolation: Interpolation) -> &'static str {
    match interpolation {
        Interpolation::Step => "阶跃",
        Interpolation::Linear => "线性",
    }
}

fn format_value(value: AnimValue) -> String {
    match value {
        AnimValue::Vec2(value) => format!("({:.3}, {:.3})", value.x, value.y),
        AnimValue::Vec3(value) => format!("({:.3}, {:.3}, {:.3})", value.x, value.y, value.z),
        AnimValue::Quat(value) => format!(
            "({:.3}, {:.3}, {:.3}, {:.3})",
            value.x, value.y, value.z, value.w
        ),
        AnimValue::Scalar(value) => format!("{value:.3}"),
        AnimValue::Region(index) => format!("帧 {index}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::{ProjectInfo, SceneDocument, SelectionSource};
    use rustlua_infra::logging::LogSink;
    use std::path::PathBuf;

    const CLIP: &str = "\
version=1
name=hero_run
duration=1
looped=true
track=Transform.position;interp=linear;keys=0:0,0;1:10,20
track=Light.intensity;interp=step;keys=0:1;0.5:2
event=0:start
event=0.5:step
";

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-timeline-{label}-{}-{}",
            std::process::id(),
            crate::util::test_token()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    /// 一个只有内存文档的编辑器：clip 写进临时工程的 `assets/anim/`，场景里放一个实体。
    fn app_with_entity(label: &str, component: Option<&str>, clip: Option<&str>) -> EditorApp {
        let root = temp_root(label);
        if let Some(text) = clip {
            let dir = root.join("assets/anim");
            std::fs::create_dir_all(&dir).expect("asset dir");
            std::fs::write(dir.join("hero.anim"), text).expect("write clip");
        }
        let mut app = EditorApp::new(
            ProjectInfo {
                root,
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(16),
        );
        let mut document = SceneDocument::new("timeline-test");
        document.dirty = false;
        document
            .scene
            .add_entity(SceneEntityId(1), "hero", None)
            .expect("entity");
        if let Some(payload) = component {
            document
                .scene
                .entities
                .get_mut(&SceneEntityId(1))
                .expect("entity")
                .components
                .insert(COMPONENT.to_owned(), payload.to_owned());
        }
        app.document.current_scene = Some(document);
        app.selection
            .set_single(SceneEntityId(1), SelectionSource::Command);
        app
    }

    fn ready(app: &EditorApp) -> &TimelinePreview {
        match &app.timeline.status {
            TimelineStatus::Ready(preview) => preview,
            other => panic!("期望可预览状态，实际 {other:?}"),
        }
    }

    fn ready_mut(app: &mut EditorApp) -> &mut TimelinePreview {
        app.timeline.status.as_ready_mut_for_test()
    }

    fn key_times(preview: &TimelinePreview, track: usize) -> Vec<f32> {
        preview.session.clip().tracks[track]
            .keyframes
            .iter()
            .map(|keyframe| keyframe.time)
            .collect()
    }

    /// 选中实体后：片段名/轨道数/事件与当前采样值都来自它的 `AnimationPlayer`。
    #[test]
    fn selected_entity_lists_its_tracks_and_samples() {
        let mut app = app_with_entity("tracks", Some("clip=assets/anim/hero.anim"), Some(CLIP));
        app.sync_timeline_from_selection();
        let preview = ready(&app);
        assert_eq!(preview.entity, SceneEntityId(1));
        assert_eq!(preview.entity_name, "hero");
        assert_eq!(preview.session.clip().name, "hero_run");
        assert_eq!(preview.player.duration(), 1.0);
        assert!(preview.player.effective_looped());
        assert_eq!(preview.session.clip().tracks.len(), 2);
        assert_eq!(preview.selected_track, Some(0));
        assert_eq!(
            preview.player.sample_all()[0].1,
            AnimValue::Vec2(rustlua_math::math2d::Vec2::new(0.0, 0.0))
        );

        // 播放半秒：时间推进、采样到中点、0 时刻事件进最近触发队列。
        app.timeline
            .status
            .as_ready_mut_for_test()
            .player
            .set_playing(true);
        app.timeline.advance(0.5);
        let preview = ready(&app);
        assert!((preview.player.time() - 0.5).abs() < 1e-6);
        assert_eq!(
            preview.player.sample_all()[0].1,
            AnimValue::Vec2(rustlua_math::math2d::Vec2::new(5.0, 10.0))
        );
        assert_eq!(
            preview.recent_events.len(),
            2,
            "{:?}",
            preview.recent_events
        );
    }

    /// 预览播放/拖动/重载都不许写文档：场景文本逐字节一致、dirty 不变。
    #[test]
    fn preview_never_touches_the_document() {
        let mut app = app_with_entity("readonly", Some("clip=assets/anim/hero.anim"), Some(CLIP));
        app.sync_timeline_from_selection();
        let before = {
            let document = app.current_scene().expect("scene");
            (document.scene.to_text(), document.dirty, document.revision)
        };

        {
            let preview = app.timeline.status.as_ready_mut_for_test();
            preview.player.set_playing(true);
        }
        for _ in 0..10 {
            app.timeline.advance(0.05);
        }
        {
            let preview = app.timeline.status.as_ready_mut_for_test();
            preview.player.seek(0.75).expect("seek");
            preview.player.set_playing(false);
        }
        // 真的画一帧面板 body（含滑杆/时间条/轨道/事件），确保 UI 路径同样只读。
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| app.draw_timeline_panel(ui));
        });

        let after = {
            let document = app.current_scene().expect("scene");
            (document.scene.to_text(), document.dirty, document.revision)
        };
        assert_eq!(before, after, "时间线预览/编辑不得改动场景文档");
    }

    /// 增删关键帧会立刻进预览（播放器重挂片段），撤销恢复原字节，保存把规范文本写回磁盘。
    #[test]
    fn editing_keyframes_updates_preview_undo_and_save() {
        let mut app = app_with_entity("edit", Some("clip=assets/anim/hero.anim"), Some(CLIP));
        app.sync_timeline_from_selection();
        let root = app.project.root.clone();
        let preview = ready_mut(&mut app);
        preview.player.seek(0.25).expect("seek");
        preview.add_key_at_playhead();
        assert_eq!(key_times(preview, 0), vec![0.0, 0.25, 1.0]);
        assert_eq!(preview.selected_key, Some(1));
        assert!(preview.session.is_dirty());
        assert_eq!(
            preview.player.clip().expect("player clip").tracks[0]
                .keyframes
                .len(),
            3,
            "预览必须用上编辑后的片段"
        );
        let edited = preview.session.text();
        AnimClip::from_text("assets/anim/hero.anim", &edited).expect("编辑后仍是合法资产");

        assert!(preview.save_to_disk(), "保存应成功：{:?}", preview.message);
        assert!(!preview.session.is_dirty());
        let on_disk =
            std::fs::read_to_string(root.join("assets/anim/hero.anim")).expect("read saved clip");
        assert_eq!(on_disk, edited);
        assert!(!root.join("assets/anim/hero.anim.tmp").exists());

        preview.undo_edit();
        assert_eq!(key_times(preview, 0), vec![0.0, 1.0]);
        assert_eq!(preview.session.text(), CLIP);
        assert!(preview.session.is_dirty(), "撤销越过保存点后应变脏");
        preview.redo_edit();
        assert_eq!(preview.session.text(), edited);
        assert!(!preview.session.is_dirty(), "重做回保存点即干净");
    }

    /// 播放头吸附到帧网格；重复时间被拒绝且不改片段。
    #[test]
    fn snap_rounds_the_playhead_and_rejects_duplicate_times() {
        let mut app = app_with_entity("snap", Some("clip=assets/anim/hero.anim"), Some(CLIP));
        app.sync_timeline_from_selection();
        let preview = ready_mut(&mut app);
        preview.snap = true;
        preview.fps = 30.0;
        preview.player.seek(0.31).expect("seek");
        preview.add_key_at_playhead();
        let times = key_times(preview, 0);
        assert_eq!(times.len(), 3);
        assert!(
            (times[1] - 0.3).abs() < 1e-6,
            "0.31s @30fps 应吸附到 0.3s：{times:?}"
        );

        let before = preview.session.text();
        preview.add_key_at_playhead();
        assert_eq!(preview.session.text(), before, "重复关键帧不得提交");
        assert!(
            preview
                .message
                .as_ref()
                .is_some_and(|(ok, message)| !ok && message.contains("already exists")),
            "{:?}",
            preview.message
        );
    }

    /// 增删轨道与事件走同一会话；删除轨道可撤销。
    #[test]
    fn track_and_event_editing_is_undoable() {
        let mut app = app_with_entity(
            "tracks-edit",
            Some("clip=assets/anim/hero.anim"),
            Some(CLIP),
        );
        app.sync_timeline_from_selection();
        let preview = ready_mut(&mut app);
        preview.new_track = "UiAnimation.opacity".to_owned();
        preview.player.seek(0.5).expect("seek");
        preview.add_track_from_input();
        assert_eq!(preview.session.clip().tracks.len(), 3);
        assert_eq!(preview.selected_track, Some(2));
        assert!(preview.session.text().contains("UiAnimation.opacity"));
        preview.remove_selected_track();
        assert_eq!(preview.session.clip().tracks.len(), 2);
        preview.undo_edit();
        assert_eq!(preview.session.clip().tracks.len(), 3);

        preview.new_event = "land".to_owned();
        preview.player.seek(0.75).expect("seek");
        preview.add_event_from_input();
        assert_eq!(preview.session.clip().events.len(), 3);
        assert_eq!(preview.session.clip().events[2].name, "land");
        assert!(preview.new_event.is_empty(), "成功后清空输入框");
        assert!(preview.session.text().contains("event=0.75:land"));
    }

    /// 有未保存修改时切换选中：保留原片段并提示；还原后才切到新目标。
    #[test]
    fn dirty_session_blocks_selection_switch_until_reverted() {
        let mut app = app_with_entity(
            "dirty-switch",
            Some("clip=assets/anim/hero.anim"),
            Some(CLIP),
        );
        app.sync_timeline_from_selection();
        {
            let preview = ready_mut(&mut app);
            preview.player.seek(0.25).expect("seek");
            preview.add_key_at_playhead();
            assert!(preview.session.is_dirty());
        }
        let scene = app.document.current_scene.as_mut().expect("scene");
        scene
            .scene
            .add_entity(SceneEntityId(2), "other", None)
            .expect("second entity");
        app.selection
            .set_single(SceneEntityId(2), SelectionSource::Command);
        app.sync_timeline_from_selection();
        {
            let preview = ready(&app);
            assert_eq!(preview.entity, SceneEntityId(1), "脏会话不得被静默丢弃");
            assert!(preview.switch_blocked);
        }
        ready_mut(&mut app).session.revert_to_saved();
        app.sync_timeline_from_selection();
        match &app.timeline.status {
            TimelineStatus::NoPlayer { name } => assert_eq!(name, "other"),
            other => panic!("还原后应切到新选中实体，实际 {other:?}"),
        }
    }

    /// 三种失败各有具体原因：组件配置坏、文件不在、片段解析失败。
    #[test]
    fn broken_or_missing_clips_report_what_is_wrong() {
        let mut invalid = app_with_entity("invalid-component", Some("clip=hero.txt"), Some(CLIP));
        invalid.sync_timeline_from_selection();
        match &invalid.timeline.status {
            TimelineStatus::Error(error) => assert!(error.contains("配置无效"), "{error}"),
            other => panic!("期望配置错误，实际 {other:?}"),
        }

        let mut missing = app_with_entity("missing-file", Some("clip=assets/anim/hero.anim"), None);
        missing.sync_timeline_from_selection();
        match &missing.timeline.status {
            TimelineStatus::Error(error) => assert!(error.contains("读不到"), "{error}"),
            other => panic!("期望缺文件错误，实际 {other:?}"),
        }

        let mut broken = app_with_entity(
            "broken-clip",
            Some("clip=assets/anim/hero.anim"),
            Some("version=1\nname=bad\n"),
        );
        broken.sync_timeline_from_selection();
        match &broken.timeline.status {
            TimelineStatus::Error(error) => assert!(error.contains("解析"), "{error}"),
            other => panic!("期望解析错误，实际 {other:?}"),
        }
    }

    /// 选中但没有组件 / 没有选中：面板给出可执行的提示，而不是空白。
    #[test]
    fn absent_selection_or_component_is_explained() {
        let mut app = app_with_entity("no-component", None, Some(CLIP));
        app.sync_timeline_from_selection();
        match &app.timeline.status {
            TimelineStatus::NoPlayer { name } => assert_eq!(name, "hero"),
            other => panic!("期望无组件状态，实际 {other:?}"),
        }

        app.selection.clear(SelectionSource::Command);
        app.sync_timeline_from_selection();
        assert!(matches!(app.timeline.status, TimelineStatus::NoSelection));
    }

    /// 重命名只刷新显示名，不重新读盘（键不含名称）。
    #[test]
    fn renaming_the_entity_does_not_reload_the_clip() {
        let mut app = app_with_entity("rename", Some("clip=assets/anim/hero.anim"), Some(CLIP));
        app.sync_timeline_from_selection();
        app.document
            .current_scene
            .as_mut()
            .expect("scene")
            .scene
            .entities
            .get_mut(&SceneEntityId(1))
            .expect("entity")
            .name = "hero_renamed".to_owned();
        app.sync_timeline_from_selection();
        let preview = ready(&app);
        assert_eq!(preview.entity_name, "hero_renamed");
    }

    /// 测试专用的小口子：拿可变的 Ready 预览（生产代码只经 `draw` 使用）。
    impl TimelineStatus {
        fn as_ready_mut_for_test(&mut self) -> &mut TimelinePreview {
            match self {
                Self::Ready(preview) => preview,
                other => panic!("期望可预览状态，实际 {other:?}"),
            }
        }
    }
}
