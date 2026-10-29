//! 整物理 ppem 的 TrueType hinting 光栅化，产出 8-bit 覆盖率图集。
//!
//! 与 [`crate::font::FontVectorAtlas`]（解析轮廓、片元着色器算覆盖率）和
//! [`crate::font::FontAtlas`]（MSDF）并列：这里让字体自己的 `fpgm`/`prep`/`cvt `
//! 字节码在**整数 ppem** 上做网格适配，笔画宽度与笔位被吸附到整像素——这正是
//! 原生 GDI/DirectWrite 在 11~14px 下比未 hinting 轮廓锐利的原因（实测 11px
//! CJK 的实心像素占比 19% → 46%，墨量几乎不变）。
//!
//! 代价是**只在 1:1 整数放大下锐利**：位图被任意缩放就又糊了。所以调用方必须
//! 按物理像素排版（`TextLayout::build_hinted`），把 ppem 取成
//! `round(设计字号 × 设计→物理比例)`，而不是设计字号本身。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source};

use crate::font::Packer;

/// 图集起始边长（2 的幂，按需倍增）。
const INITIAL_ATLAS_SIZE: u32 = 256;
/// 图集边长上限：11px 的 CJK 字形约 12×12，2048² 能装两万多字。
const MAX_ATLAS_SIZE: u32 = 2048;

/// 一个已 hinting 光栅化的字形在覆盖率图集里的位置与度量。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HintedGlyph {
    /// 归一化 UV 矩形 `[u_min, v_min, u_max, v_max]`，覆盖整个字形位图（无 padding）。
    pub uv: [f32; 4],
    /// 相对笔位/基线的像素包围盒 `[left, top, right, bottom]`（y 向下）。
    /// 来自 swash 的 `placement`，已经过 hinting 的整像素吸附。
    pub bounds: [f32; 4],
    /// 该字形的水平推进（px，整数化前的原值）。
    pub advance: f32,
}

/// 一种字体的整数 ppem 覆盖率图集。每个条目是一张 8-bit 掩膜，
/// 上传到 `Rgba8Unorm` 纹理后四个通道都写同一个覆盖率（着色器读 `.r`）。
pub struct HintedAtlas {
    primary: Arc<Vec<u8>>,
    fallback: Option<Arc<Vec<u8>>>,
    ppem: u16,
    hinted: bool,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    ascent: f32,
    descent: f32,
    cached: HashMap<char, HintedGlyph>,
    missing: HashSet<char>,
    packer: Packer,
    context: ScaleContext,
    version: u64,
    dirty: bool,
}

impl HintedAtlas {
    /// 用字体字节构建 `ppem` 像素/em 的图集（无后备字体）。
    pub fn load(bytes: &[u8], ppem: u16) -> Result<Self, String> {
        Self::load_with_fallback(bytes, None, ppem)
    }

    /// 主字体 + 可选后备字体（主字体缺字形时用后备字体 hinting）。
    pub fn load_with_fallback(
        primary: &[u8],
        fallback: Option<&[u8]>,
        ppem: u16,
    ) -> Result<Self, String> {
        Self::new(primary.to_vec(), fallback.map(<[u8]>::to_vec), ppem)
    }

    pub fn new(primary: Vec<u8>, fallback: Option<Vec<u8>>, ppem: u16) -> Result<Self, String> {
        Self::from_shared(Arc::new(primary), fallback.map(Arc::new), ppem)
    }

    /// 共享字体字节的构造入口：同一字体在多个 ppem 下各建一张图集时，字节只留
    /// 一份（24MB 的 CJK 字体若每个 ppem 各拷一份，拖窗口改尺寸就能吃光内存）。
    pub fn from_shared(
        primary: Arc<Vec<u8>>,
        fallback: Option<Arc<Vec<u8>>>,
        ppem: u16,
    ) -> Result<Self, String> {
        let ppem = ppem.max(1);
        let hinted = has_hinting_tables(&primary);
        let font = FontRef::from_index(&primary, 0).ok_or_else(|| "字体解析失败".to_owned())?;
        let metrics = font.metrics(&[]).scale(f32::from(ppem));
        if let Some(bytes) = &fallback {
            FontRef::from_index(bytes, 0).ok_or_else(|| "后备字体解析失败".to_owned())?;
        }
        Ok(Self {
            primary,
            fallback,
            ppem,
            hinted,
            width: INITIAL_ATLAS_SIZE,
            height: INITIAL_ATLAS_SIZE,
            pixels: vec![0; (INITIAL_ATLAS_SIZE * INITIAL_ATLAS_SIZE * 4) as usize],
            ascent: metrics.ascent,
            descent: metrics.descent,
            cached: HashMap::new(),
            missing: HashSet::new(),
            packer: Packer::new(INITIAL_ATLAS_SIZE, INITIAL_ATLAS_SIZE),
            context: ScaleContext::new(),
            version: 0,
            dirty: false,
        })
    }

    /// 栅格用的整数 ppem。布局侧必须按这个值排版（`em = ppem`）。
    #[must_use]
    pub const fn ppem(&self) -> u16 {
        self.ppem
    }

    /// 主字体是否带 TrueType 指令表（`fpgm` + `prep` + `cvt `）。为 false 时
    /// 本图集仍然可用，但每个字形都等于未 hinting 的光栅化——调用方应据此
    /// 回退到矢量/SDF 档，而不是以为自己在用 hinting。
    #[must_use]
    pub const fn is_hinted(&self) -> bool {
        self.hinted
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

    /// 单调版本号；新增字形 / 图集增长时递增。
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub const fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_uploaded(&mut self) {
        self.dirty = false;
    }

    /// 基线到 em 顶的距离（px，含 hinting 后的整数化前值）。
    #[must_use]
    pub const fn ascent_px(&self) -> f32 {
        self.ascent
    }

    /// 基线到 em 底的距离（px，负值）。
    #[must_use]
    pub const fn descent_px(&self) -> f32 {
        self.descent
    }

    /// 水平推进（px）。hinting 理论上可通过 phantom point 改推进，但 ttfautohint
    /// 不改；这里取 hmtx 原值，与 `FontVectorAtlas::advance_em` 同口径。
    pub fn advance(&self, character: char) -> f32 {
        let bytes = select_font(&self.primary, self.fallback.as_ref(), character);
        let Some(font) = FontRef::from_index(bytes, 0) else {
            return 0.0;
        };
        let glyph_id = font.charmap().map(character);
        if glyph_id == 0 {
            return 0.0;
        }
        font.glyph_metrics(&[])
            .scale(f32::from(self.ppem))
            .advance_width(glyph_id)
    }

    /// 该字符的 hinting 位图（首次调用时栅格化并打包）。没有字形返回 `None`。
    pub fn glyph(&mut self, character: char) -> Option<HintedGlyph> {
        if let Some(glyph) = self.cached.get(&character) {
            return Some(*glyph);
        }
        if self.missing.contains(&character) {
            return None;
        }
        let bytes = select_font(&self.primary, self.fallback.as_ref(), character);
        let advance = self.advance(character);
        let image = rasterize(bytes, &mut self.context, self.ppem, character);
        let Some(image) = image else {
            self.missing.insert(character);
            return None;
        };
        let (bitmap_w, bitmap_h) = (image.placement.width, image.placement.height);
        let insertion = if bitmap_w == 0 || bitmap_h == 0 {
            // 空白字形（空格一类）：没有位图，但仍有推进。
            Some((0, 0))
        } else {
            loop {
                if let Some(at) = self.packer.pack(bitmap_w, bitmap_h) {
                    break Some(at);
                }
                if !self.grow() {
                    break None;
                }
            }
        };
        let Some((cell_x, cell_y)) = insertion else {
            // 图集满了：不再重试（否则每帧都会重新光栅化同一个字），按缺字处理。
            self.missing.insert(character);
            return None;
        };
        if bitmap_w > 0 && bitmap_h > 0 {
            for row in 0..bitmap_h {
                let src = (row * bitmap_w) as usize;
                let dst = (((cell_y + row) * self.width + cell_x) * 4) as usize;
                for column in 0..bitmap_w as usize {
                    let coverage = image.data[src + column];
                    let pixel = dst + column * 4;
                    self.pixels[pixel..pixel + 4].copy_from_slice(&[coverage; 4]);
                }
            }
        }
        let u_min = cell_x as f32 / self.width as f32;
        let v_min = cell_y as f32 / self.height as f32;
        let u_max = (cell_x + bitmap_w) as f32 / self.width as f32;
        let v_max = (cell_y + bitmap_h) as f32 / self.height as f32;
        // swash 的 `placement.top` 是 y 向上的基线偏移；转成 y 向下的
        // [left, top, right, bottom]。
        let left = image.placement.left as f32;
        let top = -(image.placement.top as f32);
        let glyph = HintedGlyph {
            uv: [u_min, v_min, u_max, v_max],
            bounds: [left, top, left + bitmap_w as f32, top + bitmap_h as f32],
            advance,
        };
        self.cached.insert(character, glyph);
        self.version = self.version.wrapping_add(1);
        self.dirty = true;
        Some(glyph)
    }

    /// 预热一批字符（去重、跳过空白），返回本次新增的字形数。
    pub fn prewarm(&mut self, chars: impl Iterator<Item = char>) -> usize {
        let before = self.cached.len();
        let mut seen = HashSet::new();
        for character in chars.filter(|ch| !ch.is_whitespace()) {
            if seen.insert(character) {
                let _ = self.glyph(character);
            }
        }
        self.cached.len() - before
    }

    /// 图集向右下倍增（已有字形的位置与像素不变，只需重缩放归一化 UV）。
    fn grow(&mut self) -> bool {
        if self.width >= MAX_ATLAS_SIZE && self.height >= MAX_ATLAS_SIZE {
            return false;
        }
        let old_width = self.width;
        let old_height = self.height;
        let new_width = (self.width * 2).min(MAX_ATLAS_SIZE);
        let new_height = (self.height * 2).min(MAX_ATLAS_SIZE);
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
        let width_scale = old_width as f32 / new_width as f32;
        let height_scale = old_height as f32 / new_height as f32;
        for glyph in self.cached.values_mut() {
            glyph.uv[0] *= width_scale;
            glyph.uv[1] *= height_scale;
            glyph.uv[2] *= width_scale;
            glyph.uv[3] *= height_scale;
        }
        self.version = self.version.wrapping_add(1);
        self.dirty = true;
        true
    }
}

/// 主字体没有该字形时回落到后备字体（与 `HintedAtlas::glyph` 同一选择顺序）。
fn select_font<'a>(
    primary: &'a [u8],
    fallback: Option<&'a Arc<Vec<u8>>>,
    character: char,
) -> &'a [u8] {
    let has = |bytes: &[u8]| {
        FontRef::from_index(bytes, 0).is_some_and(|font| font.charmap().map(character) != 0)
    };
    if has(primary) {
        return primary;
    }
    if let Some(fallback) = fallback
        && has(fallback)
    {
        return fallback;
    }
    primary
}

/// 执行字体自带的 hinting 指令，返回 8-bit 覆盖率掩膜。
fn rasterize(
    bytes: &[u8],
    context: &mut ScaleContext,
    ppem: u16,
    character: char,
) -> Option<swash::scale::image::Image> {
    let font = FontRef::from_index(bytes, 0)?;
    let glyph_id = font.charmap().map(character);
    if glyph_id == 0 {
        return None;
    }
    let mut scaler = context
        .builder(font)
        .size(f32::from(ppem))
        .hint(true)
        .build();
    Render::new(&[Source::Outline]).render(&mut scaler, glyph_id)
}

/// 字体是否带 TrueType 指令表。缺任一表时 hinting 只能退化为未 hinting
/// （instructions 引用 cvt 槽位与 fpgm 函数，缺了就跑不起来）。
fn has_hinting_tables(bytes: &[u8]) -> bool {
    let Ok(face) = ttf_parser::Face::parse(bytes, 0) else {
        return false;
    };
    let raw = face.raw_face();
    [b"fpgm", b"prep", b"cvt "]
        .iter()
        .all(|tag| raw.table(ttf_parser::Tag::from_bytes(tag)).is_some())
}
