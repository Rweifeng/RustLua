//! P3-E03-2：可视化脚本画布（`.vgraph` 的编辑、撤销、编译诊断）。
//!
//! 画布只改 [`crate::graph_ops::GraphDocument`]：不写场景文档、不发 `EditorCommand`，
//! 与时间线面板同一套边界。保存时才落两份文件（`.vgraph` + `scripts/generated/<name>.lua`）。
//!
//! 交互口径：
//!
//! - 左键拖节点标题 → 移动；空白处拖 → 框选；中键拖 / 空白拖（未按框选键）→ 平移；滚轮 → 以指针为中心缩放；
//! - 从输出端口拖到输入端口，或点输出再点输入 → 连线；点已连接的输入端口 → 断开该输入的全部入边；
//! - `Delete` 删除选中节点；`Ctrl+Z` / `Ctrl+Y`（或 `Ctrl+Shift+Z`）撤销/重做；
//! - 参数只在右侧检查器里编辑，连续输入合并为一步撤销。
//!
//! 明确不做：复制粘贴、多选整体拖动、连线折线命中删除、节点重命名、图级变量编辑 UI。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use eframe::egui;
use rustlua_scripting::script::VgraphTraceEvent;
use rustlua_visual_script::{Category, NodeSpec, PortRef, PortType, all_nodes, node_spec};

use crate::EditorApp;
use crate::graph_ops::{GraphDocument, SaveFailure};

const NODE_WIDTH: f32 = 190.0;
const HEADER_HEIGHT: f32 = 26.0;
const ROW_HEIGHT: f32 = 18.0;
const PORT_RADIUS: f32 = 5.0;
const MIN_ZOOM: f32 = 0.35;
const MAX_ZOOM: f32 = 2.5;
const GRID: f32 = 32.0;

pub(crate) struct GraphPanelState {
    doc: Option<GraphDocument>,
    camera: egui::Vec2,
    zoom: f32,
    selected: BTreeSet<String>,
    primary: Option<String>,
    drag_id: Option<String>,
    linking: Option<PortRef>,
    box_start: Option<egui::Pos2>,
    fit_requested: bool,
    status: String,
    load_error: Option<String>,
}

impl Default for GraphPanelState {
    fn default() -> Self {
        Self {
            doc: None,
            camera: egui::Vec2::ZERO,
            zoom: 1.0,
            selected: BTreeSet::new(),
            primary: None,
            drag_id: None,
            linking: None,
            box_start: None,
            fit_requested: false,
            status: String::new(),
            load_error: None,
        }
    }
}

impl GraphPanelState {
    pub(crate) fn open(&mut self, root: &Path, relative: &Path) -> Result<(), String> {
        if self.doc.as_ref().is_some_and(GraphDocument::is_dirty) {
            return Err("当前图有未保存修改：先保存或撤销后再打开另一张图".to_owned());
        }
        let doc = GraphDocument::load(root, relative)?;
        self.status = format!(
            "已打开 {}（{} 节点 / {} 连线）",
            doc.source(),
            doc.graph.nodes.len(),
            doc.graph.edges.len()
        );
        self.doc = Some(doc);
        self.selected.clear();
        self.primary = None;
        self.drag_id = None;
        self.linking = None;
        self.camera = egui::Vec2::ZERO;
        self.zoom = 1.0;
        self.load_error = None;
        Ok(())
    }

    pub(crate) fn report_error(&mut self, message: String) {
        self.load_error = Some(message.clone());
        self.status = message;
    }

    /// 测试用的文档入口：跨面板全局 Undo 的回归测试要往图里提交一步（`command_router` 的
    /// `graph_app`）。编辑仍只走 [`GraphDocument`] 自己的方法，撤销栈由它自己维护。
    #[cfg(test)]
    pub(crate) fn doc_mut(&mut self) -> Option<&mut GraphDocument> {
        self.doc.as_mut()
    }

    // ---- 跨面板全局 Undo 的三个入口（M79 P1-2，消费方 `crate::actions::undo_ledger`）----
    //
    // 刻意**不在这里读键盘**：画布原来自己监听 Ctrl+Z/Ctrl+Y，而 shell 的全局入口也在听，
    // 一次按键于是被消费两遍（图撤一步、场景也撤一步）。快捷键归 shell，面板只留按钮。

    /// 编辑签名（无文档 = `(0, 0)`），全局账本用它判断"这张图刚刚是不是又提交了"。
    pub(crate) fn undo_signature(&self) -> (u64, usize) {
        self.doc
            .as_ref()
            .map_or((0, 0), GraphDocument::undo_signature)
    }

    pub(crate) fn undo_can_undo(&self) -> bool {
        self.doc.as_ref().is_some_and(GraphDocument::can_undo)
    }

    pub(crate) fn undo_can_redo(&self) -> bool {
        self.doc.as_ref().is_some_and(GraphDocument::can_redo)
    }

    pub(crate) fn undo_step(&mut self) -> bool {
        let Some(doc) = self.doc.as_mut() else {
            return false;
        };
        if !doc.undo() {
            return false;
        }
        // 选中集是按节点 id 记的，越过一步之后它可能已经不在图里了。
        self.selected.clear();
        self.primary = None;
        self.status = "已撤销图编辑".to_owned();
        true
    }

    pub(crate) fn redo_step(&mut self) -> bool {
        let Some(doc) = self.doc.as_mut() else {
            return false;
        };
        if !doc.redo() {
            return false;
        }
        self.selected.clear();
        self.primary = None;
        self.status = "已重做图编辑".to_owned();
        true
    }

    fn draw(&mut self, ui: &mut egui::Ui, root: &Path, trace: Option<VgraphTraceEvent>) {
        let Some(mut doc) = self.doc.take() else {
            self.draw_empty(ui);
            return;
        };
        self.draw_toolbar(ui, &mut doc, root);
        ui.separator();
        egui::TopBottomPanel::bottom("vgraph_errors")
            .resizable(true)
            .default_height(96.0)
            .max_height(240.0)
            .show_inside(ui, |ui| self.draw_errors(ui, &doc));
        egui::SidePanel::right("vgraph_inspector")
            .resizable(true)
            .default_width(236.0)
            .width_range(180.0..=360.0)
            .show_inside(ui, |ui| self.draw_inspector(ui, &mut doc));
        egui::CentralPanel::default()
            .show_inside(ui, |ui| self.draw_canvas(ui, &mut doc, trace.as_ref()));
        self.doc = Some(doc);
    }

    fn draw_empty(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            ui.heading("可视化脚本");
            ui.add_space(6.0);
            ui.weak("双击资源面板里的 `.vgraph` 打开一张图。");
            ui.weak("画布编辑只落在图文件与 `scripts/generated/*.lua`，不会修改场景文档。");
            if let Some(error) = &self.load_error {
                ui.add_space(10.0);
                ui.colored_label(egui::Color32::from_rgb(230, 110, 90), error);
            }
            if !self.status.is_empty() {
                ui.add_space(10.0);
                ui.weak(&self.status);
            }
        });
    }

    fn draw_toolbar(&mut self, ui: &mut egui::Ui, doc: &mut GraphDocument, root: &Path) {
        self.take_shortcuts(ui, doc);
        ui.horizontal_wrapped(|ui| {
            let file_name = doc
                .relative_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("未命名");
            ui.heading(file_name);
            if doc.is_dirty() {
                ui.colored_label(egui::Color32::from_rgb(240, 170, 60), "● 未保存");
            } else {
                ui.weak("已保存");
            }
            ui.separator();
            if ui
                .button("保存")
                .on_hover_text("校验并生成 Lua，再原子写回两份文件")
                .clicked()
            {
                match doc.save(root) {
                    Ok(report) => {
                        self.status = format!(
                            "已保存 {} 字节图，生成 {}（{} 字节）",
                            report.vgraph_bytes,
                            report
                                .generated
                                .strip_prefix(root)
                                .unwrap_or(&report.generated)
                                .display(),
                            report.lua_bytes
                        );
                    }
                    Err(SaveFailure::Invalid(errors)) => {
                        self.status = format!("保存中止：{} 个错误，旧文件未改动", errors.len());
                    }
                    Err(SaveFailure::Io(message)) => {
                        self.status = format!("保存失败：{message}");
                    }
                }
            }
            if ui
                .button("编译")
                .on_hover_text("只校验并生成到内存")
                .clicked()
            {
                self.status = if doc.check() {
                    format!(
                        "编译通过：{} 字节 Lua",
                        doc.compiled_lua.as_ref().map_or(0, String::len)
                    )
                } else {
                    format!("编译失败：{} 个错误", doc.errors.len())
                };
            }
            ui.separator();
            if ui
                .add_enabled(doc.can_undo(), egui::Button::new("撤销"))
                .on_hover_text("Ctrl+Z")
                .clicked()
                && doc.undo()
            {
                self.selected.clear();
                self.primary = None;
            }
            if ui
                .add_enabled(doc.can_redo(), egui::Button::new("重做"))
                .on_hover_text("Ctrl+Y / Ctrl+Shift+Z")
                .clicked()
                && doc.redo()
            {
                self.selected.clear();
                self.primary = None;
            }
            if ui.button("适配").on_hover_text("缩放到全部节点").clicked() {
                self.fit_requested = true;
            }
            ui.menu_button("添加节点", |ui| self.draw_palette(ui, doc));
            ui.separator();
            ui.weak(format!(
                "{} 节点 / {} 连线",
                doc.graph.nodes.len(),
                doc.graph.edges.len()
            ));
        });
        ui.add_space(2.0);
        ui.weak(&self.status);
    }

    /// 画布私有的快捷键。**刻意不含 Ctrl+Z / Ctrl+Y**（M79 P1-2）：撤销与重做由 shell 的
    /// 全局账本统一路由，面板再听一遍会让同一次按键消费两步（图撤一步、场景也撤一步）。
    fn take_shortcuts(&mut self, ui: &egui::Ui, doc: &mut GraphDocument) {
        if ui.ctx().wants_keyboard_input() {
            return;
        }
        let delete = ui.input(|input| input.key_pressed(egui::Key::Delete));
        if delete {
            let removed = doc.remove_nodes(&self.selected);
            if removed > 0 {
                self.selected.clear();
                self.primary = None;
                self.status = format!("已删除 {removed} 个节点");
            }
        }
    }

    fn draw_palette(&mut self, ui: &mut egui::Ui, doc: &mut GraphDocument) {
        let mut cursor = 0usize;
        for category in [
            Category::Event,
            Category::Flow,
            Category::Data,
            Category::Action,
        ] {
            ui.menu_button(category_label(category), |ui| {
                for spec in all_nodes()
                    .iter()
                    .copied()
                    .filter(|spec| spec.category == category)
                {
                    if ui
                        .button(spec.title)
                        .on_hover_text(format!("{} ({})", spec.kind, category_label(category)))
                        .clicked()
                    {
                        let offset = cursor as f32;
                        let position =
                            [60.0 + (offset % 4.0) * 220.0, 60.0 + (offset / 4.0) * 150.0];
                        match doc.add_node(spec.kind, position) {
                            Ok(id) => {
                                self.selected = BTreeSet::from([id.clone()]);
                                self.primary = Some(id.clone());
                                self.status = format!("已添加 {}（`{id}`）", spec.title);
                            }
                            Err(message) => self.status = message,
                        }
                        cursor += 1;
                    }
                }
            });
        }
    }

    fn draw_errors(&self, ui: &mut egui::Ui, doc: &GraphDocument) {
        ui.horizontal(|ui| {
            ui.strong("编译诊断");
            if doc.errors.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(120, 200, 120), "无错误");
            } else {
                ui.colored_label(
                    egui::Color32::from_rgb(235, 110, 90),
                    format!("{} 个错误", doc.errors.len()),
                );
            }
            if let Some(lua) = &doc.compiled_lua {
                ui.weak(format!("生成 {} 字节", lua.len()));
            }
            if let Some(path) = &doc.generated_path {
                ui.weak(path.display().to_string());
            }
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for error in &doc.errors {
                    ui.colored_label(egui::Color32::from_rgb(235, 140, 120), format!("• {error}"));
                }
            });
    }

    fn draw_inspector(&mut self, ui: &mut egui::Ui, doc: &mut GraphDocument) {
        let Some(primary) = self.primary.clone() else {
            ui.add_space(8.0);
            ui.weak("选中一个节点后在这里编辑参数。");
            ui.add_space(8.0);
            ui.weak("从输出端口拖到输入端口即可连线；点已连接的输入端口可断开。");
            return;
        };
        let Some(node) = doc.graph.node(&primary).cloned() else {
            self.primary = None;
            return;
        };
        let Some(spec) = node_spec(&node.kind) else {
            ui.colored_label(
                egui::Color32::RED,
                format!("未注册节点类型 `{}`", node.kind),
            );
            return;
        };
        ui.heading(spec.title);
        ui.weak(format!("id `{}` · 类型 `{}`", node.id, node.kind));
        ui.weak(format!(
            "位置 ({:.0}, {:.0})",
            node.position[0], node.position[1]
        ));
        ui.separator();
        if spec.params.is_empty() {
            ui.weak("这个节点没有参数。");
        } else {
            for param in spec.params {
                let mut value = node
                    .params
                    .get(param.name)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                ui.horizontal(|ui| {
                    ui.label(param.name);
                    let changed = match param.ty {
                        rustlua_visual_script::ValueType::Number => {
                            let mut number = value.as_f64().unwrap_or(0.0);
                            if ui
                                .add(egui::DragValue::new(&mut number).speed(0.1))
                                .changed()
                            {
                                value = serde_json::json!(number);
                                true
                            } else {
                                false
                            }
                        }
                        rustlua_visual_script::ValueType::Bool => {
                            let mut flag = value.as_bool().unwrap_or(false);
                            if ui.checkbox(&mut flag, "").changed() {
                                value = serde_json::Value::Bool(flag);
                                true
                            } else {
                                false
                            }
                        }
                        rustlua_visual_script::ValueType::String => {
                            if node.kind == "math" && param.name == "op" {
                                let mut op = value.as_str().unwrap_or("add").to_owned();
                                let before = op.clone();
                                egui::ComboBox::from_id_salt("vgraph_math_op")
                                    .selected_text(op.clone())
                                    .show_ui(ui, |ui| {
                                        for candidate in ["add", "sub", "mul", "div"] {
                                            ui.selectable_value(
                                                &mut op,
                                                candidate.to_owned(),
                                                candidate,
                                            );
                                        }
                                    });
                                if op != before {
                                    value = serde_json::json!(op);
                                    true
                                } else {
                                    false
                                }
                            } else {
                                let mut text =
                                    value.as_str().map_or_else(String::new, str::to_owned);
                                if ui
                                    .add(
                                        egui::TextEdit::singleline(&mut text)
                                            .desired_width(f32::INFINITY),
                                    )
                                    .changed()
                                {
                                    value = serde_json::json!(text);
                                    true
                                } else {
                                    false
                                }
                            }
                        }
                        _ => {
                            ui.weak("暂不支持");
                            false
                        }
                    };
                    if changed {
                        doc.set_param_coalesced(&node.id, param.name, value);
                    }
                });
            }
        }
        ui.separator();
        ui.strong("输入连线");
        let incoming = doc
            .graph
            .edges
            .iter()
            .filter(|edge| edge.to.node == node.id)
            .map(|edge| {
                (
                    edge.from.node.clone(),
                    edge.from.port.clone(),
                    edge.to.port.clone(),
                )
            })
            .collect::<Vec<_>>();
        if incoming.is_empty() {
            ui.weak("没有入边。");
        } else {
            for (from_node, from_port, to_port) in incoming {
                ui.horizontal(|ui| {
                    ui.label(format!("{from_node}.{from_port} → {to_port}"));
                    if ui.small_button("断开").clicked() {
                        doc.disconnect_input(&node.id, &to_port);
                        self.status = format!("已断开 `{}.{to_port}`", node.id);
                    }
                });
            }
        }
        ui.separator();
        if ui.button("删除节点").clicked() {
            doc.remove_nodes(&BTreeSet::from([node.id.clone()]));
            self.selected.remove(&node.id);
            self.primary = None;
            self.status = format!("已删除 `{}`", node.id);
        }
        if !doc.errors.is_empty() {
            ui.separator();
            ui.strong("与该节点相关的诊断");
            let id = node.id.clone();
            let mut any = false;
            for error in &doc.errors {
                let text = error.to_string();
                if text.contains(&format!("`{id}`")) || text.contains(&format!("`{id}.")) {
                    ui.colored_label(egui::Color32::from_rgb(235, 140, 120), text);
                    any = true;
                }
            }
            if !any {
                ui.weak("没有直接提到这个节点的错误。");
            }
        }
    }

    fn draw_canvas(
        &mut self,
        ui: &mut egui::Ui,
        doc: &mut GraphDocument,
        trace: Option<&VgraphTraceEvent>,
    ) {
        let canvas = ui.available_rect_before_wrap();
        if canvas.width() < 32.0 || canvas.height() < 32.0 {
            return;
        }
        let background = ui.allocate_rect(canvas, egui::Sense::click_and_drag());
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 0.0, egui::Color32::from_rgb(24, 26, 31));
        self.draw_grid(&painter, canvas);

        // 滚轮缩放：以指针为锚，保证指针下的世界坐标不动。
        if background.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.5 {
                let pointer = ui
                    .input(|input| input.pointer.hover_pos())
                    .unwrap_or_else(|| canvas.center());
                let before = self.screen_to_world(canvas, pointer);
                self.zoom = (self.zoom * (scroll * 0.0015).exp()).clamp(MIN_ZOOM, MAX_ZOOM);
                let after = self.world_to_screen(canvas, before);
                self.camera += pointer - after;
            }
        }
        // 中键平移。
        if background.hovered()
            && ui.input(|input| input.pointer.button_down(egui::PointerButton::Middle))
        {
            self.camera += ui.input(|input| input.pointer.delta());
        }

        if self.fit_requested {
            self.fit_requested = false;
            self.fit_to_nodes(canvas, doc);
        }

        let layouts = self.node_layouts(canvas, doc);
        let error_ids = error_node_ids(doc);
        // 只高亮"当前画布这张图"的节点：Play 可能同时跑多个 vgraph。
        let runtime_node = trace
            .filter(|event| event.script == doc.source())
            .map(|event| event.node.as_str());
        self.draw_edges(&painter, &layouts, doc);
        self.draw_nodes(ui, &painter, doc, &layouts, &error_ids, runtime_node);
        self.draw_linking_preview(ui, &painter, &layouts);
        self.handle_background(ui, &background, &layouts);
    }

    fn draw_grid(&self, painter: &egui::Painter, canvas: egui::Rect) {
        let step = GRID * self.zoom;
        if step < 8.0 {
            return;
        }
        let base = canvas.min + self.camera;
        let start_x = canvas.left() + (base.x - canvas.left()).rem_euclid(step);
        let start_y = canvas.top() + (base.y - canvas.top()).rem_euclid(step);
        let color = egui::Color32::from_rgb(32, 35, 42);
        let mut x = start_x;
        while x < canvas.right() {
            painter.line_segment(
                [egui::pos2(x, canvas.top()), egui::pos2(x, canvas.bottom())],
                stroke(1.0, color),
            );
            x += step;
        }
        let mut y = start_y;
        while y < canvas.bottom() {
            painter.line_segment(
                [egui::pos2(canvas.left(), y), egui::pos2(canvas.right(), y)],
                stroke(1.0, color),
            );
            y += step;
        }
    }

    fn node_layouts(&self, canvas: egui::Rect, doc: &GraphDocument) -> Vec<NodeLayout> {
        doc.graph
            .nodes
            .iter()
            .filter_map(|node| {
                let spec = node_spec(&node.kind)?;
                let rect = egui::Rect::from_min_size(
                    self.world_to_screen(canvas, node.position),
                    egui::vec2(NODE_WIDTH, node_height(spec)) * self.zoom,
                );
                Some(NodeLayout {
                    id: node.id.clone(),
                    spec,
                    rect,
                })
            })
            .collect()
    }

    fn draw_edges(&self, painter: &egui::Painter, layouts: &[NodeLayout], doc: &GraphDocument) {
        for edge in &doc.graph.edges {
            let Some(from) = layouts.iter().find(|layout| layout.id == edge.from.node) else {
                continue;
            };
            let Some(to) = layouts.iter().find(|layout| layout.id == edge.to.node) else {
                continue;
            };
            let Some(from_index) = from
                .spec
                .outputs
                .iter()
                .position(|port| port.name == edge.from.port)
            else {
                continue;
            };
            let Some(to_index) = to
                .spec
                .inputs
                .iter()
                .position(|port| port.name == edge.to.port)
            else {
                continue;
            };
            let start = output_port_pos(from.rect, from_index);
            let end = input_port_pos(to.rect, to_index);
            let ty = from.spec.outputs[from_index].ty;
            let base = if matches!(ty, PortType::Flow) {
                egui::Color32::from_rgb(120, 150, 210)
            } else {
                egui::Color32::from_rgb(130, 190, 140)
            };
            let highlighted =
                self.selected.contains(&edge.from.node) || self.selected.contains(&edge.to.node);
            let color = if highlighted {
                egui::Color32::WHITE
            } else {
                base
            };
            let width: f32 = if highlighted { 2.4 } else { 1.6 };
            let mid = egui::pos2((start.x + end.x) * 0.5, (start.y + end.y) * 0.5);
            let points = vec![
                start,
                egui::pos2(mid.x, start.y),
                egui::pos2(mid.x, end.y),
                end,
            ];
            painter.add(egui::Shape::line(points, stroke(width, color)));
        }
    }

    fn draw_nodes(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        doc: &mut GraphDocument,
        layouts: &[NodeLayout],
        error_ids: &BTreeSet<String>,
        runtime_node: Option<&str>,
    ) {
        let pointer = ui.input(|input| input.pointer.hover_pos());
        for layout in layouts {
            let selected = self.selected.contains(&layout.id);
            let errored = error_ids.contains(&layout.id);
            let running = runtime_node == Some(layout.id.as_str());
            let body = egui::Color32::from_rgb(40, 43, 51);
            painter.rect_filled(layout.rect, 4.0, body);
            let header = egui::Rect::from_min_size(
                layout.rect.min,
                egui::vec2(layout.rect.width(), HEADER_HEIGHT * self.zoom),
            );
            painter.rect_filled(header, 4.0, category_color(layout.spec.category));
            let title_size = (12.0 * self.zoom).clamp(8.0, 17.0);
            painter.text(
                header.left_center() + egui::vec2(8.0 * self.zoom, 0.0),
                egui::Align2::LEFT_CENTER,
                layout.spec.title,
                egui::FontId::proportional(title_size),
                egui::Color32::WHITE,
            );
            painter.text(
                header.right_center() - egui::vec2(6.0 * self.zoom, 0.0),
                egui::Align2::RIGHT_CENTER,
                &layout.id,
                egui::FontId::proportional((9.0 * self.zoom).clamp(7.0, 12.0)),
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 170),
            );
            if errored {
                painter.rect_stroke(
                    layout.rect.expand(2.0),
                    5.0,
                    stroke(2.0, egui::Color32::from_rgb(230, 90, 80)),
                    egui::StrokeKind::Inside,
                );
            } else if selected {
                painter.rect_stroke(
                    layout.rect.expand(1.0),
                    5.0,
                    stroke(2.0, egui::Color32::from_rgb(240, 200, 90)),
                    egui::StrokeKind::Inside,
                );
            } else if running {
                painter.rect_stroke(
                    layout.rect.expand(3.0),
                    6.0,
                    stroke(2.5, egui::Color32::from_rgb(110, 225, 255)),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    layout.rect.left_bottom() + egui::vec2(2.0, 4.0),
                    egui::Align2::LEFT_TOP,
                    "▶ 最近执行",
                    egui::FontId::proportional((10.0 * self.zoom).clamp(8.0, 13.0)),
                    egui::Color32::from_rgb(110, 225, 255),
                );
            } else {
                painter.rect_stroke(
                    layout.rect,
                    4.0,
                    stroke(1.0, egui::Color32::from_rgb(70, 75, 88)),
                    egui::StrokeKind::Inside,
                );
            }

            draw_port_labels(painter, layout, self.zoom);

            // 节点主体：拖动标题栏移动（避免与端口命中打架）。
            let node_response = ui.interact(
                layout.rect,
                egui::Id::new(("vgraph_node", layout.id.as_str())),
                egui::Sense::click_and_drag(),
            );
            if node_response.clicked() {
                if ui.input(|input| input.modifiers.command) {
                    if !self.selected.remove(&layout.id) {
                        self.selected.insert(layout.id.clone());
                    }
                    self.primary = self
                        .selected
                        .contains(&layout.id)
                        .then(|| layout.id.clone());
                } else {
                    self.selected = BTreeSet::from([layout.id.clone()]);
                    self.primary = Some(layout.id.clone());
                }
            }
            if node_response.drag_started() {
                if !self.selected.contains(&layout.id) {
                    self.selected = BTreeSet::from([layout.id.clone()]);
                    self.primary = Some(layout.id.clone());
                }
                doc.begin_gesture();
                self.drag_id = Some(layout.id.clone());
            }
            if node_response.dragged() && self.drag_id.as_deref() == Some(layout.id.as_str()) {
                let delta = node_response.drag_delta() / self.zoom;
                let position = doc
                    .graph
                    .node(&layout.id)
                    .map_or([0.0, 0.0], |node| node.position);
                doc.move_node(&layout.id, [position[0] + delta.x, position[1] + delta.y]);
            }
            if node_response.drag_stopped() {
                self.drag_id = None;
            }

            // 端口：节点之后注册，命中优先于节点主体。
            for (index, port) in layout.spec.inputs.iter().enumerate() {
                let center = input_port_pos(layout.rect, index);
                let hit = egui::Rect::from_center_size(center, egui::Vec2::splat(14.0));
                let response = ui.interact(
                    hit,
                    egui::Id::new(("vgraph_in", layout.id.as_str(), port.name)),
                    egui::Sense::click(),
                );
                let color = port_color(port.ty);
                painter.circle_filled(center, PORT_RADIUS * self.zoom, color);
                if response.hovered() {
                    painter.circle_stroke(
                        center,
                        (PORT_RADIUS + 3.0) * self.zoom,
                        stroke(1.0, egui::Color32::WHITE),
                    );
                }
                if response.clicked() {
                    if let Some(start) = self.linking.take() {
                        self.finish_link(doc, start, &layout.id, port.name);
                    } else if doc
                        .graph
                        .edges
                        .iter()
                        .any(|edge| edge.to.node == layout.id && edge.to.port == port.name)
                    {
                        let removed = doc.disconnect_input(&layout.id, port.name);
                        self.status = format!("已断开 {} 条入边", removed);
                    } else {
                        self.status = "先点/拖一个输出端口，再点这个输入端口完成连线".to_owned();
                    }
                }
            }
            for (index, port) in layout.spec.outputs.iter().enumerate() {
                let center = output_port_pos(layout.rect, index);
                let hit = egui::Rect::from_center_size(center, egui::Vec2::splat(14.0));
                let response = ui.interact(
                    hit,
                    egui::Id::new(("vgraph_out", layout.id.as_str(), port.name)),
                    egui::Sense::click_and_drag(),
                );
                let color = port_color(port.ty);
                painter.circle_filled(center, PORT_RADIUS * self.zoom, color);
                if response.hovered() {
                    painter.circle_stroke(
                        center,
                        (PORT_RADIUS + 3.0) * self.zoom,
                        stroke(1.0, egui::Color32::WHITE),
                    );
                }
                if response.clicked() || response.drag_started() {
                    self.linking = Some(PortRef {
                        node: layout.id.clone(),
                        port: port.name.to_owned(),
                    });
                    self.status = format!("连线起点：{}.{}", layout.id, port.name);
                }
                if response.drag_stopped()
                    && let Some(start) = self.linking.clone()
                    && let Some(target) =
                        pointer.and_then(|pointer| nearest_input(layouts, pointer, 16.0))
                {
                    self.linking = None;
                    self.finish_link(doc, start, &target.0, &target.1);
                }
            }
        }
    }

    fn finish_link(
        &mut self,
        doc: &mut GraphDocument,
        from: PortRef,
        to_node: &str,
        to_port: &str,
    ) {
        let to = PortRef {
            node: to_node.to_owned(),
            port: to_port.to_owned(),
        };
        match doc.connect(from.clone(), to) {
            Ok(()) => {
                self.status = format!("已连接 {}.{} → {to_node}.{to_port}", from.node, from.port)
            }
            Err(message) => self.status = message,
        }
    }

    fn draw_linking_preview(&self, ui: &egui::Ui, painter: &egui::Painter, layouts: &[NodeLayout]) {
        let Some(start) = &self.linking else {
            return;
        };
        let Some(pos) = layouts.iter().find_map(|layout| {
            if layout.id != start.node {
                return None;
            }
            layout
                .spec
                .outputs
                .iter()
                .position(|port| port.name == start.port)
                .map(|index| output_port_pos(layout.rect, index))
        }) else {
            return;
        };
        let Some(pointer) = ui.input(|input| input.pointer.hover_pos()) else {
            return;
        };
        painter.line_segment(
            [pos, pointer],
            stroke(1.5, egui::Color32::from_rgb(240, 200, 90)),
        );
        painter.circle_stroke(
            pos,
            (PORT_RADIUS + 3.0) * self.zoom,
            stroke(1.5, egui::Color32::from_rgb(240, 200, 90)),
        );
    }

    fn handle_background(
        &mut self,
        ui: &egui::Ui,
        background: &egui::Response,
        layouts: &[NodeLayout],
    ) {
        if background.clicked() {
            self.selected.clear();
            self.primary = None;
            self.linking = None;
        }
        if background.drag_started() {
            self.box_start = ui.input(|input| input.pointer.interact_pos());
        }
        if let Some(start) = self.box_start
            && let Some(current) = ui.input(|input| input.pointer.interact_pos())
        {
            let rect = egui::Rect::from_two_pos(start, current);
            ui.painter().rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(90, 130, 200, 40),
            );
            ui.painter().rect_stroke(
                rect,
                0.0,
                stroke(1.0, egui::Color32::from_rgb(110, 150, 220)),
                egui::StrokeKind::Inside,
            );
        }
        if background.drag_stopped()
            && let Some(start) = self.box_start.take()
            && let Some(end) = ui.input(|input| input.pointer.interact_pos())
        {
            let rect = egui::Rect::from_two_pos(start, end);
            if rect.width() > 2.0 || rect.height() > 2.0 {
                self.selected = layouts
                    .iter()
                    .filter(|layout| rect.intersects(layout.rect))
                    .map(|layout| layout.id.clone())
                    .collect();
                self.primary = self.selected.iter().next().cloned();
                self.status = format!("框选 {} 个节点", self.selected.len());
            }
        }
    }

    fn fit_to_nodes(&mut self, canvas: egui::Rect, doc: &GraphDocument) {
        let bounds = doc.graph.nodes.iter().filter_map(|node| {
            let spec = node_spec(&node.kind)?;
            Some(egui::Rect::from_min_size(
                egui::pos2(node.position[0], node.position[1]),
                egui::vec2(NODE_WIDTH, node_height(spec)),
            ))
        });
        let mut min = egui::pos2(f32::INFINITY, f32::INFINITY);
        let mut max = egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
        let mut any = false;
        for rect in bounds {
            min = min.min(rect.min);
            max = max.max(rect.max);
            any = true;
        }
        if !any {
            self.camera = egui::Vec2::ZERO;
            self.zoom = 1.0;
            return;
        }
        let width = (max.x - min.x).max(1.0);
        let height = (max.y - min.y).max(1.0);
        self.zoom = ((canvas.width() - 60.0) / width)
            .min((canvas.height() - 60.0) / height)
            .clamp(MIN_ZOOM, MAX_ZOOM);
        let center = min + egui::vec2(width, height) * 0.5;
        self.camera = canvas.center() - canvas.min - center.to_vec2() * self.zoom;
    }

    fn world_to_screen(&self, canvas: egui::Rect, world: [f32; 2]) -> egui::Pos2 {
        canvas.min + self.camera + egui::vec2(world[0], world[1]) * self.zoom
    }

    fn screen_to_world(&self, canvas: egui::Rect, screen: egui::Pos2) -> [f32; 2] {
        let local = (screen - canvas.min - self.camera) / self.zoom;
        [local.x, local.y]
    }
}

struct NodeLayout {
    id: String,
    spec: &'static NodeSpec,
    rect: egui::Rect,
}

fn stroke(width: f32, color: egui::Color32) -> egui::Stroke {
    egui::Stroke::new(width, color)
}

fn node_height(spec: &NodeSpec) -> f32 {
    let rows = spec.inputs.len().max(spec.outputs.len()).max(1) as f32;
    HEADER_HEIGHT + rows * ROW_HEIGHT + spec.params.len() as f32 * ROW_HEIGHT + 8.0
}

fn input_port_pos(rect: egui::Rect, index: usize) -> egui::Pos2 {
    egui::pos2(
        rect.left(),
        rect.top() + HEADER_HEIGHT + (index as f32 + 0.5) * ROW_HEIGHT,
    )
}

fn output_port_pos(rect: egui::Rect, index: usize) -> egui::Pos2 {
    egui::pos2(
        rect.right(),
        rect.top() + HEADER_HEIGHT + (index as f32 + 0.5) * ROW_HEIGHT,
    )
}

fn draw_port_labels(painter: &egui::Painter, layout: &NodeLayout, zoom: f32) {
    let font = egui::FontId::proportional((10.0 * zoom).clamp(7.0, 13.0));
    let color = egui::Color32::from_rgb(190, 195, 205);
    for (index, port) in layout.spec.inputs.iter().enumerate() {
        painter.text(
            input_port_pos(layout.rect, index) + egui::vec2(10.0 * zoom, 0.0),
            egui::Align2::LEFT_CENTER,
            port.name,
            font.clone(),
            color,
        );
    }
    for (index, port) in layout.spec.outputs.iter().enumerate() {
        painter.text(
            output_port_pos(layout.rect, index) - egui::vec2(10.0 * zoom, 0.0),
            egui::Align2::RIGHT_CENTER,
            port.name,
            font.clone(),
            color,
        );
    }
    let rows = layout.spec.inputs.len().max(layout.spec.outputs.len());
    for (index, param) in layout.spec.params.iter().enumerate() {
        painter.text(
            egui::pos2(
                layout.rect.left() + 8.0 * zoom,
                layout.rect.top() + HEADER_HEIGHT + (rows + index) as f32 * ROW_HEIGHT + 2.0 * zoom,
            ),
            egui::Align2::LEFT_TOP,
            format!("{}:", param.name),
            font.clone(),
            egui::Color32::from_rgb(150, 155, 168),
        );
    }
}

fn nearest_input(
    layouts: &[NodeLayout],
    pointer: egui::Pos2,
    radius: f32,
) -> Option<(String, String)> {
    let mut best: Option<(f32, String, String)> = None;
    for layout in layouts {
        for (index, port) in layout.spec.inputs.iter().enumerate() {
            let distance = input_port_pos(layout.rect, index).distance(pointer);
            if distance <= radius && best.as_ref().is_none_or(|(best, _, _)| distance < *best) {
                best = Some((distance, layout.id.clone(), port.name.to_owned()));
            }
        }
    }
    best.map(|(_, node, port)| (node, port))
}

fn error_node_ids(doc: &GraphDocument) -> BTreeSet<String> {
    let messages = doc
        .errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    doc.graph
        .nodes
        .iter()
        .filter(|node| {
            messages.iter().any(|message| {
                message.contains(&format!("`{}`", node.id))
                    || message.contains(&format!("`{}.", node.id))
                    || message.contains(&format!("{} →", node.id))
            })
        })
        .map(|node| node.id.clone())
        .collect()
}

fn category_label(category: Category) -> &'static str {
    match category {
        Category::Event => "事件",
        Category::Flow => "流程",
        Category::Data => "数据",
        Category::Action => "动作",
    }
}

fn category_color(category: Category) -> egui::Color32 {
    match category {
        Category::Event => egui::Color32::from_rgb(190, 110, 60),
        Category::Flow => egui::Color32::from_rgb(70, 110, 170),
        Category::Data => egui::Color32::from_rgb(70, 140, 90),
        Category::Action => egui::Color32::from_rgb(130, 90, 160),
    }
}

fn port_color(ty: PortType) -> egui::Color32 {
    match ty {
        PortType::Flow => egui::Color32::from_rgb(140, 170, 230),
        PortType::Value(_) => egui::Color32::from_rgb(150, 210, 160),
    }
}

impl EditorApp {
    pub(crate) fn open_graph_resource(&mut self, relative: PathBuf) {
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        match self.graph.open(&root, &relative) {
            Ok(()) => {
                self.show_graph_panel = true;
                self.status = format!("已打开可视化脚本 `{}`", relative.display());
            }
            Err(message) => {
                self.graph.report_error(message.clone());
                self.status = message;
            }
        }
    }

    pub(crate) fn draw_graph_panel(&mut self, ui: &mut egui::Ui) {
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        // Play 期间顺带打开探针：只有真正执行到节点时才有读数，编辑态不会产生假高亮。
        let trace = self.preview.as_mut().and_then(|preview| {
            preview.with_script(|runtime| {
                runtime.set_vgraph_trace_enabled(true);
                runtime.vgraph_trace_last()
            })
        });
        self.graph.draw(ui, &root, trace);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_editor_panel_reports_unregistered_node_type() {
        let temp = std::env::temp_dir().join(format!("rustlua-graph-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join("assets/graphs")).expect("建目录");
        std::fs::write(
            temp.join("assets/graphs/bad.vgraph"),
            r#"{"version":1,"name":"bad","nodes":[{"id":"x","kind":"nope","position":[0,0]}],"edges":[]}"#,
        )
        .expect("写文件");
        let mut state = GraphPanelState::default();
        state
            .open(&temp, Path::new("assets/graphs/bad.vgraph"))
            .expect("语法合法就能打开");
        let mut doc = state.doc.take().expect("文档");
        assert!(!doc.check());
        assert!(!doc.errors.is_empty());
        state.doc = Some(doc);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn graph_editor_panel_draws_headlessly_without_touching_disk() {
        let temp =
            std::env::temp_dir().join(format!("rustlua-graph-panel-draw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join("assets/graphs")).expect("建目录");
        std::fs::write(
            temp.join("assets/graphs/draw.vgraph"),
            r#"{"version":1,"name":"draw","nodes":[{"id":"start","kind":"on_start","position":[10,10]},{"id":"save","kind":"save_game","position":[240,10]}],"edges":[{"from":{"node":"start","port":"then"},"to":{"node":"save","port":"in"}}]}"#,
        )
        .expect("写文件");
        let mut state = GraphPanelState::default();
        state
            .open(&temp, Path::new("assets/graphs/draw.vgraph"))
            .expect("打开");
        state.primary = Some("start".to_owned());
        state.selected.insert("start".to_owned());
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| state.draw(ui, &temp, None));
        });
        // E03-4b：带运行时探针再画一帧，高亮路径不改变文档也不落盘。
        let trace = VgraphTraceEvent {
            script: "assets/graphs/draw.vgraph".to_owned(),
            node: "save".to_owned(),
            sequence: 3,
        };
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                state.draw(ui, &temp, Some(trace.clone()));
            });
        });
        let doc = state.doc.as_ref().expect("文档还在");
        assert!(!doc.is_dirty(), "只画一帧不应改动图");
        assert!(
            !temp.join("scripts/generated/draw.lua").exists(),
            "画布不落盘"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}
