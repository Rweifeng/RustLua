//! Reusable engine runtime: scene instantiation + GPU rendering, extracted from
//! the game executable so editors and hosts can embed a live game view (M10).
//!
//! - [`RuntimeScene`] instantiates the project scene/scripts and produces
//!   per-frame [`SceneMesh`] (2D) and [`Scene3d`] draw data.
//! - [`Renderer`] consumes those frames and renders 2D/3D passes to a window
//!   surface (and, in M10-02, an offscreen texture target).

pub mod frame;
pub mod renderer;
pub mod runtime_scene;
pub mod shader;

pub use frame::{
    BoundsSource, OverlayVertex, Scene3d, Scene3dBounds, Scene3dDrawable, SceneDraw, SceneMesh,
};
pub use renderer::{ReadbackMetrics, ReadbackStatus, RenderOutcome, Renderer};
pub use runtime_scene::RuntimeScene;
