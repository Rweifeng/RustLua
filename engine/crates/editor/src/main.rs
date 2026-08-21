use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
};

use editor::{
    ProjectDocument, ProjectError, ProjectInfo, SceneTreeState, check_project, open_project,
};
use eframe::egui;
use engine_core::scene::SceneEntityId;

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
        Box::new(move |_creation_context| Ok(Box::new(EditorApp::new(project)))),
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
            new_scene_name: "main".to_owned(),
            entity_name: "Entity".to_owned(),
            status: "尚未创建场景".to_owned(),
        }
    }

    fn current_scene(&self) -> Option<&editor::SceneDocument> {
        self.document.current_scene.as_ref()
    }

    fn scene_name(&self) -> String {
        self.current_scene()
            .map(|document| document.scene.name.clone())
            .unwrap_or_else(|| "未打开场景".to_owned())
    }

    fn create_scene(&mut self) {
        let name = self.new_scene_name.trim().to_owned();
        if name.is_empty() {
            self.status = "场景名称不能为空".to_owned();
            return;
        }
        self.document.new_scene(name.clone());
        self.tree.clear_selection();
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

        egui::CentralPanel::default().show(context, |ui| {
            ui.heading("实体编辑");
            let selected = self.tree.selected();
            if let (Some(scene), Some(id)) = (self.current_scene(), selected) {
                if let Some(entity) = scene.scene.entities.get(&id) {
                    ui.label(format!("名称：{}", entity.name));
                    ui.label(format!("EntityId：{}", entity.id.0));
                    ui.label(format!(
                        "父节点：{}",
                        entity
                            .parent
                            .map_or("无".to_owned(), |parent| parent.0.to_string())
                    ));
                    ui.label(format!("子节点数量：{}", entity.children.len()));
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.entity_name);
                        if ui.button("重命名").clicked() {
                            self.rename_selected();
                        }
                        if ui.button("删除子树").clicked() {
                            self.delete_selected();
                        }
                    });
                    if ui.button("添加子实体").clicked() {
                        self.add_entity(Some(id));
                    }
                }
            } else {
                ui.label("选择一个实体查看和编辑其基本信息");
            }
        });
    }
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
                self.entity_name = name.clone();
            }
        });
        for child in children {
            self.draw_entity(ui, child, depth + 1);
        }
    }
}
