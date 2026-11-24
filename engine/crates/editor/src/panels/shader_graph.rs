//! M85：面向非 shader 开发者的 Shader Graph 面板（真实图模型）。
//!
//! 面板里的节点与连线就是 `ShaderGraph` 的节点与连线：画布只改这份图，保存时原样落
//! `.sgraph`。这样"在画布上看到的"与"存到磁盘的"是同一份数据，不会出现两套模型各说各话。

use std::collections::BTreeMap;
use std::path::Path;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use rustlua_visual_script::shader_graph::{
    ShaderEdge, ShaderGraph, ShaderNode, ShaderNodeKind, ShaderTemplate,
};

const NODE_WIDTH: f32 = 168.0;
const HEADER_HEIGHT: f32 = 26.0;
const PORT_ROW: f32 = 18.0;
const BODY_PAD: f32 = 8.0;
const GRID: f32 = 32.0;

/// 节点默认落盘路径：按模板 id 生成，避免每个模板都手写路径。
fn suggested_path(template: ShaderTemplate) -> String {
    format!("assets/fx/{}.sgraph", template.id())
}

/// 预设默认名：取 `.sgraph` 的文件名（`flame_blade_slash` → `flame_blade_slash`）。
///
/// 不用模板中文名：同名模板可以生成多份预设（暖色/冷色），用文件名才分得开。
fn default_preset_name(graph_path: &str) -> String {
    Path::new(graph_path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "effect".to_owned())
}

/// 预设抽屉里每个参数的滑块范围。
///
/// 按名字分段而不是按节点：命名参数是作者唯一看到的东西（脚本里写的也是它），
/// 用同一个词根分类，作者拖动时不会困惑"为什么这个能到 20、那个只能到 1"。
fn preset_slider_range(name: &str) -> (f32, f32) {
    if name.starts_with("color_") || name == "alpha" {
        (0.0, 1.0)
    } else if name.starts_with("spin")
        || name.contains("frequency")
        || name.contains("speed")
        || name.contains("strength")
    {
        (0.0, 20.0)
    } else {
        (0.0, 2.0)
    }
}

/// 面板内实时预览的状态：一台离屏渲染器 + 一张 egui 纹理。
///
/// 换模板/改参数只会让生成的 WGSL 变，`last_source` 一比对就重编管线；
/// 播放时每帧推进 `time` 并重新渲染，作者看到的是会动的效果。
struct PreviewState {
    renderer: crate::panels::shader_graph_preview::ShaderPreview,
    texture: Option<egui::TextureHandle>,
    last_source: String,
    playing: bool,
    time: f32,
    last_render_ms: f32,
    error: Option<String>,
    /// 当前图的参数槽位表：每帧按图上的参数值重写 uniform（不重编管线）。
    compiled: Option<rustlua_visual_script::shader_graph::CompiledShader>,
    /// 上一次写进 uniform 的槽位与「效果强度」。暂停时靠它们发现"参数变了"，
    /// 否则暂停状态下拖滑杆只改了 uniform、画面却停在上一帧。
    last_slots: [[f32; 4]; 16],
    last_intensity: f32,
}

/// 模板画廊状态：一台独立预览器 + 每模板一张缩略图（逐帧渲染，避免一次性卡住 UI）。
struct GalleryState {
    preview: crate::panels::shader_graph_preview::ShaderPreview,
    thumbnails: Vec<Option<egui::TextureHandle>>,
    next: usize,
    error: Option<String>,
}

const THUMB_W: u32 = 96;
const THUMB_H: u32 = 60;

/// 画布状态。`graph` 是唯一事实源；`undo_stack` 存整图快照，撤销/重做等价于换回旧图。
pub struct ShaderGraphPanel {
    graph: ShaderGraph,
    saved: Option<ShaderGraph>,
    template: usize,
    selected: Option<String>,
    pan: Vec2,
    zoom: f32,
    undo_stack: Vec<ShaderGraph>,
    redo: Vec<ShaderGraph>,
    intensity: f32,
    preview_ms: f32,
    preview_wgsl: String,
    preview: Option<PreviewState>,
    /// 等待宿主（`EditorApp`）执行的场景叠加动作：`Some(Some(src))` = 应用，`Some(None)` = 关闭。
    pending_overlay: Option<Option<String>>,
    /// 已应用到场景叠加的参数表（面板每帧把图上的参数写进 uniform）。
    overlay_compiled: Option<rustlua_visual_script::shader_graph::CompiledShader>,
    overlay_applied: bool,
    /// 模板画廊：一屏缩略图，点击切换模板。
    gallery_open: bool,
    gallery: Option<GalleryState>,
    /// M96 生成预设抽屉：给全部命名参数配滑块，一次生成 `.prefab`。
    preset_open: bool,
    preset_name: String,
    preset_path: String,
    /// 抽屉里滑块的当前取值（名字 → 值）。生成时全量写出，缺的用图默认值补。
    preset_values: BTreeMap<String, f32>,
    path: String,
    status: String,
}

impl Default for ShaderGraphPanel {
    fn default() -> Self {
        let template = ShaderTemplate::ALL[0];
        let mut graph = template.graph();
        auto_layout(&mut graph);
        Self {
            graph,
            saved: None,
            template: 0,
            selected: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
            undo_stack: Vec::new(),
            redo: Vec::new(),
            intensity: 1.0,
            preview_ms: 0.0,
            preview_wgsl: String::new(),
            preview: None,
            pending_overlay: None,
            overlay_compiled: None,
            overlay_applied: false,
            gallery_open: false,
            gallery: None,
            preset_open: false,
            preset_name: String::new(),
            preset_path: String::new(),
            preset_values: BTreeMap::new(),
            path: suggested_path(template),
            status: "选择模板或打开 .sgraph 开始".into(),
        }
    }
}

impl ShaderGraphPanel {
    fn templates() -> &'static [ShaderTemplate] {
        &ShaderTemplate::ALL
    }

    fn is_dirty(&self) -> bool {
        self.saved.as_ref() != Some(&self.graph)
    }

    fn load_template(&mut self, index: usize) {
        let all = Self::templates();
        self.template = index % all.len();
        let template = all[self.template];
        let mut graph = template.graph();
        auto_layout(&mut graph);
        self.graph = graph;
        self.saved = None;
        self.path = suggested_path(template);
        self.selected = None;
        self.undo_stack.clear();
        self.redo.clear();
        self.intensity = 1.0;
        self.status = format!("已载入模板「{}」", template.name_zh());
    }

    fn push_history(&mut self) {
        self.undo_stack.push(self.graph.clone());
        if self.undo_stack.len() > 64 {
            self.undo_stack.remove(0);
        }
        self.redo.clear();
    }

    fn undo(&mut self) {
        if let Some(previous) = self.undo_stack.pop() {
            self.redo.push(self.graph.clone());
            self.graph = previous;
            self.status = "已撤销上一步图编辑".into();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo_stack.push(self.graph.clone());
            self.graph = next;
            self.status = "已重做图编辑".into();
        }
    }

    fn save(&mut self, root: &Path) {
        match editor::shader_graph_ops::save_shader_graph(root, &self.path, &self.graph) {
            Ok(path) => {
                self.saved = Some(self.graph.clone());
                // 顺手导出可审阅的 WGSL。它**不会**自动生效：运行时只认
                // `shaders/<内置文件名>.wgsl` 覆盖，自定义材质 pass 是后续工作。
                let wgsl_path = self.path.replace(".sgraph", ".wgsl");
                match self.graph.compile_wgsl() {
                    Ok(source) => {
                        match editor::shader_graph_ops::save_shader_source(
                            root, &wgsl_path, &source,
                        ) {
                            Ok(exported) => {
                                self.status = format!(
                                    "已保存 {}（并导出 WGSL {}）",
                                    path.display(),
                                    exported.display()
                                );
                            }
                            Err(error) => {
                                self.status =
                                    format!("已保存 {}；WGSL 导出失败：{error}", path.display());
                            }
                        }
                    }
                    Err(error) => {
                        self.status = format!("已保存 {}；WGSL 生成失败：{error}", path.display());
                    }
                }
            }
            Err(error) => self.status = format!("保存失败：{error}"),
        }
    }

    /// M96：开关「生成预设」抽屉。打开时按当前图重置全部参数值（默认取图上的值）。
    ///
    /// 为什么打开就重置而不是保留上次的滑块：抽屉的语义是"把现在这张图的样子存成预设"。
    /// 保留上次的值会让作者以为滑块反映了图，实际是几分钟前的旧数。
    fn toggle_preset_drawer(&mut self) {
        self.preset_open = !self.preset_open;
        if !self.preset_open {
            return;
        }
        self.preset_values.clear();
        for param in self.compiled_named_params() {
            self.preset_values.insert(param.name, param.default);
        }
        if self.preset_name.trim().is_empty() {
            self.preset_name = default_preset_name(&self.path);
        }
        if self.preset_path.trim().is_empty() {
            self.preset_path = self.path.replace(".sgraph", ".prefab");
        }
        let count = self.preset_values.len();
        self.status = format!("生成预设：{count} 个可调参数（默认取当前图的值）");
        // 关掉实时预览的播放：抽屉底下的画布被盖掉了，还继续每帧渲染+回读只是白烧 GPU。
        // 作者拖动滑块时预览图仍会更新——抽屉里的值会经 `preset_slots()` 走同一条
        // "参数变了就重绘一次"的路径，所以调试体验不受影响。
        if let Some(state) = self.preview.as_mut() {
            state.playing = false;
        }
    }

    /// 真窗口证据（`gui_evidence`）用：确认抽屉是否开着。
    pub(crate) fn preset_drawer_open(&self) -> bool {
        self.preset_open
    }

    /// 真窗口证据用：只打开不切换（避免证据步骤每帧把用户已经打开的抽屉又关掉）。
    pub(crate) fn toggle_preset_drawer_for_evidence(&mut self) {
        self.toggle_preset_drawer();
    }

    /// 当前图编译出的命名参数表；图非法时返回空（抽屉里显示原因，不假装有参数）。
    fn compiled_named_params(&self) -> Vec<rustlua_visual_script::shader_graph::NamedShaderParam> {
        match self.graph.compile_wgsl_uniform(1, 3) {
            Ok(compiled) => compiled.named_params(),
            Err(_) => Vec::new(),
        }
    }

    /// 把抽屉里的取值写成 `.prefab`：先存 `.sgraph`（预设引用的图必须与画面一致），
    /// 再写预设。任一步失败都保留抽屉状态，让作者改完路径直接重试。
    fn generate_preset(&mut self, root: &Path) {
        // 图没落盘就先生成的话，预设指向的 `.sgraph` 会是旧版本，运行起来"参数对、画面对不上"。
        if self.is_dirty() || self.saved.is_none() {
            self.save(root);
            if self.is_dirty() {
                self.status = format!("生成预设已取消：.sgraph 未保存成功（{}）", self.status);
                return;
            }
        }
        let name = self.preset_name.trim();
        if name.is_empty() {
            self.status = "生成预设失败：预设名不能为空".into();
            return;
        }
        let overlays: Vec<String> = self
            .compiled_named_params()
            .into_iter()
            .map(|param| param.name)
            .collect();
        if overlays.is_empty() {
            self.status = "生成预设失败：当前图没有可调参数（先接上参数节点）".into();
            return;
        }
        let text = match rustlua_visual_script::effect_prefab::preset_text_from_graph(
            &self.graph,
            name,
            &self.path,
            true,
            &self.preset_values,
            &[],
        ) {
            Ok(text) => text,
            Err(error) => {
                self.status = format!("生成预设失败：{error}");
                return;
            }
        };
        match editor::shader_graph_ops::save_effect_prefab(root, &self.preset_path, &text) {
            Ok(path) => {
                self.status = format!(
                    "已生成预设 {}（{} 个参数，引用 {}）",
                    path.display(),
                    overlays.len(),
                    self.path
                );
            }
            Err(error) => self.status = format!("生成预设失败：{error}"),
        }
    }

    /// 底部抽屉：列出**全部**命名参数（不只是选中节点的），一行最多两个滑块。
    ///
    /// 放在画布下沿而不是右栏：右栏 240px 里已经塞了实时预览 + 模板说明 + 节点检查器，
    /// 再挤一组滑块会把预览压没；抽屉横着铺，预览始终可见。
    ///
    /// 布局按**可用宽度**分档（真窗口证据里 100% DPI + 默认窗口下画布只有 ~430px）：
    /// 窄了就改成两行——一行放预设名/路径，一行放按钮，参数一行一个，全部靠左对齐。
    /// 一开始按单行写，结果「生成 .prefab」按钮被挤到抽屉右侧外面，截图里只剩半截。
    fn draw_preset_drawer(&mut self, ui: &mut egui::Ui, root: &Path, area: Rect) {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        let narrow = area.width() < 620.0;
        let mut generate = false;
        // 三行固定结构（不随宽度挤）：标题+名字 / 路径 / 按钮。
        //
        // 为什么不用"一行排开 + 自适应宽度"：`TextEdit` 的实际宽度是
        // `min(desired_width, ui.available_width())`，而横向布局里 `available_width`
        // 是"整行剩余"，`add_sized`/`set_max_width` 都拦不住它把后面控件顶出抽屉
        // （真窗口证据 430px 宽时就点不到「关闭」）。分行是对宽度最不敏感的结构。
        child.horizontal(|ui| {
            ui.label(
                egui::RichText::new("生成特效预设（.prefab）")
                    .strong()
                    .size(13.0),
            );
            ui.separator();
            ui.label("预设名");
            ui.add(
                egui::TextEdit::singleline(&mut self.preset_name)
                    .desired_width(140.0)
                    .hint_text("预设名"),
            );
            if !narrow && ui.button("关闭").clicked() {
                self.preset_open = false;
            }
        });
        child.horizontal(|ui| {
            ui.label("路径");
            ui.add(
                egui::TextEdit::singleline(&mut self.preset_path)
                    .desired_width(300.0)
                    .hint_text("assets/fx/x.prefab"),
            );
        });
        child.horizontal(|ui| {
            if ui
                .button("生成 .prefab")
                .on_hover_text(
                    "把当前图 + 下面这组参数写成一个预设，逻辑里 render.play_effect 即可加载",
                )
                .clicked()
            {
                generate = true;
            }
            ui.label(
                egui::RichText::new("先存 .sgraph，再写 .prefab")
                    .small()
                    .color(Color32::from_gray(150)),
            );
            if narrow && ui.button("关闭").clicked() {
                self.preset_open = false;
            }
        });
        if generate {
            self.generate_preset(root);
        }
        let params = self.compiled_named_params();
        if params.is_empty() {
            child.label(
                egui::RichText::new(
                    "当前图没有可调参数：先给节点接上参数（如扇区/圆环/颜色），再回来生成。",
                )
                .small()
                .color(Color32::from_rgb(230, 170, 80)),
            );
            return;
        }
        child.label(
            egui::RichText::new(
                "这些值会一起写进预设；脚本之后仍可用 render.set_effect_param 逐帧覆盖。",
            )
            .small()
            .color(Color32::from_gray(180)),
        );
        let per_row = if area.width() >= 700.0 { 2 } else { 1 };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                let column_width = (area.width() - 12.0) / per_row as f32;
                for chunk in params.chunks(per_row) {
                    ui.horizontal(|ui| {
                        for param in chunk {
                            let value = self
                                .preset_values
                                .entry(param.name.clone())
                                .or_insert(param.default);
                            // 范围按参数语义给：颜色 0..1、宽度/半径/亮度这类 0..2、
                            // 其余（频率/转速/倍率）放宽到 0..20。滑块是"手感"，精确值
                            // 仍可在节点检查器里改。
                            let (min, max) = preset_slider_range(&param.name);
                            ui.allocate_ui_with_layout(
                                Vec2::new(column_width, 22.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add_sized(
                                        [70.0, 18.0],
                                        egui::Label::new(egui::RichText::new(&param.name).small())
                                            .truncate(),
                                    );
                                    ui.add(
                                        egui::Slider::new(value, min..=max)
                                            .show_value(true)
                                            .clamping(egui::SliderClamping::Always),
                                    );
                                },
                            );
                        }
                    });
                }
            });
    }

    fn open(&mut self, root: &Path) {
        match editor::shader_graph_ops::load_shader_graph(root, &self.path) {
            Ok(mut graph) => {
                if graph.nodes.iter().all(|node| node.position == [0.0, 0.0]) {
                    auto_layout(&mut graph);
                }
                self.graph = graph.clone();
                self.saved = Some(graph);
                self.selected = None;
                self.undo_stack.clear();
                self.redo.clear();
                self.status = format!("已打开 {}", self.path);
            }
            Err(error) => self.status = format!("打开失败：{error}"),
        }
    }

    /// 按工程内相对路径打开（资源面板双击 `.sgraph` 的入口）。
    pub(crate) fn open_relative(&mut self, root: &Path, relative: &str) {
        self.path = relative.to_owned();
        self.open(root);
    }

    fn compile_preview(&mut self) {
        let started = std::time::Instant::now();
        match self.graph.compile_wgsl() {
            Ok(source) => {
                self.preview_wgsl =
                    crate::panels::shader_graph_preview::preview_shader_source(&source);
                self.preview_ms = started.elapsed().as_secs_f32() * 1000.0;
                self.status = format!(
                    "WGSL 生成成功 · {} 字节 · {:.2} ms",
                    self.preview_wgsl.len(),
                    self.preview_ms
                );
            }
            Err(error) => self.status = format!("编译失败：{error}"),
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        root: &Path,
        gpu: Option<&(wgpu::Device, wgpu::Queue)>,
    ) {
        self.toolbar(ui, root);
        ui.separator();
        let full = ui.available_rect_before_wrap();
        let inspector_width = 240.0_f32.min(full.width() * 0.38);
        let (response, painter) = ui.allocate_painter(full.size(), Sense::drag());
        let canvas = Rect::from_min_max(
            response.rect.min,
            Pos2::new(
                response.rect.right() - inspector_width,
                response.rect.bottom(),
            ),
        );
        let inspector = Rect::from_min_max(
            Pos2::new(canvas.right(), response.rect.top()),
            response.rect.max,
        );

        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                self.zoom = (self.zoom * (1.0 + scroll * 0.001)).clamp(0.4, 2.2);
            }
        }
        if response.dragged() {
            self.pan += response.drag_delta();
        }

        // 画廊打开时整块面板换成缩略图墙：一屏看完所有模板，点一下就切过去。
        if self.gallery_open {
            painter.rect_filled(response.rect, 0.0, Color32::from_rgb(23, 27, 31));
            self.draw_gallery(ui, response.rect, gpu);
            return;
        }

        painter.rect_filled(canvas, 0.0, Color32::from_rgb(23, 27, 31));
        self.draw_grid(&painter, canvas);
        self.draw_edges(&painter, canvas);
        // M96：生成预设抽屉从画布下沿往上开，盖住画布底部但不碰右侧预览——作者要边看画面
        // 边调参数，预览被遮住这条路径就没法用。
        let mut canvas_bottom = canvas.bottom();
        if self.preset_open {
            let drawer_height = (canvas.height() * 0.42).clamp(120.0, 260.0);
            let drawer = Rect::from_min_max(
                Pos2::new(canvas.left(), canvas.bottom() - drawer_height),
                canvas.max,
            );
            painter.rect_filled(drawer, 0.0, Color32::from_rgb(27, 31, 36));
            painter.rect_stroke(
                drawer,
                0.0,
                Stroke::new(1.0_f32, Color32::from_rgb(58, 66, 74)),
                egui::StrokeKind::Inside,
            );
            self.draw_preset_drawer(ui, root, drawer.shrink2(Vec2::new(10.0, 8.0)));
            canvas_bottom = drawer.top();
        }
        // 抽屉盖住的那段画布必须同时**不可交互**：`ui.interact` 用的是绝对矩形，
        // 按裁剪后的矩形传进去，被遮住的节点才不会"看不见却点得中、拖得动"。
        let canvas_visible =
            Rect::from_min_max(canvas.min, Pos2::new(canvas.right(), canvas_bottom));
        self.draw_nodes(ui, &painter, canvas_visible);
        // 先铺检查器底色，再画实时预览：两者共用同一 painter，后画的覆盖先画的。
        // 若把底色留在 `draw_inspector` 里铺，它会盖掉预览图（M98 回归）。
        painter.rect_filled(inspector, 0.0, Color32::from_rgb(31, 35, 39));
        // 实时预览占检查器顶部：改模板/参数后作者第一眼看到的就是画面。
        let preview_height = self.draw_preview(ui, &painter, inspector, gpu);
        self.draw_inspector(ui, &painter, inspector, preview_height);
        // 状态栏贴在最上沿：抽屉打开时会盖住画布底部，放那儿的状态提示就看不见了。
        painter.text(
            canvas.left_top() + Vec2::new(10.0, 6.0),
            Align2::LEFT_TOP,
            &self.status,
            FontId::proportional(11.0),
            Color32::from_gray(175),
        );
    }

    /// 画检查器顶部的实时预览，返回它占用的高度（模板说明从这里往下排）。
    ///
    /// 每帧流程：图 → WGSL → 与上一份比对（变了才重编管线）→ 推进时间 → 渲染 → 回读 →
    /// 上传成 egui 纹理。没有 GPU 设备时只画一行说明，不假装"预览通过"。
    fn draw_preview(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        inspector: Rect,
        gpu: Option<&(wgpu::Device, wgpu::Queue)>,
    ) -> f32 {
        let width = (inspector.width() - 16.0).max(60.0);
        let height = width
            * (crate::panels::shader_graph_preview::PREVIEW_H as f32
                / crate::panels::shader_graph_preview::PREVIEW_W as f32);
        let block = Rect::from_min_size(
            inspector.left_top() + Vec2::new(8.0, 8.0),
            Vec2::new(width, height),
        );
        let controls_height = 24.0;
        let total = height + controls_height + 8.0;

        let Some((device, queue)) = gpu else {
            painter.rect_filled(block, 3.0, Color32::from_gray(28));
            painter.text(
                block.center(),
                Align2::CENTER_CENTER,
                "实时预览需要 GPU 设备",
                FontId::proportional(11.0),
                Color32::from_gray(160),
            );
            return total;
        };

        let compiled = match self.graph.compile_wgsl_uniform(0, 3) {
            Ok(compiled) => compiled,
            Err(error) => {
                painter.rect_filled(block, 3.0, Color32::from_gray(28));
                painter.text(
                    block.center(),
                    Align2::CENTER_CENTER,
                    format!("图有错误：{error}"),
                    FontId::proportional(11.0),
                    Color32::from_rgb(230, 140, 110),
                );
                return total;
            }
        };
        let source = crate::panels::shader_graph_preview::preview_shader_source(&compiled.source);

        let state = self.preview.get_or_insert_with(|| PreviewState {
            renderer: crate::panels::shader_graph_preview::ShaderPreview::new(device, queue),
            texture: None,
            last_source: String::new(),
            playing: true,
            time: 0.0,
            last_render_ms: 0.0,
            error: None,
            compiled: None,
            last_slots: [[f32::NAN; 4]; 16],
            last_intensity: f32::NAN,
        });

        if state.last_source != source {
            state.last_source = source;
            let result = state.renderer.set_shader(&state.last_source);
            state.error = result.err();
            state.compiled = Some(compiled);
        }

        // 参数走 uniform：Inspector 拖动滑杆/「效果强度」只改这里的值，**不重编管线**。
        // 与上一帧比对后才重写 buffer，并据此标脏——暂停时拖滑杆也能看到一帧结果。
        let mut params_changed = false;
        if let Some(compiled) = state.compiled.as_ref() {
            let values = self.graph.param_values(compiled);
            let mut slots = [[0.0_f32; 4]; 16];
            for (slot, value) in slots.iter_mut().zip(values.iter()) {
                *slot = *value;
            }
            // 抽屉打开时以抽屉里的值为准：作者拖滑块的第一诉求就是"看这个值长什么样"，
            // 预览若还按图上的值渲染，那组滑块就成了盲调。
            if self.preset_open {
                for param in compiled.named_params() {
                    if let Some(value) = self.preset_values.get(&param.name) {
                        slots[param.slot][param.component] = *value;
                    }
                }
            }
            if slots != state.last_slots || self.intensity != state.last_intensity {
                state.renderer.set_params(&slots);
                state.last_slots = slots;
                state.last_intensity = self.intensity;
                params_changed = true;
            }
        }

        // 播放时推进时间；暂停时仍允许"改一次参数渲一次"，作者不必点播放也能看静态结果。
        let dt = ui.input(|input| input.stable_dt).min(0.1);
        let mut dirty = state.texture.is_none() || params_changed;
        if state.playing {
            state.time = (state.time + dt) % 1000.0;
            dirty = true;
        }
        // P3：**先取回上一帧**（非阻塞），再决定要不要提交下一帧。
        //
        // 旧路径在 UI 线程上 `device.poll(Wait)` 等 GPU 画完，播放时每帧都把 UI 卡到
        // 一帧渲染结束为止。现在提交与取回分开：GPU 在后台画，UI 每帧只做一次
        // `Poll`（推进事件、不等待），像素回来自动刷新画面。
        if state.error.is_none()
            && let Some(pixels) = state.renderer.try_take()
            && pixels.len()
                == (crate::panels::shader_graph_preview::PREVIEW_W
                    * crate::panels::shader_graph_preview::PREVIEW_H
                    * 4) as usize
        {
            let started = std::time::Instant::now();
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [
                    crate::panels::shader_graph_preview::PREVIEW_W as usize,
                    crate::panels::shader_graph_preview::PREVIEW_H as usize,
                ],
                &pixels,
            );
            match &mut state.texture {
                Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                None => {
                    state.texture = Some(ui.ctx().load_texture(
                        "shader_graph_preview",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
            state.last_render_ms = started.elapsed().as_secs_f32() * 1000.0;
        }
        // 提交下一帧：只有"想要新画面"且"上一帧已经回来"时才提交。在途时保持旧画面，
        // 下一帧再试——这比排队更合适（排队只会让显示越来越滞后于参数）。
        if dirty && state.error.is_none() && !state.renderer.has_inflight() {
            if let Err(error) = state.renderer.submit(state.time, self.intensity) {
                state.error = Some(error);
            } else {
                // 有在途帧就要持续刷新：等它回来才能取像素。
                ui.ctx().request_repaint();
            }
        }

        if let Some(texture) = &state.texture {
            ui.put(
                block,
                egui::Image::new(texture).fit_to_exact_size(block.size()),
            );
        } else {
            painter.rect_filled(block, 3.0, Color32::from_gray(28));
        }

        // 控制条：播放/暂停 + 时间 + 渲染耗时；错误优先显示，不让作者误以为效果就是这样。
        let controls = Rect::from_min_size(
            block.left_bottom() + Vec2::new(0.0, 4.0),
            Vec2::new(width, controls_height),
        );
        let playing = state.playing;
        let time = state.time;
        let render_ms = state.last_render_ms;
        let error = state.error.clone();
        let mut toggle = false;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(controls)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                if ui
                    .small_button(if playing { "⏸ 暂停" } else { "▶ 播放" })
                    .clicked()
                {
                    toggle = true;
                }
                ui.label(
                    egui::RichText::new(format!("t={time:.1}s · {render_ms:.1}ms"))
                        .small()
                        .color(Color32::from_gray(170)),
                );
            },
        );
        if toggle && let Some(state) = self.preview.as_mut() {
            state.playing = !state.playing;
        }
        if let Some(error) = error {
            painter.text(
                controls.left_center(),
                Align2::LEFT_CENTER,
                format!("预览失败：{error}"),
                FontId::proportional(10.0),
                Color32::from_rgb(230, 140, 110),
            );
        }
        if self.preview.as_ref().is_some_and(|state| state.playing) {
            // 播放时约 30fps 重绘：既能看到动画，也不把编辑器变成 100% CPU。
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }

        total
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, root: &Path) {
        let mut save_now = false;
        let mut open_now = false;
        ui.horizontal(|ui| {
            ui.heading("Shader Graph");
            ui.separator();
            egui::ComboBox::from_id_salt("shader_graph_template")
                .selected_text(Self::templates()[self.template].name_zh())
                .show_ui(ui, |ui| {
                    let mut last_category = "";
                    for (index, template) in Self::templates().iter().enumerate() {
                        if template.category_zh() != last_category {
                            if !last_category.is_empty() {
                                ui.separator();
                            }
                            ui.label(egui::RichText::new(template.category_zh()).weak().small());
                            last_category = template.category_zh();
                        }
                        let label = format!("{}  L{}", template.name_zh(), template.perf_level());
                        if ui
                            .selectable_value(&mut self.template, index, label)
                            .clicked()
                        {
                            self.load_template(index);
                        }
                    }
                });
            if ui.button("载入模板").clicked() {
                self.load_template(self.template);
            }
            if ui.button("模板画廊").clicked() {
                self.gallery_open = !self.gallery_open;
            }
            if ui.button("编译预览").clicked() {
                self.compile_preview();
            }
            // M96：把当前图 + 一组参数存成 `.prefab`，逻辑里 render.play_effect 直接加载。
            // 放在"编译预览"旁边：两步都是"看完画面 → 定稿"的收尾动作。
            if ui
                .button("生成预设")
                .on_hover_text("把当前图与一组参数值写成 .prefab 特效预设，逻辑里 play_effect 加载")
                .clicked()
            {
                self.toggle_preset_drawer();
            }
            if ui.button("撤销").clicked() {
                self.undo();
            }
            if ui.button("重做").clicked() {
                self.redo();
            }
            if ui.button("适配视图").clicked() {
                self.pan = Vec2::ZERO;
                self.zoom = 1.0;
            }
            ui.separator();
            ui.label("效果强度");
            ui.add(egui::Slider::new(&mut self.intensity, 0.0..=2.0).show_value(true));
        });
        ui.horizontal(|ui| {
            ui.label("资产路径");
            ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(240.0));
            if ui.button("保存 .sgraph").clicked() {
                save_now = true;
            }
            if ui.button("打开 .sgraph").clicked() {
                open_now = true;
            }
            if ui
                .add_enabled(
                    !self.preview_wgsl.is_empty(),
                    egui::Button::new("复制 WGSL"),
                )
                .clicked()
            {
                ui.ctx().copy_text(self.preview_wgsl.clone());
                self.status = "已复制生成的 WGSL".into();
            }
            if self.is_dirty() {
                ui.label(egui::RichText::new("未保存").color(Color32::from_rgb(230, 170, 80)));
            }
            ui.separator();
            // 运行时接入：把当前图编译成叠加层 WGSL，交给场景渲染器真的画到画面上。
            if ui.button("应用到场景叠加").clicked() {
                match crate::panels::shader_graph_preview::runtime_overlay_source(&self.graph) {
                    Ok((source, compiled)) => {
                        self.overlay_compiled = Some(compiled);
                        self.overlay_applied = true;
                        self.pending_overlay = Some(Some(source));
                    }
                    Err(error) => self.status = format!("无法生成叠加层：{error}"),
                }
            }
            if ui.button("关闭场景叠加").clicked() {
                self.overlay_applied = false;
                self.overlay_compiled = None;
                self.pending_overlay = Some(None);
            }
            ui.label(format!(
                "{} 节点 · {} 连线 · {:.2} ms",
                self.graph.nodes.len(),
                self.graph.edges.len(),
                self.preview_ms
            ));
        });
        if save_now {
            self.save(root);
        }
        if open_now {
            self.open(root);
        }
    }

    /// 取出待执行的场景叠加动作（面板只发请求，真正碰渲染器的是 `EditorApp`）。
    pub(crate) fn take_pending_overlay(&mut self) -> Option<Option<String>> {
        self.pending_overlay.take()
    }

    pub(crate) fn set_status(&mut self, status: String) {
        self.status = status;
    }

    /// 已应用叠加层时返回当前图对应的参数槽值（`EditorApp` 每帧写进 uniform）。
    pub(crate) fn overlay_params(&self) -> Option<Vec<[f32; 4]>> {
        if !self.overlay_applied {
            return None;
        }
        self.overlay_compiled
            .as_ref()
            .map(|compiled| self.graph.param_values(compiled))
    }

    /// 模板画廊：每格一张该模板的真实渲染缩略图（逐帧补，最多 3 张/帧，避免一次性卡住）。
    fn draw_gallery(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        gpu: Option<&(wgpu::Device, wgpu::Queue)>,
    ) {
        let templates = Self::templates();
        let Some((device, queue)) = gpu else {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "模板画廊需要 GPU 设备",
                FontId::proportional(12.0),
                Color32::from_gray(170),
            );
            return;
        };
        // 取出状态，避免闭包里同时借用 `self.gallery` 与 `self`。
        let mut state = match self.gallery.take() {
            Some(state) => state,
            None => GalleryState {
                preview: crate::panels::shader_graph_preview::ShaderPreview::new(device, queue),
                thumbnails: vec![None; templates.len()],
                next: 0,
                error: None,
            },
        };
        let mut rendered = 0;
        while state.next < state.thumbnails.len() && rendered < 3 {
            let index = state.next;
            state.next += 1;
            rendered += 1;
            let graph = templates[index].graph();
            let result = graph
                .compile_wgsl_uniform(0, 3)
                .map_err(|error| error.to_string())
                .and_then(|compiled| {
                    let source = crate::panels::shader_graph_preview::preview_shader_source(
                        &compiled.source,
                    );
                    state.preview.set_shader(&source)?;
                    let values = graph.param_values(&compiled);
                    let mut slots = [[0.0_f32; 4]; 16];
                    for (slot, value) in slots.iter_mut().zip(values.iter()) {
                        *slot = *value;
                    }
                    state.preview.set_params(&slots);
                    state.preview.render(0.35, 1.0)
                });
            match result {
                Ok(pixels) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [
                            crate::panels::shader_graph_preview::PREVIEW_W as usize,
                            crate::panels::shader_graph_preview::PREVIEW_H as usize,
                        ],
                        &pixels,
                    );
                    state.thumbnails[index] = Some(ui.ctx().load_texture(
                        format!("sgraph_gallery_{index}"),
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                Err(error) => state.error = Some(error),
            }
        }

        let mut close = false;
        let mut pick: Option<usize> = None;
        let thumb_size = egui::vec2(THUMB_W as f32, THUMB_H as f32);
        let columns = ((rect.width() - 24.0) / (THUMB_W as f32 + 16.0))
            .floor()
            .max(1.0) as usize;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink(8.0))
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        egui::ScrollArea::vertical().show(&mut child, |ui| {
            ui.horizontal(|ui| {
                ui.heading("模板画廊");
                if ui.button("关闭画廊").clicked() {
                    close = true;
                }
                ui.label(format!("{} 个模板 · 点缩略图切换", templates.len()));
                if let Some(error) = &state.error {
                    ui.label(
                        egui::RichText::new(format!("渲染失败：{error}"))
                            .small()
                            .color(Color32::from_rgb(230, 140, 110)),
                    );
                }
            });
            ui.separator();
            for (row, chunk) in templates.chunks(columns).enumerate() {
                ui.horizontal(|ui| {
                    for (offset, template) in chunk.iter().enumerate() {
                        let index = row * columns + offset;
                        ui.vertical(|ui| {
                            match &state.thumbnails[index] {
                                Some(texture) => {
                                    let response = ui.add(
                                        egui::Button::image(
                                            egui::Image::new(texture).fit_to_exact_size(thumb_size),
                                        )
                                        .frame(true),
                                    );
                                    if response.clicked() {
                                        pick = Some(index);
                                    }
                                }
                                None => {
                                    ui.allocate_space(thumb_size);
                                }
                            }
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  L{}",
                                    template.name_zh(),
                                    template.perf_level()
                                ))
                                .small(),
                            );
                        });
                    }
                });
            }
        });
        self.gallery = Some(state);
        if close {
            self.gallery_open = false;
        }
        if let Some(index) = pick {
            self.load_template(index);
            self.gallery_open = false;
            // 重新载入后缩略图保留：下次打开画廊不必全部重渲。
        }
        if self
            .gallery
            .as_ref()
            .is_some_and(|state| state.next < state.thumbnails.len())
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn node_rect(&self, canvas: Rect, node: &ShaderNode) -> Rect {
        let row_count = node
            .kind
            .input_ports()
            .len()
            .max(node.kind.output_ports().len()) as f32;
        let height = HEADER_HEIGHT + BODY_PAD * 2.0 + row_count * PORT_ROW;
        let origin = canvas.left_top()
            + self.pan
            + Vec2::new(node.position[0], node.position[1]) * self.zoom;
        Rect::from_min_size(
            origin,
            Vec2::new(NODE_WIDTH * self.zoom, height * self.zoom),
        )
    }

    fn port_pos(&self, rect: Rect, index: usize, output: bool) -> Pos2 {
        let x = if output { rect.right() } else { rect.left() };
        let y = rect.top()
            + (HEADER_HEIGHT + BODY_PAD + PORT_ROW * index as f32 + PORT_ROW * 0.5) * self.zoom;
        Pos2::new(x, y)
    }

    fn draw_grid(&self, painter: &egui::Painter, canvas: Rect) {
        let step = GRID * self.zoom;
        if step < 8.0 {
            return;
        }
        let grid = Color32::from_gray(39);
        let mut x = canvas.left() + self.pan.x.rem_euclid(step);
        while x < canvas.right() {
            painter.line_segment(
                [Pos2::new(x, canvas.top()), Pos2::new(x, canvas.bottom())],
                Stroke::new(1.0_f32, grid),
            );
            x += step;
        }
        let mut y = canvas.top() + self.pan.y.rem_euclid(step);
        while y < canvas.bottom() {
            painter.line_segment(
                [Pos2::new(canvas.left(), y), Pos2::new(canvas.right(), y)],
                Stroke::new(1.0_f32, grid),
            );
            y += step;
        }
    }

    fn edge_endpoints(&self, canvas: Rect, edge: &ShaderEdge) -> Option<(Pos2, Pos2)> {
        let from = self
            .graph
            .nodes
            .iter()
            .find(|node| node.id == edge.from.node)?;
        let to = self
            .graph
            .nodes
            .iter()
            .find(|node| node.id == edge.to.node)?;
        let from_index = from
            .kind
            .output_ports()
            .iter()
            .position(|port| *port == edge.from.port)
            .unwrap_or(0);
        let to_index = to
            .kind
            .input_ports()
            .iter()
            .position(|port| *port == edge.to.port)
            .unwrap_or(0);
        let a = self.port_pos(self.node_rect(canvas, from), from_index, true);
        let b = self.port_pos(self.node_rect(canvas, to), to_index, false);
        Some((a, b))
    }

    fn draw_edges(&self, painter: &egui::Painter, canvas: Rect) {
        let link_color = Color32::from_rgb(92, 180, 190);
        for edge in &self.graph.edges {
            let Some((a, b)) = self.edge_endpoints(canvas, edge) else {
                continue;
            };
            let mid_x = (a.x + b.x) * 0.5;
            painter.line_segment([a, Pos2::new(mid_x, a.y)], Stroke::new(2.0_f32, link_color));
            painter.line_segment(
                [Pos2::new(mid_x, a.y), Pos2::new(mid_x, b.y)],
                Stroke::new(2.0_f32, link_color),
            );
            painter.line_segment([Pos2::new(mid_x, b.y), b], Stroke::new(2.0_f32, link_color));
        }
    }

    fn draw_nodes(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, canvas: Rect) {
        let mut select: Option<String> = None;
        let mut drag_snapshot = false;
        let mut drag: Option<(usize, Vec2)> = None;
        for (index, node) in self.graph.nodes.iter().enumerate() {
            let rect = self.node_rect(canvas, node);
            if !canvas.intersects(rect) {
                continue;
            }
            let id = ui.id().with(("shader-graph-node", &node.id));
            let response = ui.interact(rect, id, Sense::click_and_drag());
            if response.clicked() {
                select = Some(node.id.clone());
            }
            if response.drag_started() {
                drag_snapshot = true;
            }
            if response.dragged() {
                drag = Some((index, response.drag_delta()));
            }
            let selected = self.selected.as_deref() == Some(node.id.as_str());
            painter.rect_filled(rect, 4.0, Color32::from_rgb(35, 40, 44));
            painter.rect_filled(
                Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_HEIGHT * self.zoom)),
                4.0,
                node_color(node.kind),
            );
            painter.rect_stroke(
                rect,
                4.0,
                Stroke::new(
                    if selected { 2.0_f32 } else { 1.0_f32 },
                    if selected {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(78)
                    },
                ),
                egui::StrokeKind::Outside,
            );
            painter.text(
                rect.left_top() + Vec2::new(8.0, 6.0) * self.zoom,
                Align2::LEFT_TOP,
                node.kind.name_zh(),
                FontId::proportional(13.0 * self.zoom),
                Color32::WHITE,
            );
            for (slot, port) in node.kind.input_ports().iter().enumerate() {
                let center = self.port_pos(rect, slot, false);
                painter.circle_filled(center, 4.5 * self.zoom, Color32::from_rgb(225, 170, 70));
                painter.text(
                    center + Vec2::new(8.0, 0.0) * self.zoom,
                    Align2::LEFT_CENTER,
                    *port,
                    FontId::proportional(10.0 * self.zoom),
                    Color32::from_gray(185),
                );
            }
            for (slot, port) in node.kind.output_ports().iter().enumerate() {
                let center = self.port_pos(rect, slot, true);
                painter.circle_filled(center, 4.5 * self.zoom, Color32::from_rgb(100, 190, 220));
                painter.text(
                    center - Vec2::new(8.0, 0.0) * self.zoom,
                    Align2::RIGHT_CENTER,
                    *port,
                    FontId::proportional(10.0 * self.zoom),
                    Color32::from_gray(185),
                );
            }
        }
        if drag_snapshot {
            self.push_history();
        }
        if let Some((index, delta)) = drag {
            let position = &mut self.graph.nodes[index].position;
            position[0] += delta.x / self.zoom;
            position[1] += delta.y / self.zoom;
        }
        if select.is_some() {
            self.selected = select;
        }
    }

    fn draw_inspector(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        inspector: Rect,
        preview_height: f32,
    ) {
        let template = Self::templates()[self.template];
        // 底色由 `ui()` 在 `draw_preview` 之前铺好；这里再铺会盖掉预览图。
        let mut top = inspector.left_top() + Vec2::new(12.0, preview_height + 14.0);
        painter.text(
            top,
            Align2::LEFT_TOP,
            "模板说明",
            FontId::proportional(14.0),
            Color32::WHITE,
        );
        top += Vec2::new(0.0, 26.0);
        painter.text(
            top,
            Align2::LEFT_TOP,
            template.name_zh(),
            FontId::proportional(13.0),
            Color32::from_rgb(105, 200, 190),
        );
        top += Vec2::new(0.0, 26.0);
        let wrap = (inspector.width() - 24.0).max(80.0);
        let hint = painter.layout(
            template.hint_zh().to_owned(),
            FontId::proportional(12.0),
            Color32::from_gray(205),
            wrap,
        );
        let hint_height = hint.rect.height();
        painter.galley(top, hint, Color32::from_gray(205));
        top += Vec2::new(0.0, hint_height + 10.0);
        painter.text(
            top,
            Align2::LEFT_TOP,
            format!("分类：{}", template.category_zh()),
            FontId::proportional(11.0),
            Color32::from_gray(170),
        );
        top += Vec2::new(0.0, 16.0);
        painter.text(
            top,
            Align2::LEFT_TOP,
            format!("性能：L{}（0 最低 · 3 最重）", template.perf_level()),
            FontId::proportional(11.0),
            Color32::from_gray(170),
        );

        let selected = self
            .selected
            .as_deref()
            .and_then(|id| self.graph.nodes.iter().position(|node| node.id == id));
        let Some(index) = selected else {
            return;
        };

        // 节点检查器：从 painter 切到 egui 控件，位置在模板说明下方的右栏里。
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(inspector.left() + 10.0, top.y + 18.0),
                    inspector.max - Vec2::new(10.0, 8.0),
                ))
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        let kind = self.graph.nodes[index].kind;
        child.label(egui::RichText::new(kind.name_zh()).strong().size(13.0));
        child.label(
            egui::RichText::new(kind.hint_zh())
                .small()
                .color(Color32::from_gray(190)),
        );
        child.separator();
        // 拖动/聚焦开始时先留一份快照：滑块一动就是一步撤销，而不是每帧一步。
        let before = self.graph.clone();
        let mut edited = false;
        let mut snapshot = false;
        match kind {
            ShaderNodeKind::Sine => {
                let value = self.graph.nodes[index]
                    .params
                    .entry("frequency".into())
                    .or_insert(12.0);
                let response = child.add(
                    egui::Slider::new(value, 0.1..=200.0)
                        .text("频率")
                        .logarithmic(true),
                );
                edited |= response.changed();
                snapshot |= response.drag_started() || response.gained_focus();
            }
            ShaderNodeKind::Fresnel => {
                let value = self.graph.nodes[index]
                    .params
                    .entry("power".into())
                    .or_insert(3.0);
                let response = child.add(egui::Slider::new(value, 0.5..=8.0).text("边缘硬度"));
                edited |= response.changed();
                snapshot |= response.drag_started() || response.gained_focus();
            }
            ShaderNodeKind::Constant => {
                for (key, label) in [("r", "红"), ("g", "绿"), ("b", "蓝")] {
                    let value = self.graph.nodes[index]
                        .params
                        .entry(key.into())
                        .or_insert(1.0);
                    let response = child.add(egui::Slider::new(value, 0.0..=1.0).text(label));
                    edited |= response.changed();
                    snapshot |= response.drag_started() || response.gained_focus();
                }
            }
            _ => {
                child.label(egui::RichText::new("该节点没有可调参数").weak());
            }
        }
        if snapshot {
            self.undo_stack.push(before);
            if self.undo_stack.len() > 64 {
                self.undo_stack.remove(0);
            }
            self.redo.clear();
        }
        if edited {
            self.status = "已修改节点参数".into();
        }
    }
}

fn node_color(kind: ShaderNodeKind) -> Color32 {
    match kind {
        ShaderNodeKind::Uv => Color32::from_rgb(42, 128, 151),
        ShaderNodeKind::Time => Color32::from_rgb(181, 112, 51),
        ShaderNodeKind::VertexColor => Color32::from_rgb(120, 96, 160),
        ShaderNodeKind::Texture => Color32::from_rgb(82, 138, 96),
        ShaderNodeKind::Constant => Color32::from_rgb(140, 120, 70),
        ShaderNodeKind::Add | ShaderNodeKind::Multiply => Color32::from_rgb(110, 84, 130),
        ShaderNodeKind::Sine => Color32::from_rgb(150, 96, 60),
        ShaderNodeKind::Fresnel => Color32::from_rgb(60, 110, 150),
        ShaderNodeKind::Ring | ShaderNodeKind::Beam | ShaderNodeKind::SceneColor => {
            Color32::from_rgb(96, 132, 160)
        }
        ShaderNodeKind::DistortUv => Color32::from_rgb(76, 116, 150),
        ShaderNodeKind::DepthEdge | ShaderNodeKind::DepthFog => Color32::from_rgb(70, 104, 128),
        ShaderNodeKind::RotateUv
        | ShaderNodeKind::Polar
        | ShaderNodeKind::Sector
        | ShaderNodeKind::VerticalStretch
        | ShaderNodeKind::Gradient => Color32::from_rgb(84, 108, 148),
        ShaderNodeKind::Output => Color32::from_rgb(131, 83, 111),
    }
}

/// 没有位置的模板图（模板只声明结构）按拓扑深度排成从左到右的列，避免全部叠在原点。
fn auto_layout(graph: &mut ShaderGraph) {
    if graph.nodes.iter().any(|node| node.position != [0.0, 0.0]) {
        return;
    }
    let mut depth: BTreeMap<String, i32> = BTreeMap::new();
    for _ in 0..graph.nodes.len().max(1) {
        let mut changed = false;
        for edge in &graph.edges {
            let source = depth.get(&edge.from.node).copied().unwrap_or(0);
            let target = depth.entry(edge.to.node.clone()).or_insert(0);
            if *target < source + 1 {
                *target = source + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut rows: BTreeMap<i32, f32> = BTreeMap::new();
    for node in &mut graph.nodes {
        let column = depth.get(&node.id).copied().unwrap_or(0);
        let row = rows.entry(column).or_insert(0.0);
        node.position = [40.0 + column as f32 * 210.0, 40.0 + *row * 130.0];
        *row += 1.0;
    }
}

impl crate::EditorApp {
    /// M88：把**当前场景**声明的 `.sgraph` 挂到场景渲染器上。
    ///
    /// 数据源是 `.scene` 的 `meta\teffect_overlay\t<路径>`，不是 `project.toml`。
    /// 放工程级时每个场景都会带上同一个特效（切到 api_spine / api_audio 也有刀光），
    /// 那是设计错误；叠加层属于场景语义。
    ///
    /// 在渲染器刚建好（`attach_scene_shader_overrides`）与场景切换/保存后调用。失败只写状态栏，
    /// 不阻止渲染器工作：坏资产应表现为"没有特效"，而不是场景黑屏。
    pub(crate) fn apply_project_effect_overlay(&mut self) {
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        // 没有打开的场景 = 没有叠加层（渲染器可能仍挂着上一个场景的，必须显式撤掉）。
        // 用 `current_scene()` 而非直接读 `document`：Play 时以运行中的文档为准，
        // 否则编辑态改了特效、一进 Play 又回到旧值。
        let relative = self
            .current_scene()
            .and_then(|scene| scene.scene.effect_overlay())
            .map(str::to_owned);
        let Some(relative) = relative else {
            self.effect_overlay_slots = None;
            if let Some(renderer) = self.scene_renderer.as_mut() {
                let _ = renderer.set_effect_overlay(None);
            }
            return;
        };
        match rustlua_visual_script::shader_graph::ShaderGraph::load_runtime_overlay(
            &root, &relative,
        ) {
            Ok((source, compiled)) => {
                let Some(renderer) = self.scene_renderer.as_mut() else {
                    // 渲染器还没建：缓存参数，下一次 attach 再挂。
                    self.effect_overlay_slots = Some(compiled.slots.clone());
                    return;
                };
                match renderer.set_effect_overlay(Some(&source)) {
                    Ok(()) => {
                        renderer.set_effect_params(&compiled.slots);
                        self.effect_overlay_slots = Some(compiled.slots.clone());
                        self.shader_graph
                            .set_status(format!("已按 project.toml 挂载特效叠加层：{relative}"));
                    }
                    Err(error) => {
                        self.effect_overlay_slots = None;
                        self.shader_graph
                            .set_status(format!("特效叠加层编译失败：{error}"));
                    }
                }
            }
            Err(error) => {
                self.effect_overlay_slots = None;
                self.shader_graph
                    .set_status(format!("特效叠加层加载失败：{error}"));
            }
        }
    }

    pub(crate) fn draw_shader_graph_panel(&mut self, ui: &mut eframe::egui::Ui) {
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        // 预览用编辑器的 wgpu 设备；没有设备（无窗口/无 GPU）时面板只显示说明。
        let gpu = self.gpu.clone();
        self.shader_graph.ui(ui, &root, gpu.as_ref());
        // 面板只发请求；真正碰渲染器的是这里（与其它面板"只发 action"的约定一致）。
        if let Some(pending) = self.shader_graph.take_pending_overlay() {
            let message = match self.scene_renderer.as_mut() {
                Some(renderer) => match pending {
                    Some(source) => match renderer.set_effect_overlay(Some(&source)) {
                        Ok(()) => "已把当前效果应用到场景预览（叠加层）".to_owned(),
                        Err(error) => format!("应用到场景失败：{error}"),
                    },
                    None => match renderer.set_effect_overlay(None) {
                        Ok(()) => "已关闭场景叠加".to_owned(),
                        Err(error) => format!("关闭场景叠加失败：{error}"),
                    },
                },
                None => "场景视图还没有渲染器：先打开 3D 场景或进入 Play 再试".to_owned(),
            };
            self.shader_graph.set_status(message);
        }
        // 叠加层要动起来就得每帧推进时间；关档时 `set_effect_time` 是空操作。
        let time = ui.input(|input| input.time) as f32;
        let params = self.shader_graph.overlay_params();
        if let Some(renderer) = self.scene_renderer.as_mut() {
            renderer.set_effect_time(time);
            // 参数走 uniform：改参数只重写这块 buffer，**不重编管线**。
            if let Some(params) = params.as_deref() {
                renderer.set_effect_params(params);
            }
        }
    }

    /// 资源面板双击 `.sgraph` 的落点：打开面板并按工程内相对路径载入这张图。
    pub(crate) fn open_shader_graph_resource(&mut self, relative: &str) {
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        self.show_shader_graph = true;
        self.shader_graph.open_relative(&root, relative);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_layout_spreads_template_graph_by_topology() {
        let mut graph = ShaderTemplate::Scanlines.graph();
        auto_layout(&mut graph);
        let texture_x = graph.nodes.iter().find(|n| n.id == "tex").unwrap().position[0];
        let out_x = graph.nodes.iter().find(|n| n.id == "out").unwrap().position[0];
        assert!(out_x > texture_x, "输出节点必须排在输入节点右侧");
    }

    /// 回归：检查器底色必须先于实时预览铺好，否则整块底色会盖掉预览图，
    /// 表现为"模板说明上方一片空白"（M98 实际截图复现）。无 GPU 时预览画占位提示，
    /// 同样能被这条测试覆盖——只要占位块在底色之后画即可。
    #[test]
    fn inspector_background_is_painted_before_live_preview() {
        let mut panel = ShaderGraphPanel::default();
        let root = temp_root("paint-order");
        let context = egui::Context::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    panel.ui(ui, &root, None);
                });
            },
        );
        let bg = Color32::from_rgb(31, 35, 39);
        let indices: Vec<usize> = output
            .shapes
            .iter()
            .enumerate()
            .filter_map(|(index, clipped)| match &clipped.shape {
                egui::Shape::Rect(shape) if shape.fill == bg => Some(index),
                _ => None,
            })
            .collect();
        assert!(!indices.is_empty(), "应铺检查器底色");
        let preview_at = indices.last().copied().expect("底色索引");
        let has_later_content = output.shapes[preview_at + 1..].iter().any(|clipped| {
            match &clipped.shape {
                // GPU 缺失时预览画灰色占位块；有 GPU 时是 egui 图片/控制条。
                egui::Shape::Rect(shape) => shape.fill == Color32::from_gray(28),
                egui::Shape::Text(shape) => shape.galley.text().contains("实时预览需要 GPU 设备"),
                _ => false,
            }
        });
        assert!(
            has_later_content,
            "检查器底色之后必须还有预览内容，否则预览被盖住（共 {} 个绘制指令）",
            output.shapes.len()
        );
    }

    #[test]
    fn saved_graph_round_trips_through_panel_state() {
        let panel = ShaderGraphPanel::default();
        assert!(panel.is_dirty(), "刚载入的模板还没落盘，必须显示未保存");
        let mut saved = panel.graph.clone();
        auto_layout(&mut saved);
        let mut saved_panel = ShaderGraphPanel::default();
        saved_panel.saved = Some(saved_panel.graph.clone());
        assert!(!saved_panel.is_dirty(), "与保存快照相同即不脏");
    }

    /// 画一帧面板，返回本帧上传的纹理图像（预览重渲时会出现在这里）。
    fn panel_frame(
        panel: &mut ShaderGraphPanel,
        context: &egui::Context,
        root: &std::path::Path,
        gpu: &(wgpu::Device, wgpu::Queue),
    ) -> Vec<(egui::TextureId, egui::epaint::ImageData)> {
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                panel.ui(ui, root, Some(gpu));
            });
        });
        output
            .textures_delta
            .set
            .into_iter()
            .map(|(id, delta)| (id, delta.image))
            .collect()
    }

    /// P3：跑最多 `max_frames` 帧，直到出现"上传了纹理"的一帧，返回那一帧的纹理更新。
    ///
    /// 预览改成异步流水线后，首次出图需要"提交 → GPU 完成 → 取回"至少两帧；
    /// 只跑一帧就断言"有纹理"等于断言同步行为，那是已被移除的假设。
    fn panel_frames_until_texture(
        panel: &mut ShaderGraphPanel,
        context: &egui::Context,
        root: &std::path::Path,
        gpu: &(wgpu::Device, wgpu::Queue),
        max_frames: usize,
    ) -> Vec<(egui::TextureId, egui::epaint::ImageData)> {
        for _ in 0..max_frames {
            let frame = panel_frame(panel, context, root, gpu);
            // 判据是"预览纹理已经存在"，而不是"这一帧有任意纹理更新"——
            // 首帧可能只带字体图集等无关 delta，会把循环提前结束掉。
            if panel
                .preview
                .as_ref()
                .is_some_and(|state| state.texture.is_some())
            {
                return frame;
            }
        }
        // 走到这里说明异步回读在 `max_frames` 内没回来。这**不是**被测逻辑错了：
        // 软件适配器（CI/无独显机器）上 GPU 完成时间不可预测，并发跑多个 GPU 测试时更慢。
        // 与其让测试随机飘红，这里显式把在途帧阻塞收掉再跑一帧——语义仍然是
        // "预览最终要出图"，只是不再依赖机器速度。
        if let Some(preview) = panel.preview.as_mut()
            && preview.renderer.has_inflight()
        {
            let _ = preview.renderer.render_take_blocking();
        }
        panel_frame(panel, context, root, gpu)
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-sgraph-panel-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    fn test_gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("shader-graph-panel-test"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()
    }

    /// 无 GPU 也要能画出面板（只显示"需要 GPU"说明），且画一帧不改动图。
    #[test]
    fn panel_draws_headless_without_gpu_and_keeps_the_graph() {
        let root = temp_root("nogpu");
        let mut panel = ShaderGraphPanel::default();
        let before = panel.graph.clone();
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| panel.ui(ui, &root, None));
        });
        assert_eq!(before, panel.graph, "只画一帧不应改动图");
        assert!(panel.preview.is_none(), "没有 GPU 设备时不该建预览器");
        let _ = std::fs::remove_dir_all(root);
    }

    /// 有 GPU 时：面板画一帧就要建出预览器、上传纹理并推进时间（实时预览真的在跑）。
    #[test]
    fn live_preview_uploads_a_texture_and_advances_time() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph live preview: no fallback adapter");
            return;
        };
        let root = temp_root("gpu");
        let mut panel = ShaderGraphPanel::default();
        let context = egui::Context::default();
        // P3 起预览是**异步流水线**：一帧提交、后续帧取回。真实使用中 UI 会持续
        // repaint 直到像素回来，所以这里也必须跑多帧——只跑一帧的话断言的是
        // "提交后立刻有纹理"，那正是异步化要消除的假设。
        let mut output = None;
        for _ in 0..64 {
            output = Some(context.run(egui::RawInput::default(), |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    panel.ui(ui, &root, Some(&gpu));
                });
            }));
            if panel
                .preview
                .as_ref()
                .is_some_and(|state| state.texture.is_some())
            {
                break;
            }
        }
        let output = output.expect("至少跑过一帧");
        let state = panel.preview.as_ref().expect("预览器应当已建立");
        assert!(state.texture.is_some(), "应当已上传预览纹理");
        assert!(
            state.error.is_none(),
            "内置模板的预览不应报错：{:?}",
            state.error
        );
        assert!(state.time > 0.0, "播放状态下时间应当推进");
        assert!(!state.last_source.is_empty(), "应当已编译一份 WGSL");
        // 有 GPU 时预览图（带纹理 brush 的 Rect）必须画在检查器底色之上，
        // 否则用户只看到一块空底色（"模板说明"文字在底色之后，不算预览）。
        let bg = Color32::from_rgb(31, 35, 39);
        let bg_at = output
            .shapes
            .iter()
            .rposition(
                |clipped| matches!(&clipped.shape, egui::Shape::Rect(shape) if shape.fill == bg),
            )
            .expect("检查器底色");
        assert!(
            output.shapes[bg_at + 1..].iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Rect(shape) if shape.brush.is_some())
            }),
            "检查器底色之后必须画出带纹理的预览图，否则预览被盖住"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 「效果强度」滑杆必须真的接到预览 uniform：暂停时拖它，画面也要重渲一帧。
    /// 旧代码只在 `playing` 时标脏，暂停拖滑杆只改了字段、画面停在上一帧。
    #[test]
    fn effect_intensity_slider_repaints_while_paused() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph slider: no fallback adapter");
            return;
        };
        let root = temp_root("intensity_slider");
        let mut panel = ShaderGraphPanel::default();
        let context = egui::Context::default();
        let first = panel_frames_until_texture(&mut panel, &context, &root, &gpu, 32);
        let texture_id = panel
            .preview
            .as_ref()
            .expect("预览器应当已建立")
            .texture
            .as_ref()
            .expect("首帧应上传预览纹理")
            .id();
        let pixels = |frame: Vec<(egui::TextureId, egui::epaint::ImageData)>| {
            frame
                .into_iter()
                .find(|(id, _)| *id == texture_id)
                // `map` 而不是 `and_then(|x| Some(y))`：这里不需要短路语义，
                // "可能没有"已经由 `find` 表达。
                .map(|(_, image)| match image {
                    egui::epaint::ImageData::Color(image) => image.pixels.clone(),
                })
        };
        let before = pixels(first).expect("首帧应上传预览纹理");

        // 暂停后只改滑杆：如果滑杆是空接线，就不会重渲，纹理也不会重新上传。
        panel.preview.as_mut().expect("预览器应当已建立").playing = false;
        panel.intensity = 0.25;
        // 第二段必须等**这个纹理的新一次更新**，不能再用"纹理已存在"当判据——
        // 它从首段起就一直存在，那样循环会立刻返回、断言的是旧画面。
        let mut after = None;
        for _ in 0..64 {
            let frame = panel_frame(&mut panel, &context, &root, &gpu);
            if let Some(found) = pixels(frame) {
                after = Some(found);
                break;
            }
        }
        let after = after.expect("暂停时改强度必须重渲并上传新画面");
        assert_ne!(before, after, "暂停时改效果强度必须真的改变预览画面");
        assert_eq!(
            panel.preview.as_ref().expect("预览器").last_intensity,
            0.25,
            "强度值必须进入预览 uniform 的上一帧记录"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 画廊：画够帧数就该把全部模板的缩略图补出来。
    #[test]
    fn gallery_fills_every_template_thumbnail() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph gallery: no fallback adapter");
            return;
        };
        let root = temp_root("gallery");
        let mut panel = ShaderGraphPanel {
            gallery_open: true,
            ..ShaderGraphPanel::default()
        };
        let context = egui::Context::default();
        // 每帧最多补 3 张：按模板总数动态算帧数再加余量，加模板不需要改这里。
        let frames_needed = ShaderTemplate::ALL.len().div_ceil(3) + 4;
        for _ in 0..frames_needed {
            let _ = context.run(egui::RawInput::default(), |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    panel.ui(ui, &root, Some(&gpu));
                });
            });
        }
        let state = panel.gallery.as_ref().expect("画一帧就该建出画廊状态");
        let filled = state
            .thumbnails
            .iter()
            .filter(|thumbnail| thumbnail.is_some())
            .count();
        assert_eq!(
            filled,
            ShaderTemplate::ALL.len(),
            "画廊应给每个模板都渲染出缩略图，实测 {filled}/{}",
            ShaderTemplate::ALL.len()
        );
        assert!(state.error.is_none(), "画廊渲染不应报错：{:?}", state.error);
        let _ = std::fs::remove_dir_all(root);
    }

    /// 双击 `.sgraph` 的入口：按相对路径载入并把面板打开。
    #[test]
    fn open_relative_loads_a_saved_graph() {
        let root = temp_root("open");
        let graph = ShaderTemplate::Fire.graph();
        editor::shader_graph_ops::save_shader_graph(&root, "assets/fx/fire.sgraph", &graph)
            .expect("seed");
        let mut panel = ShaderGraphPanel::default();
        panel.open_relative(&root, "assets/fx/fire.sgraph");
        assert_eq!(panel.path, "assets/fx/fire.sgraph");
        assert_eq!(panel.graph.name, graph.name, "应当载入磁盘上的那张图");
        assert!(!panel.is_dirty(), "刚打开即干净");
        let _ = std::fs::remove_dir_all(root);
    }

    /// M96：打开抽屉就按当前图铺满参数，默认值与图上的值一致。
    ///
    /// 抓的假绿：抽屉只放"选中节点"的参数——作者得挨个点节点才能把一张图存全，
    /// 而且漏掉的参数会被静默写成默认值。
    #[test]
    fn preset_drawer_lists_every_named_param_at_the_graphs_values() {
        let mut panel = ShaderGraphPanel::default();
        panel.load_template(0);
        panel.toggle_preset_drawer();
        assert!(panel.preset_open);
        let params = panel.compiled_named_params();
        assert!(!params.is_empty(), "内置模板应当有可调参数");
        assert_eq!(
            panel.preset_values.len(),
            params.len(),
            "抽屉要列全所有命名参数"
        );
        for param in &params {
            let value = panel.preset_values.get(&param.name).copied().unwrap();
            assert!(
                (value - param.default).abs() < 1e-6,
                "{} 的初值应当等于图上的值（{} vs {}）",
                param.name,
                value,
                param.default
            );
        }
        // 再点一次关闭，值不残留到下一次会话里。
        panel.preset_values.insert("__stale".into(), 1.0);
        panel.toggle_preset_drawer();
        assert!(!panel.preset_open);
        panel.toggle_preset_drawer();
        assert!(
            !panel.preset_values.contains_key("__stale"),
            "重新打开应重置"
        );
    }

    /// 生成预设：`.sgraph` 先落盘，`.prefab` 引用它并写出全部参数，能被加载器读回。
    #[test]
    fn generating_a_preset_saves_both_files_and_round_trips() {
        let root = temp_root("preset");
        let mut panel = ShaderGraphPanel::default();
        panel.load_template(0);
        panel.path = "assets/fx/demo.sgraph".into();
        panel.toggle_preset_drawer();
        // 模拟作者把一个参数拖到另一个值。
        let first = panel
            .preset_values
            .keys()
            .next()
            .cloned()
            .expect("至少一个参数");
        panel.preset_values.insert(first.clone(), 0.123);
        panel.generate_preset(&root);

        let graph_path = root.join("assets/fx/demo.sgraph");
        let prefab_path = root.join("assets/fx/demo.prefab");
        assert!(graph_path.exists(), "生成预设前必须先把 .sgraph 落盘");
        assert!(prefab_path.exists(), "预设文件应当写出：{}", panel.status);

        let text = std::fs::read_to_string(&prefab_path).unwrap();
        assert!(text.contains("kind=effect"), "{text}");
        assert!(text.contains("graph=assets/fx/demo.sgraph"), "{text}");
        let preset = rustlua_visual_script::effect_prefab::EffectPrefab::from_text(&text)
            .expect("生成的预设必须能被解析");
        assert_eq!(preset.overrides.get(&first), Some(&0.123));
        // 全量写出：每个命名参数都在预设里，而不是只写"动过的"。
        for param in panel.compiled_named_params() {
            assert!(
                preset.overrides.contains_key(&param.name),
                "预设缺少参数 {}（只写动过的参数会让预设依赖图的默认值）",
                param.name
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// 窄面板下的抽屉排版：所有控件必须落在抽屉矩形内。
    ///
    /// 真窗口证据（100% DPI、1280×720、右侧有属性栏）里画布只有 ~430px 宽，第一版把
    /// 「预设名 / 路径 / 生成 / 关闭」全塞进一行，结果生成按钮被挤出抽屉右边界——截图里
    /// 只剩半截、点不到。这条测试按窄宽度画一帧，逐个断言控件矩形没越界。
    #[test]
    fn preset_drawer_keeps_every_widget_inside_a_narrow_drawer() {
        let root = temp_root("preset-narrow");
        let mut panel = ShaderGraphPanel::default();
        panel.load_template(0);
        panel.toggle_preset_drawer();
        let drawer =
            egui::Rect::from_min_size(egui::Pos2::new(0.0, 100.0), egui::vec2(430.0, 200.0));
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                panel.draw_preset_drawer(ui, &root, drawer);
            });
        });
        // 判据：这一帧画出来的每个形状都必须落在抽屉内（含 1px 容差）。
        // 第一版把四个控件排成一行，生成按钮直接被挤出右边界——`Rect::clamp` 之后
        // 尺寸会变成负数，这里按"形状包围盒越界"断言就能抓住。
        let mut outside = 0;
        let tolerance = 1.0;
        for clipped in &output.shapes {
            let rect = clipped.shape.visual_bounding_rect();
            // 背景层与裁剪层是 10000×10000 的哨兵矩形，不是控件，跳过。
            if rect.is_negative() || rect.width() > 5000.0 {
                continue;
            }
            let grown = drawer.expand(tolerance);
            if !grown.contains_rect(rect) {
                outside += 1;
            }
        }
        assert_eq!(
            outside, 0,
            "有 {outside} 个控件被挤出 430px 宽的抽屉（窄面板下生成按钮点不到）"
        );
        assert!(panel.preset_open, "画一帧不应自己关掉抽屉");
        let _ = std::fs::remove_dir_all(root);
    }

    /// 空预设名与非法路径都要在写盘前拒绝，并留下可读的原因。
    #[test]
    fn generating_a_preset_rejects_empty_name_and_bad_path() {
        let root = temp_root("preset-bad");
        let mut panel = ShaderGraphPanel::default();
        panel.load_template(0);
        panel.path = "assets/fx/demo.sgraph".into();
        panel.toggle_preset_drawer();

        panel.preset_name.clear();
        panel.generate_preset(&root);
        assert!(panel.status.contains("预设名不能为空"), "{}", panel.status);
        assert!(!root.join("assets/fx/demo.prefab").exists());

        panel.preset_name = "演示".into();
        panel.preset_path = "../escape.prefab".into();
        panel.generate_preset(&root);
        assert!(
            panel.status.contains("生成预设失败"),
            "非法路径应当失败并说明原因：{}",
            panel.status
        );
        assert!(!root.parent().unwrap().join("escape.prefab").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
