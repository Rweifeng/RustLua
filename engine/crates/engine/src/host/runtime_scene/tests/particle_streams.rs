//! Lua `particles.stream3d` → renderer continuous-stream integration.

use super::*;
use rustlua_render::runtime_gpu::{ParticleRenderer, ParticleStepReport};
use std::collections::HashMap;

#[derive(Default)]
struct FakeStreams {
    supported: bool,
    streams: HashMap<u64, ([f32; 3], ParticleEmitter)>,
    creates: usize,
    updates: usize,
    removes: usize,
}

impl FakeStreams {
    fn supported() -> Self {
        Self {
            supported: true,
            ..Self::default()
        }
    }
}

impl ParticleRenderer for FakeStreams {
    fn compute_enabled(&self) -> bool {
        true
    }

    fn push_particle_burst(
        &mut self,
        _origin: [f32; 2],
        _emitter: ParticleEmitter,
        _capacity: u32,
    ) -> Result<(), String> {
        Ok(())
    }

    fn particle_systems(&self) -> usize {
        self.streams.len()
    }

    fn supports_particle_streams(&self) -> bool {
        self.supported
    }

    fn create_particle_stream(
        &mut self,
        key: u64,
        origin: [f32; 3],
        emitter: ParticleEmitter,
        _capacity: u32,
    ) -> Result<(), String> {
        self.creates += 1;
        self.streams.insert(key, (origin, emitter));
        Ok(())
    }

    fn update_particle_stream(&mut self, key: u64, origin: [f32; 3]) -> bool {
        let Some(entry) = self.streams.get_mut(&key) else {
            return false;
        };
        entry.0 = origin;
        self.updates += 1;
        true
    }

    fn remove_particle_stream(&mut self, key: u64) -> bool {
        self.removes += 1;
        self.streams.remove(&key).is_some()
    }

    fn step_particles(&mut self, _delta_seconds: f32) -> Result<ParticleStepReport, String> {
        Ok(ParticleStepReport {
            systems: self.streams.len(),
            recycled: 0,
            steps: 1,
        })
    }

    fn spawned_particles(&self) -> Option<u32> {
        None
    }
}

fn stream_scene(name: &str) -> RuntimeScene {
    RuntimeScene::from_scene(None, rustlua_scene::scene::Scene::new(name)).unwrap()
}

#[test]
fn lua_streams_create_update_and_clear_renderer_streams() {
    let mut runtime = stream_scene("lua-streams");
    let path = write_guard_script(
        "lua-streams",
        "frames = 0\n\
         function update()\n\
             frames = frames + 1\n\
             if frames == 1 then\n\
                 particles.stream3d('rain', 1, 2, 3, 60, {life=1, gravity_z=-9, size=0.1, area_x=8, area_z=8})\n\
             elseif frames == 2 then\n\
                 particles.stream3d('rain', 4, 5, 6, 60, {life=1, gravity_z=-9, size=0.1, area_x=8, area_z=8})\n\
             else\n\
                 particles.clear_streams3d()\n\
             end\n\
             return tostring(particles.stream_count())\n\
         end",
    );
    runtime
        .with_script(|script| script.execute_project_script(&path))
        .unwrap();

    let mut renderer = FakeStreams::supported();
    runtime
        .run_script_steps(1, 0.1, &rustlua_input::input::InputSnapshot::default())
        .unwrap();
    let report = runtime.dispatch_gpu_particle_bursts(&mut renderer, 0.1);
    assert_eq!(report.lua_streams, 1);
    assert_eq!(renderer.streams.len(), 1);
    let (key, (origin, emitter)) = renderer.streams.iter().next().unwrap();
    assert_eq!(*origin, [1.0, 2.0, 3.0]);
    assert_eq!(emitter.rate, 60.0);
    assert_eq!(emitter.spawn_area, [8.0, 0.0, 8.0]);
    let first_key = *key;
    assert!(
        first_key >= 1 << 63,
        "Lua streams must use a key namespace disjoint from scene entity ids"
    );

    // Same key, new origin: update in place, no extra stream.
    runtime
        .run_script_steps(1, 0.1, &rustlua_input::input::InputSnapshot::default())
        .unwrap();
    let report = runtime.dispatch_gpu_particle_bursts(&mut renderer, 0.1);
    assert_eq!(report.lua_streams, 1);
    assert_eq!(renderer.creates, 1);
    assert_eq!(renderer.updates, 1);
    assert_eq!(renderer.streams[&first_key].0, [4.0, 5.0, 6.0]);

    // Clearing Lua declarations removes the renderer stream next dispatch.
    runtime
        .run_script_steps(1, 0.1, &rustlua_input::input::InputSnapshot::default())
        .unwrap();
    let report = runtime.dispatch_gpu_particle_bursts(&mut renderer, 0.1);
    assert_eq!(report.lua_streams, 0);
    assert!(renderer.streams.is_empty());
    assert!(renderer.removes >= 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn lua_streams_fall_back_to_cpu_when_backend_has_no_streams() {
    let mut runtime = stream_scene("lua-streams-cpu");
    let path = write_guard_script(
        "lua-streams-cpu",
        "function update()\n\
             particles.stream3d('mist', 0, 1, 0, 100, {life=2, size=0.5})\n\
             return 'ok'\n\
         end",
    );
    runtime
        .with_script(|script| script.execute_project_script(&path))
        .unwrap();
    runtime
        .run_script_steps(1, 0.1, &rustlua_input::input::InputSnapshot::default())
        .unwrap();
    let mut renderer = FakeStreams::default();
    let report = runtime.dispatch_gpu_particle_bursts(&mut renderer, 0.1);
    assert_eq!(report.lua_streams, 0);
    assert!(report.fallback >= 1, "unsupported streams must fall back");
    assert_eq!(renderer.streams.len(), 0);
    assert!(
        runtime.script.particle_count() >= 10,
        "fallback should emit about rate*dt particles"
    );
    let _ = std::fs::remove_file(path);
}
