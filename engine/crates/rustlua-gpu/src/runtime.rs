use rustlua_assets::env_asset::EnvironmentAsset;
use rustlua_ecs::particle::ParticleEmitter;
use rustlua_render::render_settings::{
    EnvironmentLighting, EnvironmentSettings, IblMode, IblTier, RenderSettings,
};
use rustlua_render::runtime_gpu::{EnvironmentRenderer, ParticleRenderer, ParticleStepReport};

use crate::{GpuParticleConfig, Renderer};

impl EnvironmentRenderer for Renderer {
    fn ibl_mode(&self) -> IblMode {
        Renderer::ibl_mode(self)
    }

    fn ibl_tier(&self) -> IblTier {
        Renderer::ibl_tier(self)
    }

    fn environment_map_info(&self) -> Option<(u32, u32)> {
        Renderer::environment_map_info(self)
    }

    fn clear_environment(&mut self) {
        Renderer::clear_environment(self);
    }

    fn render_settings(&self) -> RenderSettings {
        Renderer::render_settings(self)
    }

    fn set_render_settings(&mut self, settings: RenderSettings) {
        Renderer::set_render_settings(self, settings);
    }

    fn set_environment_asset(
        &mut self,
        asset: &EnvironmentAsset,
        settings: EnvironmentSettings,
    ) -> Result<(), String> {
        Renderer::set_environment_asset(self, asset, settings).map_err(|error| error.to_string())
    }

    fn set_environment(&mut self, lighting: EnvironmentLighting) {
        Renderer::set_environment(self, lighting);
    }
}

impl ParticleRenderer for Renderer {
    fn compute_enabled(&self) -> bool {
        Renderer::compute_enabled(self)
    }

    fn push_particle_burst(
        &mut self,
        origin: [f32; 2],
        emitter: ParticleEmitter,
        capacity: u32,
    ) -> Result<(), String> {
        self.push_gpu_particle_burst(GpuParticleConfig {
            origin,
            emitter,
            capacity,
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn particle_systems(&self) -> usize {
        self.gpu_particle_systems()
    }

    fn step_particles(&mut self, delta_seconds: f32) -> Result<ParticleStepReport, String> {
        self.step_gpu_particles(delta_seconds)
            .map(|step| ParticleStepReport {
                systems: step.systems,
                recycled: step.recycled,
                steps: step.steps,
            })
            .map_err(|error| error.to_string())
    }

    fn spawned_particles(&self) -> Option<u32> {
        self.gpu_particle_status().map(|status| status.spawned)
    }
}
