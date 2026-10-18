//! 图集打包器（批 D，`docs/63`）：多张散图 → 一张图集 PNG + `.atlas` 描述。
//!
//! 装箱用 **shelf**（按高度降序、同高按宽度降序、再按名字字典序，完全确定性），
//! 每格带 `padding` 防漏色；超过 `max_size` 换页。POT 取整默认开启。
//! 先不做 MaxRects：2D 精灵项目的碎片率还没到需要更复杂算法的程度，
//! 而确定性是 golden 门的硬要求，shelf 的排布规则最容易钉死。

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::atlas_format::{AtlasDefinition, AtlasRect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasPackerOptions {
    /// 相邻区域之间留出的透明像素边距。
    pub padding: u32,
    /// 单页最大边长，超限换页。
    pub max_size: u32,
    /// 页尺寸向上取整到 2 的幂。
    pub pot: bool,
}

impl Default for AtlasPackerOptions {
    fn default() -> Self {
        Self {
            padding: 2,
            max_size: 2048,
            pot: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AtlasPackError {
    #[error("{name}: 读取源图失败: {reason}")]
    SourceIo { name: String, reason: String },
    #[error("{name}: 源图解码失败: {reason}")]
    SourceDecode { name: String, reason: String },
    #[error("{name}: 源图 {width}x{height} 超过单页上限 {max_size}")]
    ItemTooLarge {
        name: String,
        width: u32,
        height: u32,
        max_size: u32,
    },
    #[error("没有可打包的源图")]
    EmptyInput,
    #[error("{path}: 写出失败: {reason}")]
    WriteIo { path: String, reason: String },
}

/// 一页装箱结果：区域名 → 页内像素矩形。
#[derive(Clone, Debug, PartialEq)]
pub struct PackedPage {
    pub width: u32,
    pub height: u32,
    /// 与名字字典序一致（BTreeMap 保证 `.atlas` 写出确定性）。
    pub regions: BTreeMap<String, AtlasRect>,
    /// 打包顺序（shelf 放置序），合成时按此序逐张拷贝像素。
    pub placements: Vec<(String, AtlasRect)>,
}

impl PackedPage {
    fn blank() -> Self {
        Self {
            width: 0,
            height: 0,
            regions: BTreeMap::new(),
            placements: Vec::new(),
        }
    }
}

/// 一个打包输入项：区域名 + 源 PNG 文件。
#[derive(Clone, Debug)]
pub struct PackSource {
    pub name: String,
    pub path: PathBuf,
}

fn next_pot(value: u32) -> u32 {
    value.max(1).next_power_of_two()
}

/// 只读一张图（PNG）的像素尺寸，不解码整图。给"按面积决定是否单独成集"这类策略用。
pub fn source_dimensions(path: &std::path::Path) -> Result<(u32, u32), AtlasPackError> {
    image::image_dimensions(path).map_err(|error| AtlasPackError::SourceIo {
        name: path.display().to_string(),
        reason: error.to_string(),
    })
}

/// 只读源图头尺寸（不解码像素）做装箱，装箱与合成两次读盘保持同一尺寸事实。
pub fn pack_atlas(
    sources: &[PackSource],
    options: &AtlasPackerOptions,
) -> Result<Vec<PackedPage>, AtlasPackError> {
    if sources.is_empty() {
        return Err(AtlasPackError::EmptyInput);
    }
    let mut measured: Vec<(String, u32, u32)> = Vec::with_capacity(sources.len());
    for source in sources {
        let (width, height) =
            image::image_dimensions(&source.path).map_err(|error| AtlasPackError::SourceIo {
                name: source.name.clone(),
                reason: error.to_string(),
            })?;
        let inner = options
            .max_size
            .checked_sub(options.padding * 2)
            .ok_or_else(|| AtlasPackError::ItemTooLarge {
                name: source.name.clone(),
                width,
                height,
                max_size: options.max_size,
            })?;
        if width == 0 || height == 0 || width > inner || height > inner {
            return Err(AtlasPackError::ItemTooLarge {
                name: source.name.clone(),
                width,
                height,
                max_size: options.max_size,
            });
        }
        measured.push((source.name.clone(), width, height));
    }
    // 确定性排序：高→宽→名字字典序。
    measured.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));

    // 单图必然放得进空页（前面已按 inner 校验），循环不会卡死。
    let mut pages: Vec<PackedPage> = Vec::new();
    let mut page = PackedPage::blank();
    let mut cursor_x = options.padding;
    let mut shelf_y = options.padding;
    let mut shelf_height = 0u32;
    for (name, width, height) in measured {
        if cursor_x + width + options.padding > options.max_size && shelf_height > 0 {
            // 当前行放不下：起新 shelf。
            shelf_y += shelf_height + options.padding;
            cursor_x = options.padding;
            shelf_height = 0;
        }
        if shelf_y + height.max(shelf_height) + options.padding > options.max_size {
            // 页高耗尽：封板换页。
            finish_page(&mut page, options);
            pages.push(std::mem::replace(&mut page, PackedPage::blank()));
            cursor_x = options.padding;
            shelf_y = options.padding;
            shelf_height = 0;
        }
        let rect = AtlasRect {
            x: cursor_x,
            y: shelf_y,
            width,
            height,
        };
        page.regions.insert(name.clone(), rect);
        page.placements.push((name, rect));
        cursor_x += width + options.padding;
        shelf_height = shelf_height.max(height);
    }
    finish_page(&mut page, options);
    pages.push(page);
    Ok(pages)
}

fn finish_page(page: &mut PackedPage, options: &AtlasPackerOptions) {
    let used_width = page
        .placements
        .iter()
        .map(|(_, rect)| rect.x + rect.width + options.padding)
        .max()
        .unwrap_or(options.padding * 2);
    let used_height = page
        .placements
        .iter()
        .map(|(_, rect)| rect.y + rect.height + options.padding)
        .max()
        .unwrap_or(options.padding * 2);
    page.width = if options.pot {
        next_pot(used_width)
    } else {
        used_width
    };
    page.height = if options.pot {
        next_pot(used_height)
    } else {
        used_height
    };
}

/// 合成一页：按装箱序把 RGBA 拷进透明画布，返回 `(width, height, RGBA8)`。
///
/// 抽出来是为了让 `.rltex`（裸像素，`docs/67` §6.3）与 PNG 两条输出共用同一份合成逻辑：
/// 页的**像素**必须只有一个来源，否则"PNG 版与 .rltex 版内容不同"这类问题会永远查不完。
pub fn compose_page_rgba(
    page: &PackedPage,
    sources: &[PackSource],
) -> Result<Vec<u8>, AtlasPackError> {
    let lookup: BTreeMap<&str, &Path> = sources
        .iter()
        .map(|source| (source.name.as_str(), source.path.as_ref()))
        .collect();
    let mut canvas =
        image::RgbaImage::from_pixel(page.width, page.height, image::Rgba([0, 0, 0, 0]));
    for (name, rect) in &page.placements {
        let path = lookup
            .get(name.as_str())
            .ok_or_else(|| AtlasPackError::SourceIo {
                name: name.clone(),
                reason: "装箱引用了不存在的源".to_owned(),
            })?;
        let decoded = image::open(path)
            .map_err(|error| AtlasPackError::SourceDecode {
                name: name.clone(),
                reason: error.to_string(),
            })?
            .to_rgba8();
        for y in 0..rect.height {
            for x in 0..rect.width {
                canvas.put_pixel(rect.x + x, rect.y + y, *decoded.get_pixel(x, y));
            }
        }
    }
    Ok(canvas.into_raw())
}

/// 合成一页：按装箱序把 RGBA 拷进透明画布，写出 `png_path` 与同名 `.atlas`。
pub fn compose_page(
    page: &PackedPage,
    sources: &[PackSource],
    png_path: &Path,
    atlas_path: &Path,
) -> Result<(), AtlasPackError> {
    let pixels = compose_page_rgba(page, sources)?;
    let canvas = image::RgbaImage::from_raw(page.width, page.height, pixels).ok_or_else(|| {
        AtlasPackError::WriteIo {
            path: png_path.display().to_string(),
            reason: "合成画布尺寸与像素长度不符".to_owned(),
        }
    })?;
    canvas
        .save(png_path)
        .map_err(|error| AtlasPackError::WriteIo {
            path: png_path.display().to_string(),
            reason: error.to_string(),
        })?;
    write_atlas_definition(page, png_path, atlas_path)
}

/// 写 `.atlas` 定义（`texture` 用 `/` 分隔，与 `docs/11` 的约定一致）。
pub fn write_atlas_definition(
    page: &PackedPage,
    texture_path: &Path,
    atlas_path: &Path,
) -> Result<(), AtlasPackError> {
    let definition = AtlasDefinition {
        texture: texture_path.display().to_string().replace('\\', "/"),
        regions: page.regions.clone(),
    };
    fs::write(atlas_path, definition.to_text()).map_err(|error| AtlasPackError::WriteIo {
        path: atlas_path.display().to_string(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-atlas-pack-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_source(
        root: &Path,
        name: &str,
        width: u32,
        height: u32,
        color: [u8; 4],
    ) -> PackSource {
        let path = root.join(format!("{name}.png"));
        let mut img = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        // 左上角打一个异色像素，合成后可验证"拷的是这张图"。
        img.put_pixel(0, 0, image::Rgba([9, 9, 9, 255]));
        img.save(&path).unwrap();
        PackSource {
            name: name.to_owned(),
            path,
        }
    }

    #[test]
    fn shelf_layout_is_deterministic_golden() {
        let root = temp_root("golden");
        let sources = [
            write_source(&root, "b", 30, 40, [255, 0, 0, 255]),
            write_source(&root, "a", 40, 40, [0, 255, 0, 255]),
            write_source(&root, "c", 20, 20, [0, 0, 255, 255]),
            write_source(&root, "d", 20, 20, [255, 255, 0, 255]),
        ];
        let options = AtlasPackerOptions {
            padding: 2,
            max_size: 256,
            pot: true,
        };
        let pages = pack_atlas(&sources, &options).unwrap();
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert_eq!(page.width, 128, "POT 取整");
        assert_eq!(page.height, 64);
        // 金样：高 40 的两个先排（a 宽 40 在前，再 b 宽 30），c/d 仍在同一 shelf 续排。
        let placements: Vec<(&str, AtlasRect)> = page
            .placements
            .iter()
            .map(|(name, rect)| (name.as_str(), *rect))
            .collect();
        assert_eq!(
            placements,
            vec![
                (
                    "a",
                    AtlasRect {
                        x: 2,
                        y: 2,
                        width: 40,
                        height: 40
                    }
                ),
                (
                    "b",
                    AtlasRect {
                        x: 44,
                        y: 2,
                        width: 30,
                        height: 40
                    }
                ),
                (
                    "c",
                    AtlasRect {
                        x: 76,
                        y: 2,
                        width: 20,
                        height: 20
                    }
                ),
                (
                    "d",
                    AtlasRect {
                        x: 98,
                        y: 2,
                        width: 20,
                        height: 20
                    }
                ),
            ],
            "shelf 排布必须逐值稳定（golden 门）"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_pages_split_and_overflow_item_fails() {
        let root = temp_root("split");
        let big = write_source(&root, "big", 100, 100, [1, 2, 3, 255]);
        let wide = write_source(&root, "wide", 100, 100, [4, 5, 6, 255]);
        let options = AtlasPackerOptions {
            padding: 2,
            max_size: 128,
            pot: false,
        };
        let pages = pack_atlas(&[big.clone(), wide.clone()], &options).unwrap();
        assert_eq!(pages.len(), 2, "一页放不下的两张必须分页");
        assert_eq!(pages[0].regions.len(), 1);
        assert_eq!(pages[1].regions.len(), 1);

        let huge = write_source(&root, "huge", 200, 200, [7, 8, 9, 255]);
        let error = pack_atlas(&[huge], &options).unwrap_err();
        assert!(matches!(error, AtlasPackError::ItemTooLarge { .. }));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn compose_roundtrips_through_the_loader_pipeline() {
        let root = temp_root("compose");
        let sources = [
            write_source(&root, "hero", 8, 4, [10, 20, 30, 255]),
            write_source(&root, "npc", 4, 4, [40, 50, 60, 255]),
        ];
        let options = AtlasPackerOptions {
            padding: 1,
            max_size: 64,
            pot: false,
        };
        let pages = pack_atlas(&sources, &options).unwrap();
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        let png = root.join("sheet.png");
        let atlas = root.join("sheet.atlas");
        compose_page(page, &sources, &png, &atlas).unwrap();

        // 打包产物必须能被批 A 的解析器与批 B 的加载器原样读回（端到端一致性门）。
        let definition = AtlasDefinition::load(&atlas).expect("打包产物 .atlas 必须可解析");
        assert_eq!(
            definition.texture,
            png.display().to_string().replace('\\', "/"),
            "texture 字段一律正斜杠"
        );
        let (width, height) = image::image_dimensions(&png).unwrap();
        assert_eq!(
            (width, height),
            (page.width, page.height),
            "页尺寸即纹理尺寸"
        );
        for name in page.regions.keys() {
            assert!(
                definition.regions.contains_key(name),
                "区域 {name} 必须写进 .atlas"
            );
        }
        for rect in definition.regions.values() {
            assert!(
                rect.x + rect.width <= width && rect.y + rect.height <= height,
                "区域不能越出页边界"
            );
        }
        // hero 与 npc 的左上标记像素都应在页内原位。
        let sheet = image::open(&png).unwrap().to_rgba8();
        for (_, rect) in &page.placements {
            assert_eq!(
                sheet.get_pixel(rect.x, rect.y),
                &image::Rgba([9, 9, 9, 255]),
                "装箱位置必须与实际像素一致"
            );
        }
        // padding=1 时 (0,0) 属于边距，必须是透明的。
        assert_eq!(sheet.get_pixel(0, 0), &image::Rgba([0, 0, 0, 0]));
        let _ = fs::remove_dir_all(&root);
    }
}
