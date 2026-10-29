//! Application shell: owns the window, renderer, runtime scene and input
//! snapshot; drives the fixed-timestep simulation and frame redraw; maps
//! winit events to engine input.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use rustlua_camera::camera::{FrameViews, Viewport};
use rustlua_ecs::scheduler::ScheduleStage;
use rustlua_infra::logging::{EngineError, LogModule};
use rustlua_infra::performance::{PerformanceSnapshot, PerformanceStats};
use rustlua_infra::time_step::{FixedTimeStep, StepReport};
use rustlua_infra::window::{AppLifecycle, has_renderable_size};
use rustlua_input::input::{InputSnapshot, MouseButton};
use winit::{
    application::ApplicationHandler,
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    keyboard::ModifiersState,
    window::{Icon, Window, WindowId},
};

use engine::host::{
    FrameProfile, OverlayVertex, RuntimeScene, SceneDraw, SceneMesh, ScriptErrorPolicy,
    apply_scene_environment,
};
use rustlua_gpu::{RenderOutcome, Renderer};

pub struct GameApp {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    lifecycle: AppLifecycle,
    occluded: bool,
    time_step: FixedTimeStep,
    input: InputSnapshot,
    /// 上一次真正下发给窗口的输入法开关（M79-04）。
    ///
    /// 只在**变化**时调用 `set_ime_allowed`：Windows 那侧每帧重复下发会打断正在进行的
    /// 组合（打一半的拼音消失），而 `ime_allowed` 默认是 `false`——不主动开就只有
    /// 键盘 Unicode 字符，中文输入法拿不到。
    ime_allowed: bool,
    /// 最近一次 `WindowEvent::ModifiersChanged` 的修饰键状态（M79-04）。
    ///
    /// winit 0.30.13 把修饰键从 `KeyEvent` 里**移走了**（六个字段里没有 `modifiers`），
    /// 只在独立的 `ModifiersChanged` 事件里下发，所以收字时要自己留一份——Ctrl+A 是否
    /// 「全选命令」只能这么判。
    modifiers: ModifiersState,
    last_frame: Instant,
    performance: PerformanceStats,
    /// 调试 overlay（帧率/耗时文本）是否绘制。**默认关闭**：发布包不应带开发者
    /// 读数，游戏内按 `F3` 可随时开启/关闭（诊断用）。数值本身仍每帧统计并写
    /// `[perf]` 日志，因此关掉绘制不影响性能基线。
    overlay_enabled: bool,
    runtime_scene: Option<RuntimeScene>,
    /// 运行期失败原因（Lua 固定步异常 / 渲染致命错误 / 场景或渲染器初始化失败）。
    /// 非空时宿主进程必须以**非零退出码**结束：这些路径此前只调用
    /// `event_loop.exit()`，`main` 随后返回 `Ok(())`，于是崩溃在 CI、发布脚本和
    /// 自动化验收里都表现为成功。
    failure: Option<String>,
    project_root: Option<PathBuf>,
    window_title: String,
    /// P3：发布包模式（`--pack`）⇒ 存档与玩家设置写用户数据目录。
    packaged: bool,
    /// CPU frustum culling (M8-08); off via `--no-cull` for baselines.
    cull_enabled: bool,
    /// Same mesh/material instancing (M8-08); off via `--no-instance`.
    instancing_enabled: bool,
    splash: Option<crate::splash::Splash>,
    /// GPU 就绪之前的启动图（GDI 子窗口）。首帧 `Presented` 之后撤下，
    /// 由 `splash` 接管同一张图；见 `crate::boot_splash`。
    boot_splash: Option<crate::boot_splash::BootSplash>,
    deferred_verify: bool,
    /// M74-02 P0：帧调度分档累加器（`--frame-profile` 打开；`None` = 不记，零成本）。
    frame_profile: Option<FrameProfile>,
    /// 分档报告的落盘路径（与 `frame_profile` 同生同灭）。
    frame_profile_path: Option<PathBuf>,
    /// P3-R05-4：开发期 shader 覆盖目录（`--shader-dir`/`RUSTLUA_SHADER_DIR`）。
    /// `None` = 不监听，`Renderer` 只用编译期内嵌的源。
    shader_dir: Option<PathBuf>,
    /// 下一次检查覆盖目录的时刻（`None` = 不检查）。每次检查都往后推一个去抖窗口。
    shader_reload_due: Option<Instant>,
    /// 已经打过的重载失败文本：同一条坏源每 150ms 重复报一次会把日志刷废。
    shader_error_shown: Option<String>,
    /// P0（脚本迭代闭环）：Lua 磁盘检查的去抖节律（节律与文案与编辑器 Play 共用
    /// [`rustlua_infra::hot_reload`]，两条宿主不再各写一套）。
    script_cadence: rustlua_infra::hot_reload::ReloadCadence,
    /// 工程脚本目录；`None` = **不轮询**（发布包 / 无工程根 / 项目里没有脚本层）。
    scripts_dir: Option<PathBuf>,
    /// 已经打过的脚本重载结果文本：同一条报错每 250ms 重复一次会把日志刷废。
    script_note_shown: Option<String>,
    /// 设计分辨率（`project.toml` 的 `window_width`/`window_height`，缺省 1280×720）。
    /// B 阶段（contain 等比适配）：运行期的**逻辑屏幕空间**固定为设计分辨率——
    /// 场景/UI 顶点、2D 相机与声明视口全按它烘焙，渲染器再 `contain_rect` 到窗口
    /// （黑边居中）。区别于窗口物理尺寸（`renderer.config` / `inner_size`）。
    design_size: (u32, u32),
}

impl GameApp {
    /// 构造运行器。`packaged` = 是否发布包（`--pack`）：发布包把存档与玩家设置写到
    /// 用户数据目录（见 `rustlua_infra::user_dir`），而不是程序目录。
    pub fn new(
        project_root: Option<PathBuf>,
        window_title: String,
        cull_enabled: bool,
        instancing_enabled: bool,
        packaged: bool,
        deferred_verify: bool,
        frame_profile_path: Option<PathBuf>,
    ) -> Self {
        // P0（脚本迭代闭环）：开发期才轮询磁盘 Lua。发布包（`--pack`）不轮询——包内脚本
        // 来自 bundle，改盘上的 `scripts/` 不该改变发行产物；没有脚本目录的工程则一次都不查。
        let scripts_dir = if packaged {
            None
        } else {
            project_root
                .as_deref()
                .map(|root| root.join("scripts"))
                .filter(|dir| dir.is_dir())
        };
        Self {
            window: None,
            renderer: None,
            lifecycle: AppLifecycle::Initializing,
            occluded: false,
            time_step: FixedTimeStep::new(60, 8).expect("valid fixed timestep"),
            input: InputSnapshot::default(),
            ime_allowed: false,
            modifiers: ModifiersState::empty(),
            last_frame: Instant::now(),
            performance: PerformanceStats::new(),
            // 发布默认不带帧率 overlay（F3 可手动开启，见字段注释）。
            overlay_enabled: false,
            runtime_scene: None,
            failure: None,
            project_root,
            window_title,
            packaged,
            cull_enabled,
            instancing_enabled,
            splash: None,
            boot_splash: None,
            deferred_verify,
            frame_profile: frame_profile_path.as_ref().map(|_| {
                let mut profile = FrameProfile::new();
                // `Script`（Lua）永远串行：不标独占会让真实工程把 76% 读成"该并行"
                // （见 M74-02 §实现记录）。
                profile.set_exclusive_stages([ScheduleStage::Script]);
                profile
            }),
            frame_profile_path,
            shader_dir: None,
            shader_reload_due: None,
            shader_error_shown: None,
            script_cadence: rustlua_infra::hot_reload::ReloadCadence::script_default(),
            scripts_dir,
            script_note_shown: None,
            design_size: rustlua_infra::project_config::DEFAULT_WINDOW_SIZE,
        }
    }

    /// P3-R05-4：接上开发期 shader 覆盖目录（`None` = 保持内置源，不监听）。
    ///
    /// 单独一个构造后置方法，而不是再加一个 `new` 参数：`new` 已经在 7 个参数的位置，
    /// 再塞第 8 个只是为了让签名更难读。
    #[must_use]
    pub fn with_shader_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.shader_dir = dir;
        self
    }

    fn initialize_runtime(&mut self, event_loop: &ActiveEventLoop) {
        let root = self.project_root.as_deref();
        if self.deferred_verify {
            if let Some(root) = root
                && let Err(error) = crate::cli::verify_startup(root, self.packaged)
            {
                self.fail(event_loop, error);
                return;
            }
            self.deferred_verify = false;
        }
        self.runtime_scene = match RuntimeScene::new_with_layout(root, self.packaged) {
            Ok(mut scene) => {
                scene.set_script_error_policy(ScriptErrorPolicy::Contain);
                Some(scene)
            }
            Err(error) => {
                let diagnostic = EngineError::new(
                    LogModule::Script,
                    "initialize runtime scene",
                    "runtime scene or project script failed to start",
                )
                .with_source(&error)
                .with_suggestion("check the project scene and scripts/main.lua");
                tracing::error!(target: "game", error = %diagnostic, "runtime initialization failed");
                self.fail(
                    event_loop,
                    format!("runtime scene initialization failed: {error}"),
                );
                return;
            }
        };
        if let (Some(root), Some(scene)) = (root, self.runtime_scene.as_mut()) {
            let settings_root =
                rustlua_infra::user_dir::data_roots(root, self.packaged).settings_dir;
            let (deadzone, diagnostic) =
                rustlua_gameplay::settings::load_deadzone_section(&settings_root);
            if let Some(diagnostic) = diagnostic {
                tracing::warn!(target: "input", "[gamepad] {diagnostic}");
            }
            scene.set_gamepad_deadzone(deadzone);
            tracing::info!(target: "input", stick = deadzone.stick, trigger = deadzone.trigger, "[gamepad] deadzone applied");
        }
        // P3-R03-4：场景声明的 `Environment` 在启动时应用一次（只在这里上传 cubemap；
        // 每帧不重复绑定）。没有声明时不动渲染设置，旧场景像素不受影响。
        if let (Some(renderer), Some(scene)) = (self.renderer.as_mut(), self.runtime_scene.as_ref())
            && let Some(report) = scene.environment_report()
        {
            let applied = apply_scene_environment(renderer, scene, &report);
            if let Some(reason) = &applied.fallback_reason {
                tracing::warn!(target: "render", "[render] {reason}");
            }
            tracing::info!(
                target: "render",
                map = %report.resolved_path,
                face_size = report.face_size,
                mip_count = report.mip_count,
                ibl = applied.ibl_mode.as_str(),
                tier = applied.tier.as_str(),
                "[render] environment applied"
            );
        }
        // P3-R07-1：把渲染器的 compute 能力告知运行时，场景发射器的落点判定才说得出真话
        // （`mode=gpu` 在无 compute 的设备上是 `no_compute` 回退，而不是假装能用 GPU）。
        // 一次赋值，无 IO；没有 gpu 模式发射器时运行时只是多存一个布尔。
        if let (Some(renderer), Some(scene)) = (self.renderer.as_ref(), self.runtime_scene.as_mut())
        {
            scene.set_particle_compute_available(renderer.compute_enabled());
        }
        self.input.clear_all();
        self.last_frame = Instant::now();
        self.transition(AppLifecycle::Running);
        tracing::info!(target: "game", "[perf] overlay {} (F3 toggles the fps/latency overlay)", if self.overlay_enabled { "enabled" } else { "disabled" });
        self.window
            .as_ref()
            .expect("window exists")
            .request_redraw();
    }

    fn transition(&mut self, next: AppLifecycle) {
        if let Err(error) = self.lifecycle.transition(next) {
            tracing::info!(target: "game", "[lifecycle] {error}");
            return;
        }
        tracing::info!(target: "game", "[lifecycle] {:?}", self.lifecycle);
    }

    fn request_exit(&mut self, event_loop: &ActiveEventLoop) {
        if matches!(self.lifecycle, AppLifecycle::Running | AppLifecycle::Paused) {
            self.transition(AppLifecycle::Exiting);
        }
        event_loop.exit();
        if self.lifecycle == AppLifecycle::Exiting {
            self.transition(AppLifecycle::Exited);
        }
    }

    /// 记录一次**失败**退出：首个原因胜出（后续失败通常是它的连锁反应），然后
    /// 走正常的退出流程。`main` 在事件循环结束后读取它并返回 `Err`，从而让进程
    /// 退出码非零。
    fn fail(&mut self, event_loop: &ActiveEventLoop, reason: impl Into<String>) {
        let reason = reason.into();
        if self.failure.is_none() {
            self.failure = Some(reason);
        }
        self.request_exit(event_loop);
    }

    /// 本次运行的失败原因；`None` 表示正常退出（含 Lua `app.quit()` 与用户关窗）。
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// M74-02 P0：把本进程的帧调度分档写成 JSON，并打印单行摘要。
    ///
    /// 与 `--trace-loads` 同一出口形态：单行 `m74_frame_profile ...` 给脚本与 CI 读，
    /// JSON 给台账存档。未开 `--frame-profile`、或一帧都没采样到时什么都不写——
    /// 一份 `frames=0` 的报告只会让下游把"没量到"读成"占比 0%"。
    pub fn finish_frame_profile(&mut self) -> Option<PathBuf> {
        let summary = self.frame_profile.as_ref()?.summary();
        let path = self.frame_profile_path.clone()?;
        if summary.frames == 0 {
            eprintln!("game_frame_profile_skipped: 没有采样到任何帧");
            return None;
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, format!("{}\n", summary.to_json())) {
            Ok(()) => {
                println!("game_frame_profile_path={}", path.display());
                println!("{}", summary.summary_line());
                Some(path)
            }
            Err(error) => {
                eprintln!("game_frame_profile_failed: {error}");
                None
            }
        }
    }

    /// 窗口当前能不能出画：被遮挡或尺寸为 0（最小化）时都算不能。
    ///
    /// 这两种情况下 Windows 的 present 要么挂住、要么没人看，所以只停**画面**——
    /// 逻辑仍由 [`Self::about_to_wait`] 的定时器按 60Hz 推进：联机时一端被别的窗口
    /// 盖住，不能让对端跟着等。
    fn renderable(&self) -> bool {
        if self.occluded {
            return false;
        }
        self.window.as_ref().is_some_and(|window| {
            let size = window.inner_size();
            has_renderable_size(size.width, size.height)
        })
    }

    /// 尺寸/遮挡变化后同步渲染状态：能画就立刻补一帧（恢复可见要马上出画面），
    /// 不能画就只记一行日志，仿真照跑。
    fn sync_render_state(&mut self, size: PhysicalSize<u32>) {
        if self.occluded || !has_renderable_size(size.width, size.height) {
            tracing::info!(target: "game",
                "[window] render suppressed: size={}x{} occluded={}; simulation keeps ticking",
                size.width, size.height, self.occluded
            );
            return;
        }
        tracing::info!(target: "game", "[window] renderable size: {}x{}", size.width, size.height);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// 把脚本的收字声明（`input.text_events{ capture = true }`）下发给窗口。
    ///
    /// * 读的是 `RuntimeScene`（脚本写的那份共享状态），不是宿主自己的快照：写这个位的
    ///   是 Lua，宿主那份只负责往里放 winit 送来的文本事件。
    /// * **只在变化时调用** `set_ime_allowed`：每帧重复下发会让 Windows 输入法重置组合，
    ///   打一半的拼音直接消失。
    /// * 没有 `RuntimeScene`（初始化失败/未启动）时按"不收字"处理，输入法保持交还系统。
    fn sync_ime_allowed(&mut self) {
        let wanted = self
            .runtime_scene
            .as_ref()
            .is_some_and(RuntimeScene::text_capture_requested);
        if wanted == self.ime_allowed {
            return;
        }
        self.ime_allowed = wanted;
        if let Some(window) = self.window.as_ref() {
            window.set_ime_allowed(wanted);
            tracing::debug!(target: "game", "[input] ime_allowed={wanted}");
        }
    }

    /// 按墙钟推进固定步时钟，返回本次调度结果。
    ///
    /// 渲染帧与被遮挡时的定时器共用这一个入口：`elapsed` 一律取"距上一次推进"的墙钟，
    /// 谁先到谁推，同一段时间不会被跑两遍。
    fn advance_clock(&mut self) -> StepReport {
        // 每帧清零：`lua_ms` 是"本次推进里 Lua update() 的耗时"，不是累计值。
        self.performance.reset_lua_sample();
        let elapsed = self.last_frame.elapsed().as_secs_f64();
        self.last_frame = Instant::now();
        // P0（脚本迭代闭环）：先查磁盘 Lua 再跑本帧固定步——重载成功的那份脚本立刻参与
        // 这一帧的 `update()`，用户感到的延迟只剩一次去抖。
        self.poll_script_hot_reload(Duration::from_secs_f64(elapsed));
        let report = self.time_step.advance(elapsed);
        if report.dropped_steps > 0 {
            tracing::warn!(target: "engine", "[time] dropped catch-up steps: dropped={} total_dropped={} accumulator={:.6}s", report.dropped_steps, report.total_dropped_steps, report.accumulator_seconds);
        }
        if report.steps > 0 && report.total_steps % 60 < u64::from(report.steps) {
            tracing::info!(target: "engine", "[time] fixed-step stats: steps={} dt={:.6}s accumulator={:.6}s total_steps={} total_dropped={}", report.steps, report.delta_seconds, report.accumulator_seconds, report.total_steps, report.total_dropped_steps);
        }
        // M79-04：脚本的收字声明要变成窗口级的输入法开关，本帧生效、只在下发变化时动它。
        self.sync_ime_allowed();
        report
    }

    fn run_simulation_steps(
        &mut self,
        steps: u32,
        delta_seconds: f32,
        event_loop: &ActiveEventLoop,
    ) {
        let input = self.input.clone();
        self.performance.begin_update();
        let result = self
            .runtime_scene
            .as_mut()
            .map(|scene| scene.run_script_steps(steps, delta_seconds, &input));
        self.performance.end_update();
        // Lua 耗时：由运行时在 Lua 体内部测量后取走，只含 `update()` 本体
        // （不含 ECS/物理/渲染），所以 `lua_ms` 与指标名一致。0 步帧取到 0。
        if let Some(scene) = self.runtime_scene.as_mut() {
            let micros = scene.take_script_micros();
            self.performance.record_lua_millis(micros as f32 / 1000.0);
        }
        // P3-R07-2：脚本步之后、渲染之前，把本帧 Lua 排队的 `mode='gpu'` burst 交给渲染器
        // 派发，并推进 GPU 粒子池一次（`dispatch_gpu_particle_bursts` 内部 step + 回收）。
        // 放在这里而不是渲染路径里，是因为派发要读**这一帧**的脚本产物，而渲染函数拿不到 scene。
        // 失败帧不派发：脚本整体异常时实体/队列状态不可信，下一帧的入口保底会把它们送回 CPU 池。
        let script_succeeded = result.as_ref().is_some_and(std::result::Result::is_ok);
        if script_succeeded
            && let (Some(renderer), Some(scene)) =
                (self.renderer.as_mut(), self.runtime_scene.as_mut())
        {
            let dispatch = scene.dispatch_gpu_particle_bursts(renderer, delta_seconds);
            if dispatch.requested > 0 {
                tracing::debug!(
                    target: "render",
                    requested = dispatch.requested,
                    gpu = dispatch.gpu,
                    fallback = dispatch.fallback,
                    deferred = dispatch.deferred,
                    systems = dispatch.systems,
                    gpu_particles = dispatch.gpu_particles,
                    reason = dispatch.fallback_reason.map_or("none", |reason| reason.as_str()),
                    "[particle] gpu burst 派发"
                );
            }
        }
        if let Some(Err(error)) = result {
            // 容错档下这里只可能是**引擎侧**失败（场景/输入/资源），因为 Lua 回调错误已在
            // 脚本层被记账并继续（见 `set_script_error_policy`）。这类失败仍然必须非零退出：
            // 静默带病运行会让 CI 与发布自测把"跑过了"当"跑对了"。
            let diagnostic = EngineError::new(
                LogModule::Script,
                "fixed-step update",
                "project Lua update function",
            )
            .with_source(&error)
            .with_suggestion("fix the Lua runtime error reported above");
            tracing::error!(target: "script", error = %diagnostic, "fixed-step update failed");
            self.fail(event_loop, format!("fixed-step Lua update failed: {error}"));
        }
        // Lua `app.quit()`：只在 fixed-step 更新后消费退出标记，避免在无
        // runtime_scene（初始化/异常）时误退。
        if let Some(scene) = self.runtime_scene.as_ref()
            && scene.quit_requested()
        {
            self.request_exit(event_loop);
        }
    }

    /// P3-R05-4：把覆盖目录接到刚建好的渲染器上（只有显式开启时才做一次）。
    ///
    /// 失败**不**阻止启动，也**不**关监听：项目里一份坏 `.wgsl` 只该让画面停在旧管线上并
    /// 把原因写进日志；作者改好文件后下一次轮询就恢复。日志按文本去重（见 `shader_error_shown`）。
    fn attach_shader_override_dir(&mut self, renderer: &mut Renderer) {
        use rustlua_gpu::shader_library::SHADER_RELOAD_DEBOUNCE;

        let Some(dir) = self.shader_dir.clone() else {
            return;
        };
        tracing::info!(target: "game", dir = %dir.display(), "shader 覆盖目录已启用（开发期热重载）");
        match renderer.set_shader_override_dir(Some(dir)) {
            Ok(report) => {
                tracing::info!(target: "game", "shader 源：{}", report.summary());
                for name in &report.fell_back_to_builtin {
                    tracing::warn!(target: "game", "shader {name} 的覆盖文件已删除，回退内置源");
                }
            }
            Err(error) => {
                let text = error.to_string();
                tracing::error!(target: "game", "shader 覆盖目录初始加载失败（使用内置源）：{text}");
                self.shader_error_shown = Some(text);
            }
        }
        // 无论成功与否都开轮询：这条时间线只对显式 `--shader-dir` 存在，默认形态一次都不进。
        self.shader_reload_due = Some(Instant::now() + SHADER_RELOAD_DEBOUNCE);
    }

    /// P3-R05-4：每帧的覆盖目录检查（去抖 150ms；没开 `--shader-dir` 时是一次 `None` 判断）。
    ///
    /// `reload_shaders_if_changed` 内部只读 8 份源算哈希：内容没变 ⇒ 一次编译都不做，
    /// 所以常态成本是一次小目录扫描，而不是重编管线。
    fn poll_shader_hot_reload(&mut self) {
        use rustlua_gpu::shader_library::SHADER_RELOAD_DEBOUNCE;

        let Some(due) = self.shader_reload_due else {
            return;
        };
        let now = Instant::now();
        if now < due {
            return;
        }
        self.shader_reload_due = Some(now + SHADER_RELOAD_DEBOUNCE);
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        match renderer.reload_shaders_if_changed() {
            Ok(None) => self.shader_error_shown = None,
            Ok(Some(report)) => {
                self.shader_error_shown = None;
                tracing::info!(target: "game", "shader 热重载：{}", report.summary());
                for name in &report.fell_back_to_builtin {
                    tracing::warn!(target: "game", "shader {name} 的覆盖文件已删除，回退内置源");
                }
            }
            Err(error) => {
                let text = error.to_string();
                if self.shader_error_shown.as_deref() != Some(text.as_str()) {
                    tracing::error!(target: "game", "shader 重载失败（保留旧管线）：{text}");
                    self.shader_error_shown = Some(text);
                }
            }
        }
    }

    /// P0（脚本迭代闭环）：每帧的 Lua 磁盘检查（去抖 250ms；没工程根/发布包/无脚本目录时
    /// 是一次 `None` 判断，常态零成本）。节律与文案与编辑器 Play 共用
    /// [`rustlua_infra::hot_reload`]。
    ///
    /// 引擎侧保证**失败保旧 VM**（`reload_if_changed` 先立候选、后破旧状态），所以这里只把
    /// 结果写进日志：独立 runner 的可见通道是日志与退出码，F3 overlay 是帧率读数、不该被
    /// 借来传脚本状态。同一条结果文本只打一次——否则 250ms 一次的重复刷屏会把真实线索挤掉。
    fn poll_script_hot_reload(&mut self, frame_elapsed: std::time::Duration) {
        if self.scripts_dir.is_none() || !self.script_cadence.advance(frame_elapsed) {
            return;
        }
        let Some(runtime) = self.runtime_scene.as_mut() else {
            return;
        };
        let (note, failed) = match runtime.reload_scripts() {
            Ok((status, entity_files)) => (
                rustlua_infra::hot_reload::script_reload_note(status.is_reloaded(), entity_files),
                false,
            ),
            Err(error) => (
                Some(rustlua_infra::hot_reload::script_reload_failure_note(
                    &error.to_string(),
                )),
                true,
            ),
        };
        let Some(note) = note else {
            return;
        };
        if self.script_note_shown.as_deref() == Some(note.as_str()) {
            return;
        }
        self.script_note_shown = Some(note.clone());
        if failed {
            tracing::error!(target: "game", "[script] {note}");
        } else {
            tracing::info!(target: "game", "[script] {note}");
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_shader_hot_reload();
        if self.lifecycle == AppLifecycle::Initializing && self.splash.is_some() {
            if !self.renderable() {
                return;
            }
            if self
                .splash
                .as_ref()
                .is_some_and(|splash| splash.stage == crate::splash::Stage::ReadyToLoad)
            {
                // A distinct Redraw callback after Presented: return to winit first,
                // and leave the last presented image untouched during synchronous loading.
                self.initialize_runtime(event_loop);
                self.splash = None;
                return;
            }
            let renderer = self.renderer.as_mut().expect("renderer exists");
            let splash = self.splash.as_mut().expect("splash exists");
            let mesh = splash.mesh(renderer.config.width, renderer.config.height);
            let outcome = renderer.render(&mesh, None);
            if matches!(outcome, RenderOutcome::Fatal) {
                self.fail(
                    event_loop,
                    "renderer reported a fatal outcome during splash",
                );
            } else {
                splash.stage.observe(outcome);
                if splash.stage == crate::splash::Stage::ReadyToLoad {
                    tracing::info!(target: "game", "[splash] presented before project initialization");
                    // 交班点：GPU 已经把同一张图（同一套 contain 几何、同一个清屏底色）
                    // 呈现出来了，GDI 那一层现在撤下不会造成任何闪动。
                    self.boot_splash = None;
                }
            }
            return;
        }
        if self.lifecycle != AppLifecycle::Running {
            return;
        }
        // 遮挡/最小化时不提交画面（present 会挂在不可见表面上），逻辑不在这里推进——
        // `about_to_wait` 的定时器接上了这一段，恢复可见时这里从当前画面接着画。
        if !self.renderable() {
            return;
        }
        // M74-02 P0：整帧墙钟从这里起算（固定步 + 提取 + 提交都在里面）。调度器只知道自己
        // 那一段，用它的总耗时当分母会把"串行系统占帧比"算大好几倍。
        let frame_started = Instant::now();
        self.performance.begin_frame();
        let report = self.advance_clock();
        self.run_simulation_steps(report.steps, report.delta_seconds, event_loop);
        if self.lifecycle != AppLifecycle::Running {
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        // Only clear edge transitions on frames that actually ran a fixed step:
        // otherwise, at high frame rates a `just_pressed` (e.g. F4 scene switch)
        // is dropped before Lua ever consumes it.
        if report.steps > 0 {
            self.input.finish_frame();
        }
        let snapshot = self.performance.snapshot();
        // 先在不借用 `self.runtime_scene` 的作用域里取回结果，再处理失败——
        // 失败处理需要 `&mut self`（记录失败态并退出），不能在借用期间调用。
        // B 阶段：逻辑屏幕空间 = 设计分辨率（`self.design_size`），与窗口物理尺寸解耦。
        // 场景/UI 顶点按设计分辨率烘焙成 NDC，渲染器再按声明视口 `contain_rect` 到
        // surface（等比居中 + 黑边）。窗口任意尺寸/最大化都保持 16:9 完整画面。
        let (design_w, design_h) = self.design_size;
        // hinting 覆盖率图集按**物理**像素取 ppem：窗口把设计盒 contain 放大多少，
        // 字号就按这个比例换成整数字形尺寸（顶点与逻辑视口仍是设计像素）。
        let window_size = self
            .window
            .as_ref()
            .map(|window| {
                let size = window.inner_size();
                (size.width, size.height)
            })
            .unwrap_or((design_w, design_h));
        let (_, _, fit_width, _) =
            rustlua_render::fit::contain_rect((design_w as f32, design_h as f32), window_size);
        if let Some(scene) = self.runtime_scene.as_mut() {
            scene.set_render_scale(fit_width as f32 / design_w.max(1) as f32);
        }
        let mesh_result: Result<SceneMesh, String> = match self.runtime_scene.as_mut() {
            Some(scene) => scene
                .vertices(design_w, design_h)
                .map_err(|error| error.to_string()),
            None => Ok(SceneMesh {
                vertices: Vec::new(),
                draws: Vec::new(),
                world_sprites: Vec::new(),
                world_sprite_draws: Vec::new(),
                paths: Vec::new(),
                // 无场景时的空网格：没有资源身份（渲染器回退到按路径解析）。
                path_handles: Vec::new(),
                font_atlas: None,
                custom_font_atlases: Vec::new(),
                vector_vertices: Vec::new(),
                vector_glyphs: Vec::new(),
                vector_segments: Vec::new(),
            }),
        };
        let mut mesh = match mesh_result {
            Ok(mesh) => mesh,
            Err(error) => {
                tracing::error!(target: "render", "[runtime] 2D scene extraction failed: {error}");
                self.fail(event_loop, format!("2D scene extraction failed: {error}"));
                return;
            }
        };
        if self.overlay_enabled {
            let overlay = build_overlay(snapshot, design_w, design_h);
            if !overlay.ranges.is_empty() {
                let base = mesh.vertices.len() as u32;
                let atlas_texture = mesh.paths.len() as u32 + 1;
                let uses_atlas = overlay.ranges.iter().any(|range| range.atlas);
                mesh.vertices.extend(overlay.vertices);
                for range in overlay.ranges {
                    mesh.draws.push(SceneDraw {
                        texture: if range.atlas { atlas_texture } else { 0 },
                        start: base + range.start,
                        count: range.count,
                        sdf: false,
                        bitmap: false,
                        vector: false,
                        // 性能浮层是世界之上的叠加内容：屏幕空间、不参与深度。
                        render_layer: rustlua_render::layer::RenderLayer::Overlay2D,
                    });
                }
                if uses_atlas {
                    // 内置 5x7 字模图集是渲染器的保留纹理，走 `paths` 索引即可。
                    mesh.paths
                        .push(rustlua_render::frame::ATLAS_TEXTURE_PATH.to_owned());
                }
            }
        }
        // 拆分批 7：视口（而不是一个裸比例）是宽高比的唯一来源；`scene_3d` 不再改写相机载荷。
        // B 阶段：视口声明 = 设计分辨率 → 渲染器 `apply_declared_viewport` 等比 contain 到 surface。
        let viewport =
            Viewport::new(design_w.max(1) as f32, design_h.max(1) as f32).unwrap_or(Viewport {
                width: 1.0,
                height: 1.0,
            });
        let scene3d = self
            .runtime_scene
            .as_ref()
            .and_then(|scene| scene.scene_3d(viewport));
        // WP5：把本帧的相机侧视图交给渲染器——清屏色与 2D overlay 的视口由这些声明决定，
        // 渲染器不再自己写死。2D 视图与上面 `vertices()` 用的是同一个 `RuntimeScene` 相机，
        // 因此"取景"和"清屏/视口"不会各说一套。
        if let Some(scene) = self.runtime_scene.as_ref() {
            let overlay = scene.render_view_2d(design_w, design_h);
            // 3D 主视图直接用**提取时烘焙好的那一份**（拆分批 7）：再调一次 `main_3d`
            // 等于把同一个视口烘焙两遍，还多一个"两次烘焙会不会不一致"的问题。
            let main = scene3d.as_ref().map(|scene3d| scene3d.view);
            renderer.set_frame_views(FrameViews {
                main,
                overlay: Some(overlay),
            });
            // WP6：世界 Sprite 深度通道打开时把 2D 相机的矩阵喂给渲染器（关闭时为 None）。
            if let Some(view_projection) = scene.sprite_view_projection(design_w, design_h) {
                renderer.set_world_sprite_camera(view_projection);
            }
        }
        match renderer.render(&mesh, scene3d.as_ref()) {
            RenderOutcome::Presented => {}
            RenderOutcome::Retry | RenderOutcome::Skipped => {}
            RenderOutcome::Fatal => self.fail(event_loop, "renderer reported a fatal outcome"),
        }
        self.performance.end_frame();
        // M74-02 P0：本帧的调度分档。放在渲染提交之后，这样样本里包含提取与提交——
        // 分母就是玩家真正看到的那一帧，而不是调度器自己的一小段。
        if let (Some(profile), Some(scene)) =
            (self.frame_profile.as_mut(), self.runtime_scene.as_ref())
            && let Some(report) = scene.last_schedule_report()
        {
            profile.record(report, frame_started.elapsed());
        }
        if snapshot.frames.is_multiple_of(60) {
            tracing::info!(target: "engine", "[perf] fps={:.1} frame_ms={:.3} update_ms={:.3} lua_ms={:.3} frames={} overlay={}", snapshot.fps, snapshot.frame_ms, snapshot.update_ms, snapshot.lua_ms, snapshot.frames, self.overlay_enabled);
        }
    }
}

/// 图标图源：优先使用工程 `icon_image`，缺省或读取失败时使用
/// 和编辑器相同的内置 `engine/assets/engine_icon.png`，按目标边长做 Lanczos3。
///
/// 为什么不直接把 256² 喂给 winit：winit 在 Windows 上按 HICON 的**原始尺寸**
/// 交给 DWM 塞进标题栏那个 `SM_CXSMICON`（= 16 × DPI）的槽里，256→16 一步缩到底
/// 会把齿轮的齿和 1.3px 的笔画糊成一团。eframe 内部自己缩过，所以编辑器可以直接
/// 喂 256；这里是裸 winit，得自己按 DPI 出图。
fn icon_rgba(root: Option<&std::path::Path>, side: u32) -> image::RgbaImage {
    let master = crate::splash::icon_image(root);
    image::imageops::resize(&master, side, side, image::imageops::FilterType::Lanczos3)
}

/// `base` 逻辑像素按 DPI 放大成物理像素；上限 256 是 winit 对 `ICON_BIG` 的建议天花板。
fn icon_side(base: f64, scale_factor: f64) -> u32 {
    (base * scale_factor).round().clamp(base, 256.0) as u32
}

/// B 阶段：窗口物理光标 → 设计分辨率坐标。与渲染器 `apply_declared_viewport` 共用
/// [`rustlua_render::fit::contain_rect`] 几何：先求设计盒在窗口里的居中矩形，再按比例
/// 逆变换；黑边区域（盒外）clamp 到设计空间边缘，点击黑边等价于点边缘像素、永不越界。
fn cursor_to_design(design: (u32, u32), window: (u32, u32), pos: (f64, f64)) -> (f64, f64) {
    let (x, y, width, height) =
        rustlua_render::fit::contain_rect((design.0 as f32, design.1 as f32), window);
    let scale_x = design.0 as f64 / width.max(1) as f64;
    let scale_y = design.1 as f64 / height.max(1) as f64;
    (
        (pos.0 - x as f64)
            .mul_add(scale_x, 0.0)
            .clamp(0.0, design.0.saturating_sub(1) as f64),
        (pos.1 - y as f64)
            .mul_add(scale_y, 0.0)
            .clamp(0.0, design.1.saturating_sub(1) as f64),
    )
}

/// 标题栏图标（`WM_SETICON(ICON_SMALL)`），尺寸 = 16 × DPI。
fn window_icon(root: Option<&std::path::Path>, scale_factor: f64) -> Icon {
    let side = icon_side(16.0, scale_factor);
    let scaled = icon_rgba(root, side);
    // from_rgba 只校验长度 == 边长²×4，这里必然成立；真失败说明图源被改坏。
    Icon::from_rgba(scaled.into_raw(), side, side).expect("winit 拒绝了窗口图标")
}

/// 任务栏 / Alt-Tab 的大图标（`WM_SETICON(ICON_BIG)`），尺寸 = 32 × DPI。
///
/// **为什么必须单独设**：winit 的 `with_window_icon` 在 Windows 上只发
/// `WM_SETICON(ICON_SMALL)`；`ICON_BIG` 空着时任务栏不会退回 PE 里的文件图标，
/// 而是用窗口类的 `IDI_APPLICATION`（白纸窗口），于是 exe 明明嵌了齿轮图标，
/// 运行中的任务栏仍显示成"没有图标"。build.rs 那份 `engine_icon.ico` 只对
/// Explorer 里的**文件图标**生效。
#[cfg(windows)]
fn taskbar_icon(root: Option<&std::path::Path>, scale_factor: f64) -> Icon {
    let side = icon_side(32.0, scale_factor);
    let scaled = icon_rgba(root, side);
    Icon::from_rgba(scaled.into_raw(), side, side).expect("winit 拒绝了任务栏图标")
}

impl ApplicationHandler for GameApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // 启动图先解码：窗口一出现就要有图可画。GPU 渲染器（`Renderer::new`）在 Windows 上
        // 实测要 ~2.5 s（`Instance::new` 就占 ~1.9 s），而解码只花几十毫秒 ⇒ 同一张
        // `RgbaImage` 先后喂给 GDI 早期启动图与 GPU 纹理上传，不重复解码、也绝不出现
        // "窗口已出现但一片空白"的那 2.5 s。见 `crate::boot_splash`。
        // 窗口尺寸由工程声明（`project.toml` 的 `window_width`/`window_height`），缺省
        // 1280×720 横屏。**为什么不能写死**：竖版工程（`game_jhyt` 的 720×1280）里
        // Lua 动态 UI 是屏幕像素空间、不随参考分辨率缩放，窗口比例不对就会整块画到屏幕外。
        let design_size = match self.project_root.as_deref() {
            Some(root) => {
                let (size, diagnostics) = rustlua_infra::project_config::window_size(root);
                for diagnostic in diagnostics {
                    tracing::warn!(target: "window", "[window] {diagnostic}");
                }
                size
            }
            None => rustlua_infra::project_config::DEFAULT_WINDOW_SIZE,
        };
        // 内置启动图按设计尺寸的方向选横/竖版，所以解码排在尺寸解析之后；两者都在
        // `create_window` 之前完成，窗口不会先空一帧。
        let boot_image =
            match crate::splash::startup_image(self.project_root.as_deref(), design_size) {
                Ok(image) => Some(image),
                Err(error) => {
                    tracing::warn!(target: "game", "{error}; 继续无启动图流程");
                    None
                }
            };
        let primary_monitor = event_loop.primary_monitor();
        let scale_factor = primary_monitor
            .as_ref()
            .map(|monitor| monitor.scale_factor())
            .unwrap_or(1.0);
        // 设计分辨率装不下屏幕可用区（1080p 桌面上的 720×1280）时按同一比例缩小，
        // 并把窗口摆到可用区中央——winit 不指定落点时系统级联会把缩小后的窗口摆到
        // 屏幕外（实测外框 557×1000 落在 y=205）。运行期逻辑屏幕空间仍是设计分辨率、
        // 渲染器 contain 到窗口，所以布局不受影响。见 `crate::window_fit`。
        let monitor_position = primary_monitor
            .as_ref()
            .map(|monitor| (monitor.position().x, monitor.position().y));
        let monitor_size = primary_monitor
            .as_ref()
            .map(|monitor| (monitor.size().width, monitor.size().height));
        let initial = crate::window_fit::initial_window(
            design_size,
            crate::window_fit::screen_area(monitor_position, monitor_size),
            scale_factor,
        );
        let (window_width, window_height) = initial.inner_size;
        let mut attributes = Window::default_attributes()
            .with_title(&self.window_title)
            .with_window_icon(Some(window_icon(
                self.project_root.as_deref(),
                scale_factor,
            )))
            .with_inner_size(PhysicalSize::new(window_width, window_height));
        if let Some((x, y)) = initial.position {
            attributes = attributes.with_position(PhysicalPosition::new(x, y));
        }
        // B 阶段（contain 等比适配）：运行期逻辑屏幕空间 = 设计分辨率，与窗口物理尺寸解耦。
        // 窗口可自由缩放/最大化，渲染器按 `contain_rect` 把设计盒等比居中到窗口（黑边），
        // 详见 `redraw` 与 `CursorMoved` 的适配注释。
        self.design_size = design_size;
        // 任务栏图标：winit 的 `with_window_icon` 只设 `ICON_SMALL`，大图标得自己补。
        #[cfg(windows)]
        let attributes = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attributes.with_taskbar_icon(Some(taskbar_icon(
                self.project_root.as_deref(),
                scale_factor,
            )))
        };
        if initial.inner_size == design_size {
            tracing::info!(
                target: "game",
                "[window] requested size: {window_width}x{window_height} position={:?}",
                initial.position
            );
        } else {
            tracing::info!(
                target: "game",
                "[window] requested size: {window_width}x{window_height} position={:?} (design {}x{} scaled to fit monitor, scale_factor={scale_factor})",
                initial.position,
                design_size.0,
                design_size.1
            );
        }
        let window = Arc::new(event_loop.create_window(attributes).expect("create window"));
        tracing::info!(target: "game", "[window] created: {:?}", window.id());
        // 在 `Renderer::new` 阻塞主线程之前把启动图放到屏幕上（同步画完）。
        self.boot_splash = boot_image
            .as_ref()
            .and_then(|image| crate::boot_splash::BootSplash::show(&window, image));
        match Renderer::new(
            window.clone(),
            self.project_root.clone(),
            self.cull_enabled,
            self.instancing_enabled,
        ) {
            Ok(mut renderer) => {
                self.attach_shader_override_dir(&mut renderer);
                self.window = Some(window);
                self.renderer = Some(renderer);
                let splash = match boot_image {
                    Some(image) => crate::splash::Splash::upload(
                        self.renderer.as_mut().expect("renderer exists"),
                        image,
                    ),
                    None => Err("没有可用的启动图".to_owned()),
                };
                match splash {
                    Ok(splash) => self.splash = Some(splash),
                    Err(error) => {
                        tracing::warn!(target: "game", "{error}; 继续无启动图流程");
                        // 没有 GPU 启动图可交接：早期启动图必须现在就撤下，
                        // 否则它会一直盖在运行期画面上。
                        self.boot_splash = None;
                    }
                }
                if self.splash.is_some() {
                    self.window
                        .as_ref()
                        .expect("window exists")
                        .request_redraw();
                } else {
                    self.initialize_runtime(event_loop);
                }
            }
            Err(error) => {
                tracing::error!(target: "engine", error = %EngineError::new(LogModule::Engine, "initialize renderer", "GPU renderer startup failed").with_source(&error).with_suggestion("update the graphics driver or use a compatible GPU"), "renderer initialization failed");
                self.window = Some(window);
                self.fail(
                    event_loop,
                    format!("renderer initialization failed: {error}"),
                );
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_none_or(|window| window.id() != window_id)
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => self.request_exit(event_loop),
            WindowEvent::Resized(size) => {
                if let Some(splash) = self.splash.as_mut() {
                    splash.stage = crate::splash::Stage::AwaitingPresent;
                }
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size);
                }
                self.sync_render_state(size);
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if let Some(splash) = self.splash.as_mut() {
                    splash.stage = crate::splash::Stage::AwaitingPresent;
                }
                let size = self.window.as_ref().expect("window exists").inner_size();
                self.sync_render_state(size);
            }
            WindowEvent::KeyboardInput { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::ModifiersChanged(_)
            | WindowEvent::Ime(_)
                if self.lifecycle == AppLifecycle::Initializing => {}
            // M79-04：输入法事件是**文本**，不是键位。winit 0.30.13 在 Windows 上发
            // `Preedit`（组合中的拼音，带候选区间）/`Commit`（上屏串）/`Enabled`/`Disabled`，
            // 且**不发** `CharacterReceived`——所以英文字符只能从 `KeyboardInput.logical_key`
            // 取（见下面的 `keyboard_text`）。
            WindowEvent::Ime(ime) => apply_ime_event(&mut self.input, ime),
            // M79-04：修饰键是独立事件，`KeyEvent` 里没有（见 `modifiers` 字段注释）。
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => {
                // M22/W1：**物理键**身份（`PhysicalKey::Code`），经 `host::keymap`
                // 转换——宿主只做类型转换，不再按 `logical_key` 的字符猜键位。
                // AZERTY/Dvorak 上「W 位置」因此仍是 `move_up`，而 Tab/Enter/F5/
                // 修饰键也能表达（字符身份做不到）。
                if let Some(key) = engine::host::keymap::map_key_event(&event) {
                    self.input
                        .key_event(key, event.state == ElementState::Pressed);
                }
                // M79-04：收字模式下键盘的 Unicode 字符就是正文（英文/数字直接打，不必
                // 绕输入法）。`self.ime_allowed` 是上一帧从脚本读到的收字声明，也是"现在
                // 字段在等字"的唯一口径；组合进行中时不取——那截拼音已经由 `Preedit`/`Commit`
                // 负责，键盘再送一遍会插两次。
                if let Some(text) = keyboard_text(
                    &KeyPress {
                        logical_key: &event.logical_key,
                        pressed: event.state == ElementState::Pressed,
                        synthetic: is_synthetic,
                        modifiers: self.modifiers,
                    },
                    self.ime_allowed,
                    self.input.text_preedit().is_some(),
                ) {
                    self.input.push_text_commit(&text);
                }
                // 下面这条是**宿主自身的界面快捷键**（性能叠加层开关），不是玩法
                // 输入，因此继续按逻辑键判定：它不该受改键表影响，也不该因为键盘
                // 布局而变化。Esc 不再退出进程，只作为玩法输入交给上层。
                if event.state == ElementState::Pressed
                    && matches!(
                        &event.logical_key,
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::F3)
                    )
                {
                    self.overlay_enabled = !self.overlay_enabled;
                    tracing::info!(target: "game", "[perf] overlay {}", if self.overlay_enabled { "enabled" } else { "disabled" });
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.input
                    .mouse_event(map_mouse_button(button), state == ElementState::Pressed);
            }
            WindowEvent::CursorMoved { position, .. } => {
                // B 阶段：窗口物理光标 → 设计分辨率坐标（与渲染 `contain_rect` 同一几何）。
                // Lua UI（`ui.panel` 等）按设计分辨率命中，黑边区域 clamp 到画面边缘。
                let size = self.window.as_ref().expect("window exists").inner_size();
                let (design_x, design_y) = cursor_to_design(
                    self.design_size,
                    (size.width, size.height),
                    (position.x, position.y),
                );
                self.input.cursor_moved(design_x, design_y);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x, y),
                    winit::event::MouseScrollDelta::PixelDelta(position) => {
                        (position.x as f32 / 100.0, position.y as f32 / 100.0)
                    }
                };
                self.input.scroll_event(dx, dy);
            }
            WindowEvent::Focused(focused) => {
                self.input.set_focused(focused);
                if !focused {
                    self.input.clear_transitions();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                if !event_loop.exiting()
                    && self.renderable()
                    && (self.lifecycle == AppLifecycle::Running
                        || (self.lifecycle == AppLifecycle::Initializing && self.splash.is_some()))
                {
                    self.window
                        .as_ref()
                        .expect("window exists")
                        .request_redraw();
                }
            }

            _ => {}
        }
    }

    /// 画面被抑制（遮挡/最小化）时 winit 不再发 `RedrawRequested`，仿真改由这里的定时器推进：
    /// 睡到下一个固定步到期 → 醒来跑该跑的步 → 再睡。可见时不需要它——渲染循环每帧本来就会
    /// 调 [`Self::advance_clock`]，两条路径靠 `elapsed` 互斥，不会把同一段时间跑两遍。
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.lifecycle != AppLifecycle::Running || self.renderable() {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let report = self.advance_clock();
        // 0 步的那次唤醒不跑管线：`run_script_steps` 即使 0 步也会跑一遍帧管线，
        // 被遮挡时一秒多跑几十次没有意义。
        if report.steps > 0 {
            self.run_simulation_steps(report.steps, report.delta_seconds, event_loop);
            self.input.finish_frame();
        }
        if self.lifecycle != AppLifecycle::Running {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let remaining =
            f64::from(self.time_step.fixed_delta_seconds()) - self.time_step.accumulator_seconds();
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_secs_f64(remaining.max(0.001)),
        ));
    }
}
/// 一帧 overlay 的顶点与绘制范围（`start`/`count` 相对 `vertices`）。
struct OverlayGeometry {
    vertices: Vec<OverlayVertex>,
    ranges: Vec<OverlayRange>,
}

struct OverlayRange {
    start: u32,
    count: u32,
    /// `true` = 用内置 5x7 字模图集绘制（文字），`false` = 纯色白纹理（面板/量条）。
    atlas: bool,
}

/// 调试 overlay（F3 开关）：左上角一块半透明面板，逐行显示**带标签的性能指标**，
/// 每行下面一条量条标明当前值占参考上限的比例。
///
/// 指标（与 `[perf]` 日志同源，`PerformanceSnapshot`）：
/// * `FPS`：最近 1 秒窗口的真实帧率（参考上限 120）；
/// * `FRAME ms`：上一帧总耗时（上限 33.3ms ≈ 30fps，超过 20ms 变红）；
/// * `UPDATE ms`：`run_simulation_steps` 的固定步更新耗时（上限 16.7ms = 60fps）；
/// * `LUA ms`：本帧 Lua `update()` 累计耗时（上限 8ms）；
/// * `FRAMES`：累计帧数（无参考上限，量条按 60 帧窗口的余数显示，仅作心跳）。
///
/// 文字用引擎内置 5x7 位图字模（`TextAtlas::built_in` + `TextLayout::build_bitmap`），
/// 因此 overlay 不依赖项目字体，也不会受项目字体加载状态影响。
fn build_overlay(snapshot: PerformanceSnapshot, width: u32, height: u32) -> OverlayGeometry {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let mut vertices = Vec::new();
    let mut ranges = Vec::new();

    // 文本行：`(标签, 数值文本, 量条比例, 数值颜色)`。
    let frame_ms_color = if snapshot.frame_ms > 20.0 {
        [1.0, 0.42, 0.38, 1.0]
    } else {
        [0.92, 0.96, 0.92, 1.0]
    };
    let rows = [
        (
            "FPS",
            format!("{:.1}", snapshot.fps),
            (snapshot.fps / 120.0).clamp(0.0, 1.0),
            [0.45, 1.0, 0.55, 1.0],
        ),
        (
            "FRAME ms",
            format!("{:.2}", snapshot.frame_ms),
            (snapshot.frame_ms / 33.3).clamp(0.0, 1.0),
            frame_ms_color,
        ),
        (
            "UPDATE ms",
            format!("{:.2}", snapshot.update_ms),
            (snapshot.update_ms / 16.7).clamp(0.0, 1.0),
            frame_ms_color,
        ),
        (
            "LUA ms",
            format!("{:.2}", snapshot.lua_ms),
            (snapshot.lua_ms / 8.0).clamp(0.0, 1.0),
            frame_ms_color,
        ),
        (
            "FRAMES",
            format!("{}", snapshot.frames),
            ((snapshot.frames % 600) as f32) / 600.0,
            [0.75, 0.78, 0.72, 1.0],
        ),
    ];

    // 像素布局：面板 8px 内边距 + 5x7 字模放大 2 倍（10x14px），每行 22px。
    const PAD_X: f32 = 8.0;
    const PAD_Y: f32 = 6.0;
    const ROW_H: f32 = 22.0;
    const SCALE: f32 = 2.0;
    let panel_w = 232.0;
    let panel_h = PAD_Y * 2.0 + 20.0 + rows.len() as f32 * ROW_H;

    // 面板底 + 文字。
    let panel = rect_vertices_colored(
        -1.0,
        1.0,
        -1.0 + panel_w / width * 2.0,
        1.0 - panel_h / height * 2.0,
        [0.02, 0.03, 0.05, 0.72],
    );
    let panel_start = 0;
    let panel_count = panel.len() as u32;
    vertices.extend_from_slice(&panel);
    ranges.push(OverlayRange {
        start: panel_start,
        count: panel_count,
        atlas: false,
    });

    let atlas = rustlua_render::text::TextAtlas::built_in();
    let mut text_vertices: Vec<OverlayVertex> = Vec::new();
    let push_text = |text: &str, x: f32, y: f32, color: [f32; 4], out: &mut Vec<OverlayVertex>| {
        let style = rustlua_render::text::TextStyle {
            scale: SCALE,
            spacing: 1.0,
            line_spacing: 2.0,
            color,
            ..Default::default()
        };
        if let Ok(mesh) =
            rustlua_render::text::TextLayout::build_bitmap(text, [x, y], style, &atlas)
        {
            for vertex in mesh.vertices {
                out.push(OverlayVertex {
                    position: [
                        vertex.position[0] / width * 2.0 - 1.0,
                        1.0 - vertex.position[1] / height * 2.0,
                    ],
                    uv: vertex.uv,
                    color: vertex.color,
                    sdf: 0,
                });
            }
        }
    };
    push_text(
        "PERF  (F3)",
        PAD_X,
        PAD_Y,
        [0.55, 1.0, 0.62, 1.0],
        &mut text_vertices,
    );

    // 量条：面板内每行底部一条 6px 高的槽 + 当前值填充。
    // 注意：这里算出来的是**像素**坐标，必须换算成 NDC 再交给 `rect_vertices_colored`
    // （否则量条会被画到屏幕外，只剩面板底和文字）。
    let to_ndc_x = |px: f32| px / width * 2.0 - 1.0;
    let to_ndc_y = |py: f32| 1.0 - py / height * 2.0;
    let bars_start = vertices.len() as u32;
    let bar_x0 = PAD_X + 74.0;
    let bar_x1 = panel_w - PAD_X;
    for (index, (_, value, ratio, _)) in rows.iter().enumerate() {
        let row_y = PAD_Y + 20.0 + index as f32 * ROW_H;
        let bar_y = row_y + 4.0;
        vertices.extend(rect_vertices_colored(
            to_ndc_x(bar_x0),
            to_ndc_y(row_y),
            to_ndc_x(bar_x1),
            to_ndc_y(bar_y + 6.0),
            [0.16, 0.18, 0.20, 0.85],
        ));
        let filled = bar_x0 + (bar_x1 - bar_x0) * ratio;
        let bar_color = if index == 0 {
            [0.25, 0.85, 0.45, 0.95]
        } else {
            [0.85, 0.70, 0.30, 0.95]
        };
        vertices.extend(rect_vertices_colored(
            to_ndc_x(bar_x0),
            to_ndc_y(row_y),
            to_ndc_x(filled),
            to_ndc_y(bar_y + 6.0),
            bar_color,
        ));
        // 标签在左、数值在右侧量条上方（标签列宽 9 字符，右对齐数值）。
        let label = format!("{:<9} {}", rows[index].0, value);
        push_text(
            &label,
            PAD_X,
            row_y - 3.0,
            [0.88, 0.92, 0.88, 1.0],
            &mut text_vertices,
        );
    }
    let bars_count = vertices.len() as u32 - bars_start;
    ranges.push(OverlayRange {
        start: bars_start,
        count: bars_count,
        atlas: false,
    });

    // 文字最后画（放在顶点缓冲末尾，保证覆盖在面板与量条之上）。
    let text_start = vertices.len() as u32;
    let text_count = text_vertices.len() as u32;
    vertices.extend(text_vertices);
    ranges.push(OverlayRange {
        start: text_start,
        count: text_count,
        atlas: true,
    });

    OverlayGeometry { vertices, ranges }
}

/// 6 顶点纯色矩形（NDC）。
fn rect_vertices_colored(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    color: [f32; 4],
) -> [OverlayVertex; 6] {
    let uv = [0.0, 0.0];
    let vertex = |position: [f32; 2]| OverlayVertex {
        position,
        uv,
        color,
        sdf: 0,
    };
    [
        vertex([x0, y0]),
        vertex([x1, y0]),
        vertex([x1, y1]),
        vertex([x0, y0]),
        vertex([x1, y1]),
        vertex([x0, y1]),
    ]
}

/// 输入法事件 → 本帧输入快照上的文本动作（M79-04，纯函数：不开窗口也能测）。
///
/// 口径照 winit 0.30.13 的事件语义（`winit/src/event.rs` 的 `Ime` 文档）：拼音打字是
/// `Preedit("a") → Preedit("a b") → Preedit("") → Commit("啊不")`，**提交前一定先发一条
/// 空 Preedit**。`Disabled` 按文档要清掉组合串——否则脚本侧会一直挂着一截已经不在屏上
/// 编辑的拼音。
fn apply_ime_event(input: &mut InputSnapshot, ime: Ime) {
    match ime {
        // 输入法就绪本身不带文本；开了 `set_ime_allowed` 之后才有 Preedit/Commit。
        Ime::Enabled => {}
        Ime::Preedit(text, _) => input.set_text_preedit(&text),
        Ime::Commit(text) => input.push_text_commit(&text),
        Ime::Disabled => input.set_text_preedit(""),
    }
}

/// 一次按键里收字要看的四个读数（M79-04）。
///
/// 不直接吃 `&winit::event::KeyEvent`，是因为 0.30 的 `KeyEvent` 带**私有字段**
/// `platform_specific`——结构体字面量在 winit 之外写不出来，于是这条规则就只能靠真机
/// 点鼠标验。这里只收宿主真正读的四个量，判定逻辑一字不动地留在 [`keyboard_text`]。
struct KeyPress<'a> {
    /// `logical_key`：字符键带正文，命名键（Enter/Backspace/方向键）不带。
    logical_key: &'a winit::keyboard::Key,
    /// 是不是真的按下（抬起不插字）。
    pressed: bool,
    /// winit 补发的合成事件：换焦点时给「已经按住的键」补 Pressed。
    synthetic: bool,
    /// 修饰键——0.30 把它从 `KeyEvent` 挪到了独立的 `ModifiersChanged` 事件。
    modifiers: ModifiersState,
}

/// 收字模式下这次按键要不要当成正文（M79-04）。
///
/// winit 0.30.13 在 Windows 上**不发** `CharacterReceived`，可用的 Unicode 字符只在
/// `logical_key` 的 `Character` 里；命名键（Enter/Backspace/方向键）不是字符，删除由
/// 玩法键位走 `ui.text_input{op="edit"}`。五条例外：
/// * 没在收字——玩法态的按键是动作，不是文本；
/// * 正在组合——那截拼音由 `Preedit`/`Commit` 负责，键盘再送一遍会插两次（winit 明确
///   说过组合期间仍会发 KeyboardInput）；
/// * 不是按下（抬起不插字）；
/// * 合成事件——切回窗口时那串补发的 Pressed 不是我打的字，否则凭空多一个字符；
/// * Ctrl/Alt/Win 组合——Ctrl+A 是"全选"命令，不该往字段里塞一个 a（Shift 放过：它只
///   负责大小写，`logical_key` 已经带上了）。
fn keyboard_text(key: &KeyPress<'_>, capturing: bool, composing: bool) -> Option<String> {
    if !capturing || composing || key.synthetic || !key.pressed {
        return None;
    }
    let modifiers = key.modifiers;
    if modifiers.control_key() || modifiers.alt_key() || modifiers.super_key() {
        return None;
    }
    match key.logical_key {
        winit::keyboard::Key::Character(text) => Some(text.to_string()),
        _ => None,
    }
}

fn map_mouse_button(button: winit::event::MouseButton) -> MouseButton {
    match button {
        winit::event::MouseButton::Left => MouseButton::Left,
        winit::event::MouseButton::Right => MouseButton::Right,
        winit::event::MouseButton::Middle => MouseButton::Middle,
        winit::event::MouseButton::Other(value) => MouseButton::Other(value),
        winit::event::MouseButton::Back => MouseButton::Other(4),
        winit::event::MouseButton::Forward => MouseButton::Other(5),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    /// 造一次「真实按下、非合成、无修饰键」的按键读数。
    ///
    /// 这里给的是 [`KeyPress`] 而不是 `KeyEvent`：后者有私有字段 `platform_specific`，
    /// 在 winit 之外构造不出来（E0063），于是收字规则就只能真机点验。要打破哪条门槛，
    /// 测试里直接改对应字段。
    fn key_press(logical_key: &Key) -> KeyPress<'_> {
        KeyPress {
            logical_key,
            pressed: true,
            synthetic: false,
            modifiers: ModifiersState::empty(),
        }
    }

    /// 拼音打字的完整一节（winit 文档给的形状）：组合串逐字增长，提交前先发一条空
    /// `Preedit` 清屏，再发 `Commit`。
    #[test]
    fn ime_pinyin_sequence_reaches_the_snapshot() {
        let mut input = InputSnapshot::default();
        apply_ime_event(&mut input, Ime::Preedit("a".to_owned(), Some((1, 1))));
        assert_eq!(input.text_preedit(), Some("a"), "组合中的拼音可读");
        assert!(
            input.text_commits().is_empty(),
            "还没上屏，一个字符都没提交"
        );
        apply_ime_event(&mut input, Ime::Preedit("啊".to_owned(), Some((3, 3))));
        assert_eq!(input.text_preedit(), Some("啊"), "候选确认后组合串换成汉字");
        apply_ime_event(&mut input, Ime::Preedit(String::new(), None));
        assert_eq!(
            input.text_preedit(),
            None,
            "空 Preedit = 取消组合，不是空组合串"
        );
        apply_ime_event(&mut input, Ime::Commit("啊不".to_owned()));
        assert_eq!(input.text_commits(), ["啊不"], "提交串进了本帧文本事件");
        assert_eq!(input.text_preedit(), None, "提交后不该还挂着组合串");
    }

    /// 输入法被关掉（切窗口/切布局）时那截拼音必须消失：否则脚本会一直显示一个
    /// 已经不在屏上编辑的组合串。
    #[test]
    fn ime_disabled_clears_a_stale_preedit() {
        let mut input = InputSnapshot::default();
        apply_ime_event(&mut input, Ime::Preedit("ni".to_owned(), Some((2, 2))));
        // 先证"确实挂上了"：否则下面那条 `None` 一个什么都不做的实现也能满足
        // （反面对照 A 就是靠这一点发现的）。
        assert_eq!(
            input.text_preedit(),
            Some("ni"),
            "Disabled 之前必须真的挂着组合串"
        );
        apply_ime_event(&mut input, Ime::Disabled);
        assert_eq!(input.text_preedit(), None, "Disabled 要清掉残留组合串");
        assert!(input.text_commits().is_empty(), "关掉输入法不产生任何正文");
        apply_ime_event(&mut input, Ime::Enabled);
        assert_eq!(input.text_preedit(), None, "Enabled 只是就绪，不带文本");
    }

    /// 收字声明是跨帧意图，本帧文本读数却必须一步一清：两者不能同一个生命周期，
    /// 否则"字段还在等字"和"这帧打了什么字"会互相污染。
    #[test]
    fn text_frame_clears_each_frame_but_capture_declared_survives() {
        let mut input = InputSnapshot::default();
        input.set_text_capture(true);
        apply_ime_event(&mut input, Ime::Preedit("a".to_owned(), Some((1, 1))));
        // 清之前的"有"必须自己证一次：只断言清完之后为空，空实现也能过。
        assert_eq!(input.text_preedit(), Some("a"), "帧边界前挂着组合串");
        apply_ime_event(&mut input, Ime::Commit("啊".to_owned()));
        assert_eq!(input.text_commits(), ["啊"], "帧边界前有提交串");
        input.finish_frame();
        assert_eq!(input.text_preedit(), None, "预编辑不跨帧");
        assert!(input.text_commits().is_empty(), "提交串不跨帧");
        assert!(input.text_capture(), "收字声明要活着，宿主下一帧还得读它");
        input.set_text_capture(false);
        input.clear_all();
        assert!(
            !input.text_capture(),
            "显式关闭后才关；clear_all 不替脚本改意图"
        );
    }

    /// 只有"在收字、没在组合、不是合成、真的按下、没有命令修饰键"的 Unicode 字符才算正文。
    #[test]
    fn keyboard_text_requires_capture_and_no_composing_or_command_modifiers() {
        let letter = Key::Character("a".into());
        let pressed = key_press(&letter);
        assert_eq!(
            keyboard_text(&pressed, true, false).as_deref(),
            Some("a"),
            "收字模式下的字母就是正文"
        );
        assert_eq!(
            keyboard_text(&pressed, false, false),
            None,
            "玩法态的按键是动作不是文本"
        );
        assert_eq!(
            keyboard_text(&pressed, true, true),
            None,
            "组合期间不取——那截拼音由 Preedit/Commit 负责，取了两遍会重复插入"
        );
        let mut synthetic = key_press(&letter);
        synthetic.synthetic = true;
        assert_eq!(
            keyboard_text(&synthetic, true, false),
            None,
            "换焦点时补发的合成按下不是我打的字"
        );
        let mut released = key_press(&letter);
        released.pressed = false;
        assert_eq!(keyboard_text(&released, true, false), None, "抬起不插字");
        for modifiers in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SUPER,
        ] {
            let mut chord = key_press(&letter);
            chord.modifiers = modifiers;
            assert_eq!(
                keyboard_text(&chord, true, false),
                None,
                "带命令修饰键的组合不该往字段里塞字母: {modifiers:?}"
            );
        }
        let upper = Key::Character("A".into());
        let mut shifted = key_press(&upper);
        shifted.modifiers = ModifiersState::SHIFT;
        assert_eq!(
            keyboard_text(&shifted, true, false).as_deref(),
            Some("A"),
            "Shift 只管大小写，logical_key 已经带上去了"
        );
        let enter = Key::Named(NamedKey::Enter);
        assert_eq!(
            keyboard_text(&key_press(&enter), true, false),
            None,
            "Enter 是命名键：删除/确认走玩法键位，不是插一个控制字符"
        );
    }

    fn snapshot(fps: f32, frame_ms: f32, update_ms: f32, lua_ms: f32) -> PerformanceSnapshot {
        PerformanceSnapshot {
            fps,
            frame_ms,
            update_ms,
            lua_ms,
            frames: 1234,
        }
    }

    /// 调试 overlay 必须**带标签的文字 + 量条**，且文字用内置 5x7 字模图集绘制
    /// （`ranges` 里恰好一段 `atlas = true`），面板/量条走白色纹理。
    #[test]
    fn debug_overlay_draws_labeled_text_and_bars() {
        let overlay = build_overlay(snapshot(60.0, 8.0, 0.5, 0.2), 1280, 720);
        assert_eq!(overlay.ranges.len(), 3, "面板 / 量条 / 文字三段");
        assert!(!overlay.ranges[0].atlas, "面板底走白色纹理");
        assert!(!overlay.ranges[1].atlas, "量条走白色纹理");
        assert!(overlay.ranges[2].atlas, "文字走内置字模图集");
        // 文字段在顶点缓冲末尾（后画，覆盖在面板与量条上）。
        let text = &overlay.ranges[2];
        assert_eq!(
            (text.start + text.count) as usize,
            overlay.vertices.len(),
            "文字应位于顶点缓冲末尾"
        );
        // 面板与量条各段顶点数非零，且总数为 6 的倍数（三角形列表）。
        for range in &overlay.ranges {
            assert!(range.count > 0, "范围不得为空");
            assert_eq!(range.count % 6, 0, "顶点数应为 6 的倍数");
        }
        assert_eq!(overlay.vertices.len() % 6, 0);
        // 全部顶点落在 NDC 内（不会画到屏幕外）。
        for vertex in &overlay.vertices {
            assert!(
                (-1.0..=1.0).contains(&vertex.position[0]),
                "x 越界: {}",
                vertex.position[0]
            );
            assert!(
                (-1.0..=1.0).contains(&vertex.position[1]),
                "y 越界: {}",
                vertex.position[1]
            );
        }
    }

    /// 图标边长按 DPI 走，但不得小于逻辑尺寸、也不得超过 winit 给 `ICON_BIG`
    /// 建议的 256 天花板（winit 直接按 HICON 原尺寸交给 DWM，没有二次缩放）。
    #[test]
    fn icon_sides_follow_dpi_and_ceiling() {
        assert_eq!(icon_side(16.0, 1.0), 16);
        assert_eq!(icon_side(16.0, 1.5), 24);
        assert_eq!(icon_side(16.0, 2.0), 32);
        assert_eq!(icon_side(32.0, 1.0), 32);
        assert_eq!(icon_side(32.0, 1.25), 40);
        assert_eq!(icon_side(32.0, 2.0), 64);
        assert_eq!(
            icon_side(32.0, 0.5),
            32,
            "低 DPI 不能把大图标缩到逻辑尺寸以下"
        );
        assert_eq!(icon_side(16.0, 99.0), 256, "超过建议天花板时收敛到 256");
        assert_eq!(icon_side(32.0, 99.0), 256);
    }

    /// 图源必须真的缩到请求边长，并且不是全透明空图（空图会让任务栏更难看）。
    #[test]
    fn icon_source_is_resized_to_requested_side() {
        for side in [16, 24, 32, 48, 64, 256] {
            let icon = icon_rgba(None, side);
            assert_eq!(icon.dimensions(), (side, side));
            assert!(
                icon.pixels().any(|pixel| pixel[3] > 0),
                "{side}px 档不能是全透明"
            );
        }
    }

    /// 量条填充随数值单调变化：frame_ms 越大，填充越宽（同一面板布局下比较总宽度）。
    #[test]
    fn debug_overlay_bars_follow_metrics() {
        let light = build_overlay(snapshot(120.0, 0.5, 0.1, 0.05), 640, 360);
        let heavy = build_overlay(snapshot(30.0, 30.0, 15.0, 7.0), 640, 360);
        let width = |overlay: &OverlayGeometry| -> f32 {
            overlay.vertices[..overlay.ranges[1].count as usize]
                .iter()
                .map(|vertex| vertex.position[0])
                .sum()
        };
        // 同一套顶点顺序下，填充越宽 → 所有 x 之和越大。
        assert!(width(&heavy) > width(&light), "指标变差时量条填充应更宽");
    }

    /// P3-R05-4 的默认形态门：没有 `--shader-dir`/`RUSTLUA_SHADER_DIR` 时运行时**一次
    /// shader 文件 IO 都不做**——轮询入口必须原地返回，也不排下一次到期。
    #[test]
    fn shader_hot_reload_is_off_until_a_directory_is_given() {
        let mut app = GameApp::new(None, "test".to_owned(), true, true, false, false, None);
        assert!(app.shader_dir.is_none(), "发布默认不监听覆盖目录");
        app.poll_shader_hot_reload();
        assert!(
            app.shader_reload_due.is_none(),
            "没开监听时轮询不该排下一次到期"
        );
        // 显式开启后才有这条时间线；渲染器尚未建立时按去抖节奏重排，不能停在"永远差一帧"。
        let mut app = app.with_shader_dir(Some(PathBuf::from("shaders")));
        app.shader_reload_due = Some(Instant::now() - std::time::Duration::from_millis(1));
        app.poll_shader_hot_reload();
        assert!(
            app.shader_reload_due.is_some(),
            "渲染器还没建好时要保留轮询时间线"
        );
    }

    /// P0（脚本迭代闭环）的宿主开关门：**只有开发期的 loose 工程**才轮询磁盘 Lua。
    /// 没有工程根、发布包（`--pack`）、项目里没有 `scripts/` 目录——三种情况都必须一次 IO
    /// 都不做：否则每 250ms 白查一次，而且发布包会因盘上的文件改变运行内容（包内脚本来自
    /// bundle）。去抖节律本身由 `rustlua_infra::hot_reload` 的单测覆盖。
    #[test]
    fn script_hot_reload_only_polls_a_loose_script_project() {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let tag = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("rustlua-m79-script-{}-{tag}", std::process::id()));
        let scripts = root.join("scripts");
        let new = |root: Option<PathBuf>, packaged: bool| {
            GameApp::new(root, "t".to_owned(), true, true, packaged, false, None)
        };

        std::fs::create_dir_all(&root).expect("造临时工程根");
        assert!(
            new(Some(root.clone()), false).scripts_dir.is_none(),
            "项目里没有 scripts/ ⇒ 没有脚本层，一次都不查"
        );

        std::fs::create_dir_all(&scripts).expect("造脚本目录");
        std::fs::write(scripts.join("main.lua"), "function update() end\n").expect("写脚本");
        assert_eq!(
            new(Some(root.clone()), false).scripts_dir.as_deref(),
            Some(scripts.as_path()),
            "开发期工程要把 scripts/ 纳入轮询"
        );
        assert!(
            new(Some(root.clone()), true).scripts_dir.is_none(),
            "发布包不许因盘上的 scripts/ 改变运行内容"
        );
        assert!(new(None, false).scripts_dir.is_none(), "没有工程根不轮询");

        // 不轮询的宿主连节律都不推进：一次 `advance` 都不该发生。
        let mut idle = new(None, false);
        idle.poll_script_hot_reload(std::time::Duration::from_secs(1));
        assert_eq!(
            idle.script_cadence,
            rustlua_infra::hot_reload::ReloadCadence::script_default(),
            "未轮询时节律必须保持初值（不许白烧窗口）"
        );
        assert!(idle.script_note_shown.is_none(), "没查过就不该有结果文本");
        let _ = std::fs::remove_dir_all(&root);
    }
}
