#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use editor::{
    AutoSaveController, CommandHistory, CommandKind, InspectorState, PlaySession, ProjectDocument,
    ProjectError, ProjectInfo, RecoveryManager, RecoveryScan, ResourceBrowserState, ResourceKind,
    SceneTreeState, TransformFields, check_project, open_project,
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

#[cfg(windows)]
fn choose_project_directory() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListW};

    let title: Vec<u16> = "选择游戏项目目录\0".encode_utf16().collect();
    let mut info = BROWSEINFOW {
        hwndOwner: std::ptr::null_mut(),
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: std::ptr::null_mut(),
        lpszTitle: title.as_ptr(),
        ulFlags: 0x0000_0001,
        lpfn: None,
        lParam: 0,
        iImage: 0,
    };
    let pidl = unsafe { SHBrowseForFolderW(&mut info) };
    if pidl.is_null() {
        return None;
    }
    let mut buffer = [0u16; 32768];
    let selected = unsafe { SHGetPathFromIDListW(pidl, buffer.as_mut_ptr()) != 0 };
    unsafe {
        CoTaskMemFree(pidl.cast());
    }
    if !selected {
        return None;
    }
    let length = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    Some(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

#[cfg(not(windows))]
fn choose_project_directory() -> Option<PathBuf> {
    None
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
    workspace_tab: WorkspaceTab,
    scene_zoom: f32,
    scene_tool: SceneTool,
    hierarchy_filter: String,
    scene_state: SceneEditorState,
    inspector_clipboard: Option<ComponentClipboard>,
    inspector_transform_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceTab {
    Scene,
    Game,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SceneTool {
    Select,
    Move,
    Rotate,
    Scale,
}

#[derive(Default)]
struct SceneEditorState {
    pan: egui::Vec2,
    drag: Option<SceneDrag>,
    preview: std::collections::BTreeMap<SceneEntityId, TransformFields>,
    collapsed: std::collections::BTreeSet<SceneEntityId>,
    hidden: std::collections::BTreeSet<SceneEntityId>,
    locked: std::collections::BTreeSet<SceneEntityId>,
    selected: std::collections::BTreeSet<SceneEntityId>,
    box_select_start: Option<egui::Pos2>,
}

struct SceneDrag {
    ids: Vec<SceneEntityId>,
    start: egui::Pos2,
    initial: std::collections::BTreeMap<SceneEntityId, TransformFields>,
}

#[derive(Clone)]
struct ComponentClipboard {
    components: std::collections::BTreeMap<String, String>,
    resources: std::collections::BTreeMap<String, engine_core::resource_db::AssetId>,
    metadata: std::collections::BTreeMap<String, String>,
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
            workspace_tab: WorkspaceTab::Scene,
            scene_zoom: 1.0,
            scene_tool: SceneTool::Select,
            hierarchy_filter: String::new(),
            scene_state: SceneEditorState::default(),
            inspector_clipboard: None,
            inspector_transform_open: true,
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

    fn open_scene_resource(&mut self, relative_path: PathBuf) {
        if !self.project_loaded {
            return;
        }
        let path = self.project.root.join(&relative_path);
        match self.document.open_scene(&path) {
            Ok(()) => {
                self.history.clear();
                self.autosave.reset();
                self.play_session = None;
                self.tree.clear_selection();
                self.inspector.clear_selection();
                self.scene_state.preview.clear();
                self.scene_state.selected.clear();
                self.scene_state.collapsed.clear();
                self.sync_hierarchy_metadata();
                self.status = format!("已打开场景：{}", relative_path.display());
            }
            Err(error) => self.status = format!("打开场景失败：{error}"),
        }
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
            self.status = "应用修改".to_owned();
            return;
        };
        let name = self.entity_name.clone();
        let transform = self.transform;
        let sprite = self.sprite_reference.clone();
        let lua = self.lua_script.clone();
        let inspector = self.inspector.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "应用修改".to_owned();
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
            Ok(_) => format!("Inspector 已更新：EntityId={}", selected.0),
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
        self.sync_hierarchy_metadata();
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
                if ui.button("Browse...").clicked() {
                    if let Some(path) = choose_project_directory() {
                        self.import_path = path.to_string_lossy().into_owned();
                    }
                }
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
                    let mut scene_to_open = None;
                    for entry in self.resources.visible_entries() {
                        let status = format!("{:?}", entry.status);
                        let path = entry.path.clone();
                        ui.horizontal(|ui| {
                            let response = ui.selectable_label(false, entry.path_string());
                            if response.double_clicked() && entry.kind == ResourceKind::Scene {
                                scene_to_open = Some(path.clone());
                            }
                            ui.label(entry.kind.label());
                            ui.label(status);
                            if let Some(error) = &entry.error {
                                ui.colored_label(egui::Color32::RED, error);
                            }
                        });
                    }
                    if let Some(path) = scene_to_open {
                        self.open_scene_resource(path);
                    }
                });
            });

        egui::SidePanel::left("scene_tree").show(context, |ui| {
            ui.heading("Hierarchy");
            ui.text_edit_singleline(&mut self.hierarchy_filter);
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
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.workspace_tab, WorkspaceTab::Scene, "Scene");
                ui.selectable_value(&mut self.workspace_tab, WorkspaceTab::Game, "Game");
                ui.separator();
                ui.label("2D");
                ui.selectable_value(&mut self.scene_tool, SceneTool::Select, "Select");
                ui.selectable_value(&mut self.scene_tool, SceneTool::Move, "Move");
                ui.selectable_value(&mut self.scene_tool, SceneTool::Rotate, "Rotate");
                ui.selectable_value(&mut self.scene_tool, SceneTool::Scale, "Scale");
                ui.separator();
                ui.add(egui::Slider::new(&mut self.scene_zoom, 0.5..=2.5).text("Zoom"));
            });
            ui.separator();
            match self.workspace_tab {
                WorkspaceTab::Scene => self.draw_scene_view(ui),
                WorkspaceTab::Game => self.draw_game_view(ui),
            }
        });

        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(380.0)
            .min_width(340.0)
            .show(context, |ui| {
                self.draw_inspector_panel(ui);
            });

        if refresh_resources {
            self.refresh_resources();
        }
    }
}
fn encode_transform(transform: TransformFields) -> String {
    format!(
        "x={:.9};y={:.9};rotation={:.9};scale_x={:.9};scale_y={:.9}",
        transform.position_x,
        transform.position_y,
        transform.rotation,
        transform.scale_x,
        transform.scale_y
    )
}

fn rotated_corners(center: egui::Pos2, size: egui::Vec2, rotation_degrees: f32) -> [egui::Pos2; 4] {
    let angle = rotation_degrees.to_radians();
    let corners = [
        egui::vec2(-size.x * 0.5, -size.y * 0.5),
        egui::vec2(size.x * 0.5, -size.y * 0.5),
        egui::vec2(size.x * 0.5, size.y * 0.5),
        egui::vec2(-size.x * 0.5, size.y * 0.5),
    ];
    corners.map(|p| {
        center
            + egui::vec2(
                p.x * angle.cos() - p.y * angle.sin(),
                p.x * angle.sin() + p.y * angle.cos(),
            )
    })
}

fn rotated_contains(
    center: egui::Pos2,
    size: egui::Vec2,
    rotation_degrees: f32,
    point: egui::Pos2,
) -> bool {
    let angle = -rotation_degrees.to_radians();
    let delta = point - center;
    let local = egui::vec2(
        delta.x * angle.cos() - delta.y * angle.sin(),
        delta.x * angle.sin() + delta.y * angle.cos(),
    );
    local.x.abs() <= size.x * 0.5 && local.y.abs() <= size.y * 0.5
}

fn point_in_polygon(point: egui::Pos2, polygon: &[egui::Pos2]) -> bool {
    let mut inside = false;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        let crosses = (a.y > point.y) != (b.y > point.y)
            && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x;
        if crosses {
            inside = !inside;
        }
    }
    inside
}

fn orientation(a: egui::Pos2, b: egui::Pos2, c: egui::Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn segments_intersect(a: egui::Pos2, b: egui::Pos2, c: egui::Pos2, d: egui::Pos2) -> bool {
    let eps = 1e-5;
    let o1 = orientation(a, b, c);
    let o2 = orientation(a, b, d);
    let o3 = orientation(c, d, a);
    let o4 = orientation(c, d, b);
    let on = |p: egui::Pos2, q: egui::Pos2, r: egui::Pos2| {
        orientation(p, q, r).abs() <= eps
            && r.x >= p.x.min(q.x) - eps
            && r.x <= p.x.max(q.x) + eps
            && r.y >= p.y.min(q.y) - eps
            && r.y <= p.y.max(q.y) + eps
    };
    (o1.signum() != o2.signum() && o3.signum() != o4.signum())
        || on(a, b, c)
        || on(a, b, d)
        || on(c, d, a)
        || on(c, d, b)
}

fn polygon_intersects_rect(polygon: &[egui::Pos2], rect: egui::Rect) -> bool {
    let rect_corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    polygon.iter().any(|p| rect.contains(*p))
        || rect_corners.iter().any(|p| point_in_polygon(*p, polygon))
        || (0..polygon.len()).any(|i| {
            let a = polygon[i];
            let b = polygon[(i + 1) % polygon.len()];
            (0..4).any(|j| segments_intersect(a, b, rect_corners[j], rect_corners[(j + 1) % 4]))
        })
}

fn parse_transform(value: &str) -> Option<TransformFields> {
    let mut fields = TransformFields::default();
    for part in value.split(';') {
        let (key, raw) = part.split_once('=')?;
        let number = raw.parse::<f32>().ok()?;
        match key {
            "x" => fields.position_x = number,
            "y" => fields.position_y = number,
            "rotation" => fields.rotation = number,
            "scale_x" => fields.scale_x = number,
            "scale_y" => fields.scale_y = number,
            _ => {}
        }
    }
    Some(fields)
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
    fn sync_hierarchy_metadata(&mut self) {
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        for id in scene.scene.entities.keys().copied().collect::<Vec<_>>() {
            let hidden = scene
                .scene
                .metadata
                .get(&format!("editor.hidden.{}", id.0))
                .is_some_and(|v| v == "true");
            let locked = scene
                .scene
                .metadata
                .get(&format!("editor.locked.{}", id.0))
                .is_some_and(|v| v == "true");
            if hidden {
                self.scene_state.hidden.insert(id);
            } else {
                self.scene_state.hidden.remove(&id);
            }
            if locked {
                self.scene_state.locked.insert(id);
            } else {
                self.scene_state.locked.remove(&id);
            }
        }
    }

    fn persist_hierarchy_flag(&mut self, id: SceneEntityId, kind: &str, enabled: bool) {
        if let Some(scene) = self.document.current_scene.as_mut() {
            let key = format!("editor.{kind}.{}", id.0);
            if enabled {
                scene.scene.metadata.insert(key, "true".to_owned());
            } else {
                scene.scene.metadata.remove(&key);
            }
            scene.mark_dirty();
        }
    }
}

impl EditorApp {
    fn copy_selected_components(&mut self) {
        let Some(id) = self.tree.selected() else {
            return;
        };
        let Some(entity) = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&id))
        else {
            return;
        };
        self.inspector_clipboard = Some(ComponentClipboard {
            components: entity.components.clone(),
            resources: entity.resources.clone(),
            metadata: entity.metadata.clone(),
        });
        self.status = "Components copied".to_owned();
    }

    fn paste_selected_components(&mut self) {
        if self.is_playing() {
            return;
        }
        let Some(id) = self.tree.selected() else {
            return;
        };
        let Some(clipboard) = self.inspector_clipboard.clone() else {
            self.status = "Component clipboard is empty".to_owned();
            return;
        };
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ =
                self.history
                    .execute(scene, "Paste Components", CommandKind::Property, |scene| {
                        let entity = scene.scene.entities.get_mut(&id).ok_or(())?;
                        entity.components = clipboard.components.clone();
                        entity.resources = clipboard.resources.clone();
                        entity.metadata = clipboard.metadata.clone();
                        scene.mark_dirty();
                        Ok::<(), ()>(())
                    });
        }
        self.sync_inspector(id);
        self.status = "Components pasted".to_owned();
    }

    fn draw_inspector_panel(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(340.0);
        ui.heading("Inspector");
        let Some(selected) = self.tree.selected() else {
            ui.label("请先在 Hierarchy 中选择实体");
            return;
        };
        let Some((parent, child_count)) = self.current_scene().and_then(|scene| {
            scene
                .scene
                .entities
                .get(&selected)
                .map(|e| (e.parent, e.children.len()))
        }) else {
            ui.label("实体不存在");
            return;
        };
        ui.label(format!("实体 ID：{}", selected.0));
        ui.label(format!(
            "父实体：{}",
            parent.map_or_else(|| "无".to_owned(), |id| id.0.to_string())
        ));
        ui.label(format!("子实体数量：{child_count}"));
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("父实体 ID");
            ui.text_edit_singleline(&mut self.parent_entity);
            if ui.button("设置父级").clicked() {
                let parent = self
                    .parent_entity
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .map(SceneEntityId);
                self.reparent_selected(parent);
            }
        });
        if ui.button("解除父级").clicked() {
            self.parent_entity.clear();
            self.reparent_selected(None);
        }
        ui.horizontal(|ui| {
            ui.label("名称");
            ui.text_edit_singleline(&mut self.entity_name);
            if ui.button("重新命名").clicked() {
                self.rename_selected();
            }
        });
        let transform_header = if self.inspector_transform_open {
            "Transform  v"
        } else {
            "Transform  >"
        };
        if ui.button(transform_header).clicked() {
            self.inspector_transform_open = !self.inspector_transform_open;
        }
        if self.inspector_transform_open {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("位置 X");
                    ui.add(egui::DragValue::new(&mut self.transform.position_x));
                });
                ui.horizontal(|ui| {
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
                });
                ui.horizontal(|ui| {
                    ui.label("缩放 Y");
                    ui.add(egui::DragValue::new(&mut self.transform.scale_y));
                });
                ui.horizontal(|ui| {
                    if ui.button("Reset").clicked() {
                        self.transform = TransformFields::default();
                    }
                    if ui.button("Copy Components").clicked() {
                        self.copy_selected_components();
                    }
                    if ui.button("Paste Components").clicked() {
                        self.paste_selected_components();
                    }
                });
                if ui.button("Apply Transform").clicked() {
                    self.apply_inspector();
                }
            });
        }
        ui.horizontal(|ui| {
            ui.label("Sprite");
            ui.text_edit_singleline(&mut self.sprite_reference);
        });
        ui.horizontal(|ui| {
            ui.label("Lua");
            ui.text_edit_singleline(&mut self.lua_script);
        });
        if ui.button("应用修改").clicked() {
            self.apply_inspector();
        }
        ui.separator();
        if ui.button("添加子实体").clicked() {
            self.add_entity(Some(selected));
        }
        if ui.button("删除实体").clicked() {
            self.delete_selected();
            self.inspector.clear_selection();
        }
    }
}

impl EditorApp {
    fn draw_scene_view(&mut self, ui: &mut egui::Ui) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let rect = response.rect;
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            self.scene_zoom = (self.scene_zoom * (1.0 + scroll * 0.001)).clamp(0.25, 4.0);
        }
        if response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
        {
            self.scene_state.pan += response.drag_delta();
        }
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(28, 32, 40));
        let grid = 32.0 * self.scene_zoom;
        let mut x = rect.left();
        while x < rect.right() {
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(42, 48, 58)),
            );
            x += grid;
        }
        let mut y = rect.top();
        while y < rect.bottom() {
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(42, 48, 58)),
            );
            y += grid;
        }
        painter.line_segment(
            [
                egui::pos2(rect.center().x, rect.top()),
                egui::pos2(rect.center().x, rect.bottom()),
            ],
            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(70, 120, 180)),
        );
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.center().y),
                egui::pos2(rect.right(), rect.center().y),
            ],
            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(180, 90, 80)),
        );
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Scene View  -  2D",
            egui::FontId::proportional(16.0),
            egui::Color32::LIGHT_GRAY,
        );
        let filter = self.hierarchy_filter.to_lowercase();
        let entities: Vec<_> = self
            .current_scene()
            .map(|s| {
                s.scene
                    .entities
                    .values()
                    .filter_map(|e| {
                        if !filter.trim().is_empty()
                            && !e.name.to_lowercase().contains(filter.trim())
                        {
                            return None;
                        }
                        if self.scene_state.hidden.contains(&e.id) {
                            return None;
                        }
                        let transform = self
                            .scene_state
                            .preview
                            .get(&e.id)
                            .copied()
                            .or_else(|| {
                                e.components
                                    .get("Transform")
                                    .and_then(|v| parse_transform(v))
                            })
                            .unwrap_or_default();
                        Some((e.id, e.name.clone(), transform))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (id, name, transform) in entities.iter() {
            let center = egui::pos2(
                rect.center().x + self.scene_state.pan.x + transform.position_x * self.scene_zoom,
                rect.center().y + self.scene_state.pan.y - transform.position_y * self.scene_zoom,
            );
            let size = egui::vec2(
                (112.0 * transform.scale_x.abs().max(0.25)) * self.scene_zoom,
                (44.0 * transform.scale_y.abs().max(0.25)) * self.scene_zoom,
            );
            let box_rect = egui::Rect::from_center_size(center, size);
            let selected =
                self.tree.selected() == Some(*id) || self.scene_state.selected.contains(id);
            let corners = rotated_corners(center, size, transform.rotation);
            let stroke = egui::Stroke::new(
                if selected { 2.0_f32 } else { 1.0_f32 },
                egui::Color32::from_rgb(130, 170, 220),
            );
            painter.add(egui::Shape::convex_polygon(
                corners.to_vec(),
                if selected {
                    egui::Color32::from_rgb(55, 100, 155)
                } else {
                    egui::Color32::from_rgb(58, 66, 80)
                },
                stroke,
            ));
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                name,
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
            if response.clicked()
                && response
                    .interact_pointer_pos()
                    .is_some_and(|pos| box_rect.contains(pos))
            {
                let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                if additive {
                    if self.scene_state.selected.contains(id) {
                        self.scene_state.selected.remove(id);
                    } else {
                        self.scene_state.selected.insert(*id);
                    }
                } else {
                    self.scene_state.selected.clear();
                    self.scene_state.selected.insert(*id);
                }
                if let Some(scene) = self.current_scene() {
                    let scene = scene.clone();
                    let _ = self.tree.select(&scene, *id);
                }
                self.sync_inspector(*id);
            }
        }

        let pointer = response.interact_pointer_pos();
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(pos) = pointer {
                let hit = entities.iter().rev().find(|(id, _, transform)| {
                    let center = egui::pos2(
                        rect.center().x
                            + self.scene_state.pan.x
                            + transform.position_x * self.scene_zoom,
                        rect.center().y + self.scene_state.pan.y
                            - transform.position_y * self.scene_zoom,
                    );
                    let size = egui::vec2(
                        (112.0 * transform.scale_x.abs().max(0.25)) * self.scene_zoom,
                        (44.0 * transform.scale_y.abs().max(0.25)) * self.scene_zoom,
                    );
                    rotated_contains(center, size, transform.rotation, pos)
                        && !self.scene_state.locked.contains(id)
                });
                if let Some((id, _, _)) = hit {
                    let additive =
                        ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                    if !additive && !self.scene_state.selected.contains(id) {
                        self.scene_state.selected.clear();
                    }
                    self.scene_state.selected.insert(*id);
                    if let Some(scene) = self.current_scene() {
                        let scene = scene.clone();
                        let _ = self.tree.select(&scene, *id);
                    }
                    self.sync_inspector(*id);
                    let ids: Vec<_> = self
                        .scene_state
                        .selected
                        .iter()
                        .copied()
                        .filter(|selected| !self.scene_state.locked.contains(selected))
                        .collect();
                    let initial = entities
                        .iter()
                        .filter(|(entity_id, _, _)| ids.contains(entity_id))
                        .map(|(entity_id, _, value)| (*entity_id, *value))
                        .collect();
                    self.scene_state.drag = Some(SceneDrag {
                        ids,
                        start: pos,
                        initial,
                    });
                } else {
                    self.scene_state.box_select_start = Some(pos);
                    if !ui.input(|input| input.modifiers.command || input.modifiers.ctrl) {
                        self.scene_state.selected.clear();
                    }
                }
            }
        }
        if let (Some(start), Some(current)) = (self.scene_state.box_select_start, pointer) {
            if response.dragged_by(egui::PointerButton::Primary) {
                let selection_rect = egui::Rect::from_two_pos(start, current);
                painter.rect_stroke(
                    selection_rect,
                    0.0,
                    egui::Stroke::new(1.0_f32, egui::Color32::LIGHT_BLUE),
                    egui::StrokeKind::Outside,
                );
                for (id, _, transform) in &entities {
                    let center = egui::pos2(
                        rect.center().x
                            + self.scene_state.pan.x
                            + transform.position_x * self.scene_zoom,
                        rect.center().y + self.scene_state.pan.y
                            - transform.position_y * self.scene_zoom,
                    );
                    let size = egui::vec2(
                        (112.0 * transform.scale_x.abs().max(0.25)) * self.scene_zoom,
                        (44.0 * transform.scale_y.abs().max(0.25)) * self.scene_zoom,
                    );
                    let corners = rotated_corners(center, size, transform.rotation);
                    let contained = corners
                        .iter()
                        .all(|corner| selection_rect.contains(*corner));
                    let intersects = polygon_intersects_rect(&corners, selection_rect);
                    let left_to_right = current.x >= start.x;
                    if (left_to_right && contained) || (!left_to_right && intersects) {
                        self.scene_state.selected.insert(*id);
                    }
                }
            }
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            if let (Some(drag), Some(pos)) = (self.scene_state.drag.as_ref(), pointer) {
                let delta = pos - drag.start;
                let mut preview = std::collections::BTreeMap::new();
                let pivot = {
                    let count = drag.initial.len().max(1) as f32;
                    let x = drag
                        .initial
                        .values()
                        .map(|value| value.position_x)
                        .sum::<f32>()
                        / count;
                    let y = drag
                        .initial
                        .values()
                        .map(|value| value.position_y)
                        .sum::<f32>()
                        / count;
                    (x, y)
                };
                let angle = (delta.x * 0.5).to_radians();
                let factor = (1.0 + delta.x * 0.01).max(0.05);
                for id in &drag.ids {
                    let initial = drag.initial[id];
                    let mut next = initial;
                    match self.scene_tool {
                        SceneTool::Move | SceneTool::Select => {
                            next.position_x += delta.x / self.scene_zoom;
                            next.position_y -= delta.y / self.scene_zoom;
                        }
                        SceneTool::Rotate => {
                            let relative_x = initial.position_x - pivot.0;
                            let relative_y = initial.position_y - pivot.1;
                            next.position_x =
                                pivot.0 + relative_x * angle.cos() - relative_y * angle.sin();
                            next.position_y =
                                pivot.1 + relative_x * angle.sin() + relative_y * angle.cos();
                            next.rotation += delta.x * 0.5;
                        }
                        SceneTool::Scale => {
                            next.position_x = pivot.0 + (initial.position_x - pivot.0) * factor;
                            next.position_y = pivot.1 + (initial.position_y - pivot.1) * factor;
                            next.scale_x = (initial.scale_x * factor).max(0.05);
                            next.scale_y = (initial.scale_y * factor).max(0.05);
                        }
                    }
                    preview.insert(*id, next);
                }
                self.scene_state.preview = preview;
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            self.commit_scene_drag();
            self.scene_state.drag = None;
            self.scene_state.box_select_start = None;
        }
    }

    fn commit_scene_drag(&mut self) {
        if self.is_playing() {
            self.scene_state.preview.clear();
            return;
        }
        let preview = std::mem::take(&mut self.scene_state.preview);
        let Some(first) = preview.keys().next().copied() else {
            return;
        };
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ = self.history.execute(
                scene,
                "Scene Transform",
                CommandKind::Transform(first),
                |scene| {
                    for (id, transform) in &preview {
                        if let Some(entity) = scene.scene.entities.get_mut(id) {
                            entity
                                .components
                                .insert("Transform".to_owned(), encode_transform(*transform));
                        }
                    }
                    scene.mark_dirty();
                    Ok::<(), ()>(())
                },
            );
        }
    }

    fn draw_game_view(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::hover());
        painter.rect_filled(response.rect, 0.0, egui::Color32::from_rgb(18, 20, 24));
        let text = if self.is_playing() {
            "GAME VIEW\nRunning"
        } else {
            "GAME VIEW\nPress Play to run the scene"
        };
        painter.text(
            response.rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(20.0),
            egui::Color32::LIGHT_GRAY,
        );
    }

    fn draw_entity(&mut self, ui: &mut egui::Ui, id: SceneEntityId, depth: usize) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&id) else {
            return;
        };
        let name = entity.name.clone();
        let children = entity.children.clone();
        if !self.hierarchy_filter.trim().is_empty()
            && !name
                .to_lowercase()
                .contains(&self.hierarchy_filter.to_lowercase())
        {
            return;
        }
        let selected = self.tree.selected() == Some(id);
        let collapsed = self.scene_state.collapsed.contains(&id);
        let hidden = self.scene_state.hidden.contains(&id);
        let locked = self.scene_state.locked.contains(&id);
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 14.0);
            if !children.is_empty() {
                if ui.small_button(if collapsed { ">" } else { "v" }).clicked() {
                    if collapsed {
                        self.scene_state.collapsed.remove(&id);
                    } else {
                        self.scene_state.collapsed.insert(id);
                    }
                }
            } else {
                ui.add_space(18.0);
            }
            if ui.small_button(if hidden { "隐" } else { "显" }).clicked() {
                if hidden {
                    self.scene_state.hidden.remove(&id);
                    self.persist_hierarchy_flag(id, "hidden", false);
                } else {
                    self.scene_state.hidden.insert(id);
                    self.persist_hierarchy_flag(id, "hidden", true);
                }
            }
            if ui.small_button(if locked { "L" } else { "U" }).clicked() {
                if locked {
                    self.scene_state.locked.remove(&id);
                    self.persist_hierarchy_flag(id, "locked", false);
                } else {
                    self.scene_state.locked.insert(id);
                    self.persist_hierarchy_flag(id, "locked", true);
                }
            }
            if ui
                .add_enabled(
                    !locked,
                    egui::Button::selectable(selected, format!("{} [{}]", name, id.0)),
                )
                .clicked()
            {
                let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                if !additive {
                    self.scene_state.selected.clear();
                }
                self.scene_state.selected.insert(id);
                if let Some(scene) = self.current_scene() {
                    let scene = scene.clone();
                    let _ = self.tree.select(&scene, id);
                }
                self.sync_inspector(id);
            }
        });
        if !collapsed {
            for child in children {
                self.draw_entity(ui, child, depth + 1);
            }
        }
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn rotated_hit_test_respects_rotation() {
        let center = egui::pos2(0.0, 0.0);
        let size = egui::vec2(100.0, 20.0);
        assert!(rotated_contains(center, size, 45.0, egui::pos2(0.0, 0.0)));
        assert!(!rotated_contains(center, size, 45.0, egui::pos2(45.0, 0.0)));
    }

    #[test]
    fn box_intersection_detects_edge_crossing_without_corners_inside() {
        let polygon = rotated_corners(egui::pos2(0.0, 0.0), egui::vec2(100.0, 20.0), 45.0);
        let selection = egui::Rect::from_min_max(egui::pos2(-10.0, -10.0), egui::pos2(10.0, 10.0));
        assert!(polygon_intersects_rect(&polygon, selection));
    }

    #[test]
    fn left_to_right_containment_differs_from_intersection() {
        let polygon = rotated_corners(egui::pos2(40.0, 0.0), egui::vec2(40.0, 20.0), 0.0);
        let selection = egui::Rect::from_min_max(egui::pos2(0.0, -20.0), egui::pos2(30.0, 20.0));
        assert!(!polygon.iter().all(|p| selection.contains(*p)));
        assert!(polygon_intersects_rect(&polygon, selection));
    }
}
