//! 行3 Game 视图 + Play/Stop 运行生命周期：内嵌游戏预览。

use crate::EditorApp;
use crate::EditorContextSnapshot;
use crate::PlayState;
use crate::SceneTool;
use crate::SelectionModel;
use crate::WorkspaceTab;
use crate::preview_session::{PreviewSession, PreviewTick};
use crate::util::strip_verbatim_prefix;
use editor::PlaySession;
use eframe::egui;
use engine::core::{
    input::{InputKey, InputSnapshot, MouseButton},
    preview_diagnostics::PreviewErrorKind,
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
        // H02：进入 Play 前终止行内重命名会话，避免编辑会话跨运行上下文。
        self.hierarchy_rename = None;
        self.editor_context_snapshot = Some(self.capture_editor_context());
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
        // 统一收尾：选择修复 + Inspector 空同步 + Scene View/Game 预览失效 +
        // Hierarchy 元数据重扫（含 delete_target 复位）。
        self.on_document_changed(crate::actions::DocumentChange::PlaybackStopped);
        let snapshot =
            self.editor_context_snapshot
                .take()
                .unwrap_or_else(|| EditorContextSnapshot {
                    selection: SelectionModel::new(),
                    workspace_tab: WorkspaceTab::Scene,
                    scene_tool: SceneTool::Select,
                    scene_view_mode: self.scene_view_mode,
                    scene_zoom: 1.0,
                    scene_pan: egui::Vec2::ZERO,
                    hierarchy_collapsed: self.scene_state.collapsed.clone(),
                    preview_resolution: self.preview_resolution,
                    game_camera_2d_center: self.game_camera_2d_center,
                });
        self.restore_editor_context(snapshot);
        self.play_state = PlayState::Editing;
        self.status = "Play stopped; runtime changes were discarded".to_owned();
    }

    pub(crate) fn draw_game_view(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::hover());
        painter.rect_filled(response.rect, 0.0, egui::Color32::from_rgb(18, 20, 24));
        if self.preview.is_some() {
            // 鼠标事件要求指针在 Game 视图内；键盘只要求编辑器窗口持有焦点（M22 修复，
            // 见 `update_preview` 的文档）。
            let pointer_over = ui.rect_contains_pointer(response.rect);
            let window_focused = ui.ctx().input(|i| i.focused);
            let preview_rect = self.preview_rect(response.rect);
            self.update_preview(ui.ctx(), pointer_over, window_focused, preview_rect);
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
        let diagnostics: Vec<_> = self
            .preview_diagnostics
            .diagnostics()
            .into_iter()
            .cloned()
            .collect();
        let diagnostic_y = if self
            .preview
            .as_ref()
            .and_then(PreviewSession::diagnostic)
            .is_some()
        {
            54.0
        } else {
            8.0
        };
        let visible_diagnostics = diagnostics.iter().take(4);
        for (index, diagnostic) in visible_diagnostics.enumerate() {
            let y = diagnostic_y + index as f32 * 24.0;
            painter.rect_filled(
                egui::Rect::from_min_size(
                    response.rect.left_top() + egui::vec2(8.0, y),
                    egui::vec2(response.rect.width() - 16.0, 20.0),
                ),
                3.0,
                egui::Color32::from_rgba_unmultiplied(20, 28, 40, 210),
            );
            let message = format!(
                "{} (x{})",
                diagnostic.summary(),
                self.preview_diagnostics.occurrences(diagnostic)
            );
            painter.text(
                response.rect.left_top() + egui::vec2(14.0, y + 3.0),
                egui::Align2::LEFT_TOP,
                message,
                egui::FontId::monospace(11.0),
                preview_diagnostic_color(diagnostic.kind),
            );
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
            self.custom_font_atlases.clone(),
            self.scene_vector_atlas.clone(),
            self.custom_vector_atlases.clone(),
        )
    }

    /// 驱动预览会话：分辨率选择在 Play 期间变更时实时重建离屏纹理，
    /// 并把 egui 输入桥接为游戏输入后交给会话步进（节流/回读/上传在会话内部）。
    /// 步进 Play 预览并桥接本帧输入。
    ///
    /// * `pointer_over`：鼠标是否在 Game 视图内 —— 只影响**鼠标**事件（按钮/光标/滚轮），
    ///   否则点击编辑器其它面板会穿透到游戏；
    /// * `window_focused`：编辑器原生窗口是否持有键盘焦点（`egui::InputState::focused`）。
    ///
    /// **M22 修复**：此前两者都按「鼠标悬停在 Game 视图上」判定，于是按下 Play 后只要鼠标不在
    /// Game 视图上（很常见：用工具栏按钮进 Play 后鼠标还停在工具栏），**键盘输入会被整帧丢弃**；
    /// 手柄不受影响（它由运行时在步进时轮询合并，与快照无关），因此表现为「键盘没反应、手柄
    /// 也要把鼠标移进去才像能用」的错觉。现在键盘只要窗口有焦点就桥接；窗口失焦时用
    /// `set_focused(false)` 清空按键/鼠标/手柄的 held 状态（防卡键）。
    fn update_preview(
        &mut self,
        ctx: &egui::Context,
        pointer_over: bool,
        window_focused: bool,
        preview_rect: egui::Rect,
    ) {
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
        if window_focused {
            let (width, height) = self
                .preview
                .as_ref()
                .map(PreviewSession::size)
                .unwrap_or((640, 360));
            // 复用跨帧输入状态（M11-02）：held 按键/鼠标跨帧保留，`just_pressed`/
            // `just_released` 边沿只在本次模拟帧的 update 消费一次，随后在帧边界清空，
            // 避免按住的键在下一个 tick 重复触发一次性动作。
            let mut input = std::mem::take(&mut self.preview_input_state);
            input.set_focused(true);
            self.bridge_preview_input(&mut input, ctx, preview_rect, width, height, pointer_over);
            let stepped = self
                .preview
                .as_mut()
                .map(|preview| preview.update(ctx, &input))
                .is_some_and(consumes_input_edges);
            // **只有真的跑过逻辑步的帧才消费一次性边沿**（M22/W4 的编辑器侧补漏）。
            // `PreviewSession::update` 按 ~60fps 节流：未到 16ms 周期的帧返回
            // `PreviewTick::Idle`，**不跑固定步**（Lua 一次都没看到本帧输入）。此前这里
            // 无条件 `finish_frame()`，于是落在被节流帧里的按键/点击边沿被清掉而 Lua 从
            // 未见过它；而 `key_event` 只在「松开 → 按下」的跳变上产生边沿，仍按住的键
            // **不会补发**，所以那一下是**永久丢失**的——表现为「方向键切换按钮有时响应、
            // 有时完全没反应」（键按得久也没用，必须松开再按一次）。编辑器渲染帧率越高
            // （鼠标在动、面板动画等），被节流跳过的帧占比越大 → 丢失概率越高。
            // 独立 game 宿主早已有同样的守卫（`game_app.rs`：`steps > 0` 才 `finish_frame`）。
            if stepped {
                input.finish_frame();
            }
            self.preview_input_state = input;
        } else {
            // 窗口失焦（Alt-Tab 等）：清空 held 防止卡键（M11-02，含手柄按住状态与轴数字态）。
            self.preview_input_state.set_focused(false);
            // 失焦期间不再桥接输入，本帧的瞬态边沿也不该留到重新获得焦点之后——
            // 否则「在被节流帧按下 → 还没轮到逻辑步就失焦」的那一下会在回来时补触发
            // 一次幽灵按键（`set_focused(false)` 只清 held，不清 pressed/released）。
            self.preview_input_state.finish_frame();
            if let Some(preview) = &mut self.preview {
                preview.update(ctx, &InputSnapshot::default());
            }
        }
    }

    /// 从 egui 输入构建游戏输入快照（M20-08：**全键位**键盘 + 鼠标 + 滚轮）。
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
    ///
    /// M20-08：键盘改为**全键位**桥接——`PREVIEW_KEYS` 覆盖独立游戏窗口
    /// `game_app::map_key` 的全部键域（单字符键：字母/数字/符号 + 空格/Esc/
    /// 方向键/F1·F2·F4），因此 Play 里键盘与独立 `game` 运行时逐键一致。
    /// 列表是固定的：每帧对每个键都调 `key_event`，才能同时送出按下与松开边沿
    /// （只遍历 `keys_down` 会漏掉松开 → 卡键）。
    fn bridge_preview_input(
        &self,
        input: &mut InputSnapshot,
        ctx: &egui::Context,
        preview_rect: egui::Rect,
        width: u32,
        height: u32,
        pointer_over: bool,
    ) {
        ctx.input(|i| {
            // 键盘：**只要窗口有键盘焦点**就桥接（M22 修复：不再要求鼠标悬停在 Game 视图上，
            // 否则「按下 Play 后鼠标停在工具栏」会完全收不到按键）。
            for &egui_key in PREVIEW_KEYS {
                if let Some(game_key) = preview_input_key(egui_key) {
                    let down = i.key_down(egui_key);
                    // egui 的 `key_down` 是**帧末按住状态**：一次「按下 + 松开」若都落在同一帧内
                    // （编辑器卡顿/长帧时常见），帧末已是松开 → 按键对预览**完全不可见**，
                    // 这跟「被节流跳过的帧吞掉边沿」是同一类丢失。用本帧的**首次按下事件**补一条
                    // down + up 边沿（egui 已把 OS 连发事件标成 `repeat = true`，这里只认
                    // 首次按下，保持「一次按下 = 一步」），松开边沿一并送出，按住状态不变。
                    if !down && preview_key_first_pressed(i, egui_key) {
                        input.key_event(game_key, true);
                        input.key_event(game_key, false);
                    } else {
                        input.key_event(game_key, down);
                    }
                }
            }
            // 鼠标：只有指针在预览矩形内才桥接（否则点击编辑器其它面板会穿透到游戏）；
            // 指针离开时送松开边沿，避免「按住左键把鼠标移出」留下卡住的点击。
            input.mouse_event(MouseButton::Left, pointer_over && i.pointer.primary_down());
            input.mouse_event(
                MouseButton::Right,
                pointer_over && i.pointer.secondary_down(),
            );
            input.mouse_event(MouseButton::Middle, pointer_over && i.pointer.middle_down());
            if pointer_over && let Some(pos) = i.pointer.hover_pos() {
                // 把 egui 屏幕坐标映射到预览像素坐标，供 Button 命中测试使用。
                let px = (pos.x - preview_rect.min.x) / preview_rect.width().max(1.0)
                    * width.max(1) as f32;
                let py = (pos.y - preview_rect.min.y) / preview_rect.height().max(1.0)
                    * height.max(1) as f32;
                input.cursor_moved(px as f64, py as f64);
            }
            if pointer_over {
                let scroll = i.smooth_scroll_delta;
                if scroll.x != 0.0 || scroll.y != 0.0 {
                    input.scroll_event(scroll.x, scroll.y);
                }
            }
        });
    }
}

/// Play 预览桥接的 egui 键集合：与独立游戏窗口 `game_app::map_key` 同域。
///
/// 含字母 A..Z、数字 0..9、空格、Esc、四个方向键、F1/F2/F4（引擎把 `f1`/`f2`/`f4`
/// 解析为 `InputKey::Other(1/2/4)`）与常用符号键（`-`/`+`/`=`/`,`/`.`/`;`/`/`/`\`/
/// `[`/`]`/`` ` ``/`'`/`:`/`|`/`?`/`!`/`{`/`}`）。符号键进表是因为玩家可以把动作
/// 改绑到它们（编辑器捕获支持同一集合），列表固定才能保证每帧都送松开边沿。
const PREVIEW_KEYS: &[egui::Key] = &[
    egui::Key::A,
    egui::Key::B,
    egui::Key::C,
    egui::Key::D,
    egui::Key::E,
    egui::Key::F,
    egui::Key::G,
    egui::Key::H,
    egui::Key::I,
    egui::Key::J,
    egui::Key::K,
    egui::Key::L,
    egui::Key::M,
    egui::Key::N,
    egui::Key::O,
    egui::Key::P,
    egui::Key::Q,
    egui::Key::R,
    egui::Key::S,
    egui::Key::T,
    egui::Key::U,
    egui::Key::V,
    egui::Key::W,
    egui::Key::X,
    egui::Key::Y,
    egui::Key::Z,
    egui::Key::Num0,
    egui::Key::Num1,
    egui::Key::Num2,
    egui::Key::Num3,
    egui::Key::Num4,
    egui::Key::Num5,
    egui::Key::Num6,
    egui::Key::Num7,
    egui::Key::Num8,
    egui::Key::Num9,
    egui::Key::Space,
    egui::Key::Escape,
    egui::Key::ArrowUp,
    egui::Key::ArrowDown,
    egui::Key::ArrowLeft,
    egui::Key::ArrowRight,
    egui::Key::F1,
    egui::Key::F2,
    egui::Key::F4,
    egui::Key::Minus,
    egui::Key::Plus,
    egui::Key::Equals,
    egui::Key::Comma,
    egui::Key::Period,
    egui::Key::Semicolon,
    egui::Key::Slash,
    egui::Key::Backslash,
    egui::Key::OpenBracket,
    egui::Key::CloseBracket,
    egui::Key::Backtick,
    egui::Key::Quote,
    egui::Key::Colon,
    egui::Key::Pipe,
    egui::Key::Questionmark,
    egui::Key::Exclamationmark,
    egui::Key::OpenCurlyBracket,
    egui::Key::CloseCurlyBracket,
];

/// egui 键 → 引擎键身份（Play 预览键盘桥接）。
///
/// 字符类键与空格/Esc/方向键复用捕获面板的 `character_key_from_egui` +
/// `named_key_from_egui`（一份表），与独立游戏窗口的 `winit::Key::Character` 路径和
/// Lua 侧 `parse_key` **同源**；F1/F2/F4 在这里补齐（与 `game_app::map_key` 一致）。
/// 返回 `None` 的键（Tab/Enter/F3/F5…）不在引擎键域内，不参与桥接。
fn preview_input_key(key: egui::Key) -> Option<InputKey> {
    if let Some(key) = crate::panels::input_bindings::character_key_from_egui(key)
        .or_else(|| crate::panels::input_bindings::named_key_from_egui(key))
    {
        return Some(key);
    }
    match key.name() {
        // 与 `game_app::map_key` 的 F1/F2/F4 一致（F3 未被引擎映射）。
        "F1" => Some(InputKey::Other(1)),
        "F2" => Some(InputKey::Other(2)),
        "F4" => Some(InputKey::Other(4)),
        _ => None,
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

/// 这一 tick 是否真的跑过固定步 —— 也就是**是否允许消费本帧的一次性输入边沿**。
///
/// [`PreviewTick::Idle`] 表示「未到 16ms 节流周期，本帧只排空在途回读、不跑逻辑步」：
/// 此时 Lua 一次都没看到本帧输入，边沿必须**保留**给下一个有效帧（`update_preview`
/// 据此决定是否 `finish_frame`）。独立 game 宿主用等价的守卫（`steps > 0` 才清边沿），
/// 编辑器 Play 预览此前漏了这一步，导致被节流帧里的按键/点击被整帧吞掉。
fn consumes_input_edges(tick: PreviewTick) -> bool {
    tick != PreviewTick::Idle
}

/// 本帧是否收到了该键的**首次按下**事件（不含 OS 连发）。
///
/// egui 在 `begin_pass` 里把「已按住时再次收到的按下事件」标成 `repeat = true`
/// （见 egui `InputState::begin_pass`），因此 `repeat == false` 就是真实按下。
/// 只用于「按下 + 松开落在同一帧」的补齐路径（见 `bridge_preview_input`）：
/// 此时 `key_down` 已是 false，只看按住状态会漏掉这一下。
fn preview_key_first_pressed(input: &egui::InputState, egui_key: egui::Key) -> bool {
    input.events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                ..
            } if *key == egui_key
        )
    })
}

fn preview_diagnostic_color(kind: PreviewErrorKind) -> egui::Color32 {
    match kind {
        PreviewErrorKind::SceneLoad | PreviewErrorKind::Layout => egui::Color32::YELLOW,
        PreviewErrorKind::AssetMissing | PreviewErrorKind::ScriptRuntime => {
            egui::Color32::LIGHT_RED
        }
        PreviewErrorKind::Render | PreviewErrorKind::Input => egui::Color32::LIGHT_BLUE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlayState;
    use crate::SceneTool;
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
        engine::core::games_dir().join("examples").join("game_api")
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

    /// M22/W4：失焦期间不桥接输入，本帧残留的按下边沿也不得留到重新聚焦之后——
    /// 否则「在被节流帧按下 → 还没轮到逻辑步就 Alt-Tab」的那一下会在回来时补触发一次
    /// 幽灵按键（`set_focused(false)` 只清 held，不清 pressed/released）。
    #[test]
    fn focus_loss_drops_pending_press_edges() {
        let mut state = InputSnapshot::default();
        // 被节流帧里按下：边沿已记、held 也在，但还没有逻辑步来消费它。
        state.key_event(InputKey::ARROW_DOWN, true);
        assert!(state.just_pressed(InputKey::ARROW_DOWN));
        // 与 `update_preview` 的失焦分支同序。
        state.set_focused(false);
        state.finish_frame();
        state.set_focused(true);
        assert!(!state.is_down(InputKey::ARROW_DOWN), "失焦必须清 held");
        assert!(
            !state.just_pressed(InputKey::ARROW_DOWN),
            "重新聚焦后不得补触发失焦前的按下"
        );
    }

    #[test]
    fn sample_project_stop_restores_editor_context_and_clears_preview_input() {
        let root = copy_sample_project("stop");
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));
        let original = app.document.current_scene.as_ref().unwrap().clone();
        let original_selection = Some(SceneEntityId(2));
        let original_zoom = app.scene_zoom;
        let original_pan = app.scene_state.pan;
        let original_tool = app.scene_tool;
        let original_tab = app.workspace_tab;

        app.selection
            .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);
        app.editor_context_snapshot = Some(app.capture_editor_context());
        app.play_session = Some(PlaySession::start(&original));
        app.play_state = PlayState::Playing;
        app.workspace_tab = WorkspaceTab::Game;
        app.scene_tool = SceneTool::Move;
        app.scene_zoom = 1.6;
        app.scene_state.pan = egui::vec2(42.0, -17.0);
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
        assert_eq!(app.workspace_tab, original_tab);
        assert!(app.play_session.is_none());
        assert!(!app.preview_input_state.is_down(InputKey::W));
        assert!(!app.preview_input_state.mouse_is_down(MouseButton::Left));
        assert_eq!(app.selection.primary(), original_selection);
        assert_eq!(app.selection.source(), editor::SelectionSource::Hierarchy);
        assert_eq!(app.scene_tool, original_tool);
        assert_eq!(app.scene_zoom, original_zoom);
        assert_eq!(app.scene_state.pan, original_pan);
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

    /// M22/W4（编辑器侧）端到端回归：**Play 预览被节流跳过的帧不得吞掉按键边沿**。
    ///
    /// 预览按 ~60fps 节流：未到周期的帧只排空回读、**不跑固定步**（`PreviewTick::Idle`）。
    /// 此前 `update_preview` 无条件 `finish_frame()`，于是落在这种帧里的按下被清掉而 Lua
    /// 从未见过它；`key_event` 只在「松开 → 按下」跳变上产生边沿，仍按住的键不会补发 →
    /// 那一下**永久丢失**（用户实测：「键盘按上下键切换按钮，有时响应，有时不响应」）。
    /// 这里用 `force_throttled()` 确定性地制造一个 Idle 帧：边沿必须活到下一个有效帧。
    #[test]
    fn play_preview_throttled_frame_preserves_key_edges() {
        let Some((device, queue)) = test_device() else {
            return;
        };
        let root = copy_sample_project("preview-idle-edge");
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, Some((device, queue)), LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
        app.play();
        assert!(app.preview.is_some(), "Play should create a preview session");

        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 360.0));
        let pass = |events: Vec<Event>| RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let arrow_down = Event::Key {
            key: Key::ArrowDown,
            physical_key: Some(Key::ArrowDown),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        };

        // 第 1 帧：到期 → 步进（本帧无按键），建立 last_tick。
        ctx.begin_pass(pass(Vec::new()));
        app.update_preview(&ctx, true, true, screen);
        let _ = ctx.end_pass();
        assert!(app.preview.is_some());

        // 第 2 帧：强制落在节流周期内（Idle，不跑固定步）→ 此时按下方向键。
        if let Some(preview) = app.preview.as_mut() {
            preview.force_throttled();
        }
        ctx.begin_pass(pass(vec![arrow_down]));
        app.update_preview(&ctx, true, true, screen);
        let _ = ctx.end_pass();
        assert!(
            app.preview_input_state.just_pressed(InputKey::ARROW_DOWN),
            "被节流跳过的帧（没跑固定步）不得消费按键边沿"
        );

        // 第 3 帧：**到期** → 步进 → 边沿被正常消费，不重复触发。
        // 注意：被节流的那一帧不推进 `last_tick`，所以紧接的下一帧仍在周期内——
        // 这里显式 `force_tick()` 才能确定性地拿到一个有效帧。
        if let Some(preview) = app.preview.as_mut() {
            preview.force_tick();
        }
        ctx.begin_pass(pass(Vec::new()));
        app.update_preview(&ctx, true, true, screen);
        let _ = ctx.end_pass();
        assert!(
            !app.preview_input_state.just_pressed(InputKey::ARROW_DOWN),
            "有效帧消费边沿后不得重复触发"
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

    /// M22/W4（编辑器侧）：被节流跳过的帧（`PreviewTick::Idle`，没跑固定步）
    /// **不得**消费一次性输入边沿。
    #[test]
    fn idle_preview_frame_must_not_consume_input_edges() {
        assert!(
            !consumes_input_edges(PreviewTick::Idle),
            "Idle = 未到节流周期、没跑固定步 → 边沿必须留到下一有效帧"
        );
        // 真正跑过（或尝试跑过）固定步的 tick 才允许清边沿。
        assert!(consumes_input_edges(PreviewTick::Rendered));
        assert!(consumes_input_edges(PreviewTick::Pending));
        assert!(consumes_input_edges(PreviewTick::Failed));
    }

    /// M22/W4（编辑器侧）：同一帧内「按下 + 松开」也必须产生按下边沿。
    ///
    /// `bridge_preview_input` 轮询的是 egui 的 `key_down`（**帧末**按住状态）：一次极短的
    /// 点按若整个落在同一帧里（编辑器长帧/卡顿时常见），帧末已是松开态 —— 只按轮询结果
    /// 桥接就会**整帧漏掉**这一次按下（与「被节流帧吞边沿」同类）。这里锁住补齐路径。
    #[test]
    fn bridge_preview_input_keeps_same_frame_tap() {
        let app = app_with_scene();
        let ctx = egui::Context::default();
        let preview_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0));
        let key_event = |pressed: bool| Event::Key {
            key: Key::ArrowDown,
            physical_key: Some(Key::ArrowDown),
            pressed,
            repeat: false,
            modifiers: Modifiers::default(),
        };
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 240.0),
            )),
            // 按下与松开都在同一帧内 → `key_down(ArrowDown) == false`。
            events: vec![key_event(true), key_event(false)],
            ..Default::default()
        };
        ctx.begin_pass(raw);
        let mut input = InputSnapshot::default();
        app.bridge_preview_input(&mut input, &ctx, preview_rect, 200, 100, true);
        let _ = ctx.end_pass();

        assert!(
            !input.is_down(InputKey::ARROW_DOWN),
            "帧末已经是松开态（按住状态不应残留）"
        );
        assert!(
            input.just_pressed(InputKey::ARROW_DOWN),
            "同一帧内的快速点按必须保留按下边沿（否则方向键会偶发无反应）"
        );
        assert!(
            input.just_released(InputKey::ARROW_DOWN),
            "补齐路径同时要送出松开边沿"
        );
    }

    /// 复现「方向键有时没反应」的机制：在没跑固定步的帧里清边沿，这一下按下就
    /// **永久**丢了——`key_event` 只在「松开 → 按下」跳变上产生边沿，仍按住的键
    /// 不会补发（时长再长也没用，必须松开重按）。
    #[test]
    fn clearing_edges_in_idle_frame_loses_a_press_forever() {
        // 旧行为（无条件 `finish_frame`）：边沿被吃掉，下一帧只剩 held。
        let mut old = InputSnapshot::default();
        old.key_event(InputKey::ARROW_DOWN, true);
        assert!(old.just_pressed(InputKey::ARROW_DOWN));
        old.finish_frame();
        old.key_event(InputKey::ARROW_DOWN, true); // 每帧重喂「仍然按住」
        assert!(old.is_down(InputKey::ARROW_DOWN));
        assert!(
            !old.just_pressed(InputKey::ARROW_DOWN),
            "机制复现：Idle 帧清边沿后，按住不放也再没有 just_pressed"
        );

        // 修复后（Idle 帧保留边沿）：下一个有效帧照样把 `just_pressed` 交给 Lua。
        let mut fixed = InputSnapshot::default();
        fixed.key_event(InputKey::ARROW_DOWN, true);
        if consumes_input_edges(PreviewTick::Idle) {
            fixed.finish_frame();
        }
        fixed.key_event(InputKey::ARROW_DOWN, true);
        assert!(
            fixed.just_pressed(InputKey::ARROW_DOWN),
            "Idle 帧必须把按下边沿留到下一有效帧（否则方向键会随机失效）"
        );
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
        app.bridge_preview_input(&mut input, &ctx, preview_rect, 200, 100, true);
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

    /// M20-08：Play 预览键盘桥接为**全键位**——覆盖动作表默认绑定用到的键
    /// （J/K/G/T/Q/I/P/B/F/X/U/L/R/C/1-9/+/-）与独立游戏窗口的键域。
    #[test]
    fn bridge_preview_input_covers_full_keyboard() {
        let app = app_with_scene();
        let ctx = egui::Context::default();
        let preview_rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 100.0));
        let pressed = [
            Key::J,
            Key::K,
            Key::G,
            Key::T,
            Key::Q,
            Key::I,
            Key::P,
            Key::B,
            Key::F,
            Key::X,
            Key::U,
            Key::L,
            Key::R,
            Key::C,
            Key::Num1,
            Key::Num9,
            Key::Minus,
            Key::Plus,
            Key::Comma,
            Key::Backslash,
            Key::Quote,
            Key::F4,
        ];
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 240.0),
            )),
            events: pressed
                .iter()
                .map(|key| Event::Key {
                    key: *key,
                    physical_key: Some(*key),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::default(),
                })
                .collect(),
            ..Default::default()
        };
        ctx.begin_pass(raw);
        let mut input = InputSnapshot::default();
        app.bridge_preview_input(&mut input, &ctx, preview_rect, 200, 100, true);
        let _ = ctx.end_pass();

        // 字母/数字/符号与 `InputKey::from_char`（独立窗口与 `parse_key` 同源）一致。
        for (egui_key, character) in [
            (Key::J, 'j'),
            (Key::K, 'k'),
            (Key::G, 'g'),
            (Key::T, 't'),
            (Key::Q, 'q'),
            (Key::I, 'i'),
            (Key::P, 'p'),
            (Key::B, 'b'),
            (Key::F, 'f'),
            (Key::X, 'x'),
            (Key::U, 'u'),
            (Key::L, 'l'),
            (Key::R, 'r'),
            (Key::C, 'c'),
            (Key::Num1, '1'),
            (Key::Num9, '9'),
            (Key::Minus, '-'),
            (Key::Plus, '+'),
            (Key::Comma, ','),
            (Key::Backslash, '\\'),
            (Key::Quote, '\''),
        ] {
            assert!(
                input.is_down(InputKey::from_char(character)),
                "{egui_key:?} 未桥接为 {character:?}（Play 预览键位不完整）"
            );
        }
        // F4：引擎把 `f4` 解析为 `InputKey::Other(4)`（与独立窗口一致）。
        assert!(input.is_down(InputKey::Other(4)));

        // 松开边沿必须送到：egui 的 held 集合只在收到释放事件时变化，因此这里显式
        // 送一帧「全部释放」事件（模拟按键抬起），桥接必须把它翻成 released。
        input.finish_frame();
        ctx.begin_pass(RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 240.0),
            )),
            events: pressed
                .iter()
                .map(|key| Event::Key {
                    key: *key,
                    physical_key: Some(*key),
                    pressed: false,
                    repeat: false,
                    modifiers: Modifiers::default(),
                })
                .collect(),
            ..Default::default()
        });
        app.bridge_preview_input(&mut input, &ctx, preview_rect, 200, 100, true);
        let _ = ctx.end_pass();
        assert!(!input.is_down(InputKey::from_char('j')));
        assert!(
            input.just_released(InputKey::from_char('j')),
            "松开边沿必须桥接（否则 Play 里会卡键）"
        );
        assert!(!input.is_down(InputKey::from_char('+')));
    }

    /// M22 回归：**Play 预览的键盘不依赖鼠标悬停**。
    ///
    /// 之前 `draw_game_view` 用「鼠标是否在 Game 视图内」当作「是否桥接输入」，于是按下 Play 后
    /// 鼠标停在工具栏/面板上时键盘完全没有反应（用户实测：Play 里标题界面键盘与手柄都「选不动」，
    /// 其中键盘就是这个门控造成的）。现在键盘只看窗口焦点；鼠标仍要求指针在预览矩形内，
    /// 并且指针移出时送松开边沿（不留下卡住的点击）。
    #[test]
    fn preview_keyboard_does_not_require_pointer_hover() {
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
                    key: Key::ArrowDown,
                    physical_key: Some(Key::ArrowDown),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::default(),
                },
                Event::PointerButton {
                    pos: egui::pos2(60.0, 70.0),
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::default(),
                },
            ],
            ..Default::default()
        };
        ctx.begin_pass(raw);
        let mut input = InputSnapshot::default();
        // `pointer_over = false`：鼠标不在 Game 视图内（指针事件也落在别处）。
        app.bridge_preview_input(&mut input, &ctx, preview_rect, 200, 100, false);
        let _ = ctx.end_pass();

        assert!(
            input.is_down(InputKey::ARROW_DOWN),
            "鼠标不在 Game 视图内时按键仍必须桥接"
        );
        assert!(
            !input.mouse_is_down(MouseButton::Left),
            "指针不在预览内时鼠标按下不得进入游戏"
        );
        assert_eq!(
            input.cursor_position(),
            (0.0, 0.0),
            "指针在预览外不更新光标"
        );
    }
}
