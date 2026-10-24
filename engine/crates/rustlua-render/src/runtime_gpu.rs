use rustlua_assets::env_asset::EnvironmentAsset;
use rustlua_ecs::particle::ParticleEmitter;

use crate::render_settings::{
    EnvironmentLighting, EnvironmentSettings, IblMode, IblTier, RenderSettings,
};

pub trait EnvironmentRenderer {
    fn ibl_mode(&self) -> IblMode;
    fn ibl_tier(&self) -> IblTier;
    fn environment_map_info(&self) -> Option<(u32, u32)>;
    fn clear_environment(&mut self);
    fn render_settings(&self) -> RenderSettings;
    fn set_render_settings(&mut self, settings: RenderSettings);
    fn set_environment_asset(
        &mut self,
        asset: &EnvironmentAsset,
        settings: EnvironmentSettings,
    ) -> Result<(), String>;
    fn set_environment(&mut self, lighting: EnvironmentLighting);
}

pub struct ParticleStepReport {
    pub systems: usize,
    pub recycled: usize,
    pub steps: u64,
}

pub trait ParticleRenderer {
    fn compute_enabled(&self) -> bool;
    fn push_particle_burst(
        &mut self,
        origin: [f32; 2],
        emitter: ParticleEmitter,
        capacity: u32,
    ) -> Result<(), String>;
    fn particle_systems(&self) -> usize;
    fn step_particles(&mut self, delta_seconds: f32) -> Result<ParticleStepReport, String>;
    fn spawned_particles(&self) -> Option<u32>;
}
