//! P3-R03-1：`.hdr`（Radiance RGBE）解码与 `.rlenv` 离线烘焙。
//!
//! 运行时 crate 只读 [`rustlua_assets::env_asset`]；这里做导入期才会发生的事：
//!
//! 1. 解码 Radiance HDR（`image` 的 `hdr` feature），校验尺寸、有限、非负；
//! 2. equirect → cubemap（6 面，固定面顺序 `+X,-X,+Y,-Y,+Z,-Z`）；
//! 3. SH9 漫反射投影（已含 cosine convolution / π）；
//! 4. 镜面 GGX importance sampling 预过滤，roughness 按 mip 递增。
//!
//! **确定性**：采样序列是 Hammersley（无随机源、无时间/线程依赖），同一输入与同一
//! [`BakeOptions`] 得到逐字节相同的结果；测试钉住这条性质。
//!
//! 烘焙预算：默认 128px / 5 mip，RGBA16F 约 1 MiB；粗糙度 mip 用 64 个样本，
//! 低分辨率下足够稳定，且把单次导入耗时压在秒级。

use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use image::{ImageFormat, ImageReader, Limits};
use rustlua_assets::env_asset::{
    EnvironmentAsset, EnvironmentAssetError, EnvironmentMip, FACE_COUNT, SH_COEFFICIENTS, fnv1a64,
};
use rustlua_math::math3d::Vec3;
use thiserror::Error;

/// 默认 mip0 边长：128px / 5 mip 约 1 MiB（RGBA16F）。
pub const DEFAULT_FACE_SIZE: u32 = 128;
/// 默认 mip 数：roughness = 0, 0.25, 0.5, 0.75, 1。
pub const DEFAULT_MIP_COUNT: u32 = 5;
/// 单面边长硬上限：512 时 5 mip 约 4 MiB，再大就超出「单 env ≤8MB」预算。
pub const MAX_BAKE_FACE_SIZE: u32 = 512;
/// 每个预过滤 texel 的 GGX 样本数（固定值，保证确定性）。
pub const PREFILTER_SAMPLES: u32 = 64;

/// 源图尺寸上限：16 MP 的线性 RGB f32 约占 192 MiB。
pub const MAX_SOURCE_WIDTH: u32 = 8192;
pub const MAX_SOURCE_HEIGHT: u32 = 4096;
pub const MAX_SOURCE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_HDR_ALLOC_BYTES: u64 = 256 * 1024 * 1024;

/// 面中心方向表（与 `.rlenv` 的 `+X,-X,+Y,-Y,+Z,-Z` 顺序一致）。
///
/// `u` 向右增大、`v` 向下增大，取 DirectX/WebGPU 约定；镜像反射的一致性由导入层与
/// 运行时共用同一张表保证。
#[must_use]
pub fn face_direction(face: usize, x: u32, y: u32, size: u32) -> Vec3 {
    let size = size.max(1) as f32;
    let u = 2.0 * (x as f32 + 0.5) / size - 1.0;
    let v = 2.0 * (y as f32 + 0.5) / size - 1.0;
    let direction = match face {
        0 => Vec3::new(1.0, -v, -u),
        1 => Vec3::new(-1.0, -v, u),
        2 => Vec3::new(u, 1.0, v),
        3 => Vec3::new(u, -1.0, -v),
        4 => Vec3::new(u, -v, 1.0),
        _ => Vec3::new(-u, -v, -1.0),
    };
    direction.normalized().unwrap_or(Vec3::Z)
}

/// 解码后的 Radiance 图像：线性 RGB、alpha 视为 1。
#[derive(Clone, Debug, PartialEq)]
pub struct HdrImage {
    width: u32,
    height: u32,
    pixels: Vec<[f32; 3]>,
}

impl HdrImage {
    /// 构造并校验：尺寸在预算内、像素数匹配、全部有限且非负。
    pub fn new(
        width: u32,
        height: u32,
        pixels: Vec<[f32; 3]>,
    ) -> Result<Self, EnvironmentBakeError> {
        if width == 0 || height == 0 {
            return Err(EnvironmentBakeError::InvalidDimensions { width, height });
        }
        if width > MAX_SOURCE_WIDTH || height > MAX_SOURCE_HEIGHT {
            return Err(EnvironmentBakeError::TooLarge {
                width,
                height,
                max_width: MAX_SOURCE_WIDTH,
                max_height: MAX_SOURCE_HEIGHT,
            });
        }
        let expected = u64::from(width) * u64::from(height);
        if expected > MAX_SOURCE_PIXELS {
            return Err(EnvironmentBakeError::TooLarge {
                width,
                height,
                max_width: MAX_SOURCE_WIDTH,
                max_height: MAX_SOURCE_HEIGHT,
            });
        }
        if pixels.len() as u64 != expected {
            return Err(EnvironmentBakeError::PixelCount {
                expected,
                actual: pixels.len(),
            });
        }
        for (index, pixel) in pixels.iter().enumerate() {
            for (channel, value) in pixel.iter().enumerate() {
                if !value.is_finite() {
                    return Err(EnvironmentBakeError::NonFinite {
                        x: index as u32 % width,
                        y: index as u32 / width,
                        channel,
                    });
                }
                if *value < 0.0 {
                    return Err(EnvironmentBakeError::Negative {
                        x: index as u32 % width,
                        y: index as u32 / width,
                        channel,
                        value: *value,
                    });
                }
            }
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn pixels(&self) -> &[[f32; 3]] {
        &self.pixels
    }

    /// equirect 双线性采样。`u` 环绕，`v` 夹紧；方向约定见模块文档。
    ///
    /// `u = atan2(z, x) / 2π + 0.5`，`v = acos(y) / π`（v=0 是 +Y）。
    #[must_use]
    pub fn sample_equirect(&self, direction: Vec3) -> [f32; 3] {
        let direction = direction.normalized().unwrap_or(Vec3::Z);
        let v = (direction.y.clamp(-1.0, 1.0).acos() / std::f32::consts::PI).clamp(0.0, 1.0);
        let u = (direction.z.atan2(direction.x) / std::f32::consts::TAU + 0.5).rem_euclid(1.0);
        let fx = u * self.width as f32 - 0.5;
        let fy = (v * self.height as f32 - 0.5).clamp(0.0, self.height as f32 - 1.0);
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let x0 = (x0 as i64).rem_euclid(i64::from(self.width)) as u32;
        let x1 = (x0 + 1) % self.width;
        let y0 = y0 as u32;
        let y1 = (y0 + 1).min(self.height - 1);
        let p00 = self.pixel(x0, y0);
        let p10 = self.pixel(x1, y0);
        let p01 = self.pixel(x0, y1);
        let p11 = self.pixel(x1, y1);
        let mut rgb = [0.0_f32; 3];
        for channel in 0..3 {
            let top = p00[channel] * (1.0 - tx) + p10[channel] * tx;
            let bottom = p01[channel] * (1.0 - tx) + p11[channel] * tx;
            rgb[channel] = top * (1.0 - ty) + bottom * ty;
        }
        rgb
    }

    fn pixel(&self, x: u32, y: u32) -> [f32; 3] {
        self.pixels[(y * self.width + x) as usize]
    }
}

/// 烘焙参数。`face_size` 必须是 2 的幂，`mip_count` 不超过 `ilog2 + 1`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BakeOptions {
    pub face_size: u32,
    pub mip_count: u32,
}

impl Default for BakeOptions {
    fn default() -> Self {
        Self {
            face_size: DEFAULT_FACE_SIZE,
            mip_count: DEFAULT_MIP_COUNT,
        }
    }
}

impl BakeOptions {
    pub fn validate(&self) -> Result<(), EnvironmentBakeError> {
        if self.face_size == 0
            || !self.face_size.is_power_of_two()
            || self.face_size > MAX_BAKE_FACE_SIZE
        {
            return Err(EnvironmentBakeError::InvalidFaceSize(self.face_size));
        }
        if self.mip_count == 0 || self.mip_count > self.face_size.ilog2() + 1 {
            return Err(EnvironmentBakeError::InvalidMipCount {
                mip_count: self.mip_count,
                face_size: self.face_size,
            });
        }
        Ok(())
    }
}

/// 一次烘焙的产物与读数（`bake_ms`/`bytes` 进导入报告）。
#[derive(Clone, Debug)]
pub struct BakeReport {
    pub asset: EnvironmentAsset,
    pub source_width: u32,
    pub source_height: u32,
    pub bytes: usize,
    pub bake_ms: f64,
    pub samples_per_texel: u32,
}

impl BakeReport {
    /// 序列化结果；失败说明烘焙产物本身不合法（例如负 texel），不应写盘。
    pub fn encode(&self) -> Result<Vec<u8>, EnvironmentAssetError> {
        self.asset.encode()
    }
}

/// 读取并解码一个 `.hdr` 文件。
pub fn decode_hdr_file(path: &Path) -> Result<HdrImage, EnvironmentBakeError> {
    let bytes = std::fs::read(path).map_err(|source| EnvironmentBakeError::Io {
        path: path.display().to_string(),
        source,
    })?;
    decode_hdr(&bytes).map_err(|error| EnvironmentBakeError::WithPath {
        path: path.display().to_string(),
        source: Box::new(error),
    })
}

/// 解码 Radiance HDR。拒绝空文件、格式错误、尺寸超预算、非有限/负像素。
pub fn decode_hdr(bytes: &[u8]) -> Result<HdrImage, EnvironmentBakeError> {
    if bytes.is_empty() {
        return Err(EnvironmentBakeError::Empty);
    }
    let mut reader = ImageReader::new(Cursor::new(bytes));
    reader.set_format(ImageFormat::Hdr);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_WIDTH);
    limits.max_image_height = Some(MAX_SOURCE_HEIGHT);
    limits.max_alloc = Some(MAX_HDR_ALLOC_BYTES);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|error| EnvironmentBakeError::Decode(error.to_string()))?;
    let rgb = decoded.to_rgb32f();
    let (width, height) = (rgb.width(), rgb.height());
    // `to_rgb32f` 不会引入 NaN/负值，但导入边界仍走统一校验，坏数据只能更早失败。
    HdrImage::new(
        width,
        height,
        rgb.pixels()
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect(),
    )
}

/// 解码 + 哈希 + 烘焙。`source_hash` 取自原始 `.hdr` 字节，用于缓存失效。
pub fn bake_hdr_bytes(
    bytes: &[u8],
    options: BakeOptions,
) -> Result<BakeReport, EnvironmentBakeError> {
    let started = Instant::now();
    let image = decode_hdr(bytes)?;
    let source_hash = fnv1a64(bytes);
    bake_image(&image, source_hash, options, started)
}

/// P3-R03-4：导入期把 `.hdr` 源文件烘焙成**同名** `.rlenv`（运行时真正读的那份）。
///
/// 幂等：产物已是最新（逐字节相同）时不写盘，返回 `Ok(false)`；写了新文件返回 `Ok(true)`。
/// 先写 `.tmp` 再 `rename`，与导入缓存同一套防撕裂口径——半途崩溃不能留下半个 `.rlenv`
/// 让运行时把坏数据塞进 GPU。
pub fn bake_hdr_file(source: &Path, options: BakeOptions) -> Result<bool, EnvironmentBakeError> {
    let path = source.display().to_string();
    let bytes = std::fs::read(source).map_err(|source_error| EnvironmentBakeError::Io {
        path: path.clone(),
        source: source_error,
    })?;
    let encoded = bake_hdr_bytes(&bytes, options)?.encode()?;
    let product = source.with_extension(RLENV_EXTENSION);
    if std::fs::read(&product).is_ok_and(|existing| existing == encoded) {
        return Ok(false);
    }
    let temporary = product.with_extension("tmp");
    std::fs::write(&temporary, &encoded).map_err(|source_error| EnvironmentBakeError::Io {
        path: temporary.display().to_string(),
        source: source_error,
    })?;
    std::fs::rename(&temporary, &product).map_err(|source_error| EnvironmentBakeError::Io {
        path: product.display().to_string(),
        source: source_error,
    })?;
    Ok(true)
}

/// 运行时读取的环境产物扩展名（`.hdr` 是源文件，`.rlenv` 是烘焙产物）。
pub const RLENV_EXTENSION: &str = "rlenv";

/// 直接烘焙已解码的图像（测试与工具用；`source_hash` 由调用方决定）。
pub fn bake_equirect(
    image: &HdrImage,
    source_hash: u64,
    options: BakeOptions,
) -> Result<BakeReport, EnvironmentBakeError> {
    bake_image(image, source_hash, options, Instant::now())
}

fn bake_image(
    image: &HdrImage,
    source_hash: u64,
    options: BakeOptions,
    started: Instant,
) -> Result<BakeReport, EnvironmentBakeError> {
    options.validate()?;
    let mip0 = build_cubemap(image, options.face_size)?;
    let sh = project_sh(&mip0);
    let mut mips = Vec::with_capacity(options.mip_count as usize);
    mips.push(EnvironmentMip::new(mip0.size, mip0.faces.clone()));
    for mip_index in 1..options.mip_count {
        let size = options.face_size >> mip_index;
        let roughness = mip_index as f32 / (options.mip_count - 1).max(1) as f32;
        let faces: [Vec<[f32; 4]>; FACE_COUNT] = std::array::from_fn(|face| {
            let mut texels = Vec::with_capacity((size as usize) * (size as usize));
            for y in 0..size {
                for x in 0..size {
                    let normal = face_direction(face, x, y, size);
                    let rgb = prefilter(image, normal, roughness);
                    texels.push([rgb[0], rgb[1], rgb[2], 1.0]);
                }
            }
            texels
        });
        mips.push(EnvironmentMip::new(size, faces));
    }
    let asset = EnvironmentAsset {
        source_hash,
        face_size: options.face_size,
        sh,
        mips,
    };
    let bytes = asset.encoded_len();
    let bake_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(BakeReport {
        asset,
        source_width: image.width,
        source_height: image.height,
        bytes,
        bake_ms,
        samples_per_texel: PREFILTER_SAMPLES,
    })
}

fn build_cubemap(image: &HdrImage, size: u32) -> Result<EnvironmentMip, EnvironmentBakeError> {
    let mut faces: [Vec<[f32; 4]>; FACE_COUNT] =
        std::array::from_fn(|_| Vec::with_capacity((size as usize) * (size as usize)));
    for (face, texels) in faces.iter_mut().enumerate() {
        for y in 0..size {
            for x in 0..size {
                let direction = face_direction(face, x, y, size);
                let rgb = image.sample_equirect(direction);
                texels.push([rgb[0], rgb[1], rgb[2], 1.0]);
            }
        }
    }
    Ok(EnvironmentMip::new(size, faces))
}

/// SH9 投影：先按 cubemap texel 的均匀立体角累加，再乘 cosine 卷积因子 `A_l/π`。
fn project_sh(mip0: &EnvironmentMip) -> [f32; SH_COEFFICIENTS] {
    let size = mip0.size;
    let texel_solid_angle = 4.0 * std::f32::consts::PI / (FACE_COUNT as f32 * (size * size) as f32);
    let mut sh = [0.0_f32; SH_COEFFICIENTS];
    for (face, texels) in mip0.faces.iter().enumerate() {
        for y in 0..size {
            for x in 0..size {
                let direction = face_direction(face, x, y, size);
                let basis = sh_basis(direction);
                let rgb = texels[(y * size + x) as usize];
                for (coefficient, weight) in basis.iter().enumerate() {
                    for channel in 0..3 {
                        sh[coefficient * 3 + channel] += rgb[channel] * weight * texel_solid_angle;
                    }
                }
            }
        }
    }
    // 常数项 A0=π → 除以 π 得 1；一次项 A1=2π/3 → 2/3；二次项 A2=π/4 → 1/4。
    for (coefficient, band_factor) in [1.0, 2.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0]
        .iter()
        .chain([0.25; 5].iter())
        .enumerate()
    {
        for channel in 0..3 {
            sh[coefficient * 3 + channel] *= band_factor;
        }
    }
    sh
}

fn sh_basis(direction: Vec3) -> [f32; SH_COEFFICIENTS / 3] {
    let [x, y, z] = [direction.x, direction.y, direction.z];
    [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

fn prefilter(image: &HdrImage, normal: Vec3, roughness: f32) -> [f32; 3] {
    if roughness <= 0.0 {
        return image.sample_equirect(normal);
    }
    let up = if normal.y.abs() < 0.999 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = up.cross(normal).normalized().unwrap_or(Vec3::X);
    let bitangent = normal.cross(tangent);
    let alpha = roughness * roughness;
    let mut sum = [0.0_f32; 3];
    let mut weight = 0.0_f32;
    for sample in 0..PREFILTER_SAMPLES {
        let xi = (sample as f32 + 0.5) / PREFILTER_SAMPLES as f32;
        let yi = radical_inverse_vdc(sample);
        let phi = std::f32::consts::TAU * xi;
        let cos_theta = ((1.0 - yi) / (1.0 + (alpha * alpha - 1.0) * yi)).sqrt();
        let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
        let half = tangent * (sin_theta * phi.cos())
            + bitangent * (sin_theta * phi.sin())
            + normal * cos_theta;
        let light = half * (2.0 * half.dot(normal)) - normal;
        let n_dot_l = light.dot(normal);
        if n_dot_l <= 0.0 {
            continue;
        }
        let rgb = image.sample_equirect(light);
        for channel in 0..3 {
            sum[channel] += rgb[channel] * n_dot_l;
        }
        weight += n_dot_l;
    }
    if weight <= 0.0 {
        return image.sample_equirect(normal);
    }
    [sum[0] / weight, sum[1] / weight, sum[2] / weight]
}

/// 确定性 Hammersley 序列的第二维（base-2 radical inverse）。
fn radical_inverse_vdc(mut bits: u32) -> f32 {
    bits = bits.rotate_right(16);
    bits = ((bits & 0x5555_5555) << 1) | ((bits & 0xaaaa_aaaa) >> 1);
    bits = ((bits & 0x3333_3333) << 2) | ((bits & 0xcccc_cccc) >> 2);
    bits = ((bits & 0x0f0f_0f0f) << 4) | ((bits & 0xf0f0_f0f0) >> 4);
    bits = ((bits & 0x00ff_00ff) << 8) | ((bits & 0xff00_ff00) >> 8);
    bits as f32 * 2.328_306_4e-10
}

/// 把线性 RGB 编码成 Radiance RGBE（测试与夹具生成用）。
///
/// 宽度在 `8..=32767` 时输出 new-style RLE，否则输出 flat；两条路径都被 `image` 的
/// 解码器接受。头部不写注释，保证同内容同字节。
#[must_use]
pub fn encode_radiance(image: &HdrImage) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n");
    out.extend_from_slice(format!("-Y {} +X {}\n", image.height, image.width).as_bytes());
    let rle = (8..=0x7fff).contains(&image.width);
    for y in 0..image.height {
        let row: Vec<[u8; 4]> = (0..image.width)
            .map(|x| to_rgbe(image.pixel(x, y)))
            .collect();
        if rle {
            out.extend_from_slice(&[2, 2, (image.width >> 8) as u8, image.width as u8]);
            for channel in 0..4 {
                let mut written = 0;
                while written < image.width {
                    let count = (image.width - written).min(128);
                    out.push(count as u8);
                    for pixel in &row[written as usize..(written + count) as usize] {
                        out.push(pixel[channel]);
                    }
                    written += count;
                }
            }
        } else {
            for pixel in &row {
                out.extend_from_slice(pixel);
            }
        }
    }
    out
}

fn to_rgbe(rgb: [f32; 3]) -> [u8; 4] {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    if max < 1.0e-32 {
        return [0, 0, 0, 0];
    }
    let exponent = max.log2().floor() as i32 + 1;
    let scale = 2.0_f32.powi(-exponent) * 256.0;
    [
        (rgb[0] * scale).clamp(0.0, 255.0) as u8,
        (rgb[1] * scale).clamp(0.0, 255.0) as u8,
        (rgb[2] * scale).clamp(0.0, 255.0) as u8,
        (exponent + 128).clamp(0, 255) as u8,
    ]
}

#[derive(Debug, Error)]
pub enum EnvironmentBakeError {
    #[error("HDR 文件为空")]
    Empty,
    #[error("HDR 解码失败: {0}")]
    Decode(String),
    #[error("HDR 尺寸 {width}x{height} 非法")]
    InvalidDimensions { width: u32, height: u32 },
    #[error("HDR 尺寸 {width}x{height} 超出预算（≤{max_width}x{max_height} 且 ≤16MP）")]
    TooLarge {
        width: u32,
        height: u32,
        max_width: u32,
        max_height: u32,
    },
    #[error("HDR 像素数 {actual} 与尺寸不符（应为 {expected}）")]
    PixelCount { expected: u64, actual: usize },
    #[error("HDR 像素 ({x},{y}) 通道 {channel} 非有限")]
    NonFinite { x: u32, y: u32, channel: usize },
    #[error("HDR 像素 ({x},{y}) 通道 {channel} 为负值 {value}")]
    Negative {
        x: u32,
        y: u32,
        channel: usize,
        value: f32,
    },
    #[error("face_size {0} 非法：必须是 1..={MAX_BAKE_FACE_SIZE} 的 2 的幂")]
    InvalidFaceSize(u32),
    #[error("mip_count {mip_count} 非法（face_size={face_size}）")]
    InvalidMipCount { mip_count: u32, face_size: u32 },
    #[error("{path}: {source}")]
    WithPath {
        path: String,
        #[source]
        source: Box<EnvironmentBakeError>,
    },
    #[error("{path}: 读取失败: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Asset(#[from] EnvironmentAssetError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_assets::env_asset::{f16_to_f32, f32_to_f16, sh_irradiance};

    fn constant_image(width: u32, height: u32, rgb: [f32; 3]) -> HdrImage {
        HdrImage::new(width, height, vec![rgb; (width * height) as usize]).expect("image")
    }

    /// 每个方向一个颜色：用来检查 cubemap 朝向与 equirect 映射一致。
    fn direction_image(width: u32, height: u32) -> HdrImage {
        let pixels = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let u = (x as f32 + 0.5) / width as f32;
                    let v = (y as f32 + 0.5) / height as f32;
                    let phi = (u - 0.5) * std::f32::consts::TAU;
                    let theta = v * std::f32::consts::PI;
                    let direction = Vec3::new(
                        phi.cos() * theta.sin(),
                        theta.cos(),
                        phi.sin() * theta.sin(),
                    );
                    [
                        0.5 + 0.5 * direction.x,
                        0.5 + 0.5 * direction.y,
                        0.5 + 0.5 * direction.z,
                    ]
                })
            })
            .collect();
        HdrImage::new(width, height, pixels).expect("image")
    }

    #[test]
    fn hdr_decode_roundtrip_preserves_linear_values() {
        let image = direction_image(64, 32);
        let bytes = encode_radiance(&image);
        let decoded = decode_hdr(&bytes).expect("decode");
        assert_eq!((decoded.width(), decoded.height()), (64, 32));
        for (original, roundtrip) in image.pixels().iter().zip(decoded.pixels()) {
            // RGBE 三通道共享指数：小通道的绝对误差以该像素最大通道的 1/256 为步长。
            let step = original.iter().fold(0.0_f32, |max, value| max.max(*value)) / 256.0;
            for channel in 0..3 {
                let tolerance = original[channel].abs() * 0.02 + step * 2.0 + 1.0e-4;
                assert!(
                    (original[channel] - roundtrip[channel]).abs() <= tolerance,
                    "{original:?} -> {roundtrip:?}"
                );
            }
        }
    }

    #[test]
    fn hdr_decode_rejects_empty_garbage_and_truncated() {
        assert!(matches!(decode_hdr(&[]), Err(EnvironmentBakeError::Empty)));
        assert!(matches!(
            decode_hdr(b"not radiance"),
            Err(EnvironmentBakeError::Decode(_))
        ));
        let bytes = encode_radiance(&constant_image(8, 1, [1.0, 1.0, 1.0]));
        let truncated = &bytes[..bytes.len() - 20];
        assert!(matches!(
            decode_hdr(truncated),
            Err(EnvironmentBakeError::Decode(_))
        ));
    }

    #[test]
    fn hdr_decode_rejects_oversized_dimensions() {
        let width = MAX_SOURCE_WIDTH + 1;
        let image = HdrImage {
            width,
            height: 1,
            pixels: vec![[1.0, 1.0, 1.0]; width as usize],
        };
        let bytes = encode_radiance(&image);
        assert!(matches!(
            decode_hdr(&bytes),
            Err(EnvironmentBakeError::Decode(_) | EnvironmentBakeError::TooLarge { .. })
        ));
    }

    #[test]
    fn hdr_rejects_nan_and_negative_pixels() {
        for (pixel, negative) in [([f32::NAN, 0.0, 0.0], false), ([-0.5, 0.0, 0.0], true)] {
            let error = HdrImage::new(1, 1, vec![pixel]).expect_err("must reject");
            if negative {
                assert!(matches!(error, EnvironmentBakeError::Negative { .. }));
            } else {
                assert!(matches!(error, EnvironmentBakeError::NonFinite { .. }));
            }
        }
        let error = HdrImage::new(2, 2, vec![[1.0, 1.0, 1.0]; 3]).expect_err("count");
        assert!(matches!(error, EnvironmentBakeError::PixelCount { .. }));
    }

    #[test]
    fn hdr_bake_options_are_validated() {
        for options in [
            BakeOptions {
                face_size: 3,
                mip_count: 1,
            },
            BakeOptions {
                face_size: 0,
                mip_count: 1,
            },
            BakeOptions {
                face_size: 1024,
                mip_count: 1,
            },
            BakeOptions {
                face_size: 16,
                mip_count: 6,
            },
            BakeOptions {
                face_size: 16,
                mip_count: 0,
            },
        ] {
            assert!(options.validate().is_err(), "{options:?}");
        }
        BakeOptions::default().validate().expect("default");
    }

    #[test]
    fn hdr_bake_is_byte_deterministic() {
        let image = direction_image(32, 16);
        let options = BakeOptions {
            face_size: 8,
            mip_count: 3,
        };
        let first = bake_equirect(&image, 42, options).expect("bake");
        let second = bake_equirect(&image, 42, options).expect("bake");
        assert_eq!(first.encode().unwrap(), second.encode().unwrap());
        assert_eq!(first.bytes, first.asset.encoded_len());
        assert_eq!(first.samples_per_texel, PREFILTER_SAMPLES);
        assert!(first.bake_ms >= 0.0);
        assert_eq!(first.asset.face_size, 8);
        assert_eq!(first.asset.mip_count(), 3);
        assert_eq!(first.source_width, 32);
        assert_eq!(first.source_height, 16);
    }

    #[test]
    fn hdr_bake_constant_white_has_unit_sh_and_texels() {
        let image = constant_image(16, 8, [1.0, 1.0, 1.0]);
        let report = bake_equirect(
            &image,
            7,
            BakeOptions {
                face_size: 8,
                mip_count: 2,
            },
        )
        .expect("bake");
        for direction in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(-0.4, 0.2, 0.9)] {
            let irradiance = sh_irradiance(&report.asset.sh, direction);
            for value in irradiance {
                assert!((value - 1.0).abs() < 1.0e-3, "{irradiance:?}");
            }
        }
        for texel in &report.asset.mips[0].faces[0] {
            for channel in 0..3 {
                assert!((texel[channel] - 1.0).abs() < 1.0e-3, "{texel:?}");
            }
        }
        // f16 写盘后仍然近似 1（验收里会检查 `.rlenv` 的往返）。
        let roundtrip = EnvironmentAsset::decode(&report.encode().unwrap()).expect("decode .rlenv");
        for value in sh_irradiance(&roundtrip.sh, Vec3::Y) {
            assert!((value - 1.0).abs() < 1.0e-3, "{value}");
        }
    }

    #[test]
    fn hdr_cubemap_faces_look_at_expected_directions() {
        let image = direction_image(64, 32);
        let report = bake_equirect(
            &image,
            1,
            BakeOptions {
                face_size: 4,
                mip_count: 1,
            },
        )
        .expect("bake");
        let mip = &report.asset.mips[0];
        for (face, expected) in [
            (0, Vec3::X),
            (1, -Vec3::X),
            (2, Vec3::Y),
            (3, -Vec3::Y),
            (4, Vec3::Z),
            (5, -Vec3::Z),
        ] {
            let direction = face_direction(face, mip.size / 2 - 1, mip.size / 2 - 1, mip.size);
            assert!(direction.dot(expected) > 0.9, "face {face}: {direction:?}");
            let texel = mip.texel(face, mip.size / 2 - 1, mip.size / 2 - 1).unwrap();
            let sampled = image.sample_equirect(expected);
            for channel in 0..3 {
                // 面中心 texel 的方向与坐标轴有偏差（4px 面约 20°），只要求同号趋势。
                assert!(
                    (texel[channel] - sampled[channel]).abs() < 0.35,
                    "face {face} channel {channel}: {texel:?} vs {sampled:?}"
                );
            }
        }
    }

    #[test]
    fn hdr_rlenv_rejects_version_and_checksum() {
        let image = constant_image(8, 4, [2.0, 1.0, 0.5]);
        let report = bake_equirect(
            &image,
            9,
            BakeOptions {
                face_size: 4,
                mip_count: 2,
            },
        )
        .expect("bake");
        let bytes = report.encode().expect("encode");
        assert_eq!(&bytes[0..4], b"RLEN");
        let mut bad_version = bytes.clone();
        bad_version[4] = 2;
        assert!(matches!(
            EnvironmentAsset::decode(&bad_version),
            Err(EnvironmentAssetError::UnsupportedVersion(2))
        ));
        let mut bad_checksum = bytes;
        let last = bad_checksum.len() - 1;
        bad_checksum[last] ^= 0xff;
        assert!(matches!(
            EnvironmentAsset::decode(&bad_checksum),
            Err(EnvironmentAssetError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn hdr_studio_fixture_matches_committed_rlenv() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples/game_api/assets/env");
        let hdr_path = directory.join("studio.hdr");
        let rlenv_path = directory.join("studio.rlenv");
        let source = std::fs::read(&hdr_path).expect("studio.hdr 夹具");
        let report = bake_hdr_bytes(&source, BakeOptions::default()).expect("bake studio");
        let baked = report.encode().expect("encode studio");
        let committed = std::fs::read(&rlenv_path).expect("studio.rlenv 夹具");
        assert_eq!(
            baked, committed,
            "studio.rlenv 与 studio.hdr 不同步：请重跑 bake_environment --fixture"
        );
        let decoded = EnvironmentAsset::decode(&committed).expect("decode studio.rlenv");
        assert_eq!(decoded.source_hash, fnv1a64(&source));
        assert_eq!(decoded.face_size, DEFAULT_FACE_SIZE);
        assert_eq!(decoded.mip_count(), DEFAULT_MIP_COUNT);
        let upward = sh_irradiance(&decoded.sh, Vec3::Y);
        let downward = sh_irradiance(&decoded.sh, -Vec3::Y);
        assert!(
            upward[2] > downward[2],
            "天顶应比地面更蓝：{upward:?} vs {downward:?}"
        );
        assert!(report.bake_ms >= 0.0);
        assert!(report.bytes <= 8 * 1024 * 1024, "单 env 预算 8MB");
    }

    #[test]
    fn hdr_rgb_e_helpers_are_stable() {
        for value in [0.0_f32, 1.0, 0.25, 12.5, 1000.0] {
            let half = f32_to_f16(value);
            assert!((f16_to_f32(half) - value).abs() <= value.abs() * 0.001 + 1.0e-7);
        }
        assert_eq!(to_rgbe([0.0, 0.0, 0.0]), [0, 0, 0, 0]);
        let rgbe = to_rgbe([1.0, 0.5, 0.25]);
        assert_eq!(rgbe[3], 129);
        assert!(rgbe[0] >= 127 && rgbe[0] <= 129, "{rgbe:?}");
    }
}
