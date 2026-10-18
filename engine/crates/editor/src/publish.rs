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

/// 微信小游戏 appid 在 `project.toml` 里的键名。
///
/// 为什么放 `project.toml` 而不是编辑器自己的偏好文件：appid 是**这个工程**的身份
/// （一个小程序对应一个 appid），换台机器打开同一个工程应当还是它；而微信开发者工具
/// 导入的 `project.config.json` 每次发布都由 `plugins/wechat-minigame/` 模板重新生成，
/// 只有工程配置能活过重新打包。appid 本身会随包分发，不是密钥。
pub(crate) const WECHAT_APPID_KEY: &str = "wechat_appid";

/// 发布面板的全部 UI 状态（P1 EditorApp 瘦身：从 `EditorApp` 5 个散字段收敛）。
#[derive(Clone, Debug)]
pub(crate) struct PublishState {
    pub(crate) show_panel: bool,
    pub(crate) platform: PublishPlatform,
    pub(crate) name: String,
    pub(crate) compress: bool,
    pub(crate) encrypt: bool,
    /// 微信小游戏 appid（仅 `platform == WechatMiniGame` 时展示，随工程缓存）。
    pub(crate) wechat_appid: String,
    /// 本次会话里用户是否编辑过 appid 栏（没编辑过就不往工程配置里回写）。
    pub(crate) appid_edited: bool,
    /// 最近一次 appid 落盘的结果提示（空串表示无话可说）。
    pub(crate) message: String,
}

impl PublishState {
    pub(crate) fn new(project_root: &Path) -> Self {
        let mut state = Self {
            show_panel: false,
            platform: PublishPlatform::Pc,
            name: String::new(),
            compress: false,
            encrypt: false,
            wechat_appid: String::new(),
            appid_edited: false,
            message: String::new(),
        };
        state.reseed(project_root);
        state
    }

    /// 换工程后按**新工程**的配置重新播种「发布名 / appid」（平台、压缩等会话选择保留）。
    ///
    /// 为什么必须重新播种：`EditorApp::new` 只在启动时跑一次，而项目导入是就地替换
    /// `self.project`（`import.rs`）。不重播的话面板里还是上一个工程的值，
    /// 用户点一下发布或关面板就把旧工程的 appid/发布名写进了新工程的 `project.toml`。
    pub(crate) fn reseed(&mut self, project_root: &Path) {
        self.name = default_publish_name(project_root);
        self.wechat_appid = read_wechat_appid(project_root);
        self.appid_edited = false;
        self.message.clear();
    }
}

impl EditorApp {
    /// 启动 Release 发布打包（后台执行 scripts/build/package.ps1，非阻塞、不弹终端窗）。
    pub(crate) fn start_publish(&mut self) {
        if self.progress.running {
            return;
        }
        let Some(found) = find_publish_script(&self.project.root) else {
            self.progress.output = format!(
                "未找到发布脚本 scripts/build/package.ps1（项目：{}）",
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
        let publish_name = normalize_publish_name(&self.publish.name, &self.project.root);
        // 微信 appid：发布前先把面板里的值记进工程配置（记不进去也不挡发布——
        // 内存里的值照样传给脚本，只是下次打开工程要重填）。
        let wechat_appid = if self.publish.platform == PublishPlatform::WechatMiniGame {
            self.commit_wechat_appid();
            self.publish.wechat_appid.trim().to_owned()
        } else {
            String::new()
        };
        // 发布脚本 package.ps1 把产物直接发布到 <工程>/builds/<平台>（$finalRoot），
        // 不会再生成 <平台>/<发布名> 子目录；「打开目录」必须指向真实产物根，
        // 否则 ShellExecuteW 对不存在的路径静默失败，按钮点了没反应。
        let package_dir = project_root
            .join("builds")
            .join(self.publish.platform.script_value());
        let (tx, rx) = channel();
        spawn_publish(
            PublishRequest {
                script,
                project_root,
                platform: self.publish.platform,
                publish_name,
                wechat_appid,
                compress: self.publish.compress,
                encrypt: self.publish.encrypt,
                skip_build: false,
            },
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

    /// 把面板里的微信 appid 记进 `<工程>/project.toml`。
    ///
    /// 只在**用户真的编辑过**这一栏时写：面板字段是启动/换工程时从文件播种的，没编辑过
    /// 就提交，会把"播种失败/播种自另一个工程"的空值当成用户意图写回去，静默清掉别人
    /// 已经存好的 appid（实测踩过：切项目后关面板，`wechat_appid` 被覆成 `""`）。
    /// 提交点有三处（失焦、关面板、点发布），都走这里。
    pub(crate) fn commit_wechat_appid(&mut self) {
        if self.publish.platform != PublishPlatform::WechatMiniGame || !self.publish.appid_edited {
            return;
        }
        match write_wechat_appid(&self.project.root, &self.publish.wechat_appid) {
            Ok(_) => self.publish.message.clear(),
            Err(error) => self.publish.message = error,
        }
    }
}

/// 读工程已缓存的微信 appid（未配置返回空串）。
pub(crate) fn read_wechat_appid(project_root: &Path) -> String {
    read_project_toml(project_root)
        .and_then(|text| rustlua_infra::project_config::parse_key(&text, WECHAT_APPID_KEY))
        .unwrap_or_default()
}

/// 写微信 appid 到 `<工程>/project.toml`，返回「是否真的改了文件」。
///
/// 空值写成 `wechat_appid = ""` 而不是删行：读取侧把空值视为缺失，删行则要动别人的文件结构。
fn write_wechat_appid(project_root: &Path, appid: &str) -> Result<bool, String> {
    let path = project_root.join("project.toml");
    let text = read_project_toml(project_root).ok_or_else(|| {
        format!(
            "无法读写工程配置，微信 appid 未缓存：{}",
            display_path(&path)
        )
    })?;
    let appid = appid.trim();
    if read_wechat_appid(project_root).as_str() == appid {
        return Ok(false);
    }
    let updated = put_wechat_appid(&text, appid);
    if updated == text {
        return Ok(false);
    }
    fs::write(&path, updated)
        .map_err(|error| format!("微信 appid 写入 {} 失败：{error}", display_path(&path)))?;
    Ok(true)
}

/// 把 `wechat_appid` 落在 **`[project]` 段内**。
///
/// 为什么不能用 `panels::project_settings::upsert_project_toml`（它缺失键时追加到文件末尾）：
/// 真实工程的 `project.toml` 普遍以 `[audio]`/`[input]` 段结尾，追加进去就成了
/// `input.wechat_appid` —— 我方按行扁平扫描照样读得到，但任何真正的 TOML 解析方都会读到
/// 错的段（第一版就这么把 appid 写进了 `game_jhyt` 的 `[input]` 里）。
///
/// 顺带自愈：先把**任意位置**的旧 `wechat_appid` 行删掉再插到 `[project]` 段末，
/// 所以上面那批被写坏的工程，用户重新填一次 appid 就会被搬回正确的段。
/// 没有 `[project]` 表头的工程（纯扁平键）退回追加到文件末尾。
fn put_wechat_appid(content: &str, appid: &str) -> String {
    let line = format!("{WECHAT_APPID_KEY} = \"{appid}\"");
    let had_newline = content.ends_with('\n');
    let kept: Vec<String> = content
        .lines()
        .filter(|raw| !line_is_wechat_appid(raw))
        .map(str::to_owned)
        .collect();
    let mut section_end = None;
    let mut in_project = false;
    for (index, raw) in kept.iter().enumerate() {
        let trimmed = raw.trim().trim_start_matches('\u{feff}');
        if trimmed.starts_with('[') {
            if in_project {
                break;
            }
            in_project = trimmed == "[project]";
            continue;
        }
        // 注释与空行不算"段内最后一项"，但段存在时它们之后插入仍是段内，故记段头兜底。
        if in_project && !trimmed.is_empty() && !trimmed.starts_with('#') {
            section_end = Some(index + 1);
        }
    }
    let mut lines = kept;
    let insert_at = section_end.unwrap_or(lines.len());
    lines.insert(insert_at, line);
    let mut result = lines.join("\n");
    if had_newline || content.is_empty() {
        result.push('\n');
    }
    result
}

/// 某行是否为 `wechat_appid =`（去前导空白/BOM 后判断；注释行以 `#` 开头不会命中）。
fn line_is_wechat_appid(line: &str) -> bool {
    let trimmed = line.trim().trim_start_matches('\u{feff}');
    trimmed
        .strip_prefix(WECHAT_APPID_KEY)
        .is_some_and(|rest| rest.starts_with(" ="))
}

fn read_project_toml(project_root: &Path) -> Option<String> {
    fs::read_to_string(project_root.join("project.toml")).ok()
}

/// 定位发布脚本 `scripts/build/package.ps1`。
///
/// 发布脚本属于引擎工作区（`E:\Codex\RustLua\scripts\build\package.ps1`）。优先从编辑器
/// 自身定位工作区：
/// 1. 编译期 `CARGO_MANIFEST_DIR`（编辑器 crate 上溯 3 级 = 工作区根，见 `workspace_root`）；
/// 2. 运行时 `current_exe()` 所在目录及其祖先（编辑器被复制/安装到别处也有效）；
/// 3. 回退从项目根向上查找。
fn find_publish_script(root: &Path) -> Option<PathBuf> {
    let mut dirs = vec![crate::workspace_root()];
    if let Ok(exe) = env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.extend(parent.ancestors().map(Path::to_path_buf));
    }
    dirs.extend(root.ancestors().map(Path::to_path_buf));
    dirs.into_iter()
        .map(|dir| dir.join("scripts").join("build").join("package.ps1"))
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

/// 一次发布请求的全部参数（M20-03）。
///
/// 打成结构体而不是长参数表：`spawn_publish` 原先有 8 个参数，越过 clippy 的
/// `too_many_arguments` 阈值，且新增开关（如 `-SkipBuild`）时调用点极易错位。
#[derive(Debug, Clone)]
struct PublishRequest {
    script: PathBuf,
    project_root: PathBuf,
    platform: PublishPlatform,
    publish_name: String,
    /// 微信小游戏 appid；非微信平台传空串（脚本侧因此保持模板的 `touristappid`）。
    wechat_appid: String,
    compress: bool,
    encrypt: bool,
    /// 测试专用：复用 `target/release` 既有产物（`-SkipBuild`），正式发布传 false。
    skip_build: bool,
}

/// 后台执行发布脚本，把 stdout/stderr 行转发给 UI 线程。
/// CREATE_NO_WINDOW 保证不弹出额外终端窗口。
///
/// `skip_build`：测试专用——复用 `target/release` 既有产物（`-SkipBuild`），
/// 使端到端用例只验证「发现脚本 → spawn → 流式输出 → 原子替换」路径，
/// 不在单元测试里触发整仓 release 构建（正在运行的编辑器/杀毒会占用
/// `target/release/*.exe`，导致构建阶段被拒而误报失败）。正式发布传 false。
fn spawn_publish(request: PublishRequest, tx: Sender<ProgressEvent>) {
    let PublishRequest {
        script,
        project_root,
        platform,
        publish_name,
        wechat_appid,
        compress,
        encrypt,
        skip_build,
    } = request;
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
        // appid 只在微信小游戏平台传（非微信传的是空串，且脚本侧只有微信分支消费它）。
        if platform == PublishPlatform::WechatMiniGame && !wechat_appid.trim().is_empty() {
            command.arg("-WechatAppId").arg(wechat_appid.trim());
        }
        if compress {
            command.arg("-Compress");
        }
        if encrypt {
            command.arg("-Encrypt");
        }
        if skip_build {
            command.arg("-SkipBuild");
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
/// 对应 scripts/build/package.ps1 里的 `==>` 阶段与最终就绪标记。
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
            strip_verbatim(Path::new(r"\\?\E:\Codex\RustLua\scripts\build\package.ps1")),
            PathBuf::from(r"E:\Codex\RustLua\scripts\build\package.ps1")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\server\share\file.ps1")),
            PathBuf::from(r"\\server\share\file.ps1")
        );
        // 无前缀原样返回
        assert_eq!(
            strip_verbatim(Path::new(r"E:\Codex\RustLua\scripts\build\package.ps1")),
            PathBuf::from(r"E:\Codex\RustLua\scripts\build\package.ps1")
        );
    }

    #[test]
    fn default_publish_name_reads_project_name() {
        let root =
            std::env::temp_dir().join(format!("rustlua-publish-name-{}", std::process::id()));
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
        // 复现编辑器「打包发布」的 **spawn → 收行 → 收退出码** 全路径。
        //
        // 刻意不跑真实的 `scripts/build/package.ps1`：那条链依赖 release assetc、游戏资源与
        // 可写的 builds/，在单元测试里既慢又会因宿主 stdout 管道（assetc 的 broken pipe）
        // 产生与发布逻辑无关的红。真实脚本由 `scripts/verify` / CI 验收；这里用一个行为
        // 确定的 PowerShell 桩，钉住"参数怎么传、行怎么收、退出码怎么判"。
        let root =
            std::env::temp_dir().join(format!("rustlua-publish-spawn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("publish-stub.ps1");
        std::fs::write(
            &script,
            r#"param(
    [string]$ProjectRoot,
    [string]$Platform,
    [string]$PublishName,
    [switch]$SkipBuild,
    [switch]$Compress,
    [switch]$Encrypt,
    [string]$WechatAppId
)
Write-Output "==> Release build ($Platform)"
Write-Output "publish_stub_ok project=$ProjectRoot platform=$Platform name=$PublishName skip=$SkipBuild"
exit 0
"#,
        )
        .unwrap();
        let project_root = root.join("project");
        std::fs::create_dir_all(&project_root).unwrap();
        let (tx, rx) = channel();
        spawn_publish(
            PublishRequest {
                script,
                project_root,
                platform: PublishPlatform::Pc,
                publish_name: "examples".to_owned(),
                wechat_appid: String::new(),
                compress: false,
                encrypt: false,
                skip_build: true,
            },
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
        assert!(
            output.contains("publish_stub_ok"),
            "应收到子进程 stdout：\n{output}"
        );
        assert!(
            output.contains("platform=pc") && output.contains("skip=True"),
            "参数应原样传给脚本：\n{output}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_publish_script_finds_workspace_script_for_outside_project() {
        // 项目在工作区之外（如 C:\GitHub\games\ui_demo）：从项目根向上永远找不到脚本，
        // 应从编辑器所在工作区根解析到 scripts/build/package.ps1。
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap();
        let outside_project = workspace
            .parent()
            .map(|github| github.join("games").join("ui_demo"))
            .unwrap_or_else(|| PathBuf::from(r"C:\GitHub\games\ui_demo"));
        let script = find_publish_script(&outside_project).expect("应命中工作区根下的发布脚本");
        assert!(script.is_file());
        assert_eq!(
            script,
            workspace.join("scripts").join("build").join("package.ps1")
        );
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

    fn temp_project(tag: &str, toml: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rustlua-publish-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("project.toml"), toml).unwrap();
        dir
    }

    #[test]
    fn wechat_appid_seeds_publish_state_from_project_toml() {
        let root = temp_project(
            "appid-seed",
            "name = \"DemoGame\"\nwechat_appid = \"wx7091f1cf872130bb\"\n",
        );
        let state = PublishState::new(&root);
        assert_eq!(state.wechat_appid, "wx7091f1cf872130bb");
        // 未声明该键 → 空串（面板据此显示空输入，而不是 "None"）。
        let bare = temp_project("appid-bare", "name = \"DemoGame\"\n");
        assert_eq!(PublishState::new(&bare).wechat_appid, "");
        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&bare).ok();
    }

    #[test]
    fn wechat_appid_write_is_idempotent_and_keeps_other_lines() {
        let root = temp_project(
            "appid-write",
            "# 工程配置\nname = \"DemoGame\"\nstartup_scene = \"scenes/main.scene\"\n",
        );
        // 首次写入 → 追加，且注释与既有键原样保留。
        assert!(write_wechat_appid(&root, " wx7091f1cf872130bb ").unwrap());
        let text = fs::read_to_string(root.join("project.toml")).unwrap();
        assert!(text.contains("# 工程配置"), "{text}");
        assert!(
            text.contains("startup_scene = \"scenes/main.scene\""),
            "{text}"
        );
        assert!(
            text.contains("wechat_appid = \"wx7091f1cf872130bb\""),
            "写入前应去首尾空白：{text}"
        );
        // 同值再提交 → 不动文件（否则每次失焦都重刷工程配置）。
        assert!(!write_wechat_appid(&root, "wx7091f1cf872130bb").unwrap());
        // 换值 → 原地替换，不追加第二行。
        assert!(write_wechat_appid(&root, "wx0000000000000abc").unwrap());
        let text = fs::read_to_string(root.join("project.toml")).unwrap();
        assert_eq!(text.matches("wechat_appid =").count(), 1, "{text}");
        // 清空 → 写成空值（读取侧视为缺失），而不是删行。
        assert!(write_wechat_appid(&root, "").unwrap());
        assert_eq!(read_wechat_appid(&root), "");
        fs::remove_dir_all(&root).ok();
    }

    /// 真实工程的形状：`[project]` 后面还跟着 `[audio]`/`[input]` 段（`game_jhyt` 就是这样）。
    const SECTIONED_TOML: &str = "[project]\nname = \"Demo\"\nwindow_height = 720\n\n[audio]\nmaster = 0.5\n\n[input]\nmap = \"rustlua-actions-v2\\nattack=key:keyj\"\n";

    #[test]
    fn wechat_appid_lands_inside_the_project_section_not_at_eof() {
        // 第一版的回归：追加到文件末尾会落进 `[input]` 段，变成 `input.wechat_appid`。
        let root = temp_project("appid-section", SECTIONED_TOML);
        assert!(write_wechat_appid(&root, "wx7091f1cf872130bb").unwrap());
        let text = fs::read_to_string(root.join("project.toml")).unwrap();
        let appid = text.find("wechat_appid =").expect(&text);
        assert!(
            appid > text.find("[project]").unwrap() && appid < text.find("[audio]").unwrap(),
            "appid 必须落在 [project] 段内：{text}"
        );
        // 别人的段与键一个都不能少、顺序不能动。
        assert!(text.contains("window_height = 720"), "{text}");
        assert!(text.contains("master = 0.5"), "{text}");
        assert!(text.contains("attack=key:keyj"), "{text}");
        assert_eq!(text.matches("[input]").count(), 1, "{text}");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn wechat_appid_repairs_a_line_previously_written_into_input() {
        // 被上一版写坏的工程（键躺在 `[input]` 段末）：用户重新填一次就搬回 `[project]`。
        let broken = SECTIONED_TOML.replace(
            "map = \"rustlua-actions-v2\\nattack=key:keyj\"\n",
            "map = \"rustlua-actions-v2\\nattack=key:keyj\"\nwechat_appid = \"\"\n",
        );
        let root = temp_project("appid-repair", &broken);
        assert!(write_wechat_appid(&root, "wx7091f1cf872130bb").unwrap());
        let text = fs::read_to_string(root.join("project.toml")).unwrap();
        assert_eq!(text.matches("wechat_appid =").count(), 1, "{text}");
        assert!(
            text.find("wechat_appid =").unwrap() < text.find("[audio]").unwrap(),
            "搬回 [project] 段：{text}"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reseed_follows_the_newly_opened_project() {
        // 项目导入是就地替换 `self.project`：面板缓冲不重播就会把上一个工程的值写进新工程。
        let first = temp_project(
            "reseed-a",
            "[project]\nname = \"A\"\nwechat_appid = \"wxaaaaaaaaaaaaaaaa\"\n",
        );
        let second = temp_project(
            "reseed-b",
            "[project]\nname = \"B\"\nwechat_appid = \"wxbbbbbbbbbbbbbbbb\"\n",
        );
        let mut state = PublishState::new(&first);
        assert_eq!(state.wechat_appid, "wxaaaaaaaaaaaaaaaa");
        // 模拟"用户在 A 里改过 appid"后切到 B：脏标记必须随重播清掉。
        state.wechat_appid = "typed-by-hand".to_owned();
        state.appid_edited = true;
        state.reseed(&second);
        assert_eq!(state.wechat_appid, "wxbbbbbbbbbbbbbbbb");
        assert_eq!(state.name, "B");
        assert!(!state.appid_edited, "换工程后不该把上一个工程的值回写");
        fs::remove_dir_all(&first).ok();
        fs::remove_dir_all(&second).ok();
    }

    #[test]
    fn wechat_appid_write_reports_missing_project_toml() {
        let missing =
            std::env::temp_dir().join(format!("rustlua-publish-no-toml-{}", std::process::id()));
        let error = write_wechat_appid(&missing, "wx7091f1cf872130bb")
            .expect_err("没有 project.toml 应报错而不是静默丢弃");
        assert!(error.contains("project.toml"), "{error}");
    }
}
