#![cfg_attr(windows, windows_subsystem = "windows")]

mod actions;
mod geometry;
mod panels;
mod progress;
mod publish;
mod util;

use crate::progress::ProgressPanel;
use crate::util::{configure_cjk_fonts, format_project_error};

use std::{
    env,
    error::Error,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use editor::{
    AutoSaveController, ButtonFields, Camera3DFields, Collider3dFields, CommandHistory,
    ImageFields, InspectorState, LightFields, PlaySession, ProjectDocument, ProjectInfo,
    RecoveryManager, RecoveryScan, ResourceBrowserState, SceneTreeState, TextFields,
    Transform3DFields, TransformFields, check_project, open_project,
};
use eframe::egui;
use engine_core::{
    logging::LogSink,
    math3d::Vec3,
    scene::SceneEntityId,
    script::LuaRuntime,
    transform3d::Transform3D,
};
use engine_host::{Renderer, RuntimeScene};

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
    // 实时日志缓冲：控制台面板读取，游戏/引擎日志自动上屏。
    let log_sink = LogSink::new(500);
    let _ = engine_core::logging::init_logging_with_sink(&log_path, None, Some(log_sink.clone()));
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
                log_sink,
            );
            app.project_loaded = arguments.project.is_some();
            app.refresh_resources();
            // 启动即把当前项目记入「最近项目」。
            if app.project_loaded {
                let root = app.project.root.clone();
                app.record_recent_project(&root);
            }
            Ok(Box::new(app))
        }),
    )?;
    Ok(())
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
    text: TextFields,
    image: ImageFields,
    button: ButtonFields,
    sprite_reference: String,
    lua_script: String,
    inspector_transform3d_open: bool,
    inspector_collider3d_open: bool,
    inspector_text_open: bool,
    inspector_image_open: bool,
    inspector_button_open: bool,
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
    snap_enabled: bool,
    snap_step: f32,
    hierarchy_filter: String,
    scene_state: SceneEditorState,
    inspector_clipboard: Option<ComponentClipboard>,
    inspector_transform_open: bool,
    progress: ProgressPanel,
    publish_compress: bool,
    publish_encrypt: bool,
    /// 底部资源浏览器 / Lua 控制台面板的显示开关（收起后 Scene/Game 视图获得最大空间）。
    show_resources_panel: bool,
    show_lua_console: bool,
    /// 实时日志缓冲（tracing 层写入，控制台面板读取显示）。
    log_sink: LogSink,
    /// “版本说明”窗口是否打开。
    show_about: bool,
    /// 窗口标题（含项目路径）缓存，避免每帧重复发送标题命令。
    last_window_title: Option<String>,
    /// 最近打开的项目路径列表（最新在前，最多 5 个）。
    recent_projects: Vec<String>,
    /// eframe 的 wgpu 设备/队列（单一设备，供内嵌游戏预览使用）。
    gpu: Option<(wgpu::Device, wgpu::Queue)>,
    /// Play 时的内嵌游戏预览（M10-03）。
    preview: Option<EmbeddedPreview>,
    /// 内嵌游戏预览当前分辨率（宽×高）。
    preview_resolution: (u32, u32),
    /// 可用的预览分辨率列表（可增删）。
    preview_resolutions: Vec<(u32, u32)>,
    /// 是否正在输入自定义分辨率。
    adding_preview_resolution: bool,
    /// 自定义分辨率的输入值。
    custom_preview_resolution: (u32, u32),
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
    /// 资源浏览器纹理缩略图缓存（按资源相对路径）。
    thumbnails: std::collections::HashMap<String, egui::TextureHandle>,
    /// 2D Scene 视图渲染缓存（变更检测，避免内容未变化时重复渲染）。
    scene_view_cache_2d: SceneViewCache2d,
    /// 3D Scene 视图渲染缓存（同上）。
    scene_view_cache_3d: SceneViewCache3d,
    /// 2D 场景视图运行时对应的 SceneDocument revision（用于按需重建 scene_runtime）。
    scene_runtime_revision: Option<u64>,
    /// 场景文件列表缓存（避免每帧 read_dir 扫 scenes 目录）。
    scene_files_cache: Vec<PathBuf>,
    /// 场景文件列表最近一次扫描时间。
    scene_files_cache_time: Option<Instant>,
    /// 上次执行 hierarchy 元数据同步的场景标识（path, revision），避免每帧全量遍历。
    last_metadata_key: Option<(Option<PathBuf>, u64)>,
    /// 上次游戏预览步进时间（~30fps 节流，避免输入事件高帧率加速游戏）。
    last_preview_tick: Option<Instant>,
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

/// 对齐模式：Grid 吸附网格 / H 对齐 Y / V 对齐 X（对齐到首个选中实体）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlignMode {
    Grid,
    H,
    V,
}

/// 编辑器轨道相机（ADR-0013）：与游戏相机分离，用于 3D 透视视口导航。
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// 2D Scene 视图上次渲染状态快照（变更检测缓存）：内容未变化时跳过重渲染。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SceneViewCache2d {
    /// 上次渲染时的 SceneDocument revision；`None` 表示无场景（用 Option 区分
    /// "无场景"与"打开 revision=0 的新场景"，避免后者被缓存命中而不渲染）。
    revision: Option<u64>,
    /// 上次渲染时的视口像素尺寸。
    size: (u32, u32),
    /// (pan, zoom)：上次渲染时的 2D 相机。
    camera: (egui::Vec2, f32),
}

/// 3D Scene 视图上次渲染状态快照（变更检测缓存）。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SceneViewCache3d {
    /// 上次渲染时的 SceneDocument revision；`None` 表示无场景。
    revision: Option<u64>,
    /// 上次渲染时的视口像素尺寸。
    size: (u32, u32),
    /// 上次渲染时的编辑器轨道相机。
    camera: EditorCamera,
}

#[derive(Default)]
struct SceneEditorState {
    pan: egui::Vec2,
    drag: Option<SceneDrag>,
    preview: std::collections::BTreeMap<SceneEntityId, TransformFields>,
    /// 3D 实体变换预览（画布俯视 XZ 投影；M9-03/M10-07）。
    preview3d_transform: std::collections::BTreeMap<SceneEntityId, Transform3D>,
    /// 3D 平移/旋转/缩放 Gizmo 拖拽（M9-03/M10-07）。
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

/// 3D 变换 Gizmo 拖拽：约束到哪个轴（Y 为垂直高度，Free 为 XZ 平面；
/// Scale 为等比缩放，Rotate 由 scene_tool 决定）。
struct GizmoDrag {
    id: SceneEntityId,
    axis: GizmoAxis,
    start: egui::Pos2,
    initial: Transform3D,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GizmoAxis {
    X,
    Z,
    Y,
    Free,
    Scale,
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
    fn new(
        project: ProjectInfo,
        gpu: Option<(wgpu::Device, wgpu::Queue)>,
        log_sink: LogSink,
    ) -> Self {
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
            text: TextFields::default(),
            image: ImageFields::default(),
            button: ButtonFields::default(),
            sprite_reference: String::new(),
            lua_script: String::new(),
            inspector_transform3d_open: true,
            inspector_collider3d_open: false,
            inspector_text_open: true,
            inspector_image_open: true,
            inspector_button_open: true,
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
            snap_enabled: false,
            snap_step: 0.5,
            hierarchy_filter: String::new(),
            scene_state: SceneEditorState::default(),
            inspector_clipboard: None,
            inspector_transform_open: true,
            progress: ProgressPanel::default(),
            publish_compress: true,
            publish_encrypt: true,
            show_resources_panel: true,
            show_lua_console: true,
            log_sink,
            show_about: false,
            last_window_title: None,
            recent_projects: crate::util::load_recent_projects(),
            gpu,
            preview: None,
            preview_resolution: (640, 360),
            preview_resolutions: vec![
                (640, 360),
                (960, 540),
                (1280, 720),
                (1920, 1080),
            ],
            adding_preview_resolution: false,
            custom_preview_resolution: (640, 360),
            editor_camera: EditorCamera::default(),
            scene_renderer: None,
            scene_texture: None,
            scene_preview_tex: None,
            scene_runtime: None,
            scene_2d_tex: None,
            thumbnails: std::collections::HashMap::new(),
            scene_view_cache_2d: SceneViewCache2d::default(),
            scene_view_cache_3d: SceneViewCache3d::default(),
            scene_runtime_revision: None,
            scene_files_cache: Vec::new(),
            scene_files_cache_time: None,
            last_metadata_key: None,
            last_preview_tick: None,
        }
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


    fn scene_name(&self) -> String {
        self.current_scene()
            .map(|document| document.scene.name.clone())
            .unwrap_or_else(|| "未打开场景".to_owned())
    }
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
        if self.project_loaded
            && context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S))
        {
            self.save_scene();
        }
        self.sync_hierarchy_metadata();
        self.poll_progress();
        // 行1 标题栏 + 行2 菜单栏（含崩溃恢复「⚠ 恢复」下拉）。
        self.draw_header_panel(context);

        // 行4 控制台：先占底部以获得全宽。
        if self.show_lua_console {
            self.draw_lua_console_panel(context);
        }

        // 行3 资源面板（最左侧）。
        if self.show_resources_panel {
            let refresh = self.draw_resource_browser_panel(context);
            if refresh {
                self.refresh_resources();
            }
        }
        // 行3 层级面板（资源面板右侧）。
        self.draw_hierarchy_panel(context);

        // 行3 属性面板（右侧，先于中央绘制，确保 Scene 视图 rect 不延伸到其下方，
        // 避免在属性面板操作鼠标时误触发 Scene 视图的缩放/平移）。
        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(360.0)
            .min_width(260.0)
            .show(context, |ui| {
                self.draw_inspector_panel(ui);
            });

        // 行3 Scene+Game（中央）+ 工具栏。
        egui::CentralPanel::default().show(context, |ui| {
            self.draw_workspace_toolbar(ui);
            ui.separator();
            match self.workspace_tab {
                WorkspaceTab::Scene => self.draw_scene_view(ui),
                WorkspaceTab::Game => self.draw_game_view(ui),
            }
        });

        self.progress.window(context);
        self.draw_about_window(context);
    }
}


