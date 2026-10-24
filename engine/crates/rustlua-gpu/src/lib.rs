#[cfg(not(target_arch = "wasm32"))]
pub mod offscreen;
pub mod renderer;
mod runtime;
pub mod shader;
#[cfg(not(target_arch = "wasm32"))]
pub mod shader_library;
pub mod shader_preprocess;
#[cfg(not(target_arch = "wasm32"))]
pub mod shader_reload;
#[cfg(target_arch = "wasm32")]
pub mod web_surface;

#[cfg(not(target_arch = "wasm32"))]
pub use offscreen::{OffscreenDevice, OffscreenError, OffscreenRequest, OffscreenTarget};
pub use renderer::{
    ComputeBinding, ComputeDispatch, ComputeError, ComputeRequest, ComputeStats, GpuParticleConfig,
    GpuParticleSnapshot, GpuParticleState, GpuParticleStatus, GpuParticleStep, GpuPassReport,
    PassGpuTiming, ReadbackMetrics, ReadbackStatus, RenderFrameMetrics, RenderOutcome, Renderer,
    RendererCapabilities, RendererResourceStats, SceneColorReadback,
};
#[cfg(not(target_arch = "wasm32"))]
pub use renderer::{ShaderReloadError, ShaderReloadReport, ShaderSourceStatus};
