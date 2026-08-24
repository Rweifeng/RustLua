//! 行3 Game 视图 + Play/Stop 运行生命周期：内嵌游戏预览。

use crate::EditorApp;
use crate::EmbeddedPreview;
use crate::WorkspaceTab;
use crate::util::strip_verbatim_prefix;
use editor::PlaySession;
use eframe::egui;
use engine_core::{
    input::{InputKey, InputSnapshot, MouseButton},
    scene::Scene,
};
use engine_host::{Renderer, RuntimeScene};

/// 游戏预览的重绘节流：~30fps，保证持续运行而不忙转。
const PREVIEW_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

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
                tracing::info!(target: "editor", "[preview] created {}x{}", preview.width, preview.height);
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
                && let Some(tex) = &preview.egui_tex
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
        if let Some(message) = &self.preview_diagnostic {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    response.rect.left_top() + egui::vec2(8.0, 8.0),
                    egui::vec2(response.rect.width() - 16.0, 28.0),
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
        }
    }

    /// 创建内嵌游戏预览：运行时（当前场景）+ 离屏渲染器 + 预览纹理。
    fn create_preview(&self, scene: &Scene) -> Result<EmbeddedPreview, String> {
        let Some((device, queue)) = &self.gpu else {
            return Err("编辑器缺少 wgpu 上下文，无法内嵌游戏预览".to_owned());
        };
        // Windows 逐字前缀 `\\?\` 会破坏 Lua require 的 searchpath 解析，去除后再传运行时。
        let root = strip_verbatim_prefix(&self.project.root);
        tracing::info!(target: "editor", "[preview] project_root={}", root.display());
        let runtime = RuntimeScene::from_scene(Some(&root), scene.clone())?;
        let renderer =
            Renderer::new_offscreen(device.clone(), queue.clone(), Some(root), false, false)?;
        let (width, height) = self.preview_resolution;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p2-editor-preview"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(EmbeddedPreview {
            runtime,
            renderer,
            texture,
            texture_view,
            egui_tex: None,
            width,
            height,
        })
    }

    /// 步进内嵌游戏运行时，渲染到离屏纹理并回读更新 egui 预览纹理（M10-03）。
    /// 回读为非阻塞（滞后一帧）；按 ~30fps 节流步进，避免输入事件以 144Hz 加速游戏。
    fn update_preview(&mut self, ctx: &egui::Context, focused: bool, preview_rect: egui::Rect) {
        // 节流：未到期时不步进游戏，仅排空在途回读（补上滞后帧），保持游戏匀速。
        let now = std::time::Instant::now();
        let due = self
            .last_preview_tick
            .is_none_or(|last| now.duration_since(last) >= PREVIEW_FRAME_INTERVAL);
        if !due {
            let ready = self.preview.as_mut().and_then(|preview| {
                match preview.renderer.readback_pending() {
                    engine_host::ReadbackStatus::Ready {
                        pixels,
                        width,
                        height,
                    } => Some((pixels, width, height)),
                    _ => None,
                }
            });
            if let Some((pixels, width, height)) = ready {
                self.upload_preview_frame(ctx, pixels, width, height);
                self.preview_diagnostic = None;
            }
            ctx.request_repaint_after(PREVIEW_FRAME_INTERVAL);
            return;
        }
        self.last_preview_tick = Some(now);

        // 把 egui 的键盘/鼠标/滚轮输入桥接到游戏快照（仅在指针悬停 Game 视图时，
        // 避免编辑器输入泄漏到游戏）。先构建输入，避免与 preview 的可变借用冲突。
        let input = if focused {
            let (width, height) = self
                .preview
                .as_ref()
                .map(|preview| (preview.width, preview.height))
                .unwrap_or((640, 360));
            self.preview_input(ctx, preview_rect, width, height)
        } else {
            InputSnapshot::default()
        };
        let Some(preview) = &mut self.preview else {
            return;
        };
        if let Err(error) = preview.runtime.run_script_steps(1, 1.0 / 60.0, &input) {
            tracing::warn!(target: "editor", "[preview] script step failed: {error}");
        }
        let Some(mesh) = (match preview.runtime.vertices(preview.width, preview.height) {
            Ok(mesh) => Some(mesh),
            Err(error) => {
                tracing::error!(target: "editor", "[preview] build vertices failed: {error}");
                self.status = format!("游戏预览渲染失败：{error}");
                self.preview_diagnostic = Some(format!("渲染失败：{error}"));
                None
            }
        }) else {
            ctx.request_repaint_after(PREVIEW_FRAME_INTERVAL);
            return;
        };
        let aspect = preview.width as f32 / preview.height.max(1) as f32;
        let scene3d = preview.runtime.scene_3d(aspect);
        preview.renderer.render_into_texture(
            &mesh,
            scene3d.as_ref(),
            &preview.texture_view,
            preview.width,
            preview.height,
        );
        // 非阻塞回读：就绪则上传；未就绪沿用上一帧（滞后一帧，主线程不阻塞）。
        match preview
            .renderer
            .readback_frame(&preview.texture, preview.width, preview.height)
        {
            engine_host::ReadbackStatus::Ready {
                pixels,
                width,
                height,
            } => {
                self.upload_preview_frame(ctx, pixels, width, height);
                self.preview_diagnostic = None;
            }
            engine_host::ReadbackStatus::Pending => {
                self.preview_diagnostic = Some("等待 GPU 回读……显示上一帧".to_owned());
            }
            engine_host::ReadbackStatus::Failed(error) => {
                tracing::warn!(target: "editor", "[preview] readback failed: {error}");
                self.preview_diagnostic = Some(format!("GPU 回读失败：{error}"));
            }
        }
        // 节流到 ~30fps：无论回读是否就绪都排下一帧，保证游戏持续运行且不忙转。
        ctx.request_repaint_after(PREVIEW_FRAME_INTERVAL);
    }

    /// 把预览回读像素上传到 egui 纹理。
    fn upload_preview_frame(
        &mut self,
        ctx: &egui::Context,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    ) {
        let nonzero = pixels.iter().any(|&byte| byte > 0);
        if !nonzero {
            tracing::warn!(target: "editor", "[preview] readback frame is all-zero");
        }
        let image =
            egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &pixels);
        if let Some(preview) = &mut self.preview {
            match &mut preview.egui_tex {
                Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                None => {
                    preview.egui_tex =
                        Some(ctx.load_texture("game_preview", image, egui::TextureOptions::LINEAR));
                }
            }
        }
    }

    /// 从 egui 输入构建游戏输入快照（键盘 WASD/空格/Esc、鼠标、滚轮）。
    /// 预览在 Game 视图内纵横比居中后的实际绘制矩形。
    fn preview_rect(&self, rect: egui::Rect) -> egui::Rect {
        let Some(preview) = &self.preview else {
            return rect;
        };
        let aspect = preview.width as f32 / preview.height.max(1) as f32;
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
