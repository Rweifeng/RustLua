//! P3-R03-1：`.rlenv` 环境资产（版本化二进制）与 SH9 漫反射求值。
//!
//! 运行时只读这个格式；`.hdr` 解码、cubemap 投影与预过滤属于导入层
//! （`rustlua-assets-import::environment`），不会进浏览器编译图。
//!
//! # 布局（little-endian）
//!
//! ```text
//! 0   magic          "RLEN"
//! 4   version        u16 = 1
//! 6   format         u16 = 1 (RGBA16Float 面数据)
//! 8   source_hash    u64 (FNV-1a of the source .hdr bytes)
//! 16  face_size      u32 (mip0 每面边长，2 的幂)
//! 20  mip_count      u32
//! 24  reserved       u32 = 0
//! 28  sh             [f32; 27]（9 个 RGB 系数，已含 cosine convolution / π）
//! 136 mips           for mip in 0..mip_count:
//!                      size = face_size >> mip
//!                      for face in 0..6: size*size 个 RGBA16F texel
//! end checksum       u64 (FNV-1a of every byte before the checksum)
//! ```
//!
//! 面顺序固定为 `+X, -X, +Y, -Y, +Z, -Z`，与导入层 `environment::FACE_DIRECTIONS` 一致。
//! 未知版本/格式、坏校验和、尺寸与长度不一致一律拒绝——环境数据坏掉时宁可报错，
//! 也不要把垃圾塞进 GPU。

use rustlua_math::math3d::Vec3;
use thiserror::Error;

pub const MAGIC: [u8; 4] = *b"RLEN";
pub const VERSION: u16 = 1;
pub const FORMAT_RGBA16F: u16 = 1;
pub const SH_COEFFICIENTS: usize = 27;
pub const SH_BANDS: usize = 9;
pub const FACE_COUNT: usize = 6;
/// 单面最大边长：128 时 5 级 mip 约 1 MiB；上限 1024 防止坏文件逼运行时分配巨量内存。
pub const MAX_FACE_SIZE: u32 = 1024;
const HEADER_BYTES: usize = 4 + 2 + 2 + 8 + 4 + 4 + 4 + SH_COEFFICIENTS * 4;
const CHECKSUM_BYTES: usize = 8;

/// 一个 mip 级：6 个面，每面 `size * size` 个 RGBA texel（线性 HDR）。
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentMip {
    pub size: u32,
    pub faces: [Vec<[f32; 4]>; FACE_COUNT],
}

impl EnvironmentMip {
    #[must_use]
    pub fn new(size: u32, faces: [Vec<[f32; 4]>; FACE_COUNT]) -> Self {
        Self { size, faces }
    }

    /// 某面某 texel；越界返回 `None`（调用方自己决定回退）。
    #[must_use]
    pub fn texel(&self, face: usize, x: u32, y: u32) -> Option<&[f32; 4]> {
        if face >= FACE_COUNT || x >= self.size || y >= self.size {
            return None;
        }
        self.faces[face].get((y * self.size + x) as usize)
    }
}

/// 一份解码后的环境资产。字段公开是为了导入层构造；写盘前 [`Self::encode`] 会做完整校验。
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentAsset {
    pub source_hash: u64,
    pub face_size: u32,
    /// 9 个 RGB 系数，索引 `coefficient * 3 + channel`；已乘 cosine 卷积因子并除以 π，
    /// 因此 `sh_irradiance` 的返回值可直接乘 albedo 当作漫反射环境项。
    pub sh: [f32; SH_COEFFICIENTS],
    pub mips: Vec<EnvironmentMip>,
}

impl EnvironmentAsset {
    #[must_use]
    pub fn mip_count(&self) -> u32 {
        self.mips.len() as u32
    }

    #[must_use]
    pub fn mip_size(&self, mip: usize) -> Option<u32> {
        self.mips.get(mip).map(|mip| mip.size)
    }

    /// 序列化后的字节数（用于 `≤8MB` 预算检查，不分配）。
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        let texels: usize = self
            .mips
            .iter()
            .map(|mip| FACE_COUNT * (mip.size as usize) * (mip.size as usize))
            .sum();
        HEADER_BYTES + texels * 4 * size_of::<u16>() + CHECKSUM_BYTES
    }

    /// 校验并写盘。校验失败返回带原因的错误，不产生半份文件。
    pub fn encode(&self) -> Result<Vec<u8>, EnvironmentAssetError> {
        self.validate()?;
        let mut out = Vec::with_capacity(self.encoded_len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&FORMAT_RGBA16F.to_le_bytes());
        out.extend_from_slice(&self.source_hash.to_le_bytes());
        out.extend_from_slice(&self.face_size.to_le_bytes());
        out.extend_from_slice(&self.mip_count().to_le_bytes());
        out.extend_from_slice(&0_u32.to_le_bytes());
        for coefficient in &self.sh {
            out.extend_from_slice(&coefficient.to_le_bytes());
        }
        for mip in &self.mips {
            for face in &mip.faces {
                for texel in face {
                    out.extend_from_slice(&f32_to_f16(texel[0]).to_le_bytes());
                    out.extend_from_slice(&f32_to_f16(texel[1]).to_le_bytes());
                    out.extend_from_slice(&f32_to_f16(texel[2]).to_le_bytes());
                    out.extend_from_slice(&f32_to_f16(texel[3]).to_le_bytes());
                }
            }
        }
        let checksum = fnv1a64(&out);
        out.extend_from_slice(&checksum.to_le_bytes());
        Ok(out)
    }

    /// 解码并校验。任何不一致都拒绝，包括多余尾随字节。
    pub fn decode(bytes: &[u8]) -> Result<Self, EnvironmentAssetError> {
        if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES {
            return Err(EnvironmentAssetError::TooShort {
                actual: bytes.len(),
            });
        }
        if bytes[0..4] != MAGIC {
            return Err(EnvironmentAssetError::BadMagic);
        }
        let version = u16::from_le_bytes(bytes[4..6].try_into().expect("2-byte slice"));
        if version != VERSION {
            return Err(EnvironmentAssetError::UnsupportedVersion(version));
        }
        let format = u16::from_le_bytes(bytes[6..8].try_into().expect("2-byte slice"));
        if format != FORMAT_RGBA16F {
            return Err(EnvironmentAssetError::UnsupportedFormat(format));
        }
        let source_hash = u64::from_le_bytes(bytes[8..16].try_into().expect("8-byte slice"));
        let face_size = u32::from_le_bytes(bytes[16..20].try_into().expect("4-byte slice"));
        let mip_count = u32::from_le_bytes(bytes[20..24].try_into().expect("4-byte slice"));
        let reserved = u32::from_le_bytes(bytes[24..28].try_into().expect("4-byte slice"));
        if reserved != 0 {
            return Err(EnvironmentAssetError::InvalidLayout(
                "header reserved 必须为 0".to_owned(),
            ));
        }
        validate_face_shape(face_size, mip_count)?;
        let mut sh = [0.0_f32; SH_COEFFICIENTS];
        for (index, coefficient) in sh.iter_mut().enumerate() {
            let start = 28 + index * 4;
            *coefficient = f32::from_le_bytes(
                bytes[start..start + 4]
                    .try_into()
                    .expect("SH slice is in bounds"),
            );
            if !coefficient.is_finite() {
                return Err(EnvironmentAssetError::ShNonFinite { index });
            }
        }
        let mut expected = HEADER_BYTES;
        for mip in 0..mip_count {
            let size = mip_size(face_size, mip)?;
            expected += FACE_COUNT * (size as usize) * (size as usize) * 4 * size_of::<u16>();
        }
        expected += CHECKSUM_BYTES;
        if bytes.len() != expected {
            return Err(EnvironmentAssetError::LengthMismatch {
                expected,
                actual: bytes.len(),
            });
        }
        let stored = u64::from_le_bytes(
            bytes[bytes.len() - CHECKSUM_BYTES..]
                .try_into()
                .expect("checksum slice"),
        );
        let computed = fnv1a64(&bytes[..bytes.len() - CHECKSUM_BYTES]);
        if stored != computed {
            return Err(EnvironmentAssetError::ChecksumMismatch { stored, computed });
        }

        let mut cursor = HEADER_BYTES;
        let mut mips = Vec::with_capacity(mip_count as usize);
        for mip_index in 0..mip_count {
            let size = mip_size(face_size, mip_index)?;
            let count = (size as usize) * (size as usize);
            let mut faces: [Vec<[f32; 4]>; FACE_COUNT] =
                std::array::from_fn(|_| Vec::with_capacity(count));
            for (face_index, texels) in faces.iter_mut().enumerate() {
                for texel_index in 0..count {
                    let mut rgba = [0.0_f32; 4];
                    for channel in &mut rgba {
                        *channel = f16_to_f32(u16::from_le_bytes(
                            bytes[cursor..cursor + 2]
                                .try_into()
                                .expect("texel slice is in bounds"),
                        ));
                        cursor += 2;
                    }
                    if !rgba.iter().all(|value| value.is_finite() && *value >= 0.0) {
                        return Err(EnvironmentAssetError::TexelInvalid {
                            mip: mip_index as usize,
                            face: face_index,
                            texel: texel_index,
                        });
                    }
                    texels.push(rgba);
                }
            }
            mips.push(EnvironmentMip { size, faces });
        }
        Ok(Self {
            source_hash,
            face_size,
            sh,
            mips,
        })
    }

    fn validate(&self) -> Result<(), EnvironmentAssetError> {
        validate_face_shape(self.face_size, self.mip_count())?;
        for (index, coefficient) in self.sh.iter().enumerate() {
            if !coefficient.is_finite() {
                return Err(EnvironmentAssetError::ShNonFinite { index });
            }
        }
        for (mip_index, mip) in self.mips.iter().enumerate() {
            let expected = mip_size(self.face_size, mip_index as u32)?;
            if mip.size != expected {
                return Err(EnvironmentAssetError::InvalidLayout(format!(
                    "mip {mip_index} 尺寸 {}，应为 {expected}",
                    mip.size
                )));
            }
            for (face, texels) in mip.faces.iter().enumerate() {
                let count = (mip.size as usize) * (mip.size as usize);
                if texels.len() != count {
                    return Err(EnvironmentAssetError::InvalidLayout(format!(
                        "mip {mip_index} face {face} 有 {} 个 texel，应为 {count}",
                        texels.len()
                    )));
                }
                for (texel_index, texel) in texels.iter().enumerate() {
                    if !texel.iter().all(|value| value.is_finite() && *value >= 0.0) {
                        return Err(EnvironmentAssetError::TexelInvalid {
                            mip: mip_index,
                            face,
                            texel: texel_index,
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

fn validate_face_shape(face_size: u32, mip_count: u32) -> Result<(), EnvironmentAssetError> {
    if face_size == 0 || !face_size.is_power_of_two() || face_size > MAX_FACE_SIZE {
        return Err(EnvironmentAssetError::InvalidFaceSize(face_size));
    }
    if mip_count == 0 || mip_count > face_size.ilog2() + 1 {
        return Err(EnvironmentAssetError::InvalidMipCount {
            mip_count,
            face_size,
        });
    }
    Ok(())
}

fn mip_size(face_size: u32, mip: u32) -> Result<u32, EnvironmentAssetError> {
    let size = face_size >> mip;
    if size == 0 {
        return Err(EnvironmentAssetError::InvalidMipCount {
            mip_count: mip + 1,
            face_size,
        });
    }
    Ok(size)
}

/// FNV-1a 64：与导入缓存同一套非加密指纹口径，只要求"字节变了值就变"。
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 用 SH9 求 `E/π`（漫反射环境项可直接乘 albedo）。
///
/// 资产里的系数已经含 `A_l/π`（`A = [1, 2/3, 1/4]`），这里只做基函数展开。
#[must_use]
pub fn sh_irradiance(sh: &[f32; SH_COEFFICIENTS], direction: Vec3) -> [f32; 3] {
    let direction = direction.normalized().unwrap_or(Vec3::Y);
    let [x, y, z] = [direction.x, direction.y, direction.z];
    let basis = [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ];
    let mut rgb = [0.0_f32; 3];
    for (coefficient, weight) in sh.chunks_exact(3).zip(basis) {
        for channel in 0..3 {
            rgb[channel] += coefficient[channel] * weight;
        }
    }
    rgb
}

/// IEEE 754 半精度编码（round-to-nearest-even；超过 65504 记为 +inf，由写盘校验拒绝）。
#[must_use]
pub fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x007f_ffff;
    if exponent == 0xff {
        return sign | 0x7c00 | if mantissa != 0 { 0x0200 } else { 0 };
    }
    let half_exponent = exponent - 127 + 15;
    if half_exponent >= 0x1f {
        return sign | 0x7c00;
    }
    if half_exponent <= 0 {
        if half_exponent < -10 {
            return sign;
        }
        let mantissa = mantissa | 0x0080_0000;
        let shift = (14 - half_exponent) as u32;
        let mut half = mantissa >> shift;
        let round = 1_u32 << (shift - 1);
        if (mantissa & round) != 0 && ((mantissa & (round - 1)) != 0 || (half & 1) != 0) {
            half += 1;
        }
        return sign | half as u16;
    }
    let mut half = ((half_exponent as u16) << 10) | (mantissa >> 13) as u16;
    let round = mantissa & 0x1fff;
    if round > 0x1000 || (round == 0x1000 && (half & 1) != 0) {
        half = half.wrapping_add(1);
    }
    sign | half
}

/// IEEE 754 半精度解码。
#[must_use]
pub fn f16_to_f32(value: u16) -> f32 {
    let sign = u32::from(value & 0x8000) << 16;
    let exponent = u32::from((value >> 10) & 0x1f);
    let mantissa = u32::from(value & 0x03ff);
    let bits = if exponent == 0 {
        if mantissa == 0 {
            sign
        } else {
            let mut exponent = 127 - 15 + 1;
            let mut mantissa = mantissa;
            while (mantissa & 0x0400) == 0 {
                mantissa <<= 1;
                exponent -= 1;
            }
            sign | (exponent << 23) | ((mantissa & 0x03ff) << 13)
        }
    } else if exponent == 0x1f {
        sign | 0x7f80_0000 | (mantissa << 13)
    } else {
        sign | ((exponent + 127 - 15) << 23) | (mantissa << 13)
    };
    f32::from_bits(bits)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EnvironmentAssetError {
    #[error("环境资产只有 {actual} 字节，连文件头+校验和都不够")]
    TooShort { actual: usize },
    #[error("不是 .rlenv：magic 不是 RLEN")]
    BadMagic,
    #[error("不支持的 .rlenv 版本 {0}（期望 1）")]
    UnsupportedVersion(u16),
    #[error("不支持的 .rlenv 面格式 {0}（期望 1 = RGBA16Float）")]
    UnsupportedFormat(u16),
    #[error("face_size {0} 非法：必须是 1..={MAX_FACE_SIZE} 的 2 的幂")]
    InvalidFaceSize(u32),
    #[error("mip_count {mip_count} 非法（face_size={face_size}）")]
    InvalidMipCount { mip_count: u32, face_size: u32 },
    #[error("SH 系数 #{index} 非有限")]
    ShNonFinite { index: usize },
    #[error("mip {mip} face {face} texel {texel} 含非有限或负值")]
    TexelInvalid {
        mip: usize,
        face: usize,
        texel: usize,
    },
    #[error("文件长度 {actual} 与头部声明的 {expected} 不一致")]
    LengthMismatch { expected: usize, actual: usize },
    #[error("校验和不匹配：文件 {stored:#x}，实算 {computed:#x}")]
    ChecksumMismatch { stored: u64, computed: u64 },
    #[error("环境资产布局非法：{0}")]
    InvalidLayout(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mip(size: u32, seed: f32) -> EnvironmentMip {
        let faces = std::array::from_fn(|face| {
            (0..size * size)
                .map(|texel| {
                    let value = seed + face as f32 + texel as f32 * 0.25;
                    [value, value * 2.0, value * 0.5, 1.0]
                })
                .collect()
        });
        EnvironmentMip::new(size, faces)
    }

    fn asset() -> EnvironmentAsset {
        EnvironmentAsset {
            source_hash: 0x1234_5678_9abc_def0,
            face_size: 4,
            sh: std::array::from_fn(|index| index as f32 * 0.125 - 1.0),
            mips: vec![mip(4, 0.0), mip(2, 10.0), mip(1, 20.0)],
        }
    }

    #[test]
    fn roundtrip_preserves_header_sh_and_mips() {
        let asset = asset();
        let bytes = asset.encode().expect("encode");
        assert_eq!(bytes.len(), asset.encoded_len());
        let decoded = EnvironmentAsset::decode(&bytes).expect("decode");
        assert_eq!(decoded.source_hash, asset.source_hash);
        assert_eq!(decoded.face_size, 4);
        assert_eq!(decoded.mip_count(), 3);
        assert_eq!(decoded.sh, asset.sh);
        for (left, right) in decoded.mips.iter().zip(&asset.mips) {
            assert_eq!(left.size, right.size);
            for (left_face, right_face) in left.faces.iter().zip(&right.faces) {
                assert_eq!(left_face.len(), right_face.len());
                for (left_texel, right_texel) in left_face.iter().zip(right_face) {
                    for channel in 0..3 {
                        assert!(
                            (left_texel[channel] - right_texel[channel]).abs() < 0.01,
                            "f16 往返误差过大：{left_texel:?} vs {right_texel:?}"
                        );
                    }
                    assert_eq!(left_texel[3], 1.0);
                }
            }
        }
    }

    #[test]
    fn encoding_is_byte_deterministic() {
        let asset = asset();
        assert_eq!(asset.encode().unwrap(), asset.encode().unwrap());
    }

    #[test]
    fn decode_rejects_bad_magic_version_and_checksum() {
        let mut bytes = asset().encode().unwrap();
        let good = bytes[0];
        bytes[0] = b'X';
        assert_eq!(
            EnvironmentAsset::decode(&bytes),
            Err(EnvironmentAssetError::BadMagic)
        );
        bytes[0] = good;
        bytes[4] = 9;
        assert_eq!(
            EnvironmentAsset::decode(&bytes),
            Err(EnvironmentAssetError::UnsupportedVersion(9))
        );
        let mut bytes = asset().encode().unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        assert!(matches!(
            EnvironmentAsset::decode(&bytes),
            Err(EnvironmentAssetError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn decode_rejects_truncation_and_trailing_bytes() {
        let bytes = asset().encode().unwrap();
        assert!(matches!(
            EnvironmentAsset::decode(&bytes[..bytes.len() - 1]),
            Err(EnvironmentAssetError::TooShort { .. }
                | EnvironmentAssetError::LengthMismatch { .. })
        ));
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(matches!(
            EnvironmentAsset::decode(&longer),
            Err(EnvironmentAssetError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn encode_rejects_mismatched_shape_and_negative_texels() {
        let mut asset = asset();
        asset.mips[1].size = 3;
        assert!(matches!(
            asset.encode(),
            Err(EnvironmentAssetError::InvalidLayout(_))
        ));
        let mut asset = self::asset();
        asset.mips[0].faces[0][0] = [-1.0, 0.0, 0.0, 1.0];
        assert!(matches!(
            asset.encode(),
            Err(EnvironmentAssetError::TexelInvalid { .. })
        ));
        let mut asset = self::asset();
        asset.face_size = 3;
        assert_eq!(
            asset.encode(),
            Err(EnvironmentAssetError::InvalidFaceSize(3))
        );
    }

    #[test]
    fn f16_helpers_match_known_values_and_roundtrip() {
        for (value, bits) in [
            (0.0_f32, 0x0000_u16),
            (-0.0, 0x8000),
            (1.0, 0x3c00),
            (-2.0, 0xc000),
            (0.5, 0x3800),
            (65_504.0, 0x7bff),
        ] {
            assert_eq!(f32_to_f16(value), bits, "{value}");
            assert_eq!(f16_to_f32(bits), value, "{bits:#x}");
        }
        assert_eq!(f16_to_f32(0x0001), 2.0_f32.powi(-24));
        assert_eq!(f32_to_f16(1.0e9), 0x7c00);
        assert!(f16_to_f32(0x7c00).is_infinite());
        assert!(f16_to_f32(0x7e00).is_nan());
        for value in [0.1_f32, -1.5, 123.75, 1.0e-5, 30_000.0] {
            let roundtrip = f16_to_f32(f32_to_f16(value));
            // 亚正规数（< 2^-14）只有绝对精度：1e-5 的最邻近 f16 是 1.001358e-5。
            assert!(
                (roundtrip - value).abs() <= value.abs() * 0.001 + 1.0e-7,
                "{value} -> {roundtrip}"
            );
        }
    }

    #[test]
    fn sh_irradiance_of_constant_environment_is_one() {
        let mut sh = [0.0_f32; SH_COEFFICIENTS];
        for coefficient in sh.iter_mut().take(3) {
            *coefficient = 0.282_095 * 4.0 * std::f32::consts::PI;
        }
        for direction in [Vec3::Y, Vec3::X, Vec3::Z, Vec3::new(0.3, -0.7, 0.2)] {
            let irradiance = sh_irradiance(&sh, direction);
            for value in irradiance {
                assert!((value - 1.0).abs() < 1.0e-4, "{irradiance:?}");
            }
        }
    }
}
