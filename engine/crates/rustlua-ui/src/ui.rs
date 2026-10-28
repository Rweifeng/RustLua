//! Screen-space UI visual components and runtime-only dynamic UI state.
//!
//! Scene `Text` / `Image` components hold visual properties only; geometry is
//! owned by `UiLayout` (four-edge Anchor+Offset). Dynamic Lua UI owns its full
//! runtime rect and uses the Canvas scale helpers below.

pub use rustlua_ecs::component_schema::{escape_text, split_fields, unescape_text};

/// Errors while parsing a UI component string.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum UiComponentError {
    /// A field was not `key=value` as the scene format requires.
    #[error("malformed UI field: {0}")]
    MalformedField(String),
    /// A required field was missing.
    #[error("missing UI field: {0}")]
    MissingField(&'static str),
    /// A numeric or color field could not be parsed / was invalid.
    #[error("invalid UI value: {0}")]
    InvalidValue(String),
}

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

/// `scale = 1.0` 对应的像素字号（SDF 字体下 em 高度）。
/// 字面量与渲染侧 `text::EM_PX` 同源于 `rustlua_math::text_metrics::TEXT_BASE_PX`。
/// `font_size` 字段按此换算为渲染 scale。
pub const UI_TEXT_BASE_PX: f32 = rustlua_math::text_metrics::TEXT_BASE_PX;

/// Scene UI `Text` rendering scale derived from the pixel font size.
#[must_use]
pub fn ui_text_effective_scale(font_size: f32) -> f32 {
    font_size / UI_TEXT_BASE_PX
}

/// Horizontal text alignment (`Text.align`)，与 Unity `TextAnchor` 的水平分量一致。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl TextAlign {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "left" => Some(Self::Left),
            "center" => Some(Self::Center),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// Vertical text alignment (`Text.valign`)。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextVAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

impl TextVAlign {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Middle => "middle",
            Self::Bottom => "bottom",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "top" => Some(Self::Top),
            "middle" => Some(Self::Middle),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }
}

/// 水平溢出策略（Unity `horizontalOverflow` + 省略号）。
///
/// 默认 `Wrap` 与 Unity 一致：有布局矩形时按宽换行；点尺寸（宽高都为 0）实体
/// 没有可换行的盒，行为与旧版相同。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextOverflow {
    /// 超出矩形继续画（Unity `Overflow`，旧版行为）。
    Overflow,
    #[default]
    Wrap,
    /// 截断到矩形宽度。
    Truncate,
    /// 截断并追加省略号。
    Ellipsis,
}

impl TextOverflow {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Overflow => "overflow",
            Self::Wrap => "wrap",
            Self::Truncate => "truncate",
            Self::Ellipsis => "ellipsis",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "overflow" => Some(Self::Overflow),
            "wrap" => Some(Self::Wrap),
            "truncate" => Some(Self::Truncate),
            "ellipsis" => Some(Self::Ellipsis),
            _ => None,
        }
    }
}

/// 垂直溢出策略（Unity `verticalOverflow`）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextVOverflow {
    #[default]
    Overflow,
    Truncate,
}

impl TextVOverflow {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Overflow => "overflow",
            Self::Truncate => "truncate",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "overflow" => Some(Self::Overflow),
            "truncate" => Some(Self::Truncate),
            _ => None,
        }
    }
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
    /// 水平对齐（M80-03）。只有带真实宽度的 `UiLayout` 矩形才有意义。
    pub align: TextAlign,
    /// 垂直对齐（M80-03）。
    pub valign: TextVAlign,
    /// 水平溢出：换行 / 截断 / 省略号 / 溢出。
    pub overflow: TextOverflow,
    /// 垂直溢出：超出行高部分是否丢弃。
    pub voverflow: TextVOverflow,
    /// 富文本标签（`<b>` / `<color=#rrggbb>` / `<br/>`），默认开（与 Unity 一致）。
    pub rich_text: bool,
}

impl Default for UiText {
    /// 新建 `Text` 组件的默认值：空文本 + 基准字号 + 白色 + 项目默认字体 +
    /// 左/上对齐 + 按宽度换行 + 富文本开（Unity 的 Text 默认值）。
    ///
    /// 编辑器「添加组件 ▸ Text」直接用这个类型的编码，编辑器里不再手写这份默认值。
    fn default() -> Self {
        Self {
            text: String::new(),
            font_size: UI_TEXT_BASE_PX,
            color: [1.0, 1.0, 1.0],
            font: String::new(),
            align: TextAlign::Left,
            valign: TextVAlign::Top,
            overflow: TextOverflow::Wrap,
            voverflow: TextVOverflow::Overflow,
            rich_text: true,
        }
    }
}

/// Screen-space textured quad (`Image` component).
#[derive(Clone, Debug, PartialEq)]
pub struct UiImage {
    /// Project-relative texture path.
    pub texture: String,
    /// RGBA tint; default opaque white。第四段是整块四边形的不透明度，作者串里可以省
    /// （`color=r,g,b` ⇒ a=1）。
    ///
    /// 与动态层 `UiPanel.color`（[`UiPanel`]）同形：那边一直是四段，实体层补齐之后
    /// "半透明色块"在两层里是同一个写法，web 宿主读的也是这第四段
    /// （`plugins/webgl/web-runtime.js` 里 `entity.image.color[3]`）。
    pub color: [f32; 4],
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
    /// 整块四边形的不透明度。实体层从 `Image` 的 `color` 第四段带过来（场景 UI 走
    /// `push_image_quad` 与动态层同一份发射代码，所以这个字段挂在共用类型上）；
    /// Lua `ui.image` 这一层暂时恒为 1.0。
    pub alpha: f32,
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
    /// 0 = 矩形, 1 = 圆形（position/size 解释为外接圆外接矩形）。
    pub shape: u8,
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

/// Strict boolean parsing (`true`/`false`/`1`/`0`); typos must not silently
/// become `false`, matching `WorldText.centered`.
fn parse_bool(raw: &str) -> Result<bool, UiComponentError> {
    match raw.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(UiComponentError::InvalidValue(raw.to_owned())),
    }
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

/// Parses an `r,g,b` or `r,g,b,a` color（省略第四段 = 不透明）。
///
/// 允许省第四段是**存量数据的形状**：仓库里 120 行 `Image` 的 `color=` 全是三段（口径：
/// `find examples -name '*.prefab' -o -name '*.scene' | xargs grep -hE '^[a-z]+[[:space:]]+[0-9]+[[:space:]]+Image[[:space:]]'`），
/// 编码侧因此只在 alpha≠1 时才写第四段（见 [`UiImage::to_component_str`]），两边对称才不会被
/// 编辑器一保存就整片重排成四段。schema 侧的同一口径是 [`FieldKind::Color`]
/// （`Vec3/Vec4` 都判"恰好 N 段"，套不上）。
fn parse_rgba(raw: &str) -> Result<[f32; 4], UiComponentError> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 3 && parts.len() != 4 {
        return Err(UiComponentError::InvalidValue(raw.to_owned()));
    }
    let mut color = [
        parse_number(parts[0])?,
        parse_number(parts[1])?,
        parse_number(parts[2])?,
        1.0,
    ];
    if parts.len() == 4 {
        color[3] = parse_number(parts[3])?;
    }
    Ok(color)
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

impl UiText {
    /// Parses `text=<esc>;font_size=..;color=r,g,b[;font=<esc>]` plus the
    /// optional M80-03 layout fields `align`, `valign`, `overflow`, `voverflow`
    /// and `rich_text`.
    pub fn from_component_str(value: &str) -> Result<Self, UiComponentError> {
        let mut text = String::new();
        let mut font_size = None;
        let mut color = [1.0, 1.0, 1.0];
        let mut font = String::new();
        let mut parsed = Self::default();
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
                "align" => {
                    parsed.align = TextAlign::parse(raw.trim())
                        .ok_or_else(|| UiComponentError::InvalidValue(raw.to_owned()))?;
                }
                "valign" => {
                    parsed.valign = TextVAlign::parse(raw.trim())
                        .ok_or_else(|| UiComponentError::InvalidValue(raw.to_owned()))?;
                }
                "overflow" => {
                    parsed.overflow = TextOverflow::parse(raw.trim())
                        .ok_or_else(|| UiComponentError::InvalidValue(raw.to_owned()))?;
                }
                "voverflow" => {
                    parsed.voverflow = TextVOverflow::parse(raw.trim())
                        .ok_or_else(|| UiComponentError::InvalidValue(raw.to_owned()))?;
                }
                "rich_text" => parsed.rich_text = parse_bool(raw)?,
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
            font_size: font_size.unwrap_or(UI_TEXT_BASE_PX),
            color,
            font,
            ..parsed
        })
    }

    /// Encodes the canonical visual-only component string. Optional fields are
    /// only emitted when they differ from the defaults, so existing scenes
    /// (which predate M80-03) round-trip byte-identically.
    #[must_use]
    pub fn to_component_str(&self) -> String {
        let font = if self.font.is_empty() {
            String::new()
        } else {
            format!(";font={}", escape_text(&self.font))
        };
        let align = if self.align == TextAlign::Left {
            String::new()
        } else {
            format!(";align={}", self.align.as_str())
        };
        let valign = if self.valign == TextVAlign::Top {
            String::new()
        } else {
            format!(";valign={}", self.valign.as_str())
        };
        let overflow = if self.overflow == TextOverflow::Wrap {
            String::new()
        } else {
            format!(";overflow={}", self.overflow.as_str())
        };
        let voverflow = if self.voverflow == TextVOverflow::Overflow {
            String::new()
        } else {
            format!(";voverflow={}", self.voverflow.as_str())
        };
        let rich_text = if self.rich_text {
            String::new()
        } else {
            ";rich_text=false".to_owned()
        };
        format!(
            "text={};font_size={:.2};color={:.3},{:.3},{:.3}{align}{valign}{overflow}{voverflow}{rich_text}{font}",
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

    /// 渲染盒尺寸：有真实矩形（宽或高 ≥ 1 px）时文本才按矩形对齐/换行；
    /// 点尺寸实体保持"按 pivot 居中"的旧路径。
    #[must_use]
    pub fn box_size(&self, rect_size: [f32; 2]) -> Option<[f32; 2]> {
        let usable = rect_size
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0)
            && (rect_size[0] >= 1.0 || rect_size[1] >= 1.0);
        usable.then_some(rect_size)
    }

    /// 文本包围框/命中框是否就是布局矩形本身（对齐/截断把字形推离左上角，
    /// 逐字形 bbox 不再覆盖可点区域）。
    #[must_use]
    pub fn fills_layout_box(&self) -> bool {
        self.align != TextAlign::Left
            || self.valign != TextVAlign::Top
            || self.overflow == TextOverflow::Truncate
            || self.overflow == TextOverflow::Ellipsis
            || self.voverflow == TextVOverflow::Truncate
    }
}

impl UiImage {
    /// Parses `texture=..;color=r,g,b[,a];nine_slice=l,r,t,b`（键可省、可乱序；`color`
    /// 省略第四段 = 不透明）。
    pub fn from_component_str(value: &str) -> Result<Self, UiComponentError> {
        let mut texture = String::new();
        let mut color = [1.0, 1.0, 1.0, 1.0];
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
                "color" => color = parse_rgba(raw)?,
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
    ///
    /// alpha=1 时**只写三段**：仓库里 120 行 `Image` 的 `color=` 全是三段（口径见本文件的
    /// `parse_rgba`），写出端跟着省，编辑器一保存才不会把这些行整片重排成四段。这条目前只有
    /// `image_color_omits_alpha_only_when_opaque` 一处守护——三条"看着应该会红"的既有数据门实测
    /// 全绿（账见 examples/game_jhyt/docs/68 §6）。
    #[must_use]
    pub fn to_component_str(&self) -> String {
        let nine_slice = match self.nine_slice {
            Some(s) => format!(
                "nine_slice={:.2},{:.2},{:.2},{:.2};",
                s[0], s[1], s[2], s[3]
            ),
            None => String::new(),
        };
        if self.color[3] == 1.0 {
            return format!(
                "texture={};{nine_slice}color={:.3},{:.3},{:.3}",
                self.texture, self.color[0], self.color[1], self.color[2],
            );
        }
        format!(
            "texture={};{nine_slice}color={:.3},{:.3},{:.3},{:.3}",
            self.texture, self.color[0], self.color[1], self.color[2], self.color[3],
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
            ..UiText::default()
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
            ..UiText::default()
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
            ..UiText::default()
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
    fn text_style_fields_round_trip_and_omit_defaults() {
        let default = UiText {
            text: "Hi".to_owned(),
            font_size: 14.0,
            ..UiText::default()
        };
        let encoded = default.to_component_str();
        for key in ["align=", "valign=", "overflow=", "voverflow=", "rich_text="] {
            assert!(
                !encoded.contains(key),
                "{key} 不应出现在默认编码中：{encoded}"
            );
        }
        let decoded = UiText::from_component_str(&encoded).expect("valid");
        assert_eq!(decoded, default);

        let styled = UiText {
            text: "Hi".to_owned(),
            font_size: 14.0,
            align: TextAlign::Center,
            valign: TextVAlign::Bottom,
            overflow: TextOverflow::Ellipsis,
            voverflow: TextVOverflow::Truncate,
            rich_text: false,
            ..UiText::default()
        };
        let encoded = styled.to_component_str();
        assert!(encoded.contains("align=center"), "{encoded}");
        assert!(encoded.contains("valign=bottom"), "{encoded}");
        assert!(encoded.contains("overflow=ellipsis"), "{encoded}");
        assert!(encoded.contains("voverflow=truncate"), "{encoded}");
        assert!(encoded.contains("rich_text=false"), "{encoded}");
        let decoded = UiText::from_component_str(&encoded).expect("valid");
        assert_eq!(decoded, styled);
    }

    #[test]
    fn text_rejects_invalid_style_enum_values() {
        for field in [
            "align=middle",
            "valign=center",
            "overflow=clip",
            "voverflow=ellipsis",
            "rich_text=maybe",
        ] {
            let spec = format!("text=Hi;font_size=14;{field}");
            assert!(
                UiText::from_component_str(&spec).is_err(),
                "{field} 应被拒绝"
            );
        }
    }

    #[test]
    fn text_box_and_layout_fill_follows_style() {
        let default = UiText {
            text: "Hi".to_owned(),
            font_size: 14.0,
            ..UiText::default()
        };
        assert_eq!(default.box_size([200.0, 60.0]), Some([200.0, 60.0]));
        assert!(!default.fills_layout_box());
        assert_eq!(default.box_size([0.0, 0.0]), None);
        assert_eq!(default.box_size([0.5, 0.0]), None);

        let centered = UiText {
            align: TextAlign::Center,
            ..default.clone()
        };
        assert!(centered.fills_layout_box());
        let ellipsis = UiText {
            overflow: TextOverflow::Ellipsis,
            ..default
        };
        assert!(ellipsis.fills_layout_box());
    }

    #[test]
    fn image_round_trip() {
        let img = UiImage {
            texture: "assets/ui/panel.png".to_owned(),
            color: [0.5, 0.5, 1.0, 1.0],
            nine_slice: None,
        };
        let decoded = UiImage::from_component_str(&img.to_component_str()).expect("valid");
        assert_eq!(decoded, img);
    }

    #[test]
    fn image_nine_slice_round_trip() {
        let img = UiImage {
            texture: "assets/ui/panel.png".to_owned(),
            color: [1.0, 1.0, 1.0, 1.0],
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
            color: [1.0, 1.0, 1.0, 1.0],
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
    fn image_color_accepts_three_or_four_segments() {
        // 三段是**存量形状**（仓库里 120 行 `Image` 的 `color=` 都是三段），省略的第四段读成不透明；
        // 半透明面板才有得写。段数不是随便放宽：2 段与 5 段照样拒。
        assert_eq!(
            UiImage::from_component_str("texture=a;color=0.2,0.3,0.4")
                .expect("三段")
                .color,
            [0.2, 0.3, 0.4, 1.0]
        );
        assert_eq!(
            UiImage::from_component_str("texture=a;color=0.2,0.3,0.4,0.55")
                .expect("四段")
                .color,
            [0.2, 0.3, 0.4, 0.55]
        );
        assert_eq!(
            UiImage::from_component_str("texture=a;color=1,1").unwrap_err(),
            UiComponentError::InvalidValue("1,1".to_owned())
        );
        assert_eq!(
            UiImage::from_component_str("texture=a;color=1,1,1,1,1").unwrap_err(),
            UiComponentError::InvalidValue("1,1,1,1,1".to_owned())
        );
    }

    #[test]
    fn image_color_omits_alpha_only_when_opaque() {
        // 这条钉的是"搬版面不动存量数据"：alpha=1 必须编回**三段**，否则每一行既有
        // `color=` 都会在迁移/保存时被重排成四段（`.scene`/`.prefab` 逐字节幂等判定当场红，
        // m63 的门面黄金表也整片漂）。真半透明才出现第四段。
        let opaque = UiImage {
            texture: "assets/ui/panel.png".to_owned(),
            color: [0.949, 0.918, 0.847, 1.0],
            nine_slice: None,
        };
        assert_eq!(
            opaque.to_component_str(),
            "texture=assets/ui/panel.png;color=0.949,0.918,0.847"
        );
        let dimmed = UiImage {
            texture: "assets/ui/panel.png".to_owned(),
            color: [0.0, 0.0, 0.0, 0.55],
            nine_slice: None,
        };
        assert_eq!(
            dimmed.to_component_str(),
            "texture=assets/ui/panel.png;color=0.000,0.000,0.000,0.550"
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
            font_size: 14.0,
            color: [1.0, 1.0, 1.0],
            font: String::new(),
            ..UiText::default()
        };
        let mut scene = rustlua_scene::scene::Scene::new("ui");
        scene
            .add_entity(rustlua_scene::scene::SceneEntityId(1), "Label", None)
            .unwrap();
        scene
            .entities
            .get_mut(&rustlua_scene::scene::SceneEntityId(1))
            .unwrap()
            .components
            .insert("Text".to_owned(), ui.to_component_str());
        let reloaded = rustlua_scene::scene::Scene::from_text(&scene.to_text()).expect("reload");
        let value = reloaded.entities[&rustlua_scene::scene::SceneEntityId(1)]
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
