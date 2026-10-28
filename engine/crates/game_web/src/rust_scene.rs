//! Rust 侧**统一模拟**的第一片：在 wasm 里把工程的运行时场景跑起来。
//!
//! ## 它想解决什么
//!
//! 到本片为止，web 上的画面有两条互不相干的来源：JS `web-runtime.js` 的 Canvas2D 路径
//! （跑真正的游戏），和 `gpu_host` 里**手工搭的**三块四边形（证明 GPU 通道通）。
//! 要让 GPU 通道画出**游戏**，场景就必须由 Rust 侧产出——也就是 `RuntimeScene`
//! （桌面运行器用的同一条路径：场景实例化 → Lua 脚本 → 每帧 `vertices()`）。
//!
//! ## 两条入口，刻意分开
//!
//! | 入口 | 跑工程脚本 | 用途 |
//! | --- | --- | --- |
//! | [`RustScene::boot`] `script=false` | 否 | 证明"引擎能画出工程的**真场景**"：组件 → 网格 → GPU |
//! | [`RustScene::boot`] `script=true` | 是 | 证明"工程的 Lua 脚本能在没有 JS 补丁层的 VM 里跑" |
//!
//! 前者是本片要证的东西，后者是已知会卡死的那条（读数见
//! `artifacts/logs/gpu-web-probe/web-host-rust-sim-first-slice-20261008.md`）。
//! 合成一条会让"脚本卡死"把"引擎画不画得出"这个问题一起拖下水。

use std::path::Path;

use engine::host::runtime_scene::{ParticleDispatchReport, TextRenderMode};
use engine::host::{RuntimeScene, SceneMesh};
use rustlua_camera::camera::{DEFAULT_3D_CLEAR, FrameViews, Viewport};
use rustlua_gpu::Renderer;
use rustlua_input::input::InputSnapshot;
use rustlua_render::frame::Scene3d;
use rustlua_scene::scene::Scene;

use crate::VIRTUAL_PROJECT_ROOT;

/// 模拟来源：把"场景从哪来"与"跑不跑工程脚本"这两件事**分开**表述。
///
/// 这不是为了多一个开关，而是因为这两件事在 `RuntimeScene` 里是**两个不同的岔路**：
/// `run_script=true` 那条路会多走三处（`load_runtime_scene`、`instantiate_scene`、
/// 存档/音频块），而"卡死"必须定位到其中一处，不能靠猜。三种组合各有各的读数：
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SimRoute {
    /// 宿主给场景字节 + **不跑**脚本（`from_scene_edit`）：第 11 轮验过的路，画得出真场景。
    SceneOnly,
    /// 引擎自己加载启动场景 + 跑脚本（`RuntimeScene::new`）：第 10 轮钉住的卡死路。
    EngineStartup,
    /// 宿主给场景字节 + **跑**脚本（`from_scene`）：把上面那条路的"场景来源"换成宿主提供，
    /// 于是它卡不卡，就能把"引擎自己加载场景"与"跑脚本分支"分开。
    SceneWithScript,
    /// 宿主给场景字节 + 跑脚本，但**不给工程根**（`from_scene(None, …)`）。
    ///
    /// `RuntimeScene::init` 里"存档 / 音频 / 执行工程脚本"整块都挂在 `let Some(root)` 上，
    /// 所以这一档只保留跑脚本分支里的**实体实例化**。它存在的唯一理由是**二分**：
    /// 这一档还卡 ⇒ 卡点在 `instantiate_scene`；这一档通了 ⇒ 卡点在被它跳过的那几个调用里。
    SceneScriptNoRoot,
}

impl SimRoute {
    /// 这条来源是否会让引擎侧执行工程脚本（写进状态属性，让读数自带出处）。
    pub fn ran_script(self) -> bool {
        matches!(
            self,
            Self::EngineStartup | Self::SceneWithScript | Self::SceneScriptNoRoot
        )
    }

    /// URL / 驱动开关上的名字。
    pub fn from_name(name: &str) -> Self {
        match name {
            "script" => Self::EngineStartup,
            "scene-script" => Self::SceneWithScript,
            "scene-script-noroot" => Self::SceneScriptNoRoot,
            _ => Self::SceneOnly,
        }
    }

    /// 这条来源是否由**引擎自己**解析启动场景（`RuntimeScene::new`）。
    ///
    /// 它决定 `engine_host_begin` 能不能在**不给 `scene`** 的情况下进 scene 档：
    /// 另外三条路的场景字节由宿主提供，缺 `scene` 就无从谈起；这一条路的 `scene_path`
    /// 只进读数（见 [`RustScene::boot`]），所以"只给 `sim`"是合法请求。
    pub fn loads_engine_startup(self) -> bool {
        matches!(self, Self::EngineStartup)
    }

    /// 读数里报出的短名（阶段自报与状态属性都用它，两处不必各存一张映射表）。
    pub fn label(self) -> &'static str {
        match self {
            Self::SceneOnly => "scene-only",
            Self::EngineStartup => "engine-startup",
            Self::SceneWithScript => "scene-script",
            Self::SceneScriptNoRoot => "scene-script-noroot",
        }
    }
}

/// 一个跑在 wasm 里的工程运行时场景。
pub struct RustScene {
    scene: RuntimeScene,
    width: u32,
    height: u32,
    /// `RuntimeScene` 那一刻的资产登记数（诊断用；跨帧表，不随帧波动）。
    assets: usize,
    /// 这条来源是否跑了工程脚本（写进状态属性，让"用的是哪条入口"可核对）。
    ran_script: bool,
    /// 场景自报：`<名字> entities=<n>` 或加载失败原文。**这是"到底载入的是哪个场景"的唯一凭据**——
    /// 引擎自己的 loader 在失败时会静默退回内置诊断场景，那样报出来的仍是"起场景成功"。
    scene_desc: String,
    /// 跨帧保留的输入快照（见 [`RustScene::step`] 里"为什么不能整份重建"）。
    input: InputSnapshot,
    /// 已推进的定长步总数：`0` 说明模拟只是"摆着"，没有真的往前走。
    steps: u64,
    /// 落到快照上的输入变化累计数（键 + 鼠标 + 同帧点按 + 摇杆轴）。恒为 0 而玩家在操作，
    /// 就说明输入没进模拟——这是"画面动、模拟不动"分叉的第一手线索。
    input_edges: u64,
    /// 推进失败的累计次数与最后一次原因。
    ///
    /// **报错不拆模拟**：脚本在 `update` 里抛错会让调度器返回 `SystemFailed`，但"这一帧的逻辑错了"
    /// 与"模拟本身不该存在"是两件事。把失败记成读数、模拟继续往前走，才看得到"从哪一帧起世界不动了"；
    /// 在这里把 `RustScene` 丢掉的话，画面会退回内建场景，现象与"根本没起场景"一模一样。
    step_errors: u64,
    /// 最后一次失败原因（截断后进状态属性）。
    last_step_error: Option<String>,
    /// 逐系统耗时**累计**（毫秒）与样本帧数：引擎自己已经算好了每帧的调度报告
    /// （`RuntimeScene::last_schedule_report`），这里只是把它累起来。
    ///
    /// 为什么不在引擎侧直接打印：那条 `[schedule]` 是 DEBUG 级、**每帧一行**，
    /// 在浏览器里会很快吃光日志出口的上限（本轮实测 400 条上限下只留下 28 行样本，
    /// 而且样本全落在最早的几十帧）。累加后按帧取均值，既完整又不刷屏。
    system_ms: std::collections::BTreeMap<String, f64>,
    /// 累计进 `system_ms` 的帧数（= 成功拿到调度报告的帧数）。
    timed_steps: u64,
    /// Lua `update` 的净耗时累计（微秒）与采样帧数。
    ///
    /// 与调度里的 `lua_update` 是**两个口径**：调度那条量的是整个系统（含借出调度器、
    /// 找 `update` 全局、错误包装），这条只包住 `update.call(())` 本身。两者之差就是桥接外围，
    /// 而这条本身回答的是"脚本自己跑了多久"。
    sum_lua_us: u64,
    lua_frames: u64,
    /// 场景文件里名字含 `player` 的实体名（跨帧固定）。
    ///
    /// 为什么不写死某个工程的主角名：`game_web` 是引擎侧的宿主，认工程名就等于把某个游戏
    /// 焊进引擎。判据与桌面/浏览器一致——**名字里含 player**，这是渲染器 `entityColor`
    /// 判定主角用的同一套启发。
    player_names: Vec<String>,
    /// 主角位置的第一帧读数（原点）。有它才能报 `moved=`——**位移**才是"输入改变了世界"的判据，
    /// 绝对坐标不是：场景一开始就摆在某处，光看坐标说不出这一帧有没有动过。
    world_origin: Option<[f64; 2]>,
    /// 最近一次世界读数（人读串），随 `step` 节流刷新。
    world_note: String,
}

impl RustScene {
    /// 起场景。`width`/`height` 用于每帧 `vertices()`——就是 2D 视图的设计分辨率。
    ///
    /// 走到哪条路上由 [`SimRoute`] 决定；失败时**不退回**任何东西，原文进状态属性。
    pub fn boot(
        width: u32,
        height: u32,
        route: SimRoute,
        scene_path: &str,
    ) -> Result<Self, String> {
        // 场景/脚本都从 vfs 读；provider 与 JS 侧 `lua_boot()` 共用同一条安装（幂等）。
        crate::lua_bridge::install_resource_provider()?;
        // 平台存档槽与资源 provider 一起装：前者管读，后者管写。装上之前 save.commit()
        // 在 wasm 上必然返回 false（没有可写文件系统），玩家的存档等于没存。
        crate::lua_bridge::install_storage_slot()?;
        // 音频出口：wasm 上没有可读文件系统也没有音频设备，引擎那条 rodio 路在这里必然失败
        // （游戏全程静音）。装上它之后播放交给浏览器/微信的"用 URL 播放"通道。
        crate::audio_sink::install()?;
        let root = Path::new(VIRTUAL_PROJECT_ROOT);
        // 主角候选名从**场景文档**里取（跨帧固定，取一次就够）。`EngineStartup` 那一档没有文档，
        // 候选留空——世界读数于是改从**活世界**的命名实体里取候选（见 [`Self::refresh_world`]，接管+scene
        // 形态走的就是这一档），两边都取不到时才退化成 `world=no-player(named=… samples=…)`，那是诚实的读数，不是失败。
        let mut player_names: Vec<String> = Vec::new();
        let (scene, scene_desc) = match route {
            SimRoute::EngineStartup => {
                let scene = RuntimeScene::new(Some(root)).map_err(|error| {
                    format!("RuntimeScene::new({VIRTUAL_PROJECT_ROOT}): {error:#}")
                })?;
                // 这一条路的场景由引擎自己解析（`RuntimeScene::new`），`scene_path` 只进读数：
                // 缺省时（`engine_host_begin` 只给了 `sim`）报一句人读的出处，别留一个空位。
                let origin = if scene_path.is_empty() {
                    "(engine startup scene)"
                } else {
                    scene_path
                };
                (scene, format!("script-load {origin}"))
            }
            SimRoute::SceneOnly | SimRoute::SceneWithScript | SimRoute::SceneScriptNoRoot => {
                let raw = crate::lua_bridge::resource_raw(scene_path).ok_or_else(|| {
                    format!("资源表里没有场景 {scene_path}（manifest 里缺这一项？）")
                })?;
                let parsed = Scene::from_packed_bytes(Path::new(scene_path), &raw)
                    .map_err(|error| format!("Scene::from_packed_bytes({scene_path}): {error}"))?;
                player_names = parsed
                    .entities
                    .values()
                    .map(|entity| entity.name.clone())
                    .filter(|name| name.to_ascii_lowercase().contains("player"))
                    .collect();
                let desc = format!(
                    "{scene_path} name={} entities={}",
                    parsed.name,
                    parsed.entities.len()
                );
                let project_root = match route {
                    // 「不给工程根」这一档刻意传 `None`：`init` 里存档/音频/执行脚本整块都挂在它上面。
                    SimRoute::SceneScriptNoRoot => None,
                    _ => Some(root),
                };
                let scene = if route.ran_script() {
                    RuntimeScene::from_scene(project_root, parsed).map_err(|error| {
                        format!("RuntimeScene::from_scene({scene_path}): {error:#}")
                    })?
                } else {
                    RuntimeScene::from_scene_edit(project_root, parsed).map_err(|error| {
                        format!("RuntimeScene::from_scene_edit({scene_path}): {error:#}")
                    })?
                };
                (scene, desc)
            }
        };
        // WebGL2 exposes no storage buffers for the vector-text shader path; the renderer drops
        // those draws, so force the equivalent atlas-backed SDF path for this host.
        let mut scene = scene;
        scene.set_text_render_mode(TextRenderMode::Sdf);
        // Web 宿主也必须装工程字体图集：否则渲染器会把 `__font_atlas__` 当普通贴图去
        // host 读取（微信日志 `pack/__font_atlas__ not found`），字形只能走白块兜底。
        // loader 内部走 vfs，与场景/脚本同一条 provider，不依赖 std::fs。
        if let Some(atlas) = engine::host::load_project_font_atlas(Some(root)) {
            scene.set_font_atlas(Some(atlas));
        }
        let assets = scene.asset_entries().len();
        Ok(Self {
            scene,
            width,
            height,
            assets,
            ran_script: route.ran_script(),
            scene_desc,
            input: InputSnapshot::default(),
            steps: 0,
            input_edges: 0,
            step_errors: 0,
            last_step_error: None,
            system_ms: std::collections::BTreeMap::new(),
            timed_steps: 0,
            sum_lua_us: 0,
            lua_frames: 0,
            player_names,
            world_origin: None,
            world_note: String::new(),
        })
    }

    /// 取本帧的 2D 网格（与桌面运行器同一个调用）。
    pub fn mesh(&mut self) -> Result<SceneMesh, String> {
        self.scene
            .vertices(self.width, self.height)
            .map_err(|error| format!("RuntimeScene::vertices: {error:#}"))
    }

    /// P3-R07-2：把本帧 Lua 排队的 `mode='gpu'` burst 交给渲染器派发，并推进 GPU 池一次。
    ///
    /// 浏览器侧（WebGL2 无 compute）里它**不会**真的建出 GPU 系统：渲染器
    /// `compute_enabled()` 为假 ⇒ 判定为 `no_compute`，这一批当帧原样落回 CPU 池，
    /// Lua 仍然读得到它们。不接这一步的话同样不丢粒子（下一帧 `run_script_steps` 入口
    /// 的整批保底会接管），但会晚一帧、且日志里只剩一句"没接线"。
    pub fn dispatch_gpu_particles(
        &mut self,
        renderer: &mut Renderer,
        delta_seconds: f32,
    ) -> ParticleDispatchReport {
        self.scene
            .dispatch_gpu_particle_bursts(renderer, delta_seconds)
    }

    /// 推进模拟：**与桌面运行器、编辑器预览、headless 运行器同一个入口**（`run_script_steps`）。
    ///
    /// 顺序与桌面一致：先把宿主这一帧的输入落到快照上，再推进定长步，最后清掉边沿与增量。
    /// 先推进再取网格（`gpu_host` 就是这么排的），否则画出来的永远是上一帧的世界。
    ///
    /// 快照**跨帧保留**：线格式里 `pressed`/`released` 是宿主算好的边沿，整份重建会把
    /// "一直按住"变成"每帧都刚按下"（`frame_input` 的测试守着这条）。
    pub fn step(
        &mut self,
        steps: u32,
        delta_seconds: f32,
        frame: &rustlua_frame_abi::FrameInput,
    ) -> Result<(), String> {
        let applied = crate::frame_input::apply_frame(&mut self.input, frame);
        self.input_edges += u64::from(
            applied.key_changes
                + applied.mouse_changes
                + applied.same_frame_taps
                + applied.axis_changes,
        );
        let outcome = self
            .scene
            .run_script_steps(steps, delta_seconds, &self.input)
            .map_err(|error| format!("RuntimeScene::run_script_steps: {error:#}"));
        // 与桌面宿主同一顺序：**用完快照之后**才清边沿与增量，并且**只有真跑了固定步的帧才清**
        // （`game_app.rs` 的 `steps > 0` 守卫、编辑器 `game_view.rs` 的 `stepped` 守卫同一件事，
        // web 侧此前漏了第三处）。漏了的后果是**永久丢一下按键**：`apply_frame` 只在「松开→按下」
        // 的跳变上产生 `just_pressed`，仍按住的键不会补发边沿，所以落在 0 步帧上的那一下被
        // `finish_frame()` 抹掉之后，下一个有步的帧里 Lua 只会看到"按着"而永远看不到"刚按下"。
        // 浏览器里 0 步帧是**常态而不是异常**（rAF 快于固定步、或追帧被 `FrameClock` 夹住）：
        // 实测同一轮 `frames=240` 只有 `sim_steps=152`，即 88 帧一步都不跑，标题确认的
        // `space:30-33` 恰好落进这种帧 ⇒ 一直停在标题，而早窗 `space:10-15` 能过。
        if steps > 0 {
            self.input.finish_frame();
        }
        self.steps += u64::from(steps);
        // Lua `update` 净耗时（引擎在 `lua_update` 系统里记的微秒数）。`take_script_micros`
        // 是**取走即清空**，所以必须每一步取一次——攒着取会把几帧的量算到最后那一帧头上。
        self.sum_lua_us += self.scene.take_script_micros();
        self.lua_frames += 1;
        // 逐系统耗时：引擎每帧都算好了（`last_schedule_report`），这里累加**只取成功帧**——
        // 失败帧的调度器会在第一个出错系统处返回，把它算进均值会让"某系统很便宜"变成假象。
        if outcome.is_ok()
            && let Some(report) = self.scene.last_schedule_report()
        {
            for timing in &report.timings {
                *self.system_ms.entry(timing.name.clone()).or_default() +=
                    timing.elapsed.as_secs_f64() * 1000.0;
            }
            self.timed_steps += 1;
        }
        if let Err(error) = &outcome {
            self.step_errors += 1;
            // 只留一轮里第一条：反复失败时后面几条通常是它的连锁，留着反而盖住首因。
            if self.last_step_error.is_none() {
                self.last_step_error = Some(truncate(error, 220));
            }
        }
        // 世界读数**不在这里做**。第一版写成了"每 30 步自动刷一次"，实测把同一台机器上的驱动剧本
        // 从通过压成了失败（5/5 红；不带 GPU 宿主时同一剧本 1/1 绿）：GPU 循环每帧多做一点，
        // 就会去抢 JS 世界的帧预算，而剧本的相位预算是按帧数算的（`nav-exit: 900`
        // ⇒ `frames=901 timeout`）。这正是"观测改变被观测的量"。
        // 改成**由消费方按需调用**（宿主只在写状态属性那一帧调 [`Self::refresh_world`]）：
        // 热路径上一分钱不花，读数照样每 30 帧都新鲜（属性本来也是 30 帧写一次）。
        outcome
    }

    /// 求值一条**调用方给的只读 Lua 表达式**，返回人读串（成功＝紧凑 JSON，失败＝错误原文）。
    ///
    /// 为什么要这条通道：`world_note` 那套读数是**引擎侧写死的**（主角坐标），而"输入之后工程自己的
    /// 状态变了没有"（模式机、背包、任务、存档）只有工程自己知道。与其为每个功能加一个字段，
    /// 不如给驱动一条能问任意问题的通道——宿主的 `rustlua-world-expr=<Lua>` 就挂在它上面。
    ///
    /// **契约**：表达式必须只读。它跑在工程同一个 Lua 状态里（因此能 `require` 工程模块、
    /// 也看得见工程设的全局），写操作会真的改世界——这条通道是给验证用的，不是游戏逻辑入口。
    pub fn eval_world_expr(&mut self, expression: &str) -> String {
        match self.scene.with_script(|lua| lua.eval_json(expression)) {
            Ok(value) => truncate(&value.to_string(), 200),
            Err(error) => format!("eval-error({})", truncate(&error.to_string(), 160)),
        }
    }

    /// 跑一段**允许有副作用**的 Lua（URL `rustlua-world-do=` 挂在它上面），返回人读串。
    ///
    /// 与 [`Self::eval_world_expr`] 是**同一个机制、两个名字**，这是刻意的：读数必须只读
    /// （否则"测量"会改世界），而验存档/交互这类东西又必须有写能力；名字分开，
    /// 调用点一眼看得出这一步会不会改世界。
    ///
    /// 约定：宿主**只跑一次**（见 `gpu_host::refresh_world_readout`），它等价于"在工程同一个
    /// Lua 状态里敲一条控制台命令"——反复跑会把副作用叠起来。
    pub fn eval_world_do(&mut self, source: &str) -> String {
        self.eval_world_expr(source)
    }

    /// 读一次世界，更新 [`Self::world_note`]（并记住主角位置的原点）。
    ///
    /// **由消费方按需调用**：宿主要在写状态属性那一帧之前调一次。别放回 `step()` 里自动刷——
    /// 那会在热路径上多一次 Lua 求值，足以挤掉同一台机器上并行跑着的 JS 剧本的帧预算
    /// （实测记录见 [`Self::step`]）。
    ///
    /// 只读求值：走 `LuaRuntime::eval_json`，它的契约就是"不改引擎状态、调用方传只读查询"。
    /// 取位置用引擎自己的 Lua API（`entity.find` + `transform.get`）——这两条都是**引擎**提供的，
    /// 不依赖 `web-runtime.js` 预置的那套 `__rustlua_*`。这正是第 24 轮发现"JS 世界在遮蔽引擎
    /// API"之后该走的路。
    pub fn refresh_world(&mut self) {
        // 候选名有两个来源，顺序就是"这份读数在说什么"：
        //   ① **场景文档**里声明的实体名（`SceneOnly`/`SceneWithScript` 两条路，boot 时取一次，跨帧固定）；
        //   ② 文档没给候选时——`EngineStartup` 那条路根本不解析场景文档，而引擎宿主接管形态与浏览器
        //      默认形态走的就是它——从**活世界的登记名表**取（与工程 Lua `entity.find` 同一命名空间）。
        //      这里刻意不用 `runtime_spawned_entities()` 的名字：实测它报
        //      `named=50 samples=顾长风,陆归藏,陆青峰,苏晚晴`——那张表面向编辑器镜像、**排除**场景声明
        //      实体，而主角恰恰是文档里声明的那一位，拿它筛 `player` 永远筛不到。
        // 判据两边共用（名字含 `player`，即渲染器 `entityColor` 认主角的那套启发），不写死工程名：
        // `game_web` 是引擎侧宿主，认工程名就等于把某个游戏焊进引擎。
        let mut names = self.player_names.clone();
        let named = if names.is_empty() {
            self.scene.named_entity_names()
        } else {
            Vec::new()
        };
        if names.is_empty() {
            names = named
                .iter()
                .filter(|name| name.to_ascii_lowercase().contains("player"))
                .cloned()
                .collect();
        }
        if names.is_empty() {
            // "没有主角"要说清它**看见了什么**：只报 `world=no-player` 的话，下一轮还得先猜是
            // "启发没命中"还是"世界里真一个命名实体都没有"。上面那条排除规则就是靠这四个样本名
            // 定位到的。样本按登记名排序取前四个（ASCII 名排在中文前）。
            let samples: Vec<String> = named
                .iter()
                .take(4)
                .map(|name| truncate(name, 24))
                .collect();
            self.world_note = format!(
                "world=no-player(named={} samples={})",
                named.len(),
                if samples.is_empty() {
                    "-".to_owned()
                } else {
                    samples.join(",")
                }
            );
            return;
        }
        // 名字仍按 Lua 字面量转义（登记名里出现引号时不能把表达式拼坏）。
        let mut body = String::new();
        for name in &names {
            let escaped = name.replace('\\', "\\\\").replace('\'', "\\'");
            body.push_str(&format!(
                "do local e = entity.find('{escaped}'); local t = e and transform.get(e) or nil; \
                 out[#out + 1] = {{ name = '{escaped}', found = (e ~= nil), \
                 x = (t and t.x or 0), y = (t and t.y or 0) }} end "
            ));
        }
        let expression = format!("(function() local out = {{}} {body} return out end)()");
        let value = self.scene.with_script(|lua| lua.eval_json(&expression));
        let players = match value {
            Ok(serde_json::Value::Array(items)) => items,
            // 读失败**不拆模拟**，只把原因写进读数——否则一次读数失败会被误当成"世界坏了"。
            Ok(other) => {
                self.world_note = format!("world=unreadable({other})");
                return;
            }
            Err(error) => {
                self.world_note = format!("world=eval-error({})", truncate(&error.to_string(), 90));
                return;
            }
        };
        let mut parts: Vec<String> = Vec::new();
        for player in &players {
            let name = player
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            if player.get("found").and_then(|value| value.as_bool()) != Some(true) {
                parts.push(format!("{name}=absent"));
                continue;
            }
            let x = player
                .get("x")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            let y = player
                .get("y")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            let origin = *self.world_origin.get_or_insert([x, y]);
            // 位移取整后报：读数是给人看与给驱动断言的，小数位没有信息量。
            let dx = (x - origin[0]).round() as i64;
            let dy = (y - origin[1]).round() as i64;
            parts.push(format!(
                "{name}@{},{} moved={dx},{dy}",
                x.round() as i64,
                y.round() as i64
            ));
        }
        if parts.is_empty() {
            self.world_note = "world=no-player".to_owned();
        } else {
            self.world_note = format!("world={}", parts.join(" "));
        }
    }

    /// 最近一次 [`Self::refresh_world`] 的人读串（`world=JianghuPlayer@624,360 moved=0,280`）。
    ///
    /// 新鲜度归调用方：这份串只在有人调 `refresh_world` 时才前进，所以读它之前必须自己决定
    /// 要不要先刷（浏览器宿主在写状态属性那一帧刷，引擎宿主在 `engine_host_state` 读态时刷）。
    pub fn world_note(&self) -> &str {
        &self.world_note
    }

    /// 设置场景提取使用的逻辑视口；它保持项目设计分辨率，与 GPU 后备缓冲尺寸分离。
    ///
    /// GPU surface 可按 contain 盒缩小后备缓冲，但在这里传入缩小后的像素尺寸会让
    /// `RuntimeScene::vertices()` 再缩放一次 Canvas UI。渲染目标的实际尺寸由 renderer 单独接收。
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// 本帧的 3D 场景（与桌面运行器同一个调用）：相机、光源、网格 drawable 全部由 ECS 实时提取。
    ///
    /// 必须在 [`Self::mesh`] **之后**调用——`vertices()` 是"本帧只解析一次相机"的定帧点，
    /// 这里再解析一次会多算一遍（引擎的账本会记成同帧两次解析）。
    pub fn scene_3d(&self) -> Option<Scene3d> {
        let viewport = Viewport::new(self.width.max(1) as f32, self.height.max(1) as f32).ok()?;
        self.scene.scene_3d(viewport)
    }

    /// 本帧的取景：2D 视图由**场景自己的相机**给出（与桌面运行器同源）。
    ///
    /// 有 3D 内容时主视图用**提取时烘焙好的那一份**（`Scene3d::view`），与桌面 runner
    /// 的 `FrameViews` 拼法逐字相同；没有 3D 内容时主视图留空，由 overlay 负责清屏。
    ///
    /// 清屏必须显式声明：`overlay_2d` 的语义是"叠在已有内容上"（`ClearPolicy::None`），
    /// 而 Web 宿主在纯 2D 档**没有任何主视图垫底**——不声明就由整帧负责的这个视图也不清屏，
    /// 未画区域停在后备缓冲的透明黑上（run8 四角 [0,0,0,0] 的机制；微信接管形态下四角
    /// 透页面背景）。底色取 `DEFAULT_3D_CLEAR`：与主 3D 视图的清屏色**同值**，
    /// 因此 2D/3D 两档从此是同一个基色，画面不随切换闪色。
    pub fn frame_views(&self, scene3d: Option<&Scene3d>) -> FrameViews {
        FrameViews {
            main: scene3d.map(|scene| scene.view),
            overlay: Some(
                self.scene
                    .render_view_2d(self.width, self.height)
                    .with_clear(rustlua_camera::camera::ClearPolicy::Color(DEFAULT_3D_CLEAR)),
            ),
        }
    }

    /// 世界 Sprite 的相机矩阵（与桌面运行器同源：`world_sprite_depth` 关着时为 `None`，
    /// 此时渲染器走它自己的历史回退，与桌面完全一致）。
    pub fn world_sprite_camera(&self) -> Option<rustlua_math::math3d::Mat4> {
        self.scene.sprite_view_projection(self.width, self.height)
    }

    /// 最后一次推进失败的原因（首因，已截断）。
    pub fn last_step_error(&self) -> Option<&str> {
        self.last_step_error.as_deref()
    }

    /// 诊断用的短摘要（写进状态属性；`vertices`/`draws` 是上一帧读数）。
    ///
    /// 分通道一起报，是因为"总数在涨"这件事单看总数定不了性：原生测试已经证明**同一份世界**
    /// 连提多次是逐通道稳定的（`host::runtime_scene::tests::frame_extraction`），所以浏览器里
    /// 的数字变化只可能来自"世界真的变了"——而要知道变的是哪一路，就必须把每一路分开读。
    pub fn note(&self, mesh: &SceneMesh, scene3d: Option<&Scene3d>) -> String {
        let tail = match &self.last_step_error {
            Some(error) => format!(
                " step_errors={} last_step_error=\"{error}\"",
                self.step_errors
            ),
            None => format!(" step_errors={}", self.step_errors),
        };
        // 步内热点：按均值取前 4 名（BTreeMap 保证同分时顺序稳定，读数才可逐帧比对）。
        // 名字里带 `=`/`,` 的系统不存在（调度器名字是固定的 snake_case 标识符），所以
        // 这种 "name=value" 串不需要转义就能被驱动脚本直接切分。
        let mut top: Vec<(&str, f64)> = self
            .system_ms
            .iter()
            .map(|(name, sum)| (name.as_str(), sum / self.timed_steps.max(1) as f64))
            .collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let sched = if self.timed_steps == 0 {
            String::new()
        } else {
            let list = top
                .iter()
                .take(4)
                .map(|(name, mean)| format!("{name}={mean:.2}"))
                .collect::<Vec<_>>()
                .join(",");
            format!(" sched_frames={} top_ms=[{list}]", self.timed_steps)
        };
        // 脚本净耗时（与 `sched` 里的 `lua_update` 对照着看）：两者之差是调度/桥接外围。
        let lua = if self.lua_frames == 0 {
            String::new()
        } else {
            format!(
                " avg_lua_ms={:.2} lua_frames={}",
                self.sum_lua_us as f64 / 1000.0 / self.lua_frames as f64,
                self.lua_frames
            )
        };
        format!(
            "{} assets={} vertices={} draws={} paths={} mesh3d={} world_sprites={}/{} vector={}/{} script={} \
             steps={} spawned={} script_errors={} input_edges={} {}{tail}{sched}{lua}",
            self.scene_desc,
            self.assets,
            mesh.vertices.len(),
            mesh.draws.len(),
            mesh.paths.len(),
            scene3d.map_or(0, |scene| scene.drawables.len()),
            mesh.world_sprites.len(),
            mesh.world_sprite_draws.len(),
            mesh.vector_vertices.len(),
            mesh.vector_glyphs.len(),
            self.ran_script,
            self.steps,
            self.scene.runtime_spawned_entities().len(),
            self.scene.script_error_total(),
            self.input_edges,
            self.world_note,
        )
    }
}

/// 状态属性里的错误原文要短：属性值会整条进 DOM dump，太长会把同一条读数里的其它字段挤掉。
fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut kept = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= limit {
            break;
        }
        kept.push(ch);
    }
    kept.push('…');
    kept
}
