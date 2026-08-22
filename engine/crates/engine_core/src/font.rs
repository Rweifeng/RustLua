//! On-demand glyph atlas for CJK / arbitrary glyphs, rasterized from a TTF font
//! with `ab_glyph`. ASCII glyphs keep using the built-in 5x7 bitmap atlas; this
//! atlas fills in every other character (Chinese, etc.) as square cells.

use ab_glyph::{Font, FontArc};
use std::collections::HashMap;

/// Rasterize scale in device pixels; each glyph is stored in a `cell` square.
const RASTER_SCALE: f32 = 28.0;
/// Atlas grid: `COLS * ROWS` cells (256 slots is enough for menus/HUD).
const COLS: u32 = 16;
const ROWS: u32 = 16;
/// Padding around each glyph inside its cell.
const PAD: i32 = 2;

/// A growable-by-slot glyph atlas producing one RGBA8 texture.
#[derive(Clone, Debug)]
pub struct FontAtlas {
    font: FontArc,
    cell: u32,
    pixels: Vec<u8>,
    used: u32,
    cached: HashMap<char, [f32; 4]>,
    version: u64,
    dirty: bool,
}

impl FontAtlas {
    /// Builds an atlas from TTF font bytes.
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        // FontArc::try_from_slice 要求 'static；泄漏一份字节到进程生命周期。
        let owned: &'static [u8] = Box::leak(bytes.to_vec().into_boxed_slice());
        let font =
            FontArc::try_from_slice(owned).map_err(|error| format!("字体解析失败: {error}"))?;
        Ok(Self::new(font))
    }

    fn new(font: FontArc) -> Self {
        let cell = RASTER_SCALE.ceil() as u32 + PAD as u32 * 2;
        let width = COLS * cell;
        let height = ROWS * cell;
        Self {
            font,
            cell,
            pixels: vec![0; (width * height * 4) as usize],
            used: 0,
            cached: HashMap::new(),
            version: 0,
            dirty: false,
        }
    }

    pub const fn width(&self) -> u32 {
        COLS * self.cell
    }

    pub const fn height(&self) -> u32 {
        ROWS * self.cell
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Monotonic revision; bump when a new glyph is rasterized.
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// True when pixels changed since the last upload.
    pub const fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_uploaded(&mut self) {
        self.dirty = false;
    }

    /// Returns the normalized UV rect `[u_min, v_min, u_max, v_max]` for a
    /// glyph, rasterizing it on first use.
    pub fn uv(&mut self, character: char) -> [f32; 4] {
        if let Some(uv) = self.cached.get(&character) {
            return *uv;
        }
        let slot = self.used;
        if slot >= COLS * ROWS {
            tracing::warn!(
                target: "text",
                character = character.to_string(),
                "字形图集已满，回退到占位"
            );
            return [0.0; 4];
        }
        let col = slot % COLS;
        let row = slot / COLS;
        let origin_x = (col * self.cell) as i32 + PAD;
        let origin_y = (row * self.cell) as i32 + PAD;
        let glyph = self.font.glyph_id(character).with_scale(RASTER_SCALE);
        if let Some(outlined) = self.font.outline_glyph(glyph) {
            // draw 回调的 x/y 是相对字形包围盒左上角的 0 基位图坐标，
            // 直接以单元格原点 + PAD 放置（不可再减 bounds.min，否则 CJK
            // 大负值 min.y 会把字形推出单元格）。
            outlined.draw(|x, y, coverage| {
                // x/y 是字形包围盒内 0 基坐标；按单元内偏移裁剪，
                // 不能用绝对像素 px/py 与 cell 比（非首列 origin_x 会超过 cell）。
                if x >= self.cell || y >= self.cell {
                    return;
                }
                let px = (origin_x + x as i32) as u32;
                let py = (origin_y + y as i32) as u32;
                // px/py 已含行列单元偏移（origin_x/origin_y），offset 用绝对像素，
                // 勿再重复加 row*cell / col*cell。
                let offset = (py * self.width() + px) as usize;
                let alpha = (coverage.clamp(0.0, 1.0) * 255.0) as u8;
                self.pixels[offset * 4..offset * 4 + 4].copy_from_slice(&[255, 255, 255, alpha]);
            });
        } else {
            // 缺失字形：画一个占位方框。
            let row = slot / COLS;
            let col = slot % COLS;
            for dy in 0..self.cell {
                for dx in 0..self.cell {
                    let is_edge = dx == 0 || dy == 0 || dx + 1 == self.cell || dy + 1 == self.cell;
                    if is_edge {
                        let offset =
                            ((row * self.cell + dy) * self.width() + col * self.cell + dx) as usize;
                        self.pixels[offset * 4..offset * 4 + 4]
                            .copy_from_slice(&[200, 200, 200, 255]);
                    }
                }
            }
        }
        self.used += 1;
        self.dirty = true;
        self.version = self.version.wrapping_add(1);
        let u_min = (col * self.cell) as f32 / self.width() as f32;
        let v_min = (row * self.cell) as f32 / self.height() as f32;
        let u_max = ((col + 1) * self.cell) as f32 / self.width() as f32;
        let v_max = ((row + 1) * self.cell) as f32 / self.height() as f32;
        let uv = [u_min, v_min, u_max, v_max];
        self.cached.insert(character, uv);
        uv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_font() -> Vec<u8> {
        // 用内置最小测试：若找不到字体文件则返回空（跳过）。
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/editor/fonts/NotoSansSC-VF.ttf");
        std::fs::read(&path).unwrap_or_default()
    }

    #[test]
    fn rasterizes_cjk_glyph_and_reports_uv() {
        let bytes = test_font();
        if bytes.is_empty() {
            eprintln!("未找到测试字体，跳过");
            return;
        }
        let mut atlas = FontAtlas::load(&bytes).unwrap();
        let uv = atlas.uv('俯');
        assert_ne!(uv, [0.0; 4], "glyph uv is non-zero");
        assert!(atlas.dirty(), "new glyph marks atlas dirty");
        assert_eq!(atlas.version(), 1);
        let uv2 = atlas.uv('俯');
        assert_eq!(uv, uv2, "same glyph is cached");
        let opaque = atlas
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] > 0)
            .count();
        assert!(
            opaque > 100,
            "glyph must rasterize substantial opaque pixels (offset/clip bug caught), got {opaque}"
        );
    }

    #[test]
    fn atlas_dimensions_are_square_cells() {
        let bytes = test_font();
        if bytes.is_empty() {
            return;
        }
        let atlas = FontAtlas::load(&bytes).unwrap();
        assert_eq!(atlas.width() % 16, 0);
        assert_eq!(atlas.height(), atlas.width());
    }
}
