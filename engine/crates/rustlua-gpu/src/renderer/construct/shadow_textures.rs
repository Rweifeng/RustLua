//! 白贴图与阴影纹理（拆分批 4 / 9）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里从 `// 1x1 white texture bound` 到 `esm_layer_views` 结束（115 行、5 处）。
//!
//! ## 依赖面
//!
//! 只吃 `device` / `queue` / `format` / `point_shadow_resolution`（建点光立方体阴影的边长档），
//! **不经过 `camera_bind_group_layout`** —— 这正是它能先于批次 3 拆掉的原因。
//!
//! ## 为什么阴影清屏不在这里
//!
//! `new_async` 紧接着有一段"未渲染的远级联清成全亮"（`if shadow_cascade_limit < 4`），
//! 它消费本模块产出的 `esm_layer_views`，并且要读 `shadow_cascade_limit` 这个**构造参数**。
//! 把它一起搬进来会把一个构造参数也拖进签名，而它本质上是"建完之后的一次性初始化动作"，
//! 不是"建资源"。所以清屏留在 `new_async`，本模块只负责造。
//!
//! ## 语义零变化的等价点
//!
//! 1. `point_shadow_resolution` 是**从实际建出的纹理回读**的（`texture.size().width`），
//!    不是参数回显——门的读数是"真的建了多大"。
//! 2. 点光立方体阴影的面边长按档建（P3-R200，512 ⇒ 48 MiB），档位来自构造参数。
//! 3. ESM 是 4 层 2048² `R16Float`，与 CSM 同分辨率（P3-R24）。

use rustlua_math::math3d::Mat4;
use wgpu::util::DeviceExt as _;
use wgpu::{Buffer, Device, Texture, TextureFormat, TextureView};

use crate::renderer::POINT_SHADOW_LAYERS;

/// 白贴图 + 阴影纹理的全部资源。
pub(crate) struct ShadowTextures {
    /// 1×1 白贴图：材质没有 base-color 贴图时绑定它。
    pub white_texture: Texture,
    pub white_albedo_view: TextureView,
    pub point_shadow_texture: Texture,
    /// **从纹理回读**的边长，不是构造参数的回显。
    pub point_shadow_resolution: u32,
    pub point_shadow_array_view: TextureView,
    pub point_shadow_face_views: [TextureView; POINT_SHADOW_LAYERS],
    pub point_shadow_uniform_buffer: Buffer,
    pub point_shadow_camera_buffers: [Buffer; POINT_SHADOW_LAYERS],
    pub esm_texture: Texture,
    pub esm_array_view: TextureView,
    pub esm_layer_views: [TextureView; crate::renderer::ESM_LAYERS],
}

/// 建白贴图与点光/方向光阴影纹理。
///
/// `point_shadow_resolution` 是构造参数（已 clamp 过的档位）。
pub(crate) fn shadow_textures(
    device: &Device,
    queue: &wgpu::Queue,
    _format: TextureFormat,
    point_shadow_resolution: u32,
) -> ShadowTextures {
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
    // 前八盏（最多 8 盏）点光的立方体阴影：8×6 个深度面，主着色器按光源位置找槽位、
    // 主着色器按 cube array 方向交给硬件跨面过滤。P3-R200：面边长按档建（512 ⇒ 48 MiB）。
    let point_shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-point-shadow-array"),
        size: wgpu::Extent3d {
            width: point_shadow_resolution,
            height: point_shadow_resolution,
            depth_or_array_layers: crate::renderer::POINT_SHADOW_LAYERS as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    // 以实际建出的纹理尺寸为准回填字段：门的读数是「真的建了多大」，不是参数回显。
    let point_shadow_resolution = point_shadow_texture.size().width;
    let point_shadow_array_view = point_shadow_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("rustlua-point-shadow-cube-array-view"),
        dimension: Some(wgpu::TextureViewDimension::CubeArray),
        base_array_layer: 0,
        array_layer_count: Some(crate::renderer::POINT_SHADOW_LAYERS as u32),
        ..wgpu::TextureViewDescriptor::default()
    });
    let point_shadow_face_views: [wgpu::TextureView; crate::renderer::POINT_SHADOW_LAYERS] =
        std::array::from_fn(|layer| {
            point_shadow_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("rustlua-point-shadow-face-view"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer as u32,
                array_layer_count: Some(1),
                ..wgpu::TextureViewDescriptor::default()
            })
        });
    let point_shadow_uniform_buffer =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustlua-point-shadow-uniform"),
            contents: &[0u8; crate::renderer::POINT_SHADOW_UNIFORM_BYTES],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
    let point_shadow_camera_buffers: [wgpu::Buffer; crate::renderer::POINT_SHADOW_LAYERS] =
        std::array::from_fn(|_layer| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-point-shadow-camera"),
                contents: bytemuck::cast_slice(&Mat4::identity().columns),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        });
    // P3-R24：方向光 ESM。4 层 2048² `R16Float`，与 CSM 同分辨率。
    let esm_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-esm-shadow"),
        size: wgpu::Extent3d {
            width: crate::renderer::ESM_SIZE,
            height: crate::renderer::ESM_SIZE,
            depth_or_array_layers: crate::renderer::ESM_LAYERS as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let esm_array_view = esm_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("rustlua-esm-array-view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let esm_layer_views: [wgpu::TextureView; crate::renderer::ESM_LAYERS] =
        std::array::from_fn(|layer| {
            esm_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("rustlua-esm-layer-view"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer as u32,
                array_layer_count: Some(1),
                ..Default::default()
            })
        });
    ShadowTextures {
        white_texture,
        white_albedo_view,
        point_shadow_texture,
        point_shadow_resolution,
        point_shadow_array_view,
        point_shadow_face_views,
        point_shadow_uniform_buffer,
        point_shadow_camera_buffers,
        esm_texture,
        esm_array_view,
        esm_layer_views,
    }
}
