// Windows: this is a GUI app; do not spawn a console window when the game is
// launched by double-clicking the exe.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod cli;
mod game_app;

use std::path::{Path, PathBuf};

use engine_core::logging::init_logging;
use winit::event_loop::EventLoop;

use cli::{find_project_root, parse_arguments, verify_project};
use game_app::GameApp;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = parse_arguments().map_err(|error| format!("argument error: {error}"))?;
    let project = arguments.project.or_else(find_project_root);

    // 仅校验模式：不初始化日志/窗口，输出结果后直接退出。
    if arguments.check {
        let Some(root) = &project else {
            eprintln!("game_runner_check_failed: 未找到游戏项目");
            std::process::exit(1);
        };
        let missing = verify_project(root);
        if missing.is_empty() {
            println!("game_runner_check_ok project={}", root.display());
            return Ok(());
        }
        eprintln!("game_runner_check_failed: {}", missing.join("; "));
        std::process::exit(1);
    }

    // Resolve the log path relative to the executable so it works regardless of
    // the working directory (e.g. when double-clicking the exe).
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let log_path = [
        exe_dir.join("../../../artifacts/logs/game-runner.log"),
        exe_dir.join("../artifacts/logs/game-runner.log"),
        PathBuf::from("artifacts/logs/game-runner.log"),
    ]
    .into_iter()
    .find(|path| path.parent().is_some_and(|parent| parent.is_dir()))
    .unwrap_or_else(|| PathBuf::from("artifacts/logs/game-runner.log"));
    init_logging(&log_path, arguments.log_filter.as_deref())
        .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
    engine_core::logging::install_crash_handler(log_path.with_file_name("crash.log"));
    tracing::info!(target: "game", version = engine_core::version(), project = ?project, "game runner starting");

    // 启动期资源完整性校验：发布包模式缺失即拒绝启动，开发模式仅告警。
    if let Some(root) = &project {
        let missing = verify_project(root);
        if !missing.is_empty() {
            for item in &missing {
                tracing::error!(target: "game", "资源校验失败: {item}");
            }
            if arguments.pack {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "发布资源包校验失败（{} 项缺失）：\n  {}",
                        missing.len(),
                        missing.join("\n  ")
                    ),
                )));
            }
        }
    }

    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut GameApp::new(
        project,
        !arguments.no_cull,
        !arguments.no_instance,
    ))?;
    Ok(())
}
