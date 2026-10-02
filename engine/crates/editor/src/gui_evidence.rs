//! docs/55 阶段 0：**真实窗口**的 GUI/GPU 证据与长会话 soak 运行器。
//!
//! 计划的阶段 0 验收里有三项只能在真实窗口里完成，`crate::framegate` 的合成帧替代不了：
//!
//! 1. Windows **100% / 150% / 200% DPI** 截图；
//! 2. 窗口 **resize** 之后的真实重绘；
//! 3. **30 分钟 Play/Stop 循环**与长会话 soak 的现场数据（含真实进程内存）。
//!
//! 因此这里做成"脚本化的一次真实运行"：由环境变量打开，在 `EditorApp::draw_frame` 里按帧推进，
//! 每一步设好 DPI 与窗口尺寸 → 等布局与 GPU 稳定 → 请求 `ViewportCommand::Screenshot` →
//! 把回传的 `Event::Screenshot` 落成 PNG，同时记录该步的几何/耗时/内存数据；跑完写
//! `manifest.json` 与 `summary.txt` 再关窗。产物直接进 `artifacts/acceptance/`，可复核、可复现。
//!
//! ## 环境变量
//!
//! | 变量 | 含义 |
//! |---|---|
//! | `RUSTLUA_EDITOR_GUI_EVIDENCE=<目录>` | 打开证据运行，产物写到该目录 |
//! | `RUSTLUA_EDITOR_GUI_EVIDENCE_WARMUP=<帧数>` | 每步截图前等待的帧数（默认 20） |
//! | `RUSTLUA_EDITOR_SOAK=<秒>` | 打开 soak（真实窗口内循环 Play/Stop + 编辑），每秒采样一次内存 |
//! | `RUSTLUA_EDITOR_SOAK_PLAY_SECONDS=<秒>` | soak 里每个 Play 阶段持续多久（默认 2 秒） |
//! | `RUSTLUA_EDITOR_SOAK_STEP_SECONDS=<秒>` | 两次采样之间的间隔（默认 1 秒） |
//! | `RUSTLUA_EDITOR_SOAK_CHURN_SECONDS=<秒>` | 每做一轮**编辑负载**（dock 重排 + 拖放导入）的间隔；不设则只循环 Play/Stop（与 docs/55 阶段 0 同构） |
//! | `RUSTLUA_EDITOR_SOAK_DROP_EVERY=<轮>` | 几轮做一次拖放导入（dock 重排每轮都做，默认 1） |
//! | `RUSTLUA_EDITOR_SOAK_DROP_SOURCE=<文件>` | 拖放导入的源文件覆盖（默认自动在工程外找一张真实纹理） |
//!
//! 退出码：`RUSTLUA_EDITOR_GUI_EVIDENCE` 模式下若有步骤没拿到截图，进程退出码为 2
//! （由 `run_gui_evidence` 在关闭窗口前记录，`main` 在 `run_native` 之后核对）。

use crate::EditorApp;
use eframe::egui;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// 每一步开始时的"收敛帧"数：DPI 与窗口尺寸都要重发几帧才会同时落地（见 `tick`）。
const SETTLE_FRAMES: usize = 6;

/// 截图那一刻的**逻辑尺寸**（点）。
fn measured_screen_pts(context: &egui::Context) -> (f32, f32) {
    context.input(|input| {
        let rect = input.content_rect();
        (rect.width(), rect.height())
    })
}

/// 一步证据采集。
#[derive(Clone, Debug)]
struct EvidenceStep {
    label: String,
    pixels_per_point: f32,
    inner_size: egui::Vec2,
    /// 采集前是否切到 3D 视口（覆盖 3D 相机的真实 GPU 路径）。
    view_3d: bool,
    /// 采集前是否进入 Play（覆盖内嵌预览的真实 GPU 路径）。
    play: bool,
}

/// 一次采集的记录。
///
/// `requested_*` 是脚本要求的值，`measured_*` 是**截图那一刻从 egui 读回来的实际值**。
/// 两者都要记：第一版只记请求值，结果 200% 那一档的图其实是 150% 渲染的
/// （`InnerSize` 由 eframe 按**当前** `pixels_per_point` 换算成物理像素，而 DPI 覆盖要到
/// 下一次 pass 才生效），报告却写着 ppi=2——这种"标签与内容不一致"的证据比没有证据更糟。
#[derive(Clone, Debug)]
struct CaptureRecord {
    label: String,
    requested_pixels_per_point: f32,
    requested_inner_size: (f32, f32),
    measured_pixels_per_point: f32,
    measured_screen_pts: (f32, f32),
    play: bool,
    view_3d: bool,
    image_size: (u32, u32),
    /// 非黑像素比例：证明"窗口真的渲染出了东西"，而不是一张全黑图。
    lit_ratio: f32,
    mean_luma: f32,
    working_set_mib: f64,
    file: Option<String>,
    error: Option<String>,
}

/// 真实窗口证据运行器（每帧被 `EditorApp::draw_frame` 推进一次）。
pub(crate) struct GuiEvidence {
    out_dir: PathBuf,
    steps: Vec<EvidenceStep>,
    index: usize,
    warmup: usize,
    waited: usize,
    requested: bool,
    records: Vec<CaptureRecord>,
    started: Instant,
    active_step: Option<EvidenceStep>,
    finished: bool,
    /// 准备阶段是否结束（打开场景 + 等启动资源导入跑完）。
    prepared: bool,
    /// 准备阶段实际等了多久（写进报告，避免"截图里还挂着导入进度窗"）。
    prepare_seconds: f32,
    /// 准备阶段经过的帧数：启动资源导入是在**首帧之后**才发起的，头几帧
    /// `progress.running` 还是 false，不能据此认定"已经空闲"。
    prepare_frames: u32,
    /// 准备阶段打开的场景（报告里点名，保证截图内容可追溯）。
    opened_scene: Option<String>,
}

impl GuiEvidence {
    /// 从环境变量构造（未打开时返回 `None`，生产路径零开销——只有一个 `Option` 判断）。
    pub(crate) fn from_env() -> Option<Self> {
        let out_dir = std::env::var("RUSTLUA_EDITOR_GUI_EVIDENCE").ok()?;
        let warmup = std::env::var("RUSTLUA_EDITOR_GUI_EVIDENCE_WARMUP")
            .ok()
            .and_then(|raw| raw.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(20);
        let out_dir = PathBuf::from(out_dir);
        Some(Self {
            out_dir,
            steps: evidence_steps(),
            index: 0,
            warmup,
            waited: 0,
            requested: false,
            records: Vec::new(),
            started: Instant::now(),
            active_step: None,
            finished: false,
            prepared: false,
            prepare_seconds: 0.0,
            prepare_frames: 0,
            opened_scene: None,
        })
    }

    /// 已跑完（`main` 用它决定退出码）。
    ///
    /// 失败判据有三条，缺一不可：
    /// 1. 没拿到图（`file` 为空或带 `error`）；
    /// 2. 图基本是黑的（`lit_ratio` 太低 = 窗口没渲染出内容）；
    /// 3. **实际 DPI 与请求不符**（写成 200% 的图其实是 150% 渲染的）。
    pub(crate) fn failed(&self) -> bool {
        self.records.iter().any(|record| {
            record.file.is_none()
                || record.error.is_some()
                || record.lit_ratio < 0.2
                || (record.measured_pixels_per_point - record.requested_pixels_per_point).abs()
                    > 0.01
        })
    }

    /// 产物目录（`main` 用它准备/清理归档目录）。
    pub(crate) fn output_dir(&self) -> &Path {
        &self.out_dir
    }

    /// 每帧推进一次。返回 `true` 表示已经请求关闭窗口。
    pub(crate) fn tick(&mut self, app: &mut EditorApp, context: &egui::Context) -> bool {
        if self.finished {
            return true;
        }
        // 准备阶段：先把场景打开、等启动时的资源导入跑完。
        //
        // 这一段是必需的：编辑器启动时会在后台导入整个工程的资源，**导入进度窗会盖住工作台**
        // （第一版证据的 200% DPI 截图里就是一块"导入资源 86%"的弹窗）。不等到它结束，
        // 归档的就是"启动瞬间"而不是"编辑器界面"。
        if !self.prepared {
            self.prepare(app, context);
            return false;
        }
        // 先把已经回传的截图收下来（egui 在请求之后的某一帧通过事件送回图像）。
        self.collect_screenshots(app, context);

        if self.index >= self.steps.len() {
            self.finish(app);
            return true;
        }
        if self.active_step.is_none() {
            let step = self.steps[self.index].clone();
            self.apply_step(app, context, &step);
            self.active_step = Some(step);
            self.waited = 0;
            self.requested = false;
            context.request_repaint();
            return false;
        }
        if !self.requested {
            let step = self.active_step.clone().expect("step");
            // 收敛帧：`set_pixels_per_point` 在下一次 pass 生效，而 `InnerSize` 是 eframe 用
            // **当时的** `pixels_per_point` 换算成物理像素的。两件事都要重发几帧才能同时落地，
            // 否则会出现"窗口按旧 DPI 定了尺寸、界面按新 DPI 排版"的错配。
            if self.waited < SETTLE_FRAMES {
                self.apply_step(app, context, &step);
            }
            self.waited += 1;
            if self.waited >= SETTLE_FRAMES + self.warmup {
                context.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                    step.label.clone(),
                )));
                self.requested = true;
            }
            context.request_repaint();
            return false;
        }
        // 已经请求，等截图事件；再等若干帧仍没有就记一次失败，避免卡死。
        self.waited += 1;
        if self.waited > SETTLE_FRAMES + self.warmup + 120 {
            let step = self.active_step.clone().expect("step");
            self.records.push(CaptureRecord {
                label: step.label.clone(),
                requested_pixels_per_point: step.pixels_per_point,
                requested_inner_size: (step.inner_size.x, step.inner_size.y),
                measured_pixels_per_point: context.pixels_per_point(),
                measured_screen_pts: measured_screen_pts(context),
                play: step.play,
                view_3d: step.view_3d,
                image_size: (0, 0),
                lit_ratio: 0.0,
                mean_luma: 0.0,
                working_set_mib: working_set_mib(),
                file: None,
                error: Some("截图事件在超时帧数内没有回传".to_owned()),
            });
            self.next_step(app);
        }
        context.request_repaint();
        false
    }

    /// 准备阶段：打开场景、等后台资源导入结束（最长等 180 秒，超时也继续并如实记录）。
    fn prepare(&mut self, app: &mut EditorApp, context: &egui::Context) {
        if app.document.current_scene.is_none() {
            let relative = std::env::var("RUSTLUA_EDITOR_EVIDENCE_SCENE")
                .unwrap_or_else(|_| "scenes/main.scene".to_owned());
            app.open_scene_resource(PathBuf::from(&relative));
            if app.document.current_scene.is_some() {
                self.opened_scene = Some(relative);
            } else {
                tracing::warn!(
                    target: "editor",
                    "[gui-evidence] 打开场景 {relative} 没有生效，状态={}",
                    app.status
                );
            }
        }
        let elapsed = self.started.elapsed().as_secs_f32();
        self.prepare_seconds = elapsed;
        self.prepare_frames += 1;
        // `start_resource_import` 在首帧末尾才发起，头几帧 `progress.running` 仍是 false；
        // 所以"空闲"要配合一个最小等待帧数，否则会在导入进度窗弹出前就开始截图。
        let waited_enough = self.prepare_frames >= 45;
        let idle = app.document.current_scene.is_some() && !app.progress.running;
        if waited_enough && idle {
            self.prepared = true;
            self.started = Instant::now();
            tracing::info!(
                target: "editor",
                "[gui-evidence] 准备完成（{elapsed:.1}s / {} 帧，场景={:?}），开始采集",
                self.prepare_frames,
                self.opened_scene
            );
        } else if elapsed > 180.0 {
            self.prepared = true;
            self.started = Instant::now();
            tracing::warn!(
                target: "editor",
                "[gui-evidence] 准备超时（{elapsed:.1}s），带着当前状态开始采集"
            );
        }
        context.request_repaint();
    }

    fn apply_step(&mut self, app: &mut EditorApp, context: &egui::Context, step: &EvidenceStep) {
        context.set_pixels_per_point(step.pixels_per_point);
        context.send_viewport_cmd(egui::ViewportCommand::InnerSize(step.inner_size));
        app.scene_view_mode = if step.view_3d {
            crate::SceneViewMode::View3D
        } else {
            crate::SceneViewMode::View2D
        };
        if step.play {
            if !app.is_playing() && app.document.current_scene.is_some() {
                app.play();
            }
        } else if app.is_playing() {
            app.stop();
        }
    }

    fn next_step(&mut self, app: &mut EditorApp) {
        if app.is_playing() {
            app.stop();
        }
        self.index += 1;
        self.active_step = None;
        self.waited = 0;
        self.requested = false;
    }

    fn collect_screenshots(&mut self, app: &mut EditorApp, context: &egui::Context) {
        let events = context.input(|input| input.raw.events.clone());
        for event in events {
            let egui::Event::Screenshot {
                image, user_data, ..
            } = event
            else {
                continue;
            };
            let label = user_data
                .data
                .as_ref()
                .and_then(|value| value.downcast_ref::<String>())
                .cloned()
                .unwrap_or_else(|| "screenshot".to_owned())
                .replace(['/', '\\', ':'], "_");
            let step = self.active_step.clone().unwrap_or_else(|| EvidenceStep {
                label: label.clone(),
                pixels_per_point: context.pixels_per_point(),
                inner_size: egui::vec2(0.0, 0.0),
                view_3d: false,
                play: app.is_playing(),
            });
            let (width, height) = (image.width() as u32, image.height() as u32);
            // 截图那一刻的**实测** DPI 与点尺寸：报告里的标签必须来自这里，而不是脚本的请求值。
            let measured_ppi = context.pixels_per_point();
            let measured_pts = measured_screen_pts(context);
            let mut lit = 0_usize;
            let mut luma_sum = 0.0_f64;
            let total = (width as usize * height as usize).max(1);
            let mut rgba = Vec::with_capacity(total * 4);
            for pixel in &image.pixels {
                let [r, g, b, a] = pixel.to_array();
                luma_sum +=
                    (0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)) / 255.0;
                if r > 8 || g > 8 || b > 8 {
                    lit += 1;
                }
                rgba.extend_from_slice(&[r, g, b, a]);
            }
            let mean_luma = (luma_sum / total as f64) as f32;
            let lit_ratio = lit as f32 / total as f32;
            let file_name = format!("{:02}-{}.png", self.index + 1, label);
            let path = self.out_dir.join(&file_name);
            let error = match engine::core::texture::save_png_rgba8(&path, width, height, &rgba) {
                Ok(()) => None,
                Err(error) => Some(error.to_string()),
            };
            self.records.push(CaptureRecord {
                label: step.label.clone(),
                requested_pixels_per_point: step.pixels_per_point,
                requested_inner_size: (step.inner_size.x, step.inner_size.y),
                measured_pixels_per_point: measured_ppi,
                measured_screen_pts: measured_pts,
                play: step.play,
                view_3d: step.view_3d,
                image_size: (width, height),
                lit_ratio,
                mean_luma,
                working_set_mib: working_set_mib(),
                file: error.as_ref().map_or(Some(file_name), |_| None),
                error,
            });
            self.next_step(app);
        }
    }

    fn finish(&mut self, app: &mut EditorApp) {
        if app.is_playing() {
            app.stop();
        }
        self.write_manifest(app);
        self.finished = true;
    }

    fn write_manifest(&self, app: &mut EditorApp) {
        let profile = app.profiler.report();
        let mut text = String::new();
        text.push_str("=== RustLua editor GUI/GPU evidence (docs/55 阶段 0) ===\n");
        text.push_str(&format!(
            "profile={} os={} started_unix={}\n",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            std::env::consts::OS,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default()
        ));
        text.push_str(&format!(
            "windows={} wall_seconds={:.1} prepare_seconds={:.1} scene={}\n\n",
            self.records.len(),
            self.started.elapsed().as_secs_f32(),
            self.prepare_seconds,
            self.opened_scene.as_deref().unwrap_or("<未打开>")
        ));
        text.push_str(
            "note: pixels_per_point 是 egui 的 DPI 覆盖（等价于显示器缩放 100/150/200%），\n\
             \x20     不是操作系统设置本身；每张截图都带非黑像素比例，全黑图会被判定为失败。\n\n",
        );
        for record in &self.records {
            let settled = (record.measured_pixels_per_point - record.requested_pixels_per_point)
                .abs()
                <= 0.01;
            text.push_str(&format!(
                "{:<28} req_ppi={:<4} act_ppi={:<4} req={:.0}x{:.0} screen={:.0}x{:.0} play={:<5} 3d={:<5} image={}x{} lit={:.3} luma={:.3} ws={:.1}MiB {}\n",
                record.label,
                record.requested_pixels_per_point,
                record.measured_pixels_per_point,
                record.requested_inner_size.0,
                record.requested_inner_size.1,
                record.measured_screen_pts.0,
                record.measured_screen_pts.1,
                record.play,
                record.view_3d,
                record.image_size.0,
                record.image_size.1,
                record.lit_ratio,
                record.mean_luma,
                record.working_set_mib,
                record.file.clone().unwrap_or_else(|| format!(
                    "FAILED{}: {}",
                    if settled { "" } else { "(DPI 未收敛)" },
                    record.error.clone().unwrap_or_default()
                ))
            ));
        }
        text.push_str(&format!(
            "\nframe_samples={} panels={} commands={}\n",
            profile.frame_samples,
            profile.panels.len(),
            profile.commands.len()
        ));
        text.push_str(&profile.to_text());
        let failed = self.records.iter().filter(|r| r.file.is_none()).count();
        text.push_str(&format!(
            "\ncaptures={} failed={} result={}\n",
            self.records.len(),
            failed,
            if failed == 0 { "ok" } else { "FAILED" }
        ));

        if let Err(error) = fs::create_dir_all(&self.out_dir) {
            tracing::error!(target: "editor", "[gui-evidence] 创建目录失败 {}: {error}", self.out_dir.display());
            return;
        }
        let _ = fs::write(self.out_dir.join("summary.txt"), &text);
        let _ = fs::write(self.out_dir.join("manifest.json"), self.manifest_json());
        tracing::info!(
            target: "editor",
            "[gui-evidence] 采集 {} 张截图，失败 {}，报告={}",
            self.records.len(),
            failed,
            self.out_dir.display()
        );
    }

    fn manifest_json(&self) -> String {
        let captures: Vec<serde_json::Value> = self
            .records
            .iter()
            .map(|record| {
                serde_json::json!({
                    "label": record.label,
                    "requested_pixels_per_point": record.requested_pixels_per_point,
                    "measured_pixels_per_point": record.measured_pixels_per_point,
                    "requested_inner_size_pts": [record.requested_inner_size.0, record.requested_inner_size.1],
                    "measured_screen_pts": [record.measured_screen_pts.0, record.measured_screen_pts.1],
                    "play": record.play,
                    "view_3d": record.view_3d,
                    "image_size": [record.image_size.0, record.image_size.1],
                    "lit_ratio": record.lit_ratio,
                    "mean_luma": record.mean_luma,
                    "working_set_mib": record.working_set_mib,
                    "file": record.file,
                    "error": record.error,
                })
            })
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "kind": "docs55-phase0-gui-evidence",
            "os": std::env::consts::OS,
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "scene": self.opened_scene,
            "prepare_seconds": self.prepare_seconds,
            "wall_seconds": self.started.elapsed().as_secs_f32(),
            "captures": captures,
        }))
        .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
    }
}

/// 采集脚本：3 档 DPI × 3 档窗口尺寸（含 2D 与 3D），外加一次 Play 下的内嵌预览。
fn evidence_steps() -> Vec<EvidenceStep> {
    let mut steps = Vec::new();
    for pixels_per_point in [1.0_f32, 1.5, 2.0] {
        for (width, height) in [(1280.0_f32, 720.0_f32), (1024.0, 700.0), (1600.0, 900.0)] {
            steps.push(EvidenceStep {
                label: format!("dpi{pixels_per_point}-{width:.0}x{height:.0}-2d"),
                pixels_per_point,
                inner_size: egui::vec2(width, height),
                view_3d: false,
                play: false,
            });
        }
    }
    steps.push(EvidenceStep {
        label: "dpi1.5-1280x720-3d".to_owned(),
        pixels_per_point: 1.5,
        inner_size: egui::vec2(1280.0, 720.0),
        view_3d: true,
        play: false,
    });
    steps.push(EvidenceStep {
        label: "dpi1.5-1280x720-play".to_owned(),
        pixels_per_point: 1.5,
        inner_size: egui::vec2(1280.0, 720.0),
        view_3d: false,
        play: true,
    });
    steps
}

// ---------------------------------------------------------------------------
// 长会话 soak
// ---------------------------------------------------------------------------
/// 一次内存采样。
///
/// 除了进程工作集，还记 **egui 纹理账**（数量 / 字节）：长会话里"内存阶跃"最常见的来源
/// 就是纹理缓存（Scene View 离屏目标、资源缩略图、Play 预览），把它一起量下来，
/// 报告才说得清"涨的是谁"而不是只有一条总曲线。
#[derive(Clone, Debug)]
struct SoakSample {
    elapsed_seconds: f32,
    working_set_mib: f64,
    egui_textures: usize,
    egui_texture_bytes: usize,
    playing: bool,
    frames: u64,
    entities: usize,
    undo_entries: usize,
    console_lines: usize,
    /// 截至本采样点做过的负载轮数（未开负载时恒为 0）。
    /// 放在采样里而不是只看总数：**内存台阶要和负载对齐**才能归因，
    /// 只有总计数就分不出"涨在第 40 分钟"是导入造成的还是缓存造成的。
    churn_rounds: u64,
}

/// 真实窗口长会话 soak：循环 Play/Stop + 编辑 + 资源刷新，定时采样内存。
pub(crate) struct GuiSoak {
    out_dir: PathBuf,
    duration: Duration,
    play_seconds: Duration,
    step_seconds: Duration,
    started: Instant,
    last_sample: Instant,
    playing_since: Option<Instant>,
    samples: Vec<SoakSample>,
    frames: u64,
    play_cycles: u64,
    finished: bool,
    failed: bool,
    /// 是否已经完成准备（打开文档）。
    prepared: bool,
    /// M27-03 验收 1 的负载：设了 `RUSTLUA_EDITOR_SOAK_CHURN_SECONDS` 才开。
    /// 没设时 soak 的行为与 docs/55 阶段 0 逐字一致（Play/Stop + 采样），
    /// 这条开关不进入生产路径，也不改变既有归档的可比性。
    churn: Option<ChurnLoad>,
}

/// soak 的编辑负载运行态（M27-03 验收 1：dock 重排 + 拖放导入）。
///
/// 为什么必须**带负载**跑：docs/55 阶段 0 那两档 soak 只循环 Play/Stop，测的是"挂机不动会不会漏"。
/// M27 新加的面板重排、拖放导入、浮动窗增删都发生在**编辑路径**上，空转 soak 一条都碰不到——
/// "≥2h 无增长"如果不含这些动作，就等于没测本期。
struct ChurnLoad {
    /// 每做一轮负载的间隔（一轮 = 一次 dock 重排 + 一次可选的拖放导入）。
    every: Duration,
    /// 上一轮负载的时刻。
    last: Instant,
    /// 每几轮做一次拖放导入（dock 重排每轮都做）。
    drop_every: u64,
    /// 拖放导入的源文件：必须是**工程外**的真实纹理（工程内的文件走"只引用不复制"，
    /// 那条路径压不到复制入库与缩略图）。在准备阶段一次性定好。
    source: Option<PathBuf>,
    /// 上一轮导入留在工程里的副本。它会在**下一轮**开头删掉，而不是当轮删：
    /// 入库是后台任务，当轮删除等于专测"导入被文件消失打断"那条错误路径。
    pending_copy: Option<PathBuf>,
    rounds: u64,
    dock_churns: u64,
    drop_imports: u64,
    drop_cleanups: u64,
    /// 负载返回"跳过…"的次数——非零说明负载在空转，报告要显眼地标出来。
    skipped: u64,
    /// 最近 12 条负载返回值，进报告（证明"确实在动"而不是只加了一个计数器）。
    recent: Vec<String>,
}

impl GuiSoak {
    pub(crate) fn from_env() -> Option<Self> {
        let seconds = std::env::var("RUSTLUA_EDITOR_SOAK")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|value| *value > 0)?;
        let play_seconds = std::env::var("RUSTLUA_EDITOR_SOAK_PLAY_SECONDS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(2);
        let step_seconds = std::env::var("RUSTLUA_EDITOR_SOAK_STEP_SECONDS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1);
        let out_dir = std::env::var("RUSTLUA_EDITOR_GUI_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("rustlua-editor-soak"));
        // 负载是**可选**的：没设 CHURN_SECONDS 时 soak 的采样与 Play/Stop 节奏和 docs/55 阶段 0
        // 的既有归档同构（报告里多一列恒为 0 的 churn_rounds，是为了让"没带负载"这件事
        // 在数据里可见，而不是要读者去猜），也不会把编辑动作混进一条本来只测挂机的曲线里。
        let churn = std::env::var("RUSTLUA_EDITOR_SOAK_CHURN_SECONDS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .map(|every| ChurnLoad {
                every: Duration::from_secs(every),
                last: Instant::now(),
                drop_every: std::env::var("RUSTLUA_EDITOR_SOAK_DROP_EVERY")
                    .ok()
                    .and_then(|raw| raw.parse::<u64>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(1),
                // 在准备阶段（已经知道工程根）再定源文件。
                source: None,
                pending_copy: None,
                rounds: 0,
                dock_churns: 0,
                drop_imports: 0,
                drop_cleanups: 0,
                skipped: 0,
                recent: Vec::new(),
            });
        Some(Self {
            out_dir,
            duration: Duration::from_secs(seconds),
            play_seconds: Duration::from_secs(play_seconds),
            step_seconds: Duration::from_secs(step_seconds),
            started: Instant::now(),
            last_sample: Instant::now(),
            playing_since: None,
            samples: Vec::new(),
            frames: 0,
            play_cycles: 0,
            finished: false,
            failed: false,
            prepared: false,
            churn,
        })
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed
    }

    /// 产物目录（`main` 用它报出归档位置）。
    pub(crate) fn output_dir(&self) -> &Path {
        &self.out_dir
    }

    /// 每帧推进一次。返回 `true` 表示已经请求关闭窗口。
    pub(crate) fn tick(&mut self, app: &mut EditorApp, context: &egui::Context) -> bool {
        if self.finished {
            return true;
        }
        self.frames += 1;
        let now = Instant::now();
        if !self.prepared {
            // 长会话需要一个打开的文档：没有就自己打开默认场景（soak 的语义是"编辑器开着
            // 一直用"，而不是"空窗口挂着"）。
            if app.document.current_scene.is_none() {
                let relative = std::env::var("RUSTLUA_EDITOR_EVIDENCE_SCENE")
                    .unwrap_or_else(|_| "scenes/main.scene".to_owned());
                app.open_scene_resource(PathBuf::from(&relative));
                if app.document.current_scene.is_none() {
                    tracing::error!(
                        target: "editor",
                        "[soak] 无法打开场景 {relative}（状态={}），终止 soak",
                        app.status
                    );
                    self.failed = true;
                    self.finish(app);
                    return true;
                }
                tracing::info!(target: "editor", "[soak] 已打开场景 {relative}");
            }
            if let Some(load) = &mut self.churn {
                load.source = find_external_drop_source(&app.project.root);
                match &load.source {
                    Some(path) => tracing::info!(
                        target: "editor",
                        "[soak] 拖放负载源（工程外）={}",
                        path.display()
                    ),
                    None => tracing::warn!(
                        target: "editor",
                        "[soak] 没找到工程外的纹理：负载只做 dock 重排，不含拖放导入"
                    ),
                }
            }
            self.prepared = true;
            self.last_sample = now;
        }
        // Play / Stop 循环。
        match self.playing_since {
            Some(since) if now.duration_since(since) >= self.play_seconds => {
                app.stop();
                self.playing_since = None;
                self.play_cycles += 1;
            }
            None => {
                app.play();
                if app.is_playing() {
                    self.playing_since = Some(now);
                }
            }
            Some(_) => {}
        }
        self.run_churn_round(app, now);
        // 采样：内存 + 会话级计数（历史深度、日志行数、实体数）。
        if now.duration_since(self.last_sample) >= self.step_seconds {
            self.last_sample = now;
            let entities = scene_entity_count(app);
            let console_lines = app.log_sink.len();
            let (egui_textures, egui_texture_bytes) = {
                let manager = context.tex_manager();
                let manager = manager.read();
                (
                    manager.num_allocated(),
                    manager
                        .allocated()
                        .map(|(_, meta)| meta.bytes_used())
                        .sum::<usize>(),
                )
            };
            self.samples.push(SoakSample {
                elapsed_seconds: self.started.elapsed().as_secs_f32(),
                working_set_mib: working_set_mib(),
                egui_textures,
                egui_texture_bytes,
                playing: app.is_playing(),
                frames: self.frames,
                entities,
                undo_entries: app.history.undo_len(),
                console_lines,
                churn_rounds: self.churn.as_ref().map_or(0, |load| load.rounds),
            });
            // **增量落盘**：8 小时这一档如果被外部打断（关窗、机器重启、会话结束），
            // 只在结束时写报告就等于"什么都没留下"。每 12 个采样重写一次，`complete` 字段
            // 明确标记这次落盘是中途快照还是最终报告——半份真实数据比一份完美但没有的数据有用。
            if self.samples.len().is_multiple_of(12) {
                self.write_report(app, false);
            }
        }
        if self.started.elapsed() >= self.duration {
            self.finish(app);
            return true;
        }
        context.request_repaint();
        false
    }

    /// 做一轮 M27-03 验收 1 的编辑负载：一次 dock 重排，外加每 `drop_every` 轮一次
    /// "拖放导入 → 撤销建实体 → （下一轮开头）删掉副本"。
    ///
    /// 为什么导入要撤销、副本要删：泄漏问的是"**同一份工作**重复上百次会不会留住内存"。
    /// 每轮真留一个新文件，资源库、缩略图与 `.cache` 就随轮数单调变大，那条曲线量到的是
    /// "资产变多"而不是"重复工作"，谷值判据当场失去意义。撤销建实体那一步（导入本身是
    /// 文件系统动作、不进历史）再加下一轮的删副本，把工程大小钉成常量，同时把
    /// "复制入库 → 分配 GUID → 后台导入 → 建实体 → 撤销 → 删除 → 依赖重算"整条链每轮跑通。
    fn run_churn_round(&mut self, app: &mut EditorApp, now: Instant) {
        let Some(load) = &mut self.churn else {
            return;
        };
        if now.duration_since(load.last) < load.every {
            return;
        }
        load.last = now;
        load.rounds += 1;
        let round = load.rounds;
        let mut notes: Vec<String> = Vec::new();

        // 1) dock 重排：走 `DockState` 的真实变更 API，与鼠标拖拽改的是同一棵树
        //    （但它不是真实鼠标，这点在 `dock::churn` 的文档注释里写明）。
        //    步号从 0 起：六步里第一步必须是"把控制台拆出去"，从 1 起会让第 1 轮
        //    直接落在"跳过：控制台不在浮动窗口里"上，报告里那条 skipped 就永远是 1。
        let dock_note = match app.dock_state.as_mut() {
            Some(state) => crate::dock::churn(state, (round - 1) as usize),
            None => "跳过：dock 树正被表面绘制借走".to_owned(),
        };
        if dock_note.starts_with("跳过") {
            load.skipped += 1;
        } else {
            load.dock_churns += 1;
        }
        notes.push(dock_note);

        // 2) 拖放导入：只在轮到它、且准备阶段找到了工程外源文件时做。
        let source = if round.is_multiple_of(load.drop_every) {
            load.source.clone()
        } else {
            None
        };
        if let Some(source) = source {
            if let Some(previous) = load.pending_copy.take() {
                match fs::remove_file(&previous) {
                    Ok(()) => {
                        // 删除不需要单独再刷一次：紧随其后的导入自带后台重扫，
                        // `reconcile` 会把已经消失的文件从数据库里剪掉。
                        load.drop_cleanups += 1;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => notes.push(format!("跳过：删不掉上一轮的副本 {error}")),
                }
            }
            // Play 期间文档只读、导入会被拒绝，所以先停（这一停是真实动作，计入循环数）。
            if app.is_playing() {
                app.stop();
                self.playing_since = None;
                self.play_cycles += 1;
            }
            // 预测落点：`target_path` 按重名递增，而本轮已经删掉上一轮的副本，名字会重新
            // 可用，于是每轮都往同一路径写同一份内容——工程大小因此是常量。
            let target = crate::dnd::target_path(&app.project.root, &source);
            let entities_before = scene_entity_count(app);
            let note = app.ingest_and_spawn(std::slice::from_ref(&source), None);
            if note.contains("未导入任何文件") || note.starts_with("Play mode locks") {
                load.skipped += 1;
            } else {
                load.drop_imports += 1;
                load.pending_copy = Some(target);
                // 只有**真建出实体**才撤销：撤销一次没落地的动作等于弹掉历史里别人的改动。
                if scene_entity_count(app) > entities_before {
                    app.undo();
                }
            }
            let name = source
                .file_name()
                .map_or_else(|| "?".to_owned(), |raw| raw.to_string_lossy().into_owned());
            notes.push(format!("导入 {name}：{note}"));
        }

        for note in notes {
            if load.recent.len() >= 12 {
                load.recent.remove(0);
            }
            load.recent.push(note);
        }
    }

    fn finish(&mut self, app: &mut EditorApp) {
        if app.is_playing() {
            app.stop();
        }
        self.write_report(app, true);
        self.finished = true;
    }

    /// 报告的核心是**趋势**：前 10% 采样的均值对后 10% 的均值。
    ///
    /// `complete = false` 时是**中途快照**（长会话被外部打断也留下可分析的数据）。
    fn write_report(&self, app: &mut EditorApp, complete: bool) {
        let profile = app.profiler.report();
        let mut text = String::new();
        text.push_str("=== RustLua editor soak (docs/55 阶段 0 / §3.2 长会话) ===\n");
        text.push_str(&format!(
            "profile={} os={} requested_seconds={} samples={} frames={} play_cycles={} complete={}\n\n",
            if cfg!(debug_assertions) { "debug" } else { "release" },
            std::env::consts::OS,
            self.duration.as_secs(),
            self.samples.len(),
            self.frames,
            self.play_cycles,
            complete
        ));
        if let Some(load) = &self.churn {
            text.push_str(&format!(
                "churn: every_seconds={} drop_every_rounds={} rounds={} dock_churns={} \
                 drop_imports={} drop_cleanups={} skipped={} source={}\n\
                 \x20     rounds/dock_churns/drop_imports 是负载**已做量**：内存台阶要对着这三条线归因；\n\
                 \x20     skipped>0 表示该轮被 Play 锁定或树状态拒绝，负载在空转，曲线不能读成『重排过这么多遍』。\n",
                load.every.as_secs(),
                load.drop_every,
                load.rounds,
                load.dock_churns,
                load.drop_imports,
                load.drop_cleanups,
                load.skipped,
                load.source.as_ref().map_or_else(
                    || "（没找到工程外源文件）".to_owned(),
                    |path| path.display().to_string()
                )
            ));
        }
        text.push_str(
            "note: working_set 是**真实进程工作集**（Windows GetProcessMemoryInfo）；\n\
             \x20     egui_tex/tex_MiB 是 egui 纹理账（数量 / 字节），用于归因内存阶跃；\n\
             \x20     判据看**各窗口的谷值趋势**（窗口 8 段取 min）与首尾均值差，而不是单点绝对值：\n\
             \x20     缓存式锯齿会让均值上下跳，但谷值不抬升；真泄漏会让谷值抬升。\n\n",
        );
        text.push_str(
            "seconds\tworking_set_mib\tegui_tex\ttex_MiB\tplaying\tframes\tentities\tundo\tconsole\tchurn_rounds\n",
        );
        for sample in &self.samples {
            text.push_str(&format!(
                "{:.0}\t{:.1}\t{}\t{:.1}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                sample.elapsed_seconds,
                sample.working_set_mib,
                sample.egui_textures,
                sample.egui_texture_bytes as f64 / (1024.0 * 1024.0),
                sample.playing,
                sample.frames,
                sample.entities,
                sample.undo_entries,
                sample.console_lines,
                sample.churn_rounds
            ));
        }
        let (head, tail) = window_means(&self.samples);
        let growth = tail - head;
        text.push_str(&format!(
            "\nhead_mean_mib={head:.1} tail_mean_mib={tail:.1} growth_mib={growth:.1}\n"
        ));
        // **谷值趋势**：把整段按时长切成 8 个窗口，各取 min/p50/max。
        // 锯齿（缓存涨落）会让均值乱跳而谷值平稳；持续泄漏会让谷值抬升——
        // 这才是 docs/55 §3.2「8 小时无持续内存增长」该看的那条线。
        let windows = memory_windows(&self.samples, 8);
        text.push_str("\nwindow\tmin_mib\tp50_mib\tmax_mib\ttex_min_MiB\ttex_max_MiB\n");
        for (index, window) in windows.iter().enumerate() {
            text.push_str(&format!(
                "{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\n",
                index + 1,
                window.min_mib,
                window.p50_mib,
                window.max_mib,
                window.min_texture_mib,
                window.max_texture_mib
            ));
        }
        if let (Some(first), Some(last)) = (windows.first(), windows.last()) {
            let trough_growth = last.min_mib - first.min_mib;
            let amplitude = windows
                .iter()
                .map(|window| window.max_mib - window.min_mib)
                .fold(0.0_f64, f64::max);
            text.push_str(&format!(
                "trough_first_mib={:.1} trough_last_mib={:.1} trough_growth_mib={trough_growth:.1} max_amplitude_mib={amplitude:.1}\n",
                first.min_mib, last.min_mib
            ));
        }
        if let Some(load) = &self.churn {
            // 逐条留痕：只有计数器无法证明负载真的落到了树上，最近 12 条返回值让复核的人
            // 一眼看出"这几百轮里有没有一半在空转"。
            text.push_str(&format!("\nchurn_notes(last={}):\n", load.recent.len()));
            for note in &load.recent {
                text.push_str(&format!("  - {note}\n"));
            }
        }
        text.push_str(&profile.to_text());
        text.push_str(&format!(
            "\nresult={}\n",
            if self.failed { "FAILED" } else { "ok" }
        ));

        if let Err(error) = fs::create_dir_all(&self.out_dir) {
            tracing::error!(target: "editor", "[soak] 创建目录失败 {}: {error}", self.out_dir.display());
            return;
        }
        let _ = fs::write(self.out_dir.join("soak.txt"), &text);
        let samples: Vec<serde_json::Value> = self
            .samples
            .iter()
            .map(|sample| {
                serde_json::json!({
                    "seconds": sample.elapsed_seconds,
                    "working_set_mib": sample.working_set_mib,
                    "egui_textures": sample.egui_textures,
                    "egui_texture_bytes": sample.egui_texture_bytes,
                    "playing": sample.playing,
                    "frames": sample.frames,
                    "entities": sample.entities,
                    "undo_entries": sample.undo_entries,
                    "console_lines": sample.console_lines,
                    "churn_rounds": sample.churn_rounds,
                })
            })
            .collect();
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "kind": "docs55-phase0-soak",
            "complete": complete,
            "requested_seconds": self.duration.as_secs(),
            "actual_seconds": self.started.elapsed().as_secs_f32(),
            "frames": self.frames,
            "play_cycles": self.play_cycles,
            "churn": self.churn.as_ref().map(|load| {
                serde_json::json!({
                    "every_seconds": load.every.as_secs(),
                    "drop_every_rounds": load.drop_every,
                    "rounds": load.rounds,
                    "dock_churns": load.dock_churns,
                    "drop_imports": load.drop_imports,
                    "drop_cleanups": load.drop_cleanups,
                    "skipped": load.skipped,
                    "source": load.source.as_ref().map(|path| path.display().to_string()),
                    "recent_notes": load.recent,
                })
            }),
            "head_mean_mib": head,
            "tail_mean_mib": tail,
            "growth_mib": growth,
            "failed": self.failed,
            "samples": samples,
        }))
        .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"));
        let _ = fs::write(self.out_dir.join("soak.json"), json);
        if complete {
            tracing::info!(
                target: "editor",
                "[soak] 完成：{} 秒 / {} 帧 / Play 循环 {} / 内存首尾均值 {:.1}→{:.1} MiB",
                self.started.elapsed().as_secs(),
                self.frames,
                self.play_cycles,
                head,
                tail
            );
        } else {
            tracing::info!(
                target: "editor",
                "[soak] 中途快照：{} 秒 / {} 采样 / 内存 {:.1} MiB",
                self.started.elapsed().as_secs(),
                self.samples.len(),
                self.samples.last().map_or(0.0, |sample| sample.working_set_mib)
            );
        }
    }
}

/// 当前文档的实体数：采样的一列，也是"撤销有没有真的落地"的判据。
fn scene_entity_count(app: &EditorApp) -> usize {
    app.document
        .current_scene
        .as_ref()
        .map_or(0, |document| document.scene.entities.len())
}

/// soak 拖放负载可入库的扩展名（纹理 + 模型：能建实体、也能走复制与缩略图）。
const DROP_SOURCE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tga", "webp", "glb"];

/// 在**工程外**找一个真实可导入的源文件。
///
/// 必须落在工程根之外：`dnd::target_path` 对工程内的文件走"只引用不复制"，那条路径既不复制
/// 也不分配 GUID，拿它当负载等于没测 M27-01 的入库链。候选按示例工程的 `assets` 排，从工程根
/// 和当前工作目录各自向上找 4 层（`cargo run` 时工作目录在 `engine/`，仓库里的示例图就够得着，
/// 不必为测试造临时文件）。找不到返回 `None`：soak 照跑，只是不含拖放导入。
fn find_external_drop_source(root: &Path) -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("RUSTLUA_EDITOR_SOAK_DROP_SOURCE") {
        let path = PathBuf::from(&raw);
        return if path.is_file() && !path_inside(root, &path) {
            Some(path)
        } else {
            None
        };
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for start in [root.to_path_buf(), cwd] {
        let mut cursor = start.parent().map(PathBuf::from);
        for _ in 0..4 {
            let Some(dir) = cursor else { break };
            for candidate in [
                dir.join("examples/game_api/assets"),
                dir.join("examples/game_jhyt/assets"),
                dir.join("assets"),
            ] {
                if let Some(found) = first_drop_source_in(&candidate, root, 3) {
                    return Some(found);
                }
            }
            cursor = dir.parent().map(PathBuf::from);
        }
    }
    None
}

/// 在 `dir` 下按字典序递归找第一个可入库的文件（`depth` 限死层数，跳过本地工作目录）。
fn first_drop_source_in(dir: &Path, root: &Path, depth: u8) -> Option<PathBuf> {
    if depth == 0 || !dir.is_dir() {
        return None;
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            let name = path
                .file_name()
                .map_or(String::new(), |raw| raw.to_string_lossy().into_owned());
            if matches!(
                name.as_str(),
                ".cache" | ".temp" | ".git" | "target" | "builds"
            ) {
                continue;
            }
            if let Some(found) = first_drop_source_in(&path, root, depth - 1) {
                return Some(found);
            }
            continue;
        }
        let extension = path
            .extension()
            .map(|raw| raw.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if DROP_SOURCE_EXTENSIONS.contains(&extension.as_str()) && !path_inside(root, &path) {
            return Some(path);
        }
    }
    None
}

/// `path` 是否位于 `root` 之内。工程根可能来自 `--project` 的相对写法，而候选是绝对路径，
/// 所以两边各自 canonicalize 再比前缀（拿不到 canonical 时退化成原始比较）。
fn path_inside(root: &Path, path: &Path) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path.starts_with(&root)
}

/// 一段窗口内的内存与纹理统计（长会话的判据单位）。
#[derive(Clone, Copy, Debug, Default)]
struct MemoryWindow {
    min_mib: f64,
    p50_mib: f64,
    max_mib: f64,
    min_texture_mib: f64,
    max_texture_mib: f64,
}

/// 把采样按**时长**切成 `windows` 段（不是按条数：采样间隔变化时按条数切会歪），
/// 每段给出内存的 min/p50/max 与纹理字节的 min/max。
fn memory_windows(samples: &[SoakSample], windows: usize) -> Vec<MemoryWindow> {
    if samples.is_empty() || windows == 0 {
        return Vec::new();
    }
    let start = samples[0].elapsed_seconds;
    let end = samples[samples.len() - 1].elapsed_seconds;
    let span = (end - start).max(1.0);
    let windows = windows.min(samples.len());
    let mut out = Vec::with_capacity(windows);
    for index in 0..windows {
        let low = start + span * index as f32 / windows as f32;
        let high = start + span * (index + 1) as f32 / windows as f32;
        let mut values: Vec<f64> = Vec::new();
        let mut textures: Vec<f64> = Vec::new();
        for sample in samples {
            let inside = if index + 1 == windows {
                sample.elapsed_seconds >= low && sample.elapsed_seconds <= high
            } else {
                sample.elapsed_seconds >= low && sample.elapsed_seconds < high
            };
            if inside {
                values.push(sample.working_set_mib);
                textures.push(sample.egui_texture_bytes as f64 / (1024.0 * 1024.0));
            }
        }
        if values.is_empty() {
            continue;
        }
        values.sort_by(f64::total_cmp);
        textures.sort_by(f64::total_cmp);
        out.push(MemoryWindow {
            min_mib: values[0],
            p50_mib: values[values.len() / 2],
            max_mib: values[values.len() - 1],
            min_texture_mib: textures[0],
            max_texture_mib: textures[textures.len() - 1],
        });
    }
    out
}

/// 首尾各 10% 采样的均值（采样不足 10 个时退化为"前一半 / 后一半"）。
fn window_means(samples: &[SoakSample]) -> (f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let window = (samples.len() / 10).max(1);
    let head: f64 = samples[..window]
        .iter()
        .map(|sample| sample.working_set_mib)
        .sum::<f64>()
        / window as f64;
    let tail_slice = &samples[samples.len() - window..];
    let tail: f64 = tail_slice
        .iter()
        .map(|sample| sample.working_set_mib)
        .sum::<f64>()
        / window as f64;
    (head, tail)
}

/// 当前进程工作集（MiB）。非 Windows 平台返回 0（报告里明确说明这是缺失数据）。
pub(crate) fn working_set_mib() -> f64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: `GetCurrentProcess` 返回伪句柄，不需要也不能关闭；
        // `PROCESS_MEMORY_COUNTERS` 结构体按文档先写 `cb` 再传入，全部是栈上内存。
        unsafe {
            let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            let ok = GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            );
            if ok == 0 {
                return 0.0;
            }
            counters.WorkingSetSize as f64 / (1024.0 * 1024.0)
        }
    }
    #[cfg(not(windows))]
    {
        0.0
    }
}

/// 把证据目录清空重建（避免上一轮的 PNG 混进这一轮的归档）。
pub(crate) fn prepare_output_dir(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_script_covers_three_dpis_and_resizes() {
        let steps = evidence_steps();
        let dpis: Vec<f32> = {
            let mut values: Vec<f32> = steps.iter().map(|step| step.pixels_per_point).collect();
            values.sort_by(|a, b| a.total_cmp(b));
            values.dedup();
            values
        };
        assert_eq!(dpis, vec![1.0, 1.5, 2.0], "100/150/200% 三档 DPI 都要采");
        assert!(
            steps.iter().any(|step| step.play),
            "必须有一张 Play 下的内嵌预览截图"
        );
        assert!(
            steps.iter().any(|step| step.view_3d),
            "必须有一张 3D 视口截图"
        );
        let sizes: std::collections::BTreeSet<(i32, i32)> = steps
            .iter()
            .map(|step| (step.inner_size.x as i32, step.inner_size.y as i32))
            .collect();
        assert!(sizes.len() >= 3, "至少覆盖三种窗口尺寸：{sizes:?}");
    }

    #[test]
    fn memory_windows_splits_by_time_and_reports_troughs() {
        // 一条"锯齿但谷值不抬升"的序列：均值会上下跳，谷值必须平稳。
        // 锯齿周期（8 个采样）**短于**窗口长度（80/8 = 10 个采样），保证每个窗口都能看到谷与峰。
        let samples: Vec<SoakSample> = (0..80)
            .map(|index| {
                let seconds = index as f32 * 10.0;
                let sawtooth = if index % 8 < 4 { 0.0 } else { 120.0 };
                SoakSample {
                    elapsed_seconds: seconds,
                    working_set_mib: 300.0 + sawtooth,
                    egui_textures: 10,
                    egui_texture_bytes: 1024 * 1024,
                    playing: false,
                    frames: index as u64,
                    entities: 17,
                    undo_entries: 0,
                    console_lines: 500,
                    churn_rounds: 0,
                }
            })
            .collect();
        let windows = memory_windows(&samples, 8);
        assert_eq!(windows.len(), 8);
        // 每个窗口都覆盖到一段锯齿的低谷 → 谷值应当在 300 附近且**不抬升**。
        for window in &windows {
            assert!(
                (window.min_mib - 300.0).abs() < 1e-6,
                "谷值必须落在锯齿低点：{window:?}"
            );
            assert!(window.max_mib > 350.0, "窗口内应当看到锯齿高点");
        }
        assert!(
            (windows[7].min_mib - windows[0].min_mib).abs() < 1e-6,
            "无泄漏时首尾窗口谷值应当一致"
        );
    }

    #[test]
    fn memory_windows_detect_a_rising_trough() {
        // 持续泄漏：每个采样都比上一个高 1 MiB → 谷值必然抬升。
        let samples: Vec<SoakSample> = (0..40)
            .map(|index| SoakSample {
                elapsed_seconds: index as f32 * 10.0,
                working_set_mib: 300.0 + index as f64,
                egui_textures: 10,
                egui_texture_bytes: 1024 * 1024,
                playing: false,
                frames: index as u64,
                entities: 17,
                undo_entries: 0,
                console_lines: 500,
                churn_rounds: 0,
            })
            .collect();
        let windows = memory_windows(&samples, 8);
        let growth = windows[windows.len() - 1].min_mib - windows[0].min_mib;
        assert!(growth > 30.0, "持续泄漏必须体现在谷值抬升上：{growth}");
    }

    #[test]
    fn window_means_handles_short_and_empty_series() {
        assert_eq!(window_means(&[]), (0.0, 0.0));
        let samples: Vec<SoakSample> = (0..4)
            .map(|index| SoakSample {
                elapsed_seconds: index as f32,
                working_set_mib: if index < 2 { 100.0 } else { 130.0 },
                egui_textures: 1,
                egui_texture_bytes: 1024,
                playing: false,
                frames: 0,
                entities: 1,
                undo_entries: 0,
                console_lines: 0,
                churn_rounds: 0,
            })
            .collect();
        let (head, tail) = window_means(&samples);
        assert!((head - 100.0).abs() < f64::EPSILON, "head={head}");
        assert!((tail - 130.0).abs() < f64::EPSILON, "tail={tail}");
    }

    #[test]
    fn working_set_is_readable_on_this_platform() {
        let mib = working_set_mib();
        if cfg!(windows) {
            assert!(mib > 1.0, "进程工作集应当是可读的正数，实测 {mib}");
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("m27-soak-{label}-{}", crate::util::test_token()))
    }

    fn copy_tree(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).expect("建目录");
        for entry in fs::read_dir(source).expect("读目录").flatten() {
            let from = entry.path();
            let to = destination.join(entry.file_name());
            if entry.file_type().expect("文件类型").is_dir() {
                copy_tree(&from, &to);
            } else {
                if let Some(parent) = to.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                fs::copy(&from, &to).expect("复制文件");
            }
        }
    }

    /// 把示例工程整份拷进临时目录：soak 负载会往工程里复制资源，
    /// 直接在 `examples/game_api` 上跑等于把导入产物写进仓库。
    fn copy_sample_project(label: &str) -> PathBuf {
        let source = engine::core::games_dir().join("examples").join("game_api");
        let root = temp_root(label);
        fs::create_dir_all(&root).expect("建临时工程");
        for dir in ["assets", "scenes", "scripts", "data"] {
            if source.join(dir).is_dir() {
                copy_tree(&source.join(dir), &root.join(dir));
            }
        }
        fs::copy(source.join("project.toml"), root.join("project.toml")).expect("拷 project.toml");
        root
    }

    fn file_count(dir: &Path) -> usize {
        if !dir.is_dir() {
            return 0;
        }
        fs::read_dir(dir)
            .expect("读目录")
            .flatten()
            .map(|entry| {
                let path = entry.path();
                if path.is_dir() { file_count(&path) } else { 1 }
            })
            .sum()
    }

    /// 装配一份"每帧都做一轮负载、每轮都拖放导入"的 soak（`every = 0` 是刻意的：
    /// 测试等不起真实间隔，真实间隔由长跑本身验证）。
    fn soak_with_churn(root: &Path, drop_every: u64) -> GuiSoak {
        let source =
            find_external_drop_source(root).expect("仓库示例里应当有工程外的纹理/模型可作为拖放源");
        GuiSoak {
            out_dir: temp_root("soak-report"),
            duration: Duration::from_secs(3_600),
            play_seconds: Duration::from_secs(2),
            step_seconds: Duration::from_secs(1),
            started: Instant::now(),
            last_sample: Instant::now(),
            playing_since: None,
            samples: Vec::new(),
            frames: 0,
            play_cycles: 0,
            finished: false,
            failed: false,
            prepared: true,
            churn: Some(ChurnLoad {
                every: Duration::ZERO,
                last: Instant::now(),
                drop_every,
                source: Some(source),
                pending_copy: None,
                rounds: 0,
                dock_churns: 0,
                drop_imports: 0,
                drop_cleanups: 0,
                skipped: 0,
                recent: Vec::new(),
            }),
        }
    }

    /// M27-03 验收 1 的负载半边钉三条：① 每轮都真的改了 dock 树（`skipped == 0`）；
    /// ② 撤销 + 删副本确实把"同一份工作"钉成常量——实体数与 `assets/` 文件数
    /// 不随轮数增长（不然曲线量到的是"资产变多"，泄漏判据当场失去意义）；
    /// ③ 负载计数与逐条留痕真的写进了报告。
    #[test]
    fn churn_load_repeats_the_same_work_without_growing_the_project() {
        let root = copy_sample_project("churn");
        let project = editor::open_project(&root).expect("打开临时工程");
        let mut app = EditorApp::new(project, None, engine::core::logging::LogSink::new(256));
        app.project_loaded = true;
        app.open_scene_resource(PathBuf::from("scenes/main.scene"));
        assert!(
            app.document.current_scene.is_some(),
            "soak 负载要一个打开的文档：{}",
            app.status
        );

        let mut soak = soak_with_churn(&root, 1);
        let source = soak
            .churn
            .as_ref()
            .and_then(|load| load.source.clone())
            .expect("装配负载时已经定好源文件");
        assert!(
            !path_inside(&root, &source),
            "拖放源必须落在工程外，否则走的是『只引用不复制』：{}",
            source.display()
        );

        let entities = scene_entity_count(&app);
        let assets = file_count(&root.join("assets"));
        for _ in 0..30 {
            soak.run_churn_round(&mut app, Instant::now());
        }
        let load = soak.churn.as_ref().expect("负载在装配时就有");

        assert_eq!(load.rounds, 30, "30 次调用就该有 30 轮负载");
        assert_eq!(load.skipped, 0, "有轮次在空转：{}", load.recent.join(" / "));
        assert_eq!(load.dock_churns, 30, "每轮都必须改一次 dock 树");
        assert_eq!(load.drop_imports, 30, "drop_every=1 时每轮都应导入一次");
        assert_eq!(load.drop_cleanups, 29, "第 1 轮没有上一轮的副本可删");
        assert_eq!(scene_entity_count(&app), entities, "撤销没把实体数钉回基线");
        assert!(
            file_count(&root.join("assets")) <= assets + 1,
            "副本没删干净：导入前 {} 个，30 轮后 {}",
            assets,
            file_count(&root.join("assets"))
        );
        assert_eq!(load.recent.len(), 12, "最近留痕应当只留 12 条");

        soak.write_report(&mut app, true);
        let text = fs::read_to_string(soak.out_dir.join("soak.txt")).expect("读 soak.txt");
        assert!(
            text.contains("churn: every_seconds=0 drop_every_rounds=1 rounds=30"),
            "报告头部没有负载计数：{text}"
        );
        assert!(
            text.contains("churn_notes(last=12):"),
            "报告没有逐条留痕：{text}"
        );
        assert!(
            text.contains("churn_rounds\n"),
            "采样表缺 churn_rounds 列：{text}"
        );
        let json = fs::read_to_string(soak.out_dir.join("soak.json")).expect("读 soak.json");
        assert!(
            json.contains("\"drop_imports\": 30"),
            "soak.json 缺负载计数：{json}"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&soak.out_dir);
    }

    /// 负载是**间隔到点才做**：把间隔拉成一小时后连续调用必须原地不动。
    /// 这一条守的是可比性——负载若无条件每帧生效，"带负载的这一档"就没法和
    /// docs/55 阶段 0 那两档空转归档放在一起看。
    #[test]
    fn churn_rounds_wait_for_their_interval() {
        let root = temp_root("cadence");
        fs::create_dir_all(&root).expect("建临时工程");
        for dir in ["assets", "scenes", "scripts"] {
            fs::create_dir_all(root.join(dir)).expect("建工程目录");
        }
        fs::write(root.join("project.toml"), "[project]\nname = \"demo\"\n")
            .expect("写 project.toml");
        let project = editor::open_project(&root).expect("打开空工程");
        let mut app = EditorApp::new(project, None, engine::core::logging::LogSink::new(16));
        app.project_loaded = true;
        let mut soak = soak_with_churn(&root, 1);
        soak.churn.as_mut().expect("装配好的负载").every = Duration::from_secs(3_600);
        for _ in 0..5 {
            soak.run_churn_round(&mut app, Instant::now());
        }
        let load = soak.churn.as_ref().expect("负载在装配时就有");
        assert_eq!(load.rounds, 0, "间隔未到却做了负载");
        assert!(load.recent.is_empty(), "没做负载就不该留痕");

        let _ = fs::remove_dir_all(&root);
    }
}
