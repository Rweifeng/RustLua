//! 标题栏 + 菜单栏 + 崩溃恢复提示（编辑器顶部区域）。

use crate::EditorApp;
use crate::publish::open_in_explorer;
use crate::util::{choose_project_directory, display_path, strip_verbatim_prefix};
use crate::default_engine_path;
use eframe::egui;
use std::path::Path;

impl EditorApp {
    /// 菜单栏（标题栏已移除，项目路径显示在窗口标题）。
    pub(crate) fn draw_header_panel(&mut self, context: &egui::Context) {
        // 把「RustLua 编辑器 - <项目路径>」同步到窗口标题（去除 \\?\ 前缀）。
        self.sync_window_title(context);
        egui::TopBottomPanel::top("menu_bar").show(context, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("项目", |ui| {
                    if ui.button("新建项目").clicked() {
                        self.create_new_project();
                        ui.close();
                    }
                    if ui.button("打开项目").clicked() {
                        self.open_project_menu();
                        ui.close();
                    }
                    if ui.button("保存项目").clicked() {
                        self.save_scene();
                        ui.close();
                    }
                    ui.separator();
                    ui.checkbox(&mut self.publish_compress, "压缩");
                    ui.checkbox(&mut self.publish_encrypt, "加密");
                    if ui.button("发布项目").clicked() && !self.progress.running {
                        self.start_publish();
                        ui.close();
                    }
                });
                ui.menu_button("编辑", |ui| {
                    // 新建场景（第一个子菜单）。
                    ui.horizontal(|ui| {
                        ui.label("场景名");
                        ui.text_edit_singleline(&mut self.new_scene_name);
                    });
                    if ui.button("新建场景").clicked() {
                        self.create_scene();
                        ui.close();
                    }
                    ui.separator();
                    // 实体操作。
                    ui.horizontal(|ui| {
                        ui.label("实体名称");
                        ui.text_edit_singleline(&mut self.entity_name);
                    });
                    if ui.button("添加根实体").clicked() {
                        self.add_entity(None);
                        ui.close();
                    }
                    if ui.button("添加 3D 实体").clicked() {
                        self.add_3d_entity(None);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("创建预制体").clicked() {
                        self.create_prefab_from_selected();
                        ui.close();
                    }
                });
                ui.menu_button("帮助", |ui| {
                    if ui.button("使用手册").clicked() {
                        self.open_help();
                        ui.close();
                    }
                    if ui.button("版本说明").clicked() {
                        self.show_about = true;
                        ui.close();
                    }
                });
                // 崩溃恢复：检测到未清理的自动保存快照时才出现。
                if !self.recovery_scan.candidates.is_empty() {
                    ui.menu_button("⚠ 恢复", |ui| {
                        ui.label("检测到崩溃恢复快照：");
                        ui.separator();
                        for candidate in self.recovery_scan.candidates.clone() {
                            let label = candidate
                                .original_path
                                .as_ref()
                                .map_or_else(|| display_path(&candidate.path), |path| display_path(path));
                            ui.horizontal(|ui| {
                                ui.label(&label);
                                if ui.button("恢复").clicked() {
                                    self.recover_candidate(candidate.clone());
                                    ui.close();
                                }
                                if ui.button("丢弃").clicked() {
                                    let _ = self.recovery.discard(&candidate);
                                    self.recovery_scan = self.recovery.scan().unwrap_or_default();
                                    ui.close();
                                }
                            });
                        }
                    });
                }
                // 右侧：状态 + 资源/控制台面板开关。
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.toggle_value(&mut self.show_lua_console, "控制台");
                    ui.toggle_value(&mut self.show_resources_panel, "资源");
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

    /// 恢复一条崩溃快照并清理编辑状态。
    fn recover_candidate(&mut self, candidate: editor::RecoveryCandidate) {
        self.document.current_scene = Some(self.recovery.recover(&candidate));
        self.history.clear();
        self.autosave.reset();
        self.tree.clear_selection();
        self.inspector.clear_selection();
        self.recovery_scan = self.recovery.scan().unwrap_or_default();
        self.status = format!("已恢复崩溃快照：{}", display_path(&candidate.path));
    }

    /// 菜单：新建项目 —— 选目录，缺啥补啥（project.toml/scripts/scenes/assets），再导入。
    fn create_new_project(&mut self) {
        let Some(dir) = choose_project_directory() else {
            return;
        };
        if let Err(error) = self.scaffold_project(&dir) {
            self.status = format!("创建项目骨架失败：{error}");
            return;
        }
        self.import_path = dir.to_string_lossy().into_owned();
        self.start_import();
    }

    /// 菜单：打开项目 —— 选目录并走后台导入流程。
    fn open_project_menu(&mut self) {
        let Some(dir) = choose_project_directory() else {
            return;
        };
        self.import_path = dir.to_string_lossy().into_owned();
        self.start_import();
    }

    /// 帮助 > 版本说明窗口。
    pub(crate) fn draw_about_window(&mut self, context: &egui::Context) {
        if !self.show_about {
            return;
        }
        egui::Window::new("版本说明")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(context, |ui| {
                ui.label("RustLua 引擎编辑器（p2）");
                ui.label(format!("编辑器版本：{}", env!("CARGO_PKG_VERSION")));
                ui.label(format!("项目基线：{}", self.project.version));
                ui.label(format!("项目路径：{}", crate::util::display_path(&self.project.root)));
                ui.separator();
                if ui.button("关闭").clicked() {
                    self.show_about = false;
                }
            });
    }

    /// 菜单：使用手册 —— 在资源管理器中打开 docs 目录。
    fn open_help(&mut self) {
        let docs = default_engine_path().join("docs");
        if docs.is_dir() {
            open_in_explorer(&docs);
        } else {
            self.status = "未找到 docs 目录".to_owned();
        }
    }

    /// 若目录缺少游戏项目结构，补一个最小骨架（project.toml + main.lua + main.scene + assets/）。
    fn scaffold_project(&self, dir: &Path) -> Result<(), String> {
        use std::io::Write;
        let fs_err = |error: std::io::Error| error.to_string();
        std::fs::create_dir_all(dir.join("assets")).map_err(fs_err)?;
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
            writeln!(file, "engine = \"p2\"").map_err(fs_err)?;
            writeln!(file, "main_script = \"scripts/main.lua\"").map_err(fs_err)?;
            writeln!(file, "startup_scene = \"scenes/main.scene\"").map_err(fs_err)?;
        }
        if !dir.join("scripts/main.lua").is_file() {
            std::fs::write(
                dir.join("scripts/main.lua"),
                "-- 新建项目入口\nfunction on_create()\n    log.info(\"scene created\")\nend\n\nfunction on_update(dt)\nend\n",
            )
            .map_err(fs_err)?;
        }
        if !dir.join("scenes/main.scene").is_file() {
            std::fs::write(dir.join("scenes/main.scene"), "scene_version=1\nname=main\n")
                .map_err(fs_err)?;
        }
        Ok(())
    }
}
