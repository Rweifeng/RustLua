//! 行3 资源面板（最左侧）：扫描/筛选资源，竖排展示，纹理/预制体可拖放到场景。

use crate::EditorApp;
use crate::util::strip_verbatim_prefix;
use eframe::egui;
use editor::{ResourceBrowserState, ResourceEntry, ResourceKind};
use engine_core::texture::{TextureAsset, TextureColorSpace};

impl EditorApp {
    /// 资源面板：返回是否需要触发一次资源刷新。
    pub(crate) fn draw_resource_browser_panel(&mut self, context: &egui::Context) -> bool {
        let mut refresh_resources = false;
        self.resources.set_filter(self.resources_filter.clone());
        egui::SidePanel::left("resource_panel")
            .resizable(true)
            .default_width(200.0)
            .min_width(110.0)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("资源面板");
                    if ui.button("刷新").clicked() {
                        refresh_resources = true;
                    }
                    ui.label(format!("{} 项", self.resources.visible_entries().len()));
                });
                ui.text_edit_singleline(&mut self.resources_filter);
                if let Some(error) = self.resources.last_error() {
                    ui.colored_label(egui::Color32::RED, format!("错误：{error}"));
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let entries: Vec<ResourceEntry> = self
                        .resources
                        .visible_entries()
                        .into_iter()
                        .cloned()
                        .collect();
                    let mut scene_to_open = None;
                    for entry in &entries {
                        let status = format!("{:?}", entry.status);
                        let path = entry.path.clone();
                        let path_string = entry.path_string();
                        if matches!(entry.kind, ResourceKind::Texture | ResourceKind::Prefab) {
                            // 纹理/预制体：缩略图 + 可拖放到场景。
                            let payload = path_string.clone();
                            ui.dnd_drag_source(
                                egui::Id::new(("texture_drag", path_string.as_str())),
                                payload,
                                |ui| {
                                    ui.horizontal(|ui| {
                                        if let Some(id) =
                                            self.texture_thumbnail(&path_string, context)
                                        {
                                            ui.add(egui::Image::new(
                                                egui::load::SizedTexture::new(
                                                    id,
                                                    egui::vec2(20.0, 20.0),
                                                ),
                                            ));
                                        }
                                        ui.vertical(|ui| {
                                            ui.label(&path_string);
                                            ui.small(format!(
                                                "{} {}",
                                                entry.kind.label(),
                                                status
                                            ));
                                            if let Some(error) = &entry.error {
                                                ui.small(
                                                    egui::RichText::new(error)
                                                        .color(egui::Color32::RED),
                                                );
                                            }
                                        });
                                    });
                                },
                            );
                        } else {
                            let response = ui.selectable_label(false, &path_string);
                            if response.double_clicked() && entry.kind == ResourceKind::Scene {
                                scene_to_open = Some(path.clone());
                            }
                            ui.small(format!("{} {}", entry.kind.label(), status));
                            if let Some(error) = &entry.error {
                                ui.small(egui::RichText::new(error).color(egui::Color32::RED));
                            }
                        }
                        ui.separator();
                    }
                    if let Some(path) = scene_to_open {
                        self.open_scene_resource(path);
                    }
                });
            });
        refresh_resources
    }

    /// 纹理缩略图（缓存；按项目相对路径）。
    fn texture_thumbnail(
        &mut self,
        relative: &str,
        ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if let Some(handle) = self.thumbnails.get(relative) {
            return Some(handle.id());
        }
        let root = strip_verbatim_prefix(&self.project.root);
        let asset = TextureAsset::load_png(root.join(relative), TextureColorSpace::Srgb).ok()?;
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [asset.width() as usize, asset.height() as usize],
            asset.pixels(),
        );
        let handle = ctx.load_texture("thumb", image, egui::TextureOptions::LINEAR);
        let id = handle.id();
        self.thumbnails.insert(relative.to_owned(), handle);
        Some(id)
    }

    /// 扫描项目资源并同步到资源数据库。
    pub(crate) fn refresh_resources(&mut self) {
        self.resources.set_filter(self.resources_filter.clone());
        if !self.project_loaded {
            self.resources = ResourceBrowserState::new();
            return;
        }
        if let Err(error) = self.resources.refresh(&self.project.root) {
            self.status = error.to_string();
        }
    }
}
