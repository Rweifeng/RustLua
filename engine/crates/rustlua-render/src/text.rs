//! Bitmap text atlas and deterministic CPU-side text layout for the 2D renderer.

use crate::font::FontAtlas;
use crate::font::FontAtlasError;
use crate::font::FontVectorAtlas;

/// The built-in monochrome ASCII glyph atlas used by the first text renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextAtlas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl TextAtlas {
    /// Creates a 128x48 RGBA8 atlas containing printable ASCII glyphs.
    #[must_use]
    pub fn built_in() -> Self {
        let width = 128;
        let height = 48;
        let mut pixels = vec![0; (width * height * 4) as usize];
        for code in 32u8..=127 {
            let index = (code - 32) as u32;
            let cell_x = (index % 16) * 8;
            let cell_y = (index / 16) * 8;
            for (row, bits) in bitmap(code as char).iter().enumerate() {
                for column in 0..5 {
                    if bits & (1 << (4 - column)) != 0 {
                        let offset =
                            (((cell_y + row as u32) * width + cell_x + column) * 4) as usize;
                        pixels[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
                    }
                }
            }
        }
        Self {
            width,
            height,
            pixels,
        }
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
    pub const fn bytes_per_row(&self) -> u32 {
        self.width * 4
    }

    /// Returns normalized UVs for one printable ASCII cell.
    #[must_use]
    pub fn uv(&self, character: char) -> ([f32; 2], [f32; 2]) {
        let code = if character.is_ascii() && (character as u8) >= 32 && (character as u8) <= 127 {
            character as u8
        } else {
            b'?'
        };
        let index = (code - 32) as u32;
        let x = (index % 16) * 8;
        let y = (index / 16) * 8;
        (
            [x as f32 / self.width as f32, y as f32 / self.height as f32],
            [
                (x + 8) as f32 / self.width as f32,
                (y + 8) as f32 / self.height as f32,
            ],
        )
    }
}

/// Layout parameters for a text string.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub scale: f32,
    pub spacing: f32,
    pub line_spacing: f32,
    pub color: [f32; 4],
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            scale: 1.0,
            spacing: 1.0,
            line_spacing: 2.0,
            color: [1.0; 4],
        }
    }
}

/// One text vertex in screen-pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VectorTextVertex {
    pub position: [f32; 2],
    pub local: [f32; 2],
    pub glyph_id: u32,
    pub color: [f32; 4],
}

/// CPU-side triangles for a laid-out string.
#[derive(Clone, Debug, PartialEq)]
pub struct TextMesh {
    pub vertices: Vec<TextVertex>,
    pub glyph_count: usize,
    pub line_count: usize,
    /// `[宽, 高]`：宽度是最宽一行的推进宽度（含行尾 advance 与字间距），
    /// 高度是 em 盒高度 `ascent + |descent| + (行数-1) × 行高`
    /// （M30 起修正；此前少算了 descender）。与 [`measure_text`] 完全同口径。
    pub size: [f32; 2],
    /// `true` 表示此 mesh 是**硬边点阵**：由 `build_bitmap`（内置 5×7 位图字模）或
    /// 像素字体（[`FontAtlas::is_pixel_font`]，1 texel = 1 设计像素）生成，
    /// 渲染侧会以 Nearest 采样 + MSDF 阈值化（`sdf=2`）处理，
    /// 与小字号 SDF 路径（`sdf=1`，Linear + smoothstep）区分。
    pub bitmap: bool,
}

impl Default for TextMesh {
    fn default() -> Self {
        Self {
            vertices: Vec::new(),
            glyph_count: 0,
            line_count: 1,
            size: [0.0, 0.0],
            bitmap: false,
        }
    }
}

impl TextMesh {
    /// 字形实际像素范围 `[min_x, min_y, max_x, max_y]`，相对 mesh 原点。
    ///
    /// 与 `size` 不同：`size` 含 ascender/descender 空白与尾随 advance 空隙
    /// （字形在其中不垂直居中）；`pixel_bounds` 是可见字形的紧致包围盒，
    /// 编辑器 UI 包围框用它贴合实际文字。
    #[must_use]
    pub fn pixel_bounds(&self) -> [f32; 4] {
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for v in &self.vertices {
            min[0] = min[0].min(v.position[0]);
            min[1] = min[1].min(v.position[1]);
            max[0] = max[0].max(v.position[0]);
            max[1] = max[1].max(v.position[1]);
        }
        if self.vertices.is_empty() {
            [0.0; 4]
        } else {
            [min[0], min[1], max[0], max[1]]
        }
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum TextError {
    #[error("invalid text style")]
    InvalidStyle,
    #[error("text origin is not finite")]
    NonFiniteOrigin,
    #[error("font atlas error: {0}")]
    FontAtlas(#[from] FontAtlasError),
}

/// Display px of one em at `style.scale == 1` (real-font metrics).
/// 也是场景 UI `Text` 组件 `font_size`（像素字号）与 `scale` 的换算基准。
/// 字面量在 `rustlua_math::text_metrics::TEXT_BASE_PX`，与 ui 侧共用一个事实源。
pub const EM_PX: f32 = rustlua_math::text_metrics::TEXT_BASE_PX;
/// Line height as a multiple of the em box.
const LINE_HEIGHT_EM: f32 = 1.2;

/// 与渲染同口径的文本度量结果（不生成顶点、不光栅化字形）。
#[derive(Clone, Debug, PartialEq)]
pub struct MeasuredText {
    /// 最宽一行的推进宽度（px），含行尾 advance 与字间距。
    pub width: f32,
    /// 行盒高度（px）：`ascent + (行数-1) × 行高 + |descent|`。
    pub height: f32,
    /// 计入的字形数（空格与换行不计，缺字仍计——它占一个 `.notdef` 框）。
    pub glyphs: usize,
    pub lines: usize,
    /// 字体（含后备字体）没有字形的字符，去重、按首次出现顺序。
    pub missing: Vec<char>,
}

impl MeasuredText {
    /// 缺字串（便于 Lua/日志直接断言）：`missing` 的字符拼接。
    #[must_use]
    pub fn missing_text(&self) -> String {
        self.missing.iter().collect()
    }
}

/// 与 [`TextLayout::build`]（SDF 路径）**同口径**的文本度量。
///
/// `size_px` 是像素字号（= `EM_PX * style.scale`，即 `ui.text_at` 的 `font_size`）；
/// `spacing` 是字间距（`TextStyle::default().spacing = 1.0`）。
/// 单测 `measure_matches_layout_size` 直接对 `TextLayout::build` 的输出钉住这条一致性。
#[must_use]
pub fn measure_text(
    font: &crate::font::FontProbe,
    text: &str,
    size_px: f32,
    spacing: f32,
) -> MeasuredText {
    let size = if size_px.is_finite() && size_px > 0.0 {
        size_px
    } else {
        EM_PX
    };
    let spacing = if spacing.is_finite() && spacing >= 0.0 {
        spacing
    } else {
        0.0
    };
    let line_height = size * LINE_HEIGHT_EM + spacing;
    let mut width: f32 = 0.0;
    let mut line_width: f32 = 0.0;
    let mut lines = 1usize;
    let mut glyphs = 0usize;
    let mut missing: Vec<char> = Vec::new();
    let mut missing_seen = std::collections::HashSet::new();
    for character in text.chars() {
        if character == '\n' {
            width = width.max(line_width);
            line_width = 0.0;
            lines += 1;
            continue;
        }
        if !font.covers(character) && missing_seen.insert(character) {
            missing.push(character);
        }
        if character == ' ' {
            // 空格没有字形：只推进，不绘制（与 `TextLayout::build` 同分支）。
            line_width += font.advance_px(' ', size) + spacing;
            width = width.max(line_width);
            continue;
        }
        line_width += font.advance_px(character, size) + spacing;
        width = width.max(line_width);
        glyphs += 1;
    }
    let ascent = font.ascent_em() * size;
    let descent = font.descent_em().abs() * size;
    MeasuredText {
        width,
        height: ascent + (lines as f32 - 1.0) * line_height + descent,
        glyphs,
        lines,
        missing,
    }
}

/// Deterministic text layout.
pub struct TextLayout;
impl TextLayout {
    /// Bitmap layout using the built-in 5x7 glyph metrics for the no-font
    /// fallback and standalone examples.
    pub fn build_bitmap(
        text: &str,
        origin: [f32; 2],
        style: TextStyle,
        atlas: &TextAtlas,
    ) -> Result<TextMesh, TextError> {
        if !origin[0].is_finite()
            || !origin[1].is_finite()
            || !style.scale.is_finite()
            || style.scale <= 0.0
            || !style.spacing.is_finite()
            || style.spacing < 0.0
            || !style.line_spacing.is_finite()
            || style.line_spacing < 0.0
            || style.color.iter().any(|v| !v.is_finite())
        {
            return Err(if origin[0].is_finite() && origin[1].is_finite() {
                TextError::InvalidStyle
            } else {
                TextError::NonFiniteOrigin
            });
        }
        let advance = 6.0 * style.scale + style.spacing;
        let line_height = 8.0 * style.scale + style.line_spacing;
        let mut x = origin[0];
        let mut y = origin[1];
        let mut max_x = x;
        let mut glyph_count = 0;
        let mut line_count = 1;
        let mut vertices = Vec::with_capacity(text.chars().count() * 6);
        for character in text.chars() {
            if character == '\n' {
                max_x = max_x.max(x);
                x = origin[0];
                y += line_height;
                line_count += 1;
                continue;
            }
            let (uv_min, uv_max) = atlas.uv(character);
            let w = 5.0 * style.scale;
            let h = 7.0 * style.scale;
            let x1 = x + w;
            let y1 = y + h;
            let quad = [
                [x, y, uv_min[0], uv_min[1]],
                [x1, y, uv_max[0], uv_min[1]],
                [x1, y1, uv_max[0], uv_max[1]],
                [x, y1, uv_min[0], uv_max[1]],
            ];
            let make = |v: [f32; 4]| TextVertex {
                position: [v[0], v[1]],
                uv: [v[2], v[3]],
                color: style.color,
            };
            vertices.extend_from_slice(&[
                make(quad[0]),
                make(quad[1]),
                make(quad[2]),
                make(quad[0]),
                make(quad[2]),
                make(quad[3]),
            ]);
            x += advance;
            max_x = max_x.max(x);
            glyph_count += 1;
        }
        Ok(TextMesh {
            vertices,
            glyph_count,
            line_count,
            size: [
                (max_x - origin[0]).max(0.0),
                (y - origin[1] + 7.0 * style.scale).max(0.0),
            ],
            bitmap: true,
        })
    }

    pub fn build_vector(
        text: &str,
        origin: [f32; 2],
        style: TextStyle,
        font: &mut FontVectorAtlas,
    ) -> Result<Vec<VectorTextVertex>, TextError> {
        if !origin.iter().all(|v| v.is_finite())
            || !style.scale.is_finite()
            || style.scale <= 0.0
            || !style.spacing.is_finite()
            || style.spacing < 0.0
            || !style.line_spacing.is_finite()
            || style.line_spacing < 0.0
            || style.color.iter().any(|v| !v.is_finite())
        {
            return Err(TextError::InvalidStyle);
        }
        let base_px = EM_PX * style.scale;
        let ascent = font.ascent_em();
        let mut x = origin[0];
        let mut y = origin[1] + ascent * base_px;
        let mut out = Vec::new();
        let s = EM_PX * style.scale;
        for ch in text.chars() {
            if ch == '\n' {
                x = origin[0];
                y += EM_PX * style.scale * LINE_HEIGHT_EM + style.line_spacing;
                continue;
            }
            if ch == ' ' {
                // 空格没有轮廓、`glyph()` 把它记成 missing，但它的 hmtx 仍要推进，
                // 否则整句空格零宽（"Rust Lua" 画成 "RustLua"），与 SDF 档不一致。
                x += font.advance_em(' ') * s + style.spacing;
                continue;
            }
            let Some(glyph) = font.glyph(ch).cloned() else {
                x += style.spacing;
                continue;
            };
            // bounds 是 y 向下的 [left, top, right, bottom]（相对笔位，em 单位）。
            let [min_x, min_y, max_x, max_y] = glyph.bounds;
            // 墨迹窄于一个像素的笔画（Noto Sans SC 子集里的 `i` 在 13 px 下只有
            // 0.86 px）会被 1x 光栅化整个丢掉——片元着色器的 AA 只在四边形内部
            // 生效，救不了一个采样点都没落进来的情况。按墨迹中心补到 1 px 见方；
            // 常规字形（≥1 px）逐字节不变，因此锚点/包围盒对齐不受影响。
            let ink_w = ((max_x - min_x) * s).max(0.0);
            let ink_h = ((max_y - min_y) * s).max(0.0);
            let cx = x + (min_x + max_x) * 0.5 * s;
            let cy = y + (min_y + max_y) * 0.5 * s;
            let w = ink_w.max(1.001);
            let h = ink_h.max(1.001);
            let x0 = cx - w * 0.5;
            let y0 = cy - h * 0.5;
            let x1 = cx + w * 0.5;
            let y1 = cy + h * 0.5;
            let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
            for &p in &[
                corners[0], corners[1], corners[2], corners[0], corners[2], corners[3],
            ] {
                out.push(VectorTextVertex {
                    position: p,
                    local: [(p[0] - x) / s, (p[1] - y) / s],
                    glyph_id: glyph.id,
                    color: style.color,
                });
            }
            x += glyph.advance * s + style.spacing;
        }
        Ok(out)
    }

    /// SDF text layout using real font metrics (TMP-style, crisp at any scale).
    /// All glyphs (ASCII + CJK) come from the one SDF `FontAtlas`.
    pub fn build(
        text: &str,
        origin: [f32; 2],
        style: TextStyle,
        font: &mut FontAtlas,
    ) -> Result<TextMesh, TextError> {
        if !origin[0].is_finite()
            || !origin[1].is_finite()
            || !style.scale.is_finite()
            || style.scale <= 0.0
            || !style.spacing.is_finite()
            || style.spacing < 0.0
            || !style.line_spacing.is_finite()
            || style.line_spacing < 0.0
            || style.color.iter().any(|v| !v.is_finite())
        {
            return Err(if origin[0].is_finite() && origin[1].is_finite() {
                TextError::InvalidStyle
            } else {
                TextError::NonFiniteOrigin
            });
        }
        let ascent = font.ascent_em();
        let descent = font.descent_em();
        let base_px = EM_PX * style.scale;
        // 1 屏幕像素对应的图集 texel 数（display size = base_px）。图集栅格尺寸由
        // 字体自己决定：MSDF 是 RASTER_SCALE=96 px/em，像素字体是 1 texel/设计像素
        // （实测 12 px/em），写死 96 会把点阵字形放大成 8 倍。
        let raster_scale = font.raster_scale();
        let pixel = font.is_pixel_font();
        let tex_scale = base_px / raster_scale;
        let line_height = EM_PX * style.scale * LINE_HEIGHT_EM + style.line_spacing;
        let mut x = origin[0];
        let mut y = origin[1] + ascent * base_px;
        let mut max_x = x;
        let mut glyph_count = 0;
        let mut line_count = 1;
        let mut vertices = Vec::with_capacity(text.chars().count() * 6);
        for character in text.chars() {
            if character == '\n' {
                max_x = max_x.max(x);
                x = origin[0];
                y += line_height;
                line_count += 1;
                continue;
            }
            if character == ' ' {
                // 空格没有字形/轮廓：只推进，不绘制（避免渲染成占位方框）。
                x += font.advance(' ') * tex_scale + style.spacing;
                max_x = max_x.max(x);
                continue;
            }
            let metrics = font.try_glyph_metrics(character)?;
            // 字形在整张图集中的 UV = 单元 UV + 单元内偏移在图集中的比例。
            // 四边形尺寸 = 字形显示尺寸。
            let rect = metrics.cell_rect;
            let atlas_w = font.width() as f32;
            let atlas_h = font.height() as f32;
            let u0 = metrics.uv[0] + rect[0] / atlas_w;
            let v0 = metrics.uv[1] + rect[1] / atlas_h;
            let u1 = metrics.uv[0] + rect[2] / atlas_w;
            let v1 = metrics.uv[1] + rect[3] / atlas_h;
            let mut glyph_w = (rect[2] - rect[0]) * tex_scale;
            let mut glyph_h = (rect[3] - rect[1]) * tex_scale;
            let mut x0 = x + metrics.bounds[0] * tex_scale;
            let mut y0 = y + metrics.bounds[1] * tex_scale;
            if pixel {
                // 整数吸附：点阵的一个 texel 必须正好盖住整数个设备像素。次像素
                // 定位下 Nearest 采样会把边界 texel 采到两次（或零次），字形边缘
                // 出现"抖一格"，判据见 `jhyt_m72_pixel_font` 的 1:1 覆盖计数。
                x0 = x0.round();
                y0 = y0.round();
                glyph_w = glyph_w.round();
                glyph_h = glyph_h.round();
            }
            let x1 = x0 + glyph_w;
            let y1 = y0 + glyph_h;
            let quad = [
                [x0, y0, u0, v0],
                [x1, y0, u1, v0],
                [x1, y1, u1, v1],
                [x0, y1, u0, v1],
            ];
            let make = |v: [f32; 4]| TextVertex {
                position: [v[0], v[1]],
                uv: [v[2], v[3]],
                color: style.color,
            };
            vertices.extend_from_slice(&[
                make(quad[0]),
                make(quad[1]),
                make(quad[2]),
                make(quad[0]),
                make(quad[2]),
                make(quad[3]),
            ]);
            x += metrics.advance * tex_scale + style.spacing;
            max_x = max_x.max(x);
            glyph_count += 1;
        }
        Ok(TextMesh {
            vertices,
            glyph_count,
            line_count,
            size: [
                (max_x - origin[0]).max(0.0),
                // em 盒高度 = ascent + |descent|（`descent_em()` 为负值，故用减法）。
                // M30 修：此前是 `+ descent * base_px`，等于把 descender 从盒子里减掉
                //（Noto Sans SC 下 0.8em 而非 1.2em），编辑器文字包围框与
                // `measure_ui_text` 的高度因此都比实际字形矮一截。
                (y - descent * base_px - origin[1]).max(0.0),
            ],
            // 像素字体的字形是 1 bit/texel 的点阵，必须走阈值档（`sdf=2`）+ Nearest
            // 采样；若仍按 MSDF（`sdf=1`）做 smoothstep，点阵会被插值成灰边。
            bitmap: pixel,
        })
    }
}

fn bitmap(character: char) -> [u8; 7] {
    let c = character.to_ascii_uppercase();
    match c {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0F, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0F],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0F, 0x10, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1F],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x10, 0x1E, 0x01, 0x01, 0x1E],
        '6' => [0x0E, 0x10, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x01, 0x0E],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '?' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        _ => [0; 7],
    }
}

/// Measures text with the built-in 5x7 bitmap metrics, returning
/// `(size, pixel_bounds)` exactly like `TextMesh`.
///
/// Hosts without a font atlas (scene UI layout, editor bounding boxes) use this
/// so intrinsic sizes and rendered glyphs share one measurement path. `None`
/// only when the style is invalid (`scale` non-finite or `<= 0`).
#[must_use]
pub fn measure_builtin_text(text: &str, scale: f32) -> Option<([f32; 2], [f32; 4])> {
    let atlas = TextAtlas::built_in();
    let style = TextStyle {
        scale,
        ..Default::default()
    };
    TextLayout::build_bitmap(text, [0.0, 0.0], style, &atlas)
        .ok()
        .map(|mesh| (mesh.size, mesh.pixel_bounds()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_builtin_text_matches_the_bitmap_mesh() {
        let atlas = TextAtlas::built_in();
        let mesh = TextLayout::build_bitmap(
            "Wg",
            [0.0, 0.0],
            TextStyle {
                scale: 2.0,
                ..Default::default()
            },
            &atlas,
        )
        .unwrap();
        let expected = (mesh.size, mesh.pixel_bounds());
        assert_eq!(measure_builtin_text("Wg", 2.0), Some(expected));
        assert_eq!(measure_builtin_text("Wg", 0.0), None);
    }

    #[test]
    fn atlas_has_expected_dimensions_and_non_empty_glyph() {
        let atlas = TextAtlas::built_in();
        assert_eq!((atlas.width(), atlas.height()), (128, 48));
        assert!(
            atlas
                .pixels()
                .as_chunks::<4>()
                .0
                .contains(&[255, 255, 255, 255])
        );
    }
    #[test]
    fn layout_handles_newlines_and_lowercase() {
        let atlas = TextAtlas::built_in();
        let mesh = TextLayout::build_bitmap("Rust\nLua", [2.0, 3.0], TextStyle::default(), &atlas)
            .unwrap();
        assert_eq!(
            (mesh.glyph_count, mesh.line_count, mesh.vertices.len()),
            (7, 2, 42)
        );
        assert!(mesh.size[1] > 7.0);
    }

    #[test]
    fn sdf_build_single_mesh_mixed_ascii_and_cjk() {
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&font_path).unwrap_or_else(|error| {
            panic!("测试字体缺失或不可读 {}: {error}", font_path.display())
        });
        assert!(!bytes.is_empty(), "测试字体为空: {}", font_path.display());
        let mut font = crate::font::FontAtlas::load(&bytes).unwrap();
        let mesh =
            TextLayout::build("开始 SPACE", [0.0, 0.0], TextStyle::default(), &mut font).unwrap();
        assert_eq!(
            mesh.glyph_count, 7,
            "ASCII + CJK 在同一个 mesh（空格不计字形）"
        );
        assert!(!mesh.vertices.is_empty());
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.position.iter().all(|p| p.is_finite()))
        );
        assert!(mesh.size[0] > 0.0 && mesh.size[1] > 0.0);
    }

    #[test]
    fn sdf_mesh_pixel_bounds_tracks_glyph_pixels_not_metric_box() {
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&font_path).unwrap_or_else(|error| {
            panic!("测试字体缺失或不可读 {}: {error}", font_path.display())
        });
        let mut font = crate::font::FontAtlas::load(&bytes).unwrap();
        let mesh = TextLayout::build(
            "RustLua Production",
            [0.0, 0.0],
            TextStyle::default(),
            &mut font,
        )
        .unwrap();
        let bounds = mesh.pixel_bounds();
        assert!(
            bounds[2] > bounds[0] && bounds[3] > bounds[1],
            "glyph bbox non-degenerate"
        );
        // 网格 size[0] 含尾随 advance，字形 bbox 更窄，贴合可见字形。
        assert!(
            bounds[2] - bounds[0] < mesh.size[0],
            "bbox trims trailing advance"
        );
        // 垂直方向：网格含 ascender/descender 空白，字形中心与网格中心不重合，
        // 故编辑器包围框用 pixel_bounds 而非 mesh.size。
        let glyph_center_y = (bounds[1] + bounds[3]) * 0.5;
        let mesh_center_y = mesh.size[1] * 0.5;
        assert!((glyph_center_y - mesh_center_y).abs() > 0.01);
    }

    /// 墨迹窄于一个像素的笔画必须仍然被光栅化：矢量档的 AA 在片元里做，但光栅化是
    /// 1x 采样，`i` 的竖笔在 6 px 字号下只有 0.73 px 宽——不补到一像素，整个四边形
    /// 可以一个采样中心都不落在里面，字就凭空消失。
    #[test]
    fn vector_quads_never_fall_below_one_pixel() {
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&font_path).unwrap();
        let mut vector = crate::font::FontVectorAtlas::load(&bytes).unwrap();
        let style = TextStyle {
            scale: 0.5,
            ..Default::default()
        };
        let s = EM_PX * style.scale;
        let bounds = vector.glyph('i').cloned().unwrap().bounds;
        assert!(
            (bounds[2] - bounds[0]) * s < 1.0,
            "前提不成立：`i` 的墨迹宽度已不小于一个像素"
        );
        let run = TextLayout::build_vector("i", [0.0, 0.0], style, &mut vector).unwrap();
        let min_x = run.iter().map(|v| v.position[0]).fold(f32::MAX, f32::min);
        let max_x = run.iter().map(|v| v.position[0]).fold(f32::MIN, f32::max);
        assert!(
            max_x - min_x >= 1.0,
            "`i` 的四边形只有 {} px，可能一个采样中心都盖不住",
            max_x - min_x
        );
        // 补宽只能按墨迹中心对称外扩，且不得超过片元着色器的裁剪余量
        // （`bounds ± fwidth` ≈ 一个像素），否则补出来的那一圈会被 discard 掉。
        let local_min = run.iter().map(|v| v.local[0]).fold(f32::MAX, f32::min);
        let local_max = run.iter().map(|v| v.local[0]).fold(f32::MIN, f32::max);
        let ink_center = (bounds[0] + bounds[2]) * 0.5;
        assert!(
            ((local_min + local_max) * 0.5 - ink_center).abs() < 1e-3,
            "补宽把笔画推离了墨迹中心：{} vs {ink_center}",
            (local_min + local_max) * 0.5
        );
        let overflow = run
            .iter()
            .map(|v| {
                (v.local[0] - bounds[2])
                    .max(bounds[0] - v.local[0])
                    .max(v.local[1] - bounds[3])
                    .max(bounds[1] - v.local[1])
            })
            .fold(0.0_f32, f32::max);
        assert!(
            overflow <= 0.5 / s + 1e-3,
            "补宽外扩 {} em 超过了片元裁剪余量 {}",
            overflow,
            0.5 / s
        );
    }

    #[test]
    fn vector_and_sdf_layouts_share_the_baseline() {
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&font_path).unwrap();
        let style = TextStyle {
            scale: 3.0,
            ..Default::default()
        };
        let mut msdf = crate::font::FontAtlas::load(&bytes).unwrap();
        let mut vector = crate::font::FontVectorAtlas::load(&bytes).unwrap();
        let sdf = TextLayout::build("汉字", [10.0, 20.0], style, &mut msdf).unwrap();
        let outline = TextLayout::build_vector("汉字", [10.0, 20.0], style, &mut vector).unwrap();
        let sdf_top = sdf
            .vertices
            .iter()
            .map(|vertex| vertex.position[1])
            .fold(f32::INFINITY, f32::min);
        let vector_top = outline
            .iter()
            .map(|vertex| vertex.position[1])
            .fold(f32::INFINITY, f32::min);
        assert!(
            (sdf_top - vector_top).abs() < 0.01,
            "sdf_top={sdf_top}, vector_top={vector_top}"
        );
    }

    /// 矢量档必须为空格推进：空格没有轮廓、进不了字形缓存，但它的 hmtx 仍是排版
    /// 宽度。此前零宽空格把整句压成 "RustLua"，且与 SDF 档同一句话宽度不一致。
    #[test]
    fn vector_layout_advances_spaces() {
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&font_path).unwrap();
        let style = TextStyle {
            scale: 2.0,
            ..Default::default()
        };
        let mut msdf = crate::font::FontAtlas::load(&bytes).unwrap();
        let mut vector = crate::font::FontVectorAtlas::load(&bytes).unwrap();
        let right = |run: &[VectorTextVertex]| {
            run.iter()
                .map(|vertex| vertex.position[0])
                .fold(f32::NEG_INFINITY, f32::max)
        };
        let spaced = TextLayout::build_vector("Rust Lua", [0.0, 0.0], style, &mut vector).unwrap();
        let joined = TextLayout::build_vector("RustLua", [0.0, 0.0], style, &mut vector).unwrap();
        let vector_gap = right(&spaced) - right(&joined);
        assert!(
            vector_gap > 5.0,
            "矢量档的空格被压成零宽：spaced={} joined={} gap={vector_gap}",
            right(&spaced),
            right(&joined)
        );
        let sdf_spaced = TextLayout::build("Rust Lua", [0.0, 0.0], style, &mut msdf).unwrap();
        let sdf_joined = TextLayout::build("RustLua", [0.0, 0.0], style, &mut msdf).unwrap();
        let sdf_gap = sdf_spaced.size[0] - sdf_joined.size[0];
        assert!(
            (vector_gap - sdf_gap).abs() < 0.5,
            "两条通道的空格推进不一致：vector={vector_gap} sdf={sdf_gap}"
        );
    }

    /// 度量必须与渲染布局同口径：同一字体、同一像素字号下，
    /// `measure_text().width/height` 要等于 `TextLayout::build` 的 `mesh.size`
    /// （这条断言是"排版预算可信"的根基——度量漂了，溢出检查就是假的）。
    #[test]
    fn measure_matches_layout_size() {
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&font_path).unwrap();
        let probe = crate::font::FontProbe::load(&bytes).unwrap();
        let mut atlas = crate::font::FontAtlas::load(&bytes).unwrap();
        for size_px in [13.0_f32, 15.0, 18.0, 22.0] {
            for text in ["江湖有田", "Day 3 · 雨", "灵芝×2", "长数字 99999 文"] {
                let style = TextStyle {
                    scale: size_px / EM_PX,
                    ..Default::default()
                };
                let mesh = TextLayout::build(text, [0.0, 0.0], style, &mut atlas).unwrap();
                let measured = measure_text(&probe, text, size_px, TextStyle::default().spacing);
                assert!(
                    (measured.width - mesh.size[0]).abs() < 0.05,
                    "width {text:?}@{size_px}: measured={} mesh={}",
                    measured.width,
                    mesh.size[0]
                );
                assert!(
                    (measured.height - mesh.size[1]).abs() < 0.05,
                    "height {text:?}@{size_px}: measured={} mesh={}",
                    measured.height,
                    mesh.size[1]
                );
                assert_eq!(measured.glyphs, mesh.glyph_count);
                assert_eq!(measured.lines, mesh.line_count);
            }
        }
    }

    /// 缺字检测：字体里没有的字符（emoji）必须被报出来；常用中文/标点不得误报。
    #[test]
    fn measure_reports_missing_glyphs() {
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&font_path).unwrap();
        let probe = crate::font::FontProbe::load(&bytes).unwrap();
        let ok = measure_text(&probe, "江湖有田·雨，晴。（灵芝）", 15.0, 1.0);
        assert!(ok.missing.is_empty(), "常用中文缺字: {:?}", ok.missing);
        let probe_missing = measure_text(&probe, "江湖🙂", 15.0, 1.0);
        assert_eq!(probe_missing.missing, vec!['🙂']);
        assert_eq!(probe_missing.missing_text(), "🙂");
        // 换行：行数与高度按行盒累加。
        let single = measure_text(&probe, "江湖", 15.0, 1.0);
        let two = measure_text(&probe, "江湖\n有田", 15.0, 1.0);
        assert_eq!(two.lines, 2);
        assert!(two.height > single.height);
    }

    #[test]
    fn invalid_style_is_rejected() {
        let atlas = TextAtlas::built_in();
        let style = TextStyle {
            scale: 0.0,
            ..TextStyle::default()
        };
        assert_eq!(
            TextLayout::build_bitmap("A", [0.0, 0.0], style, &atlas),
            Err(TextError::InvalidStyle)
        );
    }
}
