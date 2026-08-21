use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
};

use editor::{
    InspectorState, ProjectDocument, ProjectError, ProjectInfo, ResourceBrowserState,
    SceneTreeState, TransformFields, check_project, open_project,
};
use eframe::egui;
use engine_core::{
    scene::SceneEntityId,
    script::{LuaRuntime, ScriptDiagnostic},
};

#[derive(Debug, Default)]
struct Arguments {
    project: Option<PathBuf>,
    check: bool,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut arguments = Arguments::default();
    let mut values = env::args().skip(1);
    while let Some(argument) = values.next() {
        match argument.as_str() {
            "--project" => {
                let path = values
                    .next()
                    .ok_or_else(|| "--project 需要一个路径参数".to_owned())?;
                arguments.project = Some(PathBuf::from(path));
            }
            "--check" => arguments.check = true,
            "--help" | "-h" => {
                println!("用法：editor [--project <项目路径>] [--check]");
                std::process::exit(0);
            }
            unknown => return Err(format!("未知参数：{unknown}")),
        }
    }
    Ok(arguments)
}

fn default_project_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments().map_err(|message| format!("参数错误：{message}"))?;
    let project_path = arguments.project.unwrap_or_else(default_project_path);
    if arguments.check {
        print!(
            "{}",
            check_project(&project_path)
                .map_err(|error| format_project_error(&project_path, error))?
        );
        return Ok(());
    }

    let project =
        open_project(&project_path).map_err(|error| format_project_error(&project_path, error))?;
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "P2 RustLua 编辑器",
        options,
        Box::new(move |_creation_context| {
            let mut app = EditorApp::new(project);
            app.refresh_resources();
            Ok(Box::new(app))
        }),
    )?;
    Ok(())
}

fn format_project_error(path: &Path, error: ProjectError) -> String {
    format!("无法打开项目 {}：{}", path.display(), error)
}

struct EditorApp {
    project: ProjectInfo,
    document: ProjectDocument,
    tree: SceneTreeState,
    resources: ResourceBrowserState,
    resources_filter: String,
    lua_runtime: Option<LuaRuntime>,
    console_input: String,
    console_output: Vec<String>,
    console_filter: String,
    inspector: InspectorState,
    transform: TransformFields,
    sprite_reference: String,
    lua_script: String,
    new_scene_name: String,
    entity_name: String,
    status: String,
}

impl EditorApp {
    fn new(project: ProjectInfo) -> Self {
        Self {
            document: ProjectDocument::open(project.clone()),
            project,
            tree: SceneTreeState::new(),
            resources: ResourceBrowserState::new(),
            resources_filter: String::new(),
            lua_runtime: None,
            console_input: String::new(),
            console_output: Vec::new(),
            console_filter: String::new(),
            inspector: InspectorState::new(),
            transform: TransformFields::default(),
            sprite_reference: String::new(),
            lua_script: String::new(),
            new_scene_name: "main".to_owned(),
            entity_name: "Entity".to_owned(),
            status: "尚未创建场景".to_owned(),
        }
    }

    fn refresh_resources(&mut self) {
        self.resources.set_filter(self.resources_filter.clone());
        if let Err(error) = self.resources.refresh(&self.project.root) {
            self.status = error.to_string();
        }
    }

    fn ensure_lua_runtime(&mut self) -> Result<&mut LuaRuntime, String> {
        if self.lua_runtime.is_none() {
            self.lua_runtime = Some(LuaRuntime::new().map_err(|error| error.to_string())?);
        }
        self.lua_runtime
            .as_mut()
            .ok_or_else(|| "Lua VM 未初始化".to_owned())
    }

    fn run_console_command(&mut self) {
        let source = self.console_input.trim().to_owned();
        if source.is_empty() {
            return;
        }
        let result = self.ensure_lua_runtime().and_then(|runtime| {
            runtime
                .execute_console(&source)
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(value) => self.console_output.push(format!("> {source}\n{value}")),
            Err(error) => {
                self.status = error.clone();
                self.console_output
                    .push(format!("> {source}\n错误：{error}"));
            }
        }
        self.console_input.clear();
    }

    fn clear_console(&mut self) {
        self.console_output.clear();
        if let Some(runtime) = self.lua_runtime.as_mut() {
            runtime.clear_diagnostics();
        }
    }

    fn run_selected_script(&mut self) {
        let script = self.lua_script.trim().to_owned();
        if script.is_empty() {
            self.status = "当前实体没有 Lua 脚本引用".to_owned();
            return;
        }
        let path = self.project.root.join(&script);
        let result = self.ensure_lua_runtime().and_then(|runtime| {
            runtime
                .execute_project_script(&path)
                .and_then(|_| runtime.call_update_debug())
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(value) => {
                self.status = format!("Lua 脚本运行成功：{script}");
                self.console_output.push(format!("运行 {script}\n{value}"));
            }
            Err(error) => {
                self.status = error.clone();
                self.console_output
                    .push(format!("运行 {script}\n错误：{error}"));
            }
        }
    }

    fn console_diagnostics(&mut self) -> Vec<ScriptDiagnostic> {
        let filter = self.console_filter.trim().to_owned();
        let Some(runtime) = self.lua_runtime.as_mut() else {
            return Vec::new();
        };
        runtime.set_debug_filter((!filter.is_empty()).then_some(filter));
        runtime.diagnostics()
    }

    fn current_scene(&self) -> Option<&editor::SceneDocument> {
        self.document.current_scene.as_ref()
    }

    fn scene_name(&self) -> String {
        self.current_scene()
            .map(|document| document.scene.name.clone())
            .unwrap_or_else(|| "未打开场景".to_owned())
    }

    fn sync_inspector(&mut self, id: SceneEntityId) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let scene = scene.clone();
        if self.inspector.select(&scene, id).is_ok()
            && let Ok(snapshot) = self.inspector.snapshot(&scene)
        {
            self.entity_name = snapshot.name;
            self.transform = snapshot.transform;
            self.sprite_reference = snapshot.sprite_reference.unwrap_or_default();
            self.lua_script = snapshot.lua_script.unwrap_or_default();
        }
    }

    fn apply_inspector(&mut self) {
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "请先创建场景".to_owned();
            return;
        };
        let result = self
            .inspector
            .set_name(scene, self.entity_name.clone())
            .and_then(|_| self.inspector.set_transform(scene, self.transform))
            .and_then(|_| {
                if self.sprite_reference.trim().is_empty() {
                    self.inspector.clear_sprite_reference(scene)
                } else {
                    self.inspector
                        .set_sprite_reference(scene, self.sprite_reference.clone())
                }
            })
            .and_then(|_| {
                if self.lua_script.trim().is_empty() {
                    self.inspector.clear_lua_script(scene)
                } else {
                    self.inspector
                        .set_lua_script(scene, self.lua_script.clone())
                }
            });
        self.status = match result {
            Ok(()) => "Inspector 修改已应用".to_owned(),
            Err(error) => error.to_string(),
        };
    }
    fn create_scene(&mut self) {
        let name = self.new_scene_name.trim().to_owned();
        if name.is_empty() {
            self.status = "场景名称不能为空".to_owned();
            return;
        }
        self.document.new_scene(name.clone());
        self.tree.clear_selection();
        self.inspector.clear_selection();
        self.status = format!("已创建场景 {name}（未保存）");
    }

    fn add_entity(&mut self, parent: Option<SceneEntityId>) {
        let name = self.entity_name.trim().to_owned();
        if name.is_empty() {
            self.status = "实体名称不能为空".to_owned();
            return;
        }
        let (document, tree) = (&mut self.document, &mut self.tree);
        let Some(scene) = document.current_scene.as_mut() else {
            self.status = "请先创建场景".to_owned();
            return;
        };
        match tree.create_entity(scene, name.clone(), parent) {
            Ok(id) => self.status = format!("已创建实体 {name}（ID={}）", id.0),
            Err(error) => self.status = error.to_string(),
        }
    }

    fn rename_selected(&mut self) {
        let name = self.entity_name.trim().to_owned();
        let (document, tree) = (&mut self.document, &mut self.tree);
        let Some(scene) = document.current_scene.as_mut() else {
            self.status = "请先创建场景".to_owned();
            return;
        };
        match tree.rename_selected(scene, name) {
            Ok(()) => self.status = "实体名称已更新".to_owned(),
            Err(error) => self.status = error.to_string(),
        }
    }

    fn delete_selected(&mut self) {
        let (document, tree) = (&mut self.document, &mut self.tree);
        let Some(scene) = document.current_scene.as_mut() else {
            self.status = "请先创建场景".to_owned();
            return;
        };
        match tree.delete_selected(scene) {
            Ok(()) => self.status = "实体及其子树已删除".to_owned(),
            Err(error) => self.status = error.to_string(),
        }
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("header").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.heading("P2 RustLua 编辑器");
                ui.separator();
                ui.label(format!("项目：{}", self.project.root.display()));
            });
            ui.horizontal(|ui| {
                ui.label(format!("场景：{}", self.scene_name()));
                if self.current_scene().is_some_and(|document| document.dirty) {
                    ui.colored_label(egui::Color32::YELLOW, "● 未保存");
                } else {
                    ui.label("已保存");
                }
                ui.label(format!("状态：{}", self.status));
            });
        });

        egui::SidePanel::left("project_status").show(context, |ui| {
            ui.heading("项目状态");
            ui.label(format!("基线：{}", self.project.version));
            ui.separator();
            for directory in &self.project.directories {
                ui.label(format!("✓ {}/", directory.name));
            }
            ui.separator();
            ui.heading("场景文档");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.new_scene_name);
                if ui.button("新建").clicked() {
                    self.create_scene();
                }
            });
            if let Some(document) = self.current_scene() {
                ui.label(format!(
                    "路径：{}",
                    document
                        .path
                        .as_deref()
                        .map_or_else(|| "未保存".to_owned(), |path| path.display().to_string())
                ));
            }
        });

        let mut refresh_resources = false;
        self.resources.set_filter(self.resources_filter.clone());
        egui::TopBottomPanel::bottom("resource_browser")
            .resizable(true)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("资源浏览器");
                    ui.text_edit_singleline(&mut self.resources_filter);
                    if ui.button("刷新").clicked() {
                        refresh_resources = true;
                    }
                    ui.label(format!("{} 项", self.resources.visible_entries().len()));
                });
                if let Some(error) = self.resources.last_error() {
                    ui.colored_label(egui::Color32::RED, format!("错误：{error}"));
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for entry in self.resources.visible_entries() {
                        let status = format!("{:?}", entry.status);
                        ui.horizontal(|ui| {
                            ui.label(entry.path_string());
                            ui.label(entry.kind.label());
                            ui.label(status);
                            if let Some(error) = &entry.error {
                                ui.colored_label(egui::Color32::RED, error);
                            }
                        });
                    }
                });
            });

        egui::SidePanel::right("scene_tree").show(context, |ui| {
            ui.heading("场景树");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.entity_name);
                if ui.button("添加根实体").clicked() {
                    self.add_entity(None);
                }
            });
            ui.separator();
            let roots: Vec<_> = self
                .current_scene()
                .map(|document| self.tree.roots(document).map(|entity| entity.id).collect())
                .unwrap_or_default();
            for id in roots {
                self.draw_entity(ui, id, 0);
            }
            if self.current_scene().is_none() {
                ui.label("请先新建场景");
            }
        });

        let selected = self.tree.selected();
        let entity_info = selected.and_then(|id| {
            self.current_scene().and_then(|scene| {
                scene.scene.entities.get(&id).map(|entity| {
                    (
                        entity.id.0,
                        entity.parent.map(|parent| parent.0),
                        entity.children.len(),
                    )
                })
            })
        });
        let mut apply_inspector = false;
        let mut rename = false;
        let mut delete = false;
        let mut add_child = false;

        egui::TopBottomPanel::bottom("lua_console")
            .resizable(true)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Lua Console");
                    ui.text_edit_singleline(&mut self.console_filter);
                    if ui.button("运行选中脚本").clicked() {
                        self.run_selected_script();
                    }
                    if ui.button("清空").clicked() {
                        self.clear_console();
                    }
                });
                ui.horizontal(|ui| {
                    let response = ui.text_edit_singleline(&mut self.console_input);
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        self.run_console_command();
                    }
                    if ui.button("执行").clicked() {
                        self.run_console_command();
                    }
                });
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for line in &self.console_output {
                            ui.monospace(line);
                        }
                        for diagnostic in self.console_diagnostics() {
                            ui.colored_label(egui::Color32::RED, format_diagnostic(&diagnostic));
                        }
                    });
            });

        egui::CentralPanel::default().show(context, |ui| {
            ui.heading("实体 Inspector");
            let Some((entity_id, parent, child_count)) = entity_info else {
                ui.label("选择一个实体查看和编辑 Inspector");
                return;
            };

            ui.label(format!("EntityId：{entity_id}"));
            ui.label(format!(
                "父节点：{}",
                parent.map_or_else(|| "无".to_owned(), |id| id.to_string())
            ));
            ui.label(format!("子节点数量：{child_count}"));
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("名称");
                ui.text_edit_singleline(&mut self.entity_name);
                if ui.button("重命名").clicked() {
                    rename = true;
                }
            });
            ui.collapsing("Transform", |ui| {
                ui.horizontal(|ui| {
                    ui.label("位置 X");
                    ui.add(egui::DragValue::new(&mut self.transform.position_x));
                    ui.label("位置 Y");
                    ui.add(egui::DragValue::new(&mut self.transform.position_y));
                });
                ui.horizontal(|ui| {
                    ui.label("旋转");
                    ui.add(egui::DragValue::new(&mut self.transform.rotation));
                });
                ui.horizontal(|ui| {
                    ui.label("缩放 X");
                    ui.add(egui::DragValue::new(&mut self.transform.scale_x));
                    ui.label("缩放 Y");
                    ui.add(egui::DragValue::new(&mut self.transform.scale_y));
                });
            });
            ui.horizontal(|ui| {
                ui.label("Sprite");
                ui.text_edit_singleline(&mut self.sprite_reference);
            });
            ui.horizontal(|ui| {
                ui.label("Lua 脚本");
                ui.text_edit_singleline(&mut self.lua_script);
            });
            if ui.button("应用 Inspector 修改").clicked() {
                apply_inspector = true;
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("添加子实体").clicked() {
                    add_child = true;
                }
                if ui.button("删除子树").clicked() {
                    delete = true;
                }
            });
        });

        if refresh_resources {
            self.refresh_resources();
        }
        if rename {
            self.rename_selected();
        }
        if apply_inspector {
            self.apply_inspector();
        }
        if add_child {
            self.add_entity(selected);
        }
        if delete {
            self.delete_selected();
            self.inspector.clear_selection();
            self.sprite_reference.clear();
            self.lua_script.clear();
            self.transform = TransformFields::default();
        }
    }
}
fn format_diagnostic(diagnostic: &ScriptDiagnostic) -> String {
    let location = diagnostic
        .line
        .map_or_else(|| "未知行".to_owned(), |line| format!("第 {line} 行"));
    format!(
        "[{}] {} {}：{}（重复 {} 次）",
        diagnostic.phase,
        diagnostic.path.display(),
        location,
        diagnostic.message,
        diagnostic.occurrences
    )
}

impl EditorApp {
    fn draw_entity(&mut self, ui: &mut egui::Ui, id: SceneEntityId, depth: usize) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&id) else {
            return;
        };
        let name = entity.name.clone();
        let children = entity.children.clone();
        let selected = self.tree.selected() == Some(id);
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 16.0);
            if ui
                .selectable_label(selected, format!("{} [{}]", name, id.0))
                .clicked()
            {
                if let Some(scene) = self.current_scene() {
                    let scene = scene.clone();
                    let _ = self.tree.select(&scene, id);
                }
                self.sync_inspector(id);
            }
        });
        for child in children {
            self.draw_entity(ui, child, depth + 1);
        }
    }
}
