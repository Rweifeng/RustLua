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
    /// 三维 burst。默认实现退化为 2D（忽略 z），纯 CPU mock 不必实现；
    /// GPU 后端覆盖它并保留 `origin[2]`。
    fn push_particle_burst_3d(
        &mut self,
        origin: [f32; 3],
        emitter: ParticleEmitter,
        capacity: u32,
    ) -> Result<(), String> {
        self.push_particle_burst([origin[0], origin[1]], emitter, capacity)
    }
    fn particle_systems(&self) -> usize;
    /// 后端是否支持**连续粒子流**（`rate > 0` 的 GPU 发射）。
    ///
    /// 默认 `false`：旧 mock 与没有环形队列的后端继续让 rate 回 CPU，落点判定不谎报。
    fn supports_particle_streams(&self) -> bool {
        false
    }
    /// 创建连续流。默认拒绝，调用方据此回 CPU。
    fn create_particle_stream(
        &mut self,
        key: u64,
        origin: [f32; 3],
        emitter: ParticleEmitter,
        capacity: u32,
    ) -> Result<(), String> {
        let _ = (key, origin, emitter, capacity);
        Err("backend has no particle stream support".to_owned())
    }
    /// 更新连续流原点；false = key 不在池里，调用方应重建。
    fn update_particle_stream(&mut self, key: u64, origin: [f32; 3]) -> bool {
        let _ = (key, origin);
        false
    }
    /// 删除连续流；false = key 本来就不在池里。
    fn remove_particle_stream(&mut self, key: u64) -> bool {
        let _ = key;
        false
    }
    /// 把宿主/脚本层的全局风传给 GPU 粒子后端；默认 no-op，纯 CPU mock 不受影响。
    fn set_particle_wind(&mut self, wind: [f32; 2]) {
        let _ = wind;
    }
    /// 三维风：默认把 x/y 转给 2D 方法，纯 CPU mock 不必关心 z。
    fn set_particle_wind_3d(&mut self, wind: [f32; 3]) {
        self.set_particle_wind([wind[0], wind[1]]);
    }
    /// 风场 `(shear, gust, gust_hz)`：高度剪切 + 空间阵风。默认 no-op。
    fn set_particle_wind_profile(&mut self, profile: [f32; 3]) {
        let _ = profile;
    }
    /// 4 个局部风源 `[x, y(height), z, radius, radial, lift]`；默认 no-op。
    fn set_particle_wind_sources(&mut self, sources: [[f32; 6]; 4]) {
        let _ = sources;
    }
    fn step_particles(&mut self, delta_seconds: f32) -> Result<ParticleStepReport, String>;
    fn spawned_particles(&self) -> Option<u32>;
}
