// Windows: this is a GUI app; do not spawn a console window when the game is
// launched by double-clicking the exe.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod cli;
mod game_app;
mod splash;

use std::path::{Path, PathBuf};

use rustlua_infra::logging::init_logging;
use winit::event_loop::EventLoop;

use cli::{find_project_root, parse_arguments, resolve_window_title, verify_project};
use game_app::GameApp;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = parse_arguments().map_err(|error| format!("argument error: {error}"))?;
    let project = arguments.project.or_else(find_project_root);

    // 仅校验模式：不初始化日志/窗口，输出结果后直接退出。
    if arguments.check {
        let Some(root) = &project else {
            eprintln!("game_check_failed: 未找到游戏项目");
            std::process::exit(1);
        };
        let missing = verify_project(root);
        if missing.is_empty() {
            println!("game_check_ok project={}", root.display());
            // P3：把解析出的玩家数据位置打出来，发布脚本据此断言"发布包的存档/设置
            // 落在用户数据目录，而不是程序目录"（真实二进制、真实命令行参数）。
            let roots = rustlua_infra::user_dir::data_roots(root, arguments.pack);
            println!(
                "player_data_roots packaged={} save_dir={} settings_dir={}",
                roots.packaged,
                roots.save_dir.display(),
                roots.settings_dir.display()
            );
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
    );
    event_loop.run_app(&mut app)?;
    run_result(app.failure())
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
