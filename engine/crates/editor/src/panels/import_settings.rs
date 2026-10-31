//! 「导入设置」面板（属性面板的资源分支）：模型缩放 / 音频音量 / 字体字重。
//!
//! 与贴图那一档「导入采样」同源同口径：写的是工程根的 `resources.meta`
//! （[`ResourceMeta`]）——资源库的**持久那一半**。`.cache/resources.db` 在 `.gitignore`
//! 里（那是可重建缓存），而"这个模型要多大""这个音效多响"是**作者意图**，换一台机器
//! 必须还在，所以它落在工程根那份可 review 的文本文件里。
//!
//! 三个家族的**生效时机不一样**，面板必须说清，否则作者会以为"改了没反应"：
//! - **模型缩放**：画的时候乘进模型矩阵 ⇒ Scene View / Play 立刻生效（刷新一次导入设置即可）；
//! - **音频音量**：与总线音量相乘 ⇒ **下次进入 Play** 生效（音效在载入场景时读那一档）；
//! - **字体字重**：字形图集在载入场景时构建 ⇒ **下次进入 Play** 生效。
//!
//! 写入是显式「应用」而不是选完就写盘：拖一次滑杆就是一条 diff，而且写完会把运行期
//! 读到的那一档换掉。"改了没应用"就写在按钮旁边，比事后翻 `git diff` 好解释。

use std::path::Path;

use eframe::egui;
use rustlua_assets::import_settings::{
    AUDIO_GAIN_MAX, AUDIO_GAIN_MIN, AudioImportSettings, FONT_WEIGHT_MAX, FONT_WEIGHT_MIN,
    FontImportSettings, MODEL_SCALE_MAX, MODEL_SCALE_MIN, ModelImportSettings, ResourceMeta,
};
use rustlua_assets::resource_kind::ResourceKind;

use crate::EditorApp;

/// 有导入设置的三个家族（贴图那一档在「贴图预览」窗里，不在这里）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImportFamily {
    Model,
    Audio,
    Font,
}

impl ImportFamily {
    fn from_kind(kind: ResourceKind) -> Option<Self> {
        match kind {
            ResourceKind::Model => Some(Self::Model),
            ResourceKind::Audio => Some(Self::Audio),
            ResourceKind::Font => Some(Self::Font),
            _ => None,
        }
    }
}

/// 「导入设置」段的状态（挂在 `EditorApp` 上）。
///
/// 草稿值与"磁盘上那一档"分开存：面板要能说清"改了还没应用"，应用后还要用**重读**
/// 回来的值刷新自己（面板报自己刚写的值是循环论证）。
#[derive(Clone, Debug)]
pub(crate) struct ImportSettingsState {
    /// 缓冲属于哪个资源（工程相对路径）；换资源就重读盘。
    relative: Option<String>,
    /// 缓冲属于哪个家族（换家族也要重读：同一路径不会跨家族，但状态是共用的）。
    family: Option<ImportFamily>,
    /// 磁盘上那一档。
    model_scale: f32,
    audio_gain: f32,
    font_weight: Option<u16>,
    /// 面板里的草稿值。
    draft_model_scale: f32,
    draft_audio_gain: f32,
    draft_font_weight: Option<u16>,
    /// 上一次读/写 `resources.meta` 的结果（成功/失败都在这里说）。
    pub message: Option<String>,
}

impl Default for ImportSettingsState {
    fn default() -> Self {
        Self {
            relative: None,
            family: None,
            model_scale: 1.0,
            audio_gain: 1.0,
            font_weight: None,
            draft_model_scale: 1.0,
            draft_audio_gain: 1.0,
            draft_font_weight: None,
            message: None,
        }
    }
}

impl ImportSettingsState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 切换工程：缓冲里的路径按旧工程根解析，跨工程整份作废。
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// 换资源（或换家族）就重读一次盘；同一资源反复绘制不反复读盘。
    fn ensure_loaded(&mut self, root: &Path, relative: &str, family: ImportFamily) {
        if self.relative.as_deref() == Some(relative) && self.family == Some(family) {
            return;
        }
        self.relative = Some(relative.to_owned());
        self.family = Some(family);
        match ResourceMeta::open(root) {
            Ok(meta) => {
                self.model_scale = meta.model_settings(relative).scale;
                self.audio_gain = meta.audio_settings(relative).gain;
                self.font_weight = meta.font_settings(relative).weight;
            }
            Err(error) => {
                // 读不了不拦面板：按默认档显示并说明原因。「应用」那一步会照实再报一次错。
                self.message = Some(format!("resources.meta 读不了：{error}（按默认档显示）"));
                self.model_scale = 1.0;
                self.audio_gain = 1.0;
                self.font_weight = None;
            }
        }
        self.draft_model_scale = self.model_scale;
        self.draft_audio_gain = self.audio_gain;
        self.draft_font_weight = self.font_weight;
    }

    /// 草稿值与磁盘上那一档是否不同（不同才亮「应用」）。
    fn dirty(&self, family: ImportFamily) -> bool {
        match family {
            ImportFamily::Model => self.draft_model_scale != self.model_scale,
            ImportFamily::Audio => self.draft_audio_gain != self.audio_gain,
            ImportFamily::Font => self.draft_font_weight != self.font_weight,
        }
    }
}

impl EditorApp {
    /// 属性面板里的「导入设置」段。返回 `true` = 这一段画了可编辑的东西。
    pub(crate) fn draw_import_settings(
        &mut self,
        ui: &mut egui::Ui,
        kind: ResourceKind,
        relative: &str,
    ) -> bool {
        let Some(family) = ImportFamily::from_kind(kind) else {
            ui.weak(match kind {
                ResourceKind::Texture => {
                    "贴图的采样设置在「贴图预览」窗里改（双击资源面板里的那张图）。"
                }
                _ => "这一类资源没有可导入设置。",
            });
            return false;
        };
        let root = self.project.root.clone();
        self.import_settings.ensure_loaded(&root, relative, family);
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label("导入设置");
            match family {
                ImportFamily::Model => {
                    ui.label("缩放");
                    ui.add(
                        egui::DragValue::new(&mut self.import_settings.draft_model_scale)
                            .speed(0.01)
                            .range(MODEL_SCALE_MIN..=MODEL_SCALE_MAX),
                    );
                }
                ImportFamily::Audio => {
                    ui.label("音量");
                    ui.add(
                        egui::DragValue::new(&mut self.import_settings.draft_audio_gain)
                            .speed(0.01)
                            .range(AUDIO_GAIN_MIN..=AUDIO_GAIN_MAX),
                    );
                }
                ImportFamily::Font => {
                    ui.label("字重");
                    match self.import_settings.draft_font_weight {
                        None => {
                            ui.weak("沿用工程级字重");
                            if ui.button("设为 400").clicked() {
                                self.import_settings.draft_font_weight = Some(400);
                            }
                        }
                        Some(weight) => {
                            let mut weight = weight;
                            ui.add(
                                egui::DragValue::new(&mut weight)
                                    .range(FONT_WEIGHT_MIN..=FONT_WEIGHT_MAX),
                            );
                            self.import_settings.draft_font_weight = Some(weight);
                            if ui.button("沿用工程级").clicked() {
                                self.import_settings.draft_font_weight = None;
                            }
                        }
                    }
                }
            }
        });
        let dirty = self.import_settings.dirty(family);
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(dirty, |ui| {
                if ui
                    .button("应用")
                    .on_hover_text("写进工程根的 resources.meta（入库）")
                    .clicked()
                {
                    self.apply_import_settings(&root, relative, family);
                }
            });
            if !dirty {
                ui.weak("已是磁盘上的那一档");
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.weak(match family {
                ImportFamily::Model => "缩放乘进模型矩阵（不重导网格）· 在下一次绘制时生效",
                ImportFamily::Audio => "音量与总线音量相乘 · 下次进入 Play 时生效",
                ImportFamily::Font => "字重覆盖工程级 font_weight · 下次进入 Play 时生效",
            });
            if let Some(message) = self.import_settings.message.clone() {
                ui.label(message);
            }
        });
        true
    }

    /// 把草稿值写进 `resources.meta`，并按家族刷新运行期。
    fn apply_import_settings(&mut self, root: &Path, relative: &str, family: ImportFamily) {
        let mut meta = match ResourceMeta::open(root) {
            Ok(meta) => meta,
            Err(error) => {
                self.import_settings.message =
                    Some(format!("写入失败（读不了现有 resources.meta）：{error}"));
                return;
            }
        };
        let outcome = match family {
            ImportFamily::Model => meta
                .set_model_settings(
                    relative,
                    ModelImportSettings::new(self.import_settings.draft_model_scale),
                )
                .map(|_| ()),
            ImportFamily::Audio => meta
                .set_audio_settings(
                    relative,
                    AudioImportSettings::new(self.import_settings.draft_audio_gain),
                )
                .map(|_| ()),
            ImportFamily::Font => meta
                .set_font_settings(
                    relative,
                    FontImportSettings::new(self.import_settings.draft_font_weight),
                )
                .map(|_| ()),
        };
        if let Err(error) = outcome {
            self.import_settings.message = Some(format!("写入失败（取值不合法）：{error}"));
            return;
        }
        if let Err(error) = meta.save() {
            self.import_settings.message = Some(format!("写入失败：{error}"));
            return;
        }
        // 落盘的到底是什么，用**重读**说话：键的规范化、"默认档删行"这些规则只有再过一遍
        // load 才看得见。
        let disk = match ResourceMeta::open(root) {
            Ok(meta) => meta,
            Err(error) => {
                self.import_settings.message = Some(format!("已写盘但读回失败：{error}"));
                return;
            }
        };
        let note = match family {
            ImportFamily::Model => {
                let scale = disk.model_settings(relative).scale;
                self.import_settings.model_scale = scale;
                self.import_settings.draft_model_scale = scale;
                if scale == 1.0 {
                    "默认档已从 resources.meta 移除（那文件只登记非默认）".to_owned()
                } else {
                    format!("已写入 resources.meta：缩放 {scale}")
                }
            }
            ImportFamily::Audio => {
                let gain = disk.audio_settings(relative).gain;
                self.import_settings.audio_gain = gain;
                self.import_settings.draft_audio_gain = gain;
                if gain == 1.0 {
                    "默认档已从 resources.meta 移除（那文件只登记非默认）".to_owned()
                } else {
                    format!("已写入 resources.meta：音量 {gain}")
                }
            }
            ImportFamily::Font => {
                let weight = disk.font_settings(relative).weight;
                self.import_settings.font_weight = weight;
                self.import_settings.draft_font_weight = weight;
                match weight {
                    None => "默认档已从 resources.meta 移除（那文件只登记非默认）".to_owned(),
                    Some(weight) => format!("已写入 resources.meta：字重 {weight}"),
                }
            }
        };
        let effect = match family {
            ImportFamily::Model => {
                // 运行期读的那份元数据是缓存的：不刷新的话下一次绘制还用旧档，
                // 面板说"已生效"而画出来没变。
                self.refresh_import_settings();
                "缩放在下一次绘制时生效".to_owned()
            }
            ImportFamily::Audio => "音量在下次进入 Play 时生效".to_owned(),
            ImportFamily::Font => "字重在下次进入 Play 时生效".to_owned(),
        };
        self.import_settings.message = Some(format!("{note} · {effect}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_assets::import_settings::RESOURCE_META_FILE;
    use rustlua_infra::logging::LogSink;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-editor-import-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/models")).unwrap();
        root
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
    fn draw(app: &mut EditorApp, kind: ResourceKind, relative: &str) {
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                app.draw_import_settings(ui, kind, relative);
            });
        });
    }

    /// 元数据段现在是什么（重开一次读，别用面板的缓存自证）。
    fn documented(root: &Path) -> ResourceMeta {
        ResourceMeta::open(root).unwrap()
    }

    /// 面板显示的是磁盘上那一档。
    #[test]
    fn the_panel_shows_the_documented_setting() {
        let root = temp_root("read");
        let mut meta = ResourceMeta::open(&root).unwrap();
        meta.set_model_settings("assets/models/hero.glb", ModelImportSettings::new(0.5))
            .unwrap();
        meta.save().unwrap();
        let mut app = app(root.clone());
        draw(&mut app, ResourceKind::Model, "assets/models/hero.glb");
        assert!(
            (app.import_settings.draft_model_scale - 0.5).abs() < 1e-6,
            "面板没读到磁盘上那一档：{}",
            app.import_settings.draft_model_scale
        );
        assert!(
            !app.import_settings.dirty(ImportFamily::Model),
            "刚读来就是\"已应用\"态：没改过不该亮着「应用」"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 应用把草稿值写进 `resources.meta`，键是**工程相对路径**。
    #[test]
    fn applying_writes_the_meta_file_keyed_by_the_relative_path() {
        let root = temp_root("apply");
        let mut app = app(root.clone());
        draw(&mut app, ResourceKind::Model, "assets/models/hero.glb");
        app.import_settings.draft_model_scale = 0.25;
        app.apply_import_settings(&root, "assets/models/hero.glb", ImportFamily::Model);
        let meta = documented(&root);
        assert!(
            (meta.model_settings("assets/models/hero.glb").scale - 0.25).abs() < 1e-6,
            "写进去的缩放不对：{:?}",
            meta.model_settings("assets/models/hero.glb")
        );
        assert_eq!(
            meta.model_entries().next().unwrap().0,
            "assets/models/hero.glb",
            "键的口径跑掉了：{:?}",
            meta.model_entries().map(|(key, _)| key).collect::<Vec<_>>()
        );
        let message = app.import_settings.message.clone().unwrap();
        assert!(message.contains("已写入"), "{message}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// 写回默认值 = 把那一行从文件里删掉（默认不该占 diff）。
    #[test]
    fn applying_the_default_back_removes_the_line() {
        let root = temp_root("revert");
        let mut meta = ResourceMeta::open(&root).unwrap();
        meta.set_audio_settings("assets/audio/hit.ogg", AudioImportSettings::new(0.25))
            .unwrap();
        meta.save().unwrap();
        let mut app = app(root.clone());
        draw(&mut app, ResourceKind::Audio, "assets/audio/hit.ogg");
        assert!((app.import_settings.draft_audio_gain - 0.25).abs() < 1e-6);
        app.import_settings.draft_audio_gain = 1.0;
        app.apply_import_settings(&root, "assets/audio/hit.ogg", ImportFamily::Audio);
        let meta = ResourceMeta::open(&root).unwrap();
        assert!(meta.is_empty(), "回到默认不该占 diff：{meta:?}");
        assert!(
            !meta.path().exists(),
            "空表连文件一起删：工程里不该留一份只有表头的噪音"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 字重覆盖能写进去，也能**显式撤回**（撤回 = 沿用工程级）。
    #[test]
    fn the_font_weight_can_be_set_and_withdrawn() {
        let root = temp_root("font");
        let mut app = app(root.clone());
        draw(&mut app, ResourceKind::Font, "assets/fonts/main.ttf");
        assert_eq!(
            app.import_settings.draft_font_weight, None,
            "没登记过时是\"沿用工程级\"，不是\"默认 400\""
        );
        app.import_settings.draft_font_weight = Some(700);
        app.apply_import_settings(&root, "assets/fonts/main.ttf", ImportFamily::Font);
        let meta = ResourceMeta::open(&root).unwrap();
        assert_eq!(
            meta.font_settings("assets/fonts/main.ttf").weight,
            Some(700)
        );
        // 撤回：那一行从文件里消失。
        app.import_settings.draft_font_weight = None;
        app.apply_import_settings(&root, "assets/fonts/main.ttf", ImportFamily::Font);
        let meta = ResourceMeta::open(&root).unwrap();
        assert_eq!(meta.font_settings("assets/fonts/main.ttf").weight, None);
        assert!(!meta.path().exists());
        let _ = std::fs::remove_dir_all(root);
    }

    /// 越界取值必须**报错**，不能悄悄夹到范围里（夹了就是在骗人）。
    #[test]
    fn an_out_of_range_value_is_refused_instead_of_being_clamped() {
        let root = temp_root("range");
        let mut app = app(root.clone());
        draw(&mut app, ResourceKind::Model, "assets/models/hero.glb");
        app.import_settings.draft_model_scale = 1e9;
        app.apply_import_settings(&root, "assets/models/hero.glb", ImportFamily::Model);
        let message = app.import_settings.message.clone().unwrap();
        assert!(message.contains("写入失败"), "{message}");
        assert!(
            !ResourceMeta::open(&root).unwrap().path().exists(),
            "被拒绝的一趟不该落盘"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 读不了那份文件时：按默认档显示 + 说清原因，而且**不覆盖**别人的文件。
    #[test]
    fn an_unreadable_meta_file_is_reported_and_never_clobbered() {
        let root = temp_root("broken");
        let foreign = "# rustlua-resource-meta-v9\nmodel\ta.glb\t0.5\n";
        let meta_path = root.join(RESOURCE_META_FILE);
        std::fs::write(&meta_path, foreign).unwrap();
        let mut app = app(root.clone());
        draw(&mut app, ResourceKind::Model, "assets/models/hero.glb");
        assert!(
            (app.import_settings.draft_model_scale - 1.0).abs() < 1e-6,
            "读不了按默认档显示，不是按猜的显示"
        );
        assert!(
            app.import_settings
                .message
                .as_deref()
                .unwrap()
                .contains("读不了"),
            "{:?}",
            app.import_settings.message
        );
        // 这时候点应用必须报错而不是"看起来成功了"。
        app.import_settings.draft_model_scale = 0.5;
        app.apply_import_settings(&root, "assets/models/hero.glb", ImportFamily::Model);
        let message = app.import_settings.message.clone().unwrap();
        assert!(message.contains("写入失败"), "{message}");
        assert_eq!(
            std::fs::read_to_string(&meta_path).unwrap(),
            foreign,
            "失败的一趟不能把别人那份文件改掉"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 贴图那一档在「贴图预览」窗里，这里只做指路（不重复一份编辑器）。
    #[test]
    fn a_texture_points_at_the_preview_window() {
        let root = temp_root("texture");
        let mut app = app(root.clone());
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                assert!(
                    !app.draw_import_settings(ui, ResourceKind::Texture, "assets/a.png"),
                    "贴图不该在这里有可编辑设置"
                );
            });
        });
        let _ = std::fs::remove_dir_all(root);
    }

    /// 换工程：缓冲整份作废（路径按旧工程根解析）。
    #[test]
    fn switching_projects_forgets_the_buffered_setting() {
        let mut app = app(temp_root("clear"));
        app.import_settings.relative = Some("assets/models/hero.glb".to_owned());
        app.import_settings.draft_model_scale = 0.5;
        app.import_settings.clear();
        assert_eq!(app.import_settings.relative, None);
        assert!(
            (app.import_settings.draft_model_scale - 1.0).abs() < 1e-6,
            "清完回到默认档"
        );
        assert_eq!(app.import_settings.message, None);
    }
}
