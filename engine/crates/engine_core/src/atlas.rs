//! Texture atlas regions and pixel-to-UV conversion for 2D Sprites.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::{
    sprite::{Sprite, SpriteError},
    texture::TextureAsset,
    transform::Vec2,
};

/// A normalized texture rectangle. UV origin is the texture's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtlasRegion {
    pub uv_min: Vec2,
    pub uv_max: Vec2,
    pub pixel_min: [u32; 2],
    pub pixel_size: [u32; 2],
}

impl AtlasRegion {
    pub fn from_pixels(
        texture_width: u32,
        texture_height: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Self, AtlasError> {
        if texture_width == 0 || texture_height == 0 || width == 0 || height == 0 {
            return Err(AtlasError::InvalidDimensions {
                texture_width,
                texture_height,
                x,
                y,
                width,
                height,
            });
        }
        let end_x = x.checked_add(width).ok_or(AtlasError::InvalidDimensions {
            texture_width,
            texture_height,
            x,
            y,
            width,
            height,
        })?;
        let end_y = y.checked_add(height).ok_or(AtlasError::InvalidDimensions {
            texture_width,
            texture_height,
            x,
            y,
            width,
            height,
        })?;
        if end_x > texture_width || end_y > texture_height {
            return Err(AtlasError::InvalidDimensions {
                texture_width,
                texture_height,
                x,
                y,
                width,
                height,
            });
        }
        Ok(Self {
            uv_min: Vec2::new(
                x as f32 / texture_width as f32,
                y as f32 / texture_height as f32,
            ),
            uv_max: Vec2::new(
                end_x as f32 / texture_width as f32,
                end_y as f32 / texture_height as f32,
            ),
            pixel_min: [x, y],
            pixel_size: [width, height],
        })
    }
    pub fn from_asset(
        asset: &TextureAsset,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Self, AtlasError> {
        Self::from_pixels(asset.width(), asset.height(), x, y, width, height)
    }
    pub fn apply(self, sprite: Sprite) -> Result<Sprite, SpriteError> {
        sprite.with_uv(self.uv_min, self.uv_max)
    }
}

/// Named atlas regions backed by one texture.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteAtlas {
    width: u32,
    height: u32,
    regions: BTreeMap<String, AtlasRegion>,
}

impl SpriteAtlas {
    pub fn new(width: u32, height: u32) -> Result<Self, AtlasError> {
        if width == 0 || height == 0 {
            return Err(AtlasError::InvalidAtlasSize { width, height });
        }
        Ok(Self {
            width,
            height,
            regions: BTreeMap::new(),
        })
    }
    pub fn from_asset(asset: &TextureAsset) -> Result<Self, AtlasError> {
        Self::new(asset.width(), asset.height())
    }
    pub const fn width(&self) -> u32 {
        self.width
    }
    pub const fn height(&self) -> u32 {
        self.height
    }
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<AtlasRegion, AtlasError> {
        let region = AtlasRegion::from_pixels(self.width, self.height, x, y, width, height)?;
        self.regions.insert(name.into(), region);
        Ok(region)
    }
    #[must_use]
    pub fn get(&self, name: &str) -> Option<AtlasRegion> {
        self.regions.get(name).copied()
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.regions.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtlasError {
    InvalidAtlasSize {
        width: u32,
        height: u32,
    },
    InvalidDimensions {
        texture_width: u32,
        texture_height: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
}
impl fmt::Display for AtlasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAtlasSize { width, height } => {
                write!(f, "invalid atlas size: {width}x{height}")
            }
            Self::InvalidDimensions {
                texture_width,
                texture_height,
                x,
                y,
                width,
                height,
            } => write!(
                f,
                "atlas region ({x},{y},{width},{height}) exceeds {texture_width}x{texture_height}"
            ),
        }
    }
}
impl Error for AtlasError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sprite::Sprite,
        texture::{TextureAsset, TextureColorSpace},
        transform::Vec2,
    };
    #[test]
    fn converts_pixel_rect_to_top_left_uv() {
        let r = AtlasRegion::from_pixels(256, 128, 64, 32, 128, 64).unwrap();
        assert_eq!(r.uv_min, Vec2::new(0.25, 0.25));
        assert_eq!(r.uv_max, Vec2::new(0.75, 0.75));
        assert_eq!(r.pixel_min, [64, 32]);
        assert_eq!(r.pixel_size, [128, 64]);
    }
    #[test]
    fn rejects_out_of_bounds_and_zero_regions() {
        assert!(AtlasRegion::from_pixels(4, 4, 0, 0, 0, 1).is_err());
        assert!(AtlasRegion::from_pixels(4, 4, 3, 3, 2, 1).is_err());
        assert!(SpriteAtlas::new(0, 1).is_err());
    }
    #[test]
    fn named_regions_are_stable_and_apply_to_sprite() {
        let asset = TextureAsset::from_rgba8(4, 2, vec![255; 32], TextureColorSpace::Srgb).unwrap();
        let mut atlas = SpriteAtlas::from_asset(&asset).unwrap();
        let region = atlas.insert("hero", 0, 0, 2, 2).unwrap();
        assert_eq!(atlas.get("hero"), Some(region));
        assert_eq!(atlas.len(), 1);
        let sprite = region
            .apply(Sprite::new(Vec2::new(1.0, 1.0)).unwrap())
            .unwrap();
        assert_eq!(sprite.uv_max, Vec2::new(0.5, 1.0));
    }
}
