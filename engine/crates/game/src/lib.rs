mod boot_splash;
mod bundle_boot;
mod cli;
mod game_app;
mod memory;
mod splash;
mod window_fit;

use std::path::{Path, PathBuf};

use rustlua_infra::logging::init_logging;
use winit::event_loop::EventLoop;

use cli::{find_project_root, parse_arguments, resolve_window_title, verify_project};
use game_app::GameApp;

pub fn run_desktop() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = parse_arguments().map_err(|error| format!("argument error: {error}"))?;
    let project = arguments.project.or_else(find_project_root);

    // 加载集观测（`docs/67` §5）：必须在任何资源读取之前开始——`GameApp::new` 会读
    // project.toml / 场景 / 脚本，晚一步就会漏掉 boot 上下文。
    if let (Some(root), Some(_)) = (project.as_deref(), arguments.trace_loads.as_deref()) {
        rustlua_infra::vfs::start_load_trace(root);
    }

    // 发布包形态（RLB 容器）在这里挂载：**必须早于任何资源读取**。
    // 目录形态（没有 `bundles/`）走这里什么都不发生，行为与批 B 之前逐字节一致。
    let mount = match project.as_deref() {
        Some(root) => bundle_boot::mount_pack_bundles(root)
            .map_err(|error| format!("挂载资源包失败: {error}"))?,
        None => None,
    };
    if let Some(report) = &mount {
        // `--check` 也要看得见：stdout 是发布脚本解析的那条通道。
        println!("game_bundles_mounted {}", report.summary());
    }

    // 仅校验模式：不初始化日志/窗口，输出结果后直接退出。
    if arguments.check {
        let Some(root) = &project else {
            eprintln!("game_check_failed: 未找到游戏项目");
            std::process::exit(1);
        };
        let mut missing = verify_project(root);
        // 容器形态下再补两条：计划里缺失的容器（整组没发布出去）、以及逐条目全量校验
        // （只有它能把"某个块坏了"抓出来）。
        for gap in bundle_boot::missing_bundles() {
            missing.push(format!("资源包缺失（bundle_plan 里声明过）：{gap}"));
        }
        if let Err(error) = bundle_boot::verify_mounted() {
            missing.push(error);
        }
        if missing.is_empty() {
            println!("game_check_ok project={}", root.display());
            // M81 批 3：发布后的 API 面探针——运行期 `_G` 才是事实源（feature 边漏改时
            // 构建照样绿，只有这里能抓住"2D 包里混进 3D 表"）。打包脚本按 api 维度断言行内容。
            let runtime = rustlua_scripting::script::LuaRuntime::with_installers(
                rustlua_gameplay::input_lua::registry(),
            )
            .map_err(|error| format!("API 面探针 VM 构建失败: {error}"))?;
            let surface = runtime
                .eval_json(rustlua_scripting::script::LUA_API_PROBE)
                .map_err(|error| format!("API 面探针导出失败: {error}"))?;
            let tables = surface["tables"].as_array().expect("tables 应为数组");
            let funcs: usize = tables
                .iter()
                .map(|table| table["funcs"].as_array().expect("funcs").len())
                .sum();
            println!("lua_api_surface tables={} funcs={funcs}", tables.len());
            // P3：把解析出的玩家数据位置打出来，发布脚本据此断言"发布包的存档/设置
            // 落在用户数据目录，而不是程序目录"（真实二进制、真实命令行参数）。
            let roots = rustlua_infra::user_dir::data_roots(root, arguments.pack);
            println!(
                "player_data_roots packaged={} save_dir={} settings_dir={}",
                roots.packaged,
                roots.save_dir.display(),
                roots.settings_dir.display()
            );
            // `--check` 也能产出 boot 上下文的加载集（不需要开窗口）。
            finish_load_trace(arguments.trace_loads.as_deref());
            return Ok(());
        }
        eprintln!("game_check_failed: {}", missing.join("; "));
        std::process::exit(1);
    }

    // Resolve the log path relative to the executable so it works regardless of
    // the working directory (e.g. when double-clicking the exe).
    //
    // P3：发布包形态下日志与崩溃报告改写到**用户数据目录**（`<user>/logs/`）。
    // 安装目录（`C:\Program Files\...`）对普通用户不可写，写在那里会静默丢日志，
    // 而且会在发布目录里留下运行期残留（`jhyt_m31_pc_release` ① 节据此断言"发布目录
    // 不得有 .log/.sav/.cache 残留"，所以这条不是洁癖而是被验收锁住的行为）。
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let log_path = match project
        .as_deref()
        .map(|root| rustlua_infra::user_dir::data_roots(root, arguments.pack))
    {
        Some(roots) if roots.packaged => roots.log_dir().join("game.log"),
        _ => [
            exe_dir.join("../../../artifacts/logs/game.log"),
            exe_dir.join("../artifacts/logs/game.log"),
            PathBuf::from("artifacts/logs/game.log"),
        ]
        .into_iter()
        .find(|path| path.parent().is_some_and(|parent| parent.is_dir()))
        .unwrap_or_else(|| PathBuf::from("artifacts/logs/game.log")),
    };
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    init_logging(&log_path, arguments.log_filter.as_deref())
        .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
    rustlua_infra::logging::install_crash_handler(log_path.with_file_name("crash.log"));
    tracing::info!(target: "game", version = rustlua_infra::version(), project = ?project, "game runner starting");

    // Default and project startup images both precede script/domain validation.
    // GameApp only initializes the runtime after a successful splash present.
    let deferred_verify = project.is_some();

    let event_loop = EventLoop::new()?;
    let window_title = resolve_window_title(project.as_deref(), arguments.title);
    // P3：`--pack` 启动的发布包把存档与玩家设置写到用户数据目录
    // （`rustlua_infra::user_dir`），不写程序目录。
    let mut app = GameApp::new(
        project,
        window_title,
        !arguments.no_cull,
        !arguments.no_instance,
        arguments.pack,
        deferred_verify,
        arguments.frame_profile.clone(),
    )
    .with_shader_dir(arguments.shader_dir.clone())
    .with_skip_mesh(arguments.skip_mesh.clone())
    .with_depth_sort(!arguments.no_depth_sort)
    .with_bake_cache(!arguments.no_bake_cache)
    .with_planar_cull(!arguments.no_planar_cull)
    .with_point_face_cull(!arguments.no_point_face_cull)
    .with_planar_divisor(arguments.planar_divisor)
    .with_water_cheap(arguments.water_cheap)
    .with_terrain_cheap(arguments.terrain_cheap)
    .with_crowd_cheap(arguments.crowd_cheap)
    .with_draw_distance(arguments.draw_distance)
    .with_draw_distance_filter(arguments.draw_distance_filter.clone())
    .with_shadow_caster_skip(arguments.shadow_caster_skip.clone())
    .with_point_shadow_caster_skip(arguments.point_shadow_caster_skip.clone())
    .with_shadow_caster_distance(arguments.shadow_caster_distance)
    .with_water_refraction(arguments.water_refraction)
    .with_underwater_view(arguments.underwater_view)
    .with_water_refract_steps(arguments.water_refract_steps)
    .with_prewarm_particles(arguments.prewarm_particles)
    .with_gpu_motion(arguments.gpu_motion)
    .with_gpu_motion_cull(arguments.gpu_motion_cull)
    .with_gpu_motion_frustum_cull(arguments.gpu_motion_frustum_cull)
    .with_gpu_motion_thin(arguments.gpu_motion_thin)
    .with_gpu_motion_thin_far(arguments.gpu_motion_thin_far)
    .with_clip_cap(arguments.clip_cap)
    .with_clip_cap_both_sides(arguments.clip_cap_both_sides)
    .with_shadow_resolution(arguments.shadow_resolution)
    .with_shadow_cascades(arguments.shadow_cascades)
    .with_point_shadow_resolution(arguments.point_shadow_resolution)
    .with_point_shadow_lights(arguments.point_shadow_lights)
    .with_quality_low(arguments.quality == cli::Quality::Low)
    .with_quality_medium(arguments.quality == cli::Quality::Medium)
    .with_fixed_step(arguments.fixed_step)
    .with_freeze_clock(arguments.freeze_clock);
    event_loop.run_app(&mut app)?;
    // M74-02 P0：退出时落盘帧调度分档（未开 `--frame-profile` 时是 no-op）。
    app.finish_frame_profile();
    finish_load_trace(arguments.trace_loads.as_deref());
    run_result(app.failure())
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    if let Err(error) = run_android(app) {
        eprintln!("RustLua Android startup failed: {error}");
    }
}

#[cfg(target_os = "android")]
fn run_android(
    app: winit::platform::android::activity::AndroidApp,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    use winit::platform::android::EventLoopBuilderExtAndroid;

    let data_dir = app
        .internal_data_path()
        .ok_or_else(|| std::io::Error::other("Android app has no internal data directory"))?;
    let project_root = extract_android_pack(&app, &data_dir)?;
    let user_dir = data_dir.join("user");
    std::fs::create_dir_all(&user_dir)?;
    // This is set before any worker thread or runtime subsystem is created.
    unsafe {
        std::env::set_var(rustlua_infra::user_dir::USER_DIR_ENV, &user_dir);
    }

    bundle_boot::mount_pack_bundles(&project_root).map_err(std::io::Error::other)?;
    let log_dir = user_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("game.log");
    init_logging(&log_path, None)?;
    rustlua_infra::logging::install_crash_handler(log_dir.join("crash.log"));

    let title = resolve_window_title(Some(&project_root), None);
    let mut game = GameApp::new(Some(project_root), title, true, true, true, true, None);
    let mut builder = EventLoop::builder();
    builder.with_android_app(app);
    let event_loop = builder.build()?;
    event_loop.run_app(&mut game)?;
    game.finish_frame_profile();
    run_result(game.failure())
}

#[cfg(target_os = "android")]
fn extract_android_pack(
    app: &winit::platform::android::activity::AndroidApp,
    data_dir: &std::path::Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use std::{
        ffi::CString,
        io::{Read, Write},
    };

    const INDEX_ASSET: &str = "rustlua-asset-index.txt";
    const VERSION_FILE: &str = ".rustlua-asset-version";

    let assets = app.asset_manager();
    let index_name = CString::new(INDEX_ASSET)?;
    let mut index_asset = assets
        .open(index_name.as_c_str())
        .ok_or_else(|| std::io::Error::other("APK is missing rustlua-asset-index.txt"))?;
    let mut index = String::new();
    index_asset.read_to_string(&mut index)?;
    let mut lines = index.lines();
    let version = lines
        .next()
        .and_then(|line| line.strip_prefix("version="))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| std::io::Error::other("invalid Android asset index header"))?;

    let project_root = data_dir.join("pack");
    let version_path = project_root.join(VERSION_FILE);
    if std::fs::read_to_string(&version_path).is_ok_and(|installed| installed.trim() == version)
        && project_root.join("project.toml").is_file()
    {
        return Ok(project_root);
    }

    let staging = data_dir.join("pack.installing");
    let backup = data_dir.join("pack.previous");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    if backup.exists() {
        std::fs::remove_dir_all(&backup)?;
    }
    std::fs::create_dir_all(&staging)?;

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        for relative in lines.filter(|line| !line.is_empty()) {
            let relative_path = PathBuf::from(relative);
            if relative_path.is_absolute()
                || relative_path.components().any(|component| {
                    !matches!(
                        component,
                        std::path::Component::Normal(_) | std::path::Component::CurDir
                    )
                })
                || relative_path.components().next()
                    != Some(std::path::Component::Normal(std::ffi::OsStr::new("pack")))
            {
                return Err(std::io::Error::other(format!(
                    "invalid path in Android asset index: {relative}"
                ))
                .into());
            }

            let asset_name = CString::new(relative)?;
            let mut source = assets.open(asset_name.as_c_str()).ok_or_else(|| {
                std::io::Error::other(format!("APK asset is missing: {relative}"))
            })?;
            let destination = staging.join(&relative_path);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut target = std::fs::File::create(destination)?;
            std::io::copy(&mut source, &mut target)?;
            target.flush()?;
        }

        let extracted_root = staging.join("pack");
        if !extracted_root.join("project.toml").is_file() {
            return Err(std::io::Error::other("APK pack has no project.toml").into());
        }
        std::fs::write(extracted_root.join(VERSION_FILE), version)?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    let extracted_root = staging.join("pack");
    if project_root.exists() {
        std::fs::rename(&project_root, &backup)?;
    }
    if let Err(error) = std::fs::rename(&extracted_root, &project_root) {
        if backup.exists() {
            std::fs::rename(&backup, &project_root)?;
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error.into());
    }
    std::fs::remove_dir_all(staging)?;
    if backup.exists() {
        std::fs::remove_dir_all(backup)?;
    }
    Ok(project_root)
}

/// 结束加载集采集并写文件（没开 `--trace-loads` 时什么都不做）。
///
/// 输出行 `game_load_trace_ok path=… entries=… contexts=…` 是给脚本/面板解析的契约：
/// 条目数与上下文数至少能证明"这次真的采到了东西"，不是写了一个空壳。
fn finish_load_trace(path: Option<&Path>) {
    let Some(path) = path else {
        return;
    };
    let Some(trace) = rustlua_infra::vfs::stop_load_trace() else {
        eprintln!("game_load_trace_skipped: 没有在采集（缺 --trace-loads 或工程未加载）");
        return;
    };
    let contexts = trace.by_context().len();
    match trace.write(path) {
        Ok(()) => println!(
            "game_load_trace_ok path={} entries={} contexts={}",
            path.display(),
            trace.entries.len(),
            contexts
        ),
        Err(error) => eprintln!("game_load_trace_failed: {error}"),
    }
}

/// 运行结束后的退出判定。`None` = 正常退出（用户关窗、Lua `app.quit()`），
/// `Some(reason)` = 运行期失败 → 返回 `Err`，进程退出码非零。
///
/// 单独抽成函数是为了让"失败必须变成非零退出码"这条契约可以被单测锁住：此前
/// Lua 崩溃、渲染致命错误、场景/渲染器初始化失败都只调用 `event_loop.exit()`，
/// `main` 随后返回 `Ok(())`，CI、发布脚本与自动化验收都会把崩溃读成成功。
fn run_result(failure: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    match failure {
        None => Ok(()),
        Some(reason) => Err(Box::new(std::io::Error::other(format!(
            "game run failed: {reason}"
        )))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 失败态必须转成 `Err`（→ 非零退出码），正常退出才允许 `Ok`。
    #[test]
    fn runtime_failure_must_not_exit_zero() {
        assert!(run_result(None).is_ok(), "用户关窗 / app.quit() 属正常退出");
        let failure = run_result(Some("fixed-step Lua update failed: boom"));
        let error = failure.expect_err("运行期失败不得返回 Ok");
        assert!(
            error.to_string().contains("fixed-step Lua update failed"),
            "错误信息必须带原始原因: {error}"
        );
    }

    /// 退出码语义：`Err` 落在 `main` 上 → 进程退出码 1（`Termination` 默认行为）。
    #[test]
    fn failure_message_is_attached_to_the_process_error() {
        for reason in [
            "fixed-step Lua update failed: attempt to index a nil value",
            "renderer initialization failed: no suitable adapter",
            "runtime scene initialization failed: script error",
            "2D scene extraction failed: vertex buffer overflow",
            "renderer reported a fatal outcome",
        ] {
            let error = run_result(Some(reason)).expect_err("必须失败");
            assert!(error.to_string().ends_with(reason));
        }
    }
}
