//! Reusable engine runtime: scene instantiation + GPU rendering, extracted from
//! the game executable so editors and hosts can embed a live game view (M10).
//!
//! - [`RuntimeScene`] instantiates the project scene/scripts and produces
//!   per-frame [`SceneMesh`] (2D) and [`Scene3d`] draw data.
//! - [`Renderer`] consumes those frames and renders 2D/3D passes to a window
//!   surface (and, in M10-02, an offscreen texture target).

/// 声明视口 → 目标附件里的等比居中矩形（宿主与渲染器共用的唯一口径）。
pub mod fit;
pub mod frame;
/// 帧调度分档统计（M74-02 P0 度量闸门）：每系统 p50/p99、每 stage 合计与占帧比。
pub mod frame_profile;
/// 手柄设备轮询适配层（M20-06）。
pub mod gamepad;
/// 平台事件 → 引擎物理键身份的唯一转换层（M22/W1）。
pub mod keymap;
pub mod renderer;
pub mod runtime_scene;
/// 示例工程 `examples/game_api` 的工程级门（headless 步进 / 脚本编译 / API 与组件覆盖率）。
#[cfg(test)]
mod sample_project;
/// 表现层组件 codec 的装表入口（P1 ④）：组合根在这里把 render/ui 的 codec 接给 scene。
pub mod scene_codecs;
/// 场景文本 → 渲染层类型的旧提取（P1 ④ 之后从 `rustlua-scene` 搬来：它产出 render 的类型）。
pub mod scene_extract;
pub mod shader;
/// wasm 上"从画布建 `wgpu::Surface`"的唯一实现（web 宿主与探针共用，见模块注释）。
#[cfg(target_arch = "wasm32")]
pub mod web_surface;

pub use frame::{
    BoundsSource, OverlayVertex, Scene3d, Scene3dBounds, Scene3dDrawable, SceneDraw, SceneMesh,
};
pub use frame_profile::{
    FrameProfile, FrameProfileSummary, PARALLELISM_GATE_SHARE_PCT, StageProfile, SystemProfile,
    TimingStats,
};
pub use gamepad::{
    FakeBackend, GamepadBackend, GamepadDeadzone, GamepadFrame, GamepadPoller,
    button as gamepad_button,
};
pub use renderer::{
    ComputeBinding, ComputeDispatch, ComputeError, ComputeRequest, ComputeStats, GpuParticleConfig,
    GpuParticleSnapshot, GpuParticleState, GpuParticleStatus, GpuParticleStep, ReadbackMetrics,
    ReadbackStatus, RenderFrameMetrics, RenderOutcome, Renderer, RendererCapabilities,
    RendererResourceStats,
};
pub use runtime_scene::{
    RuntimeScene, RuntimeSpawnedEntity, load_project_font_atlas, load_project_vector_atlas,
};
/// 脚本错误处置策略：宿主在 `RuntimeScene` 上选择严格（验收默认）或容错（交互宿主）。
pub use rustlua_scripting::script::ScriptErrorPolicy;
