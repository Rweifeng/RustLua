//! 世界 Sprite 通道的资源（拆分批 7 / 9）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里 `// WP6 世界 Sprite 通道` 那段（86 行、6 处资源创建）。
//!
//! ## 依赖面
//!
//! 相对独立：只吃 `device` / `format` / `capabilities`（判 HDR 变体要否）/
//! `texture_bind_group_layout`（group1 复用全局贴图布局）/ `DepthPass`。
//! **不依赖** `world_sprite_depth_view`（那是 1430 行的 `create_depth_view`，留在原处）。
//!
//! ## 语义零变化的等价点
//!
//! 1. **两条管线只差深度策略**：不透明写深度（可与网格互相遮挡）、半透明测深度不写深度
//!    （画家顺序叠加）。除 `DepthPass` 外必须逐字段相同——否则"同一份几何换一个策略"
//!    就变成两条不同的渲染路径。
//! 2. **HDR 变体只在 `capabilities.hdr_usable()` 时创建**，否则 `(None, None)`。
//!    这个 `if` 是启动预算的一部分（见 `pipeline_cache` 的变体策略），不是可选优化。
//! 3. 顶点缓冲容量取 `WORLD_SPRITE_BUFFER_BYTES`（`world_sprite` 模块的常量），
//!    不在这里重新算。

use wgpu::util::DeviceExt as _;
use wgpu::{
    BindGroup, BindGroupLayout, Buffer, Device, PipelineLayout, RenderPipeline, TextureFormat,
};

use crate::renderer::world_sprite::WORLD_SPRITE_BUFFER_BYTES;
use crate::renderer::{DepthPass, RendererCapabilities};
use crate::shader::WORLD_SPRITE_SHADER;

use super::build_world_sprite_pipeline;

/// 世界 Sprite 通道的一整套资源。
pub(crate) struct WorldSpriteResources {
    pub camera_layout: BindGroupLayout,
    /// 后文（`layer bind groups` 那段）还要用它，所以一并带出来。
    pub pipeline_layout: PipelineLayout,
    pub camera_buffer: Buffer,
    pub camera_bind_group: BindGroup,
    pub pipeline: RenderPipeline,
    pub opaque_pipeline: RenderPipeline,
    pub hdr_pipeline: Option<RenderPipeline>,
    pub hdr_opaque_pipeline: Option<RenderPipeline>,
    pub vertex_buffer: Buffer,
}

/// 建世界 Sprite 通道的全部资源。
pub(crate) fn world_sprite_resources(
    device: &Device,
    format: TextureFormat,
    capabilities: &RendererCapabilities,
    texture_bind_group_layout: &BindGroupLayout,
) -> WorldSpriteResources {
    // WP6 世界 Sprite 通道：独立的一套资源（2D 相机 uniform + 顶点缓冲 + 深度附件）。
    // 与 overlay 路径互不影响：宿主不喂世界顶点时它一次也不会被用到。
    let world_sprite_camera_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rustlua-world-sprite-camera-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
    let world_sprite_camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("rustlua-world-sprite-camera"),
        contents: bytemuck::bytes_of(&[0.0_f32; 20]),
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
    let (hdr_world_sprite_pipeline, hdr_world_sprite_opaque_pipeline) = if capabilities.hdr_usable()
    {
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
    WorldSpriteResources {
        camera_layout: world_sprite_camera_layout,
        pipeline_layout: world_sprite_pipeline_layout,
        camera_buffer: world_sprite_camera_buffer,
        camera_bind_group: world_sprite_camera_bind_group,
        pipeline: world_sprite_pipeline,
        opaque_pipeline: world_sprite_opaque_pipeline,
        hdr_pipeline: hdr_world_sprite_pipeline,
        hdr_opaque_pipeline: hdr_world_sprite_opaque_pipeline,
        vertex_buffer: world_sprite_buffer,
    }
}
