//! GPU renderer: device/surface/pipelines, texture upload, depth/shadow
//! resources, 2D/3D render passes, and M8-08 culling/instancing. The scene
//! layer produces [`SceneMesh`]/[`Scene3d`] frames; this module consumes them.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use engine_core::{
    camera3d::Camera3D,
    light::Light,
    logging::{EngineError, LogModule},
    math3d::{Mat4, Vec3},
    mesh3d::{GROUND_PLANE_SENTINEL, MeshAsset},
    text::TextAtlas,
    texture::{TextureAsset, TextureColorSpace},
    window::has_renderable_size,
};
use wgpu::util::DeviceExt;
use wgpu::{
    CompositeAlphaMode, Device, DeviceDescriptor, Instance, InstanceDescriptor, PresentMode, Queue,
    Surface, SurfaceConfiguration, SurfaceError, TextureUsages,
};
use winit::{dpi::PhysicalSize, window::Window};

use crate::{
    frame::{
        ATLAS_TEXTURE_PATH, FONT_ATLAS_TEXTURE_PATH, FontAtlasPixels, OverlayVertex, Scene3d,
        SceneMesh, WHITE_TEXTURE_PATH,
    },
    shader::{MESH3D_SHADER, SHADOW_SHADER, TEXTURED_SHADER},
};

/// A texture uploaded to the GPU and ready to sample.
struct LoadedTexture {
    path: String,
    bind_group: wgpu::BindGroup,
}

/// Shared vertex/index buffers for one mesh primitive, cached by mesh path.
#[derive(Clone)]
struct GpuGeometry {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

/// A decoded base-color texture uploaded to the GPU (sRGB) plus its view.
struct CachedAlbedoTexture {
    /// Kept alive so `view` stays valid; never read directly.
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// File-authored material parameters for one primitive. The per-draw model
/// matrix is supplied through the instance buffer instead.
#[derive(Clone, Copy)]
struct GpuMaterial {
    base_color: [f32; 4],
    metallic: f32,
    roughness: f32,
    /// Whether the owning entity has a skinned `Animation3D` palette.
    skinned: bool,
    /// Index into `Renderer::albedo_textures`; `None` samples a white fallback.
    albedo: Option<u32>,
}

/// Cached GPU geometry + file materials for one mesh path.
struct CachedMesh {
    primitives: Vec<CachedPrimitive>,
}

struct CachedPrimitive {
    geometry: GpuGeometry,
    base_color: [f32; 4],
    metallic: f32,
    roughness: f32,
    /// Index into `Renderer::albedo_textures`; `None` samples a white fallback.
    albedo: Option<u32>,
    /// Local-space bounding sphere (center, radius) used for frustum culling.
    bounds: (Vec3, f32),
}
/// A fully-resolved frame draw: cached geometry plus per-entity material, the
/// joint palette (empty for static meshes), and per-instance model matrices.
struct FrameDraw {
    geometry: GpuGeometry,
    material: GpuMaterial,
    palette: Vec<Mat4>,
    models: Vec<Mat4>,
}

/// Static drawables sharing one mesh path + primitive, accumulated for a
/// single instanced draw call.
struct StaticGroup {
    geometry: GpuGeometry,
    material: GpuMaterial,
    models: Vec<Mat4>,
}

/// 一个可复用的非阻塞回读槽：GPU 拷贝目标 buffer + 提交序号 + 映射完成标志。
struct ReadbackSlot {
    buffer: wgpu::Buffer,
    /// 最近提交的拷贝序号；`None` 表示槽空闲（已读完并 unmap，可重新提交）。
    submission: Option<wgpu::SubmissionIndex>,
    /// `map_async` 完成回调置位（跨线程，主线程轮询读取）。
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
    width: u32,
    height: u32,
    row_bytes: u32,
    bytes_per_row: u32,
    /// Reusable compact CPU output storage.
    pixels: Vec<u8>,
}

/// 非阻塞离屏回读的结果。
pub enum ReadbackStatus {
    /// 就绪的紧凑 RGBA8 像素（每行 `row_bytes`，无行尾填充），附实际尺寸。
    /// 尺寸可能与当前渲染尺寸不同（排空的是上一帧不同分辨率的槽）。
    Ready {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// 尚未就绪（主线程未阻塞等待 GPU）；调用方沿用上一帧纹理即可。
    Pending,
    /// 回读失败。
    Failed(String),
}

/// Largest axis scale of a model matrix (used to inflate culling spheres).
fn max_model_scale(model: Mat4) -> f32 {
    let sx =
        model.columns[0][0].powi(2) + model.columns[0][1].powi(2) + model.columns[0][2].powi(2);
    let sy =
        model.columns[1][0].powi(2) + model.columns[1][1].powi(2) + model.columns[1][2].powi(2);
    let sz =
        model.columns[2][0].powi(2) + model.columns[2][1].powi(2) + model.columns[2][2].powi(2);
    sx.max(sy).max(sz).sqrt().max(f32::EPSILON)
}
pub struct Renderer {
    /// Present surface in window mode; `None` in offscreen (embedded) mode.
    surface: Option<Surface<'static>>,
    device: Device,
    queue: Queue,
    pub config: SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    camera_bind_group: wgpu::BindGroup,
    shadow_bind_group: wgpu::BindGroup,
    shadow_pipeline: wgpu::RenderPipeline,
    depth_view: wgpu::TextureView,
    /// Current depth-view size; rebuilt when the render target changes size.
    depth_size: (u32, u32),
    shadow_view: wgpu::TextureView,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    textures: Vec<LoadedTexture>,
    white: Option<wgpu::BindGroup>,
    atlas: Option<wgpu::BindGroup>,
    font_atlas_texture: Option<wgpu::Texture>,
    font_atlas_bind: Option<wgpu::BindGroup>,
    /// SDF text bind group (Linear sampler); used by `draw.sdf` ranges.
    font_atlas_sdf_bind: Option<wgpu::BindGroup>,
    /// Linear sampler for SDF distance fields (text only).
    sdf_sampler: wgpu::Sampler,
    /// Pipeline for SDF text draws (Linear sampling + SDF shader branch).
    pipeline_sdf: wgpu::RenderPipeline,
    font_atlas_uploaded_version: u64,
    project_root: Option<PathBuf>,
    /// Camera uniform buffer, rewritten every frame from the active camera.
    camera_buffer: wgpu::Buffer,
    /// Directional light uniform buffer, rewritten every frame.
    light_buffer: wgpu::Buffer,
    /// Light-space camera uniform buffer, rewritten every frame from the light.
    shadow_camera_buffer: wgpu::Buffer,
    /// Shared identity joint palette bound by non-skinned draws so the shader's
    /// skinning branch is a no-op without a per-draw allocation.
    identity_palette: wgpu::Buffer,
    /// GPU geometry + file materials cached by mesh path.
    mesh_cache: HashMap<String, CachedMesh>,
    /// Base-color textures uploaded from glTF materials (sRGB), indexed by
    /// `GpuMaterial::albedo`. Index 0 is reserved for the white fallback.
    albedo_textures: Vec<CachedAlbedoTexture>,
    /// Shared sampler for base-color textures.
    albedo_sampler: wgpu::Sampler,
    /// 1x1 white texture view bound when a material has no base-color texture.
    white_albedo_view: wgpu::TextureView,
    /// CPU frustum culling of scene drawables (M8-08).
    cull_enabled: bool,
    /// Grouping of same mesh/material draws into instanced calls (M8-08).
    instancing_enabled: bool,
    /// Frames rendered since startup, for periodic baseline logging.
    frame_count: u64,
    _material_bind_group_layout: wgpu::BindGroupLayout,
    /// 非阻塞回读槽（离屏纹理 → CPU），避免主线程阻塞等待 GPU。
    readback_slots: Vec<ReadbackSlot>,
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> Result<Self, String> {
        pollster::block_on(Self::new_async(
            Some(window),
            None,
            project_root,
            cull_enabled,
            instancing_enabled,
        ))
    }

    /// Creates a renderer with no window surface, using the caller-provided
    /// device/queue (e.g. the editor's eframe device). Rendering goes through
    /// [`Renderer::render_into_texture`].
    pub fn new_offscreen(
        device: Device,
        queue: Queue,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> Result<Self, String> {
        pollster::block_on(Self::new_async(
            None,
            Some((device, queue)),
            project_root,
            cull_enabled,
            instancing_enabled,
        ))
    }

    async fn new_async(
        window: Option<Arc<Window>>,
        external_device: Option<(Device, Queue)>,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> Result<Self, String> {
        // Surface mode: create instance/adapter/device from the window.
        // Offscreen mode: use the caller-provided device/queue.
        let (device, queue, surface, format, config, adapter_name) = if let Some(window) = window {
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
            (device, queue, Some(surface), format, config, adapter_name)
        } else {
            let (device, queue) = external_device
                .ok_or_else(|| "offscreen renderer requires a device/queue".to_owned())?;
            let format = wgpu::TextureFormat::Rgba8UnormSrgb;
            let config = SurfaceConfiguration {
                usage: TextureUsages::RENDER_ATTACHMENT,
                format,
                width: 1280,
                height: 720,
                present_mode: PresentMode::Fifo,
                alpha_mode: CompositeAlphaMode::Auto,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            (device, queue, None, format, config, "offscreen".to_owned())
        };
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
        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("p2-camera3d-bind-group-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(64),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(32),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(64),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                        count: None,
                    },
                ],
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
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 32,
                            shader_location: 3,
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
        // SDF 文本需要 Linear 采样（距离场插值）；精灵保持 Nearest。
        let sdf_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("p2-sdf-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline_sdf = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("p2-textured-sdf-pipeline"),
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
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 32,
                            shader_location: 3,
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
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("p2-vertex-buffer"),
            size: 1024 * 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let material_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("p2-material3d-bind-group-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(32),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(
                                (engine_core::mesh3d::MAX_BONES * 64) as u64,
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(64),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let camera = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            config.width as f32 / config.height as f32,
            0.1,
            100.0,
        )
        .map_err(|error| error.to_string())?;
        let camera_matrix = camera
            .view_projection_matrix()
            .map_err(|error| error.to_string())?;
        let light_camera =
            Camera3D::perspective(Vec3::new(4.0, 6.0, 4.0), Vec3::ZERO, 1.3, 1.0, 0.1, 30.0)
                .map_err(|error| error.to_string())?;
        let light_view = light_camera
            .view_matrix()
            .map_err(|error| error.to_string())?;
        let light_projection = Mat4 {
            columns: [
                [0.1, 0.0, 0.0, 0.0],
                [0.0, 0.1, 0.0, 0.0],
                [0.0, 0.0, -1.0 / 29.9, 0.0],
                [0.0, 0.0, -0.1 / 29.9, 1.0],
            ],
        };
        let light_matrix = light_projection * light_view;
        let shadow_view = Self::create_shadow_view(&device, 2048);
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("p2-shadow-comparison-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("p2-camera3d-buffer"),
            contents: bytemuck::cast_slice(&camera_matrix.columns),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("p2-directional-light-buffer"),
            contents: bytemuck::cast_slice(&[[0.4_f32, 0.8, 0.5, 0.0], [1.0_f32, 1.0, 1.0, 1.0]]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shadow_camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("p2-shadow-camera-buffer"),
            contents: bytemuck::cast_slice(&light_matrix.columns),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let identity_palette = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("p2-identity-joint-palette"),
            contents: bytemuck::cast_slice(&identity_palette_data()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let albedo_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("p2-albedo-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // 1x1 white texture bound when a material has no base-color texture.
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p2-albedo-white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_albedo_view = white_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("p2-camera3d-bind-group"),
            layout: &camera_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: shadow_camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
            ],
        });
        let shadow_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("p2-shadow-bind-group-layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(64),
                    },
                    count: None,
                }],
            });
        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("p2-shadow-bind-group"),
            layout: &shadow_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_camera_buffer.as_entire_binding(),
            }],
        });
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("p2-mesh3d-shader"),
            source: wgpu::ShaderSource::Wgsl(MESH3D_SHADER.into()),
        });
        let mesh_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("p2-mesh3d-pipeline-layout"),
            bind_group_layouts: &[&camera_bind_group_layout, &material_bind_group_layout],
            push_constant_ranges: &[],
        });
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("p2-mesh3d-pipeline"),
            layout: Some(&mesh_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &mesh_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<engine_core::mesh3d::MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 24,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32x4,
                            offset: 32,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 48,
                            shader_location: 4,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &mesh_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("p2-shadow-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("p2-shadow-pipeline-layout"),
                bind_group_layouts: &[&shadow_bind_group_layout, &material_bind_group_layout],
                push_constant_ranges: &[],
            });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("p2-shadow-pipeline"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<engine_core::mesh3d::MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 24,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32x4,
                            offset: 32,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 48,
                            shader_location: 4,
                        },
                    ],
                }],
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let depth_view = Self::create_depth_view(&device, config.width, config.height);
        let depth_init = (config.width, config.height);
        tracing::info!(target: "game",
            "[render] initialized: adapter={adapter_name} format={format:?} present_mode={:?}",
            config.present_mode
        );
        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            mesh_pipeline,
            camera_bind_group,
            shadow_bind_group,
            shadow_pipeline,
            depth_view,
            depth_size: depth_init,
            shadow_view,
            texture_bind_group_layout,
            sampler,
            vertex_buffer,
            textures: Vec::new(),
            white: None,
            atlas: None,
            font_atlas_texture: None,
            font_atlas_bind: None,
            font_atlas_sdf_bind: None,
            sdf_sampler,
            pipeline_sdf,
            font_atlas_uploaded_version: u64::MAX,
            project_root,
            camera_buffer,
            light_buffer,
            shadow_camera_buffer,
            identity_palette,
            mesh_cache: HashMap::new(),
            albedo_textures: Vec::new(),
            albedo_sampler,
            white_albedo_view,
            cull_enabled,
            instancing_enabled,
            frame_count: 0,
            _material_bind_group_layout: material_bind_group_layout,
            readback_slots: Vec::new(),
        })
    }

    fn create_depth_view(device: &Device, width: u32, height: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("p2-depth32-texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    fn create_shadow_view(device: &Device, size: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("p2-shadow-map"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Returns the cached geometry + materials for a mesh path, loading it from
    /// the project on first use. Missing or unloadable meshes are skipped with
    /// a warning so a bad reference never crashes the renderer.
    fn cached_mesh(&mut self, path: &str) -> Option<&CachedMesh> {
        if !self.mesh_cache.contains_key(path)
            && let Some(cached) = self.load_cached_mesh(path)
        {
            self.mesh_cache.insert(path.to_owned(), cached);
            tracing::info!(target: "render", path = %path, "[render] 3D mesh cached");
        }
        self.mesh_cache.get(path)
    }

    fn load_cached_mesh(&mut self, path: &str) -> Option<CachedMesh> {
        if path == GROUND_PLANE_SENTINEL {
            return Some(self.ground_plane_cached_mesh());
        }
        let root = self.project_root.as_ref()?;
        let file = root.join(path);
        if !file.is_file() {
            tracing::warn!(target: "render", path = %path, "[render] 3D mesh file missing; skipped");
            return None;
        }
        let meshes = match MeshAsset::from_gltf_path(&file) {
            Ok(meshes) => meshes,
            Err(error) => {
                tracing::warn!(target: "render", path = %path, error = %error, "[render] 3D mesh load failed; skipped");
                return None;
            }
        };
        let mut primitives = Vec::new();
        for mesh in &meshes {
            for primitive in &mesh.primitives {
                let material = mesh.materials.get(primitive.material.unwrap_or(usize::MAX));
                let albedo = material
                    .and_then(|value| value.base_color_image.as_ref())
                    .map(|image| self.upload_albedo(&image.rgba, image.width, image.height));
                primitives.push(CachedPrimitive {
                    geometry: self.upload_geometry(&primitive.vertices, &primitive.indices),
                    base_color: material
                        .map(|value| value.base_color_factor)
                        .unwrap_or([0.55, 0.7, 1.0, 1.0]),
                    metallic: material.map(|value| value.metallic_factor).unwrap_or(0.0),
                    roughness: material.map(|value| value.roughness_factor).unwrap_or(0.6),
                    albedo,
                    bounds: compute_mesh_bounds(&primitive.vertices),
                });
            }
        }
        if primitives.is_empty() {
            tracing::warn!(target: "render", path = %path, "[render] 3D mesh has no primitives; skipped");
            return None;
        }
        Some(CachedMesh { primitives })
    }

    /// Uploads decoded base-color pixels (RGBA8) as an sRGB GPU texture and
    /// returns its index in `albedo_textures`.
    fn upload_albedo(&mut self, rgba: &[u8], width: u32, height: u32) -> u32 {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p2-albedo-texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width.max(1)),
                rows_per_image: Some(height.max(1)),
            },
            wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.albedo_textures
            .push(CachedAlbedoTexture { texture, view });
        tracing::info!(
            target: "render",
            index = self.albedo_textures.len() - 1,
            width,
            height,
            "[render] 3D albedo texture uploaded"
        );
        (self.albedo_textures.len() - 1) as u32
    }

    /// Synthesizes the built-in ground receiver plane for the
    /// `__ground_plane__` MeshRenderer sentinel so shadows have a surface.
    fn ground_plane_cached_mesh(&self) -> CachedMesh {
        let vertices = [
            engine_core::mesh3d::MeshVertex {
                position: [-4.0, 0.0, -4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [0.0, 0.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            engine_core::mesh3d::MeshVertex {
                position: [4.0, 0.0, -4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [1.0, 0.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            engine_core::mesh3d::MeshVertex {
                position: [4.0, 0.0, 4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [1.0, 1.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            engine_core::mesh3d::MeshVertex {
                position: [-4.0, 0.0, 4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [0.0, 1.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
        ];
        CachedMesh {
            primitives: vec![CachedPrimitive {
                geometry: self.upload_geometry(&vertices, &[0, 2, 1, 0, 3, 2]),
                base_color: [0.25, 0.3, 0.38, 1.0],
                metallic: 0.0,
                roughness: 0.9,
                albedo: None,
                bounds: compute_mesh_bounds(&vertices),
            }],
        }
    }

    fn upload_geometry(
        &self,
        vertices: &[engine_core::mesh3d::MeshVertex],
        indices: &[u32],
    ) -> GpuGeometry {
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("p2-mesh3d-vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("p2-mesh3d-indices"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        GpuGeometry {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        }
    }

    /// Creates a material bind group carrying one primitive's file material,
    /// the joint palette (identity for static meshes, animated for skinned
    /// drawables), and the per-instance model matrices.
    fn material_bind_group(
        &self,
        material: GpuMaterial,
        palette: &[Mat4],
        models: &[Mat4],
    ) -> wgpu::BindGroup {
        let mut material_data = Vec::with_capacity(2);
        material_data.push(material.base_color);
        material_data.push([
            material.metallic,
            material.roughness,
            if material.skinned { 1.0 } else { 0.0 },
            0.0,
        ]);
        let material_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("p2-material3d-frame-buffer"),
                contents: bytemuck::cast_slice(&material_data),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let palette_buffer = if material.skinned && !palette.is_empty() {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("p2-joint-palette-frame-buffer"),
                    contents: bytemuck::cast_slice(&joint_palette_data(palette)),
                    usage: wgpu::BufferUsages::UNIFORM,
                })
        } else {
            self.identity_palette.clone()
        };
        let mut instance_data = Vec::with_capacity(models.len() * 4);
        for model in models {
            instance_data.extend_from_slice(&model.columns);
        }
        let instance_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("p2-instance-models-frame-buffer"),
                contents: bytemuck::cast_slice(&instance_data),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let albedo = material
            .albedo
            .and_then(|index| self.albedo_textures.get(index as usize))
            .map_or(&self.white_albedo_view, |cached| &cached.view);
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("p2-material3d-frame-bind-group"),
            layout: &self._material_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: material_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: palette_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(albedo),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.albedo_sampler),
                },
            ],
        })
    }

    /// Rewrites the camera, light, and light-space uniform buffers from the
    /// active scene's camera and directional light.
    fn update_3d_uniforms(&mut self, scene: &Scene3d) {
        if let Ok(matrix) = scene.camera.view_projection_matrix() {
            self.queue.write_buffer(
                &self.camera_buffer,
                0,
                bytemuck::cast_slice(&matrix.columns),
            );
        }
        let (direction, color_intensity) = scene.light.uniform();
        self.queue.write_buffer(
            &self.light_buffer,
            0,
            bytemuck::cast_slice(&[direction, color_intensity]),
        );
        let light_matrix = light_view_projection(scene.light);
        self.queue.write_buffer(
            &self.shadow_camera_buffer,
            0,
            bytemuck::cast_slice(&light_matrix.columns),
        );
    }

    /// Gathers this frame's 3D draws: every scene entity with both a
    /// `Transform3D` and a `MeshRenderer`, CPU-culled against the camera
    /// frustum, resolved to cached geometry. Static drawables sharing a mesh
    /// path and primitive are grouped into one instanced draw when enabled.
    fn frame_3d_draws(&mut self, scene: &Scene3d) -> Vec<FrameDraw> {
        let frustum = if self.cull_enabled {
            scene.camera.frustum().ok()
        } else {
            None
        };
        let mut groups: HashMap<(String, usize), StaticGroup> = HashMap::new();
        let mut draws: Vec<FrameDraw> = Vec::new();
        let instancing_enabled = self.instancing_enabled;
        for drawable in &scene.drawables {
            let Some(cached) = self.cached_mesh(&drawable.mesh_path) else {
                continue;
            };
            for (prim_index, primitive) in cached.primitives.iter().enumerate() {
                if let Some(frustum) = frustum {
                    let world_center = drawable
                        .model
                        .transform_point(primitive.bounds.0)
                        .unwrap_or(primitive.bounds.0);
                    let world_radius = primitive.bounds.1 * max_model_scale(drawable.model);
                    if !frustum.contains_sphere(world_center, world_radius) {
                        continue;
                    }
                }
                let material = GpuMaterial {
                    base_color: primitive.base_color,
                    metallic: primitive.metallic,
                    roughness: primitive.roughness,
                    skinned: !drawable.palette.is_empty(),
                    albedo: primitive.albedo,
                };
                if drawable.palette.is_empty() && instancing_enabled {
                    groups
                        .entry((drawable.mesh_path.clone(), prim_index))
                        .or_insert_with(|| StaticGroup {
                            geometry: primitive.geometry.clone(),
                            material,
                            models: Vec::new(),
                        })
                        .models
                        .push(drawable.model);
                } else {
                    draws.push(FrameDraw {
                        geometry: primitive.geometry.clone(),
                        material,
                        palette: drawable.palette.clone(),
                        models: vec![drawable.model],
                    });
                }
            }
        }
        for group in groups.into_values() {
            draws.push(FrameDraw {
                geometry: group.geometry,
                material: group.material,
                palette: Vec::new(),
                models: group.models,
            });
        }
        // Periodic M8-08 baseline: instance/draw counts with culling and
        // instancing active, useful for comparing against --no-cull/--no-instance.
        self.frame_count = self.frame_count.wrapping_add(1);
        if self.frame_count.is_multiple_of(120) {
            let instances: usize = draws.iter().map(|draw| draw.models.len()).sum();
            tracing::info!(
                target: "render",
                draws = draws.len(),
                instances,
                cull = self.cull_enabled,
                instancing = self.instancing_enabled,
                "[render] 3D baseline draws/instances"
            );
        }
        draws
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if !has_renderable_size(size.width, size.height) {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        self.ensure_depth_view(size.width, size.height);
        tracing::info!(target: "game",
            "[render] surface reconfigured: {}x{}",
            size.width, size.height
        );
    }

    /// Rebuilds the depth view when the render target size changes (window
    /// resize or an offscreen target of a different size).
    fn ensure_depth_view(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.depth_size != (width, height) {
            self.depth_view = Self::create_depth_view(&self.device, width, height);
            self.depth_size = (width, height);
        }
    }

    pub fn render(&mut self, mesh: &SceneMesh, scene3d: Option<&Scene3d>) -> RenderOutcome {
        let Some(surface) = &self.surface else {
            tracing::warn!(target: "game", "[render] render() requires a window surface; use render_into_texture");
            return RenderOutcome::Skipped;
        };
        let output = match surface.get_current_texture() {
            Ok(output) => output,
            Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                surface.configure(&self.device, &self.config);
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
        self.render_passes(&mut encoder, mesh, scene3d, &view);
        self.queue.submit([encoder.finish()]);
        output.present();
        RenderOutcome::Presented
    }

    /// Renders the current frame into an offscreen texture view (no present).
    /// The caller owns the target texture and supplies its view + dimensions;
    /// the depth attachment is rebuilt to match the target size.
    pub fn render_into_texture(
        &mut self,
        mesh: &SceneMesh,
        scene3d: Option<&Scene3d>,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> RenderOutcome {
        self.ensure_depth_view(width, height);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("p2-offscreen-encoder"),
            });
        self.render_passes(&mut encoder, mesh, scene3d, view);
        self.queue.submit([encoder.finish()]);
        RenderOutcome::Presented
    }

    /// Copies an RGBA8 texture back to the CPU (synchronous readback), used by
    /// the editor to display the embedded preview. The texture must have
    /// `COPY_SRC` usage.
    pub fn copy_texture_to_cpu(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        // `bytes_per_row` must be a multiple of 256 for `copy_texture_to_buffer`.
        let row_bytes = 4 * width;
        let bytes_per_row = row_bytes.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("p2-preview-readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("p2-preview-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let map_result = rx
            .recv()
            .map_err(|error| format!("preview readback channel error: {error}"))?;
        map_result.map_err(|error| format!("preview texture map failed: {error}"))?;
        // 去掉行尾填充，返回紧凑 RGBA8（每行 `row_bytes`）。
        let padded = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((row_bytes * height) as usize);
        for row in 0..height as usize {
            let start = row * bytes_per_row as usize;
            pixels.extend_from_slice(&padded[start..start + row_bytes as usize]);
        }
        Ok(pixels)
    }

    /// 非阻塞离屏回读：总是提交一次纹理拷贝（写进环中下一空闲槽），同时尝试
    /// 读取之前提交的结果。就绪返回 `Ready(pixels)`，未就绪返回 `Pending`——
    /// 主线程永不阻塞等待 GPU。纹理须有 `COPY_SRC` usage。
    pub fn readback_frame(
        &mut self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> ReadbackStatus {
        // 先排空任意就绪槽（滞后一帧的结果）。
        let mut ready = None;
        for index in 0..self.readback_slots.len() {
            if let Some(result) = self.try_finish_readback(index) {
                ready = Some(result);
                break;
            }
        }
        // 无论是否排空到结果，都提交本次拷贝，保证每个渲染帧都会被回读。
        self.submit_readback(texture, width, height);
        match ready {
            Some((pixels, width, height)) => ReadbackStatus::Ready {
                pixels,
                width,
                height,
            },
            None => ReadbackStatus::Pending,
        }
    }

    pub fn readback_slot_count(&self) -> usize {
        self.readback_slots.len()
    }

    pub fn readback_busy_count(&self) -> usize {
        self.readback_slots
            .iter()
            .filter(|slot| slot.submission.is_some())
            .count()
    }

    /// 是否有在途（已提交未读）的回读等待排空。
    pub fn readback_in_flight(&self) -> bool {
        self.readback_slots
            .iter()
            .any(|slot| slot.submission.is_some())
    }

    /// 仅排空待读的回读（内容未变、无需新提交时调用），返回就绪像素。
    pub fn readback_pending(&mut self) -> ReadbackStatus {
        for index in 0..self.readback_slots.len() {
            if let Some((pixels, width, height)) = self.try_finish_readback(index) {
                return ReadbackStatus::Ready {
                    pixels,
                    width,
                    height,
                };
            }
        }
        ReadbackStatus::Pending
    }

    /// 非阻塞检查一个回读槽：映射完成则读出行数据并置回空闲，返回紧凑像素与尺寸。
    fn try_finish_readback(&mut self, index: usize) -> Option<(Vec<u8>, u32, u32)> {
        let slot = self.readback_slots.get(index)?;
        slot.submission.as_ref()?;
        // 非阻塞 maintain：处理所有已完成的提交与 `map_async` 回调（超时即返回，
        // 主线程不阻塞）。就绪与否由 `ready` 标志判断。
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::ZERO),
        });
        if !slot.ready.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        let (width, height, row_bytes, bytes_per_row) =
            (slot.width, slot.height, slot.row_bytes, slot.bytes_per_row);
        let buffer = slot.buffer.clone();
        let padded = buffer.slice(..).get_mapped_range();
        let mut pixels = std::mem::take(&mut self.readback_slots[index].pixels);
        pixels.clear();
        pixels.reserve((row_bytes * height) as usize);
        for row in 0..height as usize {
            let start = row * bytes_per_row as usize;
            pixels.extend_from_slice(&padded[start..start + row_bytes as usize]);
        }
        drop(padded);
        buffer.unmap();
        // 槽置回空闲，允许复用。
        let slot = &mut self.readback_slots[index];
        slot.submission = None;
        slot.ready
            .store(false, std::sync::atomic::Ordering::Release);
        Some((pixels, width, height))
    }

    /// 向一个空闲回读槽提交离屏纹理 → staging buffer 的拷贝（buffer 尺寸不符时重建）。
    fn submit_readback(&mut self, texture: &wgpu::Texture, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let row_bytes = 4 * width;
        let bytes_per_row = row_bytes.div_ceil(256);
        let bytes_per_row = bytes_per_row * 256;
        let size_bytes = (bytes_per_row * height) as u64;
        tracing::debug!(target: "render", width, height, "[readback] submit");
        // 惰性创建双缓冲槽。
        while self.readback_slots.len() < 2 {
            self.readback_slots.push(ReadbackSlot {
                buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("p2-editor-readback"),
                    size: size_bytes.max(4),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                submission: None,
                ready: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                width,
                height,
                row_bytes,
                bytes_per_row,
                pixels: Vec::with_capacity((row_bytes * height) as usize),
            });
        }
        // 找一个空闲槽（无在途提交）。
        let Some(index) = self
            .readback_slots
            .iter()
            .position(|slot| slot.submission.is_none())
        else {
            return; // 全部在途，跳过本次提交
        };
        // 尺寸不符时重建 buffer，避免越界写入。
        let slot = &mut self.readback_slots[index];
        if slot.width != width || slot.height != height || slot.buffer.size() < size_bytes {
            slot.buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("p2-editor-readback"),
                size: size_bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            slot.width = width;
            slot.height = height;
            slot.row_bytes = row_bytes;
            slot.bytes_per_row = bytes_per_row;
        }
        let buffer = slot.buffer.clone();
        let ready = slot.ready.clone();
        slot.ready
            .store(false, std::sync::atomic::Ordering::Release);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("p2-editor-readback-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        // 先提交拷贝，再请求映射（与同步回读一致：map_async 会把 buffer 标记为
        // mapped-pending，若在 copy 提交前调用会触发 "Buffer is still mapped"）。
        let submission = self.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |_result| {
                ready.store(true, std::sync::atomic::Ordering::Release);
            });
        slot.submission = Some(submission);
    }

    /// Shared pass rendering into a given color view (window surface or
    /// offscreen target): clears the view, draws 3D (shadow + mesh into the
    /// depth view), then the 2D overlay.
    fn render_passes(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        mesh: &SceneMesh,
        scene3d: Option<&Scene3d>,
        view: &wgpu::TextureView,
    ) {
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("p2-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
        let frame_draws = if let Some(scene) = scene3d {
            self.update_3d_uniforms(scene);
            self.frame_3d_draws(scene)
        } else {
            Vec::new()
        };
        if !frame_draws.is_empty() {
            {
                let mut shadow_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("p2-shadow-pass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.shadow_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                shadow_pass.set_pipeline(&self.shadow_pipeline);
                shadow_pass.set_bind_group(0, &self.shadow_bind_group, &[]);
                for draw in &frame_draws {
                    let bind = self.material_bind_group(draw.material, &draw.palette, &draw.models);
                    shadow_pass.set_bind_group(1, &bind, &[]);
                    shadow_pass.set_vertex_buffer(0, draw.geometry.vertex_buffer.slice(..));
                    shadow_pass.set_index_buffer(
                        draw.geometry.index_buffer.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    shadow_pass.draw_indexed(
                        0..draw.geometry.index_count,
                        0,
                        0..draw.models.len() as u32,
                    );
                }
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("p2-mesh3d-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.mesh_pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            for draw in &frame_draws {
                let bind = self.material_bind_group(draw.material, &draw.palette, &draw.models);
                pass.set_bind_group(1, &bind, &[]);
                pass.set_vertex_buffer(0, draw.geometry.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    draw.geometry.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(0..draw.geometry.index_count, 0, 0..draw.models.len() as u32);
            }
        }
        if !mesh.vertices.is_empty() {
            let bytes = bytemuck::cast_slice(&mesh.vertices);
            if bytes.len() as u64 <= self.vertex_buffer.size() {
                self.queue.write_buffer(&self.vertex_buffer, 0, bytes);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("p2-scene-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
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
                if let Some(font) = &mesh.font_atlas {
                    let _ = self.ensure_font_atlas(font);
                }
                tracing::debug!(target: "render", sdf_draws = mesh.draws.iter().filter(|d| d.sdf).count(), font_bind = self.font_atlas_sdf_bind.is_some(), "[render] sdf draws");
                for draw in &mesh.draws {
                    if draw.sdf {
                        // SDF 文本：Linear 采样 + SDF shader 分支。
                        let Some(sdf_bind) = &self.font_atlas_sdf_bind else {
                            continue;
                        };
                        pass.set_pipeline(&self.pipeline_sdf);
                        pass.set_bind_group(0, sdf_bind, &[]);
                        pass.draw(draw.start..draw.start + draw.count, 0..1);
                        continue;
                    }
                    let Some(bind) = self.bind_group_for(draw.texture, &mesh.paths) else {
                        continue;
                    };
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &bind, &[]);
                    pass.draw(draw.start..draw.start + draw.count, 0..1);
                }
            }
        }
    }

    /// Returns the bind group for a draw: the white texture for `texture == 0`
    /// or the white sentinel path, the built-in glyph atlas for the atlas
    /// sentinel path, otherwise the cached (or freshly loaded) asset at
    /// `paths[texture - 1]`.
    fn bind_group_for(&mut self, texture: u32, paths: &[String]) -> Option<wgpu::BindGroup> {
        if texture == 0 {
            return self.ensure_white();
        }
        let path = &paths[texture as usize - 1];
        if path == WHITE_TEXTURE_PATH {
            return self.ensure_white();
        }
        if path == ATLAS_TEXTURE_PATH {
            return self.ensure_atlas();
        }
        if path == FONT_ATLAS_TEXTURE_PATH {
            return self.font_atlas_bind.clone();
        }
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

    fn ensure_atlas(&mut self) -> Option<wgpu::BindGroup> {
        if let Some(atlas) = &self.atlas {
            return Some(atlas.clone());
        }
        let built_in = TextAtlas::built_in();
        let asset = TextureAsset::from_rgba8(
            built_in.width(),
            built_in.height(),
            built_in.pixels().to_vec(),
            TextureColorSpace::Srgb,
        )
        .ok()?;
        let bind = self.upload_texture(&asset, "p2-builtin-text-atlas")?;
        self.atlas = Some(bind.clone());
        Some(bind)
    }

    /// Uploads the CJK font atlas texture when its version changed.
    fn ensure_font_atlas(&mut self, font: &FontAtlasPixels) -> Option<wgpu::BindGroup> {
        if font.version == self.font_atlas_uploaded_version || font.pixels.is_empty() {
            return self.font_atlas_bind.clone();
        }
        // SDF 距离场不能走 sRGB 解码（会非线性化距离斜坡），用 Rgba8Unorm。
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let texture = if self.font_atlas_texture.as_ref().is_some_and(|texture| {
            let size = texture.size();
            size.width == font.width && size.height == font.height
        }) {
            self.font_atlas_texture.as_ref().unwrap().clone()
        } else {
            self.font_atlas_bind = None;
            self.font_atlas_sdf_bind = None;
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("p2-font-atlas"),
                size: wgpu::Extent3d {
                    width: font.width,
                    height: font.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.font_atlas_texture = Some(texture.clone());
            texture
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &font.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(font.width * 4),
                rows_per_image: Some(font.height),
            },
            wgpu::Extent3d {
                width: font.width,
                height: font.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("p2-font-atlas-bind"),
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
        self.font_atlas_uploaded_version = font.version;
        self.font_atlas_bind = Some(bind_group.clone());
        // SDF 文本用 Linear 采样绑定（同一纹理，不同采样器）。
        let sdf_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("p2-font-atlas-sdf-bind"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sdf_sampler),
                },
            ],
        });
        self.font_atlas_sdf_bind = Some(sdf_bind);
        Some(bind_group)
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

/// Conservative local-space bounding sphere from a primitive's vertex AABB.
fn compute_mesh_bounds(vertices: &[engine_core::mesh3d::MeshVertex]) -> (Vec3, f32) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for vertex in vertices {
        for index in 0..3 {
            min[index] = min[index].min(vertex.position[index]);
            max[index] = max[index].max(vertex.position[index]);
        }
    }
    if !min[0].is_finite() {
        return (Vec3::ZERO, 0.0);
    }
    let center = Vec3::new(
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    );
    let half = Vec3::new(
        (max[0] - min[0]) * 0.5,
        (max[1] - min[1]) * 0.5,
        (max[2] - min[2]) * 0.5,
    );
    let radius = half.length().max(f32::EPSILON);
    (center, radius)
}

/// Builds the joint-palette uniform data padded to `MAX_BONES` identity
/// matrices: `[f32; 4]` rows so `bytemuck` casts the whole buffer at once.
fn joint_palette_data(palette: &[Mat4]) -> Vec<[f32; 4]> {
    let mut data = Vec::with_capacity(engine_core::mesh3d::MAX_BONES * 4);
    for matrix in palette.iter().take(engine_core::mesh3d::MAX_BONES) {
        data.extend_from_slice(&matrix.columns);
    }
    while data.len() < engine_core::mesh3d::MAX_BONES * 4 {
        data.extend_from_slice(&IDENTITY_MAT4);
    }
    data
}

const IDENTITY_MAT4: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn identity_palette_data() -> Vec<[f32; 4]> {
    joint_palette_data(&[])
}

/// Light-space view-projection used by the shadow map: an orthographic-style
/// projection from a camera placed along the light direction, looking at the
/// origin. Mirrors the fixed matrix used by the M8-05 shadow pass.
fn light_view_projection(light: Light) -> Mat4 {
    let direction = light
        .direction
        .normalized()
        .unwrap_or(Vec3::new(0.0, -1.0, 0.0));
    let Ok(camera) = Camera3D::perspective(direction * 12.0, Vec3::ZERO, 1.3, 1.0, 0.1, 30.0)
    else {
        return Mat4::identity();
    };
    let Ok(view) = camera.view_matrix() else {
        return Mat4::identity();
    };
    let projection = Mat4 {
        columns: [
            [0.1, 0.0, 0.0, 0.0],
            [0.0, 0.1, 0.0, 0.0],
            [0.0, 0.0, -1.0 / 29.9, 0.0],
            [0.0, 0.0, -0.1 / 29.9, 1.0],
        ],
    };
    projection * view
}

pub enum RenderOutcome {
    Presented,
    Retry,
    Skipped,
    Fatal,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders an empty frame to an offscreen texture and reads it back,
    /// proving the offscreen path (`render_into_texture` +
    /// `copy_texture_to_cpu`) produces real pixels. Skips when no GPU is
    /// available.
    #[test]
    fn offscreen_renderer_produces_pixels() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("p2-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
            .expect("offscreen renderer");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p2-test-preview"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mesh = crate::frame::SceneMesh {
            vertices: Vec::new(),
            draws: Vec::new(),
            paths: Vec::new(),
            font_atlas: None,
        };
        let outcome = renderer.render_into_texture(&mesh, None, &view, 64, 64);
        assert!(matches!(outcome, RenderOutcome::Presented));
        let pixels = renderer
            .copy_texture_to_cpu(&texture, 64, 64)
            .expect("preview readback");
        assert_eq!(pixels.len(), 64 * 64 * 4);
        // Clear color is (0.04, 0.07, 0.12); the frame must not be all zero.
        assert!(
            pixels.iter().any(|&byte| byte > 0),
            "offscreen frame is blank"
        );
    }

    /// Non-blocking readback (`readback_frame`) eventually yields the rendered
    /// pixels without a blocking `device.poll(Wait)` on the first call. Skips
    /// when no GPU is available.
    #[test]
    fn non_blocking_readback_eventually_yields_pixels() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            return; // no GPU available; skip
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("p2-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
            .expect("offscreen renderer");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p2-test-readback"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mesh = crate::frame::SceneMesh {
            vertices: Vec::new(),
            draws: Vec::new(),
            paths: Vec::new(),
            font_atlas: None,
        };
        renderer.render_into_texture(&mesh, None, &view, 64, 64);
        // 首帧提交；随后的调用非阻塞轮询，直到返回就绪像素（不应无限阻塞）。
        let mut result = renderer.readback_frame(&texture, 64, 64);
        for _ in 0..120 {
            match &result {
                ReadbackStatus::Ready {
                    pixels,
                    width,
                    height,
                } => {
                    assert_eq!((*width, *height), (64, 64));
                    assert_eq!(pixels.len(), 64 * 64 * 4);
                    assert!(
                        pixels.iter().any(|&byte| byte > 0),
                        "offscreen frame is blank"
                    );
                    return;
                }
                ReadbackStatus::Pending => {
                    // 未就绪：等 GPU 完成拷贝（真实编辑器帧间隔远大于此），再轮询。
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    result = renderer.readback_frame(&texture, 64, 64);
                }
                ReadbackStatus::Failed(error) => panic!("readback failed: {error}"),
            }
        }
        panic!("readback never became ready within 120 polls");
    }

    /// Reproduces the editor's `create_preview` path against the real game
    /// project: instantiate `main.scene`, step, build frames, render offscreen
    /// and read back. Requires the game project beside the engine workspace.
    #[test]
    fn editor_preview_path_renders_game_scene() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
            .ok()
        else {
            panic!("无法获取用于编辑器预览测试的 wgpu adapter");
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("p2-test-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
            .expect("offscreen renderer");
        let game = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("game");
        let scene_text =
            std::fs::read_to_string(game.join("scenes/main.scene")).expect("read main.scene");
        let scene = engine_core::scene::Scene::from_text(&scene_text).expect("parse main.scene");
        let mut runtime = crate::runtime_scene::RuntimeScene::from_scene(Some(&game), scene)
            .expect("instantiate runtime");
        runtime
            .run_script_steps(1, 1.0 / 60.0, &engine_core::input::InputSnapshot::default())
            .expect("step runtime");
        let mesh = runtime.vertices(640, 360).expect("build 2D mesh");
        let scene3d = runtime.scene_3d(640.0 / 360.0);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p2-test-preview"),
            size: wgpu::Extent3d {
                width: 640,
                height: 360,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        renderer.render_into_texture(&mesh, scene3d.as_ref(), &view, 640, 360);
        let pixels = renderer
            .copy_texture_to_cpu(&texture, 640, 360)
            .expect("readback");
        assert_eq!(pixels.len(), 640 * 360 * 4);
        assert!(
            pixels.iter().any(|&byte| byte > 0),
            "preview frame is blank"
        );
    }
}
