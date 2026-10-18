//! 图集面板：`atlas.json` 的配置编辑 + 打页结果的可视化预览。
//!
//! 三条约束（都来自已经踩过的坑）：
//! 1. **只预览、不改产物**：本面板绝不写 `.rltex`/`.atlas`、绝不删源图——那是
//!    `assetc::atlas_cook` 的职责（发布期才发生）。面板只写 `atlas.json`。
//! 2. **归约规则只有一份**：配置→任务→装箱全部走 `rustlua_assets::atlas_plan`，
//!    与发布期同一个函数。否则"编辑器里看到 14 页、打出来 11 页"这类漂移迟早发生
//!    （`docs/67` §8 第 4 项）。
//! 3. **像素只在选中页时合成**：页的像素是图集的真实成本（一页可达几十 MB），
//!    列表只显示尺寸/区域数；选中后才 `render_page_rgba` 并上传一张纹理。

use std::path::{Path, PathBuf};

use eframe::egui;
use rustlua_assets::atlas_plan::{self, AtlasConfig, AtlasSpec, AutoAtlas, PlannedPage};

use crate::EditorApp;

/// 图集面板状态（挂在 `EditorApp` 上，切换工程时重置）。
pub(crate) struct AtlasPanelState {
    pub show_panel: bool,
    /// 当前编辑中的配置（唯一真源；保存时整体写盘）。
    pub config: AtlasConfig,
    /// 是否有未保存改动（整份比较得出，不给每个控件挂脏标记）。
    pub dirty: bool,
    pub message: String,
    /// `separate` 输入框。
    pub separate_input: String,
    /// 最近一次扫描结果（配置改动后失效）。
    pub preview: Option<AtlasPreview>,
    pub selected_page: usize,
    pub zoom: f32,
    /// 缩放跟随窗口宽度（大页可关掉后滚动）。
    pub zoom_follow: bool,
    /// 选中页的 GPU 纹理；`(页号, 扫描代次)` 变了才重建。
    pub texture: Option<egui::TextureHandle>,
    pub texture_key: Option<(usize, u64)>,
    pub generation: u64,
}

impl AtlasPanelState {
    pub fn new() -> Self {
        Self {
            show_panel: false,
            config: AtlasConfig::default(),
            dirty: false,
            message: String::new(),
            separate_input: String::new(),
            preview: None,
            selected_page: 0,
            zoom: 1.0,
            zoom_follow: true,
            texture: None,
            texture_key: None,
            generation: 0,
        }
    }

    /// 切换/重新打开工程：从 `<root>/atlas.json` 重新读，没有就是空配置。
    pub fn reseed(&mut self, root: &Path) {
        self.config = AtlasConfig::load(&root.join(AtlasConfig::FILE))
            .ok()
            .flatten()
            .unwrap_or_default();
        self.dirty = false;
        self.message.clear();
        self.separate_input.clear();
        self.preview = None;
        self.texture = None;
        self.texture_key = None;
        self.selected_page = 0;
    }

    /// 扫描工程并重新装箱（不写盘、不删源图）。
    pub fn scan(&mut self, root: &Path) -> anyhow::Result<()> {
        let files = atlas_plan::collect_png_files(root, "assets")?;
        let excluded = atlas_plan::collect_excluded(root)?;
        let set = atlas_plan::plan_pages(&self.config, &files, &excluded)?;
        self.generation = self.generation.wrapping_add(1);
        self.texture = None;
        self.texture_key = None;
        self.selected_page = self.selected_page.min(set.pages.len().saturating_sub(1));
        self.preview = Some(AtlasPreview {
            pages: set.pages,
            warnings: set.warnings,
            candidates: files.len(),
            excluded: set.excluded,
            generation: self.generation,
        });
        Ok(())
    }

    /// 写 `<root>/atlas.json`（先写临时文件再改名，避免半截文件）。
    pub fn save(&mut self, root: &Path) -> anyhow::Result<PathBuf> {
        let bytes = self.config.to_json()?;
        let path = root.join(AtlasConfig::FILE);
        write_atomic(&path, &bytes)?;
        self.dirty = false;
        Ok(path)
    }
}

impl Default for AtlasPanelState {
    fn default() -> Self {
        Self::new()
    }
}

/// 一次扫描的结果。
pub(crate) struct AtlasPreview {
    pub pages: Vec<PlannedPage>,
    pub warnings: Vec<String>,
    pub candidates: usize,
    pub excluded: usize,
    pub generation: u64,
}

impl AtlasPreview {
    pub fn total_pixels(&self) -> u64 {
        self.pages.iter().map(PlannedPage::pixel_area).sum()
    }

    /// 按图集名分组（保持 `plan_pages` 的产出顺序）。
    pub fn groups(&self) -> Vec<(String, Vec<usize>)> {
        let mut out: Vec<(String, Vec<usize>)> = Vec::new();
        for (index, page) in self.pages.iter().enumerate() {
            match out.last_mut() {
                Some((name, indices)) if *name == page.spec_name => indices.push(index),
                _ => out.push((page.spec_name.clone(), vec![index])),
            }
        }
        out
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            Err(error)
        }
    }
}

/// 编码形态下拉（`rltex` / `png` / `auto`）。取值校验在 `AtlasConfig::validate`。
fn format_combo(ui: &mut egui::Ui, id: &str, value: &mut String) {
    let current = match value.as_str() {
        "rltex" => "rltex（裸像素）",
        "png" => "png",
        "auto" => "auto（逐页取小）",
        other => other,
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (code, label) in [
                ("auto", "auto（逐页取小）"),
                ("rltex", "rltex（裸像素）"),
                ("png", "png"),
            ] {
                ui.selectable_value(value, code.to_owned(), label);
            }
        });
}

impl EditorApp {
    /// 菜单入口：打开面板并按需从磁盘载入配置。
    pub(crate) fn open_atlas_panel(&mut self) {
        if !self.project_loaded {
            // 与其它"需要工程"的面板一致：无工程时动作本身安全但不做事（菜单项是灰的）。
            return;
        }
        if !self.atlas.show_panel {
            let root = crate::util::strip_verbatim_prefix(&self.project.root);
            self.atlas.reseed(&root);
        }
        self.atlas.show_panel = true;
    }

    pub(crate) fn draw_atlas_panel(&mut self, context: &egui::Context) {
        if !self.atlas.show_panel {
            return;
        }
        let mut open = self.atlas.show_panel;
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        let project_loaded = self.project_loaded;
        egui::Window::new("图集（Atlas）")
            .open(&mut open)
            .default_width(900.0)
            .default_height(640.0)
            .resizable(true)
            .show(context, |ui| {
                if !project_loaded {
                    ui.label("先打开一个工程，才能编辑该工程的 atlas.json。");
                    return;
                }
                self.draw_atlas_body(ui, &root);
            });
        self.atlas.show_panel = open && self.atlas.show_panel;
    }

    fn draw_atlas_body(&mut self, ui: &mut egui::Ui, root: &Path) {
        let before = self.atlas.config.clone();

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("配置：{}", root.join(AtlasConfig::FILE).display()))
                    .small()
                    .weak(),
            );
            if self.atlas.dirty {
                ui.label(
                    egui::RichText::new("● 未保存")
                        .small()
                        .color(egui::Color32::YELLOW),
                );
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("从磁盘重载").clicked() {
                self.atlas.reseed(root);
                self.atlas.message = "已从磁盘重载".to_owned();
            }
            if ui.button("保存 atlas.json").clicked() {
                match self.atlas.save(root) {
                    Ok(path) => self.atlas.message = format!("已保存 {}", path.display()),
                    Err(error) => self.atlas.message = format!("保存失败：{error}"),
                }
            }
            if ui.button("扫描并预览").clicked() {
                match self.atlas.scan(root) {
                    Ok(()) => {
                        let pages = self.atlas.preview.as_ref().map_or(0, |p| p.pages.len());
                        self.atlas.message = format!("扫描完成：{pages} 页");
                    }
                    Err(error) => self.atlas.message = format!("扫描失败：{error}"),
                }
            }
            if !self.atlas.message.is_empty() {
                ui.label(
                    egui::RichText::new(self.atlas.message.as_str())
                        .small()
                        .weak(),
                );
            }
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt("atlas_config_scroll")
            .max_height(300.0)
            .show(ui, |ui| {
                self.draw_atlas_config(ui);
            });

        if before != self.atlas.config {
            self.atlas.dirty = true;
            // 配置改了：旧预览不再代表当前配置，主动失效（不自动重扫，避免每帧读盘）。
            self.atlas.preview = None;
            self.atlas.texture = None;
        }

        ui.separator();
        self.draw_atlas_preview(ui);
    }

    fn draw_atlas_config(&mut self, ui: &mut egui::Ui) {
        ui.heading("目录聚合（推荐）");
        if self.atlas.config.auto.is_none() {
            ui.horizontal(|ui| {
                ui.label("未启用：按目录把同目录的小图自动并成一张页。");
                if ui.button("启用目录聚合").clicked() {
                    self.atlas.config.auto = Some(AutoAtlas::for_ui_root());
                }
            });
        }
        let mut remove_auto = false;
        if let Some(auto) = self.atlas.config.auto.as_mut() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut auto.enabled, "启用");
                ui.label("根目录");
                ui.add(egui::TextEdit::singleline(&mut auto.root).desired_width(240.0));
            });
            ui.horizontal(|ui| {
                ui.label("单页上限");
                ui.add(
                    egui::DragValue::new(&mut auto.max_size)
                        .range(64..=8192)
                        .speed(8.0),
                );
                ui.label("边距");
                ui.add(egui::DragValue::new(&mut auto.padding).range(0..=16));
                ui.label("单图阈值（像素²）");
                ui.add(
                    egui::DragValue::new(&mut auto.single_min_pixels)
                        .range(1024..=16_777_216)
                        .speed(1024.0),
                );
            });
            ui.horizontal(|ui| {
                ui.label("页编码");
                format_combo(ui, "atlas_auto_format", &mut auto.format);
                ui.label(
                    egui::RichText::new("auto = 逐页在 PNG 与裸像素之间取落盘字节小者")
                        .small()
                        .weak(),
                );
            });
            if ui.button("移除目录聚合").clicked() {
                remove_auto = true;
            }
        }
        if remove_auto {
            self.atlas.config.auto = None;
        }

        ui.add_space(6.0);
        ui.heading("单独成集（separate）");
        ui.label(
            egui::RichText::new("被点名的图不进目录默认页，各自成为单图集。")
                .small()
                .weak(),
        );
        let mut remove: Option<usize> = None;
        for (index, path) in self.atlas.config.separate.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.monospace(path);
                if ui.small_button("移除").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            self.atlas.config.separate.remove(index);
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.atlas.separate_input)
                    .hint_text("assets/art/ui/title/ui3.png")
                    .desired_width(360.0),
            );
            if ui.button("加入").clicked() {
                let value = self
                    .atlas
                    .separate_input
                    .trim()
                    .trim_matches('"')
                    .to_owned();
                if !value.is_empty() && !self.atlas.config.separate.contains(&value) {
                    self.atlas.config.separate.push(value);
                    self.atlas.separate_input.clear();
                }
            }
        });

        ui.add_space(6.0);
        ui.heading("显式图集（atlases）");
        ui.label(
            egui::RichText::new("按 include 前缀手工装箱；优先级高于目录聚合。")
                .small()
                .weak(),
        );
        let mut remove_spec: Option<usize> = None;
        for (index, spec) in self.atlas.config.atlases.iter_mut().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("名称");
                    ui.add(egui::TextEdit::singleline(&mut spec.name).desired_width(150.0));
                    ui.label("上限");
                    ui.add(
                        egui::DragValue::new(&mut spec.max_size)
                            .range(64..=8192)
                            .speed(8.0),
                    );
                    ui.label("边距");
                    ui.add(egui::DragValue::new(&mut spec.padding).range(0..=16));
                    ui.checkbox(&mut spec.pot, "POT");
                    format_combo(ui, &format!("atlas_spec_format_{index}"), &mut spec.format);
                    if ui.small_button("删除").clicked() {
                        remove_spec = Some(index);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("include");
                    let mut text = spec.include.join(", ");
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .hint_text("assets/art/ui/hud/, assets/art/ui/common/")
                            .desired_width(440.0),
                    );
                    if response.changed() {
                        spec.include = text
                            .split(',')
                            .map(str::trim)
                            .filter(|item| !item.is_empty())
                            .map(str::to_owned)
                            .collect();
                    }
                });
            });
        }
        if let Some(index) = remove_spec {
            self.atlas.config.atlases.remove(index);
        }
        if ui.button("新增显式图集").clicked() {
            self.atlas.config.atlases.push(AtlasSpec {
                name: "ui".to_owned(),
                include: vec!["assets/art/ui/".to_owned()],
                max_size: 2048,
                padding: 2,
                pot: false,
                format: "auto".to_owned(),
                oversize: None,
            });
        }
    }

    fn draw_atlas_preview(&mut self, ui: &mut egui::Ui) {
        let Some(preview) = self.atlas.preview.as_ref() else {
            ui.label(
                egui::RichText::new(
                    "点「扫描并预览」查看按当前配置打出来的页（不会修改任何文件）。",
                )
                .weak(),
            );
            return;
        };
        // 先把要用的量拷出来，随后才能自由修改 `self.atlas`（缩放/选中）。
        let pages = preview.pages.len();
        let candidates = preview.candidates;
        let excluded = preview.excluded;
        let total_pixels = preview.total_pixels();
        let warnings = preview.warnings.clone();
        let groups = preview.groups();
        let metas: Vec<(usize, u32, u32, usize)> = preview
            .pages
            .iter()
            .map(|page| {
                (
                    page.index,
                    page.page.width,
                    page.page.height,
                    page.region_count(),
                )
            })
            .collect();
        let selected = self.atlas.selected_page;

        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "候选 {candidates} 张 · 排除 {excluded} 条 · 图集 {} 个 · 页 {pages} 张 · 总像素 {:.2} MPx",
                groups.len(),
                total_pixels as f64 / 1_048_576.0,
            ));
            ui.checkbox(&mut self.atlas.zoom_follow, "缩放到窗口");
        });
        if !self.atlas.zoom_follow {
            ui.horizontal(|ui| {
                ui.label("倍率");
                ui.add(
                    egui::DragValue::new(&mut self.atlas.zoom)
                        .range(0.05..=2.0)
                        .speed(0.01),
                );
            });
        }
        if !warnings.is_empty() {
            egui::CollapsingHeader::new(format!("跳过/警告（{}）", warnings.len())).show(
                ui,
                |ui| {
                    for warning in &warnings {
                        ui.label(egui::RichText::new(warning).small());
                    }
                },
            );
        }
        if pages == 0 {
            ui.label("当前配置没有产生任何页（检查根目录/排除规则）。");
            return;
        }
        ui.separator();
        ui.horizontal_top(|ui| {
            egui::ScrollArea::vertical()
                .id_salt("atlas_page_list")
                .max_height(360.0)
                .show(ui, |ui| {
                    for (name, indices) in &groups {
                        ui.label(egui::RichText::new(name).strong());
                        for index in indices {
                            let (page_no, width, height, regions) = metas[*index];
                            let label =
                                format!("  第 {page_no} 页  {width}×{height}  · {regions} 区");
                            if ui.selectable_label(selected == *index, label).clicked() {
                                self.atlas.selected_page = *index;
                            }
                        }
                    }
                });
            ui.separator();
            ui.vertical(|ui| {
                self.draw_selected_page(ui);
            });
        });
    }

    fn draw_selected_page(&mut self, ui: &mut egui::Ui) {
        let index = self.atlas.selected_page;
        // 先把"这一页是谁"拷出来，释放对 `self.atlas.preview` 的借用。
        let Some((width, height, origin, placements, generation)) = self
            .atlas
            .preview
            .as_ref()
            .and_then(|preview| preview.pages.get(index))
            .map(|page| {
                (
                    page.page.width,
                    page.page.height,
                    page.origin.clone(),
                    page.page.placements.clone(),
                    self.atlas.preview.as_ref().map_or(0, |p| p.generation),
                )
            })
        else {
            return;
        };
        ui.label(format!(
            "{origin} · {width}×{height} · {} 区",
            placements.len()
        ));

        let key = (index, generation);
        if self.atlas.texture_key != Some(key) || self.atlas.texture.is_none() {
            let rgba = self
                .atlas
                .preview
                .as_ref()
                .and_then(|preview| preview.pages.get(index))
                .map(atlas_plan::render_page_rgba);
            match rgba {
                Some(Ok(rgba)) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [width as usize, height as usize],
                        &rgba,
                    );
                    match self.atlas.texture.as_mut() {
                        Some(texture) => texture.set(image, egui::TextureOptions::NEAREST),
                        None => {
                            self.atlas.texture = Some(ui.ctx().load_texture(
                                "atlas_page_preview",
                                image,
                                egui::TextureOptions::NEAREST,
                            ));
                        }
                    }
                    self.atlas.texture_key = Some(key);
                }
                Some(Err(error)) => {
                    ui.label(
                        egui::RichText::new(format!("合成失败：{error}")).color(egui::Color32::RED),
                    );
                    return;
                }
                None => return,
            }
        }
        let Some(texture) = self.atlas.texture.as_ref() else {
            return;
        };

        let available = ui.available_size();
        let zoom = if self.atlas.zoom_follow {
            (available.x / width.max(1) as f32)
                .min(available.y / height.max(1) as f32)
                .clamp(0.05, 1.0)
        } else {
            self.atlas.zoom
        };
        let size = egui::vec2(width as f32 * zoom, height as f32 * zoom);
        egui::ScrollArea::both()
            .id_salt("atlas_page_scroll")
            .max_height(available.y.max(160.0))
            .show(ui, |ui| {
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                // 区域框 + 名字：缩放足够大时才画名字，避免小页挤成一团。
                let show_names = zoom >= 0.5;
                let pointer = response.hover_pos();
                let mut hovered: Option<String> = None;
                for (name, place) in &placements {
                    let min = rect.min + egui::vec2(place.x as f32 * zoom, place.y as f32 * zoom);
                    let max =
                        min + egui::vec2(place.width as f32 * zoom, place.height as f32 * zoom);
                    let cell = egui::Rect::from_min_max(min, max);
                    painter.rect_stroke(
                        cell,
                        0.0,
                        egui::Stroke::new(
                            1.0_f32,
                            egui::Color32::from_rgba_unmultiplied(0, 255, 180, 200),
                        ),
                        egui::StrokeKind::Inside,
                    );
                    if show_names {
                        painter.text(
                            cell.min + egui::vec2(2.0, 1.0),
                            egui::Align2::LEFT_TOP,
                            name,
                            egui::FontId::proportional(10.0),
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 220),
                        );
                    }
                    if pointer.is_some_and(|pointer| cell.contains(pointer)) {
                        hovered = Some(name.clone());
                    }
                }
                if let Some(name) = hovered {
                    response.on_hover_text(name);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-editor-atlas-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/art/ui/hud")).unwrap();
        root
    }

    fn write_png(path: &Path, w: u32, h: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let pixels = vec![200u8; (w * h * 4) as usize];
        rustlua_render::texture::save_png_rgba8(path, w, h, &pixels).unwrap();
    }

    #[test]
    fn save_and_reseed_roundtrip() {
        let root = temp_root("roundtrip");
        let mut state = AtlasPanelState::new();
        state.config.auto = Some(AutoAtlas::for_ui_root());
        state.config.separate = vec!["assets/art/ui/title/ui3.png".to_owned()];
        state.save(&root).expect("保存 atlas.json");
        assert!(root.join(AtlasConfig::FILE).is_file());
        assert!(!state.dirty);

        let mut reloaded = AtlasPanelState::new();
        reloaded.reseed(&root);
        assert_eq!(reloaded.config, state.config);
        assert!(reloaded.config.auto.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_packs_pages_and_skips_previews() {
        let root = temp_root("scan");
        write_png(&root.join("assets/art/ui/hud/a.png"), 8, 8);
        write_png(&root.join("assets/art/ui/hud/b.png"), 8, 8);
        write_png(&root.join("assets/art/ui/hud/_preview.png"), 64, 64);

        let mut state = AtlasPanelState::new();
        state.config.auto = Some(AutoAtlas {
            enabled: true,
            root: "assets/art/ui".to_owned(),
            max_size: 64,
            padding: 1,
            format: "auto".to_owned(),
            single_min_pixels: 65536,
        });
        state.scan(&root).expect("扫描");
        let preview = state.preview.as_ref().expect("有预览");
        assert_eq!(preview.candidates, 3, "候选包含预览图");
        assert_eq!(preview.pages.len(), 1, "两张小图并成一页");
        assert_eq!(preview.pages[0].region_count(), 2, "预览图不得进页");
        assert_eq!(state.selected_page, 0);
        assert_eq!(preview.generation, state.generation);

        // 再扫一次：代次递增，纹理缓存键必须失效。
        let first = preview.generation;
        state.scan(&root).expect("再次扫描");
        assert!(state.preview.as_ref().unwrap().generation > first);
        assert!(state.texture_key.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_atomic_replaces_existing_file() {
        let root = temp_root("atomic");
        let path = root.join("atlas.json");
        std::fs::write(&path, b"old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(
            !path.with_extension("json.tmp").exists(),
            "临时文件必须清掉"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn groups_keep_spec_order() {
        let root = temp_root("groups");
        write_png(&root.join("assets/art/ui/hud/a.png"), 8, 8);
        write_png(&root.join("assets/art/ui/hud/b.png"), 8, 8);
        write_png(&root.join("assets/art/ui/title/c.png"), 8, 8);
        write_png(&root.join("assets/art/ui/title/d.png"), 8, 8);
        let mut state = AtlasPanelState::new();
        state.config.auto = Some(AutoAtlas {
            enabled: true,
            root: "assets/art/ui".to_owned(),
            max_size: 64,
            padding: 1,
            format: "rltex".to_owned(),
            single_min_pixels: 65536,
        });
        state.scan(&root).expect("扫描");
        let groups = state.preview.as_ref().unwrap().groups();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "hud_default");
        assert_eq!(groups[1].0, "title_default");
        let _ = std::fs::remove_dir_all(&root);
    }
}
