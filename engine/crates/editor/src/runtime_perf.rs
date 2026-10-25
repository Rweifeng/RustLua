//! 运行时性能采样：把 Play 预览的**活数据**（帧率 / draw call / set pass / 顶点面数 /
//! CPU/GPU / 内存 / 资源加载与缓存）收成一份有界的逐帧快照与趋势窗口。
//!
//! 分工：
//! - `preview_session.rs` 负责在真实的预览 tick 上测量（帧间隔、脚本/更新耗时、渲染器指标）；
//! - 本模块只做**数据模型 + 有界历史 + 采样节流**，不碰 egui，也不碰 GPU；
//! - `panels/performance.rs` 只负责把快照排成表、把历史画成折线。
//!
//! 历史窗口与编辑器 profiler（`profiler.rs`）同口径：固定容量环形缓冲，长会话不会持续吃内存。

use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Write as _,
    time::Instant,
};

use engine::host::{
    CaptureBundle, CaptureScenario, FrameStats, MemorySnapshot, SceneColorFormat, TimingStat,
    now_unix_ms,
};
use rustlua_gpu::{GpuPassReport, ReadbackMetrics};
use rustlua_render::post_process::Tonemap;
use rustlua_render::render_settings::{IblMode, IblTier};

/// 趋势窗口容量：约 4 秒 @ 60 FPS（与编辑器 profiler 的 240 帧同口径）。
pub(crate) const RUNTIME_SAMPLE_CAPACITY: usize = 240;

/// IBL 模式的中文名（面板只显示这个，不上屏 `off/diffuse/specular/full`）。
fn ibl_mode_label(mode: IblMode) -> &'static str {
    match mode {
        IblMode::Off => "关闭",
        IblMode::Diffuse => "漫反射",
        IblMode::Specular => "镜面反射",
        IblMode::Full => "完整",
    }
}
/// 进程工作集的采样间隔。`GetProcessMemoryInfo` 很便宜，但没必要每个渲染帧都问一次。
#[cfg_attr(not(windows), allow(dead_code))]
const MEMORY_SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// 一帧运行时性能快照（Play 预览）。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RuntimePerfSnapshot {
    /// 采样目标（目前只有 `Play 预览`）。
    pub target: &'static str,
    /// 预览渲染分辨率。
    pub resolution: (u32, u32),
    /// 预览累计提交帧数（会话内单调递增；趋势窗口用它去重）。
    pub frames: u64,
    /// 最近一秒的预览帧率。
    pub fps: f32,
    /// 最近两帧之间的墙钟间隔（毫秒）。
    pub frame_ms: f32,
    /// 本帧预览 tick 的 CPU 耗时（脚本步进 + 提取 + 渲染提交；毫秒）。
    pub update_ms: f32,
    /// 本帧 Lua `update()` 的净耗时（毫秒）。
    pub lua_ms: f32,
    /// 渲染器内部测得的 CPU 提交耗时（毫秒）。
    pub render_cpu_ms: f32,
    /// 渲染器 GPU timestamp 回读（毫秒）。设备未启用时间戳时为 `None`。
    pub render_gpu_ms: Option<f32>,
    /// GPU 时间不可用时的人读原因。
    pub gpu_timing_note: Option<String>,
    pub draw_calls: u64,
    /// shader pipeline 状态设置次数（Unity SetPass Calls 的对应量）。
    pub set_pass_calls: u64,
    pub instances: u64,
    pub triangles: u64,
    pub vertices: u64,
    pub dropped_draws: u64,
    /// 本帧实际执行的渲染通道（按执行顺序）。
    pub passes: Vec<&'static str>,
    /// E06-3：上一帧逐通道 GPU 时间（与 `render_gpu_ms` 来自同一次提交）。
    /// 设备不支持 timestamp 查询时是 `supported=false` + 原因，不是 0 µs。
    pub gpu_passes: GpuPassReport,
    /// P3-R01：本帧是否走了 HDR 场景色（不是"策略开没开"，而是真的启用）。
    pub hdr_active: bool,
    /// 本帧场景颜色的实际格式（HDR 时为 `Rgba16Float`）。
    pub scene_format: SceneColorFormat,
    /// HDR 帧使用的 Tonemap 曲线；LDR 帧为 `None`。
    pub tonemap: Option<Tonemap>,
    /// P3-R03：解析后的 IBL 模式（Off/Diffuse/Specular/Full，见 `resolve_ibl`）。
    pub ibl_mode: IblMode,
    /// P3-R03：设备档位。C = 只有 SH9（镜面关闭）。`None` 只在默认快照里出现。
    pub ibl_tier: Option<IblTier>,
    /// 场景声明的环境贴图（`.rlenv`）；没有 `Environment` 组件时为 `None`。
    pub environment_map: Option<String>,
    /// 环境加载/应用失败的人读原因；成功或未声明时为 `None`。
    pub environment_error: Option<String>,
    pub loaded_textures: usize,
    pub font_atlas_textures: usize,
    pub texture_bytes: u64,
    pub texture_identity_hits: u64,
    pub texture_identity_misses: u64,
    pub cached_meshes: usize,
    pub mesh_bytes: u64,
    pub mesh_identity_hits: u64,
    pub mesh_identity_misses: u64,
    /// 加载器是否走异步（后台线程）模式。
    pub async_mode: bool,
    pub async_workers: usize,
    pub async_pending: usize,
    pub async_completed: usize,
    pub vertex_buffer_capacity: u64,
    pub vertex_buffer_growths: u64,
    pub vertex_bytes_peak: u64,
    /// P3-R04-3：已编译的 3D 网格/阴影 shader 变体数。
    pub pipeline_variants: usize,
    /// 变体缓存命中次数（构造期重复请求同一键）。
    pub pipeline_variant_hits: u64,
    /// 变体缓存未命中次数（= 编译尝试次数）。
    pub pipeline_variant_misses: u64,
    /// 启动期变体编译累计耗时（毫秒）。
    pub pipeline_compile_ms: f32,
    pub readback: ReadbackMetrics,
    /// 进程工作集（MiB）。非 Windows 平台为 `None`（不假装有数据）。
    pub process_memory_mib: Option<f64>,
    /// P3-E06-4：Lua VM 实际占用的堆字节（mlua `used_memory`）。
    pub lua_heap_bytes: u64,
    /// 运行时资源身份表条目数。
    pub asset_entries: usize,
    /// 场景声明的实体数。
    pub scene_entities: usize,
    /// 运行时 ECS 世界实体数（含脚本 spawn 的实体）。
    pub runtime_entities: usize,
}

impl RuntimePerfSnapshot {
    /// 帧率是否低于 50 FPS（面板据此标警示色）。首帧 FPS 为 0，不算越界。
    pub(crate) fn fps_below_target(&self) -> bool {
        self.frames > 2 && self.fps > 0.0 && self.fps < 50.0
    }

    /// 帧间隔是否越过 60 FPS 预算（20 ms 留一点余量，避免 59.9 FPS 全标黄）。
    pub(crate) fn frame_ms_over_budget(&self) -> bool {
        self.frames > 2 && self.frame_ms > 20.0
    }

    /// GPU 时间的一行读数（可用时是毫秒，不可用时是人话原因）。
    pub(crate) fn gpu_timing_label(&self) -> String {
        if let Some(ms) = self.render_gpu_ms {
            return format!("{ms:.2} ms");
        }
        self.gpu_timing_note
            .clone()
            .unwrap_or_else(|| "不可用".to_owned())
    }

    /// P3-R01：Tonemap 曲线的面板读数（LDR 帧明确显示"无"而不是空）。
    pub(crate) fn tonemap_label(&self) -> &'static str {
        self.tonemap.map_or("无（LDR）", Tonemap::as_str)
    }

    /// P3-R03：IBL 模式 + 设备档位（面板一行）。
    pub(crate) fn ibl_label(&self) -> String {
        match self.ibl_tier {
            Some(tier) => format!(
                "{}（档位 {}）",
                ibl_mode_label(self.ibl_mode),
                tier.as_str()
            ),
            None => ibl_mode_label(self.ibl_mode).to_owned(),
        }
    }

    /// P3-R03：环境声明与加载状态（面板一行）。
    pub(crate) fn environment_label(&self) -> String {
        match (&self.environment_map, &self.environment_error) {
            (Some(map), Some(error)) => format!("{map}（{error}）"),
            (Some(map), None) => map.clone(),
            (None, Some(error)) => error.clone(),
            (None, None) => "未声明".to_owned(),
        }
    }

    /// 完整文本报告（「复制报告」与测试都走这一条，保证面板与归档口径一致）。
    pub(crate) fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "runtime perf: target={} resolution={}x{} frames={}",
            self.target, self.resolution.0, self.resolution.1, self.frames
        );
        let _ = writeln!(
            out,
            "frame: fps={:.1} frame_ms={:.2} update_ms={:.2} lua_ms={:.3} \
             cpu_render_ms={:.2} gpu_render_ms={}",
            self.fps,
            self.frame_ms,
            self.update_ms,
            self.lua_ms,
            self.render_cpu_ms,
            self.gpu_timing_label()
        );
        let _ = writeln!(
            out,
            "render: draw_calls={} set_pass_calls={} instances={} triangles={} vertices={} \
             dropped_draws={} passes={}",
            self.draw_calls,
            self.set_pass_calls,
            self.instances,
            self.triangles,
            self.vertices,
            self.dropped_draws,
            if self.passes.is_empty() {
                "none".to_owned()
            } else {
                self.passes.join(",")
            }
        );
        let _ = writeln!(
            out,
            "gpu_passes: supported={} reason={} frame_us={} sum_us={} delta_pct={} consistent={}",
            self.gpu_passes.supported,
            self.gpu_passes.reason.as_deref().unwrap_or("none"),
            self.gpu_passes
                .frame_us
                .map_or_else(|| "none".to_owned(), |us| us.to_string()),
            self.gpu_passes.pass_sum_us,
            self.gpu_passes
                .delta_pct
                .map_or_else(|| "n/a".to_owned(), |pct| format!("{pct:+.2}")),
            self.gpu_passes.consistent.map_or("n/a", |consistent| {
                if consistent { "yes" } else { "NO" }
            })
        );
        for timing in &self.gpu_passes.passes {
            let _ = writeln!(
                out,
                "  gpu_pass {}={}us",
                timing.pass.as_str(),
                timing.gpu_us
            );
        }
        let _ = writeln!(
            out,
            "hdr: active={} scene_format={} tonemap={}",
            self.hdr_active,
            self.scene_format.as_str(),
            self.tonemap.map_or("none", Tonemap::as_str)
        );
        let _ = writeln!(
            out,
            "ibl: mode={} tier={} map={} status={}",
            self.ibl_mode.as_str(),
            self.ibl_tier.map_or("none", IblTier::as_str),
            self.environment_map.as_deref().unwrap_or("none"),
            self.environment_error.as_deref().unwrap_or("ok")
        );
        let _ = writeln!(
            out,
            "shaders: variants={} compile_ms={:.2} hits={} misses={}",
            self.pipeline_variants,
            self.pipeline_compile_ms,
            self.pipeline_variant_hits,
            self.pipeline_variant_misses
        );
        let _ = writeln!(
            out,
            "resources: textures={} font_atlases={} texture_bytes={} texture_hits={} \
             texture_misses={} meshes={} mesh_bytes={} mesh_hits={} mesh_misses={} \
             load_mode={} workers={} pending={} completed={}",
            self.loaded_textures,
            self.font_atlas_textures,
            self.texture_bytes,
            self.texture_identity_hits,
            self.texture_identity_misses,
            self.cached_meshes,
            self.mesh_bytes,
            self.mesh_identity_hits,
            self.mesh_identity_misses,
            if self.async_mode { "async" } else { "sync" },
            self.async_workers,
            self.async_pending,
            self.async_completed
        );
        let _ = writeln!(
            out,
            "memory: working_set_mib={} lua_heap_mib={:.2} gpu_texture_mib={:.2} gpu_mesh_mib={:.2} \
             vertex_buffer_mib={:.2} readback_cpu_mib={:.2}",
            self.process_memory_mib
                .map_or_else(|| "unavailable".to_owned(), |mib| format!("{mib:.1}")),
            bytes_to_mib(self.lua_heap_bytes),
            bytes_to_mib(self.texture_bytes),
            bytes_to_mib(self.mesh_bytes),
            bytes_to_mib(self.vertex_buffer_capacity),
            bytes_to_mib(self.readback.cpu_bytes as u64)
        );
        let _ = writeln!(
            out,
            "scene: assets={} scene_entities={} runtime_entities={}",
            self.asset_entries, self.scene_entities, self.runtime_entities
        );
        out
    }
}

/// 趋势窗口里的一帧（只保留画图需要的字段）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RuntimePerfSample {
    pub frame_ms: f32,
    pub fps: f32,
    pub render_cpu_ms: f32,
    pub render_gpu_ms: Option<f32>,
    pub draw_calls: u64,
    pub set_pass_calls: u64,
    pub triangles: u64,
    pub vertices: u64,
    pub memory_mib: Option<f64>,
}

impl RuntimePerfSample {
    fn from_snapshot(snapshot: &RuntimePerfSnapshot) -> Self {
        Self {
            frame_ms: snapshot.frame_ms,
            fps: snapshot.fps,
            render_cpu_ms: snapshot.render_cpu_ms,
            render_gpu_ms: snapshot.render_gpu_ms,
            draw_calls: snapshot.draw_calls,
            set_pass_calls: snapshot.set_pass_calls,
            triangles: snapshot.triangles,
            vertices: snapshot.vertices,
            memory_mib: snapshot.process_memory_mib,
        }
    }
}

/// 性能面板能画的趋势序列。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeSeries {
    Fps,
    FrameMs,
    RenderCpuMs,
    RenderGpuMs,
    DrawCalls,
    SetPassCalls,
    Triangles,
    Vertices,
    MemoryMib,
}

impl RuntimeSeries {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Fps => "帧率（FPS）",
            Self::FrameMs => "帧间隔（ms）",
            Self::RenderCpuMs => "CPU 渲染（ms）",
            Self::RenderGpuMs => "GPU 渲染（ms）",
            Self::DrawCalls => "绘制调用",
            Self::SetPassCalls => "设置通道",
            Self::Triangles => "三角形",
            Self::Vertices => "顶点",
            Self::MemoryMib => "进程工作集（MiB）",
        }
    }

    pub(crate) fn value(self, sample: &RuntimePerfSample) -> Option<f32> {
        match self {
            Self::Fps => Some(sample.fps),
            Self::FrameMs => Some(sample.frame_ms),
            Self::RenderCpuMs => Some(sample.render_cpu_ms),
            Self::RenderGpuMs => sample.render_gpu_ms,
            Self::DrawCalls => Some(sample.draw_calls as f32),
            Self::SetPassCalls => Some(sample.set_pass_calls as f32),
            Self::Triangles => Some(sample.triangles as f32),
            Self::Vertices => Some(sample.vertices as f32),
            Self::MemoryMib => sample.memory_mib.map(|mib| mib as f32),
        }
    }
}

/// 固定容量的逐帧趋势窗口。
#[derive(Clone, Debug)]
pub(crate) struct RuntimePerfHistory {
    capacity: usize,
    samples: VecDeque<RuntimePerfSample>,
}

impl Default for RuntimePerfHistory {
    fn default() -> Self {
        Self::new(RUNTIME_SAMPLE_CAPACITY)
    }
}

impl RuntimePerfHistory {
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            samples: VecDeque::with_capacity(capacity),
        }
    }

    fn push(&mut self, sample: RuntimePerfSample) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    pub(crate) fn clear(&mut self) {
        self.samples.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// 某条序列在窗口内的取值（跳过缺失项，例如没有 GPU 时间戳的帧）。
    pub(crate) fn series(&self, series: RuntimeSeries) -> Vec<f32> {
        self.samples
            .iter()
            .filter_map(|sample| series.value(sample))
            .collect()
    }
}

/// 预览 tick 的轻量计时器：帧间隔、一秒滚动帧率、本帧更新/Lua 耗时。
///
/// 只在 `PreviewSession` **真正到期并提交了一帧**时调用（节流跳过的编辑器帧不进来），
/// 因此 `fps` 是预览渲染帧率，而不是编辑器 UI 帧率。
#[derive(Clone, Debug)]
pub(crate) struct PreviewPerfMeter {
    window_started: Instant,
    window_frames: u32,
    fps: f32,
    last_tick: Option<Instant>,
    frame_ms: f32,
    update_ms: f32,
    lua_ms: f32,
    frames: u64,
}

impl Default for PreviewPerfMeter {
    fn default() -> Self {
        Self {
            window_started: Instant::now(),
            window_frames: 0,
            fps: 0.0,
            last_tick: None,
            frame_ms: 0.0,
            update_ms: 0.0,
            lua_ms: 0.0,
            frames: 0,
        }
    }
}

/// 超过这个间隔就认为预览刚从暂停/失焦里恢复，不把这段墙钟算成一帧的帧时间。
const LONG_PAUSE_MS: f32 = 500.0;

impl PreviewPerfMeter {
    /// 记一次预览 tick。`now` 是 tick 开始的时刻（同一帧的后续测量用它算间隔）。
    pub(crate) fn record_tick(&mut self, now: Instant, update_ms: f32, lua_ms: f32) {
        // 会话可能在建好几秒后才第一次 tick（GPU 初始化、Play 切换）。窗口必须从**第一帧**
        // 起算，否则第一秒的 FPS 会被建会话到首帧的空闲时间摊薄成一个假的小数。
        if self.last_tick.is_none() {
            self.window_started = now;
        }
        if let Some(last) = self.last_tick {
            let interval_ms = now.saturating_duration_since(last).as_secs_f32() * 1000.0;
            if interval_ms <= LONG_PAUSE_MS {
                self.frame_ms = interval_ms;
            } else {
                // 暂停/失焦回来：重新开一个窗口，不把暂停时长画成一个 10 秒的尖峰。
                self.frame_ms = 0.0;
                self.fps = 0.0;
                self.window_started = now;
                self.window_frames = 0;
            }
        }
        self.last_tick = Some(now);
        self.update_ms = update_ms;
        self.lua_ms = lua_ms;
        self.frames = self.frames.saturating_add(1);
        self.window_frames = self.window_frames.saturating_add(1);
        let elapsed = now
            .saturating_duration_since(self.window_started)
            .as_secs_f32();
        if elapsed >= 1.0 {
            self.fps = self.window_frames as f32 / elapsed;
            self.window_frames = 0;
            self.window_started = now;
        }
    }

    /// 帧率：窗口未满一秒时用最近帧间隔兜底，避免面板开局显示 0。
    pub(crate) fn fps(&self) -> f32 {
        if self.fps > 0.0 {
            self.fps
        } else if self.frame_ms > 0.0 {
            1000.0 / self.frame_ms
        } else {
            0.0
        }
    }

    /// `(fps, frame_ms, update_ms, lua_ms, frames)`。
    pub(crate) fn snapshot(&self) -> (f32, f32, f32, f32, u64) {
        (
            self.fps(),
            self.frame_ms,
            self.update_ms,
            self.lua_ms,
            self.frames,
        )
    }
}

/// 运行时性能跟踪器：当前快照 + 趋势窗口 + 内存采样节流 + 冻结开关。
#[derive(Debug, Default)]
pub(crate) struct RuntimePerfTracker {
    snapshot: Option<RuntimePerfSnapshot>,
    history: RuntimePerfHistory,
    last_frames: Option<u64>,
    paused: bool,
    memory_mib: Option<f64>,
    memory_sampled_at: Option<Instant>,
    capture: Option<RuntimeCapture>,
}

/// E06-4：一次采集的帧数上限（600 帧 ≈ 10 秒 @60 FPS），防止忘记停止时无界增长。
pub(crate) const CAPTURE_FRAME_LIMIT: usize = 600;

/// 编辑器内的一次采集窗口：逐帧帧时间 + 逐系统/逐 GPU 通道样本 + 结束快照。
///
/// 与无头 `wp0_capture_replay` 的差别：编辑器没有把输入事件按帧持久化（Play 预览的
/// 一次性输入边沿在 `game_view` 消费），因此采集包不带可重放的 `input_log`；它只用于
/// **同机前后对比**，不用于跨进程重放。真正的重放证据由无头例子给出。
#[derive(Clone, Debug)]
pub(crate) struct RuntimeCapture {
    frames: Vec<f64>,
    systems: BTreeMap<String, Vec<f64>>,
    gpu_passes: BTreeMap<String, Vec<f64>>,
    last_snapshot: RuntimePerfSnapshot,
    last_frames: Option<u64>,
}

impl RuntimeCapture {
    fn new(snapshot: RuntimePerfSnapshot) -> Self {
        Self {
            frames: Vec::new(),
            systems: BTreeMap::new(),
            gpu_passes: BTreeMap::new(),
            last_frames: Some(snapshot.frames),
            last_snapshot: snapshot,
        }
    }

    /// 收一帧（同一帧号重复喂入会被忽略）。返回是否真的收下。
    fn push(&mut self, snapshot: &RuntimePerfSnapshot, systems: &[(String, f64)]) -> bool {
        if self.frames.len() >= CAPTURE_FRAME_LIMIT {
            return false;
        }
        if self.last_frames == Some(snapshot.frames) {
            return false;
        }
        self.last_frames = Some(snapshot.frames);
        self.frames.push(f64::from(snapshot.frame_ms));
        for (name, micros) in systems {
            self.systems.entry(name.clone()).or_default().push(*micros);
        }
        if snapshot.gpu_passes.supported {
            for pass in &snapshot.gpu_passes.passes {
                self.gpu_passes
                    .entry(pass.pass.as_str().to_owned())
                    .or_default()
                    .push(pass.gpu_us as f64);
            }
        }
        self.last_snapshot = snapshot.clone();
        true
    }

    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn last_snapshot(&self) -> &RuntimePerfSnapshot {
        &self.last_snapshot
    }

    /// 组装采集包。场景描述、状态指纹与提交号由宿主提供（本模块不读文件系统）。
    pub(crate) fn into_bundle(
        self,
        scenario: CaptureScenario,
        engine_commit: String,
        state_hash: String,
    ) -> CaptureBundle {
        let Self {
            frames,
            systems,
            gpu_passes,
            last_snapshot,
            ..
        } = self;
        let memory = MemorySnapshot {
            process_memory_mib: last_snapshot.process_memory_mib,
            lua_heap_bytes: Some(last_snapshot.lua_heap_bytes),
            texture_bytes: Some(last_snapshot.texture_bytes),
            mesh_bytes: Some(last_snapshot.mesh_bytes),
            vertex_buffer_bytes: Some(last_snapshot.vertex_buffer_capacity),
            readback_cpu_bytes: Some(last_snapshot.readback.cpu_bytes as u64),
            asset_entries: Some(last_snapshot.asset_entries as u64),
            scene_entities: Some(last_snapshot.scene_entities as u64),
            runtime_entities: Some(last_snapshot.runtime_entities as u64),
        };
        let gpu_available = !gpu_passes.is_empty();
        let gpu_note = if gpu_available {
            None
        } else if !last_snapshot.gpu_passes.supported {
            Some(
                last_snapshot
                    .gpu_passes
                    .reason
                    .clone()
                    .unwrap_or_else(|| "设备不支持 GPU 通道时间戳".to_owned()),
            )
        } else {
            Some("采集窗口内没有 GPU 通道回读".to_owned())
        };
        CaptureBundle {
            report_version: engine::host::CAPTURE_REPORT_VERSION.to_owned(),
            engine_version: engine::host::ENGINE_VERSION.to_owned(),
            engine_commit,
            captured_at_unix_ms: now_unix_ms(),
            scenario,
            state_hash,
            frames: FrameStats::from_millis(&frames),
            memory,
            systems: systems
                .into_iter()
                .filter_map(|(name, samples)| TimingStat::from_micros(name, &samples))
                .collect(),
            gpu_passes: gpu_passes
                .into_iter()
                .filter_map(|(name, samples)| TimingStat::from_micros(name, &samples))
                .collect(),
            gpu_available,
            gpu_note,
        }
    }
}

impl RuntimePerfTracker {
    pub(crate) fn snapshot(&self) -> Option<&RuntimePerfSnapshot> {
        self.snapshot.as_ref()
    }

    pub(crate) fn history(&self) -> &RuntimePerfHistory {
        &self.history
    }

    pub(crate) fn paused(&self) -> bool {
        self.paused
    }

    pub(crate) fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// 只清趋势窗口，保留当前快照（用户清空采样后仍能读这一帧的数值）。
    pub(crate) fn clear_history(&mut self) {
        self.history.clear();
    }

    pub(crate) fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    pub(crate) fn capture_len(&self) -> usize {
        self.capture.as_ref().map_or(0, RuntimeCapture::len)
    }

    /// 开始采集。没有 Play 快照时返回 `false`（不创建空采集）。
    pub(crate) fn start_capture(&mut self) -> bool {
        let Some(snapshot) = self.snapshot.clone() else {
            return false;
        };
        self.capture = Some(RuntimeCapture::new(snapshot));
        true
    }

    pub(crate) fn cancel_capture(&mut self) {
        self.capture = None;
    }

    /// 收一帧系统耗时（微秒）。暂停或没有活动采集时是空操作。
    pub(crate) fn capture_frame(
        &mut self,
        snapshot: &RuntimePerfSnapshot,
        systems: &[(String, f64)],
    ) -> bool {
        if self.paused {
            return false;
        }
        self.capture
            .as_mut()
            .is_some_and(|capture| capture.push(snapshot, systems))
    }

    /// 取出采集窗口（导出后调用方负责组装场景描述与状态指纹）。
    pub(crate) fn take_capture(&mut self) -> Option<RuntimeCapture> {
        self.capture.take()
    }

    /// 进程工作集（MiB），最多每 [`MEMORY_SAMPLE_INTERVAL`] 采样一次。
    ///
    /// 非 Windows 返回 `None`：`working_set_mib()` 在那些平台恒为 0，把它当"0 MiB"显示
    /// 会是假数据（面板显示"不可用"）。
    pub(crate) fn memory_mib_now(&mut self) -> Option<f64> {
        #[cfg(windows)]
        {
            let due = self
                .memory_sampled_at
                .is_none_or(|at| at.elapsed() >= MEMORY_SAMPLE_INTERVAL);
            if due {
                // 失败时 `working_set_mib` 返回 0；进程不可能真的 0 MiB，按"不可用"处理。
                self.memory_mib = match crate::gui_evidence::working_set_mib() {
                    mib if mib > 0.0 => Some(mib),
                    _ => None,
                };
                self.memory_sampled_at = Some(Instant::now());
            }
            self.memory_mib
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// 喂入本帧快照（`None` = Play 已停止）。
    pub(crate) fn sample(&mut self, snapshot: Option<RuntimePerfSnapshot>) {
        let Some(snapshot) = snapshot else {
            // Play 停止：快照与趋势都清掉，避免下次打开面板看到上一次会话的过期读数。
            // 采集窗口也一起丢弃：它的样本来自上一局，不能和新一局拼成一个采集包。
            self.snapshot = None;
            self.history.clear();
            self.last_frames = None;
            self.capture = None;
            return;
        };
        if self.paused {
            return;
        }
        // 新会话（帧计数回退）不能把上一次 Play 的趋势接在一起。
        if self.last_frames.is_some_and(|last| snapshot.frames < last) {
            self.history.clear();
        }
        let is_new_frame = self.last_frames != Some(snapshot.frames);
        if is_new_frame && snapshot.frames > 0 {
            self.history
                .push(RuntimePerfSample::from_snapshot(&snapshot));
            self.last_frames = Some(snapshot.frames);
        }
        self.snapshot = Some(snapshot);
    }
}

/// 字节 → MiB（面板与文本报告共用同一换算）。
pub(crate) fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn snapshot(frames: u64) -> RuntimePerfSnapshot {
        RuntimePerfSnapshot {
            target: "Play 预览",
            resolution: (1280, 720),
            frames,
            fps: 60.0,
            frame_ms: 16.6,
            update_ms: 3.2,
            lua_ms: 0.8,
            render_cpu_ms: 1.4,
            render_gpu_ms: Some(2.5),
            gpu_timing_note: None,
            draw_calls: 42,
            set_pass_calls: 7,
            instances: 18,
            triangles: 1200,
            vertices: 3600,
            dropped_draws: 0,
            passes: vec!["clear", "legacy_2d_overlay"],
            gpu_passes: GpuPassReport::default(),
            hdr_active: true,
            scene_format: SceneColorFormat::Rgba16Float,
            tonemap: Some(Tonemap::Aces),
            ibl_mode: IblMode::Full,
            ibl_tier: Some(IblTier::A),
            environment_map: Some("assets/env/studio.rlenv".to_owned()),
            environment_error: None,
            loaded_textures: 12,
            font_atlas_textures: 1,
            texture_bytes: 4 * 1024 * 1024,
            texture_identity_hits: 100,
            texture_identity_misses: 2,
            cached_meshes: 3,
            mesh_bytes: 2 * 1024 * 1024,
            mesh_identity_hits: 50,
            mesh_identity_misses: 1,
            async_mode: false,
            async_workers: 0,
            async_pending: 0,
            async_completed: 15,
            vertex_buffer_capacity: 1024 * 1024,
            vertex_buffer_growths: 1,
            vertex_bytes_peak: 900 * 1024,
            pipeline_variants: 6,
            pipeline_variant_hits: 0,
            pipeline_variant_misses: 6,
            pipeline_compile_ms: 12.5,
            readback: ReadbackMetrics::default(),
            process_memory_mib: Some(512.0),
            lua_heap_bytes: 86_113,
            asset_entries: 20,
            scene_entities: 40,
            runtime_entities: 43,
        }
    }

    #[test]
    fn history_is_bounded_and_deduplicates_frames() {
        let mut tracker = RuntimePerfTracker::default();
        for frame in 1..=(RUNTIME_SAMPLE_CAPACITY as u64 + 50) {
            tracker.sample(Some(snapshot(frame)));
            // 同一帧重复喂入不能重复入窗口。
            tracker.sample(Some(snapshot(frame)));
        }
        assert_eq!(tracker.history().len(), RUNTIME_SAMPLE_CAPACITY);
        assert_eq!(
            tracker.snapshot().map(|snapshot| snapshot.frames),
            Some(RUNTIME_SAMPLE_CAPACITY as u64 + 50)
        );
    }

    #[test]
    fn a_new_play_session_clears_the_previous_history() {
        let mut tracker = RuntimePerfTracker::default();
        for frame in 1..=10 {
            tracker.sample(Some(snapshot(frame)));
        }
        assert_eq!(tracker.history().len(), 10);
        // 新会话从 1 重新计数：历史必须清空，不能把两局接成一条曲线。
        tracker.sample(Some(snapshot(1)));
        assert_eq!(tracker.history().len(), 1);
        assert_eq!(tracker.snapshot().map(|snapshot| snapshot.frames), Some(1));
    }

    #[test]
    fn stopping_play_clears_snapshot_and_history() {
        let mut tracker = RuntimePerfTracker::default();
        tracker.sample(Some(snapshot(1)));
        tracker.sample(None);
        assert!(tracker.snapshot().is_none());
        assert!(tracker.history().is_empty());
    }

    #[test]
    fn paused_tracker_keeps_the_last_snapshot_and_does_not_advance() {
        let mut tracker = RuntimePerfTracker::default();
        tracker.sample(Some(snapshot(1)));
        tracker.set_paused(true);
        tracker.sample(Some(snapshot(2)));
        assert_eq!(tracker.snapshot().map(|snapshot| snapshot.frames), Some(1));
        assert_eq!(tracker.history().len(), 1);
        assert!(tracker.paused());
    }

    #[test]
    fn zero_frame_snapshots_do_not_enter_the_history() {
        let mut tracker = RuntimePerfTracker::default();
        tracker.sample(Some(snapshot(0)));
        assert!(tracker.history().is_empty());
        assert!(tracker.snapshot().is_some());
    }

    #[test]
    fn text_report_contains_required_key_metrics() {
        let text = snapshot(12).to_text();
        for expected in [
            "fps=60.0",
            "draw_calls=42",
            "set_pass_calls=7",
            "vertices=3600",
            "triangles=1200",
            "working_set_mib=512.0",
            "lua_heap_mib=0.08",
            "textures=12",
            "load_mode=sync",
            "runtime_entities=43",
            "hdr: active=true scene_format=rgba16float tonemap=aces",
            "ibl: mode=full tier=A map=assets/env/studio.rlenv status=ok",
            "shaders: variants=6 compile_ms=12.50 hits=0 misses=6",
        ] {
            assert!(text.contains(expected), "报告缺少 {expected}：\n{text}");
        }
    }

    #[test]
    fn series_values_skip_missing_samples() {
        let mut history = RuntimePerfHistory::new(4);
        let mut with_gpu = snapshot(1);
        with_gpu.render_gpu_ms = Some(4.0);
        let mut without_gpu = snapshot(2);
        without_gpu.render_gpu_ms = None;
        without_gpu.process_memory_mib = None;
        history.push(RuntimePerfSample::from_snapshot(&with_gpu));
        history.push(RuntimePerfSample::from_snapshot(&without_gpu));
        assert_eq!(history.series(RuntimeSeries::RenderGpuMs), vec![4.0]);
        assert_eq!(history.series(RuntimeSeries::MemoryMib).len(), 1);
        assert_eq!(history.series(RuntimeSeries::DrawCalls), vec![42.0, 42.0]);
    }

    #[test]
    fn thresholds_flag_only_after_warmup() {
        let mut early = snapshot(1);
        early.fps = 10.0;
        early.frame_ms = 100.0;
        assert!(!early.fps_below_target());
        assert!(!early.frame_ms_over_budget());

        let mut late = snapshot(10);
        late.fps = 42.0;
        late.frame_ms = 23.8;
        assert!(late.fps_below_target());
        assert!(late.frame_ms_over_budget());
    }

    #[test]
    fn gpu_label_falls_back_to_reason() {
        let mut snap = snapshot(1);
        snap.render_gpu_ms = None;
        snap.gpu_timing_note = Some("不可用（设备未启用时间戳）".to_owned());
        assert_eq!(snap.gpu_timing_label(), "不可用（设备未启用时间戳）");
        snap.gpu_timing_note = None;
        assert_eq!(snap.gpu_timing_label(), "不可用");
    }

    #[test]
    fn preview_meter_tracks_interval_and_rolling_fps() {
        let base = Instant::now();
        let mut meter = PreviewPerfMeter::default();
        meter.record_tick(base, 2.0, 0.5);
        // 首帧没有前一帧可比：FPS 为 0，而不是拿"建会话到首帧的空闲时间"算出一个假值。
        assert_eq!(meter.fps(), 0.0);
        meter.record_tick(base + Duration::from_millis(16), 3.0, 0.7);
        meter.record_tick(base + Duration::from_millis(32), 4.0, 0.9);
        let (_, frame_ms, update_ms, lua_ms, frames) = meter.snapshot();
        assert_eq!(frames, 3);
        assert!((frame_ms - 16.0).abs() < 0.001, "frame_ms={frame_ms}");
        assert!((update_ms - 4.0).abs() < f32::EPSILON);
        assert!((lua_ms - 0.9).abs() < 0.001);
        // 窗口不足一秒：用帧间隔兜底，不能显示 0 FPS。
        assert!((meter.fps() - 62.5).abs() < 0.01, "fps={}", meter.fps());
    }

    #[test]
    fn preview_meter_reopens_the_window_after_a_long_pause() {
        let base = Instant::now();
        let mut meter = PreviewPerfMeter::default();
        meter.record_tick(base, 2.0, 0.5);
        meter.record_tick(base + Duration::from_secs(5), 3.0, 0.7);
        let (_, frame_ms, _, _, frames) = meter.snapshot();
        assert_eq!(frames, 2);
        assert_eq!(frame_ms, 0.0, "暂停回来不应把 5 秒算成一帧");
    }

    #[test]
    fn bytes_to_mib_uses_binary_mebibytes() {
        assert!((bytes_to_mib(1024 * 1024) - 1.0).abs() < f64::EPSILON);
    }

    fn capture_scenario() -> CaptureScenario {
        CaptureScenario {
            name: "editor:test".to_owned(),
            scene_path: None,
            scene_hash: "0123456789abcdef".to_owned(),
            viewport: [1280, 720],
            fixed_dt_seconds: 1.0 / 60.0,
            seed: 0,
            warmup_frames: 0,
            frames: 3,
            input_log: Vec::new(),
        }
    }

    fn finished_bundle(frames: u64) -> CaptureBundle {
        let mut tracker = RuntimePerfTracker::default();
        tracker.sample(Some(snapshot(frames)));
        assert!(tracker.start_capture());
        for frame in 1..=3 {
            let snap = snapshot(frame);
            tracker.capture_frame(
                &snap,
                &[
                    ("physics".to_owned(), 20.0 + frame as f64),
                    ("lua_update".to_owned(), 1.0),
                ],
            );
        }
        tracker.take_capture().expect("capture active").into_bundle(
            capture_scenario(),
            "deadbeef".to_owned(),
            "state".to_owned(),
        )
    }

    #[test]
    fn capture_collects_frames_systems_and_memory() {
        let bundle = finished_bundle(3);
        assert_eq!(bundle.frames.samples, 3);
        assert_eq!(bundle.systems.len(), 2);
        assert_eq!(bundle.memory.lua_heap_bytes, Some(86_113));
        assert!(!bundle.gpu_available, "默认快照没有 GPU 通道读数");
        assert!(bundle.gpu_note.is_some());
        // 同帧号重复喂入不会重复计数。
        let mut tracker = RuntimePerfTracker::default();
        tracker.sample(Some(snapshot(1)));
        tracker.start_capture();
        // 开始采集时已经记过的这一帧不再重复收（下一次预览帧才是第一条样本）。
        let snap = snapshot(2);
        assert!(tracker.capture_frame(&snap, &[]));
        assert!(!tracker.capture_frame(&snap, &[]));
        assert_eq!(tracker.capture_len(), 1);
    }

    #[test]
    fn two_identical_editor_captures_diff_stable() {
        let baseline = finished_bundle(3);
        let candidate = finished_bundle(3);
        let diff = baseline.diff(&candidate, engine::host::DEFAULT_DRIFT_THRESHOLD_PCT);
        assert!(diff.stable, "{diff:?}");
        assert_eq!(diff.verdict, "stable");
        assert!(diff.state.equal);
        assert!(diff.slowest.is_empty());
    }
}
