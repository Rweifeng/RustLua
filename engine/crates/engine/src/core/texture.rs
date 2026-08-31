//! PNG decoding and CPU-side texture asset representation for M3-02.

use std::{error::Error, fmt, path::Path};

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
        let bytes = std::fs::read(path).map_err(|source| TextureError::Read {
            path: path.display().to_string(),
            source: source.to_string(),
        })?;
        let bytes = crate::core::pack::decode(&bytes).map_err(|source| TextureError::Read {
            path: path.display().to_string(),
            source,
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

/// Errors returned by PNG decoding and texture data validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextureError {
    EmptyInput,
    Read {
        path: String,
        source: String,
    },
    Decode {
        path: String,
        reason: String,
    },
    TooLarge {
        width: u32,
        height: u32,
    },
    InvalidDecodedSize {
        width: u32,
        height: u32,
        actual: usize,
        expected: usize,
    },
    InvalidRgba8Size {
        width: u32,
        height: u32,
        actual: usize,
        expected: usize,
    },
}

impl fmt::Display for TextureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(formatter, "texture input is empty"),
            Self::Read { path, source } => write!(formatter, "could not read PNG {path}: {source}"),
            Self::Decode { path, reason } => {
                write!(formatter, "could not decode PNG {path}: {reason}")
            }
            Self::TooLarge { width, height } => write!(
                formatter,
                "texture dimensions overflow RGBA8 size: {width}x{height}"
            ),
            Self::InvalidDecodedSize {
                width,
                height,
                actual,
                expected,
            } => write!(
                formatter,
                "decoded PNG size mismatch for {width}x{height}: got {actual} bytes, expected {expected}"
            ),
            Self::InvalidRgba8Size {
                width,
                height,
                actual,
                expected,
            } => write!(
                formatter,
                "invalid RGBA8 size for {width}x{height}: got {actual} bytes, expected {expected}"
            ),
        }
    }
}

impl Error for TextureError {}

#[cfg(test)]
mod tests {
    use super::{TextureAsset, TextureColorSpace, TextureError};

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
