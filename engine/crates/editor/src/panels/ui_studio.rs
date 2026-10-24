//! M76-04：UI Studio 面板（egui 薄壳）。
//!
//! 状态与动作全在 `editor::ui_studio::UiStudio`；这里只负责绘制与转调。
//! 写入仍走 Spec 宿主的提案 → 接受/撤销/拒绝，面板不直接改 `.ui.json`。

use crate::EditorApp;
use editor::ui_studio::{
    AssetCandidate, InspectorEdit, InspectorSection, PreviewResolution, UiStudio,
};
use eframe::egui;
use rustlua_ui_spec::{ResizeHandle, SliceRegion};
use std::collections::BTreeMap;

/// 画布拖动：移动整块或拖某个手柄。`total` 是跨帧累计的屏幕位移。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CanvasDragMode {
    Move,
    Resize(ResizeHandle),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CanvasDrag {
    pub(crate) mode: CanvasDragMode,
    pub(crate) total: egui::Vec2,
}

/// 面板级缓存：预览纹理按 (revision, 分辨率) 失效；文档输入框不在 UiStudio 里。
#[derive(Default)]
pub(crate) struct UiStudioPanel {
    pub(crate) studio: Option<UiStudio>,
    pub(crate) document_input: String,
    pub(crate) status: String,
    pub(crate) preview: Option<(u64, [u32; 2], egui::TextureHandle)>,
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

/// 设计坐标 → 屏幕矩形：先按预览取景（fit/offset）落到图像素，再按图像显示尺寸缩放。
fn design_rect_to_screen(
    design: [f32; 4],
    image: egui::Rect,
    image_size: [u32; 2],
    fit: f32,
    offset: [f32; 2],
) -> egui::Rect {
    let scale = image.width() / image_size[0] as f32;
    let min = image.min
        + egui::vec2(
            (offset[0] + design[0] * fit) * scale,
            (offset[1] + design[1] * fit) * scale,
        );
    egui::Rect::from_min_size(
        min,
        egui::vec2(design[2] * fit * scale, design[3] * fit * scale),
    )
}

/// 画布叠层：安全区框、选中节点轮廓、锚点十字、九宫格线、缩放手柄、间距标尺。
fn draw_overlays(ui: &egui::Ui, studio: &UiStudio, image: egui::Rect, image_size: [u32; 2]) {
    let (fit, offset) = studio.preview_fit();
    let painter = ui.painter_at(image);
    let to_screen = |design| design_rect_to_screen(design, image, image_size, fit, offset);
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
    let Some(selected) = rows.iter().find(|row| Some(&row.key) == selection.as_ref()) else {
        return;
    };
    let rect = to_screen(selected.rect);
    let accent = egui::Color32::from_rgb(96, 200, 255);
    // 间距标尺：选中框到父框四边的设计像素距离。
    if let Some(parent) = selected
        .parent
        .as_ref()
        .and_then(|key| rows.iter().find(|row| &row.key == key).map(|row| row.rect))
    {
        let gaps = [
            (
                [
                    selected.rect[0] - parent[0],
                    selected.rect[1],
                    0.0,
                    selected.rect[3],
                ],
                selected.rect[0] - parent[0],
                egui::Align2::RIGHT_CENTER,
            ),
            (
                [
                    selected.rect[0] + selected.rect[2],
                    selected.rect[1],
                    0.0,
                    selected.rect[3],
                ],
                parent[0] + parent[2] - (selected.rect[0] + selected.rect[2]),
                egui::Align2::LEFT_CENTER,
            ),
            (
                [selected.rect[0], selected.rect[1], selected.rect[2], 0.0],
                selected.rect[1] - parent[1],
                egui::Align2::CENTER_BOTTOM,
            ),
            (
                [
                    selected.rect[0],
                    selected.rect[1] + selected.rect[3],
                    selected.rect[2],
                    0.0,
                ],
                parent[1] + parent[3] - (selected.rect[1] + selected.rect[3]),
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
    // 九宫格：四边内缩线（设计像素 × fit × 显示缩放）。
    if let Some(insets) = studio.selected_nine_slice() {
        let px = fit * (image.width() / image_size[0] as f32);
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
        &selected.key,
        egui::FontId::monospace(10.0),
        accent,
    );
}

fn draw_preview(
    ui: &mut egui::Ui,
    studio: &UiStudio,
    preview: &mut Option<(u64, [u32; 2], egui::TextureHandle)>,
) -> Option<egui::Response> {
    let revision = studio.revision();
    let size = studio.resolution().size();
    let stale = preview
        .as_ref()
        .is_none_or(|(cached_revision, cached_size, _)| {
            *cached_revision != revision || *cached_size != size
        });
    if stale
        && let Ok(png) = studio.preview_png()
        && let Ok(icon) = eframe::icon_data::from_png_bytes(&png)
    {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        );
        let handle = ui.ctx().load_texture(
            format!("ui_studio_preview_{revision}_{}x{}", size[0], size[1]),
            image,
            egui::TextureOptions::NEAREST,
        );
        *preview = Some((revision, size, handle));
    }
    let Some((_, _, texture)) = preview else {
        return None;
    };
    let natural = texture.size_vec2();
    let max = egui::vec2(ui.available_width().max(64.0), 240.0);
    let scale = (max.x / natural.x).min(max.y / natural.y).min(1.0);
    let response = ui.add(egui::Image::new((texture.id(), natural * scale)));
    draw_overlays(ui, studio, response.rect, size);
    Some(response)
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
            field("provider", provenance.provider.as_deref()),
            field("model", provenance.model.as_deref()),
            provenance
                .seed
                .map_or_else(|| "seed —".to_owned(), |seed| format!("seed {seed}"))
        ));
        let prompt = provenance
            .prompt_hash
            .as_deref()
            .map(|hash| hash.chars().take(12).collect::<String>())
            .unwrap_or_else(|| "—".to_owned());
        ui.weak(format!(
            "prompt {prompt} · {} · {}",
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
            ui.weak("UI Studio 需要打开工程（project.toml）");
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
                    self.ui_studio_panel.preview = None;
                    self.ui_studio_panel.studio = Some(studio);
                }
                Err(error) => self.ui_studio_panel.status = error.to_string(),
            }
        }
        if reload_clicked && let Some(studio) = self.ui_studio_panel.studio.as_mut() {
            match studio.reload() {
                Ok(()) => {
                    self.ui_studio_panel.status = "已刷新".to_owned();
                    self.ui_studio_panel.preview = None;
                }
                Err(error) => self.ui_studio_panel.status = error.to_string(),
            }
        }
        if undo_clicked && let Some(studio) = self.ui_studio_panel.studio.as_mut() {
            match studio.undo() {
                Ok(true) => {
                    self.ui_studio_panel.status =
                        format!("已撤销一次接受（revision {}）", studio.revision());
                    self.ui_studio_panel.preview = None;
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
                    self.ui_studio_panel.preview = None;
                }
                Ok(false) => self.ui_studio_panel.status = "没有可重做的接受".to_owned(),
                Err(error) => self.ui_studio_panel.status = error.to_string(),
            }
        }

        let UiStudioPanel {
            studio: Some(studio),
            status,
            preview,
            proposal_selection,
            token_name,
            token_value,
            field_inputs,
            hierarchy_drag,
            canvas_drag,
            wrap_key,
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

        draw_toolbar(ui, studio);
        ui.separator();
        // 画布放在层级/检查器之前：面板高度有限时先保证画布可见（证据截图与日常编辑
        // 都以画布为主），令牌与提案箱向下滚动。
        let preview_response = draw_preview(ui, studio, preview);
        if let Some(response) = preview_response {
            let size = studio.resolution().size();
            let (fit, offset) = studio.preview_fit();
            let scale = fit * response.rect.width() / size[0] as f32;
            let selection_screen = studio
                .selection_frame()
                .map(|(_, rect, _)| design_rect_to_screen(rect, response.rect, size, fit, offset));
            let drag = ui.interact(
                response.rect,
                ui.id().with("ui_studio_canvas"),
                egui::Sense::click_and_drag(),
            );
            if drag.drag_started()
                && let Some(point) = drag.interact_pointer_pos()
                && let Some(mode) = canvas_drag_mode(point, selection_screen)
            {
                *canvas_drag = Some(CanvasDrag {
                    mode,
                    total: egui::Vec2::ZERO,
                });
            }
            if drag.dragged()
                && let Some(state) = canvas_drag.as_mut()
            {
                state.total += drag.drag_delta();
            }
            if drag.drag_stopped()
                && let Some(state) = canvas_drag.take()
                && state.total.length() > 1.0
            {
                let delta = [
                    state.total.x / scale.max(1e-4),
                    state.total.y / scale.max(1e-4),
                ];
                let outcome = match state.mode {
                    CanvasDragMode::Move => studio.move_selection_by(delta),
                    CanvasDragMode::Resize(handle) => studio.resize_selection(handle, delta),
                }
                .map_err(|error| error.to_string());
                report_proposal(outcome, "画布", status);
            }
        }
        ui.separator();
        let rows = studio.hierarchy();
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
                                let label =
                                    format!("{}{} · {}", "  ".repeat(row.depth), row.key, row.kind);
                                let selected = studio.selection() == Some(row.key.as_str());
                                // 可拖动：松手时按落点决定“成为子节点”还是“插到兄弟前/后”。
                                let response = ui
                                    .add(
                                        egui::Button::new(label)
                                            .selected(selected)
                                            .sense(egui::Sense::click_and_drag()),
                                    )
                                    .on_hover_text("拖动改父级/排序；点击选中");
                                if response.clicked() {
                                    studio.select(&row.key);
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
                                    let id = format!(
                                        "{}/{}",
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
                InspectorEdit::Layout => {
                    let outcome = serde_json::from_str::<rustlua_ui_spec::UiLayoutSpec>(&text)
                        .map_err(|error| format!("布局 JSON 不合法：{error}"))
                        .and_then(|layout| {
                            let value =
                                serde_json::to_value(layout).map_err(|error| error.to_string())?;
                            studio
                                .set_field(&key, "/layout", value)
                                .map_err(|error| error.to_string())
                        });
                    report_proposal(outcome, "布局", status);
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
            ui.add(
                egui::TextEdit::singleline(wrap_key)
                    .desired_width(110.0)
                    .hint_text("新容器 key"),
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
                ui.label(format!("{:?}", proposal.kind));
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
                    *status = format!("已接受提案，revision {}", accepted.revision);
                    *preview = None;
                }
                Err(error) => *status = error.to_string(),
            }
        } else if let Some((id, indices)) = accept_subset {
            proposal_selection.remove(&id);
            match studio.accept_subset(&id, &indices) {
                Ok(accepted) => {
                    *status = format!(
                        "已接受选中 {} 个操作，revision {}",
                        indices.len(),
                        accepted.revision
                    );
                    *preview = None;
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
    fn overlay_maps_design_coordinates_into_the_displayed_image() {
        let image = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(640.0, 360.0));
        let mapped = design_rect_to_screen(
            [540.0, 310.0, 200.0, 100.0],
            image,
            [1280, 720],
            1.0,
            [0.0, 0.0],
        );
        assert_eq!(mapped.min, egui::pos2(280.0, 175.0));
        assert_eq!(mapped.size(), egui::vec2(100.0, 50.0));

        // 手机档 375×667 + 1280×720 参考：fit≈0.29297、纵向信箱 offset≈228.03，
        // 整块画布应正好落在可见的 375×210.9375 区域。
        let (fit, offset) = rustlua_ui_spec::preview_fit([1280.0, 720.0], 375, 667, 1.0);
        let image = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(375.0, 667.0));
        let canvas =
            design_rect_to_screen([0.0, 0.0, 1280.0, 720.0], image, [375, 667], fit, offset);
        assert!(canvas.left().abs() < 1e-4);
        assert!((canvas.top() - 228.031_25).abs() < 1e-3);
        assert!((canvas.width() - 375.0).abs() < 1e-4);
        assert!((canvas.height() - 210.937_5).abs() < 1e-3);
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
    fn studio_tab_enters_the_dock_and_renders_its_preview() {
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
            app.ui_studio_panel.preview.is_some(),
            "第一帧聚焦后必须解码并上传预览纹理"
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
