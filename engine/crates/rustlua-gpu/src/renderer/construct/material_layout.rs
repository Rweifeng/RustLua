//! 材质 3D 的绑定组布局（拆分批 C / 基础布局）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里 `material_entries` 的逐项构建 + `material_bind_group_layout`
//! （**110 行，是剩下最大的独立块**）。
//!
//! ## 依赖面（实测，最干净的一块）
//!
//! 只有 2 个输入：`device` / `storage_capable`。零其他依赖。
//!
//! ## 为什么它是最后一把钥匙
//!
//! 三条管线布局（拆分批次 K）**全部**要用它：
//!
//! | pipeline layout | 用到的 group |
//! |---|---|
//! | `mesh_pipeline_layout` | camera + **material** |
//! | `pbr_pipeline_layout` | camera + **material** + environment |
//! | `shadow_pipeline_layout` | shadow + **material** |
//!
//! 所以 C → K → L 是一条链：材质布局没了，管线布局拼不出来；管线布局没了，
//! 变体编译也跑不了。
//!
//! ## 语义零变化的等价点
//!
//! 1. **`material_entries` 是逐项 push/extend 出来的**，不是一次性字面量——
//!    顺序就是 binding 编号的声明顺序，不能重排。
//! 2. **binding 2 是逐实例模型矩阵**：有存储缓冲时是 storage 数组
//!    （支持实例化合批），没有时（WebGL2）换成 64 字节 uniform。
//!    两条路都真画，只是取法不同——不是"能力不足就占位"。
//! 3. **回滚档（Legacy）的布局必须保持不变**：多一组会让旧着色器绑定不上，
//!    也会让 `pbr=off` 的像素门失去"逐字节不变"的前提。

use wgpu::{BindGroupLayout, Device};

/// 建材质 3D 的绑定组布局（group1）。
pub(crate) fn material_bind_group_layout(
    device: &Device,
    storage_capable: bool,
) -> BindGroupLayout {
    let mut material_entries: Vec<wgpu::BindGroupLayoutEntry> = vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                // P3-R02：6×vec4 = 基色 / 表面 / 自发光 / 参数 / 贴图开关 / 动画。
                // P3-R162–165：+4×vec4 = 清漆 / SSS / 虹彩 / 绒毛（共 10×vec4 = 160 字节）。
                min_binding_size: wgpu::BufferSize::new(crate::renderer::MATERIAL_UNIFORM_BYTES),
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
    // 材质布局的 binding 2 是**逐实例模型矩阵**。有存储缓冲的设备走 storage 数组（支持
    // 实例化合批）；没有的设备（WebGL2）换成 64 字节 uniform——同一批 draw，只是取法不同。
    // 两条路都真画，不再"能力不足就整条 3D 通道占位"（见 `passes.rs` 的帧内 draw 与
    // `arena.rs` 的绑定端）。
    material_entries.push(wgpu::BindGroupLayoutEntry {
        binding: 2,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: if storage_capable {
                wgpu::BufferBindingType::Storage { read_only: true }
            } else {
                wgpu::BufferBindingType::Uniform
            },
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(64),
        },
        count: None,
    });
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

    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rustlua-material3d-bind-group-layout"),
        entries: &material_entries,
    })
}
