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
    GpuParticleConfig3D, GpuParticleSnapshot, GpuParticleState, GpuParticleStatus, GpuParticleStep,
    GpuParticleStreamConfig, GpuPassReport, PassGpuTiming, ReadbackMetrics, ReadbackStatus,
    RenderFrameMetrics, RenderOutcome, Renderer, RendererCapabilities, RendererResourceStats,
    SceneColorReadback, write_capture_placeholder,
};
#[cfg(not(target_arch = "wasm32"))]
pub use renderer::{ShaderReloadError, ShaderReloadReport, ShaderSourceStatus};
