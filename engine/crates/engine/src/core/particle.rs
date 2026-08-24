//! A small pooled particle system: bursts spawn particles that integrate with a
//! velocity, fade out over their life, and are recycled back into the pool.

use crate::core::transform::Vec2;

/// One active particle in screen-space world coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub position: Vec2,
    pub velocity: Vec2,
    /// Seconds remaining before the particle is recycled.
    pub life: f32,
    /// Total life when spawned, used to compute the fade factor.
    pub max_life: f32,
    /// World-space side length of the particle quad.
    pub size: f32,
    pub color: [f32; 4],
}

impl Particle {
    /// Returns a `0..=1` fade factor, `1` fresh and `0` just before death.
    #[must_use]
    pub fn fade(&self) -> f32 {
        if self.max_life <= 0.0 {
            return 1.0;
        }
        (self.life / self.max_life).clamp(0.0, 1.0)
    }
}

/// Owns a pool of active particles. Dead particles are compacted away so their
/// slots are reused by the next burst.
#[derive(Debug, Default)]
pub struct ParticleSystem {
    particles: Vec<Particle>,
    /// Deterministic LCG so bursts are reproducible.
    seed: u64,
}

impl ParticleSystem {
    /// Creates an empty particle system with a fixed seed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            particles: Vec::new(),
            seed: 0x1234_5678_9abc_def0,
        }
    }

    /// Spawns `count` particles at `position` bursting outward up to `speed`
    /// world units per second, fading from `color` over roughly `life` seconds.
    pub fn burst(&mut self, position: Vec2, count: usize, speed: f32, life: f32, color: [f32; 4]) {
        let mut spawned = Vec::with_capacity(count);
        for _ in 0..count {
            let angle = self.next_rand() * std::f32::consts::TAU;
            let magnitude = speed * (0.25 + self.next_rand() * 0.75);
            let max_life = life * (0.5 + self.next_rand() * 0.5);
            let size = 4.0 + self.next_rand() * 4.0;
            spawned.push(Particle {
                position,
                velocity: Vec2::new(angle.cos() * magnitude, angle.sin() * magnitude),
                life: max_life,
                max_life,
                size,
                color,
            });
        }
        self.particles.extend(spawned);
    }

    /// Advances particles by `dt` seconds and recycles the finished ones.
    pub fn step(&mut self, delta_seconds: f32) {
        if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return;
        }
        for particle in &mut self.particles {
            particle.position.x += particle.velocity.x * delta_seconds;
            particle.position.y += particle.velocity.y * delta_seconds;
            particle.life -= delta_seconds;
        }
        // Compact out dead particles, recycling their slots.
        self.particles.retain(|particle| particle.life > 0.0);
    }

    /// Returns the active particles.
    #[must_use]
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    /// Returns the number of active particles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.particles.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    fn next_rand(&mut self) -> f32 {
        self.seed = self
            .seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.seed >> 33) as f32) / (1u64 << 31) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_spawns_and_updates_particles() {
        let mut system = ParticleSystem::new();
        system.burst(Vec2::zero(), 20, 50.0, 1.0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(system.len(), 20);
        let before = system.particles()[0];
        system.step(0.1);
        let after = system.particles()[0];
        assert_ne!(after.position, before.position, "particle should move");
        assert!(after.life < before.life, "particle life should decrease");
    }

    #[test]
    fn finished_particles_are_recycled() {
        let mut system = ParticleSystem::new();
        system.burst(Vec2::zero(), 10, 10.0, 0.2, [1.0, 1.0, 1.0, 1.0]);
        for _ in 0..30 {
            system.step(0.1);
        }
        assert_eq!(system.len(), 0, "all particles should expire and recycle");
        // the pool can be reused for a new burst
        system.burst(Vec2::zero(), 5, 10.0, 1.0, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(system.len(), 5);
    }

    #[test]
    fn fade_goes_from_one_to_zero() {
        let mut system = ParticleSystem::new();
        system.burst(Vec2::zero(), 1, 0.0, 1.0, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(system.particles()[0].fade(), 1.0);
        system.step(0.5);
        let fade = system.particles()[0].fade();
        assert!(fade > 0.0 && fade < 1.0, "fade={fade}");
    }
}
