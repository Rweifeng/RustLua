use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use editor::{
    AutoSaveController, CommandHistory, CommandKind, InspectorState, PlaySession, ProjectDocument,
    ProjectError, ProjectInfo, RecoveryManager, RecoveryScan, ResourceBrowserState, SceneTreeState,
    TransformFields, check_project, open_project,
};
use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
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

fn default_engine_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments().map_err(|message| format!("参数错误：{message}"))?;
    if arguments.check {
        let project_path = arguments
            .project
            .ok_or("--check requires --project <path>")?;
        print!(
            "{}",
            check_project(&project_path)
                .map_err(|error| format_project_error(&project_path, error))?
        );
        return Ok(());
    }

    let project = arguments
        .project
        .as_ref()
        .map(|path| open_project(path).map_err(|error| format_project_error(path, error)))
        .transpose()?;
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "P2 RustLua 编辑器",
        options,
        Box::new(move |creation_context| {
            configure_cjk_fonts(&creation_context.egui_ctx);
            let mut app = EditorApp::new(project.unwrap_or_else(|| ProjectInfo {
                root: default_engine_path(),
                version: "engine-only".to_owned(),
                directories: Vec::new(),
            }));
            app.project_loaded = arguments.project.is_some();
            app.refresh_resources();
            Ok(Box::new(app))
        }),
    )?;
    Ok(())
}

fn format_project_error(path: &Path, error: ProjectError) -> String {
    format!("无法打开项目 {}：{}", path.display(), error)
}

const CJK_FONT_FILE_NAMES: &[&str] = &["NotoSansSC-VF.ttf", "simhei.ttf", "Deng.ttf"];

fn cjk_font_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fonts")
            .join("NotoSansSC-VF.ttf"),
    ];
    if let Ok(executable) = env::current_exe()
        && let Some(parent) = executable.parent()
    {
        candidates.extend(
            CJK_FONT_FILE_NAMES
                .iter()
                .map(|name| parent.join("fonts").join(name)),
        );
        candidates.extend(CJK_FONT_FILE_NAMES.iter().map(|name| parent.join(name)));
    }
    if cfg!(target_os = "windows") {
        candidates.extend(
            CJK_FONT_FILE_NAMES
                .iter()
                .map(|name| PathBuf::from("C:/Windows/Fonts").join(name)),
        );
    }
    candidates
}

fn configure_cjk_fonts(context: &egui::Context) {
    let Some(path) = cjk_font_candidates()
        .into_iter()
        .find(|path| path.is_file())
    else {
        eprintln!("editor: no CJK font found; Chinese text may render as tofu boxes");
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("editor: failed to read CJK font {}", path.display());
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("p2-cjk".to_owned(), FontData::from_owned(bytes).into());
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "p2-cjk".to_owned());
    }
    context.set_fonts(fonts);
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
    parent_entity: String,
    status: String,
    play_session: Option<PlaySession>,
    history: CommandHistory,
    recovery: RecoveryManager,
    autosave: AutoSaveController,
    recovery_scan: RecoveryScan,
    project_loaded: bool,
    import_path: String,
}

impl EditorApp {
    fn new(project: ProjectInfo) -> Self {
        let project_loaded = project.root.join("project.toml").is_file();
        let recovery = RecoveryManager::new(project.root.join(".codex/recovery"));
        let recovery_scan = recovery.scan().unwrap_or_default();
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
            parent_entity: String::new(),
            status: "尚未创建场景".to_owned(),
            play_session: None,
            history: CommandHistory::new(),
            recovery,
            autosave: AutoSaveController::new(Duration::from_secs(10)),
            recovery_scan,
            project_loaded,
            import_path: String::new(),
        }
    }

    fn import_game_project(&mut self) {
        let path = self.import_path.trim();
        if path.is_empty() {
            self.status = "Enter a game project directory first".to_owned();
            return;
        }
        match open_project(path) {
            Ok(project) if project.root.join("project.toml").is_file() => {
                self.project = project.clone();
                self.document = ProjectDocument::open(project.clone());
                self.recovery = RecoveryManager::new(project.root.join(".codex/recovery"));
                self.recovery_scan = self.recovery.scan().unwrap_or_default();
                self.history.clear();
                self.autosave.reset();
                self.lua_runtime = None;
                self.play_session = None;
                self.tree.clear_selection();
                self.inspector.clear_selection();
                self.project_loaded = true;
                self.refresh_resources();
                self.status = format!("Imported game project: {}", project.root.display());
            }
            Ok(_) => {
                self.status = "Not a standalone game project: project.toml is missing".to_owned()
            }
            Err(error) => self.status = format_project_error(Path::new(path), error),
        }
    }

    fn undo(&mut self) {
        let label = {
            let h = &mut self.history;
            let d = &mut self.document;
            d.current_scene
                .as_mut()
                .and_then(|x| h.undo(x).ok())
                .map(str::to_owned)
        };
        self.status = label.map_or_else(|| "Nothing to undo".into(), |x| format!("Undo: {x}"));
    }
    fn redo(&mut self) {
        let label = {
            let h = &mut self.history;
            let d = &mut self.document;
            d.current_scene
                .as_mut()
                .and_then(|x| h.redo(x).ok())
                .map(str::to_owned)
        };
        self.status = label.map_or_else(|| "Nothing to redo".into(), |x| format!("Redo: {x}"));
    }
    fn save_scene(&mut self) {
        if !self.project_loaded {
            self.status = "Import a game project before editing".to_owned();
            return;
        }
        let Some(doc) = self.document.current_scene.as_mut() else {
            self.status = "No scene".into();
            return;
        };
        let r = if doc.path.is_some() {
            self.recovery.save_document(doc).map_err(|e| e.to_string())
        } else {
            doc.save_as(
                self.project
                    .root
                    .join("scenes")
                    .join(format!("{}.scene", doc.scene.name)),
            )
            .map_err(|e| e.to_string())
        };
        self.status = match r {
            Ok(()) => {
                if let Some(doc) = self.document.current_scene.as_ref() {
                    let _ = self.recovery.clear_for(doc);
                }
                self.autosave.reset();
                self.recovery_scan = self.recovery.scan().unwrap_or_default();
                "Scene saved".into()
            }
            Err(error) => error,
        };
    }

    fn refresh_resources(&mut self) {
        self.resources.set_filter(self.resources_filter.clone());
        if !self.project_loaded {
            self.resources = ResourceBrowserState::new();
            return;
        }
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

    fn is_playing(&self) -> bool {
        self.play_session.is_some()
    }

    fn current_scene(&self) -> Option<&editor::SceneDocument> {
        self.play_session
            .as_ref()
            .map(PlaySession::runtime_document)
            .or(self.document.current_scene.as_ref())
    }

    fn play(&mut self) {
        if self.is_playing() {
            self.status = "Not currently in Play mode".to_owned();
            return;
        }
        let Some(document) = self.current_scene() else {
            self.status = "Please create or open a scene first".to_owned();
            return;
        };
        self.play_session = Some(PlaySession::start(document));
        self.status = "Play mode started; runtime changes stay isolated".to_owned();
    }

    fn stop(&mut self) {
        let Some(session) = self.play_session.take() else {
            self.status = "Not currently in Play mode".to_owned();
            return;
        };
        self.document.current_scene = Some(session.stop());
        self.tree.clear_selection();
        self.inspector.clear_selection();
        self.status = "Play stopped; runtime changes were discarded".to_owned();
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
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(selected) = self.inspector.selected() else {
            self.status = "??????".to_owned();
            return;
        };
        let name = self.entity_name.clone();
        let transform = self.transform;
        let sprite = self.sprite_reference.clone();
        let lua = self.lua_script.clone();
        let inspector = self.inspector.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "??????".to_owned();
            return;
        };
        let result =
            self.history
                .execute(scene, "Inspector", CommandKind::Property, move |scene| {
                    inspector.set_name(scene, name)?;
                    inspector.set_transform(scene, transform)?;
                    if sprite.trim().is_empty() {
                        inspector.clear_sprite_reference(scene)?;
                    } else {
                        inspector.set_sprite_reference(scene, sprite)?;
                    }
                    if lua.trim().is_empty() {
                        inspector.clear_lua_script(scene)?;
                    } else {
                        inspector.set_lua_script(scene, lua)?;
                    }
                    Ok::<(), editor::InspectorError>(())
                });
        self.status = match result {
            Ok(_) => format!("Inspector ??????EntityId={}?", selected.0),
            Err(error) => error.to_string(),
        };
    }
    fn create_scene(&mut self) {
        if !self.project_loaded {
            self.status = "Import a game project before editing".to_owned();
            return;
        }
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }

        let name = self.new_scene_name.trim().to_owned();
        if name.is_empty() {
            self.status = "场景名称不能为空".to_owned();
            return;
        }
        self.document.new_scene(name.clone());
        self.history.clear();
        self.autosave.reset();
        self.tree.clear_selection();
        self.inspector.clear_selection();
        self.status = format!("已创建场景 {name}（未保存）");
    }

    fn add_entity(&mut self, parent: Option<SceneEntityId>) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let name = self.entity_name.trim().to_owned();
        if name.is_empty() {
            self.status = "Entity name cannot be empty".into();
            return;
        }
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "Create entity", CommandKind::Create, move |scene| {
                    let mut tree = tree;
                    tree.create_entity(scene, name.clone(), parent)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
        match result {
            Ok(true) => {
                if let Some(scene) = self.document.current_scene.as_ref()
                    && let Some(id) = scene.scene.entities.keys().max_by_key(|id| id.0).copied()
                {
                    let _ = self.tree.select(scene, id);
                    self.sync_inspector(id);
                }
                self.status = "Entity created".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    fn rename_selected(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let name = self.entity_name.trim().to_owned();
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "Rename entity", CommandKind::Rename, move |scene| {
                    let mut tree = tree;
                    tree.rename_selected(scene, name).map_err(|e| e.to_string())
                });
        match result {
            Ok(true) => {
                self.status = "Entity renamed".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    fn reparent_selected(&mut self, parent: Option<SceneEntityId>) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result = self.history.execute(
            scene,
            "Reparent entity",
            CommandKind::Reparent,
            move |scene| {
                let mut tree = tree;
                tree.reparent_selected(scene, parent)
                    .map_err(|e| e.to_string())
            },
        );
        match result {
            Ok(true) => {
                self.status = "Parent updated".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    fn delete_selected(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "Delete entity", CommandKind::Delete, move |scene| {
                    let mut tree = tree;
                    tree.delete_selected(scene).map_err(|e| e.to_string())
                });
        match result {
            Ok(true) => {
                if self.tree.selected().is_some_and(|id| {
                    self.document
                        .current_scene
                        .as_ref()
                        .is_none_or(|scene| !scene.scene.entities.contains_key(&id))
                }) {
                    self.tree.clear_selection();
                    self.inspector.clear_selection();
                }
                self.status = "Entity deleted".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if self.project_loaded
            && let Some(doc) = self.document.current_scene.as_ref()
        {
            let _ = self.autosave.tick(&self.recovery, doc, SystemTime::now());
        }
        if self.project_loaded
            && context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z))
        {
            self.undo();
        }
        if self.project_loaded
            && context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Y))
        {
            self.redo();
        }
        egui::TopBottomPanel::top("header").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.heading("P2 RustLua 编辑器");
                ui.separator();
                ui.label(format!("项目：{}", self.project.root.display()));
                if ui.button("Undo").clicked() {
                    self.undo();
                }
                if ui.button("Redo").clicked() && self.project_loaded {
                    self.redo();
                }
                if ui.button("Save").clicked() {
                    self.save_scene();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Import game project");
                ui.text_edit_singleline(&mut self.import_path);
                if ui.button("Import").clicked() {
                    self.import_game_project();
                }
            });
            ui.horizontal(|ui| {
                ui.label(format!("场景：{}", self.scene_name()));
                if self.current_scene().is_some_and(|document| document.dirty) {
                    ui.colored_label(egui::Color32::YELLOW, "● 未保存");
                } else {
                    ui.label("已保存");
                }
                ui.label(format!("状态：{}", self.status));
                ui.separator();
                if self.is_playing() {
                    ui.colored_label(egui::Color32::LIGHT_GREEN, "Play");
                    if ui.button("Stop").clicked() {
                        self.stop();
                    }
                } else if ui.button("Play").clicked() && self.project_loaded {
                    self.play();
                }
            });
        });

        if !self.recovery_scan.candidates.is_empty() {
            egui::TopBottomPanel::top("recovery_prompt").show(context, |ui| {
                ui.label("Recovery files detected");
                for candidate in self.recovery_scan.candidates.clone() {
                    if ui.button("Recover").clicked() {
                        self.document.current_scene = Some(self.recovery.recover(&candidate));
                        self.history.clear();
                        self.autosave.reset();
                        self.tree.clear_selection();
                        self.inspector.clear_selection();
                        self.recovery_scan = self.recovery.scan().unwrap_or_default();
                    }
                    if ui.button("Discard").clicked() {
                        let _ = self.recovery.discard(&candidate);
                        self.recovery_scan = self.recovery.scan().unwrap_or_default();
                    }
                }
            });
        }

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
                ui.label("parent id");
                ui.text_edit_singleline(&mut self.parent_entity);
                if ui.button("set parent").clicked() {
                    let parent = self
                        .parent_entity
                        .trim()
                        .parse::<u64>()
                        .ok()
                        .map(SceneEntityId);
                    self.reparent_selected(parent);
                }
                if ui.button("set root").clicked() {
                    self.parent_entity.clear();
                    self.reparent_selected(None);
                }
            });
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
