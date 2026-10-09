//! `Renderer` 的构造侧（WP12 阶段 4d 从 `renderer/mod.rs` 原样搬出）：`new`、
//! `new_offscreen` 和 1,070 行的 `new_async`——适配器/设备/表面、七条管线、采样器与
//! 各张 GPU 缓冲的创建都在这里。
//!
//! 计划 §5 的硬规矩 2：**只搬位置，不拆函数体** ⇒ `new_async` 整段原样搬，它的分段
//! （管线、缓冲、时间戳）另开工作包（§9）。`Renderer` 结构体与全部字段仍留在
//! `renderer/mod.rs`（§3.1.2）；本模块是它的后代，结构体字面量按私有字段构造**不需要**
//! 放宽任何可见性。

use crate::core::{
    async_loader::{AsyncLoader, LoadMode},
    camera::{Camera3D, FrameViews, Viewport},
    math3d::{Mat4, Vec3},
    render_plan::RenderPlan,
};
use crate::host::{
    frame::{OverlayVertex, VectorTextVertex, WorldSpriteVertex},
    shader::{
        MESH3D_SHADER, POST_PROCESS_SHADER, SHADOW_SHADER, TEXTURED_SHADER, VECTOR_TEXT_SHADER,
        WORLD_SPRITE_SHADER,
    },
};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use wgpu::util::DeviceExt;
use wgpu::{
    CompositeAlphaMode, Device, DeviceDescriptor, Instance, InstanceDescriptor, PresentMode, Queue,
    SurfaceConfiguration, TextureUsages,
};
use winit::window::Window;

use super::world_sprite::WORLD_SPRITE_BUFFER_BYTES;
use super::{
    DepthPass, FRAME_ARENA_INITIAL_BYTES, GpuTimestampState, IDENTITY_MAT4,
    INITIAL_2D_VERTEX_BUFFER_BYTES, RenderFrameMetrics, Renderer, depth_stencil_state,
    identity_palette_data,
};

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> anyhow::Result<Self> {
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
    ) -> anyhow::Result<Self> {
        pollster::block_on(Self::new_async(
            None,
            Some((device, queue)),
            project_root,
            cull_enabled,
            instancing_enabled,
        ))
    }

    /// [`Self::new_offscreen`] 的**异步**版本，供没有阻塞能力的宿主使用（web）。
    ///
    /// 为什么需要另开一个入口：`new_offscreen` 用 `pollster::block_on` 驱动，而
    /// `wasm32-unknown-unknown` 上 `thread::park_timeout` 未实现——一旦 `new_async` 里
    /// 任何一个 await 点没有在首次 poll 就绪，它就会 panic。浏览器里正确的形态是**由调用方
    /// await**（wasm-bindgen 的 `async fn` 导出会变成 Promise，正好接上 JS 微任务）。
    /// 原生侧行为一字不变：`new_offscreen` 仍是同一段 `new_async`，只是这里把等待方式交给调用方。
    pub async fn new_offscreen_async(
        device: Device,
        queue: Queue,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> anyhow::Result<Self> {
        Self::new_async(
            None,
            Some((device, queue)),
            project_root,
            cull_enabled,
            instancing_enabled,
        )
        .await
    }

    async fn new_async(
        window: Option<Arc<Window>>,
        external_device: Option<(Device, Queue)>,
        project_root: Option<PathBuf>,
        cull_enabled: bool,
        instancing_enabled: bool,
    ) -> anyhow::Result<Self> {
        // P0-②：窗口渲染（游戏运行）走异步加载；离屏渲染（golden / 编辑器预览）走同步，
        // 保证确定性输出里绝无异步加载。
        let async_mode = if window.is_some() {
            LoadMode::Async
        } else {
            LoadMode::Sync
        };
        // Surface mode: create instance/adapter/device from the window.
        // Offscreen mode: use the caller-provided device/queue.
        let (device, queue, surface, format, config, adapter_name) = if let Some(window) = window {
            let size = window.inner_size();
            let instance = Instance::new(&InstanceDescriptor::default());
            let surface = instance
                .create_surface(window)
                .map_err(|error| anyhow::anyhow!("could not create surface: {error}"))?;
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await
                .map_err(|error| {
                    anyhow::anyhow!("could not find a compatible GPU adapter: {error}")
                })?;
            let adapter_name = adapter.get_info().name;
            let timestamp_features =
                wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
            let required_features = if adapter.features().contains(timestamp_features) {
                timestamp_features
            } else {
                wgpu::Features::empty()
            };
            let (device, queue) = adapter
                .request_device(&DeviceDescriptor {
                    label: Some("rustlua-render-device"),
                    required_features,
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                })
                .await
                .map_err(|error| anyhow::anyhow!("could not create GPU device: {error}"))?;
            let capabilities = surface.get_capabilities(&adapter);
            let format = capabilities
                .formats
                .iter()
                .copied()
                .find(|format| format.is_srgb())
                .or_else(|| capabilities.formats.first().copied())
                .ok_or_else(|| anyhow::anyhow!("GPU surface reported no supported formats"))?;
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
                .ok_or_else(|| anyhow::anyhow!("offscreen renderer requires a device/queue"))?;
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
        // 设备有没有存储缓冲（WebGL2 / 任何 downlevel 设备上都是 0）。构造路径上有三处要用它：
        // 矢量文字的两个绑定、3D 材质的实例矩阵、以及实例环。不该建的去建**不是**"功能少一点"，
        // 而是在 `create_bind_group_layout` 上直接吃一条校验错误——wgpu 对未被 error scope 捕获的
        // 校验错误走默认 handler，wasm 上是 `panic=abort`，整个渲染器就此死掉。
        // 所以这里按**设备能力**（而不是按平台 `cfg`）分叉：原生桌面照旧，web 走"没有这两条路"的构造。
        let storage_capable = device.limits().max_storage_buffers_per_shader_stage > 0;
        let storage_usage = if storage_capable {
            wgpu::BufferUsages::STORAGE
        } else {
            wgpu::BufferUsages::empty()
        };
        // 实例化本身也吃存储缓冲（逐实例模型矩阵），能力不足时把参数一并关掉：
        // 这是"同一件事的两条实现里选不需要存储缓冲的那条"，不是少画。
        let instancing_enabled = instancing_enabled && storage_capable;
        if !storage_capable {
            // 一次告警，而不是每帧：每帧的信号在 `dropped_draws` 上（少画东西而日志里一个字
            // 都没有，是本仓最不能接受的失败类型）。
            tracing::warn!(target: "game",
                "设备没有存储缓冲（max_storage_buffers_per_shader_stage=0，WebGL2 即如此）：\
                 矢量文字与 3D 网格/阴影两条路不构造，它们的 draw 会计入 dropped_draws");
        }
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-texture-bind-group-layout"),
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
            label: Some("rustlua-textured-pipeline-layout"),
            bind_group_layouts: &[&texture_bind_group_layout],
            push_constant_ranges: &[],
        });
        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-camera3d-bind-group-layout"),
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
            label: Some("rustlua-textured-shader"),
            source: wgpu::ShaderSource::Wgsl(TEXTURED_SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rustlua-textured-pipeline"),
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
            label: Some("rustlua-texture-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        // SDF 文本需要 Linear 采样（距离场插值）；精灵保持 Nearest。
        let sdf_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-sdf-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline_sdf = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rustlua-textured-sdf-pipeline"),
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
        // 矢量文字的两个绑定是**片元存储缓冲**，能力不足时这里就是空布局（合法），
        // 对应的管线也换成占位管线（见下面 `vector_pipeline`）。
        let mut vector_entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::new();
        if storage_capable {
            vector_entries.extend([
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ]);
        }
        let vector_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-vector-text-bind-group-layout"),
                entries: &vector_entries,
            });
        let vector_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rustlua-vector-text-pipeline-layout"),
            bind_group_layouts: &[&vector_bind_group_layout],
            push_constant_ranges: &[],
        });
        let vector_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-vector-text-shader"),
            source: wgpu::ShaderSource::Wgsl(VECTOR_TEXT_SHADER.into()),
        });
        let vector_pipeline = if storage_capable {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rustlua-vector-text-pipeline"),
                layout: Some(&vector_layout),
                vertex: wgpu::VertexState {
                    module: &vector_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<VectorTextVertex>() as u64,
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
                                format: wgpu::VertexFormat::Uint32,
                                offset: 16,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 20,
                                shader_location: 3,
                            },
                        ],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &vector_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        } else {
            placeholder_pipeline(&device, format, "rustlua-vector-text-placeholder-pipeline")
        };
        let vector_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-vector-text-vertex-buffer"),
            size: 1024 * 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vector_glyph_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-vector-text-glyph-buffer"),
            size: 64 * 1024,
            usage: storage_usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vector_segment_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-vector-text-segment-buffer"),
            size: 1024 * 1024,
            usage: storage_usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-vertex-buffer"),
            size: INITIAL_2D_VERTEX_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // 材质布局 5 个绑定里有 1 个是**存储缓冲**（`binding 2` = 逐实例模型矩阵，给实例化用）。
        // 能力不足时不放它：布局本身仍然合法（原生那条路完全不变），而依赖它的管线会走占位
        // （见下面的 `mesh_pipeline` / `shadow_pipeline`），3D 通道在 `passes.rs` 里也不再声明。
        let mut material_entries: Vec<wgpu::BindGroupLayoutEntry> = vec![
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
                        (crate::core::mesh3d::MAX_BONES * 64) as u64,
                    ),
                },
                count: None,
            },
        ];
        if storage_capable {
            material_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            });
        }
        material_entries.extend([
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
        ]);
        let material_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-material3d-bind-group-layout"),
                entries: &material_entries,
            });
        // 首帧 uniform 的初值：相机矩阵按**目标视口**算（宽高比只从视口来，拆分批 7）。
        let initial_viewport = Viewport::new(config.width as f32, config.height as f32)?;
        let camera = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )?;
        let camera_matrix = camera.view_projection_matrix(initial_viewport)?;
        let light_camera =
            Camera3D::perspective(Vec3::new(4.0, 6.0, 4.0), Vec3::ZERO, 1.3, 0.1, 30.0)?;
        let light_view = light_camera.view_matrix()?;
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
            label: Some("rustlua-shadow-comparison-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-camera3d-buffer"),
            contents: bytemuck::cast_slice(&camera_matrix.columns),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-directional-light-buffer"),
            // 与 `Light::default()` 一致：`direction` 是**光传播方向**，所以天空里的太阳 y 分量为负。
            contents: bytemuck::cast_slice(&[[0.4_f32, -0.8, 0.5, 0.0], [1.0_f32, 1.0, 1.0, 1.0]]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shadow_camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-shadow-camera-buffer"),
            contents: bytemuck::cast_slice(&light_matrix.columns),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let identity_palette = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-identity-joint-palette"),
            contents: bytemuck::cast_slice(&identity_palette_data()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // P0-①：帧级环形缓冲的容量与对齐来自设备限制，跨后端（DX12/Vulkan/Metal/GL）都成立。
        let uniform_align = device.limits().min_uniform_buffer_offset_alignment.max(1) as u64;
        let storage_align = device.limits().min_storage_buffer_offset_alignment.max(1) as u64;
        let material_arena = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-material3d-frame-arena"),
            size: FRAME_ARENA_INITIAL_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_arena = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-instance-models-frame-arena"),
            size: FRAME_ARENA_INITIAL_BYTES,
            usage: storage_usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let albedo_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-albedo-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // 1x1 white texture bound when a material has no base-color texture.
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-albedo-white"),
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
            label: Some("rustlua-camera3d-bind-group"),
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
                label: Some("rustlua-shadow-bind-group-layout"),
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
            label: Some("rustlua-shadow-bind-group"),
            layout: &shadow_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_camera_buffer.as_entire_binding(),
            }],
        });
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-mesh3d-shader"),
            source: wgpu::ShaderSource::Wgsl(MESH3D_SHADER.into()),
        });
        let mesh_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rustlua-mesh3d-pipeline-layout"),
            bind_group_layouts: &[&camera_bind_group_layout, &material_bind_group_layout],
            push_constant_ranges: &[],
        });
        let mesh_pipeline = if storage_capable {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rustlua-mesh3d-pipeline"),
                layout: Some(&mesh_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &mesh_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<crate::core::mesh3d::MeshVertex>() as u64,
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
                depth_stencil: Some(depth_stencil_state(
                    DepthPass::Mesh3d,
                    wgpu::DepthBiasState::default(),
                )),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        } else {
            // 3D 网格管线的着色器无条件声明了 `instance_models` 存储绑定，而布局在能力不足时
            // 不含它 ⇒ 真管线建不出来（校验错误）。占位管线永远不该被绑定：
            // `passes.rs` 在能力不足时不声明 3D 通道，并把丢弃的 draw 记进 `dropped_draws`。
            placeholder_pipeline(&device, format, "rustlua-mesh3d-placeholder-pipeline")
        };
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-shadow-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rustlua-shadow-pipeline-layout"),
                bind_group_layouts: &[&shadow_bind_group_layout, &material_bind_group_layout],
                push_constant_ranges: &[],
            });
        let shadow_pipeline = if storage_capable {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rustlua-shadow-pipeline"),
                layout: Some(&shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<crate::core::mesh3d::MeshVertex>() as u64,
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
                depth_stencil: Some(depth_stencil_state(
                    DepthPass::Shadow,
                    wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 2.0,
                        clamp: 0.0,
                    },
                )),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        } else {
            // 见 `mesh_pipeline`：阴影着色器同样声明了实例矩阵存储绑定，能力不足时走占位。
            placeholder_pipeline(&device, format, "rustlua-shadow-placeholder-pipeline")
        };
        let depth_view = Self::create_depth_view(&device, config.width, config.height);
        let depth_init = (config.width, config.height);
        let timestamp_enabled = device.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        );
        let timestamp_query_set = timestamp_enabled.then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("rustlua-frame-timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        let timestamp_period_ns = queue.get_timestamp_period();
        // WP6 世界 Sprite 通道：独立的一套资源（2D 相机 uniform + 顶点缓冲 + 深度附件）。
        // 与 overlay 路径互不影响：宿主不喂世界顶点时它一次也不会被用到。
        let world_sprite_camera_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-world-sprite-camera-layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let world_sprite_camera_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-world-sprite-camera"),
                contents: bytemuck::bytes_of(&IDENTITY_MAT4),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let world_sprite_camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-world-sprite-camera-bind-group"),
            layout: &world_sprite_camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: world_sprite_camera_buffer.as_entire_binding(),
            }],
        });
        let world_sprite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-world-sprite-shader"),
            source: wgpu::ShaderSource::Wgsl(WORLD_SPRITE_SHADER.into()),
        });
        let world_sprite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rustlua-world-sprite-pipeline-layout"),
                bind_group_layouts: &[&world_sprite_camera_layout, &texture_bind_group_layout],
                push_constant_ranges: &[],
            });
        // 世界 Sprite 的两条管线只差深度策略：不透明写深度（可与网格互相遮挡），半透明
        // 测深度不写深度（画家顺序叠加）。除 `DepthPass` 外必须逐字段相同，否则"同一份几何
        // 换一个策略"就会变成两条不同的渲染路径。
        let create_world_sprite_pipeline = |label: &str, pass: DepthPass| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&world_sprite_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &world_sprite_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<WorldSpriteVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 12,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 20,
                                shader_location: 2,
                            },
                        ],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &world_sprite_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(depth_stencil_state(pass, wgpu::DepthBiasState::default())),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let world_sprite_pipeline =
            create_world_sprite_pipeline("rustlua-world-sprite-pipeline", DepthPass::WorldSprite);
        let world_sprite_opaque_pipeline = create_world_sprite_pipeline(
            "rustlua-world-sprite-opaque-pipeline",
            DepthPass::WorldSpriteOpaque,
        );
        let world_sprite_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-world-sprite-buffer"),
            size: WORLD_SPRITE_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let world_sprite_depth_view = Self::create_depth_view(&device, 1, 1);
        // docs/54 §4.3：后处理通道的管线与资源。宿主不打开时这些都只是闲置对象（不产生 pass、
        // 不产生离屏纹理），因此"没开后处理"的运行时代价与收敛前一致。
        let post_process_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-post-process-shader"),
            source: wgpu::ShaderSource::Wgsl(POST_PROCESS_SHADER.into()),
        });
        let post_process_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-post-process-bind-group-layout"),
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let post_process_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rustlua-post-process-pipeline-layout"),
                bind_group_layouts: &[&post_process_layout],
                push_constant_ranges: &[],
            });
        let post_process_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rustlua-post-process-pipeline"),
                layout: Some(&post_process_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &post_process_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    // 全屏三角形由 `@builtin(vertex_index)` 生成，因此没有顶点缓冲。
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &post_process_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
        let post_process_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-post-process-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let post_process_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-post-process-params"),
            // 32 字节：exposure + vignette + 两个填充位，随后是一个 vec4 着色（见 crate::core::post_process）。
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        tracing::info!(target: "game",
            "[render] initialized: adapter={adapter_name} format={format:?} present_mode={:?} gpu_timestamp={timestamp_enabled}",
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
            vertex_buffer_growths: 0,
            vertex_bytes_peak: 0,
            textures: Vec::new(),
            texture_paths: HashMap::new(),
            texture_slots: HashMap::new(),
            texture_identity_hits: 0,
            texture_identity_misses: 0,
            async_loader: AsyncLoader::new(async_mode),
            white: None,
            atlas: None,
            font_atlas_texture: None,
            font_atlas_bind: None,
            font_atlas_sdf_bind: None,
            sdf_sampler,
            pipeline_sdf,
            vector_pipeline,
            vector_bind_group_layout,
            vector_vertex_buffer,
            vector_glyph_buffer,
            vector_segment_buffer,
            vector_bind_group: None,
            vector_glyph_cache: Vec::new(),
            vector_segment_cache: Vec::new(),
            vector_glyph_hash: 0,
            vector_segment_hash: 0,
            storage_capable,
            font_atlas_uploaded_version: u64::MAX,
            font_atlas_uploaded_hash: 0,
            custom_fonts: HashMap::new(),
            project_root,
            camera_buffer,
            light_buffer,
            shadow_camera_buffer,
            identity_palette,
            material_arena,
            material_arena_used: 0,
            instance_arena,
            instance_arena_used: 0,
            uniform_align,
            storage_align,
            mesh_cache: HashMap::new(),
            mesh_slots: HashMap::new(),
            mesh_identity_hits: 0,
            mesh_identity_misses: 0,
            albedo_textures: Vec::new(),
            albedo_sampler,
            white_albedo_view,
            cull_enabled,
            instancing_enabled,
            frame_count: 0,
            _material_bind_group_layout: material_bind_group_layout,
            readback_slots: Vec::new(),
            readback_submitted: 0,
            readback_completed: 0,
            readback_dropped: 0,
            readback_pending_count: 0,
            readback_buffer_rebuilds: 0,
            timestamp_query_set,
            timestamp_slots: Vec::new(),
            timestamp_period_ns,
            timestamp_state: if timestamp_enabled {
                GpuTimestampState::Ready
            } else {
                GpuTimestampState::Unsupported
            },
            timestamp_error: None,
            last_frame_metrics: RenderFrameMetrics::default(),
            last_frame_plan: RenderPlan::empty(),
            last_frame_passes: Vec::new(),
            frame_views: FrameViews::new(),
            world_sprite_pipeline,
            world_sprite_opaque_pipeline,
            world_sprite_camera_buffer,
            world_sprite_camera_bind_group,
            world_sprite_buffer,
            world_sprite_capacity: WORLD_SPRITE_BUFFER_BYTES,
            world_sprite_depth_view,
            world_sprite_depth_size: (0, 0),
            world_sprite_view_projection: Mat4 {
                columns: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            },
            post_process: None,
            post_process_pipeline,
            post_process_layout,
            post_process_sampler,
            post_process_params,
            post_process_texture: None,
            post_process_scene_view: None,
            post_process_size: (0, 0),
        })
    }
}

/// 设备能力不足时给"吃存储缓冲"的管线槽位填一条**占位管线**（矢量文字 / 3D 网格 / 阴影）。
///
/// 为什么要占位而不是把字段改成 `Option`：那三个字段在 `passes.rs`、`gpu_texture.rs`、
/// `arena.rs`、`world_sprite.rs` 和全部原生测试里被引用一百多次，`Option` 化会让整个渲染器
/// 的签名扩散开，而收益只是"少画一条本来就不该画的路径"。占位管线是合法对象、几乎零成本，
/// 而且**不可能**画出看起来对的东西：顶点着色器只吐一个退化点，片元写全 0（含 alpha），
/// 即使被误绑也只是什么都没画。
///
/// 它永远不该被绑定——由 `passes.rs` 的 `storage_capable` 守卫保证（能力不足时计划里就没有
/// 矢量文字与 3D 两个通道，同时把丢弃的 draw 记进 `dropped_draws`）。
fn placeholder_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    const PLACEHOLDER_SHADER: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}
"#;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(PLACEHOLDER_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}
