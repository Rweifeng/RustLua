//! Bitmap text atlas and deterministic CPU-side text layout for the 2D renderer.

use crate::font::FontAtlas;
use crate::font::FontAtlasError;
use crate::font::FontVectorAtlas;

/// Glyph size inside the built-in atlas: 5x7 pixels at the top-left of an
/// 8x8 cell. The rest of the cell is transparent padding.
const GLYPH_W: u32 = 5;
const GLYPH_H: u32 = 7;

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

    /// Returns normalized UVs for the 5x7 glyph inside one printable ASCII cell.
    ///
    /// The atlas cell is 8x8 with the glyph in the top-left and transparent
    /// padding on the right/bottom. `build_bitmap` draws the glyph into a
    /// `5xscale` by `7xscale` quad, so the UV must cover exactly the 5x7
    /// content: sampling the whole 8x8 cell squeezes 8 texels into 5 px and
    /// Nearest drops whole glyph columns (P loses its right stem and reads
    /// as F at `font_size` 13).
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
                (x + GLYPH_W) as f32 / self.width as f32,
                (y + GLYPH_H) as f32 / self.height as f32,
            ],
        )
    }
}

/// Horizontal alignment inside [`TextStyle::box_size`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl TextAlign {
    /// Offset of a `line_width` line inside a `box_width` box.
    #[must_use]
    pub fn offset(self, box_width: f32, line_width: f32) -> f32 {
        match self {
            Self::Left => 0.0,
            Self::Center => (box_width - line_width) * 0.5,
            Self::Right => box_width - line_width,
        }
    }
}

/// Vertical alignment inside [`TextStyle::box_size`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextVAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

impl TextVAlign {
    /// Offset of a `content_height` block inside a `box_height` box.
    #[must_use]
    pub fn offset(self, box_height: f32, content_height: f32) -> f32 {
        match self {
            Self::Top => 0.0,
            Self::Middle => (box_height - content_height) * 0.5,
            Self::Bottom => box_height - content_height,
        }
    }
}

/// Horizontal overflow rule (Unity `Text.horizontalOverflow` + ellipsis).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextOverflow {
    /// Draw past the box (legacy behaviour; Unity's `Overflow`).
    #[default]
    Overflow,
    /// Greedy word wrap at spaces; breaks inside a word when it is wider than the box.
    Wrap,
    /// Cut the line at the box edge.
    Truncate,
    /// Cut the line and append the backend's ellipsis when characters were dropped.
    Ellipsis,
}

/// Vertical overflow rule (Unity `Text.verticalOverflow`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextVOverflow {
    /// Draw every line past the box.
    #[default]
    Overflow,
    /// Drop lines that do not fit inside the box.
    Truncate,
}

/// Layout parameters for a text string.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub scale: f32,
    pub spacing: f32,
    pub line_spacing: f32,
    pub color: [f32; 4],
    /// Typesetting box `[width, height]` in pixels. `None` keeps the legacy
    /// auto-sized layout (no wrap and no alignment box).
    pub box_size: Option<[f32; 2]>,
    pub align: TextAlign,
    pub valign: TextVAlign,
    pub overflow: TextOverflow,
    pub voverflow: TextVOverflow,
    /// Parse `<b>`, `<color=#rrggbb[aa]>`, `</color>`, `<br/>` and `<icon=..>`
    /// tags. Off by default so low-level callers (dynamic UI, world text) get
    /// literal text; the scene `Text` component opts in.
    pub rich_text: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            scale: 1.0,
            spacing: 1.0,
            line_spacing: 2.0,
            color: [1.0; 4],
            box_size: None,
            align: TextAlign::Left,
            valign: TextVAlign::Top,
            overflow: TextOverflow::Overflow,
            voverflow: TextVOverflow::Overflow,
            rich_text: false,
        }
    }
}

impl TextStyle {
    /// Rejects non-finite / non-positive metrics, including a malformed box.
    fn is_valid(&self) -> bool {
        self.scale.is_finite()
            && self.scale > 0.0
            && self.spacing.is_finite()
            && self.spacing >= 0.0
            && self.line_spacing.is_finite()
            && self.line_spacing >= 0.0
            && self.color.iter().all(|value| value.is_finite())
            && self.box_size.is_none_or(|box_size| {
                box_size
                    .iter()
                    .all(|value| value.is_finite() && *value >= 0.0)
            })
    }

    fn box_width(&self) -> Option<f32> {
        self.box_size
            .map(|box_size| box_size[0])
            .filter(|width| *width > 0.0)
    }

    fn box_height(&self) -> Option<f32> {
        self.box_size
            .map(|box_size| box_size[1])
            .filter(|height| *height > 0.0)
    }
}

/// One character after rich-text tag removal, carrying its resolved colour.
#[derive(Clone, Debug, PartialEq)]
struct StyledChar {
    ch: char,
    color: [f32; 4],
    bold: bool,
}

/// Synthetic bold offset in em (the renderer has one font face; a second shifted
/// copy thickens the stroke the same way most bitmap fonts fake bold).
const BOLD_SHIFT_EM: f32 = 0.06;

/// Parses the restricted rich-text subset shared with `ui.rich_text`:
/// `<b>`, `<color=#rrggbb[aa]>`, `</color>`, `<br/>`, `<icon=..>`. Unknown tags
/// stay literal so authored `<` text never disappears. `<icon>` is zero-width
/// (icons are not rendered yet), matching `parse_rich_text().plain`.
fn styled_chars(text: &str, style: &TextStyle) -> Vec<StyledChar> {
    let plain = |source: &str, out: &mut Vec<StyledChar>| {
        out.extend(source.chars().map(|ch| StyledChar {
            ch,
            color: style.color,
            bold: false,
        }));
    };
    if !style.rich_text {
        let mut out = Vec::with_capacity(text.len());
        plain(text, &mut out);
        return out;
    }
    fn push(out: &mut Vec<StyledChar>, source: &str, color: [f32; 4], bold: bool) {
        out.extend(source.chars().map(|ch| StyledChar { ch, color, bold }));
    }
    let mut out = Vec::with_capacity(text.len());
    let mut color = style.color;
    let mut color_stack: Vec<[f32; 4]> = Vec::new();
    let mut bold = false;
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        push(&mut out, &rest[..open], color, bold);
        rest = &rest[open..];
        let Some(close) = rest.find('>') else {
            push(&mut out, rest, color, bold);
            rest = "";
            break;
        };
        let tag = &rest[1..close];
        let lowered = tag.trim().to_ascii_lowercase();
        let consumed = close + 1;
        if lowered == "b" {
            bold = true;
        } else if lowered == "/b" {
            bold = false;
        } else if lowered == "br" || lowered == "br/" {
            out.push(StyledChar {
                ch: '\n',
                color,
                bold,
            });
        } else if let Some(raw) = lowered.strip_prefix("color=") {
            color_stack.push(color);
            color = parse_tag_color(raw.trim()).unwrap_or(color);
        } else if lowered == "/color" {
            color = color_stack.pop().unwrap_or(style.color);
        } else if lowered.starts_with("icon=") {
            // 图标是零宽标签：与 `ui_rich_text::parse_rich_text().plain` 同口径。
        } else {
            push(&mut out, &rest[..consumed], color, bold);
        }
        rest = &rest[consumed..];
    }
    push(&mut out, rest, color, bold);
    out
}

fn parse_tag_color(raw: &str) -> Option<[f32; 4]> {
    let hex = raw.strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    let channel = |shift: u32| ((value >> shift) & 0xff) as f32 / 255.0;
    Some([
        channel(if hex.len() == 8 { 24 } else { 16 }),
        channel(if hex.len() == 8 { 16 } else { 8 }),
        channel(if hex.len() == 8 { 8 } else { 0 }),
        if hex.len() == 8 { channel(0) } else { 1.0 },
    ])
}

/// Applies alignment/overflow and returns visual lines of styled characters.
fn layout_lines(
    text: &str,
    style: &TextStyle,
    advance: &mut dyn FnMut(char) -> f32,
    ellipsis: &str,
) -> Vec<Vec<StyledChar>> {
    let mut paragraphs: Vec<Vec<StyledChar>> = vec![Vec::new()];
    for character in styled_chars(text, style) {
        if character.ch == '\n' {
            paragraphs.push(Vec::new());
        } else if let Some(paragraph) = paragraphs.last_mut() {
            paragraph.push(character);
        }
    }
    let mut lines = Vec::new();
    for paragraph in paragraphs {
        match style.overflow {
            TextOverflow::Wrap => match style.box_width() {
                Some(width) => wrap_paragraph(paragraph, width, advance, &mut lines),
                None => lines.push(paragraph),
            },
            TextOverflow::Truncate | TextOverflow::Ellipsis => match style.box_width() {
                Some(width) => lines.push(truncate_paragraph(
                    paragraph,
                    width,
                    style.overflow,
                    advance,
                    ellipsis,
                )),
                None => lines.push(paragraph),
            },
            TextOverflow::Overflow => lines.push(paragraph),
        }
    }
    lines
}

fn wrap_paragraph(
    mut rest: Vec<StyledChar>,
    width: f32,
    advance: &mut dyn FnMut(char) -> f32,
    lines: &mut Vec<Vec<StyledChar>>,
) {
    while !rest.is_empty() {
        let mut line_width = 0.0;
        let mut end = 0;
        let mut last_space = None;
        while end < rest.len() {
            let next = advance(rest[end].ch);
            if end > 0 && line_width + next > width {
                break;
            }
            if rest[end].ch == ' ' {
                last_space = Some(end);
            }
            line_width += next;
            end += 1;
        }
        if end == rest.len() {
            lines.push(rest);
            break;
        }
        let cut = last_space.filter(|space| *space > 0).unwrap_or(end);
        let mut line: Vec<StyledChar> = rest.drain(..cut).collect();
        while line.last().is_some_and(|character| character.ch == ' ') {
            line.pop();
        }
        if rest.first().is_some_and(|character| character.ch == ' ') {
            rest.remove(0);
        }
        if line.is_empty() {
            // 零 advance 字体保护：至少推进一个字符，避免死循环。
            if let Some(character) = rest.first().cloned() {
                line.push(character);
                rest.remove(0);
            } else {
                break;
            }
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }
}

fn truncate_paragraph(
    mut characters: Vec<StyledChar>,
    width: f32,
    overflow: TextOverflow,
    advance: &mut dyn FnMut(char) -> f32,
    ellipsis: &str,
) -> Vec<StyledChar> {
    let total: f32 = characters
        .iter()
        .map(|character| advance(character.ch))
        .sum();
    if total <= width {
        return characters;
    }
    let ellipsis_chars: Vec<char> = if overflow == TextOverflow::Ellipsis {
        ellipsis.chars().collect()
    } else {
        Vec::new()
    };
    let ellipsis_width: f32 = ellipsis_chars
        .iter()
        .map(|character| advance(*character))
        .sum();
    let budget = (width - ellipsis_width).max(0.0);
    let mut line_width = 0.0;
    let mut keep = 0;
    for character in &characters {
        let next = advance(character.ch);
        if line_width + next > budget {
            break;
        }
        line_width += next;
        keep += 1;
    }
    let style = characters.last().cloned().unwrap_or(StyledChar {
        ch: ' ',
        color: [1.0; 4],
        bold: false,
    });
    characters.truncate(keep);
    if !ellipsis_chars.is_empty() && ellipsis_width <= width {
        characters.extend(ellipsis_chars.into_iter().map(|ch| StyledChar {
            ch,
            color: style.color,
            bold: style.bold,
        }));
    }
    characters
}

/// Number of lines that fit the box when `voverflow=truncate`.
fn visible_lines(
    style: &TextStyle,
    line_count: usize,
    line_height: f32,
    first_line_height: f32,
) -> usize {
    if style.voverflow != TextVOverflow::Truncate {
        return line_count;
    }
    let Some(height) = style.box_height() else {
        return line_count;
    };
    if line_height <= 0.0 || height < first_line_height {
        return 0;
    }
    let fit = (1.0 + ((height - first_line_height) / line_height).floor()).max(0.0) as usize;
    fit.min(line_count)
}

fn line_width(line: &[StyledChar], advance: &mut dyn FnMut(char) -> f32) -> f32 {
    line.iter()
        .map(|character| advance(character.ch))
        .sum::<f32>()
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
        if !origin[0].is_finite() || !origin[1].is_finite() || !style.is_valid() {
            return Err(if origin[0].is_finite() && origin[1].is_finite() {
                TextError::InvalidStyle
            } else {
                TextError::NonFiniteOrigin
            });
        }
        let advance = 6.0 * style.scale + style.spacing;
        let mut advance_fn = |_: char| advance;
        let lines = layout_lines(text, &style, &mut advance_fn, "...");
        let line_height = 8.0 * style.scale + style.line_spacing;
        let first_line_height = 7.0 * style.scale;
        let visible = visible_lines(&style, lines.len(), line_height, first_line_height);
        let lines = &lines[..visible];
        let content_width = lines
            .iter()
            .map(|line| line_width(line, &mut advance_fn))
            .fold(0.0_f32, f32::max);
        let content_height = if visible == 0 {
            0.0
        } else {
            first_line_height + (visible as f32 - 1.0) * line_height
        };
        let box_top = origin[1]
            + style
                .valign
                .offset(style.box_height().unwrap_or(content_height), content_height);
        let bold_shift = BOLD_SHIFT_EM * 8.0 * style.scale;
        let w = 5.0 * style.scale;
        let h = 7.0 * style.scale;
        let mut glyph_count = 0;
        let mut vertices = Vec::with_capacity(text.chars().count() * 6);
        for (line_index, line) in lines.iter().enumerate() {
            let line_offset = style.align.offset(
                style.box_width().unwrap_or(content_width),
                line_width(line, &mut advance_fn),
            );
            let mut x = origin[0] + line_offset;
            let y = box_top + line_index as f32 * line_height;
            for character in line {
                let (uv_min, uv_max) = atlas.uv(character.ch);
                let shifts = [0.0_f32, bold_shift];
                let shift_count = if character.bold { 2 } else { 1 };
                for shift in &shifts[..shift_count] {
                    let x0 = x + shift;
                    let x1 = x0 + w;
                    let y1 = y + h;
                    let quad = [
                        [x0, y, uv_min[0], uv_min[1]],
                        [x1, y, uv_max[0], uv_min[1]],
                        [x1, y1, uv_max[0], uv_max[1]],
                        [x0, y1, uv_min[0], uv_max[1]],
                    ];
                    let make = |v: [f32; 4]| TextVertex {
                        position: [v[0], v[1]],
                        uv: [v[2], v[3]],
                        color: character.color,
                    };
                    vertices.extend_from_slice(&[
                        make(quad[0]),
                        make(quad[1]),
                        make(quad[2]),
                        make(quad[0]),
                        make(quad[2]),
                        make(quad[3]),
                    ]);
                }
                x += advance;
                glyph_count += 1;
            }
        }
        Ok(TextMesh {
            vertices,
            glyph_count,
            line_count: visible.max(1),
            size: [content_width, content_height],
            bitmap: true,
        })
    }

    pub fn build_vector(
        text: &str,
        origin: [f32; 2],
        style: TextStyle,
        font: &mut FontVectorAtlas,
    ) -> Result<Vec<VectorTextVertex>, TextError> {
        if !origin.iter().all(|v| v.is_finite()) || !style.is_valid() {
            return Err(TextError::InvalidStyle);
        }
        let base_px = EM_PX * style.scale;
        let s = base_px;
        let line_height = EM_PX * style.scale * LINE_HEIGHT_EM + style.line_spacing;
        let lines = {
            let mut advance = |ch: char| font.advance_em(ch) * s + style.spacing;
            layout_lines(text, &style, &mut advance, "…")
        };
        let ascent = font.ascent_em();
        let visible = visible_lines(
            &style,
            lines.len(),
            line_height,
            ascent * base_px + font.descent_em().abs() * base_px,
        );
        let lines = &lines[..visible];
        let first_line_height = ascent * base_px + font.descent_em().abs() * base_px;
        let content_height = if visible == 0 {
            0.0
        } else {
            first_line_height + (visible as f32 - 1.0) * line_height
        };
        let box_top = origin[1]
            + style
                .valign
                .offset(style.box_height().unwrap_or(content_height), content_height);
        let bold_shift = BOLD_SHIFT_EM * base_px;
        let mut out = Vec::new();
        for (line_index, line) in lines.iter().enumerate() {
            let content_width = line
                .iter()
                .map(|character| font.advance_em(character.ch) * s + style.spacing)
                .sum::<f32>();
            let line_offset = style
                .align
                .offset(style.box_width().unwrap_or(content_width), content_width);
            let mut x = origin[0] + line_offset;
            let baseline = box_top + ascent * base_px + line_index as f32 * line_height;
            for character in line {
                if character.ch == ' ' {
                    // 空格没有轮廓、`glyph()` 把它记成 missing，但它的 hmtx 仍要推进，
                    // 否则整句空格零宽（"Rust Lua" 画成 "RustLua"），与 SDF 档不一致。
                    x += font.advance_em(' ') * s + style.spacing;
                    continue;
                }
                let Some(glyph) = font.glyph(character.ch).cloned() else {
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
                let cy = baseline + (min_y + max_y) * 0.5 * s;
                let w = ink_w.max(1.001);
                let h = ink_h.max(1.001);
                let shifts = [0.0_f32, bold_shift];
                let shift_count = if character.bold { 2 } else { 1 };
                for shift in &shifts[..shift_count] {
                    let x0 = cx - w * 0.5 + shift;
                    let y0 = cy - h * 0.5;
                    let x1 = cx + w * 0.5 + shift;
                    let y1 = cy + h * 0.5;
                    let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
                    for &p in &[
                        corners[0], corners[1], corners[2], corners[0], corners[2], corners[3],
                    ] {
                        out.push(VectorTextVertex {
                            position: p,
                            local: [(p[0] - x - shift) / s, (p[1] - baseline) / s],
                            glyph_id: glyph.id,
                            color: character.color,
                        });
                    }
                }
                x += glyph.advance * s + style.spacing;
            }
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
        if !origin[0].is_finite() || !origin[1].is_finite() || !style.is_valid() {
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
        let lines = {
            let mut advance = |character: char| font.advance(character) * tex_scale + style.spacing;
            layout_lines(text, &style, &mut advance, "…")
        };
        let first_line_height = ascent * base_px - descent * base_px;
        let visible = visible_lines(&style, lines.len(), line_height, first_line_height);
        let lines = &lines[..visible];
        let content_width = lines
            .iter()
            .map(|line| {
                line.iter()
                    .map(|character| font.advance(character.ch) * tex_scale + style.spacing)
                    .sum::<f32>()
            })
            .fold(0.0_f32, f32::max);
        let content_height = if visible == 0 {
            0.0
        } else {
            first_line_height + (visible as f32 - 1.0) * line_height
        };
        let box_top = origin[1]
            + style
                .valign
                .offset(style.box_height().unwrap_or(content_height), content_height);
        let bold_shift = BOLD_SHIFT_EM * base_px;
        let mut glyph_count = 0;
        let mut vertices = Vec::with_capacity(text.chars().count() * 6);
        for (line_index, line) in lines.iter().enumerate() {
            let line_width = line
                .iter()
                .map(|character| font.advance(character.ch) * tex_scale + style.spacing)
                .sum::<f32>();
            let line_offset = style
                .align
                .offset(style.box_width().unwrap_or(content_width), line_width);
            let baseline = box_top + ascent * base_px + line_index as f32 * line_height;
            let mut x = origin[0] + line_offset;
            for character in line {
                if character.ch == ' ' {
                    // 空格没有字形/轮廓：只推进，不绘制（避免渲染成占位方框）。
                    x += font.advance(' ') * tex_scale + style.spacing;
                    continue;
                }
                let metrics = font.try_glyph_metrics(character.ch)?;
                // 字形在整张图集中的 UV = 单元 UV + 单元内偏移在图集中的比例。
                // 四边形尺寸 = 字形显示尺寸。
                let rect = metrics.cell_rect;
                let atlas_w = font.width() as f32;
                let atlas_h = font.height() as f32;
                let mut glyph_w = (rect[2] - rect[0]) * tex_scale;
                let mut glyph_h = (rect[3] - rect[1]) * tex_scale;
                let mut x0 = x + metrics.bounds[0] * tex_scale;
                let mut y0 = baseline + metrics.bounds[1] * tex_scale;
                let (u0, v0, u1, v1);
                if pixel {
                    // 整数吸附：点阵的一个 texel 必须正好盖住整数个设备像素。次像素
                    // 定位下 Nearest 采样会把边界 texel 采到两次（或零次），字形边缘
                    // 出现"抖一格"，判据见 `jhyt_m72_pixel_font` 的 1:1 覆盖计数。
                    x0 = x0.round();
                    y0 = y0.round();
                    glyph_w = glyph_w.round();
                    glyph_h = glyph_h.round();
                    u0 = metrics.uv[0] + rect[0] / atlas_w;
                    v0 = metrics.uv[1] + rect[1] / atlas_h;
                    u1 = metrics.uv[0] + rect[2] / atlas_w;
                    v1 = metrics.uv[1] + rect[3] / atlas_h;
                } else if glyph_w < 1.001 || glyph_h < 1.001 {
                    // 墨迹窄于 1px 的笔画会让四边形一个像素中心都盖不住。实测
                    // Noto Sans SC 的 `I` 在 18px 只有 0.67px 宽、`-` 只有 0.63px 高，
                    // WebGL2 强制走 SDF 时整字消失（桌面矢量档掩盖了这个问题）。
                    // 这里改铺**整个带 padding 的格**：四边形足够大，墨迹仍按原尺寸
                    // 落在格内，边缘由 SDF 的过渡区解出——不能只把墨迹 UV 拉伸到
                    // 1px，否则笔画会变粗，且 1.001px 仍可能压不到采样中心。
                    let cell_w = (metrics.uv[2] - metrics.uv[0]) * atlas_w;
                    let cell_h = (metrics.uv[3] - metrics.uv[1]) * atlas_h;
                    glyph_w = cell_w * tex_scale;
                    glyph_h = cell_h * tex_scale;
                    x0 = x + (metrics.bounds[0] - rect[0]) * tex_scale;
                    y0 = baseline + (metrics.bounds[1] - rect[1]) * tex_scale;
                    u0 = metrics.uv[0];
                    v0 = metrics.uv[1];
                    u1 = metrics.uv[2];
                    v1 = metrics.uv[3];
                } else {
                    u0 = metrics.uv[0] + rect[0] / atlas_w;
                    v0 = metrics.uv[1] + rect[1] / atlas_h;
                    u1 = metrics.uv[0] + rect[2] / atlas_w;
                    v1 = metrics.uv[1] + rect[3] / atlas_h;
                }
                let shifts: &[f32] = if character.bold {
                    &[0.0, bold_shift]
                } else {
                    &[0.0]
                };
                for shift in shifts {
                    let sx0 = x0 + shift;
                    let x1 = sx0 + glyph_w;
                    let y1 = y0 + glyph_h;
                    let quad = [
                        [sx0, y0, u0, v0],
                        [x1, y0, u1, v0],
                        [x1, y1, u1, v1],
                        [sx0, y1, u0, v1],
                    ];
                    let make = |v: [f32; 4]| TextVertex {
                        position: [v[0], v[1]],
                        uv: [v[2], v[3]],
                        color: character.color,
                    };
                    vertices.extend_from_slice(&[
                        make(quad[0]),
                        make(quad[1]),
                        make(quad[2]),
                        make(quad[0]),
                        make(quad[2]),
                        make(quad[3]),
                    ]);
                }
                x += metrics.advance * tex_scale + style.spacing;
                glyph_count += 1;
            }
        }
        Ok(TextMesh {
            vertices,
            glyph_count,
            line_count: visible.max(1),
            size: [content_width, content_height],
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
    fn builtin_atlas_uv_covers_only_the_5x7_glyph() {
        let atlas = TextAtlas::built_in();
        let (min, max) = atlas.uv('P');
        let texels = [
            (max[0] - min[0]) * atlas.width() as f32,
            (max[1] - min[1]) * atlas.height() as f32,
        ];
        assert!((texels[0] - GLYPH_W as f32).abs() < 1e-4);
        assert!((texels[1] - GLYPH_H as f32).abs() < 1e-4);

        // A 5x7 quad must sample exactly one texel per pixel column at scale 1,
        // otherwise Nearest drops the glyph's right stem (P renders as F).
        let mesh = TextLayout::build_bitmap("P", [0.0, 0.0], TextStyle::default(), &atlas).unwrap();
        let u_max = mesh.vertices[1].uv[0];
        let u_min = mesh.vertices[0].uv[0];
        let quad_width = mesh.vertices[1].position[0] - mesh.vertices[0].position[0];
        assert_eq!(quad_width, GLYPH_W as f32);
        assert!((u_max - u_min) * atlas.width() as f32 <= quad_width);
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

    /// SDF 档的同一道门：Noto Sans SC 的 `I` 在 18px 只有 0.67px 宽、`-` 只有
    /// 0.63px 高——紧贴墨迹的四边形可以一个采样中心都不落进去，字就整条消失
    /// （WebGL2 强制走 SDF 时实测）。窄墨迹必须铺满带 padding 的整格。
    #[test]
    fn sdf_quads_never_fall_below_one_pixel() {
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&font_path).unwrap();
        let mut font = crate::font::FontAtlas::load(&bytes).unwrap();
        let style = TextStyle {
            scale: 18.0 / EM_PX,
            ..Default::default()
        };
        let mesh = TextLayout::build("I-", [0.0, 0.0], style, &mut font).unwrap();
        assert_eq!(mesh.glyph_count, 2);
        for (index, quad) in mesh.vertices.chunks(6).enumerate() {
            let min_x = quad.iter().map(|v| v.position[0]).fold(f32::MAX, f32::min);
            let max_x = quad.iter().map(|v| v.position[0]).fold(f32::MIN, f32::max);
            let min_y = quad.iter().map(|v| v.position[1]).fold(f32::MAX, f32::min);
            let max_y = quad.iter().map(|v| v.position[1]).fold(f32::MIN, f32::max);
            assert!(
                max_x - min_x >= 1.0,
                "第 {index} 个字形四边形只有 {} px 宽，可能一个采样中心都盖不住",
                max_x - min_x
            );
            assert!(
                max_y - min_y >= 1.0,
                "第 {index} 个字形四边形只有 {} px 高，可能一个采样中心都盖不住",
                max_y - min_y
            );
        }
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
        let bad_box = TextStyle {
            box_size: Some([-1.0, 10.0]),
            ..TextStyle::default()
        };
        assert_eq!(
            TextLayout::build_bitmap("A", [0.0, 0.0], bad_box, &atlas),
            Err(TextError::InvalidStyle)
        );
    }

    /// 位图档：按盒宽贪心换行（空格处断行）、水平/垂直对齐、以及行高一致性。
    #[test]
    fn bitmap_wrap_and_alignment_use_the_box() {
        let atlas = TextAtlas::built_in();
        let wrapped = TextStyle {
            spacing: 0.0,
            line_spacing: 0.0,
            box_size: Some([12.0, 100.0]),
            overflow: TextOverflow::Wrap,
            ..Default::default()
        };
        let mesh = TextLayout::build_bitmap("AB CD", [0.0, 0.0], wrapped, &atlas).unwrap();
        assert_eq!(mesh.line_count, 2);
        assert_eq!(mesh.glyph_count, 4);
        assert_eq!(mesh.size, [12.0, 15.0]);
        let second_line_top = mesh
            .vertices
            .iter()
            .map(|vertex| vertex.position[1])
            .filter(|y| *y >= 8.0)
            .fold(f32::INFINITY, f32::min);
        assert_eq!(
            second_line_top, 8.0,
            "第二行应落在 8px 行高之后（首行 7px 字模 + 1px 行距）"
        );

        let centered = TextStyle {
            spacing: 0.0,
            line_spacing: 0.0,
            box_size: Some([24.0, 16.0]),
            align: TextAlign::Center,
            valign: TextVAlign::Middle,
            ..Default::default()
        };
        let mesh = TextLayout::build_bitmap("AB", [10.0, 20.0], centered, &atlas).unwrap();
        let bounds = mesh.pixel_bounds();
        assert!((bounds[0] - 16.0).abs() < 1e-6, "bounds={bounds:?}");
        assert!((bounds[1] - 24.5).abs() < 1e-6, "bounds={bounds:?}");
    }

    /// 截断与省略号：省略号必须计入宽度预算，且不能被截成半截。
    #[test]
    fn truncate_and_ellipsis_reserve_the_suffix() {
        let atlas = TextAtlas::built_in();
        let truncated = TextStyle {
            spacing: 0.0,
            box_size: Some([24.0, 100.0]),
            overflow: TextOverflow::Truncate,
            ..Default::default()
        };
        let mesh = TextLayout::build_bitmap("ABCDEF", [0.0, 0.0], truncated, &atlas).unwrap();
        assert_eq!(mesh.glyph_count, 4);
        assert_eq!(mesh.size[0], 24.0);

        let ellipsis = TextStyle {
            overflow: TextOverflow::Ellipsis,
            ..truncated
        };
        let mesh = TextLayout::build_bitmap("ABCDEF", [0.0, 0.0], ellipsis, &atlas).unwrap();
        assert_eq!(mesh.glyph_count, 4, "一个字符 + 三个省略点");
        assert_eq!(mesh.size[0], 24.0);
    }

    /// 垂直溢出截断：放不下的整行丢弃，不画半行。
    #[test]
    fn vertical_truncate_drops_whole_lines() {
        let atlas = TextAtlas::built_in();
        let style = TextStyle {
            spacing: 0.0,
            line_spacing: 0.0,
            box_size: Some([100.0, 15.0]),
            voverflow: TextVOverflow::Truncate,
            ..Default::default()
        };
        let mesh = TextLayout::build_bitmap("A\nB\nC", [0.0, 0.0], style, &atlas).unwrap();
        assert_eq!(mesh.glyph_count, 2);
        assert_eq!(mesh.line_count, 2);
    }

    /// 富文本：颜色按标签分段，`<b>` 用同字形的加粗副本（字形数不变）。
    #[test]
    fn rich_text_colors_and_synthetic_bold() {
        let atlas = TextAtlas::built_in();
        let style = TextStyle {
            spacing: 0.0,
            rich_text: true,
            ..Default::default()
        };
        let mesh = TextLayout::build_bitmap(
            "A<color=#ff0000>BC</color><b>D</b>",
            [0.0, 0.0],
            style,
            &atlas,
        )
        .unwrap();
        assert_eq!(mesh.glyph_count, 4);
        assert_eq!(mesh.vertices.len(), 6 * 5, "A/B/C 各一块，D 有加粗副本");
        let red = [1.0, 0.0, 0.0, 1.0];
        assert_eq!(mesh.vertices[6].color, red, "B 必须是标签颜色");
        assert_eq!(mesh.vertices[12].color, red, "C 必须是标签颜色");
        assert_ne!(mesh.vertices[0].color, red, "A 在标签外保持基础色");
        // 未知标签按字面量保留，不能被吞掉。
        let literal = TextLayout::build_bitmap("a<script>x", [0.0, 0.0], style, &atlas).unwrap();
        assert_eq!(literal.glyph_count, "a<script>x".chars().count());
    }

    /// SDF 路径的换行/对齐必须与位图档同口径；度量与渲染共用同一份排版。
    #[test]
    fn sdf_wrap_matches_the_measured_block() {
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&font_path).unwrap();
        let mut font = crate::font::FontAtlas::load(&bytes).unwrap();
        let style = TextStyle {
            scale: 1.0,
            box_size: Some([60.0, 200.0]),
            overflow: TextOverflow::Wrap,
            align: TextAlign::Center,
            rich_text: true,
            ..Default::default()
        };
        let mesh = TextLayout::build("Hello world", [100.0, 0.0], style, &mut font).unwrap();
        assert!(mesh.line_count >= 2, "60px 盒宽装不下整句");
        assert_eq!(mesh.line_count, 2);
        assert!(mesh.size[0] <= 60.0, "换行后每行都不超过盒宽");
        let bounds = mesh.pixel_bounds();
        assert!(
            bounds[0] > 100.0 && bounds[2] <= 160.0,
            "居中后的字形必须落在盒内：{bounds:?}"
        );
        // 富文本标签不占宽度。
        let plain =
            TextLayout::build("Hello", [0.0, 0.0], TextStyle::default(), &mut font).unwrap();
        let tagged = TextLayout::build(
            "<b>Hello</b>",
            [0.0, 0.0],
            TextStyle {
                rich_text: true,
                ..Default::default()
            },
            &mut font,
        )
        .unwrap();
        assert!((plain.size[0] - tagged.size[0]).abs() < 1e-4);
    }

    /// 矢量档同样吃盒宽/对齐：居中的字形不能从盒左边缘起笔。
    #[test]
    fn vector_alignment_respects_the_box() {
        let font_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&font_path).unwrap();
        let mut vector = crate::font::FontVectorAtlas::load(&bytes).unwrap();
        let style = TextStyle {
            scale: 2.0,
            box_size: Some([400.0, 80.0]),
            align: TextAlign::Center,
            valign: TextVAlign::Middle,
            ..Default::default()
        };
        let run = TextLayout::build_vector("Hi", [50.0, 10.0], style, &mut vector).unwrap();
        let min_x = run
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::INFINITY, f32::min);
        let min_y = run
            .iter()
            .map(|vertex| vertex.position[1])
            .fold(f32::INFINITY, f32::min);
        assert!(min_x > 200.0, "居中后应越过盒中线：min_x={min_x}");
        assert!(min_y > 10.0, "垂直居中后应下移：min_y={min_y}");
    }
}
