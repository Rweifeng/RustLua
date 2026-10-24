//! P3-R05-4：编辑器侧的 shader 源热重载接线。
//!
//! 计划 §6.5 给编辑器的形态是**始终监听**项目 `shaders/`（开发工具默认就是开发态），
//! 与桌面运行时"只有 `--shader-dir`/`RUSTLUA_SHADER_DIR` 才监听"刻意不同。这一层只做三件事：
//!
//! 1. **命中判定**：观察器给的是项目相对路径，只有 `shaders/<内置文件名>.wgsl` 这一种形态
//!    可能改变生效源（`ShaderLibrary::override_path` 就是这个口径），其余变化一次重载都不该触发。
//! 2. **去抖**：一次保存会被观察器切成多个中间态，而一次重载要校验全部变体；把 150 ms 窗口内的
//!    事件并成一次，作者看到的是"改完 → 一份结果"。
//! 3. **诊断落地**：成功写状态栏 + Console（`重载 N/M 个变体 · 校验 Xms · 编译 Yms`），失败写
//!    Console + Scene View 诊断（错误文本自带**原文件:行:列**，见 `shader_reload` 的 `locate`）。
//!    "失败时一个字节都不改"这条不变量在渲染器侧（R05-3），这里只负责把它说清楚。
//!
//! 形状上的约束：`Renderer` 存在 `self.scene_renderer` 里，所以"碰渲染器"与"写 App 的
//! Console/状态栏/缓存"不能发生在同一个 `&mut self` 方法里（会双重借用）。因此渲染器侧的
//! 调用是**自由函数**（返回 [`ShaderWatchOutcome`]），把读数变成文本的工作在
//! [`EditorApp::record_shader_watch`] 里做。

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use editor::WatchDiff;
use eframe::egui;
use rustlua_gpu::shader_library::{
    SHADER_DIR_ENV, SHADER_RELOAD_DEBOUNCE, SHADER_SOURCE_DIR, shader_override_dir,
};
use rustlua_gpu::{Renderer, ShaderReloadError, ShaderReloadReport};

use crate::EditorApp;
use crate::util::strip_verbatim_prefix;

/// 观察器给出的**项目相对路径**是否命中 `shaders/*.wgsl`（覆盖目录的直接子文件）。
///
/// 为什么按"直接子文件"：解析口径是 `shaders/<内置文件名>.wgsl`，子目录里的文件永远读不到；
/// 为它重载一次等于给无关的临时文件付全量校验的钱。
#[must_use]
pub(crate) fn is_shader_override_path(path: &Path) -> bool {
    path.parent() == Some(Path::new(SHADER_SOURCE_DIR))
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wgsl"))
}

/// 失败诊断的行首：成功重载时按它认出"Scene View 上那句是我写的"，只擦自己那句。
const SHADER_FAILURE_PREFIX: &str = "shader 重载失败（旧管线保留）：";

/// 一次覆盖目录应用/重载的**结果**，还没变成 UI 文本（见模块注释里的借用约束）。
pub(crate) enum ShaderWatchOutcome {
    /// 内容与当前生效源逐字节相同（或目录里根本没有覆盖文件）：一次编译都没做，什么都不播。
    Unchanged,
    /// 真的重建了管线：状态栏 + Console 摘要 + 读数，画面需要重画。
    Reloaded {
        lines: Vec<String>,
        status: String,
        validate_ms: f64,
        compile_ms: f64,
    },
    /// 失败：渲染器保留了旧管线，`text` 是给用户看的一行（含原文件行列）。
    Failed { text: String },
}

/// 把覆盖目录接到刚建好/刚复用的渲染器上（首次应用；目录空 = 与内置源相同 = `Unchanged`）。
pub(crate) fn apply_shader_dir(renderer: &mut Renderer, dir: PathBuf) -> ShaderWatchOutcome {
    outcome_from_result(renderer.set_shader_override_dir(Some(dir)))
}

/// 到期后的那一次检查：内容没变时 `reload_shaders_if_changed` 返回 `Ok(None)`。
pub(crate) fn poll_shader_reload(renderer: &mut Renderer) -> ShaderWatchOutcome {
    outcome_from_optional(renderer.reload_shaders_if_changed())
}

fn outcome_from_result(
    result: Result<ShaderReloadReport, ShaderReloadError>,
) -> ShaderWatchOutcome {
    match result {
        Ok(report) => outcome_from_report(&report),
        Err(error) => ShaderWatchOutcome::Failed {
            text: error.to_string(),
        },
    }
}

fn outcome_from_optional(
    result: Result<Option<ShaderReloadReport>, ShaderReloadError>,
) -> ShaderWatchOutcome {
    match result {
        Ok(None) => ShaderWatchOutcome::Unchanged,
        Ok(Some(report)) => outcome_from_report(&report),
        Err(error) => ShaderWatchOutcome::Failed {
            text: error.to_string(),
        },
    }
}

/// 成功报告的文本化：只列**被覆盖**的那几份源——内置源是默认，逐条播报八行会让真正
/// 生效的那一条看不见。
fn outcome_from_report(report: &ShaderReloadReport) -> ShaderWatchOutcome {
    let overridden: Vec<&str> = report
        .sources
        .iter()
        .filter(|source| source.overridden)
        .map(|source| source.name)
        .collect();
    if report.rebuilt == 0 && overridden.is_empty() && report.fell_back_to_builtin.is_empty() {
        return ShaderWatchOutcome::Unchanged;
    }
    let mut lines = vec![format!(
        "shader：{} · 覆盖源 {}",
        report.summary(),
        if overridden.is_empty() {
            "无".to_owned()
        } else {
            overridden.join(", ")
        }
    )];
    for name in &report.fell_back_to_builtin {
        lines.push(format!(
            "shader {name} 的覆盖文件已删除，已回退内置源（画面回到发布形态）"
        ));
    }
    ShaderWatchOutcome::Reloaded {
        lines,
        status: format!(
            "shader 重载 {}/{} 个变体 · {:.1}ms",
            report.rebuilt,
            report.variants,
            report.validate_ms + report.compile_ms
        ),
        validate_ms: report.validate_ms,
        compile_ms: report.compile_ms,
    }
}

impl EditorApp {
    /// 本次编辑会话生效的覆盖目录：`--shader-dir` > `RUSTLUA_SHADER_DIR` > 项目 `shaders/`。
    ///
    /// 与运行时共用 [`shader_override_dir`] 这一条口径，差别只在 fallback：编辑器给项目
    /// `shaders/`（始终监听），运行时给 `None`（不监听）。
    pub(crate) fn shader_override_dir(&self) -> PathBuf {
        let root = strip_verbatim_prefix(&self.project.root);
        shader_override_dir(
            self.shader_dir_flag.clone(),
            std::env::var_os(SHADER_DIR_ENV).as_deref(),
            Some(root.join(SHADER_SOURCE_DIR)),
        )
        .expect("编辑器有项目 shaders/ 兜底，解析不会为空")
    }

    /// Scene View / Play 渲染器就绪时接上覆盖目录（两条路径不许读不同的源），并排好轮询节奏。
    pub(crate) fn attach_scene_shader_overrides(&mut self) {
        let dir = self.shader_override_dir();
        // 文件观察器只走**项目相对**路径：`--shader-dir` 指到项目外时它给不了事件，
        // 只能由 `tick_shader_reload` 自轮询。
        self.shader_watch_external =
            dir != strip_verbatim_prefix(&self.project.root).join(SHADER_SOURCE_DIR);
        let outcome = self
            .scene_renderer
            .as_mut()
            .map(|renderer| apply_shader_dir(renderer, dir));
        if let Some(outcome) = outcome {
            self.record_shader_watch(&outcome);
        }
    }

    /// 文件事件里命中 `shaders/*.wgsl` → 把去抖窗口往后推（连续保存只重载一次）。
    pub(crate) fn note_shader_watch(&mut self, diff: &WatchDiff) {
        if !diff.paths().any(|path| is_shader_override_path(path)) {
            return;
        }
        self.profiler.bump("shader_watch_hits");
        self.shader_watch_due = Some(Instant::now() + SHADER_RELOAD_DEBOUNCE);
    }

    /// 每帧一次的到期检查（由 `update` 调用）；不在监听窗口里时只是一次 `None`/时间判断。
    ///
    /// 需要 `context`：去抖到期时编辑器可能已经空闲（egui 不再重绘），不主动约一帧就会表现为
    /// "我保存了，画面等到下次动鼠标才变"。自轮询那条（覆盖目录在项目外）不约帧——否则空闲的
    /// 编辑器要永远每 150ms 重绘一次，改动会在下一次自然帧生效。
    pub(crate) fn tick_shader_reload(&mut self, context: &egui::Context) {
        let now = Instant::now();
        if self.shader_watch_external && self.shader_watch_due.is_none() {
            self.shader_watch_due = Some(now + SHADER_RELOAD_DEBOUNCE);
        }
        let Some(due) = self.shader_watch_due else {
            return;
        };
        if now < due {
            if !self.shader_watch_external {
                context.request_repaint_after(due - now);
            }
            return;
        }
        self.shader_watch_due = None;
        let Some(outcome) = self.scene_renderer.as_mut().map(poll_shader_reload) else {
            // Scene View 还没建渲染器（纯 2D 项目或首帧之前）：Play 会走 attach 路径，不报警。
            return;
        };
        if self.record_shader_watch(&outcome) {
            context.request_repaint();
        }
    }

    /// 清掉"上一次重载失败"留下的去抖状态与 Scene View 浮层，返回**之前是否真有失败**。
    ///
    /// 浮层只按前缀认自己的那句：`preview_diagnostic` 也被拖拽/框选用过，不能顺手清空。
    fn clear_shader_failure(&mut self) -> bool {
        let had_failure = self.shader_error_shown.take().is_some();
        if had_failure
            && self
                .preview_diagnostic
                .as_deref()
                .is_some_and(|text| text.starts_with(SHADER_FAILURE_PREFIX))
        {
            self.preview_diagnostic = None;
        }
        had_failure
    }

    /// 把一次重载的结果落到 Console / 状态栏 / Profiler。
    ///
    /// 返回"要不要约一帧"：`Reloaded` 作废 Scene View 两个视图的缓存并约一帧，
    /// `Failed`（以及失败被消除的那一刻）只约一帧让浮层文本可见，一直没有任何变化时返回 `false`。
    fn record_shader_watch(&mut self, outcome: &ShaderWatchOutcome) -> bool {
        match outcome {
            ShaderWatchOutcome::Unchanged => {
                // "没有变化"也是**恢复信号**：作者把坏覆盖文件删掉后，生效源回到内置、与管线里
                // 编译着的那份一致，于是一次重编都不做。此时画面没变（失败从来不曾 swap 管线），
                // 所以只清掉自己那句失败诊断并约一帧，不动视图缓存。
                if !self.clear_shader_failure() {
                    return false;
                }
                self.push_console_notice(
                    "shader 覆盖目录已回到可用状态（生效源与管线一致，无需重建）".to_owned(),
                );
                self.status = "shader 源正常".to_owned();
                true
            }
            ShaderWatchOutcome::Reloaded {
                lines,
                status,
                validate_ms,
                compile_ms,
            } => {
                self.clear_shader_failure();
                self.profiler.bump("shader_reloads");
                self.profiler
                    .set_gauge("shader_reload_validate_ms", *validate_ms);
                self.profiler
                    .set_gauge("shader_reload_compile_ms", *compile_ms);
                for line in lines {
                    self.push_console_notice(line.clone());
                }
                self.status = status.clone();
                // 管线换了，离屏纹理里那张画面就是过期的：两个视图缓存一起作废，下一帧重画。
                self.scene_view_cache_2d = crate::SceneViewCache2d::default();
                self.scene_view_cache_3d = crate::SceneViewCache3d::default();
                true
            }
            ShaderWatchOutcome::Failed { text } => {
                let message = format!("{SHADER_FAILURE_PREFIX}{text}");
                // 同一条原因只播一次：观察器每轮都会重新命中同一份坏文件。
                if self.shader_error_shown.as_deref() == Some(message.as_str()) {
                    return false;
                }
                self.shader_error_shown = Some(message.clone());
                self.profiler.bump("shader_reload_failures");
                self.push_console_notice(message.clone());
                // 不走 `set_scene_view_diagnostic`：它自己会再推一行 `[scene view] …`，
                // 同一条失败会在 Console 出现两次。这里只要 Scene View 的浮层文本。
                self.preview_diagnostic = Some(message);
                self.status = "shader 重载失败".to_owned();
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SceneViewCache3d;
    use editor::ProjectInfo;
    use rustlua_gpu::shader_reload::ShaderValidateError;
    use rustlua_infra::logging::LogSink;
    use std::{fs, time::Duration};

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-shader-watch-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp root");
        root
    }

    /// 没有 GPU 也能建 App（`gpu = None`）：这条接线测试只测命中判定、去抖状态机与文本化。
    fn headless_app(label: &str) -> (EditorApp, PathBuf) {
        let root = temp_root(label);
        let mut app = EditorApp::new(
            ProjectInfo {
                root: root.clone(),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        app.project_loaded = true;
        (app, root)
    }

    /// 真设备上的 App：R05-4 的出口门（"错误可定位到原文件行"）只有走真实渲染器才算跑过。
    ///
    /// GPU 缺席时**直接返回**是本仓编辑器的既有写法，但那样这条门一行都没跑；
    /// 所以在 `RUSTLUA_REQUIRE_GPU=1` 下把它变成硬失败（"没跑 ≠ 通过"）。
    fn gpu_app(label: &str) -> Option<(EditorApp, PathBuf)> {
        let Some((device, queue)) = test_device() else {
            assert!(
                std::env::var("RUSTLUA_REQUIRE_GPU").as_deref() != Ok("1"),
                "RUSTLUA_REQUIRE_GPU=1 但 GPU 不可用：这条出口门一行都没跑，不许记成通过"
            );
            return None;
        };
        let root = temp_root(label);
        let mut app = EditorApp::new(
            ProjectInfo {
                root: root.clone(),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            Some((device, queue)),
            LogSink::new(64),
        );
        app.project_loaded = true;
        Some((app, root))
    }

    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rustlua-shader-watch-test"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()
    }

    #[test]
    fn only_direct_shader_override_files_trigger_a_reload() {
        assert!(is_shader_override_path(Path::new("shaders/mesh3d.wgsl")));
        assert!(is_shader_override_path(Path::new("shaders/textured.wgsl")));
        // 观察器给的是项目相对路径：别的根、别的扩展名、子目录都不算。
        assert!(!is_shader_override_path(Path::new("assets/a.png")));
        assert!(!is_shader_override_path(Path::new("shaders/readme.md")));
        assert!(!is_shader_override_path(Path::new(
            "shaders/sub/mesh3d.wgsl"
        )));
        assert!(!is_shader_override_path(Path::new(
            "scripts/shaders/x.wgsl"
        )));
        assert!(!is_shader_override_path(Path::new("mesh3d.wgsl")));
    }

    #[test]
    fn override_dir_prefers_the_flag_then_the_project_default() {
        assert_eq!(
            std::env::var_os(SHADER_DIR_ENV).as_deref(),
            None,
            "测试进程不该带着 RUSTLUA_SHADER_DIR 跑"
        );
        let (mut app, root) = headless_app("flag");
        assert_eq!(
            app.shader_override_dir(),
            root.join(SHADER_SOURCE_DIR),
            "默认监听项目 shaders/"
        );
        app.shader_dir_flag = Some(PathBuf::from("explicit-dir"));
        assert_eq!(
            app.shader_override_dir(),
            PathBuf::from("explicit-dir"),
            "--shader-dir 必须压过项目默认"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_shader_event_arms_the_debounce_and_other_events_do_not() {
        let (mut app, root) = headless_app("debounce");
        let hit = WatchDiff {
            modified: vec![PathBuf::from("shaders/mesh3d.wgsl")],
            ..WatchDiff::default()
        };
        app.note_shader_watch(&hit);
        assert!(
            app.shader_watch_due.is_some(),
            "命中 shaders/*.wgsl 必须排上去抖窗口"
        );
        app.shader_watch_due = None;
        let miss = WatchDiff {
            modified: vec![PathBuf::from("assets/a.png")],
            removed: vec![PathBuf::from("shaders/notes.txt")],
            ..WatchDiff::default()
        };
        app.note_shader_watch(&miss);
        assert!(app.shader_watch_due.is_none(), "非覆盖文件不该触发重载");
        let _ = fs::remove_dir_all(root);
    }

    /// 到期检查在没有 Scene View 渲染器时也要把窗口关掉：否则每一帧都重新排一次，
    /// 状态机停在"永远差一帧"。
    #[test]
    fn an_expired_due_without_a_renderer_is_consumed() {
        let (mut app, root) = headless_app("expired");
        app.shader_watch_due = Some(Instant::now() - Duration::from_millis(1));
        let context = egui::Context::default();
        app.tick_shader_reload(&context);
        assert!(app.shader_watch_due.is_none());
        assert!(app.scene_renderer.is_none());
        let _ = fs::remove_dir_all(root);
    }

    /// 失败诊断：同一条原因只播一次，且必须写 Scene View 诊断（作者看的是那块画面）。
    #[test]
    fn a_failure_is_said_once_and_reaches_the_scene_view() {
        let (mut app, root) = headless_app("failure");
        let outcome = outcome_from_result(Err(ShaderReloadError::Validate(
            ShaderValidateError::Parse("mesh3d.wgsl:12:8: expected }".to_owned()),
        )));
        assert!(app.record_shader_watch(&outcome));
        let notices = app.console_notices.clone();
        assert_eq!(notices.len(), 1, "一条失败只播一行：{notices:?}");
        assert!(
            notices[0].contains("mesh3d.wgsl:12:8"),
            "诊断要带原文件行号：{}",
            notices[0]
        );
        assert_eq!(app.status, "shader 重载失败");
        assert!(
            app.preview_diagnostic.is_some(),
            "Scene View 诊断里要能看到失败原因"
        );
        app.record_shader_watch(&outcome);
        assert_eq!(
            app.console_notices.len(),
            1,
            "同一条原因不该每轮重播（观察器会反复命中同一份坏文件）"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 只改状态不改画面的两种结果都不许作废视图缓存（作废一次就多渲染一整帧）。
    #[test]
    fn unchanged_keeps_the_view_caches_and_reloaded_drops_them() {
        let (mut app, root) = headless_app("cache");
        app.scene_view_cache_3d = SceneViewCache3d {
            revision: Some(3),
            size: (128, 72),
            ..SceneViewCache3d::default()
        };
        assert!(!app.record_shader_watch(&ShaderWatchOutcome::Unchanged));
        assert_eq!(
            app.scene_view_cache_3d.revision,
            Some(3),
            "没重编就不该作废缓存"
        );
        let outcome = ShaderWatchOutcome::Reloaded {
            lines: vec!["shader：重载 1/6 个变体".to_owned()],
            status: "shader 重载 1/6 个变体 · 30.0ms".to_owned(),
            validate_ms: 12.0,
            compile_ms: 18.0,
        };
        assert!(app.record_shader_watch(&outcome));
        assert_eq!(
            app.scene_view_cache_3d.revision, None,
            "重编后两个视图都要重画"
        );
        assert_eq!(app.status, "shader 重载 1/6 个变体 · 30.0ms");
        let _ = fs::remove_dir_all(root);
    }

    /// 坏文件修好之后，Scene View 上必须只剩"没有诊断"：作者最容易盯着那块画面确认成功。
    /// 别人写的诊断（拖拽/框选）不在清除范围内。
    #[test]
    fn a_successful_reload_clears_only_our_own_diagnostic() {
        let (mut app, root) = headless_app("clear");
        let failure = outcome_from_result(Err(ShaderReloadError::Validate(
            ShaderValidateError::Parse("tonemap.wgsl:3:1: bad".to_owned()),
        )));
        assert!(app.record_shader_watch(&failure));
        let reloaded = ShaderWatchOutcome::Reloaded {
            lines: vec![],
            status: "shader 重载 1/1 个变体 · 5.0ms".to_owned(),
            validate_ms: 2.0,
            compile_ms: 3.0,
        };
        assert!(app.record_shader_watch(&reloaded));
        assert!(
            app.preview_diagnostic.is_none(),
            "成功后要擦掉自己那句：{:?}",
            app.preview_diagnostic
        );
        assert!(app.shader_error_shown.is_none(), "失败去抖状态也要一起清掉");
        app.preview_diagnostic = Some("[scene view] 别人写的诊断".to_owned());
        assert!(app.record_shader_watch(&reloaded));
        assert_eq!(
            app.preview_diagnostic.as_deref(),
            Some("[scene view] 别人写的诊断"),
            "不属于本模块的诊断不该被清除"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// R05-4 出口门（真设备）：项目 `shaders/` 里一份坏覆盖文件必须**在编辑器路径上**报出
    /// 原文件行列、把旧管线保住，并且作者把文件删掉后下一次轮询就恢复。
    /// 这条只有走真实 `Renderer` 才算跑过——前面的用例喂的是合成文本，证明的是文本化与状态机。
    #[test]
    fn a_broken_project_shader_reports_its_source_line_and_recovers() {
        let Some((mut app, root)) = gpu_app("broken-file") else {
            return;
        };
        let dir = root.join(SHADER_SOURCE_DIR);
        fs::create_dir_all(&dir).expect("覆盖目录");
        fs::write(
            dir.join("textured.wgsl"),
            "// 覆盖源：下面这行不是 WGSL\nfn ok() {}\n\n@@@ bad token\n",
        )
        .expect("写坏文件");
        // 真实入口：Scene View 建渲染器时顺带接上覆盖目录（与 Play 同源）。
        app.ensure_scene_renderer(64, 64);
        assert!(app.scene_renderer.is_some(), "离屏渲染器要能建起来");
        let notices = app.console_notices.clone();
        let line = notices
            .iter()
            .find(|notice| notice.contains("textured.wgsl:"))
            .unwrap_or_else(|| panic!("诊断要带原文件行号：{notices:?}"));
        assert!(line.contains(":4:"), "应指向第 4 行的坏 token：{line}");
        assert_eq!(app.status, "shader 重载失败");
        assert!(
            app.preview_diagnostic.is_some(),
            "Scene View 浮层要看到失败"
        );
        assert!(app.shader_error_shown.is_some(), "失败原因要留在去抖状态里");

        // 删掉覆盖文件 = 生效源回到内置，而管线里编译着的**本来就是**内置源（失败从不 swap）：
        // 这一次检查是"没有变化"，但它同时是恢复信号——失败状态与浮层必须一起消失。
        fs::remove_file(dir.join("textured.wgsl")).expect("删掉覆盖文件");
        let outcome = app
            .scene_renderer
            .as_mut()
            .map(poll_shader_reload)
            .expect("渲染器还在");
        assert!(
            matches!(outcome, ShaderWatchOutcome::Unchanged),
            "没有重编就不该报成 Reloaded"
        );
        let caches_before = (
            app.scene_view_cache_2d.revision,
            app.scene_view_cache_3d.revision,
        );
        assert!(
            app.record_shader_watch(&outcome),
            "恢复要约一帧，让浮层文本真的消失"
        );
        assert!(app.shader_error_shown.is_none(), "恢复后清掉失败去抖状态");
        assert!(
            app.preview_diagnostic.is_none(),
            "恢复后 Scene View 不该留着失败：{:?}",
            app.preview_diagnostic
        );
        assert_eq!(app.status, "shader 源正常");
        assert_eq!(
            (
                app.scene_view_cache_2d.revision,
                app.scene_view_cache_3d.revision
            ),
            caches_before,
            "没有重建管线就不该作废视图缓存"
        );
        assert!(
            app.console_notices.len() > notices.len(),
            "恢复也要在 Console 留一行：{:?}",
            app.console_notices
        );
        let _ = fs::remove_dir_all(root);
    }
}
