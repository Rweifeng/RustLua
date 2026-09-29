//! Application shell: owns the window, renderer, runtime scene and input
//! snapshot; drives the fixed-timestep simulation and frame redraw; maps
//! winit events to engine input.

use std::{path::PathBuf, sync::Arc, time::Instant};

use engine::core::{
    camera::{FrameViews, Viewport},
    input::{InputSnapshot, MouseButton},
    logging::{EngineError, LogModule},
    performance::{PerformanceSnapshot, PerformanceStats},
    time_step::FixedTimeStep,
    window::{AppLifecycle, has_renderable_size},
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, WindowEvent},
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};

use engine::host::{OverlayVertex, RenderOutcome, Renderer, RuntimeScene, SceneDraw, SceneMesh};

pub struct GameApp {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    lifecycle: AppLifecycle,
    occluded: bool,
    time_step: FixedTimeStep,
    input: InputSnapshot,
    last_frame: Instant,
    performance: PerformanceStats,
    /// 调试 overlay（帧率/耗时文本）是否绘制。**默认关闭**：发布包不应带开发者
    /// 读数，游戏内按 `F3` 可随时开启/关闭（诊断用）。数值本身仍每帧统计并写
    /// `[perf]` 日志，因此关掉绘制不影响性能基线。
    overlay_enabled: bool,
    runtime_scene: Option<RuntimeScene>,
    /// 运行期失败原因（Lua 固定步异常 / 渲染致命错误 / 场景或渲染器初始化失败）。
    /// 非空时宿主进程必须以**非零退出码**结束：这些路径此前只调用
    /// `event_loop.exit()`，`main` 随后返回 `Ok(())`，于是崩溃在 CI、发布脚本和
    /// 自动化验收里都表现为成功。
    failure: Option<String>,
    project_root: Option<PathBuf>,
    window_title: String,
    /// P3：发布包模式（`--pack`）⇒ 存档与玩家设置写用户数据目录。
    packaged: bool,
    /// CPU frustum culling (M8-08); off via `--no-cull` for baselines.
    cull_enabled: bool,
    /// Same mesh/material instancing (M8-08); off via `--no-instance`.
    instancing_enabled: bool,
}

impl GameApp {
    /// 构造运行器。`packaged` = 是否发布包（`--pack`）：发布包把存档与玩家设置写到
    /// 用户数据目录（见 `engine::core::user_dir`），而不是程序目录。
    pub fn new(
        project_root: Option<PathBuf>,
        window_title: String,
        cull_enabled: bool,
        instancing_enabled: bool,
        packaged: bool,
    ) -> Self {
        Self {
            window: None,
            renderer: None,
            lifecycle: AppLifecycle::Initializing,
            occluded: false,
            time_step: FixedTimeStep::new(60, 8).expect("valid fixed timestep"),
            input: InputSnapshot::default(),
            last_frame: Instant::now(),
            performance: PerformanceStats::new(),
            // 发布默认不带帧率 overlay（F3 可手动开启，见字段注释）。
            overlay_enabled: false,
            runtime_scene: None,
            failure: None,
            project_root,
            window_title,
            packaged,
            cull_enabled,
            instancing_enabled,
        }
    }

    fn transition(&mut self, next: AppLifecycle) {
        if let Err(error) = self.lifecycle.transition(next) {
            tracing::info!(target: "game", "[lifecycle] {error}");
            return;
        }
        tracing::info!(target: "game", "[lifecycle] {:?}", self.lifecycle);
    }

    fn request_exit(&mut self, event_loop: &ActiveEventLoop) {
        if matches!(self.lifecycle, AppLifecycle::Running | AppLifecycle::Paused) {
            self.transition(AppLifecycle::Exiting);
        }
        event_loop.exit();
        if self.lifecycle == AppLifecycle::Exiting {
            self.transition(AppLifecycle::Exited);
        }
    }

    /// 记录一次**失败**退出：首个原因胜出（后续失败通常是它的连锁反应），然后
    /// 走正常的退出流程。`main` 在事件循环结束后读取它并返回 `Err`，从而让进程
    /// 退出码非零。
    fn fail(&mut self, event_loop: &ActiveEventLoop, reason: impl Into<String>) {
        let reason = reason.into();
        if self.failure.is_none() {
            self.failure = Some(reason);
        }
        self.request_exit(event_loop);
    }

    /// 本次运行的失败原因；`None` 表示正常退出（含 Lua `app.quit()` 与用户关窗）。
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    fn update_pause_state(&mut self, size: PhysicalSize<u32>) {
        let should_pause = self.occluded || !has_renderable_size(size.width, size.height);
        match (should_pause, self.lifecycle) {
            (true, AppLifecycle::Running) => {
                self.input.clear_all();
                self.transition(AppLifecycle::Paused);
            }
            (false, AppLifecycle::Paused) => self.transition(AppLifecycle::Running),
            _ => {}
        }
        if should_pause {
            tracing::info!(target: "game",
                "[window] simulation paused: size={}x{} occluded={}",
                size.width, size.height, self.occluded
            );
        } else {
            tracing::info!(target: "game", "[window] renderable size: {}x{}", size.width, size.height);
        }
    }

    fn run_simulation_steps(
        &mut self,
        steps: u32,
        delta_seconds: f32,
        event_loop: &ActiveEventLoop,
    ) {
        let input = self.input.clone();
        self.performance.begin_update();
        let result = self
            .runtime_scene
            .as_mut()
            .map(|scene| scene.run_script_steps(steps, delta_seconds, &input));
        self.performance.end_update();
        // Lua 耗时：由运行时在 Lua 体内部测量后取走，只含 `update()` 本体
        // （不含 ECS/物理/渲染），所以 `lua_ms` 与指标名一致。0 步帧取到 0。
        if let Some(scene) = self.runtime_scene.as_mut() {
            let micros = scene.take_script_micros();
            self.performance.record_lua_millis(micros as f32 / 1000.0);
        }
        if let Some(Err(error)) = result {
            let diagnostic = EngineError::new(
                LogModule::Script,
                "fixed-step update",
                "project Lua update function",
            )
            .with_source(&error)
            .with_suggestion("fix the Lua runtime error reported above");
            tracing::error!(target: "script", error = %diagnostic, "fixed-step update failed");
            self.fail(event_loop, format!("fixed-step Lua update failed: {error}"));
        }
        // Lua `app.quit()`：只在 fixed-step 更新后消费退出标记，避免在无
        // runtime_scene（初始化/异常）时误退。
        if let Some(scene) = self.runtime_scene.as_ref()
            && scene.quit_requested()
        {
            self.request_exit(event_loop);
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.lifecycle != AppLifecycle::Running {
            return;
        }
        self.performance.begin_frame();
        self.performance.reset_lua_sample();
        let elapsed = self.last_frame.elapsed().as_secs_f64();
        self.last_frame = Instant::now();
        let report = self.time_step.advance(elapsed);
        if report.dropped_steps > 0 {
            tracing::warn!(target: "engine", "[time] dropped catch-up steps: dropped={} total_dropped={} accumulator={:.6}s", report.dropped_steps, report.total_dropped_steps, report.accumulator_seconds);
        }
        if report.steps > 0 && report.total_steps % 60 < u64::from(report.steps) {
            tracing::info!(target: "engine", "[time] fixed-step stats: steps={} dt={:.6}s accumulator={:.6}s total_steps={} total_dropped={}", report.steps, report.delta_seconds, report.accumulator_seconds, report.total_steps, report.total_dropped_steps);
        }
        self.run_simulation_steps(report.steps, report.delta_seconds, event_loop);
        if self.lifecycle != AppLifecycle::Running {
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        // Only clear edge transitions on frames that actually ran a fixed step:
        // otherwise, at high frame rates a `just_pressed` (e.g. F4 scene switch)
        // is dropped before Lua ever consumes it.
        if report.steps > 0 {
            self.input.finish_frame();
        }
        let snapshot = self.performance.snapshot();
        // 先在不借用 `self.runtime_scene` 的作用域里取回结果，再处理失败——
        // 失败处理需要 `&mut self`（记录失败态并退出），不能在借用期间调用。
        let mesh_result: Result<SceneMesh, String> = match self.runtime_scene.as_mut() {
            Some(scene) => scene
                .vertices(renderer.config.width, renderer.config.height)
                .map_err(|error| error.to_string()),
            None => Ok(SceneMesh {
                vertices: Vec::new(),
                draws: Vec::new(),
                world_sprites: Vec::new(),
                world_sprite_draws: Vec::new(),
                paths: Vec::new(),
                // 无场景时的空网格：没有资源身份（渲染器回退到按路径解析）。
                path_handles: Vec::new(),
                font_atlas: None,
                custom_font_atlases: Vec::new(),
                vector_vertices: Vec::new(),
                vector_glyphs: Vec::new(),
                vector_segments: Vec::new(),
            }),
        };
        let mut mesh = match mesh_result {
            Ok(mesh) => mesh,
            Err(error) => {
                tracing::error!(target: "render", "[runtime] 2D scene extraction failed: {error}");
                self.fail(event_loop, format!("2D scene extraction failed: {error}"));
                return;
            }
        };
        if self.overlay_enabled {
            let overlay = build_overlay(snapshot, renderer.config.width, renderer.config.height);
            if !overlay.ranges.is_empty() {
                let base = mesh.vertices.len() as u32;
                let atlas_texture = mesh.paths.len() as u32 + 1;
                let uses_atlas = overlay.ranges.iter().any(|range| range.atlas);
                mesh.vertices.extend(overlay.vertices);
                for range in overlay.ranges {
                    mesh.draws.push(SceneDraw {
                        texture: if range.atlas { atlas_texture } else { 0 },
                        start: base + range.start,
                        count: range.count,
                        sdf: false,
                        bitmap: false,
                        vector: false,
                        // 性能浮层是世界之上的叠加内容：屏幕空间、不参与深度。
                        render_layer: engine::core::layer::RenderLayer::Overlay2D,
                    });
                }
                if uses_atlas {
                    // 内置 5x7 字模图集是渲染器的保留纹理，走 `paths` 索引即可。
                    mesh.paths
                        .push(engine::host::frame::ATLAS_TEXTURE_PATH.to_owned());
                }
            }
        }
        // 拆分批 7：视口（而不是一个裸比例）是宽高比的唯一来源；`scene_3d` 不再改写相机载荷。
        let viewport = Viewport::new(
            renderer.config.width.max(1) as f32,
            renderer.config.height.max(1) as f32,
        )
        .unwrap_or(Viewport {
            width: 1.0,
            height: 1.0,
        });
        let scene3d = self
            .runtime_scene
            .as_ref()
            .and_then(|scene| scene.scene_3d(viewport));
        // WP5：把本帧的相机侧视图交给渲染器——清屏色与 2D overlay 的视口由这些声明决定，
        // 渲染器不再自己写死。2D 视图与上面 `vertices()` 用的是同一个 `RuntimeScene` 相机，
        // 因此"取景"和"清屏/视口"不会各说一套。
        if let Some(scene) = self.runtime_scene.as_ref() {
            let overlay = scene.render_view_2d(renderer.config.width, renderer.config.height);
            // 3D 主视图直接用**提取时烘焙好的那一份**（拆分批 7）：再调一次 `main_3d`
            // 等于把同一个视口烘焙两遍，还多一个"两次烘焙会不会不一致"的问题。
            let main = scene3d.as_ref().map(|scene3d| scene3d.view);
            renderer.set_frame_views(FrameViews {
                main,
                overlay: Some(overlay),
            });
            // WP6：世界 Sprite 深度通道打开时把 2D 相机的矩阵喂给渲染器（关闭时为 None）。
            if let Some(view_projection) =
                scene.sprite_view_projection(renderer.config.width, renderer.config.height)
            {
                renderer.set_world_sprite_camera(view_projection);
            }
        }
        match renderer.render(&mesh, scene3d.as_ref()) {
            RenderOutcome::Presented => {}
            RenderOutcome::Retry | RenderOutcome::Skipped => {}
            RenderOutcome::Fatal => self.fail(event_loop, "renderer reported a fatal outcome"),
        }
        self.performance.end_frame();
        if snapshot.frames.is_multiple_of(60) {
            tracing::info!(target: "engine", "[perf] fps={:.1} frame_ms={:.3} update_ms={:.3} lua_ms={:.3} frames={} overlay={}", snapshot.fps, snapshot.frame_ms, snapshot.update_ms, snapshot.lua_ms, snapshot.frames, self.overlay_enabled);
        }
    }
}

impl ApplicationHandler for GameApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // 窗口尺寸由工程声明（`project.toml` 的 `window_width`/`window_height`），缺省
        // 1280×720 横屏。**为什么不能写死**：竖版工程（`game_jhyt` 的 720×1280）里
        // Lua 动态 UI 是屏幕像素空间、不随参考分辨率缩放，窗口比例不对就会整块画到屏幕外。
        let (window_width, window_height) = match self.project_root.as_deref() {
            Some(root) => {
                let (size, diagnostics) = engine::core::project_config::window_size(root);
                for diagnostic in diagnostics {
                    tracing::warn!(target: "window", "[window] {diagnostic}");
                }
                size
            }
            None => engine::core::project_config::DEFAULT_WINDOW_SIZE,
        };
        let attributes = Window::default_attributes()
            .with_title(&self.window_title)
            .with_inner_size(PhysicalSize::new(window_width, window_height));
        tracing::info!(
            target: "game",
            "[window] requested size: {window_width}x{window_height}"
        );
        let window = Arc::new(event_loop.create_window(attributes).expect("create window"));
        tracing::info!(target: "game", "[window] created: {:?}", window.id());
        match Renderer::new(
            window.clone(),
            self.project_root.clone(),
            self.cull_enabled,
            self.instancing_enabled,
        ) {
            Ok(renderer) => {
                self.window = Some(window);
                self.renderer = Some(renderer);
                self.runtime_scene = match RuntimeScene::new_with_layout(
                    self.project_root.as_deref(),
                    self.packaged,
                ) {
                    Ok(scene) => Some(scene),
                    Err(error) => {
                        let diagnostic = EngineError::new(
                            LogModule::Script,
                            "initialize runtime scene",
                            "runtime scene or project script failed to start",
                        )
                        .with_source(&error)
                        .with_suggestion("check the project scene and scripts/main.lua");
                        tracing::error!(target: "game", error = %diagnostic, "runtime initialization failed");
                        self.fail(
                            event_loop,
                            format!("runtime scene initialization failed: {error}"),
                        );
                        return;
                    }
                };
                // M20-08：把手柄死区（`settings.toml` 的 `[input]` 段）应用到设备后端。
                // 文件缺失 → 引擎默认 0.15/0.05；读取失败只记日志。P3：发布包读的是
                // **用户数据目录**里的那份设置（`user_dir::data_roots`），与 Lua 侧的
                // `input.settings_path()` 同源，否则会出现"面板显示一套、设备用另一套"。
                if let (Some(root), Some(scene)) =
                    (self.project_root.as_deref(), self.runtime_scene.as_mut())
                {
                    let settings_root =
                        engine::core::user_dir::data_roots(root, self.packaged).settings_dir;
                    let (deadzone, diagnostic) =
                        engine::core::settings::load_deadzone_section(&settings_root);
                    if let Some(diagnostic) = diagnostic {
                        tracing::warn!(target: "input", "[gamepad] {diagnostic}");
                    }
                    scene.set_gamepad_deadzone(deadzone);
                    tracing::info!(
                        target: "input",
                        stick = deadzone.stick,
                        trigger = deadzone.trigger,
                        "[gamepad] deadzone applied"
                    );
                }
                self.transition(AppLifecycle::Running);
                // 发布默认不画调试 overlay；提示一次开启方式，便于需要时诊断。
                tracing::info!(
                    target: "game",
                    "[perf] overlay {} (F3 toggles the fps/latency overlay)",
                    if self.overlay_enabled { "enabled" } else { "disabled" }
                );
                self.last_frame = Instant::now();
                self.window
                    .as_ref()
                    .expect("window exists")
                    .request_redraw();
            }
            Err(error) => {
                tracing::error!(target: "engine", error = %EngineError::new(LogModule::Engine, "initialize renderer", "GPU renderer startup failed").with_source(&error).with_suggestion("update the graphics driver or use a compatible GPU"), "renderer initialization failed");
                self.window = Some(window);
                self.fail(
                    event_loop,
                    format!("renderer initialization failed: {error}"),
                );
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_none_or(|window| window.id() != window_id)
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => self.request_exit(event_loop),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size);
                }
                self.update_pause_state(size);
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                let size = self.window.as_ref().expect("window exists").inner_size();
                self.update_pause_state(size);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // M22/W1：**物理键**身份（`PhysicalKey::Code`），经 `host::keymap`
                // 转换——宿主只做类型转换，不再按 `logical_key` 的字符猜键位。
                // AZERTY/Dvorak 上「W 位置」因此仍是 `move_up`，而 Tab/Enter/F5/
                // 修饰键也能表达（字符身份做不到）。
                if let Some(key) = engine::host::keymap::map_key_event(&event) {
                    self.input
                        .key_event(key, event.state == ElementState::Pressed);
                }
                // 下面两条是**宿主自身的界面快捷键**（性能叠加层开关、退出），
                // 不是玩法输入，因此继续按逻辑键判定：它们不该受改键表影响，
                // 也不该因为键盘布局而变化。
                if event.state == ElementState::Pressed
                    && matches!(
                        &event.logical_key,
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::F3)
                    )
                {
                    self.overlay_enabled = !self.overlay_enabled;
                    tracing::info!(target: "game", "[perf] overlay {}", if self.overlay_enabled { "enabled" } else { "disabled" });
                }
                if event.state == ElementState::Pressed
                    && event.logical_key
                        == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
                {
                    self.request_exit(event_loop);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.input
                    .mouse_event(map_mouse_button(button), state == ElementState::Pressed);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input.cursor_moved(position.x, position.y);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x, y),
                    winit::event::MouseScrollDelta::PixelDelta(position) => {
                        (position.x as f32 / 100.0, position.y as f32 / 100.0)
                    }
                };
                self.input.scroll_event(dx, dy);
            }
            WindowEvent::Focused(focused) => {
                self.input.set_focused(focused);
                if !focused {
                    self.input.clear_transitions();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                if self.lifecycle == AppLifecycle::Running {
                    self.window
                        .as_ref()
                        .expect("window exists")
                        .request_redraw();
                }
            }

            _ => {}
        }
    }
}
/// 一帧 overlay 的顶点与绘制范围（`start`/`count` 相对 `vertices`）。
struct OverlayGeometry {
    vertices: Vec<OverlayVertex>,
    ranges: Vec<OverlayRange>,
}

struct OverlayRange {
    start: u32,
    count: u32,
    /// `true` = 用内置 5x7 字模图集绘制（文字），`false` = 纯色白纹理（面板/量条）。
    atlas: bool,
}

/// 调试 overlay（F3 开关）：左上角一块半透明面板，逐行显示**带标签的性能指标**，
/// 每行下面一条量条标明当前值占参考上限的比例。
///
/// 指标（与 `[perf]` 日志同源，`PerformanceSnapshot`）：
/// * `FPS`：最近 1 秒窗口的真实帧率（参考上限 120）；
/// * `FRAME ms`：上一帧总耗时（上限 33.3ms ≈ 30fps，超过 20ms 变红）；
/// * `UPDATE ms`：`run_simulation_steps` 的固定步更新耗时（上限 16.7ms = 60fps）；
/// * `LUA ms`：本帧 Lua `update()` 累计耗时（上限 8ms）；
/// * `FRAMES`：累计帧数（无参考上限，量条按 60 帧窗口的余数显示，仅作心跳）。
///
/// 文字用引擎内置 5x7 位图字模（`TextAtlas::built_in` + `TextLayout::build_bitmap`），
/// 因此 overlay 不依赖项目字体，也不会受项目字体加载状态影响。
fn build_overlay(snapshot: PerformanceSnapshot, width: u32, height: u32) -> OverlayGeometry {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let mut vertices = Vec::new();
    let mut ranges = Vec::new();

    // 文本行：`(标签, 数值文本, 量条比例, 数值颜色)`。
    let frame_ms_color = if snapshot.frame_ms > 20.0 {
        [1.0, 0.42, 0.38, 1.0]
    } else {
        [0.92, 0.96, 0.92, 1.0]
    };
    let rows = [
        (
            "FPS",
            format!("{:.1}", snapshot.fps),
            (snapshot.fps / 120.0).clamp(0.0, 1.0),
            [0.45, 1.0, 0.55, 1.0],
        ),
        (
            "FRAME ms",
            format!("{:.2}", snapshot.frame_ms),
            (snapshot.frame_ms / 33.3).clamp(0.0, 1.0),
            frame_ms_color,
        ),
        (
            "UPDATE ms",
            format!("{:.2}", snapshot.update_ms),
            (snapshot.update_ms / 16.7).clamp(0.0, 1.0),
            frame_ms_color,
        ),
        (
            "LUA ms",
            format!("{:.2}", snapshot.lua_ms),
            (snapshot.lua_ms / 8.0).clamp(0.0, 1.0),
            frame_ms_color,
        ),
        (
            "FRAMES",
            format!("{}", snapshot.frames),
            ((snapshot.frames % 600) as f32) / 600.0,
            [0.75, 0.78, 0.72, 1.0],
        ),
    ];

    // 像素布局：面板 8px 内边距 + 5x7 字模放大 2 倍（10x14px），每行 22px。
    const PAD_X: f32 = 8.0;
    const PAD_Y: f32 = 6.0;
    const ROW_H: f32 = 22.0;
    const SCALE: f32 = 2.0;
    let panel_w = 232.0;
    let panel_h = PAD_Y * 2.0 + 20.0 + rows.len() as f32 * ROW_H;

    // 面板底 + 文字。
    let panel = rect_vertices_colored(
        -1.0,
        1.0,
        -1.0 + panel_w / width * 2.0,
        1.0 - panel_h / height * 2.0,
        [0.02, 0.03, 0.05, 0.72],
    );
    let panel_start = 0;
    let panel_count = panel.len() as u32;
    vertices.extend_from_slice(&panel);
    ranges.push(OverlayRange {
        start: panel_start,
        count: panel_count,
        atlas: false,
    });

    let atlas = engine::core::text::TextAtlas::built_in();
    let mut text_vertices: Vec<OverlayVertex> = Vec::new();
    let push_text = |text: &str, x: f32, y: f32, color: [f32; 4], out: &mut Vec<OverlayVertex>| {
        let style = engine::core::text::TextStyle {
            scale: SCALE,
            spacing: 1.0,
            line_spacing: 2.0,
            color,
        };
        if let Ok(mesh) = engine::core::text::TextLayout::build_bitmap(text, [x, y], style, &atlas)
        {
            for vertex in mesh.vertices {
                out.push(OverlayVertex {
                    position: [
                        vertex.position[0] / width * 2.0 - 1.0,
                        1.0 - vertex.position[1] / height * 2.0,
                    ],
                    uv: vertex.uv,
                    color: vertex.color,
                    sdf: 0,
                });
            }
        }
    };
    push_text(
        "PERF  (F3)",
        PAD_X,
        PAD_Y,
        [0.55, 1.0, 0.62, 1.0],
        &mut text_vertices,
    );

    // 量条：面板内每行底部一条 6px 高的槽 + 当前值填充。
    // 注意：这里算出来的是**像素**坐标，必须换算成 NDC 再交给 `rect_vertices_colored`
    // （否则量条会被画到屏幕外，只剩面板底和文字）。
    let to_ndc_x = |px: f32| px / width * 2.0 - 1.0;
    let to_ndc_y = |py: f32| 1.0 - py / height * 2.0;
    let bars_start = vertices.len() as u32;
    let bar_x0 = PAD_X + 74.0;
    let bar_x1 = panel_w - PAD_X;
    for (index, (_, value, ratio, _)) in rows.iter().enumerate() {
        let row_y = PAD_Y + 20.0 + index as f32 * ROW_H;
        let bar_y = row_y + 4.0;
        vertices.extend(rect_vertices_colored(
            to_ndc_x(bar_x0),
            to_ndc_y(row_y),
            to_ndc_x(bar_x1),
            to_ndc_y(bar_y + 6.0),
            [0.16, 0.18, 0.20, 0.85],
        ));
        let filled = bar_x0 + (bar_x1 - bar_x0) * ratio;
        let bar_color = if index == 0 {
            [0.25, 0.85, 0.45, 0.95]
        } else {
            [0.85, 0.70, 0.30, 0.95]
        };
        vertices.extend(rect_vertices_colored(
            to_ndc_x(bar_x0),
            to_ndc_y(row_y),
            to_ndc_x(filled),
            to_ndc_y(bar_y + 6.0),
            bar_color,
        ));
        // 标签在左、数值在右侧量条上方（标签列宽 9 字符，右对齐数值）。
        let label = format!("{:<9} {}", rows[index].0, value);
        push_text(
            &label,
            PAD_X,
            row_y - 3.0,
            [0.88, 0.92, 0.88, 1.0],
            &mut text_vertices,
        );
    }
    let bars_count = vertices.len() as u32 - bars_start;
    ranges.push(OverlayRange {
        start: bars_start,
        count: bars_count,
        atlas: false,
    });

    // 文字最后画（放在顶点缓冲末尾，保证覆盖在面板与量条之上）。
    let text_start = vertices.len() as u32;
    let text_count = text_vertices.len() as u32;
    vertices.extend(text_vertices);
    ranges.push(OverlayRange {
        start: text_start,
        count: text_count,
        atlas: true,
    });

    OverlayGeometry { vertices, ranges }
}

/// 6 顶点纯色矩形（NDC）。
fn rect_vertices_colored(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    color: [f32; 4],
) -> [OverlayVertex; 6] {
    let uv = [0.0, 0.0];
    let vertex = |position: [f32; 2]| OverlayVertex {
        position,
        uv,
        color,
        sdf: 0,
    };
    [
        vertex([x0, y0]),
        vertex([x1, y0]),
        vertex([x1, y1]),
        vertex([x0, y0]),
        vertex([x1, y1]),
        vertex([x0, y1]),
    ]
}

fn map_mouse_button(button: winit::event::MouseButton) -> MouseButton {
    match button {
        winit::event::MouseButton::Left => MouseButton::Left,
        winit::event::MouseButton::Right => MouseButton::Right,
        winit::event::MouseButton::Middle => MouseButton::Middle,
        winit::event::MouseButton::Other(value) => MouseButton::Other(value),
        winit::event::MouseButton::Back => MouseButton::Other(4),
        winit::event::MouseButton::Forward => MouseButton::Other(5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(fps: f32, frame_ms: f32, update_ms: f32, lua_ms: f32) -> PerformanceSnapshot {
        PerformanceSnapshot {
            fps,
            frame_ms,
            update_ms,
            lua_ms,
            frames: 1234,
        }
    }

    /// 调试 overlay 必须**带标签的文字 + 量条**，且文字用内置 5x7 字模图集绘制
    /// （`ranges` 里恰好一段 `atlas = true`），面板/量条走白色纹理。
    #[test]
    fn debug_overlay_draws_labeled_text_and_bars() {
        let overlay = build_overlay(snapshot(60.0, 8.0, 0.5, 0.2), 1280, 720);
        assert_eq!(overlay.ranges.len(), 3, "面板 / 量条 / 文字三段");
        assert!(!overlay.ranges[0].atlas, "面板底走白色纹理");
        assert!(!overlay.ranges[1].atlas, "量条走白色纹理");
        assert!(overlay.ranges[2].atlas, "文字走内置字模图集");
        // 文字段在顶点缓冲末尾（后画，覆盖在面板与量条上）。
        let text = &overlay.ranges[2];
        assert_eq!(
            (text.start + text.count) as usize,
            overlay.vertices.len(),
            "文字应位于顶点缓冲末尾"
        );
        // 面板与量条各段顶点数非零，且总数为 6 的倍数（三角形列表）。
        for range in &overlay.ranges {
            assert!(range.count > 0, "范围不得为空");
            assert_eq!(range.count % 6, 0, "顶点数应为 6 的倍数");
        }
        assert_eq!(overlay.vertices.len() % 6, 0);
        // 全部顶点落在 NDC 内（不会画到屏幕外）。
        for vertex in &overlay.vertices {
            assert!(
                (-1.0..=1.0).contains(&vertex.position[0]),
                "x 越界: {}",
                vertex.position[0]
            );
            assert!(
                (-1.0..=1.0).contains(&vertex.position[1]),
                "y 越界: {}",
                vertex.position[1]
            );
        }
    }

    /// 量条填充随数值单调变化：frame_ms 越大，填充越宽（同一面板布局下比较总宽度）。
    #[test]
    fn debug_overlay_bars_follow_metrics() {
        let light = build_overlay(snapshot(120.0, 0.5, 0.1, 0.05), 640, 360);
        let heavy = build_overlay(snapshot(30.0, 30.0, 15.0, 7.0), 640, 360);
        let width = |overlay: &OverlayGeometry| -> f32 {
            overlay.vertices[..overlay.ranges[1].count as usize]
                .iter()
                .map(|vertex| vertex.position[0])
                .sum()
        };
        // 同一套顶点顺序下，填充越宽 → 所有 x 之和越大。
        assert!(width(&heavy) > width(&light), "指标变差时量条填充应更宽");
    }
}
