//! E15/E16/E17 engine-core layout layer: four-edge Anchor + Offset
//! ([`crate::ui_layout::UiRectLayout`]), named anchor presets ([`crate::ui_layout::UiLayoutPreset`]), basic
//! Container auto-layout ([`crate::ui_layout::layout_container`]), and Canvas configuration
//! ([`crate::ui_layout::CanvasConfig`]).
//!
//! All computation here is pure logic with no GPU/window dependency, so the
//! editor preview and the runtime share the same resolve paths.

/// A pixel-space rectangle: top-left origin (x, y) plus size (w, h).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UiRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl UiRect {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    #[must_use]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge (`x + w`).
    #[must_use]
    pub const fn right(&self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge (`y + h`).
    #[must_use]
    pub const fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Point containment test.
    #[must_use]
    pub const fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
}

/// Four-edge anchors in normalized parent coordinates (each `0.0..=1.0`):
/// `[left, top, right, bottom]`. Point anchors have `left == right` and
/// `top == bottom`; differing edges stretch the element with its parent.
pub type UiAnchors = [f32; 4];

/// Four-edge offsets in pixels relative to the corresponding anchor edge:
/// `[left, top, right, bottom]`. For point anchors this is
/// `[x, y, x + w, y + h]`.
pub type UiOffsets = [f32; 4];

/// Authored layout for a UI element using the four-edge anchor + offset model
/// (E16). Resolution is against the parent (viewport or canvas) size.
#[derive(Clone, Debug, PartialEq)]
pub struct UiRectLayout {
    pub anchors: UiAnchors,
    pub offsets: UiOffsets,
    /// Per-axis minimum size in pixels; `0.0` means unconstrained.
    pub min_size: [f32; 2],
    /// Per-axis maximum size in pixels; `0.0` means unconstrained.
    pub max_size: [f32; 2],
    /// Pivot in normalized element coordinates (`0.0..=1.0`, `[0.5, 0.5]` =
    /// center). The resolved rect grows/shrinks around the pivot.
    pub pivot: [f32; 2],
}

impl Default for UiRectLayout {
    fn default() -> Self {
        Self {
            anchors: [0.0, 0.0, 0.0, 0.0],
            offsets: [0.0, 0.0, 0.0, 0.0],
            min_size: [0.0, 0.0],
            max_size: [0.0, 0.0],
            pivot: [0.5, 0.5],
        }
    }
}

impl UiRectLayout {
    /// Builds a layout from a named preset with explicit offsets.
    #[must_use]
    pub fn from_preset(preset: UiLayoutPreset, offsets: UiOffsets) -> Self {
        let mut layout = Self::default();
        layout.apply_preset(preset);
        layout.offsets = offsets;
        layout
    }

    /// 新建 `UiLayout` 组件的默认值：中心锚点、200×100 的元素矩形。
    ///
    /// 编辑器「添加组件 ▸ UiLayout」直接用它的编码——这份产品默认值只在这里定义一处，
    /// 编辑器不再自己拼 `from_preset` + 手写 offsets（docs/55 阶段 2 的"默认值只有引擎一处"）。
    #[must_use]
    pub fn default_element() -> Self {
        Self::from_preset(UiLayoutPreset::Center, [-100.0, -50.0, 100.0, 50.0])
    }

    /// Overwrites the anchors (and resets the pivot) for a named preset.
    pub fn apply_preset(&mut self, preset: UiLayoutPreset) {
        self.anchors = preset.anchors();
        self.pivot = match preset {
            UiLayoutPreset::FullRect
            | UiLayoutPreset::StretchHorizontal
            | UiLayoutPreset::StretchVertical => [0.5, 0.5],
            other => [other.anchors()[0], other.anchors()[1]],
        };
    }

    /// Resolves the final pixel rect against `parent_size`.
    ///
    /// Edge positions are `parent * anchor + offset`: the edges directly
    /// define the rect, so an unclamped stretch rect sits exactly at its
    /// authored edges. When `min_size` / `max_size` (zero = unconstrained)
    /// changes a dimension, the pivot point stays stationary while the rect
    /// grows or shrinks.
    #[must_use]
    pub fn resolve(&self, parent_size: [f32; 2]) -> UiRect {
        let left = parent_size[0] * self.anchors[0] + self.offsets[0];
        let top = parent_size[1] * self.anchors[1] + self.offsets[1];
        let right = parent_size[0] * self.anchors[2] + self.offsets[2];
        let bottom = parent_size[1] * self.anchors[3] + self.offsets[3];
        let width = (right - left).max(0.0);
        let height = (bottom - top).max(0.0);

        let clamped_w = clamp_axis(width, self.min_size[0], self.max_size[0]);
        let clamped_h = clamp_axis(height, self.min_size[1], self.max_size[1]);

        // Keep the pivot point stationary when a dimension is clamped.
        let x = if clamped_w == width {
            left
        } else {
            left + self.pivot[0] * (width - clamped_w)
        };
        let y = if clamped_h == height {
            top
        } else {
            top + self.pivot[1] * (height - clamped_h)
        };
        UiRect::new(x, y, clamped_w, clamped_h)
    }

    /// Parses the `UiLayout` component string:
    /// `anchors=l,t,r,b;offsets=l,t,r,b;min_size=w,h;max_size=w,h;pivot=x,y`
    /// (all fields optional; missing fields fall back to defaults).
    pub fn from_component_str(value: &str) -> Result<Self, String> {
        let mut layout = Self::default();
        for field in value.split(';') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, raw)) = field.split_once('=') else {
                return Err(format!("malformed UiLayout field: {field}"));
            };
            match key {
                "anchors" => layout.anchors = parse_f32x4(raw)?,
                "offsets" => layout.offsets = parse_f32x4(raw)?,
                "min_size" => layout.min_size = parse_f32x2(raw)?,
                "max_size" => layout.max_size = parse_f32x2(raw)?,
                "pivot" => layout.pivot = parse_f32x2(raw)?,
                _ => return Err(format!("unknown UiLayout field: {key}")),
            }
        }
        validate_anchors(&layout.anchors)?;
        validate_unit(&layout.pivot, "pivot")?;
        Ok(layout)
    }

    /// Serializes to the canonical component string (fields in fixed order).
    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "anchors={:.4},{:.4},{:.4},{:.4};offsets={:.2},{:.2},{:.2},{:.2};min_size={:.2},{:.2};max_size={:.2},{:.2};pivot={:.4},{:.4}",
            self.anchors[0],
            self.anchors[1],
            self.anchors[2],
            self.anchors[3],
            self.offsets[0],
            self.offsets[1],
            self.offsets[2],
            self.offsets[3],
            self.min_size[0],
            self.min_size[1],
            self.max_size[0],
            self.max_size[1],
            self.pivot[0],
            self.pivot[1],
        )
    }
}

/// Named anchor presets (E16): the ten required layouts plus the remaining
/// nine-slice points so the palette can cover every corner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiLayoutPreset {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    StretchHorizontal,
    StretchVertical,
    FullRect,
}

impl UiLayoutPreset {
    pub const ALL: [Self; 12] = [
        Self::TopLeft,
        Self::TopCenter,
        Self::TopRight,
        Self::CenterLeft,
        Self::Center,
        Self::CenterRight,
        Self::BottomLeft,
        Self::BottomCenter,
        Self::BottomRight,
        Self::StretchHorizontal,
        Self::StretchVertical,
        Self::FullRect,
    ];

    /// Editor-facing display name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "Top Left",
            Self::TopCenter => "Top Center",
            Self::TopRight => "Top Right",
            Self::CenterLeft => "Center Left",
            Self::Center => "Center",
            Self::CenterRight => "Center Right",
            Self::BottomLeft => "Bottom Left",
            Self::BottomCenter => "Bottom Center",
            Self::BottomRight => "Bottom Right",
            Self::StretchHorizontal => "Stretch Horizontal",
            Self::StretchVertical => "Stretch Vertical",
            Self::FullRect => "Full Rect",
        }
    }

    /// The four-edge anchors this preset produces.
    #[must_use]
    pub const fn anchors(self) -> UiAnchors {
        match self {
            Self::TopLeft => [0.0, 0.0, 0.0, 0.0],
            Self::TopCenter => [0.5, 0.0, 0.5, 0.0],
            Self::TopRight => [1.0, 0.0, 1.0, 0.0],
            Self::CenterLeft => [0.0, 0.5, 0.0, 0.5],
            Self::Center => [0.5, 0.5, 0.5, 0.5],
            Self::CenterRight => [1.0, 0.5, 1.0, 0.5],
            Self::BottomLeft => [0.0, 1.0, 0.0, 1.0],
            Self::BottomCenter => [0.5, 1.0, 0.5, 1.0],
            Self::BottomRight => [1.0, 1.0, 1.0, 1.0],
            Self::StretchHorizontal => [0.0, 0.0, 1.0, 0.0],
            Self::StretchVertical => [0.0, 0.0, 0.0, 1.0],
            Self::FullRect => [0.0, 0.0, 1.0, 1.0],
        }
    }
}

/// Container kind for basic auto-layout (E17).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContainerKind {
    /// Children stacked vertically.
    #[default]
    VBox,
    /// Children laid out horizontally.
    HBox,
    /// Children in a fixed column count grid.
    Grid,
    /// All children inset by padding (overlapping).
    Margin,
    /// All children centered, keeping measured sizes.
    Center,
}

impl ContainerKind {
    /// Parses the `Container` component `kind` field.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "vbox" => Some(Self::VBox),
            "hbox" => Some(Self::HBox),
            "grid" => Some(Self::Grid),
            "margin" => Some(Self::Margin),
            "center" => Some(Self::Center),
            _ => None,
        }
    }

    /// Canonical component-string token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VBox => "vbox",
            Self::HBox => "hbox",
            Self::Grid => "grid",
            Self::Margin => "margin",
            Self::Center => "center",
        }
    }
}

/// Cross-axis alignment for non-stretched children.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ChildAlign {
    /// Align to the cross-axis start.
    #[default]
    Start,
    /// Center on the cross axis.
    Center,
    /// Align to the cross-axis end.
    End,
}

impl ChildAlign {
    /// Parses `start|center|end`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "start" => Some(Self::Start),
            "center" => Some(Self::Center),
            "end" => Some(Self::End),
            _ => None,
        }
    }

    /// Canonical token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Center => "center",
            Self::End => "end",
        }
    }
}

/// Authored style for a `Container` component (E17).
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerStyle {
    pub kind: ContainerKind,
    /// Inner padding `[left, top, right, bottom]` in pixels.
    pub padding: [f32; 4],
    /// Gap between children along the main axis (and between grid cells).
    pub spacing: f32,
    /// Grid column count (`0` = infer a square-ish layout).
    pub columns: usize,
    /// Stretch children along the main axis to consume leftover space,
    /// proportionally to each child's expand weight.
    pub stretch: bool,
    /// Fill the cross axis when the child allows it.
    pub fill: bool,
    /// Cross-axis alignment for non-filled children.
    pub align: ChildAlign,
}

impl Default for ContainerStyle {
    fn default() -> Self {
        Self {
            kind: ContainerKind::VBox,
            padding: [0.0; 4],
            spacing: 0.0,
            columns: 0,
            stretch: false,
            fill: false,
            align: ChildAlign::Start,
        }
    }
}

impl ContainerStyle {
    /// Inner content size after padding.
    #[must_use]
    fn inner_size(&self, container: UiRect) -> [f32; 2] {
        [
            (container.w - self.padding[0] - self.padding[2]).max(0.0),
            (container.h - self.padding[1] - self.padding[3]).max(0.0),
        ]
    }

    /// Parses the `Container` component string:
    /// `kind=vbox;padding=l,t,r,b;spacing=g;columns=n;stretch=true;fill=false;align=center`
    /// (all fields optional).
    pub fn from_component_str(value: &str) -> Result<Self, String> {
        let mut style = Self::default();
        for field in value.split(';') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, raw)) = field.split_once('=') else {
                return Err(format!("malformed Container field: {field}"));
            };
            match key {
                "kind" => {
                    style.kind = ContainerKind::parse(raw)
                        .ok_or_else(|| format!("unknown Container kind: {raw}"))?;
                }
                "padding" => style.padding = parse_f32x4(raw)?,
                "spacing" => style.spacing = parse_number(raw)?,
                "columns" => {
                    style.columns = raw
                        .parse::<usize>()
                        .map_err(|_| format!("invalid columns: {raw}"))?;
                }
                "stretch" => style.stretch = parse_bool(raw)?,
                "fill" => style.fill = parse_bool(raw)?,
                "align" => {
                    style.align =
                        ChildAlign::parse(raw).ok_or_else(|| format!("unknown align: {raw}"))?;
                }
                _ => return Err(format!("unknown Container field: {key}")),
            }
        }
        Ok(style)
    }

    /// Serializes to the canonical component string.
    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "kind={};padding={:.2},{:.2},{:.2},{:.2};spacing={:.2};columns={};stretch={};fill={};align={}",
            self.kind.as_str(),
            self.padding[0],
            self.padding[1],
            self.padding[2],
            self.padding[3],
            self.spacing,
            self.columns,
            self.stretch,
            self.fill,
            self.align.as_str(),
        )
    }
}

/// Per-child layout options (E17: Expand / Fill / Alignment).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChildLayout {
    /// Measured content size (pixels). `0.0` axes collapse when not expanded.
    pub size: [f32; 2],
    /// Main-axis expand weight (`0.0` = keep measured size).
    pub expand: f32,
    /// Whether this child fills the cross axis when the container allows it.
    pub fill: bool,
}

impl ChildLayout {
    /// A fixed-size child with default options.
    #[must_use]
    pub const fn sized(size: [f32; 2]) -> Self {
        Self {
            size,
            expand: 0.0,
            fill: false,
        }
    }
}

/// Computes child rects inside `container` (E17). Returned rects are relative
/// to the container's top-left corner. This is a pure function: the caller
/// decides when the affected subtree is recomputed.
#[must_use]
pub fn layout_container(
    container: UiRect,
    children: &[ChildLayout],
    style: &ContainerStyle,
) -> Vec<UiRect> {
    if children.is_empty() {
        return Vec::new();
    }
    let [inner_w, inner_h] = style.inner_size(container);
    let origin_x = style.padding[0];
    let origin_y = style.padding[1];
    let cross_align_offset = |child_size: f32, inner: f32| -> f32 {
        match style.align {
            ChildAlign::Start => 0.0,
            ChildAlign::Center => ((inner - child_size) * 0.5).max(0.0),
            ChildAlign::End => (inner - child_size).max(0.0),
        }
    };

    match style.kind {
        ContainerKind::Margin => children
            .iter()
            .map(|_| UiRect::new(origin_x, origin_y, inner_w, inner_h))
            .collect(),
        ContainerKind::Center => children
            .iter()
            .map(|child| {
                UiRect::new(
                    origin_x + ((inner_w - child.size[0]) * 0.5).max(0.0),
                    origin_y + ((inner_h - child.size[1]) * 0.5).max(0.0),
                    child.size[0],
                    child.size[1],
                )
            })
            .collect(),
        ContainerKind::VBox | ContainerKind::HBox => {
            let (inner_main, inner_cross, spacing) = if style.kind == ContainerKind::VBox {
                (inner_h, inner_w, style.spacing)
            } else {
                (inner_w, inner_h, style.spacing)
            };
            let child_main = |child: &ChildLayout| {
                if style.kind == ContainerKind::VBox {
                    child.size[1]
                } else {
                    child.size[0]
                }
            };
            let child_cross = |child: &ChildLayout| {
                if style.kind == ContainerKind::VBox {
                    child.size[0]
                } else {
                    child.size[1]
                }
            };

            let total_expand: f32 = if style.stretch {
                children.iter().map(|child| child.expand.max(0.0)).sum()
            } else {
                0.0
            };
            let used_main: f32 = children.iter().map(child_main).sum();
            let gaps = spacing * (children.len().saturating_sub(1)) as f32;
            let free_main = (inner_main - used_main - gaps).max(0.0);

            let mut cursor = 0.0;
            children
                .iter()
                .map(|child| {
                    let mut main = child_main(child);
                    if style.stretch && total_expand > 0.0 && child.expand > 0.0 {
                        main += free_main * (child.expand / total_expand);
                    }
                    let cross = if style.fill && child.fill {
                        inner_cross
                    } else {
                        child_cross(child)
                    };
                    let cross_offset = if style.fill && child.fill {
                        0.0
                    } else {
                        cross_align_offset(cross, inner_cross)
                    };
                    let rect = if style.kind == ContainerKind::VBox {
                        UiRect::new(origin_x + cross_offset, origin_y + cursor, cross, main)
                    } else {
                        UiRect::new(origin_x + cursor, origin_y + cross_offset, main, cross)
                    };
                    cursor += main + spacing;
                    rect
                })
                .collect()
        }
        ContainerKind::Grid => {
            let columns = if style.columns > 0 {
                style.columns
            } else {
                (children.len() as f32).sqrt().ceil().max(1.0) as usize
            };
            let spacing = style.spacing;
            // Measure column/row sizes from children (grid keeps child sizes;
            // stretch distributes leftover space across columns/rows).
            let rows = children.len().div_ceil(columns);
            let mut column_widths = vec![0.0_f32; columns];
            let mut row_heights = vec![0.0_f32; rows];
            for (index, child) in children.iter().enumerate() {
                let col = index % columns;
                let row = index / columns;
                column_widths[col] = column_widths[col].max(child.size[0]);
                row_heights[row] = row_heights[row].max(child.size[1]);
            }
            if style.stretch {
                let used_w: f32 = column_widths.iter().sum::<f32>()
                    + spacing * (columns.saturating_sub(1)) as f32;
                let free_w = (inner_w - used_w).max(0.0);
                if columns > 0 {
                    let extra = free_w / columns as f32;
                    for width in &mut column_widths {
                        *width += extra;
                    }
                }
                let used_h: f32 =
                    row_heights.iter().sum::<f32>() + spacing * (rows.saturating_sub(1)) as f32;
                let free_h = (inner_h - used_h).max(0.0);
                if rows > 0 {
                    let extra = free_h / rows as f32;
                    for height in &mut row_heights {
                        *height += extra;
                    }
                }
            }
            let mut column_x = vec![0.0_f32; columns];
            let mut acc = origin_x;
            for (index, width) in column_widths.iter().enumerate() {
                column_x[index] = acc;
                acc += width + spacing;
            }
            let mut row_y = vec![0.0_f32; rows];
            acc = origin_y;
            for (index, height) in row_heights.iter().enumerate() {
                row_y[index] = acc;
                acc += height + spacing;
            }
            children
                .iter()
                .enumerate()
                .map(|(index, child)| {
                    let col = index % columns;
                    let row = index / columns;
                    let cell_w = column_widths[col];
                    let cell_h = row_heights[row];
                    let (w, h, dx, dy) = if style.fill && child.fill {
                        (cell_w, cell_h, 0.0, 0.0)
                    } else {
                        (
                            child.size[0],
                            child.size[1],
                            cross_align_offset(child.size[0], cell_w),
                            cross_align_offset(child.size[1], cell_h),
                        )
                    };
                    UiRect::new(column_x[col] + dx, row_y[row] + dy, w, h)
                })
                .collect()
        }
    }
}

/// Canvas configuration (E15): reference resolution, scale mode, and safe
/// area insets. The editor preview and the runtime share this configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasConfig {
    /// Design (reference) resolution in pixels.
    pub reference: [f32; 2],
    pub scale_mode: CanvasScaleMode,
    /// Safe-area insets in pixels `[left, top, right, bottom]`.
    pub safe_area: [f32; 4],
}

impl Default for CanvasConfig {
    fn default() -> Self {
        Self {
            reference: [1280.0, 720.0],
            scale_mode: CanvasScaleMode::FitBoth,
            safe_area: [0.0; 4],
        }
    }
}

/// How authored canvas space maps onto the live viewport.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CanvasScaleMode {
    /// No scaling: canvas pixels map 1:1 onto viewport pixels.
    None,
    /// Uniform scale so the reference width matches the viewport width
    /// (height may overflow or letterbox).
    FitWidth,
    /// Uniform scale so the reference height matches the viewport height.
    FitHeight,
    /// Non-uniform per-axis scale (current behavior: `viewport / reference`).
    #[default]
    FitBoth,
}

impl CanvasScaleMode {
    /// Parses `none|fit_width|fit_height|fit_both`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "none" => Some(Self::None),
            "fit_width" => Some(Self::FitWidth),
            "fit_height" => Some(Self::FitHeight),
            "fit_both" => Some(Self::FitBoth),
            _ => None,
        }
    }

    /// Canonical token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::FitWidth => "fit_width",
            Self::FitHeight => "fit_height",
            Self::FitBoth => "fit_both",
        }
    }
}

impl CanvasConfig {
    /// Per-axis authored → viewport scale for the given live viewport.
    #[must_use]
    pub fn scale(&self, viewport: [f32; 2]) -> [f32; 2] {
        let width_scale = viewport[0] / self.reference[0].max(f32::EPSILON);
        let height_scale = viewport[1] / self.reference[1].max(f32::EPSILON);
        match self.scale_mode {
            CanvasScaleMode::None => [1.0, 1.0],
            CanvasScaleMode::FitWidth => [width_scale, width_scale],
            CanvasScaleMode::FitHeight => [height_scale, height_scale],
            CanvasScaleMode::FitBoth => [width_scale, height_scale],
        }
    }

    /// The safe-area rectangle in viewport pixels (content should stay inside).
    #[must_use]
    pub fn safe_rect(&self, viewport: [f32; 2]) -> UiRect {
        let [left, top, right, bottom] = self.safe_area;
        UiRect::new(
            left,
            top,
            (viewport[0] - left - right).max(0.0),
            (viewport[1] - top - bottom).max(0.0),
        )
    }

    /// Parses scene metadata values into a `CanvasConfig`.
    /// `reference` = `"w,h"`; `scale_mode` = token; `safe_area` = `"l,t,r,b"`.
    pub fn from_metadata(
        reference: Option<&str>,
        scale_mode: Option<&str>,
        safe_area: Option<&str>,
    ) -> Result<Self, String> {
        let mut config = Self::default();
        if let Some(raw) = reference {
            let parsed = parse_f32x2(raw)?;
            if parsed[0] <= 0.0 || parsed[1] <= 0.0 {
                return Err(format!("Canvas reference must be positive: {raw}"));
            }
            config.reference = parsed;
        }
        if let Some(raw) = scale_mode {
            config.scale_mode = CanvasScaleMode::parse(raw)
                .ok_or_else(|| format!("unknown Canvas scale_mode: {raw}"))?;
        }
        if let Some(raw) = safe_area {
            config.safe_area = parse_f32x4(raw)?;
        }
        Ok(config)
    }
}

fn parse_number(raw: &str) -> Result<f32, String> {
    raw.trim()
        .parse::<f32>()
        .map_err(|_| format!("invalid number: {raw}"))
}

fn parse_bool(raw: &str) -> Result<bool, String> {
    match raw.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(format!("invalid bool: {raw}")),
    }
}

fn parse_f32x2(raw: &str) -> Result<[f32; 2], String> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 2 {
        return Err(format!("expected x,y pair: {raw}"));
    }
    Ok([parse_number(parts[0])?, parse_number(parts[1])?])
}

fn parse_f32x4(raw: &str) -> Result<[f32; 4], String> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 4 {
        return Err(format!("expected 4-value tuple: {raw}"));
    }
    Ok([
        parse_number(parts[0])?,
        parse_number(parts[1])?,
        parse_number(parts[2])?,
        parse_number(parts[3])?,
    ])
}

fn validate_anchors(anchors: &UiAnchors) -> Result<(), String> {
    for (index, value) in anchors.iter().enumerate() {
        if !value.is_finite() || !(0.0..=1.0).contains(value) {
            return Err(format!("anchor[{index}] must be in 0..=1: {value}"));
        }
    }
    Ok(())
}

fn validate_unit(values: &[f32; 2], name: &str) -> Result<(), String> {
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() || !(0.0..=1.0).contains(value) {
            return Err(format!("{name}[{index}] must be in 0..=1: {value}"));
        }
    }
    Ok(())
}

fn clamp_axis(value: f32, min: f32, max: f32) -> f32 {
    let clamped = value.max(min.max(0.0));
    if max > 0.0 { clamped.min(max) } else { clamped }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-4,
            "expected {expected}, got {actual}"
        );
    }

    fn assert_rect(actual: UiRect, expected: [f32; 4]) {
        assert_close(actual.x, expected[0]);
        assert_close(actual.y, expected[1]);
        assert_close(actual.w, expected[2]);
        assert_close(actual.h, expected[3]);
    }

    // ---- E16: UiRectLayout resolve ---------------------------------------

    #[test]
    fn stretch_horizontal_tracks_parent_width() {
        let mut layout = UiRectLayout::default();
        layout.apply_preset(UiLayoutPreset::StretchHorizontal);
        layout.offsets = [0.0, 600.0, 0.0, 660.0];
        let narrow = layout.resolve([1280.0, 720.0]);
        assert_rect(narrow, [0.0, 600.0, 1280.0, 60.0]);
        let wide = layout.resolve([1920.0, 1080.0]);
        assert_rect(wide, [0.0, 600.0, 1920.0, 60.0]);
    }

    #[test]
    fn full_rect_with_padding_resolves_content_area() {
        let layout = UiRectLayout {
            anchors: [0.0, 0.0, 1.0, 1.0],
            offsets: [16.0, 16.0, -16.0, -16.0],
            pivot: [0.0, 0.0],
            ..UiRectLayout::default()
        };
        let rect = layout.resolve([1280.0, 720.0]);
        assert_rect(rect, [16.0, 16.0, 1248.0, 688.0]);
    }

    #[test]
    fn min_and_max_size_clamp_resolved_dimensions() {
        let layout = UiRectLayout {
            anchors: [0.0, 0.0, 1.0, 0.0],
            offsets: [0.0, 0.0, 0.0, 80.0],
            min_size: [200.0, 0.0],
            max_size: [400.0, 0.0],
            pivot: [0.0, 0.0],
        };
        // Parent width 100 → clamped up to min 200.
        assert_rect(layout.resolve([100.0, 600.0]), [0.0, 0.0, 200.0, 80.0]);
        // Parent width 1000 → clamped down to max 400.
        assert_rect(layout.resolve([1000.0, 600.0]), [0.0, 0.0, 400.0, 80.0]);
    }

    #[test]
    fn pivot_keeps_pivot_point_stationary() {
        // Point anchor at center with pivot center: clamping shrinks the
        // rect around the pivot point, which stays at (740, 410).
        let layout = UiRectLayout {
            anchors: [0.5, 0.5, 0.5, 0.5],
            offsets: [0.0, 0.0, 200.0, 100.0],
            max_size: [100.0, 100.0],
            pivot: [0.5, 0.5],
            ..UiRectLayout::default()
        };
        let rect = layout.resolve([1280.0, 720.0]);
        assert_rect(rect, [690.0, 360.0, 100.0, 100.0]);
    }

    #[test]
    fn component_string_round_trip() {
        let layout = UiRectLayout {
            anchors: [0.5, 0.0, 0.5, 0.0],
            offsets: [-160.0, 20.0, 160.0, 80.0],
            min_size: [100.0, 0.0],
            max_size: [0.0, 400.0],
            pivot: [0.5, 0.0],
        };
        let parsed = UiRectLayout::from_component_str(&layout.to_component_str()).expect("valid");
        assert_eq!(parsed, layout);
    }

    #[test]
    fn component_string_defaults_and_validation() {
        let layout = UiRectLayout::from_component_str("").expect("empty uses defaults");
        assert_eq!(layout, UiRectLayout::default());
        assert!(UiRectLayout::from_component_str("anchors=2,0,0,0").is_err());
        assert!(UiRectLayout::from_component_str("pivot=0.5,5").is_err());
        assert!(UiRectLayout::from_component_str("offsets=1,2,3").is_err());
        assert!(UiRectLayout::from_component_str("bogus=1").is_err());
    }

    #[test]
    fn preset_anchors_cover_required_layouts() {
        assert_eq!(UiLayoutPreset::TopCenter.anchors(), [0.5, 0.0, 0.5, 0.0]);
        assert_eq!(UiLayoutPreset::BottomRight.anchors(), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(UiLayoutPreset::Center.anchors(), [0.5, 0.5, 0.5, 0.5]);
        assert_eq!(UiLayoutPreset::FullRect.anchors(), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(
            UiLayoutPreset::StretchHorizontal.anchors(),
            [0.0, 0.0, 1.0, 0.0]
        );
        assert_eq!(
            UiLayoutPreset::StretchVertical.anchors(),
            [0.0, 0.0, 0.0, 1.0]
        );
        // Center preset resolves a 200×100 child dead-center.
        let layout =
            UiRectLayout::from_preset(UiLayoutPreset::Center, [-100.0, -50.0, 100.0, 50.0]);
        let rect = layout.resolve([1280.0, 720.0]);
        assert_rect(rect, [540.0, 310.0, 200.0, 100.0]);
    }

    // ---- E17: Container layout -------------------------------------------

    fn container_rect() -> UiRect {
        UiRect::new(0.0, 0.0, 400.0, 300.0)
    }

    #[test]
    fn vbox_stacks_children_with_spacing() {
        let style = ContainerStyle {
            kind: ContainerKind::VBox,
            spacing: 10.0,
            ..Default::default()
        };
        let children = [
            ChildLayout::sized([100.0, 40.0]),
            ChildLayout::sized([80.0, 60.0]),
        ];
        let rects = layout_container(container_rect(), &children, &style);
        assert_eq!(rects.len(), 2);
        assert_rect(rects[0], [0.0, 0.0, 100.0, 40.0]);
        assert_rect(rects[1], [0.0, 50.0, 80.0, 60.0]);
    }

    #[test]
    fn vbox_padding_and_center_alignment() {
        let style = ContainerStyle {
            kind: ContainerKind::VBox,
            padding: [20.0, 10.0, 20.0, 10.0],
            align: ChildAlign::Center,
            ..Default::default()
        };
        let children = [ChildLayout::sized([100.0, 40.0])];
        let rects = layout_container(container_rect(), &children, &style);
        // Inner width = 400 - 40 = 360; offset = (360-100)/2 = 130; x = 20+130.
        assert_rect(rects[0], [150.0, 10.0, 100.0, 40.0]);
    }

    #[test]
    fn hbox_stretches_expanding_child() {
        let style = ContainerStyle {
            kind: ContainerKind::HBox,
            spacing: 8.0,
            stretch: true,
            ..Default::default()
        };
        let children = [
            ChildLayout {
                size: [60.0, 30.0],
                expand: 1.0,
                fill: false,
            },
            ChildLayout::sized([40.0, 30.0]),
        ];
        let rects = layout_container(container_rect(), &children, &style);
        // Free = 400 - 100 - 8 = 292 → expanding child gains it all.
        assert_rect(rects[0], [0.0, 0.0, 352.0, 30.0]);
        assert_rect(rects[1], [360.0, 0.0, 40.0, 30.0]);
    }

    #[test]
    fn hbox_fill_fills_cross_axis() {
        let style = ContainerStyle {
            kind: ContainerKind::HBox,
            fill: true,
            ..Default::default()
        };
        let children = [
            ChildLayout {
                size: [100.0, 30.0],
                expand: 0.0,
                fill: true,
            },
            ChildLayout::sized([50.0, 30.0]),
        ];
        let rects = layout_container(container_rect(), &children, &style);
        assert_rect(rects[0], [0.0, 0.0, 100.0, 300.0]);
        assert_rect(rects[1], [100.0, 0.0, 50.0, 30.0]);
    }

    #[test]
    fn grid_places_children_in_cells() {
        let style = ContainerStyle {
            kind: ContainerKind::Grid,
            columns: 2,
            spacing: 6.0,
            ..Default::default()
        };
        let children = [
            ChildLayout::sized([100.0, 40.0]),
            ChildLayout::sized([60.0, 40.0]),
            ChildLayout::sized([80.0, 50.0]),
        ];
        let rects = layout_container(container_rect(), &children, &style);
        // Column widths: [100, 60]; row heights: [40, 50].
        assert_rect(rects[0], [0.0, 0.0, 100.0, 40.0]);
        assert_rect(rects[1], [106.0, 0.0, 60.0, 40.0]);
        assert_rect(rects[2], [0.0, 46.0, 80.0, 50.0]);
    }

    #[test]
    fn grid_stretch_expands_cells_to_fill() {
        let style = ContainerStyle {
            kind: ContainerKind::Grid,
            columns: 2,
            spacing: 0.0,
            stretch: true,
            fill: true,
            ..Default::default()
        };
        let children = [
            ChildLayout {
                size: [100.0, 40.0],
                expand: 0.0,
                fill: true,
            },
            ChildLayout {
                size: [100.0, 40.0],
                expand: 0.0,
                fill: true,
            },
        ];
        let rects = layout_container(container_rect(), &children, &style);
        // Columns split 400/2 = 200 each; rows split 300 (one row → 300).
        assert_rect(rects[0], [0.0, 0.0, 200.0, 300.0]);
        assert_rect(rects[1], [200.0, 0.0, 200.0, 300.0]);
    }

    #[test]
    fn margin_insets_single_child() {
        let style = ContainerStyle {
            kind: ContainerKind::Margin,
            padding: [12.0, 8.0, 16.0, 4.0],
            ..Default::default()
        };
        let children = [ChildLayout::sized([0.0, 0.0])];
        let rects = layout_container(container_rect(), &children, &style);
        assert_rect(rects[0], [12.0, 8.0, 372.0, 288.0]);
    }

    #[test]
    fn center_keeps_measured_sizes() {
        let style = ContainerStyle {
            kind: ContainerKind::Center,
            ..Default::default()
        };
        let children = [
            ChildLayout::sized([120.0, 50.0]),
            ChildLayout::sized([40.0, 20.0]),
        ];
        let rects = layout_container(container_rect(), &children, &style);
        assert_rect(rects[0], [140.0, 125.0, 120.0, 50.0]);
        assert_rect(rects[1], [180.0, 140.0, 40.0, 20.0]);
    }

    #[test]
    fn empty_and_degenerate_containers_are_safe() {
        let style = ContainerStyle::default();
        assert!(layout_container(container_rect(), &[], &style).is_empty());
        // Zero-size container: everything collapses to the origin.
        let children = [ChildLayout::sized([10.0, 10.0])];
        let rects = layout_container(UiRect::ZERO, &children, &style);
        assert_rect(rects[0], [0.0, 0.0, 10.0, 10.0]);
    }

    #[test]
    fn container_style_component_string_round_trip() {
        let style = ContainerStyle {
            kind: ContainerKind::Grid,
            padding: [8.0, 4.0, 8.0, 4.0],
            spacing: 6.0,
            columns: 3,
            stretch: true,
            fill: true,
            align: ChildAlign::Center,
        };
        let parsed = ContainerStyle::from_component_str(&style.to_component_str()).expect("valid");
        assert_eq!(parsed, style);
    }

    #[test]
    fn container_style_parses_defaults_and_rejects_bad_input() {
        let style = ContainerStyle::from_component_str("").expect("defaults");
        assert_eq!(style.kind, ContainerKind::VBox);
        assert!(!style.stretch);
        assert!(ContainerStyle::from_component_str("kind=unknown").is_err());
        assert!(ContainerStyle::from_component_str("columns=abc").is_err());
        assert!(ContainerStyle::from_component_str("align=middle").is_err());
    }

    // ---- E15: CanvasConfig -------------------------------------------------

    #[test]
    fn canvas_scale_modes() {
        let config = CanvasConfig {
            reference: [1280.0, 720.0],
            scale_mode: CanvasScaleMode::FitBoth,
            safe_area: [0.0; 4],
        };
        assert_eq!(config.scale([1920.0, 1080.0]), [1.5, 1.5]);
        let fit_width = CanvasConfig {
            scale_mode: CanvasScaleMode::FitWidth,
            ..config
        };
        assert_eq!(fit_width.scale([1920.0, 1080.0]), [1.5, 1.5]);
        assert_eq!(
            fit_width.scale([1920.0, 720.0]),
            [1.5, 1.5],
            "fit_width keeps uniform scale"
        );
        let none = CanvasConfig {
            scale_mode: CanvasScaleMode::None,
            ..config
        };
        assert_eq!(none.scale([1920.0, 1080.0]), [1.0, 1.0]);
    }

    #[test]
    fn canvas_safe_rect_clamps_to_viewport() {
        let config = CanvasConfig {
            safe_area: [40.0, 20.0, 40.0, 20.0],
            ..Default::default()
        };
        let rect = config.safe_rect([1280.0, 720.0]);
        assert_rect(rect, [40.0, 20.0, 1200.0, 680.0]);
        // Oversized insets clamp to zero-size.
        let huge = CanvasConfig {
            safe_area: [1000.0, 1000.0, 1000.0, 1000.0],
            ..Default::default()
        };
        let rect = huge.safe_rect([1280.0, 720.0]);
        assert_eq!(rect.w, 0.0);
        assert_eq!(rect.h, 0.0);
    }

    #[test]
    fn canvas_metadata_parse_and_validation() {
        let config =
            CanvasConfig::from_metadata(Some("1920,1080"), Some("fit_width"), Some("0,0,0,0"))
                .expect("valid");
        assert_eq!(config.reference, [1920.0, 1080.0]);
        assert_eq!(config.scale_mode, CanvasScaleMode::FitWidth);
        assert!(CanvasConfig::from_metadata(Some("0,720"), None, None).is_err());
        assert!(CanvasConfig::from_metadata(None, Some("fit_x"), None).is_err());
        assert!(CanvasConfig::from_metadata(None, None, Some("1,2,3")).is_err());
    }
}
