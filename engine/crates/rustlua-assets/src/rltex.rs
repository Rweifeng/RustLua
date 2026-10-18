//! `.rltex`：图集页/大纹理的发布形态（`docs/67` §6.3）。
//!
//! 为什么不是 PNG：同一张 2048×1024 的图集页，`image` 写出的 PNG 是 153,012 B，
//! 而这批像素**裸存 + 容器块压缩（zstd L19）= 14,711 B**（实测，§1.3 结论 3）。
//! 页里 88% 的像素是纯零，按行滤波 + zlib 分块对这类内容是在制造熵。所以：
//! **页存裸像素，压缩交给容器**——`.rltex` 内部不再压一层（双压无收益，还挡住块级复用）。
//!
//! 布局（16 B 头 + 裸像素，小端）：
//!
//! ```text
//! "RLT1"(4) │ width u32 │ height u32 │ fmt u8 │ reserved[3]
//! pixels：RGBA8 紧密排列（width*height*4 字节）
//! ```

/// 魔数。
pub const RLTEX_MAGIC: &[u8; 4] = b"RLT1";
/// 头部长度。
pub const RLTEX_HEADER_LEN: usize = 16;

/// 像素格式。v1 只有 RGBA8（其它值保留，读到就报错而不是猜）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RltexFormat {
    Rgba8,
}

impl RltexFormat {
    pub fn code(self) -> u8 {
        match self {
            Self::Rgba8 => 0,
        }
    }

    pub fn from_code(code: u8) -> Result<Self, RltexError> {
        match code {
            0 => Ok(Self::Rgba8),
            other => Err(RltexError::UnsupportedFormat(other)),
        }
    }
}

/// 解码结果：尺寸 + RGBA8 像素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RltexImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RltexImage {
    /// 期望的像素字节数（溢出即报错，不静默截断）。
    pub fn expected_len(width: u32, height: u32) -> Result<usize, RltexError> {
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|value| value.checked_mul(4))
            .ok_or(RltexError::TooLarge { width, height })?;
        usize::try_from(pixels).map_err(|_| RltexError::TooLarge { width, height })
    }
}

/// `.rltex` 错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RltexError {
    #[error("不是 .rltex：魔数不符")]
    NotRltex,
    #[error(".rltex 头部截断（{0} B < {RLTEX_HEADER_LEN} B）")]
    Truncated(usize),
    #[error(".rltex 尺寸非法：{width}x{height}")]
    TooLarge { width: u32, height: u32 },
    #[error("不支持的 .rltex 像素格式 {0}")]
    UnsupportedFormat(u8),
    #[error(".rltex 像素长度不符：{actual} B，期望 {expected} B")]
    LengthMismatch { actual: usize, expected: usize },
}

/// 是不是 `.rltex`（只看魔数，不校验尺寸）。
pub fn is_rltex(bytes: &[u8]) -> bool {
    bytes.starts_with(RLTEX_MAGIC)
}

/// 只读尺寸（不解包、不拷贝像素）。给 `load_atlas` 那种"只要宽高"的调用方。
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < RLTEX_HEADER_LEN || !is_rltex(bytes) {
        return None;
    }
    let width = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    Some((width, height))
}

/// 编码：`fmt` 目前只支持 RGBA8，`pixels` 必须是 `width*height*4`。
pub fn encode(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, RltexError> {
    let expected = RltexImage::expected_len(width, height)?;
    if width == 0 || height == 0 || pixels.len() != expected {
        return Err(RltexError::LengthMismatch {
            actual: pixels.len(),
            expected,
        });
    }
    let mut out = Vec::with_capacity(RLTEX_HEADER_LEN + expected);
    out.extend_from_slice(RLTEX_MAGIC);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.push(RltexFormat::Rgba8.code());
    out.extend_from_slice(&[0, 0, 0]);
    out.extend_from_slice(pixels);
    Ok(out)
}

/// 解码。
pub fn decode(bytes: &[u8]) -> Result<RltexImage, RltexError> {
    if !is_rltex(bytes) {
        return Err(RltexError::NotRltex);
    }
    if bytes.len() < RLTEX_HEADER_LEN {
        return Err(RltexError::Truncated(bytes.len()));
    }
    let width = u32::from_le_bytes(bytes[4..8].try_into().expect("4 字节"));
    let height = u32::from_le_bytes(bytes[8..12].try_into().expect("4 字节"));
    RltexFormat::from_code(bytes[12])?;
    let expected = RltexImage::expected_len(width, height)?;
    let pixels = &bytes[RLTEX_HEADER_LEN..];
    if width == 0 || height == 0 || pixels.len() != expected {
        return Err(RltexError::LengthMismatch {
            actual: pixels.len(),
            expected,
        });
    }
    Ok(RltexImage {
        width,
        height,
        pixels: pixels.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_pixels() {
        let pixels = vec![0u8; 4 * 4 * 4]
            .into_iter()
            .enumerate()
            .map(|(index, _)| index as u8)
            .collect::<Vec<_>>();
        let encoded = encode(4, 4, &pixels).unwrap();
        assert!(is_rltex(&encoded));
        assert_eq!(dimensions(&encoded), Some((4, 4)));
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 4);
        assert_eq!(decoded.pixels, pixels);
    }

    #[test]
    fn header_is_exactly_sixteen_bytes() {
        let encoded = encode(1, 1, &[1, 2, 3, 4]).unwrap();
        assert_eq!(RLTEX_HEADER_LEN, 16);
        assert_eq!(encoded.len(), 16 + 4);
        assert_eq!(&encoded[0..4], RLTEX_MAGIC);
        assert_eq!(&encoded[13..16], &[0, 0, 0], "保留位必须写零");
    }

    #[test]
    fn rejects_bad_input_loudly() {
        assert!(matches!(decode(b"not-a-rtex"), Err(RltexError::NotRltex)));
        assert!(matches!(decode(b"RLT1\x01"), Err(RltexError::Truncated(5))));
        // 长度不符
        let mut short = encode(2, 2, &[7u8; 16]).unwrap();
        short.truncate(short.len() - 1);
        assert!(matches!(
            decode(&short),
            Err(RltexError::LengthMismatch { .. })
        ));
        // 格式位不认
        let mut unknown = encode(1, 1, &[0, 0, 0, 0]).unwrap();
        unknown[12] = 9;
        assert!(matches!(
            decode(&unknown),
            Err(RltexError::UnsupportedFormat(9))
        ));
    }

    #[test]
    fn encode_rejects_size_mismatch() {
        assert!(matches!(
            encode(2, 2, &[0; 4]),
            Err(RltexError::LengthMismatch { .. })
        ));
        assert!(matches!(
            encode(0, 1, &[]),
            Err(RltexError::LengthMismatch { .. })
        ));
    }
}
