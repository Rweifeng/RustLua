//! GPU 粒子 instanced 管线的**布局**（拆分批 8 / 9）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里 `P3-R07-3` 那段（67 行、3 处资源创建）。
//!
//! ## 依赖面
//!
//! 依赖 `storage_capable`（无存储缓冲的设备不建：binding 1 是只读存储，
//! `create_bind_group_layout` 会直接报校验错误，而 wasm 上未捕获的校验错误是 abort），
//! 以及**两个已在别处的布局**：`world_sprite_camera_layout`（批次 7 的产物）与
//! `texture_bind_group_layout`（216 行，仍在 `mod.rs`）。
//!
//! 所以这批**只能在批次 7 之后做** —— 它引用了世界 Sprite 的相机布局。
//! 这也是为什么它没按行号顺序排在批次 3 后面。
//!
//! ## 这里只建布局，不建管线（不是遗漏）
//!
//! 管线要按目标格式（LDR/HDR）延迟编译，理由与 PBR 变体一样：启动预算 `compile_ms`
//! 里没有理由为一条"没配 GPU 粒子就永远用不到"的管线付钱。布局必须留在渲染器上——
//! 延迟编译与（未来的）热重载都要按同一份描述符重建。
//!
//! 语义零变化：返回的三元组在无存储缓冲时是 `(None, None, None)`，与搬迁前一致。

use wgpu::{BindGroupLayout, Device, PipelineLayout};

/// 三条可选布局：draw（group2）、pipeline layout、depth（group3）。
///
/// `None` = 设备没有存储缓冲，GPU 粒子在这些设备上走 CPU
/// （`configure_gpu_particles` 会返回 Unsupported）。
pub(crate) type GpuParticleLayouts = (
    Option<BindGroupLayout>,
    Option<PipelineLayout>,
    Option<BindGroupLayout>,
);

/// 建 GPU 粒子 instanced 管线的三条布局。
pub(crate) fn gpu_particle_layouts(
    device: &Device,
    storage_capable: bool,
    world_sprite_camera_layout: &BindGroupLayout,
    texture_bind_group_layout: &BindGroupLayout,
) -> GpuParticleLayouts {
    // P3-R07-3：GPU 粒子 instanced 管线的**布局**（group2 = 参数 uniform + 状态只读存储）。
    // 这里只建两条布局，不建管线：管线要按目标格式（LDR/HDR）延迟编译，理由与 R02 的 PBR
    // 变体一样——启动预算（`compile_ms`）里没有理由为一条"没配 GPU 粒子就永远用不到"的
    // 管线付钱。布局必须留在渲染器上：延迟编译与（未来的）热重载都要按同一份描述符重建。
    // 无存储缓冲的设备（WebGL2 即如此）不建这两条布局：binding 1 是只读存储，
    // `create_bind_group_layout` 会直接报校验错误，而 wasm 上的未捕获校验错误是 abort。
    // GPU 粒子在这些设备上本来就走 CPU，`configure_gpu_particles` 会返回 Unsupported。
    let (gpu_particle_draw_layout, gpu_particle_pipeline_layout, gpu_particle_depth_layout) =
        if storage_capable {
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
            // P3-R107：group3 = 场景深度，粒子片元自己做深度测试 + 软边。
            let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rustlua-gpu-particles-depth-layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });
            // group0/group1 与世界 Sprite 管线**共用同两个布局对象**，所以相机绑定组与白贴图绑定组
            // 可以直接复用——粒子不需要自己的相机 uniform，也不需要第二张贴图。
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rustlua-gpu-particles-pipeline-layout"),
                bind_group_layouts: &[
                    &world_sprite_camera_layout,
                    &texture_bind_group_layout,
                    &draw_layout,
                    &depth_layout,
                ],
                push_constant_ranges: &[],
            });
            (Some(draw_layout), Some(pipeline_layout), Some(depth_layout))
        } else {
            (None, None, None)
        };
    (
        gpu_particle_draw_layout,
        gpu_particle_pipeline_layout,
        gpu_particle_depth_layout,
    )
}
