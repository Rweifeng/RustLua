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

/// 重定向表的内存形态：`原图路径 → (图集路径, 区域名)`。
type RedirectMap = std::collections::HashMap<String, (String, String)>;
/// 重定向表缓存：工程根 → 表（`None` = 该根下没有/读不出表）。
type RedirectCache = std::sync::Mutex<
    std::collections::HashMap<std::path::PathBuf, Option<std::sync::Arc<RedirectMap>>>,
>;

/// 页缓存的键：`(工程根, .atlas 路径)`。
type AtlasCacheKey = (std::path::PathBuf, std::path::PathBuf);
/// 页缓存：已解析的页 + 区域表。
type AtlasCache =
    std::sync::Mutex<std::collections::HashMap<AtlasCacheKey, std::sync::Arc<LoadedAtlas>>>;

/// 重定向表缓存（进程内）。
static REDIRECT_CACHE: std::sync::OnceLock<RedirectCache> = std::sync::OnceLock::new();

/// 页缓存：`(工程根, .atlas 路径)` → 已解析的页 + 区域表。
///
/// **为什么必须有**：装配（`runtime_scene` 的逐帧提取、`mesh_ui` 的逐帧 UI）每帧对
/// 每个精灵/每张图各查一次重定向。没有这层缓存时每次都要读 `.atlas`、读**整页字节**、
/// 重建区域表——容器形态下还要对整页做 FNV 校验并拷贝出来（页 raw 实测可到 3.7 MB），
/// 一帧几十 MB 的拷贝，图集在局内根本不可用（`docs/67` 复核 P0-1）。
static ATLAS_CACHE: std::sync::OnceLock<AtlasCache> = std::sync::OnceLock::new();

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
    // 页的两种形态：`.rltex`（裸像素，发布形态）与 PNG（开发形态/美术手工图集）。
    // 只读尺寸，不解像素——两边都不该为拿宽高付一次完整解码。
    let (width, height) = if rustlua_assets::rltex::is_rltex(&bytes) {
        rustlua_assets::rltex::dimensions(&bytes)
            .ok_or_else(|| io_error(".rltex 头部截断，取不到尺寸".to_owned()))?
    } else {
        image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|error| io_error(error.to_string()))
            .and_then(|reader| {
                reader
                    .into_dimensions()
                    .map_err(|error| io_error(error.to_string()))
            })?
    };
    let atlas = SpriteAtlas::from_definition(&definition, width, height)?;
    Ok(LoadedAtlas {
        texture_path,
        atlas,
    })
}

/// 读一个 `.atlas`（带缓存）：同一进程里同一张页只解析一次。
///
/// `root = None` 时不缓存（开发期按 cwd 直接给绝对路径的场合，路径不构成稳定键）。
fn load_atlas_cached(
    root: Option<&std::path::Path>,
    atlas_path: &std::path::Path,
) -> Result<std::sync::Arc<LoadedAtlas>, AtlasLoadError> {
    let Some(root) = root else {
        return Ok(std::sync::Arc::new(load_atlas(None, atlas_path)?));
    };
    let key = (root.to_path_buf(), atlas_path.to_path_buf());
    let cache = ATLAS_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    // 先查后放：不持锁做 IO（读盘/解包可能慢，锁里做会让所有装配线程排队）。
    if let Ok(guard) = cache.lock()
        && let Some(hit) = guard.get(&key)
    {
        return Ok(std::sync::Arc::clone(hit));
    }
    let loaded = std::sync::Arc::new(load_atlas(Some(root), atlas_path)?);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, std::sync::Arc::clone(&loaded));
    }
    Ok(loaded)
}

/// 清空装配期缓存（热重载 / 编辑器重新 cook 之后调用）。
///
/// 两个缓存都按"进程内不变"假设建立（工程根、页内容），热重载会打破这个前提，
/// 因此必须有一个显式出口；不调用时的行为等于"重启后读到什么就一直用什么"。
pub fn clear_atlas_cache() {
    if let Some(cache) = REDIRECT_CACHE.get()
        && let Ok(mut guard) = cache.lock()
    {
        guard.clear();
    }
    if let Some(cache) = ATLAS_CACHE.get()
        && let Ok(mut guard) = cache.lock()
    {
        guard.clear();
    }
}

/// 解析 `图集路径#区域名` 引用；不含 `#` 的普通纹理路径返回 `Ok(None)`（原样透传）。
/// `root` 同 [`load_atlas`]：项目根，`None` 表示按 cwd 解析。
///
/// **先查重定向表**（`docs/67` §6.4）：构建期图集 cook 把散图换成了页，工程源里仍然写着
/// `assets/art/ui/hud/ui_icon_hp.png`。这条重定向让"工程源一行不改"成立——装配期把原路径
/// 翻成 `图集#区域`，后面的抠图/合批代码零改动。
pub fn resolve_atlas_reference(
    root: Option<&std::path::Path>,
    raw: &str,
) -> Result<Option<(String, AtlasRegion)>, AtlasLoadError> {
    if let Some((atlas, region)) = redirect_lookup(root, raw) {
        return resolve_atlas_reference(root, &format!("{atlas}#{region}"));
    }
    let Some((atlas_path, region_name)) = raw.split_once('#') else {
        return Ok(None);
    };
    if atlas_path.trim().is_empty() || region_name.trim().is_empty() {
        return Err(AtlasLoadError::InvalidReference {
            raw: raw.to_owned(),
        });
    }
    let loaded = load_atlas_cached(root, std::path::Path::new(atlas_path))?;
    let region = loaded
        .atlas
        .get(region_name)
        .ok_or_else(|| AtlasLoadError::UnknownRegion {
            atlas: atlas_path.to_owned(),
            region: region_name.to_owned(),
        })?;
    Ok(Some((loaded.texture_path.clone(), region)))
}

/// 查构建期生成的重定向表（`atlas_redirect.json`，`docs/67` §6.4）。
///
/// 缓存按**工程根**一行：同一进程里 pack 根不会变，而每个 Sprite 实例装配一次
/// （几百个实例 = 几百次查询），不缓存就是几百次小文件 IO + JSON 解析。
fn redirect_lookup(root: Option<&std::path::Path>, original: &str) -> Option<(String, String)> {
    let root = root?;
    let cache =
        REDIRECT_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let table = {
        let mut guard = cache.lock().ok()?;
        let entry = guard
            .entry(root.to_path_buf())
            .or_insert_with(|| load_redirect_map(root).map(std::sync::Arc::new));
        entry.clone()?
    };
    table.get(original).cloned()
}

fn load_redirect_map(
    root: &std::path::Path,
) -> Option<std::collections::HashMap<String, (String, String)>> {
    let path = root.join(rustlua_infra::atlas_redirect::REDIRECT_FILE);
    let bytes = rustlua_infra::vfs::read(&path).ok()?;
    let bytes = rustlua_infra::pack::decode(&bytes).ok()?;
    let redirect = rustlua_infra::atlas_redirect::AtlasRedirect::parse(&bytes).ok()?;
    Some(
        redirect
            .entries
            .into_iter()
            .map(|(source, entry)| (source, (entry.atlas, entry.region)))
            .collect(),
    )
}

/// 装配期用的工程根：wasm 上固定 `/project`（与 vfs provider 的键域一致），桌面上是 cwd。
///
/// 为什么需要它：容器/图集的读取都经 `vfs`，而 `vfs` 的键域是"工程根 + 相对路径"。
/// 桌面跑在工程目录里，`None`（= 相对路径原样）就够；wasm 必须显式带 `/project`，
/// 否则 provider 的 `key_of` 剥不掉前缀、查不到条目。
///
/// 注意：这里存**字符串**而不是 `Option<&Path>`。`Path::new` 在 wasm 那套固定工具链
/// （`+1.88.0`，见 `scripts\build\package.ps1`）里不是 const fn，写成
/// `const … = Some(Path::new("/project"))` 会让整个 web 构建在 E0015 停下（2026-10-18 实测）。
#[cfg(target_arch = "wasm32")]
pub const PROJECT_ROOT: &str = "/project";
#[cfg(not(target_arch = "wasm32"))]
pub const PROJECT_ROOT: &str = "";

/// 把"工程源里写的原图路径"翻译成"图集页 + 区域"（`docs/67` §6.4）。
///
/// 这是图集在**渲染路径**上生效的唯一入口：GPU 侧精灵装配与贴图上传都以它为准——
/// 命中后 `Sprite.texture` 换成页路径、`uv` 换成区域子矩形，于是
/// ① 贴图上传会去取**页**（不再去找已被 cook 删掉的散图）；
/// ② 同一页的多个精灵天然共享一个 texture id，合批在同一层内自动成立。
///
/// 返回 `None` 表示没有重定向（普通贴图），调用方按原路径处理。
pub fn redirect_region_with_root(
    root: Option<&std::path::Path>,
    path: &str,
) -> Option<(String, AtlasRegion)> {
    let (atlas, region_name) = redirect_lookup(root, path)?;
    let loaded = load_atlas_cached(root, std::path::Path::new(&atlas)).ok()?;
    let region = loaded.atlas.get(&region_name)?;
    Some((loaded.texture_path.clone(), region))
}

/// 同 [`redirect_region_with_root`]，工程根取平台默认值（见 [`PROJECT_ROOT`]）。
pub fn redirect_region(path: &str) -> Option<(String, AtlasRegion)> {
    let root = if PROJECT_ROOT.is_empty() {
        None
    } else {
        Some(std::path::Path::new(PROJECT_ROOT))
    };
    redirect_region_with_root(root, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::TextureColorSpace;

    /// 页缓存是**进程级全局**：任何测试里的 `clear_atlas_cache()` 都必须串行，
    /// 否则会清掉另一个测试刚建立、正准备第二次命中的缓存（并行跑实测偶发红）。
    static ATLAS_CACHE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _cache_guard = ATLAS_CACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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
        // 页缓存按"进程内同一张页只解析一次"工作，所以要先清缓存再模拟"冷启动读不到"。
        std::fs::remove_file(&png).unwrap();
        clear_atlas_cache();
        let error = resolve_atlas_reference(Some(root), "hero.atlas#walk").unwrap_err();
        assert!(
            matches!(error, AtlasLoadError::TextureIo { .. })
                && error.to_string().contains("sheet.png"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// P0-1 的性能门：同一张页的重复查询**不该再读盘**（装配期是逐帧调用的）。
    ///
    /// 判据选"删掉文件后仍然命中"：如果实现退化成每次 `load_atlas`，第二次就会报错。
    #[test]
    fn repeated_lookups_use_the_cached_page() {
        use rustlua_infra::atlas_redirect::{AtlasRedirect, REDIRECT_FILE, RedirectEntry};

        let _cache_guard = ATLAS_CACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        clear_atlas_cache();
        let root = std::env::temp_dir().join(format!(
            "rustlua-atlas-cache-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/atlases")).unwrap();
        let page = rustlua_assets::rltex::encode(16, 16, &vec![0u8; 16 * 16 * 4]).unwrap();
        std::fs::write(root.join("assets/atlases/ui-0.rltex"), &page).unwrap();
        std::fs::write(
            root.join("assets/atlases/ui-0.atlas"),
            "atlas_version=1\ntexture=assets/atlases/ui-0.rltex\nregion\thud_a\t0\t0\t8\t8\n",
        )
        .unwrap();
        let mut redirect = AtlasRedirect::empty();
        redirect.entries.insert(
            "assets/art/ui/hud/a.png".to_owned(),
            RedirectEntry {
                atlas: "assets/atlases/ui-0.atlas".to_owned(),
                region: "hud_a".to_owned(),
            },
        );
        std::fs::write(root.join(REDIRECT_FILE), redirect_json(&redirect)).unwrap();

        let first = redirect_region_with_root(Some(&root), "assets/art/ui/hud/a.png")
            .expect("首次查询必须命中");
        // 删掉页与定义：缓存命中时第二次照样给结果，未缓存则必然 None。
        std::fs::remove_file(root.join("assets/atlases/ui-0.rltex")).unwrap();
        std::fs::remove_file(root.join("assets/atlases/ui-0.atlas")).unwrap();
        let second = redirect_region_with_root(Some(&root), "assets/art/ui/hud/a.png")
            .expect("第二次查询必须走缓存");
        assert_eq!(first.0, second.0);
        assert_eq!(first.1, second.1);

        // 显式清缓存后回到"读盘"语义（热重载/编辑器重新 cook 的出口）。
        clear_atlas_cache();
        assert!(redirect_region_with_root(Some(&root), "assets/art/ui/hud/a.png").is_none());
        let _ = std::fs::remove_dir_all(&root);
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

    /// 手写重定向表 JSON（`rustlua-render` 没有 `serde_json` 依赖，测试也不该为它引入一个）。
    fn redirect_json(redirect: &rustlua_infra::atlas_redirect::AtlasRedirect) -> Vec<u8> {
        let entries: Vec<String> = redirect
            .entries
            .iter()
            .map(|(source, entry)| {
                format!(
                    "\"{source}\":{{\"atlas\":\"{}\",\"region\":\"{}\"}}",
                    entry.atlas, entry.region
                )
            })
            .collect();
        format!(
            "{{\"schema\":1,\"format\":\"assetc-atlas-redirect-1\",\"entries\":{{{}}}}}",
            entries.join(",")
        )
        .into_bytes()
    }

    /// 装配期入口（`redirect_region_with_root`）：命中就给出「页 + 区域」，没命中返回 None。
    ///
    /// 这条是 GPU 侧图集生效的关键——不是再写一遍 `resolve_atlas_reference` 的断言，
    /// 而是确认"给定工程根，装配期能拿到页路径与区域 UV"。
    #[test]
    fn redirect_region_returns_page_and_region() {
        use rustlua_infra::atlas_redirect::{AtlasRedirect, REDIRECT_FILE, RedirectEntry};

        let root = std::env::temp_dir().join(format!(
            "rustlua-atlas-assemble-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/atlases")).unwrap();
        let page = rustlua_assets::rltex::encode(16, 16, &vec![0u8; 16 * 16 * 4]).unwrap();
        std::fs::write(root.join("assets/atlases/ui-0.rltex"), &page).unwrap();
        std::fs::write(
            root.join("assets/atlases/ui-0.atlas"),
            "atlas_version=1\ntexture=assets/atlases/ui-0.rltex\nregion\thud_a\t0\t0\t8\t8\n",
        )
        .unwrap();
        let mut redirect = AtlasRedirect::empty();
        redirect.entries.insert(
            "assets/art/ui/hud/a.png".to_owned(),
            RedirectEntry {
                atlas: "assets/atlases/ui-0.atlas".to_owned(),
                region: "hud_a".to_owned(),
            },
        );
        std::fs::write(
            root.join(REDIRECT_FILE),
            concat!(
                "{\"schema\":1,\"format\":\"assetc-atlas-redirect-1\",\"entries\":",
                "{\"assets/art/ui/hud/a.png\":",
                "{\"atlas\":\"assets/atlases/ui-0.atlas\",\"region\":\"hud_a\"}}}"
            ),
        )
        .unwrap();

        let (page_path, region) =
            redirect_region_with_root(Some(&root), "assets/art/ui/hud/a.png").expect("命中重定向");
        assert_eq!(page_path, "assets/atlases/ui-0.rltex");
        assert_eq!(region.pixel_size, [8, 8]);
        assert_eq!(region.uv_max, Vec2::new(0.5, 0.5));

        // 没登记过的路径必须原样透传（返回 None）
        assert!(redirect_region_with_root(Some(&root), "assets/art/props/well.png").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 批 D 的等价性门：图集 cook 之后，工程源里写的**原图路径**必须解析到
    /// `图集#区域`，且区域尺寸等于原图尺寸——装配期那一层看不出"图被换掉了"。
    #[test]
    fn redirect_maps_original_path_to_atlas_region() {
        use rustlua_infra::atlas_redirect::{AtlasRedirect, REDIRECT_FILE, RedirectEntry};

        let root = std::env::temp_dir().join(format!(
            "rustlua-atlas-redirect-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/atlases")).unwrap();

        // 页：16x16 的 .rltex（4 个 8x8 区域足够放下素材），区域 `hud_a` 在 (0,0) 8x8。
        let pixels = vec![0u8; 16 * 16 * 4];
        let page = rustlua_assets::rltex::encode(16, 16, &pixels).unwrap();
        std::fs::write(root.join("assets/atlases/ui-0.rltex"), &page).unwrap();
        std::fs::write(
            root.join("assets/atlases/ui-0.atlas"),
            "atlas_version=1\ntexture=assets/atlases/ui-0.rltex\nregion\thud_a\t0\t0\t8\t8\n",
        )
        .unwrap();

        let mut redirect = AtlasRedirect::empty();
        redirect.entries.insert(
            "assets/art/ui/hud/a.png".to_owned(),
            RedirectEntry {
                atlas: "assets/atlases/ui-0.atlas".to_owned(),
                region: "hud_a".to_owned(),
            },
        );
        std::fs::write(root.join(REDIRECT_FILE), redirect_json(&redirect)).unwrap();

        // ① 原图路径不再存在，但通过重定向解析出图集 + 区域
        assert!(!root.join("assets/art/ui/hud/a.png").exists());
        let (texture, region) = resolve_atlas_reference(Some(&root), "assets/art/ui/hud/a.png")
            .unwrap()
            .expect("重定向必须命中");
        assert_eq!(texture, "assets/atlases/ui-0.rltex");
        assert_eq!(region.pixel_size, [8, 8], "区域尺寸必须等于原图尺寸");
        assert_eq!(region.uv_min, Vec2::new(0.0, 0.0));
        assert_eq!(region.uv_max, Vec2::new(0.5, 0.5));

        // ② 没有重定向的普通贴图仍然原样透传（不会把整棵树都当图集）
        assert!(
            resolve_atlas_reference(Some(&root), "assets/art/props/well.png")
                .unwrap()
                .is_none()
        );

        // ③ 页能被纹理加载器当纹理解码（`.rltex` → RGBA8）
        let texture_asset = crate::texture::TextureAsset::load_png(
            root.join("assets/atlases/ui-0.rltex"),
            crate::texture::TextureColorSpace::Srgb,
        )
        .expect("纹理加载器必须认 .rltex");
        assert_eq!((texture_asset.width(), texture_asset.height()), (16, 16));

        let _ = std::fs::remove_dir_all(&root);
    }
}
