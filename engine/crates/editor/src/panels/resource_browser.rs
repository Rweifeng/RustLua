//! 行3 资源面板（最左侧）：扫描/筛选资源，目录树展示（可折叠），纹理/预制体可拖放到场景。

use crate::EditorApp;
use crate::util::strip_verbatim_prefix;
use editor::{ResourceBrowserState, ResourceEntry, ResourceKind};
use eframe::egui;
use engine::core::resource_db::ResourceState;
use engine::core::texture::{TextureAsset, TextureColorSpace};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

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
                    // 把资源按目录组织成树（可折叠）；过滤后无可见子项的目录自动隐藏。
                    // 树按 (filter, refresh_count) 缓存，命中时复用上次构建的树，
                    // 避免大资源项目每帧 clone 全部条目重建。
                    let mut resource_to_open = None;
                    let tree = self.resource_tree_for_draw();
                    self.draw_resource_tree(ui, &tree, "", 1, context, &mut resource_to_open);
                    if let Some(path) = resource_to_open {
                        self.open_scene_resource(path);
                    }
                    self.resource_tree_cache = Some(ResourceTreeCache {
                        filter: self.resources_filter.clone(),
                        refresh_count: self.resources.refresh_count(),
                        tree,
                    });
                });
            });
        refresh_resources
    }

    /// 取本次要绘制的资源树：命中缓存时直接移出缓存（绘制后写回），避免大
    /// 资源项目每帧 clone 全部条目；过滤词或资源刷新计数变化时才重建。
    fn resource_tree_for_draw(&mut self) -> ResourceTree {
        let filter = self.resources_filter.clone();
        let refresh_count = self.resources.refresh_count();
        let cache_valid = self
            .resource_tree_cache
            .as_ref()
            .map(|cached| cached.filter == filter && cached.refresh_count == refresh_count)
            .unwrap_or(false);
        if cache_valid {
            return self
                .resource_tree_cache
                .take()
                .expect("valid cache present")
                .tree;
        }
        let visible: Vec<ResourceEntry> = self
            .resources
            .visible_entries()
            .into_iter()
            .cloned()
            .collect();
        build_resource_tree(&visible)
    }

    /// 递归绘制资源目录树：子文件夹用 CollapsingHeader 折叠，叶子是资源条目。
    /// `depth` 从 1 起（根目录的直接子文件夹）；只有顶层目录默认展开，深层目录
    /// 默认折叠——大资源项目（如 game_farm 上千张图）全展开会每帧渲染数千行，
    /// 拖慢整个编辑器。
    fn draw_resource_tree(
        &mut self,
        ui: &mut egui::Ui,
        node: &ResourceTree,
        prefix: &str,
        depth: usize,
        context: &egui::Context,
        resource_to_open: &mut Option<PathBuf>,
    ) {
        for (name, child) in &node.folders {
            // 过滤后没有可见内容的目录不展示。
            if child.files.is_empty() && child.folders.is_empty() {
                continue;
            }
            let child_prefix = format!("{prefix}{name}/");
            // 用完整目录路径做折叠状态 id，避免不同层级同名目录互相串状态。
            let id = ui.make_persistent_id(("resource_tree", child_prefix.as_str()));
            egui::CollapsingHeader::new(name)
                .id_salt(id)
                .default_open(depth <= 1)
                .show(ui, |ui| {
                    self.draw_resource_tree(
                        ui,
                        child,
                        &child_prefix,
                        depth + 1,
                        context,
                        resource_to_open,
                    );
                });
        }
        for entry in &node.files {
            self.draw_resource_entry(ui, entry, context, resource_to_open);
        }
    }

    /// 绘制单个资源条目：纹理/模型/预制体可拖放到场景，其余为文件名标签。
    /// 只展示资源名（不显示「类型 Registered」状态行）。
    fn draw_resource_entry(
        &mut self,
        ui: &mut egui::Ui,
        entry: &ResourceEntry,
        context: &egui::Context,
        resource_to_open: &mut Option<PathBuf>,
    ) {
        let path_string = entry.path_string();
        let file_name = Path::new(&path_string)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_string.clone());
        let marker = status_marker(entry.status);
        let is_selected = self.selected_resource.as_ref() == Some(&entry.path);
        if matches!(
            entry.kind,
            ResourceKind::Texture | ResourceKind::Prefab | ResourceKind::Model
        ) {
            // 纹理/模型/预制体：可拖放到场景（纹理/预制体带缩略图）。
            let payload = path_string.clone();
            ui.horizontal(|ui| {
                if let Some(id) = self.texture_thumbnail(&path_string, context) {
                    ui.add(egui::Image::new(egui::load::SizedTexture::new(
                        id,
                        egui::vec2(20.0, 20.0),
                    )));
                } else {
                    // 缩略图后台解码中：用同宽占位，避免行宽随加载进度跳动。
                    ui.add_space(20.0);
                }
                if let Some((icon, color)) = marker {
                    ui.colored_label(color, icon);
                }
                let response = ui.selectable_label(is_selected, &file_name);
                if response.clicked() {
                    self.select_resource(entry.path.clone());
                }
                if response.double_clicked() && entry.kind == ResourceKind::Prefab {
                    *resource_to_open = Some(entry.path.clone());
                }
                ui.dnd_drag_source(
                    egui::Id::new(("texture_drag", path_string.as_str())),
                    payload,
                    |ui| {
                        ui.small("::");
                    },
                );
                if entry.kind == ResourceKind::Prefab
                    && ui
                        .small_button("✎")
                        .on_hover_text("打开 Prefab 编辑")
                        .clicked()
                {
                    *resource_to_open = Some(entry.path.clone());
                }
            });
        } else {
            ui.horizontal(|ui| {
                if let Some((icon, color)) = marker {
                    ui.colored_label(color, icon);
                }
                let response = ui.selectable_label(is_selected, &file_name);
                if response.clicked() {
                    self.select_resource(entry.path.clone());
                }
                if response.double_clicked()
                    && matches!(entry.kind, ResourceKind::Scene | ResourceKind::Prefab)
                {
                    *resource_to_open = Some(entry.path.clone());
                }
            });
        }
        if let Some(error) = &entry.error {
            ui.small(egui::RichText::new(error).color(egui::Color32::RED));
        }
    }

    /// 纹理缩略图（缓存 + 后台解码）。大图在后台线程降采样到 ≤64px，主线程
    /// 只接收结果并上传 GPU；未就绪时返回 `None`（本帧不画图，用占位符）。
    fn texture_thumbnail(
        &mut self,
        relative: &str,
        ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if let Some(handle) = self.thumbnails.get(relative) {
            return Some(handle.id());
        }
        // 上传后台已解码的缩略图（每帧限量，避免一帧上传过多）。
        for result in self.thumbnail_worker.drain(MAX_THUMBNAIL_UPLOADS_PER_FRAME) {
            let Some(pixels) = result.pixels else {
                if result.key == relative {
                    return None; // 当前条目解码失败：不再重试（requested 已记录）。
                }
                continue;
            };
            let image =
                egui::ColorImage::from_rgba_unmultiplied([result.width, result.height], &pixels);
            let handle = ctx.load_texture("thumb", image, egui::TextureOptions::LINEAR);
            let id = handle.id();
            if result.key == relative {
                self.thumbnails.insert(relative.to_owned(), handle);
                return Some(id);
            }
            self.thumbnails.insert(result.key, handle);
        }
        // 未缓存且未在解码中：提交后台解码请求（每帧预算内），本帧不画图。
        let root = strip_verbatim_prefix(&self.project.root);
        self.thumbnail_worker
            .request(root.join(relative), relative.to_owned());
        None
    }

    /// 扫描项目资源并同步到资源数据库。
    pub(crate) fn refresh_resources(&mut self) {
        self.resources.set_filter(self.resources_filter.clone());
        if !self.project_loaded {
            self.resources = ResourceBrowserState::new();
            self.clear_resource_selection();
            return;
        }
        if let Err(error) = self.resources.refresh(&self.project.root) {
            self.status = error.to_string();
        }
        if self.selected_resource.as_ref().is_some_and(|selected| {
            self.resources
                .entries()
                .iter()
                .all(|entry| entry.path != *selected)
        }) {
            self.clear_resource_selection();
        }
        // 资源/场景文件可能新增或删除：让场景下拉下次重扫。
        self.scene_files_cache_time = None;
    }

    pub(crate) fn delete_selected_resource(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks resource deletion; click Stop first".to_owned();
            return;
        }
        let Some(relative_path) = self.selected_resource.clone() else {
            self.status = "请先在资源面板选中资源".to_owned();
            return;
        };
        let absolute_path = self.project.root.join(&relative_path);
        if self
            .document
            .current_scene
            .as_ref()
            .and_then(|document| document.path.as_ref())
            .is_some_and(|path| {
                strip_verbatim_prefix(path) == strip_verbatim_prefix(&absolute_path)
            })
        {
            self.status = format!("当前打开的文档不能直接删除：{}", relative_path.display());
            return;
        }
        match fs::remove_file(&absolute_path) {
            Ok(()) => {
                self.thumbnails
                    .remove(&relative_path.to_string_lossy().replace('\\', "/"));
                self.clear_resource_selection();
                self.refresh_resources();
                self.status = format!("已删除资源：{}", relative_path.display());
            }
            Err(error) => {
                self.status = format!("删除资源失败：{}：{error}", relative_path.display());
            }
        }
    }
}

/// 资源状态图标：健康状态（Ready/Discovered）不打扰，问题状态给彩色标记，
/// 便于一眼看出「为什么还没导入/为何是断链」。
fn status_marker(status: ResourceState) -> Option<(&'static str, egui::Color32)> {
    match status {
        ResourceState::Ready | ResourceState::Discovered => None,
        ResourceState::Stale => Some(("▲", egui::Color32::YELLOW)),
        ResourceState::Missing | ResourceState::Orphaned => Some(("✕", egui::Color32::GRAY)),
        ResourceState::ImportFailed => Some(("✕", egui::Color32::RED)),
        ResourceState::Invalid => Some(("断链", egui::Color32::RED)),
        // 瞬态状态不会出现在浏览器条目中。
        ResourceState::ImportQueued | ResourceState::Importing | ResourceState::Removed => None,
    }
}

/// 资源目录树节点：子文件夹（递归）+ 直接文件。
#[derive(Default)]
pub(crate) struct ResourceTree {
    folders: BTreeMap<String, ResourceTree>,
    files: Vec<ResourceEntry>,
}

/// 资源目录树缓存：过滤词与资源刷新计数不变时复用上次构建的树，避免大资源
/// 项目每帧 clone 全部条目重建（树在绘制时移出、绘制后写回）。
pub(crate) struct ResourceTreeCache {
    pub(crate) filter: String,
    pub(crate) refresh_count: u64,
    pub(crate) tree: ResourceTree,
}

/// 把资源条目列表组织成目录树（按相对路径逐段分组）。
fn build_resource_tree(entries: &[ResourceEntry]) -> ResourceTree {
    let mut root = ResourceTree::default();
    for entry in entries {
        let path = entry.path_string();
        let mut node = &mut root;
        let mut segments = path.split('/').peekable();
        while let Some(segment) = segments.next() {
            if segments.peek().is_none() {
                // 最后一段是文件名。
                node.files.push(entry.clone());
            } else {
                node = node.folders.entry(segment.to_owned()).or_default();
            }
        }
    }
    sort_tree(&mut root);
    root
}

/// 递归排序文件（BTreeMap 的文件夹键天然字典序）。
fn sort_tree(node: &mut ResourceTree) {
    node.files.sort_by_key(|entry| entry.path.clone());
    for child in node.folders.values_mut() {
        sort_tree(child);
    }
}

/// RGBA8 最近邻降采样：长边超过 `max` 时缩到 max，否则原样返回。
/// 用于缩略图，避免全分辨率上传；大纹理也能保留整体色调。
fn downscale_rgba(
    pixels: &[u8],
    width: usize,
    height: usize,
    max: usize,
) -> (Vec<u8>, usize, usize) {
    if (width <= max && height <= max) || width == 0 || height == 0 {
        return (pixels.to_vec(), width, height);
    }
    let scale = max as f32 / width.max(height) as f32;
    let new_w = ((width as f32 * scale).round() as usize).max(1);
    let new_h = ((height as f32 * scale).round() as usize).max(1);
    let mut out = vec![0u8; new_w * new_h * 4];
    for y in 0..new_h {
        let sy = ((y as f32 + 0.5) / new_h as f32 * height as f32) as usize;
        let sy = sy.min(height - 1);
        for x in 0..new_w {
            let sx = ((x as f32 + 0.5) / new_w as f32 * width as f32) as usize;
            let sx = sx.min(width - 1);
            let src = (sy * width + sx) * 4;
            let dst = (y * new_w + x) * 4;
            out[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
        }
    }
    (out, new_w, new_h)
}

/// 后台缩略图解码：主线程只提交请求/接收结果并上传 GPU，PNG 解码与降采样在
/// 一条后台线程串行执行，避免大资源项目（如 game_farm 上千张图）在 UI 线程
/// 同步解码导致打开/刷新卡死。
pub(crate) struct ThumbnailWorker {
    request_tx: std::sync::mpsc::Sender<ThumbnailRequest>,
    result_rx: std::sync::mpsc::Receiver<ThumbnailResult>,
    /// 已提交请求的 key（项目相对路径），避免重复提交。
    requested: HashSet<String>,
    /// 本帧已提交的请求数（每帧预算，防止一帧排空整个队列）。
    submitted_this_frame: usize,
    /// 项目代际：项目切换时自增，旧代际的请求结果直接丢弃，防错配。
    generation: u64,
}

/// 一个缩略图解码请求（主线程 → 后台线程）。
struct ThumbnailRequest {
    /// 绝对源路径。
    path: PathBuf,
    /// 结果键（项目相对路径，正斜杠）。
    key: String,
    generation: u64,
}

/// 一个解码结果（后台线程 → 主线程）。`pixels: None` 表示解码失败。
struct ThumbnailResult {
    key: String,
    pixels: Option<Vec<u8>>,
    width: usize,
    height: usize,
    generation: u64,
}

/// 每帧最多新提交的缩略图解码请求数。
const MAX_THUMBNAIL_SUBMITS_PER_FRAME: usize = 32;
/// 每帧最多上传到 GPU 的解码结果数。
const MAX_THUMBNAIL_UPLOADS_PER_FRAME: usize = 64;

impl ThumbnailWorker {
    pub(crate) fn new() -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<ThumbnailRequest>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<ThumbnailResult>();
        std::thread::spawn(move || {
            // 主线程退出（request_tx 被 Drop）后 recv 返回 Err，线程自然结束。
            while let Ok(request) = request_rx.recv() {
                let result = decode_thumbnail(&request);
                if result_tx.send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            request_tx,
            result_rx,
            requested: HashSet::new(),
            submitted_this_frame: 0,
            generation: 0,
        }
    }

    /// 每帧开头调用：重置提交预算。
    pub(crate) fn begin_frame(&mut self) {
        self.submitted_this_frame = 0;
    }

    /// 提交一个解码请求（同 key 只提交一次；每帧受预算限制）。
    fn request(&mut self, path: PathBuf, key: String) {
        if self.requested.contains(&key) {
            return;
        }
        if self.submitted_this_frame >= MAX_THUMBNAIL_SUBMITS_PER_FRAME {
            return;
        }
        self.requested.insert(key.clone());
        self.submitted_this_frame += 1;
        let _ = self.request_tx.send(ThumbnailRequest {
            path,
            key,
            generation: self.generation,
        });
    }

    /// 取出已完成的解码结果（非阻塞，最多 `max` 条；旧代际直接丢弃）。
    fn drain(&mut self, max: usize) -> Vec<ThumbnailResult> {
        let mut out = Vec::new();
        for _ in 0..max {
            match self.result_rx.try_recv() {
                Ok(result) if result.generation == self.generation => out.push(result),
                Ok(_) => {} // 旧代际结果：丢弃。
                Err(_) => break,
            }
        }
        out
    }

    /// 项目切换：自增代际并清空已请求集合，旧项目的结果与请求全部失效。
    pub(crate) fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.requested.clear();
        self.submitted_this_frame = 0;
    }
}

/// 后台线程的解码实现：读取 + PNG 解码 + 降采样到 ≤64px。
fn decode_thumbnail(request: &ThumbnailRequest) -> ThumbnailResult {
    let generation = request.generation;
    let key = request.key.clone();
    match TextureAsset::load_png(&request.path, TextureColorSpace::Srgb) {
        Ok(asset) => {
            let (pixels, width, height) = downscale_rgba(
                asset.pixels(),
                asset.width() as usize,
                asset.height() as usize,
                64,
            );
            ThumbnailResult {
                key,
                pixels: Some(pixels),
                width,
                height,
                generation,
            }
        }
        Err(_) => ThumbnailResult {
            key,
            pixels: None,
            width: 0,
            height: 0,
            generation,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{build_resource_tree, downscale_rgba};
    use crate::EditorApp;
    use editor::{ResourceEntry, ResourceKind, open_project};
    use engine::core::logging::LogSink;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn entry(path: &str, kind: ResourceKind) -> ResourceEntry {
        ResourceEntry {
            path: PathBuf::from(path),
            kind,
            status: engine::core::resource_db::ResourceState::Discovered,
            asset_id: None,
            error: None,
        }
    }

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos()
    }

    fn temp_project_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "p2-editor-resource-browser-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::create_dir_all(root.join("scenes")).unwrap();
        fs::write(
            root.join("project.toml"),
            "[project]\nname = \"t\"\nversion = \"0.1.0\"\nengine = \"p2\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        fs::write(root.join("scripts/main.lua"), b"return true\n").unwrap();
        fs::write(root.join("scenes/main.scene"), b"name = \"main\"\n").unwrap();
        root
    }

    #[test]
    fn tree_groups_by_directory_and_sorts_files() {
        let entries = vec![
            entry("scenes/main.scene", ResourceKind::Scene),
            entry("assets/sprites/player.png", ResourceKind::Texture),
            entry("scripts/main.lua", ResourceKind::LuaScript),
            entry("assets/player.png", ResourceKind::Texture),
            entry("assets/sprites/bg.png", ResourceKind::Texture),
        ];
        let tree = build_resource_tree(&entries);
        // 顶层目录：assets / scenes / scripts。
        assert_eq!(tree.folders.len(), 3);
        assert!(tree.folders.contains_key("assets"));
        assert!(tree.folders.contains_key("scenes"));
        assert!(tree.folders.contains_key("scripts"));
        // assets 下：文件排序 + 子目录 sprites。
        let assets = &tree.folders["assets"];
        assert_eq!(assets.files.len(), 1);
        assert_eq!(assets.files[0].path_string(), "assets/player.png");
        assert_eq!(assets.folders.len(), 1);
        assert!(assets.folders.contains_key("sprites"));
        // sprites 下两个文件按名排序。
        let sprites = &assets.folders["sprites"];
        assert_eq!(sprites.files.len(), 2);
        assert_eq!(sprites.files[0].path_string(), "assets/sprites/bg.png");
        assert_eq!(sprites.files[1].path_string(), "assets/sprites/player.png");
        // 顶层文件列表为空（资源都在子目录里）。
        assert!(tree.files.is_empty());
    }

    #[test]
    fn small_image_is_returned_unchanged() {
        let pixels = vec![255u8; 8 * 8 * 4];
        let (out, width, height) = downscale_rgba(&pixels, 8, 8, 64);
        assert_eq!((width, height), (8, 8));
        assert_eq!(out, pixels);
    }

    #[test]
    fn large_image_is_downscaled_to_max_side() {
        let pixels = vec![128u8; 512 * 256 * 4];
        let (out, width, height) = downscale_rgba(&pixels, 512, 256, 64);
        assert_eq!((width, height), (64, 32), "长边缩到 64，保持比例");
        assert_eq!(out.len(), width * height * 4);
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|px| *px == [128, 128, 128, 128])
        );
    }

    #[test]
    fn delete_shortcut_removes_selected_resource_from_browser() {
        let root = temp_project_root("delete-resource");
        fs::write(root.join("assets/player.png"), b"png").unwrap();
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.select_resource(PathBuf::from("assets/player.png"));

        app.handle_delete_shortcut();

        assert!(
            !root.join("assets/player.png").exists(),
            "Delete should remove selected resource file"
        );
        assert!(app.selected_resource.is_none());
        assert!(app.status.contains("已删除资源"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn delete_shortcut_prefers_hierarchy_when_scene_selection_is_active() {
        let root = temp_project_root("delete-entity");
        fs::write(root.join("assets/player.png"), b"png").unwrap();
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.select_resource(PathBuf::from("assets/player.png"));
        let scene = app.document.new_scene("main");
        let id = app.tree.create_entity(scene, "Player", None).unwrap();
        app.selection
            .set_single(id, editor::SelectionSource::Hierarchy);
        // 选中实体后 Delete 键应作用于场景树（优先级高于已选中的资源）。
        app.delete_target = crate::DeleteTarget::Hierarchy;
        app.sync_inspector(id);

        app.handle_delete_shortcut();

        assert!(
            !app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .contains_key(&id),
            "Delete should remove the selected hierarchy entity"
        );
        assert!(
            root.join("assets/player.png").exists(),
            "resource should stay when hierarchy is the active delete target"
        );
        assert!(app.selection.is_empty());
        assert_eq!(app.delete_target, crate::DeleteTarget::None);
        assert!(app.status.contains("Entity deleted"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn thumbnail_worker_decodes_png_and_reports_failure() {
        let mut worker = super::ThumbnailWorker::new();
        let png = engine::core::games_dir().join("game_test/assets/textures/red-1x1.png");
        let good_key = "assets/textures/red-1x1.png".to_owned();
        worker.request(png.clone(), good_key.clone());
        // 不存在的文件：后台应回传一个 pixels=None 的结果（而非卡住或丢失）。
        let bad_key = "assets/textures/ghost.png".to_owned();
        worker.request(png.join("ghost.png"), bad_key.clone());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut good = None;
        let mut bad = None;
        while (good.is_none() || bad.is_none()) && std::time::Instant::now() < deadline {
            for result in worker.drain(8) {
                if result.key == good_key && good.is_none() {
                    good = Some(result);
                } else if result.key == bad_key && bad.is_none() {
                    bad = Some(result);
                }
            }
            if good.is_none() || bad.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        let good = good.expect("red-1x1.png 应解码成功");
        assert!(good.pixels.is_some());
        assert_eq!((good.width, good.height), (1, 1));
        let bad = bad.expect("缺失文件应回传解码失败结果");
        assert!(bad.pixels.is_none());
    }
}
