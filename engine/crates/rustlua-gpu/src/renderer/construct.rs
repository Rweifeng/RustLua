//! `Renderer` 的构造侧（WP12 阶段 4d 从 `renderer/mod.rs` 原样搬出）：`new`、
//! `new_offscreen` 和 1,070 行的 `new_async`——适配器/设备/表面、七条管线、采样器与
//! 各张 GPU 缓冲的创建都在这里。
//!
//! 计划 §5 的硬规矩 2：**只搬位置，不拆函数体** ⇒ `new_async` 整段原样搬，它的分段
//! （管线、缓冲、时间戳）另开工作包（§9）。`Renderer` 结构体与全部字段仍留在
//! `renderer/mod.rs`（§3.1.2）；本模块是它的后代，结构体字面量按私有字段构造**不需要**
//! 放宽任何可见性。

use crate::{
    shader::{
        MESH3D, MESH3D_PBR, POST_PROCESS_SHADER, SHADOW, ShaderSource, TEXTURED_SHADER,
        TONEMAP_SHADER, VECTOR_TEXT_SHADER, WORLD_SPRITE_SHADER,
    },
    shader_preprocess::{PreprocessError, PreprocessOptions, PreprocessedShader, preprocess},
};
use rustlua_camera::camera::{Camera3D, FrameViews, Viewport};
use rustlua_math::math3d::{Mat4, Vec3};
use rustlua_render::async_loader::{AsyncLoader, LoadMode};
use rustlua_render::frame::{OverlayVertex, VectorTextVertex, WorldSpriteVertex};
use rustlua_render::render_plan::RenderPlan;
use rustlua_render::render_settings::{EnvironmentLighting, IblMode, RenderSettings};
use std::{borrow::Cow, collections::HashMap, path::PathBuf, sync::Arc};
use wgpu::util::DeviceExt;
use wgpu::{
    CompositeAlphaMode, Device, DeviceDescriptor, Instance, InstanceDescriptor, PresentMode, Queue,
    SurfaceConfiguration, TextureUsages,
};
use winit::window::Window;

use super::pipeline_cache::{
    ColorTarget, PipelineCache, PipelineFamily, ShaderVariantKey, Shading, SkinMode,
    VARIANT_COMPILE_BUDGET_MS,
};
use super::world_sprite::WORLD_SPRITE_BUFFER_BYTES;
use super::{
    DepthPass, FRAME_ARENA_INITIAL_BYTES, FrameDraw, GpuPassReport, GpuTimestampState,
    IDENTITY_MAT4, INITIAL_2D_VERTEX_BUFFER_BYTES, RenderFrameMetrics, Renderer,
    RendererCapabilities, camera_uniform, compute::ComputeState, depth_stencil_state,
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
        // 窗口路径的 adapter 只用于能力探测；离屏路径没有 adapter，靠限制值判定。
        let mut adapter_for_caps: Option<wgpu::Adapter> = None;
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
            adapter_for_caps = Some(adapter);
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
        // M74-03 P0：能力快照在构造期采一次。`adapter_for_caps` 只有窗口路径才有——
        // 离屏渲染器拿的是外部 device（编辑器 eframe），没有 downlevel flag 可查，
        // 由 `RendererCapabilities::probe` 退到限制值判定。
        let capabilities =
            RendererCapabilities::probe(&device, &queue, adapter_for_caps.as_ref()).await;
        if let Some(reason) = capabilities.compute_unavailable_reason() {
            tracing::info!(
                target: "render",
                backend = capabilities.backend_name(),
                reason = %reason,
                "[render] compute 通道不可用：GPU 粒子等消费者必须走 CPU 档"
            );
        } else {
            tracing::info!(
                target: "render",
                backend = capabilities.backend_name(),
                workgroup = ?capabilities.max_compute_workgroup_size,
                "[render] compute 通道可用"
            );
        }
        // P3-R01：HDR 能力是**两轴独立证据**（可渲染/读回 + 可线性过滤）。这里只报告，不决定
        // 是否启用——是否启用由 `RenderSettings.hdr` 与本帧内容在 `render_passes` 里解析。
        if let Some(reason) = capabilities.hdr_unavailable_reason() {
            tracing::info!(
                target: "render",
                backend = capabilities.backend_name(),
                reason = %reason,
                "[render] HDR 场景颜色不可用：HDR 策略将回退到 LDR"
            );
        } else {
            tracing::info!(
                target: "render",
                backend = capabilities.backend_name(),
                "[render] HDR 场景颜色可用（Rgba16Float + 线性过滤）；默认策略仍为关闭"
            );
        }
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
                        // P3-R02：PBR 片元要读相机世界位置算视线；旧网格着色器只声明 64 字节
                        // 结构，读写同一块 buffer 的前半段。尺寸 80 = mat4 + vec4。
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(80),
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
        let pipeline = build_textured_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            format,
            "rustlua-textured-pipeline",
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-texture-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        // M79 P1-3c：逐纹理采样器表（滤镜 × 寻址 的全组合，下标 = `filter * Wrap::COUNT + wrap`）。
        // **第一格必须与上面那个全局 `sampler` 同参数**（Nearest + ClampToEdge）——那是"没在
        // `resources.meta` 里登记过的贴图"走的路径，它要逐字节维持今天的样子，这条由
        // `the_undocumented_texture_path_keeps_todays_pixels` 钉住。
        let mut texture_samplers = Vec::new();
        for filter in [
            rustlua_assets::import_settings::TextureFilter::Nearest,
            rustlua_assets::import_settings::TextureFilter::Linear,
        ] {
            use rustlua_assets::import_settings::TextureWrap;
            for wrap in [
                TextureWrap::ClampToEdge,
                TextureWrap::Repeat,
                TextureWrap::MirroredRepeat,
            ] {
                texture_samplers.push(device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some(&format!(
                        "rustlua-texture-sampler-{}-{}",
                        filter.token(),
                        wrap.token()
                    )),
                    mag_filter: Renderer::wgpu_filter_mode(filter),
                    min_filter: Renderer::wgpu_filter_mode(filter),
                    // 本批不生成 mipmap（`mip_level_count` 恒为 1），三线性采样无从谈起。
                    mipmap_filter: wgpu::FilterMode::Nearest,
                    address_mode_u: Renderer::wgpu_address_mode(wrap),
                    address_mode_v: Renderer::wgpu_address_mode(wrap),
                    address_mode_w: Renderer::wgpu_address_mode(wrap),
                    ..Default::default()
                }));
            }
        }
        debug_assert_eq!(
            texture_samplers.len(),
            rustlua_assets::import_settings::TextureFilter::COUNT
                * rustlua_assets::import_settings::TextureWrap::COUNT
        );
        // SDF 文本需要 Linear 采样（距离场插值）；精灵保持 Nearest。
        let sdf_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-sdf-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline_sdf = build_textured_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            format,
            "rustlua-textured-sdf-pipeline",
        );
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
            build_vector_text_pipeline(
                &device,
                &vector_layout,
                &vector_shader,
                format,
                "rustlua-vector-text-pipeline",
            )
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
        // 材质布局里有 1 个是**存储缓冲**（`binding 2` = 逐实例模型矩阵，给实例化用）。
        // 能力不足时不放它：布局本身仍然合法（原生那条路完全不变），而依赖它的管线会走占位
        // （见下面的 `mesh_pipeline` / `shadow_pipeline`），3D 通道在 `passes.rs` 里也不再声明。
        let mut material_entries: Vec<wgpu::BindGroupLayoutEntry> = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    // P3-R02：5×vec4 = 基色 / 表面 / 自发光 / 参数 / 贴图开关。
                    min_binding_size: wgpu::BufferSize::new(80),
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
                        (rustlua_render::mesh3d::MAX_BONES * 64) as u64,
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
            // P3-R02：metallic-roughness（G/B）与 emissive。旧 WGSL 不声明这两个绑定，
            // 布局里多出的条目对旧管线无影响（`pbr=off` 的像素门因此仍逐字节一致）。
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // P3-R02-3：法线（切线空间）与遮蔽（R 通道）贴图，均为线性数据。
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 8,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
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
            contents: bytemuck::cast_slice(&camera_uniform(&camera_matrix, camera.position)),
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
            // VERTEX：PBR 的逐实例法线矩阵与模型矩阵共用这块环形缓冲（不同偏移）。
            usage: storage_usage | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
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
        // P3-R03：环境 group（uniform + 预过滤 cubemap + 采样器）。即使 IBL 关闭也绑定一份
        // 全零 SH 与 1×1 黑 cube，这样 PBR 管线布局只有一种，不需要为 Off/On 切换布局。
        // 浮点线性过滤不可用时按 nearest/不可过滤声明（Tier B），避免绑定校验失败。
        let environment_bind_group_layout =
            super::environment::bind_group_layout(&device, capabilities.float_linear_filter);
        let environment_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-environment-buffer"),
            contents: bytemuck::cast_slice(&EnvironmentLighting::default().uniform(IblMode::Off)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let environment_fallback_view = super::environment::fallback_view(&device, &queue);
        let environment_sampler =
            super::environment::sampler(&device, capabilities.float_linear_filter);
        let environment_view = environment_fallback_view.clone();
        let environment_bind_group = super::environment::bind_group(
            &device,
            &environment_bind_group_layout,
            &environment_buffer,
            &environment_view,
            &environment_sampler,
        );
        let mesh_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rustlua-mesh3d-pipeline-layout"),
            bind_group_layouts: &[&camera_bind_group_layout, &material_bind_group_layout],
            push_constant_ranges: &[],
        });
        // 回滚档（Legacy）保持两组布局：多一组会让旧着色器绑定不上，也会让 `pbr=off`
        // 的像素门失去"逐字节不变"的前提。
        let pbr_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rustlua-mesh3d-pbr-pipeline-layout"),
            bind_group_layouts: &[
                &camera_bind_group_layout,
                &material_bind_group_layout,
                &environment_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rustlua-shadow-pipeline-layout"),
                bind_group_layouts: &[&shadow_bind_group_layout, &material_bind_group_layout],
                push_constant_ranges: &[],
            });
        // P3-R04-3：按固定轴预编译变体。LDR 目标格式来自表面配置；HDR 变体只在能力允许时
        // 存在（与 R01 的门控一致）。设备没有存储缓冲时每个 family 只放一条占位管线——
        // `passes.rs` 的 `storage_capable` 守卫保证它不会被绑定。
        let mut pipelines: PipelineCache<Arc<wgpu::RenderPipeline>> = PipelineCache::new();
        if storage_capable {
            let hdr_format = capabilities
                .hdr_usable()
                .then_some(wgpu::TextureFormat::Rgba16Float);
            for skin in [SkinMode::Off, SkinMode::On] {
                let shader = variant_shader_module(
                    &device,
                    &MESH3D,
                    skin,
                    IblMode::Off,
                    &format!("rustlua-mesh3d-{}-shader", skin_tag(skin)),
                );
                let targets = if let Some(hdr) = hdr_format {
                    vec![(ColorTarget::Ldr, format), (ColorTarget::Hdr, hdr)]
                } else {
                    vec![(ColorTarget::Ldr, format)]
                };
                for (target, target_format) in targets {
                    let key = ShaderVariantKey::mesh(Shading::Legacy, skin, target);
                    let label = format!("rustlua-{}-pipeline", key.label());
                    pipelines
                        .get_or_create(key, || {
                            Ok::<_, std::convert::Infallible>(Arc::new(create_mesh_pipeline(
                                &device,
                                &mesh_pipeline_layout,
                                &shader,
                                target_format,
                                &label,
                                false,
                                rustlua_render::mesh3d::MaterialAlphaMode::Opaque,
                                false,
                            )))
                        })
                        .expect("3D 网格变体数远低于 family/全局上限");
                }
            }
            for skin in [SkinMode::Off, SkinMode::On] {
                let shader = variant_shader_module(
                    &device,
                    &SHADOW,
                    skin,
                    IblMode::Off,
                    &format!("rustlua-shadow-{}-shader", skin_tag(skin)),
                );
                let key = ShaderVariantKey::shadow(skin);
                let label = format!("rustlua-{}-pipeline", key.label());
                pipelines
                    .get_or_create(key, || {
                        Ok::<_, std::convert::Infallible>(Arc::new(create_shadow_pipeline(
                            &device,
                            &shadow_pipeline_layout,
                            &shader,
                            &label,
                            false,
                        )))
                    })
                    .expect("阴影变体数远低于 family/全局上限");
            }
        } else {
            // 3D 网格/阴影着色器声明了 `instance_models` 存储绑定，而布局在能力不足时不含它
            // ⇒ 真管线建不出来（校验错误）。占位管线永远不该被绑定：`passes.rs` 在能力不足时
            // 不声明 3D 通道，并把丢弃的 draw 记进 `dropped_draws`。
            let mesh_placeholder = Arc::new(placeholder_pipeline(
                &device,
                format,
                "rustlua-mesh3d-placeholder-pipeline",
            ));
            let shadow_placeholder = Arc::new(placeholder_pipeline(
                &device,
                format,
                "rustlua-shadow-placeholder-pipeline",
            ));
            for skin in [SkinMode::Off, SkinMode::On] {
                pipelines
                    .get_or_create(
                        ShaderVariantKey::mesh(Shading::Legacy, skin, ColorTarget::Ldr),
                        || Ok::<_, std::convert::Infallible>(mesh_placeholder.clone()),
                    )
                    .expect("占位变体不超限");
                pipelines
                    .get_or_create(ShaderVariantKey::shadow(skin), || {
                        Ok::<_, std::convert::Infallible>(shadow_placeholder.clone())
                    })
                    .expect("占位变体不超限");
            }
        }
        // P3-R04-4：预编译发生在启动期（构造 `Renderer` 时），统计随资源快照暴露给 Profiler。
        // 超预算只告警一次：此刻管线已经建好，取消它们只会把同样的卡顿挪到首帧。
        let variant_stats = pipelines.stats();
        if variant_stats.within_budget() {
            tracing::info!(
                target: "game",
                "[render] shader variants: count={} compile_ms={:.2} hits={} misses={} budget_ms={:.0}",
                variant_stats.variants,
                variant_stats.compile_ms(),
                variant_stats.hits,
                variant_stats.misses,
                VARIANT_COMPILE_BUDGET_MS
            );
        } else {
            tracing::warn!(
                target: "render",
                "[runtime] shader 变体预编译超出预算：count={} compile_ms={:.2} budget_ms={:.0}；请缩减预编译清单，不要每帧补编译",
                variant_stats.variants,
                variant_stats.compile_ms(),
                VARIANT_COMPILE_BUDGET_MS
            );
        }
        let depth_view = Self::create_depth_view(&device, config.width, config.height);
        let depth_init = (config.width, config.height);
        let timestamp_enabled = device.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        );
        let timestamp_query_set = timestamp_enabled.then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("rustlua-frame-timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: super::pass_timestamps::TIMESTAMP_QUERY_COUNT,
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
        // 换一个策略"就会变成两条不同的渲染路径。P3-R01 起目标格式也是参数：HDR 变体只换格式。
        let create_world_sprite_pipeline = |label: &str, pass: DepthPass, target_format| {
            build_world_sprite_pipeline(
                &device,
                &world_sprite_pipeline_layout,
                &world_sprite_shader,
                pass,
                target_format,
                label,
            )
        };
        let world_sprite_pipeline = create_world_sprite_pipeline(
            "rustlua-world-sprite-pipeline",
            DepthPass::WorldSprite,
            format,
        );
        let world_sprite_opaque_pipeline = create_world_sprite_pipeline(
            "rustlua-world-sprite-opaque-pipeline",
            DepthPass::WorldSpriteOpaque,
            format,
        );
        let (hdr_world_sprite_pipeline, hdr_world_sprite_opaque_pipeline) =
            if capabilities.hdr_usable() {
                (
                    Some(create_world_sprite_pipeline(
                        "rustlua-world-sprite-hdr-pipeline",
                        DepthPass::WorldSprite,
                        wgpu::TextureFormat::Rgba16Float,
                    )),
                    Some(create_world_sprite_pipeline(
                        "rustlua-world-sprite-opaque-hdr-pipeline",
                        DepthPass::WorldSpriteOpaque,
                        wgpu::TextureFormat::Rgba16Float,
                    )),
                )
            } else {
                (None, None)
            };
        let world_sprite_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-world-sprite-buffer"),
            size: WORLD_SPRITE_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // P3-R07-3：GPU 粒子 instanced 管线的**布局**（group2 = 参数 uniform + 状态只读存储）。
        // 这里只建两条布局，不建管线：管线要按目标格式（LDR/HDR）延迟编译，理由与 R02 的 PBR
        // 变体一样——启动预算（`compile_ms`）里没有理由为一条"没配 GPU 粒子就永远用不到"的
        // 管线付钱。布局必须留在渲染器上：延迟编译与（未来的）热重载都要按同一份描述符重建。
        // 无存储缓冲的设备（WebGL2 即如此）不建这两条布局：binding 1 是只读存储，
        // `create_bind_group_layout` 会直接报校验错误，而 wasm 上的未捕获校验错误是 abort。
        // GPU 粒子在这些设备上本来就走 CPU，`configure_gpu_particles` 会返回 Unsupported。
        let (gpu_particle_draw_layout, gpu_particle_pipeline_layout) = if storage_capable {
            let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-gpu-particles-draw-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
            // group0/group1 与世界 Sprite 管线**共用同两个布局对象**，所以相机绑定组与白贴图绑定组
            // 可以直接复用——粒子不需要自己的相机 uniform，也不需要第二张贴图。
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rustlua-gpu-particles-pipeline-layout"),
                bind_group_layouts: &[
                    &world_sprite_camera_layout,
                    &texture_bind_group_layout,
                    &draw_layout,
                ],
                push_constant_ranges: &[],
            });
            (Some(draw_layout), Some(pipeline_layout))
        } else {
            (None, None)
        };
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
        let post_process_pipeline = build_fullscreen_pipeline(
            &device,
            &post_process_pipeline_layout,
            &post_process_shader,
            format,
            "rustlua-post-process-pipeline",
        );
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
            // 32 字节：exposure + vignette + exposure_ev + tonemap code，随后是一个 vec4 着色
            // （见 rustlua_render::post_process 的 `uniform`）。
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // P3-R01：Tonemap 全屏管线。复用后处理的绑定布局（HDR 纹理 + 线性过滤采样器 + uniform），
        // 片元先把线性值乘 `2^exposure_ev` 再走曲线。能力不足时不创建，HDR 因此不会被启用。
        let tonemap_pipeline = if capabilities.hdr_usable() {
            let tonemap_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("rustlua-tonemap-shader"),
                source: wgpu::ShaderSource::Wgsl(TONEMAP_SHADER.into()),
            });
            Some(build_fullscreen_pipeline(
                &device,
                &post_process_pipeline_layout,
                &tonemap_shader,
                format,
                "rustlua-tonemap-pipeline",
            ))
        } else {
            None
        };
        let tonemap_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-tonemap-params"),
            // 16 字节：exposure_ev + tonemap code + 两个填充位。
            size: 16,
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
            pipelines,
            #[cfg(not(target_arch = "wasm32"))]
            mesh_pipeline_layout: mesh_pipeline_layout.clone(),
            pbr_pipeline_layout,
            shadow_pipeline_layout,
            #[cfg(not(target_arch = "wasm32"))]
            shader_library: crate::shader_library::ShaderLibrary::builtin(),
            #[cfg(not(target_arch = "wasm32"))]
            shader_reload: None,
            #[cfg(not(target_arch = "wasm32"))]
            shader_committed: HashMap::new(),
            #[cfg(not(target_arch = "wasm32"))]
            fixed_slot_layouts: crate::renderer::shader_hot_reload::FixedSlotLayouts {
                textured: pipeline_layout.clone(),
                vector_text: vector_layout.clone(),
                world_sprite: world_sprite_pipeline_layout.clone(),
                post_process: post_process_pipeline_layout.clone(),
            },
            pbr_pipelines_ready: false,
            environment_buffer,
            environment_bind_group,
            environment_bind_group_layout,
            environment_fallback_view,
            environment_sampler,
            environment_view,
            environment_map: None,
            environment: None,
            camera_bind_group,
            shadow_bind_group,
            depth_view,
            depth_size: depth_init,
            shadow_view,
            texture_bind_group_layout,
            sampler,
            texture_samplers,
            resource_meta: None,
            resource_meta_loaded: false,
            sampling_epoch: 0,
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
            capabilities,
            compute_state: ComputeState::new(),
            gpu_particles: Vec::new(),
            gpu_particle_generation: 0,
            gpu_particle_draw_layout,
            gpu_particle_pipeline_layout,
            gpu_particle_pipeline: None,
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
            linear_textures: Vec::new(),
            texture_content_cache: HashMap::new(),
            material_cache: HashMap::new(),
            material_preview_path: None,
            material_preview_textures: None,
            material_preview_indices: None,
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
            last_gpu_pass_report: GpuPassReport::default(),
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
            render_settings: RenderSettings::default(),
            hdr_warning_logged: false,
            hdr_scene_texture: None,
            hdr_scene_view: None,
            hdr_scene_size: (0, 0),
            hdr_world_sprite_pipeline,
            hdr_world_sprite_opaque_pipeline,
            tonemap_pipeline,
            tonemap_params,
        })
    }
}

impl Renderer {
    /// P3-R02：在宿主首次打开 `RenderSettings.pbr` 时编译 PBR 网格变体（静态/蒙皮 × LDR/HDR）。
    ///
    /// 为什么不放进启动期无条件预编译：旧 3D 场景默认 `pbr=off`，它们不该为一份永远不用的
    /// 管线付启动时间。这里仍然不是"每帧补编译"——`pbr_pipelines_ready` 一旦置位，后续
    /// 帧只做缓存查找；重复 `set_render_settings` 也不会重新编译。
    ///
    /// 阴影变体与着色模型无关，不在这里重复创建；不支持存储缓冲的设备（WebGL2）3D 通道
    /// 本来就是占位档，这里只留一条一次性告警并置位，避免每次设置都刷屏。
    pub(super) fn ensure_pbr_pipelines(&mut self) {
        if self.pbr_pipelines_ready {
            return;
        }
        self.pbr_pipelines_ready = true;
        if !self.storage_capable {
            tracing::warn!(
                target: "render",
                "[render] RenderSettings.pbr=true 但设备不支持存储缓冲（逐实例模型矩阵）；3D 通道保持占位档"
            );
            return;
        }
        let started = web_time::Instant::now();
        let hdr_supported = self.capabilities.hdr_usable();
        let ldr_only = [ColorTarget::Ldr];
        let both = [ColorTarget::Ldr, ColorTarget::Hdr];
        let targets: &[ColorTarget] = if hdr_supported { &both } else { &ldr_only };
        // 源文本在这里过 `variant_module`（覆盖目录优先），所以循环不能提前借用
        // `self.pipelines`：管线先在局部建好，再交给缓存。
        for skin in [SkinMode::Off, SkinMode::On] {
            for &target in targets {
                let key = ShaderVariantKey::mesh(Shading::Pbr, skin, target);
                let shader = self.variant_module(&key);
                let target_format = match target {
                    ColorTarget::Ldr => self.config.format,
                    ColorTarget::Hdr => wgpu::TextureFormat::Rgba16Float,
                };
                let label = format!("rustlua-{}-pipeline", key.label());
                let pipeline = Arc::new(create_mesh_pipeline(
                    &self.device,
                    &self.pbr_pipeline_layout,
                    &shader,
                    target_format,
                    &label,
                    true,
                    rustlua_render::mesh3d::MaterialAlphaMode::Opaque,
                    false,
                ));
                self.pipelines
                    .get_or_create(key, || Ok::<_, std::convert::Infallible>(pipeline))
                    .expect("PBR 变体数远低于 family/全局上限");
            }
        }
        let compile_ms = started.elapsed().as_secs_f64() * 1000.0;
        if compile_ms <= VARIANT_COMPILE_BUDGET_MS {
            tracing::info!(
                target: "game",
                "[render] PBR shader variants compiled on enable: compile_ms={compile_ms:.2} budget_ms={VARIANT_COMPILE_BUDGET_MS:.0}"
            );
        } else {
            tracing::warn!(
                target: "render",
                "[runtime] PBR shader 变体编译耗时 {compile_ms:.2}ms 超出预算 {VARIANT_COMPILE_BUDGET_MS:.0}ms；仅发生一次，请检查着色器复杂度/设备"
            );
        }
    }

    /// P3-R02-4 / P3-R03-2：按本帧实际出现的 alpha/双面/IBL 组合补编译管线。
    ///
    /// 启动期只预热默认的 opaque/单面/IBL-Off 组合；mask/blend/双面与 IBL 档位是场景内容
    /// 决定的，提前全编译（PBR 组合数会乘法增长）会越过 250ms 预算。这里每个组合只建一次，
    /// 之后和预热变体一样命中缓存。Legacy 着色忽略 alpha/双面/IBL（回滚档逐字节不变），
    /// 因此不补编译。
    pub(super) fn ensure_frame_pipelines(
        &mut self,
        draws: &[FrameDraw],
        shading: Shading,
        target: ColorTarget,
        ibl: IblMode,
    ) {
        if shading != Shading::Pbr || !self.storage_capable {
            return;
        }
        use rustlua_render::mesh3d::MaterialAlphaMode;
        let target_format = match target {
            ColorTarget::Ldr => self.config.format,
            ColorTarget::Hdr => wgpu::TextureFormat::Rgba16Float,
        };
        let mut material_keys = Vec::new();
        let mut shadow_keys = Vec::new();
        for draw in draws {
            let skin = SkinMode::from(draw.material.skinned);
            let mesh_key = ShaderVariantKey::mesh_with_alpha(
                shading,
                skin,
                target,
                draw.material.alpha_mode,
                draw.material.double_sided,
            )
            .with_ibl(ibl);
            if !material_keys.contains(&mesh_key) {
                material_keys.push(mesh_key);
            }
            if draw.material.alpha_mode != MaterialAlphaMode::Blend {
                let shadow_key = ShaderVariantKey::shadow_with_alpha(
                    skin,
                    draw.material.alpha_mode,
                    draw.material.double_sided,
                );
                if !shadow_keys.contains(&shadow_key) {
                    shadow_keys.push(shadow_key);
                }
            }
        }
        for key in material_keys {
            if self.pipelines.get(&key).is_some() {
                continue;
            }
            let shader = self.variant_module(&key);
            let label = format!("rustlua-{}-pipeline", key.label());
            self.pipelines
                .get_or_create(key, || {
                    Ok::<_, std::convert::Infallible>(Arc::new(create_mesh_pipeline(
                        &self.device,
                        &self.pbr_pipeline_layout,
                        &shader,
                        target_format,
                        &label,
                        true,
                        key.alpha,
                        key.double_sided,
                    )))
                })
                .expect("alpha/双面/IBL 网格变体数低于 family/全局上限");
            tracing::info!(
                target: "render",
                variant = %key.label(),
                "[render] mesh variant compiled on demand"
            );
        }
        for key in shadow_keys {
            if self.pipelines.get(&key).is_some() {
                continue;
            }
            let shader = self.variant_module(&key);
            let label = format!("rustlua-{}-pipeline", key.label());
            self.pipelines
                .get_or_create(key, || {
                    Ok::<_, std::convert::Infallible>(Arc::new(create_shadow_pipeline(
                        &self.device,
                        &self.shadow_pipeline_layout,
                        &shader,
                        &label,
                        key.double_sided,
                    )))
                })
                .expect("alpha/双面阴影变体数低于 family/全局上限");
            tracing::info!(
                target: "render",
                variant = %key.label(),
                "[render] shadow alpha variant compiled on demand"
            );
        }
    }

    /// P3-R05：变体用的源文本——开发期覆盖目录优先，否则内置源。
    ///
    /// 覆盖文件**读不出来**时这里记一条 warning 并回退内置源：构造期还没有"旧管线"可保留，
    /// 而运行期的重载走 [`Renderer::set_shader_override_dir`]，那里把错误原样报给作者。
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn variant_source(
        &self,
        root: &'static ShaderSource,
    ) -> (&'static str, Cow<'static, str>) {
        match self.shader_library.resolve(root.name) {
            Ok(Some(resolved)) => (resolved.repo_path, resolved.source),
            Ok(None) => (root.path, Cow::Borrowed(root.source)),
            Err(error) => {
                tracing::warn!(target: "render", "[render] {error}；本次沿用内置源");
                (root.path, Cow::Borrowed(root.source))
            }
        }
    }

    /// wasm/release：恒为内置源。覆盖目录在这两个平台上没有入口（计划 §6.5 的发布门），
    /// 因此也不该把文件 IO 分支编进浏览器产物。
    #[cfg(target_arch = "wasm32")]
    pub(super) fn variant_source(
        &self,
        root: &'static ShaderSource,
    ) -> (&'static str, Cow<'static, str>) {
        let _ = &self;
        (root.path, Cow::Borrowed(root.source))
    }

    /// 延迟补编译/重载路径的着色器模块：与启动期共用 [`variant_shader_module`] 的预处理口径，
    /// 只是源文本先过 [`Renderer::variant_source`]。
    ///
    /// 标签按**变体键**命名（`rustlua-<变体>-shader`）：重载后要能在诊断里认出是哪个变体用了
    /// 哪份源，共用一个标签就查不出来了。
    pub(super) fn variant_module(&self, key: &ShaderVariantKey) -> wgpu::ShaderModule {
        let root = variant_root(key);
        let label = format!("rustlua-{}-shader", key.label());
        let (path, source) = self.variant_source(root);
        let preprocessed = match preprocessed_variant(path, &source, key.skin, key.ibl) {
            Ok(preprocessed) => preprocessed,
            Err(error) => {
                // 覆盖文件里 `//#if` 拼错时，延迟补编译（首次打开 PBR 那一帧）正好撞上它。
                // panic 会把整个进程带走；这里沿用内置源并报 warning，逐变体的错误文本由
                // `set_shader_override_dir` 那条路负责返回。
                tracing::warn!(
                    target: "render",
                    "[render] {path} 预处理失败：{error}；本次沿用内置源"
                );
                preprocessed_variant(root.path, root.source, key.skin, key.ibl)
                    .unwrap_or_else(|error| panic!("内置源 {} 预处理失败：{error}", root.path))
            }
        };
        module_from_preprocessed(&self.device, &preprocessed, &label)
    }
}

/// 变体键 → 着色器模块：先过预处理器，只保留该变体选中的分支。
///
/// 预处理失败属于构建期不变量（feature 拼写/指令配对由单测钉住），所以这里直接 panic，
/// 而不是把错误拖到第一次绘制。
pub(super) fn variant_shader_module(
    device: &wgpu::Device,
    shader: &ShaderSource,
    skin: SkinMode,
    ibl: IblMode,
    label: &str,
) -> wgpu::ShaderModule {
    variant_shader_module_from(device, shader.path, shader.source, skin, ibl, label)
}

/// 上一函数的另一半：根 shader 的**源文本由调用方给**（P3-R05 的覆盖目录走这条路）。
///
/// 拆成两半的理由：内置源在编译期就固定（`include_str!`），覆盖源要到运行期才读得出来，
/// 但两条路必须共用**同一份预处理口径**（feature 表、known_features、行号映射），否则
/// "覆盖版变体"和"内置版变体"画的不是同一个程序。
pub(super) fn variant_shader_module_from(
    device: &wgpu::Device,
    path: &str,
    source: &str,
    skin: SkinMode,
    ibl: IblMode,
    label: &str,
) -> wgpu::ShaderModule {
    let preprocessed = preprocessed_variant(path, source, skin, ibl)
        .unwrap_or_else(|error| panic!("{path} 预处理失败：{error}"));
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(preprocessed.source().to_owned().into()),
    })
}

/// 预处理结果 → 模块（热重载的提交路径）。
///
/// 与 [`variant_shader_module_from`] 的区别只在于**谁负责处理预处理失败**：构造期的内置源
/// 出错是仓库 bug，panic 是对的；热重载的出错要在 GPU 之外变成报告并保留旧管线，所以那条路
/// 先拿到已校验的 [`PreprocessedShader`]，这里不再重复预处理（重复一次就多一次 panic 分支）。
pub(super) fn module_from_preprocessed(
    device: &wgpu::Device,
    preprocessed: &PreprocessedShader,
    label: &str,
) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(preprocessed.source().to_owned().into()),
    })
}

/// 2D 叠层顶点布局（`OverlayVertex`：位置 2 + UV 2 + 颜色 4 + 纹理槽 u32）。
fn overlay_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
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
    }
}

/// 矢量文字顶点布局（位置 2 + UV 2 + 字形索引 u32 + 颜色 4）。
fn vector_text_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
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
    }
}

/// 世界 Sprite 顶点布局（位置 3 + UV 2 + 颜色 4）。
fn world_sprite_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
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
    }
}

/// 把三个 2D 批次的公共骨架收成一个函数：它们只差顶点布局、深度策略与标签。
///
/// 构造期与热重载期共用它，"重载后的 2D 管线"和"首帧的 2D 管线"才是同一份描述符；否则
/// 覆盖文件能过画面、重启后行为却变了，是最难查的一种漂移。
#[allow(clippy::too_many_arguments)] // 描述符字段就是这么多，拆结构体只会多一层搬运
fn two_d_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    vertex_buffers: &[wgpu::VertexBufferLayout<'static>],
    topology: wgpu::PrimitiveTopology,
    format: wgpu::TextureFormat,
    depth: Option<wgpu::DepthStencilState>,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: vertex_buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: depth,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// `textured.wgsl` 的叠层管线。`pipeline` 与 `pipeline_sdf` 只差标签：SDF 与非 SDF 用的
/// 采样器在绘制时绑定，管线本身逐字段相同。
pub(super) fn build_textured_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    two_d_pipeline(
        device,
        layout,
        module,
        &[overlay_vertex_layout()],
        wgpu::PrimitiveTopology::TriangleList,
        format,
        None,
        label,
    )
}

/// `vector_text.wgsl` 的管线（存储缓冲缺失时调用方给占位管线，不走这里）。
pub(super) fn build_vector_text_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    two_d_pipeline(
        device,
        layout,
        module,
        &[vector_text_vertex_layout()],
        wgpu::PrimitiveTopology::TriangleList,
        format,
        None,
        label,
    )
}

/// `world_sprite.wgsl` 的管线；`pass` 决定深度策略，`format` 决定 LDR/HDR 变体。
pub(super) fn build_world_sprite_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    pass: DepthPass,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    two_d_pipeline(
        device,
        layout,
        module,
        &[world_sprite_vertex_layout()],
        wgpu::PrimitiveTopology::TriangleList,
        format,
        Some(depth_stencil_state(pass, wgpu::DepthBiasState::default())),
        label,
    )
}

/// GPU 粒子（`gpu_particles.rs` 内联 draw 段）的 instanced 管线。
///
/// 与世界 Sprite 管线只差两处：**没有顶点缓冲**（几何由 `@builtin(vertex_index)` 的四角给出，
/// 逐实例数据走 group2 的只读存储缓冲）与 **triangle strip**（4 顶点/实例，旧实现是 6 顶点
/// triangle list）。深度策略、alpha 混合、片元输出全部一致——粒子本来就是半透明段的一种。
pub(super) fn build_gpu_particle_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    pass: DepthPass,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    two_d_pipeline(
        device,
        layout,
        module,
        &[],
        wgpu::PrimitiveTopology::TriangleStrip,
        format,
        Some(depth_stencil_state(pass, wgpu::DepthBiasState::default())),
        label,
    )
}

/// 无顶点缓冲的全屏三角管线（`post_process.wgsl` 与 `tonemap.wgsl` 共用：顶点由
/// `@builtin(vertex_index)` 生成，不开混合也不读深度）。
pub(super) fn build_fullscreen_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module,
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
    })
}

/// 变体轴 → 预处理后的源（带行号映射）。
///
/// 热重载先走这里而不是直接建模块：预处理失败要**报告并保留旧管线**，不能 panic。
/// 返回的 [`PreprocessedShader::line_map`] 还把出错行映射回原文件，编辑器 Console 因此能指到
/// 覆盖目录里那一行的真实行号。
pub(super) fn preprocessed_variant(
    path: &str,
    source: &str,
    skin: SkinMode,
    ibl: IblMode,
) -> Result<PreprocessedShader, PreprocessError> {
    let mut features = Vec::with_capacity(skin.features().len() + ibl.features().len());
    features.extend_from_slice(skin.features());
    features.extend_from_slice(ibl.features());
    preprocess(
        path,
        source,
        PreprocessOptions {
            features: &features,
            known_features: KNOWN_FEATURES,
            ..Default::default()
        },
    )
}

/// 预处理器认识的 feature 全集（与 `//#if` 指令同源；拼错的名字按未知 feature 报错）。
pub(super) const KNOWN_FEATURES: &[&str] = &["SKIN", "IBL_DIFFUSE", "IBL_SPECULAR"];

/// 变体键 → 根 shader（重建与接口比对都从这张表出发）。
///
/// 阴影与着色模型无关（[`ShaderVariantKey::shadow`] 恒 Legacy），PBR 是第二份片元着色，
/// 所以只有 `Mesh3d + Pbr` 落到 `mesh3d_pbr.wgsl`。
pub(super) fn variant_root(key: &ShaderVariantKey) -> &'static ShaderSource {
    match (key.family, key.shading) {
        (PipelineFamily::Shadow, _) => &SHADOW,
        (PipelineFamily::Mesh3d, Shading::Legacy) => &MESH3D,
        (PipelineFamily::Mesh3d, Shading::Pbr) => &MESH3D_PBR,
    }
}

const fn skin_tag(skin: SkinMode) -> &'static str {
    match skin {
        SkinMode::Off => "static",
        SkinMode::On => "skin",
    }
}

/// 与 `rustlua_render::mesh3d::MeshVertex` 的 `#[repr(C)]` 布局逐字段对应。
const MESH_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 6] = [
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
    // 切线与骨骼属性刻意保持"旧位置不动、新属性追加"：旧网格/阴影着色器只声明到
    // location 4，多出来的属性不会被消费，`pbr=off` 的像素因此不受影响。
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 32,
        shader_location: 5,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Uint32x4,
        offset: 48,
        shader_location: 3,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 64,
        shader_location: 4,
    },
];

fn mesh_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<rustlua_render::mesh3d::MeshVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &MESH_VERTEX_ATTRIBUTES,
    }
}

/// P3-R02：逐实例法线矩阵（`transpose(inverse(mat3(model)))`）的顶点缓冲槽 1。
/// 每实例 4 个 `vec4`，与着色器里的 `mat4x4<f32>` 列主序一一对应。
const PBR_NORMAL_ATTRIBUTES: [wgpu::VertexAttribute; 4] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 0,
        shader_location: 6,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 16,
        shader_location: 7,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 32,
        shader_location: 8,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 48,
        shader_location: 9,
    },
];

fn pbr_normal_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: 64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &PBR_NORMAL_ATTRIBUTES,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn create_mesh_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
    pbr_normals: bool,
    alpha: rustlua_render::mesh3d::MaterialAlphaMode,
    double_sided: bool,
) -> wgpu::RenderPipeline {
    use rustlua_render::mesh3d::MaterialAlphaMode;
    let buffers = if pbr_normals {
        vec![mesh_vertex_layout(), pbr_normal_layout()]
    } else {
        vec![mesh_vertex_layout()]
    };
    let mut depth = depth_stencil_state(DepthPass::Mesh3d, wgpu::DepthBiasState::default());
    // 半透明不写深度：否则后画的透明片会挡住更远的透明片；深度测试仍然保留。
    if alpha == MaterialAlphaMode::Blend {
        depth.depth_write_enabled = false;
    }
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(if alpha == MaterialAlphaMode::Blend {
                    wgpu::BlendState::ALPHA_BLENDING
                } else {
                    wgpu::BlendState::REPLACE
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: if double_sided {
                None
            } else {
                Some(wgpu::Face::Back)
            },
            ..Default::default()
        },
        depth_stencil: Some(depth),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}

pub(super) fn create_shadow_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    double_sided: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[mesh_vertex_layout()],
        },
        // mask 材质需要在片元阶段按贴图 alpha 剔除；opaque 分支不采样，深度结果不变。
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: if double_sided {
                None
            } else {
                Some(wgpu::Face::Back)
            },
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
