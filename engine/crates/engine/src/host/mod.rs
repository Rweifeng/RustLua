//! Reusable engine runtime: scene instantiation + GPU rendering, extracted from
//! the game executable so editors and hosts can embed a live game view (M10).
//!
//! - [`RuntimeScene`] instantiates the project scene/scripts and produces
//!   per-frame [`SceneMesh`] (2D) and [`Scene3d`] draw data.
//! - [`Renderer`] consumes those frames and renders 2D/3D passes to a window
//!   surface (and, in M10-02, an offscreen texture target).

pub mod frame;
/// 手柄设备轮询适配层（M20-06）。
pub mod gamepad;
/// 平台事件 → 引擎物理键身份的唯一转换层（M22/W1）。
pub mod keymap;
pub mod renderer;
pub mod runtime_scene;
/// 示例工程 `examples/game_api` 的工程级门（headless 步进 / 脚本编译 / API 与组件覆盖率）。
#[cfg(test)]
mod sample_project;
pub mod shader;

pub use frame::{
    BoundsSource, OverlayVertex, Scene3d, Scene3dBounds, Scene3dDrawable, SceneDraw, SceneMesh,
};
pub use gamepad::{
    FakeBackend, GamepadBackend, GamepadDeadzone, GamepadFrame, GamepadPoller,
    button as gamepad_button,
};
pub use renderer::{ReadbackMetrics, ReadbackStatus, RenderFrameMetrics, RenderOutcome, Renderer};
pub use runtime_scene::{RuntimeScene, load_project_font_atlas, load_project_vector_atlas};
/// 脚本错误处置策略：宿主在 `RuntimeScene` 上选择严格（验收默认）或容错（交互宿主）。
pub use rustlua_scripting::script::ScriptErrorPolicy;
