//! 贴图预览窗（P1「双击贴图看原图」的等价物）。
//!
//! 三条刻意的口径：
//! 1. **只预览，不改图片本身；导入设置只写一份入库文本**：M79 P1-3c 起，本窗多了一行
//!    「导入采样」，写的是工程根的 `resources.meta`（[`ResourceMeta`]）——资源库的**持久那一半**：
//!    `.cache/resources.db` 在 `.gitignore` 里（那是可重建缓存），而"这张图放大要糊还是要锐"是
//!    作者意图，换一台机器必须还在。PNG 一个字节都不动；写完让 Scene View / Play 两个渲染器
//!    **就地换绑定组**（不重传像素），所以"应用"不需要重启预览。
//! 2. **全尺寸、单张、按需解码**：一次双击只解一张 PNG，解码器与运行期同一个
//!    `TextureAsset::load_png`（不走资源面板那条后台降采样链路——那条只留 ≤64px，
//!    看像素恰好是它砍掉的部分）。代价是 UI 线程同步解码：几千像素的图在 Debug 下
//!    几十毫秒，因为它是显式用户动作而不是列表滚动，可以接受。
//! 3. **「逐像素」≠「导入采样」**：前者只换本窗这一张预览纹理的 `egui::TextureOptions`，
//!    不碰任何工程数据（`the_preview_pixel_toggle_touches_no_project_data` 钉的就是这条）；
//!    后者才是运行期那一档——真机走的是贴图绑定组里的采样器（`textured.wgsl` 的 `samp`），
//!    而它只由 `resources.meta` 决定。两行挨在一起但语义完全不同，文案分开写。

use std::path::{Path, PathBuf};

use eframe::egui;
use rustlua_assets::import_settings::{
    ResourceMeta, TextureFilter, TextureImportSettings, TextureWrap,
};
use rustlua_render::texture::{TextureAsset, TextureColorSpace};

use crate::EditorApp;

/// 预览窗状态（挂在 `EditorApp` 上；只缓存当前这一张，切图即丢弃上一张）。
pub(crate) struct TexturePreviewState {
    /// 窗口是否打开（`egui::Window::open` 直接绑到这里）。
    pub show: bool,
    /// 正在预览的贴图（工程内相对路径，与资源索引同一口径）。
    pub relative: Option<String>,
    /// 缩放（`zoom_follow` 关掉后才生效）。
    pub zoom: f32,
    /// 缩放是否跟随窗口宽度（大图的默认档，避免开窗就滚动）。
    pub zoom_follow: bool,
    /// 逐像素（NEAREST）还是平滑（LINEAR）。**只影响本窗这张预览**，见文件头口径 3。
    pub pixelated: bool,
    /// 这张图在 `resources.meta` 里那一档的运行期采样设置（面板里可编辑，「应用」才写盘）。
    pub import_settings: TextureImportSettings,
    /// 磁盘上（或上次应用后）的那一档：与 `import_settings` 不等就表示"改了还没应用"。
    import_saved: TextureImportSettings,
    /// `import_settings` 属于哪张图。与当前 `relative` 不一致时下一趟绘制重读一次磁盘。
    import_for: Option<String>,
    /// 上一次读/写 `resources.meta` 的结果。失败要在这里说，而 `AssetRecord` 那边没有地方说。
    pub import_message: Option<String>,
    /// 解码后的像素缓存：切换采样器只重传纹理，不重读磁盘。
    decoded: Option<(String, egui::ColorImage)>,
    texture: Option<egui::TextureHandle>,
    /// 纹理对应的 `(相对路径, 采样方式)`：任一变化就重传。
    texture_key: Option<(String, bool)>,
    /// 解码失败原因（与"还没预览任何贴图"区分开）。
    pub error: Option<String>,
    /// 解码出来的宽高（信息行用；不拿文件字节去猜"导入后"体积）。
    pub size: (u32, u32),
    /// 源文件字节数。
    pub source_bytes: u64,
}

impl Default for TexturePreviewState {
    fn default() -> Self {
        Self::new()
    }
}

impl TexturePreviewState {
    pub fn new() -> Self {
        Self {
            show: false,
            relative: None,
            zoom: 1.0,
            zoom_follow: true,
            pixelated: false,
            import_settings: TextureImportSettings::default(),
            import_saved: TextureImportSettings::default(),
            import_for: None,
            import_message: None,
            decoded: None,
            texture: None,
            texture_key: None,
            error: None,
            size: (0, 0),
            source_bytes: 0,
        }
    }

    /// 切换/关闭工程：正在预览的路径不再属于这个工程，整份作废（连窗口一起关）。
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// 换图（或点「重新解码」）：丢掉上一张的像素与纹理，下一趟绘制重新解码。
    ///
    /// 顺带作废导入设置的缓存：外部改过 `resources.meta` 之后点这个按钮就该看到新的那一档。
    pub fn invalidate(&mut self) {
        self.decoded = None;
        self.texture = None;
        self.texture_key = None;
        self.error = None;
        self.size = (0, 0);
        self.source_bytes = 0;
        self.import_for = None;
    }

    /// 测试探针：缓存里是否还有像素（换图必须立刻为空）。
    #[cfg(test)]
    pub(crate) fn has_decoded_pixels(&self) -> bool {
        self.decoded.is_some()
    }
}

impl EditorApp {
    /// 资源面板双击贴图的入口：记下目标并开窗。
    /// 解码留给绘制时——这里拿不到 `egui::Context`，也就传不了纹理。
    pub(crate) fn open_texture_preview(&mut self, relative: impl Into<String>) {
        let relative = relative.into();
        if self.texture_preview.relative.as_deref() != Some(relative.as_str()) {
            self.texture_preview.invalidate();
        }
        self.texture_preview.relative = Some(relative);
        self.texture_preview.show = true;
    }

    pub(crate) fn draw_texture_preview(&mut self, context: &egui::Context) {
        if !self.texture_preview.show {
            return;
        }
        let mut open = true;
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        egui::Window::new("贴图预览")
            .open(&mut open)
            .default_width(760.0)
            .default_height(560.0)
            .resizable(true)
            .show(context, |ui| {
                let context = ui.ctx().clone();
                self.draw_texture_preview_body(ui, &root, &context);
            });
        self.texture_preview.show = open && self.texture_preview.show;
    }

    fn draw_texture_preview_body(
        &mut self,
        ui: &mut egui::Ui,
        root: &Path,
        context: &egui::Context,
    ) {
        let Some(relative) = self.texture_preview.relative.clone() else {
            ui.weak("还没有预览目标：在资源面板里双击一张贴图（PNG）。");
            return;
        };
        let fresh = self
            .texture_preview
            .decoded
            .as_ref()
            .is_some_and(|(key, _)| key == &relative);
        if !fresh {
            self.decode_preview_texture(context, root.join(&relative), &relative);
        }

        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&relative).small().weak());
            ui.separator();
            if let Some(error) = &self.texture_preview.error {
                ui.colored_label(egui::Color32::RED, format!("解码失败：{error}"));
            } else if self.texture_preview.size.0 > 0 {
                let (width, height) = self.texture_preview.size;
                let sampler = if self.texture_preview.pixelated {
                    "逐像素"
                } else {
                    "平滑"
                };
                ui.label(format!(
                    "{width}×{height} · RGBA8 · 源文件 {} · 预览采样：{sampler}",
                    byte_text(self.texture_preview.source_bytes)
                ));
            }
        });
        ui.horizontal_wrapped(|ui| {
            let follow = self.texture_preview.zoom_follow;
            ui.add_enabled_ui(!follow, |ui| {
                ui.add(
                    egui::Slider::new(&mut self.texture_preview.zoom, 0.05..=8.0)
                        .text("缩放")
                        .logarithmic(true),
                );
            });
            if ui
                .checkbox(&mut self.texture_preview.zoom_follow, "适应窗口")
                .changed()
            {
                self.texture_preview.invalidate_texture_only();
                context.request_repaint();
            }
            if ui
                .checkbox(&mut self.texture_preview.pixelated, "逐像素")
                .on_hover_text("只改本窗这张预览的采样方式：不写盘、不动运行期（运行期那一档看下面「导入采样」）")
                .changed()
            {
                // 采样方式在纹理上传时就定死了：换档必须重传（像素缓存留着，不重读磁盘）。
                self.texture_preview.invalidate_texture_only();
                context.request_repaint();
            }
            if ui
                .button("重新解码")
                .on_hover_text("外部改过这张图之后重读一次磁盘（工程里其它引用不受影响）")
                .clicked()
            {
                self.texture_preview.invalidate();
                self.decode_preview_texture(context, root.join(&relative), &relative);
            }
            if ui
                .button("在资源面板中定位")
                .on_hover_text("选中并滚到这一行——大图里找路径比翻目录树快")
                .clicked()
            {
                self.pending_resource_locate = Some(PathBuf::from(&relative));
            }
        });
        self.draw_texture_import_row(ui, root, &relative, context);
        ui.separator();

        if self.texture_preview.texture.is_none() {
            // 解码失败的原因已经在上面说过；这里不留第二处文案，也不画假图。
            return;
        }
        let Some(id) = self
            .texture_preview
            .texture
            .as_ref()
            .map(|texture| texture.id())
        else {
            return;
        };
        let (width, height) = self.texture_preview.size;
        let available = ui.available_size();
        let zoom = if self.texture_preview.zoom_follow {
            (available.x / width.max(1) as f32)
                .min(available.y.max(1.0) / height.max(1) as f32)
                .clamp(0.02, 1.0)
        } else {
            self.texture_preview.zoom
        };
        let size = egui::vec2(width as f32 * zoom, height as f32 * zoom);
        egui::ScrollArea::both()
            .id_salt("texture_preview_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // 半透明贴图需要底板才看得出 alpha：整块画成中性灰，
                // 不做棋盘格——一页棋盘在 1024² 上要多上万个矩形，代价不值。
                let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, egui::Color32::from_gray(64));
                painter.image(
                    id,
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            });
    }

    /// 「导入采样」那一行：这一档才是运行期用的（写 `resources.meta` + 换绑定组）。
    ///
    /// 为什么是显式「应用」而不是选完就写盘：与 P1-1 的 Play 调参同一口径——这个窗一次只放
    /// 一张图，但选错一档就是一条 diff，而且写完会把已上传贴图的绑定组换掉（Scene/Play 同时
    /// 变样）。"改了没应用"直接在按钮旁边显示，比事后翻 `git diff` 好解释。
    fn draw_texture_import_row(
        &mut self,
        ui: &mut egui::Ui,
        root: &Path,
        relative: &str,
        context: &egui::Context,
    ) {
        self.ensure_import_settings(root, relative);
        let message = self.texture_preview.import_message.clone();
        ui.horizontal_wrapped(|ui| {
            ui.label("导入采样");
            let mut filter = self.texture_preview.import_settings.filter;
            egui::ComboBox::from_id_salt("texture_import_filter")
                .selected_text(filter_text(filter))
                .show_ui(ui, |ui| {
                    for option in [TextureFilter::Nearest, TextureFilter::Linear] {
                        ui.selectable_value(&mut filter, option, filter_text(option));
                    }
                });
            self.texture_preview.import_settings.filter = filter;
            let mut wrap = self.texture_preview.import_settings.wrap;
            egui::ComboBox::from_id_salt("texture_import_wrap")
                .selected_text(wrap_text(wrap))
                .show_ui(ui, |ui| {
                    for option in [
                        TextureWrap::ClampToEdge,
                        TextureWrap::Repeat,
                        TextureWrap::MirroredRepeat,
                    ] {
                        ui.selectable_value(&mut wrap, option, wrap_text(option));
                    }
                });
            self.texture_preview.import_settings.wrap = wrap;
            let dirty = self.texture_preview.import_settings != self.texture_preview.import_saved;
            ui.add_enabled_ui(dirty, |ui| {
                if ui
                    .button("应用")
                    .on_hover_text(
                        "写进工程根的 resources.meta（入库），并让 Scene/Play 就地换采样器：\
                         不重传像素、不改图片文件本身",
                    )
                    .clicked()
                {
                    self.apply_texture_import_settings(root, relative);
                    context.request_repaint();
                }
            });
            if !dirty {
                ui.weak("已是磁盘上的那一档");
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.weak(
                "滤镜=放大取色，寻址=UV 越出 0..1 时怎么取（平铺才看得见差别）· 只改采样器，不改像素",
            );
            if let Some(message) = message {
                ui.label(message);
            }
        });
    }

    /// 换图（或「重新解码」）后读一次盘；同一张图反复绘制不反复读盘。
    fn ensure_import_settings(&mut self, root: &Path, relative: &str) {
        if self.texture_preview.import_for.as_deref() == Some(relative) {
            return;
        }
        self.texture_preview.import_for = Some(relative.to_owned());
        let loaded = match ResourceMeta::open(root) {
            Ok(meta) => Some(meta.texture_settings(relative)),
            Err(error) => {
                // 读不了不拦面板：按默认档显示并说明原因。「应用」那一步会照实再报一次错。
                self.texture_preview.import_message =
                    Some(format!("resources.meta 读不了：{error}（按默认档显示）"));
                None
            }
        };
        if let Some(settings) = loaded {
            self.texture_preview.import_settings = settings;
            self.texture_preview.import_saved = settings;
        }
    }

    /// 把当前这一档写进 `resources.meta`，并立刻刷新已上传贴图的绑定组。
    fn apply_texture_import_settings(&mut self, root: &Path, relative: &str) {
        let wanted = self.texture_preview.import_settings;
        let mut meta = match ResourceMeta::open(root) {
            Ok(meta) => meta,
            Err(error) => {
                self.texture_preview.import_message =
                    Some(format!("写入失败（读不了现有 resources.meta）：{error}"));
                return;
            }
        };
        if let Err(error) = meta.set_texture_settings(relative, wanted) {
            self.texture_preview.import_message = Some(format!("写入失败（键不合法）：{error}"));
            return;
        }
        if let Err(error) = meta.save() {
            self.texture_preview.import_message = Some(format!("写入失败：{error}"));
            return;
        }
        // 落盘的到底是什么，用**重读**说话：键的规范化、"默认档删行"这些规则只有再过一遍
        // load 才看得见，面板报自己刚写的值是循环论证。
        let disk = match ResourceMeta::open(root) {
            Ok(meta) => meta.texture_settings(relative),
            Err(error) => {
                self.texture_preview.import_message = Some(format!("已写盘但读回失败：{error}"));
                return;
            }
        };
        self.texture_preview.import_settings = disk;
        self.texture_preview.import_saved = disk;
        let reloaded = self.refresh_texture_samplers();
        let note = if disk.is_default() {
            "默认档已从 resources.meta 移除（那文件只登记非默认）".to_owned()
        } else {
            format!("已写入 resources.meta：{}", settings_text(disk))
        };
        self.texture_preview.import_message =
            Some(format!("{note} · 运行期刷新 {reloaded} 张已上传贴图"));
    }

    /// 让 Scene View 与 Play 预览两个渲染器重读那份元数据段；谁不在就跳过谁。
    ///
    /// 不在场时返回 0 是**真话**（没有已上传的贴图要换），不是失败——headless 测试走的就是这条。
    fn refresh_texture_samplers(&mut self) -> usize {
        let mut reloaded = 0;
        if let Some(renderer) = self.scene_renderer.as_mut() {
            reloaded += renderer.reload_texture_settings();
        }
        if let Some(preview) = self.preview.as_mut() {
            reloaded += preview.reload_texture_settings();
        }
        reloaded
    }

    /// 同步解码一张贴图（显式用户动作，一次一张）。
    /// 失败时把原因留在 `error` 里而不是清空窗口——「双击没反应」比「报了解码失败」更难解释。
    fn decode_preview_texture(
        &mut self,
        context: &egui::Context,
        absolute: PathBuf,
        relative: &str,
    ) {
        self.texture_preview.invalidate();
        self.texture_preview.source_bytes =
            std::fs::metadata(&absolute).map_or(0, |meta| meta.len());
        match TextureAsset::load_png(&absolute, TextureColorSpace::Srgb) {
            Ok(asset) => {
                let width = asset.width();
                let height = asset.height();
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [width as usize, height as usize],
                    asset.pixels(),
                );
                self.texture_preview.size = (width, height);
                self.texture_preview.decoded = Some((relative.to_owned(), image));
            }
            Err(error) => {
                self.texture_preview.error = Some(error.to_string());
            }
        }
        self.upload_preview_texture(context);
    }

    /// 把缓存的像素按当前采样方式传成纹理；只留这一张。
    fn upload_preview_texture(&mut self, context: &egui::Context) {
        let Some(key) = self.preview_texture_key() else {
            self.texture_preview.texture = None;
            self.texture_preview.texture_key = None;
            return;
        };
        if self.texture_preview.texture_key.as_ref() == Some(&key)
            && self.texture_preview.texture.is_some()
        {
            return;
        }
        let options = if self.texture_preview.pixelated {
            egui::TextureOptions::NEAREST
        } else {
            egui::TextureOptions::LINEAR
        };
        // 像素从缓存里 clone：切采样方式付一次上传，不付一次磁盘 + PNG 解码。
        let Some((_, image)) = self.texture_preview.decoded.as_ref() else {
            self.texture_preview.texture = None;
            self.texture_preview.texture_key = None;
            return;
        };
        let image = image.clone();
        match self.texture_preview.texture.as_mut() {
            Some(texture) => texture.set(image, options),
            None => {
                self.texture_preview.texture = Some(context.load_texture(
                    format!("texture_preview:{}", key.0),
                    image,
                    options,
                ));
            }
        }
        self.texture_preview.texture_key = Some(key);
    }

    /// 当前该显示哪张纹理：`(相对路径, 采样方式)`；没有可显示的内容时 `None`。
    fn preview_texture_key(&self) -> Option<(String, bool)> {
        let relative = self.texture_preview.relative.clone()?;
        self.texture_preview
            .decoded
            .as_ref()
            .filter(|(key, _)| key == &relative)
            .map(|_| (relative, self.texture_preview.pixelated))
    }
}

impl TexturePreviewState {
    /// 只作废纹理（采样方式或跟随档变了）：像素缓存留着，省下重读磁盘。
    fn invalidate_texture_only(&mut self) {
        self.texture = None;
        self.texture_key = None;
    }
}

/// 滤镜档的中文读数。面板不显示 `nearest`/`linear` 这种 token：作者在这一档要看的是
/// "放大糊不糊"，而 token 只该出现在 `resources.meta` 里（机器读的）。
fn filter_text(filter: TextureFilter) -> &'static str {
    match filter {
        TextureFilter::Nearest => "最近邻·锐利",
        TextureFilter::Linear => "线性·平滑",
    }
}

/// 寻址档的中文读数（同上：token 归文件，人话归面板）。
fn wrap_text(wrap: TextureWrap) -> &'static str {
    match wrap {
        TextureWrap::ClampToEdge => "夹到边",
        TextureWrap::Repeat => "平铺",
        TextureWrap::MirroredRepeat => "镜像平铺",
    }
}

/// 「应用」后的回执文案：两档一起说，省得作者对着两个下拉框回忆刚才选了哪个。
fn settings_text(settings: TextureImportSettings) -> String {
    format!(
        "{} / {}",
        filter_text(settings.filter),
        wrap_text(settings.wrap)
    )
}

/// 体积读数：预览信息里的字节数按 KiB/MiB 说人话（1024 进制）。
fn byte_text(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ResourceMeta, TextureFilter, TextureImportSettings, TexturePreviewState, TextureWrap,
        byte_text,
    };
    use crate::EditorApp;
    use eframe::egui;
    use rustlua_assets::import_settings::RESOURCE_META_FILE;
    use rustlua_infra::logging::LogSink;
    use std::path::{Path, PathBuf};

    /// 默认档以外的两档（写门时用，别在断言里重复拼字面量）。
    const LINEAR_CLAMP: TextureImportSettings =
        TextureImportSettings::new(TextureFilter::Linear, TextureWrap::ClampToEdge);
    const LINEAR_REPEAT: TextureImportSettings =
        TextureImportSettings::new(TextureFilter::Linear, TextureWrap::Repeat);

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-editor-texprev-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        root
    }

    /// 与图集面板测试同一台编码器：写出真 PNG，别用假字节。
    fn write_png(path: &Path, width: u32, height: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut pixels = vec![180u8; (width * height * 4) as usize];
        pixels[0] = 12;
        pixels[3] = 90;
        rustlua_render::texture::save_png_rgba8(path, width, height, &pixels).unwrap();
    }

    fn app(root: PathBuf) -> EditorApp {
        let project = editor::ProjectInfo {
            root,
            version: "test".to_owned(),
            directories: Vec::new(),
        };
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.project_loaded = true;
        app
    }

    /// 画一帧（无 GPU：`egui::Context::default()` 的纹理表是纯 CPU 的）。
    fn draw(app: &mut EditorApp) {
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            app.draw_texture_preview(context);
        });
    }

    #[test]
    fn a_double_click_opens_the_window_and_remembers_the_texture() {
        let root = temp_root("open");
        let mut app = app(root.clone());
        assert!(!app.texture_preview.show, "开窗前不该画这个窗");
        app.open_texture_preview("assets/textures/player.png");
        assert!(app.texture_preview.show);
        assert_eq!(
            app.texture_preview.relative.as_deref(),
            Some("assets/textures/player.png")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_real_png_decodes_at_full_size_and_uploads_one_texture() {
        let root = temp_root("decode");
        // 故意取超过缩略图上限（64px）的尺寸：这一条断言同时证明"没走降采样那条链"。
        write_png(&root.join("assets/textures/a.png"), 128, 96);
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        assert_eq!(
            app.texture_preview.size,
            (128, 96),
            "必须按原尺寸解码，不是缩略图那条链的 64 封顶"
        );
        assert!(app.texture_preview.has_decoded_pixels());
        assert!(app.texture_preview.texture.is_some(), "预览纹理已上传");
        assert_eq!(app.texture_preview.error, None);
        assert!(app.texture_preview.source_bytes > 0, "源文件体积是真实读数");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn switching_textures_drops_the_previous_decode() {
        let root = temp_root("switch");
        write_png(&root.join("assets/textures/a.png"), 8, 4);
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        assert!(app.texture_preview.has_decoded_pixels());

        app.open_texture_preview("assets/textures/b.png");
        assert!(
            !app.texture_preview.has_decoded_pixels(),
            "换图必须当场丢掉上一张的像素缓存"
        );
        draw(&mut app);
        assert!(
            app.texture_preview.error.is_some(),
            "b.png 不存在要报原因，而不是留着一张 a 的图"
        );
        assert_eq!(app.texture_preview.size, (0, 0));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_file_reports_a_reason_instead_of_a_blank_window() {
        let root = temp_root("missing");
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/nope.png");
        draw(&mut app);
        let error = app.texture_preview.error.clone().expect("缺失文件要有原因");
        assert!(!error.is_empty(), "原因不能是空串");
        assert!(app.texture_preview.texture.is_none());
        assert_eq!(app.texture_preview.source_bytes, 0, "读不到的文件没有体积");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn toggling_the_sampler_re_uploads_without_touching_the_disk() {
        let root = temp_root("sampler");
        write_png(&root.join("assets/textures/a.png"), 4, 4);
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        let first = app
            .texture_preview
            .texture
            .as_ref()
            .map(|texture| texture.id());
        // 只作废纹理、保留像素缓存：采样档变了就应该重传，但不重新解码。
        app.texture_preview.pixelated = true;
        app.texture_preview.invalidate_texture_only();
        app.upload_preview_texture(&egui::Context::default());
        assert!(app.texture_preview.has_decoded_pixels(), "切换采样不丢像素");
        assert_eq!(
            app.texture_preview.texture_key,
            Some(("assets/textures/a.png".to_owned(), true))
        );
        assert_ne!(first, None, "重传后仍然有一张可画的纹理");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn closing_the_window_does_not_reopen_it_on_the_next_frame() {
        let root = temp_root("close");
        write_png(&root.join("assets/textures/a.png"), 4, 4);
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        assert!(app.texture_preview.show);
        app.texture_preview.show = false;
        draw(&mut app);
        assert!(!app.texture_preview.show, "关掉的窗不会被下一帧重新打开");
        let _ = std::fs::remove_dir_all(root);
    }

    /// 用**生产端**写一份元数据段（面板读的就是这一份，门不该另拼一种格式）。
    fn document(root: &Path, relative: &str, settings: TextureImportSettings) {
        let mut meta = ResourceMeta::open(root).unwrap();
        meta.set_texture_settings(relative, settings).unwrap();
        meta.save().unwrap();
    }

    /// 元数据段现在这张图是哪一档（重开一次读，别用面板的缓存自证）。
    fn documented(root: &Path, relative: &str) -> TextureImportSettings {
        ResourceMeta::open(root).unwrap().texture_settings(relative)
    }

    #[test]
    fn the_import_row_shows_the_documented_settings_for_this_texture() {
        let root = temp_root("import-read");
        write_png(&root.join("assets/textures/a.png"), 4, 4);
        document(&root, "assets/textures/a.png", LINEAR_REPEAT);
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        assert_eq!(app.texture_preview.import_settings, LINEAR_REPEAT);
        assert_eq!(
            app.texture_preview.import_for.as_deref(),
            Some("assets/textures/a.png"),
            "读来的是哪张图要记下来，换图才知道该重读"
        );
        assert_eq!(
            app.texture_preview.import_saved, LINEAR_REPEAT,
            "刚读来就是\"已应用\"态：没改过不该亮着「应用」"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn applying_writes_the_meta_file_keyed_by_the_relative_path() {
        let root = temp_root("import-apply");
        write_png(&root.join("assets/textures/a.png"), 4, 4);
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        assert_eq!(
            app.texture_preview.import_settings,
            TextureImportSettings::default(),
            "没登记过的图显示默认档"
        );
        app.texture_preview.import_settings = LINEAR_CLAMP;
        app.apply_texture_import_settings(&root, "assets/textures/a.png");
        assert_eq!(documented(&root, "assets/textures/a.png"), LINEAR_CLAMP);
        // 键必须是工程相对路径：绝对路径换一台机器就失效，UUID 会随导入器升版漂移。
        let meta = ResourceMeta::open(&root).unwrap();
        assert_eq!(
            meta.entries().next().unwrap().0,
            "assets/textures/a.png",
            "键的口径跑掉了：{:?}",
            meta.entries().map(|(key, _)| key).collect::<Vec<_>>()
        );
        let message = app.texture_preview.import_message.clone().unwrap();
        assert!(message.contains("已写入"), "{message}");
        assert!(
            message.contains("0 张已上传贴图"),
            "headless 下没有渲染器就照实报 0，不能含糊过去：{message}"
        );
        assert_eq!(
            app.texture_preview.import_saved, LINEAR_CLAMP,
            "应用后回到非脏态"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn applying_the_default_back_removes_the_line_from_the_project() {
        let root = temp_root("import-revert");
        write_png(&root.join("assets/textures/a.png"), 4, 4);
        document(&root, "assets/textures/a.png", LINEAR_CLAMP);
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        assert_eq!(app.texture_preview.import_settings, LINEAR_CLAMP);
        app.texture_preview.import_settings = TextureImportSettings::default();
        app.apply_texture_import_settings(&root, "assets/textures/a.png");
        let meta = ResourceMeta::open(&root).unwrap();
        assert!(meta.is_empty(), "回到默认不该占 diff：{meta:?}");
        assert!(
            !meta.path().exists(),
            "空表连文件一起删：工程里不该留一份只有表头的噪音"
        );
        assert!(
            app.texture_preview
                .import_message
                .as_deref()
                .unwrap()
                .contains("移除"),
            "{:?}",
            app.texture_preview.import_message
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn switching_textures_re_reads_the_settings_from_disk() {
        let root = temp_root("import-switch");
        write_png(&root.join("assets/textures/a.png"), 4, 4);
        write_png(&root.join("assets/textures/b.png"), 4, 4);
        document(&root, "assets/textures/a.png", LINEAR_CLAMP);
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        assert_eq!(app.texture_preview.import_settings, LINEAR_CLAMP);
        // 外部改盘（另一台机器 checkout 回来、或手改了那份文件）后换图再换回来：重读，不用缓存。
        document(
            &root,
            "assets/textures/a.png",
            TextureImportSettings::default(),
        );
        app.open_texture_preview("assets/textures/b.png");
        draw(&mut app);
        assert_eq!(
            app.texture_preview.import_settings,
            TextureImportSettings::default(),
            "b 从没登记过"
        );
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        assert_eq!(
            app.texture_preview.import_settings,
            TextureImportSettings::default(),
            "a 的那一行已经被删掉：面板不许继续显示上一次读到的线性档"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 「重新解码」是**同一张图**重走一遍：`relative` 没变，所以只有 `invalidate()` 能
    /// 作废导入设置的缓存。上面那条换图门走的是"键不一样"，钉不住这条线。
    #[test]
    fn the_redecode_button_drops_the_cached_import_settings_too() {
        let root = temp_root("import-redecode");
        write_png(&root.join("assets/textures/a.png"), 4, 4);
        document(&root, "assets/textures/a.png", LINEAR_CLAMP);
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        assert_eq!(app.texture_preview.import_settings, LINEAR_CLAMP);
        // 外部把那一行删掉（手改那份文件、或另一台机器 checkout 回来），然后点「重新解码」。
        document(
            &root,
            "assets/textures/a.png",
            TextureImportSettings::default(),
        );
        app.texture_preview.invalidate();
        draw(&mut app);
        assert_eq!(
            app.texture_preview.import_settings,
            TextureImportSettings::default(),
            "「重新解码」后还显示上一次读到的线性档：invalidate 没作废导入设置缓存"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 反面对照（口径 3）：预览态那个开关只是本窗的采样，它不该在工程里留下任何文件。
    #[test]
    fn the_preview_pixel_toggle_touches_no_project_data() {
        let root = temp_root("pixel-only");
        write_png(&root.join("assets/textures/a.png"), 4, 4);
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        app.texture_preview.pixelated = true;
        draw(&mut app);
        let meta = ResourceMeta::open(&root).unwrap();
        assert!(
            !meta.path().exists(),
            "勾「逐像素」不该在工程里落一份入库文件：{}",
            meta.path().display()
        );
        assert_eq!(
            app.texture_preview.import_settings,
            TextureImportSettings::default(),
            "预览开关不碰运行期那一档"
        );
        assert_eq!(app.texture_preview.import_message, None, "没写盘就没有回执");
        let _ = std::fs::remove_dir_all(root);
    }

    /// 读不了那份文件时：显示默认档 + 说清原因，而且**不覆盖**别人的文件。
    #[test]
    fn an_unreadable_meta_file_is_reported_and_never_clobbered() {
        let root = temp_root("import-broken");
        write_png(&root.join("assets/textures/a.png"), 4, 4);
        let foreign = "# rustlua-resource-meta-v9\ntexture\ta.png\tlinear\tclamp\n";
        let meta_path = root.join(RESOURCE_META_FILE);
        std::fs::write(&meta_path, foreign).unwrap();
        let mut app = app(root.clone());
        app.open_texture_preview("assets/textures/a.png");
        draw(&mut app);
        assert_eq!(
            app.texture_preview.import_settings,
            TextureImportSettings::default(),
            "读不了按默认档显示，不是按猜的显示"
        );
        assert!(
            app.texture_preview
                .import_message
                .as_deref()
                .unwrap()
                .contains("读不了"),
            "{:?}",
            app.texture_preview.import_message
        );
        // 这时候点应用必须报错而不是"看起来成功了"。
        app.texture_preview.import_settings = LINEAR_CLAMP;
        app.apply_texture_import_settings(&root, "assets/textures/a.png");
        let message = app.texture_preview.import_message.clone().unwrap();
        assert!(message.contains("写入失败"), "{message}");
        assert_eq!(
            std::fs::read_to_string(&meta_path).unwrap(),
            foreign,
            "失败的一趟不能把别人那份文件改掉"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clearing_the_preview_state_forgets_the_texture_and_the_reason() {
        let mut state = TexturePreviewState::new();
        state.relative = Some("assets/textures/a.png".to_owned());
        state.show = true;
        state.error = Some("解码失败".to_owned());
        state.size = (8, 8);
        state.clear();
        assert!(!state.show);
        assert_eq!(state.relative, None);
        assert_eq!(state.error, None);
        assert_eq!(state.size, (0, 0));
        assert_eq!(state.zoom, 1.0, "清完回到默认档");
    }

    #[test]
    fn byte_text_switches_units_at_the_right_boundaries() {
        assert_eq!(byte_text(0), "0 B");
        assert_eq!(byte_text(1023), "1023 B");
        assert_eq!(byte_text(1024), "1.0 KiB");
        assert_eq!(byte_text(3 * 1024 * 1024), "3.0 MiB");
        assert_eq!(
            byte_text(1024 * 1024 - 1),
            "1024.0 KiB",
            "不到 1 MiB 就说 KiB"
        );
    }
}
