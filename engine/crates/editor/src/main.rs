#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    env,
    error::Error,
    io::BufRead,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{Receiver, Sender, channel},
    thread,
    time::{Duration, SystemTime},
};

use editor::{
    AutoSaveController, Camera3DFields, Collider3dFields, CommandHistory, CommandKind,
    InspectorState, LightFields, PlaySession, ProjectDocument, ProjectError, ProjectInfo,
    RecoveryManager, RecoveryScan, ResourceBrowserState, ResourceKind, SceneTreeState,
    Transform3DFields, TransformFields, check_project, open_project,
};
use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use engine_core::{
    camera3d::Camera3D,
    input::{InputKey, InputSnapshot, MouseButton},
    light::Light,
    math2d::Camera2D,
    math3d::Vec3,
    scene::SceneEntityId,
    script::{LuaRuntime, ScriptDiagnostic},
    transform::Vec2,
    transform3d::Transform3D,
};
use engine_host::{Renderer, RuntimeScene, Scene3d, Scene3dDrawable, SceneMesh};

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
    let info = BROWSEINFOW {
        hwndOwner: std::ptr::null_mut(),
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: std::ptr::null_mut(),
        lpszTitle: title.as_ptr(),
        ulFlags: 0x0000_0001,
        lpfn: None,
        lParam: 0,
        iImage: 0,
    };
    let pidl = unsafe { SHBrowseForFolderW(&info) };
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
    // 崩溃日志：写在编辑器可执行文件同目录，便于定位。
    let crash_log_path = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("crash.log")))
        .unwrap_or_else(|| PathBuf::from("crash.log"));
    engine_core::logging::install_crash_handler(crash_log_path);
    // 编辑器日志：写入可执行文件同目录，便于排查内嵌预览等问题。
    let log_path = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("editor.log")))
        .unwrap_or_else(|| PathBuf::from("editor.log"));
    let _ = engine_core::logging::init_logging(&log_path, None);
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "RustLua 编辑器",
        options,
        Box::new(move |creation_context| {
            configure_cjk_fonts(&creation_context.egui_ctx);
            // 单一 wgpu 设备（ADR-0012）：编辑器与内嵌游戏预览共用 eframe 的设备。
            let gpu = creation_context
                .wgpu_render_state
                .as_ref()
                .map(|state| (state.device.clone(), state.queue.clone()));
            let mut app = EditorApp::new(
                project.unwrap_or_else(|| ProjectInfo {
                    root: default_engine_path(),
                    version: "engine-only".to_owned(),
                    directories: Vec::new(),
                }),
                gpu,
            );
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

/// 去除 Windows 逐字路径前缀 `\\?\`（会破坏 Lua require 的 searchpath 解析）。
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
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
    transform3d: Transform3DFields,
    mesh_renderer: String,
    camera3d: Camera3DFields,
    light: LightFields,
    collider3d: Collider3dFields,
    sprite_reference: String,
    lua_script: String,
    inspector_transform3d_open: bool,
    inspector_collider3d_open: bool,
    inspector_add_component: String,
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
    scene_view_mode: SceneViewMode,
    hierarchy_filter: String,
    scene_state: SceneEditorState,
    inspector_clipboard: Option<ComponentClipboard>,
    inspector_transform_open: bool,
    progress: ProgressPanel,
    publish_compress: bool,
    publish_encrypt: bool,
    /// eframe 的 wgpu 设备/队列（单一设备，供内嵌游戏预览使用）。
    gpu: Option<(wgpu::Device, wgpu::Queue)>,
    /// Play 时的内嵌游戏预览（M10-03）。
    preview: Option<EmbeddedPreview>,
    /// 编辑器轨道相机（3D 透视视口，M10-04）。
    editor_camera: EditorCamera,
    /// 3D 场景视图的离屏渲染器（渲染编辑场景 + 编辑器相机）。
    scene_renderer: Option<Renderer>,
    /// 3D 场景视图的离屏纹理 + 视图。
    scene_texture: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// 3D 场景视图显示用的 egui 纹理。
    scene_preview_tex: Option<egui::TextureHandle>,
    /// 编辑场景的 2D 运行时（不跑 main.lua，用于 2D 场景视图实时渲染）。
    scene_runtime: Option<RuntimeScene>,
    /// 2D 场景视图显示用的 egui 纹理。
    scene_2d_tex: Option<egui::TextureHandle>,
}

/// 通用进度面板：后台任务 + 进度/日志 + 非阻塞窗口。
/// 可复用于导入、发布等任何耗时操作（任务在线程中执行，UI 不阻塞）。
#[derive(Default)]
struct ProgressPanel {
    title: String,
    rx: Option<Receiver<ProgressEvent>>,
    output: String,
    /// 确定进度 `0..=1`；`None` 表示不定进度（动画条）。
    progress: Option<f32>,
    running: bool,
    done: bool,
    success: bool,
    show: bool,
    /// 任务结果负载（任务线程发送，主线程 poll 完成后按需取用）。
    result: Option<Box<dyn std::any::Any + Send>>,
    /// 完成时展示的目录（如发布包目录），可「打开目录」。
    output_dir: Option<PathBuf>,
}

enum ProgressEvent {
    Line(String),
    // 通用组件 API：供后续进度任务发送确定进度（当前任务未用，保留）。
    #[allow(dead_code)]
    Progress(f32),
    Finished {
        success: bool,
        result: Option<Box<dyn std::any::Any + Send>>,
    },
}

impl ProgressPanel {
    fn start(
        &mut self,
        title: impl Into<String>,
        rx: Receiver<ProgressEvent>,
        output_dir: Option<PathBuf>,
    ) {
        self.title = title.into();
        self.rx = Some(rx);
        self.output.clear();
        self.progress = None;
        self.running = true;
        self.done = false;
        self.success = false;
        self.show = true;
        self.result = None;
        self.output_dir = output_dir;
    }

    /// 每帧排空事件；返回本帧是否刚完成。
    fn poll(&mut self) -> bool {
        if !self.running {
            return false;
        }
        let events: Vec<ProgressEvent> = if let Some(rx) = self.rx.as_ref() {
            std::iter::from_fn(|| rx.try_recv().ok()).collect()
        } else {
            Vec::new()
        };
        let mut finished = false;
        for event in events {
            match event {
                ProgressEvent::Line(text) => self.output.push_str(&text),
                ProgressEvent::Progress(value) => self.progress = Some(value),
                ProgressEvent::Finished { success, result } => {
                    finished = true;
                    self.success = success;
                    self.result = result;
                }
            }
        }
        if finished {
            self.running = false;
            self.done = true;
            self.rx = None;
        }
        finished
    }

    fn window(&mut self, context: &egui::Context) {
        if !self.show {
            return;
        }
        egui::Window::new(&self.title).show(context, |ui| {
            let status = if self.running {
                "进行中..."
            } else if self.success {
                "✔ 完成"
            } else {
                "✘ 失败"
            };
            ui.colored_label(
                if self.success {
                    egui::Color32::LIGHT_GREEN
                } else {
                    egui::Color32::LIGHT_RED
                },
                status,
            );
            if let Some(progress) = self.progress {
                ui.add(egui::ProgressBar::new(progress).show_percentage());
            } else if self.running {
                ui.add(egui::ProgressBar::new(0.0).animate(true));
            }
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.monospace(&self.output);
                });
            if self.done {
                if let Some(dir) = &self.output_dir {
                    ui.label(format!("输出：{}", dir.display()));
                    if ui.button("打开目录").clicked() {
                        open_in_explorer(dir);
                    }
                }
                if ui.button("关闭").clicked() {
                    self.show = false;
                    self.done = false;
                }
            }
        });
    }
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

/// 场景视图投影模式：2D 画布（俯视 XY）或 3D 画布（俯视 XZ）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SceneViewMode {
    #[default]
    View2D,
    View3D,
}

/// 编辑器轨道相机（ADR-0013）：与游戏相机分离，用于 3D 透视视口导航。
#[derive(Clone, Copy, Debug)]
struct EditorCamera {
    yaw: f32,
    pitch: f32,
    dist: f32,
    target: Vec3,
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            yaw: 0.6,
            pitch: 0.6,
            dist: 10.0,
            target: Vec3::ZERO,
        }
    }
}

#[derive(Default)]
struct SceneEditorState {
    pan: egui::Vec2,
    drag: Option<SceneDrag>,
    preview: std::collections::BTreeMap<SceneEntityId, TransformFields>,
    /// 3D 实体平移预览（画布俯视 XZ 投影；M9-03）。
    preview3d: std::collections::BTreeMap<SceneEntityId, Vec3>,
    /// 3D 平移 Gizmo 拖拽（M9-03）。
    gizmo_drag: Option<GizmoDrag>,
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

/// 3D 平移 Gizmo 拖拽：约束到哪个轴（Y 为垂直高度，Free 为 XZ 平面）。
struct GizmoDrag {
    id: SceneEntityId,
    axis: GizmoAxis,
    start: egui::Pos2,
    initial: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GizmoAxis {
    X,
    Z,
    Y,
    Free,
}

#[derive(Clone)]
struct ComponentClipboard {
    components: std::collections::BTreeMap<String, String>,
    resources: std::collections::BTreeMap<String, engine_core::resource_db::AssetId>,
    metadata: std::collections::BTreeMap<String, String>,
}

/// 编辑器内嵌游戏预览（M10-03）：Play 时创建的运行时 + 离屏渲染器，
/// 渲染到离屏纹理，回读后显示在 Game 视图。
struct EmbeddedPreview {
    runtime: RuntimeScene,
    renderer: Renderer,
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    egui_tex: Option<egui::TextureHandle>,
    width: u32,
    height: u32,
}

impl EditorApp {
    fn new(project: ProjectInfo, gpu: Option<(wgpu::Device, wgpu::Queue)>) -> Self {
        let project_loaded = project.root.join("project.toml").is_file();
        let recovery = RecoveryManager::new(project.root.join(".temp"));
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
            transform3d: Transform3DFields::default(),
            mesh_renderer: String::new(),
            camera3d: Camera3DFields::default(),
            light: LightFields::default(),
            collider3d: Collider3dFields::default(),
            sprite_reference: String::new(),
            lua_script: String::new(),
            inspector_transform3d_open: true,
            inspector_collider3d_open: false,
            inspector_add_component: "Transform3D".to_owned(),
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
            scene_view_mode: SceneViewMode::default(),
            hierarchy_filter: String::new(),
            scene_state: SceneEditorState::default(),
            inspector_clipboard: None,
            inspector_transform_open: true,
            progress: ProgressPanel::default(),
            publish_compress: true,
            publish_encrypt: true,
            gpu,
            preview: None,
            editor_camera: EditorCamera::default(),
            scene_renderer: None,
            scene_texture: None,
            scene_preview_tex: None,
            scene_runtime: None,
            scene_2d_tex: None,
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
        let Some(document) = self.current_scene().cloned() else {
            self.status = "Please create or open a scene first".to_owned();
            return;
        };
        // M10-03：创建内嵌游戏预览（Play 真正运行 Lua + 物理 + 渲染）。
        match self.create_preview(&document.scene) {
            Ok(preview) => {
                tracing::info!(target: "editor", "[preview] created {}x{}", preview.width, preview.height);
                self.preview = Some(preview);
            }
            Err(error) => {
                tracing::error!(target: "editor", "[preview] create failed: {error}");
                self.status = format!("游戏预览启动失败：{error}");
            }
        }
        self.play_session = Some(PlaySession::start(&document));
        self.status = "Play mode started; runtime changes stay isolated".to_owned();
    }

    fn stop(&mut self) {
        let Some(session) = self.play_session.take() else {
            self.status = "Not currently in Play mode".to_owned();
            return;
        };
        self.preview = None;
        self.document.current_scene = Some(session.stop());
        self.tree.clear_selection();
        self.inspector.clear_selection();
        self.status = "Play stopped; runtime changes were discarded".to_owned();
    }

    /// 创建内嵌游戏预览：运行时（当前场景）+ 离屏渲染器 + 预览纹理。
    fn create_preview(&self, scene: &engine_core::scene::Scene) -> Result<EmbeddedPreview, String> {
        let Some((device, queue)) = &self.gpu else {
            return Err("编辑器缺少 wgpu 上下文，无法内嵌游戏预览".to_owned());
        };
        // Windows 逐字前缀 `\\?\` 会破坏 Lua require 的 searchpath 解析，去除后再传运行时。
        let root = strip_verbatim_prefix(&self.project.root);
        tracing::info!(target: "editor", "[preview] project_root={}", root.display());
        let runtime = RuntimeScene::from_scene(Some(&root), scene.clone())?;
        let renderer =
            Renderer::new_offscreen(device.clone(), queue.clone(), Some(root), false, false)?;
        let width = 640;
        let height = 360;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("p2-editor-preview"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(EmbeddedPreview {
            runtime,
            renderer,
            texture,
            texture_view,
            egui_tex: None,
            width,
            height,
        })
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
            self.transform3d = snapshot.transform3d.unwrap_or_default();
            self.mesh_renderer = snapshot.mesh_renderer.unwrap_or_default();
            self.camera3d = snapshot.camera3d.unwrap_or_default();
            self.light = snapshot.light.unwrap_or_default();
            self.collider3d = snapshot.collider3d.unwrap_or_default();
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
        let transform3d = self.transform3d;
        let mesh = self.mesh_renderer.clone();
        let camera3d = self.camera3d;
        let light = self.light;
        let collider3d = self.collider3d.clone();
        let sprite = self.sprite_reference.clone();
        let lua = self.lua_script.clone();
        // 3D 组件只在实体已有该组件时更新，避免给普通实体注入默认组件。
        let had_transform3d = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Transform3D")
        });
        let had_camera3d = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Camera3D")
        });
        let had_light = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Light")
        });
        let had_collider3d = self.current_scene().is_some_and(|scene| {
            scene.scene.entities[&selected]
                .components
                .contains_key("Collider3d")
        });
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
                    if had_transform3d {
                        inspector.set_transform3d(scene, transform3d)?;
                    }
                    if mesh.trim().is_empty() {
                        inspector.clear_mesh_renderer(scene)?;
                    } else {
                        inspector.set_mesh_renderer(scene, mesh)?;
                    }
                    if had_camera3d {
                        inspector.set_camera3d(scene, camera3d)?;
                    }
                    if had_light {
                        inspector.set_light(scene, light)?;
                    }
                    if had_collider3d {
                        inspector.set_collider3d(scene, collider3d)?;
                    }
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

    /// 为选中实体添加 Collider3d 组件（用当前字段编码，经 history 可撤销）。
    fn add_collider3d_component(&mut self) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if self.inspector.selected().is_none() {
            return Ok(());
        }
        let inspector = self.inspector.clone();
        let collider3d = self.collider3d.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            "Add Collider3d",
            CommandKind::Property,
            move |scene| {
                inspector.set_collider3d(scene, collider3d)?;
                Ok::<(), editor::InspectorError>(())
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// 移除选中实体的 Collider3d 组件（经 history 可撤销）。
    fn remove_collider3d_component(&mut self) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if self.inspector.selected().is_none() {
            return Ok(());
        }
        let inspector = self.inspector.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            "Remove Collider3d",
            CommandKind::Property,
            move |scene| {
                inspector.clear_collider3d(scene)?;
                Ok::<(), editor::InspectorError>(())
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// 选中实体的组件键列表（Inspector 组件管理用）。
    fn current_components(&self) -> Vec<String> {
        self.current_scene()
            .and_then(|s| {
                self.inspector.selected().map(|id| {
                    s.scene
                        .entities
                        .get(&id)
                        .map(|e| e.components.keys().cloned().collect())
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default()
    }

    /// 为选中实体添加组件（默认值，经 history 可撤销）。
    fn add_component(&mut self, name: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let Some(selected) = self.inspector.selected() else {
            return Ok(());
        };
        let Some(scene) = self.document.current_scene.as_ref() else {
            return Ok(());
        };
        if scene.scene.entities[&selected]
            .components
            .contains_key(name)
        {
            return Err(format!("实体已有 {name} 组件"));
        }
        let inspector = self.inspector.clone();
        let name = name.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            format!("Add {name}"),
            CommandKind::Property,
            move |scene| {
                match name.as_str() {
                    "Transform3D" => {
                        inspector.set_transform3d(scene, Transform3DFields::default())?
                    }
                    "Camera3D" => inspector.set_camera3d(scene, Camera3DFields::default())?,
                    "Light" => inspector.set_light(scene, LightFields::default())?,
                    "Collider3d" => inspector.set_collider3d(scene, Collider3dFields::default())?,
                    "MeshRenderer" => inspector.set_mesh_renderer(scene, "__ground_plane__")?,
                    other => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(other.to_owned(), String::new());
                            scene.mark_dirty();
                        }
                    }
                }
                Ok::<(), editor::InspectorError>(())
            },
        );
        match result {
            Ok(_) => {
                self.sync_inspector(selected);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// 移除选中实体的组件（经 history 可撤销）。
    fn remove_component(&mut self, name: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let Some(selected) = self.inspector.selected() else {
            return Ok(());
        };
        let name = name.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            format!("Remove {name}"),
            CommandKind::Property,
            move |scene| {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.remove(&name);
                    scene.mark_dirty();
                }
                Ok::<(), editor::InspectorError>(())
            },
        );
        match result {
            Ok(_) => {
                self.sync_inspector(selected);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
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

    /// 创建一个带 Transform3D 组件的 3D 实体（M9-03）。
    fn add_3d_entity(&mut self, parent: Option<SceneEntityId>) {
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
        let result = self.history.execute(
            scene,
            "Create 3D entity",
            CommandKind::Create,
            move |scene| {
                let mut tree = tree;
                let id = tree
                    .create_entity(scene, name.clone(), parent)
                    .map_err(|e| e.to_string())?;
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    entity.components.insert(
                        "Transform3D".into(),
                        Transform3D::default().to_component_str(),
                    );
                }
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(true) => {
                if let Some(scene) = self.document.current_scene.as_ref()
                    && let Some(id) = scene.scene.entities.keys().max_by_key(|id| id.0).copied()
                {
                    let _ = self.tree.select(scene, id);
                    self.sync_inspector(id);
                }
                self.status = "3D entity created".into();
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

impl EditorApp {
    /// 启动 Release 发布打包（后台执行 scripts/package.ps1，非阻塞、不弹终端窗）。
    fn start_publish(&mut self) {
        if self.progress.running {
            return;
        }
        let Some(found) = find_publish_script(&self.project.root) else {
            self.progress.output = format!(
                "未找到发布脚本（向上查找 scripts/package.ps1）：{}",
                self.project.root.display()
            );
            self.progress.done = true;
            self.progress.success = false;
            self.progress.show = true;
            return;
        };
        // open_project 会生成 \\?\ 逐字路径，PowerShell -File 不识别，需先去掉前缀。
        let script = strip_verbatim(&found);
        // 发布脚本位于 <workspace>/scripts/package.ps1，发布包输出到 <workspace>/artifacts/builds/release/p2-game。
        let workspace_root = script
            .parent()
            .and_then(|parent| parent.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let package_dir = workspace_root.join("artifacts/builds/release/p2-game");
        let (tx, rx) = channel();
        spawn_publish(script, self.publish_compress, self.publish_encrypt, tx);
        self.progress.start("发布打包", rx, Some(package_dir));
    }

    /// 启动导入游戏项目（后台执行，非阻塞）。
    fn start_import(&mut self) {
        let path = self.import_path.trim().to_owned();
        if path.is_empty() {
            self.status = "Enter a game project directory first".to_owned();
            return;
        }
        if self.progress.running {
            return;
        }
        let (tx, rx) = channel();
        spawn_import(PathBuf::from(path), tx);
        self.progress.start("导入游戏项目", rx, None);
    }

    /// 每帧轮询通用进度面板；任务完成时应用结果（导入）并落盘发布日志。
    fn poll_progress(&mut self) {
        let finished = self.progress.poll();
        if !finished {
            return;
        }
        self.apply_progress_result();
        // 发布：把输出落盘到 <workspace>/artifacts/builds/release/publish-last.log
        if let Some(dir) = &self.progress.output_dir
            && let Some(parent) = dir.parent()
        {
            let log = parent.join("publish-last.log");
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::write(&log, &self.progress.output);
        }
    }

    /// 任务完成：若是导入，把后台结果应用到编辑器状态。
    fn apply_progress_result(&mut self) {
        if !self.progress.done {
            return;
        }
        let Some(result) = self.progress.result.take() else {
            return;
        };
        if let Ok(outcome) = result.downcast::<ImportOutcome>() {
            let outcome = *outcome;
            self.project = outcome.project;
            self.document = outcome.document;
            self.recovery = outcome.recovery;
            self.recovery_scan = outcome.recovery_scan;
            self.resources = outcome.resources;
            self.history.clear();
            self.autosave.reset();
            self.lua_runtime = None;
            self.play_session = None;
            self.tree.clear_selection();
            self.inspector.clear_selection();
            self.project_loaded = true;
            self.status = format!("Imported game project: {}", self.project.root.display());
        }
    }
}

/// 在资源管理器中打开目录（ShellExecuteW，比 `explorer <path>` 更可靠）。
#[cfg(windows)]
fn open_in_explorer(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let op_wide: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op_wide.as_ptr(),
            path_wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(not(windows))]
fn open_in_explorer(_path: &Path) {}

/// 从项目根向上查找 `scripts/package.ps1`（编辑器根可能是游戏目录或工作区根）。
fn find_publish_script(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .map(|dir| dir.join("scripts").join("package.ps1"))
        .find(|path| path.is_file())
}

/// 去掉 Windows `\\?\` 逐字路径前缀（open_project 规范化产生），
/// 外部工具（如 PowerShell -File）不识别该前缀，会静默失败。
fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let text = text
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| text.into_owned());
    PathBuf::from(text)
}

/// 常见的 rustup cargo bin 目录（双击启动时系统 PATH 可能没有 cargo）。
fn cargo_bin_dir() -> Option<PathBuf> {
    let home = env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)?;
    let dir = home.join(".cargo").join("bin");
    (dir.join("cargo.exe").is_file() || dir.join("cargo").is_file()).then_some(dir)
}

/// 把 bin 目录加到 PATH 最前面（去重），确保子进程能找到 cargo。
fn prepend_path(bin: &Path) -> String {
    let current = env::var("PATH").unwrap_or_default();
    let mut parts = vec![bin.to_string_lossy().into_owned()];
    for part in current.split(';').filter(|part| !part.is_empty()) {
        if !parts.iter().any(|existing| existing == part) {
            parts.push(part.to_owned());
        }
    }
    parts.join(";")
}

/// 后台执行发布脚本，把 stdout/stderr 行转发给 UI 线程。
/// CREATE_NO_WINDOW 保证不弹出额外终端窗口。
fn spawn_publish(script: PathBuf, compress: bool, encrypt: bool, tx: Sender<ProgressEvent>) {
    thread::spawn(move || {
        let mut command = Command::new("powershell");
        command
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // PowerShell switch 参数：勾选才传 `-Compress`/`-Encrypt`。
        if compress {
            command.arg("-Compress");
        }
        if encrypt {
            command.arg("-Encrypt");
        }
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        // 编辑器可能是双击启动（系统 PATH 无 cargo）；把 cargo bin 补进子进程 PATH。
        if let Some(bin) = cargo_bin_dir() {
            command.env("PATH", prepend_path(&bin));
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = tx.send(ProgressEvent::Line(format!("无法启动发布脚本：{error}\n")));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
                return;
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let tx_out = tx.clone();
        let tx_err = tx.clone();
        let h1 = thread::spawn(move || {
            if let Some(pipe) = stdout {
                for line in std::io::BufReader::new(pipe).lines() {
                    if let Ok(line) = line {
                        if tx_out.send(ProgressEvent::Line(line + "\n")).is_err() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        });
        let h2 = thread::spawn(move || {
            if let Some(pipe) = stderr {
                for line in std::io::BufReader::new(pipe).lines() {
                    if let Ok(line) = line {
                        if tx_err.send(ProgressEvent::Line(line + "\n")).is_err() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        });
        let status = child.wait();
        let _ = h1.join();
        let _ = h2.join();
        let code = status.as_ref().ok().and_then(|s| s.code());
        let success = status.map(|s| s.success()).unwrap_or(false);
        let _ = tx.send(ProgressEvent::Line(format!(
            "\n[editor] 发布进程退出 code={code:?}\n"
        )));
        let _ = tx.send(ProgressEvent::Finished {
            success,
            result: None,
        });
    });
}

/// 导入游戏项目的后台结果。
struct ImportOutcome {
    project: ProjectInfo,
    document: ProjectDocument,
    recovery: RecoveryManager,
    recovery_scan: RecoveryScan,
    resources: ResourceBrowserState,
}

/// 后台导入游戏项目：打开项目 + 加载文档 + 扫描资源，结果回传主线程。
fn spawn_import(path: PathBuf, tx: Sender<ProgressEvent>) {
    thread::spawn(move || {
        let _ = tx.send(ProgressEvent::Line(format!(
            "打开项目 {} ...\n",
            path.display()
        )));
        let result = (|| -> Result<ImportOutcome, String> {
            let project =
                open_project(&path).map_err(|error| format_project_error(&path, error))?;
            if !project.root.join("project.toml").is_file() {
                return Err("不是独立游戏项目：缺少 project.toml".to_owned());
            }
            let document = ProjectDocument::open(project.clone());
            let recovery = RecoveryManager::new(project.root.join(".temp"));
            let recovery_scan = recovery.scan().unwrap_or_default();
            let mut resources = ResourceBrowserState::new();
            if let Err(error) = resources.refresh(&project.root) {
                return Err(format!("资源扫描失败：{error}"));
            }
            Ok(ImportOutcome {
                project,
                document,
                recovery,
                recovery_scan,
                resources,
            })
        })();
        match result {
            Ok(outcome) => {
                let _ = tx.send(ProgressEvent::Line("导入完成。\n".to_owned()));
                let _ = tx.send(ProgressEvent::Finished {
                    success: true,
                    result: Some(Box::new(outcome)),
                });
            }
            Err(error) => {
                let _ = tx.send(ProgressEvent::Line(format!("导入失败：{error}\n")));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
            }
        }
    });
}

impl eframe::App for EditorApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if self.project_loaded
            && let Some(doc) = self.document.current_scene.as_ref()
        {
            let _ = self.autosave.tick(&self.recovery, doc, SystemTime::now());
        }
        // 调试钩子：`P2_EDITOR_AUTOPLAY=1` 打开 main.scene、切 3D 视口并自动 Play。
        if std::env::var("P2_EDITOR_AUTOPLAY").is_ok() && !self.is_playing() {
            self.scene_view_mode = SceneViewMode::View3D;
            if self.document.current_scene.is_none() {
                let _ = self
                    .document
                    .open_scene(self.project.root.join("scenes/main.scene"));
            }
            if self.document.current_scene.is_some() {
                self.play();
            }
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
        self.poll_progress();
        egui::TopBottomPanel::top("header").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.heading("RustLua 编辑器");
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
                ui.separator();
                ui.checkbox(&mut self.publish_compress, "压缩");
                ui.checkbox(&mut self.publish_encrypt, "加密");
                if ui.button("打包发布").clicked() && !self.progress.running {
                    self.start_publish();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Import game project");
                ui.text_edit_singleline(&mut self.import_path);
                if ui.button("Browse...").clicked()
                    && let Some(path) = choose_project_directory()
                {
                    self.import_path = path.to_string_lossy().into_owned();
                }
                if ui.button("Import").clicked() {
                    self.start_import();
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
                if ui.button("添加 3D 实体").clicked() {
                    self.add_3d_entity(None);
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
                ui.selectable_value(&mut self.scene_view_mode, SceneViewMode::View2D, "2D");
                ui.selectable_value(&mut self.scene_view_mode, SceneViewMode::View3D, "3D");
                ui.separator();
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

        self.progress.window(context);
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

/// 解析 Transform3D 组件串的位置（用于 3D 场景视图俯视投影）。
fn parse_transform3d(value: &str) -> Option<Vec3> {
    Transform3D::from_component_str(value)
        .ok()
        .map(|t| t.position)
}

/// 在保留旋转/缩放的前提下更新 Transform3D 组件串的 position。
fn encode_transform3d_position(current: &str, position: Vec3) -> String {
    let mut transform = Transform3D::from_component_str(current).unwrap_or_default();
    transform.position = position;
    transform.to_component_str()
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
        // M10-06：组件管理（添加/移除）。
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("添加组件");
            egui::ComboBox::from_id_salt("inspector_add_component")
                .selected_text(&self.inspector_add_component)
                .show_ui(ui, |ui| {
                    for name in [
                        "Transform3D",
                        "MeshRenderer",
                        "Camera3D",
                        "Light",
                        "Collider3d",
                    ] {
                        ui.selectable_value(
                            &mut self.inspector_add_component,
                            name.to_owned(),
                            name,
                        );
                    }
                });
            if ui.button("添加").clicked() {
                let component = self.inspector_add_component.clone();
                if let Err(error) = self.add_component(&component) {
                    self.status = error;
                }
            }
        });
        let components = self.current_components();
        if !components.is_empty() {
            ui.label("组件：");
            for key in &components {
                ui.horizontal(|ui| {
                    ui.label(key);
                    if ui.small_button("移除").clicked() {
                        let key = key.clone();
                        if let Err(error) = self.remove_component(&key) {
                            self.status = error;
                        }
                    }
                });
            }
        }
        ui.separator();
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
        let transform3d_header = if self.inspector_transform3d_open {
            "Transform3D  v"
        } else {
            "Transform3D  >"
        };
        if ui.button(transform3d_header).clicked() {
            self.inspector_transform3d_open = !self.inspector_transform3d_open;
        }
        if self.inspector_transform3d_open {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("位置");
                    ui.add(egui::DragValue::new(&mut self.transform3d.position_x));
                    ui.add(egui::DragValue::new(&mut self.transform3d.position_y));
                    ui.add(egui::DragValue::new(&mut self.transform3d.position_z));
                });
                ui.horizontal(|ui| {
                    ui.label("旋转 Q");
                    ui.add(egui::DragValue::new(&mut self.transform3d.rotation_x));
                    ui.add(egui::DragValue::new(&mut self.transform3d.rotation_y));
                    ui.add(egui::DragValue::new(&mut self.transform3d.rotation_z));
                    ui.add(egui::DragValue::new(&mut self.transform3d.rotation_w));
                });
                ui.horizontal(|ui| {
                    ui.label("缩放");
                    ui.add(egui::DragValue::new(&mut self.transform3d.scale_x));
                    ui.add(egui::DragValue::new(&mut self.transform3d.scale_y));
                    ui.add(egui::DragValue::new(&mut self.transform3d.scale_z));
                });
                ui.horizontal(|ui| {
                    if ui.button("Reset 3D").clicked() {
                        self.transform3d = Transform3DFields::default();
                    }
                });
            });
        }
        ui.horizontal(|ui| {
            ui.label("MeshRenderer");
            ui.text_edit_singleline(&mut self.mesh_renderer);
        });
        ui.group(|ui| {
            ui.label("Camera3D");
            ui.horizontal(|ui| {
                ui.label("位置");
                ui.add(egui::DragValue::new(&mut self.camera3d.position_x));
                ui.add(egui::DragValue::new(&mut self.camera3d.position_y));
                ui.add(egui::DragValue::new(&mut self.camera3d.position_z));
            });
            ui.horizontal(|ui| {
                ui.label("目标");
                ui.add(egui::DragValue::new(&mut self.camera3d.target_x));
                ui.add(egui::DragValue::new(&mut self.camera3d.target_y));
                ui.add(egui::DragValue::new(&mut self.camera3d.target_z));
            });
            ui.horizontal(|ui| {
                ui.label("FOV");
                ui.add(egui::DragValue::new(&mut self.camera3d.fov_degrees));
                ui.label("近/远");
                ui.add(egui::DragValue::new(&mut self.camera3d.near));
                ui.add(egui::DragValue::new(&mut self.camera3d.far));
            });
        });
        ui.group(|ui| {
            ui.label("Light");
            ui.horizontal(|ui| {
                ui.label("方向");
                ui.add(egui::DragValue::new(&mut self.light.direction_x));
                ui.add(egui::DragValue::new(&mut self.light.direction_y));
                ui.add(egui::DragValue::new(&mut self.light.direction_z));
            });
            ui.horizontal(|ui| {
                ui.label("颜色");
                ui.add(egui::DragValue::new(&mut self.light.color_r));
                ui.add(egui::DragValue::new(&mut self.light.color_g));
                ui.add(egui::DragValue::new(&mut self.light.color_b));
            });
            ui.horizontal(|ui| {
                ui.label("强度");
                ui.add(egui::DragValue::new(&mut self.light.intensity));
            });
        });
        let collider3d_header = if self.inspector_collider3d_open {
            "Collider3d  v"
        } else {
            "Collider3d  >"
        };
        if ui.button(collider3d_header).clicked() {
            self.inspector_collider3d_open = !self.inspector_collider3d_open;
        }
        if self.inspector_collider3d_open {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("形状");
                    egui::ComboBox::from_id_salt("collider3d_shape")
                        .selected_text(&self.collider3d.shape)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.collider3d.shape,
                                "ball".to_owned(),
                                "Ball",
                            );
                            ui.selectable_value(
                                &mut self.collider3d.shape,
                                "cuboid".to_owned(),
                                "Cuboid",
                            );
                            ui.selectable_value(
                                &mut self.collider3d.shape,
                                "capsule".to_owned(),
                                "Capsule",
                            );
                        });
                    ui.checkbox(&mut self.collider3d.sensor, "sensor");
                    ui.checkbox(&mut self.collider3d.dynamic, "dynamic");
                });
                match self.collider3d.shape.as_str() {
                    "cuboid" => {
                        ui.horizontal(|ui| {
                            ui.label("半宽/半高/半深");
                            ui.add(egui::DragValue::new(&mut self.collider3d.half_width));
                            ui.add(egui::DragValue::new(&mut self.collider3d.half_height));
                            ui.add(egui::DragValue::new(&mut self.collider3d.half_z));
                        });
                    }
                    "capsule" => {
                        ui.horizontal(|ui| {
                            ui.label("半高/半径");
                            ui.add(egui::DragValue::new(&mut self.collider3d.half_height));
                            ui.add(egui::DragValue::new(&mut self.collider3d.radius));
                        });
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            ui.label("半径");
                            ui.add(egui::DragValue::new(&mut self.collider3d.radius));
                        });
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("偏移");
                    ui.add(egui::DragValue::new(&mut self.collider3d.offset_x));
                    ui.add(egui::DragValue::new(&mut self.collider3d.offset_y));
                    ui.add(egui::DragValue::new(&mut self.collider3d.offset_z));
                });
                ui.horizontal(|ui| {
                    if ui.button("添加 Collider3d").clicked()
                        && let Err(error) = self.add_collider3d_component()
                    {
                        self.status = error;
                    }
                    if ui.button("移除 Collider3d").clicked()
                        && let Err(error) = self.remove_collider3d_component()
                    {
                        self.status = error;
                    }
                });
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
    /// 确保 3D 场景视图的离屏渲染器存在；按视图尺寸重建纹理（M10-04）。
    fn ensure_scene_renderer(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.scene_renderer.is_none()
            && let Some((device, queue)) = &self.gpu
        {
            let root = strip_verbatim_prefix(&self.project.root);
            self.scene_renderer =
                Renderer::new_offscreen(device.clone(), queue.clone(), Some(root), false, false)
                    .ok();
        }
        let size_changed = self
            .scene_texture
            .as_ref()
            .is_none_or(|(texture, _)| texture.width() != width || texture.height() != height);
        if size_changed && let Some((device, _)) = &self.gpu {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("p2-scene-view"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.scene_texture = Some((texture, view));
        }
    }

    /// 从当前编辑场景 + 编辑器轨道相机构建 3D 渲染帧（M10-04）。
    fn build_scene3d(&self, aspect: f32) -> Option<Scene3d> {
        let scene = self.current_scene()?;
        let drawables = scene
            .scene
            .entities
            .values()
            .filter_map(|entity| {
                let transform = entity
                    .components
                    .get("Transform3D")
                    .and_then(|value| Transform3D::from_component_str(value).ok())?;
                let mesh = entity
                    .components
                    .get("MeshRenderer")
                    .and_then(|value| value.strip_prefix("mesh="))
                    .map(str::to_owned)?;
                let model = transform.model_matrix().ok()?;
                Some(Scene3dDrawable {
                    mesh_path: mesh,
                    model,
                    palette: Vec::new(),
                })
            })
            .collect::<Vec<_>>();
        if drawables.is_empty() {
            return None;
        }
        let cam = self.editor_camera;
        let position = Vec3::new(
            cam.target.x + cam.dist * cam.pitch.cos() * cam.yaw.sin(),
            cam.target.y + cam.dist * cam.pitch.sin(),
            cam.target.z + cam.dist * cam.pitch.cos() * cam.yaw.cos(),
        );
        let camera = Camera3D {
            position,
            target: cam.target,
            up: Vec3::Y,
            fov_y: 60f32.to_radians(),
            aspect_ratio: aspect,
            near: 0.1,
            far: 200.0,
        };
        let light = Light {
            direction: Vec3::new(0.4, 0.8, 0.5),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        };
        Some(Scene3d {
            camera,
            light,
            drawables,
        })
    }

    /// 渲染 3D 场景视图（编辑场景 + 编辑器相机）到离屏纹理并更新 egui 预览。
    fn render_scene_view(&mut self, width: u32, height: u32, ctx: &egui::Context) {
        if width == 0 || height == 0 {
            return;
        }
        self.ensure_scene_renderer(width, height);
        let scene3d = self.build_scene3d(width as f32 / height.max(1) as f32);
        let mesh = SceneMesh {
            vertices: Vec::new(),
            draws: Vec::new(),
            paths: Vec::new(),
            font_atlas: None,
        };
        let pixels = {
            let Some(renderer) = &mut self.scene_renderer else {
                return;
            };
            let Some((texture, view)) = &self.scene_texture else {
                return;
            };
            renderer.render_into_texture(&mesh, scene3d.as_ref(), view, width, height);
            renderer.copy_texture_to_cpu(texture, width, height).ok()
        };
        if let Some(pixels) = pixels {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [width as usize, height as usize],
                &pixels,
            );
            match &mut self.scene_preview_tex {
                Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                None => {
                    self.scene_preview_tex =
                        Some(ctx.load_texture("scene_view", image, egui::TextureOptions::LINEAR));
                }
            }
        }
    }

    /// 确保编辑场景的 2D 运行时存在（当前编辑场景，不跑 main.lua）。
    fn ensure_scene_runtime(&mut self) {
        if self.scene_runtime.is_some() {
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        if let Ok(runtime) = RuntimeScene::from_scene_edit(Some(&root), scene.scene.clone()) {
            self.scene_runtime = Some(runtime);
        }
    }

    /// 渲染 2D 场景视图（编辑场景，匹配编辑器 pan/zoom）到离屏纹理并更新 egui 预览。
    fn render_scene_view_2d(&mut self, width: u32, height: u32, ctx: &egui::Context) {
        if width == 0 || height == 0 {
            return;
        }
        self.ensure_scene_runtime();
        self.ensure_scene_renderer(width, height);
        let zoom = self.scene_zoom;
        let pan = self.scene_state.pan;
        // 2D 相机匹配编辑器视图：屏幕中心对应世界 -pan/zoom。
        let camera =
            Camera2D::new(Vec2::new(-pan.x / zoom, -pan.y / zoom), zoom).unwrap_or_else(|_| {
                Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("zero camera is valid")
            });
        let mesh = {
            let Some(runtime) = &mut self.scene_runtime else {
                return;
            };
            runtime.set_camera_2d(camera);
            runtime
                .vertices(width, height)
                .unwrap_or_else(|_| SceneMesh {
                    vertices: Vec::new(),
                    draws: Vec::new(),
                    paths: Vec::new(),
                    font_atlas: None,
                })
        };
        let pixels = {
            let Some(renderer) = &mut self.scene_renderer else {
                return;
            };
            let Some((texture, view)) = &self.scene_texture else {
                return;
            };
            renderer.render_into_texture(&mesh, None, view, width, height);
            renderer.copy_texture_to_cpu(texture, width, height).ok()
        };
        if let Some(pixels) = pixels {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [width as usize, height as usize],
                &pixels,
            );
            match &mut self.scene_2d_tex {
                Some(tex) => tex.set(image, egui::TextureOptions::NEAREST),
                None => {
                    self.scene_2d_tex =
                        Some(ctx.load_texture("scene_2d", image, egui::TextureOptions::NEAREST));
                }
            }
        }
    }

    /// 把世界坐标投影到屏幕（编辑器相机透视投影）。
    fn project_to_screen(&self, world: Vec3, rect: egui::Rect) -> Option<egui::Pos2> {
        let cam = self.editor_camera;
        let position = Vec3::new(
            cam.target.x + cam.dist * cam.pitch.cos() * cam.yaw.sin(),
            cam.target.y + cam.dist * cam.pitch.sin(),
            cam.target.z + cam.dist * cam.pitch.cos() * cam.yaw.cos(),
        );
        let camera = Camera3D {
            position,
            target: cam.target,
            up: Vec3::Y,
            fov_y: 60f32.to_radians(),
            aspect_ratio: rect.width() / rect.height().max(1.0),
            near: 0.1,
            far: 200.0,
        };
        let vp = camera.view_projection_matrix().ok()?;
        let c = vp.columns;
        let x = c[0][0] * world.x + c[1][0] * world.y + c[2][0] * world.z + c[3][0];
        let y = c[0][1] * world.x + c[1][1] * world.y + c[2][1] * world.z + c[3][1];
        let w = c[0][3] * world.x + c[1][3] * world.y + c[2][3] * world.z + c[3][3];
        if w <= 0.0 {
            return None;
        }
        Some(egui::pos2(
            rect.center().x + (x / w) * rect.width() * 0.5,
            rect.center().y - (y / w) * rect.height() * 0.5,
        ))
    }

    /// 3D 场景视图（M9-03）：XZ 俯视投影绘制 Transform3D 实体，选中实体显示
    /// 平移 Gizmo（X/Z 轴手柄、Y 高度手柄、自由拖拽），拖拽写回 Transform3D。
    fn draw_scene_view_3d(&mut self, ui: &mut egui::Ui) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let rect = response.rect;
        let scale = ui.ctx().pixels_per_point();
        let width = (rect.width() * scale) as u32;
        let height = (rect.height() * scale) as u32;

        // 导航：滚轮缩放 / 中键轨道 / 右键平移（编辑器相机，与游戏相机分离）。
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            self.editor_camera.dist =
                (self.editor_camera.dist * (1.0 + scroll * 0.001)).clamp(1.0, 100.0);
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            let d = response.drag_delta();
            self.editor_camera.yaw += d.x * 0.008;
            self.editor_camera.pitch = (self.editor_camera.pitch + d.y * 0.008).clamp(0.05, 1.4);
        } else if response.dragged_by(egui::PointerButton::Secondary) {
            let d = response.drag_delta();
            let fx = -self.editor_camera.yaw.sin();
            let fz = -self.editor_camera.yaw.cos();
            let rx = self.editor_camera.yaw.cos();
            let rz = -self.editor_camera.yaw.sin();
            let s = self.editor_camera.dist * 0.0016;
            self.editor_camera.target.x += (-d.x * rx + d.y * fx) * s;
            self.editor_camera.target.z += (-d.x * rz + d.y * fz) * s;
        }

        // 渲染透视视口（编辑场景 + 编辑器相机）到离屏纹理并显示。
        self.render_scene_view(width, height, ui.ctx());
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(28, 32, 40));
        if let Some(tex) = &self.scene_preview_tex {
            painter.image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Scene View  -  3D  中键旋转 / 右键平移 / 滚轮缩放",
            egui::FontId::proportional(15.0),
            egui::Color32::LIGHT_GRAY,
        );

        // 收集 3D 实体（Transform3D），投影到屏幕。
        let filter = self.hierarchy_filter.to_lowercase();
        let entities3d: Vec<_> = self
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
                        let position = e
                            .components
                            .get("Transform3D")
                            .and_then(|v| parse_transform3d(v))?;
                        Some((e.id, e.name.clone(), position))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // 绘制实体标记（投影后的菱形 + 名称）。
        for (id, name, position) in &entities3d {
            let Some(screen) = self.project_to_screen(*position, rect) else {
                continue;
            };
            let selected =
                self.tree.selected() == Some(*id) || self.scene_state.selected.contains(id);
            let half = 8.0;
            let stroke = egui::Stroke::new(
                if selected { 2.0_f32 } else { 1.0_f32 },
                egui::Color32::from_rgb(230, 170, 60),
            );
            let corners = [
                screen + egui::vec2(-half, 0.0),
                screen + egui::vec2(0.0, -half),
                screen + egui::vec2(half, 0.0),
                screen + egui::vec2(0.0, half),
            ];
            painter.add(egui::Shape::convex_polygon(
                corners.to_vec(),
                if selected {
                    egui::Color32::from_rgb(120, 90, 40)
                } else {
                    egui::Color32::from_rgb(58, 66, 80)
                },
                stroke,
            ));
            painter.text(
                screen + egui::vec2(0.0, half + 10.0),
                egui::Align2::CENTER_CENTER,
                name,
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
        }

        // 点选：点击实体投影标记选中。
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let hit = entities3d.iter().rev().find(|(id, _, position)| {
                let Some(screen) = self.project_to_screen(*position, rect) else {
                    return false;
                };
                let box_rect = egui::Rect::from_center_size(screen, egui::vec2(20.0, 20.0));
                box_rect.contains(pos) && !self.scene_state.locked.contains(id)
            });
            if let Some((id, _, _)) = hit {
                let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                if !additive {
                    self.scene_state.selected.clear();
                }
                self.scene_state.selected.insert(*id);
                if let Some(scene) = self.current_scene() {
                    let scene = scene.clone();
                    let _ = self.tree.select(&scene, *id);
                }
                self.sync_inspector(*id);
            } else {
                self.scene_state.selected.clear();
            }
        }

        // Gizmo：选中实体绘制 X/Z/Y 手柄，拖拽在相机地面平面移动。
        let selected_entity = entities3d.iter().find(|(id, _, _)| {
            self.tree.selected() == Some(*id) || self.scene_state.selected.contains(id)
        });
        if let Some((id, _, position)) = selected_entity
            && matches!(self.scene_tool, SceneTool::Move | SceneTool::Select)
        {
            let Some(screen) = self.project_to_screen(*position, rect) else {
                return;
            };
            let len = 40.0;
            painter.line_segment(
                [screen, screen + egui::vec2(len, 0.0)],
                egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(220, 80, 80)),
            );
            painter.circle_filled(
                screen + egui::vec2(len, 0.0),
                5.0,
                egui::Color32::from_rgb(220, 80, 80),
            );
            painter.line_segment(
                [screen, screen + egui::vec2(0.0, -len)],
                egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(80, 120, 220)),
            );
            painter.circle_filled(
                screen + egui::vec2(0.0, -len),
                5.0,
                egui::Color32::from_rgb(80, 120, 220),
            );
            painter.line_segment(
                [screen, screen + egui::vec2(-len * 0.7, -len * 0.7)],
                egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 200, 90)),
            );
            painter.circle_filled(
                screen + egui::vec2(-len * 0.7, -len * 0.7),
                5.0,
                egui::Color32::from_rgb(90, 200, 90),
            );
            // 拖拽：命中手柄轴约束；命中实体自由 XZ（相机地面平面）。
            let pointer = response.interact_pointer_pos();
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(pos) = pointer
            {
                let hit_radius = 12.0;
                let axis = if (pos - (screen + egui::vec2(len, 0.0))).length() < hit_radius {
                    Some(GizmoAxis::X)
                } else if (pos - (screen + egui::vec2(0.0, -len))).length() < hit_radius {
                    Some(GizmoAxis::Z)
                } else if (pos - (screen + egui::vec2(-len * 0.7, -len * 0.7))).length()
                    < hit_radius
                {
                    Some(GizmoAxis::Y)
                } else {
                    let box_rect = egui::Rect::from_center_size(screen, egui::vec2(24.0, 24.0));
                    if box_rect.contains(pos) && !self.scene_state.locked.contains(id) {
                        Some(GizmoAxis::Free)
                    } else {
                        None
                    }
                };
                if let Some(axis) = axis {
                    self.scene_state.gizmo_drag = Some(GizmoDrag {
                        id: *id,
                        axis,
                        start: pos,
                        initial: *position,
                    });
                }
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && let (Some(drag), Some(pos)) = (self.scene_state.gizmo_drag.as_ref(), pointer)
            {
                let delta = pos - drag.start;
                let d = self.editor_camera.dist * 0.002;
                let mut next = drag.initial;
                match drag.axis {
                    GizmoAxis::X => next.x += delta.x * d,
                    GizmoAxis::Z => next.z -= delta.y * d,
                    GizmoAxis::Y => next.y -= delta.y * d,
                    GizmoAxis::Free => {
                        let fx = -self.editor_camera.yaw.sin();
                        let fz = -self.editor_camera.yaw.cos();
                        let rx = self.editor_camera.yaw.cos();
                        let rz = -self.editor_camera.yaw.sin();
                        next.x += (delta.x * rx - delta.y * fx) * d;
                        next.z += (delta.x * rz - delta.y * fz) * d;
                    }
                }
                self.scene_state.preview3d.insert(drag.id, next);
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                self.commit_gizmo_drag3d();
                self.scene_state.gizmo_drag = None;
            }
        }
    }
    fn draw_scene_view(&mut self, ui: &mut egui::Ui) {
        if self.scene_view_mode == SceneViewMode::View3D {
            self.draw_scene_view_3d(ui);
            return;
        }
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
        // 实时渲染编辑场景（精灵/图块/文本）作为背景。
        let scale = ui.ctx().pixels_per_point();
        let view_w = (rect.width() * scale) as u32;
        let view_h = (rect.height() * scale) as u32;
        self.render_scene_view_2d(view_w, view_h, ui.ctx());
        if let Some(tex) = &self.scene_2d_tex {
            painter.image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
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
            // 场景已实时渲染，实体只画选中轮廓 + 名称。
            if selected {
                painter.rect_stroke(
                    box_rect,
                    0.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(230, 170, 60)),
                    egui::StrokeKind::Outside,
                );
                painter.text(
                    center + egui::vec2(0.0, -size.y * 0.5 - 6.0),
                    egui::Align2::CENTER_BOTTOM,
                    name,
                    egui::FontId::proportional(14.0),
                    egui::Color32::LIGHT_YELLOW,
                );
            }
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
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pos) = pointer
        {
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
                let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
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
        if let (Some(start), Some(current)) = (self.scene_state.box_select_start, pointer)
            && response.dragged_by(egui::PointerButton::Primary)
        {
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
        if response.dragged_by(egui::PointerButton::Primary)
            && let (Some(drag), Some(pos)) = (self.scene_state.drag.as_ref(), pointer)
        {
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

    /// 提交 3D 平移 Gizmo 拖拽：只改 Transform3D 的 position，保留旋转/缩放。
    fn commit_gizmo_drag3d(&mut self) {
        if self.is_playing() {
            self.scene_state.preview3d.clear();
            return;
        }
        let preview = std::mem::take(&mut self.scene_state.preview3d);
        let Some((id, position)) = preview.iter().next().map(|(id, position)| (*id, *position))
        else {
            return;
        };
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ = self.history.execute(
                scene,
                "3D Transform",
                CommandKind::Transform3D(id),
                |scene| {
                    if let Some(entity) = scene.scene.entities.get_mut(&id) {
                        let current = entity
                            .components
                            .get("Transform3D")
                            .cloned()
                            .unwrap_or_default();
                        entity.components.insert(
                            "Transform3D".to_owned(),
                            encode_transform3d_position(&current, position),
                        );
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
        if self.preview.is_some() {
            let focused = ui.rect_contains_pointer(response.rect);
            self.update_preview(ui.ctx(), focused);
            if let Some(preview) = &self.preview
                && let Some(tex) = &preview.egui_tex
            {
                // 保持预览纵横比，居中绘制。
                let aspect = preview.width as f32 / preview.height.max(1) as f32;
                let mut rect = response.rect;
                let avail = rect.width() / rect.height().max(1.0);
                if avail > aspect {
                    let width = rect.height() * aspect;
                    let extra = (rect.width() - width) / 2.0;
                    rect.min.x += extra;
                    rect.max.x -= extra;
                } else {
                    let height = rect.width() / aspect;
                    let extra = (rect.height() - height) / 2.0;
                    rect.min.y += extra;
                    rect.max.y -= extra;
                }
                painter.image(
                    tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        } else {
            let text = if self.is_playing() {
                "GAME VIEW\n(preview unavailable)"
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
    }

    /// 步进内嵌游戏运行时，渲染到离屏纹理并回读更新 egui 预览纹理（M10-03）。
    fn update_preview(&mut self, ctx: &egui::Context, focused: bool) {
        // 把 egui 的键盘/鼠标/滚轮输入桥接到游戏快照（仅在指针悬停 Game 视图时，
        // 避免编辑器输入泄漏到游戏）。先构建输入，避免与 preview 的可变借用冲突。
        let input = if focused {
            self.preview_input(ctx)
        } else {
            InputSnapshot::default()
        };
        let Some(preview) = &mut self.preview else {
            return;
        };
        if let Err(error) = preview.runtime.run_script_steps(1, 1.0 / 60.0, &input) {
            tracing::warn!(target: "editor", "[preview] script step failed: {error}");
            return;
        }
        let Some(mesh) = preview.runtime.vertices(preview.width, preview.height).ok() else {
            tracing::warn!(target: "editor", "[preview] build vertices failed");
            return;
        };
        let aspect = preview.width as f32 / preview.height.max(1) as f32;
        let scene3d = preview.runtime.scene_3d(aspect);
        preview.renderer.render_into_texture(
            &mesh,
            scene3d.as_ref(),
            &preview.texture_view,
            preview.width,
            preview.height,
        );
        let readback =
            preview
                .renderer
                .copy_texture_to_cpu(&preview.texture, preview.width, preview.height);
        match readback {
            Ok(pixels) => {
                let nonzero = pixels.iter().any(|&byte| byte > 0);
                if !nonzero {
                    tracing::warn!(target: "editor", "[preview] readback frame is all-zero");
                }
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [preview.width as usize, preview.height as usize],
                    &pixels,
                );
                match &mut preview.egui_tex {
                    Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                    None => {
                        preview.egui_tex = Some(ctx.load_texture(
                            "game_preview",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
            }
            Err(error) => {
                tracing::warn!(target: "editor", "[preview] readback failed: {error}");
            }
        }
    }

    /// 从 egui 输入构建游戏输入快照（键盘 WASD/空格/Esc、鼠标、滚轮）。
    fn preview_input(&self, ctx: &egui::Context) -> InputSnapshot {
        let mut input = InputSnapshot::default();
        ctx.input(|i| {
            for (egui_key, game_key) in [
                (egui::Key::W, InputKey::W),
                (egui::Key::A, InputKey::A),
                (egui::Key::S, InputKey::S),
                (egui::Key::D, InputKey::D),
                (egui::Key::Space, InputKey::Space),
                (egui::Key::Escape, InputKey::Escape),
            ] {
                input.key_event(game_key, i.key_down(egui_key));
            }
            input.mouse_event(MouseButton::Left, i.pointer.primary_down());
            input.mouse_event(MouseButton::Right, i.pointer.secondary_down());
            input.mouse_event(MouseButton::Middle, i.pointer.middle_down());
            if let Some(pos) = i.pointer.hover_pos() {
                input.cursor_moved(pos.x as f64, pos.y as f64);
            }
            let scroll = i.smooth_scroll_delta;
            if scroll.x != 0.0 || scroll.y != 0.0 {
                input.scroll_event(scroll.x, scroll.y);
            }
        });
        input
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

#[cfg(test)]
mod publish_tests {
    use super::*;

    #[test]
    fn cargo_bin_dir_points_to_rustup_cargo() {
        let Some(dir) = cargo_bin_dir() else {
            eprintln!("本机无 ~/.cargo/bin，跳过");
            return;
        };
        assert!(dir.join("cargo.exe").is_file() || dir.join("cargo").is_file());
    }

    #[test]
    fn prepend_path_puts_bin_first_and_dedupes() {
        let bin = PathBuf::from("C:/fake/cargo/bin");
        let result = prepend_path(&bin);
        let parts: Vec<_> = result.split(';').collect();
        assert_eq!(parts[0], "C:/fake/cargo/bin");
        // 不重复
        assert_eq!(
            parts.iter().filter(|p| **p == "C:/fake/cargo/bin").count(),
            1
        );
    }

    #[test]
    fn strip_verbatim_removes_windows_verbatim_prefix() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\GitHub\p2\scripts\package.ps1")),
            PathBuf::from(r"C:\GitHub\p2\scripts\package.ps1")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\server\share\file.ps1")),
            PathBuf::from(r"\\server\share\file.ps1")
        );
        // 无前缀原样返回
        assert_eq!(
            strip_verbatim(Path::new(r"C:\GitHub\p2\scripts\package.ps1")),
            PathBuf::from(r"C:\GitHub\p2\scripts\package.ps1")
        );
    }

    #[test]
    fn publish_spawn_end_to_end() {
        // 复现编辑器「打包发布」全路径：找到脚本、spawn、收集输出与退出码。
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let Some(script) = find_publish_script(&workspace) else {
            eprintln!("未找到发布脚本，跳过");
            return;
        };
        let (tx, rx) = channel();
        spawn_publish(script, false, false, tx);
        let mut output = String::new();
        let mut success = false;
        while let Ok(event) = rx.recv() {
            match event {
                ProgressEvent::Line(text) => output.push_str(&text),
                ProgressEvent::Finished { success: s, .. } => {
                    success = s;
                    break;
                }
                ProgressEvent::Progress(_) => {}
            }
        }
        eprintln!("=== 发布输出开始 ===\n{output}=== 发布输出结束 === success={success}");
        assert!(success, "发布应成功；输出：\n{output}");
    }

    #[test]
    fn import_spawn_end_to_end() {
        // 复现编辑器「导入游戏项目」后台路径：打开 game 项目并扫描资源。
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let game = workspace.join("game");
        let (tx, rx) = channel();
        spawn_import(game, tx);
        let mut output = String::new();
        let mut success = false;
        while let Ok(event) = rx.recv() {
            match event {
                ProgressEvent::Line(text) => output.push_str(&text),
                ProgressEvent::Finished { success: s, result } => {
                    success = s;
                    assert!(result.is_some(), "导入应带回 ImportOutcome 结果");
                    break;
                }
                ProgressEvent::Progress(_) => {}
            }
        }
        assert!(success, "导入应成功；输出：\n{output}");
    }

    #[test]
    fn encode_transform3d_position_preserves_rotation_and_scale() {
        let current = "position=1,2,3;rotation=0,0,0,1;scale=2,2,2";
        let next = encode_transform3d_position(current, Vec3::new(9.0, 5.0, -3.0));
        let transform = Transform3D::from_component_str(&next).expect("valid Transform3D");
        assert_eq!(transform.position, Vec3::new(9.0, 5.0, -3.0));
        assert_eq!(transform.scale, Vec3::new(2.0, 2.0, 2.0));
        assert!(transform.rotation.is_unit());
    }

    #[test]
    fn parse_transform3d_reads_position_only() {
        let value = "position=4,7,1;rotation=0,0,0,1;scale=1,1,1";
        assert_eq!(parse_transform3d(value), Some(Vec3::new(4.0, 7.0, 1.0)));
        assert_eq!(parse_transform3d("not a transform"), None);
    }
}
