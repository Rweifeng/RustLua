//! Reusable engine runtime: scene instantiation + GPU rendering, extracted from
//! the game executable so editors and hosts can embed a live game view (M10).
//!
//! - [`RuntimeScene`] instantiates the project scene/scripts and produces
//!   per-frame [`SceneMesh`] (2D) and [`Scene3d`] draw data.
//! - [`Renderer`] consumes those frames and renders 2D/3D passes to a window
//!   surface (and, in M10-02, an offscreen texture target).

/// 帧调度分档统计（M74-02 P0 度量闸门）：每系统 p50/p99、每 stage 合计与占帧比。
pub mod frame_profile;
/// 手柄设备轮询适配层（M20-06）。
pub mod gamepad;
/// 平台事件 → 引擎物理键身份的唯一转换层（M22/W1）。
pub mod keymap;
/// P3-E06-4：可复现的性能采集包、重放对比与差分（场景/输入/读数/状态指纹）。
pub mod profile_capture;
/// P3-E06-2：有界 span 环形缓冲与 Chrome Trace Event 导出（编辑器与 headless 证据共用）。
pub mod profile_trace;
pub mod runtime_scene;
/// 示例工程 `examples/game_api` 的工程级门（headless 步进 / 脚本编译 / API 与组件覆盖率）。
#[cfg(test)]
mod sample_project;
/// 表现层组件 codec 的装表入口（P1 ④）：组合根在这里把 render/ui 的 codec 接给 scene。
pub mod scene_codecs;
/// 场景文本 → 渲染层类型的旧提取（P1 ④ 之后从 `rustlua-scene` 搬来：它产出 render 的类型）。
pub mod scene_extract;

pub use frame_profile::{
    FrameProfile, FrameProfileSummary, PARALLELISM_GATE_SHARE_PCT, StageProfile, SystemProfile,
    TimingStats,
};
pub use gamepad::{
    FakeBackend, GamepadBackend, GamepadDeadzone, GamepadFrame, GamepadPoller,
    button as gamepad_button,
};
pub use profile_capture::{
    CAPTURE_DIFF_VERSION, CAPTURE_REPORT_VERSION, CaptureBundle, CaptureDiff, CaptureIdentity,
    CaptureScenario, CapturedInput, DEFAULT_DRIFT_THRESHOLD_PCT, ENGINE_VERSION,
    FingerprintBuilder, FrameDelta, FrameStats, MIN_GATED_P50_US, MemorySnapshot, StateDiff,
    TimingDelta, TimingStat, TimingStatus, fingerprint_bytes, now_unix_ms,
};
pub use profile_trace::{
    CaptureLevel, DEFAULT_SPAN_CAPACITY, SpanRecord, SpanStat, SpanTrace, SpanTraceSummary,
    TRACE_OVERHEAD_BUDGET_PCT, TRACE_REPORT_VERSION, capture_level_for_overhead, overhead_pct,
    overhead_within_budget,
};
pub use runtime_scene::{
    EnvironmentApplyReport, EnvironmentReport, RuntimeScene, RuntimeSceneEnvironment,
    RuntimeSpawnedEntity, apply_to_renderer as apply_scene_environment, load_project_font_atlas,
    load_project_vector_atlas,
};
pub use rustlua_render::frame::{
    BoundsSource, OverlayVertex, Scene3d, Scene3dBounds, Scene3dDrawable, SceneDraw, SceneMesh,
};
/// P3-R01：渲染能力开关与场景颜色格式的对外类型（宿主/编辑器/验收共用同一份定义）。
pub use rustlua_render::render_settings::{RenderSettings, SceneColorFormat, ToggleMode};
/// 脚本错误处置策略：宿主在 `RuntimeScene` 上选择严格（验收默认）或容错（交互宿主）。
pub use rustlua_scripting::script::ScriptErrorPolicy;
