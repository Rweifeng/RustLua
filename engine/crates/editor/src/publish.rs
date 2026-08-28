//! 发布打包：后台执行发布脚本、进度回传。

use crate::EditorApp;
use crate::panels::progress::{ProgressEvent, ProgressKind};
use crate::util::{display_path, strip_verbatim_prefix};
use std::{
    env, fmt, fs,
    io::BufRead,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{Sender, channel},
    thread,
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublishPlatform {
    Pc,
    WebGl,
    WechatMiniGame,
}

impl PublishPlatform {
    pub(crate) const ALL: [Self; 3] = [Self::Pc, Self::WebGl, Self::WechatMiniGame];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Pc => "PC",
            Self::WebGl => "WebGL",
            Self::WechatMiniGame => "微信小游戏",
        }
    }

    fn script_value(self) -> &'static str {
        match self {
            Self::Pc => "pc",
            Self::WebGl => "webgl",
            Self::WechatMiniGame => "wechat-minigame",
        }
    }
}

impl fmt::Display for PublishPlatform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

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
        let project_root = strip_verbatim(&self.project.root);
        let publish_name = normalize_publish_name(&self.publish_name, &self.project.root);
        let package_dir = project_root
            .join("builds")
            .join(self.publish_platform.script_value())
            .join(&publish_name);
        let (tx, rx) = channel();
        spawn_publish(
            script,
            project_root,
            self.publish_platform,
            publish_name,
            self.publish_compress,
            self.publish_encrypt,
            tx,
        );
        self.progress.start(
            ProgressKind::Publish,
            "发布打包",
            rx,
            Some(package_dir),
            None,
        );
    }
}

/// 从项目根向上查找 `scripts/package.ps1`（编辑器根可能是游戏目录或工作区根）。
fn find_publish_script(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .map(|dir| dir.join("scripts").join("package.ps1"))
        .find(|path| path.is_file())
}

/// 去掉 Windows `\\?\` 逐字路径前缀（open_project 规范化产生），
/// 外部工具（如 PowerShell -File）不识别该前缀，会静默失败。
fn strip_verbatim(path: &Path) -> PathBuf {
    strip_verbatim_prefix(path)
}

pub(crate) fn default_publish_name(project_root: &Path) -> String {
    let fallback = project_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("game")
        .to_owned();
    let Ok(text) = fs::read_to_string(project_root.join("project.toml")) else {
        return fallback;
    };
    extract_project_name(&text).unwrap_or(fallback)
}

fn extract_project_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("name =")
            .map(|rest| rest.trim().trim_matches('"').to_owned())
            .filter(|name| !name.trim().is_empty())
    })
}

fn normalize_publish_name(name: &str, project_root: &Path) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        default_publish_name(project_root)
    } else {
        trimmed.to_owned()
    }
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
fn spawn_publish(
    script: PathBuf,
    project_root: PathBuf,
    platform: PublishPlatform,
    publish_name: String,
    compress: bool,
    encrypt: bool,
    tx: Sender<ProgressEvent>,
) {
    thread::spawn(move || {
        let mut command = Command::new("powershell");
        command
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // PowerShell 参数：项目路径、发布平台必传，switch 参数勾选才传 `-Compress`/`-Encrypt`。
        command.arg("-ProjectRoot").arg(&project_root);
        command.arg("-Platform").arg(platform.script_value());
        command.arg("-PublishName").arg(&publish_name);
        if compress {
            command.arg("-Compress");
        }
        if encrypt {
            command.arg("-Encrypt");
        }
        #[cfg(windows)]
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
                        // 按脚本阶段标记推进确定进度；未知行保持当前进度。
                        if let Some(value) = publish_stage_progress(&line) {
                            let _ = tx_out.send(ProgressEvent::Progress(value));
                        }
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

/// 把发布脚本的阶段标记映射为大致进度 `0..=1`；未知行返回 `None`（保持当前进度）。
/// 对应 scripts/package.ps1 里的 `==>` 阶段与最终就绪标记。
fn publish_stage_progress(line: &str) -> Option<f32> {
    if line.contains("==> Release build") {
        Some(0.15)
    } else if line.contains("==> Install wasm-bindgen-cli") {
        Some(0.4)
    } else if line.contains("==> assetc package") {
        Some(0.55)
    } else if line.contains("==> Copy runner") {
        Some(0.7)
    } else if line.contains("==> Directory-isolation self test") {
        Some(0.85)
    } else if line.contains("Release package ready") {
        Some(0.95)
    } else {
        None
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
    fn default_publish_name_reads_project_name() {
        let root = std::env::temp_dir().join(format!("p2-publish-name-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nname = \"DemoGame\"\n",
        )
        .unwrap();
        assert_eq!(default_publish_name(&root), "DemoGame");
        std::fs::remove_dir_all(&root).ok();
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
        let project_root = workspace.join("game_test");
        let (tx, rx) = channel();
        spawn_publish(
            script,
            project_root,
            PublishPlatform::Pc,
            "game_test".to_owned(),
            false,
            false,
            tx,
        );
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
    fn publish_stage_progress_maps_stages_in_order() {
        // 阶段标记 → 进度；未知行 → None（保持当前进度）。
        assert_eq!(publish_stage_progress("==> Release build (pc)"), Some(0.15));
        assert_eq!(
            publish_stage_progress("==> assetc package (...)"),
            Some(0.55)
        );
        assert_eq!(
            publish_stage_progress("==> Copy runner and docs"),
            Some(0.7)
        );
        assert_eq!(
            publish_stage_progress("Release package ready: C:/x"),
            Some(0.95)
        );
        assert_eq!(publish_stage_progress("cargo build finished"), None);
        // 进度随阶段单调不减。
        let stages = [
            publish_stage_progress("==> Release build").unwrap(),
            publish_stage_progress("==> Install wasm-bindgen-cli").unwrap(),
            publish_stage_progress("==> assetc package").unwrap(),
            publish_stage_progress("==> Copy runner").unwrap(),
            publish_stage_progress("==> Directory-isolation self test").unwrap(),
            publish_stage_progress("Release package ready").unwrap(),
        ];
        for pair in stages.windows(2) {
            assert!(pair[1] >= pair[0], "阶段进度应单调递增");
        }
    }
}
