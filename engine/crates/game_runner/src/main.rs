use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use bytemuck::{Pod, Zeroable};
use engine_core::{
    input::{InputKey, InputSnapshot, MouseButton},
    logging::{EngineError, LogModule, init_logging},
    math2d::{Camera2D, Viewport},
    performance::{PerformanceSnapshot, PerformanceStats},
    scene::{Scene, SceneEntityId},
    script::LuaRuntime,
    sprite::{Sprite, SpriteVertex},
    sprite_batch::{SpriteBatch, SpriteBatchItem},
    tilemap::{TILE_PALETTE, Tilemap},
    time_step::FixedTimeStep,
    transform::{Transform, Vec2, integrate_velocity},
    window::{AppLifecycle, has_renderable_size},
};
use wgpu::{
    CompositeAlphaMode, Device, DeviceDescriptor, Instance, InstanceDescriptor, PresentMode, Queue,
    Surface, SurfaceConfiguration, SurfaceError, TextureUsages,
};
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct OverlayVertex {
    position: [f32; 2],
    color: [f32; 4],
}

const OVERLAY_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};
@vertex fn vs_main(@location(0) position: vec2<f32>, @location(1) color: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.color = color;
    return out;
}
@fragment fn fs_main(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return color;
}
"#;

use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

struct Renderer {
    surface: Surface<'static>,
    device: Device,
    queue: Queue,
    config: SurfaceConfiguration,
    overlay_pipeline: wgpu::RenderPipeline,
    overlay_vertex_buffer: wgpu::Buffer,
}

impl Renderer {
    fn new(window: Arc<Window>) -> Result<Self, String> {
        pollster::block_on(Self::new_async(window))
    }

    async fn new_async(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = Instance::new(&InstanceDescriptor::default());
        let surface = instance
            .create_surface(window)
            .map_err(|error| format!("could not create surface: {error}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| format!("could not find a compatible GPU adapter: {error}"))?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("p2-render-device"),
                required_features: wgpu::Features::empty(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| format!("could not create GPU device: {error}"))?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| "GPU surface reported no supported formats".to_owned())?;
        let present_mode = if capabilities.present_modes.contains(&PresentMode::Mailbox) {
            PresentMode::Mailbox
        } else {
            PresentMode::Fifo
        };
        let alpha_mode = capabilities
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(CompositeAlphaMode::Auto);
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("p2-overlay-shader"),
            source: wgpu::ShaderSource::Wgsl(OVERLAY_SHADER.into()),
        });
        let overlay_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("p2-overlay-pipeline-layout"),
                bind_group_layouts: &[],
                push_constant_ranges: &[],
            });
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("p2-overlay-pipeline"),
            layout: Some(&overlay_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<OverlayVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 8,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let overlay_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("p2-overlay-vertex-buffer"),
            size: 1024 * 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        tracing::info!(target: "game",
            "[render] initialized: adapter={adapter_name} format={format:?} present_mode={present_mode:?}"
        );
        Ok(Self {
            surface,
            device,
            queue,
            config,
            overlay_pipeline,
            overlay_vertex_buffer,
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if !has_renderable_size(size.width, size.height) {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        tracing::info!(target: "game",
            "[render] surface reconfigured: {}x{}",
            size.width, size.height
        );
    }

    fn render(&mut self, overlay: Option<&[OverlayVertex]>) -> RenderOutcome {
        let output = match self.surface.get_current_texture() {
            Ok(output) => output,
            Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                tracing::info!(target: "game", "[render] surface reconfigured after transient error");
                return RenderOutcome::Retry;
            }
            Err(SurfaceError::Timeout) => {
                tracing::info!(target: "game", "[render] surface acquire timed out; skipping frame");
                return RenderOutcome::Skipped;
            }
            Err(SurfaceError::OutOfMemory) => {
                tracing::error!(target: "engine", error = %EngineError::new(LogModule::Engine, "render frame", "GPU is out of memory").with_suggestion("close other GPU applications or reduce resource usage"), "GPU out of memory; requesting shutdown");
                return RenderOutcome::Fatal;
            }
            Err(SurfaceError::Other) => {
                tracing::info!(target: "game", "[render] surface acquire failed with an unknown error");
                return RenderOutcome::Skipped;
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("p2-clear-encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("p2-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.04,
                            g: 0.07,
                            b: 0.12,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        if let Some(vertices) = overlay.filter(|vertices| !vertices.is_empty()) {
            let bytes = bytemuck::cast_slice(vertices);
            if bytes.len() as u64 <= self.overlay_vertex_buffer.size() {
                self.queue
                    .write_buffer(&self.overlay_vertex_buffer, 0, bytes);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("p2-overlay-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.overlay_pipeline);
                pass.set_vertex_buffer(0, self.overlay_vertex_buffer.slice(..));
                pass.draw(0..vertices.len() as u32, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        output.present();
        RenderOutcome::Presented
    }
}

enum RenderOutcome {
    Presented,
    Retry,
    Skipped,
    Fatal,
}

struct RuntimeScene {
    script: LuaRuntime,
    sprite_batch: SpriteBatch,
    camera: Camera2D,
}

impl RuntimeScene {
    fn new(project_root: Option<&Path>) -> Result<Self, String> {
        let scene = load_runtime_scene(project_root);
        let mut script = LuaRuntime::new().map_err(|error| error.to_string())?;
        let instantiated = script
            .instantiate_scene(&scene)
            .map_err(|error| error.to_string())?;
        tracing::info!(target: "scene", "[scene] loaded name={} entities={}", scene.name, scene.entities.len());
        tracing::info!(target: "ecs", "[ecs] world initialized entities={instantiated}");
        if let Some(root) = project_root {
            let script_path = root.join("scripts/main.lua");
            if script_path.is_file() {
                script
                    .execute_project_script(&script_path)
                    .map_err(|error| error.to_string())?;
            } else {
                tracing::info!(target: "script", path = %script_path.display(), "[script] game project has no scripts/main.lua; script runtime idle");
            }
        }
        let camera =
            Camera2D::new(Vec2::new(640.0, 360.0), 1.0).map_err(|error| error.to_string())?;
        tracing::info!(target: "render", "[render] SpriteBatch and Camera2D initialized");
        Ok(Self {
            script,
            sprite_batch: SpriteBatch::new(),
            camera,
        })
    }

    fn run_script_steps(
        &mut self,
        steps: u32,
        delta_seconds: f32,
        input: &InputSnapshot,
    ) -> Result<(), String> {
        self.script.set_delta(delta_seconds);
        for _ in 0..steps {
            self.script.set_input(input.clone());
            let result = self
                .script
                .call_update()
                .map_err(|error| error.to_string())?;
            tracing::debug!(target: "game", "[simulation] fixed update returned: {result}");
        }
        self.script.with_world_mut(|world| {
            let _ = integrate_velocity(world, delta_seconds * steps as f32);
        });
        Ok(())
    }

    fn vertices(&mut self, width: u32, height: u32) -> Result<Vec<OverlayVertex>, String> {
        let viewport = Viewport::new(width.max(1) as f32, height.max(1) as f32)
            .map_err(|error| error.to_string())?;
        self.sprite_batch.clear();
        let sprites = self.script.with_world(|world| {
            world
                .entities_with2::<Transform, Sprite>()
                .into_iter()
                .filter_map(|id| {
                    let transform = world.get::<Transform>(id).ok().flatten().copied();
                    let sprite = world.get::<Sprite>(id).ok().flatten().copied();
                    transform.zip(sprite)
                })
                .collect::<Vec<_>>()
        });
        // Tile maps draw first (layer 0), sprite entities on top (layer 1+).
        self.push_tiles()?;
        for (index, (transform, sprite)) in sprites.into_iter().enumerate() {
            self.sprite_batch
                .push(SpriteBatchItem::new(0, 1 + index as i32, sprite, transform));
        }
        let mesh = self
            .sprite_batch
            .build(self.camera, viewport)
            .map_err(|error| error.to_string())?;
        tracing::debug!(
            target: "render",
            "[render] batch items={} vertices={} draw_ranges={}",
            self.sprite_batch.len(),
            mesh.vertices.len(),
            mesh.draws.len()
        );
        Ok(mesh
            .vertices
            .iter()
            .map(|vertex| sprite_vertex(vertex, viewport))
            .collect())
    }

    /// Pushes every non-empty tile as a single-batch colored quad. All tiles
    /// share texture id and layer, so the whole map is one draw range.
    fn push_tiles(&mut self) -> Result<(), String> {
        let tilemaps = self.script.with_world(|world| {
            world
                .entities_with2::<Transform, Tilemap>()
                .into_iter()
                .filter_map(|id| {
                    let transform = world.get::<Transform>(id).ok().flatten().copied();
                    let tilemap = world.get::<Tilemap>(id).ok().flatten().cloned();
                    transform.zip(tilemap)
                })
                .collect::<Vec<_>>()
        });
        for (transform, tilemap) in tilemaps {
            let base = Sprite::new(tilemap.tile_size).map_err(|error| error.to_string())?;
            for y in 0..tilemap.height {
                for x in 0..tilemap.width {
                    let index = y * tilemap.width + x;
                    let cell = tilemap.tiles[index];
                    if cell == 0 {
                        continue;
                    }
                    let tint = TILE_PALETTE[(cell as usize - 1) % TILE_PALETTE.len()];
                    let sprite = base.with_tint(tint).map_err(|error| error.to_string())?;
                    let offset = Vec2::new(
                        x as f32 * tilemap.tile_size.x * transform.scale.x,
                        y as f32 * tilemap.tile_size.y * transform.scale.y,
                    );
                    let tile_transform = Transform {
                        position: Vec2::new(
                            transform.position.x + offset.x,
                            transform.position.y + offset.y,
                        ),
                        rotation: transform.rotation,
                        scale: transform.scale,
                    };
                    self.sprite_batch
                        .push(SpriteBatchItem::new(0, 0, sprite, tile_transform));
                }
            }
        }
        Ok(())
    }
}

fn load_runtime_scene(project_root: Option<&Path>) -> Scene {
    project_root
        .map(|root| root.join("scenes/main.scene"))
        .and_then(|path| Scene::load(path).ok())
        // Standalone engine mode uses this engine-owned diagnostic scene and never
        // probes the current directory for game files.
        .unwrap_or_else(|| {
            let mut scene = Scene::new("runtime-main");
            let _ = scene.add_entity(SceneEntityId(1), "SpriteRoot", None);
            scene
                .entities
                .get_mut(&SceneEntityId(1))
                .unwrap()
                .components
                .insert(
                    "Transform".into(),
                    "x=640;y=360;rotation=0;scale_x=1;scale_y=1".into(),
                );
            scene
                .entities
                .get_mut(&SceneEntityId(1))
                .unwrap()
                .components
                .insert("Sprite".into(), "assets/red-1x1.png".into());
            scene
        })
}

fn sprite_vertex(vertex: &SpriteVertex, viewport: Viewport) -> OverlayVertex {
    OverlayVertex {
        position: [
            vertex.position[0] / viewport.width * 2.0 - 1.0,
            1.0 - vertex.position[1] / viewport.height * 2.0,
        ],
        color: vertex.color,
    }
}

struct GameApp {
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
}

impl GameApp {
    fn new(project_root: Option<PathBuf>) -> Self {
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
        self.input.finish_frame();
        let snapshot = self.performance.snapshot();
        let mut vertices = self
            .runtime_scene
            .as_mut()
            .and_then(|scene| {
                scene
                    .vertices(renderer.config.width, renderer.config.height)
                    .ok()
            })
            .unwrap_or_default();
        if self.overlay_enabled {
            vertices.extend(build_overlay_vertices(
                snapshot,
                renderer.config.width,
                renderer.config.height,
            ));
        }
        match renderer.render((!vertices.is_empty()).then_some(vertices.as_slice())) {
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
            .with_title("P2 RustLua Engine - M1-07 Performance Overlay")
            .with_inner_size(PhysicalSize::new(1280, 720));
        let window = Arc::new(event_loop.create_window(attributes).expect("create window"));
        tracing::info!(target: "game", "[window] created: {:?}", window.id());
        match Renderer::new(window.clone()) {
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
    [
        OverlayVertex {
            position: [x0, y0],
            color: [0.05, 0.8, 0.35, 0.88],
        },
        OverlayVertex {
            position: [x1, y0],
            color: [0.05, 0.8, 0.35, 0.88],
        },
        OverlayVertex {
            position: [x1, y1],
            color: [0.05, 0.8, 0.35, 0.88],
        },
        OverlayVertex {
            position: [x0, y0],
            color: [0.05, 0.8, 0.35, 0.88],
        },
        OverlayVertex {
            position: [x1, y1],
            color: [0.05, 0.8, 0.35, 0.88],
        },
        OverlayVertex {
            position: [x0, y1],
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
            _ => None,
        },
        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Space) => Some(InputKey::Space),
        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) => Some(InputKey::Escape),
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
#[derive(Debug, Default)]
struct Arguments {
    project: Option<PathBuf>,
    log_filter: Option<String>,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut args = Arguments::default();
    let mut values = env::args().skip(1);
    while let Some(value) = values.next() {
        match value.as_str() {
            "--project" => {
                args.project = Some(PathBuf::from(
                    values.next().ok_or("--project requires a path")?,
                ))
            }
            "--log-filter" => {
                args.log_filter = Some(
                    values
                        .next()
                        .ok_or("--log-filter requires a tracing filter")?,
                )
            }
            "--help" | "-h" => {
                println!(
                    "Usage: game_runner [--project <game-project-root>] [--log-filter <tracing-filter>]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = parse_arguments().map_err(|error| format!("argument error: {error}"))?;
    let log_path = [
        PathBuf::from("artifacts/logs/game-runner.log"),
        PathBuf::from("../artifacts/logs/game-runner.log"),
    ]
    .into_iter()
    .find(|path| path.parent().is_some_and(|parent| parent.is_dir()))
    .unwrap_or_else(|| PathBuf::from("artifacts/logs/game-runner.log"));
    init_logging(log_path, arguments.log_filter.as_deref())
        .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
    tracing::info!(target: "game", version = engine_core::version(), project = ?arguments.project, "game runner starting");
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut GameApp::new(arguments.project))?;
    Ok(())
}
