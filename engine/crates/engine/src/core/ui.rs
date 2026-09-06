//! Screen-space UI scene components: `Text`, `Image`, `Button`.
//!
//! These are ordinary scene-entity components (opaque `k=v;...` strings) that
//! render as NDC overlay quads on top of the scene. Coordinates use a top-left
//! pixel origin, but may additionally opt into a reference-resolution
//! "Canvas" space plus an anchor point so authored UI can scale more cleanly
//! across preview resolutions.

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

/// Shared authored layout data for screen-space UI components.
///
/// Existing `Text` and `Image` component formats still store these fields
/// inline; this value object gives editor and runtime code one layout contract
/// without requiring a scene-format migration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiLayout {
    pub position: [f32; 2],
    pub size: [f32; 2],
    pub space: UiLayoutSpace,
    pub anchor: [f32; 2],
}

impl UiLayout {
    #[must_use]
    pub fn from_image(image: &UiImage) -> Self {
        Self {
            position: image.position,
            size: image.size,
            space: image.space,
            anchor: image.anchor,
        }
    }

    #[must_use]
    pub fn with_position(mut self, position: [f32; 2]) -> Self {
        self.position = position;
        self
    }

    #[must_use]
    pub fn resolve(self, viewport: [f32; 2], reference: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        resolve_ui_rect(
            self.position,
            self.size,
            self.space,
            self.anchor,
            viewport,
            reference,
        )
    }
}

/// 默认的 Canvas 参考分辨率（设计分辨率）。发布游戏未显式配置时的回退值；
/// 编辑器按设计分辨率（preview_resolution）显式传入，见 `resolve_ui_rect`。
pub const UI_REFERENCE_SIZE: [f32; 2] = [1280.0, 720.0];

/// `scale = 1.0` 对应的像素字号（SDF 字体下 em 高度，见 [`crate::core::text::EM_PX`]）。
/// `font_size` 字段按此换算为渲染 scale。
pub const UI_TEXT_BASE_PX: f32 = crate::core::text::EM_PX;

/// 场景 UI `Text` 组件渲染/测量用的最终 glyph scale：`font_size` 提供时优先
/// （按 [`UI_TEXT_BASE_PX`] 从像素字号换算），否则退回 `scale` 字段。
#[must_use]
pub fn ui_text_effective_scale(scale: f32, font_size: Option<f32>) -> f32 {
    font_size.map_or(scale, |px| px / UI_TEXT_BASE_PX)
}

/// Screen-space text label (`Text` component).
#[derive(Clone, Debug, PartialEq)]
pub struct UiText {
    pub text: String,
    /// Offset from the chosen anchor point.
    pub position: [f32; 2],
    /// 像素字号（SDF em 高度，px）。提供时优先于 `scale`；
    /// 换算见 [`ui_text_effective_scale`]。
    pub font_size: Option<f32>,
    /// 字形缩放系数（`1.0` = SDF 字体 em 12px / 位图 5x7 字形 7px）。
    /// 无 `font_size` 时的降级因子；两种语义下 `scale=1.0` 的换算见
    /// [`UI_TEXT_BASE_PX`]。
    pub scale: f32,
    /// RGB tint; default white.
    pub color: [f32; 3],
    /// Screen or canvas authored space.
    pub space: UiLayoutSpace,
    /// Anchor in normalized viewport coordinates (`0..=1`).
    pub anchor: [f32; 2],
}

/// Screen-space textured quad (`Image` component).
#[derive(Clone, Debug, PartialEq)]
pub struct UiImage {
    /// Project-relative texture path.
    pub texture: String,
    /// Offset from the chosen anchor point.
    pub position: [f32; 2],
    /// Quad size in authored space pixels.
    pub size: [f32; 2],
    /// RGB tint; default white.
    pub color: [f32; 3],
    pub space: UiLayoutSpace,
    pub anchor: [f32; 2],
    /// 九宫格边框（纹理像素）`[left, right, top, bottom]`；`None` = 单四边形拉伸（默认）。
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

fn parse_layout_space(raw: &str) -> Result<UiLayoutSpace, UiComponentError> {
    match raw {
        "screen" => Ok(UiLayoutSpace::Screen),
        "canvas" => Ok(UiLayoutSpace::Canvas),
        _ => Err(UiComponentError::InvalidValue(raw.to_owned())),
    }
}

fn parse_anchor(raw: &str) -> Result<[f32; 2], UiComponentError> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 2 {
        return Err(UiComponentError::InvalidValue(raw.to_owned()));
    }
    let anchor = [parse_number(parts[0])?, parse_number(parts[1])?];
    validate_anchor(anchor)?;
    Ok(anchor)
}

fn validate_anchor(anchor: [f32; 2]) -> Result<(), UiComponentError> {
    if anchor[0].is_finite()
        && anchor[1].is_finite()
        && (0.0..=1.0).contains(&anchor[0])
        && (0.0..=1.0).contains(&anchor[1])
    {
        Ok(())
    } else {
        Err(UiComponentError::InvalidValue(format!(
            "{:.3},{:.3}",
            anchor[0], anchor[1]
        )))
    }
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

#[must_use]
pub fn resolve_ui_text_origin(
    position: [f32; 2],
    text_size: [f32; 2],
    scale: f32,
    space: UiLayoutSpace,
    anchor: [f32; 2],
    viewport: [f32; 2],
    reference: [f32; 2],
) -> ([f32; 2], f32) {
    let canvas_scale = authored_scale(space, viewport, reference);
    let resolved_scale = match space {
        UiLayoutSpace::Screen => scale,
        UiLayoutSpace::Canvas => scale * canvas_scale[0].min(canvas_scale[1]),
    };
    let anchor_point = [viewport[0] * anchor[0], viewport[1] * anchor[1]];
    let offset = [position[0] * canvas_scale[0], position[1] * canvas_scale[1]];
    let origin = [
        anchor_point[0] + offset[0] - text_size[0] * anchor[0],
        anchor_point[1] + offset[1] - text_size[1] * anchor[1],
    ];
    (origin, resolved_scale)
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
fn split_fields(value: &str) -> Vec<&str> {
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
    /// Parses
    /// `text=<esc>;x=..;y=..;font_size=..;color=r,g,b;space=..;anchor=x,y`.
    /// Legacy `scale` is accepted and converted to `font_size`.
    pub fn from_component_str(value: &str) -> Result<Self, UiComponentError> {
        let mut text = String::new();
        let mut position = [0.0; 2];
        let mut scale = 1.0;
        let mut font_size: Option<f32> = None;
        let mut color = [1.0, 1.0, 1.0];
        let mut space = UiLayoutSpace::Screen;
        let mut anchor = [0.0, 0.0];
        let mut seen = [false; 4]; // text, x, y
        for field in split_fields(value) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| UiComponentError::MalformedField(field.to_owned()))?;
            match key {
                "text" => {
                    text = unescape_text(raw);
                    seen[0] = true;
                }
                "x" => {
                    position[0] = parse_number(raw)?;
                    seen[1] = true;
                }
                "y" => {
                    position[1] = parse_number(raw)?;
                    seen[2] = true;
                }
                "scale" => {
                    scale = parse_number(raw)?;
                    if scale <= 0.0 {
                        return Err(UiComponentError::InvalidValue(raw.to_owned()));
                    }
                    seen[3] = true;
                }
                "font_size" => {
                    let px = parse_number(raw)?;
                    if px <= 0.0 {
                        return Err(UiComponentError::InvalidValue(raw.to_owned()));
                    }
                    font_size = Some(px);
                }
                "color" => color = parse_rgb(raw)?,
                "space" => space = parse_layout_space(raw)?,
                "anchor" => anchor = parse_anchor(raw)?,
                _ => return Err(UiComponentError::MalformedField(field.to_owned())),
            }
        }
        if !seen[0] {
            return Err(UiComponentError::MissingField("text"));
        }
        if !seen[1] || !seen[2] {
            return Err(UiComponentError::MissingField("x"));
        }
        let _ = seen[3];
        Ok(Self {
            text,
            position,
            scale,
            font_size,
            color,
            space,
            anchor,
        })
    }

    /// Encodes the canonical component string; legacy `scale` is not written.
    #[must_use]
    pub fn to_component_str(&self) -> String {
        let font_size = self
            .font_size
            .unwrap_or_else(|| self.scale * UI_TEXT_BASE_PX);
        format!(
            "text={};x={:.2};y={:.2};font_size={:.2};color={:.3},{:.3},{:.3};space={};anchor={:.3},{:.3}",
            escape_text(&self.text),
            self.position[0],
            self.position[1],
            font_size,
            self.color[0],
            self.color[1],
            self.color[2],
            self.space.as_str(),
            self.anchor[0],
            self.anchor[1]
        )
    }

    /// 渲染/测量用的最终 glyph scale：`font_size`（像素字号）优先，否则 `scale`。
    #[must_use]
    pub fn effective_scale(&self) -> f32 {
        ui_text_effective_scale(self.scale, self.font_size)
    }
}

impl UiImage {
    /// Parses `texture=..;x=..;y=..;width=..;height=..;color=r,g,b;space=..;anchor=x,y`
    /// (`color/space/anchor` optional).
    pub fn from_component_str(value: &str) -> Result<Self, UiComponentError> {
        let mut texture = String::new();
        let mut position = [0.0; 2];
        let mut size = [0.0; 2];
        let mut color = [1.0, 1.0, 1.0];
        let mut space = UiLayoutSpace::Screen;
        let mut anchor = [0.0, 0.0];
        let mut nine_slice: Option<[f32; 4]> = None;
        let mut seen = [false; 6];
        for field in split_fields(value) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| UiComponentError::MalformedField(field.to_owned()))?;
            match key {
                "texture" => {
                    texture = raw.to_owned();
                    seen[0] = true;
                }
                "x" => {
                    position[0] = parse_number(raw)?;
                    seen[1] = true;
                }
                "y" => {
                    position[1] = parse_number(raw)?;
                    seen[2] = true;
                }
                "width" => {
                    size[0] = parse_number(raw)?;
                    if size[0] <= 0.0 {
                        return Err(UiComponentError::InvalidValue(raw.to_owned()));
                    }
                    seen[3] = true;
                }
                "height" => {
                    size[1] = parse_number(raw)?;
                    if size[1] <= 0.0 {
                        return Err(UiComponentError::InvalidValue(raw.to_owned()));
                    }
                    seen[4] = true;
                }
                "color" => color = parse_rgb(raw)?,
                "space" => space = parse_layout_space(raw)?,
                "anchor" => anchor = parse_anchor(raw)?,
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
        for (field, present) in [
            ("texture", seen[0]),
            ("x", seen[1]),
            ("y", seen[2]),
            ("width", seen[3]),
            ("height", seen[4]),
        ] {
            if !present {
                return Err(UiComponentError::MissingField(field));
            }
        }
        Ok(Self {
            texture,
            position,
            size,
            color,
            space,
            anchor,
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
            "texture={};x={:.2};y={:.2};width={:.2};height={:.2};{nine_slice}color={:.3},{:.3},{:.3};space={};anchor={:.3},{:.3}",
            self.texture,
            self.position[0],
            self.position[1],
            self.size[0],
            self.size[1],
            self.color[0],
            self.color[1],
            self.color[2],
            self.space.as_str(),
            self.anchor[0],
            self.anchor[1]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_layout_resolves_image_like_legacy_path() {
        let image = UiImage {
            texture: "assets/ui.png".to_owned(),
            position: [24.0, 36.0],
            size: [160.0, 48.0],
            color: [1.0, 0.8, 0.6],
            space: UiLayoutSpace::Canvas,
            anchor: [0.5, 0.5],
            nine_slice: None,
        };
        let viewport = [1280.0, 720.0];
        let reference = [1280.0, 720.0];
        assert_eq!(
            UiLayout::from_image(&image).resolve(viewport, reference),
            resolve_ui_rect(
                image.position,
                image.size,
                image.space,
                image.anchor,
                viewport,
                reference,
            )
        );
    }

    #[test]
    fn text_round_trip_preserves_escaped_specials() {
        let ui = UiText {
            text: "Score;1=0 \\ 高".to_owned(),
            position: [12.0, 30.0],
            scale: 2.0,
            font_size: Some(24.0),
            color: [1.0, 0.5, 0.0],
            space: UiLayoutSpace::Canvas,
            anchor: [0.5, 0.0],
        };
        let encoded = ui.to_component_str();
        let decoded = UiText::from_component_str(&encoded).expect("valid text");
        assert_eq!(decoded.text, ui.text);
        assert_eq!(decoded.position, ui.position);
        assert_eq!(decoded.font_size, ui.font_size);
        assert_eq!(decoded.color, ui.color);
        // font_size 24px → scale 2.0（UI_TEXT_BASE_PX = 12）。
        assert_eq!(decoded.effective_scale(), 2.0);
    }

    #[test]
    fn text_font_size_takes_priority_over_scale() {
        // font_size 提供时优先于 scale。
        let ui = UiText::from_component_str("text=Hi;x=1;y=1;scale=3;font_size=12").expect("valid");
        assert_eq!(ui.scale, 3.0);
        assert_eq!(ui.font_size, Some(12.0));
        assert_eq!(ui.effective_scale(), 1.0);
        // 仅 scale（旧数据）：effective_scale = scale。
        let legacy = UiText::from_component_str("text=Hi;x=1;y=1;scale=2").expect("valid");
        assert_eq!(legacy.font_size, None);
        assert_eq!(legacy.effective_scale(), 2.0);
        // Legacy scale canonicalizes to the latest font_size field.
        assert_eq!(
            legacy.to_component_str(),
            "text=Hi;x=1.00;y=1.00;font_size=24.00;color=1.000,1.000,1.000;space=screen;anchor=0.000,0.000"
        );
        assert!(ui.to_component_str().contains("font_size=12.00"));
    }

    #[test]
    fn text_defaults_scale_and_color() {
        let ui = UiText::from_component_str("text=Hello;x=10;y=20").expect("valid");
        assert_eq!(ui.text, "Hello");
        assert_eq!(ui.position, [10.0, 20.0]);
        assert_eq!(ui.scale, 1.0);
        assert_eq!(ui.font_size, None);
        assert_eq!(ui.color, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn text_rejects_missing_required_field_and_bad_number() {
        assert_eq!(
            UiText::from_component_str("text=Hi;y=1").unwrap_err(),
            UiComponentError::MissingField("x")
        );
        assert_eq!(
            UiText::from_component_str("text=Hi;x=abc;y=1").unwrap_err(),
            UiComponentError::InvalidValue("abc".to_owned())
        );
        assert_eq!(
            UiText::from_component_str("text=Hi;x=1;y=1;scale=0").unwrap_err(),
            UiComponentError::InvalidValue("0".to_owned())
        );
        assert_eq!(
            UiText::from_component_str("text=Hi;x=1;y=1;font_size=0").unwrap_err(),
            UiComponentError::InvalidValue("0".to_owned())
        );
    }

    #[test]
    fn image_round_trip() {
        let img = UiImage {
            texture: "assets/ui/panel.png".to_owned(),
            position: [100.0, 50.0],
            size: [64.0, 64.0],
            color: [0.5, 0.5, 1.0],
            space: UiLayoutSpace::Canvas,
            anchor: [0.5, 0.5],
            nine_slice: None,
        };
        let decoded = UiImage::from_component_str(&img.to_component_str()).expect("valid");
        assert_eq!(decoded, img);
    }

    #[test]
    fn image_nine_slice_round_trip() {
        let img = UiImage {
            texture: "assets/ui/panel.png".to_owned(),
            position: [100.0, 50.0],
            size: [192.0, 64.0],
            color: [1.0, 1.0, 1.0],
            space: UiLayoutSpace::Canvas,
            anchor: [0.5, 0.5],
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
            position: [0.0, 0.0],
            size: [64.0, 64.0],
            color: [1.0, 1.0, 1.0],
            space: UiLayoutSpace::Screen,
            anchor: [0.0, 0.0],
            nine_slice: None,
        };
        let encoded = img.to_component_str();
        assert!(!encoded.contains("nine_slice"), "{encoded}");
        // Legacy string without the field parses to None.
        let legacy = "texture=assets/ui/panel.png;x=0;y=0;width=64;height=64";
        assert_eq!(
            UiImage::from_component_str(legacy)
                .expect("valid")
                .nine_slice,
            None
        );
    }

    #[test]
    fn image_rejects_malformed_nine_slice() {
        assert_eq!(
            UiImage::from_component_str("texture=a;x=0;y=0;width=10;height=10;nine_slice=1,2,3")
                .unwrap_err(),
            UiComponentError::InvalidValue("1,2,3".to_owned())
        );
        assert!(
            UiImage::from_component_str(
                "texture=a;x=0;y=0;width=10;height=10;nine_slice=1,2,3,abc"
            )
            .is_err()
        );
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
            position: [1.0, 2.0],
            scale: 1.0,
            font_size: None,
            color: [1.0, 1.0, 1.0],
            space: UiLayoutSpace::Screen,
            anchor: [0.0, 0.0],
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
    fn canvas_rect_and_text_origin_follow_anchor_and_viewport() {
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

        let (text_origin, scale) = resolve_ui_text_origin(
            [0.0, 0.0],
            [200.0, 50.0],
            1.0,
            UiLayoutSpace::Canvas,
            [0.5, 0.5],
            viewport,
            [1280.0, 720.0],
        );
        assert_eq!(text_origin, [860.0, 515.0]);
        assert!((scale - 1.5).abs() < 0.001);
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
