//! 相机绑定组与平面反射资源（拆分批 H / 基础管线）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里 `camera_bind_group` + `planar_camera_*` + `planar_reflection_*`
//! （**142 行**）。
//!
//! ## 依赖面：10 个输入，全部来自已拆批次
//!
//! | 依赖 | 批次 |
//! |---|---|
//! | `camera_bind_group_layout` | A |
//! | `camera` / `camera_buffer` / `light_buffer` / `shadow_camera_buffer` | D |
//! | `shadow_sampler` / `point_shadow_plain_sampler` / `point_shadow_uniform_buffer` / `point_shadow_array_view` | D/F |
//! | `esm_array_view` | F |
//!
//! 这也是为什么 H 只能排在最后——它要把前面几乎所有绑定类产物都拿到手。
//!
//! ## 语义零变化的等价点
//!
//! 1. **镜像相机绑定组与主相机绑定组逐项相同，只有 binding 0 指向另一块 buffer**
//!    ——反射 pass 因此能原样复用材质/环境绑定，不用第二套 layout。
//! 2. **镜像相机 uniform 是 176 字节**（11×vec4，P3-R209 起含封口深度），与主相机同尺寸。
//! 3. **占位反射图是 1×1 透明**：未启用平面反射时 Tonemap 绑它，**绑定组形状恒定**，
//!    着色器用 `alpha < 0.5` 判定"没命中"——避开绑定组形状切换的时序问题。
//! 4. 深度视图走 `Renderer::create_depth_view`，与别处的深度附件同一套创建方式。

use wgpu::{BindGroup, Buffer, Device, Queue, Texture, TextureView};

use crate::renderer::Renderer;

/// 相机绑定组与平面反射资源。
pub(crate) struct CameraBindings {
    pub camera_bind_group: BindGroup,
    pub planar_camera_buffer: Buffer,
    pub planar_camera_bind_group: BindGroup,
    pub planar_reflection_texture: Texture,
    pub planar_reflection_view: TextureView,
    pub planar_reflection_depth: TextureView,
}

/// 建主相机绑定组与平面反射的全部资源。
#[allow(clippy::too_many_arguments)]
pub(crate) fn camera_bindings(
    device: &Device,
    queue: &Queue,
    camera_bind_group_layout: &wgpu::BindGroupLayout,
    camera_buffer: &Buffer,
    light_buffer: &Buffer,
    shadow_camera_buffer: &Buffer,
    point_shadow_uniform_buffer: &Buffer,
    point_shadow_array_view: &TextureView,
    point_shadow_plain_sampler: &wgpu::Sampler,
    shadow_sampler: &wgpu::Sampler,
    esm_array_view: &TextureView,
    shadow_sample_view: &TextureView,
) -> CameraBindings {
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
                resource: wgpu::BindingResource::TextureView(&shadow_sample_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: point_shadow_uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&point_shadow_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Sampler(&shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&esm_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::Sampler(&point_shadow_plain_sampler),
            },
        ],
    });
    // P3-R170：平面反射的镜像相机 uniform 与绑定组。除 binding 0 指向另一块 buffer 外
    // 与主相机绑定组逐项相同——反射 pass 因此能原样复用材质/环境绑定。
    // P3-R209：尺寸跟 Camera 结构走（11×vec4 = 176 字节），反射 pass 也跑同一份 PBR 着色器。
    let planar_camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-planar-camera"),
        size: 176,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let planar_camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("rustlua-planar-camera-bind-group"),
        layout: &camera_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: planar_camera_buffer.as_entire_binding(),
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
                resource: wgpu::BindingResource::TextureView(&shadow_sample_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: point_shadow_uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&point_shadow_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Sampler(&shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&esm_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::Sampler(&point_shadow_plain_sampler),
            },
        ],
    });
    // 未启用平面反射时 Tonemap 绑定这个 1×1 透明占位图：绑定组形状恒定，
    // 着色器用 alpha<0.5 判定"没命中"，与绑定组形状切换的时序问题无关。
    let planar_reflection_texture = device.create_texture(&wgpu::TextureDescriptor {
        // 回读门要能证明"没跑时仍是这张占位图"，所以占位图也带 COPY_SRC。
        label: Some("rustlua-planar-reflection-placeholder"),
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
            texture: &planar_reflection_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[0u8; 8],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let planar_reflection_view =
        planar_reflection_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let planar_reflection_depth = Renderer::create_depth_view(&device, 1, 1);
    CameraBindings {
        camera_bind_group,
        planar_camera_buffer,
        planar_camera_bind_group,
        planar_reflection_texture,
        planar_reflection_view,
        planar_reflection_depth,
    }
}
