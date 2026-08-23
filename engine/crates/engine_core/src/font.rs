//! On-demand glyph atlas for ASCII + CJK, rasterized from a TTF font with
//! `ab_glyph` into **signed-distance-field** cells (R channel = normalized
//! signed distance, edge = 0.5). The host shader samples R and applies a
//! `smoothstep` so glyph edges stay crisp at any scale (TMP-style).

use ab_glyph::{Font, FontArc};
use std::collections::HashMap;

/// Rasterize scale in device pixels; each glyph is stored in a `cell` square.
pub const RASTER_SCALE: f32 = 64.0;
/// Atlas grid: `COLS * ROWS` cells (256 slots is enough for menus/HUD).
const COLS: u32 = 16;
const ROWS: u32 = 16;
/// Signed-distance band around each glyph outline (px). The normalized SDF
/// maps `±PAD` texels to `[0,1]`, so `smoothstep(0.5-w,0.5+w,d)` stays inside
/// the linear region for ~1px falloff at any practical scale.
const PAD: i32 = 8;
/// Non-feature cost for the distance transform.
const BIG: f32 = 1e30;

/// Per-glyph metrics used by text layout: UV rect + cell-space geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphMetrics {
    /// Normalized UV rect `[u_min, v_min, u_max, v_max]` of the whole cell.
    pub uv: [f32; 4],
    /// Glyph rect inside the cell in texels `[left, top, right, bottom]`.
    pub cell_rect: [f32; 4],
    /// Glyph pixel bounds `[min.x, min.y, max.x, max.y]` relative to the pen
    /// (y-down; `min.y` negative above the baseline).
    pub bounds: [f32; 4],
    /// Horizontal advance in px at `RASTER_SCALE`.
    pub advance: f32,
}

/// A growable-by-slot glyph atlas producing one RGBA8 texture.
#[derive(Clone, Debug)]
pub struct FontAtlas {
    font: FontArc,
    cell: u32,
    pixels: Vec<u8>,
    used: u32,
    cached: HashMap<char, GlyphMetrics>,
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

    /// Cell size in texels (glyph + padding).
    pub const fn cell(&self) -> u32 {
        self.cell
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

    /// Ascent in em (fraction of the em box), for baseline layout.
    pub fn ascent_em(&self) -> f32 {
        self.font.ascent_unscaled() / self.font.units_per_em().unwrap_or(1000.0)
    }

    /// Descent in em (negative), for baseline layout.
    pub fn descent_em(&self) -> f32 {
        self.font.descent_unscaled() / self.font.units_per_em().unwrap_or(1000.0)
    }

    /// Returns the normalized UV rect for a glyph (back-compat shim).
    pub fn uv(&mut self, character: char) -> [f32; 4] {
        self.glyph_metrics(character).uv
    }

    /// Horizontal advance in px at `RASTER_SCALE` without rasterizing the glyph
    /// (used for whitespace, which has no outline / SDF cell).
    pub fn advance(&self, character: char) -> f32 {
        self.font.h_advance_unscaled(self.font.glyph_id(character))
            / self.font.units_per_em().unwrap_or(1000.0)
            * RASTER_SCALE
    }

    /// Returns per-glyph metrics, rasterizing the SDF cell on first use.
    pub fn glyph_metrics(&mut self, character: char) -> GlyphMetrics {
        if let Some(metrics) = self.cached.get(&character) {
            return *metrics;
        }
        let slot = self.used;
        if slot >= COLS * ROWS {
            tracing::warn!(
                target: "text",
                character = character.to_string(),
                "字形图集已满，回退到占位"
            );
            return GlyphMetrics {
                uv: [0.0; 4],
                cell_rect: [0.0; 4],
                bounds: [0.0; 4],
                advance: self.cell as f32,
            };
        }
        let col = slot % COLS;
        let row = slot / COLS;
        let cell_x = col * self.cell;
        let cell_y = row * self.cell;
        let u_min = cell_x as f32 / self.width() as f32;
        let v_min = cell_y as f32 / self.height() as f32;
        let u_max = (cell_x + self.cell) as f32 / self.width() as f32;
        let v_max = (cell_y + self.cell) as f32 / self.height() as f32;
        let uv = [u_min, v_min, u_max, v_max];

        let glyph = self.font.glyph_id(character).with_scale(RASTER_SCALE);
        let (sdf_cell, cell_rect, bounds) = if let Some(outlined) = self.font.outline_glyph(glyph)
        {
            rasterize_sdf(&outlined, self.cell)
        } else {
            placeholder_sdf(self.cell)
        };
        let metrics = GlyphMetrics {
            uv,
            cell_rect,
            bounds,
            advance: self.font.h_advance_unscaled(self.font.glyph_id(character))
                / self.font.units_per_em().unwrap_or(1000.0)
                * RASTER_SCALE,
        };

        for dy in 0..self.cell {
            for dx in 0..self.cell {
                let value = sdf_cell[(dy * self.cell + dx) as usize];
                let offset = (((cell_y + dy) * self.width() + cell_x + dx) as usize) * 4;
                self.pixels[offset..offset + 4].copy_from_slice(&[value, 0, 0, 255]);
            }
        }
        self.used += 1;
        self.dirty = true;
        self.version = self.version.wrapping_add(1);
        self.cached.insert(character, metrics);
        metrics
    }
}

/// Rasterizes a glyph into a cell-sized SDF (R values `0..=255`).
/// Returns `(sdf_cell, cell_rect, bounds)` where `cell_rect` is the glyph's
/// `[left, top, right, bottom]` in cell texels.
fn rasterize_sdf(outlined: &ab_glyph::OutlinedGlyph, cell: u32) -> (Vec<u8>, [f32; 4], [f32; 4]) {
    let cell_i = cell as i32;
    let bounds = outlined.px_bounds();
    // 笔原点（把字形设备包围盒居中到 cell 内）。
    let ox = (cell_i - (bounds.min.x as i32 + bounds.max.x as i32)) / 2;
    let oy = (cell_i - (bounds.min.y as i32 + bounds.max.y as i32)) / 2;
    let left = ox + bounds.min.x as i32;
    let top = oy + bounds.min.y as i32;
    let right = ox + bounds.max.x as i32;
    let bottom = oy + bounds.max.y as i32;
    let mut mask = vec![false; (cell * cell) as usize];
    outlined.draw(|x, y, coverage| {
        // draw 的 x/y 是 0 基（相对 bounds.min 的栅格坐标）。
        let xi = left + x as i32;
        let yi = top + y as i32;
        if xi >= 0 && yi >= 0 && xi < cell_i && yi < cell_i && coverage > 0.5 {
            mask[yi as usize * cell as usize + xi as usize] = true;
        }
    });
    let sdf = sdf_from_mask(&mask, cell);
    (
        sdf,
        [left as f32, top as f32, right as f32, bottom as f32],
        [bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y],
    )
}

/// 缺失字形：画一个占位方框（边框为实心），同样走 SDF 抗锯齿。
fn placeholder_sdf(cell: u32) -> (Vec<u8>, [f32; 4], [f32; 4]) {
    let cell_i = cell as i32;
    let inset = cell_i / 5;
    let mut mask = vec![false; (cell * cell) as usize];
    for y in 0..cell_i {
        for x in 0..cell_i {
            let is_edge = (x >= inset && x < cell_i - inset && (y == inset || y == cell_i - inset - 1))
                || (y >= inset && y < cell_i - inset && (x == inset || x == cell_i - inset - 1));
            if is_edge {
                mask[y as usize * cell as usize + x as usize] = true;
            }
        }
    }
    let sdf = sdf_from_mask(&mask, cell);
    (
        sdf,
        [inset as f32, inset as f32, (cell_i - inset) as f32, (cell_i - inset) as f32],
        [inset as f32, inset as f32, (cell_i - inset) as f32, (cell_i - inset) as f32],
    )
}

/// 二值掩码 → 有向距离场（R 值 `0..=255`，边缘归一化为 128）。
fn sdf_from_mask(mask: &[bool], cell: u32) -> Vec<u8> {
    let w = cell as usize;
    let inside = edt_2d(mask, w, w);
    let inv: Vec<bool> = mask.iter().map(|value| !value).collect();
    let outside = edt_2d(&inv, w, w);
    let spread = PAD as f32;
    mask.iter()
        .enumerate()
        .map(|(i, _)| {
            // 有向距离：字形内部为正（inside 距离小、outside 距离大）。
            let d = outside[i].sqrt() - inside[i].sqrt();
            let norm = (0.5 + d / (2.0 * spread)).clamp(0.0, 1.0);
            (norm * 255.0) as u8
        })
        .collect()
}

/// 精确 L2 距离变换：1D 下包络（Felzenszwalb–Huttenlocher），O(n)。
fn edt_1d(f: &mut [f32]) {
    let n = f.len();
    let mut v = vec![0usize; n];
    let mut z = vec![f32::NEG_INFINITY; n + 1];
    z[1] = f32::INFINITY;
    let mut k = 0usize;
    v[0] = 0;
    for q in 1..n {
        let mut s;
        loop {
            let p = v[k];
            s = ((f[q] + q as f32 * q as f32) - (f[p] + p as f32 * p as f32))
                / (2.0 * q as f32 - 2.0 * p as f32);
            if s > z[k] {
                break;
            }
            k -= 1;
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let d = q as f32 - v[k] as f32;
        f[q] = d * d + f[v[k]];
    }
}

/// 二值掩码 → 精确平方 L2 距离（列趟 + 行趟）。
fn edt_2d(mask: &[bool], width: usize, height: usize) -> Vec<f32> {
    let mut dist = vec![BIG; width * height];
    let mut col = vec![0.0; height];
    for x in 0..width {
        for y in 0..height {
            col[y] = if mask[y * width + x] { 0.0 } else { BIG };
        }
        edt_1d(&mut col);
        for y in 0..height {
            dist[y * width + x] = col[y];
        }
    }
    let mut row = vec![0.0; width];
    for y in 0..height {
        row.copy_from_slice(&dist[y * width..y * width + width]);
        edt_1d(&mut row);
        dist[y * width..y * width + width].copy_from_slice(&row);
    }
    dist
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_font() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/editor/fonts/NotoSansSC-VF.ttf");
        std::fs::read(&path).unwrap_or_default()
    }

    #[test]
    fn rasterizes_sdf_glyph_and_reports_metrics() {
        let bytes = test_font();
        if bytes.is_empty() {
            eprintln!("未找到测试字体，跳过");
            return;
        }
        let mut atlas = FontAtlas::load(&bytes).unwrap();
        let metrics = atlas.glyph_metrics('俯');
        assert_ne!(metrics.uv, [0.0; 4], "glyph uv is non-zero");
        assert!(metrics.advance > 0.0, "advance is positive");
        assert!(metrics.bounds[0].is_finite() && metrics.bounds[2] > metrics.bounds[0]);
        assert!(atlas.dirty(), "new glyph marks atlas dirty");
        assert_eq!(atlas.version(), 1);
        let metrics2 = atlas.glyph_metrics('俯');
        assert_eq!(metrics, metrics2, "same glyph is cached");
        // SDF 符号验证：单元角（远离字形）应为外部 R<128；存在内部像素（R>140）。
        let (cell_x, cell_y) = (0u32, 0u32); // 第一个字形在 (0,0) 单元。
        let corner = atlas.pixels()[((cell_y * atlas.width() + cell_x) as usize) * 4];
        assert!(corner < 128, "cell corner should be outside glyph, got {corner}");
        let mut inside = 0;
        for dy in 0..atlas.cell {
            for dx in 0..atlas.cell {
                let offset = (((cell_y + dy) * atlas.width() + cell_x + dx) as usize) * 4;
                if atlas.pixels()[offset] > 140 {
                    inside += 1;
                }
            }
        }
        assert!(inside > 0, "glyph stroke must be inside (R>128)");
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
