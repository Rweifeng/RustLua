#![cfg_attr(windows, windows_subsystem = "windows")]

mod actions;
mod command_router;
mod geometry;
mod hierarchy_ops;
mod import;
mod menu;
mod panels;
mod perf_baseline;
mod preview_session;
mod publish;
mod scene_drag;
mod scene_interaction;
mod scene_ops;
mod ui_editor;
mod util;

use crate::panels::progress::ProgressPanel;
use crate::preview_session::PreviewSession;
// E11：场景工具枚举只有**一处**定义（交互词汇表里），main.rs 不再自带一份。
use crate::scene_interaction::SceneTool;
use crate::util::{configure_cjk_fallback, format_project_error};

use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use editor::{
    AutoSaveController, Collider3dFields, InspectorState,
    PlaySession, ProjectDocument, ProjectInfo, RecoveryManager, RecoveryScan, ResourceBrowserState,
    SceneTreeState, SelectionModel, TransformFields,
    check_project, open_project,
};
use eframe::egui;
use engine::core::{
    animation::{AnimationClip, AnimationStateMachine},
    camera::DEFAULT_2D_CAMERA_CENTER,
    editor_plugins::PluginRegistry,
    font::FontAtlas,
    input::InputSnapshot,
    logging::LogSink,
    math3d::Vec3,
    preview_diagnostics::DiagnosticCollector,
    scene::SceneEntityId,
    script::LuaRuntime,
    texture::{TextureAsset, TextureColorSpace},
    tilemap::Tilemap,
    transform::Transform3D,
    transform::Vec2,
    ui_animation::{UiAnimationClip, UiAnimationPreview},
    ui_binding::BindingSet,
    ui_events::UiEvents,
    ui_layout::CanvasConfig,
    ui_layout_cache::UiLayoutCache,
    ui_states::ButtonStateSet,
};
use engine::host::{Renderer, RuntimeScene};

#[derive(Debug, Default)]
struct Arguments {
    project: Option<PathBuf>,
    check: bool,
    /// WP2 迁移命令：`--migrate-spatial`（dry-run）/ `--migrate-spatial-write` / `--restore-spatial`。
    migrate_spatial: bool,
    migrate_spatial_write: bool,
    restore_spatial: bool,
    /// 迁移报告的落盘路径（docs/54 §8-⑧ 的"报告"）：给了就写一份可归档的报告 artifact。
    migrate_report: Option<PathBuf>,
    /// ER-01 基线：无窗口、无 GPU 的编辑器性能基线报告（docs/55 阶段 0）。
    perf_baseline: bool,
    /// 基线报告落盘路径（可选；同时始终打印到 stdout）。
    perf_report: Option<PathBuf>,
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
            "--migrate-spatial" => arguments.migrate_spatial = true,
            "--migrate-spatial-write" => {
                arguments.migrate_spatial = true;
                arguments.migrate_spatial_write = true;
            }
            "--restore-spatial" => arguments.restore_spatial = true,
            "--migrate-report" => {
                let path = values
                    .next()
                    .ok_or_else(|| "--migrate-report 需要一个路径参数".to_owned())?;
                arguments.migrate_report = Some(PathBuf::from(path));
            }
            "--perf-baseline" => arguments.perf_baseline = true,
            "--perf-report" => {
                let path = values
                    .next()
                    .ok_or_else(|| "--perf-report 需要一个路径参数".to_owned())?;
                arguments.perf_report = Some(PathBuf::from(path));
            }
            "--help" | "-h" => {
                println!(
                    "用法：editor [--project <项目路径>] [--check]\n\
                     \x20     editor --project <项目路径> --migrate-spatial [--migrate-spatial-write]\n\
                     \x20     editor --project <项目路径> --restore-spatial\n\
                     \x20     editor --perf-baseline [--perf-report <路径>]\n\
                     \x20     （迁移命令可加 --migrate-report <路径> 把报告落盘；有实体被拒绝或出错时退出码为 2）\n\
                     \x20     （--perf-baseline 无窗口运行 docs/55 ER-01 基线场景；有场景不达标时退出码为 2）"
                );
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

/// p2 工作区根（含 scripts/、docs/ 等），相对编辑器 crate 上溯 3 级：`E:\Codex\RustLua`。
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

/// WP2 空间层级迁移命令：对项目 `scenes/**/*.scene` 逐文件 dry-run / 写回 / 回滚。
///
/// - 默认 **dry-run**：只报告每个场景有多少实体会被转成局部坐标、多少无法无损迁移；
/// - `--migrate-spatial-write`：写回（备份 `<file>.bak` + 把幂等标记 `meta spatial=local`
///   写进场景**自身**，因此只有一个提交点），并逐条报出命中 docs/30「已删除的旧设计」的用法；
/// - `--restore-spatial`：从 `.bak` 一键回滚（标记随备份内容一起消失）。
///
/// 返回值是 `(报告文本, 是否被阻断)`。阻断 = 有实体被拒绝（无法无损迁移）或有权处理失败；
/// 退出码 2 由调用方给出，让脚本能把它当成质量门使用（**报告仍会打印/落盘**，不是静默失败）。
fn run_spatial_migration(project_root: &Path, write: bool, restore: bool) -> (String, bool) {
    use engine::core::scene_migration::{
        MigrationOptions, migrate_scene_file, project_scene_files, restore_scene_backup,
    };

    let files = project_scene_files(project_root);
    if files.is_empty() {
        return (
            format!("{}: 没有找到 scenes/**/*.scene\n", project_root.display()),
            true,
        );
    }
    let mut out = String::new();
    let mut refused_total = 0usize;
    let mut converted_total = 0usize;
    let mut written_total = 0usize;
    let mut removed_total = 0usize;
    let mut skipped_total = 0usize;
    let mut error_total = 0usize;
    for file in &files {
        let relative = file
            .strip_prefix(project_root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        if restore {
            match restore_scene_backup(file) {
                Ok(true) => out.push_str(&format!("restored\t{relative}\n")),
                Ok(false) => out.push_str(&format!("no-backup\t{relative}\n")),
                Err(error) => {
                    error_total += 1;
                    out.push_str(&format!("error\t{relative}\t{error}\n"));
                }
            }
            continue;
        }
        match migrate_scene_file(
            file,
            MigrationOptions {
                write,
                backup: true,
            },
        ) {
            Ok(outcome) => {
                refused_total += outcome.refused;
                converted_total += outcome.converted;
                removed_total += outcome.removed_designs.len();
                if outcome.written {
                    written_total += 1;
                }
                if outcome.already_migrated {
                    skipped_total += 1;
                }
                out.push_str(&format!(
                    "{}\t{relative}\tentities={}\tconverted={}\trefused={}\tskipped={}\talready_migrated={}\tverified={}\tremoved={}\n",
                    if outcome.written {
                        "written"
                    } else if outcome.already_migrated {
                        "already-migrated"
                    } else {
                        "dry-run"
                    },
                    outcome.entities,
                    outcome.converted,
                    outcome.refused,
                    outcome.skipped,
                    outcome.already_migrated,
                    outcome.verified,
                    outcome.removed_designs.len(),
                ));
                // docs/54 §8-⑧「弃用告警」：点名旧写法 + 现在该写在哪（docs/30 的表）。
                for design in &outcome.removed_designs {
                    out.push_str(&format!(
                        "removed\t{relative}\tentity={}\t{}.{}\t→ {}\n",
                        design.entity.0, design.component, design.removed, design.replacement
                    ));
                }
            }
            Err(error) => {
                error_total += 1;
                out.push_str(&format!("error\t{relative}\t{error}\n"));
            }
        }
    }
    let blocked = refused_total > 0 || error_total > 0;
    out.push_str(&format!(
        "{} files={} converted={} refused={} removed={} already_migrated={} written={} errors={} mode={}\n",
        if blocked {
            "spatial_migration_blocked"
        } else {
            "spatial_migration_ok"
        },
        files.len(),
        converted_total,
        refused_total,
        removed_total,
        skipped_total,
        written_total,
        error_total,
        if restore {
            "restore"
        } else if write {
            "write"
        } else {
            "dry-run"
        }
    ));
    (out, blocked)
}

/// 把迁移报告落盘（docs/54 §8-⑧ 的"报告"要能归档，而不是只在终端滚过去）。
fn write_migration_report(path: &Path, report: &str) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建报告目录失败 {}: {error}", parent.display()))?;
    }
    std::fs::write(path, report)
        .map_err(|error| format!("写入迁移报告失败 {}: {error}", path.display()))
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

    if arguments.migrate_spatial || arguments.restore_spatial {
        let project_path = arguments
            .project
            .ok_or("--migrate-spatial requires --project <path>")?;
        let (report, blocked) = run_spatial_migration(
            &project_path,
            arguments.migrate_spatial_write,
            arguments.restore_spatial,
        );
        print!("{report}");
        if let Some(report_path) = &arguments.migrate_report {
            write_migration_report(report_path, &report)?;
        }
        if blocked {
            // 退出码 2 = 有实体无法无损迁移或有权处理失败：脚本可以把它当质量门，
            // 而报告已经打印/落盘，不是静默失败。
            std::process::exit(2);
        }
        return Ok(());
    }

    if arguments.perf_baseline {
        // ER-01：无窗口基线报告；有场景不达标时退出码 2（可当质量门）。
        let ok = perf_baseline::run_and_write_report(arguments.perf_report.as_deref())?;
        if !ok {
            std::process::exit(2);
        }
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
            configure_cjk_fallback(&creation_context.egui_ctx);
            // 编辑器皮肤默认深色：固定使用 egui 深色主题，不随系统深浅色切换。
            creation_context.egui_ctx.set_theme(egui::Theme::Dark);
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
            app.initialize_ui_extensions();
            app.project_loaded = arguments.project.is_some();
            // 资源扫描延后到首帧 update：窗口先绘制，避免启动阻塞。
            app.refresh_pending = true;
            // 启动即把当前项目记入「最近项目」。
            if app.project_loaded {
                let root = app.project.root.clone();
                app.record_recent_project(&root);
                // 读取持久化布局（缺失/损坏回落默认，损坏不覆盖源文件）。
                app.load_layout_for_project();
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
    /// 资源右键"重命名…"的进行中状态：`(原路径, 输入缓冲)`（第 44 轮 P1）。
    resource_rename: Option<(PathBuf, String)>,
    resources_filter: String,
    lua_runtime: Option<LuaRuntime>,
    console_input: String,
    console_output: Vec<String>,
    /// 编辑器侧提示（如 Scene View 回读状态），与 Lua 控制台输出分开，显示在“编辑器日志”分类。
    console_notices: Vec<String>,
    /// 控制台行索引缓存（按分类/过滤/级别/来源代数失效），配合固定行高虚拟滚动。
    console_rows_cache: crate::panels::console::ConsoleRowCache,
    console_filter: String,
    console_log_level_filter: ConsoleLogLevelFilter,
    /// 控制台左侧分类导航当前选中的日志来源。
    console_tab: ConsoleTab,
    inspector: InspectorState,
    mesh_renderer: String,
    collider3d: Collider3dFields,
    container: editor::ContainerFields,
    tilemap: TilemapEditorState,
    animation: AnimationEditorState,
    lua_script: String,
    inspector_collider3d_open: bool,
    inspector_tilemap_open: bool,
    inspector_animation_open: bool,
    inspector_add_component: String,
    entity_name: String,
    status: String,
    /// Play 生命周期状态机（M11-01）；`play_session` 只在实际运行/开始切换时存在。
    /// Play 模式状态机（Pause/Step/Restart + 固定步计划，docs/55 ER-10）。
    play: editor::PlayController,
    play_session: Option<PlaySession>,
    history: editor::CommandService,
    recovery: RecoveryManager,
    autosave: AutoSaveController,
    recovery_scan: RecoveryScan,
    /// Inspector Override 摘要的源 Prefab 缓存（按文件 (长度, mtime) 失效）。
    /// 之前每帧都 `Prefab::load`：读文件 + 解析 + 建 BTreeMap。
    pub(crate) prefab_source_cache:
        std::collections::HashMap<std::path::PathBuf, crate::panels::inspector::CachedPrefabSource>,
    project_loaded: bool,
    import_path: String,
    workspace_tab: WorkspaceTab,
    scene_zoom: f32,
    /// Scene View 交互状态机（E11）：**工具与交互阶段的唯一事实源**。
    ///
    /// 以前这里是裸的 `scene_tool: SceneTool` 字段 + `scene_state` 里的
    /// `box_select_start`，工具的判断散落在各处；现在工具切换与指针事件都走这里，
    /// 阶段（Idle/Dragging/Marquee/Painting）由状态机推导。
    interaction: crate::scene_interaction::ToolStateMachine,
    scene_view_mode: SceneViewMode,
    show_scene_guides: bool,
    show_safe_area: bool,
    show_pixel_grid: bool,
    show_performance_hud: bool,
    snap_enabled: bool,
    snap_step: f32,
    hierarchy_filter: String,
    /// Inspector 的**字段搜索**（第 42 轮 P1）：只影响 schema 字段行的呈现，不改文档。
    inspector_field_filter: String,
    /// Gizmo 的**变换空间**（第 43 轮 P1：世界/局部）：只影响拖拽语义，不改文档。
    gizmo_space: crate::scene_interaction::GizmoSpace,
    /// Gizmo 的**轴心模式**（第 46 轮 P1：轴心/中心）：只影响手柄原点与 2D 旋转缩放中心。
    gizmo_pivot: crate::scene_interaction::GizmoPivot,
    scene_state: SceneEditorState,
    inspector_clipboard: Option<ComponentClipboard>,
    progress: ProgressPanel,
    show_publish_panel: bool,
    publish_platform: crate::publish::PublishPlatform,
    publish_name: String,
    publish_compress: bool,
    publish_encrypt: bool,
    /// 底部资源浏览器 / Lua 控制台面板的显示开关（收起后 Scene/Game 视图获得最大空间）。
    show_resources_panel: bool,
    show_lua_console: bool,
    /// M19-03 领域数据面板（Lua 域校验 + 只读视图 + PersistId 注册表）。
    show_domain_data_panel: bool,
    domain_report: Option<crate::panels::domain_data::DomainDataReport>,
    persist_registry: Option<crate::panels::domain_data::PersistRegistry>,
    /// M19-03 Scene 视图的出口/地点只读覆盖层开关。
    show_domain_overlay: bool,
    /// M19-03 运行时调试面板（只读探针）与最近一次读取结果。
    show_runtime_debug_panel: bool,
    runtime_probe_results: Option<Vec<crate::panels::runtime_debug::RuntimeProbeResult>>,
    runtime_probe_error: Option<String>,
    /// M19-03 Inspector 的 `entity_meta persist_id` 编辑缓冲与目标实体。
    persist_id_buffer: String,
    persist_id_editing: Option<SceneEntityId>,
    /// 实时日志缓冲（tracing 层写入，控制台面板读取显示）。
    log_sink: LogSink,
    /// “版本说明”窗口是否打开。
    show_about: bool,
    /// 「项目设置」窗口缓冲与开关（内容在每次打开时从 project.toml 读取）。
    project_settings: crate::panels::project_settings::ProjectSettingsUi,
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
    /// 共享默认矢量轮廓图集：Vector 模式下运行时随 revision 重建时复用，
    /// 避免每次拖动重新解析大字体文件。项目切换时重置。
    scene_vector_atlas: Option<engine::host::frame::VectorFontHandle>,
    /// 场景 Text 自定义字体的共享缓存（key = `font://<asset>`）。编辑器注入同一
    /// 实例给场景视图/预览运行时，跨重建复用字形；项目切换/重新收集字符时清空。
    custom_font_atlases: engine::host::frame::FontAtlasMap,
    /// 场景 Text 自定义字体的矢量轮廓共享缓存（key = `font://<asset>`）。
    custom_vector_atlases: engine::host::frame::VectorAtlasMap,
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
    /// 资源刷新任务代际；只允许最新任务提交结果。
    resource_refresh_generation: u64,
    resource_refresh_job: Option<editor::RefreshHandle>,
    /// 文件监听句柄（worker 线程扫描 + 通道回传）：外部改动 2 秒预算内触发后台刷新。
    file_watcher: Option<editor::WatchHandle>,
    /// 文件轮询间隔（默认 500 ms：预算 2 秒，留足余量给后台扫描与提交）。
    file_watch_interval: std::time::Duration,
    /// 持久化布局（docs/55 P1）：面板宽度/可见性、工作台标签、视图模式与缩放。
    layout: editor::EditorLayout,
    /// 布局加载结果（缺失/损坏时的原因，用于状态栏提示与"重置布局"）。
    layout_load: editor::LayoutLoad,
    /// Editor Profiler（docs/55 P0）：帧/面板/命令耗时、计数器与 gauge。
    profiler: editor::EditorProfiler,
    hierarchy_visible_cache: crate::panels::hierarchy::HierarchyVisibleCache,
    /// Hierarchy 小写化名称缓存（按文档修订失效）：搜索改词不再重算 10k 个名字。
    hierarchy_lower: crate::panels::hierarchy::HierarchyLowerCache,
    /// Hierarchy 行内容高度探针（docs/55 §8 的"不得重叠/裁字"门槛用测量而非肉眼）。
    hierarchy_row_probe: crate::panels::hierarchy::RowHeightProbe,
    /// Console 行内容高度探针（同上）。
    console_row_probe: crate::panels::console::RowHeightProbe,
    /// 2D Scene 视图渲染缓存（变更检测，避免内容未变化时重复渲染）。
    scene_view_cache_2d: SceneViewCache2d,
    /// 3D Scene 视图渲染缓存（同上）。
    scene_view_cache_3d: SceneViewCache3d,
    /// 3D Scene 视图的 typed 提取缓存（WP1-4）：按文档 revision 失效，避免每帧解析
    /// 组件字符串。
    scene_view_3d_typed: SceneView3dTyped,
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
    /// E07 活动编辑事务：一条连续交互（拖拽等）在释放前只更新 preview，不写
    /// 磁盘、不追加 History；提交/取消在顶层网关统一处理。
    edit_transaction: editor::EditTransactionState,
    /// E08 Inspector 缓冲脏编辑标记 + 已提交基线（见 actions.rs 顶部 EditState/EditSnapshot）。
    /// Inspector 面板改动 buffer 后与基线 diff，不一致才自动开事务直写文档，
    /// 从而一次拖动/输入只对应一条历史、不产生空操作。
    inspector_edit: crate::actions::EditState,
    inspector_baseline: crate::actions::EditSnapshot,
    /// E15/E18: Canvas metadata editing buffer for the selected UI root.
    canvas_config: CanvasConfig,
    /// E22/E23: structured UI editing buffers loaded from the selected entity.
    events_editor: UiEvents,
    bindings_editor: BindingSet,
    /// E20: five-state button texture configuration for the selected widget.
    button_states_editor: ButtonStateSet,
    /// E26: animation editing data plus an isolated preview playback state.
    ui_animation_editor: UiAnimationClip,
    ui_animation_preview: Option<UiAnimationPreview>,
    ui_animation_playing: bool,
    /// E24: deduplicated Game View / Console diagnostics.
    preview_diagnostics: DiagnosticCollector,
    /// E25: layout cache statistics exposed through the performance HUD.
    ui_layout_cache: UiLayoutCache,
    /// E27: fault-isolated extension container held by the editor app.
    plugin_registry: PluginRegistry,
    plugin_status: Vec<String>,
    /// E06 未保存切换保护：待确认的文档离开操作。挂起时顶层弹「保存并继续 /
    /// 放弃并继续 / 取消」确认窗，确认前**不**改动文档 / 选择 / 视图 / 历史。
    pending_document_change: Option<crate::actions::PendingChange>,
    /// 确认窗内「保存并继续」保存失败：保留 pending（文档保持 dirty、不切换），
    /// 弹窗内显示失败原因并允许重试或放弃。
    pending_save_failed: bool,
    /// Game View 输入快照的跨帧状态（M11-02）：held 按键/鼠标跨帧保留，
    /// `just_pressed`/`just_released` 边沿只在模拟帧边界消费一次，失焦/Play-Stop 时清空。
    preview_input_state: InputSnapshot,
    /// Hierarchy 实体复制剪贴板，保存源实体及其子树快照。
    hierarchy_clipboard: Option<engine::core::scene::SceneEntity>,
    /// H02 行内重命名会话（目标实体 + 输入缓冲 + 首帧聚焦标记）。
    hierarchy_rename: Option<HierarchyRename>,
    /// H03 Shift 范围选择的锚点：最近一次非 Shift 选择/加选的实体。
    selection_anchor: Option<SceneEntityId>,
    /// Play/Stop 前的编辑器上下文快照：恢复时重建选择、工具、视图和分辨率状态。
    editor_context_snapshot: Option<EditorContextSnapshot>,
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
    /// M19-03 Tile 编辑模式：单格画笔 / 矩形填充 / 区域复制 / 区域粘贴。
    mode: TileEditMode,
    /// 区域复制剪贴板（tile 值 + solid 位；编辑期状态，与文档无关，不产生历史）。
    clipboard: Option<crate::panels::tilemap_tools::TileRegion>,
}

/// M19-03 Tilemap 生产工具模式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TileEditMode {
    /// 原有单格画笔（左键画、右键擦）。
    #[default]
    Point,
    /// 拖拽框选 → 一次矩形填充（单条 Undo）。
    RectFill,
    /// 拖拽框选 → 复制区域到剪贴板。
    RegionCopy,
    /// 单击 → 把剪贴板内容粘贴到该格（越界裁剪，单条 Undo）。
    RegionPaste,
}

impl TileEditMode {
    /// 工具与预览文案（场景视图左上角提示 / 工具栏按钮 tooltip）。
    fn label(self) -> &'static str {
        match self {
            Self::Point => "单格画笔",
            Self::RectFill => "矩形填充",
            Self::RegionCopy => "区域复制",
            Self::RegionPaste => "区域粘贴",
        }
    }
}

impl Default for TilemapEditorState {
    fn default() -> Self {
        Self {
            map: Tilemap::new(engine::core::transform::Vec2::new(32.0, 32.0), 8, 8)
                .expect("default tilemap should be valid"),
            brush: 1,
            paint_collision: false,
            mode: TileEditMode::default(),
            clipboard: None,
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
                    events: Vec::new(),
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

/// H02 行内重命名会话：目标实体 + 输入缓冲。
/// `just_started` 表示刚进入编辑态（或空名被拒后重试），下一帧强制聚焦输入框。
struct HierarchyRename {
    id: SceneEntityId,
    buffer: String,
    just_started: bool,
}

/// Play 运行生命周期状态机（M11-01）：
/// `Editing → Starting → Playing / StartFailed → Stopping → Editing`。
///
/// 关键约束：初始化全部成功（运行时 + 离屏渲染器 + 预览纹理）后才能进入
/// `Playing`；任一初始化失败进入 `StartFailed` 并保持编辑态；`Stopping` 期间
/// 释放临时世界/输入/GPU 纹理/Lua 状态后回到 `Editing`。
/// Play 模式由 `editor::PlayController` 持有状态机（含 Pause/Step/Restart，docs/55 ER-10）；
/// 这里只保留一个转发访问器，避免"状态两处各写一份"。
impl EditorApp {
    fn play_state(&self) -> editor::PlayState {
        self.play.state()
    }

    /// 当前场景工具（E11：由交互状态机持有，不再是一个裸字段）。
    pub(crate) fn scene_tool(&self) -> SceneTool {
        self.interaction.tool()
    }

    /// 切换场景工具：走状态机，因此**切换工具会取消进行中的交互**
    /// （`ToolStateMachine::handle(ToolSelected)` 的既有语义，不再需要调用方自己清场）。
    pub(crate) fn set_scene_tool(&mut self, tool: SceneTool) {
        self.interaction
            .handle(crate::scene_interaction::InteractionEvent::ToolSelected(tool));
    }
}

/// 场景视图投影模式：2D 画布（俯视 XY）或 3D 画布（俯视 XZ）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SceneViewMode {
    #[default]
    View2D,
    Widget,
    Prefab,
    View3D,
}

#[derive(Clone, Debug)]
struct EditorContextSnapshot {
    selection: SelectionModel,
    workspace_tab: WorkspaceTab,
    scene_tool: SceneTool,
    /// Gizmo 的变换空间（世界/局部）：编辑器态，随布局持久化。
    gizmo_space: crate::scene_interaction::GizmoSpace,
    /// Gizmo 的轴心模式（轴心/中心）：编辑器态，随布局持久化。
    gizmo_pivot: crate::scene_interaction::GizmoPivot,
    scene_view_mode: SceneViewMode,
    scene_zoom: f32,
    scene_pan: egui::Vec2,
    /// Hierarchy 折叠状态：Play 期间用户仍可折叠/展开行，Stop 后恢复原状。
    hierarchy_collapsed: std::collections::BTreeSet<SceneEntityId>,
    preview_resolution: (u32, u32),
    game_camera_2d_center: Vec2,
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

/// 3D Scene 视图的 typed 提取缓存（WP1-4）。
///
/// 取代了每帧对场景字符串调用 `extract_serialized_scene_3d`：组件字符串只在文档
/// revision 变化时解析一次。用宽容编译，坏组件只丢自己而不会让整个视图空白。
#[derive(Clone, Debug, Default)]
pub(crate) struct SceneView3dTyped {
    revision: Option<u64>,
    /// 同时具备 Transform3D 与 MeshRenderer 的实体（与旧 serialized 提取同集合）。
    entities: Vec<SceneView3dTypedEntity>,
    /// 3D 相关组件的编译诊断文案。
    diagnostics: Vec<String>,
}

/// 3D 视图需要的单个实体数据（不含 GPU 资源，纯 CPU 投影用）。
#[derive(Clone, Debug)]
pub(crate) struct SceneView3dTypedEntity {
    id: SceneEntityId,
    name: String,
    transform: Transform3D,
    mesh_path: String,
}

#[derive(Default)]
struct SceneEditorState {
    pan: egui::Vec2,
    drag: Option<SceneDrag>,
    preview: std::collections::BTreeMap<SceneEntityId, TransformFields>,
    /// UI 组件拖拽预览（实体 id → UiLayout 四边 offset 预览值）。布局与视觉
    /// 组件分离，拖拽时单独预览，提交时写回 `UiLayout`。
    preview_ui: std::collections::BTreeMap<SceneEntityId, [f32; 4]>,
    /// 3D 实体变换预览（画布俯视 XZ 投影；M9-03/M10-07）。
    preview3d_transform: std::collections::BTreeMap<SceneEntityId, Transform3D>,
    /// 3D 平移/旋转/缩放 Gizmo 拖拽（M9-03/M10-07）。
    gizmo_drag: Option<GizmoDrag>,
    collapsed: std::collections::BTreeSet<SceneEntityId>,
    hidden: std::collections::BTreeSet<SceneEntityId>,
    locked: std::collections::BTreeSet<SceneEntityId>,
    tile_paint_active: bool,
    tile_paint_last_cell: Option<(usize, usize, bool)>,
    /// M19-03 矩形工具拖拽起点/当前格（TilePaint 的矩形填充与区域复制共用）。
    tile_rect_anchor: Option<(usize, usize)>,
    tile_rect_current: Option<(usize, usize)>,
}

struct SceneDrag {
    ids: Vec<SceneEntityId>,
    start: egui::Pos2,
    initial: std::collections::BTreeMap<SceneEntityId, TransformFields>,
    /// UI 组件拖拽初始 offsets。仅在选中实体带 UiLayout 视觉组件时填充；
    /// 拖拽预览与提交均走该路径。
    ui_initial: std::collections::BTreeMap<SceneEntityId, UiDragInitial>,
}

/// 一个 UI 组件拖拽的初始状态。组件 key 在提交时由实体重新推导
/// （见 `set_ui_component_position`）。
#[derive(Clone, Copy)]
struct UiDragInitial {
    /// `UiLayout` 四边 offset（authored 坐标）。UI 元素绘制在设计分辨率
    /// 包围框内，拖动增量换算为 zoom/ppp（见 draw_scene_view）。
    offsets: [f32; 4],
}

/// 3D 变换 Gizmo 拖拽：约束到哪个轴（Y 为垂直高度，Free 为 XZ 平面；
/// Scale 为等比缩放，Rotate 由 scene_tool 决定）。
struct GizmoDrag {
    id: SceneEntityId,
    axis: GizmoAxis,
    start: egui::Pos2,
    initial: Transform3D,
    /// 轴心模式 `Center` 时的**公转中心**（手柄起点）；`None` = 原地旋转/缩放。
    pivot: Option<engine::core::math3d::Vec3>,
    start_ray: Option<engine::core::camera::Ray3>,
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
        // 启动扫描失败（权限/IO）不能让编辑器假装"没有待恢复内容"：把原因写进状态栏，
        // 而不是用 `unwrap_or_default()` 静默吞掉。
        let (recovery_scan, recovery_scan_error) = match recovery.scan() {
            Ok(scan) => (scan, None),
            Err(error) => (RecoveryScan::default(), Some(error.to_string())),
        };
        Self {
            document: ProjectDocument::open(project.clone()),
            project,
            tree: SceneTreeState::new(),
            resources: ResourceBrowserState::new(),
            selected_resource: None,
            resource_rename: None,
            resources_filter: String::new(),
            lua_runtime: None,
            console_input: String::new(),
            console_output: Vec::new(),
            console_notices: Vec::new(),
            console_rows_cache: Default::default(),
            console_filter: String::new(),
            console_log_level_filter: ConsoleLogLevelFilter::Error,
            console_tab: ConsoleTab::Editor,
            inspector: InspectorState::new(),
            mesh_renderer: String::new(),
            collider3d: Collider3dFields::default(),
            container: editor::ContainerFields::default(),
            tilemap: TilemapEditorState::default(),
            animation: AnimationEditorState::default(),
            lua_script: String::new(),
            inspector_collider3d_open: false,
            inspector_tilemap_open: true,
            inspector_animation_open: true,
            inspector_add_component: "Transform3D".to_owned(),
            entity_name: "Entity".to_owned(),
            status: recovery_scan_error.map_or_else(
                || "尚未创建场景".to_owned(),
                |error| format!("恢复快照扫描失败：{error}"),
            ),
            play: editor::PlayController::new(),
            play_session: None,
            history: editor::CommandService::new(editor::CommandService::DEFAULT_MAX_ENTRIES),
            recovery,
            autosave: AutoSaveController::new(Duration::from_secs(10)),
            recovery_scan,
            prefab_source_cache: std::collections::HashMap::new(),
            project_loaded,
            import_path: String::new(),
            workspace_tab: WorkspaceTab::Scene,
            scene_zoom: 1.0,
            interaction: crate::scene_interaction::ToolStateMachine::new(),
            gizmo_space: crate::scene_interaction::GizmoSpace::default(),
            gizmo_pivot: crate::scene_interaction::GizmoPivot::default(),
            scene_view_mode: SceneViewMode::default(),
            show_scene_guides: false,
            show_safe_area: false,
            show_pixel_grid: false,
            show_performance_hud: false,
            snap_enabled: false,
            snap_step: 0.5,
            hierarchy_filter: String::new(),
            inspector_field_filter: String::new(),
            scene_state: SceneEditorState::default(),
            inspector_clipboard: None,
            progress: ProgressPanel::default(),
            show_publish_panel: false,
            publish_platform: crate::publish::PublishPlatform::Pc,
            publish_name,
            publish_compress: false,
            publish_encrypt: false,
            show_resources_panel: true,
            show_lua_console: true,
            show_domain_data_panel: false,
            domain_report: None,
            persist_registry: None,
            show_domain_overlay: true,
            show_runtime_debug_panel: false,
            runtime_probe_results: None,
            runtime_probe_error: None,
            persist_id_buffer: String::new(),
            persist_id_editing: None,
            log_sink,
            show_about: false,
            project_settings: Default::default(),
            last_window_title: None,
            recent_projects: crate::util::load_recent_projects(),
            gpu,
            render_state: None,
            preview: None,
            preview_resolution: (1280, 720),
            // E18：多分辨率预设（横屏/竖屏/超宽/小屏），覆盖常见设备比例。
            preview_resolutions: vec![
                (800, 600),
                (1280, 720),
                (1920, 1080),
                (2560, 1440),
                (3440, 1440),
                (1080, 1920),
            ],
            adding_preview_resolution: false,
            custom_preview_resolution: (1280, 720),
            game_camera_2d_center: DEFAULT_2D_CAMERA_CENTER,
            editor_camera: EditorCamera::default(),
            scene_renderer: None,
            scene_texture: None,
            scene_preview_tex: None,
            scene_runtime: None,
            scene_font_atlas: None,
            scene_vector_atlas: None,
            custom_font_atlases: engine::host::frame::FontAtlasMap::default(),
            custom_vector_atlases: engine::host::frame::VectorAtlasMap::default(),
            scene_2d_tex: None,
            scene_screen_texture: None,
            scene_screen_tex_id: None,
            scene_screen_cache_key: None,
            thumbnails: std::collections::HashMap::new(),
            thumbnail_worker: crate::panels::resource_browser::ThumbnailWorker::new(),
            resource_tree_cache: None,
            refresh_pending: false,
            resource_refresh_generation: 0,
            resource_refresh_job: None,
            file_watcher: None,
            file_watch_interval: std::time::Duration::from_millis(500),
            layout: editor::EditorLayout::default(),
            layout_load: editor::LayoutLoad::Missing,
            profiler: editor::EditorProfiler::new(editor::DEFAULT_FRAME_BUDGET_MS),
            hierarchy_visible_cache: Default::default(),
            hierarchy_lower: Default::default(),
            hierarchy_row_probe: Default::default(),
            console_row_probe: Default::default(),
            scene_view_cache_2d: SceneViewCache2d::default(),
            scene_view_cache_3d: SceneViewCache3d::default(),
            scene_view_3d_typed: SceneView3dTyped::default(),
            scene_runtime_revision: None,
            scene_files_cache: Vec::new(),
            scene_files_cache_time: None,
            preview_diagnostic: None,
            scene_view_repaint_pending: false,
            scene_view_fit_pending: true,
            delete_target: DeleteTarget::None,
            selection: SelectionModel::new(),
            edit_transaction: editor::EditTransactionState::default(),
            inspector_edit: crate::actions::EditState::default(),
            inspector_baseline: crate::actions::EditSnapshot::empty(),
            canvas_config: CanvasConfig::default(),
            events_editor: UiEvents::default(),
            bindings_editor: BindingSet::default(),
            button_states_editor: ButtonStateSet::default(),
            ui_animation_editor: UiAnimationClip::new("preview"),
            ui_animation_preview: None,
            ui_animation_playing: false,
            preview_diagnostics: DiagnosticCollector::default(),
            ui_layout_cache: UiLayoutCache::default(),
            plugin_registry: PluginRegistry::default(),
            plugin_status: Vec::new(),
            pending_document_change: None,
            pending_save_failed: false,
            last_metadata_key: None,
            preview_input_state: InputSnapshot::default(),
            hierarchy_clipboard: None,
            hierarchy_rename: None,
            selection_anchor: None,
            editor_context_snapshot: None,
            thumbnail_benchmark_ran: false,
            play_stop_benchmark_ran: false,
            benchmark_textures: Vec::new(),
        }
    }

    /// 编辑器是否处于 Play 模式（含启动中/停止中的过渡态）。
    ///
    /// 工具栏的 Play/Stop 按钮、实体编辑的拒绝判断都走这个口径：`Starting` 期间
    /// 快照已经隔离，必须显示"停止"而不是"开始"，否则双击会重复启动。
    /// 与 `PlayController::is_playing()`（是否真的持有运行时会话）刻意区分开。
    fn is_playing(&self) -> bool {
        self.play.state().is_in_play_mode()
    }

    /// 当前项目可用的字体资源（项目相对正斜杠路径，如 `assets/fonts/x.ttf`），
    /// 供 Text 组件的「字体」下拉使用。空项目/未加载项目 → 空列表。
    pub(crate) fn project_font_paths(&self) -> Vec<String> {
        use std::collections::BTreeSet;
        self.resources
            .entries()
            .iter()
            .filter(|entry| entry.kind == editor::ResourceKind::Font)
            .map(|entry| entry.path_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// 当前项目里**符合该字段扩展名白名单**的资源路径（供 Inspector 的"引用选择器"下拉）。
    ///
    /// 候选规则本身在 `editor::picker_candidates`（纯函数，可单测）：这里只负责把资源索引
    /// 的路径喂进去。索引为空 → 空列表（此时视图不画选择器，作者仍可手打路径）。
    pub(crate) fn picker_candidates_for(&self, extensions: &[&str], current: &str) -> Vec<String> {
        let paths: Vec<String> = self
            .resources
            .entries()
            .iter()
            .map(editor::ResourceEntry::path_string)
            .collect();
        editor::picker_candidates(&paths, extensions, current)
    }

    fn set_render_state(&mut self, render_state: Option<eframe::egui_wgpu::RenderState>) {
        self.render_state = render_state;
    }

    fn capture_editor_context(&self) -> EditorContextSnapshot {
        EditorContextSnapshot {
            selection: self.selection.clone(),
            workspace_tab: self.workspace_tab,
            scene_tool: self.scene_tool(),
            gizmo_space: self.gizmo_space,
            gizmo_pivot: self.gizmo_pivot,
            scene_view_mode: self.scene_view_mode,
            scene_zoom: self.scene_zoom,
            scene_pan: self.scene_state.pan,
            hierarchy_collapsed: self.scene_state.collapsed.clone(),
            preview_resolution: self.preview_resolution,
            game_camera_2d_center: self.game_camera_2d_center,
        }
    }

    fn restore_editor_context(&mut self, snapshot: EditorContextSnapshot) {
        self.selection = snapshot.selection;
        self.workspace_tab = snapshot.workspace_tab;
        self.set_scene_tool(snapshot.scene_tool);
        self.gizmo_space = snapshot.gizmo_space;
        self.gizmo_pivot = snapshot.gizmo_pivot;
        self.scene_view_mode = snapshot.scene_view_mode;
        self.scene_zoom = snapshot.scene_zoom;
        self.scene_state.pan = snapshot.scene_pan;
        self.scene_state.collapsed = snapshot.hierarchy_collapsed;
        self.preview_resolution = snapshot.preview_resolution;
        self.game_camera_2d_center = snapshot.game_camera_2d_center;
        // 快照里的 zoom/pan 已是权威值：不置 fit_pending，避免下一帧视图适配
        // 把 Play 前的视图状态覆盖回 fit 值。
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
            if !self.play_state().is_running() {
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

    /// 从项目读取持久化布局并应用到当前会话（docs/55 P1：可保存布局 + 损坏恢复）。
    ///
    /// 缺失/损坏都回落到默认布局；损坏时**不覆盖**源文件，并把原因写进状态栏，
    /// 用户可据此修复或显式「重置布局」。
    pub(crate) fn load_layout_for_project(&mut self) {
        let load = editor::load_layout(&crate::util::strip_verbatim_prefix(&self.project.root));
        if let Some(reason) = load.reason() {
            self.push_console_notice(format!("布局文件损坏，已回落到默认布局：{reason}"));
            self.status = format!("布局文件损坏，已用默认布局：{reason}");
        }
        self.layout = load.layout();
        self.layout_load = load;
        self.apply_layout();
    }

    /// 把布局字段映射到编辑器状态（面板可见性、工作台标签、视图模式/工具/缩放/分辨率）。
    fn apply_layout(&mut self) {
        self.show_resources_panel = self.layout.panel_is_visible(editor::PANEL_RESOURCE);
        self.show_lua_console = self.layout.panel_is_visible(editor::PANEL_CONSOLE);
        self.workspace_tab = match self.layout.workspace_tab.as_str() {
            "game" => WorkspaceTab::Game,
            _ => WorkspaceTab::Scene,
        };
        self.scene_view_mode = match self.layout.scene_view_mode.as_str() {
            "3d" => SceneViewMode::View3D,
            "prefab" => SceneViewMode::Prefab,
            "widget" => SceneViewMode::Widget,
            _ => SceneViewMode::View2D,
        };
        self.set_scene_tool(match self.layout.scene_tool.as_str() {
            "move" => SceneTool::Move,
            "rotate" => SceneTool::Rotate,
            "scale" => SceneTool::Scale,
            _ => SceneTool::Select,
        });
        self.scene_zoom = self.layout.scene_zoom;
        self.gizmo_space = crate::scene_interaction::GizmoSpace::parse(&self.layout.gizmo_space)
            .unwrap_or_default();
        self.gizmo_pivot = crate::scene_interaction::GizmoPivot::parse(&self.layout.gizmo_pivot)
            .unwrap_or_default();
        self.preview_resolution = self.layout.preview_resolution;
    }

    /// 把当前会话状态写回布局字段（保存前调用）。
    pub(crate) fn capture_layout(&mut self) {
        self.layout
            .panel_visible
            .insert(editor::PANEL_RESOURCE.to_owned(), self.show_resources_panel);
        self.layout
            .panel_visible
            .insert(editor::PANEL_CONSOLE.to_owned(), self.show_lua_console);
        self.layout.workspace_tab = match self.workspace_tab {
            WorkspaceTab::Scene => "scene",
            WorkspaceTab::Game => "game",
        }
        .to_owned();
        self.layout.scene_view_mode = match self.scene_view_mode {
            SceneViewMode::View2D => "2d",
            SceneViewMode::View3D => "3d",
            SceneViewMode::Prefab => "prefab",
            SceneViewMode::Widget => "widget",
        }
        .to_owned();
        self.layout.scene_tool = match self.scene_tool() {
            SceneTool::Select => "select",
            SceneTool::Move => "move",
            SceneTool::Rotate => "rotate",
            SceneTool::Scale => "scale",
            // TilePaint 是模式化工具（进入/离开由瓦片地图工具条驱动），不持久化。
            SceneTool::TilePaint => "select",
        }
        .to_owned();
        self.layout.scene_zoom = self.scene_zoom;
        self.layout.gizmo_space = self.gizmo_space.as_str().to_owned();
        self.layout.gizmo_pivot = self.gizmo_pivot.as_str().to_owned();
        self.layout.preview_resolution = self.preview_resolution;
        self.layout.clamp();
    }

    /// 面板可见性/工作台/视图模式变化后自动保存布局（无变化时不写盘）。
    fn maybe_persist_layout_on_change(&mut self) {
        if !self.project_loaded {
            return;
        }
        let mut current = self.layout.clone();
        let saved = current.clone();
        let visible_resource = current.panel_is_visible(editor::PANEL_RESOURCE);
        let visible_console = current.panel_is_visible(editor::PANEL_CONSOLE);
        current
            .panel_visible
            .insert(editor::PANEL_RESOURCE.to_owned(), self.show_resources_panel);
        current
            .panel_visible
            .insert(editor::PANEL_CONSOLE.to_owned(), self.show_lua_console);
        let tab = match self.workspace_tab {
            WorkspaceTab::Scene => "scene",
            WorkspaceTab::Game => "game",
        };
        current.workspace_tab = tab.to_owned();
        let view_mode = match self.scene_view_mode {
            SceneViewMode::View2D => "2d",
            SceneViewMode::View3D => "3d",
            SceneViewMode::Prefab => "prefab",
            SceneViewMode::Widget => "widget",
        };
        current.scene_view_mode = view_mode.to_owned();
        current.scene_tool = match self.scene_tool() {
            SceneTool::Select => "select",
            SceneTool::Move => "move",
            SceneTool::Rotate => "rotate",
            SceneTool::Scale => "scale",
            SceneTool::TilePaint => "select",
        }
        .to_owned();
        current.scene_zoom = self.scene_zoom;
        current.gizmo_space = self.gizmo_space.as_str().to_owned();
        current.gizmo_pivot = self.gizmo_pivot.as_str().to_owned();
        current.preview_resolution = self.preview_resolution;
        current.clamp();
        let changed = current.panel_is_visible(editor::PANEL_RESOURCE) != visible_resource
            || current.panel_is_visible(editor::PANEL_CONSOLE) != visible_console
            || current.workspace_tab != saved.workspace_tab
            || current.scene_view_mode != saved.scene_view_mode
            || current.scene_tool != saved.scene_tool
            || current.gizmo_space != saved.gizmo_space
            || current.gizmo_pivot != saved.gizmo_pivot
            || (current.scene_zoom - saved.scene_zoom).abs() > f32::EPSILON
            || current.preview_resolution != saved.preview_resolution;
        self.layout = current;
        if !changed {
            return;
        }
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        match editor::save_layout(&root, &self.layout) {
            Ok(()) => self.profiler.bump("layout_auto_saved"),
            Err(error) => {
                self.profiler.bump("layout_save_failed");
                tracing::warn!(target: "editor", "[layout] auto-save failed: {error}");
            }
        }
    }

    /// 保存布局（原子写；失败只提示，不中断编辑）。
    pub(crate) fn persist_layout(&mut self) {
        if !self.project_loaded {
            return;
        }
        self.capture_layout();
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        match editor::save_layout(&root, &self.layout) {
            Ok(()) => {
                self.profiler.bump("layout_saved");
                self.status = "已保存布局".to_owned();
            }
            Err(error) => {
                self.profiler.bump("layout_save_failed");
                self.status = format!("保存布局失败：{error}");
            }
        }
    }

    /// 重置为默认布局并落盘（布局重置）。
    pub(crate) fn reset_layout_to_default(&mut self) {
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        match editor::reset_layout(&root) {
            Ok(layout) => {
                self.layout = layout;
                self.layout_load = editor::LayoutLoad::Loaded(self.layout.clone());
                self.apply_layout();
                self.profiler.bump("layout_reset");
                self.status = "已重置为默认布局".to_owned();
            }
            Err(error) => self.status = format!("重置布局失败：{error}"),
        }
    }

    /// 帧末刷新 profiler gauge：历史深度、日志/输出行数、资源条目数与后台队列长度。
    fn update_profiler_gauges(&mut self) {
        if !self.profiler.enabled() {
            return;
        }
        let undo_len = self.history.undo_len() as f64;
        let redo_len = self.history.redo_len() as f64;
        let dropped = self.history.dropped_entries() as f64;
        let log_lines = self.log_sink.snapshot().len() as f64;
        let console_lines = self.console_output.len() as f64;
        let notices = self.console_notices.len() as f64;
        let console_rows = self.console_rows_cache.len() as f64;
        let resource_entries = self.resources.entries().len() as f64;
        let pending_imports = self.resources.pending_imports().len() as f64;
        let refresh_in_flight = f64::from(self.resource_refresh_job.is_some());
        let scene_entities = self
            .document
            .current_scene
            .as_ref()
            .map_or(0.0, |document| document.scene.entities.len() as f64);
        let visible_entities = self.hierarchy_visible_cache.len() as f64;
        let profiler = &mut self.profiler;
        profiler.set_gauge("history_undo_len", undo_len);
        profiler.set_gauge("history_redo_len", redo_len);
        profiler.set_gauge("history_dropped_entries", dropped);
        profiler.set_gauge("log_sink_lines", log_lines);
        profiler.set_gauge("console_output_lines", console_lines);
        profiler.set_gauge("console_notice_lines", notices);
        profiler.set_gauge("console_rows_indexed", console_rows);
        profiler.set_gauge("resource_entries", resource_entries);
        profiler.set_gauge("resource_pending_imports", pending_imports);
        profiler.set_gauge("resource_refresh_in_flight", refresh_in_flight);
        profiler.set_gauge("scene_entities", scene_entities);
        profiler.set_gauge("hierarchy_visible_rows", visible_entities);
        profiler.set_gauge(
            "hierarchy_row_content_height",
            f64::from(self.hierarchy_row_probe.max_content_height()),
        );
        profiler.set_gauge(
            "console_row_content_height",
            f64::from(self.console_row_probe.max_content_height()),
        );
        profiler.set_gauge("thumbnail_cache_entries", self.thumbnails.len() as f64);
    }

    fn push_console_output(&mut self, line: impl Into<String>) {
        const MAX_CONSOLE_LINES: usize = 10_000;
        self.console_output.push(line.into());
        if self.console_output.len() > MAX_CONSOLE_LINES {
            let overflow = self.console_output.len() - MAX_CONSOLE_LINES;
            self.console_output.drain(..overflow);
        }
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
        // Editor Profiler（docs/55 P0）：帧起点 + 本帧各面板/命令耗时。
        let frame_started = Instant::now();
        self.profiler.begin_frame(frame_started);
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
        // M19-03：领域数据在首次需要时收集一次（Scene 视图覆盖层默认开启、或打开的
        // 领域数据面板）；之后的刷新走面板「重新校验」按钮（数据文件可能已改）。
        if self.project_loaded
            && self.domain_report.is_none()
            && (self.show_domain_overlay || self.show_domain_data_panel)
        {
            self.refresh_domain_report();
        }
        // E26: advance the isolated animation preview even while the Inspector
        // is visible; playback time never writes back to the scene document.
        let delta_seconds = context.input(|input| input.stable_dt).max(0.0);
        self.advance_ui_animation_preview(delta_seconds);
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
        // M20-08：输入重绑定捕获。必须在所有编辑器快捷键之前运行——它消费本帧的
        // 键盘事件（见 `EditorApp::capture_input_binding`），使捕获不会顺手触发
        // Ctrl+Z / Ctrl+S / Delete / Q/W/E/R 等命令。
        self.capture_input_binding(context);
        // 快捷键统一走 typed 命令入口（docs/55 §4.1 规则 1：视图只发 action，
        // 提交边界由 dispatch 统一编排）。Ctrl+Z 仍先做事务网关。
        if self.project_loaded
            && context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z))
        {
            self.dispatch(editor::EditorCommand::Undo);
        }
        if self.project_loaded
            && context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Y))
        {
            self.dispatch(editor::EditorCommand::Redo);
        }
        if self.project_loaded
            && context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S))
        {
            self.dispatch(editor::EditorCommand::Save);
        }
        // Ctrl+Shift+L：保存布局（docs/55 P1：可保存布局；重置在菜单里）。
        if self.project_loaded
            && context
                .input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::L))
        {
            self.persist_layout();
        }
        // H04：Ctrl+D 原地复制 / Ctrl+C 复制 / Ctrl+V 粘贴为子节点（Unity 语义）。
        // 与 Ctrl+S 同级：仅场景工作台、无文本焦点、无活动事务时生效；Play 锁
        // 由各命令入口自查。
        if self.project_loaded
            && self.workspace_tab == WorkspaceTab::Scene
            && !context.wants_keyboard_input()
            && !self.is_edit_transaction_active()
        {
            if context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::D)) {
                self.dispatch(editor::EditorCommand::DuplicateSelection);
            } else if context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::C)) {
                self.copy_selected_entity();
            } else if context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::V)) {
                let parent = self.selection.primary();
                self.paste_entity(parent);
            }
        }
        if self.project_loaded
            && !context.wants_keyboard_input()
            && context.input(|i| i.key_pressed(egui::Key::Delete))
        {
            self.handle_delete_shortcut();
        }
        if self.workspace_tab == WorkspaceTab::Scene && !context.wants_keyboard_input() {
            let mut requested = None;
            context.input(|input| {
                if input.key_pressed(egui::Key::Q) {
                    requested = Some(SceneTool::Select);
                } else if input.key_pressed(egui::Key::W) {
                    requested = Some(SceneTool::Move);
                } else if input.key_pressed(egui::Key::E) {
                    requested = Some(SceneTool::Rotate);
                } else if input.key_pressed(egui::Key::R) {
                    requested = Some(SceneTool::Scale);
                }
            });
            // 走状态机：切换工具会自动取消进行中的拖拽/框选（E11 既有语义）。
            if let Some(tool) = requested {
                self.set_scene_tool(tool);
            }
        }
        // E08：Escape 取消活动事务（拖动/Inspector 编辑的取消语义，见
        // cancel_edit_on_escape）。置于 Delete 之后，非键入状态生效。确实执行了
        // 取消时，顺带清掉 Scene View 拖拽残留（drag/gizmo_drag/preview），防止
        // 回滚后的 stale preview 在下一帧被重新写回文档。
        if self.cancel_edit_on_escape(context) {
            self.clear_active_scene_drag();
        }
        self.sync_hierarchy_metadata();
        self.poll_progress();
        self.poll_resource_refresh();
        // 外部文件变化（保存/生成器/其它工具）：轮询监听 → 触发后台资源刷新。
        self.poll_file_watcher();
        // 待导入资源非空且无进行中任务时，启动后台资源导入（不阻塞 UI）。
        self.start_resource_import();
        // 行1 标题栏 + 行2 菜单栏（含崩溃恢复「⚠ 恢复」下拉）。
        self.draw_header_panel(context);

        // 行4 控制台：先占底部以获得全宽。
        if self.show_lua_console {
            let started = Instant::now();
            self.draw_lua_console_panel(context);
            self.profiler.record_panel("console", started.elapsed());
        }

        // 行3 资源面板（最左侧）。
        if self.show_resources_panel {
            let started = Instant::now();
            let refresh = self.draw_resource_browser_panel(context);
            self.profiler
                .record_panel("resource_browser", started.elapsed());
            if refresh {
                // 用户显式刷新：清空"已尝试"记录，让上次没能导入成功的资源重新排队
                // （自动路径不会清，见 `ResourceBrowserState::import_attempted`）。
                self.resources.clear_import_attempts();
                self.refresh_resources();
            }
        }
        // 行3 层级面板（资源面板右侧）。
        {
            let started = Instant::now();
            self.draw_hierarchy_panel(context);
            self.profiler.record_panel("hierarchy", started.elapsed());
        }

        // 行3 属性面板（右侧，先于中央绘制，确保 Scene 视图 rect 不延伸到其下方，
        // 避免在属性面板操作鼠标时误触发 Scene 视图的缩放/平移）。
        {
            let started = Instant::now();
            egui::SidePanel::right("inspector")
                .resizable(true)
                .default_width(self.layout.panel_width(editor::PANEL_INSPECTOR))
                .min_width(260.0)
                .show(context, |ui| {
                    self.draw_inspector_panel(ui);
                });
            self.profiler.record_panel("inspector", started.elapsed());
        }

        // 行3 Scene+Game（中央）+ 工具栏。
        {
            let started = Instant::now();
            egui::CentralPanel::default().show(context, |ui| {
                self.draw_workspace_toolbar(ui);
                ui.separator();
                match self.workspace_tab {
                    WorkspaceTab::Scene => self.draw_scene_view(ui),
                    WorkspaceTab::Game => self.draw_game_view(ui),
                }
            });
            self.profiler.record_panel("workspace", started.elapsed());
        }

        self.progress.window(context);
        self.draw_publish_panel(context);
        self.draw_project_settings_panel(context);
        // M19-03：领域数据面板（Lua 校验 / PersistId 注册表）与运行时调试面板
        // （只读探针）都是可开关的独立窗口。
        self.draw_domain_data_panel(context);
        self.draw_runtime_debug_panel(context);
        self.draw_about_window(context);
        // E06：最后绘制「未保存的更改」确认窗，置于其余窗口之上；仅在 dirty 且
        // 有挂起的离开操作时显示，此时其它面板保持原样、文档不被改动。
        self.draw_pending_change_window(context);
        // E08：帧末统一提交边界 —— 本帧任意 release/Enter 时收敛 Inspector 挂起
        // 缓冲为一条历史（Inspector 面板与 Scene View 各自的释放提交已先执行，
        // 这里只兜底「释放发生在面板外 / 文本 Enter」等漏网情形；无挂起即 no-op）。
        self.flush_frame_edit_boundary(context);
        // 面板可见性/工作台/视图变化后自动落盘布局（一次小文件原子写，仅在真正变化时发生）。
        self.maybe_persist_layout_on_change();
        // 帧末汇总：帧耗时 + 会话级 gauge（历史深度/日志行数/资源条目/队列）。
        self.profiler.end_frame(Instant::now());
        self.update_profiler_gauges();
    }
}

#[cfg(test)]
mod layout_tests {
    use crate::{EditorApp, SceneTool, SceneViewMode, WorkspaceTab};
    use eframe::egui;
    use engine::core::logging::LogSink;

    /// 布局持久化的**接线**测试（不只是模块单测）：保存 → 重启 → 恢复同一状态；
    /// 损坏文件回落默认且不改动源文件；重置写回默认。
    #[test]
    fn layout_round_trips_through_the_editor_shell() {
        let root = std::env::temp_dir().join(format!(
            "p2-editor-layout-shell-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");

        let project = || editor::ProjectInfo {
            root: root.clone(),
            version: "test".to_owned(),
            directories: Vec::new(),
        };
        let mut app = EditorApp::new(project(), None, LogSink::new(64));
        app.project_loaded = true;
        // 改一批布局字段并保存。
        app.show_lua_console = false;
        app.workspace_tab = WorkspaceTab::Game;
        app.scene_view_mode = SceneViewMode::View3D;
        app.set_scene_tool(SceneTool::Rotate);
        app.scene_zoom = 3.25;
        app.preview_resolution = (1920, 1080);
        app.layout
            .panel_widths
            .insert(editor::PANEL_INSPECTOR.to_owned(), 420.0);
        app.persist_layout();
        assert!(
            editor::layout_path(&root).exists(),
            "保存布局必须落盘：{}",
            editor::layout_path(&root).display()
        );

        // 「重启」：新 App 读取同一项目。
        let mut reloaded = EditorApp::new(project(), None, LogSink::new(64));
        reloaded.project_loaded = true;
        reloaded.load_layout_for_project();
        assert!(!reloaded.show_lua_console, "面板可见性必须恢复");
        assert_eq!(reloaded.workspace_tab, WorkspaceTab::Game);
        assert_eq!(reloaded.scene_view_mode, SceneViewMode::View3D);
        assert_eq!(reloaded.scene_tool(), SceneTool::Rotate);
        assert!((reloaded.scene_zoom - 3.25).abs() < 1e-6);
        assert_eq!(reloaded.preview_resolution, (1920, 1080));
        assert_eq!(reloaded.layout.panel_width(editor::PANEL_INSPECTOR), 420.0);

        // 损坏恢复：写入垃圾后加载必须回落默认，且**不覆盖**源文件。
        let garbage = "{ this is not json";
        std::fs::write(editor::layout_path(&root), garbage).expect("write garbage");
        let mut broken = EditorApp::new(project(), None, LogSink::new(64));
        broken.project_loaded = true;
        broken.load_layout_for_project();
        assert!(broken.show_lua_console, "损坏时必须回落默认布局");
        assert_eq!(broken.workspace_tab, WorkspaceTab::Scene);
        assert!(
            broken.status.contains("损坏"),
            "状态栏应说明原因：{}",
            broken.status
        );
        assert_eq!(
            std::fs::read_to_string(editor::layout_path(&root)).expect("read"),
            garbage,
            "损坏文件不得被静默覆盖"
        );

        // 重置：写回默认布局。
        broken.reset_layout_to_default();
        assert_eq!(
            editor::load_layout(&root).layout(),
            editor::EditorLayout::default()
        );
        let _ = std::fs::remove_dir_all(root);
    }

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

/// docs/54 §8-⑧：迁移命令的**报告 / 退出码门 / 弃用告警 / 失败路径**。
///
/// 这些是不需要 egui 的纯数据层测试，因此单独一个模块：命令的输出就是质量门要断言的契约
/// （`spatial_migration_ok` / `refused=` / `removed=`），而不是"跑一下看看"。
#[cfg(test)]
mod migration_tests {
    use super::*;
    use engine::core::scene::Scene;

    fn temp_project(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "p2-editor-migration-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(dir.join("scenes")).expect("create project");
        dir
    }

    /// 父缩放 + 子按世界坐标写：迁移应当把子转成局部（`converted=1`）。
    /// 另外塞一个带旧内联几何的 `Text`：迁移报告必须点名它（docs/30 的弃用告警）。
    fn project_scene(with_legacy_text: bool) -> Scene {
        let mut scene = Scene::new("hierarchy");
        let parent = SceneEntityId(1);
        let child = SceneEntityId(2);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        scene.entities.get_mut(&parent).unwrap().components.insert(
            "Transform".into(),
            "x=10;y=0;rotation=0;scale_x=2;scale_y=2".into(),
        );
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform".into(),
            "x=3;y=0;rotation=0;scale_x=1;scale_y=1".into(),
        );
        if with_legacy_text {
            let hud = SceneEntityId(3);
            scene.add_entity(hud, "Hud", None).unwrap();
            scene.entities.get_mut(&hud).unwrap().components.insert(
                "Text".into(),
                "text=hi;font_size=12;color=1,1,1;x=8;y=8".into(),
            );
        }
        scene
    }

    #[test]
    fn dry_run_report_is_lossless_and_names_removed_designs() {
        let root = temp_project("dry-run");
        project_scene(true)
            .save(root.join("scenes/main.scene"))
            .expect("save scene");
        let (report, blocked) = run_spatial_migration(&root, false, false);
        assert!(!blocked, "没有拒绝时不得阻断：{report}");
        assert!(report.contains("spatial_migration_ok"), "{report}");
        assert!(report.contains("converted=1"), "{report}");
        assert!(report.contains("refused=0"), "{report}");
        assert!(report.contains("removed=2"), "两条旧内联几何：{report}");
        assert!(
            report.contains("removed\tscenes/main.scene\tentity=3\tText.x\t→ UiLayout"),
            "弃用告警必须点名组件、字段与去处：{report}"
        );
        assert!(report.contains("mode=dry-run"), "{report}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_mode_reports_verified_backups_and_is_idempotent() {
        let root = temp_project("write");
        let scene_path = root.join("scenes/main.scene");
        project_scene(false).save(&scene_path).expect("save scene");
        let (report, blocked) = run_spatial_migration(&root, true, false);
        assert!(!blocked, "{report}");
        assert!(report.contains("written\tscenes/main.scene"), "{report}");
        assert!(report.contains("verified=true"), "{report}");
        assert!(report.contains("mode=write"), "{report}");
        assert!(scene_path.with_extension("scene.bak").is_file());
        // 幂等标记在场景自身里（与组件数据同一次原子写）。
        let written = std::fs::read_to_string(&scene_path).expect("read written scene");
        assert!(written.contains("meta\tspatial\tlocal"), "{written}");

        let (again, blocked) = run_spatial_migration(&root, true, false);
        assert!(!blocked, "{again}");
        assert!(again.contains("already-migrated"), "{again}");
        assert!(again.contains("already_migrated=1"), "{again}");
        assert_eq!(
            std::fs::read_to_string(&scene_path).expect("read again"),
            written,
            "重复运行不得改动文件"
        );

        // 回滚：内容与标记一起回到原样。
        let (restore, blocked) = run_spatial_migration(&root, false, true);
        assert!(!blocked, "{restore}");
        assert!(restore.contains("restored\tscenes/main.scene"), "{restore}");
        assert!(restore.contains("mode=restore"), "{restore}");
        assert!(
            !std::fs::read_to_string(&scene_path)
                .expect("read restored")
                .contains("meta\tspatial\tlocal")
        );
        assert!(!scene_path.with_extension("scene.bak").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 有实体无法无损迁移时：报告行变成 `spatial_migration_blocked` 且调用方拿到 `blocked = true`
    /// （`main` 据此 `exit(2)`，让脚本能把它当质量门）。报告**仍然完整**，不是静默失败。
    #[test]
    fn refused_entities_block_the_gate_and_are_reported() {
        let root = temp_project("refused");
        let mut scene = Scene::new("shear");
        let parent = SceneEntityId(1);
        let child = SceneEntityId(2);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        scene.entities.get_mut(&parent).unwrap().components.insert(
            "Transform".into(),
            "x=0;y=0;rotation=0;scale_x=2;scale_y=1".into(),
        );
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform".into(),
            "x=4;y=0;rotation=45;scale_x=1;scale_y=1".into(),
        );
        scene
            .save(root.join("scenes/shear.scene"))
            .expect("save scene");
        let (report, blocked) = run_spatial_migration(&root, false, false);
        assert!(blocked, "被拒实体必须阻断：{report}");
        assert!(report.contains("spatial_migration_blocked"), "{report}");
        assert!(report.contains("refused=1"), "{report}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 报告 artifact：给定路径就落盘（含自动建目录），内容与终端输出逐字节一致。
    #[test]
    fn migration_report_is_written_to_the_requested_path() {
        let root = temp_project("report");
        project_scene(false)
            .save(root.join("scenes/main.scene"))
            .expect("save scene");
        let (report, _) = run_spatial_migration(&root, false, false);
        let report_path = root.join("artifacts/baselines/spatial-migration.txt");
        write_migration_report(&report_path, &report).expect("write report");
        assert_eq!(
            std::fs::read_to_string(&report_path).expect("read report"),
            report,
            "落盘的报告必须与终端输出一致（否则归档的证据不是判定用的那一份）"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 空工程也不能静默通过：没有场景可处理时报 `blocked`，否则质量门会把"扫了个寂寞"当成功。
    #[test]
    fn empty_project_is_blocked() {
        let root = temp_project("empty");
        let (report, blocked) = run_spatial_migration(&root, false, false);
        assert!(blocked, "没有场景必须阻断：{report}");
        assert!(report.contains("没有找到"), "{report}");
        let _ = std::fs::remove_dir_all(&root);
    }
}