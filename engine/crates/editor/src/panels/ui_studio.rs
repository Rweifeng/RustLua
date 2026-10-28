//! M76-04：UI Studio 面板（egui 薄壳）。
//!
//! 状态与动作全在 `editor::ui_studio::UiStudio`；这里只负责绘制与转调。
//! 写入仍走 Spec 宿主的提案 → 接受/撤销/拒绝，面板不直接改 `.ui.json`。

use crate::EditorApp;
use editor::ui_studio::{
    AlignMode, AssetCandidate, DistributeAxis, InspectorEdit, InspectorSection, PreviewResolution,
    SnapGuides, UiStudio,
};
use eframe::egui;
use rustlua_ui_spec::{AnchorPreset, ResizeHandle, SliceRegion, UiNodeKind};
use std::collections::{BTreeMap, BTreeSet};

/// 拖动吸附阈值（设计像素）：比这个更近才吸到参考线。
const SNAP_THRESHOLD: f32 = 6.0;

/// 画布拖动：移动整块、拖某个手柄，或在空白处平移画布。
/// `total` 是跨帧累计的屏幕位移；`start` 是按下时的设计矩形（幽灵框与提交都用它）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CanvasDragMode {
    Move,
    Resize(ResizeHandle),
    Pan,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CanvasDrag {
    pub(crate) mode: CanvasDragMode,
    pub(crate) total: egui::Vec2,
    pub(crate) start: [f32; 4],
    pub(crate) key: String,
    /// 本帧吸上的参考线（画布叠层用来画十字线）。
    pub(crate) guides: SnapGuides,
}

/// 面板级缓存：画布的缩放/平移与图片纹理。文档或 revision 变了就清纹理。
pub(crate) struct UiStudioPanel {
    pub(crate) studio: Option<UiStudio>,
    pub(crate) document_input: String,
    pub(crate) status: String,
    /// 画布缩放：1.0 = 适应窗口。
    pub(crate) canvas_zoom: f32,
    /// 画布屏幕位移（与缩放解耦，缩放锚点由 [`crate::ui_canvas::CanvasView`] 算）。
    pub(crate) canvas_pan: egui::Vec2,
    /// 画布图片纹理缓存：工程内相对路径 → 纹理。
    pub(crate) canvas_textures: BTreeMap<String, egui::TextureHandle>,
    /// 画布真正绘制过的帧数（测试用来确认面板走到了画布路径）。
    pub(crate) canvas_paints: u64,
    /// 每个补丁提案的逐 op 勾选状态（true = 会被「接受选中」落盘）。
    pub(crate) proposal_selection: BTreeMap<String, Vec<bool>>,
    /// 令牌编辑：名字与值都先攒在输入框，提交时生成提案。
    pub(crate) token_name: String,
    pub(crate) token_value: String,
    /// 检查器字段输入框，键是 `选中节点/字段`；只有点「应用」才生成提案。
    pub(crate) field_inputs: BTreeMap<String, String>,
    /// 层级拖动的源节点 key。
    pub(crate) hierarchy_drag: Option<String>,
    /// 画布拖动状态（松手时才提交提案）。
    pub(crate) canvas_drag: Option<CanvasDrag>,
    /// 包裹容器的新 key 输入框。
    pub(crate) wrap_key: String,
    /// 「添加子节点」下拉框选中的控件类型（下标进 [`addable_kinds`]）。
    pub(crate) add_child_kind: usize,
    /// 候选图选区（详情面板按它显示来源；点击格子切换）。
    pub(crate) asset_selected: Option<String>,
    /// 审阅时填写的许可，切换候选时按已有边车回填。
    pub(crate) asset_license: String,
    /// 候选图缩略图缓存：键是资产 key，值随文件缺失/出现重试。
    pub(crate) asset_textures: BTreeMap<String, egui::TextureHandle>,
    /// 切图表单：源图、输出目录、逐行 `name,x,y,w,h`。
    pub(crate) slice_source: String,
    pub(crate) slice_output: String,
    pub(crate) slice_regions: String,
    /// 资产列表/审查缓存：审查要做图集装箱与全工程扫描，不能每帧跑。
    pub(crate) asset_cache: Option<AssetCache>,
}

impl Default for UiStudioPanel {
    fn default() -> Self {
        Self {
            studio: None,
            document_input: String::new(),
            status: String::new(),
            canvas_zoom: 1.0,
            canvas_pan: egui::Vec2::ZERO,
            canvas_textures: BTreeMap::new(),
            canvas_paints: 0,
            proposal_selection: BTreeMap::new(),
            token_name: String::new(),
            token_value: String::new(),
            field_inputs: BTreeMap::new(),
            hierarchy_drag: None,
            canvas_drag: None,
            wrap_key: String::new(),
            add_child_kind: 0,
            asset_selected: None,
            asset_license: String::new(),
            asset_textures: BTreeMap::new(),
            slice_source: String::new(),
            slice_output: String::new(),
            slice_regions: String::new(),
            asset_cache: None,
        }
    }
}

/// 一次资产审查的读数；`revision` 变了（文档被接受/撤销）才失效。
#[derive(Default)]
pub(crate) struct AssetCache {
    revision: u64,
    candidates: Vec<AssetCandidate>,
    errors: usize,
    warnings: usize,
    total_bytes: u64,
    review_error: Option<String>,
}

impl AssetCache {
    fn load(studio: &UiStudio) -> Self {
        let revision = studio.revision();
        let candidates = studio.asset_candidates();
        match studio.asset_review() {
            Ok(report) => Self {
                revision,
                candidates,
                errors: report
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.severity == rustlua_ui_spec::Severity::Error)
                    .count(),
                warnings: report.warnings(),
                total_bytes: report.total_bytes,
                review_error: None,
            },
            Err(error) => Self {
                revision,
                candidates,
                review_error: Some(error.to_string()),
                ..Self::default()
            },
        }
    }
}

/// 提案箱里的 op 摘要：只取变体名，避免把整份补丁打印成多行 Debug。
/// 提案类型的中文名（面板不直接上屏 Rust 枚举名）。
fn proposal_kind_label(kind: rustlua_ui_spec::ProposalKind) -> &'static str {
    use rustlua_ui_spec::ProposalKind;
    match kind {
        ProposalKind::Spec => "整份规格",
        ProposalKind::Patch => "补丁",
        ProposalKind::AssetPlan => "资产生成计划",
    }
}

fn patch_op_label(op: &rustlua_ui_spec::UiPatchOp) -> String {
    let debug = format!("{op:?}");
    debug
        .split(['(', ' ', '{'])
        .next()
        .unwrap_or("op")
        .to_owned()
}

fn draw_toolbar(ui: &mut egui::Ui, studio: &mut UiStudio) {
    let mut resolution = studio.resolution();
    let mut safe_area = studio.safe_area();
    ui.horizontal(|ui| {
        ui.label("预览");
        egui::ComboBox::from_id_salt("ui_studio_resolution")
            .selected_text(resolution.label())
            .show_ui(ui, |ui| {
                for candidate in PreviewResolution::ALL {
                    ui.selectable_value(&mut resolution, candidate, candidate.label());
                }
            });
        if ui.checkbox(&mut safe_area, "安全区").changed() {
            studio.set_safe_area(safe_area);
        }
    });
    if resolution != studio.resolution() {
        studio.set_resolution(resolution);
    }
}

/// 画布叠层：安全区框、选中节点轮廓、锚点十字、九宫格线、缩放手柄、间距标尺。
/// `ghost` 是拖动中的实时矩形（设计坐标）：叠层跟着指针走，松手才提交提案。
fn draw_overlays(
    ui: &egui::Ui,
    studio: &UiStudio,
    view: crate::ui_canvas::CanvasView,
    ghost: Option<([f32; 4], &str)>,
    move_delta: Option<[f32; 2]>,
    guides: Option<SnapGuides>,
) {
    let painter = ui.painter_at(view.viewport);
    let to_screen = |design| view.design_to_screen(design);
    if studio.safe_area() {
        painter.rect_stroke(
            to_screen(studio.safe_area_rect()),
            0.0,
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(232, 172, 72)),
            egui::StrokeKind::Inside,
        );
    }
    // 顶部/左侧设计刻度：每 100 设计像素一格，标数字的是 200 的倍数。
    let reference = studio
        .spec()
        .map_or([1280.0, 720.0], |spec| spec.document.canvas.reference);
    let tick = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(110));
    let mut x = 100.0;
    while x < reference[0] {
        let at = to_screen([x, 0.0, 0.0, 0.0]).min;
        painter.line_segment([at, at + egui::vec2(0.0, 4.0)], tick);
        if (x as i32) % 200 == 0 {
            painter.text(
                at + egui::vec2(1.0, 5.0),
                egui::Align2::LEFT_TOP,
                format!("{x:.0}"),
                egui::FontId::monospace(8.0),
                egui::Color32::from_gray(140),
            );
        }
        x += 100.0;
    }
    let mut y = 100.0;
    while y < reference[1] {
        let at = to_screen([0.0, y, 0.0, 0.0]).min;
        painter.line_segment([at, at + egui::vec2(4.0, 0.0)], tick);
        y += 100.0;
    }
    let selection = studio.selection().map(str::to_owned);
    let rows = studio.layout_rects();
    let (key, design_rect) = match ghost {
        Some((rect, key)) => (key.to_owned(), rect),
        None => {
            let Some(selected) = rows.iter().find(|row| Some(&row.key) == selection.as_ref())
            else {
                return;
            };
            (selected.key.clone(), selected.rect)
        }
    };
    let rect = to_screen(design_rect);
    let accent = egui::Color32::from_rgb(96, 200, 255);
    // 多选：非主选中的框画细轮廓；整组拖动时跟着同样的位移预览。
    let secondary = egui::Color32::from_rgb(64, 148, 196);
    for (frame_key, frame) in studio.selection_frames() {
        if frame_key == key {
            continue;
        }
        let preview = match move_delta {
            Some(delta) => [frame[0] + delta[0], frame[1] + delta[1], frame[2], frame[3]],
            None => frame,
        };
        let screen = to_screen(preview);
        painter.rect_stroke(
            screen,
            0.0,
            egui::Stroke::new(1.0_f32, secondary),
            egui::StrokeKind::Outside,
        );
        painter.text(
            screen.left_top() + egui::vec2(2.0, -2.0),
            egui::Align2::LEFT_BOTTOM,
            frame_key,
            egui::FontId::monospace(9.0),
            secondary,
        );
    }
    // 吸附参考线：整条画布高度/宽度，拖动时给出“对齐到哪”的确定读数。
    if let Some(guides) = guides {
        let stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 96, 208));
        if let Some(x) = guides.x {
            let x = to_screen([x, 0.0, 0.0, 0.0]).min.x;
            painter.line_segment(
                [
                    egui::pos2(x, view.viewport.top()),
                    egui::pos2(x, view.viewport.bottom()),
                ],
                stroke,
            );
        }
        if let Some(y) = guides.y {
            let y = to_screen([0.0, y, 0.0, 0.0]).min.y;
            painter.line_segment(
                [
                    egui::pos2(view.viewport.left(), y),
                    egui::pos2(view.viewport.right(), y),
                ],
                stroke,
            );
        }
    }
    // 间距标尺：选中框到父框四边的设计像素距离（拖动中不画，父级距离已过期）。
    if ghost.is_none()
        && let Some(parent) = rows
            .iter()
            .find(|row| row.key == key)
            .and_then(|selected| selected.parent.as_ref())
            .and_then(|key| rows.iter().find(|row| &row.key == key).map(|row| row.rect))
    {
        let gaps = [
            (
                [
                    design_rect[0] - parent[0],
                    design_rect[1],
                    0.0,
                    design_rect[3],
                ],
                design_rect[0] - parent[0],
                egui::Align2::RIGHT_CENTER,
            ),
            (
                [
                    design_rect[0] + design_rect[2],
                    design_rect[1],
                    0.0,
                    design_rect[3],
                ],
                parent[0] + parent[2] - (design_rect[0] + design_rect[2]),
                egui::Align2::LEFT_CENTER,
            ),
            (
                [design_rect[0], design_rect[1], design_rect[2], 0.0],
                design_rect[1] - parent[1],
                egui::Align2::CENTER_BOTTOM,
            ),
            (
                [
                    design_rect[0],
                    design_rect[1] + design_rect[3],
                    design_rect[2],
                    0.0,
                ],
                parent[1] + parent[3] - (design_rect[1] + design_rect[3]),
                egui::Align2::CENTER_TOP,
            ),
        ];
        for (design, gap, align) in gaps {
            let at = to_screen(design).min;
            painter.text(
                at,
                align,
                format!("{gap:.0}"),
                egui::FontId::monospace(8.0),
                egui::Color32::from_rgb(150, 210, 140),
            );
        }
    }
    // 缩放手柄：8 个小方块，命中判定与这里用同一套 anchor。
    for handle in ResizeHandle::ALL {
        let [ax, ay] = handle.anchor();
        let center = egui::pos2(
            rect.left() + rect.width() * ax,
            rect.top() + rect.height() * ay,
        );
        painter.rect_filled(
            egui::Rect::from_center_size(center, egui::vec2(6.0, 6.0)),
            1.0,
            accent,
        );
    }
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.5_f32, accent),
        egui::StrokeKind::Outside,
    );
    // 锚点参考线：选中矩形中心的十字。
    let center = rect.center();
    painter.line_segment(
        [center - egui::vec2(6.0, 0.0), center + egui::vec2(6.0, 0.0)],
        egui::Stroke::new(1.0_f32, accent),
    );
    painter.line_segment(
        [center - egui::vec2(0.0, 6.0), center + egui::vec2(0.0, 6.0)],
        egui::Stroke::new(1.0_f32, accent),
    );
    // 九宫格：四边内缩线（设计像素 × 画布总缩放）。
    if let Some(insets) = studio.selected_nine_slice() {
        let px = view.design_scale();
        let (left, right) = (rect.left() + insets[0] * px, rect.right() - insets[2] * px);
        let (top, bottom) = (rect.top() + insets[1] * px, rect.bottom() - insets[3] * px);
        let stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 120, 120));
        painter.line_segment(
            [
                egui::pos2(left, rect.top()),
                egui::pos2(left, rect.bottom()),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(right, rect.top()),
                egui::pos2(right, rect.bottom()),
            ],
            stroke,
        );
        painter.line_segment(
            [egui::pos2(rect.left(), top), egui::pos2(rect.right(), top)],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(rect.left(), bottom),
                egui::pos2(rect.right(), bottom),
            ],
            stroke,
        );
    }
    painter.text(
        rect.left_top() + egui::vec2(2.0, -2.0),
        egui::Align2::LEFT_BOTTOM,
        &key,
        egui::FontId::monospace(10.0),
        accent,
    );
}

/// 画布视口高度：面板高度有限，画布优先，令牌/提案箱在它下面滚动。
const CANVAS_HEIGHT: f32 = 380.0;

fn cursor_for_handle(handle: ResizeHandle) -> egui::CursorIcon {
    match handle {
        ResizeHandle::Left | ResizeHandle::Right => egui::CursorIcon::ResizeHorizontal,
        ResizeHandle::Top | ResizeHandle::Bottom => egui::CursorIcon::ResizeVertical,
        ResizeHandle::TopLeft | ResizeHandle::BottomRight => egui::CursorIcon::ResizeNwSe,
        ResizeHandle::TopRight | ResizeHandle::BottomLeft => egui::CursorIcon::ResizeNeSw,
    }
}

/// 画布拖动松手后：提案立刻接受（一次拖动 = 一条撤销），状态栏给出 revision。
fn commit_canvas_proposal(
    studio: &mut UiStudio,
    outcome: Result<rustlua_ui_spec::UiProposal, String>,
    status: &mut String,
) {
    match outcome {
        Ok(proposal) => match studio.accept(&proposal.proposal_id) {
            Ok(accepted) => {
                *status = format!(
                    "画布已落盘（revision {}），可用「撤销接受」回退",
                    accepted.revision
                );
            }
            Err(error) => *status = error.to_string(),
        },
        Err(error) => *status = error,
    }
}

/// 画布：按 spec 画真实内容（底色/文字/图片），并接缩放、平移与节点拖动。
///
/// 直接操作走「拖动中画幽灵框 → 松手生成一条提案 → 立刻接受」：画布能看到手指
/// 下的真实几何，落盘仍经过 Spec 宿主的提案与撤销快照。
#[allow(clippy::too_many_arguments)]
fn draw_canvas(
    ui: &mut egui::Ui,
    studio: &mut UiStudio,
    zoom: &mut f32,
    pan: &mut egui::Vec2,
    textures: &mut BTreeMap<String, egui::TextureHandle>,
    drag: &mut Option<CanvasDrag>,
    status: &mut String,
) {
    ui.horizontal(|ui| {
        if ui.small_button("适应窗口").clicked() {
            *zoom = 1.0;
            *pan = egui::Vec2::ZERO;
        }
        ui.label(format!("{:.0}%", f64::from(*zoom) * 100.0));
        ui.weak("滚轴缩放 · 右键/空白处左键拖动平移 · 拖节点或手柄调整");
    });
    let (response, _) = ui.allocate_painter(
        egui::vec2(ui.available_width().max(240.0), CANVAS_HEIGHT),
        egui::Sense::click_and_drag(),
    );
    let resolution = studio.resolution().size();
    let reference = studio
        .spec()
        .map_or([1280.0, 720.0], |spec| spec.document.canvas.reference);
    let size = [resolution[0] as f32, resolution[1] as f32];
    let mut view = crate::ui_canvas::CanvasView::new(response.rect, size, reference, *zoom, *pan);

    // 滚轴缩放：以光标为锚点。
    if response.hovered() {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > 0.5 {
            let cursor = response.hover_pos().unwrap_or(response.rect.center());
            let (next_zoom, next_pan) = view.zoomed_at((scroll * 0.0015).exp(), cursor);
            *zoom = next_zoom;
            *pan = next_pan;
            view = crate::ui_canvas::CanvasView::new(response.rect, size, reference, *zoom, *pan);
        }
    }
    // 右键 / 中键拖动平移。
    let panning = ui.input(|input| {
        input.pointer.button_down(egui::PointerButton::Secondary)
            || input.pointer.button_down(egui::PointerButton::Middle)
    });
    if panning && response.hovered() {
        *pan += ui.input(|input| input.pointer.delta());
        view = crate::ui_canvas::CanvasView::new(response.rect, size, reference, *zoom, *pan);
    }

    let rows = studio.layout_rects();
    let selection_frame = studio.selection_frame();
    if let Some(pointer) = response.hover_pos() {
        let selection_screen = selection_frame
            .as_ref()
            .map(|(_, rect, _)| view.design_to_screen(*rect));
        let icon = match canvas_drag_mode(pointer, selection_screen) {
            Some(CanvasDragMode::Resize(handle)) => cursor_for_handle(handle),
            Some(CanvasDragMode::Move) => egui::CursorIcon::Grab,
            _ => egui::CursorIcon::Default,
        };
        ui.ctx().set_cursor_icon(icon);
    }

    // Ctrl / Shift 是加选，与 Unity 的层级/场景视图一致。
    let additive =
        ui.input(|input| input.modifiers.command || input.modifiers.ctrl || input.modifiers.shift);
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let design = view.screen_to_design(pointer);
        let hit = crate::ui_canvas::pick_node(&rows, design).map(|row| row.key.clone());
        let selection_screen = selection_frame
            .as_ref()
            .map(|(_, rect, _)| view.design_to_screen(*rect));
        match canvas_drag_mode(pointer, selection_screen) {
            Some(CanvasDragMode::Resize(handle)) => {
                if let Some((key, rect, _)) = &selection_frame {
                    *drag = Some(CanvasDrag {
                        mode: CanvasDragMode::Resize(handle),
                        total: egui::Vec2::ZERO,
                        start: *rect,
                        key: key.clone(),
                        guides: SnapGuides::default(),
                    });
                }
            }
            Some(CanvasDragMode::Move) => {
                if let Some((key, rect, _)) = &selection_frame {
                    *drag = Some(CanvasDrag {
                        mode: CanvasDragMode::Move,
                        total: egui::Vec2::ZERO,
                        start: *rect,
                        key: key.clone(),
                        guides: SnapGuides::default(),
                    });
                }
            }
            _ => match hit {
                Some(key) => {
                    if additive {
                        studio.toggle_select(&key);
                    } else if !studio.selected_keys().contains(&key) {
                        studio.select(&key);
                    }
                    // 点中的节点在多选里（或刚被选中）就整组拖动。
                    if studio.selected_keys().contains(&key)
                        && let Some(rect) = node_rect(&rows, &key)
                    {
                        *drag = Some(CanvasDrag {
                            mode: CanvasDragMode::Move,
                            total: egui::Vec2::ZERO,
                            start: rect,
                            key,
                            guides: SnapGuides::default(),
                        });
                    }
                }
                // 空白处左键拖动 = 平移画布（带修饰键时保留选择）。
                None => {
                    if !additive {
                        studio.clear_selection();
                    }
                    *drag = Some(CanvasDrag {
                        mode: CanvasDragMode::Pan,
                        total: egui::Vec2::ZERO,
                        start: [0.0; 4],
                        key: String::new(),
                        guides: SnapGuides::default(),
                    });
                }
            },
        }
    }
    if response.dragged_by(egui::PointerButton::Primary)
        && let Some(state) = drag.as_mut()
    {
        let delta = response.drag_delta();
        if state.mode == CanvasDragMode::Pan {
            *pan += delta;
            view = crate::ui_canvas::CanvasView::new(response.rect, size, reference, *zoom, *pan);
        } else {
            state.total += delta;
        }
    }
    if response.drag_stopped_by(egui::PointerButton::Primary)
        && let Some(state) = drag.take()
        && state.mode != CanvasDragMode::Pan
    {
        let scale = view.design_scale().max(1e-4);
        let raw = [state.total.x / scale, state.total.y / scale];
        if state.total.length() > 1.0 {
            // 松手时按与幽灵框同一套吸附规则提交，避免“看到吸上了、落盘却差 3px”。
            let delta = match state.mode {
                CanvasDragMode::Move => studio.snap_delta(raw, SNAP_THRESHOLD).0,
                _ => raw,
            };
            let outcome = match state.mode {
                CanvasDragMode::Move => studio.move_selection_by(delta),
                CanvasDragMode::Resize(handle) => studio.resize_selection(handle, delta),
                CanvasDragMode::Pan => unreachable!(),
            }
            .map_err(|error| error.to_string());
            commit_canvas_proposal(studio, outcome, status);
        }
    }
    if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        match crate::ui_canvas::pick_node(&rows, view.screen_to_design(pointer)) {
            Some(hit) => {
                if additive {
                    studio.toggle_select(&hit.key);
                } else {
                    studio.select(&hit.key);
                }
            }
            None if !additive => studio.clear_selection(),
            None => {}
        }
    }

    if let Some(spec) = studio.spec() {
        let ctx = ui.ctx().clone();
        let mut load = |path: &str| -> Option<egui::TextureHandle> {
            if let Some(texture) = textures.get(path) {
                return Some(texture.clone());
            }
            let bytes = studio.read_asset_bytes(path)?;
            let icon = eframe::icon_data::from_png_bytes(&bytes).ok()?;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            let texture = ctx.load_texture(
                format!("ui_canvas::{path}"),
                image,
                egui::TextureOptions::NEAREST,
            );
            textures.insert(path.to_owned(), texture.clone());
            Some(texture)
        };
        crate::ui_canvas::paint(ui, spec, view, &mut load);
    }
    let mut guides = None;
    let mut move_delta = None;
    let ghost = drag.as_ref().map(|state| {
        let scale = view.design_scale().max(1e-4);
        let raw = [state.total.x / scale, state.total.y / scale];
        let (delta, snapped) = match state.mode {
            CanvasDragMode::Move => studio.snap_delta(raw, SNAP_THRESHOLD),
            _ => (raw, SnapGuides::default()),
        };
        if state.mode == CanvasDragMode::Move {
            move_delta = Some(delta);
            if snapped != SnapGuides::default() {
                guides = Some(snapped);
            }
        }
        let rect = match state.mode {
            CanvasDragMode::Move => [
                state.start[0] + delta[0],
                state.start[1] + delta[1],
                state.start[2],
                state.start[3],
            ],
            CanvasDragMode::Resize(handle) => {
                rustlua_ui_spec::resize_rect(state.start, handle, delta, 8.0)
            }
            CanvasDragMode::Pan => state.start,
        };
        (rect, state.key.as_str())
    });
    draw_overlays(ui, studio, view, ghost, move_delta, guides);
}

/// 层级行里找某个节点的当前设计矩形（面板只读用途，不重新解析布局）。
fn node_rect(rows: &[rustlua_ui_spec::LayoutRect], key: &str) -> Option<[f32; 4]> {
    rows.iter().find(|row| row.key == key).map(|row| row.rect)
}

/// 锚点预设短标签，顺序与 [`AnchorPreset::ALL`] 一致（九宫格 + 横/纵/全拉伸）。
const ANCHOR_PRESET_LABELS: [&str; 12] = [
    "左上", "上中", "右上", "左中", "居中", "右中", "左下", "下中", "右下", "横拉", "纵拉", "全铺",
];

/// 锚点预设面板：换锚点但保持当前矩形（与 Unity 的 Anchor Presets 同语义）。
fn draw_anchor_presets(ui: &mut egui::Ui, studio: &UiStudio, key: &str, status: &mut String) {
    let current = studio
        .inspector_rows()
        .into_iter()
        .find(|row| row.field == "anchor")
        .map(|row| row.value);
    ui.label("锚点预设（保持当前矩形）");
    egui::Grid::new(("ui_studio_anchor_presets", key))
        .num_columns(3)
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            for (index, preset) in AnchorPreset::ALL.iter().enumerate() {
                let selected = current.as_deref() == Some(preset.as_str());
                if ui
                    .add(
                        egui::Button::new(ANCHOR_PRESET_LABELS[index])
                            .min_size(egui::vec2(34.0, 18.0))
                            .selected(selected),
                    )
                    .on_hover_text(preset.as_str())
                    .clicked()
                {
                    let outcome = studio
                        .apply_anchor_preset(key, *preset)
                        .map_err(|error| error.to_string());
                    report_proposal(outcome, "锚点", status);
                }
                if (index + 1) % 3 == 0 {
                    ui.end_row();
                }
            }
        });
}

/// 可添加的控件类型（顺序即下拉框顺序）；新节点布局由模型按类型给默认值。
fn addable_kinds() -> Vec<(UiNodeKind, &'static str)> {
    vec![
        (UiNodeKind::Panel { src: None }, "panel"),
        (
            UiNodeKind::Container {
                direction: Default::default(),
                padding: None,
                spacing: None,
                justify: Default::default(),
                align: Default::default(),
                columns: None,
            },
            "container",
        ),
        (
            UiNodeKind::Text {
                text: "文本".to_owned(),
                rich_text: false,
            },
            "text",
        ),
        (
            UiNodeKind::Image {
                src: String::new(),
                nine_slice: None,
            },
            "image",
        ),
        (
            UiNodeKind::Button {
                text: Some("按钮".to_owned()),
                src: None,
            },
            "button",
        ),
        (
            UiNodeKind::Toggle {
                text: Some("开关".to_owned()),
                checked: false,
            },
            "toggle",
        ),
        (
            UiNodeKind::Slider {
                min: 0.0,
                max: 1.0,
                value: 0.5,
            },
            "slider",
        ),
        (UiNodeKind::Progress { value: 0.5 }, "progress"),
        (
            UiNodeKind::Scrollbar {
                value: 0.0,
                size: 0.4,
                direction: Default::default(),
                handle: None,
            },
            "scrollbar",
        ),
        (UiNodeKind::Input { placeholder: None }, "input"),
        (UiNodeKind::ScrollView { horizontal: false }, "scroll_view"),
        (UiNodeKind::Mask { padding: None }, "mask"),
        (
            UiNodeKind::CanvasGroup {
                alpha: 1.0,
                interactable: true,
                blocks_raycasts: true,
            },
            "canvas_group",
        ),
        (UiNodeKind::List { virtualized: false }, "list"),
        (UiNodeKind::Grid { columns: 2 }, "grid"),
        (UiNodeKind::Tabs { selected: 0 }, "tabs"),
        (UiNodeKind::Modal, "modal"),
        (
            UiNodeKind::Tooltip {
                text: "提示".to_owned(),
            },
            "tooltip",
        ),
        (UiNodeKind::Spacer, "spacer"),
        (UiNodeKind::SafeArea, "safe_area"),
    ]
}

/// 多选对齐/分布 + 添加子节点：与 Unity 的 Align/Distribute 和 Hierarchy 菜单同语义。
fn draw_edit_ops(
    ui: &mut egui::Ui,
    studio: &UiStudio,
    add_child_kind: &mut usize,
    status: &mut String,
) {
    let count = studio.selection_count();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("对齐（已选 {count}）"));
        for mode in AlignMode::ALL {
            if ui
                .add_enabled(count >= 2, egui::Button::new(mode.label()))
                .clicked()
            {
                let outcome = studio
                    .align_selection(mode)
                    .map_err(|error| error.to_string());
                report_proposal(outcome, mode.label(), status);
            }
        }
        ui.separator();
        for (axis, label) in [
            (DistributeAxis::Horizontal, "水平分布"),
            (DistributeAxis::Vertical, "垂直分布"),
        ] {
            if ui
                .add_enabled(count >= 3, egui::Button::new(label))
                .clicked()
            {
                let outcome = studio
                    .distribute_selection(axis)
                    .map_err(|error| error.to_string());
                report_proposal(outcome, label, status);
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("添加子节点");
        let kinds = addable_kinds();
        let index = (*add_child_kind).min(kinds.len() - 1);
        egui::ComboBox::from_id_salt("ui_studio_add_child_kind")
            .selected_text(kinds[index].1)
            .show_ui(ui, |ui| {
                for (candidate, (_, label)) in kinds.iter().enumerate() {
                    ui.selectable_value(add_child_kind, candidate, *label);
                }
            });
        let parent = studio
            .selection()
            .filter(|key| studio.accepts_children(key))
            .map(str::to_owned);
        if ui
            .add_enabled(parent.is_some(), egui::Button::new("添加到选中"))
            .clicked()
            && let Some(parent) = parent
        {
            match studio.add_node(&parent, kinds[index].0.clone()) {
                Ok((proposal, key)) => {
                    *status = format!(
                        "已生成添加提案：{parent} → {key}（{}）",
                        proposal.proposal_id
                    );
                }
                Err(error) => *status = error.to_string(),
            }
        }
    });
}

/// 屏幕点落在哪个手柄上；四角优先，其次是距边 6px 的带状区域。
fn handle_at(point: egui::Pos2, rect: egui::Rect) -> Option<ResizeHandle> {
    let tol = 7.0;
    let near = |x: f32, y: f32| (point.x - x).abs() <= tol && (point.y - y).abs() <= tol;
    let corners = [
        (ResizeHandle::TopLeft, rect.left(), rect.top()),
        (ResizeHandle::TopRight, rect.right(), rect.top()),
        (ResizeHandle::BottomLeft, rect.left(), rect.bottom()),
        (ResizeHandle::BottomRight, rect.right(), rect.bottom()),
    ];
    if let Some((handle, _, _)) = corners.iter().find(|(_, x, y)| near(*x, *y)) {
        return Some(*handle);
    }
    let inside_y = point.y >= rect.top() - tol && point.y <= rect.bottom() + tol;
    let inside_x = point.x >= rect.left() - tol && point.x <= rect.right() + tol;
    if inside_y && (point.x - rect.left()).abs() <= tol {
        return Some(ResizeHandle::Left);
    }
    if inside_y && (point.x - rect.right()).abs() <= tol {
        return Some(ResizeHandle::Right);
    }
    if inside_x && (point.y - rect.top()).abs() <= tol {
        return Some(ResizeHandle::Top);
    }
    if inside_x && (point.y - rect.bottom()).abs() <= tol {
        return Some(ResizeHandle::Bottom);
    }
    None
}

/// 命中手柄；不在手柄上但落在选中框内就是整块移动。
fn canvas_drag_mode(point: egui::Pos2, selection: Option<egui::Rect>) -> Option<CanvasDragMode> {
    let rect = selection?;
    if let Some(handle) = handle_at(point, rect) {
        return Some(CanvasDragMode::Resize(handle));
    }
    rect.contains(point).then_some(CanvasDragMode::Move)
}

/// 提案箱的逐 op 摘要：勾选框标签只放变体名，悬停显示具体字段变化。
fn patch_op_detail(op: &rustlua_ui_spec::UiPatchOp) -> String {
    use rustlua_ui_spec::UiPatchOp;
    match op {
        UiPatchOp::Replace { key, path, value } => format!("{key} {path} = {value}"),
        UiPatchOp::Insert {
            parent,
            index,
            node,
        } => format!(
            "插入 {} → {parent}[{}]",
            node.key,
            index.map_or_else(|| "-1".to_owned(), |index| index.to_string())
        ),
        UiPatchOp::Remove { key } => format!("删除 {key}"),
        UiPatchOp::Move { key, parent, index } => format!(
            "移动 {key} → {parent}[{}]",
            index.map_or_else(|| "-1".to_owned(), |index| index.to_string())
        ),
        UiPatchOp::SetEvent {
            key,
            event,
            function,
        } => format!("{key}.{event} = {function}"),
        UiPatchOp::SetStyle { key, style } => {
            format!("{key}.style = {}", style.as_deref().unwrap_or("<无>"))
        }
        UiPatchOp::SetToken { name, value } => format!("令牌 {name} = {value}"),
        UiPatchOp::RemoveToken { name } => format!("删除令牌 {name}"),
    }
}

/// 令牌面板：列表只读，增删改一律走提案（接受前不落盘）。
fn draw_token_section(
    ui: &mut egui::Ui,
    studio: &UiStudio,
    token_name: &mut String,
    token_value: &mut String,
    status: &mut String,
) {
    let tokens = studio
        .spec()
        .map(|spec| spec.tokens.clone())
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("令牌");
        ui.weak(format!("共 {} 个", tokens.len()));
    });
    if !tokens.is_empty() {
        egui::ScrollArea::vertical()
            .max_height(64.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (name, value) in &tokens {
                    ui.weak(format!("{name} = {value}"));
                }
            });
    }
    let matrix = studio.state_matrix();
    if !matrix.is_empty() {
        egui::CollapsingHeader::new(format!("样式类（{}）", matrix.len()))
            .id_salt("ui_studio_state_matrix")
            .show(ui, |ui| {
                for (class, states) in &matrix {
                    let summary = states
                        .iter()
                        .map(|(state, properties)| format!("{state:?}: {}", properties.join(", ")))
                        .collect::<Vec<_>>()
                        .join(" · ");
                    if summary.is_empty() {
                        ui.weak(format!("{class} — 无状态覆盖"));
                    } else {
                        ui.weak(format!("{class} — {summary}"));
                    }
                }
            });
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(token_name)
                .desired_width(120.0)
                .hint_text("color.accent"),
        );
        ui.add(
            egui::TextEdit::singleline(token_value)
                .desired_width(120.0)
                .hint_text("#ff8800 或 [4, 8]"),
        );
        if ui.button("提交令牌").clicked() {
            let name = token_name.trim().to_owned();
            let raw = token_value.trim();
            if name.is_empty() {
                *status = "令牌名不能为空".to_owned();
            } else {
                // 先按 JSON 解析（数字/数组），失败就当字符串。
                let value = serde_json::from_str(raw)
                    .unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()));
                match studio.set_token(&name, value) {
                    Ok(proposal) => {
                        *status = format!(
                            "已生成令牌提案（改动 {} 个令牌）",
                            proposal.diff.tokens_changed
                        );
                        token_name.clear();
                        token_value.clear();
                    }
                    Err(error) => *status = error.to_string(),
                }
            }
        }
        if ui.button("删除令牌").clicked() {
            let name = token_name.trim().to_owned();
            if name.is_empty() {
                *status = "删除令牌要先填名字".to_owned();
            } else {
                match studio.remove_token(&name) {
                    Ok(proposal) => {
                        *status = format!(
                            "已生成删除令牌提案（改动 {} 个令牌）",
                            proposal.diff.tokens_changed
                        );
                    }
                    Err(error) => *status = error.to_string(),
                }
            }
        }
    });
}

/// 切图表单的一行：`name,x,y,w,h`，空行与 `#` 注释跳过。
fn parse_slice_regions(text: &str) -> Result<Vec<SliceRegion>, String> {
    let mut regions = Vec::new();
    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(',').map(str::trim).collect();
        if parts.len() != 5 {
            return Err(format!(
                "第 {} 行需要 5 列 name,x,y,w,h（当前 {} 列）",
                line_number + 1,
                parts.len()
            ));
        }
        if parts[0].is_empty() {
            return Err(format!("第 {} 行缺少区域名", line_number + 1));
        }
        let mut rect = [0u32; 4];
        for (index, part) in parts[1..].iter().enumerate() {
            rect[index] = part.parse::<u32>().map_err(|_| {
                format!(
                    "第 {} 行第 {} 个数不是非负整数：{part}",
                    line_number + 1,
                    index + 1
                )
            })?;
        }
        regions.push(SliceRegion {
            name: parts[0].to_owned(),
            rect,
        });
    }
    if regions.is_empty() {
        return Err("至少要有一行 name,x,y,w,h".to_owned());
    }
    Ok(regions)
}

/// 候选缩略图：只解码一次；路径文件出现/消失时由 `read_asset_bytes` 返回 `None` 重试。
fn asset_thumbnail(
    ui: &egui::Ui,
    studio: &UiStudio,
    candidate: &AssetCandidate,
    textures: &mut BTreeMap<String, egui::TextureHandle>,
) -> Option<egui::TextureHandle> {
    if let Some(handle) = textures.get(&candidate.key) {
        return Some(handle.clone());
    }
    let bytes = studio.read_asset_bytes(candidate.path.as_deref()?)?;
    let icon = eframe::icon_data::from_png_bytes(&bytes).ok()?;
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    );
    let handle = ui.ctx().load_texture(
        format!("ui_studio_asset_{}", candidate.key),
        image,
        egui::TextureOptions::NEAREST,
    );
    textures.insert(candidate.key.clone(), handle.clone());
    Some(handle)
}

/// 资产面板（M76-05）：候选缩略图条 + 来源详情 + 人工审阅 + 手框切图表单。
///
/// 审阅只写 `ui/provenance/<key>.json`，不碰 `.ui.json`；切图落盘与清单由宿主负责。
// 面板绘制函数与 scene_view 各段同形：ui/studio/缓存槽/状态串都是借用，保持扁平不入 struct。
#[allow(clippy::too_many_arguments)]
fn draw_asset_section(
    ui: &mut egui::Ui,
    studio: &UiStudio,
    cache: &mut Option<AssetCache>,
    selected: &mut Option<String>,
    license: &mut String,
    textures: &mut BTreeMap<String, egui::TextureHandle>,
    slice_source: &mut String,
    slice_output: &mut String,
    slice_regions: &mut String,
    status: &mut String,
) {
    if cache
        .as_ref()
        .is_none_or(|cache| cache.revision != studio.revision())
    {
        *cache = Some(AssetCache::load(studio));
    }
    let mut refresh = false;
    let cached = cache.as_ref().expect("上一行刚填充");
    let candidates = &cached.candidates;
    ui.horizontal(|ui| {
        ui.label("资产");
        ui.weak(format!("{} 个候选", candidates.len()));
        if let Some(error) = &cached.review_error {
            ui.weak(error.clone());
        } else {
            ui.weak(format!(
                "error {} · warning {} · 共 {} 字节",
                cached.errors, cached.warnings, cached.total_bytes
            ));
        }
        if ui.small_button("刷新").clicked() {
            refresh = true;
        }
    });

    if candidates.is_empty() {
        ui.weak("还没有候选图（出图落盘或登记 assets[] 后出现在这里）");
    } else {
        egui::ScrollArea::horizontal()
            .id_salt("ui_studio_asset_strip")
            .max_height(86.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for candidate in candidates {
                        let selected_now = selected.as_deref() == Some(candidate.key.as_str());
                        let response = ui
                            .allocate_ui(egui::vec2(68.0, 78.0), |ui| {
                                ui.vertical_centered(|ui| {
                                    if let Some(texture) =
                                        asset_thumbnail(ui, studio, candidate, textures)
                                    {
                                        ui.add(
                                            egui::Image::new(&texture)
                                                .fit_to_exact_size(egui::vec2(48.0, 48.0)),
                                        );
                                    } else {
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(48.0, 48.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().rect_stroke(
                                            rect,
                                            2.0,
                                            egui::Stroke::new(
                                                1.0_f32,
                                                egui::Color32::from_gray(90),
                                            ),
                                            egui::StrokeKind::Inside,
                                        );
                                        ui.painter().text(
                                            rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "缺图",
                                            egui::FontId::proportional(10.0),
                                            egui::Color32::GRAY,
                                        );
                                    }
                                    let key: String = candidate.key.chars().take(10).collect();
                                    ui.small(key);
                                    if selected_now {
                                        ui.small("●");
                                    }
                                });
                            })
                            .response
                            .interact(egui::Sense::click())
                            .on_hover_text(format!(
                                "{} · {}{}",
                                candidate.key,
                                candidate.path.as_deref().unwrap_or("无路径"),
                                if candidate.registered {
                                    " · 已登记"
                                } else {
                                    " · 未登记"
                                }
                            ));
                        if response.clicked() {
                            *selected = Some(candidate.key.clone());
                            *license = candidate
                                .provenance
                                .as_ref()
                                .and_then(|provenance| provenance.license.clone())
                                .unwrap_or_default();
                            if slice_source.trim().is_empty()
                                && let Some(path) = &candidate.path
                            {
                                *slice_source = path.clone();
                            }
                        }
                    }
                });
            });
    }

    // 切图表单不依赖选中：没有任何候选时也能对任意工程内 PNG 手框。
    if draw_slice_form(
        ui,
        studio,
        slice_source,
        slice_output,
        slice_regions,
        status,
    ) {
        refresh = true;
    }

    let Some(candidate) = selected
        .as_deref()
        .and_then(|key| candidates.iter().find(|candidate| candidate.key == key))
    else {
        if refresh {
            *cache = None;
        }
        return;
    };
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(&candidate.key);
        ui.weak(if candidate.registered {
            "已登记 assets[]"
        } else {
            "仅候选（未进 assets[]）"
        });
        if candidate.path.is_some()
            && ui.small_button("设为切图源").clicked()
            && let Some(path) = &candidate.path
        {
            *slice_source = path.clone();
        }
    });
    if let Some(provenance) = &candidate.provenance {
        let source = match provenance.source {
            rustlua_ui_spec::AssetSource::Generated => "生成",
            rustlua_ui_spec::AssetSource::Human => "人工",
            rustlua_ui_spec::AssetSource::Imported => "导入",
        };
        let field = |label: &str, value: Option<&str>| {
            format!(
                "{label} {}",
                value.filter(|value| !value.is_empty()).unwrap_or("—")
            )
        };
        ui.weak(format!(
            "{} · {} · {} · {}",
            source,
            field("服务商", provenance.provider.as_deref()),
            field("模型", provenance.model.as_deref()),
            provenance
                .seed
                .map_or_else(|| "种子 —".to_owned(), |seed| format!("种子 {seed}"))
        ));
        let prompt = provenance
            .prompt_hash
            .as_deref()
            .map(|hash| hash.chars().take(12).collect::<String>())
            .unwrap_or_else(|| "—".to_owned());
        ui.weak(format!(
            "提示词 {prompt} · {} · {}",
            field("许可", provenance.license.as_deref()),
            if provenance.reviewed {
                "已审阅"
            } else {
                "未审阅"
            }
        ));
        if !provenance.used_by.is_empty() {
            ui.weak(format!("引用 {}", provenance.used_by.join(", ")));
        }
    } else {
        ui.weak("没有来源边车（人工图可在这里补许可并标记已审阅）");
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(license)
                .desired_width(180.0)
                .hint_text("许可（CC0 / 商用授权…）"),
        );
        let label = if candidate.provenance.as_ref().is_some_and(|p| p.reviewed) {
            "更新审阅"
        } else {
            "标记已审阅"
        };
        if ui.button(label).clicked() {
            let value = license.trim();
            *status = match studio.review_asset(
                &candidate.key,
                (!value.is_empty()).then(|| value.to_owned()),
            ) {
                Ok(provenance) => {
                    refresh = true;
                    format!(
                        "已写来源边车 {}（reviewed，许可 {}）",
                        provenance.asset_id,
                        provenance.license.as_deref().unwrap_or("未填写")
                    )
                }
                Err(error) => error.to_string(),
            };
        }
    });

    if refresh {
        *cache = None;
    }
}

/// 手框切图表单。返回 `true` 表示刚写过文件，调用方要让资产缓存失效。
fn draw_slice_form(
    ui: &mut egui::Ui,
    studio: &UiStudio,
    slice_source: &mut String,
    slice_output: &mut String,
    slice_regions: &mut String,
    status: &mut String,
) -> bool {
    let mut wrote = false;
    egui::CollapsingHeader::new("切图（手框）")
        .id_salt("ui_studio_slice_form")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(slice_source)
                        .desired_width(200.0)
                        .hint_text("源图 assets/ui/sheet.png"),
                );
                ui.add(
                    egui::TextEdit::singleline(slice_output)
                        .desired_width(160.0)
                        .hint_text("输出目录 assets/ui/sliced"),
                );
            });
            ui.add(
                egui::TextEdit::multiline(slice_regions)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("每行 name,x,y,w,h\nbuy,0,0,32,32"),
            );
            if ui.button("切图并写清单").clicked() {
                *status = match parse_slice_regions(slice_regions) {
                    Ok(regions) => {
                        match studio.slice_asset(slice_source.trim(), regions, slice_output.trim())
                        {
                            Ok(manifest) => {
                                wrote = true;
                                format!(
                                    "已切出 {} 块 → {}/slices.json（源 {}×{}）",
                                    manifest.slices.len(),
                                    manifest.output_dir,
                                    manifest.source_width,
                                    manifest.source_height
                                )
                            }
                            Err(error) => error.to_string(),
                        }
                    }
                    Err(error) => error,
                };
            }
        });
    wrote
}

/// 层级拖动落点 → (父级 key, 插入下标)。中线附近落成子节点，靠边落成兄弟。
fn hierarchy_drop(
    studio: &UiStudio,
    source: &str,
    target: &str,
    pointer_y: f32,
    target_rect: egui::Rect,
) -> Option<(String, Option<i32>)> {
    if source == target {
        return None;
    }
    let fraction =
        ((pointer_y - target_rect.top()) / target_rect.height().max(1.0)).clamp(0.0, 1.0);
    if (0.3..=0.7).contains(&fraction) && studio.accepts_children(target) {
        return Some((target.to_owned(), None));
    }
    let (parent, index) = studio.parent_and_index(target)?;
    let mut index = index as i32;
    if fraction >= 0.3 {
        index += 1;
    }
    // 同一父级里先把源摘走会前移目标下标。
    if let Some((source_parent, source_index)) = studio.parent_and_index(source)
        && source_parent == parent
        && source_index < index as usize
    {
        index -= 1;
    }
    Some((parent, Some(index)))
}

/// 预设落盘的默认路径（「导出为 UI 预设」）：工程内相对路径，与手写文档同一扩展名。
/// 目标已存在时 `export_prefab` 会直接拒绝——导出只负责新建，不覆盖别人正在编辑的文档。
fn prefab_document_path(key: &str) -> String {
    format!("ui/prefabs/{key}.ui.json")
}

fn report_proposal(
    outcome: Result<rustlua_ui_spec::UiProposal, String>,
    action: &str,
    status: &mut String,
) {
    *status = match outcome {
        Ok(proposal) => format!(
            "{action}提案已生成：+{} ~{} -{}（{}）",
            proposal.diff.nodes_added,
            proposal.diff.nodes_changed,
            proposal.diff.nodes_removed,
            proposal.proposal_id
        ),
        Err(error) => error,
    };
}

impl EditorApp {
    pub(crate) fn draw_ui_studio_panel(&mut self, ui: &mut egui::Ui) {
        if !self.project_loaded {
            ui.weak("UI 工作室需要打开工程（project.toml）");
            return;
        }

        let mut open_clicked = false;
        let mut reload_clicked = false;
        let mut undo_clicked = false;
        let mut redo_clicked = false;
        ui.horizontal(|ui| {
            ui.label("文档");
            ui.add(
                egui::TextEdit::singleline(&mut self.ui_studio_panel.document_input)
                    .desired_width(240.0)
                    .hint_text("ui/shop.ui.json"),
            );
            open_clicked = ui.button("打开").clicked();
            if self.ui_studio_panel.studio.is_some() && ui.button("刷新").clicked() {
                reload_clicked = true;
            }
            let can_undo = self
                .ui_studio_panel
                .studio
                .as_ref()
                .is_some_and(UiStudio::can_undo);
            if ui
                .add_enabled(can_undo, egui::Button::new("撤销接受"))
                .clicked()
            {
                undo_clicked = true;
            }
            let can_redo = self
                .ui_studio_panel
                .studio
                .as_ref()
                .is_some_and(UiStudio::can_redo);
            if ui
                .add_enabled(can_redo, egui::Button::new("重做接受"))
                .clicked()
            {
                redo_clicked = true;
            }
        });

        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        if open_clicked {
            let document = self.ui_studio_panel.document_input.trim().to_owned();
            let mut studio = UiStudio::new(root);
            match studio.open(&document) {
                Ok(()) => {
                    self.ui_studio_panel.status = format!("已打开 {document}");
                    self.ui_studio_panel.canvas_textures.clear();
                    self.ui_studio_panel.studio = Some(studio);
                }
                Err(error) => self.ui_studio_panel.status = error.to_string(),
            }
        }
        if reload_clicked && let Some(studio) = self.ui_studio_panel.studio.as_mut() {
            match studio.reload() {
                Ok(()) => {
                    self.ui_studio_panel.status = "已刷新".to_owned();
                    self.ui_studio_panel.canvas_textures.clear();
                }
                Err(error) => self.ui_studio_panel.status = error.to_string(),
            }
        }
        if undo_clicked && let Some(studio) = self.ui_studio_panel.studio.as_mut() {
            match studio.undo() {
                Ok(true) => {
                    self.ui_studio_panel.status =
                        format!("已撤销一次接受（revision {}）", studio.revision());
                    self.ui_studio_panel.canvas_textures.clear();
                }
                Ok(false) => self.ui_studio_panel.status = "没有可撤销的接受".to_owned(),
                Err(error) => self.ui_studio_panel.status = error.to_string(),
            }
        }
        if redo_clicked && let Some(studio) = self.ui_studio_panel.studio.as_mut() {
            match studio.redo() {
                Ok(true) => {
                    self.ui_studio_panel.status =
                        format!("已重做一次接受（revision {}）", studio.revision());
                    self.ui_studio_panel.canvas_textures.clear();
                }
                Ok(false) => self.ui_studio_panel.status = "没有可重做的接受".to_owned(),
                Err(error) => self.ui_studio_panel.status = error.to_string(),
            }
        }

        let UiStudioPanel {
            studio: Some(studio),
            status,
            canvas_zoom,
            canvas_pan,
            canvas_textures,
            canvas_paints,
            proposal_selection,
            token_name,
            token_value,
            field_inputs,
            hierarchy_drag,
            canvas_drag,
            wrap_key,
            add_child_kind,
            asset_selected,
            asset_license,
            asset_textures,
            slice_source,
            slice_output,
            slice_regions,
            asset_cache,
            ..
        } = &mut self.ui_studio_panel
        else {
            ui.weak("输入 `.ui.json` 路径后点「打开」");
            return;
        };

        *canvas_paints += 1;
        draw_toolbar(ui, studio);
        ui.separator();
        // 画布放在层级/检查器之前：面板高度有限时先保证画布可见（证据截图与日常编辑
        // 都以画布为主），令牌与提案箱向下滚动。
        draw_canvas(
            ui,
            studio,
            canvas_zoom,
            canvas_pan,
            canvas_textures,
            canvas_drag,
            status,
        );
        ui.separator();
        let rows = studio.hierarchy();
        let selected_keys: BTreeSet<String> = studio.selected_keys().into_iter().collect();
        let additive = ui.input(|input| {
            input.modifiers.command || input.modifiers.ctrl || input.modifiers.shift
        });
        let mut row_rects: Vec<(String, egui::Rect)> = Vec::new();
        let mut inspector_apply: Option<(editor::ui_studio::InspectorRow, String)> = None;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(200.0, 240.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for row in &rows {
                                let label = format!(
                                    "{}{} · {}",
                                    "  ".repeat(row.depth),
                                    row.key,
                                    crate::util::widget_display_name(row.kind)
                                );
                                let selected = selected_keys.contains(&row.key);
                                // 可拖动：松手时按落点决定“成为子节点”还是“插到兄弟前/后”。
                                let response = ui
                                    .add(
                                        egui::Button::new(label)
                                            .selected(selected)
                                            .sense(egui::Sense::click_and_drag()),
                                    )
                                    .on_hover_text("拖动改父级/排序；点击选中，Ctrl/Shift 加选");
                                if response.clicked() {
                                    if additive {
                                        studio.toggle_select(&row.key);
                                    } else {
                                        studio.select(&row.key);
                                    }
                                }
                                if response.drag_started() {
                                    *hierarchy_drag = Some(row.key.clone());
                                }
                                row_rects.push((row.key.clone(), response.rect));
                            }
                        });
                },
            );
            ui.separator();
            ui.vertical(|ui| {
                let mut section = studio.section();
                ui.horizontal(|ui| {
                    for candidate in InspectorSection::ALL {
                        if ui
                            .selectable_label(section == candidate, candidate.label())
                            .clicked()
                        {
                            section = candidate;
                        }
                    }
                });
                studio.set_section(section);
                if studio.selection_count() > 1 {
                    ui.weak(format!(
                        "多选 {} 个：检查器编辑主选中，对齐/分布用下方按钮",
                        studio.selection_count()
                    ));
                }
                let inspector = studio.inspector_rows();
                if inspector.is_empty() {
                    ui.weak(studio.section_placeholder());
                } else {
                    egui::Grid::new("ui_studio_inspector")
                        .num_columns(3)
                        .striped(true)
                        .show(ui, |ui| {
                            for row in &inspector {
                                ui.label(&row.field);
                                if row.read_only {
                                    ui.weak(&row.value);
                                    ui.label("");
                                } else {
                                    // 缓存键带 revision：接受提案后输入框自动回填新值。
                                    let id = format!(
                                        "{}/{}/{}",
                                        studio.revision(),
                                        studio.selection().unwrap_or(""),
                                        row.field
                                    );
                                    let buffer =
                                        field_inputs.entry(id).or_insert_with(|| row.value.clone());
                                    ui.add(
                                        egui::TextEdit::singleline(buffer)
                                            .desired_width(220.0)
                                            .font(egui::TextStyle::Monospace),
                                    );
                                    if ui.button("应用").clicked() {
                                        inspector_apply = Some((row.clone(), buffer.clone()));
                                    }
                                }
                                ui.end_row();
                            }
                        });
                }
                if section == InspectorSection::Layout
                    && let Some(key) = studio.selection().map(str::to_owned)
                {
                    draw_anchor_presets(ui, studio, &key, status);
                }
            });
        });

        // 层级拖动落点：中线以上/下是兄弟，中间是子节点（叶子控件不做父级）。
        if let Some(source) = hierarchy_drag.clone() {
            if ui.input(|input| input.pointer.any_down()) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if ui.input(|input| input.pointer.any_released()) {
                if let Some(point) = ui.input(|input| input.pointer.interact_pos())
                    && let Some((target, rect)) =
                        row_rects.iter().find(|(_, rect)| rect.contains(point))
                {
                    *status = match hierarchy_drop(studio, &source, target, point.y, *rect) {
                        Some((parent, index)) => match studio.move_node(&source, &parent, index) {
                            Ok(proposal) => format!(
                                "已生成移动提案：{} → {}（revision {}）",
                                source,
                                parent,
                                proposal.base_revision + 1
                            ),
                            Err(error) => error.to_string(),
                        },
                        None => format!("{source} 没有有效落点"),
                    };
                }
                *hierarchy_drag = None;
            }
        }

        if let Some((row, text)) = inspector_apply
            && let Some(key) = studio.selection().map(str::to_owned)
        {
            match row.edit {
                InspectorEdit::None => {}
                InspectorEdit::LayoutField(field) => {
                    let outcome = studio
                        .set_layout_field(&key, field, &text)
                        .map_err(|error| error.to_string());
                    report_proposal(outcome, field.label(), status);
                }
                InspectorEdit::Style => {
                    let style = text.trim().to_owned();
                    let outcome = studio
                        .set_style(&key, (!style.is_empty()).then_some(style.as_str()))
                        .map_err(|error| error.to_string());
                    report_proposal(outcome, "样式", status);
                }
                InspectorEdit::Event(event) => {
                    let outcome = studio
                        .set_event(&key, &event, text.trim())
                        .map_err(|error| error.to_string());
                    report_proposal(outcome, "事件", status);
                }
            }
        }

        // 层级操作：复制 / 删除 / 包裹进新容器，仍然全部走提案。
        ui.horizontal(|ui| {
            ui.label("层级操作");
            let selected = studio.selection().map(str::to_owned);
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("复制"))
                .clicked()
                && let Some(key) = selected.clone()
            {
                let outcome = studio
                    .duplicate_node(&key)
                    .map_err(|error| error.to_string());
                report_proposal(outcome, "复制", status);
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("删除"))
                .clicked()
                && let Some(key) = selected.clone()
            {
                let outcome = studio.remove_node(&key).map_err(|error| error.to_string());
                report_proposal(outcome, "删除", status);
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("导出为 UI 预设"))
                .on_hover_text("把选中的子树落成 ui/prefabs/<节点>.ui.json")
                .clicked()
                && let Some(key) = selected.clone()
            {
                match studio.export_prefab(&key, &prefab_document_path(&key)) {
                    Ok(accepted) => {
                        *status = format!(
                            "已导出预设：{}（版本 {}）",
                            accepted.document, accepted.revision
                        );
                    }
                    Err(error) => *status = error.to_string(),
                }
            }
            ui.add(
                egui::TextEdit::singleline(wrap_key)
                    .desired_width(110.0)
                    .hint_text("新容器标识"),
            );
            if ui
                .add_enabled(
                    selected.is_some() && !wrap_key.trim().is_empty(),
                    egui::Button::new("包裹"),
                )
                .clicked()
                && let Some(key) = selected
            {
                let outcome = studio
                    .wrap_node(&key, wrap_key.trim())
                    .map_err(|error| error.to_string());
                report_proposal(outcome, "包裹", status);
            }
        });
        draw_edit_ops(ui, studio, add_child_kind, status);

        ui.separator();
        draw_token_section(ui, studio, token_name, token_value, status);
        ui.separator();
        draw_asset_section(
            ui,
            studio,
            asset_cache,
            asset_selected,
            asset_license,
            asset_textures,
            slice_source,
            slice_output,
            slice_regions,
            status,
        );
        ui.separator();
        let proposals = studio.proposals().unwrap_or_default();
        if proposals.is_empty() {
            ui.weak("没有待处理提案（Copilot / MCP 提交后出现在这里）");
        }
        let mut accept: Option<String> = None;
        let mut accept_subset: Option<(String, Vec<usize>)> = None;
        let mut reject: Option<String> = None;
        for proposal in &proposals {
            let ops = studio
                .proposal_ops(&proposal.proposal_id)
                .unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(proposal_kind_label(proposal.kind));
                ui.label(format!(
                    "+{} ~{} -{} 资产+{} 令牌~{}",
                    proposal.diff.nodes_added,
                    proposal.diff.nodes_changed,
                    proposal.diff.nodes_removed,
                    proposal.diff.assets_added,
                    proposal.diff.tokens_changed
                ));
                if !proposal.diagnostics.is_empty() {
                    ui.label(format!("诊断 {}", proposal.diagnostics.len()));
                }
                if ui.button("接受").clicked() {
                    accept = Some(proposal.proposal_id.clone());
                }
                if ui.button("拒绝").clicked() {
                    reject = Some(proposal.proposal_id.clone());
                }
            });
            if ops.is_empty() {
                continue;
            }
            let selection = proposal_selection
                .entry(proposal.proposal_id.clone())
                .or_insert_with(|| vec![true; ops.len()]);
            selection.resize(ops.len(), true);
            ui.horizontal_wrapped(|ui| {
                ui.label("拆分接受：");
                for (index, op) in ops.iter().enumerate() {
                    ui.checkbox(
                        &mut selection[index],
                        format!("{index}. {}", patch_op_label(op)),
                    )
                    .on_hover_text(patch_op_detail(op));
                }
                let chosen = selection.iter().filter(|selected| **selected).count();
                let label = format!("接受选中 {chosen}/{}", ops.len());
                if ui
                    .add_enabled(chosen > 0, egui::Button::new(label))
                    .clicked()
                {
                    accept_subset = Some((
                        proposal.proposal_id.clone(),
                        selection
                            .iter()
                            .enumerate()
                            .filter(|(_, selected)| **selected)
                            .map(|(index, _)| index)
                            .collect(),
                    ));
                }
            });
        }
        if let Some(id) = accept {
            proposal_selection.remove(&id);
            match studio.accept(&id) {
                Ok(accepted) => {
                    *status = format!("已接受提案（版本 {}）", accepted.revision);
                    canvas_textures.clear();
                }
                Err(error) => *status = error.to_string(),
            }
        } else if let Some((id, indices)) = accept_subset {
            proposal_selection.remove(&id);
            match studio.accept_subset(&id, &indices) {
                Ok(accepted) => {
                    *status = format!(
                        "已接受选中 {} 个操作（版本 {}）",
                        indices.len(),
                        accepted.revision
                    );
                    canvas_textures.clear();
                }
                Err(error) => *status = error.to_string(),
            }
        } else if let Some(id) = reject {
            proposal_selection.remove(&id);
            match studio.reject(&id) {
                Ok(()) => *status = "已拒绝提案".to_owned(),
                Err(error) => *status = error.to_string(),
            }
        }

        ui.separator();
        let diagnostics = studio.diagnostics();
        let errors = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == rustlua_ui_spec::Severity::Error)
            .count();
        let document = studio.document().unwrap_or("<未打开>").to_owned();
        ui.label(format!(
            "{} · revision {} · 诊断 {}（error {}）· 撤销 {}",
            document,
            studio.revision(),
            diagnostics.len(),
            errors,
            studio.undo_len(),
        ));
        for diagnostic in diagnostics.iter().take(3) {
            ui.weak(format!("{}: {}", diagnostic.code, diagnostic.message));
        }
        ui.weak(status.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::open_project;
    use rustlua_infra::logging::LogSink;
    use std::path::{Path, PathBuf};

    fn write_spec(root: &Path) {
        std::fs::create_dir_all(root.join("ui")).unwrap();
        let spec: rustlua_ui_spec::UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "panel"},
            "root": {
                "key": "root",
                "type": "panel",
                "children": [
                    {"key": "title", "type": "text", "text": "Studio"},
                    {"key": "buy", "type": "button", "text": "Buy"}
                ]
            }
        }))
        .unwrap();
        let encoded = editor::ui_spec_host::encode_spec(&spec).unwrap();
        std::fs::write(root.join("ui/panel.ui.json"), encoded).unwrap();
    }

    #[test]
    fn canvas_maps_design_coordinates_into_the_viewport() {
        let view = crate::ui_canvas::CanvasView::new(
            egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(640.0, 360.0)),
            [1280.0, 720.0],
            [1280.0, 720.0],
            1.0,
            egui::Vec2::ZERO,
        );
        let mapped = view.design_to_screen([540.0, 310.0, 200.0, 100.0]);
        let scale = view.design_scale();
        assert!((mapped.width() - 200.0 * scale).abs() < 1e-3);
        let back = view.screen_to_design(mapped.min);
        assert!((back[0] - 540.0).abs() < 1e-3, "{back:?}");
        assert!((back[1] - 310.0).abs() < 1e-3, "{back:?}");

        // 375×667 手机档 + 1280×720 参考：整张预览必须完整落在视口内（留边距）。
        let view = crate::ui_canvas::CanvasView::new(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(375.0, 667.0)),
            [375.0, 667.0],
            [1280.0, 720.0],
            1.0,
            egui::Vec2::ZERO,
        );
        let canvas = view.canvas_rect();
        assert!(
            canvas.left() >= -1.0 && canvas.right() <= 376.0,
            "{canvas:?}"
        );
        assert!(
            canvas.top() >= -1.0 && canvas.bottom() <= 668.0,
            "{canvas:?}"
        );
    }

    #[test]
    fn canvas_hit_testing_prefers_corners_then_edges_then_body() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 50.0));
        assert_eq!(
            handle_at(egui::pos2(100.0, 0.0), rect),
            Some(ResizeHandle::TopRight)
        );
        assert_eq!(
            handle_at(egui::pos2(50.0, 50.0), rect),
            Some(ResizeHandle::Bottom)
        );
        assert_eq!(
            handle_at(egui::pos2(0.0, 25.0), rect),
            Some(ResizeHandle::Left)
        );
        assert_eq!(handle_at(egui::pos2(50.0, 25.0), rect), None);
        assert_eq!(
            canvas_drag_mode(egui::pos2(50.0, 25.0), Some(rect)),
            Some(CanvasDragMode::Move)
        );
        assert_eq!(
            canvas_drag_mode(egui::pos2(99.0, 25.0), Some(rect)),
            Some(CanvasDragMode::Resize(ResizeHandle::Right))
        );
        assert_eq!(canvas_drag_mode(egui::pos2(500.0, 500.0), Some(rect)), None);
        assert_eq!(canvas_drag_mode(egui::pos2(50.0, 25.0), None), None);
    }

    #[test]
    fn hierarchy_drop_maps_pointer_band_to_parent_or_sibling() {
        let temp =
            std::env::temp_dir().join(format!("rustlua-m76-hierarchy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        write_spec(&temp);
        let mut studio = UiStudio::new(&temp);
        studio.open("ui/panel.ui.json").expect("打开文档");
        let row = |y: f32| egui::Rect::from_min_size(egui::pos2(0.0, y), egui::vec2(100.0, 20.0));

        assert_eq!(
            hierarchy_drop(&studio, "title", "root", 10.0, row(0.0)),
            Some(("root".to_owned(), None)),
            "容器中线落成子节点"
        );
        assert_eq!(
            hierarchy_drop(&studio, "title", "buy", 1.0, row(0.0)),
            Some(("root".to_owned(), Some(0))),
            "上带落成兄弟，且同父级源在前要先扣下标"
        );
        assert_eq!(
            hierarchy_drop(&studio, "title", "buy", 19.0, row(0.0)),
            Some(("root".to_owned(), Some(1))),
            "下带落成兄弟：摘掉 title 后插到 buy 之后"
        );
        assert_eq!(
            hierarchy_drop(&studio, "buy", "title", 10.0, row(0.0)),
            Some(("root".to_owned(), Some(1))),
            "叶子控件中线不能当父级，退化成“放到它后面”"
        );
        assert_eq!(hierarchy_drop(&studio, "buy", "buy", 10.0, row(0.0)), None);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn studio_tab_enters_the_dock_and_draws_its_canvas() {
        let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/game_api");
        let project = open_project(&examples).expect("样板工程");
        let mut app = EditorApp::new(project, None, LogSink::new(16));
        let temp = std::env::temp_dir().join(format!("rustlua-m76-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        write_spec(&temp);
        let mut studio = UiStudio::new(temp.clone());
        studio.open("ui/panel.ui.json").expect("打开文档");
        app.ui_studio_panel.studio = Some(studio);
        app.ui_studio_panel.document_input = "ui/panel.ui.json".to_owned();
        app.show_ui_studio = true;

        let context = egui::Context::default();
        crate::util::configure_cjk_fallback(&context);
        let raw = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = context.run(raw(), |context| {
            crate::dock::show_workspace(context, &mut app)
        });
        let dock = app.dock_state.as_mut().expect("dock state");
        assert!(
            crate::dock::focus_tab(dock, crate::dock::DockTab::UiStudio),
            "UI Studio 必须在打开后进入 dock 树"
        );
        let _ = context.run(raw(), |context| {
            crate::dock::show_workspace(context, &mut app)
        });

        assert!(
            app.ui_studio_panel.canvas_paints > 0,
            "第一帧聚焦后画布必须真正绘制（不是只读文档）"
        );
        assert!(
            app.ui_studio_panel.canvas_zoom > 0.0,
            "画布缩放初始为适应窗口"
        );
        assert!(
            app.ui_studio_panel
                .studio
                .as_ref()
                .is_some_and(|studio| studio.hierarchy().len() == 3),
            "层级应读到 root/title/buy 三个节点"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn slice_region_lines_require_name_and_four_numbers() {
        let regions =
            parse_slice_regions("# 注释行\nbuy, 0, 0, 32, 16\n\nclose,32,0,16,16\n").unwrap();
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].name, "buy");
        assert_eq!(regions[0].rect, [0, 0, 32, 16]);
        assert_eq!(regions[1].rect, [32, 0, 16, 16]);

        assert!(parse_slice_regions("").unwrap_err().contains("至少"));
        assert!(parse_slice_regions("# 只有注释").is_err());
        assert!(parse_slice_regions("buy,0,0,32").is_err(), "少一列");
        assert!(parse_slice_regions("buy,0,0,-4,8").is_err(), "负数");
        assert!(parse_slice_regions(",0,0,4,4").is_err(), "缺名字");
    }

    #[test]
    fn asset_strip_decodes_a_candidate_thumbnail_and_keeps_the_selection() {
        let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/game_api");
        let project = open_project(&examples).expect("样板工程");
        let mut app = EditorApp::new(project, None, LogSink::new(16));
        let temp = std::env::temp_dir().join(format!("rustlua-m76-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        write_spec(&temp);
        let mut studio = UiStudio::new(temp.clone());
        studio.open("ui/panel.ui.json").expect("打开文档");

        let png = rustlua_ui_spec::encode_rgba_png(4, 4, &[200u8, 80, 40, 255].repeat(16));
        std::fs::create_dir_all(temp.join("assets/generated/ui")).unwrap();
        std::fs::write(temp.join("assets/generated/ui/ui_button.png"), &png).unwrap();
        std::fs::create_dir_all(temp.join("ui/provenance")).unwrap();
        let provenance = rustlua_ui_spec::AssetProvenance {
            asset_id: "ui/button".to_owned(),
            source: rustlua_ui_spec::AssetSource::Generated,
            provider: Some("local".to_owned()),
            model: Some("checker".to_owned()),
            prompt_hash: Some("abc".to_owned()),
            seed: Some(3),
            license: Some("CC0".to_owned()),
            reviewed: true,
            used_by: Vec::new(),
            created_at: None,
        };
        std::fs::write(
            temp.join("ui/provenance/ui_button.json"),
            serde_json::to_vec_pretty(&provenance).unwrap(),
        )
        .unwrap();

        app.ui_studio_panel.studio = Some(studio);
        app.ui_studio_panel.document_input = "ui/panel.ui.json".to_owned();
        app.ui_studio_panel.asset_selected = Some("ui/button".to_owned());
        app.ui_studio_panel.asset_license = "CC0".to_owned();
        app.show_ui_studio = true;

        let context = egui::Context::default();
        crate::util::configure_cjk_fallback(&context);
        let raw = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = context.run(raw(), |context| {
            crate::dock::show_workspace(context, &mut app)
        });
        let dock = app.dock_state.as_mut().expect("dock state");
        assert!(crate::dock::focus_tab(dock, crate::dock::DockTab::UiStudio));
        let _ = context.run(raw(), |context| {
            crate::dock::show_workspace(context, &mut app)
        });

        assert!(
            app.ui_studio_panel.asset_textures.contains_key("ui/button"),
            "候选缩略图必须解码并缓存"
        );
        assert_eq!(
            app.ui_studio_panel.asset_selected.as_deref(),
            Some("ui/button"),
            "渲染不能清掉选区"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}
