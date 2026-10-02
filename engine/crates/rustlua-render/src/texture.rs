//! PNG decoding and CPU-side texture asset representation for M3-02.

use std::path::Path;

/// The color interpretation to use when creating the GPU texture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureColorSpace {
    /// Color data such as sprites, UI, and base color textures.
    Srgb,
    /// Data textures such as normal maps, masks, and material channels.
    Linear,
}

/// A decoded RGBA8 texture ready for GPU upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextureAsset {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    color_space: TextureColorSpace,
}

impl TextureAsset {
    /// Decodes a PNG file as RGBA8.
    pub fn load_png(
        path: impl AsRef<Path>,
        color_space: TextureColorSpace,
    ) -> Result<Self, TextureError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|reason| TextureError::Read {
            path: path.display().to_string(),
            reason: reason.to_string(),
        })?;
        let bytes = rustlua_infra::pack::decode(&bytes).map_err(|reason| TextureError::Read {
            path: path.display().to_string(),
            reason,
        })?;
        Self::from_png_bytes(&bytes, color_space).map_err(|error| TextureError::Decode {
            path: path.display().to_string(),
            reason: error.to_string(),
        })
    }

    /// Decodes PNG bytes as RGBA8.
    pub fn from_png_bytes(
        bytes: &[u8],
        color_space: TextureColorSpace,
    ) -> Result<Self, TextureError> {
        if bytes.is_empty() {
            return Err(TextureError::EmptyInput);
        }
        let image = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|error| TextureError::Decode {
                path: "<memory>".to_owned(),
                reason: error.to_string(),
            })?
            .decode()
            .map_err(|error| TextureError::Decode {
                path: "<memory>".to_owned(),
                reason: error.to_string(),
            })?
            .into_rgba8();
        let (width, height) = image.dimensions();
        let pixels = image.into_raw();
        let expected = width
            .checked_mul(height)
            .and_then(|value| value.checked_mul(4))
            .ok_or(TextureError::TooLarge { width, height })? as usize;
        if pixels.len() != expected {
            return Err(TextureError::InvalidDecodedSize {
                width,
                height,
                actual: pixels.len(),
                expected,
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
            color_space,
        })
    }

    /// Creates an RGBA8 texture asset from already-decoded pixels.
    pub fn from_rgba8(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        color_space: TextureColorSpace,
    ) -> Result<Self, TextureError> {
        let expected = width
            .checked_mul(height)
            .and_then(|value| value.checked_mul(4))
            .ok_or(TextureError::TooLarge { width, height })? as usize;
        if width == 0 || height == 0 || pixels.len() != expected {
            return Err(TextureError::InvalidRgba8Size {
                width,
                height,
                actual: pixels.len(),
                expected,
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
            color_space,
        })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub const fn color_space(&self) -> TextureColorSpace {
        self.color_space
    }

    #[must_use]
    pub const fn bytes_per_row(&self) -> u32 {
        self.width * 4
    }
}

/// 把 RGBA8 像素写成 PNG 文件。
///
/// 解码侧一直有入口（[`TextureAsset::from_png_bytes`]），编码侧此前没有——于是"真实窗口截图"
/// 只能留在内存里，而**证据不能归档就等于没有证据**（docs/55 阶段 0 要求 Windows
/// 100/150/200% DPI 截图归档）。这里复用引擎已经依赖的 `image` crate，不引入新依赖。
pub fn save_png_rgba8(
    path: impl AsRef<Path>,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), TextureError> {
    let path = path.as_ref();
    let expected = width
        .checked_mul(height)
        .and_then(|value| value.checked_mul(4))
        .ok_or(TextureError::TooLarge { width, height })? as usize;
    if width == 0 || height == 0 || pixels.len() != expected {
        return Err(TextureError::InvalidRgba8Size {
            width,
            height,
            actual: pixels.len(),
            expected,
        });
    }
    let image = image::RgbaImage::from_raw(width, height, pixels.to_vec()).ok_or(
        TextureError::InvalidRgba8Size {
            width,
            height,
            actual: pixels.len(),
            expected,
        },
    )?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|reason| TextureError::Write {
            path: path.display().to_string(),
            reason: reason.to_string(),
        })?;
    }
    image
        .save_with_format(path, image::ImageFormat::Png)
        .map_err(|reason| TextureError::Write {
            path: path.display().to_string(),
            reason: reason.to_string(),
        })?;
    Ok(())
}

/// Errors returned by PNG decoding and texture data validation.
///
/// 文本字段统一命名为 `reason`（而不是 `source`）：thiserror 会把名为 `source` 的字段
/// 视为错误源并要求它实现 `std::error::Error`，而这里携带的是已格式化的文本。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TextureError {
    #[error("texture input is empty")]
    EmptyInput,
    #[error("could not read PNG {path}: {reason}")]
    Read { path: String, reason: String },
    #[error("could not write PNG {path}: {reason}")]
    Write { path: String, reason: String },
    #[error("could not decode PNG {path}: {reason}")]
    Decode { path: String, reason: String },
    #[error("texture dimensions overflow RGBA8 size: {width}x{height}")]
    TooLarge { width: u32, height: u32 },
    #[error(
        "decoded PNG size mismatch for {width}x{height}: got {actual} bytes, expected {expected}"
    )]
    InvalidDecodedSize {
        width: u32,
        height: u32,
        actual: usize,
        expected: usize,
    },
    #[error("invalid RGBA8 size for {width}x{height}: got {actual} bytes, expected {expected}")]
    InvalidRgba8Size {
        width: u32,
        height: u32,
        actual: usize,
        expected: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::{TextureAsset, TextureColorSpace, TextureError, save_png_rgba8};

    #[test]
    fn saved_png_round_trips_through_the_loader() {
        // docs/55 阶段 0：截图归档要能"写出去再读回来"，否则归档的文件无法被证据脚本校验。
        let root = std::env::temp_dir().join(format!(
            "rustlua-texture-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        let path = root.join("nested/evidence.png");
        let pixels: Vec<u8> = (0..(4 * 3 * 4))
            .map(|index| (index * 7 % 251) as u8)
            .collect();
        save_png_rgba8(&path, 4, 3, &pixels).expect("保存 PNG 必须成功（含自动建目录）");
        let loaded = TextureAsset::load_png(&path, TextureColorSpace::Srgb).expect("回读");
        assert_eq!(loaded.width(), 4);
        assert_eq!(loaded.height(), 3);
        assert_eq!(loaded.pixels(), pixels.as_slice());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn saving_rejects_wrong_pixel_count_before_touching_the_disk() {
        let path = std::env::temp_dir().join("rustlua-texture-save-invalid/must-not-exist.png");
        let error = save_png_rgba8(&path, 2, 2, &[0, 0, 0]).unwrap_err();
        assert!(matches!(error, TextureError::InvalidRgba8Size { .. }));
        assert!(!path.exists(), "尺寸不合法时不得留下半截文件");
    }

    fn red_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0])))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn decodes_png_to_rgba8_and_preserves_color_space() {
        let red_png = red_png();
        let texture = TextureAsset::from_png_bytes(&red_png, TextureColorSpace::Srgb).unwrap();
        assert_eq!(texture.width(), 1);
        assert_eq!(texture.height(), 1);
        assert_eq!(texture.pixels(), &[255, 0, 0, 255]);
        assert_eq!(texture.bytes_per_row(), 4);
        assert_eq!(texture.color_space(), TextureColorSpace::Srgb);
    }

    #[test]
    fn rejects_empty_and_invalid_png_input() {
        assert_eq!(
            TextureAsset::from_png_bytes(&[], TextureColorSpace::Srgb),
            Err(TextureError::EmptyInput)
        );
        assert!(matches!(
            TextureAsset::from_png_bytes(b"not png", TextureColorSpace::Linear),
            Err(TextureError::Decode { .. })
        ));
    }

    #[test]
    fn validates_raw_rgba8_dimensions() {
        assert!(TextureAsset::from_rgba8(2, 1, vec![0; 4], TextureColorSpace::Linear).is_err());
        let texture =
            TextureAsset::from_rgba8(2, 1, vec![0; 8], TextureColorSpace::Linear).unwrap();
        assert_eq!(texture.color_space(), TextureColorSpace::Linear);
    }
}
