//! On-demand glyph atlas for ASCII + CJK, rasterized from a TTF font with
//! `ab_glyph` outlines into **multi-channel signed-distance-field** cells
//! (RGB = MSDF 三通道，A = 255；边缘处 RGB 的 median ≈ 0.5)。宿主 shader 对
//! RGB 取 median 再经 fwidth 解码，直角/笔画末梢保持锐利、任意缩放不失真
//! （TextMeshPro 同源方案）。

use ab_glyph::{
    Font, FontArc, FontVec, Glyph, Outline, OutlineCurve, OutlinedGlyph, Point, PxScaleFactor,
    VariableFont,
};
use bymsdfgen_core::{
    Bitmap, DistanceMapping, GeneratorConfig, Projection, Range, SdfTransformation, generate_sdf,
    geometry::{Contour, EdgeSegment, Shape},
    math::Vector2,
};
use std::collections::{HashMap, HashSet};

/// Rasterize scale in device pixels（px/em）。96px/em 在 4096² 打包图集内可容纳
/// ~1300 个 CJK 字形，锐利度约为旧 64px/em 的 1.5×；如需更锐可上调，但 CJK
/// 容量会相应下降（128px/em 约 ~900）。
pub const RASTER_SCALE: f32 = 96.0;
/// Default variable-font weight used when a project does not configure one.
/// Many CJK variable fonts default to Thin, which becomes sub-pixel at UI sizes.
pub const DEFAULT_FONT_WEIGHT: f32 = 400.0;
/// 打包图集初始边长（正方形）。
const INITIAL_ATLAS_SIZE: u32 = 1024;
/// 打包图集最大边长（常见 GPU 纹理上限内）。
const MAX_ATLAS_SIZE: u32 = 4096;
/// Signed-distance band around each glyph outline (px)。MSDF 映射 `±PAD` texels
/// 到 `[0,1]`（边缘 median=0.5），shader 的 fwidth 解码在任意字号下都保持
/// ~1px 渐变。
const PAD: i32 = 8;
/// 像素字体（硬边点阵）栅格的 gutter：1 texel 透明边，防止 Nearest 采样在四边形
/// 边界外溢到相邻单元。MSDF 的 `PAD` 是距离带宽度，点阵没有距离带，只需要防溢。
const PIXEL_PAD: u32 = 1;
/// 像素字体探测的候选 em 尺寸（px/em）下界与上界。
/// 下界 8：再小的 CJK 点阵不可读，且探针覆盖像素数不足以判别。
/// 上界 24：常见像素字的设计高度都在 8–24 之间（Fusion Pixel 12px、Zpix 12px）。
const PIXEL_PROBE_MIN_PX: f32 = 8.0;
const PIXEL_PROBE_MAX_PX: f32 = 24.0;
/// 判定的最小样本：至少要这么多字形有非零覆盖，且**每个字形平均**点亮到
/// `em² / PIXEL_PROBE_MIN_FILL_DIVISOR` 个 texel 以上，否则一次"全二值"只是没测到东西。
///
/// 亮度下限必须按 em 面积归一，不能写绝对 texel 数：实测 Fusion Pixel 的**子集**（发布
/// 形态）里 `国`/`鼎` 不在收字范围内 → 探针只剩 6 个字形，12px 档总覆盖 188 texel，
/// 绝对下限 200 判不过，检测就滑到下一个二值档 24px（= 设计像素的 2 倍），整条点阵路的
/// `raster_scale` 跟着错。子集与全量必须判出同一个值，所以这里用尺度无关的占比判据。
const PIXEL_PROBE_MIN_GLYPHS: usize = 4;
const PIXEL_PROBE_MIN_FILL_DIVISOR: usize = 16;
/// 探测用探针字符：密集 CJK + 半角 + 全角标点。像素字体的判别力主要来自
/// 密集 CJK（笔画多 → 边界多 → 非整数网格时必然出现部分覆盖像素）。
const PIXEL_PROBE_CHARS: [char; 8] = ['国', '露', '鼎', '藏', '8', 'A', '，', '。'];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VectorSegment {
    pub start: [f32; 2],
    pub end: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct VectorGlyph {
    pub id: u32,
    pub bounds: [f32; 4],
    pub advance: f32,
    pub segments: Vec<VectorSegment>,
}

#[derive(Clone, Debug)]
pub struct FontVectorAtlas {
    font: FontArc,
    fallback: Option<FontArc>,
    /// 与 `FontAtlas::is_pixel_font` 同一条判据：像素字体的轮廓是点阵矩形，矢量
    /// 路径会把它们 AA 重采样成灰边，因此渲染层要据此退回点阵档。
    pixel: Option<f32>,
    cached: HashMap<char, VectorGlyph>,
    by_id: Vec<VectorGlyph>,
    missing: HashSet<char>,
}

impl FontVectorAtlas {
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        let font = load_font_with_weight(bytes, DEFAULT_FONT_WEIGHT, "字体解析失败")?;
        Ok(Self::new(font, None))
    }

    pub fn load_with_fallback(primary: &[u8], fallback: Option<&[u8]>) -> Result<Self, String> {
        let font = load_font_with_weight(primary, DEFAULT_FONT_WEIGHT, "字体解析失败")?;
        let fallback = fallback
            .map(|bytes| load_font_with_weight(bytes, DEFAULT_FONT_WEIGHT, "后备字体解析失败"))
            .transpose()?;
        Ok(Self::new(font, fallback))
    }

    pub fn new(font: FontArc, fallback: Option<FontArc>) -> Self {
        let pixel = detect_pixel_size(&font);
        Self {
            font,
            fallback,
            pixel,
            cached: HashMap::new(),
            by_id: Vec::new(),
            missing: HashSet::new(),
        }
    }

    /// 主字体是否被判为像素字体（与 `FontAtlas::is_pixel_font` 同一判据、同一字体
    /// 字节）。矢量渲染会把点阵轮廓 AA 成灰边，所以为真时渲染层必须放弃矢量档。
    #[must_use]
    pub const fn is_pixel_font(&self) -> bool {
        self.pixel.is_some()
    }

    /// 像素字体的设计像素高度（`None` = 不是像素字体）。
    #[must_use]
    pub const fn pixel_design_px(&self) -> Option<f32> {
        self.pixel
    }

    /// Ascent in em, matching `FontAtlas::ascent_em` so vector and MSDF text
    /// start layout from the same baseline.
    pub fn ascent_em(&self) -> f32 {
        self.font.ascent_unscaled() / self.font.units_per_em().unwrap_or(1000.0)
    }

    /// Descent in em (negative), matching [`FontAtlas::descent_em`] so vertical
    /// alignment and block height agree between the vector and SDF paths.
    pub fn descent_em(&self) -> f32 {
        self.font.descent_unscaled() / self.font.units_per_em().unwrap_or(1000.0)
    }

    pub fn glyph(&mut self, character: char) -> Option<&VectorGlyph> {
        if self.missing.contains(&character) {
            return None;
        }
        if !self.cached.contains_key(&character) {
            let (font, glyph_id) = self.font_for(character);
            let Some(outline) = font.outline(glyph_id) else {
                self.missing.insert(character);
                return None;
            };
            let units = font.units_per_em().unwrap_or(1000.0);
            let segments = flatten_outline(&outline, units);
            if segments.is_empty() {
                self.missing.insert(character);
                return None;
            }
            let b = outline.bounds;
            // ab_glyph 的 outline.bounds 采用屏幕约定：min=左上、max=右下（y 向下）。
            // 这里统一转成 y 向下的 [left, top, right, bottom]（top < bottom）。
            let glyph = VectorGlyph {
                id: self.by_id.len() as u32,
                bounds: [
                    b.min.x / units,
                    -b.min.y / units,
                    b.max.x / units,
                    -b.max.y / units,
                ],
                advance: font.h_advance_unscaled(glyph_id) / units,
                segments,
            };
            self.by_id.push(glyph.clone());
            self.cached.insert(character, glyph);
        }
        self.cached.get(&character)
    }

    /// 该字符的 em 相对 advance（`advance / units_per_em`），与
    /// [`FontProbe::advance_em`] 同一口径。空白字符没有轮廓、进不了 `glyph()`，
    /// 但排版必须照它的 hmtx 推进——矢量档因此需要这条只读通道。
    #[must_use]
    pub fn advance_em(&self, character: char) -> f32 {
        let (font, glyph_id) = self.font_for(character);
        font.h_advance_unscaled(glyph_id) / font.units_per_em().unwrap_or(1000.0)
    }

    /// 主字体没有该字形时回落到后备字体（与 [`FontVectorAtlas::glyph`] 同一选择顺序）。
    fn font_for(&self, character: char) -> (&FontArc, ab_glyph::GlyphId) {
        let primary = self.font.glyph_id(character);
        if primary.0 != 0 {
            return (&self.font, primary);
        }
        if let Some(fallback) = &self.fallback {
            let id = fallback.glyph_id(character);
            if id.0 != 0 {
                return (fallback, id);
            }
        }
        (&self.font, primary)
    }

    pub fn prewarm(&mut self, chars: impl Iterator<Item = char>) -> usize {
        let before = self.cached.len();
        for ch in chars.filter(|ch| !ch.is_whitespace()) {
            let _ = self.glyph(ch);
        }
        self.cached.len() - before
    }

    pub fn glyphs(&self) -> impl Iterator<Item = &VectorGlyph> {
        self.cached.values()
    }

    pub fn glyph_by_id(&self, id: u32) -> Option<&VectorGlyph> {
        self.by_id.get(id as usize)
    }
}

/// 字形来源选择：主字体有该字用主字体，否则用后备字体，再否则主字体（`.notdef`）。
/// `FontAtlas` 与 [`FontProbe`] 共用这一处判定，避免"渲染用一套、度量用另一套"。
fn select_font(
    primary: &FontArc,
    fallback: Option<&FontArc>,
    character: char,
) -> (FontArc, ab_glyph::GlyphId) {
    let primary_id = primary.glyph_id(character);
    if primary_id.0 != 0 {
        return (primary.clone(), primary_id);
    }
    if let Some(fallback) = fallback {
        let fallback_id = fallback.glyph_id(character);
        if fallback_id.0 != 0 {
            return (fallback.clone(), fallback_id);
        }
    }
    (primary.clone(), primary_id)
}

/// 字体度量探针：只读**字形覆盖**与 **advance**，不做 MSDF 光栅化、不占图集内存。
///
/// 用途：排版预算与缺字检查（`ui.measure_text`）。渲染路径仍然走 [`FontAtlas`]；
/// 两者的 advance 口径必须一致，因此 `FontAtlas::advance` 直接委托到这里
/// （单测 `probe_matches_atlas_advance` 钉住这条一致性）。
#[derive(Clone, Debug)]
pub struct FontProbe {
    font: FontArc,
    fallback: Option<FontArc>,
}

impl FontProbe {
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        Self::load_with_fallback(bytes, None)
    }

    pub fn load_with_fallback(primary: &[u8], fallback: Option<&[u8]>) -> Result<Self, String> {
        Self::load_with_fallback_and_weight(primary, fallback, DEFAULT_FONT_WEIGHT)
    }

    pub fn load_with_fallback_and_weight(
        primary: &[u8],
        fallback: Option<&[u8]>,
        weight: f32,
    ) -> Result<Self, String> {
        let font = load_font_with_weight(primary, weight, "字体解析失败")?;
        let fallback = fallback
            .map(|bytes| load_font_with_weight(bytes, weight, "后备字体解析失败"))
            .transpose()?;
        Ok(Self { font, fallback })
    }

    fn font_for(&self, character: char) -> (FontArc, ab_glyph::GlyphId) {
        select_font(&self.font, self.fallback.as_ref(), character)
    }

    /// 字体是否覆盖该字符（主字体或后备字体存在非 `.notdef` 字形）。
    /// 空白字符（`\n`/`\t`）不参与缺字判定——它们不需要字形。
    #[must_use]
    pub fn covers(&self, character: char) -> bool {
        if character.is_whitespace() {
            return true;
        }
        self.font_for(character).1.0 != 0
    }

    /// 该字符的 em 相对 advance（`advance / units_per_em`）。
    #[must_use]
    pub fn advance_em(&self, character: char) -> f32 {
        let (font, glyph_id) = self.font_for(character);
        font.h_advance_unscaled(glyph_id) / font.units_per_em().unwrap_or(1000.0)
    }

    /// 该字符在 `size_px` 像素字号下的水平 advance（px）。
    #[must_use]
    pub fn advance_px(&self, character: char, size_px: f32) -> f32 {
        self.advance_em(character) * size_px
    }

    /// Ascent / descent（em 相对值，descent 为负），用于行盒高度。
    #[must_use]
    pub fn ascent_em(&self) -> f32 {
        self.font.ascent_unscaled() / self.font.units_per_em().unwrap_or(1000.0)
    }

    #[must_use]
    pub fn descent_em(&self) -> f32 {
        self.font.descent_unscaled() / self.font.units_per_em().unwrap_or(1000.0)
    }
}

/// 字体文件读取与探针加载（项目字体配置的消费方；`project.toml` 的解析在
/// [`rustlua_infra::project_config`]，本模块不再自带一份键解析器）。
pub mod project_font {
    use super::{DEFAULT_FONT_WEIGHT, FontProbe};
    use rustlua_infra::project_config;
    use std::path::Path;

    /// `project.toml` 的单个键（`key = "value"`，去引号、忽略 BOM 与缩进）。
    #[must_use]
    pub fn read_key(root: &Path, key: &str) -> Option<String> {
        project_config::read_key(root, key)
    }

    /// 主字体路径（相对项目根）：缺省 `assets/fonts/NotoSansSC-VF.ttf`。
    #[must_use]
    pub fn primary_path(root: &Path) -> String {
        read_key(root, "font").unwrap_or_else(|| "assets/fonts/NotoSansSC-VF.ttf".to_owned())
    }

    /// 后备字体路径（相对项目根，可选）。
    #[must_use]
    pub fn fallback_path(root: &Path) -> Option<String> {
        read_key(root, "font_fallback")
    }

    /// 可变字体字重；无效或缺失回退引擎默认值。
    #[must_use]
    pub fn weight(root: &Path) -> f32 {
        read_key(root, "font_weight")
            .and_then(|weight| weight.parse::<f32>().ok())
            .filter(|weight| weight.is_finite() && *weight > 0.0)
            .unwrap_or(DEFAULT_FONT_WEIGHT)
    }

    /// 读取项目字体字节（`(primary, fallback)`；打包编码会先解码）。
    /// 主字体缺失返回 `None`。
    #[must_use]
    pub fn read_bytes(root: &Path) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let decode = |path: &Path| {
            let bytes = read_project_file(path)?;
            rustlua_infra::pack::decode(&bytes).ok()
        };
        let primary = decode(&root.join(primary_path(root)))?;
        let fallback = fallback_path(root).and_then(|font| decode(&root.join(font)));
        Some((primary, fallback))
    }

    /// 加载只读度量探针（缺字检查 / 排版预算用）。
    pub fn load_probe(root: &Path) -> Result<FontProbe, String> {
        let (primary, fallback) = read_bytes(root).ok_or_else(|| "字体文件缺失".to_owned())?;
        FontProbe::load_with_fallback_and_weight(&primary, fallback.as_deref(), weight(root))
    }

    /// 读一个工程文件并解码：走 `rustlua_infra::vfs::read_resource`（**唯一资源加载通道**）。
    ///
    /// Web/微信上工程是打包随宿主资源表走的，`std::fs` 恒失败——字体因此"文件缺失"，
    /// 下游就是 `ui.measure_text` 报"工程字体不可用"。桌面无 provider 时这条通道就是
    /// `std::fs` + 解码，行为不变。
    fn read_project_file(path: &Path) -> Option<Vec<u8>> {
        rustlua_infra::vfs::read_resource(path).ok()
    }
}

#[allow(dead_code)]
fn flatten_quad(a: Point, c: Point, b: Point, units: f32, out: &mut Vec<VectorSegment>) {
    flatten_quad_recursive(a, c, b, units, 0, out);
}

#[allow(dead_code)]
fn flatten_quad_recursive(
    a: Point,
    c: Point,
    b: Point,
    units: f32,
    depth: u32,
    out: &mut Vec<VectorSegment>,
) {
    let flatness = distance_to_line(c, a, b);
    if depth >= 8 || flatness <= 1.5 {
        out.push(VectorSegment {
            start: [a.x / units, -a.y / units],
            end: [b.x / units, -b.y / units],
        });
        return;
    }
    let ac = midpoint(a, c);
    let cb = midpoint(c, b);
    let m = midpoint(ac, cb);
    flatten_quad_recursive(a, ac, m, units, depth + 1, out);
    flatten_quad_recursive(m, cb, b, units, depth + 1, out);
}

#[allow(dead_code)]
fn flatten_cubic(a: Point, c: Point, d: Point, b: Point, units: f32, out: &mut Vec<VectorSegment>) {
    flatten_cubic_recursive(a, c, d, b, units, 0, out);
}

#[allow(dead_code)]
fn flatten_cubic_recursive(
    a: Point,
    c: Point,
    d: Point,
    b: Point,
    units: f32,
    depth: u32,
    out: &mut Vec<VectorSegment>,
) {
    let flatness = distance_to_line(c, a, b).max(distance_to_line(d, a, b));
    if depth >= 9 || flatness <= 1.5 {
        out.push(VectorSegment {
            start: [a.x / units, -a.y / units],
            end: [b.x / units, -b.y / units],
        });
        return;
    }
    let ac = midpoint(a, c);
    let cd = midpoint(c, d);
    let db = midpoint(d, b);
    let accd = midpoint(ac, cd);
    let cddb = midpoint(cd, db);
    let m = midpoint(accd, cddb);
    flatten_cubic_recursive(a, ac, accd, m, units, depth + 1, out);
    flatten_cubic_recursive(m, cddb, db, b, units, depth + 1, out);
}

fn midpoint(a: Point, b: Point) -> Point {
    Point {
        x: (a.x + b.x) * 0.5,
        y: (a.y + b.y) * 0.5,
    }
}

#[allow(dead_code)]
fn distance_to_line(point: Point, a: Point, b: Point) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= f32::EPSILON {
        return ((point.x - a.x).powi(2) + (point.y - a.y).powi(2)).sqrt();
    }
    ((point.x - a.x) * dy - (point.y - a.y) * dx).abs() / length
}

fn flatten_outline(outline: &Outline, units: f32) -> Vec<VectorSegment> {
    let mut out = Vec::with_capacity(outline.curves.len() * 4);
    for curve in &outline.curves {
        match curve {
            OutlineCurve::Line(a, b) => out.push(VectorSegment {
                start: [a.x / units, -a.y / units],
                end: [b.x / units, -b.y / units],
            }),
            OutlineCurve::Quad(a, c, b) => flatten_quad(*a, *c, *b, units, &mut out),
            OutlineCurve::Cubic(a, c, d, b) => flatten_cubic(*a, *c, *d, *b, units, &mut out),
        }
    }
    out
}

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
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FontAtlasError {
    /// The atlas reached its configured maximum capacity.
    #[error("字体图集已满：无法加入字形 {character:?}（{width}×{height}，容量 {capacity}）")]
    AtlasFull {
        character: char,
        width: u32,
        height: u32,
        capacity: u32,
    },
}

/// A growable packed glyph atlas producing one RGBA8 texture.
#[derive(Clone, Debug)]
pub struct FontAtlas {
    font: FontArc,
    /// 后备字体：主字体缺的字形（glyph_id == 0）用它动态生成补充。
    fallback: Option<FontArc>,
    /// `Some(设计像素高度)` = 像素字体（硬边点阵，1 texel = 1 设计像素）；
    /// `None` = 平滑字体，走 MSDF。见 [`detect_pixel_size`]。
    pixel: Option<f32>,
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
        Self::load_with_fallback_and_weight(primary, fallback, DEFAULT_FONT_WEIGHT)
    }

    /// Builds an atlas with an explicit variable-font weight. Static fonts ignore it.
    pub fn load_with_fallback_and_weight(
        primary: &[u8],
        fallback: Option<&[u8]>,
        weight: f32,
    ) -> Result<Self, String> {
        let font = load_font_with_weight(primary, weight, "字体解析失败")?;
        let fallback = fallback
            .map(|bytes| load_font_with_weight(bytes, weight, "后备字体解析失败"))
            .transpose()?;
        Ok(Self::new(font, fallback))
    }

    fn new(font: FontArc, fallback: Option<FontArc>) -> Self {
        let size = INITIAL_ATLAS_SIZE;
        let pixel = detect_pixel_size(&font);
        Self {
            font,
            fallback,
            pixel,
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

    /// 字形栅格的 em 尺寸（px/em）：MSDF 是 [`RASTER_SCALE`]，像素字体是它的设计
    /// 像素高度（1 texel = 1 设计像素）。布局侧的 `tex_scale` 必须由这个值算，
    /// 不能写死 `RASTER_SCALE`（写死会让点阵字形显示成 8 倍大小）。
    #[must_use]
    pub fn raster_scale(&self) -> f32 {
        self.pixel.unwrap_or(RASTER_SCALE)
    }

    /// `true` = 主字体被判为像素字体（硬边点阵），字形单元按 1 texel/设计像素栅格，
    /// 文字 mesh 走 `sdf=2` 阈值档 + Nearest 采样。
    #[must_use]
    pub const fn is_pixel_font(&self) -> bool {
        self.pixel.is_some()
    }

    /// 像素字体的设计像素高度（`None` = 不是像素字体）。
    #[must_use]
    pub const fn pixel_design_px(&self) -> Option<f32> {
        self.pixel
    }

    /// 字形所属字体：主字体有该字用主字体，否则用后备字体，再否则主字体（.notdef）。
    fn font_for(&self, character: char) -> (FontArc, ab_glyph::GlyphId) {
        select_font(&self.font, self.fallback.as_ref(), character)
    }

    /// 字体是否覆盖该字符（与 [`FontProbe::covers`] 同一判定）。
    #[must_use]
    pub fn covers(&self, character: char) -> bool {
        if character.is_whitespace() {
            return true;
        }
        self.font_for(character).1.0 != 0
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

    /// Horizontal advance in px at [`FontAtlas::raster_scale`] without rasterizing
    /// the glyph (used for whitespace, which has no outline / SDF cell).
    pub fn advance(&self, character: char) -> f32 {
        let (font, glyph_id) = self.font_for(character);
        font.h_advance_unscaled(glyph_id) / font.units_per_em().unwrap_or(1000.0)
            * self.raster_scale()
    }

    /// 只读度量探针（同一字体/后备字体，不复制字节、不光栅化）。
    #[must_use]
    pub fn probe(&self) -> FontProbe {
        FontProbe {
            font: self.font.clone(),
            fallback: self.fallback.clone(),
        }
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
        // Existing pixel positions are preserved, but normalized UVs are tied
        // to the atlas size. Re-scale them or cached glyphs sample the wrong
        // quadrant after the first growth.
        let width_scale = old_width as f32 / new_width as f32;
        let height_scale = old_height as f32 / new_height as f32;
        for metrics in self.cached.values_mut() {
            metrics.uv[0] *= width_scale;
            metrics.uv[1] *= height_scale;
            metrics.uv[2] *= width_scale;
            metrics.uv[3] *= height_scale;
        }
        self.version = self.version.wrapping_add(1);
        self.dirty = true;
        true
    }

    /// Returns per-glyph metrics, rasterizing the MSDF on first use (packed).
    pub fn try_glyph_metrics(&mut self, character: char) -> Result<GlyphMetrics, FontAtlasError> {
        if let Some(metrics) = self.cached.get(&character) {
            return Ok(*metrics);
        }
        let glyph = rasterize_glyph(&self.font, &self.fallback, character, self.pixel);
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
        let pixel = self.pixel;
        // Phase A（并行）：光栅化全部字形，不触碰图集状态。按核数分块，避免
        // 每字形一线程的调度开销（CJK 预热数百字形时明显）。
        //
        // **wasm 上必须串行**：`std::thread::scope` 在 `wasm32-unknown-unknown` 上直接 panic
        // （`std/thread/scoped.rs` 的 `unreachable`）——wasm 里没有任何线程，连"起不来就降级"
        // 的机会都没有。并行只影响建图集的耗时，不改变字形内容与顺序（Phase B 仍按 chars
        // 顺序打包，布局逐字节不变）。
        #[cfg(target_arch = "wasm32")]
        let glyphs: Vec<PrewarmedGlyph> = unique
            .iter()
            .map(|&ch| rasterize_glyph(&font, &fallback, ch, pixel))
            .collect();
        #[cfg(not(target_arch = "wasm32"))]
        let glyphs: Vec<PrewarmedGlyph> = {
            let workers = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(unique.len())
                .max(1);
            let chunk = unique.len().div_ceil(workers);
            std::thread::scope(|scope| {
                let mut handles: Vec<std::thread::ScopedJoinHandle<'_, Vec<PrewarmedGlyph>>> =
                    Vec::with_capacity(workers);
                for block in unique.chunks(chunk) {
                    let font = font.clone();
                    let fallback = fallback.clone();
                    let block = block.to_vec();
                    handles.push(scope.spawn(move || {
                        block
                            .into_iter()
                            .map(|ch| rasterize_glyph(&font, &fallback, ch, pixel))
                            .collect()
                    }));
                }
                // 按块序 join 后扁平化，保持 chars 原始顺序（打包布局与逐字预热一致）。
                handles
                    .into_iter()
                    .flat_map(|handle| handle.join().ok().into_iter().flatten())
                    .collect()
            })
        };
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

fn load_font_with_weight(bytes: &[u8], weight: f32, error_prefix: &str) -> Result<FontArc, String> {
    let mut font = FontVec::try_from_vec(bytes.to_vec())
        .map_err(|error| format!("{error_prefix}: {error}"))?;
    if weight.is_finite() && weight > 0.0 {
        font.set_variation(b"wght", weight);
    }
    Ok(FontArc::from(font))
}

/// Shelf 打包器：把不同尺寸的字形矩形密排进图集。字形只增不删（缓存后永驻），
/// 无需释放；空间不足由调用方增长画布（增长向右下扩展，已有位置不变）。
#[derive(Clone, Debug)]
pub(crate) struct Packer {
    width: u32,
    height: u32,
    /// 打开的 shelf：(y, shelf_height, 已用 x)。
    shelves: Vec<(u32, u32, u32)>,
    next_y: u32,
}

impl Packer {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Packer {
            width,
            height,
            shelves: Vec::new(),
            next_y: 0,
        }
    }

    pub(crate) fn set_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// 放置 `w×h` 矩形，返回左上角坐标；放不下返回 `None`。
    pub(crate) fn pack(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
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

/// 解析字形所属字体（主字体缺失 → 后备字体 → 主字体 .notdef）并光栅化。
/// `pixel = Some(设计像素高度)` 走硬边点阵（1 texel = 1 设计像素），`None` 走 MSDF。
/// 与 `FontAtlas::font_for` 选择逻辑一致；只读字体数据，可跨线程调用。
fn rasterize_glyph(
    font: &FontArc,
    fallback: &Option<FontArc>,
    character: char,
    pixel: Option<f32>,
) -> PrewarmedGlyph {
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
    let scale = pixel.unwrap_or(RASTER_SCALE);
    let advance = face.h_advance_unscaled(glyph_id) / units_per_em * scale;
    let outline = face.outline(glyph_id);
    let (rect_w, rect_h, rgba, cell_rect, bounds) = match (pixel, outline) {
        (Some(design_px), Some(outline)) => {
            let (rect_w, rect_h) = pixel_rect_size(&outline, units_per_em, design_px);
            let (rgba, cell_rect, bounds) =
                rasterize_pixel(&outline, rect_w, rect_h, units_per_em, design_px);
            (rect_w, rect_h, rgba, cell_rect, bounds)
        }
        (None, Some(outline)) => {
            let (rect_w, rect_h) = glyph_rect_size(&outline, units_per_em);
            let (rgba, cell_rect, bounds) = rasterize_msdf(&outline, rect_w, rect_h, units_per_em);
            (rect_w, rect_h, rgba, cell_rect, bounds)
        }
        (Some(design_px), None) => {
            let side = pixel_placeholder_size(design_px);
            let (rgba, cell_rect, bounds) = placeholder_pixel(side);
            (side, side, rgba, cell_rect, bounds)
        }
        (None, None) => {
            let (rect_w, rect_h) = placeholder_rect_size();
            let (rgba, cell_rect, bounds) = placeholder_msdf(rect_w, rect_h);
            (rect_w, rect_h, rgba, cell_rect, bounds)
        }
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
    let min_x = b.min.x.min(b.max.x);
    let max_x = b.min.x.max(b.max.x);
    let min_y = b.min.y.min(b.max.y);
    let max_y = b.min.y.max(b.max.y);
    let w = ((max_x - min_x) as f64 * scale).ceil() as i64 + PAD as i64 * 2;
    let h = ((max_y - min_y) as f64 * scale).ceil() as i64 + PAD as i64 * 2;
    (w.max(1) as u32, h.max(1) as u32)
}

/// 占位字形（方框）的打包矩形尺寸。
fn placeholder_rect_size() -> (u32, u32) {
    let side = RASTER_SCALE as u32 + PAD as u32 * 2;
    (side, side)
}

/// 像素字体缺字占位方框的边长（设计像素，含 gutter）。
fn pixel_placeholder_size(design_px: f32) -> u32 {
    (design_px.round().max(1.0) as u32) + PIXEL_PAD * 2
}

/// 点阵字形在单元矩形里的整数设备像素包围盒（相对笔，y 向下）。
///
/// 不复用 `rasterize_msdf` 的"字形居中 + 非整数偏移"摆位：居中偏移本身是小数
/// （`(rect - 字形宽)/2`），点阵一旦落在半 texel 上就不再是点阵。这里把字形贴在
/// `(PIXEL_PAD, PIXEL_PAD)`，包围盒取栅格器自己的 `px_bounds`（保守整数盒）。
fn pixel_px_bounds(outline: &Outline, units_per_em: f32, design_px: f32) -> ab_glyph::Rect {
    let factor = PxScaleFactor {
        horizontal: design_px / units_per_em,
        vertical: design_px / units_per_em,
    };
    outline.px_bounds(factor, ab_glyph::point(0.0, 0.0))
}

/// 像素字体的打包矩形 = 整数包围盒 + `2*PIXEL_PAD` gutter。
fn pixel_rect_size(outline: &Outline, units_per_em: f32, design_px: f32) -> (u32, u32) {
    let b = pixel_px_bounds(outline, units_per_em, design_px);
    (
        (b.max.x - b.min.x).max(0.0) as u32 + PIXEL_PAD * 2,
        (b.max.y - b.min.y).max(0.0) as u32 + PIXEL_PAD * 2,
    )
}

/// Rasterizes a pixel-font glyph as a **hard-edged 1-bit block**: one texel per
/// design pixel, coverage quantized at 0.5, value written to all of RGB (A=255)
/// so the existing atlas format and the shader's `sdf == 2` threshold branch
/// (`median(rgb) >= 0.5`) apply unchanged.
///
/// 覆盖阈值不会把本就该亮的像素压暗：实测 Fusion Pixel 12px 在 12 px/em 栅格下
/// 471 个工程用字的覆盖值**只有 0.0 与 1.0 两档**（最大偏差 0.000000，见
/// `tasks/done/M72-03`），同一份字体在 `RASTER_SCALE=96` 下有 5876 个部分覆盖像素。
fn rasterize_pixel(
    outline: &Outline,
    width: u32,
    height: u32,
    units_per_em: f32,
    design_px: f32,
) -> (Vec<u8>, [f32; 4], [f32; 4]) {
    let factor = PxScaleFactor {
        horizontal: design_px / units_per_em,
        vertical: design_px / units_per_em,
    };
    let origin = outline.px_bounds(factor, ab_glyph::point(0.0, 0.0));
    let w = width as usize;
    let h = height as usize;
    let mut pixels = vec![0u8; w * h * 4];
    // alpha 整块写满（与 MSDF 档同口径：A 恒 255，信息只在 RGB 三通道）。
    // 否则 gutter 会留 A=0，图集上传后"alpha 直方图"就混进了与点阵无关的一档。
    for pixel in pixels.chunks_mut(4) {
        pixel[3] = 255;
    }
    // 把字形整数包围盒的左上角落在单元 (PIXEL_PAD, PIXEL_PAD) 上。
    let position = ab_glyph::point(
        PIXEL_PAD as f32 - origin.min.x,
        PIXEL_PAD as f32 - origin.min.y,
    );
    let outlined = OutlinedGlyph::new(
        Glyph {
            id: ab_glyph::GlyphId(0),
            scale: design_px.into(),
            position,
        },
        outline.clone(),
        factor,
    );
    let block = outlined.px_bounds();
    let block_w = block.max.x - block.min.x;
    let block_h = block.max.y - block.min.y;
    outlined.draw(|x, y, coverage| {
        // 回调给的 (x,y) 是块内索引，落到单元坐标要加块的左上角（即 PIXEL_PAD）。
        // 越界只可能是浮点误差（包围盒本来就是保守整数盒），丢弃而不是扩容，
        // 保证矩形尺寸与打包器一致。
        let cx = block.min.x.round() + x as f32;
        let cy = block.min.y.round() + y as f32;
        if cx < 0.0 || cy < 0.0 || cx >= width as f32 || cy >= height as f32 {
            return;
        }
        let value = if coverage >= 0.5 { 255 } else { 0 };
        let off = ((cy as usize) * w + cx as usize) * 4;
        pixels[off..off + 3].fill(value);
    });
    let cell_rect = [
        PIXEL_PAD as f32,
        PIXEL_PAD as f32,
        PIXEL_PAD as f32 + block_w,
        PIXEL_PAD as f32 + block_h,
    ];
    // 相对笔的设备像素包围盒：position 只是单元内摆位，几何位置来自原点盒。
    let bounds = [
        origin.min.x,
        origin.min.y,
        origin.min.x + block_w,
        origin.min.y + block_h,
    ];
    (pixels, cell_rect, bounds)
}

/// 缺失字形（点阵档）：画一个 1 texel 粗的空心方框。
fn placeholder_pixel(side: u32) -> (Vec<u8>, [f32; 4], [f32; 4]) {
    let w = side as usize;
    let h = side as usize;
    let mut pixels = vec![0u8; w * h * 4];
    for pixel in pixels.chunks_mut(4) {
        pixel[3] = 255;
    }
    let inner = side.saturating_sub(PIXEL_PAD * 2 + 1);
    let write = |pixels: &mut Vec<u8>, x: u32, y: u32| {
        let off = ((y as usize) * w + x as usize) * 4;
        pixels[off] = 255;
        pixels[off + 1] = 255;
        pixels[off + 2] = 255;
        pixels[off + 3] = 255;
    };
    for index in 0..side {
        write(&mut pixels, index, 0);
        write(&mut pixels, index, h as u32 - 1);
        write(&mut pixels, 0, index);
        write(&mut pixels, w as u32 - 1, index);
    }
    let rect = [
        PIXEL_PAD as f32,
        PIXEL_PAD as f32,
        PIXEL_PAD as f32 + inner as f32 + 1.0,
        PIXEL_PAD as f32 + inner as f32 + 1.0,
    ];
    (pixels, rect, rect)
}

/// 判定主字体是不是**像素字体**，并返回它的设计像素高度（em 尺寸）。
///
/// 判据是实测的，不是猜名字也不是取轮廓坐标 GCD：对 8..=24 的每个整数 em 尺寸，
/// 把探针字形按 `1 texel = 1 em/d` 栅格化，要求**每个像素的覆盖值恰好是 0 或 1**
/// （量化容差 [`COVERAGE_QUANTUM`]，ab_glyph 栅格器按 1/64 量化）。第一个满足的
/// 尺寸就是该字体的设计像素高度。
///
/// 为什么不能用"轮廓坐标 GCD"：实测 Fusion Pixel 12px 的 470 个字形里只有 429 个
/// 的坐标 GCD 是 100（=1 设计像素），35 个带非整数坐标、6 个落在 50/25/10/2 的
/// 更细网格上——GCD 的众数能选对，但个别字形会把整体 GCD 拉到 1，判定直接失效。
///
/// 实测对照（同一条判据）：
/// - Fusion Pixel 12px Monospaced zhHans（unitsPerEm 1200、36417 字形、无 fvar）：
///   8→2378、10→4456、**12→0**、16→2417、20→5583、24→2488 个部分覆盖像素 → 判为像素字体，d=12。
/// - Noto Sans SC VF（unitsPerEm 1000）：6..=192 每个尺寸都有 ≥25970 个部分覆盖
///   像素 → 判不出来 → 走 MSDF（现状不变）。
/// - 上面那份 12px 的子集（1605 字形，`国`/`鼎` 不在收字内）：12 档仍全二值、总覆盖
///   188 texel（6 个字形）→ 判 d=12。绝对样本下限（曾经的 200）在这里会判不过而滑到
///   24 档，所以样本量按 em 面积归一 + 探针从字体自己的 cmap 补（见 [`probe_glyphs`]）。
fn detect_pixel_size(font: &FontArc) -> Option<f32> {
    let units = font.units_per_em()?;
    if !units.is_finite() || units <= 0.0 {
        return None;
    }
    let probes = probe_glyphs(font);
    let mut design_px = PIXEL_PROBE_MIN_PX;
    while design_px <= PIXEL_PROBE_MAX_PX {
        if pixel_size_is_binary(font, &probes, units, design_px) {
            return Some(design_px);
        }
        design_px += 1.0;
    }
    None
}

/// 探针字符集：优先固定表（密集 CJK + 半角 + 全角标点，对非整数网格最敏感）；子集字体
/// 常缺其中几个字，就从这份字体自己的 cmap 里补 —— 先补 CJK，仍不足再补任意非空白字符。
/// 不补的话样本量会被子集化削掉（实测同一款字体全量判 12、子集判 24），开发形态与发布
/// 形态就会一边点阵一边平滑。
fn probe_glyphs(font: &FontArc) -> Vec<char> {
    let want = PIXEL_PROBE_CHARS.len().max(PIXEL_PROBE_MIN_GLYPHS);
    let mut picked: Vec<char> = PIXEL_PROBE_CHARS
        .iter()
        .copied()
        .filter(|character| font.glyph_id(*character).0 != 0)
        .collect();
    if picked.len() >= want {
        return picked;
    }
    let mut cjk: Vec<char> = Vec::new();
    let mut other: Vec<char> = Vec::new();
    for (glyph_id, character) in font.codepoint_ids() {
        if glyph_id.0 == 0 || character.is_whitespace() || picked.contains(&character) {
            continue;
        }
        let code = character as u32;
        if (0x4E00..=0x9FFF).contains(&code) {
            if cjk.len() < want {
                cjk.push(character);
            }
        } else if other.len() < want {
            other.push(character);
        }
        if cjk.len() >= want && other.len() >= want {
            break;
        }
    }
    picked.extend(cjk.into_iter().chain(other).take(want - picked.len()));
    picked
}

/// 覆盖量化容差：ab_glyph 栅格器输出 1/64 的整数倍，实测最小的"真软像素"是
/// 3/64 = 0.046875，所以任何落在 `(0, 1/64)` 的容差都判得一样准。
const COVERAGE_QUANTUM: f32 = 1.0 / 64.0;

/// `design_px` 档下探针字形是否全部硬边二值（且样本量够，避免空字形蒙混过关）。
///
/// 样本量按 em 面积归一（见 [`PIXEL_PROBE_MIN_FILL_DIVISOR`] 处的实测说明），这样同一份
/// 字体的全量与子集（探针字符可能被裁掉几个）会判出同一个设计像素高度。
fn pixel_size_is_binary(font: &FontArc, probes: &[char], units: f32, design_px: f32) -> bool {
    let factor = PxScaleFactor {
        horizontal: design_px / units,
        vertical: design_px / units,
    };
    let mut sampled = 0usize;
    let mut covered = 0usize;
    for character in probes {
        let glyph_id = font.glyph_id(*character);
        if glyph_id.0 == 0 {
            continue;
        }
        let Some(outline) = font.outline(glyph_id) else {
            continue;
        };
        if outline.curves.is_empty() {
            continue;
        }
        let outlined = OutlinedGlyph::new(
            Glyph {
                id: glyph_id,
                scale: design_px.into(),
                position: ab_glyph::point(0.0, 0.0),
            },
            outline,
            factor,
        );
        let mut glyph_covered = 0usize;
        let mut soft = false;
        outlined.draw(|_x, _y, coverage| {
            if coverage > COVERAGE_QUANTUM && coverage < 1.0 - COVERAGE_QUANTUM {
                soft = true;
            }
            if coverage > 0.5 {
                glyph_covered += 1;
            }
        });
        if soft {
            return false;
        }
        sampled += 1;
        covered += glyph_covered;
    }
    sampled >= PIXEL_PROBE_MIN_GLYPHS
        && covered * PIXEL_PROBE_MIN_FILL_DIVISOR >= sampled * (design_px * design_px) as usize
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
    let min_x = b.min.x.min(b.max.x);
    let max_x = b.min.x.max(b.max.x);
    let min_y = b.min.y.min(b.max.y);
    let max_y = b.min.y.max(b.max.y);
    let left_px = min_x as f64 * scale;
    let right_px = max_x as f64 * scale;
    let top_up = max_y as f64 * scale;
    let bottom_up = min_y as f64 * scale;
    let wf = width as f64;
    let hf = height as f64;
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

    let rgba = sdf_rect_from_shape(&mut shape, width, height);
    let cell_y0 = hf - (top_up + oy);
    let cell_y1 = hf - (bottom_up + oy);
    let cell_rect = [
        (left_px + ox) as f32,
        cell_y0.min(cell_y1) as f32,
        (right_px + ox) as f32,
        cell_y0.max(cell_y1) as f32,
    ];
    let bounds_y0 = (-top_up).min(-bottom_up);
    let bounds_y1 = (-top_up).max(-bottom_up);
    let bounds = [
        left_px as f32,
        bounds_y0 as f32,
        right_px as f32,
        bounds_y1 as f32,
    ];
    (rgba, cell_rect, bounds)
}

/// 从已就绪的 Shape 生成单通道 SDF 像素（width×height×4 RGBA，行 0 = 顶部）。
/// 距离值复制到 RGB 三通道，保持现有 atlas 格式；shader 使用 R 通道解码。
fn sdf_rect_from_shape(shape: &mut Shape, width: u32, height: u32) -> Vec<u8> {
    // 只做 normalize（单边轮廓三分 + 尖角 deconverge）。
    //
    // 这里刻意不调用 `shape.orient_contours()`：bymsdfgen 的
    // `orient_contours` 依赖扫描线奇偶判定轮廓方向，而它缺少 msdfgen 中
    // Skia 自相交消解（FULL_PREPROCESS）这一前置步骤（见其 README
    // "Parity notes"）。CJK 字形普遍存在自相交/重叠轮廓，扫描线奇偶会误判，
    // 把本已正确的外轮廓反转，导致 SDF 出现大面积实心块（笔画丢失、孔洞被填）。
    //
    // TTF/CFF 字体的轮廓本身遵循各自的 winding 约定（外轮廓一方向、孔洞反向），
    // `generate_sdf` 的 `OverlappingContourCombiner` 正是按每条轮廓的
    // `winding()` 符号区分内外，因此直接沿用字体原生 winding 即可，无需再修正。
    shape.normalize();
    let w = width as usize;
    let h = height as usize;
    let mut sdf: Bitmap<f32, 1> = Bitmap::new(w, h);
    let transformation = SdfTransformation::new(
        Projection::default(),
        DistanceMapping::from_range(Range::symmetric(PAD as f64)),
    );
    generate_sdf(
        &mut sdf,
        shape,
        &transformation,
        &GeneratorConfig::default(),
    );
    let mut pixels = vec![0u8; w * h * 4];
    for by in 0..h {
        let ay = h - 1 - by;
        for bx in 0..w {
            let distance = sdf.pixel(bx, by)[0].clamp(0.0, 1.0);
            let value = (distance * 255.0) as u8;
            let off = (ay * w + bx) * 4;
            pixels[off] = value;
            pixels[off + 1] = value;
            pixels[off + 2] = value;
            pixels[off + 3] = 255;
        }
    }
    pixels
}

/// 缺失字形：画一个占位方框，同样走单通道 SDF。
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
    let rgba = sdf_rect_from_shape(&mut shape, width, height);
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
    fn probe_covers_chars_and_matches_atlas_advance() {
        let bytes = test_font();
        let probe = FontProbe::load(&bytes).unwrap();
        assert!(probe.covers('江'), "CJK 应被覆盖");
        assert!(probe.covers('A'), "ASCII 应被覆盖");
        assert!(probe.covers('\n'), "换行不需要字形，视为覆盖");
        assert!(!probe.covers('🙂'), "emoji 不该被 Noto Sans SC 覆盖");
        // CJK 满宽 vs ASCII 半宽：这正是"按字数裁剪"会算错的原因。
        let cjk = probe.advance_em('江');
        let ascii = probe.advance_em('A');
        assert!(cjk > ascii * 1.5, "cjk={cjk} ascii={ascii}");
        // 度量探针与渲染图集的 advance 必须逐字一致（否则排版预算不可信）。
        let atlas = FontAtlas::load(&bytes).unwrap();
        for character in ['A', 'W', 'i', '江', '，', ' '] {
            assert_eq!(
                probe.advance_px(character, RASTER_SCALE),
                atlas.advance(character),
                "{character:?} advance 不一致"
            );
        }
        assert!(atlas.probe().covers('江'));
        assert!(!atlas.covers('🙂'));
    }

    /// 测试字体：Fusion Pixel 12px Monospaced zhHans（真点阵字体，上游全量文件）。
    fn pixel_font() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/editor/fonts/FusionPixel-12px-Monospaced-zhHans.ttf");
        std::fs::read(&path)
            .unwrap_or_else(|error| panic!("测试字体缺失或不可读 {}: {error}", path.display()))
    }

    /// 判别实测：同一条判据（见 `detect_pixel_size`）下点阵字体给出设计高度 12，
    /// 平滑字体判不出来。矢量缓存共用这个判定，渲染层据此放弃矢量档。
    #[test]
    fn detects_pixel_font_and_leaves_smooth_font_on_msdf() {
        let atlas = FontAtlas::load(&pixel_font()).unwrap();
        assert!(atlas.is_pixel_font());
        assert_eq!(atlas.pixel_design_px(), Some(12.0));
        assert_eq!(atlas.raster_scale(), 12.0);
        assert!(
            FontVectorAtlas::load(&pixel_font())
                .unwrap()
                .is_pixel_font()
        );

        let smooth = FontAtlas::load(&test_font()).unwrap();
        assert!(!smooth.is_pixel_font());
        assert_eq!(smooth.raster_scale(), RASTER_SCALE);
        assert_eq!(smooth.pixel_design_px(), None);
        assert!(!FontVectorAtlas::load(&test_font()).unwrap().is_pixel_font());
    }

    /// 点阵档的每一 texel 只能全亮或全暗（RGB ∈ {0,255}），且几何量全是整数——
    /// 布局侧的整数吸附只有在本就是整数时才算生效（非整数即丢档，见 M72-03 反面对照）。
    #[test]
    fn pixel_glyph_raster_is_binary_and_geometry_integral() {
        let font =
            load_font_with_weight(&pixel_font(), DEFAULT_FONT_WEIGHT, "字体解析失败").unwrap();
        for character in ['山', '庄', '炼', '药', 'A', '8'] {
            let glyph = rasterize_glyph(&font, &None, character, Some(12.0));
            let expected = (glyph.width * glyph.height * 4) as usize;
            assert_eq!(glyph.rgba.len(), expected, "{character:?} 矩形尺寸不符");
            assert!(
                glyph.rgba.iter().all(|value| *value == 0 || *value == 255),
                "{character:?} 点阵档出现灰阶"
            );
            assert!(
                glyph.rgba.chunks(4).all(|pixel| pixel[3] == 255),
                "{character:?} alpha 未写满"
            );
            let lit = glyph.rgba.chunks(4).filter(|pixel| pixel[0] == 255).count();
            assert!(lit > 10, "{character:?} 亮点过少：{lit}");
            let non_integral = glyph
                .cell_rect
                .iter()
                .chain(glyph.bounds.iter())
                .filter(|value| value.fract() != 0.0)
                .count();
            assert_eq!(non_integral, 0, "{character:?} 几何量非整数");
            assert_eq!(
                glyph.cell_rect,
                [
                    1.0,
                    1.0,
                    glyph.width as f32 - 1.0,
                    glyph.height as f32 - 1.0
                ],
                "{character:?} 点阵块没有居中在 1 texel gutter 内"
            );
            assert!(
                glyph.advance.fract() == 0.0,
                "{character:?} advance 非整数：{}",
                glyph.advance
            );
        }
    }

    /// 同一字体在 `RASTER_SCALE` 档栅格化必然产生灰阶：证明上面那条「全 0/255」
    /// 是在测点阵档，而不是在测字体本身有多简单。
    #[test]
    fn same_pixel_font_is_not_binary_at_msdf_scale() {
        let font =
            load_font_with_weight(&pixel_font(), DEFAULT_FONT_WEIGHT, "字体解析失败").unwrap();
        let glyph = rasterize_glyph(&font, &None, '山', None);
        let greys = glyph
            .rgba
            .chunks(4)
            .filter(|pixel| pixel[0] != 0 && pixel[0] != 255)
            .count();
        assert!(
            greys > 100,
            "MSDF 档下应有大量中间值（实测对照），实际 {greys}"
        );
    }

    /// 同一款像素字体的**子集**（发布形态）必须判出与全量相同的设计像素高度。
    ///
    /// 这条钉的是探针样本量：子集常把固定探针表里的密集 CJK 裁掉几个，绝对 texel 数下限
    /// 就会在真正的 12px 档判不过、滑到下一个二值档（实测 24px = 设计像素的 2 倍），
    /// 于是开发形态点阵、发布形态按 2 倍栅格画，两者字号看着一样但笔画厚度差一倍。
    #[test]
    fn subset_of_a_pixel_font_judges_the_same_design_px() {
        let full = pixel_font();
        // 探针表里的四个密字（国露鼎藏）一个都不收，把样本量压到最难看的情形。
        let subset = crate::font_subset::subset_ttf(&full, "山庄炼药甲乙丙丁戊己").expect("子集化");
        let subset_atlas = FontAtlas::load(&subset).unwrap();
        assert_eq!(
            subset_atlas.pixel_design_px(),
            FontAtlas::load(&full).unwrap().pixel_design_px(),
            "子集与全量判出不同的点阵档"
        );
        assert_eq!(subset_atlas.pixel_design_px(), Some(12.0));
        // 反面对照：补不出足够样本时宁可判"不是像素字"，也不许猜一个错的档。
        let starved = crate::font_subset::subset_ttf(&full, "，。").expect("子集化");
        assert_eq!(
            FontAtlas::load(&starved).unwrap().pixel_design_px(),
            None,
            "探针样本不足时应退回 MSDF 档，而不是猜一个设计像素高度"
        );
    }

    #[test]
    fn vector_cjk_outline_is_non_empty() {
        let mut atlas = FontVectorAtlas::load(&test_font()).unwrap();
        let glyph = atlas.glyph('俯').expect("CJK outline");
        assert!(!glyph.segments.is_empty());
        assert!(glyph.segments.iter().all(|segment| {
            segment
                .start
                .iter()
                .chain(segment.end.iter())
                .all(|value| value.is_finite())
        }));
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

    /// CJK variable fonts commonly default to Thin; project loading must pin a
    /// readable UI weight before layout metrics or MSDF outlines are cached.
    #[test]
    fn variable_font_weight_changes_glyph_metrics() {
        let bytes = test_font();
        let mut thin = FontAtlas::load_with_fallback_and_weight(&bytes, None, 100.0).unwrap();
        let mut regular =
            FontAtlas::load_with_fallback_and_weight(&bytes, None, DEFAULT_FONT_WEIGHT).unwrap();
        let thin_metrics = thin.try_glyph_metrics('I').unwrap();
        let regular_metrics = regular.try_glyph_metrics('I').unwrap();
        assert_ne!(
            thin_metrics.advance, regular_metrics.advance,
            "variable weight should change glyph metrics"
        );
    }

    /// 微软雅黑（msyh.ttc，TTF 集合）应能被 FontArc 解析（face 0），且 MSDF
    /// 图集对复杂 CJK 字形正常生成（角外部 median<128、存在内部像素 median>140）。
    #[test]
    fn msdf_generates_for_msyh_ttc_collection() {
        let path = rustlua_infra::fixture_games_dir().join("ui_demo/assets/fonts/msyh.ttc");
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
        let path = rustlua_infra::fixture_games_dir().join("ui_demo/assets/fonts/msyh.ttc");
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
        let new_metrics = atlas.try_glyph_metrics('俯').unwrap();
        let old_uv_texels = [
            first.uv[0] * old_width as f32,
            first.uv[1] * old_height as f32,
            first.uv[2] * old_width as f32,
            first.uv[3] * old_height as f32,
        ];
        let new_uv_texels = [
            new_metrics.uv[0] * atlas.width() as f32,
            new_metrics.uv[1] * atlas.height() as f32,
            new_metrics.uv[2] * atlas.width() as f32,
            new_metrics.uv[3] * atlas.height() as f32,
        ];
        for (old, new) in old_uv_texels.iter().zip(new_uv_texels.iter()) {
            assert!(
                (old - new).abs() < 0.01,
                "growth must preserve absolute glyph texel UVs: {old} != {new}"
            );
        }
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
