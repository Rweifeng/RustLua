//! 标题栏 + 菜单栏 + 崩溃恢复提示（编辑器顶部区域）。

use crate::EditorApp;
use crate::util::{choose_project_directory, icon_toggle, open_in_explorer, strip_verbatim_prefix};
use crate::workspace_root;
use eframe::egui;
use std::path::Path;

impl EditorApp {
    /// 菜单栏（标题栏已移除，项目路径显示在窗口标题）。
    pub(crate) fn draw_header_panel(&mut self, context: &egui::Context) {
        // 把「RustLua 编辑器 - <项目路径>」同步到窗口标题（去除 \\?\ 前缀）。
        self.sync_window_title(context);
        egui::TopBottomPanel::top("menu_bar").show(context, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                // 菜单结构是数据（`crate::menu::MENU_BAR`，按 Unity 的七个顶级菜单组织）：
                // 这里只负责渲染 + 把动作交回 `run_menu_action`。
                self.draw_menu_bar(ui);
                // 旧的内联菜单（项目/编辑/视图/帮助/恢复）已迁移到 `crate::menu`：
                // 结构与动作都在那边的声明表里，这里不再逐个按钮地拼。

                // 右侧：状态 + 资源/控制台面板开关。
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_toggle(ui, self.show_lua_console, "⌨", "显示或隐藏控制台").clicked()
                    {
                        self.show_lua_console = !self.show_lua_console;
                    }
                    if icon_toggle(ui, self.show_resources_panel, "▤", "显示或隐藏资源面板")
                        .clicked()
                    {
                        self.show_resources_panel = !self.show_resources_panel;
                    }
                    ui.separator();
                    ui.label(format!("状态：{}", self.status));
                });
            });
        });
    }

    /// 把「RustLua 编辑器 - <项目路径>」同步到窗口标题（路径去除 \\?\ 前缀）。
    fn sync_window_title(&mut self, context: &egui::Context) {
        let title = self.window_title();
        if self.last_window_title.as_deref() != Some(title.as_str()) {
            self.last_window_title = Some(title.clone());
            context.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    fn window_title(&self) -> String {
        let root = strip_verbatim_prefix(&self.project.root);
        format!("RustLua 编辑器 - {}", root.display())
    }

    /// 恢复一条崩溃快照（E06：dirty 时先经确认网关，确认后才替换文档）。
    pub(crate) fn recover_candidate(&mut self, candidate: editor::RecoveryCandidate) {
        self.request_document_change(crate::actions::PendingChange::Recover { candidate });
    }

    // `perform_recover_candidate` **已搬到 shell**（`actions.rs`，第 28 轮）：它会替换当前文档，
    // 属于提交边界。面板只发确认/typed action。

    /// 重新扫描崩溃恢复目录。
    ///
    /// 失败时**保留上一份扫描结果并把原因写进状态栏**，而不是
    /// `unwrap_or_default()`：后者会把"恢复目录读不了"伪装成"没有任何待恢复内容"，
    /// 用户会以为崩溃前的编辑已经丢了，也看不到环境本身出了问题。
    pub(crate) fn refresh_recovery_scan(&mut self) {
        match self.recovery.scan() {
            Ok(scan) => {
                if !scan.invalid.is_empty() {
                    tracing::warn!(
                        target: "editor",
                        invalid = scan.invalid.len(),
                        "[editor] recovery scan found {} invalid snapshot(s)",
                        scan.invalid.len()
                    );
                }
                self.recovery_scan = scan;
            }
            Err(error) => {
                tracing::warn!(target: "editor", error = %error, "[editor] recovery scan failed; keeping the previous result");
                self.status = format!("扫描崩溃恢复快照失败：{error}");
            }
        }
    }

    /// 菜单：新建项目 —— 选目录，缺啥补啥（project.toml/scripts/scenes/assets），再导入。
    /// E06：目录选定并补齐骨架后，经确认网关切换项目（dirty 时弹确认窗）。
    pub(crate) fn create_new_project(&mut self) {
        let Some(dir) = choose_project_directory() else {
            return;
        };
        if let Err(error) = Self::scaffold_project(&dir) {
            self.status = format!("创建项目骨架失败：{error}");
            return;
        }
        self.request_project_switch(dir);
    }

    /// 菜单：打开项目 —— 选目录并走后台导入流程（E06：dirty 时先确认）。
    pub(crate) fn open_project_menu(&mut self) {
        let Some(dir) = choose_project_directory() else {
            return;
        };
        self.request_project_switch(dir);
    }

    /// 把「新建/打开/最近项目」统一为经未保存确认网关注入后台导入路径。
    fn request_project_switch(&mut self, dir: std::path::PathBuf) {
        self.request_document_change(crate::actions::PendingChange::SwitchProject {
            path: dir.to_string_lossy().into_owned(),
        });
    }

    /// 帮助 > 版本说明窗口。
    pub(crate) fn draw_about_window(&mut self, context: &egui::Context) {
        if !self.show_about {
            return;
        }
        egui::Window::new("版本说明")
            .open(&mut self.show_about)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(context, |ui| {
                ui.label("RustLua 引擎编辑器");
                ui.label(format!("编辑器版本：{}", env!("CARGO_PKG_VERSION")));
                ui.label(format!("项目基线：{}", self.project.version));
                ui.label(format!(
                    "项目路径：{}",
                    crate::util::display_path(&self.project.root)
                ));
                // 插件加载结果（P4）：`plugin_status` 此前只写不显示，注册/加载失败
                // 在编辑器里完全静默——现在在版本说明里列出来，失败一眼可见。
                if !self.plugin_status.is_empty() {
                    ui.separator();
                    ui.label("插件：");
                    for line in &self.plugin_status {
                        ui.label(format!("· {line}"));
                    }
                }
            });
    }

    /// 菜单：使用手册 —— 在资源管理器中打开 docs 目录（docs 位于工作区根，不在 engine/ 下）。
    pub(crate) fn open_help(&mut self) {
        let docs = workspace_root().join("docs");
        if docs.is_dir() {
            open_in_explorer(&docs);
        } else {
            self.status = "未找到 docs 目录".to_owned();
        }
    }

    /// 若目录缺少游戏项目结构，补一个最小骨架（project.toml + main.lua + main.scene + assets/）。
    fn scaffold_project(dir: &Path) -> Result<(), String> {
        use std::io::Write;
        let fs_err = |error: std::io::Error| error.to_string();
        std::fs::create_dir_all(dir.join("assets/fonts")).map_err(fs_err)?;
        let font_path = dir.join("assets/fonts/NotoSansSC-VF.ttf");
        if !font_path.exists() {
            let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("fonts/NotoSansSC-VF.ttf");
            std::fs::copy(&source, &font_path).map_err(fs_err)?;
        }
        std::fs::create_dir_all(dir.join("scripts")).map_err(fs_err)?;
        std::fs::create_dir_all(dir.join("scenes")).map_err(fs_err)?;
        if !dir.join("project.toml").is_file() {
            let name = dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("new_project");
            let mut file = std::fs::File::create(dir.join("project.toml")).map_err(fs_err)?;
            writeln!(file, "[project]").map_err(fs_err)?;
            writeln!(file, "name = \"{name}\"").map_err(fs_err)?;
            writeln!(file, "version = \"0.1.0\"").map_err(fs_err)?;
            writeln!(file, "engine = \"RustLua\"").map_err(fs_err)?;
            writeln!(file, "main_script = \"scripts/main.lua\"").map_err(fs_err)?;
            writeln!(file, "startup_scene = \"scenes/main.scene\"").map_err(fs_err)?;
            writeln!(file, "font = \"assets/fonts/NotoSansSC-VF.ttf\"").map_err(fs_err)?;
            writeln!(file, "font_weight = 400.0").map_err(fs_err)?;
            writeln!(file, "text_render_mode = \"auto\"").map_err(fs_err)?;
        }
        if !dir.join("scripts/main.lua").is_file() {
            std::fs::write(
                dir.join("scripts/main.lua"),
                "-- 新建项目入口\nfunction on_create()\n    log.info(\"scene created\")\nend\n\nfunction on_update(dt)\nend\n",
            )
            .map_err(fs_err)?;
        }
        if !dir.join("scenes/main.scene").is_file() {
            std::fs::write(
                dir.join("scenes/main.scene"),
                format!(
                    "scene_version={}\nname=main\n",
                    engine::core::scene::CURRENT_SCENE_VERSION
                ),
            )
            .map_err(fs_err)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::{SceneDocument, open_project};
    use engine::core::{logging::LogSink, scene::SceneEntityId};
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn unique_token() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos()
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("m14-03-header-{label}-{}", unique_token()))
    }

    fn sample_project_root() -> PathBuf {
        engine::core::games_dir().join("examples").join("game_api")
    }

    fn copy_dir(source: &Path, destination: &Path) {
        if !source.is_dir() {
            return;
        }
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let source_path = entry.path();
            let target_path = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                fs::create_dir_all(&target_path).unwrap();
                copy_dir(&source_path, &target_path);
            } else {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::copy(&source_path, &target_path).unwrap();
            }
        }
    }

    fn copy_sample_project(label: &str) -> PathBuf {
        let source = sample_project_root();
        assert!(
            source.join("project.toml").is_file(),
            "sample project missing: {}",
            source.display()
        );
        let root = temp_root(label);
        fs::create_dir_all(&root).unwrap();
        fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
        copy_dir(&source.join("scripts"), &root.join("scripts"));
        copy_dir(&source.join("scenes"), &root.join("scenes"));
        copy_dir(&source.join("assets"), &root.join("assets"));
        root
    }

    #[test]
    fn scaffold_project_creates_font_and_config() {
        let root = temp_root("scaffold");
        fs::create_dir_all(&root).unwrap();

        EditorApp::scaffold_project(&root).unwrap();

        let font = root.join("assets/fonts/NotoSansSC-VF.ttf");
        assert!(font.is_file());
        let project = fs::read_to_string(root.join("project.toml")).unwrap();
        assert!(project.contains("font = \"assets/fonts/NotoSansSC-VF.ttf\""));
        assert!(project.contains("font_weight = 400.0"));
        assert!(project.contains("text_render_mode = \"auto\""));

        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn app_startup_scans_recovery_and_recover_candidate_restores_scene() {
        let root = copy_sample_project("recover-menu");
        let scene_path = root.join("scenes/api_gameplay.scene");
        let manager = editor::RecoveryManager::new(root.join(".temp"));
        let mut modified = SceneDocument::open(&scene_path).unwrap();
        let original = modified.clone();
        modified
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .name = "RecoveredViaMenu".to_owned();
        modified.mark_dirty();
        manager.autosave(&modified).unwrap();

        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        assert_eq!(app.recovery_scan.candidates.len(), 1);

        app.document.current_scene = Some(SceneDocument::open(&scene_path).unwrap());
        app.selection
            .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);

        let candidate = app.recovery_scan.candidates[0].clone();
        app.recover_candidate(candidate.clone());

        let recovered = app.document.current_scene.as_ref().unwrap();
        assert_eq!(recovered.path, Some(scene_path.clone()));
        assert!(recovered.is_dirty());
        assert_eq!(
            recovered.scene.entities[&SceneEntityId(2)].name,
            "RecoveredViaMenu"
        );
        assert!(app.selection.is_empty());
        assert!(
            app.status.contains("已恢复崩溃快照"),
            "status should report recovery result"
        );

        let reopened = SceneDocument::open(&scene_path).unwrap();
        assert_eq!(
            reopened.scene.entities[&SceneEntityId(2)].name,
            original.scene.entities[&SceneEntityId(2)].name,
            "recovering a candidate must not overwrite the formal scene file"
        );
        let _ = fs::remove_dir_all(root);
    }
}
