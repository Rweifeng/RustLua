use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use bytemuck::{Pod, Zeroable};
use engine_core::{
    animation::AnimationStateMachine,
    input::{InputKey, InputSnapshot, MouseButton},
    logging::{EngineError, LogModule, init_logging},
    math2d::{Camera2D, Viewport},
    performance::{PerformanceSnapshot, PerformanceStats},
    scene::{Scene, SceneEntityId},
    script::LuaRuntime,
    sprite::{Sprite, SpriteAsset, SpriteVertex},
    sprite_batch::{SpriteBatch, SpriteBatchItem},
    texture::{TextureAsset, TextureColorSpace},
    tilemap::Tilemap,
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
    uv: [f32; 2],
    color: [f32; 4],
}

const TEXTURED_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@vertex fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    return out;
}
@fragment fn fs_main(@location(0) uv: vec2<f32>, @location(1) color: vec4<f32>) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, uv) * color;
}
"#;

use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

/// One draw range over the scene vertex buffer, bound to a texture.
/// `texture == 0` selects the built-in white texture; `texture == i > 0`
/// selects `paths[i - 1]`.
struct SceneDraw {
    texture: u32,
    start: u32,
    count: u32,
}

/// Scene geometry plus the texture paths its draw ranges reference.
struct SceneMesh {
    vertices: Vec<OverlayVertex>,
    draws: Vec<SceneDraw>,
    paths: Vec<String>,
}

/// A texture uploaded to the GPU and ready to sample.
struct LoadedTexture {
    path: String,
    bind_group: wgpu::BindGroup,
}

struct Renderer {
    surface: Surface<'static>,
    device: Device,
    queue: Queue,
    config: SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    textures: Vec<LoadedTexture>,
    white: Option<wgpu::BindGroup>,
    project_root: Option<PathBuf>,
}

impl Renderer {
    fn new(window: Arc<Window>, project_root: Option<PathBuf>) -> Result<Self, String> {
        pollster::block_on(Self::new_async(window, project_root))
    }

    async fn new_async(window: Arc<Window>, project_root: Option<PathBuf>) -> Result<Self, String> {
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
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("p2-texture-bind-group-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("p2-textured-pipeline-layout"),
            bind_group_layouts: &[&texture_bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("p2-textured-shader"),
            source: wgpu::ShaderSource::Wgsl(TEXTURED_SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("p2-textured-pipeline"),
            layout: Some(&pipeline_layout),
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
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 16,
                            shader_location: 2,
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("p2-texture-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("p2-vertex-buffer"),
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
            pipeline,
            texture_bind_group_layout,
            sampler,
            vertex_buffer,
            textures: Vec::new(),
            white: None,
            project_root,
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

    fn render(&mut self, mesh: &SceneMesh) -> RenderOutcome {
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
        if !mesh.vertices.is_empty() {
            let bytes = bytemuck::cast_slice(&mesh.vertices);
            if bytes.len() as u64 <= self.vertex_buffer.size() {
                self.queue.write_buffer(&self.vertex_buffer, 0, bytes);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("p2-scene-pass"),
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
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                for draw in &mesh.draws {
                    let Some(bind) = self.bind_group_for(draw.texture, &mesh.paths) else {
                        continue;
                    };
                    pass.set_bind_group(0, &bind, &[]);
                    pass.draw(draw.start..draw.start + draw.count, 0..1);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        output.present();
        RenderOutcome::Presented
    }

    /// Returns the bind group for a draw: the white texture for `texture == 0`,
    /// otherwise the cached (or freshly loaded) asset at `paths[texture - 1]`.
    fn bind_group_for(&mut self, texture: u32, paths: &[String]) -> Option<wgpu::BindGroup> {
        if texture == 0 {
            return self.ensure_white();
        }
        let path = &paths[texture as usize - 1];
        if let Some(entry) = self.textures.iter().find(|entry| entry.path == *path) {
            return Some(entry.bind_group.clone());
        }
        let resolved = self
            .project_root
            .as_ref()
            .map(|root| root.join(path))
            .unwrap_or_else(|| PathBuf::from(path));
        let asset = match TextureAsset::load_png(&resolved, TextureColorSpace::Srgb) {
            Ok(asset) => asset,
            Err(error) => {
                tracing::warn!(target: "render", path = %path, error = %error, "[render] texture load failed; falling back to white");
                return self.ensure_white();
            }
        };
        let bind = self.upload_texture(&asset, path)?;
        self.textures.push(LoadedTexture {
            path: path.to_owned(),
            bind_group: bind.clone(),
        });
        tracing::info!(target: "render", path = %path, dimensions = ?(asset.width(), asset.height()), "[render] texture loaded");
        Some(bind)
    }

    fn ensure_white(&mut self) -> Option<wgpu::BindGroup> {
        if let Some(white) = &self.white {
            return Some(white.clone());
        }
        let asset =
            TextureAsset::from_rgba8(1, 1, vec![255, 255, 255, 255], TextureColorSpace::Srgb)
                .ok()?;
        let bind = self.upload_texture(&asset, "p2-white-1x1")?;
        self.white = Some(bind.clone());
        Some(bind)
    }

    fn upload_texture(&self, asset: &TextureAsset, label: &str) -> Option<wgpu::BindGroup> {
        let format = match asset.color_space() {
            TextureColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: asset.width(),
                height: asset.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            asset.pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(asset.bytes_per_row()),
                rows_per_image: Some(asset.height()),
            },
            wgpu::Extent3d {
                width: asset.width(),
                height: asset.height(),
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Some(bind_group)
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
    texture_paths: Vec<String>,
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
            texture_paths: Vec::new(),
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
            let elapsed = delta_seconds * steps as f32;
            for id in world.entities_with::<AnimationStateMachine>() {
                if let Ok(Some(machine)) = world.get_mut::<AnimationStateMachine>(id) {
                    machine.advance(elapsed);
                }
            }
        });
        Ok(())
    }

    fn vertices(&mut self, width: u32, height: u32) -> Result<SceneMesh, String> {
        let viewport = Viewport::new(width.max(1) as f32, height.max(1) as f32)
            .map_err(|error| error.to_string())?;
        self.sprite_batch.clear();
        self.texture_paths.clear();
        let sprites = self.script.with_world(|world| {
            world
                .entities_with::<Sprite>()
                .into_iter()
                .filter_map(|id| {
                    let transform = world.get::<Transform>(id).ok().flatten().copied()?;
                    let sprite = world.get::<Sprite>(id).ok().flatten().copied()?;
                    let animation = world
                        .get::<AnimationStateMachine>(id)
                        .ok()
                        .flatten()
                        .cloned();
                    let asset = world
                        .get::<SpriteAsset>(id)
                        .ok()
                        .flatten()
                        .map(|entry| entry.path().to_owned());
                    Some((transform, sprite, animation, asset))
                })
                .collect::<Vec<_>>()
        });
        // Tile maps draw first (layer 0), sprite entities on top (layer 1+).
        self.push_tiles()?;
        for (index, (transform, sprite, animation, asset)) in sprites.into_iter().enumerate() {
            let (texture, sprite) = if let Some(machine) = animation {
                // Animated sprite: sample the current clip's sheet at its frame UV.
                let (uv_min, uv_max) = machine.current_frame_uv();
                let sprite = sprite
                    .with_uv(uv_min, uv_max)
                    .map_err(|error| error.to_string())?;
                let sheet = machine.current_clip().sheet.clone();
                (self.texture_id(&sheet), sprite)
            } else if let Some(path) = asset {
                (self.texture_id(&path), sprite)
            } else {
                continue;
            };
            self.sprite_batch.push(SpriteBatchItem::new(
                texture,
                1 + index as i32,
                sprite,
                transform,
            ));
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
        let draws = mesh
            .draws
            .iter()
            .map(|draw| SceneDraw {
                texture: draw.texture_id + 1, // 0 is reserved for the white texture
                start: draw.vertex_start as u32,
                count: draw.vertex_count as u32,
            })
            .collect();
        let vertices = mesh
            .vertices
            .iter()
            .map(|vertex| sprite_vertex(vertex, viewport))
            .collect();
        Ok(SceneMesh {
            vertices,
            draws,
            paths: self.texture_paths.clone(),
        })
    }

    /// Deduplicates a texture asset path and returns its id for batch items.
    fn texture_id(&mut self, path: &str) -> u32 {
        if let Some(index) = self.texture_paths.iter().position(|entry| entry == path) {
            return index as u32;
        }
        self.texture_paths.push(path.to_owned());
        (self.texture_paths.len() - 1) as u32
    }

    /// Pushes every non-empty tile as a textured quad sampling its tileset
    /// column. All tiles of a map share texture id and layer, so the whole map
    /// is one draw range.
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
            let texture = self.texture_id(&tilemap.texture);
            for y in 0..tilemap.height {
                for x in 0..tilemap.width {
                    let index = y * tilemap.width + x;
                    let cell = tilemap.tiles[index];
                    if cell == 0 {
                        continue;
                    }
                    let (uv_min, uv_max) = Tilemap::tile_uv(cell);
                    let sprite = Sprite::new(tilemap.tile_size)
                        .map_err(|error| error.to_string())?
                        .with_uv(uv_min, uv_max)
                        .map_err(|error| error.to_string())?;
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
                    self.sprite_batch.push(SpriteBatchItem::new(
                        texture,
                        0,
                        sprite,
                        tile_transform,
                    ));
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
        uv: vertex.uv,
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
        match renderer.render(&mesh) {
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
        match Renderer::new(window.clone(), self.project_root.clone()) {
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
