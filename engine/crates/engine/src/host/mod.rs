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
pub mod shader;

pub use frame::{
    BoundsSource, OverlayVertex, Scene3d, Scene3dBounds, Scene3dDrawable, SceneDraw, SceneMesh,
};
pub use gamepad::{
    FakeBackend, GamepadBackend, GamepadDeadzone, GamepadFrame, GamepadPoller,
    button as gamepad_button,
};
pub use renderer::{ReadbackMetrics, ReadbackStatus, RenderOutcome, Renderer};
pub use runtime_scene::{RuntimeScene, load_project_font_atlas, load_project_vector_atlas};
