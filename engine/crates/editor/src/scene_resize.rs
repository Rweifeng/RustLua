//! Phase 5a：缩放手柄 + 对齐辅助线（Widget 模式专用）。
//!
//! 缩放手柄：选中 UI 实体的包围框上 8 个可拖拽区域（4 角 + 4 边中点），
//! 拖动时修改 `UiLayout.offsets` 对应边，实时预览并走已有的 undo/redo 管线。
//!
//! 对齐辅助线：拖动 / 缩放过程中检测选中实体与其他实体的边缘 / 中心对齐，
//! 返回可绘制的辅助线集合（面板坐标）。

use eframe::egui;
use engine::core::scene::SceneEntityId;

/// 8 个缩放手柄：4 角 + 4 边中点。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    MiddleRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// 手柄在面板坐标里的像素半径（egui 点）。
pub const HANDLE_HALF_SIZE: f32 = 5.0;

/// 手柄颜色（Unity 风格蓝色填充 + 白色描边）。
const HANDLE_FILL: egui::Color32 = egui::Color32::from_rgb(70, 140, 220);
const HANDLE_STROKE: egui::Color32 = egui::Color32::WHITE;

/// 辅助线颜色（品红色，与 Unity / Figma 对齐线一致）。
const GUIDE_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 0, 128);

/// 对齐检测阈值（面板像素）。
const SNAP_THRESHOLD: f32 = 6.0;

impl ResizeHandle {
    /// 所有 8 个手柄。
    pub const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::TopCenter,
        Self::TopRight,
        Self::MiddleLeft,
        Self::MiddleRight,
        Self::BottomLeft,
        Self::BottomCenter,
        Self::BottomRight,
    ];

    /// 手柄在包围框矩形上的锚点（面板坐标）。
    pub fn anchor(self, rect: egui::Rect) -> egui::Pos2 {
        match self {
            Self::TopLeft => rect.left_top(),
            Self::TopCenter => egui::pos2(rect.center().x, rect.top()),
            Self::TopRight => rect.right_top(),
            Self::MiddleLeft => egui::pos2(rect.left(), rect.center().y),
            Self::MiddleRight => egui::pos2(rect.right(), rect.center().y),
            Self::BottomLeft => rect.left_bottom(),
            Self::BottomCenter => egui::pos2(rect.center().x, rect.bottom()),
            Self::BottomRight => rect.right_bottom(),
        }
    }

    /// 手柄对应的鼠标光标形状。
    #[allow(dead_code)]
    pub fn cursor(self) -> egui::CursorIcon {
        match self {
            Self::TopLeft | Self::BottomRight => egui::CursorIcon::ResizeNwSe,
            Self::TopRight | Self::BottomLeft => egui::CursorIcon::ResizeNeSw,
            Self::TopCenter | Self::BottomCenter => egui::CursorIcon::ResizeVertical,
            Self::MiddleLeft | Self::MiddleRight => egui::CursorIcon::ResizeHorizontal,
        }
    }

    /// 手柄是否影响左 / 上 / 右 / 下边（决定哪些 offset 要改）。
    pub fn affects(self) -> [bool; 4] {
        match self {
            Self::TopLeft => [true, true, false, false],
            Self::TopCenter => [false, true, false, false],
            Self::TopRight => [false, true, true, false],
            Self::MiddleLeft => [true, false, false, false],
            Self::MiddleRight => [false, false, true, false],
            Self::BottomLeft => [true, false, false, true],
            Self::BottomCenter => [false, false, false, true],
            Self::BottomRight => [false, false, true, true],
        }
    }
}

/// 在包围框矩形上命中测试：指针落在哪个手柄上？
///
/// 从最"具体"的手柄开始检测（角优先于边中点），避免角与边重叠时误判。
pub fn hit_test_handle(
    rect: egui::Rect,
    pointer: egui::Pos2,
) -> Option<ResizeHandle> {
    let half = HANDLE_HALF_SIZE + 2.0;
    for handle in ResizeHandle::ALL {
        let anchor = handle.anchor(rect);
        if (pointer.x - anchor.x).abs() <= half && (pointer.y - anchor.y).abs() <= half {
            return Some(handle);
        }
    }
    None
}

/// 绘制 8 个缩放手柄。
pub fn draw_resize_handles(painter: &egui::Painter, rect: egui::Rect) {
    let stroke = egui::Stroke::new(1.0_f32, HANDLE_STROKE);
    for handle in ResizeHandle::ALL {
        let anchor = handle.anchor(rect);
        let size = HANDLE_HALF_SIZE;
        let handle_rect = egui::Rect::from_center_size(anchor, egui::vec2(size * 2.0, size * 2.0));
        painter.rect_filled(handle_rect, 1.0, HANDLE_FILL);
        painter.rect_stroke(handle_rect, 1.0, stroke, egui::StrokeKind::Outside);
    }
}

/// 把面板坐标的拖动增量换算成设计像素增量。
///
/// `panel_delta` 是面板坐标（egui 点）的位移，`zoom / ppp` 是面板点到设计像素的比例。
pub fn panel_delta_to_design_delta(panel_delta: egui::Vec2, zoom: f32, ppp: f32) -> egui::Vec2 {
    let ratio = (zoom / ppp.max(1.0e-3)).max(1.0e-3);
    egui::vec2(panel_delta.x / ratio, panel_delta.y / ratio)
}

/// 按手柄与拖动增量计算新的 `UiLayout.offsets`。
///
/// `initial_offsets` 是拖动开始时的 `[left, top, right, bottom]`。
/// `design_delta` 是面板增量换算到设计像素后的位移（右正、下正）。
///
/// 返回修改后的 offsets。
pub fn compute_resize_offsets(
    initial_offsets: [f32; 4],
    design_delta: egui::Vec2,
    handle: ResizeHandle,
) -> [f32; 4] {
    let affects = handle.affects();
    let mut offsets = initial_offsets;
    if affects[0] {
        // 左边：指针右移 ⇒ left offset 增大（元素左边界右移）
        offsets[0] = initial_offsets[0] + design_delta.x;
    }
    if affects[1] {
        // 上边：指针下移 ⇒ top offset 增大（元素上边界下移）
        offsets[1] = initial_offsets[1] + design_delta.y;
    }
    if affects[2] {
        // 右边：指针右移 ⇒ right offset 增大（元素右边界右移）
        offsets[2] = initial_offsets[2] + design_delta.x;
    }
    if affects[3] {
        // 下边：指针下移 ⇒ bottom offset 增大（元素下边界下移）
        offsets[3] = initial_offsets[3] + design_delta.y;
    }
    offsets
}

// ---------------------------------------------------------------------------
// 对齐辅助线
// ---------------------------------------------------------------------------

/// 一条对齐辅助线：位置（面板坐标）+ 方向 + 绘制范围。
#[derive(Clone, Copy, Debug)]
pub struct SnapGuide {
    /// 辅助线位置（水平线 = y 坐标，垂直线 = x 坐标）。
    pub position: f32,
    /// 水平 or 垂直。
    pub axis: SnapAxis,
    /// 绘制范围起点（垂直线 = y_min，水平线 = x_min）。
    pub span_start: f32,
    /// 绘制范围终点。
    pub span_end: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapAxis {
    Horizontal,
    Vertical,
}

/// 一个实体的 5 条对齐参考线（左 / 中 / 右 / 上 / 中 / 下）。
struct EntityGuides {
    id: SceneEntityId,
    left: f32,
    center_x: f32,
    right: f32,
    top: f32,
    center_y: f32,
    bottom: f32,
}

impl EntityGuides {
    fn from_rect(id: SceneEntityId, rect: egui::Rect) -> Self {
        Self {
            id,
            left: rect.left(),
            center_x: rect.center().x,
            right: rect.right(),
            top: rect.top(),
            center_y: rect.center().y,
            bottom: rect.bottom(),
        }
    }

    fn vertical_values(&self) -> [f32; 3] {
        [self.left, self.center_x, self.right]
    }

    fn horizontal_values(&self) -> [f32; 3] {
        [self.top, self.center_y, self.bottom]
    }
}

/// 检测拖动实体与其他实体的对齐关系，返回辅助线集合。
///
/// `dragged_rect` 是正在拖动 / 缩放的实体的当前面板包围框。
/// `others` 是其他实体的面板包围框。
pub fn compute_snap_guides(
    dragged_rect: egui::Rect,
    others: &[(SceneEntityId, egui::Rect)],
) -> Vec<SnapGuide> {
    let dragged = EntityGuides::from_rect(SceneEntityId(0), dragged_rect);
    let mut guides = Vec::new();

    // 收集其他实体的参考线
    let other_guides: Vec<EntityGuides> = others
        .iter()
        .map(|(id, rect)| EntityGuides::from_rect(*id, *rect))
        .collect();

    // 垂直辅助线（x 坐标对齐：左 / 中 / 右）
    for dragged_x in dragged.vertical_values() {
        for other in &other_guides {
            for other_x in other.vertical_values() {
                if (dragged_x - other_x).abs() <= SNAP_THRESHOLD {
                    let y_min = dragged_rect.top().min(
                        others
                            .iter()
                            .find(|(id, _)| *id == other.id)
                            .map_or(dragged_rect.bottom(), |(_, r)| r.top()),
                    );
                    let y_max = dragged_rect.bottom().max(
                        others
                            .iter()
                            .find(|(id, _)| *id == other.id)
                            .map_or(dragged_rect.top(), |(_, r)| r.bottom()),
                    );
                    guides.push(SnapGuide {
                        position: other_x,
                        axis: SnapAxis::Vertical,
                        span_start: y_min,
                        span_end: y_max,
                    });
                }
            }
        }
    }

    // 水平辅助线（y 坐标对齐：上 / 中 / 下）
    for dragged_y in dragged.horizontal_values() {
        for other in &other_guides {
            for other_y in other.horizontal_values() {
                if (dragged_y - other_y).abs() <= SNAP_THRESHOLD {
                    let x_min = dragged_rect.left().min(
                        others
                            .iter()
                            .find(|(id, _)| *id == other.id)
                            .map_or(dragged_rect.right(), |(_, r)| r.left()),
                    );
                    let x_max = dragged_rect.right().max(
                        others
                            .iter()
                            .find(|(id, _)| *id == other.id)
                            .map_or(dragged_rect.left(), |(_, r)| r.right()),
                    );
                    guides.push(SnapGuide {
                        position: other_y,
                        axis: SnapAxis::Horizontal,
                        span_start: x_min,
                        span_end: x_max,
                    });
                }
            }
        }
    }

    guides
}

/// 绘制对齐辅助线。
pub fn draw_snap_guides(painter: &egui::Painter, guides: &[SnapGuide]) {
    let stroke = egui::Stroke::new(1.0_f32, GUIDE_COLOR);
    for guide in guides {
        match guide.axis {
            SnapAxis::Vertical => {
                painter.line_segment(
                    [
                        egui::pos2(guide.position, guide.span_start),
                        egui::pos2(guide.position, guide.span_end),
                    ],
                    stroke,
                );
            }
            SnapAxis::Horizontal => {
                painter.line_segment(
                    [
                        egui::pos2(guide.span_start, guide.position),
                        egui::pos2(guide.span_end, guide.position),
                    ],
                    stroke,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 5c：Anchor 可视化编辑
// ---------------------------------------------------------------------------

/// 4 个锚点手柄：锚点矩形的 4 个角。
/// 拖动锚点手柄修改 `UiLayout.anchors` 的对应分量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorHandle {
    /// 左上角：影响 anchor_left (0) 和 anchor_top (1)。
    TopLeft,
    /// 右上角：影响 anchor_right (2) 和 anchor_top (1)。
    TopRight,
    /// 左下角：影响 anchor_left (0) 和 anchor_bottom (3)。
    BottomLeft,
    /// 右下角：影响 anchor_right (2) 和 anchor_bottom (3)。
    BottomRight,
}

/// 锚点手柄颜色（Godot 风格绿色）。
const ANCHOR_FILL: egui::Color32 = egui::Color32::from_rgb(80, 200, 120);
const ANCHOR_STROKE: egui::Color32 = egui::Color32::WHITE;

impl AnchorHandle {
    /// 所有 4 个锚点手柄。
    pub const ALL: [Self; 4] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    /// 手柄在锚点矩形上的位置（面板坐标）。
    pub fn anchor_pos(self, anchor_rect: egui::Rect) -> egui::Pos2 {
        match self {
            Self::TopLeft => anchor_rect.left_top(),
            Self::TopRight => anchor_rect.right_top(),
            Self::BottomLeft => anchor_rect.left_bottom(),
            Self::BottomRight => anchor_rect.right_bottom(),
        }
    }

    /// 手柄影响的 anchor 分量索引：[left, top, right, bottom]。
    /// 返回 (x_index, y_index)，None 表示不影响该轴。
    pub fn affects(self) -> (Option<usize>, Option<usize>) {
        match self {
            Self::TopLeft => (Some(0), Some(1)),
            Self::TopRight => (Some(2), Some(1)),
            Self::BottomLeft => (Some(0), Some(3)),
            Self::BottomRight => (Some(2), Some(3)),
        }
    }
}

/// 在锚点矩形上命中测试：指针落在哪个锚点手柄上？
pub fn hit_test_anchor_handle(
    anchor_rect: egui::Rect,
    pointer: egui::Pos2,
) -> Option<AnchorHandle> {
    let half = HANDLE_HALF_SIZE + 3.0;
    for handle in AnchorHandle::ALL {
        let pos = handle.anchor_pos(anchor_rect);
        if (pointer.x - pos.x).abs() <= half && (pointer.y - pos.y).abs() <= half {
            return Some(handle);
        }
    }
    None
}

/// 绘制锚点可视化：锚点矩形 + 4 个角手柄。
///
/// `anchor_rect` 是锚点在设计分辨率内的矩形（面板坐标），
/// 由 `anchors * parent_size` 换算而来。
pub fn draw_anchor_handles(painter: &egui::Painter, anchor_rect: egui::Rect) {
    // 填充锚点矩形（半透明）
    painter.rect_filled(
        anchor_rect,
        0.0,
        egui::Color32::from_rgba_unmultiplied(80, 200, 120, 60),
    );
    // 描边
    let stroke = egui::Stroke::new(1.0_f32, ANCHOR_STROKE);
    painter.rect_stroke(anchor_rect, 0.0, stroke, egui::StrokeKind::Outside);
    // 4 个角手柄
    let handle_size = HANDLE_HALF_SIZE * 1.5;
    for handle in AnchorHandle::ALL {
        let pos = handle.anchor_pos(anchor_rect);
        let handle_rect =
            egui::Rect::from_center_size(pos, egui::vec2(handle_size * 2.0, handle_size * 2.0));
        painter.rect_filled(handle_rect, 2.0, ANCHOR_FILL);
        painter.rect_stroke(handle_rect, 2.0, stroke, egui::StrokeKind::Outside);
    }
}

/// 按手柄和面板增量计算新的 anchor 值。
///
/// `initial_anchors` 是拖动开始时的 `[left, top, right, bottom]`（归一化 0..1）。
/// `panel_delta` 是面板坐标增量。
/// `parent_panel_size` 是父容器（设计画布）在面板坐标中的尺寸。
///
/// 返回修改后的 anchors，各分量 clamp 到 [0, 1]。
pub fn compute_anchor_values(
    initial_anchors: [f32; 4],
    panel_delta: egui::Vec2,
    parent_panel_size: egui::Vec2,
    handle: AnchorHandle,
) -> [f32; 4] {
    let mut anchors = initial_anchors;
    let (x_idx, y_idx) = handle.affects();
    if let Some(xi) = x_idx {
        let delta_norm = panel_delta.x / parent_panel_size.x.max(1.0);
        anchors[xi] = (initial_anchors[xi] + delta_norm).clamp(0.0, 1.0);
    }
    if let Some(yi) = y_idx {
        let delta_norm = panel_delta.y / parent_panel_size.y.max(1.0);
        anchors[yi] = (initial_anchors[yi] + delta_norm).clamp(0.0, 1.0);
    }
    anchors
}

// ─── Phase 5d: Container padding 可视化编辑 ────────────────────────────

/// Container 内边距手柄（4 边）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaddingHandle {
    Left,
    Top,
    Right,
    Bottom,
}

impl PaddingHandle {
    /// 所有 4 个 padding 手柄。
    pub const ALL: [Self; 4] = [Self::Left, Self::Top, Self::Right, Self::Bottom];

    /// 手柄影响的 padding 索引：[left, top, right, bottom]。
    pub fn padding_index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Top => 1,
            Self::Right => 2,
            Self::Bottom => 3,
        }
    }

    /// 手柄在 padding 矩形上的边中点（面板坐标）。
    pub fn edge_center(self, padding_rect: egui::Rect) -> egui::Pos2 {
        match self {
            Self::Left => padding_rect.left_center(),
            Self::Top => padding_rect.center_top(),
            Self::Right => padding_rect.right_center(),
            Self::Bottom => padding_rect.center_bottom(),
        }
    }
}

/// 绘制 Container padding 可视化：外框（实体边界）+ 内框（padding 后内容区）+ 手柄。
///
/// `outer_rect` 是实体的设计像素边界（面板坐标）。
/// `padding` 是 `[left, top, right, bottom]` 设计像素值。
/// `design_to_panel_scale` 是设计像素 → 面板像素的缩放因子。
pub fn draw_padding_overlay(
    painter: &egui::Painter,
    outer_rect: egui::Rect,
    padding: [f32; 4],
    design_to_panel_scale: f32,
) {
    let [pl, pt, pr, pb] = padding;
    let inner_rect = egui::Rect::from_min_max(
        egui::pos2(
            outer_rect.min.x + pl * design_to_panel_scale,
            outer_rect.min.y + pt * design_to_panel_scale,
        ),
        egui::pos2(
            outer_rect.max.x - pr * design_to_panel_scale,
            outer_rect.max.y - pb * design_to_panel_scale,
        ),
    );
    if inner_rect.is_negative() {
        return;
    }
    // 填充 padding 区域（半透明橙色，类似浏览器 devtools）
    let padding_color = egui::Color32::from_rgba_unmultiplied(255, 160, 60, 50);
    // 上边
    painter.rect_filled(
        egui::Rect::from_min_max(outer_rect.left_top(), inner_rect.right_top()),
        0.0,
        padding_color,
    );
    // 下边
    painter.rect_filled(
        egui::Rect::from_min_max(inner_rect.left_bottom(), outer_rect.right_bottom()),
        0.0,
        padding_color,
    );
    // 左边
    painter.rect_filled(
        egui::Rect::from_min_max(outer_rect.left_top(), inner_rect.left_bottom()),
        0.0,
        padding_color,
    );
    // 右边
    painter.rect_filled(
        egui::Rect::from_min_max(inner_rect.right_top(), outer_rect.right_bottom()),
        0.0,
        padding_color,
    );
    // 内框描边
    let stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 160, 60));
    painter.rect_stroke(inner_rect, 0.0, stroke, egui::StrokeKind::Outside);
    // 4 个边手柄
    let handle_half = HANDLE_HALF_SIZE * 0.8;
    for handle in PaddingHandle::ALL {
        let center = handle.edge_center(inner_rect);
        let handle_rect = egui::Rect::from_center_size(
            center,
            egui::vec2(handle_half * 2.0, handle_half * 2.0),
        );
        painter.rect_filled(handle_rect, 1.0, egui::Color32::from_rgb(255, 160, 60));
        painter.rect_stroke(handle_rect, 1.0, stroke, egui::StrokeKind::Outside);
    }
}

/// 命中测试 padding 手柄。返回命中的手柄或 None。
///
/// `outer_rect` 是实体边界，`padding` 是设计像素内边距，
/// `design_to_panel_scale` 是缩放因子，`pointer` 是面板坐标指针位置。
pub fn hit_test_padding_handle(
    outer_rect: egui::Rect,
    padding: [f32; 4],
    design_to_panel_scale: f32,
    pointer: egui::Pos2,
) -> Option<PaddingHandle> {
    let [pl, pt, pr, pb] = padding;
    let inner_rect = egui::Rect::from_min_max(
        egui::pos2(
            outer_rect.min.x + pl * design_to_panel_scale,
            outer_rect.min.y + pt * design_to_panel_scale,
        ),
        egui::pos2(
            outer_rect.max.x - pr * design_to_panel_scale,
            outer_rect.max.y - pb * design_to_panel_scale,
        ),
    );
    if inner_rect.is_negative() {
        return None;
    }
    let hit_half = HANDLE_HALF_SIZE * 1.5;
    for handle in PaddingHandle::ALL {
        let center = handle.edge_center(inner_rect);
        if (pointer.x - center.x).abs() <= hit_half && (pointer.y - center.y).abs() <= hit_half {
            return Some(handle);
        }
    }
    None
}

/// 按手柄和面板增量计算新的 padding 值。
///
/// `initial_padding` 是拖动开始时的 `[left, top, right, bottom]`（设计像素）。
/// `panel_delta` 是面板坐标增量。
/// `design_to_panel_scale` 是缩放因子。
///
/// 返回修改后的 padding，各分量 clamp 到 [0, 1000]。
pub fn compute_padding_values(
    initial_padding: [f32; 4],
    panel_delta: egui::Vec2,
    design_to_panel_scale: f32,
    handle: PaddingHandle,
) -> [f32; 4] {
    let mut padding = initial_padding;
    let idx = handle.padding_index();
    let scale = design_to_panel_scale.max(0.001);
    // 设计像素增量 = 面板增量 / 缩放
    let delta_design = match handle {
        PaddingHandle::Left => panel_delta.x / scale,
        PaddingHandle::Top => panel_delta.y / scale,
        PaddingHandle::Right => -panel_delta.x / scale,
        PaddingHandle::Bottom => -panel_delta.y / scale,
    };
    padding[idx] = (initial_padding[idx] + delta_design).clamp(0.0, 1000.0);
    padding
}
