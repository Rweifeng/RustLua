//! 方向光 ESM（指数阴影贴图）通道（拆分批 G / 基础管线）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里从 `esm_bind_group_layout` 到 `esm_pipeline` 结束
//! （**84 行、6 处资源创建**）。
//!
//! ## 依赖面
//!
//! **只有 2 个输入**：`device` 与 `shadow_sample_view`（拆分批 D 的产物）。
//!
//! 探查时以为还要 B 的 `shader` / `pipeline`，实际 ESM 的着色器与管线都是**自己现建**的
//! ——它用的 `crate::shader::ESM` 是独立的一份，不借 textured 那条路。
//!
//! ## ESM 是什么、为什么和 CSM 并存
//!
//! CSM（级联阴影贴图）是**深度**比较；ESM 存的是 `exp(k·z)`，采样时再做对数还原
//! ——好处是可以**线性过滤**，软阴影过渡更平滑，代价是精度和额外一张 `R16Float`。
//! 两者都在用：CSM 管近处细节，ESM 管远处的软边。
//!
//! ## 语义零变化的等价点
//!
//! 1. **每层一个 uniform buffer + 一个绑定组**（`4 层`）：ESM 的光照矩阵逐层不同，
//!    不能像 CSM 那样共享。层数由 `ESM_LAYERS` 决定。
//! 2. **管线用 `vs_main` / `fs_main` 两个入口**（和别的管线不同）：
//!    ESM 的顶点着色器只画一个全屏三角形，不做几何变换。
//! 3. **目标格式固定 `R16Float`**，不跟表面 `format` 走 —— 它写的是离屏纹理，
//!    不是屏幕。这里的 `format` 参数只用于对齐管线别的字段。
//! 4. **`esm_layer_views` 不在本模块建**（拆分批 F 的 `shadow_textures` 产出），
//!    本模块只消费它。

use wgpu::util::DeviceExt as _;
use wgpu::{
    BindGroup, BindGroupLayout, Buffer, Device, PipelineLayout, RenderPipeline, ShaderModule,
    TextureView,
};

use crate::renderer::ESM_LAYERS;

/// ESM 通道的全部资源。
pub(crate) struct EsmShadow {
    pub bind_group_layout: BindGroupLayout,
    pub layer_buffers: [Buffer; ESM_LAYERS],
    pub bind_groups: [BindGroup; ESM_LAYERS],
    pub shader: ShaderModule,
    pub pipeline_layout: PipelineLayout,
    pub pipeline: RenderPipeline,
}

/// 建 ESM 的布局、逐层 buffer/绑定组与管线。
pub(crate) fn esm_shadow(device: &Device, shadow_sample_view: &TextureView) -> EsmShadow {
    let esm_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rustlua-esm-bind-group-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
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
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            },
        ],
    });
    let esm_layer_buffers: [wgpu::Buffer; ESM_LAYERS] = std::array::from_fn(|layer| {
        let words = [layer as u32, 0, 0, 0];
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-esm-layer"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::UNIFORM,
        })
    });
    let esm_bind_groups: [wgpu::BindGroup; ESM_LAYERS] = std::array::from_fn(|layer| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-esm-bind-group"),
            layout: &esm_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&shadow_sample_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: esm_layer_buffers[layer].as_entire_binding(),
                },
            ],
        })
    });
    let esm_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rustlua-esm-shadow-shader"),
        source: wgpu::ShaderSource::Wgsl(crate::shader::ESM.source.into()),
    });
    let esm_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rustlua-esm-pipeline-layout"),
        bind_group_layouts: &[&esm_bind_group_layout],
        push_constant_ranges: &[],
    });
    let esm_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("rustlua-esm-pipeline"),
        layout: Some(&esm_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &esm_shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &esm_shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R16Float,
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
    EsmShadow {
        bind_group_layout: esm_bind_group_layout,
        layer_buffers: esm_layer_buffers,
        bind_groups: esm_bind_groups,
        shader: esm_shader,
        pipeline_layout: esm_pipeline_layout,
        pipeline: esm_pipeline,
    }
}
