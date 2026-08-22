//! 行3 Game 视图 + Play/Stop 运行生命周期：内嵌游戏预览。

use crate::EditorApp;
use crate::EmbeddedPreview;
use crate::util::strip_verbatim_prefix;
use editor::PlaySession;
use eframe::egui;
use engine_core::{
    input::{InputKey, InputSnapshot, MouseButton},
    scene::Scene,
};
use engine_host::{Renderer, RuntimeScene};

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
        self.status = "Play stopped; runtime changes were discarded".to_owned();
    }

    pub(crate) fn draw_game_view(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::hover());
        painter.rect_filled(response.rect, 0.0, egui::Color32::from_rgb(18, 20, 24));
        if self.preview.is_some() {
            let focused = ui.rect_contains_pointer(response.rect);
            self.update_preview(ui.ctx(), focused);
            if let Some(preview) = &self.preview
                && let Some(tex) = &preview.egui_tex
            {
                // 保持预览纵横比，居中绘制。
                let aspect = preview.width as f32 / preview.height.max(1) as f32;
                let mut rect = response.rect;
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
                painter.image(
                    tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
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
        let width = 640;
        let height = 360;
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
    fn update_preview(&mut self, ctx: &egui::Context, focused: bool) {
        // 把 egui 的键盘/鼠标/滚轮输入桥接到游戏快照（仅在指针悬停 Game 视图时，
        // 避免编辑器输入泄漏到游戏）。先构建输入，避免与 preview 的可变借用冲突。
        let input = if focused {
            self.preview_input(ctx)
        } else {
            InputSnapshot::default()
        };
        let Some(preview) = &mut self.preview else {
            return;
        };
        if let Err(error) = preview.runtime.run_script_steps(1, 1.0 / 60.0, &input) {
            tracing::warn!(target: "editor", "[preview] script step failed: {error}");
            return;
        }
        let Some(mesh) = preview.runtime.vertices(preview.width, preview.height).ok() else {
            tracing::warn!(target: "editor", "[preview] build vertices failed");
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
        let readback =
            preview
                .renderer
                .copy_texture_to_cpu(&preview.texture, preview.width, preview.height);
        match readback {
            Ok(pixels) => {
                let nonzero = pixels.iter().any(|&byte| byte > 0);
                if !nonzero {
                    tracing::warn!(target: "editor", "[preview] readback frame is all-zero");
                }
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [preview.width as usize, preview.height as usize],
                    &pixels,
                );
                match &mut preview.egui_tex {
                    Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                    None => {
                        preview.egui_tex = Some(ctx.load_texture(
                            "game_preview",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
            }
            Err(error) => {
                tracing::warn!(target: "editor", "[preview] readback failed: {error}");
            }
        }
    }

    /// 从 egui 输入构建游戏输入快照（键盘 WASD/空格/Esc、鼠标、滚轮）。
    fn preview_input(&self, ctx: &egui::Context) -> InputSnapshot {
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
                input.cursor_moved(pos.x as f64, pos.y as f64);
            }
            let scroll = i.smooth_scroll_delta;
            if scroll.x != 0.0 || scroll.y != 0.0 {
                input.scroll_event(scroll.x, scroll.y);
            }
        });
        input
    }
}
