#![cfg_attr(windows, windows_subsystem = "windows")]

mod actions;
mod geometry;
mod import;
mod panels;
mod preview_session;
mod publish;
mod util;

use crate::panels::progress::ProgressPanel;
use crate::preview_session::PreviewSession;
use crate::util::{configure_cjk_fonts, format_project_error};

use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use editor::{
    AutoSaveController, Camera3DFields, Collider3dFields, CommandHistory, ImageFields,
    InspectorState, LightFields, PlaySession, ProjectDocument, ProjectInfo, RecoveryManager,
    RecoveryScan, ResourceBrowserState, SceneTreeState, SelectionModel, TextFields,
    Transform3DFields, TransformFields, check_project, open_project,
};
use eframe::egui;
use engine::core::{
    animation::{AnimationClip, AnimationStateMachine},
    font::FontAtlas,
    input::InputSnapshot,
    logging::LogSink,
    math2d::DEFAULT_2D_CAMERA_CENTER,
    math3d::Vec3,
    scene::SceneEntityId,
    script::LuaRuntime,
    sprite::{SpriteComponent, SpriteSorting},
    texture::{TextureAsset, TextureColorSpace},
    tilemap::Tilemap,
    transform::Vec2,
    transform3d::Transform3D,
};
use engine::host::{Renderer, RuntimeScene};

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

/// p2 工作区根（含 scripts/、docs/ 等），相对编辑器 crate 上溯 3 级：`C:\GitHub\p2`。
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn downscale_rgba_benchmark(
    pixels: &[u8],
    width: usize,
    height: usize,
    max: usize,
) -> (Vec<u8>, usize, usize) {
    if (width <= max && height <= max) || width == 0 || height == 0 {
        return (pixels.to_vec(), width, height);
    }
    let scale = max as f32 / width.max(height) as f32;
    let new_w = ((width as f32 * scale).round() as usize).max(1);
    let new_h = ((height as f32 * scale).round() as usize).max(1);
    let mut out = vec![0u8; new_w * new_h * 4];
    for y in 0..new_h {
        let sy = ((y as f32 + 0.5) / new_h as f32 * height as f32) as usize;
        let sy = sy.min(height - 1);
        for x in 0..new_w {
            let sx = ((x as f32 + 0.5) / new_w as f32 * width as f32) as usize;
            let sx = sx.min(width - 1);
            let src = (sy * width + sx) * 4;
            let dst = (y * new_w + x) * 4;
            out[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
        }
    }
    (out, new_w, new_h)
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
    engine::core::logging::install_crash_handler(crash_log_path);
    // 编辑器日志：写入可执行文件同目录，便于排查内嵌预览等问题。
    let log_path = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("editor.log")))
        .unwrap_or_else(|| PathBuf::from("editor.log"));
    // 实时日志缓冲：控制台面板读取，游戏/引擎日志自动上屏。
    let log_sink = LogSink::new(500);
    let _ = engine::core::logging::init_logging_with_sink(&log_path, None, Some(log_sink.clone()));
    // 编辑器主窗口：初始按常规编辑尺寸打开（默认 800x600 过小）。
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
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
            app.set_render_state(creation_context.wgpu_render_state.clone());
            app.project_loaded = arguments.project.is_some();
            // 资源扫描延后到首帧 update：窗口先绘制，避免启动阻塞。
            app.refresh_pending = true;
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
    selected_resource: Option<PathBuf>,
    resources_filter: String,
    lua_runtime: Option<LuaRuntime>,
    console_input: String,
    console_output: Vec<String>,
    /// 编辑器侧提示（如 Scene View 回读状态），与 Lua 控制台输出分开，显示在“编辑器日志”分类。
    console_notices: Vec<String>,
    console_filter: String,
    console_log_level_filter: ConsoleLogLevelFilter,
    /// 控制台左侧分类导航当前选中的日志来源。
    console_tab: ConsoleTab,
    inspector: InspectorState,
    transform: TransformFields,
    transform3d: Transform3DFields,
    mesh_renderer: String,
    camera3d: Camera3DFields,
    light: LightFields,
    collider3d: Collider3dFields,
    text: TextFields,
    image: ImageFields,
    sprite_reference: String,
    sprite_component: SpriteComponent,
    tilemap: TilemapEditorState,
    animation: AnimationEditorState,
    lua_script: String,
    inspector_transform3d_open: bool,
    inspector_collider3d_open: bool,
    inspector_text_open: bool,
    inspector_image_open: bool,
    inspector_sprite_open: bool,
    inspector_tilemap_open: bool,
    inspector_animation_open: bool,
    inspector_add_component: String,
    new_scene_name: String,
    entity_name: String,
    parent_entity: String,
    status: String,
    /// Play 生命周期状态机（M11-01）；`play_session` 只在实际运行/开始切换时存在。
    play_state: PlayState,
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
    show_publish_panel: bool,
    publish_platform: crate::publish::PublishPlatform,
    publish_name: String,
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
    /// eframe 的 egui_wgpu 渲染状态（用于把离屏纹理直接注册为 egui 原生纹理）。
    render_state: Option<eframe::egui_wgpu::RenderState>,
    /// Play 时的内嵌游戏预览（M10-03；生命周期见 preview_session.rs）。
    preview: Option<PreviewSession>,
    /// 设计分辨率（宽×高）：Game 视图渲染目标 / Scene 视图包围框尺寸（Unity 方式）。
    preview_resolution: (u32, u32),
    /// 可用的预览分辨率列表（可增删）。
    preview_resolutions: Vec<(u32, u32)>,
    /// 是否正在输入自定义分辨率。
    adding_preview_resolution: bool,
    /// 自定义分辨率的输入值。
    custom_preview_resolution: (u32, u32),
    /// 游戏 2D 相机中心（世界坐标）：设计分辨率包围框的世界中心，使包围框与
    /// 游戏取景范围一致（默认 `DEFAULT_2D_CAMERA_CENTER`；编辑器场景视图运行时
    /// 重建时捕获）。
    game_camera_2d_center: Vec2,
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
    /// 共享字体图集：运行时随 revision 重建时复用同一实例，避免每帧重新生成
    /// MSDF 字形（拖动 UI/精灵卡顿的根因）。项目切换时重置。
    scene_font_atlas: Option<Arc<Mutex<FontAtlas>>>,
    /// 2D 场景视图显示用的 egui 纹理。
    scene_2d_tex: Option<egui::TextureHandle>,
    /// 游戏画面（设计分辨率 + 游戏相机 + UI 覆盖层）离屏纹理 + 视图：在 Scene 视图
    /// 的设计分辨率包围框内显示，UI 随包围框缩放/平移。
    scene_screen_texture: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// 游戏画面的原生 egui 纹理 ID（经 egui_wgpu 注册，免 CPU 回读，避免与
    /// 编辑器视图的回读冲突）。
    scene_screen_tex_id: Option<egui::TextureId>,
    /// 游戏画面渲染缓存 key：(场景修订, 设计分辨率)，变化时重渲染。
    scene_screen_cache_key: Option<(u64, (u32, u32))>,
    /// 资源浏览器纹理缩略图缓存（按资源相对路径）。
    thumbnails: std::collections::HashMap<String, egui::TextureHandle>,
    /// 后台缩略图解码 worker：避免大资源项目在 UI 线程同步解码导致打开卡死。
    thumbnail_worker: crate::panels::resource_browser::ThumbnailWorker,
    /// 资源目录树缓存（按 filter + refresh_count 复用），避免每帧 clone 全部条目重建。
    resource_tree_cache: Option<crate::panels::resource_browser::ResourceTreeCache>,
    /// 首次 update 时执行一次资源刷新：启动先绘制窗口，扫描不阻塞首帧。
    refresh_pending: bool,
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
    /// 上次执行 hierarchy 元数据同步的场景标识。
    last_metadata_key: Option<(Option<PathBuf>, u64)>,
    /// 最近一次 Scene View 的结构化诊断；用于避免重复把同一条提示刷进控制台。
    /// 游戏预览的诊断由 PreviewSession 自己持有（见 preview_session.rs）。
    preview_diagnostic: Option<String>,
    /// 文档切换后请求一次额外重绘，确保 Scene View 立刻消费新的缓存失效状态。
    scene_view_repaint_pending: bool,
    /// 打开场景/切入 2D 视图后，把 2D 视图适配到设计分辨率包围框（完整显示、
    /// 居中，避免大包围框在窄面板下被裁剪到属性面板边缘）。
    scene_view_fit_pending: bool,
    /// Delete 键当前作用的选区来源：资源面板或层级/Scene View。
    delete_target: DeleteTarget,
    /// E03 统一选择模型：Hierarchy / Scene View / Inspector / 命令层共享的单一权威选区。
    /// 原 `SceneTreeState`/`InspectorState`/`SceneEditorState::selected` 三套选择已全部收敛到此。
    selection: SelectionModel,
    /// E06 未保存切换保护：待确认的文档离开操作。挂起时顶层弹「保存并继续 /
    /// 放弃并继续 / 取消」确认窗，确认前**不**改动文档 / 选择 / 视图 / 历史。
    pending_document_change: Option<crate::actions::PendingChange>,
    /// 确认窗内「保存并继续」保存失败：保留 pending（文档保持 dirty、不切换），
    /// 弹窗内显示失败原因并允许重试或放弃。
    pending_save_failed: bool,
    /// Game View 输入快照的跨帧状态（M11-02）：held 按键/鼠标跨帧保留，
    /// `just_pressed`/`just_released` 边沿只在模拟帧边界消费一次，失焦/Play-Stop 时清空。
    preview_input_state: InputSnapshot,
    /// 自动缩略图压测钩子是否已执行，避免每帧重复跑。
    thumbnail_benchmark_ran: bool,
    /// 自动 Play/Stop 压测钩子是否已执行，避免每帧重复跑。
    play_stop_benchmark_ran: bool,
    /// 自动缩略图压测期间保留纹理句柄，避免上传后立即释放。
    benchmark_textures: Vec<egui::TextureHandle>,
}

#[derive(Clone, Debug)]
struct TilemapEditorState {
    map: Tilemap,
    brush: u8,
    paint_collision: bool,
}

impl Default for TilemapEditorState {
    fn default() -> Self {
        Self {
            map: Tilemap::new(engine::core::transform::Vec2::new(32.0, 32.0), 8, 8)
                .expect("default tilemap should be valid"),
            brush: 1,
            paint_collision: false,
        }
    }
}

#[derive(Clone, Debug)]
struct AnimationEditorState {
    machine: AnimationStateMachine,
}

impl Default for AnimationEditorState {
    fn default() -> Self {
        Self {
            machine: AnimationStateMachine::new(
                "idle",
                vec![AnimationClip {
                    name: "idle".to_owned(),
                    sheet: "assets/textures/player.png".to_owned(),
                    cols: 1,
                    frame_rate: 1.0,
                    looped: true,
                }],
            )
            .expect("default animation should be valid"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceTab {
    Scene,
    Game,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ConsoleLogLevelFilter {
    All,
    Info,
    Warning,
    #[default]
    Error,
}

/// 控制台左侧分类导航：切换“编辑器日志”与“游戏逻辑 Lua 日志”。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ConsoleTab {
    /// 编辑器自身日志（tracing LogSink 缓冲）。
    #[default]
    Editor,
    /// 游戏逻辑 Lua 日志（控制台输出 + 脚本诊断）。
    Lua,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum DeleteTarget {
    #[default]
    None,
    Hierarchy,
    Resource(PathBuf),
}

/// Play 运行生命周期状态机（M11-01）：
/// `Editing → Starting → Playing / StartFailed → Stopping → Editing`。
///
/// 关键约束：初始化全部成功（运行时 + 离屏渲染器 + 预览纹理）后才能进入
/// `Playing`；任一初始化失败进入 `StartFailed` 并保持编辑态；`Stopping` 期间
/// 释放临时世界/输入/GPU 纹理/Lua 状态后回到 `Editing`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PlayState {
    #[default]
    Editing,
    Starting,
    Playing,
    StartFailed,
    Stopping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SceneTool {
    Select,
    Move,
    Rotate,
    Scale,
    TilePaint,
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
    /// UI 组件拖拽预览（实体 id → 组件 x/y 字段的预览位置）。UI 组件的位置
    /// 存在组件自身而非 Transform，拖拽时单独预览，提交时写回组件。
    preview_ui: std::collections::BTreeMap<SceneEntityId, [f32; 2]>,
    /// 3D 实体变换预览（画布俯视 XZ 投影；M9-03/M10-07）。
    preview3d_transform: std::collections::BTreeMap<SceneEntityId, Transform3D>,
    /// 3D 平移/旋转/缩放 Gizmo 拖拽（M9-03/M10-07）。
    gizmo_drag: Option<GizmoDrag>,
    collapsed: std::collections::BTreeSet<SceneEntityId>,
    hidden: std::collections::BTreeSet<SceneEntityId>,
    locked: std::collections::BTreeSet<SceneEntityId>,
    box_select_start: Option<egui::Pos2>,
    tile_paint_active: bool,
    tile_paint_last_cell: Option<(usize, usize, bool)>,
}

struct SceneDrag {
    ids: Vec<SceneEntityId>,
    start: egui::Pos2,
    initial: std::collections::BTreeMap<SceneEntityId, TransformFields>,
    /// UI 组件拖拽初始位置（组件 key、初始 x/y、authored space）。仅在选中
    /// 实体带 Image/Button/Text 组件时填充；拖拽预览与提交均走该路径。
    ui_initial: std::collections::BTreeMap<SceneEntityId, UiDragInitial>,
}

/// 一个 UI 组件拖拽的初始状态。组件 key 在提交时由实体重新推导
/// （见 `set_ui_component_position`）。
#[derive(Clone, Copy)]
struct UiDragInitial {
    /// 组件 x/y 字段（authored 坐标）。UI 组件绘制在设计分辨率包围框内，
    /// Screen/Canvas 两空间拖动增量换算同为 zoom/ppp（见 draw_scene_view）。
    position: [f32; 2],
}

/// 3D 变换 Gizmo 拖拽：约束到哪个轴（Y 为垂直高度，Free 为 XZ 平面；
/// Scale 为等比缩放，Rotate 由 scene_tool 决定）。
struct GizmoDrag {
    id: SceneEntityId,
    axis: GizmoAxis,
    start: egui::Pos2,
    initial: Transform3D,
    start_ray: Option<engine::core::camera3d::Ray3>,
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
    resources: std::collections::BTreeMap<String, engine::core::resource_db::AssetId>,
    metadata: std::collections::BTreeMap<String, String>,
}

impl EditorApp {
    fn new(
        project: ProjectInfo,
        gpu: Option<(wgpu::Device, wgpu::Queue)>,
        log_sink: LogSink,
    ) -> Self {
        let project_loaded = project.root.join("project.toml").is_file();
        let publish_name = crate::publish::default_publish_name(&project.root);
        let recovery = RecoveryManager::new(project.root.join(".temp"));
        let recovery_scan = recovery.scan().unwrap_or_default();
        Self {
            document: ProjectDocument::open(project.clone()),
            project,
            tree: SceneTreeState::new(),
            resources: ResourceBrowserState::new(),
            selected_resource: None,
            resources_filter: String::new(),
            lua_runtime: None,
            console_input: String::new(),
            console_output: Vec::new(),
            console_notices: Vec::new(),
            console_filter: String::new(),
            console_log_level_filter: ConsoleLogLevelFilter::Error,
            console_tab: ConsoleTab::Editor,
            inspector: InspectorState::new(),
            transform: TransformFields::default(),
            transform3d: Transform3DFields::default(),
            mesh_renderer: String::new(),
            camera3d: Camera3DFields::default(),
            light: LightFields::default(),
            collider3d: Collider3dFields::default(),
            text: TextFields::default(),
            image: ImageFields::default(),
            sprite_reference: String::new(),
            sprite_component: SpriteComponent {
                texture: "assets/textures/player.png".to_owned(),
                size: engine::core::transform::Vec2::new(128.0, 128.0),
                pivot: engine::core::transform::Vec2::new(0.5, 0.5),
                tint: [1.0; 4],
                sorting: SpriteSorting::default(),
            },
            tilemap: TilemapEditorState::default(),
            animation: AnimationEditorState::default(),
            lua_script: String::new(),
            inspector_transform3d_open: true,
            inspector_collider3d_open: false,
            inspector_text_open: true,
            inspector_image_open: true,
            inspector_sprite_open: true,
            inspector_tilemap_open: true,
            inspector_animation_open: true,
            inspector_add_component: "Transform3D".to_owned(),
            new_scene_name: "main".to_owned(),
            entity_name: "Entity".to_owned(),
            parent_entity: String::new(),
            status: "尚未创建场景".to_owned(),
            play_state: PlayState::Editing,
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
            show_publish_panel: false,
            publish_platform: crate::publish::PublishPlatform::Pc,
            publish_name,
            publish_compress: false,
            publish_encrypt: false,
            show_resources_panel: true,
            show_lua_console: true,
            log_sink,
            show_about: false,
            last_window_title: None,
            recent_projects: crate::util::load_recent_projects(),
            gpu,
            render_state: None,
            preview: None,
            preview_resolution: (1280, 720),
            preview_resolutions: vec![(640, 360), (960, 540), (1280, 720), (1920, 1080)],
            adding_preview_resolution: false,
            custom_preview_resolution: (1280, 720),
            game_camera_2d_center: DEFAULT_2D_CAMERA_CENTER,
            editor_camera: EditorCamera::default(),
            scene_renderer: None,
            scene_texture: None,
            scene_preview_tex: None,
            scene_runtime: None,
            scene_font_atlas: None,
            scene_2d_tex: None,
            scene_screen_texture: None,
            scene_screen_tex_id: None,
            scene_screen_cache_key: None,
            thumbnails: std::collections::HashMap::new(),
            thumbnail_worker: crate::panels::resource_browser::ThumbnailWorker::new(),
            resource_tree_cache: None,
            refresh_pending: false,
            scene_view_cache_2d: SceneViewCache2d::default(),
            scene_view_cache_3d: SceneViewCache3d::default(),
            scene_runtime_revision: None,
            scene_files_cache: Vec::new(),
            scene_files_cache_time: None,
            preview_diagnostic: None,
            scene_view_repaint_pending: false,
            scene_view_fit_pending: true,
            delete_target: DeleteTarget::None,
            selection: SelectionModel::new(),
            pending_document_change: None,
            pending_save_failed: false,
            last_metadata_key: None,
            preview_input_state: InputSnapshot::default(),
            thumbnail_benchmark_ran: false,
            play_stop_benchmark_ran: false,
            benchmark_textures: Vec::new(),
        }
    }

    fn is_playing(&self) -> bool {
        matches!(
            self.play_state,
            PlayState::Starting | PlayState::Playing | PlayState::Stopping
        )
    }

    fn set_render_state(&mut self, render_state: Option<eframe::egui_wgpu::RenderState>) {
        self.render_state = render_state;
    }

    fn autoplay_scene_path(&self) -> PathBuf {
        std::env::var("P2_EDITOR_AUTOPLAY_SCENE")
            .ok()
            .map(|path| self.project.root.join(path))
            .unwrap_or_else(|| self.project.root.join("scenes/main.scene"))
    }

    fn maybe_run_auto_benchmarks(&mut self, context: &egui::Context) {
        if !self.project_loaded {
            return;
        }
        if self.run_thumbnail_window_benchmark(context) {
            return;
        }
        let _ = self.run_play_stop_window_benchmark(context);
    }

    fn run_thumbnail_window_benchmark(&mut self, context: &egui::Context) -> bool {
        let Some(raw) = std::env::var("P2_EDITOR_THUMBNAIL_BENCH").ok() else {
            return false;
        };
        if self.thumbnail_benchmark_ran {
            return true;
        }
        self.thumbnail_benchmark_ran = true;
        let count = raw
            .parse::<usize>()
            .ok()
            .filter(|count| *count > 0)
            .unwrap_or(1000);
        let relative = std::env::var("P2_EDITOR_THUMBNAIL_SOURCE")
            .ok()
            .unwrap_or_else(|| "assets/textures/slime.png".to_owned());
        let path = self.project.root.join(&relative);
        let started = Instant::now();
        let mut total_rgba_bytes = 0usize;
        let mut total_thumb_bytes = 0usize;
        let mut total_pixels = 0usize;
        self.benchmark_textures.clear();
        for index in 0..count {
            let Ok(asset) = TextureAsset::load_png(&path, TextureColorSpace::Srgb) else {
                tracing::error!(
                    target: "editor",
                    "[editor-perf] thumbnail window failed: source={} missing_or_invalid",
                    path.display()
                );
                context.send_viewport_cmd(egui::ViewportCommand::Close);
                return true;
            };
            total_rgba_bytes += asset.pixels().len();
            let (pixels, width, height) = downscale_rgba_benchmark(
                asset.pixels(),
                asset.width() as usize,
                asset.height() as usize,
                64,
            );
            total_thumb_bytes += pixels.len();
            total_pixels += width * height;
            let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &pixels);
            let handle = context.load_texture(
                format!("thumb-bench-{index}"),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.benchmark_textures.push(handle);
        }
        let total_ms = started.elapsed().as_secs_f64() * 1000.0;
        tracing::info!(
            target: "editor",
            "[editor-perf] thumbnail window: thumbnails={count} total_ms={total_ms:.3} avg_ms={:.3} rgba_bytes={} thumb_bytes={} thumb_pixels={} source={}",
            total_ms / count as f64,
            total_rgba_bytes,
            total_thumb_bytes,
            total_pixels,
            relative
        );
        context.send_viewport_cmd(egui::ViewportCommand::Close);
        true
    }

    fn run_play_stop_window_benchmark(&mut self, context: &egui::Context) -> bool {
        let Some(raw) = std::env::var("P2_EDITOR_PLAYSTOP_BENCH").ok() else {
            return false;
        };
        if self.play_stop_benchmark_ran {
            return true;
        }
        self.play_stop_benchmark_ran = true;
        let iterations = raw
            .parse::<usize>()
            .ok()
            .filter(|count| *count > 0)
            .unwrap_or(100);
        if self.document.current_scene.is_none() {
            let _ = self.open_scene_document(self.autoplay_scene_path());
        }
        let Some(original) = self.document.current_scene.clone() else {
            tracing::error!(target: "editor", "[editor-perf] playstop window failed: no scene");
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            return true;
        };
        let started = Instant::now();
        for iteration in 0..iterations {
            self.play();
            if !matches!(self.play_state, PlayState::Playing) {
                tracing::error!(
                    target: "editor",
                    "[editor-perf] playstop window failed: start failed at iteration={iteration}"
                );
                context.send_viewport_cmd(egui::ViewportCommand::Close);
                return true;
            }
            self.stop();
            if self.document.current_scene.as_ref() != Some(&original) {
                tracing::error!(
                    target: "editor",
                    "[editor-perf] playstop window failed: drift at iteration={iteration}"
                );
                context.send_viewport_cmd(egui::ViewportCommand::Close);
                return true;
            }
        }
        let total_ms = started.elapsed().as_secs_f64() * 1000.0;
        tracing::info!(
            target: "editor",
            "[editor-perf] playstop window: iterations={iterations} total_ms={total_ms:.3} avg_ms={:.3} entities={}",
            total_ms / iterations as f64,
            original.scene.entities.len()
        );
        context.send_viewport_cmd(egui::ViewportCommand::Close);
        true
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

    /// 游戏 2D 相机中心转为 egui 坐标，供设计分辨率包围框 / UI 映射定位。
    fn game_camera_center_egui(&self) -> egui::Vec2 {
        egui::vec2(self.game_camera_2d_center.x, self.game_camera_2d_center.y)
    }

    fn select_resource(&mut self, path: PathBuf) {
        self.selected_resource = Some(path.clone());
        self.delete_target = DeleteTarget::Resource(path);
    }

    fn clear_resource_selection(&mut self) {
        self.selected_resource = None;
        if matches!(self.delete_target, DeleteTarget::Resource(_)) {
            self.delete_target = DeleteTarget::None;
        }
    }

    fn focus_hierarchy_delete_target(&mut self) {
        self.delete_target = DeleteTarget::Hierarchy;
    }

    fn push_console_notice(&mut self, line: impl Into<String>) {
        // 编辑器侧提示走独立缓冲，避免混进 Lua 控制台输出（“游戏 Lua 日志”分类）。
        self.console_notices.push(line.into());
        const MAX_CONSOLE_LINES: usize = 500;
        if self.console_notices.len() > MAX_CONSOLE_LINES {
            let overflow = self.console_notices.len() - MAX_CONSOLE_LINES;
            self.console_notices.drain(0..overflow);
        }
    }

    fn handle_delete_shortcut(&mut self) {
        match self.delete_target.clone() {
            DeleteTarget::Resource(path) => {
                if self.selected_resource.as_ref() == Some(&path) {
                    self.delete_selected_resource();
                }
            }
            DeleteTarget::Hierarchy => self.delete_selected_entity(),
            DeleteTarget::None => {}
        }
    }

    /// E06：关闭编辑器入口（窗口 X / Alt+F4 / 系统退出）。
    ///
    /// - 当前编辑文档 clean → 放行，正常关闭。
    /// - dirty 且没有待确认操作 → `CancelClose` 阻止关闭，挂起 `QuitEditor` 弹确认窗。
    /// - 已有待确认操作（含用户再次按 X）→ 一律先 `CancelClose`，不覆盖现有 pending
    ///   （防重复触发切换请求与重复弹窗）。
    fn guard_close_request(&mut self, context: &egui::Context) {
        let close_requested = context.input(|input| input.viewport().close_requested());
        if !close_requested {
            return;
        }
        if self.pending_document_change.is_some() {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        let dirty = self
            .document
            .current_scene
            .as_ref()
            .is_some_and(editor::SceneDocument::is_dirty);
        if !dirty {
            // 无未保存更改：允许关闭。
            return;
        }
        context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.pending_document_change = Some(crate::actions::PendingChange::QuitEditor);
        self.pending_save_failed = false;
    }

    /// E06：顶层「未保存的更改」确认窗。三选项：
    /// 「保存并继续」（保存成功才继续，失败保留 pending 并提示）、
    /// 「放弃并继续」、以及「取消」（取消挂起的离开操作，保持文档 / 选择 / 视图 / 历史）。
    /// 关闭编辑器（QuitEditor）在确认后发送真正的 Close。
    pub(crate) fn draw_pending_change_window(&mut self, context: &egui::Context) {
        let Some(change) = self.pending_document_change.clone() else {
            return;
        };
        let scene_name = self
            .document
            .current_scene
            .as_ref()
            .map(|doc| doc.scene.name.clone())
            .unwrap_or_else(|| "当前场景".to_owned());
        let save_failed = self.pending_save_failed;
        let failure_status = if save_failed {
            self.status.clone()
        } else {
            String::new()
        };
        let action = change.action_label();

        let mut decision: Option<crate::actions::PendingDecision> = None;
        egui::Window::new("未保存的更改")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(context, |ui| {
                ui.label(format!("场景「{scene_name}」有尚未保存的更改。"));
                ui.label(format!("操作：{action}"));
                if save_failed {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 90, 90),
                        format!("保存失败：{failure_status}"),
                    );
                    ui.label("未保存的更改已保留，可重试保存或选择放弃。");
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("保存并继续").clicked() {
                        decision = Some(crate::actions::PendingDecision::Save);
                    }
                    if ui.button("放弃并继续").clicked() {
                        decision = Some(crate::actions::PendingDecision::Discard);
                    }
                    if ui.button("取消").clicked() {
                        decision = Some(crate::actions::PendingDecision::Cancel);
                    }
                });
            });
        if let Some(decision) = decision {
            self.resolve_pending_change(context, decision);
        }
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        // E06：用户点窗口 X / Alt+F4 —— 若当前文档 dirty，阻止关闭并弹确认窗
        // （确认「保存并继续 / 放弃并继续」后再真正 Close；「取消」保持原状态）。
        self.guard_close_request(context);
        // 重置缩略图 worker 每帧提交预算。
        self.thumbnail_worker.begin_frame();
        // 启动时把资源扫描延后到首帧：窗口先绘制，扫描不阻塞首帧展示。
        if self.refresh_pending {
            self.refresh_pending = false;
            self.refresh_resources();
        }
        if self.scene_view_repaint_pending {
            context.request_repaint();
            self.scene_view_repaint_pending = false;
        }
        if self.project_loaded
            && let Some(doc) = self.document.current_scene.as_ref()
        {
            let _ = self.autosave.tick(&self.recovery, doc, SystemTime::now());
        }
        // 调试钩子：`P2_EDITOR_AUTOPLAY=1` 打开指定场景、切 3D 视口并自动 Play。
        // 可通过 `P2_EDITOR_AUTOPLAY_SCENE=scenes/foo.scene` 覆盖默认场景。
        if std::env::var("P2_EDITOR_AUTOPLAY").is_ok() && !self.is_playing() {
            self.scene_view_mode = SceneViewMode::View3D;
            if self.document.current_scene.is_none() {
                let _ = self.open_scene_document(self.autoplay_scene_path());
            }
            if self.document.current_scene.is_some() {
                self.play();
            }
        }
        self.maybe_run_auto_benchmarks(context);
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
        if self.project_loaded
            && !context.wants_keyboard_input()
            && context.input(|i| i.key_pressed(egui::Key::Delete))
        {
            self.handle_delete_shortcut();
        }
        self.sync_hierarchy_metadata();
        self.poll_progress();
        // 待导入资源非空且无进行中任务时，启动后台资源导入（不阻塞 UI）。
        self.start_resource_import();
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
        self.draw_publish_panel(context);
        self.draw_about_window(context);
        // E06：最后绘制「未保存的更改」确认窗，置于其余窗口之上；仅在 dirty 且
        // 有挂起的离开操作时显示，此时其它面板保持原样、文档不被改动。
        self.draw_pending_change_window(context);
    }
}

#[cfg(test)]
mod layout_tests {
    use eframe::egui;

    /// 面板布局：左侧资源(200)+层级(190)、右侧属性(360)、中央。验证在多种窗口
    /// 宽度下，中央面板（Scene/Game 视图所在）都不与右侧属性面板重叠。
    #[test]
    fn central_panel_does_not_overlap_inspector_at_any_width() {
        for width in [1280.0, 1024.0, 900.0, 800.0, 700.0, 600.0] {
            let ctx = egui::Context::default();
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 720.0),
                )),
                ..Default::default()
            };
            ctx.begin_pass(raw);
            egui::SidePanel::left("t_resource")
                .default_width(200.0)
                .show(&ctx, |ui| {
                    ui.label("resource");
                });
            egui::SidePanel::left("t_tree")
                .default_width(190.0)
                .show(&ctx, |ui| {
                    ui.label("hierarchy");
                });
            let mut inspector_rect = egui::Rect::NOTHING;
            egui::SidePanel::right("t_inspector")
                .default_width(360.0)
                .show(&ctx, |ui| {
                    inspector_rect = ui.max_rect();
                });
            let mut central_rect = egui::Rect::NOTHING;
            egui::CentralPanel::default().show(&ctx, |ui| {
                central_rect = ui.max_rect();
            });
            let _ = ctx.end_pass();

            assert!(
                central_rect.right() <= inspector_rect.left() + 1.0,
                "窗口宽 {width}: 中央面板右缘 {:.1} 越过属性面板左缘 {:.1}（重叠）",
                central_rect.right(),
                inspector_rect.left()
            );
            assert!(
                central_rect.width() >= 0.0,
                "窗口宽 {width}: 中央面板宽度为负：{}",
                central_rect.width()
            );
        }
    }
}
