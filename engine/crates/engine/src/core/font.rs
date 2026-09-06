//! On-demand glyph atlas for ASCII + CJK, rasterized from a TTF font with
//! `ab_glyph` outlines into **multi-channel signed-distance-field** cells
//! (RGB = MSDF 三通道，A = 255；边缘处 RGB 的 median ≈ 0.5)。宿主 shader 对
//! RGB 取 median 再经 fwidth 解码，直角/笔画末梢保持锐利、任意缩放不失真
//! （TextMeshPro 同源方案）。

use ab_glyph::{Font, FontArc, Outline, OutlineCurve, Point};
use bymsdfgen_core::{
    Bitmap, DistanceMapping, MsdfGeneratorConfig, Projection, Range, SdfTransformation,
    coloring::edge_coloring_simple,
    generate_msdf,
    geometry::{Contour, EdgeSegment, Shape},
    math::Vector2,
};
use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
};

/// Rasterize scale in device pixels（px/em）。96px/em 在 4096² 打包图集内可容纳
/// ~1300 个 CJK 字形，锐利度约为旧 64px/em 的 1.5×；如需更锐可上调，但 CJK
/// 容量会相应下降（128px/em 约 ~900）。
pub const RASTER_SCALE: f32 = 96.0;
/// 打包图集初始边长（正方形）。
const INITIAL_ATLAS_SIZE: u32 = 1024;
/// 打包图集最大边长（常见 GPU 纹理上限内）。
const MAX_ATLAS_SIZE: u32 = 4096;
/// Signed-distance band around each glyph outline (px)。MSDF 映射 `±PAD` texels
/// 到 `[0,1]`（边缘 median=0.5），shader 的 fwidth 解码在任意字号下都保持
/// ~1px 渐变。
const PAD: i32 = 8;

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

/// Errors returned while rasterizing or growing a glyph atlas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontAtlasError {
    /// The atlas reached its configured maximum capacity.
    AtlasFull {
        character: char,
        width: u32,
        height: u32,
        capacity: u32,
    },
}

impl fmt::Display for FontAtlasError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AtlasFull {
                character,
                width,
                height,
                capacity,
            } => write!(
                formatter,
                "字体图集已满：无法加入字形 {character:?}（{width}×{height}，容量 {capacity}）"
            ),
        }
    }
}

impl Error for FontAtlasError {}

/// A growable packed glyph atlas producing one RGBA8 texture.
#[derive(Clone, Debug)]
pub struct FontAtlas {
    font: FontArc,
    /// 后备字体：主字体缺的字形（glyph_id == 0）用它动态生成补充。
    fallback: Option<FontArc>,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    used: u32,
    cached: HashMap<char, GlyphMetrics>,
    packer: Packer,
    version: u64,
    dirty: bool,
}

impl FontAtlas {
    /// Builds an atlas from TTF font bytes（无后备字体）。
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        Self::load_with_fallback(bytes, None)
    }

    /// 用主字体 + 可选后备字体构建图集。后备字体用于主字体缺失的字形。
    pub fn load_with_fallback(primary: &[u8], fallback: Option<&[u8]>) -> Result<Self, String> {
        let font = FontArc::try_from_vec(primary.to_vec())
            .map_err(|error| format!("字体解析失败: {error}"))?;
        let fallback = fallback
            .map(|bytes| FontArc::try_from_vec(bytes.to_vec()))
            .transpose()
            .map_err(|error| format!("后备字体解析失败: {error}"))?;
        Ok(Self::new(font, fallback))
    }

    fn new(font: FontArc, fallback: Option<FontArc>) -> Self {
        let size = INITIAL_ATLAS_SIZE;
        Self {
            font,
            fallback,
            width: size,
            height: size,
            pixels: vec![0; (size * size * 4) as usize],
            used: 0,
            cached: HashMap::new(),
            packer: Packer::new(size, size),
            version: 0,
            dirty: false,
        }
    }

    /// 字形所属字体：主字体有该字用主字体，否则用后备字体，再否则主字体（.notdef）。
    fn font_for(&self, character: char) -> (FontArc, ab_glyph::GlyphId) {
        let primary_id = self.font.glyph_id(character);
        if primary_id.0 != 0 {
            return (self.font.clone(), primary_id);
        }
        if let Some(fallback) = &self.fallback {
            let fallback_id = fallback.glyph_id(character);
            if fallback_id.0 != 0 {
                return (fallback.clone(), fallback_id);
            }
        }
        (self.font.clone(), primary_id)
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
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

    /// Horizontal advance in px at `RASTER_SCALE` without rasterizing the glyph
    /// (used for whitespace, which has no outline / SDF cell).
    pub fn advance(&self, character: char) -> f32 {
        let (font, glyph_id) = self.font_for(character);
        font.h_advance_unscaled(glyph_id) / font.units_per_em().unwrap_or(1000.0) * RASTER_SCALE
    }

    fn grow(&mut self) -> bool {
        if self.width >= MAX_ATLAS_SIZE && self.height >= MAX_ATLAS_SIZE {
            return false;
        }
        let old_width = self.width;
        let old_height = self.height;
        let new_width = (self.width * 2).min(MAX_ATLAS_SIZE);
        let new_height = (self.height * 2).min(MAX_ATLAS_SIZE);
        // 打包都在左上区域，增长向右下扩展，已有字形位置/UV 不变（无需重打包）。
        let mut pixels = vec![0; (new_width * new_height * 4) as usize];
        for y in 0..old_height {
            let old_start = (y * old_width * 4) as usize;
            let new_start = (y * new_width * 4) as usize;
            let len = (old_width * 4) as usize;
            pixels[new_start..new_start + len]
                .copy_from_slice(&self.pixels[old_start..old_start + len]);
        }
        self.width = new_width;
        self.height = new_height;
        self.pixels = pixels;
        self.packer.set_size(new_width, new_height);
        self.version = self.version.wrapping_add(1);
        self.dirty = true;
        true
    }

    /// Returns per-glyph metrics, rasterizing the MSDF on first use (packed).
    pub fn try_glyph_metrics(&mut self, character: char) -> Result<GlyphMetrics, FontAtlasError> {
        if let Some(metrics) = self.cached.get(&character) {
            return Ok(*metrics);
        }
        let glyph = rasterize_glyph(&self.font, &self.fallback, character);
        self.insert_rasterized(glyph)
    }

    /// 并行预热一批字形（chars.txt 收集的字符）：MSDF 光栅化互相独立、跨线程
    /// 并行（debug 构建下大幅加速），打包/写入图集串行，布局与 `try_glyph_metrics`
    /// 逐字调用完全一致（顺序 = chars 顺序，UV/图集增长不变）。返回成功加入数。
    pub fn prewarm(&mut self, chars: impl Iterator<Item = char>) -> usize {
        // 去重（保持首次出现顺序），跳过空白与已缓存字形。
        let mut seen = HashSet::new();
        let mut unique: Vec<char> = Vec::new();
        for ch in chars.filter(|ch| !ch.is_whitespace()) {
            if seen.insert(ch) && !self.cached.contains_key(&ch) {
                unique.push(ch);
            }
        }
        if unique.is_empty() {
            return 0;
        }
        let font = self.font.clone();
        let fallback = self.fallback.clone();
        // Phase A（并行）：光栅化全部字形，不触碰图集状态。按核数分块，避免
        // 每字形一线程的调度开销（CJK 预热数百字形时明显）。
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(unique.len())
            .max(1);
        let chunk = unique.len().div_ceil(workers);
        let glyphs: Vec<PrewarmedGlyph> = std::thread::scope(|scope| {
            let mut handles: Vec<std::thread::ScopedJoinHandle<'_, Vec<PrewarmedGlyph>>> =
                Vec::with_capacity(workers);
            for block in unique.chunks(chunk) {
                let font = font.clone();
                let fallback = fallback.clone();
                let block = block.to_vec();
                handles.push(scope.spawn(move || {
                    block
                        .into_iter()
                        .map(|ch| rasterize_glyph(&font, &fallback, ch))
                        .collect()
                }));
            }
            // 按块序 join 后扁平化，保持 chars 原始顺序（打包布局与逐字预热一致）。
            handles
                .into_iter()
                .flat_map(|handle| handle.join().ok().into_iter().flatten())
                .collect()
        });
        // Phase B（串行）：按 chars 顺序打包进图集，布局与逐字预热一致。
        let mut count = 0;
        for glyph in glyphs {
            if self.cached.contains_key(&glyph.character) {
                continue;
            }
            if self.insert_rasterized(glyph).is_ok() {
                count += 1;
            }
        }
        count
    }

    /// 把一个已光栅化字形的像素打包进图集（空间不足时增长；放不下则报满）。
    /// 提取自 `try_glyph_metrics`，供其与 `prewarm` 复用同一打包/写入路径。
    fn insert_rasterized(&mut self, glyph: PrewarmedGlyph) -> Result<GlyphMetrics, FontAtlasError> {
        let PrewarmedGlyph {
            character,
            rgba: cell_rgba,
            width: rect_w,
            height: rect_h,
            cell_rect,
            bounds,
            advance,
        } = glyph;
        // 打包；空间不足则增长图集（已有字形位置不变）。
        let (x, y) = loop {
            if let Some(at) = self.packer.pack(rect_w, rect_h) {
                break at;
            }
            if !self.grow() {
                return Err(FontAtlasError::AtlasFull {
                    character,
                    width: self.width,
                    height: self.height,
                    capacity: self.used,
                });
            }
        };
        // cell_rgba 是 rect_w×rect_h×4 的 RGBA（MSDF 三通道 + alpha=255）。
        let row_bytes = (rect_w * 4) as usize;
        for dy in 0..rect_h {
            let src = dy as usize * row_bytes;
            let dst = (((y + dy) * self.width + x) as usize) * 4;
            self.pixels[dst..dst + row_bytes].copy_from_slice(&cell_rgba[src..src + row_bytes]);
        }
        let uv = [
            x as f32 / self.width as f32,
            y as f32 / self.height as f32,
            (x + rect_w) as f32 / self.width as f32,
            (y + rect_h) as f32 / self.height as f32,
        ];
        let metrics = GlyphMetrics {
            uv,
            cell_rect,
            bounds,
            advance,
        };
        self.used += 1;
        self.dirty = true;
        self.version = self.version.wrapping_add(1);
        self.cached.insert(character, metrics);
        Ok(metrics)
    }
}

/// Shelf 打包器：把不同尺寸的字形矩形密排进图集。字形只增不删（缓存后永驻），
/// 无需释放；空间不足由调用方增长画布（增长向右下扩展，已有位置不变）。
#[derive(Clone, Debug)]
struct Packer {
    width: u32,
    height: u32,
    /// 打开的 shelf：(y, shelf_height, 已用 x)。
    shelves: Vec<(u32, u32, u32)>,
    next_y: u32,
}

impl Packer {
    fn new(width: u32, height: u32) -> Self {
        Packer {
            width,
            height,
            shelves: Vec::new(),
            next_y: 0,
        }
    }

    fn set_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// 放置 `w×h` 矩形，返回左上角坐标；放不下返回 `None`。
    fn pack(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        for (y, shelf_h, used_x) in &mut self.shelves {
            if *shelf_h >= h && *used_x + w <= self.width {
                let at = (*used_x, *y);
                *used_x += w;
                return Some(at);
            }
        }
        if w <= self.width && self.next_y + h <= self.height {
            let at = (0, self.next_y);
            self.next_y += h;
            self.shelves.push((at.1, h, w));
            return Some(at);
        }
        None
    }
}

/// 字形在 `RASTER_SCALE` 下的打包矩形尺寸 = 像素包围盒 + `2*PAD` 边距。
/// 单个已光栅化字形（未打包）：供并行预热阶段产出，再由 `insert_rasterized`
/// 串行打包进图集。`cell_rect`/`bounds` 与 `rasterize_msdf` 返回一致。
struct PrewarmedGlyph {
    character: char,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    cell_rect: [f32; 4],
    bounds: [f32; 4],
    advance: f32,
}

/// 解析字形所属字体（主字体缺失 → 后备字体 → 主字体 .notdef）并光栅化 MSDF。
/// 与 `FontAtlas::font_for` 选择逻辑一致；只读字体数据，可跨线程调用。
fn rasterize_glyph(font: &FontArc, fallback: &Option<FontArc>, character: char) -> PrewarmedGlyph {
    let primary_id = font.glyph_id(character);
    let (face, glyph_id) = if primary_id.0 != 0 {
        (font, primary_id)
    } else if let Some(fallback) = fallback {
        let fallback_id = fallback.glyph_id(character);
        if fallback_id.0 != 0 {
            (fallback, fallback_id)
        } else {
            (font, primary_id)
        }
    } else {
        (font, primary_id)
    };
    let units_per_em = face.units_per_em().unwrap_or(1000.0);
    let advance = face.h_advance_unscaled(glyph_id) / units_per_em * RASTER_SCALE;
    // 打包矩形 = 字形像素包围盒 + 2*PAD 边距。
    let (rect_w, rect_h) = match face.outline(glyph_id) {
        Some(outline) => glyph_rect_size(&outline, units_per_em),
        None => placeholder_rect_size(),
    };
    let (rgba, cell_rect, bounds) = match face.outline(glyph_id) {
        Some(outline) => rasterize_msdf(&outline, rect_w, rect_h, units_per_em),
        None => placeholder_msdf(rect_w, rect_h),
    };
    PrewarmedGlyph {
        character,
        rgba,
        width: rect_w,
        height: rect_h,
        cell_rect,
        bounds,
        advance,
    }
}

fn glyph_rect_size(outline: &Outline, units_per_em: f32) -> (u32, u32) {
    let scale = RASTER_SCALE as f64 / units_per_em as f64;
    let b = &outline.bounds;
    let w = ((b.max.x - b.min.x) as f64 * scale).ceil() as i64 + PAD as i64 * 2;
    let h = ((b.min.y - b.max.y) as f64 * scale).ceil() as i64 + PAD as i64 * 2;
    (w.max(1) as u32, h.max(1) as u32)
}

/// 占位字形（方框）的打包矩形尺寸。
fn placeholder_rect_size() -> (u32, u32) {
    let side = RASTER_SCALE as u32 + PAD as u32 * 2;
    (side, side)
}

/// Rasterizes a glyph outline (design space, Y-up) into an MSDF rect
/// (RGBA8: RGB = 多通道距离场, A = 255；边缘处 median ≈ 128)。返回
/// `(rgba, cell_rect, bounds)`：`cell_rect` 为字形在 rect 内的
/// `[left, top, right, bottom]`（Y 向下），`bounds` 为相对笔的设备像素包围盒。
fn rasterize_msdf(
    outline: &Outline,
    width: u32,
    height: u32,
    units_per_em: f32,
) -> (Vec<u8>, [f32; 4], [f32; 4]) {
    let scale = RASTER_SCALE as f64 / units_per_em as f64;
    let b = &outline.bounds;
    // ab_glyph Rect.min = 左上、Rect.max = 右下。设计空间点（Y 向上）：b.min.y = 顶部。
    let left_px = b.min.x as f64 * scale;
    let right_px = b.max.x as f64 * scale;
    let top_up = b.min.y as f64 * scale; // Y-up 顶部（y 大）
    let bottom_up = b.max.y as f64 * scale; // Y-up 底部（y 小）
    let wf = width as f64;
    let hf = height as f64;
    // 把字形居中到 rect（Y-up 像素坐标，0..height）。
    let ox = wf / 2.0 - (left_px + right_px) / 2.0;
    let oy = hf / 2.0 - (top_up + bottom_up) / 2.0;

    let transform = |p: &Point| Vector2::new(p.x as f64 * scale + ox, p.y as f64 * scale + oy);
    let mut shape = Shape::new();
    let mut contour = Contour::new();
    let mut prev_end: Option<[f32; 2]> = None;
    for curve in &outline.curves {
        let (start, end, segment) = match curve {
            OutlineCurve::Line(a, b) => (a, b, EdgeSegment::line(transform(a), transform(b))),
            OutlineCurve::Quad(a, c, b) => (
                a,
                b,
                EdgeSegment::quadratic(transform(a), transform(c), transform(b)),
            ),
            OutlineCurve::Cubic(a, c, d, b) => (
                a,
                b,
                EdgeSegment::cubic(transform(a), transform(c), transform(d), transform(b)),
            ),
        };
        if let Some(prev) = prev_end {
            // move_to 后新轮廓的第一条曲线起点 != 上一条曲线终点 → 新轮廓。
            if (start.x != prev[0] || start.y != prev[1]) && !contour.is_empty() {
                shape.add_contour(std::mem::take(&mut contour));
            }
        }
        contour.add_edge(segment);
        prev_end = Some([end.x, end.y]);
    }
    if !contour.is_empty() {
        shape.add_contour(contour);
    }
    if shape.contours.is_empty() {
        return placeholder_msdf(width, height);
    }

    let rgba = msdf_rect_from_shape(&mut shape, width, height);
    // cell_rect（Y 向下）：水平不变，垂直把 Y-up 顶部翻到 rect 顶部。
    let cell_rect = [
        (left_px + ox) as f32,
        (hf - (top_up + oy)) as f32,
        (right_px + ox) as f32,
        (hf - (bottom_up + oy)) as f32,
    ];
    // bounds（Y 向下、相对 pen 的设备像素）：y_down = -y_up。
    let bounds = [
        left_px as f32,
        (-top_up) as f32,
        right_px as f32,
        (-bottom_up) as f32,
    ];
    (rgba, cell_rect, bounds)
}

/// 从已就绪的 Shape 生成 MSDF 像素（width×height×4 RGBA，行 0 = 顶部）。
/// bymsdfgen 的 `Bitmap` 行 0 在底部（Y 向上），翻转行序写入以匹配 Y 向下的图集。
fn msdf_rect_from_shape(shape: &mut Shape, width: u32, height: u32) -> Vec<u8> {
    shape.normalize();
    shape.orient_contours();
    edge_coloring_simple(shape, 3.0, 0);
    let w = width as usize;
    let h = height as usize;
    let mut msdf: Bitmap<f32, 3> = Bitmap::new(w, h);
    let transformation = SdfTransformation::new(
        Projection::default(),
        DistanceMapping::from_range(Range::symmetric(PAD as f64)),
    );
    generate_msdf(
        &mut msdf,
        shape,
        &transformation,
        &MsdfGeneratorConfig::default(),
    );
    let mut pixels = vec![0u8; w * h * 4];
    for by in 0..h {
        let ay = h - 1 - by;
        for bx in 0..w {
            let c = msdf.pixel(bx, by);
            let off = (ay * w + bx) * 4;
            pixels[off] = (c[0].clamp(0.0, 1.0) * 255.0) as u8;
            pixels[off + 1] = (c[1].clamp(0.0, 1.0) * 255.0) as u8;
            pixels[off + 2] = (c[2].clamp(0.0, 1.0) * 255.0) as u8;
            pixels[off + 3] = 255;
        }
    }
    pixels
}

/// 缺失字形：画一个占位方框，同样走 MSDF。
fn placeholder_msdf(width: u32, height: u32) -> (Vec<u8>, [f32; 4], [f32; 4]) {
    let wf = width as f32;
    let hf = height as f32;
    let inset_x = (wf / 5.0).min(wf / 2.0 - 1.0).max(1.0);
    let inset_y = (hf / 5.0).min(hf / 2.0 - 1.0).max(1.0);
    let pts = [
        [inset_x, inset_y],
        [wf - inset_x, inset_y],
        [wf - inset_x, hf - inset_y],
        [inset_x, hf - inset_y],
    ];
    let mut shape = Shape::new();
    let contour = shape.add_contour_mut();
    for i in 0..4 {
        contour.add_edge(EdgeSegment::line(
            Vector2::new(pts[i][0] as f64, pts[i][1] as f64),
            Vector2::new(pts[(i + 1) % 4][0] as f64, pts[(i + 1) % 4][1] as f64),
        ));
    }
    let rgba = msdf_rect_from_shape(&mut shape, width, height);
    let rect = [inset_x, inset_y, wf - inset_x, hf - inset_y];
    (rgba, rect, rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_font() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/editor/fonts/NotoSansSC-VF.ttf");
        std::fs::read(&path)
            .unwrap_or_else(|error| panic!("测试字体缺失或不可读 {}: {error}", path.display()))
    }

    #[test]
    fn rasterizes_sdf_glyph_and_reports_metrics() {
        let bytes = test_font();
        assert!(!bytes.is_empty(), "测试字体为空");
        let mut atlas = FontAtlas::load(&bytes).unwrap();
        let metrics = atlas.try_glyph_metrics('俯').unwrap();
        assert_ne!(metrics.uv, [0.0; 4], "glyph uv is non-zero");
        assert!(metrics.advance > 0.0, "advance is positive");
        assert!(metrics.bounds[0].is_finite() && metrics.bounds[2] > metrics.bounds[0]);
        assert!(atlas.dirty(), "new glyph marks atlas dirty");
        assert_eq!(atlas.version(), 1);
        let metrics2 = atlas.try_glyph_metrics('俯').unwrap();
        assert_eq!(metrics, metrics2, "same glyph is cached");
        // MSDF 符号验证：打包矩形角落（边距区）应为外部（RGB median<128）；字形
        // 内部存在 median>140 的像素。median(r,g,b)≈0.5 即边缘。
        let pixel_median = |offset: usize| -> u8 {
            let mut v = [
                atlas.pixels()[offset],
                atlas.pixels()[offset + 1],
                atlas.pixels()[offset + 2],
            ];
            v.sort_unstable();
            v[1]
        };
        // 第一个字形打包在 (0,0)，矩形尺寸由 uv 换算。
        let rect_w = ((metrics.uv[2] - metrics.uv[0]) * atlas.width() as f32) as u32;
        let rect_h = ((metrics.uv[3] - metrics.uv[1]) * atlas.height() as f32) as u32;
        let corner = pixel_median(0);
        assert!(
            corner < 128,
            "rect corner should be outside glyph, got {corner}"
        );
        let mut inside = 0;
        for dy in 0..rect_h {
            for dx in 0..rect_w {
                let offset = ((dy * atlas.width() + dx) as usize) * 4;
                if pixel_median(offset) > 140 {
                    inside += 1;
                }
            }
        }
        assert!(inside > 0, "glyph stroke must be inside (median>128)");
        // MSDF 路径的 cell_rect/bounds：落在打包矩形内且非退化。
        let rect = metrics.cell_rect;
        assert!(
            rect[0] >= 0.0
                && rect[1] >= 0.0
                && rect[2] <= rect_w as f32
                && rect[3] <= rect_h as f32,
            "cell_rect within rect: {rect:?}"
        );
        assert!(
            rect[2] > rect[0] && rect[3] > rect[1],
            "glyph rect non-degenerate: {rect:?}"
        );
    }

    #[test]
    fn rejects_invalid_font_bytes_without_leaking_owned_storage() {
        let error = FontAtlas::load(b"not-a-font").expect_err("invalid bytes must fail");
        assert!(error.contains("字体解析失败"));
    }

    /// 微软雅黑（msyh.ttc，TTF 集合）应能被 FontArc 解析（face 0），且 MSDF
    /// 图集对复杂 CJK 字形正常生成（角外部 median<128、存在内部像素 median>140）。
    #[test]
    fn msdf_generates_for_msyh_ttc_collection() {
        let path = crate::core::fixture_games_dir().join("ui_demo/assets/fonts/msyh.ttc");
        if !path.is_file() {
            return;
        }
        let bytes = std::fs::read(&path).expect("msyh.ttc 已拷入 ui_demo assets/fonts");
        let mut atlas = FontAtlas::load(&bytes).expect("FontArc 应能解析 msyh.ttc 集合");
        let metrics = atlas.try_glyph_metrics('腾').unwrap();
        assert!(metrics.advance > 0.0);
        assert!(metrics.bounds[2] > metrics.bounds[0] && metrics.bounds[3] > metrics.bounds[1]);
        let pixel_median = |offset: usize| -> u8 {
            let mut v = [
                atlas.pixels()[offset],
                atlas.pixels()[offset + 1],
                atlas.pixels()[offset + 2],
            ];
            v.sort_unstable();
            v[1]
        };
        let rect_w = ((metrics.uv[2] - metrics.uv[0]) * atlas.width() as f32) as u32;
        let rect_h = ((metrics.uv[3] - metrics.uv[1]) * atlas.height() as f32) as u32;
        let corner = pixel_median(0);
        assert!(corner < 128, "rect corner outside glyph, got {corner}");
        let mut inside = 0;
        for dy in 0..rect_h {
            for dx in 0..rect_w {
                let offset = ((dy * atlas.width() + dx) as usize) * 4;
                if pixel_median(offset) > 140 {
                    inside += 1;
                }
            }
        }
        assert!(inside > 0, "msyh 字形应有内部像素（median>128）");
    }

    /// 后备字体：主字体缺失的字形用后备字体生成（advance/bounds 来自后备字体）。
    #[test]
    fn fallback_font_serves_chars_missing_from_primary() {
        let path = crate::core::fixture_games_dir().join("ui_demo/assets/fonts/msyh.ttc");
        if !path.is_file() {
            return;
        }
        let primary_bytes = std::fs::read(&path).expect("msyh.ttc");
        let fallback_bytes = test_font(); // Noto Sans SC
        // 找一个主字体缺失、后备字体存在的字符（msyh 覆盖有限、Noto 更全）。
        let primary = FontArc::try_from_vec(primary_bytes.clone()).unwrap();
        let fallback = FontArc::try_from_vec(fallback_bytes.clone()).unwrap();
        let candidate = (0x4E00u32..0x9FFF)
            .chain(0x2000..0x2070)
            .chain(0x20A0..0x20C0)
            .chain(0x3000..0x3040)
            .filter_map(char::from_u32)
            .find(|c| primary.glyph_id(*c).0 == 0 && fallback.glyph_id(*c).0 != 0)
            .expect("应存在主字体缺而后备字体有的字符");
        let mut atlas =
            FontAtlas::load_with_fallback(&primary_bytes, Some(&fallback_bytes)).unwrap();
        let metrics = atlas.try_glyph_metrics(candidate).unwrap();
        assert!(metrics.advance > 0.0, "后备字形应有 advance");
        assert!(
            metrics.bounds[2] > metrics.bounds[0] && metrics.bounds[3] > metrics.bounds[1],
            "后备字形应有非退化 bounds: {:?}",
            metrics.bounds
        );
        // 主字体有的字符仍由主字体提供（advance 与仅主字体加载一致）。
        let mut primary_only = FontAtlas::load(&primary_bytes).unwrap();
        let a = atlas.try_glyph_metrics('A').unwrap();
        let b = primary_only.try_glyph_metrics('A').unwrap();
        assert_eq!(a.advance, b.advance, "主字体字符的 advance 应与单字体一致");
    }

    #[test]
    fn atlas_grows_and_preserves_cached_glyph_geometry() {
        let bytes = test_font();
        let mut atlas = FontAtlas::load(&bytes).unwrap();
        let first = atlas.try_glyph_metrics('俯').unwrap();
        let old_width = atlas.width();
        let old_height = atlas.height();
        // 捕获首个字形打包矩形（原点在 (0,0)）在旧图集中的像素块。
        let rect_w = ((first.uv[2] - first.uv[0]) * old_width as f32) as u32;
        let rect_h = ((first.uv[3] - first.uv[1]) * old_height as f32) as u32;
        let old_rect_pixels: Vec<u8> = (0..rect_h)
            .flat_map(|y| {
                let start = (y * old_width * 4) as usize;
                atlas.pixels()[start..start + (rect_w * 4) as usize].to_vec()
            })
            .collect();
        let old_version = atlas.version();
        atlas.mark_uploaded();
        // 填满初始图集并触发增长；增长不应破坏已缓存字形（打包位置不变 → UV 稳定）。
        for index in 0..=256u32 {
            let character = char::from_u32(0x4e00 + index).unwrap();
            let _ = atlas.try_glyph_metrics(character).unwrap();
        }
        assert!(atlas.width() > old_width || atlas.height() > old_height);
        assert!(atlas.version() > old_version);
        assert!(atlas.dirty());
        assert_eq!(
            atlas.try_glyph_metrics('俯').unwrap().cell_rect,
            first.cell_rect
        );
        assert_eq!(atlas.try_glyph_metrics('俯').unwrap().bounds, first.bounds);
        assert_eq!(
            atlas.try_glyph_metrics('俯').unwrap().advance,
            first.advance
        );
        assert_eq!(
            atlas.try_glyph_metrics('俯').unwrap().uv,
            first.uv,
            "打包 UV 应随增长保持稳定"
        );
        for y in 0..rect_h {
            let start = (y * atlas.width() * 4) as usize;
            let end = start + (rect_w * 4) as usize;
            let expected_start = (y * rect_w * 4) as usize;
            assert_eq!(
                &atlas.pixels()[start..end],
                &old_rect_pixels[expected_start..expected_start + (rect_w * 4) as usize],
                "old first-glyph rect pixels are preserved"
            );
        }
    }

    #[test]
    fn atlas_full_is_reported_structurally() {
        let bytes = test_font();
        let mut atlas = FontAtlas::load(&bytes).unwrap();
        // 用不同 Unicode 字形填满打包图集（增长到 4096² 后报满），记录首个失败字形。
        let mut full = None;
        for index in 0..(MAX_ATLAS_SIZE / 2) {
            let character = char::from_u32(0x1000 + index).unwrap();
            if atlas.try_glyph_metrics(character).is_err() {
                full = Some(character);
                break;
            }
        }
        assert!(full.is_some(), "打包图集应在某字形处报满");
        let error = atlas.try_glyph_metrics(full.unwrap()).unwrap_err();
        assert!(matches!(error, FontAtlasError::AtlasFull { .. }));
        assert!(error.to_string().contains("字体图集已满"));
    }

    #[test]
    fn atlas_dirty_can_be_cleared_after_upload() {
        let bytes = test_font();
        let mut atlas = FontAtlas::load(&bytes).unwrap();
        let _ = atlas.try_glyph_metrics('A').unwrap();
        assert!(atlas.dirty());
        atlas.mark_uploaded();
        assert!(!atlas.dirty());
    }

    /// M11-05：AtlasFull 之后 glyph 缓存 / 像素缓冲都不再增长——继续请求未缓存
    /// 字形只会失败（返回 AtlasFull 错误），不会扩张内存。
    #[test]
    fn glyph_cache_and_pixels_stay_bounded_after_atlas_full() {
        let bytes = test_font();
        let mut atlas = FontAtlas::load(&bytes).unwrap();
        for index in 0..(MAX_ATLAS_SIZE / 2) {
            let character = char::from_u32(0x1000 + index).unwrap();
            if atlas.try_glyph_metrics(character).is_err() {
                break;
            }
        }
        let cached = atlas.cached.len();
        let pixels = atlas.pixels.len();
        let width = atlas.width();
        let height = atlas.height();
        for index in 0..256u32 {
            let character = char::from_u32(0x8000 + index).unwrap();
            assert!(
                matches!(
                    atlas.try_glyph_metrics(character),
                    Err(FontAtlasError::AtlasFull { .. })
                ),
                "已满后新字形应返回 AtlasFull"
            );
        }
        assert_eq!(atlas.cached.len(), cached, "缓存不应继续增长");
        assert_eq!(atlas.pixels.len(), pixels, "像素缓冲不应继续增长");
        assert_eq!(atlas.width(), width, "图集宽度不应继续增长");
        assert_eq!(atlas.height(), height, "图集高度不应继续增长");
        assert_eq!(atlas.width(), MAX_ATLAS_SIZE, "打包图集应增长到 4096 上限");
    }

    /// M11-05：字体热重载 / 场景切换（反复 load → rasterize → drop）无持续内存
    /// 增长——每轮重建的新 atlas 内存有界，glyph 缓存不跨实例累积，owned buffer
    /// 随 drop 释放（用缓存/像素上限作为无泄漏的代理证据）。
    #[test]
    fn repeated_load_rasterize_drop_keeps_memory_bounded() {
        let bytes = test_font();
        let sample: Vec<char> = "引擎编辑器场景切换热重载字体内存有界".chars().collect();
        let mut max_cached = 0usize;
        let mut max_pixels = 0usize;
        for _ in 0..50 {
            let mut atlas = FontAtlas::load(&bytes).unwrap();
            for character in &sample {
                let _ = atlas.try_glyph_metrics(*character);
            }
            max_cached = max_cached.max(atlas.cached.len());
            max_pixels = max_pixels.max(atlas.pixels.len());
            // drop 释放 FontArc owned buffer 与 glyph 缓存。
        }
        // 每轮内存有界：缓存 ≤ 输入字形数，像素 ≤ 初始网格（样例未触发 grow）。
        assert!(
            max_cached <= sample.len(),
            "缓存应 ≤ 输入字形数，实际 {max_cached}"
        );
        let initial_atlas_pixels =
            (INITIAL_ATLAS_SIZE as usize) * (INITIAL_ATLAS_SIZE as usize) * 4;
        assert!(
            max_pixels <= initial_atlas_pixels,
            "像素缓冲应 ≤ 初始图集（样例未触发增长），实际 {max_pixels}"
        );
    }

    /// 并行预热与逐字顺序预热产出一致：同一批字符下，图集尺寸/像素、字形 UV 与
    /// 图集版本都相同。这是把光栅化拆出并行（`rasterize_glyph` + 串行打包）的
    /// 回归防线——布局顺序必须与 `try_glyph_metrics` 逐字调用完全一致。
    #[test]
    fn parallel_prewarm_matches_sequential_layout() {
        let bytes = test_font();
        let sample: Vec<char> = "并行预热与顺序一致渲染不乱码ABC123，。！？"
            .chars()
            .collect();
        // 顺序预热：逐字 try_glyph_metrics。
        let mut sequential = FontAtlas::load(&bytes).unwrap();
        let mut seq_metrics = Vec::new();
        for &ch in &sample {
            let metrics = sequential.try_glyph_metrics(ch).unwrap();
            seq_metrics.push((ch, metrics));
        }
        // 并行预热：prewarm（内部去重、按 chars 顺序串行打包）。
        let mut parallel = FontAtlas::load(&bytes).unwrap();
        let count = parallel.prewarm(sample.iter().copied());
        assert_eq!(count, sample.len(), "prewarm 应成功加入全部样例字形");
        assert_eq!(
            parallel.width(),
            sequential.width(),
            "并行预热图集宽度应与顺序预热一致"
        );
        assert_eq!(
            parallel.height(),
            sequential.height(),
            "并行预热图集高度应与顺序预热一致"
        );
        assert_eq!(
            parallel.pixels(),
            sequential.pixels(),
            "图集像素应逐字节一致"
        );
        for (ch, expected) in seq_metrics {
            let actual = parallel.try_glyph_metrics(ch).unwrap();
            assert_eq!(
                actual, expected,
                "字形 {ch:?} 的 UV/metrics 应在并行与顺序预热间一致"
            );
        }
    }
}
