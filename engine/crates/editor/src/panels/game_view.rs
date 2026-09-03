//! 行3 Game 视图 + Play/Stop 运行生命周期：内嵌游戏预览。

use crate::EditorApp;
use crate::PlayState;
use crate::WorkspaceTab;
use crate::preview_session::PreviewSession;
use crate::util::strip_verbatim_prefix;
use editor::PlaySession;
use eframe::egui;
use engine::core::{
    input::{InputKey, InputSnapshot, MouseButton},
    scene::Scene,
};
use engine::host::load_project_font_atlas;

impl EditorApp {
    /// 进入 Play：创建内嵌游戏预览（运行 Lua + 物理 + 渲染），并把编辑器场景隔离快照。
    /// M11-01 状态机：`Editing → Starting → Playing`；初始化失败进入 `StartFailed`，
    /// 绝不设置 `play_session`（R-01，编辑器状态与实际运行状态保持一致）。
    pub(crate) fn play(&mut self) {
        if self.is_playing() {
            return;
        }
        let Some(document) = self.current_scene().cloned() else {
            self.status = "Please create or open a scene first".to_owned();
            return;
        };
        self.play_state = PlayState::Starting;
        // 确保共享字体图集已预热：Play 预览复用它，避免进入游戏时再 MSDF 预热
        // 全部字形（debug 下约 2s/项目），Play 立即响应。
        if self.scene_font_atlas.is_none() {
            let root = strip_verbatim_prefix(&self.project.root);
            self.scene_font_atlas = load_project_font_atlas(Some(&root));
        }
        match self.create_preview(&document.scene) {
            Ok(preview) => {
                let (width, height) = preview.size();
                tracing::info!(target: "editor", "[preview] created {}x{}", width, height);
                self.preview = Some(preview);
                self.play_session = Some(PlaySession::start(&document));
                self.play_state = PlayState::Playing;
                // 运行预览时自动切到 Game 视图。
                self.workspace_tab = WorkspaceTab::Game;
                self.status = "Play mode started; runtime changes stay isolated".to_owned();
            }
            Err(error) => {
                tracing::error!(target: "editor", "[preview] create failed: {error}");
                // 初始化失败不得进入 Playing：不设置 play_session / preview，并清空输入。
                self.preview = None;
                self.play_session = None;
                self.preview_input_state.clear_all();
                self.play_state = PlayState::StartFailed;
                self.status = format!("游戏预览启动失败：{error}");
            }
        }
    }

    /// 停止 Play：丢弃运行态变更，恢复编辑器快照。
    /// M11-01 状态机：`Playing → Stopping → Editing`，Stop 期间释放临时世界/GPU 纹理/Lua 状态。
    pub(crate) fn stop(&mut self) {
        let Some(session) = self.play_session.take() else {
            self.status = "Not currently in Play mode".to_owned();
            return;
        };
        self.play_state = PlayState::Stopping;
        self.preview = None;
        self.set_current_scene_document(session.stop());
        // Play/Stop 清空输入状态，避免下一轮 Play 残留边沿（M11-02）。
        self.preview_input_state.clear_all();
        // 停止后回到 Scene 视图。
        self.workspace_tab = WorkspaceTab::Scene;
        // 统一收尾：选择修复 + Inspector 空同步 + Scene View/Game 预览失效 +
        // Hierarchy 元数据重扫（含 delete_target 复位）。
        self.on_document_changed(crate::actions::DocumentChange::PlaybackStopped);
        self.play_state = PlayState::Editing;
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
                && let Some(tex) = preview.texture_id()
            {
                painter.image(
                    tex,
                    preview_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                // 右上角徽标：设计分辨率 + 当前显示缩放比（Unity Game 视图风格）。
                // 面板小于原生分辨率时按比例等比缩小适配，徽标实时反映该缩放比。
                let (width, height) = preview.size();
                let ppp = ui.ctx().pixels_per_point();
                let fit = preview_fit_scale(preview_rect, ppp, width);
                let label = format!("{width}×{height} · {:.0}%", fit * 100.0);
                let galley = painter.layout_no_wrap(
                    label.clone(),
                    egui::FontId::monospace(11.0),
                    egui::Color32::from_rgb(215, 230, 245),
                );
                let pad = egui::vec2(8.0, 4.0);
                let badge = egui::Rect::from_min_max(
                    response.rect.left_top() + egui::vec2(6.0, 6.0),
                    response.rect.left_top()
                        + egui::vec2(
                            6.0 + galley.size().x + pad.x * 2.0,
                            6.0 + galley.size().y + pad.y * 2.0,
                        ),
                );
                painter.rect_filled(
                    badge,
                    4.0,
                    egui::Color32::from_rgba_unmultiplied(20, 28, 40, 210),
                );
                painter.rect_stroke(
                    badge,
                    4.0,
                    egui::Stroke::new(
                        1.0_f32,
                        egui::Color32::from_rgba_unmultiplied(130, 170, 220, 140),
                    ),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    badge.left_center() + egui::vec2(pad.x, 0.0),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::monospace(11.0),
                    egui::Color32::from_rgb(215, 230, 245),
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
        PreviewSession::create(
            device,
            queue,
            self.render_state.clone(),
            &root,
            scene,
            self.preview_resolution,
            self.scene_font_atlas.clone(),
        )
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
                // UI 参考分辨率（设计分辨率）随分辨率变更同步更新，保证 Canvas 适配一致。
                preview.set_ui_reference([width as f32, height as f32]);
            }
        }
        if focused {
            let (width, height) = self
                .preview
                .as_ref()
                .map(PreviewSession::size)
                .unwrap_or((640, 360));
            // 复用跨帧输入状态（M11-02）：held 按键/鼠标跨帧保留，`just_pressed`/
            // `just_released` 边沿只在本次模拟帧的 update 消费一次，随后在帧边界清空，
            // 避免按住的键在下一个 tick 重复触发一次性动作。
            let mut input = std::mem::take(&mut self.preview_input_state);
            self.bridge_preview_input(&mut input, ctx, preview_rect, width, height);
            if let Some(preview) = &mut self.preview {
                preview.update(ctx, &input);
            }
            // 在模拟帧边界消费 transient edge：清边沿但保留 held 状态。
            input.finish_frame();
            self.preview_input_state = input;
        } else {
            // 失焦：清空 held 防止卡键（M11-02）。
            self.preview_input_state.clear_all();
            if let Some(preview) = &mut self.preview {
                preview.update(ctx, &InputSnapshot::default());
            }
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
    /// 跨帧 held 状态；M11-02）。预览在 Game 视图内纵横比居中后的实际绘制矩形。
    fn bridge_preview_input(
        &self,
        input: &mut InputSnapshot,
        ctx: &egui::Context,
        preview_rect: egui::Rect,
        width: u32,
        height: u32,
    ) {
        ctx.input(|i| {
            for (egui_key, game_key) in [
                (egui::Key::W, InputKey::W),
                (egui::Key::A, InputKey::A),
                (egui::Key::S, InputKey::S),
                (egui::Key::D, InputKey::D),
                (egui::Key::Space, InputKey::Space),
                (egui::Key::Escape, InputKey::Escape),
                // 农场玩法键：按 `InputKey::from_char` 统一到小写 ASCII 码，
                // 与独立游戏窗口和 `parse_key` 一致（M16 输入修复）。
                (egui::Key::Q, InputKey::from_char('q')),
                (egui::Key::E, InputKey::from_char('e')),
                (egui::Key::R, InputKey::from_char('r')),
                (egui::Key::T, InputKey::from_char('t')),
                (egui::Key::F, InputKey::from_char('f')),
                (egui::Key::G, InputKey::from_char('g')),
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
    }
}

/// Game 视图显示缩放比：预览纹理像素宽以物理像素显示时的放大/缩小比例
/// （面板逻辑宽 × pixels_per_point ÷ 原生宽）。面板小于原生分辨率时 < 1，
/// 表示按比例等比缩小适配（Unity Game 视图的 Scale 指示）。
fn preview_fit_scale(preview_rect: egui::Rect, ppp: f32, native_w: u32) -> f32 {
    if native_w == 0 {
        return 1.0;
    }
    (preview_rect.width() * ppp) / native_w as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlayState;
    use crate::WorkspaceTab;
    use editor::ProjectInfo;
    use editor::open_project;
    use eframe::egui::{self, Event, Key, Modifiers, MouseWheelUnit, PointerButton, RawInput};
    use engine::core::input::{InputKey, MouseButton};
    use engine::core::logging::LogSink;
    use engine::core::scene::SceneEntityId;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 无 GPU 上下文 + 含一个实体的编辑器应用（用于失败路径测试）。
    fn app_with_scene() -> EditorApp {
        let root = PathBuf::from(std::env::var("TEMP").unwrap_or_else(|_| ".".into()))
            .join(format!("p2-m11-01-{}", std::process::id()));
        let project = ProjectInfo {
            root,
            version: "test".into(),
            directories: Vec::new(),
        };
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.document.new_scene("main");
        app.document
            .current_scene
            .as_mut()
            .unwrap()
            .scene
            .add_entity(SceneEntityId(1), "Root", None)
            .unwrap();
        app
    }

    fn unique_token() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos()
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rustlua-m14-01-game-view-{label}-{}",
            unique_token()
        ))
    }

    fn sample_project_root() -> PathBuf {
        engine::core::games_dir().join("game_test_2d_production")
    }

    fn copy_dir(source: &Path, destination: &Path) {
        if !source.is_dir() {
            return;
        }
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let source_path = entry.path();
            let target_path = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                fs::create_dir_all(&target_path).unwrap();
                copy_dir(&source_path, &target_path);
            } else {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::copy(&source_path, &target_path).unwrap();
            }
        }
    }

    fn copy_sample_project(label: &str) -> PathBuf {
        let source = sample_project_root();
        assert!(
            source.join("project.toml").is_file(),
            "sample project missing: {}",
            source.display()
        );
        let root = temp_root(label);
        fs::create_dir_all(&root).unwrap();
        fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
        copy_dir(&source.join("scripts"), &root.join("scripts"));
        copy_dir(&source.join("scenes"), &root.join("scenes"));
        copy_dir(&source.join("assets"), &root.join("assets"));
        root
    }

    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("p2-game-view-test"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()
    }

    /// R-01：无 GPU 时 Play 初始化失败，绝不进入 Playing，也不设置 play_session。
    #[test]
    fn play_failed_without_gpu_never_enters_playing() {
        let mut app = app_with_scene();
        app.play();
        assert_eq!(app.play_state, PlayState::StartFailed);
        assert!(
            app.play_session.is_none(),
            "初始化失败不得持有 play_session"
        );
        assert!(!app.is_playing(), "StartFailed 不是运行态");
        assert!(app.preview.is_none());
        assert!(app.status.contains("游戏预览启动失败"), "需给用户可读错误");
    }

    /// M11-01：is_playing 反映状态机而非仅 play_session 是否被设置。
    #[test]
    fn is_playing_reflects_state_machine() {
        let mut app = app_with_scene();
        assert_eq!(app.play_state, PlayState::Editing);
        assert!(!app.is_playing());
        app.play_state = PlayState::Starting;
        assert!(app.is_playing());
        app.play_state = PlayState::Playing;
        assert!(app.is_playing());
        app.play_state = PlayState::StartFailed;
        assert!(!app.is_playing(), "StartFailed 必须保持编辑态");
        app.play_state = PlayState::Stopping;
        assert!(app.is_playing());
        app.play_state = PlayState::Editing;
        assert!(!app.is_playing());
    }

    /// M11-01：未进入 Play 时 Stop 是安全 no-op，状态保持 Editing。
    #[test]
    fn stop_without_play_is_noop() {
        let mut app = app_with_scene();
        app.stop();
        assert_eq!(app.play_state, PlayState::Editing);
        assert!(app.play_session.is_none());
    }

    /// M11-02：跨帧复用同一 InputSnapshot 的 held 状态 + 帧边界 finish_frame 后，
    /// 按住的键只在按下那一帧触发 just_pressed，后续 tick 不重复触发一次性动作。
    #[test]
    fn held_key_reuse_does_not_repeat_just_pressed_across_ticks() {
        let mut state = InputSnapshot::default();
        // 第 1 tick：按下 W。
        state.key_event(InputKey::W, true);
        assert!(state.just_pressed(InputKey::W));
        state.finish_frame();
        // 第 2 tick：继续按住 W（模拟 bridge_preview_input 再次写入 true）。
        state.key_event(InputKey::W, true);
        assert!(
            !state.just_pressed(InputKey::W),
            "按住期间不得重复触发一次性动作"
        );
        assert!(state.is_down(InputKey::W));
        state.finish_frame();
        // 松开 W：产生 just_released 且不再 held。
        state.key_event(InputKey::W, false);
        assert!(state.just_released(InputKey::W));
        state.finish_frame();
        assert!(!state.is_down(InputKey::W));
    }

    /// M11-02：失焦清空 held 状态防止卡键。
    #[test]
    fn focus_loss_clears_held_state_prevents_stuck_input() {
        let mut state = InputSnapshot::default();
        state.key_event(InputKey::W, true);
        state.mouse_event(MouseButton::Left, true);
        state.clear_all();
        assert!(!state.is_down(InputKey::W));
        assert!(!state.mouse_is_down(MouseButton::Left));
    }

    #[test]
    fn sample_project_stop_restores_scene_tab_and_clears_preview_input() {
        let root = copy_sample_project("stop");
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));
        let original = app.document.current_scene.as_ref().unwrap().clone();

        app.play_session = Some(PlaySession::start(&original));
        app.play_state = PlayState::Playing;
        app.workspace_tab = WorkspaceTab::Game;
        app.selection
            .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);
        app.preview_input_state.key_event(InputKey::W, true);
        app.preview_input_state.mouse_event(MouseButton::Left, true);
        app.play_session
            .as_mut()
            .unwrap()
            .runtime_document_mut()
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .name = "RuntimePlayer".to_owned();

        app.stop();

        assert_eq!(app.play_state, PlayState::Editing);
        assert_eq!(app.workspace_tab, WorkspaceTab::Scene);
        assert!(app.play_session.is_none());
        assert!(!app.preview_input_state.is_down(InputKey::W));
        assert!(!app.preview_input_state.mouse_is_down(MouseButton::Left));
        assert!(
            app.selection.is_empty(),
            "stop should clear scene selection"
        );
        assert_eq!(app.document.current_scene.as_ref().unwrap(), &original);
        assert_eq!(app.status, "Play stopped; runtime changes were discarded");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_play_starts_preview_and_switches_to_game_tab() {
        let Some((device, queue)) = test_device() else {
            return;
        };
        let root = copy_sample_project("play-start");
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, Some((device, queue)), LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));

        app.play();

        assert_eq!(app.play_state, PlayState::Playing);
        assert!(
            app.play_session.is_some(),
            "play should create play_session"
        );
        assert!(app.preview.is_some(), "play should create preview session");
        assert_eq!(app.workspace_tab, WorkspaceTab::Game);
        assert_eq!(
            app.status,
            "Play mode started; runtime changes stay isolated"
        );
        assert_eq!(
            app.preview.as_ref().map(PreviewSession::size),
            Some(app.preview_resolution)
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 显示缩放比 = 面板逻辑宽 × pixels_per_point ÷ 原生宽：
    /// 面板小于原生分辨率时 < 1（等比缩小适配），面板更大或高 DPI 时放大。
    #[test]
    fn preview_fit_scale_measures_display_scale() {
        // 原生 1280，显示宽 640 逻辑像素、ppp 1 → 等比缩小到一半。
        let preview_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 360.0));
        assert!((preview_fit_scale(preview_rect, 1.0, 1280) - 0.5).abs() < 1e-6);
        // 面板逻辑宽不变但 ppp 翻倍（150% DPI 物理像素更多）→ 物理显示 2 倍。
        assert!((preview_fit_scale(preview_rect, 2.0, 640) - 2.0).abs() < 1e-6);
        // 原生宽为 0 的退化输入：返回 1.0，避免除零。
        assert_eq!(preview_fit_scale(preview_rect, 1.0, 0), 1.0);
    }

    #[test]
    fn bridge_preview_input_maps_keys_cursor_and_scroll() {
        let app = app_with_scene();
        let ctx = egui::Context::default();
        let preview_rect =
            egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(210.0, 120.0));
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 240.0),
            )),
            events: vec![
                Event::Key {
                    key: Key::W,
                    physical_key: Some(Key::W),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::default(),
                },
                Event::PointerMoved(egui::pos2(60.0, 70.0)),
                Event::PointerButton {
                    pos: egui::pos2(60.0, 70.0),
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::default(),
                },
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, 24.0),
                    modifiers: Modifiers::default(),
                },
            ],
            ..Default::default()
        };
        ctx.begin_pass(raw);
        let mut input = InputSnapshot::default();
        app.bridge_preview_input(&mut input, &ctx, preview_rect, 200, 100);
        let _ = ctx.end_pass();

        assert!(input.is_down(InputKey::W));
        assert!(input.mouse_is_down(MouseButton::Left));
        assert_eq!(input.cursor_position(), (50.0, 50.0));
        assert_eq!(input.scroll_delta().0, 0.0);
        assert!(
            input.scroll_delta().1 > 0.0,
            "mouse wheel should produce positive smooth scroll delta"
        );
    }
}
