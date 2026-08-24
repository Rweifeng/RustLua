//! 行3 Game 视图 + Play/Stop 运行生命周期：内嵌游戏预览。

use crate::EditorApp;
use crate::WorkspaceTab;
use crate::preview_session::PreviewSession;
use crate::util::strip_verbatim_prefix;
use editor::PlaySession;
use eframe::egui;
use engine_core::{
    input::{InputKey, InputSnapshot, MouseButton},
    scene::Scene,
};

impl EditorApp {
    /// 进入 Play：创建内嵌游戏预览（运行 Lua + 物理 + 渲染），并把编辑器场景隔离快照。
    pub(crate) fn play(&mut self) {
        if self.is_playing() {
            self.status = "Not currently in Play mode".to_owned();
            return;
        }
        let Some(document) = self.current_scene().cloned() else {
            self.status = "Please create or open a scene first".to_owned();
            return;
        };
        // M10-03：创建内嵌游戏预览（Play 真正运行 Lua + 物理 + 渲染）。
        match self.create_preview(&document.scene) {
            Ok(preview) => {
                let (width, height) = preview.size();
                tracing::info!(target: "editor", "[preview] created {}x{}", width, height);
                self.preview = Some(preview);
            }
            Err(error) => {
                tracing::error!(target: "editor", "[preview] create failed: {error}");
                self.status = format!("游戏预览启动失败：{error}");
            }
        }
        self.play_session = Some(PlaySession::start(&document));
        // 运行预览时自动切到 Game 视图。
        self.workspace_tab = WorkspaceTab::Game;
        self.status = "Play mode started; runtime changes stay isolated".to_owned();
    }

    /// 停止 Play：丢弃运行态变更，恢复编辑器快照。
    pub(crate) fn stop(&mut self) {
        let Some(session) = self.play_session.take() else {
            self.status = "Not currently in Play mode".to_owned();
            return;
        };
        self.preview = None;
        self.document.current_scene = Some(session.stop());
        self.tree.clear_selection();
        self.inspector.clear_selection();
        // 停止后回到 Scene 视图。
        self.workspace_tab = WorkspaceTab::Scene;
        self.status = "Play stopped; runtime changes were discarded".to_owned();
    }

    pub(crate) fn draw_game_view(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::hover());
        painter.rect_filled(response.rect, 0.0, egui::Color32::from_rgb(18, 20, 24));
        if self.preview.is_some() {
            let focused = ui.rect_contains_pointer(response.rect);
            let preview_rect = self.preview_rect(response.rect);
            self.update_preview(ui.ctx(), focused, preview_rect);
            if let Some(preview) = &self.preview
                && let Some(tex) = preview.texture()
            {
                painter.image(
                    tex.id(),
                    preview_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                let text = if self.is_playing() {
                    "GAME VIEW\n(preview unavailable)"
                } else {
                    "GAME VIEW\nPress Play to run the scene"
                };
                painter.text(
                    response.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(20.0),
                    egui::Color32::LIGHT_GRAY,
                );
            }
        } else {
            let text = if self.is_playing() {
                "GAME VIEW\n(preview unavailable)"
            } else {
                "GAME VIEW\nPress Play to run the scene"
            };
            painter.text(
                response.rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(20.0),
                egui::Color32::LIGHT_GRAY,
            );
        }
        if let Some(message) = self.preview.as_ref().and_then(PreviewSession::diagnostic) {
            let metrics = self.preview.as_ref().map(PreviewSession::readback_metrics);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    response.rect.left_top() + egui::vec2(8.0, 8.0),
                    egui::vec2(response.rect.width() - 16.0, 46.0),
                ),
                4.0,
                egui::Color32::from_rgba_unmultiplied(100, 30, 30, 220),
            );
            painter.text(
                response.rect.left_top() + egui::vec2(16.0, 14.0),
                egui::Align2::LEFT_TOP,
                message,
                egui::FontId::proportional(13.0),
                egui::Color32::LIGHT_RED,
            );
            if let Some(metrics) = metrics {
                let caption = format!(
                    "readback: {} slots / {} busy / {} dropped / {} rebuilds / {:.1} KiB cpu",
                    metrics.slot_count,
                    metrics.busy_count,
                    metrics.dropped,
                    metrics.buffer_rebuilds,
                    metrics.cpu_bytes as f32 / 1024.0,
                );
                painter.text(
                    response.rect.left_top() + egui::vec2(16.0, 30.0),
                    egui::Align2::LEFT_TOP,
                    caption,
                    egui::FontId::monospace(11.0),
                    egui::Color32::from_rgb(230, 180, 180),
                );
            }
        }
    }

    /// 创建内嵌游戏预览会话：运行时 + 离屏渲染器 + 预览纹理（委托 PreviewSession）。
    fn create_preview(&self, scene: &Scene) -> Result<PreviewSession, String> {
        let Some((device, queue)) = &self.gpu else {
            return Err("编辑器缺少 wgpu 上下文，无法内嵌游戏预览".to_owned());
        };
        // Windows 逐字前缀 `\\?\` 会破坏 Lua require 的 searchpath 解析，去除后再传运行时。
        let root = strip_verbatim_prefix(&self.project.root);
        PreviewSession::create(device, queue, &root, scene, self.preview_resolution)
    }

    /// 驱动预览会话：分辨率选择在 Play 期间变更时实时重建离屏纹理，
    /// 并把 egui 输入桥接为游戏输入后交给会话步进（节流/回读/上传在会话内部）。
    fn update_preview(&mut self, ctx: &egui::Context, focused: bool, preview_rect: egui::Rect) {
        // 分辨率实时变更：会话尺寸与工具栏选择不一致时原子重建离屏纹理。
        if self
            .preview
            .as_ref()
            .is_some_and(|preview| preview.size() != self.preview_resolution)
            && let Some((device, _)) = &self.gpu
        {
            let (width, height) = self.preview_resolution;
            if let Some(preview) = &mut self.preview {
                preview.resize(device, width, height);
            }
        }
        // 把 egui 的键盘/鼠标/滚轮输入桥接到游戏快照（仅在指针悬停 Game 视图时，
        // 避免编辑器输入泄漏到游戏）。
        let input = if focused {
            let (width, height) = self
                .preview
                .as_ref()
                .map(PreviewSession::size)
                .unwrap_or((640, 360));
            self.preview_input(ctx, preview_rect, width, height)
        } else {
            InputSnapshot::default()
        };
        if let Some(preview) = &mut self.preview {
            preview.update(ctx, &input);
        }
    }

    /// 从 egui 输入构建游戏输入快照（键盘 WASD/空格/Esc、鼠标、滚轮）。
    /// 预览在 Game 视图内纵横比居中后的实际绘制矩形。
    fn preview_rect(&self, rect: egui::Rect) -> egui::Rect {
        let Some(preview) = &self.preview else {
            return rect;
        };
        let (width, height) = preview.size();
        let aspect = width as f32 / height.max(1) as f32;
        let mut rect = rect;
        let avail = rect.width() / rect.height().max(1.0);
        if avail > aspect {
            let width = rect.height() * aspect;
            let extra = (rect.width() - width) / 2.0;
            rect.min.x += extra;
            rect.max.x -= extra;
        } else {
            let height = rect.width() / aspect;
            let extra = (rect.height() - height) / 2.0;
            rect.min.y += extra;
            rect.max.y -= extra;
        }
        rect
    }

    fn preview_input(
        &self,
        ctx: &egui::Context,
        preview_rect: egui::Rect,
        width: u32,
        height: u32,
    ) -> InputSnapshot {
        let mut input = InputSnapshot::default();
        ctx.input(|i| {
            for (egui_key, game_key) in [
                (egui::Key::W, InputKey::W),
                (egui::Key::A, InputKey::A),
                (egui::Key::S, InputKey::S),
                (egui::Key::D, InputKey::D),
                (egui::Key::Space, InputKey::Space),
                (egui::Key::Escape, InputKey::Escape),
            ] {
                input.key_event(game_key, i.key_down(egui_key));
            }
            input.mouse_event(MouseButton::Left, i.pointer.primary_down());
            input.mouse_event(MouseButton::Right, i.pointer.secondary_down());
            input.mouse_event(MouseButton::Middle, i.pointer.middle_down());
            if let Some(pos) = i.pointer.hover_pos() {
                // 把 egui 屏幕坐标映射到预览像素坐标，供 Button 命中测试使用。
                let px = (pos.x - preview_rect.min.x) / preview_rect.width().max(1.0)
                    * width.max(1) as f32;
                let py = (pos.y - preview_rect.min.y) / preview_rect.height().max(1.0)
                    * height.max(1) as f32;
                input.cursor_moved(px as f64, py as f64);
            }
            let scroll = i.smooth_scroll_delta;
            if scroll.x != 0.0 || scroll.y != 0.0 {
                input.scroll_event(scroll.x, scroll.y);
            }
        });
        input
    }
}
