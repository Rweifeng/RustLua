//! 后处理通道的全部资源（拆分批 9 / 9）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里 `docs/54 §4.3` 那段（**661 行、31 处资源创建**），是拆分里最大的一块。
//!
//! ## 为什么它不受批次 3 阻塞
//!
//! 探查结论：这一段**只在最后 `Ok(Self { ... })` 结构体字面量里**才碰到
//! `material_bind_group_layout` / `mesh_pipeline_layout` / `light_buffer` 那批变量
//! ——那是"构造完的字段赋值"，**不是创建资源时的输入**。
//!
//! 真正的输入只有四个：`device` / `format` / `queue` / `capabilities`。
//! 所以它能独立搬，尽管它排在 `new_async` 的后半段。
//!
//! ## 三个子通道
//!
//! | 子组 | 内容 |
//! |---|---|
//! | Tonemap + 后处理 | `post_process_*`、`tonemap_*`、`section_height_white` |
//! | SSAO | `ssao_*`（含 blur 与几张 1×1 占位） |
//! | SSR | `ssr_*`（含 blur、history 与几张占位） |
//!
//! 宿主不打开后处理时，这些都只是闲置对象（不产生 pass、不产生离屏纹理），
//! 因此"没开后处理"的运行时代价与收敛前一致。
//!
//! ## 语义零变化的等价点
//!
//! 1. **1×1 占位图**是绑定组形状恒定的前提：每个槽位都必须有资源，
//!    所以没内容时绑一张占位图而不是空绑定。
//! 2. **能力不足时不创建 Tonemap**（看 `capabilities.hdr_usable()`），
//!    `tonemap_pipeline` 因此是 `Option`，HDR 也不会被启用。
//! 3. `ssao_depth_view` **不在这里**：它是 `depth_view.clone()`，而 `depth_view` 由更早的
//!    上下文提供，且那行紧挨着 `Ok(Self { ... })`，属于收尾而非建资源。
//! 4. 子段之间的创建顺序**逐项保持不变**——`tests/construct_creation_order.rs` 会验证。

use wgpu::{
    BindGroupLayout, Buffer, Device, PipelineLayout, Queue, RenderPipeline, Sampler, ShaderModule,
    Texture, TextureFormat, TextureView,
};

use crate::renderer::{RendererCapabilities, TONEMAP_UNIFORM_BYTES};
use crate::shader::{POST_PROCESS_SHADER, TONEMAP_SHADER};

use super::build_fullscreen_pipeline;

/// 后处理通道的全部资源（41 个）。
pub(crate) struct PostProcessResources {
    pub post_process_shader: ShaderModule,
    pub post_process_layout: BindGroupLayout,
    pub post_process_pipeline_layout: PipelineLayout,
    pub post_process_pipeline: RenderPipeline,
    pub post_process_sampler: Sampler,
    pub post_process_params: Buffer,
    pub tonemap_layout: BindGroupLayout,
    pub tonemap_pipeline_layout: PipelineLayout,
    pub tonemap_pipeline: Option<RenderPipeline>,
    pub tonemap_params: Buffer,
    pub section_height_white: TextureView,
    pub ssao_bind_group_layout: BindGroupLayout,
    pub ssao_pipeline_layout: PipelineLayout,
    pub ssao_shader: ShaderModule,
    pub ssao_pipeline: RenderPipeline,
    pub ssao_params: Buffer,
    pub ssao_white: Texture,
    pub ssao_raw_view: TextureView,
    pub ssao_blur_white: Texture,
    pub ssao_view: TextureView,
    pub ssao_blur_bind_group_layout: BindGroupLayout,
    pub ssao_blur_pipeline_layout: PipelineLayout,
    pub ssao_blur_shader: ShaderModule,
    pub ssao_blur_pipeline: RenderPipeline,
    pub ssao_blur_params: Buffer,
    pub ssr_bind_group_layout: BindGroupLayout,
    pub ssr_pipeline_layout: PipelineLayout,
    pub ssr_shader: ShaderModule,
    pub ssr_pipeline: RenderPipeline,
    pub ssr_params: Buffer,
    pub ssr_white: Texture,
    pub ssr_raw_view: TextureView,
    pub ssr_blur_white: Texture,
    pub ssr_view: TextureView,
    pub ssr_history_white: Texture,
    pub ssr_history_view: TextureView,
    pub ssr_blur_bind_group_layout: BindGroupLayout,
    pub ssr_blur_pipeline_layout: PipelineLayout,
    pub ssr_blur_shader: ShaderModule,
    pub ssr_blur_pipeline: RenderPipeline,
    pub ssr_blur_params: Buffer,
}

/// 建后处理通道的全部资源（Tonemap + SSAO + SSR）。
pub(crate) fn post_process_resources(
    device: &Device,
    queue: &Queue,
    format: TextureFormat,
    capabilities: &RendererCapabilities,
) -> PostProcessResources {
    // docs/54 §4.3：后处理通道的管线与资源。宿主不打开时这些都只是闲置对象（不产生 pass、
    // 不产生离屏纹理），因此"没开后处理"的运行时代价与收敛前一致。
    let post_process_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rustlua-post-process-shader"),
        source: wgpu::ShaderSource::Wgsl(POST_PROCESS_SHADER.into()),
    });
    let post_process_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        device,
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
    // Tonemap 需要额外的深度与逆视图投影做 SSAO，因此单独一份绑定布局；
    // 后处理 LDR 路径继续用上面 3 项的布局，不去读深度。
    let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rustlua-tonemap-bind-group-layout"),
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
                    min_binding_size: wgpu::BufferSize::new(TONEMAP_UNIFORM_BYTES),
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
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // P3-R166：`sect=1` 地形的高度图（R=高 8 位、G=低 8 位，16 位定点）。
            // 不走采样器：Tonemap 用 `textureLoad` 自己插值，避免线性过滤把两个字节混掉。
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
            // P3-R170：水面平面反射的离屏色图（半分辨率的 HDR 镜像视图）。
            // 关掉时绑的是 1×1 透明占位图，着色器按 `planar.x` 与 alpha 掩码跳过。
            wgpu::BindGroupLayoutEntry {
                binding: 9,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 10,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let tonemap_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rustlua-tonemap-pipeline-layout"),
        bind_group_layouts: &[&tonemap_layout],
        push_constant_ranges: &[],
    });
    // P3-R01：Tonemap 全屏管线。片元先把线性值乘 `2^exposure_ev` 再走曲线；
    // P3-R09 起还可读深度做 SSAO。能力不足时不创建，HDR 因此不会被启用。
    let tonemap_pipeline = if capabilities.hdr_usable() {
        let tonemap_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-tonemap-shader"),
            source: wgpu::ShaderSource::Wgsl(TONEMAP_SHADER.into()),
        });
        Some(build_fullscreen_pipeline(
            device,
            &tonemap_pipeline_layout,
            &tonemap_shader,
            format,
            "rustlua-tonemap-pipeline",
        ))
    } else {
        None
    };
    let tonemap_params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-tonemap-params"),
        // 400 字节：8 个标量 + 视图投影 + 逆视图投影 + 世界→局部 + 相机位置 + 光照方向 + 雾 + 景深
        // + 天空 + 水面反射 + 云层 + 剖面裁剪平面/世界→局部/地形范围（P3-R166）
        // + 平面反射开关/强度（P3-R170）。
        size: TONEMAP_UNIFORM_BYTES,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // 没有剖面地形时绑定这张 1×1 占位高度图；着色器靠 `section_terrain.w` 关闭分支，
    // 不会真读它，但绑定布局要求每个槽位都有资源。
    let section_height_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-tonemap-height-white"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &section_height_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[0u8, 0, 0, 255],
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
    // 原文用遮蔽把 `Texture` 直接变成 `TextureView`；这里显式两个名字，
    // 语义不变（下游只认视图），但避免"同名两类型"在结构体里对不上。
    let section_height_white =
        section_height_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let ssao_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rustlua-ssao-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(144),
                    },
                    count: None,
                },
            ],
        });
    let ssao_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rustlua-ssao-pipeline-layout"),
        bind_group_layouts: &[&ssao_bind_group_layout],
        push_constant_ranges: &[],
    });
    let ssao_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rustlua-ssao-shader"),
        source: wgpu::ShaderSource::Wgsl(crate::shader::SSAO.source.into()),
    });
    let ssao_pipeline = build_fullscreen_pipeline(
        device,
        &ssao_pipeline_layout,
        &ssao_shader,
        wgpu::TextureFormat::R8Unorm,
        "rustlua-ssao-pipeline",
    );
    let ssao_params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-ssao-params"),
        size: 144,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let ssao_white = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-ssao-white"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &ssao_white,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[255u8],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(1),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let ssao_raw_view = ssao_white.create_view(&wgpu::TextureViewDescriptor::default());
    let ssao_blur_white = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-ssao-blur-white"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &ssao_blur_white,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[255u8],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(1),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let ssao_view = ssao_blur_white.create_view(&wgpu::TextureViewDescriptor::default());
    let ssao_blur_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rustlua-ssao-blur-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
    let ssao_blur_pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rustlua-ssao-blur-pipeline-layout"),
            bind_group_layouts: &[&ssao_blur_bind_group_layout],
            push_constant_ranges: &[],
        });
    let ssao_blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rustlua-ssao-blur-shader"),
        source: wgpu::ShaderSource::Wgsl(crate::shader::SSAO_BLUR.source.into()),
    });
    let ssao_blur_pipeline = build_fullscreen_pipeline(
        device,
        &ssao_blur_pipeline_layout,
        &ssao_blur_shader,
        wgpu::TextureFormat::R8Unorm,
        "rustlua-ssao-blur-pipeline",
    );
    let ssao_blur_params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-ssao-blur-params"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let ssr_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rustlua-ssr-bind-group-layout"),
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
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(176),
                },
                count: None,
            },
        ],
    });
    let ssr_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rustlua-ssr-pipeline-layout"),
        bind_group_layouts: &[&ssr_bind_group_layout],
        push_constant_ranges: &[],
    });
    let ssr_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rustlua-ssr-shader"),
        source: wgpu::ShaderSource::Wgsl(crate::shader::SSR.source.into()),
    });
    let ssr_pipeline = build_fullscreen_pipeline(
        device,
        &ssr_pipeline_layout,
        &ssr_shader,
        wgpu::TextureFormat::Rgba16Float,
        "rustlua-ssr-pipeline",
    );
    let ssr_params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-ssr-params"),
        size: 176,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let ssr_white = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-ssr-white"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &ssr_white,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&[0.0_f32, 0.0, 0.0, 0.0]),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let ssr_raw_view = ssr_white.create_view(&wgpu::TextureViewDescriptor::default());
    let ssr_blur_white = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-ssr-blur-white"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &ssr_blur_white,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&[0.0_f32, 0.0, 0.0, 0.0]),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let ssr_view = ssr_blur_white.create_view(&wgpu::TextureViewDescriptor::default());
    let ssr_history_white = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-ssr-history-white"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &ssr_history_white,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&[0.0_f32, 0.0, 0.0, 0.0]),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let ssr_history_view = ssr_history_white.create_view(&wgpu::TextureViewDescriptor::default());
    let ssr_blur_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rustlua-ssr-blur-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(144),
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
    let ssr_blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rustlua-ssr-blur-pipeline-layout"),
        bind_group_layouts: &[&ssr_blur_bind_group_layout],
        push_constant_ranges: &[],
    });
    let ssr_blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rustlua-ssr-blur-shader"),
        source: wgpu::ShaderSource::Wgsl(crate::shader::SSR_BLUR.source.into()),
    });
    let ssr_blur_pipeline = build_fullscreen_pipeline(
        device,
        &ssr_blur_pipeline_layout,
        &ssr_blur_shader,
        wgpu::TextureFormat::Rgba16Float,
        "rustlua-ssr-blur-pipeline",
    );
    let ssr_blur_params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-ssr-blur-params"),
        size: 144,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    PostProcessResources {
        post_process_shader,
        post_process_layout,
        post_process_pipeline_layout,
        post_process_pipeline,
        post_process_sampler,
        post_process_params,
        tonemap_layout,
        tonemap_pipeline_layout,
        tonemap_pipeline,
        tonemap_params,
        section_height_white,
        ssao_bind_group_layout,
        ssao_pipeline_layout,
        ssao_shader,
        ssao_pipeline,
        ssao_params,
        ssao_white,
        ssao_raw_view,
        ssao_blur_white,
        ssao_view,
        ssao_blur_bind_group_layout,
        ssao_blur_pipeline_layout,
        ssao_blur_shader,
        ssao_blur_pipeline,
        ssao_blur_params,
        ssr_bind_group_layout,
        ssr_pipeline_layout,
        ssr_shader,
        ssr_pipeline,
        ssr_params,
        ssr_white,
        ssr_raw_view,
        ssr_blur_white,
        ssr_view,
        ssr_history_white,
        ssr_history_view,
        ssr_blur_bind_group_layout,
        ssr_blur_pipeline_layout,
        ssr_blur_shader,
        ssr_blur_pipeline,
        ssr_blur_params,
    }
}
