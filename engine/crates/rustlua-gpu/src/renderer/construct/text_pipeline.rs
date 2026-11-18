//! 2D 贴图 / 文本 / 矢量字的管线与缓冲（拆分批 B / 基础管线）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里从 `let shader = device.create_shader_module(TEXTURED_SHADER)` 到
//! `vertex_buffer` 结束（**112 行、10 处资源创建**，15 个产出）。
//!
//! ## 依赖面
//!
//! 5 个输入：`device` / `format` / `storage_capable` / `storage_usage` / `pipeline_layout`。
//! **不依赖 `camera_bind_group_layout`** —— 它只借基础的 textured pipeline layout。
//!
//! ## 为什么它是依赖图的第二把钥匙
//!
//! 探查结论：这一段产出的 `sampler` / `shader` / `pipeline` 被下游三个簇引用：
//!
//! - **D 相机与光照**（`shadow_sampler` / `point_shadow_plain_sampler` / `albedo_sampler`
//!   都从 `sampler` 派生参数）
//! - **J 环境**（`environment_sampler` 同样派生）
//! - **G ESM**（`esm_shader` / `esm_pipeline_layout` 借 `shader` / `pipeline` 的创建方式）
//!
//! 拆掉它之后 D 才能拆，D 拆了 E / I / H 才能拆。
//!
//! ## 语义零变化的等价点
//!
//! 1. **能力不足时矢量文字不构造**（`storage_capable` 为 false 时 WebGL2）：
//!    它的两个绑定是片元存储缓冲，建了会直接吃 wgpu 校验错误，而 wasm 上那是 abort。
//!    这时 `vector_*` 换占位管线，宿主该用 SDF 文字。
//! 2. **采样器口径不同**：`sampler` 是 Nearest（未登记贴图走它，要逐字节维持原样），
//!    `sdf_sampler` 是 Linear（距离场插值）。
//! 3. **顶点缓冲容量**：`vertex_buffer` 用 `INITIAL_2D_VERTEX_BUFFER_BYTES`，
//!    矢量字三个缓冲各有自己的初始容量，不要统一。

use wgpu::{Buffer, Device, PipelineLayout, RenderPipeline, Sampler, ShaderModule, TextureFormat};

use crate::renderer::INITIAL_2D_VERTEX_BUFFER_BYTES;
use crate::shader::{TEXTURED_SHADER, VECTOR_TEXT_SHADER};

use super::{build_textured_pipeline, build_vector_text_pipeline, placeholder_pipeline};

/// 2D 贴图 / 文本 / 矢量字的全部资源（15 个）。
pub(crate) struct TextPipelineResources {
    pub shader: ShaderModule,
    pub pipeline: RenderPipeline,
    pub sampler: Sampler,
    pub texture_samplers: Vec<Sampler>,
    pub sdf_sampler: Sampler,
    pub pipeline_sdf: RenderPipeline,
    pub vector_bind_group_layout: wgpu::BindGroupLayout,
    pub vector_layout: PipelineLayout,
    pub vector_shader: ShaderModule,
    /// 能力不足时这里是**占位管线**（不是 Option）——布局仍合法，只是画不出东西。
    pub vector_pipeline: RenderPipeline,
    pub vector_vertex_buffer: Buffer,
    pub vector_glyph_buffer: Buffer,
    pub vector_segment_buffer: Buffer,
    pub vertex_buffer: Buffer,
}

/// 建 2D 贴图 / 文本 / 矢量字的管线与缓冲。
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_pipeline(
    device: &Device,
    format: TextureFormat,
    storage_capable: bool,
    storage_usage: wgpu::BufferUsages,
    pipeline_layout: &PipelineLayout,
) -> TextPipelineResources {
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
    // 逐纹理采样器表：拆分批 2 搬到 `construct/texture.rs`。
    // 这一段自包含（只吃 device + 两个静态方法），是按**依赖关系**而非行号切分的样板。
    let texture_samplers = super::texture::texture_samplers(&device);
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

    TextPipelineResources {
        shader,
        pipeline,
        sampler,
        texture_samplers,
        sdf_sampler,
        pipeline_sdf,
        vector_bind_group_layout,
        vector_layout,
        vector_shader,
        vector_pipeline,
        vector_vertex_buffer,
        vector_glyph_buffer,
        vector_segment_buffer,
        vertex_buffer,
    }
}
