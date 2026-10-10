//! Texture atlas regions and pixel-to-UV conversion for 2D Sprites.

use std::collections::BTreeMap;

use crate::sprite::{Sprite, SpriteError};
use crate::texture::TextureAsset;
use rustlua_math::math2d::Vec2;

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
    /// 从 `.atlas` 源解析结果按真实纹理尺寸构造命名图集。
    ///
    /// 导入器（rustlua-assets）只做语法与零尺寸校验；矩形越界要到这一步
    /// 拿到纹理尺寸后才能判定，由 `AtlasRegion::from_pixels` 拒绝。
    pub fn from_definition(
        definition: &rustlua_assets::atlas_format::AtlasDefinition,
        width: u32,
        height: u32,
    ) -> Result<Self, AtlasBuildError> {
        let mut atlas = Self::new(width, height)?;
        for (name, rect) in &definition.regions {
            atlas.insert(name.clone(), rect.x, rect.y, rect.width, rect.height)?;
        }
        Ok(atlas)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AtlasError {
    #[error("invalid atlas size: {width}x{height}")]
    InvalidAtlasSize { width: u32, height: u32 },
    #[error("atlas region ({x},{y},{width},{height}) exceeds {texture_width}x{texture_height}")]
    InvalidDimensions {
        texture_width: u32,
        texture_height: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
}

/// 从 `.atlas` 定义构造 `SpriteAtlas` 时的失败：源语法/字段问题或区域越界。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AtlasBuildError {
    #[error(transparent)]
    Format(#[from] rustlua_assets::atlas_format::AtlasFormatError),
    #[error(transparent)]
    Atlas(#[from] AtlasError),
}

/// 从磁盘加载完成的图集：纹理工程根相对路径 + 命名区域表。
#[derive(Clone, Debug)]
pub struct LoadedAtlas {
    pub texture_path: String,
    pub atlas: SpriteAtlas,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AtlasLoadError {
    #[error("无效的图集引用 {raw:?}（需为 图集路径#区域名）")]
    InvalidReference { raw: String },
    #[error("区域 {region:?} 不存在于图集 {atlas:?}")]
    UnknownRegion { atlas: String, region: String },
    #[error("{path}: 读取图集纹理尺寸失败: {reason}")]
    TextureIo { path: String, reason: String },
    #[error(transparent)]
    Format(#[from] rustlua_assets::atlas_format::AtlasFormatError),
    #[error(transparent)]
    Build(#[from] AtlasBuildError),
}

/// 把项目根相对路径落到磁盘，规则与渲染器读纹理一致（`runtime_scene` 的
/// `project_root.join(path)`，无根则用原始路径）。
fn resolve_with_root(
    root: Option<&std::path::Path>,
    relative: &std::path::Path,
) -> std::path::PathBuf {
    match root {
        Some(root) => root.join(relative),
        None => relative.to_path_buf(),
    }
}

/// 读一个 `.atlas` 源并绑定其纹理的实际尺寸。
///
/// `root` 是项目根（`None` = 路径已按 cwd 给出）。`.atlas` 与它的 `texture=` 都按同一
/// 规则解析；返回的 `texture_path` **保持 `.atlas` 里写的那份路径**（工程根相对），
/// 交给 `SpriteAsset` 后由渲染器再 join 一次，避免两条消费路径对不上。
///
/// 尺寸只取图片头，不做整图解码，但**要过 [`rustlua_infra::pack::decode`]**：发布包里
/// assetc 会把 `assets/` 下的 png 压缩成 `RustLua` 容器，直接读原始字节连尺寸都取不出来。
/// 像素数据仍由既有纹理加载/上传管线负责。
pub fn load_atlas(
    root: Option<&std::path::Path>,
    atlas_path: &std::path::Path,
) -> Result<LoadedAtlas, AtlasLoadError> {
    let definition =
        rustlua_assets::atlas_format::AtlasDefinition::load(resolve_with_root(root, atlas_path))?;
    let texture_path = definition.texture.clone();
    let resolved_texture = resolve_with_root(root, std::path::Path::new(&definition.texture));
    let io_error = |reason: String| AtlasLoadError::TextureIo {
        path: resolved_texture.display().to_string(),
        reason,
    };
    // 同 `TextureAsset::load_png`：wasm 上没有 `std::fs`，图集贴图必须经 `vfs` 取
    // （桌面无 provider 时行为不变）。图集定义文本那一步在
    // `rustlua_assets::atlas_format::AtlasDefinition::load` 里同样走 `vfs`。
    let bytes =
        rustlua_infra::vfs::read(&resolved_texture).map_err(|error| io_error(error.to_string()))?;
    let bytes = rustlua_infra::pack::decode(&bytes).map_err(io_error)?;
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| io_error(error.to_string()))
        .and_then(|reader| {
            reader
                .into_dimensions()
                .map_err(|error| io_error(error.to_string()))
        })?;
    let atlas = SpriteAtlas::from_definition(&definition, width, height)?;
    Ok(LoadedAtlas {
        texture_path,
        atlas,
    })
}

/// 解析 `图集路径#区域名` 引用；不含 `#` 的普通纹理路径返回 `Ok(None)`（原样透传）。
/// `root` 同 [`load_atlas`]：项目根，`None` 表示按 cwd 解析。
pub fn resolve_atlas_reference(
    root: Option<&std::path::Path>,
    raw: &str,
) -> Result<Option<(String, AtlasRegion)>, AtlasLoadError> {
    let Some((atlas_path, region_name)) = raw.split_once('#') else {
        return Ok(None);
    };
    if atlas_path.trim().is_empty() || region_name.trim().is_empty() {
        return Err(AtlasLoadError::InvalidReference {
            raw: raw.to_owned(),
        });
    }
    let loaded = load_atlas(root, std::path::Path::new(atlas_path))?;
    let region = loaded
        .atlas
        .get(region_name)
        .ok_or_else(|| AtlasLoadError::UnknownRegion {
            atlas: atlas_path.to_owned(),
            region: region_name.to_owned(),
        })?;
    Ok(Some((loaded.texture_path, region)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::TextureColorSpace;

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
    #[test]
    fn builds_atlas_from_definition_and_rejects_out_of_bounds() {
        let definition = rustlua_assets::atlas_format::AtlasDefinition::from_text(
            "atlas_version=1\ntexture=sheet.png\nregion\tidle\t0\t0\t2\t2\nregion\twalk\t2\t0\t2\t2\n",
        )
        .unwrap();
        let atlas = SpriteAtlas::from_definition(&definition, 4, 2).unwrap();
        assert_eq!(atlas.len(), 2);
        assert_eq!(
            atlas.get("walk").unwrap().uv_min,
            Vec2::new(0.5, 0.0),
            "UV 换算必须沿用 from_pixels 的左上原点约定"
        );
        // 越界矩形在拿到纹理尺寸后拒绝，而不是静默截断。
        let error = SpriteAtlas::from_definition(&definition, 3, 2)
            .expect_err("3px 宽的纹理容不下 x=2,w=2 的区域");
        assert!(matches!(
            error,
            AtlasBuildError::Atlas(AtlasError::InvalidDimensions { .. })
        ));
    }
    #[test]
    fn resolve_atlas_reference_loads_real_files() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-atlas-resolve-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let png = root.join("sheet.png");
        {
            let mut img = image::RgbaImage::new(4, 2);
            for (x, _y, pixel) in img.enumerate_pixels_mut() {
                *pixel = if x < 2 {
                    image::Rgba([255, 0, 0, 255])
                } else {
                    image::Rgba([0, 255, 0, 255])
                };
            }
            img.save(&png).unwrap();
        }
        // 工程根相对写法（真实项目里就是这个形状）：`.atlas` 与 `texture=` 都按根解析。
        std::fs::write(
            root.join("hero.atlas"),
            "atlas_version=1\ntexture=sheet.png\nregion\twalk\t2\t0\t2\t2\n",
        )
        .unwrap();
        let root = root.as_path();

        // 不含 '#' 的普通纹理路径原样透传。
        assert_eq!(
            resolve_atlas_reference(Some(root), "textures/plain.png").unwrap(),
            None
        );
        let resolved = resolve_atlas_reference(Some(root), "hero.atlas#walk")
            .unwrap()
            .expect("atlas 引用必须解析");
        assert_eq!(
            resolved.0, "sheet.png",
            "返回的纹理路径要保持工程根相对，交给渲染器再 join"
        );
        assert_eq!(resolved.1.uv_min, Vec2::new(0.5, 0.0));
        assert_eq!(resolved.1.uv_max, Vec2::new(1.0, 1.0));
        assert_eq!(
            resolve_atlas_reference(Some(root), "hero.atlas#fly").unwrap_err(),
            AtlasLoadError::UnknownRegion {
                atlas: "hero.atlas".to_owned(),
                region: "fly".to_owned(),
            }
        );
        assert!(matches!(
            resolve_atlas_reference(Some(root), "#walk").unwrap_err(),
            AtlasLoadError::InvalidReference { .. }
        ));
        // 图集文件不在根下（相对路径解析不到）也要响亮失败，而不是静默回退整图。
        assert!(resolve_atlas_reference(Some(root), "nope.atlas#walk").is_err());
        // 纹理文件缺失：诊断指向解析后的磁盘路径。
        std::fs::remove_file(&png).unwrap();
        let error = resolve_atlas_reference(Some(root), "hero.atlas#walk").unwrap_err();
        assert!(
            matches!(error, AtlasLoadError::TextureIo { .. })
                && error.to_string().contains("sheet.png"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 发布包里的 png 会被 assetc gzip 压缩成 `RustLua` 容器，纹理尺寸读取必须先过
    /// `pack::decode`，否则连头信息都取不到；`.atlas` 在包里保持明文，同一次调用里
    /// 两份文件按各自真实形态读取。
    #[test]
    fn load_atlas_reads_packed_texture_with_plain_definition() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-atlas-packed-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let mut img = image::RgbaImage::new(8, 2);
        for (x, _y, pixel) in img.enumerate_pixels_mut() {
            *pixel = if x < 4 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            };
        }
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        std::fs::write(
            root.join("sheet.png"),
            rustlua_infra::pack::encode(&png_bytes, true, false),
        )
        .unwrap();
        std::fs::write(
            root.join("hero.atlas"),
            "atlas_version=1\ntexture=sheet.png\nregion\twalk\t4\t0\t4\t2\n",
        )
        .unwrap();

        let loaded = load_atlas(Some(&root), std::path::Path::new("hero.atlas")).unwrap();
        assert_eq!(loaded.texture_path, "sheet.png");
        assert_eq!(loaded.atlas.width(), 8);
        assert_eq!(loaded.atlas.height(), 2);
        let region = loaded.atlas.get("walk").unwrap();
        assert_eq!(region.uv_min, Vec2::new(0.5, 0.0));
        assert_eq!(region.uv_max, Vec2::new(1.0, 1.0));
        assert_eq!(region.pixel_size, [4, 2]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
