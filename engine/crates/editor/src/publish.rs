//! 发布打包与游戏项目导入：后台线程任务、进度回传、结果应用。

use crate::EditorApp;
use crate::progress::{ProgressEvent, ProgressKind};
use crate::util::{
    display_path, format_project_error, save_recent_projects, strip_verbatim_prefix,
};
use editor::{
    ProjectDocument, ProjectInfo, RecoveryManager, RecoveryScan, ResourceBrowserState, open_project,
};
use engine::core::{
    import_cache::ImportCache,
    importer::{ImportSummary, ImporterRegistry, run_import_batch},
    resource_db::{AssetId, ResourceDatabase},
};
use std::{
    env,
    io::BufRead,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::AtomicBool,
        mpsc::{Sender, channel},
    },
    thread,
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

impl EditorApp {
    /// 启动 Release 发布打包（后台执行 scripts/package.ps1，非阻塞、不弹终端窗）。
    pub(crate) fn start_publish(&mut self) {
        if self.progress.running {
            return;
        }
        let Some(found) = find_publish_script(&self.project.root) else {
            self.progress.output = format!(
                "未找到发布脚本（向上查找 scripts/package.ps1）：{}",
                display_path(&self.project.root)
            );
            self.progress.done = true;
            self.progress.success = false;
            self.progress.show = true;
            return;
        };
        // open_project 会生成 \\?\ 逐字路径，PowerShell -File 不识别，需先去掉前缀。
        let script = strip_verbatim(&found);
        // 发布脚本位于 <workspace>/scripts/package.ps1，发布包输出到 <workspace>/artifacts/builds/release/game。
        let workspace_root = script
            .parent()
            .and_then(|parent| parent.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let package_dir = workspace_root.join("artifacts/builds/release/game");
        let (tx, rx) = channel();
        spawn_publish(script, self.publish_compress, self.publish_encrypt, tx);
        self.progress.start(
            ProgressKind::Publish,
            "发布打包",
            rx,
            Some(package_dir),
            None,
        );
    }

    /// 启动导入游戏项目（后台执行，非阻塞）。
    pub(crate) fn start_import(&mut self) {
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
        self.progress
            .start(ProgressKind::ImportProject, "导入游戏项目", rx, None, None);
    }

    /// 每帧轮询通用进度面板；任务完成时应用结果（导入）并落盘发布日志。
    pub(crate) fn poll_progress(&mut self) {
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

    /// 启动资源后台导入（资源面板刷新后的待导入项），非阻塞。
    pub(crate) fn start_resource_import(&mut self) {
        if self.progress.running || !self.project_loaded {
            return;
        }
        let pending = self.resources.pending_imports().to_vec();
        if pending.is_empty() {
            return;
        }
        let root = self.project.root.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let (tx, rx) = channel();
        spawn_resource_import(root, pending.clone(), thread_cancel, tx);
        self.resources.record_import_attempts(&pending);
        self.progress.start(
            ProgressKind::ResourceImport,
            "导入资源",
            rx,
            None,
            Some(cancel),
        );
    }

    /// 任务完成：按任务类型应用后台结果。
    fn apply_progress_result(&mut self) {
        if !self.progress.done {
            return;
        }
        match self.progress.kind {
            // 资源导入完成：重新扫描读取最终状态（Ready/ImportFailed）。
            ProgressKind::ResourceImport => {
                if self.progress.success {
                    self.refresh_resources();
                    self.status = format!("资源导入完成：{}", display_path(&self.project.root));
                }
            }
            ProgressKind::ImportProject => {
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
                    self.status = format!(
                        "Imported game project: {}",
                        display_path(&self.project.root)
                    );
                    let root = self.project.root.clone();
                    self.record_recent_project(&root);
                }
            }
            ProgressKind::Publish => {}
        }
    }

    /// 记录最近打开的项目（去重、最新在前、最多 5 个），并落盘。
    pub(crate) fn record_recent_project(&mut self, path: &Path) {
        let clean = strip_verbatim_prefix(path).to_string_lossy().into_owned();
        self.recent_projects.retain(|p| p != &clean);
        self.recent_projects.insert(0, clean);
        self.recent_projects.truncate(5);
        save_recent_projects(&self.recent_projects);
    }

    /// 打开最近项目（项目菜单用）：走后台导入流程。
    pub(crate) fn open_recent_project(&mut self, path: String) {
        self.import_path = path;
        self.start_import();
    }
}

/// 在资源管理器中打开目录（ShellExecuteW，比 `explorer <path>` 更可靠）。
#[cfg(windows)]
pub(crate) fn open_in_explorer(path: &Path) {
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
pub(crate) fn open_in_explorer(_path: &Path) {}

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
            display_path(&path)
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

/// 后台资源导入：对待导入项跑 importer batch，回写 DB，进度逐行回传。
/// 不依赖前台状态：自行打开 DB/缓存实例，原子写盘防撕裂。
fn spawn_resource_import(
    root: PathBuf,
    items: Vec<(AssetId, PathBuf)>,
    cancel: Arc<AtomicBool>,
    tx: Sender<ProgressEvent>,
) {
    thread::spawn(move || {
        let _ = tx.send(ProgressEvent::Line(format!(
            "导入 {} 个资源...\n",
            items.len()
        )));
        let result = (|| -> Result<ImportSummary, String> {
            let mut database = ResourceDatabase::open(&root)
                .map_err(|error| format!("资源库打开失败：{error}"))?;
            let mut cache =
                ImportCache::open(&root).map_err(|error| format!("导入缓存打开失败：{error}"))?;
            let registry = ImporterRegistry::builtin();
            let items: Vec<(AssetId, PathBuf)> = items
                .into_iter()
                .map(|(id, relative)| (id, root.join(relative)))
                .collect();
            Ok(run_import_batch(
                &root,
                &mut database,
                &mut cache,
                registry,
                &items,
                &cancel,
            ))
        })();
        match result {
            Ok(summary) => {
                let _ = tx.send(ProgressEvent::Line(format!(
                    "导入完成：{} 个（导入 {}/复用 {}/失败 {}/跳过 {}），耗时 {}ms\n",
                    summary.total,
                    summary.imported,
                    summary.reused,
                    summary.failed,
                    summary.skipped,
                    summary.elapsed_ms
                )));
                let _ = tx.send(ProgressEvent::Finished {
                    success: true,
                    result: None,
                });
            }
            Err(error) => {
                let _ = tx.send(ProgressEvent::Line(format!("资源导入失败：{error}\n")));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
            }
        }
    });
}

#[cfg(test)]
mod publish_tests {
    use super::*;
    use engine::core::math3d::{Quat, Vec3};
    use engine::core::transform3d::Transform3D;

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
        let game = workspace.join("game_test");
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
    fn gizmo_drag_roundtrip_writes_full_transform3d() {
        // 用 commit_gizmo_drag3d 的编码路径验证：完整 Transform3D（位置/旋转/缩放）往返一致。
        let transform = Transform3D {
            position: Vec3::new(9.0, 5.0, -3.0),
            rotation: Quat::identity(),
            scale: Vec3::new(2.0, 2.0, 2.0),
        };
        let encoded = transform.to_component_str();
        let decoded = Transform3D::from_component_str(&encoded).expect("valid Transform3D");
        assert_eq!(decoded.position, Vec3::new(9.0, 5.0, -3.0));
        assert_eq!(decoded.scale, Vec3::new(2.0, 2.0, 2.0));
        assert!(decoded.rotation.is_unit());
    }
}
