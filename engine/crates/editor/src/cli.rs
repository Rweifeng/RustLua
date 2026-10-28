//! 编辑器进程的**命令行面**：参数解析 + 无窗口命令用到的辅助（迁移报告落盘、截图缩放）。
//! 从 `main.rs` 原样搬出（P1 编辑器结构收口），行为一字未改。
//!
//! 可见性：根上 `pub(crate) use cli::{...}` 再导出一次，`crate::workspace_root` 这类既有
//! 路径保持不变（`panels/header.rs`、`publish.rs` 依赖它）。

use std::{
    env,
    path::{Path, PathBuf},
};

#[derive(Debug, Default)]
pub(crate) struct Arguments {
    pub(crate) project: Option<PathBuf>,
    pub(crate) check: bool,
    /// WP2 迁移命令：`--migrate-spatial`（dry-run）/ `--migrate-spatial-write` / `--restore-spatial`。
    pub(crate) migrate_spatial: bool,
    pub(crate) migrate_spatial_write: bool,
    pub(crate) restore_spatial: bool,
    /// 迁移报告的落盘路径（docs/54 §8-⑧ 的"报告"）：给了就写一份可归档的报告 artifact。
    pub(crate) migrate_report: Option<PathBuf>,
    /// ER-01 基线：无窗口、无 GPU 的编辑器性能基线报告（docs/55 阶段 0）。
    pub(crate) perf_baseline: bool,
    /// 基线报告落盘路径（可选；同时始终打印到 stdout）。
    pub(crate) perf_report: Option<PathBuf>,
    /// docs/55 阶段 0「GUI smoke」：无窗口的合成输入整帧用例（可当质量门）。
    pub(crate) gui_smoke: bool,
    /// GUI smoke 报告落盘路径（可选；同时始终打印到 stdout）。
    pub(crate) gui_report: Option<PathBuf>,
    /// M75：以 MCP stdio server 形态运行（供外部 AI 客户端拉起）。
    #[cfg(feature = "ai-bridge")]
    pub(crate) mcp_stdio: bool,
    /// P3-R05-4：显式指定 shader 覆盖目录；不给时编辑器用项目 `shaders/`（编辑器默认始终监听）。
    pub(crate) shader_dir: Option<PathBuf>,
}

pub(crate) fn parse_arguments() -> Result<Arguments, String> {
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
            "--shader-dir" => {
                let path = values
                    .next()
                    .ok_or_else(|| "--shader-dir 需要一个路径参数".to_owned())?;
                arguments.shader_dir = Some(PathBuf::from(path));
            }
            "--gui-smoke" => arguments.gui_smoke = true,
            "--gui-report" => {
                let path = values
                    .next()
                    .ok_or_else(|| "--gui-report 需要一个路径参数".to_owned())?;
                arguments.gui_report = Some(PathBuf::from(path));
            }
            #[cfg(feature = "ai-bridge")]
            "--mcp-stdio" => arguments.mcp_stdio = true,
            "--help" | "-h" => {
                #[cfg(feature = "ai-bridge")]
                let mcp_help = "\x20     editor --project <项目路径> --mcp-stdio（JSON-RPC 2.0 over stdio；stdout 只走协议）\n\
                     \x20     RUSTLUA_EDITOR_AI_AUTO_T1=1 让本次进程的 MCP 写请求自动批准 T1（T2/T3 仍拒绝）\n\
                     \x20     RUSTLUA_EDITOR_AI_AUTO_TOOLS=ui_create_preset 只自动放行列出的 T1 tool（逗号分隔；T2/T3 仍拒绝）\n";
                #[cfg(not(feature = "ai-bridge"))]
                let mcp_help = "";
                println!(
                    "用法：editor [--project <项目路径>] [--check]\n\
                     \x20     editor --project <项目路径> --migrate-spatial [--migrate-spatial-write]\n\
                     \x20     editor --project <项目路径> --restore-spatial\n\
                     \x20     editor --perf-baseline [--perf-report <路径>]\n\
                     \x20     editor --gui-smoke [--gui-report <路径>]\n\
                     \x20     editor --project <项目路径> [--shader-dir <目录>]\n\
                     {mcp_help}\
                     \x20     （迁移命令可加 --migrate-report <路径> 把报告落盘；有实体被拒绝或出错时退出码为 2）\n\
                     \x20     （--perf-baseline 无窗口运行 docs/55 ER-01 基线场景；有场景不达标时退出码为 2）\n\
                     \x20     （--gui-smoke 无窗口运行 docs/55 阶段 0 的合成输入 GUI 用例；有用例失败时退出码为 2）\n\
                     \x20     （shader 热重载：编辑器默认监听项目 shaders/，同名 .wgsl 覆盖内置源；\n\
                     \x20       --shader-dir 或 RUSTLUA_SHADER_DIR 可把覆盖目录指到别处）"
                );
                std::process::exit(0);
            }
            unknown => return Err(format!("未知参数：{unknown}")),
        }
    }
    Ok(arguments)
}

pub(crate) fn default_engine_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// RustLua 工作区根（含 scripts/、docs/ 等），相对编辑器 crate 上溯 3 级：`E:\Codex\RustLua`。
pub(crate) fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn downscale_rgba_benchmark(
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
pub(crate) fn run_spatial_migration(
    project_root: &Path,
    write: bool,
    restore: bool,
) -> (String, bool) {
    use rustlua_scene::scene_migration::{
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
pub(crate) fn write_migration_report(path: &Path, report: &str) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建报告目录失败 {}: {error}", parent.display()))?;
    }
    std::fs::write(path, report)
        .map_err(|error| format!("写入迁移报告失败 {}: {error}", path.display()))
}
