//! Screen-space UI visual components and runtime-only dynamic UI state.
//!
//! Scene `Text` / `Image` components hold visual properties only; geometry is
//! owned by `UiLayout` (four-edge Anchor+Offset). Dynamic Lua UI owns its full
//! runtime rect and uses the Canvas scale helpers below.

use std::{error::Error, fmt};

/// Errors while parsing a UI component string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiComponentError {
    /// A field was not `key=value` as the scene format requires.
    MalformedField(String),
    /// A required field was missing.
    MissingField(&'static str),
    /// A numeric or color field could not be parsed / was invalid.
    InvalidValue(String),
}

impl fmt::Display for UiComponentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedField(field) => write!(f, "malformed UI field: {field}"),
            Self::MissingField(field) => write!(f, "missing UI field: {field}"),
            Self::InvalidValue(value) => write!(f, "invalid UI value: {value}"),
        }
    }
}
impl Error for UiComponentError {}

/// Authored layout space for scene UI components.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiLayoutSpace {
    /// Raw viewport pixels; values are used as-authored.
    Screen,
    /// Reference-resolution canvas (1280x720) scaled to the live viewport.
    Canvas,
}

impl UiLayoutSpace {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Canvas => "canvas",
        }
    }
}

/// 默认的 Canvas 参考分辨率（设计分辨率）。发布游戏未显式配置时的回退值；
/// 编辑器按设计分辨率（preview_resolution）显式传入，见 `resolve_ui_rect`。
pub const UI_REFERENCE_SIZE: [f32; 2] = [1280.0, 720.0];

/// `scale = 1.0` 对应的像素字号（SDF 字体下 em 高度，见 [`crate::core::text::EM_PX`]）。
/// `font_size` 字段按此换算为渲染 scale。
pub const UI_TEXT_BASE_PX: f32 = crate::core::text::EM_PX;

/// Scene UI `Text` rendering scale derived from the pixel font size.
#[must_use]
pub fn ui_text_effective_scale(font_size: f32) -> f32 {
    font_size / UI_TEXT_BASE_PX
}

/// Screen-space text label (`Text` component).
#[derive(Clone, Debug, PartialEq)]
pub struct UiText {
    pub text: String,
    /// Pixel font size (`SDF em height`, converted with `UI_TEXT_BASE_PX`).
    pub font_size: f32,
    /// RGB tint; default white.
    pub color: [f32; 3],
    /// 自定义字体：项目相对路径（如 `assets/fonts/MyFont.ttf`）；空 = 项目默认字体。
    pub font: String,
}

/// Screen-space textured quad (`Image` component).
#[derive(Clone, Debug, PartialEq)]
pub struct UiImage {
    /// Project-relative texture path.
    pub texture: String,
    /// RGB tint; default white.
    pub color: [f32; 3],
    /// 九宫格边框（纹理像素）`[left, right, top, bottom]`；`None` = 单四边形拉伸（默认）。
    pub nine_slice: Option<[f32; 4]>,
}

/// Runtime-only screen-space textured quad created by Lua (`ui.image`).
///
/// Scene `Image` components are visual-only; scene geometry comes from
/// `UiLayout`. Dynamic UI owns its full screen-space rect.
#[derive(Clone, Debug, PartialEq)]
pub struct DynamicUiImage {
    /// Project-relative texture path.
    pub texture: String,
    pub position: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 3],
    pub space: UiLayoutSpace,
    pub anchor: [f32; 2],
    pub nine_slice: Option<[f32; 4]>,
}

/// Runtime-only screen-space panel created by Lua (`ui.panel`).
#[derive(Clone, Debug, PartialEq)]
pub struct UiPanel {
    pub position: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
    pub visible: bool,
    pub space: UiLayoutSpace,
    pub anchor: [f32; 2],
}

/// One rendered cell inside a Lua-created [`UiGrid`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiGridCell {
    pub texture: Option<String>,
    pub color: [f32; 4],
}

/// Runtime-only inventory-style grid created by Lua (`ui.grid`). Cells are
/// drawn as tinted quads using `texture` when provided, otherwise a flat color.
#[derive(Clone, Debug, PartialEq)]
pub struct UiGrid {
    pub position: [f32; 2],
    pub columns: usize,
    pub rows: usize,
    pub cell_size: [f32; 2],
    pub gap: f32,
    pub background: [f32; 4],
    pub cells: Vec<UiGridCell>,
    pub space: UiLayoutSpace,
    pub anchor: [f32; 2],
}

impl UiGrid {
    /// Authored-space total grid size including gaps between cells.
    #[must_use]
    pub fn total_size(&self) -> [f32; 2] {
        let columns = self.columns.max(1) as f32;
        let rows = self.rows.max(1) as f32;
        [
            columns * self.cell_size[0] + self.gap * (columns - 1.0),
            rows * self.cell_size[1] + self.gap * (rows - 1.0),
        ]
    }

    /// Resolves the pixel rect for a 1-based cell index, or `None` when the
    /// index is outside the grid.
    #[must_use]
    pub fn cell_rect(
        &self,
        index: usize,
        viewport: [f32; 2],
        reference: [f32; 2],
    ) -> Option<([f32; 2], [f32; 2])> {
        if index == 0 || index > self.columns * self.rows {
            return None;
        }
        let authored_total = self.total_size();
        let (origin, resolved_total) = resolve_ui_rect(
            self.position,
            authored_total,
            self.space,
            self.anchor,
            viewport,
            reference,
        );
        let columns = self.columns.max(1) as f32;
        let rows = self.rows.max(1) as f32;
        let resolved_gap = self.gap * (resolved_total[0] / authored_total[0].max(f32::EPSILON));
        let cell_w = (resolved_total[0] - resolved_gap * (columns - 1.0)) / columns;
        let cell_h = (resolved_total[1] - resolved_gap * (rows - 1.0)) / rows;
        let zero = index - 1;
        let col = (zero % self.columns.max(1)) as f32;
        let row = (zero / self.columns.max(1)) as f32;
        Some((
            [
                origin[0] + col * (cell_w + resolved_gap),
                origin[1] + row * (cell_h + resolved_gap),
            ],
            [cell_w, cell_h],
        ))
    }
}

fn parse_number(raw: &str) -> Result<f32, UiComponentError> {
    let value = raw
        .parse::<f32>()
        .map_err(|_| UiComponentError::InvalidValue(raw.to_owned()))?;
    if !value.is_finite() {
        return Err(UiComponentError::InvalidValue(raw.to_owned()));
    }
    Ok(value)
}

/// Parses an `r,g,b` color triple (each in `0..=1`).
fn parse_rgb(raw: &str) -> Result<[f32; 3], UiComponentError> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 3 {
        return Err(UiComponentError::InvalidValue(raw.to_owned()));
    }
    Ok([
        parse_number(parts[0])?,
        parse_number(parts[1])?,
        parse_number(parts[2])?,
    ])
}

/// Canvas 空间 authored → 屏幕像素的每轴缩放：`viewport / reference`。
/// `reference` 是 Canvas 参考分辨率（设计分辨率，由调用方提供）；Screen 空间恒 1:1。
fn authored_scale(space: UiLayoutSpace, viewport: [f32; 2], reference: [f32; 2]) -> [f32; 2] {
    match space {
        UiLayoutSpace::Screen => [1.0, 1.0],
        UiLayoutSpace::Canvas => [viewport[0] / reference[0], viewport[1] / reference[1]],
    }
}

#[must_use]
pub fn resolve_ui_rect(
    position: [f32; 2],
    size: [f32; 2],
    space: UiLayoutSpace,
    anchor: [f32; 2],
    viewport: [f32; 2],
    reference: [f32; 2],
) -> ([f32; 2], [f32; 2]) {
    let scale = authored_scale(space, viewport, reference);
    let resolved_size = [size[0] * scale[0], size[1] * scale[1]];
    let anchor_point = [viewport[0] * anchor[0], viewport[1] * anchor[1]];
    let offset = [position[0] * scale[0], position[1] * scale[1]];
    // origin 对齐到设备像素（viewport 像素），使 Nearest 采样的 UI 图片贴合
    // texel 网格，消除非整数缩放下图片边缘的锯齿/模糊。size 不做舍入——
    // Canvas 缩放的分数尺寸若舍入会破坏宽高比与相邻 UI 的对齐。
    let origin = [
        (anchor_point[0] + offset[0] - resolved_size[0] * anchor[0]).round(),
        (anchor_point[1] + offset[1] - resolved_size[1] * anchor[1]).round(),
    ];
    (origin, resolved_size)
}

/// One 9-slice cell: destination rect (relative to the image top-left, in the
/// same unit as `dest_width`/`dest_height`, y-down) and source UV rect (0..1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NineSliceCell {
    /// `[x0, y0, x1, y1]` relative to the image top-left (y-down).
    pub dest: [f32; 4],
    /// `[u0, v0, u1, v1]` normalized source UVs (0..1).
    pub src: [f32; 4],
}

/// Computes the 3×3 nine-slice grid for a UI image.
///
/// `slice` = `[left, right, top, bottom]` in the same unit as `dest_width` /
/// `dest_height` (callers map authored borders into destination space first).
/// Corners never overlap: when the destination is smaller than the corner sum
/// the borders shrink proportionally, and when the texture is smaller than the
/// borders the source clamps/shrinks so source rects never go negative. The
/// center collapses to zero area in those degenerate cases but is still
/// returned (callers skip zero-area cells). Returns an empty `Vec` on
/// non-positive dimensions.
#[must_use]
pub fn nine_slice_cells(
    dest_width: f32,
    dest_height: f32,
    texture_width: f32,
    texture_height: f32,
    slice: [f32; 4],
) -> Vec<NineSliceCell> {
    if dest_width <= 0.0 || dest_height <= 0.0 || texture_width <= 0.0 || texture_height <= 0.0 {
        return Vec::new();
    }
    let (l, r, t, b) = (slice[0], slice[1], slice[2], slice[3]);

    // --- Source columns/rows in texture pixels ------------------------------
    let sl = l.clamp(0.0, texture_width);
    let sr = r.clamp(0.0, texture_width);
    let st = t.clamp(0.0, texture_height);
    let sb = b.clamp(0.0, texture_height);
    let col_f = if sl + sr > texture_width && sl + sr > 0.0 {
        texture_width / (sl + sr)
    } else {
        1.0
    };
    let row_f = if st + sb > texture_height && st + sb > 0.0 {
        texture_height / (st + sb)
    } else {
        1.0
    };
    let src_cols = [
        sl * col_f,
        (texture_width - sl * col_f - sr * col_f).max(0.0),
        sr * col_f,
    ];
    let src_rows = [
        st * row_f,
        (texture_height - st * row_f - sb * row_f).max(0.0),
        sb * row_f,
    ];

    // --- Destination columns/rows in dest units -----------------------------
    let dst_col_f = if l + r > dest_width && l + r > 0.0 {
        dest_width / (l + r)
    } else {
        1.0
    };
    let dst_row_f = if t + b > dest_height && t + b > 0.0 {
        dest_height / (t + b)
    } else {
        1.0
    };
    let dst_cols = [
        l * dst_col_f,
        (dest_width - l * dst_col_f - r * dst_col_f).max(0.0),
        r * dst_col_f,
    ];
    let dst_rows = [
        t * dst_row_f,
        (dest_height - t * dst_row_f - b * dst_row_f).max(0.0),
        b * dst_row_f,
    ];

    // --- Emit the 3×3 grid, row-major with cumulative offsets --------------
    let mut cells = Vec::with_capacity(9);
    let mut y_src = 0.0;
    let mut y_dst = 0.0;
    for row in 0..3 {
        let mut x_src = 0.0;
        let mut x_dst = 0.0;
        for col in 0..3 {
            let cw = src_cols[col];
            let ch = src_rows[row];
            cells.push(NineSliceCell {
                dest: [x_dst, y_dst, x_dst + dst_cols[col], y_dst + dst_rows[row]],
                src: [
                    x_src / texture_width,
                    y_src / texture_height,
                    (x_src + cw) / texture_width,
                    (y_src + ch) / texture_height,
                ],
            });
            x_src += cw;
            x_dst += dst_cols[col];
        }
        y_src += src_rows[row];
        y_dst += dst_rows[row];
    }
    cells
}

/// Splits a component string on `;` only when NOT preceded by a backslash
/// escape (so escaped `\;` inside text fields survives intact).
/// 按 `;` 切分组件字段，但**尊重转义**（`text` 值里的 `\;` 不算分隔符）。
///
/// 对 crate 内其它组件公开：任何要复用 `UiText` 字段的外部组件（如 `WorldText`）
/// 都必须用同一套切分，否则带分号的文字会被切坏。
pub(crate) fn split_fields(value: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let bytes = value.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b';' {
            let mut backslashes = 0;
            let mut j = i;
            while j > start && bytes[j - 1] == b'\\' {
                backslashes += 1;
                j -= 1;
            }
            if backslashes % 2 == 0 {
                fields.push(&value[start..i]);
                start = i + 1;
            }
        }
        i += 1;
    }
    fields.push(&value[start..]);
    fields
}

/// Escapes `\`, `;`, `=` inside a free-form text field so it survives the
/// `;`-separated component encoding.
fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            '=' => out.push_str("\\="),
            _ => out.push(ch),
        }
    }
    out
}

/// Inverse of [`escape_text`]; unknown `\X` sequences pass through unchanged.
fn unescape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some(';') => out.push(';'),
                Some('=') => out.push('='),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

impl UiText {
    /// Parses `text=<esc>;font_size=..;color=r,g,b[;font=<esc>]`.
    pub fn from_component_str(value: &str) -> Result<Self, UiComponentError> {
        let mut text = String::new();
        let mut font_size = None;
        let mut color = [1.0, 1.0, 1.0];
        let mut font = String::new();
        let mut seen = [false; 2]; // text, font_size
        for field in split_fields(value) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| UiComponentError::MalformedField(field.to_owned()))?;
            match key {
                "text" => {
                    text = unescape_text(raw);
                    seen[0] = true;
                }
                "font_size" => {
                    let px = parse_number(raw)?;
                    if px <= 0.0 {
                        return Err(UiComponentError::InvalidValue(raw.to_owned()));
                    }
                    font_size = Some(px);
                    seen[1] = true;
                }
                "color" => color = parse_rgb(raw)?,
                "font" => font = unescape_text(raw),
                _ => return Err(UiComponentError::MalformedField(field.to_owned())),
            }
        }
        if !seen[0] {
            return Err(UiComponentError::MissingField("text"));
        }
        if !seen[1] {
            return Err(UiComponentError::MissingField("font_size"));
        }
        Ok(Self {
            text,
            font_size: font_size.unwrap_or_default(),
            color,
            font,
        })
    }

    /// Encodes the canonical visual-only component string. `font` only emitted
    /// when non-empty so scenes without a custom font round-trip byte-identically.
    #[must_use]
    pub fn to_component_str(&self) -> String {
        let font = if self.font.is_empty() {
            String::new()
        } else {
            format!(";font={}", escape_text(&self.font))
        };
        format!(
            "text={};font_size={:.2};color={:.3},{:.3},{:.3}{font}",
            escape_text(&self.text),
            self.font_size,
            self.color[0],
            self.color[1],
            self.color[2],
        )
    }

    /// Rendering scale for the pixel font size.
    #[must_use]
    pub fn effective_scale(&self) -> f32 {
        ui_text_effective_scale(self.font_size)
    }
}

impl UiImage {
    /// Parses `texture=..;color=r,g,b;nine_slice=l,r,t,b`.
    pub fn from_component_str(value: &str) -> Result<Self, UiComponentError> {
        let mut texture = String::new();
        let mut color = [1.0, 1.0, 1.0];
        let mut nine_slice: Option<[f32; 4]> = None;
        let mut seen = [false; 1];
        for field in split_fields(value) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| UiComponentError::MalformedField(field.to_owned()))?;
            match key {
                "texture" => {
                    texture = raw.to_owned();
                    seen[0] = true;
                }
                "color" => color = parse_rgb(raw)?,
                "nine_slice" => {
                    let parts: Vec<&str> = raw.split(',').collect();
                    if parts.len() != 4 {
                        return Err(UiComponentError::InvalidValue(raw.to_owned()));
                    }
                    nine_slice = Some([
                        parse_number(parts[0])?,
                        parse_number(parts[1])?,
                        parse_number(parts[2])?,
                        parse_number(parts[3])?,
                    ]);
                }
                _ => return Err(UiComponentError::MalformedField(field.to_owned())),
            }
        }
        if !seen[0] {
            return Err(UiComponentError::MissingField("texture"));
        }
        Ok(Self {
            texture,
            color,
            nine_slice,
        })
    }

    /// Encodes back to the component string.
    #[must_use]
    pub fn to_component_str(&self) -> String {
        let nine_slice = match self.nine_slice {
            Some(s) => format!(
                "nine_slice={:.2},{:.2},{:.2},{:.2};",
                s[0], s[1], s[2], s[3]
            ),
            None => String::new(),
        };
        format!(
            "texture={};{nine_slice}color={:.3},{:.3},{:.3}",
            self.texture, self.color[0], self.color[1], self.color[2],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip_preserves_escaped_specials() {
        let ui = UiText {
            text: "Score;1=0 \\ 高".to_owned(),
            font_size: 24.0,
            color: [1.0, 0.5, 0.0],
            font: String::new(),
        };
        let encoded = ui.to_component_str();
        let decoded = UiText::from_component_str(&encoded).expect("valid text");
        assert_eq!(decoded.text, ui.text);
        assert_eq!(decoded.font_size, ui.font_size);
        assert_eq!(decoded.color, ui.color);
        // font_size 24px → scale 2.0（UI_TEXT_BASE_PX = 12）。
        assert_eq!(decoded.effective_scale(), 2.0);
    }

    #[test]
    fn text_font_empty_omits_font_key_and_stays_byte_stable() {
        let ui = UiText {
            text: "Hi".to_owned(),
            font_size: 14.0,
            color: [1.0, 1.0, 1.0],
            font: String::new(),
        };
        let encoded = ui.to_component_str();
        assert!(!encoded.contains("font="), "{encoded}");
        assert_eq!(encoded, "text=Hi;font_size=14.00;color=1.000,1.000,1.000");
    }

    #[test]
    fn text_legacy_without_font_parses_to_empty() {
        let ui = UiText::from_component_str("text=Hi;font_size=14").expect("valid");
        assert_eq!(ui.font, "");
    }

    #[test]
    fn text_custom_font_round_trips_with_escaping() {
        let ui = UiText {
            text: "Hi".to_owned(),
            font_size: 14.0,
            color: [1.0, 1.0, 1.0],
            font: "assets/fonts/a;b=数字.ttf".to_owned(),
        };
        let encoded = ui.to_component_str();
        assert!(
            encoded.contains("font=assets/fonts/a\\;b\\=数字.ttf"),
            "{encoded}"
        );
        let decoded = UiText::from_component_str(&encoded).expect("valid");
        assert_eq!(decoded, ui);
    }

    #[test]
    fn text_requires_visual_fields_and_uses_color_default() {
        let ui = UiText::from_component_str("text=Hello;font_size=14").expect("valid");
        assert_eq!(ui.text, "Hello");
        assert_eq!(ui.font_size, 14.0);
        assert_eq!(ui.color, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn text_rejects_missing_required_field_and_bad_number() {
        assert_eq!(
            UiText::from_component_str("text=Hi").unwrap_err(),
            UiComponentError::MissingField("font_size")
        );
        assert_eq!(
            UiText::from_component_str("text=Hi;font_size=abc").unwrap_err(),
            UiComponentError::InvalidValue("abc".to_owned())
        );
        assert_eq!(
            UiText::from_component_str("text=Hi;font_size=0").unwrap_err(),
            UiComponentError::InvalidValue("0".to_owned())
        );
    }

    #[test]
    fn image_round_trip() {
        let img = UiImage {
            texture: "assets/ui/panel.png".to_owned(),
            color: [0.5, 0.5, 1.0],
            nine_slice: None,
        };
        let decoded = UiImage::from_component_str(&img.to_component_str()).expect("valid");
        assert_eq!(decoded, img);
    }

    #[test]
    fn image_nine_slice_round_trip() {
        let img = UiImage {
            texture: "assets/ui/panel.png".to_owned(),
            color: [1.0, 1.0, 1.0],
            nine_slice: Some([8.0, 12.0, 16.0, 20.0]),
        };
        let encoded = img.to_component_str();
        assert!(
            encoded.contains("nine_slice=8.00,12.00,16.00,20.00"),
            "{encoded}"
        );
        let decoded = UiImage::from_component_str(&encoded).expect("valid");
        assert_eq!(decoded, img);
    }

    #[test]
    fn image_nine_slice_absent_when_none() {
        let img = UiImage {
            texture: "assets/ui/panel.png".to_owned(),
            color: [1.0, 1.0, 1.0],
            nine_slice: None,
        };
        let encoded = img.to_component_str();
        assert!(!encoded.contains("nine_slice"), "{encoded}");
    }

    #[test]
    fn image_rejects_malformed_nine_slice() {
        assert_eq!(
            UiImage::from_component_str("texture=a;nine_slice=1,2,3").unwrap_err(),
            UiComponentError::InvalidValue("1,2,3".to_owned())
        );
        assert!(UiImage::from_component_str("texture=a;nine_slice=1,2,3,abc").is_err());
    }

    #[test]
    fn nine_slice_corners_fixed_edges_stretch() {
        // Texture 192×64, border 16: corners stay 16×16, top edge spans the
        // full width, center covers the middle 160×32.
        let cells = nine_slice_cells(192.0, 64.0, 192.0, 64.0, [16.0, 16.0, 16.0, 16.0]);
        assert_eq!(cells.len(), 9);
        // TL corner.
        assert_eq!(cells[0].dest, [0.0, 0.0, 16.0, 16.0]);
        // Top edge (row 0, col 1): dest [16,0,176,16].
        assert_eq!(cells[1].dest, [16.0, 0.0, 176.0, 16.0]);
        // Center (row 1, col 1): dest [16,16,176,48].
        assert_eq!(cells[4].dest, [16.0, 16.0, 176.0, 48.0]);
        // TR corner source UV = right 16px of texture.
        assert_eq!(cells[2].src, [176.0 / 192.0, 0.0, 1.0, 16.0 / 64.0]);
        // Center source UV = middle 160×32.
        assert_eq!(
            cells[4].src,
            [16.0 / 192.0, 16.0 / 64.0, 176.0 / 192.0, 48.0 / 64.0]
        );
    }

    #[test]
    fn nine_slice_center_stretches_beyond_texture() {
        // Dest 384×128 from a 64×64 texture: corners stay 8×8, the middle
        // stretches to 368×112 while the source center is a fixed 48×48.
        let cells = nine_slice_cells(384.0, 128.0, 64.0, 64.0, [8.0, 8.0, 8.0, 8.0]);
        assert_eq!(cells.len(), 9);
        assert_eq!(cells[0].dest, [0.0, 0.0, 8.0, 8.0]);
        assert_eq!(cells[1].dest, [8.0, 0.0, 376.0, 8.0]);
        assert_eq!(cells[4].dest, [8.0, 8.0, 376.0, 120.0]);
        assert_eq!(
            cells[4].src,
            [8.0 / 64.0, 8.0 / 64.0, 56.0 / 64.0, 56.0 / 64.0]
        );
    }

    #[test]
    fn nine_slice_dest_smaller_than_corner_sum() {
        // Dest 24×24 with borders 16: borders shrink proportionally (0.75),
        // corners never overlap, center collapses to zero.
        let cells = nine_slice_cells(24.0, 24.0, 192.0, 64.0, [16.0, 16.0, 16.0, 16.0]);
        assert_eq!(cells.len(), 9);
        assert_eq!(cells[0].dest, [0.0, 0.0, 12.0, 12.0]);
        assert_eq!(cells[2].dest, [12.0, 0.0, 24.0, 12.0]);
        assert_eq!(cells[4].dest, [12.0, 12.0, 12.0, 12.0]); // degenerate center
    }

    #[test]
    fn nine_slice_texture_smaller_than_border() {
        // Texture 32×32 with borders 40: source clamps/shrinks to 16 each,
        // dest corners 40, center 20. Source UV covers half the texture.
        let cells = nine_slice_cells(100.0, 100.0, 32.0, 32.0, [40.0, 40.0, 40.0, 40.0]);
        assert_eq!(cells.len(), 9);
        assert_eq!(cells[0].src, [0.0, 0.0, 0.5, 0.5]);
        assert_eq!(cells[0].dest, [0.0, 0.0, 40.0, 40.0]);
        assert_eq!(cells[4].dest, [40.0, 40.0, 60.0, 60.0]);
    }

    #[test]
    fn nine_slice_degenerate_inputs_return_empty() {
        assert!(nine_slice_cells(0.0, 10.0, 10.0, 10.0, [1.0; 4]).is_empty());
        assert!(nine_slice_cells(10.0, 10.0, 0.0, 10.0, [1.0; 4]).is_empty());
    }

    #[test]
    fn text_survives_scene_file_round_trip() {
        // Multi-line / special text passes through Scene to_text/from_text.
        let ui = UiText {
            text: "L1\nL2=大;x".to_owned(),
            font_size: 14.0,
            color: [1.0, 1.0, 1.0],
            font: String::new(),
        };
        let mut scene = crate::core::scene::Scene::new("ui");
        scene
            .add_entity(crate::core::scene::SceneEntityId(1), "Label", None)
            .unwrap();
        scene
            .entities
            .get_mut(&crate::core::scene::SceneEntityId(1))
            .unwrap()
            .components
            .insert("Text".to_owned(), ui.to_component_str());
        let reloaded = crate::core::scene::Scene::from_text(&scene.to_text()).expect("reload");
        let value = reloaded.entities[&crate::core::scene::SceneEntityId(1)]
            .components
            .get("Text")
            .expect("Text component");
        let decoded = UiText::from_component_str(value).expect("valid text");
        assert_eq!(decoded.text, ui.text);
    }

    #[test]
    fn canvas_rect_follows_anchor_and_viewport() {
        let viewport = [1920.0, 1080.0];
        // 参考分辨率与视口不同 → canvas 每轴缩放 = viewport / reference。
        let (origin, size) = resolve_ui_rect(
            [0.0, 0.0],
            [320.0, 180.0],
            UiLayoutSpace::Canvas,
            [0.5, 0.5],
            viewport,
            [1280.0, 720.0],
        );
        assert_eq!(size, [480.0, 270.0]);
        assert_eq!(origin, [720.0, 405.0]);
    }

    #[test]
    fn canvas_reference_resolution_drives_adaptation_scale() {
        let viewport = [1280.0, 720.0];
        // 参考 = 视口：canvas 1:1。
        let (_, size) = resolve_ui_rect(
            [0.0, 0.0],
            [320.0, 180.0],
            UiLayoutSpace::Canvas,
            [0.0, 0.0],
            viewport,
            [1280.0, 720.0],
        );
        assert_eq!(size, [320.0, 180.0]);
        // 参考分辨率更大（1920×1080）：canvas 按视口/参考等比缩小。
        let (_, size) = resolve_ui_rect(
            [0.0, 0.0],
            [320.0, 180.0],
            UiLayoutSpace::Canvas,
            [0.0, 0.0],
            viewport,
            [1920.0, 1080.0],
        );
        assert!((size[0] - 213.3333).abs() < 0.001);
        assert!((size[1] - 120.0).abs() < 0.001);
        // Screen 空间不受参考分辨率影响（恒 1:1）。
        let (_, size) = resolve_ui_rect(
            [0.0, 0.0],
            [320.0, 180.0],
            UiLayoutSpace::Screen,
            [0.0, 0.0],
            viewport,
            [1920.0, 1080.0],
        );
        assert_eq!(size, [320.0, 180.0]);
    }

    #[test]
    fn resolve_ui_rect_snaps_origin_preserves_size() {
        // 非整数 origin → 舍入到设备像素；size 保持分数不动。
        let (origin, size) = resolve_ui_rect(
            [10.3, 4.7],
            [64.0, 64.0],
            UiLayoutSpace::Screen,
            [0.0, 0.0],
            [1280.0, 720.0],
            [1280.0, 720.0],
        );
        assert_eq!(origin, [10.0, 5.0]);
        assert_eq!(size, [64.0, 64.0]);
        // 负 origin 的舍入（round half away from zero）。
        let (origin, _) = resolve_ui_rect(
            [-10.5, -4.2],
            [32.0, 32.0],
            UiLayoutSpace::Screen,
            [0.0, 0.0],
            [1280.0, 720.0],
            [1280.0, 720.0],
        );
        assert_eq!(origin, [-11.0, -4.0]);
    }
}
