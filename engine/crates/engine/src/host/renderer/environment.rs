//! P3-R03-3：环境 cubemap 的 GPU 上传与 group 2 绑定。
//!
//! `.rlenv` 在导入期已经烘焙好 6 面 + 逐 mip 的 GGX 预过滤结果，运行时只做两件事：
//!
//! 1. 把每级 mip 的 6 个面按 `Rgba16Float` 写进一张 cube 纹理（不重新过滤、不重排 mip）；
//! 2. 建一个与设备能力匹配的采样器/绑定组：能线性过滤就用 `Linear`，否则用 `Nearest`
//!    并把纹理声明为不可过滤——Tier B 的最近邻档位在这里落地，不是静默降级。
//!
//! 环境 uniform 的 `params.w` 记录 `mip_count - 1`，着色器据此把 roughness 映射到 mip；
//! 预过滤的约定是 `roughness = mip / (mip_count - 1)`，与这里的选择完全对应。

use rustlua_assets::env_asset::{EnvironmentAsset, FACE_COUNT, f32_to_f16};

/// 环境 cubemap 上传失败的原因（全部是资产/调用方问题，不 panic）。
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentMapError {
    #[error("环境资产没有 mip 级")]
    NoMips,
    #[error("环境面尺寸为 0")]
    ZeroFaceSize,
    #[error("环境面尺寸 {0} 不是 2 的幂")]
    FaceSizeNotPowerOfTwo(u32),
    #[error("环境面尺寸 {face_size} 超过 mip 链上限 {limit}")]
    FaceSizeExceedsLimit { face_size: u32, limit: u32 },
    #[error("环境 mip 数 {mip_count} 超过面尺寸 {face_size} 允许的级数")]
    TooManyMips { face_size: u32, mip_count: u32 },
    #[error("环境 mip {mip} 尺寸 {actual} 与面尺寸 {face_size} 不匹配（应为 {expected}）")]
    MipSizeMismatch {
        mip: usize,
        face_size: u32,
        expected: u32,
        actual: u32,
    },
    #[error("环境 mip {mip} 面 {face} 有 {actual} 个 texel，应为 {expected}")]
    FaceLenMismatch {
        mip: usize,
        face: usize,
        expected: usize,
        actual: usize,
    },
    #[error("环境 mip {mip} 面 {face} 的第 {texel} 个 texel 含非有限值")]
    NonFiniteTexel {
        mip: usize,
        face: usize,
        texel: usize,
    },
}

/// 已上传的环境 cubemap（纹理本身由调用方持有，防止被提前释放）。
pub struct GpuEnvironmentMap {
    pub(crate) _texture: wgpu::Texture,
    pub(crate) face_size: u32,
    pub(crate) mip_count: u32,
}

impl GpuEnvironmentMap {
    /// 着色器采样用的最大 LOD（`mip_count - 1`，至少 0）。
    #[must_use]
    pub(crate) const fn max_lod(&self) -> f32 {
        self.mip_count.saturating_sub(1) as f32
    }
}

/// 建环境 group 2 的布局：0=uniform，1=cubemap，2=sampler。
///
/// `filterable` 来自设备能力：不支持浮点线性过滤时，纹理与采样器都按不可过滤声明，
/// 否则布局校验会在部分后端直接失败。
pub(crate) fn bind_group_layout(device: &wgpu::Device, filterable: bool) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rustlua-environment-bind-group-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(160),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable },
                    view_dimension: wgpu::TextureViewDimension::Cube,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(if filterable {
                    wgpu::SamplerBindingType::Filtering
                } else {
                    wgpu::SamplerBindingType::NonFiltering
                }),
                count: None,
            },
        ],
    })
}

/// 采样器：能力允许时三轴线性，否则全部最近邻（mip 也按最近级选）。
pub(crate) fn sampler(device: &wgpu::Device, filterable: bool) -> wgpu::Sampler {
    let filter = if filterable {
        wgpu::FilterMode::Linear
    } else {
        wgpu::FilterMode::Nearest
    };
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("rustlua-environment-sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: filter,
        ..Default::default()
    })
}

/// 没有环境时的 1×1 黑色 cube：布局与 PBR 管线只有一种，不必为 IBL 开关切布局。
pub(crate) fn fallback_view(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    const SIZE: u32 = 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-environment-fallback-cube"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: FACE_COUNT as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // 6 个面各 1 个 RGBA8 黑 texel。
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[0_u8; FACE_COUNT * 4],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SIZE * 4),
            rows_per_image: Some(SIZE),
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: FACE_COUNT as u32,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("rustlua-environment-fallback-cube-view"),
        dimension: Some(wgpu::TextureViewDimension::Cube),
        base_array_layer: 0,
        array_layer_count: Some(FACE_COUNT as u32),
        ..Default::default()
    })
}

/// 把 `.rlenv` 的 mip 链逐级写进一张 cube 纹理，返回纹理与 cube 视图。
pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    asset: &EnvironmentAsset,
) -> Result<(wgpu::Texture, wgpu::TextureView), EnvironmentMapError> {
    if asset.mips.is_empty() {
        return Err(EnvironmentMapError::NoMips);
    }
    if asset.face_size == 0 {
        return Err(EnvironmentMapError::ZeroFaceSize);
    }
    if !asset.face_size.is_power_of_two() {
        return Err(EnvironmentMapError::FaceSizeNotPowerOfTwo(asset.face_size));
    }
    let limit = device.limits().max_texture_dimension_2d;
    if asset.face_size > limit {
        return Err(EnvironmentMapError::FaceSizeExceedsLimit {
            face_size: asset.face_size,
            limit,
        });
    }
    let max_mips = asset.face_size.ilog2() + 1;
    if asset.mip_count() > max_mips {
        return Err(EnvironmentMapError::TooManyMips {
            face_size: asset.face_size,
            mip_count: asset.mip_count(),
        });
    }
    for (mip_index, mip) in asset.mips.iter().enumerate() {
        let expected = asset.face_size >> mip_index;
        if mip.size != expected {
            return Err(EnvironmentMapError::MipSizeMismatch {
                mip: mip_index,
                face_size: asset.face_size,
                expected,
                actual: mip.size,
            });
        }
        let texels = (mip.size as usize) * (mip.size as usize);
        for (face, data) in mip.faces.iter().enumerate() {
            if data.len() != texels {
                return Err(EnvironmentMapError::FaceLenMismatch {
                    mip: mip_index,
                    face,
                    expected: texels,
                    actual: data.len(),
                });
            }
            if let Some(texel) = data
                .iter()
                .position(|texel| !texel.iter().all(|value| value.is_finite()))
            {
                return Err(EnvironmentMapError::NonFiniteTexel {
                    mip: mip_index,
                    face,
                    texel,
                });
            }
        }
    }

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-environment-cube"),
        size: wgpu::Extent3d {
            width: asset.face_size,
            height: asset.face_size,
            depth_or_array_layers: FACE_COUNT as u32,
        },
        mip_level_count: asset.mip_count(),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (mip_index, mip) in asset.mips.iter().enumerate() {
        let bytes_per_row = mip.size * 4 * size_of::<u16>() as u32;
        for (face, data) in mip.faces.iter().enumerate() {
            let mut bytes = Vec::with_capacity(data.len() * 4 * size_of::<u16>());
            for texel in data {
                for channel in texel {
                    bytes.extend_from_slice(&f32_to_f16(*channel).to_le_bytes());
                }
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip_index as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: face as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(mip.size),
                },
                wgpu::Extent3d {
                    width: mip.size,
                    height: mip.size,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("rustlua-environment-cube-view"),
        dimension: Some(wgpu::TextureViewDimension::Cube),
        base_mip_level: 0,
        mip_level_count: None,
        base_array_layer: 0,
        array_layer_count: Some(FACE_COUNT as u32),
        ..Default::default()
    });
    Ok((texture, view))
}

/// 用当前 uniform 缓冲与 cubemap 视图重建 group 2。
pub(crate) fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    cube: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("rustlua-environment-bind-group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(cube),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
