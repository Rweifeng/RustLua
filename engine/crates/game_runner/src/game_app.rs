//! Application shell: owns the window, renderer, runtime scene and input
//! snapshot; drives the fixed-timestep simulation and frame redraw; maps
//! winit events to engine input.

use std::{path::PathBuf, sync::Arc, time::Instant};

use engine_core::{
    input::{InputKey, InputSnapshot, MouseButton},
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

use engine_host::{OverlayVertex, RenderOutcome, Renderer, RuntimeScene, SceneDraw, SceneMesh};

pub struct GameApp {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    lifecycle: AppLifecycle,
    occluded: bool,
    time_step: FixedTimeStep,
    input: InputSnapshot,
    last_frame: Instant,
    performance: PerformanceStats,
    overlay_enabled: bool,
    runtime_scene: Option<RuntimeScene>,
    project_root: Option<PathBuf>,
    /// CPU frustum culling (M8-08); off via `--no-cull` for baselines.
    cull_enabled: bool,
    /// Same mesh/material instancing (M8-08); off via `--no-instance`.
    instancing_enabled: bool,
}

impl GameApp {
    pub fn new(
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
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
            overlay_enabled: true,
            runtime_scene: None,
            project_root,
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
        if let Some(Err(error)) = result {
            let diagnostic = EngineError::new(
                LogModule::Script,
                "fixed-step update",
                "project Lua update function",
            )
            .with_source(&error)
            .with_suggestion("fix the Lua runtime error reported above");
            tracing::error!(target: "script", error = %diagnostic, "fixed-step update failed");
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
        let mut mesh = self
            .runtime_scene
            .as_mut()
            .and_then(|scene| {
                scene
                    .vertices(renderer.config.width, renderer.config.height)
                    .ok()
            })
            .unwrap_or_else(|| SceneMesh {
                vertices: Vec::new(),
                draws: Vec::new(),
                paths: Vec::new(),
                font_atlas: None,
            });
        if self.overlay_enabled {
            let start = mesh.vertices.len() as u32;
            mesh.vertices.extend(build_overlay_vertices(
                snapshot,
                renderer.config.width,
                renderer.config.height,
            ));
            let count = mesh.vertices.len() as u32 - start;
            if count > 0 {
                mesh.draws.push(SceneDraw {
                    texture: 0,
                    start,
                    count,
                });
            }
        }
        let aspect = renderer.config.width as f32 / renderer.config.height.max(1) as f32;
        let scene3d = self
            .runtime_scene
            .as_ref()
            .and_then(|scene| scene.scene_3d(aspect));
        match renderer.render(&mesh, scene3d.as_ref()) {
            RenderOutcome::Presented => {}
            RenderOutcome::Retry | RenderOutcome::Skipped => {}
            RenderOutcome::Fatal => self.request_exit(event_loop),
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
        let attributes = Window::default_attributes()
            .with_title("RustLua Engine - M1-07 Performance Overlay")
            .with_inner_size(PhysicalSize::new(1280, 720));
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
                self.runtime_scene = match RuntimeScene::new(self.project_root.as_deref()) {
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
                        self.request_exit(event_loop);
                        return;
                    }
                };
                self.transition(AppLifecycle::Running);
                self.last_frame = Instant::now();
                self.window
                    .as_ref()
                    .expect("window exists")
                    .request_redraw();
            }
            Err(error) => {
                tracing::error!(target: "engine", error = %EngineError::new(LogModule::Engine, "initialize renderer", "GPU renderer startup failed").with_source(&error).with_suggestion("update the graphics driver or use a compatible GPU"), "renderer initialization failed");
                self.window = Some(window);
                self.request_exit(event_loop);
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
                if let Some(key) = map_key(&event.logical_key) {
                    self.input
                        .key_event(key, event.state == ElementState::Pressed);
                }
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
fn build_overlay_vertices(
    snapshot: PerformanceSnapshot,
    width: u32,
    height: u32,
) -> Vec<OverlayVertex> {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let panel_width = (260.0 / width * 2.0).min(0.7);
    let panel_height = 72.0 / height * 2.0;
    let x0 = -1.0;
    let y0 = 1.0;
    let x1 = x0 + panel_width;
    let y1 = y0 - panel_height;
    let bars = [
        (snapshot.fps / 120.0).clamp(0.0, 1.0),
        (snapshot.frame_ms / 33.3).clamp(0.0, 1.0),
        (snapshot.update_ms / 16.7).clamp(0.0, 1.0),
        (snapshot.lua_ms / 8.0).clamp(0.0, 1.0),
    ];
    let mut vertices = rect_vertices(x0, y0, x1, y1).to_vec();
    for (index, value) in bars.into_iter().enumerate() {
        let top = y1 - 0.03 - index as f32 * 0.028;
        let right = x0 + 0.02 + (x1 - x0 - 0.04) * value;
        vertices.extend(rect_vertices(x0 + 0.02, top, right, top - 0.018));
    }
    vertices
}

fn rect_vertices(x0: f32, y0: f32, x1: f32, y1: f32) -> [OverlayVertex; 6] {
    let uv = [0.0, 0.0];
    [
        OverlayVertex {
            position: [x0, y0],
            uv,
            color: [0.05, 0.8, 0.35, 0.88],
        },
        OverlayVertex {
            position: [x1, y0],
            uv,
            color: [0.05, 0.8, 0.35, 0.88],
        },
        OverlayVertex {
            position: [x1, y1],
            uv,
            color: [0.05, 0.8, 0.35, 0.88],
        },
        OverlayVertex {
            position: [x0, y0],
            uv,
            color: [0.05, 0.8, 0.35, 0.88],
        },
        OverlayVertex {
            position: [x1, y1],
            uv,
            color: [0.05, 0.8, 0.35, 0.88],
        },
        OverlayVertex {
            position: [x0, y1],
            uv,
            color: [0.05, 0.8, 0.35, 0.88],
        },
    ]
}

fn map_key(key: &winit::keyboard::Key) -> Option<InputKey> {
    match key {
        winit::keyboard::Key::Character(value) => match value.as_str() {
            "w" | "W" => Some(InputKey::W),
            "a" | "A" => Some(InputKey::A),
            "s" | "S" => Some(InputKey::S),
            "d" | "D" => Some(InputKey::D),
            "r" | "R" => Some(InputKey::Other(99)), // 重玩一局
            _ => None,
        },
        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Space) => Some(InputKey::Space),
        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) => Some(InputKey::Escape),
        winit::keyboard::Key::Named(winit::keyboard::NamedKey::F1) => Some(InputKey::Other(1)),
        winit::keyboard::Key::Named(winit::keyboard::NamedKey::F2) => Some(InputKey::Other(2)),
        winit::keyboard::Key::Named(winit::keyboard::NamedKey::F4) => Some(InputKey::Other(4)),
        _ => None,
    }
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
