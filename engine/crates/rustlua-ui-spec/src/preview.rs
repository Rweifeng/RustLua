//! M76-03: deterministic wireframe preview for `UiSpec`.
//!
//! Produces a real PNG (no external renderer, no GPU) so `ui_preview_spec` works
//! in CI and on hosts without a surface. The preview is intentionally a
//! wireframe: colours identify widget classes and the exact rectangles match the
//! layout contract, which is enough to review structure before acceptance.

use crate::spec::{
    Align, AnchorPreset, ContainerDirection, Justify, UiLayoutSpec, UiNode, UiNodeKind, UiSpec,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Color([u8; 4]);

const BACKGROUND: Color = Color([24, 24, 28, 255]);
const PANEL: Color = Color([40, 44, 54, 255]);
const CONTAINER: Color = Color([52, 58, 72, 255]);
const TEXT: Color = Color([196, 200, 208, 255]);
const BUTTON: Color = Color([62, 130, 84, 255]);
const IMAGE: Color = Color([96, 72, 140, 255]);
const INPUT: Color = Color([70, 96, 130, 255]);
const SCROLL: Color = Color([72, 86, 108, 255]);
const ACCENT: Color = Color([232, 172, 72, 255]);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn intersect(self, other: Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        (x1 > x0 && y1 > y0).then_some(Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

/// 设计坐标 → 预览像素的适应参数（与 [`render_rgba`] 的取景规则完全一致）。
///
/// 编辑器的画布叠层用同一组 `fit`/`offset` 把节点矩形贴到预览图上，避免预览
/// 和叠层各算一套百分比而错位。
pub fn preview_fit(reference: [f32; 2], width: u32, height: u32, scale: f32) -> (f32, [f32; 2]) {
    let fit = (width as f32 / reference[0]).min(height as f32 / reference[1]) * scale;
    let offset = [
        (width as f32 - reference[0] * fit) * 0.5,
        (height as f32 - reference[1] * fit) * 0.5,
    ];
    (fit, offset)
}

/// 一个节点在设计坐标系下的矩形与滚动裁剪。
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutRect {
    pub key: String,
    /// `[x, y, w, h]`，设计坐标（未乘预览 `fit`）。
    pub rect: [f32; 4],
    /// 父节点 key；根节点为 `None`。画布编辑要靠它把绝对矩形换算回 offsets。
    pub parent: Option<String>,
    /// 被 `scroll_view` 裁剪后的可见矩形；`None` = 未裁剪。
    pub clip: Option<[f32; 4]>,
}

/// 画布手柄：四边 + 四角。拖动时只改对应方向的边。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeHandle {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl ResizeHandle {
    pub const ALL: [Self; 8] = [
        Self::Left,
        Self::Right,
        Self::Top,
        Self::Bottom,
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    /// 手柄在矩形上的相对位置（0..=1），画布叠层按它贴手柄。
    #[must_use]
    pub fn anchor(self) -> [f32; 2] {
        match self {
            Self::Left => [0.0, 0.5],
            Self::Right => [1.0, 0.5],
            Self::Top => [0.5, 0.0],
            Self::Bottom => [0.5, 1.0],
            Self::TopLeft => [0.0, 0.0],
            Self::TopRight => [1.0, 0.0],
            Self::BottomLeft => [0.0, 1.0],
            Self::BottomRight => [1.0, 1.0],
        }
    }
}

/// 按手柄拖动 `delta`（设计像素）得到新矩形；最小边长为 `min`，反向拖过头会夹住。
#[must_use]
pub fn resize_rect(rect: [f32; 4], handle: ResizeHandle, delta: [f32; 2], min: f32) -> [f32; 4] {
    let [mut x, mut y, mut w, mut h] = rect;
    let (mut left, mut right, mut top, mut bottom) = (x, x + w, y, y + h);
    let [ax, ay] = handle.anchor();
    if ax == 0.0 {
        left += delta[0];
    } else if ax == 1.0 {
        right += delta[0];
    }
    if ay == 0.0 {
        top += delta[1];
    } else if ay == 1.0 {
        bottom += delta[1];
    }
    if right - left < min {
        if ax == 0.0 {
            left = right - min;
        } else {
            right = left + min;
        }
    }
    if bottom - top < min {
        if ay == 0.0 {
            top = bottom - min;
        } else {
            bottom = top + min;
        }
    }
    x = left;
    y = top;
    w = right - left;
    h = bottom - top;
    [x, y, w, h]
}

/// 绝对矩形 → 相对父级的 `offsets`（left, top, right, bottom）。
#[must_use]
pub fn offsets_for_rect(parent: [f32; 4], rect: [f32; 4]) -> [f32; 4] {
    let left = rect[0] - parent[0];
    let top = rect[1] - parent[1];
    let right = parent[0] + parent[2] - (rect[0] + rect[2]);
    let bottom = parent[1] + parent[3] - (rect[1] + rect[3]);
    [left, top, right, bottom]
}

/// 按 [`render_rgba`] 同一套规则解析布局，供编辑器画布叠层与几何门使用。
pub fn layout_rects(spec: &UiSpec) -> Vec<LayoutRect> {
    fn visit(
        node: &UiNode,
        rect: Rect,
        parent_key: Option<String>,
        inherited_clip: Option<Rect>,
        out: &mut Vec<LayoutRect>,
    ) {
        let clip = match (inherited_clip, &node.kind) {
            (Some(parent_clip), UiNodeKind::ScrollView { .. }) => parent_clip.intersect(rect),
            (Some(parent_clip), _) => Some(parent_clip),
            (None, UiNodeKind::ScrollView { .. }) => Some(rect),
            (None, _) => None,
        };
        out.push(LayoutRect {
            key: node.key.clone(),
            rect: [rect.x, rect.y, rect.w, rect.h],
            parent: parent_key,
            clip: clip.map(|clip| [clip.x, clip.y, clip.w, clip.h]),
        });
        for (child, child_rect) in node.children.iter().zip(resolve_children(node, rect)) {
            visit(child, child_rect, Some(node.key.clone()), clip, out);
        }
    }
    let reference = spec.document.canvas.reference;
    let canvas = Rect {
        x: 0.0,
        y: 0.0,
        w: reference[0],
        h: reference[1],
    };
    let mut out = Vec::new();
    visit(
        &spec.root,
        resolve_rect(&spec.root.layout, canvas),
        None,
        None,
        &mut out,
    );
    out
}

/// Renders a wireframe PNG. `scale` multiplies the fitted design resolution
/// (`document.canvas.reference`); `width`/`height` are the output pixels.
pub fn render_preview(
    spec: &UiSpec,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<Vec<u8>, String> {
    let (width, height, pixels) = render_rgba(spec, width, height, scale)?;
    Ok(encode_rgba_png(width, height, &pixels))
}

/// Raw RGBA wireframe. Exposed for tests and for hosts that composite the
/// preview themselves; `render_preview` wraps this with PNG encoding.
pub fn render_rgba(
    spec: &UiSpec,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<(u32, u32, Vec<u8>), String> {
    if width == 0 || height == 0 {
        return Err("preview width/height must be positive".to_owned());
    }
    if !scale.is_finite() || scale <= 0.0 {
        return Err("preview scale must be finite and positive".to_owned());
    }
    let reference = spec.document.canvas.reference;
    if reference[0] <= 0.0 || reference[1] <= 0.0 {
        return Err("canvas reference must be positive".to_owned());
    }
    let (fit, [offset_x, offset_y]) = preview_fit(reference, width, height, scale);

    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    fill(&mut pixels, width, height, BACKGROUND);

    let canvas = Rect {
        x: 0.0,
        y: 0.0,
        w: reference[0],
        h: reference[1],
    };
    draw_node(
        &spec.root,
        resolve_rect(&spec.root.layout, canvas),
        None,
        fit,
        offset_x,
        offset_y,
        width,
        height,
        &mut pixels,
    );
    Ok((width, height, pixels))
}

#[allow(clippy::too_many_arguments)]
fn draw_node(
    node: &UiNode,
    rect: Rect,
    inherited_clip: Option<Rect>,
    fit: f32,
    offset_x: f32,
    offset_y: f32,
    width: u32,
    height: u32,
    pixels: &mut [u8],
) {
    let clip = match (inherited_clip, &node.kind) {
        (Some(parent_clip), UiNodeKind::ScrollView { .. }) => parent_clip.intersect(rect),
        (Some(parent_clip), _) => Some(parent_clip),
        (None, UiNodeKind::ScrollView { .. }) => Some(rect),
        (None, _) => None,
    };
    let color = color_for(&node.kind);
    fill_logical(
        pixels, width, height, rect, clip, color, fit, offset_x, offset_y,
    );
    if matches!(
        node.kind,
        UiNodeKind::Panel { .. } | UiNodeKind::Container { .. } | UiNodeKind::Image { .. }
    ) {
        stroke_logical(
            pixels, width, height, rect, clip, ACCENT, fit, offset_x, offset_y,
        );
    }
    for (child, child_rect) in node.children.iter().zip(resolve_children(node, rect)) {
        draw_node(
            child, child_rect, clip, fit, offset_x, offset_y, width, height, pixels,
        );
    }
}

fn color_for(kind: &UiNodeKind) -> Color {
    match kind {
        UiNodeKind::Panel { .. } => PANEL,
        UiNodeKind::Container { .. } | UiNodeKind::Spacer | UiNodeKind::SafeArea => CONTAINER,
        UiNodeKind::Text { .. } | UiNodeKind::Tooltip { .. } => TEXT,
        UiNodeKind::Button { .. } | UiNodeKind::Toggle { .. } => BUTTON,
        UiNodeKind::Image { .. } => IMAGE,
        UiNodeKind::Slider { .. } | UiNodeKind::Progress { .. } => ACCENT,
        UiNodeKind::Input { .. } => INPUT,
        UiNodeKind::ScrollView { .. } | UiNodeKind::List { .. } => SCROLL,
        UiNodeKind::Grid { .. } | UiNodeKind::Tabs { .. } => CONTAINER,
        UiNodeKind::Modal => Color([30, 30, 34, 200]),
    }
}

fn anchor_fraction(anchor: AnchorPreset) -> ([f32; 2], bool, bool) {
    match anchor {
        AnchorPreset::TopLeft => ([0.0, 0.0], false, false),
        AnchorPreset::TopCenter => ([0.5, 0.0], false, false),
        AnchorPreset::TopRight => ([1.0, 0.0], false, false),
        AnchorPreset::CenterLeft => ([0.0, 0.5], false, false),
        AnchorPreset::Center => ([0.5, 0.5], false, false),
        AnchorPreset::CenterRight => ([1.0, 0.5], false, false),
        AnchorPreset::BottomLeft => ([0.0, 1.0], false, false),
        AnchorPreset::BottomCenter => ([0.5, 1.0], false, false),
        AnchorPreset::BottomRight => ([1.0, 1.0], false, false),
        AnchorPreset::StretchHorizontal => ([0.5, 0.5], true, false),
        AnchorPreset::StretchVertical => ([0.5, 0.5], false, true),
        AnchorPreset::FullRect => ([0.5, 0.5], true, true),
    }
}

/// 容器的子项矩形。
///
/// 有流向的容器（`v_box` / `h_box` / `grid` / `wrap`）按方向、`spacing`、`padding`、
/// `justify`、`align` 排布子项——预览器、`layout_rects` 与编辑器画布共用这一套几何，
/// 三者不会各算一套。没有流向的节点（面板、文本、`center`/`margin` 容器）保持
/// 子项各自锚定的绝对定位。
fn resolve_children(node: &UiNode, rect: Rect) -> Vec<Rect> {
    let UiNodeKind::Container {
        direction,
        padding,
        spacing,
        justify,
        align,
        columns,
    } = &node.kind
    else {
        return node
            .children
            .iter()
            .map(|child| resolve_rect(&child.layout, rect))
            .collect();
    };
    if matches!(
        direction,
        ContainerDirection::Center | ContainerDirection::Margin
    ) {
        return node
            .children
            .iter()
            .map(|child| resolve_rect(&child.layout, rect))
            .collect();
    }
    let pad = padding.unwrap_or([0.0; 4]);
    let inner = Rect {
        x: rect.x + pad[0],
        y: rect.y + pad[1],
        w: (rect.w - pad[0] - pad[2]).max(0.0),
        h: (rect.h - pad[1] - pad[3]).max(0.0),
    };
    let spacing = spacing.unwrap_or(0.0).max(0.0);
    // `offsets` = 画布拖拽后的手动定位：它退出流，按绝对矩形摆；兄弟项不吃它的槽位。
    let flow: Vec<usize> = node
        .children
        .iter()
        .enumerate()
        .filter(|(_, child)| child.layout.offsets.is_none())
        .map(|(index, _)| index)
        .collect();
    let sizes: Vec<[f32; 2]> = flow
        .iter()
        .map(|index| measure_child(&node.children[*index].layout, inner))
        .collect();
    let slots = match direction {
        ContainerDirection::VBox => stack(&sizes, inner, spacing, true, *justify, *align),
        ContainerDirection::HBox => stack(&sizes, inner, spacing, false, *justify, *align),
        ContainerDirection::Grid => grid(&sizes, inner, spacing, columns.unwrap_or(2)),
        ContainerDirection::Wrap => wrap(&sizes, inner, spacing),
        ContainerDirection::Center | ContainerDirection::Margin => unreachable!(),
    };
    let mut out: Vec<Rect> = node
        .children
        .iter()
        .map(|child| resolve_rect(&child.layout, rect))
        .collect();
    for (index, mut slot) in flow.iter().copied().zip(slots) {
        // 流式布局里的 `position` 是相对槽位的微调，不是绝对坐标。
        if let Some(offset) = node.children[index].layout.position {
            slot.x += offset[0];
            slot.y += offset[1];
        }
        out[index] = slot;
    }
    out
}

/// 子项在设计坐标下的尺寸：显式 `size` → 相对父级的 `percent` → 与绝对定位一致的默认值。
fn measure_child(layout: &UiLayoutSpec, inner: Rect) -> [f32; 2] {
    let mut w = layout
        .size
        .map(|size| size[0])
        .or_else(|| {
            layout
                .percent
                .map(|percent| inner.w * percent[0] / 100.0)
                .filter(|value| *value > 0.0)
        })
        .unwrap_or(120.0);
    let mut h = layout
        .size
        .map(|size| size[1])
        .or_else(|| {
            layout
                .percent
                .map(|percent| inner.h * percent[1] / 100.0)
                .filter(|value| *value > 0.0)
        })
        .unwrap_or(32.0);
    if let Some(min) = layout.min_size {
        w = w.max(min[0]);
        h = h.max(min[1]);
    }
    if let Some(max) = layout.max_size {
        w = w.min(max[0]);
        h = h.min(max[1]);
    }
    [w.max(0.0), h.max(0.0)]
}

fn cross_offset(align: Align, span: f32, size: f32) -> f32 {
    match align {
        Align::Start => 0.0,
        Align::Center => (span - size) * 0.5,
        Align::End => span - size,
        Align::Stretch => 0.0,
    }
}

/// 主轴起点与步进（步进已含 `spacing`）。
fn main_step(sizes: &[f32], spacing: f32, available: f32, justify: Justify) -> (f32, f32) {
    if sizes.is_empty() {
        return (0.0, spacing);
    }
    let used: f32 = sizes.iter().sum::<f32>() + spacing * (sizes.len() as f32 - 1.0);
    let free = (available - used).max(0.0);
    match justify {
        Justify::Start => (0.0, spacing),
        Justify::Center => (free * 0.5, spacing),
        Justify::End => (free, spacing),
        Justify::SpaceBetween if sizes.len() > 1 => {
            (0.0, spacing + free / (sizes.len() as f32 - 1.0))
        }
        Justify::SpaceBetween => (0.0, spacing),
        Justify::SpaceAround => {
            let extra = free / sizes.len() as f32;
            (extra * 0.5, spacing + extra)
        }
        Justify::SpaceEvenly => {
            let extra = free / (sizes.len() as f32 + 1.0);
            (extra, spacing + extra)
        }
    }
}

fn stack(
    sizes: &[[f32; 2]],
    inner: Rect,
    spacing: f32,
    vertical: bool,
    justify: Justify,
    align: Align,
) -> Vec<Rect> {
    let spans: Vec<f32> = if vertical {
        sizes.iter().map(|size| size[1]).collect()
    } else {
        sizes.iter().map(|size| size[0]).collect()
    };
    let available = if vertical { inner.h } else { inner.w };
    let (start, step) = main_step(&spans, spacing, available, justify);
    let mut cursor = start;
    sizes
        .iter()
        .map(|size| {
            let rect = if vertical {
                let width = if align == Align::Stretch {
                    inner.w
                } else {
                    size[0]
                };
                Rect {
                    x: inner.x + cross_offset(align, inner.w, width),
                    y: inner.y + cursor,
                    w: width,
                    h: size[1],
                }
            } else {
                let height = if align == Align::Stretch {
                    inner.h
                } else {
                    size[1]
                };
                Rect {
                    x: inner.x + cursor,
                    y: inner.y + cross_offset(align, inner.h, height),
                    w: size[0],
                    h: height,
                }
            };
            cursor += if vertical { size[1] } else { size[0] } + step;
            rect
        })
        .collect()
}

fn grid(sizes: &[[f32; 2]], inner: Rect, spacing: f32, columns: u32) -> Vec<Rect> {
    let columns = columns.max(1) as usize;
    let cell_w = (inner.w - spacing * (columns as f32 - 1.0)).max(0.0) / columns as f32;
    let mut out = Vec::with_capacity(sizes.len());
    let mut y = inner.y;
    for row in sizes.chunks(columns) {
        let row_h = row.iter().map(|size| size[1]).fold(0.0, f32::max);
        for (column, size) in row.iter().enumerate() {
            out.push(Rect {
                x: inner.x + column as f32 * (cell_w + spacing),
                y,
                w: if size[0] > 0.0 { size[0] } else { cell_w },
                h: size[1],
            });
        }
        y += row_h + spacing;
    }
    out
}

fn wrap(sizes: &[[f32; 2]], inner: Rect, spacing: f32) -> Vec<Rect> {
    let mut out = Vec::with_capacity(sizes.len());
    let mut x = inner.x;
    let mut y = inner.y;
    let mut row_h = 0.0_f32;
    for size in sizes {
        if x > inner.x && x + size[0] > inner.x + inner.w {
            x = inner.x;
            y += row_h + spacing;
            row_h = 0.0;
        }
        out.push(Rect {
            x,
            y,
            w: size[0],
            h: size[1],
        });
        x += size[0] + spacing;
        row_h = row_h.max(size[1]);
    }
    out
}

fn resolve_rect(layout: &UiLayoutSpec, parent: Rect) -> Rect {
    let (anchor, stretch_x, stretch_y) = anchor_fraction(layout.anchor);
    if let Some(offsets) = layout.offsets {
        return Rect {
            x: parent.x + offsets[0],
            y: parent.y + offsets[1],
            w: (parent.w - offsets[0] - offsets[2]).max(0.0),
            h: (parent.h - offsets[1] - offsets[3]).max(0.0),
        };
    }
    let mut w = layout.size.map_or_else(
        || {
            layout.percent.map_or(120.0, |percent| {
                if percent[0] > 0.0 {
                    parent.w * percent[0] / 100.0
                } else {
                    120.0
                }
            })
        },
        |size| size[0],
    );
    let mut h = layout.size.map_or_else(
        || {
            layout.percent.map_or(32.0, |percent| {
                if percent[1] > 0.0 {
                    parent.h * percent[1] / 100.0
                } else {
                    32.0
                }
            })
        },
        |size| size[1],
    );
    if stretch_x {
        w = parent.w;
    }
    if stretch_y {
        h = parent.h;
    }
    if let Some(min) = layout.min_size {
        w = w.max(min[0]);
        h = h.max(min[1]);
    }
    if let Some(max) = layout.max_size {
        w = w.min(max[0]);
        h = h.min(max[1]);
    }
    let pivot = layout.pivot.unwrap_or(anchor);
    let position = layout.position.unwrap_or([0.0, 0.0]);
    let x = parent.x + parent.w * anchor[0] + position[0] - w * pivot[0];
    let y = parent.y + parent.h * anchor[1] + position[1] - h * pivot[1];
    Rect {
        x,
        y,
        w: w.max(0.0),
        h: h.max(0.0),
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_logical(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    rect: Rect,
    clip: Option<Rect>,
    color: Color,
    fit: f32,
    offset_x: f32,
    offset_y: f32,
) {
    let Some(rect) = clip.map_or(Some(rect), |clip| rect.intersect(clip)) else {
        return;
    };
    let x0 = (offset_x + rect.x * fit).floor() as i64;
    let y0 = (offset_y + rect.y * fit).floor() as i64;
    let x1 = (offset_x + (rect.x + rect.w) * fit).ceil() as i64;
    let y1 = (offset_y + (rect.y + rect.h) * fit).ceil() as i64;
    for y in y0.max(0)..y1.min(i64::from(height)) {
        for x in x0.max(0)..x1.min(i64::from(width)) {
            let index = (y as usize * width as usize + x as usize) * 4;
            blend(&mut pixels[index..index + 4], color.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stroke_logical(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    rect: Rect,
    clip: Option<Rect>,
    color: Color,
    fit: f32,
    offset_x: f32,
    offset_y: f32,
) {
    let edges = [
        Rect {
            h: 1.0 / fit,
            ..rect
        },
        Rect {
            y: rect.y + rect.h - 1.0 / fit,
            h: 1.0 / fit,
            ..rect
        },
        Rect {
            w: 1.0 / fit,
            ..rect
        },
        Rect {
            x: rect.x + rect.w - 1.0 / fit,
            w: 1.0 / fit,
            ..rect
        },
    ];
    for edge in edges {
        fill_logical(
            pixels, width, height, edge, clip, color, fit, offset_x, offset_y,
        );
    }
}

fn blend(dst: &mut [u8], src: [u8; 4]) {
    let alpha = u32::from(src[3]);
    if alpha == 255 {
        dst.copy_from_slice(&src);
        return;
    }
    let inverse = 255 - alpha;
    for channel in 0..3 {
        dst[channel] =
            ((u32::from(src[channel]) * alpha + u32::from(dst[channel]) * inverse) / 255) as u8;
    }
    dst[3] = 255;
}

fn fill(pixels: &mut [u8], width: u32, height: u32, color: Color) {
    for y in 0..height {
        for x in 0..width {
            let index = (y as usize * width as usize + x as usize) * 4;
            pixels[index..index + 4].copy_from_slice(&color.0);
        }
    }
}

/// Encodes raw RGBA bytes as PNG (filter 0, deterministic). Shared with the
/// local image provider so generated assets and previews use one encoder.
#[must_use]
pub fn encode_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        width as usize * height as usize * 4,
        "RGBA 缓冲长度与尺寸不符"
    );
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0); // filter: none
        let start = y * width as usize * 4;
        raw.extend_from_slice(&rgba[start..start + width as usize * 4]);
    }
    let mut zlib = vec![0x78, 0x01];
    for (index, block) in raw.chunks(65_535).enumerate() {
        let last = (index + 1) * 65_535 >= raw.len();
        zlib.push(u8::from(last));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&png_chunk(b"IDAT", &zlib));
    png.extend_from_slice(&png_chunk(b"IEND", &[]));
    png
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = Vec::with_capacity(data.len() + 12);
    chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
    chunk.extend_from_slice(kind);
    chunk.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(data.len() + 4);
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    chunk.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    chunk
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1_u32;
    let mut b = 0_u32;
    for byte in data {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> UiSpec {
        serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"name": "preview"},
            "root": {"key": "root", "type": "panel",
                     "layout": {"anchor": "center", "size": [200, 100]},
                     "children": [
                         {"key": "label", "type": "text", "text": "Hi",
                          "layout": {"anchor": "center", "size": [80, 20]}}
                     ]}
        }))
        .unwrap()
    }

    #[test]
    fn renders_valid_png_deterministically() {
        let png = render_preview(&spec(), 320, 180, 1.0).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((width, height), (320, 180));
        assert_eq!(png, render_preview(&spec(), 320, 180, 1.0).unwrap());
        assert_ne!(png, render_preview(&spec(), 320, 180, 2.0).unwrap());
    }

    #[test]
    fn draws_widget_rectangles_at_their_layout_position() {
        let mut spec = spec();
        // 320×180 output over a 1280×720 reference → fit 0.25. The panel is
        // centred: 200×100 → 50×25 px at (135, 77.5).
        let (_, _, pixels) = render_rgba(&spec, 320, 180, 1.0).unwrap();
        let pixel = |x: usize, y: usize| {
            let index = (y * 320 + x) * 4;
            [pixels[index], pixels[index + 1], pixels[index + 2]]
        };
        assert_eq!(pixel(160, 90), [TEXT.0[0], TEXT.0[1], TEXT.0[2]]);
        assert_eq!(pixel(140, 80), [PANEL.0[0], PANEL.0[1], PANEL.0[2]]);
        // Text child is centred and overwrites the panel centre.
        spec.root.children[0].layout.size = Some([300.0, 60.0]);
        let (_, _, pixels) = render_rgba(&spec, 320, 180, 1.0).unwrap();
        let pixel = |x: usize, y: usize| {
            let index = (y * 320 + x) * 4;
            [pixels[index], pixels[index + 1], pixels[index + 2]]
        };
        assert_eq!(pixel(150, 85), [TEXT.0[0], TEXT.0[1], TEXT.0[2]]);
    }

    #[test]
    fn layout_rects_follow_the_renderer_geometry() {
        let rects = layout_rects(&spec());
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].key, "root");
        assert_eq!(rects[0].rect, [540.0, 310.0, 200.0, 100.0]);
        assert_eq!(rects[0].clip, None);
        assert_eq!(rects[1].key, "label");
        assert_eq!(rects[1].rect, [600.0, 350.0, 80.0, 20.0]);

        // 320×180 输出、1280×720 参考分辨率 → fit 0.25，无信箱边。
        let (fit, offset) = preview_fit([1280.0, 720.0], 320, 180, 1.0);
        assert_eq!((fit, offset), (0.25, [0.0, 0.0]));
        let (fit, offset) = preview_fit([1280.0, 720.0], 375, 667, 1.0);
        assert!((fit - 0.292_968_75).abs() < 1e-6, "{fit}");
        assert_eq!(offset[0], 0.0);
        assert!((offset[1] - 228.031_25).abs() < 1e-4, "{offset:?}");
    }

    #[test]
    fn layout_rects_keep_scroll_clip_for_children() {
        let spec: UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"name": "scroll"},
            "root": {"key": "root", "type": "panel",
                     "layout": {"anchor": "full_rect"},
                     "children": [{
                         "key": "scroller", "type": "scroll_view",
                         "layout": {"anchor": "top_left", "size": [100, 50]},
                         "children": [{
                             "key": "out", "type": "panel",
                             "layout": {"anchor": "top_left", "position": [80, 0],
                                        "size": [40, 40]}
                         }]
                     }]}
        }))
        .unwrap();
        let rects = layout_rects(&spec);
        let scroller = rects.iter().find(|rect| rect.key == "scroller").unwrap();
        assert_eq!(scroller.rect, [0.0, 0.0, 100.0, 50.0]);
        assert_eq!(scroller.clip, Some([0.0, 0.0, 100.0, 50.0]));
        let out = rects.iter().find(|rect| rect.key == "out").unwrap();
        assert_eq!(out.rect, [80.0, 0.0, 40.0, 40.0], "矩形本身不被改写");
        assert_eq!(out.clip, Some([0.0, 0.0, 100.0, 50.0]), "子节点继承裁剪");
    }

    #[test]
    fn flow_containers_stack_children_with_spacing_and_alignment() {
        let spec: UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"name": "flow", "canvas": {"reference": [400, 300]}},
            "root": {"key": "root", "type": "panel",
                     "layout": {"anchor": "top_left", "size": [400, 300]},
                     "children": [{
                         "key": "box", "type": "container", "direction": "v_box",
                         "padding": [10, 20, 10, 20], "spacing": 8, "align": "center",
                         "layout": {"anchor": "top_left", "size": [300, 200]},
                         "children": [
                             {"key": "a", "type": "button", "text": "A",
                              "layout": {"size": [100, 40]}},
                             {"key": "b", "type": "button", "text": "B",
                              "layout": {"size": [160, 40]}},
                             {"key": "c", "type": "button", "text": "C",
                              "layout": {"size": [60, 40]}}
                         ]
                     }]}
        }))
        .unwrap();
        let rows = layout_rects(&spec);
        let rect = |key: &str| rows.iter().find(|row| row.key == key).unwrap().rect;
        // 内容区 = (10,20,280,160)：v_box 按 spacing 8 竖排，交叉轴居中。
        assert_eq!(rect("a"), [100.0, 20.0, 100.0, 40.0]);
        assert_eq!(rect("b"), [70.0, 68.0, 160.0, 40.0]);
        assert_eq!(rect("c"), [120.0, 116.0, 60.0, 40.0]);

        // h_box + 主轴居中：三块 60 宽、间距 10，可用宽度 300。
        let hbox: UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"name": "row", "canvas": {"reference": [400, 300]}},
            "root": {"key": "root", "type": "panel",
                     "layout": {"anchor": "top_left", "size": [400, 300]},
                     "children": [{
                         "key": "row", "type": "container", "direction": "h_box",
                         "spacing": 10, "justify": "center",
                         "layout": {"anchor": "top_left", "size": [300, 100]},
                         "children": [
                             {"key": "a", "type": "button", "text": "A",
                              "layout": {"size": [60, 40]}},
                             {"key": "b", "type": "button", "text": "B",
                              "layout": {"size": [60, 40]}},
                             {"key": "c", "type": "button", "text": "C",
                              "layout": {"size": [60, 40]}}
                         ]
                     }]}
        }))
        .unwrap();
        let rows = layout_rects(&hbox);
        let rect = |key: &str| rows.iter().find(|row| row.key == key).unwrap().rect;
        assert_eq!(rect("a")[0], 50.0, "主轴居中：(300 - 200) / 2");
        assert_eq!(rect("b")[0], 120.0);
        assert_eq!(rect("c")[0], 190.0);
        assert_eq!(rect("a")[1], 0.0, "交叉轴默认 start");
    }

    #[test]
    fn flow_children_with_offsets_are_absolutely_placed() {
        let mut spec: UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"name": "drag", "canvas": {"reference": [400, 300]}},
            "root": {"key": "root", "type": "panel",
                     "layout": {"anchor": "top_left", "size": [400, 300]},
                     "children": [{
                         "key": "box", "type": "container", "direction": "v_box",
                         "spacing": 8,
                         "layout": {"anchor": "top_left", "size": [300, 200]},
                         "children": [
                             {"key": "a", "type": "button", "text": "A",
                              "layout": {"size": [100, 40]}},
                             {"key": "b", "type": "button", "text": "B",
                              "layout": {"size": [100, 40]}}
                         ]
                     }]}
        }))
        .unwrap();
        assert_eq!(layout_rects(&spec)[2].rect, [0.0, 0.0, 100.0, 40.0]);
        // 画布把 a 拖到 (50,60)：offsets 是相对容器的四边距离，之后就按绝对定位走。
        let box_node = &mut spec.root.children[0];
        let a = &mut box_node.children[0];
        a.layout.offsets = Some([50.0, 60.0, 150.0, 100.0]);
        a.layout.size = None;
        let rows = layout_rects(&spec);
        let rect = |key: &str| rows.iter().find(|row| row.key == key).unwrap().rect;
        assert_eq!(rect("a"), [50.0, 60.0, 100.0, 40.0]);
        assert_eq!(
            rect("b"),
            [0.0, 0.0, 100.0, 40.0],
            "a 脱离流之后 b 顶上第一槽，不留空位"
        );
    }

    #[test]
    fn rejects_zero_sizes_and_bad_scale() {
        assert!(render_preview(&spec(), 0, 180, 1.0).is_err());
        assert!(render_preview(&spec(), 320, 0, 1.0).is_err());
        assert!(render_preview(&spec(), 320, 180, 0.0).is_err());
        assert!(render_preview(&spec(), 320, 180, f32::NAN).is_err());
    }

    #[test]
    fn layout_rects_report_parents_for_canvas_editing() {
        let spec: UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "tree", "canvas": {"reference": [200, 100]}},
            "root": {"key": "root", "type": "panel", "children": [
                {"key": "card", "type": "container", "children": [
                    {"key": "label", "type": "text", "text": "Hi"}
                ]}
            ]}
        }))
        .unwrap();
        let rows = layout_rects(&spec);
        let by_key = |key: &str| rows.iter().find(|row| row.key == key).unwrap();
        assert_eq!(by_key("root").parent, None);
        assert_eq!(by_key("card").parent.as_deref(), Some("root"));
        assert_eq!(by_key("label").parent.as_deref(), Some("card"));
    }

    #[test]
    fn resize_handles_move_only_their_own_edges() {
        let rect = [10.0, 20.0, 100.0, 50.0];
        assert_eq!(
            resize_rect(rect, ResizeHandle::Right, [30.0, 99.0], 8.0),
            [10.0, 20.0, 130.0, 50.0],
            "右边手柄只吃 x"
        );
        assert_eq!(
            resize_rect(rect, ResizeHandle::TopLeft, [5.0, -5.0], 8.0),
            [15.0, 15.0, 95.0, 55.0],
            "左上角同时推左/上边"
        );
        assert_eq!(
            resize_rect(rect, ResizeHandle::Left, [200.0, 0.0], 8.0),
            [102.0, 20.0, 8.0, 50.0],
            "反向拖过头夹到最小边长"
        );
        assert_eq!(
            resize_rect(rect, ResizeHandle::Bottom, [0.0, -200.0], 8.0),
            [10.0, 20.0, 100.0, 8.0]
        );
    }

    #[test]
    fn offsets_for_rect_round_trips_against_resolve_rect() {
        let parent = Rect {
            x: 100.0,
            y: 50.0,
            w: 400.0,
            h: 300.0,
        };
        let absolute = [140.0, 90.0, 120.0, 60.0];
        let offsets = offsets_for_rect([100.0, 50.0, 400.0, 300.0], absolute);
        assert_eq!(offsets, [40.0, 40.0, 240.0, 200.0]);
        let layout = UiLayoutSpec {
            offsets: Some(offsets),
            ..Default::default()
        };
        let resolved = resolve_rect(&layout, parent);
        assert_eq!([resolved.x, resolved.y, resolved.w, resolved.h], absolute);
    }
}
