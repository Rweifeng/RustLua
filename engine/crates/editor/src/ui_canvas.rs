//! UI Studio 画布：把 `UiSpec` 按样式画成真实内容（底色/文字/图片），并负责
//! 设计坐标 ↔ 屏幕坐标、命中测试与缩放锚点这些纯几何。
//!
//! 线框 PNG（`rustlua_ui_spec::render_preview`）仍是 `ui_preview_spec` 与发布门
//! 的产物；编辑器画布需要看到设计本身，所以这里直接按 spec 绘制，几何只认
//! `layout_rects` 一份，叠层与画布不会各算一套。

use eframe::egui;
use rustlua_ui_spec::{LayoutRect, UiNode, UiNodeKind, UiSpec, UiStyle};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) const MIN_ZOOM: f32 = 0.1;
pub(crate) const MAX_ZOOM: f32 = 8.0;

/// 设计坐标 → 屏幕坐标的取景。
///
/// 两级映射与预览器一致：先用 `fit`/`offset` 把设计参考分辨率落到预览分辨率
/// 像素（含信箱边），再用 `scale`（含用户缩放）落到屏幕。`pan` 是屏幕像素位移，
/// 与缩放解耦，改变窗口大小不会把用户已经平移到的地方弹回去。
#[derive(Clone, Copy, Debug)]
pub(crate) struct CanvasView {
    pub viewport: egui::Rect,
    pub resolution: [f32; 2],
    /// 设计参考分辨率（`document.canvas.reference`）。
    pub reference: [f32; 2],
    pub fit: f32,
    pub offset: [f32; 2],
    /// 预览分辨率像素 → 屏幕像素（基础适配 × 用户缩放）。
    pub scale: f32,
    pub pan: egui::Vec2,
}

impl CanvasView {
    pub(crate) fn new(
        viewport: egui::Rect,
        resolution: [f32; 2],
        reference: [f32; 2],
        zoom: f32,
        pan: egui::Vec2,
    ) -> Self {
        let (fit, offset) = rustlua_ui_spec::preview_fit(
            reference,
            resolution[0].max(1.0) as u32,
            resolution[1].max(1.0) as u32,
            1.0,
        );
        let margin = 12.0;
        let base = ((viewport.width() - margin * 2.0).max(32.0) / resolution[0].max(1.0))
            .min((viewport.height() - margin * 2.0).max(32.0) / resolution[1].max(1.0))
            .clamp(0.02, 4.0);
        Self {
            viewport,
            resolution,
            reference,
            fit,
            offset,
            scale: base * zoom.clamp(MIN_ZOOM, MAX_ZOOM),
            pan,
        }
    }

    /// 设计像素 → 屏幕像素的总比例（画字号、线宽都用它）。
    pub(crate) fn design_scale(&self) -> f32 {
        self.fit * self.scale
    }

    fn resolution_to_screen(&self, point: [f32; 2]) -> egui::Pos2 {
        let center = self.viewport.center();
        egui::pos2(
            center.x + self.pan.x + (point[0] - self.resolution[0] * 0.5) * self.scale,
            center.y + self.pan.y + (point[1] - self.resolution[1] * 0.5) * self.scale,
        )
    }

    pub(crate) fn design_to_screen(&self, rect: [f32; 4]) -> egui::Rect {
        let min = self.resolution_to_screen([
            self.offset[0] + rect[0] * self.fit,
            self.offset[1] + rect[1] * self.fit,
        ]);
        egui::Rect::from_min_size(
            min,
            egui::vec2(rect[2] * self.design_scale(), rect[3] * self.design_scale()),
        )
    }

    pub(crate) fn screen_to_design(&self, point: egui::Pos2) -> [f32; 2] {
        let center = self.viewport.center();
        let x = (point.x - center.x - self.pan.x) / self.scale.max(1e-4) + self.resolution[0] * 0.5;
        let y = (point.y - center.y - self.pan.y) / self.scale.max(1e-4) + self.resolution[1] * 0.5;
        [
            (x - self.offset[0]) / self.fit.max(1e-4),
            (y - self.offset[1]) / self.fit.max(1e-4),
        ]
    }

    /// 设计画布（`document.canvas.reference`）在屏幕上的矩形。
    pub(crate) fn canvas_rect(&self) -> egui::Rect {
        self.design_to_screen([0.0, 0.0, self.reference[0], self.reference[1]])
    }

    /// 以光标为锚点缩放：缩放前后光标下的设计点不动。返回新的 `(zoom, pan)`。
    pub(crate) fn zoomed_at(&self, factor: f32, cursor: egui::Pos2) -> (f32, egui::Vec2) {
        let anchor = self.screen_to_design(cursor);
        let zoom = ((self.scale * factor) / self.base_scale()).clamp(MIN_ZOOM, MAX_ZOOM);
        let scale = self.base_scale() * zoom;
        let center = self.viewport.center();
        let pan = egui::vec2(
            cursor.x
                - center.x
                - (self.offset[0] + anchor[0] * self.fit - self.resolution[0] * 0.5) * scale,
            cursor.y
                - center.y
                - (self.offset[1] + anchor[1] * self.fit - self.resolution[1] * 0.5) * scale,
        );
        (zoom, pan)
    }

    /// 当前缩放下的“基础适配”比例：`scale = base_scale * zoom`。
    fn base_scale(&self) -> f32 {
        let margin = 12.0;
        ((self.viewport.width() - margin * 2.0).max(32.0) / self.resolution[0].max(1.0))
            .min((self.viewport.height() - margin * 2.0).max(32.0) / self.resolution[1].max(1.0))
            .clamp(0.02, 4.0)
    }
}

/// 命中测试：后画的（子）节点优先。被 `scroll_view` 裁掉的区域不可点。
pub(crate) fn pick_node(rows: &[LayoutRect], point: [f32; 2]) -> Option<&LayoutRect> {
    rows.iter()
        .rev()
        .find(|row| contains(row.rect, point) && row.clip.is_none_or(|clip| contains(clip, point)))
}

fn contains(rect: [f32; 4], point: [f32; 2]) -> bool {
    point[0] >= rect[0]
        && point[1] >= rect[1]
        && point[0] <= rect[0] + rect[2]
        && point[1] <= rect[1] + rect[3]
}

/// 画布：底色、整张预览分辨率框、spec 内容。
pub(crate) fn paint(
    ui: &egui::Ui,
    spec: &UiSpec,
    view: CanvasView,
    textures: &mut impl FnMut(&str) -> Option<egui::TextureHandle>,
) {
    let painter = ui.painter().with_clip_rect(view.viewport);
    painter.rect_filled(view.viewport, 0.0, egui::Color32::from_rgb(14, 14, 17));
    let canvas = view.canvas_rect();
    painter.rect_filled(canvas, 0.0, egui::Color32::from_rgb(24, 24, 28));
    painter.rect_stroke(
        canvas,
        0.0,
        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(72)),
        egui::StrokeKind::Inside,
    );
    let rows = rustlua_ui_spec::layout_rects(spec);
    let index: BTreeMap<String, LayoutRect> =
        rows.into_iter().map(|row| (row.key.clone(), row)).collect();
    draw_node(&painter, spec, &spec.root, &index, view, textures, None);
}

#[allow(clippy::too_many_arguments)]
fn draw_node(
    painter: &egui::Painter,
    spec: &UiSpec,
    node: &UiNode,
    index: &BTreeMap<String, LayoutRect>,
    view: CanvasView,
    textures: &mut impl FnMut(&str) -> Option<egui::TextureHandle>,
    inherited_clip: Option<egui::Rect>,
) {
    let Some(row) = index.get(&node.key) else {
        return;
    };
    let rect = view.design_to_screen(row.rect);
    let clip = row
        .clip
        .map(|clip| view.design_to_screen(clip))
        .or(inherited_clip);
    let painter = clip.map_or_else(
        || painter.clone(),
        |clip| painter.with_clip_rect(clip.intersect(painter.clip_rect())),
    );
    let style = Style::new(
        node.style.as_deref().and_then(|name| spec.styles.get(name)),
        &spec.tokens,
    );
    let opacity = style.number("opacity").unwrap_or(1.0).clamp(0.0, 1.0);
    let radius = style.number("radius").unwrap_or(0.0) * view.design_scale();
    if let Some(bg) = style.color("bg").or_else(|| style.color("background")) {
        painter.rect_filled(rect, radius, with_alpha(bg, opacity));
    }
    if let Some(src) = node_src(node) {
        paint_image(
            &painter,
            spec,
            src,
            node_nine_slice(node),
            rect,
            view,
            textures,
        );
    }
    if let (Some(width), Some(color)) = (style.number("border_width"), style.color("border_color"))
    {
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(
                (width * view.design_scale()).max(1.0),
                with_alpha(color, opacity),
            ),
            egui::StrokeKind::Inside,
        );
    }
    let text = match &node.kind {
        UiNodeKind::Text { text } => Some(text.as_str()),
        UiNodeKind::Button { text, .. } => text.as_deref(),
        _ => None,
    };
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        let default_align = if matches!(node.kind, UiNodeKind::Button { .. }) {
            egui::Align::Center
        } else {
            egui::Align::Min
        };
        let align = match style.text("text_align") {
            Some("center") => egui::Align::Center,
            Some("right") => egui::Align::Max,
            Some("left") => egui::Align::Min,
            _ => default_align,
        };
        let size = style.number("font_size").unwrap_or(16.0) * view.design_scale();
        let color = style
            .color("color")
            .map_or(egui::Color32::from_rgb(236, 223, 198), |color| {
                with_alpha(color, opacity)
            });
        let pos = egui::pos2(
            match align {
                egui::Align::Center => rect.center().x,
                egui::Align::Max => rect.right(),
                _ => rect.left(),
            },
            rect.center().y,
        );
        let anchor = match align {
            egui::Align::Center => egui::Align2::CENTER_CENTER,
            egui::Align::Max => egui::Align2::RIGHT_CENTER,
            _ => egui::Align2::LEFT_CENTER,
        };
        painter.text(
            pos,
            anchor,
            text,
            egui::FontId::proportional(size.max(1.0)),
            color,
        );
    }
    for child in &node.children {
        draw_node(&painter, spec, child, index, view, textures, clip);
    }
}

fn node_src(node: &UiNode) -> Option<&str> {
    match &node.kind {
        UiNodeKind::Panel { src } => src.as_deref(),
        UiNodeKind::Image { src, .. } => Some(src.as_str()),
        UiNodeKind::Button { src, .. } => src.as_deref(),
        _ => None,
    }
}

fn node_nine_slice(node: &UiNode) -> Option<[f32; 4]> {
    match &node.kind {
        UiNodeKind::Image { nine_slice, .. } => *nine_slice,
        _ => None,
    }
}

fn paint_image(
    painter: &egui::Painter,
    spec: &UiSpec,
    src: &str,
    nine_slice: Option<[f32; 4]>,
    rect: egui::Rect,
    view: CanvasView,
    textures: &mut impl FnMut(&str) -> Option<egui::TextureHandle>,
) {
    let Some(path) = asset_path(spec, src) else {
        return;
    };
    let Some(texture) = textures(path) else {
        return;
    };
    let Some(insets) = nine_slice else {
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        return;
    };
    let size = texture.size_vec2();
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    // 九宫格：四边内缩在设计里给，按屏幕比例换成源图像素并夹到一半以内。
    let scale = view.design_scale().max(1e-4);
    let left = (insets[0] * scale).clamp(0.0, rect.width() * 0.5);
    let top = (insets[1] * scale).clamp(0.0, rect.height() * 0.5);
    let right = (insets[2] * scale).clamp(0.0, rect.width() - left);
    let bottom = (insets[3] * scale).clamp(0.0, rect.height() - top);
    let u_left = left / rect.width().max(1.0);
    let u_top = top / rect.height().max(1.0);
    let u_right = right / rect.width().max(1.0);
    let u_bottom = bottom / rect.height().max(1.0);
    let uv = |x0: f32, y0: f32, x1: f32, y1: f32| {
        egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1.max(x0), y1.max(y0)))
    };
    let colors = [
        uv(0.0, 0.0, u_left, u_top),
        uv(u_left, 0.0, 1.0 - u_right, u_top),
        uv(1.0 - u_right, 0.0, 1.0, u_top),
        uv(0.0, u_top, u_left, 1.0 - u_bottom),
        uv(u_left, u_top, 1.0 - u_right, 1.0 - u_bottom),
        uv(1.0 - u_right, u_top, 1.0, 1.0 - u_bottom),
        uv(0.0, 1.0 - u_bottom, u_left, 1.0),
        uv(u_left, 1.0 - u_bottom, 1.0 - u_right, 1.0),
        uv(1.0 - u_right, 1.0 - u_bottom, 1.0, 1.0),
    ];
    let x = [rect.left(), rect.left() + left, rect.right() - right];
    let y = [rect.top(), rect.top() + top, rect.bottom() - bottom];
    let widths = [left, rect.width() - left - right, right];
    let heights = [top, rect.height() - top - bottom, bottom];
    for (row, height) in heights.iter().enumerate() {
        for (column, width) in widths.iter().enumerate() {
            if *width <= 0.0 || *height <= 0.0 {
                continue;
            }
            let dest = egui::Rect::from_min_size(
                egui::pos2(x[column], y[row]),
                egui::vec2(*width, *height),
            );
            painter.image(
                texture.id(),
                dest,
                colors[row * 3 + column],
                egui::Color32::WHITE,
            );
        }
    }
}

/// `asset:<key>` → 工程内相对路径；其余按路径处理（校验器负责拦非法值）。
pub(crate) fn asset_path<'a>(spec: &'a UiSpec, src: &'a str) -> Option<&'a str> {
    if let Some(key) = src.strip_prefix("asset:") {
        return spec
            .assets
            .iter()
            .find(|asset| asset.key == key)
            .and_then(|asset| asset.path.as_deref());
    }
    (!src.trim().is_empty()).then_some(src)
}

fn with_alpha(color: egui::Color32, alpha: f32) -> egui::Color32 {
    let a = (f32::from(color.a()) * alpha).round().clamp(0.0, 255.0) as u8;
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

/// 解析样式值：`@token` 取令牌，颜色支持 `#rgb/#rgba/#rrggbb/#rrggbbaa`、
/// `transparent` 与 `[r,g,b(,a)]`（0..=1 或 0..=255）。
pub(crate) fn parse_color(value: &Value) -> Option<egui::Color32> {
    match value {
        Value::String(text) => parse_color_str(text),
        Value::Array(items) if items.len() >= 3 => {
            let channel = |index: usize| -> Option<u8> {
                let value = items.get(index)?.as_f64()?;
                Some(if value <= 1.0 {
                    (value * 255.0).round().clamp(0.0, 255.0) as u8
                } else {
                    value.round().clamp(0.0, 255.0) as u8
                })
            };
            let alpha = items
                .get(3)
                .and_then(|value| value.as_f64())
                .map_or(255, |value| {
                    if value <= 1.0 {
                        (value * 255.0).round().clamp(0.0, 255.0) as u8
                    } else {
                        value.round().clamp(0.0, 255.0) as u8
                    }
                });
            Some(egui::Color32::from_rgba_unmultiplied(
                channel(0)?,
                channel(1)?,
                channel(2)?,
                alpha,
            ))
        }
        _ => None,
    }
}

fn parse_color_str(text: &str) -> Option<egui::Color32> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("transparent") {
        return Some(egui::Color32::TRANSPARENT);
    }
    let hex = text.strip_prefix('#')?;
    let digit =
        |index: usize| -> Option<u8> { u8::from_str_radix(hex.get(index..index + 1)?, 16).ok() };
    let pair =
        |index: usize| -> Option<u8> { u8::from_str_radix(hex.get(index..index + 2)?, 16).ok() };
    match hex.len() {
        3 => Some(egui::Color32::from_rgb(
            digit(0)? * 17,
            digit(1)? * 17,
            digit(2)? * 17,
        )),
        4 => Some(egui::Color32::from_rgba_unmultiplied(
            digit(0)? * 17,
            digit(1)? * 17,
            digit(2)? * 17,
            digit(3)? * 17,
        )),
        6 => Some(egui::Color32::from_rgb(pair(0)?, pair(2)?, pair(4)?)),
        8 => Some(egui::Color32::from_rgba_unmultiplied(
            pair(0)?,
            pair(2)?,
            pair(4)?,
            pair(6)?,
        )),
        _ => None,
    }
}

/// 一份样式的读值入口：令牌解引用 + 类型转换只在这里做一次。
#[derive(Clone, Copy)]
pub(crate) struct Style<'a> {
    base: Option<&'a BTreeMap<String, Value>>,
    tokens: &'a BTreeMap<String, Value>,
}

impl<'a> Style<'a> {
    pub(crate) fn new(style: Option<&'a UiStyle>, tokens: &'a BTreeMap<String, Value>) -> Self {
        Self {
            base: style.map(|style| &style.base),
            tokens,
        }
    }

    fn value(&self, key: &str) -> Option<&'a Value> {
        let value = self.base?.get(key)?;
        match value {
            Value::String(text) => text
                .strip_prefix('@')
                .and_then(|name| self.tokens.get(name))
                .or(Some(value)),
            _ => Some(value),
        }
    }

    pub(crate) fn number(&self, key: &str) -> Option<f32> {
        self.value(key)
            .and_then(Value::as_f64)
            .map(|value| value as f32)
    }

    pub(crate) fn color(&self, key: &str) -> Option<egui::Color32> {
        self.value(key).and_then(parse_color)
    }

    pub(crate) fn text(&self, key: &str) -> Option<&'a str> {
        self.value(key).and_then(Value::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> CanvasView {
        CanvasView::new(
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 600.0)),
            [1280.0, 720.0],
            [1280.0, 720.0],
            1.0,
            egui::Vec2::ZERO,
        )
    }

    #[test]
    fn design_and_screen_coordinates_round_trip() {
        let view = view();
        let rect = view.design_to_screen([100.0, 50.0, 200.0, 100.0]);
        let back = view.screen_to_design(rect.min);
        assert!((back[0] - 100.0).abs() < 1e-3, "{back:?}");
        assert!((back[1] - 50.0).abs() < 1e-3, "{back:?}");
        assert!((rect.width() - 200.0 * view.design_scale()).abs() < 1e-3);
    }

    #[test]
    fn paint_emits_text_and_rectangles_for_a_spec() {
        let spec: UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "blacksmith",
                         "canvas": {"reference": [1280.0, 720.0]}},
            "tokens": {"color.ink": "#ecdfc6", "color.ember": "#d9873f"},
            "styles": {
                "panel": {"base": {"bg": "#211d19", "radius": 14}},
                "title": {"base": {"color": "@color.ember", "font_size": 32,
                                    "text_align": "center"}},
                "primary": {"base": {"bg": "#6f3a1c", "color": "@color.ink", "radius": 8}}
            },
            "root": {"key": "root", "type": "panel", "style": "panel",
                     "layout": {"anchor": "center", "size": [720, 440]},
                     "children": [
                         {"key": "title", "type": "text", "text": "Blacksmith",
                          "style": "title",
                          "layout": {"anchor": "top_center", "position": [0, 30],
                                     "size": [340, 44]}},
                         {"key": "actions", "type": "container", "direction": "v_box",
                          "spacing": 12, "align": "center",
                          "layout": {"anchor": "center", "position": [0, 52],
                                     "size": [320, 162]},
                          "children": [
                              {"key": "forge", "type": "button", "text": "Forge",
                               "style": "primary", "layout": {"size": [320, 46]}},
                              {"key": "repair", "type": "button", "text": "Repair",
                               "style": "primary", "layout": {"size": [320, 46]}}
                          ]}
                     ]}
        }))
        .unwrap();
        let context = egui::Context::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    let view = CanvasView::new(
                        ui.max_rect(),
                        [1280.0, 720.0],
                        [1280.0, 720.0],
                        1.0,
                        egui::Vec2::ZERO,
                    );
                    paint(ui, &spec, view, &mut |_| None);
                });
            },
        );
        let mut texts = 0;
        let mut rects = 0;
        for clipped in &output.shapes {
            match &clipped.shape {
                egui::Shape::Text(_) => texts += 1,
                egui::Shape::Rect(_) => rects += 1,
                _ => {}
            }
        }
        assert!(texts >= 3, "标题与两个按钮都要出文字形状：{texts}");
        assert!(rects >= 3, "画布/面板/按钮都要出矩形：{rects}");
    }

    #[test]
    fn zoom_keeps_the_design_point_under_the_cursor() {
        let view = view();
        let cursor = egui::pos2(420.0, 260.0);
        let before = view.screen_to_design(cursor);
        let (zoom, pan) = view.zoomed_at(2.0, cursor);
        let zoomed = CanvasView::new(view.viewport, view.resolution, [1280.0, 720.0], zoom, pan);
        let after = zoomed.screen_to_design(cursor);
        assert!(
            (after[0] - before[0]).abs() < 0.5,
            "{before:?} -> {after:?}"
        );
        assert!(
            (after[1] - before[1]).abs() < 0.5,
            "{before:?} -> {after:?}"
        );
        assert!(zoomed.scale > view.scale);
    }

    #[test]
    fn zoom_is_clamped_to_the_supported_range() {
        let view = view();
        let (zoom, _) = view.zoomed_at(1000.0, view.viewport.center());
        assert!((zoom - MAX_ZOOM).abs() < 1e-6, "{zoom}");
        let (zoom, _) = view.zoomed_at(0.0001, view.viewport.center());
        assert!((zoom - MIN_ZOOM).abs() < 1e-6, "{zoom}");
    }

    #[test]
    fn hit_testing_prefers_the_deepest_node_and_respects_clip() {
        let rows = vec![
            LayoutRect {
                key: "root".to_owned(),
                rect: [0.0, 0.0, 200.0, 100.0],
                parent: None,
                clip: None,
            },
            LayoutRect {
                key: "child".to_owned(),
                rect: [10.0, 10.0, 40.0, 40.0],
                parent: Some("root".to_owned()),
                clip: None,
            },
            LayoutRect {
                key: "clipped".to_owned(),
                rect: [150.0, 0.0, 100.0, 100.0],
                parent: Some("root".to_owned()),
                clip: Some([0.0, 0.0, 160.0, 100.0]),
            },
        ];
        assert_eq!(
            pick_node(&rows, [20.0, 20.0]).map(|row| row.key.as_str()),
            Some("child")
        );
        assert_eq!(
            pick_node(&rows, [180.0, 20.0]).map(|row| row.key.as_str()),
            Some("root")
        );
        assert_eq!(pick_node(&rows, [500.0, 500.0]), None);
    }

    #[test]
    fn colors_parse_from_hex_tokens_and_channels() {
        let color = parse_color(&serde_json::json!("#d9873f")).unwrap();
        assert_eq!((color.r(), color.g(), color.b()), (217, 135, 63));
        let color = parse_color(&serde_json::json!("#211d1980")).unwrap();
        assert_eq!(color.a(), 0x80);
        let color = parse_color(&serde_json::json!([1.0, 0.5, 0.0])).unwrap();
        assert_eq!((color.r(), color.g(), color.b()), (255, 128, 0));
        assert!(parse_color(&serde_json::json!("not-a-color")).is_none());
    }

    #[test]
    fn style_values_follow_tokens() {
        let spec: UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"name": "style"},
            "tokens": {"color.ink": "#ecdfc6", "space": 8},
            "styles": {"title": {"base": {
                "color": "@color.ink", "font_size": 32, "text_align": "center",
                "radius": "@space"
            }}},
            "root": {"key": "root", "type": "text", "text": "Hi", "style": "title"}
        }))
        .unwrap();
        let style = Style::new(spec.styles.get("title"), &spec.tokens);
        assert_eq!(
            style.color("color").unwrap(),
            egui::Color32::from_rgb(236, 223, 198)
        );
        assert_eq!(style.number("font_size"), Some(32.0));
        assert_eq!(style.number("radius"), Some(8.0));
        assert_eq!(style.text("text_align"), Some("center"));
    }
}
