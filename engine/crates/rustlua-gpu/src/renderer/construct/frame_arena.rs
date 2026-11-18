//! 帧级环形缓冲与地形 albedo 采样器（拆分批 E / 基础管线）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里 `P0-①` 那段（26 行、3 处资源创建）。
//!
//! ## 依赖面
//!
//! `device` / `storage_usage`（能力不足时不加 `STORAGE`）/ `FRAME_ARENA_INITIAL_BYTES`。
//!
//! ## 语义零变化的等价点
//!
//! 1. **对齐量取自设备限制**（`min_uniform_buffer_offset_alignment` /
//!    `min_storage_buffer_offset_alignment`），`max(1)` 是防零除——某些后端报 0。
//!    这两条跨 DX12/Vulkan/Metal/GL 都成立，不能写死。
//! 2. **instance_arena 的 usage 额外带 `VERTEX`**：PBR 的逐实例法线矩阵与模型矩阵
//!    共用同一块环形缓冲（不同偏移），所以它既要能当 storage 又要能当 vertex。
//! 3. **albedo_sampler 是三线性**（`mipmap_filter: Linear`）：只有内置地形细节图集
//!    上传 mip 链，三线性让远景按米制 LOD 平滑过渡。别的采样器多为 Nearest。

use wgpu::{Buffer, Device, Sampler};

use crate::renderer::FRAME_ARENA_INITIAL_BYTES;

/// 帧环与地形采样器。
pub(crate) struct FrameArena {
    pub uniform_align: u64,
    pub storage_align: u64,
    pub material_arena: Buffer,
    pub instance_arena: Buffer,
    pub albedo_sampler: Sampler,
}

/// 建帧级环形缓冲与 albedo 采样器。
pub(crate) fn frame_arena(device: &Device, storage_usage: wgpu::BufferUsages) -> FrameArena {
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
        // 只有内置地形细节图集上传 mip 链；三线性过滤让远景按米制 LOD 平滑过渡。
        mipmap_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    FrameArena {
        uniform_align,
        storage_align,
        material_arena,
        instance_arena,
        albedo_sampler,
    }
}
