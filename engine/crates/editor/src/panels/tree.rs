//! 与模型无关的树控件原语：缩进、行标签、拖放落点与反馈。
//!
//! 场景层级（`hierarchy.rs`，实体）和 UI 工作室（`ui_studio.rs`，UI 节点）是两套模型，
//! 但树的外观与拖放语义是同一套：层距 14px 的缩进参考线、可选中行、落在行上三分带
//! → 插到前面 / 成为子节点 / 插到后面。这里只放两边共用的几何与绘制；过滤、重命名、
//! 预制体徽标这类模型专属的东西留在各自面板。

use eframe::egui;

/// 每层缩进步长（像素）。参考线与行首控件都按它对齐。
pub(crate) const INDENT_STEP: f32 = 14.0;
/// 缩进参考线的竖向长度：与 18px 行首控件 + 行上下留白对齐。
const INDENT_GUIDE_HEIGHT: f32 = 22.0;
/// 行首控件（折叠箭头 / 图标）的统一边长。
pub(crate) const LEADING_SIZE: f32 = 18.0;

/// 拖到一行上的落点。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DropSlot {
    /// 插到这一行前面（成为它的兄弟）。
    Before,
    /// 成为这一行的子节点。
    Child,
    /// 插到这一行后面（成为它的兄弟）。
    After,
}

impl DropSlot {
    /// 按指针在行内的相对高度取落点：上下三分之二是兄弟带，中间是子节点带。
    ///
    /// `accepts_children=false`（叶子控件）没有子节点带，中间按中线折回前/后，
    /// 否则会出现"画着成子节点、松手却落成兄弟"的错位提示。
    pub(crate) fn from_pointer(
        row_rect: egui::Rect,
        pointer_y: f32,
        accepts_children: bool,
    ) -> Self {
        let fraction = ((pointer_y - row_rect.top()) / row_rect.height().max(1.0)).clamp(0.0, 1.0);
        if !accepts_children {
            return if fraction < 0.5 {
                Self::Before
            } else {
                Self::After
            };
        }
        if fraction < 1.0 / 3.0 {
            Self::Before
        } else if fraction > 2.0 / 3.0 {
            Self::After
        } else {
            Self::Child
        }
    }

    fn is_after(self) -> bool {
        matches!(self, Self::After)
    }
}

/// 兄弟插入下标：`After` 落在目标之后；源与目标同父且源在目标前面时，
/// 落盘会先把源摘走，目标下标因此前移一位。
pub(crate) fn sibling_index(
    target_index: usize,
    slot: DropSlot,
    same_parent_source: Option<usize>,
) -> usize {
    let index = target_index + usize::from(slot.is_after());
    match same_parent_source {
        Some(source) if source < index => index - 1,
        _ => index,
    }
}

/// 画拖放落点反馈：兄弟带画横线，子节点带画整行描边。`valid=false` 画红。
pub(crate) fn paint_drop_target(
    painter: &egui::Painter,
    rect: egui::Rect,
    slot: DropSlot,
    valid: bool,
) {
    let stroke = egui::Stroke::new(
        2.0_f32,
        if valid {
            egui::Color32::from_rgb(110, 210, 130)
        } else {
            egui::Color32::from_rgb(220, 100, 100)
        },
    );
    match slot {
        DropSlot::Before => {
            painter.line_segment([rect.left_top(), rect.right_top()], stroke);
        }
        DropSlot::After => {
            painter.line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
        }
        DropSlot::Child => {
            painter.rect_stroke(rect, 0.0_f32, stroke, egui::StrokeKind::Inside);
        }
    }
}

/// 按深度画参考线并空出缩进。参考线画在当前光标处，因此要在行首调用。
pub(crate) fn indent(ui: &mut egui::Ui, depth: usize) {
    if depth == 0 {
        return;
    }
    let row_left = ui.cursor().left();
    let row_top = ui.cursor().top();
    let guide_color = ui
        .visuals()
        .widgets
        .noninteractive
        .fg_stroke
        .color
        .gamma_multiply(0.35);
    for level in 0..depth {
        let x = row_left + level as f32 * INDENT_STEP + 6.0;
        ui.painter().line_segment(
            [
                egui::pos2(x, row_top + 2.0),
                egui::pos2(x, row_top + INDENT_GUIDE_HEIGHT),
            ],
            egui::Stroke::new(1.0_f32, guide_color),
        );
    }
    ui.add_space(depth as f32 * INDENT_STEP);
}

/// 行标签按钮：选中态由调用方给，`sense` 决定这一行是只点还是可拖。
pub(crate) fn label_button(
    ui: &mut egui::Ui,
    selected: bool,
    enabled: bool,
    sense: egui::Sense,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::selectable(selected, text).sense(sense),
    )
}

/// 紧凑三角形展开/折叠按钮：用 painter 直接绘制，不依赖字体字形。
pub(crate) fn disclosure_button(
    ui: &mut egui::Ui,
    expanded: bool,
    tooltip: &str,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(LEADING_SIZE, LEADING_SIZE), egui::Sense::click());
    let color = if response.hovered() || response.is_pointer_button_down_on() {
        ui.visuals().widgets.hovered.fg_stroke.color
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    };
    let c = rect.center();
    let points: Vec<egui::Pos2> = if expanded {
        // 向下：底边在上，尖端朝下。
        vec![
            egui::pos2(c.x - 5.0, c.y - 3.0),
            egui::pos2(c.x + 5.0, c.y - 3.0),
            egui::pos2(c.x, c.y + 4.0),
        ]
    } else {
        // 向右：底边在左，尖端朝右。
        vec![
            egui::pos2(c.x - 3.0, c.y - 5.0),
            egui::pos2(c.x - 3.0, c.y + 5.0),
            egui::pos2(c.x + 4.0, c.y),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
    response.on_hover_text(tooltip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(top: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, top), egui::vec2(100.0, 20.0))
    }

    #[test]
    fn drop_slot_uses_thirds_when_children_are_allowed() {
        assert_eq!(
            DropSlot::from_pointer(row(100.0), 101.0, true),
            DropSlot::Before
        );
        assert_eq!(
            DropSlot::from_pointer(row(100.0), 110.0, true),
            DropSlot::Child
        );
        assert_eq!(
            DropSlot::from_pointer(row(100.0), 119.0, true),
            DropSlot::After
        );
    }

    #[test]
    fn leaf_rows_split_at_the_midline_instead_of_offering_child() {
        assert_eq!(
            DropSlot::from_pointer(row(100.0), 109.0, false),
            DropSlot::Before
        );
        assert_eq!(
            DropSlot::from_pointer(row(100.0), 111.0, false),
            DropSlot::After
        );
    }

    #[test]
    fn drop_slot_clamps_outside_the_row() {
        assert_eq!(
            DropSlot::from_pointer(row(100.0), 50.0, true),
            DropSlot::Before
        );
        assert_eq!(
            DropSlot::from_pointer(row(100.0), 150.0, true),
            DropSlot::After
        );
    }

    #[test]
    fn sibling_index_shifts_when_the_source_is_removed_first() {
        assert_eq!(sibling_index(2, DropSlot::Before, None), 2);
        assert_eq!(sibling_index(2, DropSlot::After, None), 3);
        assert_eq!(
            sibling_index(2, DropSlot::After, Some(0)),
            2,
            "同父级源在前：摘下源后目标前移一位"
        );
        assert_eq!(
            sibling_index(2, DropSlot::Before, Some(5)),
            2,
            "源在目标后面不影响下标"
        );
    }
}
