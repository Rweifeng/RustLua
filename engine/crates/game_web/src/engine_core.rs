//! 共享引擎核心（P1 第一片）：从**调用方给的画布**到 present 的整条渲染链，两个宿主同一条路。
//!
//! ## 为什么抽这一层
//!
//! 同一个 GPU 栈搭建此前在两个宿主里各写了一份（`gpu_host::GpuHost::new` 与
//! `host_runtime::begin_inner`），而"画出工程真场景"的能力**只在浏览器那一份里**——
//! 微信引擎宿主到今天只有 clear-color 通道。本片把平台无关的那一段
//! （surface/device → Renderer → `RustScene` boot → step → mesh → 贴图 → 取景 →
//! render → present → poll）收进这一个类型，两个外壳各留各的宿主项：
//!
//! | 外壳 | 留在外壳的 |
//! | --- | --- |
//! | `gpu_host`（浏览器） | rAF、`fit_now` 两层几何、DOM 输入监听、URL 开关、`data-rustlua-gpu` 状态属性 |
//! | `host_runtime`（微信/引擎宿主） | JS 帧驱动、RLFI 输入批量、`tick=lua` 通道、pixel_sample 回读、定长 ABI 与 JSON 状态 |
//!
//! **P1 第二片之后**：本类型只负责"一帧怎么画"，**不再**持有帧时钟——时钟与
//! "这一帧该跑几步"的所有权在 [`crate::engine_host::EngineHost`]（见它的模块文档对照表）。
//!
//! ## 抽取纪律：行为等价，不是重写
//!
//! 帧序（先 step 再 mesh 再 render）、首帧不计入稳态均值、`sim_boot_*` 阶段自报、
//! 贴图只对齐一次、失败降级成内建场景但**保留原因**——这些都是既有门与无头剧本读过的现场，
//! 本文件只做搬动；要改行为请另开一片并留读数。唯一新增的是 `report` 通道：
//! 阶段自报从直接摸 `document.body` 改成回调，浏览器外壳传 `set_status`，
//! 引擎宿主外壳传 `report_line`——**文案逐字不变**。

use std::collections::HashSet;

use wasm_bindgen::JsValue;

use engine::host::{OverlayVertex, SceneDraw, SceneMesh};
use rustlua_frame_abi::FrameInput;
use rustlua_gpu::{RenderOutcome, Renderer};

use crate::rust_scene::{RustScene, SimRoute};

/// 内存里造一张 2×2 四色棋盘。资源里找不到可用贴图时的兜底，也是"贴图路径真的被采样到了"的判据：
/// 四色里**没有白**，所以最后那格若不是对应颜色，只可能是白兜底。
const CHECKER: [[u8; 4]; 4] = [
    [255, 0, 0, 255],
    [0, 255, 0, 255],
    [0, 0, 255, 255],
    [255, 255, 0, 255],
];
const CHECKER_KEY: &str = "web://checker2x2";

/// GPU 栈五件套（+ 适配器自报）。两个宿主的"建栈"从此只有这一份。
pub(crate) struct GpuStack {
    /// 保活：surface/adapter 的生命周期挂在它上面（外壳从不读它，下划线是语义不是疏忽）。
    pub _instance: wgpu::Instance,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub adapter: String,
    pub backend: String,
    pub format: wgpu::TextureFormat,
}

/// 从**调用方给的画布**建整套 GL 栈。
///
/// 三条已实测钉死的口径（两份旧实现逐字沿用）：
/// - wasm 上必须显式 `Backends::GL`（默认在 wasm 只有 webgpu）；
/// - `Rgba8UnormSrgb` 是渲染器构造时烘死的目标格式，宿主不提供就**响亮失败**，
///   不往不匹配的 surface 上渲；
/// - surface 只要 `RENDER_ATTACHMENT`：画布纹理当不了 copy 源，申请 `COPY_SRC` 会让
///   wgpu 在未捕获的校验错误上 panic，而 wasm 是 `panic=abort`——整个模块会死。
pub(crate) async fn build_gpu_stack(
    canvas: &JsValue,
    width: u32,
    height: u32,
    device_label: &str,
) -> Result<GpuStack, String> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        ..Default::default()
    });
    // 引擎里那**唯一**一份"从画布建 surface"的实现（web_surface 模块注释解释了这个
    // 不得已而为之的 unsafe 通路：wgpu 27 的 `create_surface(canvas)` 在本版本用不了）。
    let surface = rustlua_gpu::web_surface::canvas_surface(&instance, canvas)
        .map_err(|error| format!("canvas_surface: {error}"))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .map_err(|error| format!("request_adapter: {error}"))?;
    let info = adapter.get_info();
    let capabilities = surface.get_capabilities(&adapter);
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    if !capabilities.formats.contains(&format) {
        return Err(format!(
            "画布不提供 {format:?}（给的是 {:?}）；渲染器的目标格式尚未参数化",
            capabilities.formats
        ));
    }
    // 起点用 WebGL2 一定接受的地板，再把分辨率上限抬到适配器实际值。
    let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some(device_label),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|error| format!("request_device: {error}"))?;
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width,
        height,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: capabilities.alpha_modes[0],
        view_formats: vec![],
    };
    surface.configure(&device, &config);
    Ok(GpuStack {
        _instance: instance,
        surface,
        device,
        queue,
        config,
        adapter: info.name,
        backend: format!("{:?}", info.backend),
        format,
    })
}

/// 建核心的入参。`canvas` 收 `JsValue`（不是 `HtmlCanvasElement`）：微信宿主里画布不是
/// 那个构造器的实例，任何**构造器判定**都会失败；引擎需要的下游本来就只收原始对象。
pub struct CoreArgs<'a> {
    pub canvas: &'a JsValue,
    pub width: u32,
    pub height: u32,
    pub scene_path: &'a str,
    pub route: SimRoute,
    /// `sim_boot_route=` 的尾部说明（浏览器给的 `rustlua-sim-lines` 截断说明；没有就空串）。
    pub route_note: String,
    pub device_label: &'static str,
    /// 阶段自报通道：浏览器外壳 → `set_status`（状态属性），引擎宿主外壳 → `report_line`。
    /// 文案逐字沿用抽取前，外壳与无头驱动的断言才不用改。
    pub report: Box<dyn FnMut(&str)>,
}

/// 一帧的产出（外壳拿去做读数拼装）。
pub struct FrameOutput {
    pub draw_calls: u64,
    pub triangles: u64,
    /// `true` = 这一帧画的是内建三四边形（场景起不来或取不出）。
    pub builtin: bool,
    /// `RustScene::note` 的读数（`builtin` 时为空串）。
    pub scene_note: String,
    /// 场景输出差值（draw/顶点/路径），相对稳态第一帧基线。
    pub scene_delta: [i64; 3],
}

/// 共享核心：GPU 栈 + 渲染器 + Rust 侧运行时场景 + 每帧渲染链。
pub struct EngineCore {
    stack: GpuStack,
    renderer: Renderer,
    /// Rust 侧运行时场景。`None` = 起不来（`sim_note` 里就是原因），画面退回内建场景。
    sim: Option<RustScene>,
    /// 场景来源的人读说明（`rust(boot ok)` / `rust-failed:{原因}`）。
    pub sim_note: String,
    /// 贴图注册用的键。**必须与内建场景写进 `paths[0]` 的是同一个字符串**，
    /// 否则渲染器查不到这张贴图、静默退回白兜底——画面"看着正常"，只是那块贴图永远不对。
    texture_key: String,
    /// 贴图来源（写进读数，便于一眼分辨"用的是真资源还是兜底棋盘"）。
    pub texture_note: String,
    /// 已经交给渲染器的贴图路径（避免每帧重复解码/上传）。
    uploaded: HashSet<String>,
    /// `(已上传, 失败)` 计数。
    pub texture_stats: (usize, usize),
    /// 贴图只在**第一帧**对齐一次：本片不处理运行期换图（流式重传是后续内容）。
    textures_ensured: bool,
    /// 第一帧专用：阶段自报只在第一帧写（诊断卡死用，不污染每帧路径）。
    first_frame_done: bool,
    /// 推进失败只报一次（外壳 `take` 走；读数里的 `step_errors` 每帧都在，
    /// 这一条是给外壳的事件通道看的，不必等状态属性心跳）。
    pub pending_step_failure: Option<String>,
    /// 启动（场景 + 工程脚本装载）耗时，毫秒。**必须活到最后一帧**：阶段属性会被后续
    /// 覆盖，只有进读数才留得下来。
    pub boot_ms: f64,
    /// **首帧**耗时，毫秒：首步 + 首次取网格（见 `RustScene` 与旧 `gpu_host` 的同款说明）。
    pub first_frame_ms: f64,
    /// 稳态每帧三分：逻辑 / 取网格 / 渲染+呈现。**首帧不计入**。
    pub sum_step_ms: f64,
    pub sum_mesh_ms: f64,
    pub sum_render_ms: f64,
    pub timed_frames: u64,
    /// 稳态第一帧的场景输出计数（draw/顶点/路径），用于 `scene_delta=` 读数。
    mesh_baseline: Option<[usize; 3]>,
    report: Box<dyn FnMut(&str)>,
}

impl EngineCore {
    /// 建整套：GPU 栈 → 渲染器 → 贴图样本 → Rust 场景。任何一步失败都要能说出**是哪一步**
    /// （wasm 上 panic 不给回溯）。
    pub async fn build(args: CoreArgs<'_>) -> Result<Self, String> {
        let CoreArgs {
            canvas,
            width,
            height,
            scene_path,
            route,
            route_note,
            device_label,
            mut report,
        } = args;
        let stack = build_gpu_stack(canvas, width, height, device_label).await?;
        let mut renderer = Renderer::new_offscreen_async(
            stack.device.clone(),
            stack.queue.clone(),
            // **必须挂工程根**：按需贴图/网格（`gpu_texture.rs` / `gpu_mesh.rs`）用
            // `root.join(path)` 把场景里的相对路径解析成工程路径。渲染器自己不读磁盘，
            // 但 `TextureAsset::load_png` / `MeshAsset::from_gltf_path` 会把结果交给 vfs；
            // 容器形态下包的键必须是 `/project/<相对>`（`BundleSet::key_of` 靠 `strip_prefix`），
            // 给 `None` 时相对路径原样落进 vfs ⇒ `vfs: resource not found: assets/art/...`，
            // 表现是玩法画面里角色/地表贴图全走白兜底（2026-10-19 grouped 成为默认后的现场）。
            // 桌面宿主一直传 `Some(工程根)`，wasm 侧同一个语义。
            Some(std::path::PathBuf::from(crate::VIRTUAL_PROJECT_ROOT)),
            false,
            false,
        )
        .await
        .map_err(|error| format!("Renderer::new_offscreen_async: {error:#}"))?;
        let (texture_key, texture_note) = prepare_texture(&mut renderer)?;
        // Rust 侧运行时场景：**失败不致命**，原因原样带进读数，画面退回内建场景。
        //
        // 阶段自报（`sim_boot_begin` → …）：这一步要跑整个工程的 Lua 脚本，是宿主侧
        // **唯一的同步长任务**——万一它卡住，外壳停在上一条读数上什么线索都没有。
        // 所以每过一个阶段就报一次（与抽取前的 `gpu_host` 同款文案）。
        report("sim_boot_begin");
        report(&format!("sim_boot_route={} {route_note}", route.label()));
        // 启动耗时（wasm 上唯一的长同步任务）：`Instant::now()` 在这里是 panic 路径，
        // 所以用与引擎同一颗 `web_time`。
        let boot_started = web_time::Instant::now();
        let (sim, sim_note) = match RustScene::boot(width, height, route, scene_path) {
            Ok(scene) => {
                report("sim_boot_ok");
                (Some(scene), "rust(boot ok)".to_owned())
            }
            Err(error) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "[gpu] RustScene::boot failed: {error}; builtin scene will be used"
                )));
                report(&format!("sim_boot_failed:{error}"));
                (None, format!("rust-failed:{error}"))
            }
        };
        let boot_ms = boot_started.elapsed().as_secs_f64() * 1000.0;
        Ok(Self {
            stack,
            renderer,
            sim,
            sim_note,
            texture_key,
            texture_note,
            uploaded: HashSet::new(),
            texture_stats: (0, 0),
            textures_ensured: false,
            first_frame_done: false,
            pending_step_failure: None,
            boot_ms,
            first_frame_ms: f64::NAN,
            sum_step_ms: 0.0,
            sum_mesh_ms: 0.0,
            sum_render_ms: 0.0,
            timed_frames: 0,
            mesh_baseline: None,
            report: Box::new(move |s| report(s)),
        })
    }

    /// 推进模拟、取网格、渲到画布纹理并 present（与抽取前的
    /// `GpuHost::next_mesh` + `GpuHost::frame` 的渲染段同一顺序）。
    ///
    /// **先推进模拟（含工程脚本），再取网格**——与桌面运行器同序。反过来会永远画上一帧的世界。
    ///
    /// `input` 由外壳采好（浏览器 = DOM 生产者并上 JS 世界的 `RLFI`；引擎宿主 = 本帧的
    /// `RLFI` 批量）。核心的不变量只有"这份输入进模拟"。
    pub fn frame(
        &mut self,
        steps: u32,
        delta_seconds: f32,
        input: &FrameInput,
    ) -> Result<FrameOutput, String> {
        let first_frame = !self.first_frame_done;
        if first_frame {
            (self.report)("mesh_begin");
        }
        let frame = self
            .stack
            .surface
            .get_current_texture()
            .map_err(|error| format!("get_current_texture: {error}"))?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        // 借出再调：模拟要独占借用，先取出结果、再决定用哪条分支。
        let mut step_failed = false;
        // 首帧单独计时：它是"工程脚本第一次真正跑起来"的那一帧，里面含 `ui.measure_text`
        // 的**首次**字体读取与首次文本图集，是启动耗时里最不可预测的一段。
        let step_started = web_time::Instant::now();
        let mut step_ms = 0.0f64;
        let mut mesh_ms = 0.0f64;
        let attempt = self.sim.as_mut().map(|sim| -> Result<SceneMesh, String> {
            step_failed = sim.step(steps, delta_seconds, input).is_err();
            step_ms = step_started.elapsed().as_secs_f64() * 1000.0;
            let mesh_started = web_time::Instant::now();
            let mesh = sim.mesh();
            mesh_ms = mesh_started.elapsed().as_secs_f64() * 1000.0;
            mesh
        });
        if first_frame {
            self.first_frame_ms = step_started.elapsed().as_secs_f64() * 1000.0;
            // 首帧耗时与启动耗时是**两个不同的瓶颈**，合起来才等于冷启动；再带线性内存读数
            // （docs/65 P0：字体图集在 mesh 趟里，这一步的内存增量要能单独指认）。
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[gpu] sim_boot_ms={:.1} first_frame_ms={:.1} first_frame_step_ms={step_ms:.1} first_frame_mesh_ms={mesh_ms:.1} mem_after_first_mesh={}",
                self.boot_ms,
                self.first_frame_ms,
                crate::wasm_memory_bytes(),
            )));
        }
        // 推进失败**不拆模拟**：原因由 `RustScene` 记成读数（`step_errors` / `last_step_error`），
        // 这里只把"第一次失败"的原因挂出来，外壳决定发什么事件。
        if step_failed && self.pending_step_failure.is_none() {
            let reason = self
                .sim
                .as_ref()
                .and_then(|sim| sim.last_step_error())
                .unwrap_or("unknown")
                .to_string();
            self.pending_step_failure = Some(reason);
        }
        let (mesh, builtin) = match attempt {
            Some(Ok(mesh)) => (mesh, false),
            Some(Err(error)) => {
                // 起得来、取不出：把它降级成"内建场景"，但**保留原因**（这一片就是来收集原因的）。
                self.sim = None;
                self.sim_note = format!("rust-failed:{error}");
                (scene_mesh(&self.texture_key), true)
            }
            None => (scene_mesh(&self.texture_key), true),
        };
        // P3-R07-2：GPU 粒子派发口（与桌面运行器、编辑器 Play 同一个入口）。
        // 放在取网格之后没问题：GPU 粒子不进 2D 网格，由渲染器在自己的 pass 里画，
        // 只需要在下面的 `render_into_texture` 之前把 compute 提交出去。失败帧不派发。
        if !step_failed && let Some(sim) = self.sim.as_mut() {
            let dispatch = sim.dispatch_gpu_particles(&mut self.renderer, delta_seconds);
            if dispatch.requested > 0 {
                web_sys::console::log_1(&JsValue::from_str(&format!(
                    "[particle] requested={} gpu={} fallback={} systems={} reason={}",
                    dispatch.requested,
                    dispatch.gpu,
                    dispatch.fallback,
                    dispatch.systems,
                    dispatch
                        .fallback_reason
                        .map_or("none", |reason| reason.as_str())
                )));
            }
        }
        if !first_frame {
            self.sum_step_ms += step_ms;
            self.sum_mesh_ms += mesh_ms;
        }
        let mut scene_note = String::new();
        let mut scene_delta = [0i64; 3];
        if !builtin {
            if !self.textures_ensured {
                self.textures_ensured = true;
                (self.report)("textures_begin");
                self.ensure_textures(&mesh.paths);
                (self.report)(&format!(
                    "textures_done={}/{}",
                    self.texture_stats.0,
                    self.texture_stats.0 + self.texture_stats.1
                ));
            }
            let views = self.sim.as_ref().map(|sim| sim.frame_views());
            if let Some(views) = views {
                // 2D 取景由**场景自己的相机**给出（与桌面运行器同源）：不清屏/视口两套说法。
                self.renderer.set_frame_views(views);
            }
            // 世界 Sprite 的深度通道：与桌面运行器同一处调用（场景说没有就保持渲染器默认）。
            let camera = self.sim.as_ref().and_then(|sim| sim.world_sprite_camera());
            if let Some(camera) = camera {
                self.renderer.set_world_sprite_camera(camera);
            }
            let stats = self.sim.as_ref().map(|sim| sim.note(&mesh));
            if let Some(stats) = stats {
                // 场景输出基线：稳态第一帧记下 draw/顶点/路径三项，之后每帧报**差值**
                // （为什么是差值、为什么基线不含首帧：见旧 `gpu_host` 的同款注释）。
                let counts = [mesh.draws.len(), mesh.vertices.len(), mesh.paths.len()];
                scene_delta = match self.mesh_baseline {
                    Some(base) => [
                        counts[0] as i64 - base[0] as i64,
                        counts[1] as i64 - base[1] as i64,
                        counts[2] as i64 - base[2] as i64,
                    ],
                    None => {
                        if !first_frame {
                            self.mesh_baseline = Some(counts);
                        }
                        [0, 0, 0]
                    }
                };
                scene_note = stats;
            }
        }
        // 渲染+呈现：这一段是 GPU 提交侧的成本（含 present 与一次显式 poll）。
        let render_started = web_time::Instant::now();
        let outcome = self.renderer.render_into_texture(
            &mesh,
            None,
            &view,
            self.stack.config.width,
            self.stack.config.height,
        );
        let metrics = self.renderer.last_frame_metrics();
        if !matches!(outcome, RenderOutcome::Presented) {
            // `RenderOutcome` 没有 `Debug`（它的语义靠 `matches!` 读），所以这里给一句人话。
            return Err("render_into_texture 未返回 Presented".to_owned());
        }
        frame.present();
        // WebGL2 没有阻塞能力，错误域与 fence 都靠 poll 推进；这里显式轮询一次，
        // 让"这一帧的校验错误"在本函数返回前暴露，而不是变成下一帧的谜题。
        let _ = self.stack.device.poll(wgpu::PollType::Poll);
        if !first_frame {
            self.sum_render_ms += render_started.elapsed().as_secs_f64() * 1000.0;
            self.timed_frames += 1;
        }
        self.first_frame_done = true;
        Ok(FrameOutput {
            draw_calls: metrics.draw_calls,
            triangles: metrics.triangles,
            builtin,
            scene_note,
            scene_delta,
        })
    }

    /// 改后备缓冲尺寸并重配 surface（画布属性由外壳自己写——那是 DOM 的事）。
    /// 三者必须同一帧改完，否则会出现"按旧尺寸渲、往新尺寸显"的一帧。
    pub fn resize(&mut self, width: u32, height: u32) {
        self.stack.config.width = width;
        self.stack.config.height = height;
        self.stack
            .surface
            .configure(&self.stack.device, &self.stack.config);
    }

    pub fn width(&self) -> u32 {
        self.stack.config.width
    }

    pub fn height(&self) -> u32 {
        self.stack.config.height
    }

    /// 场景可变访问（外壳的世界读数/探针通道——**只在观测时机调**，见
    /// `RustScene::refresh_world` 里的实测记录：热路径上多一次 Lua 求值会挤掉并行剧本的帧预算）。
    pub fn sim_mut(&mut self) -> Option<&mut RustScene> {
        self.sim.as_mut()
    }

    /// 适配器自报（引擎宿主的 begin summary 与六问读数要原样带出去）。
    pub fn stack_info(&self) -> (&str, &str, String) {
        (
            self.stack.adapter.as_str(),
            self.stack.backend.as_str(),
            format!("{:?}", self.stack.format),
        )
    }

    /// 把本帧网格引用的贴图从资源表（或已装的 vfs provider）交进渲染器。
    ///
    /// 跳过两个**不是路径**的保留项：白纹理（渲染器就地造）与内建字模图集（保留纹理）。
    fn ensure_textures(&mut self, paths: &[String]) {
        // PNG 解码前后各一次读数（docs/65 P0 阶段遥测）：这一批是首帧之后最大的一次
        // 突发性分配（每张图 解码 RGBA + GPU 上传），要能和字体图集的账分开。
        let decode_started = web_time::Instant::now();
        let mem_before = crate::wasm_memory_bytes();
        let mut loaded = 0usize;
        let mut failed = 0usize;
        let mut png_bytes = 0usize;
        for path in paths {
            if path == rustlua_render::frame::WHITE_TEXTURE_PATH
                || path == rustlua_render::frame::ATLAS_TEXTURE_PATH
            {
                continue;
            }
            if !self.uploaded.insert(path.clone()) {
                loaded += 1;
                continue;
            }
            match crate::lua_bridge::resource_png(path) {
                Some(bytes) => {
                    png_bytes += bytes.len();
                    match self.renderer.upload_texture_png(path, &bytes) {
                        Ok(_) => loaded += 1,
                        Err(error) => {
                            failed += 1;
                            web_sys::console::warn_1(&JsValue::from_str(&format!(
                                "[gpu] upload_texture_png({path}) failed: {error}"
                            )));
                        }
                    }
                }
                None => {
                    failed += 1;
                    // 资源拿不到这张图：**不静默**——它意味着这一帧的贴图会退回白兜底。
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "[gpu] 资源表没有贴图 {path}（该四边形会退回白兜底）"
                    )));
                }
            }
        }
        self.texture_stats = (loaded, failed);
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "[gpu] textures_decoded ok={loaded} fail={failed} png_bytes={png_bytes} decode_ms={:.1} mem_before={mem_before} mem_after={}",
            decode_started.elapsed().as_secs_f64() * 1000.0,
            crate::wasm_memory_bytes(),
        )));
    }
}

/// 给场景准备一张贴图，返回 **(注册用的键, 人读的来源说明)**：优先用工程资源里第一张 PNG
/// （证明"打包资源 → GPU 贴图"这条路在真宿主里也是通的），拿不到就退回内存棋盘
/// （保证通道自身可验，不依赖某个游戏的内容）。
///
/// 键必须**只在这一处决定**：调用方（`scene_mesh`）拿到的是同一个字符串，
/// 不允许两处各自去挑一次——那样"上传失败退回棋盘"时，场景仍按资源键去查，
/// 于是静默变成白兜底，而白兜底与"贴图被采样到了"在画面上无法区分。
fn prepare_texture(renderer: &mut Renderer) -> Result<(String, String), String> {
    if let Some((path, bytes)) = crate::lua_bridge::first_png_resource() {
        match renderer.upload_texture_png(&path, &bytes) {
            Ok(_) => return Ok((path.clone(), format!("resource:{path} ({} B)", bytes.len()))),
            Err(error) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "[gpu] upload_texture_png({path}) failed: {error}; falling back to builtin checker"
                )));
            }
        }
    }
    renderer
        .upload_texture_rgba8(CHECKER_KEY, 2, 2, &CHECKER.concat())
        .map_err(|error| format!("upload_texture_rgba8: {error}"))?;
    Ok((CHECKER_KEY.to_owned(), format!("builtin:{CHECKER_KEY}")))
}

/// 手工场景：两块内建白四边形 + 一块**贴图**四边形（用 `texture = 1` 指向 `paths[0]`）。
///
/// 贴图那一格四色之一必须出现：白兜底与"贴图被采样到了"是两回事，
/// 所以这一格的颜色是判据，不是装饰。
fn scene_mesh(texture_key: &str) -> SceneMesh {
    let rects = [
        [-0.9f32, -0.3, 0.1, 0.9],
        [-0.1, 0.7, 0.1, 0.9],
        [-0.9, -0.1, -0.9, -0.1],
    ];
    let mut vertices = Vec::with_capacity(18);
    let mut draws = Vec::with_capacity(3);
    for (index, quad) in rects.iter().enumerate() {
        let [x0, x1, y0, y1] = *quad;
        let corner = |x: f32, y: f32, u: f32, v: f32| OverlayVertex {
            position: [x, y],
            uv: [u, v],
            color: [1.0, 1.0, 1.0, 1.0],
            sdf: 0,
        };
        let start = vertices.len() as u32;
        vertices.push(corner(x0, y0, 0.0, 0.0));
        vertices.push(corner(x1, y0, 1.0, 0.0));
        vertices.push(corner(x1, y1, 1.0, 1.0));
        vertices.push(corner(x0, y0, 0.0, 0.0));
        vertices.push(corner(x1, y1, 1.0, 1.0));
        vertices.push(corner(x0, y1, 0.0, 1.0));
        let texture = if index == rects.len() - 1 { 1 } else { 0 };
        draws.push(SceneDraw::screen_ui(texture, start, 6));
    }
    let paths = vec![texture_key.to_owned()];
    SceneMesh {
        vertices,
        draws,
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths,
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    }
}
