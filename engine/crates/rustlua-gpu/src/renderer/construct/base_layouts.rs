//! 相机 3D 的绑定组布局（拆分批 A / 基础布局）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里 `camera_bind_group_layout`（**101 行，是全部 layout 里最大的一个**）。
//!
//! ## 为什么它是最好拆的对象
//!
//! 依赖探查结论：这一段**只吃 `device`**，零其他输入；产出**恰好一个** `BindGroupLayout`。
//! 它是整张依赖图（A→B/D/C→E/I/H→K→L）的根，而根的内容只有这一个 101 行的布局。
//!
//! 之前把它当成"障碍"是因为它**物理上夹在** `texture_bind_group_layout`(216) 与
//! `shader`(364) 之间——但"夹在中间"和"依赖"是两回事。它不依赖任何邻居。
//!
//! ## 被谁用
//!
//! 4 处：主相机绑定组、平面反射的镜像相机绑定组、`mesh_pipeline_layout`、`pbr_pipeline_layout`。
//! 它们全都可以把这个 layout 当参数拿。
//!
//! ## 语义零变化的等价点
//!
//! **binding 编号本身是契约**——着色器按编号取资源。移动位置不能动任何 `binding:` 值，
//! `min_binding_size` 也不能改：
//!
//! - binding 0 = 176 B（11×vec4，P3-R209 起含封口深度）
//! - binding 1 = `LIGHT_UNIFORM_BYTES`（方向光 48B + 16 盏点光 × 32B）
//! - binding 2..5 = 256 B（四级阴影，4 × mat4）

use wgpu::Device;

/// 建相机 3D 的绑定组布局（group0）。
pub(crate) fn camera_bind_group_layout(device: &Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rustlua-camera3d-bind-group-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                // P3-R02：PBR 片元要读相机世界位置算视线；旧网格着色器只声明 64 字节
                // 结构，读写同一块 buffer 的前半段。尺寸 176 = 11×vec4（P3-R209 起含封口深度）。
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(176),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    // 方向光 48B + 16 盏点光 × 32B = 560B（见 `light_uniform_bytes`）。
                    min_binding_size: wgpu::BufferSize::new(crate::renderer::LIGHT_UNIFORM_BYTES),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    // 四级阴影：4 × mat4 = 256B。
                    min_binding_size: wgpu::BufferSize::new(256),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
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
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    // 48 个面矩阵 + 8 组点光位置/远平面 = 3200B。
                    min_binding_size: wgpu::BufferSize::new(
                        crate::renderer::POINT_SHADOW_UNIFORM_BYTES as u64,
                    ),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::CubeArray,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 8,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 11,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}
