//! 池化粒子系统：burst 生成粒子，按速度积分、按寿命淡出，死亡后回收槽位。
//!
//! 曲线用**起止值线性插值**表达（尺寸、颜色），物理支持重力与阻力，另有自转。
//! 没有曲线参数时行为与旧的"固定尺寸/固定颜色"一致，因此既有调用方零改动。

use std::f32::consts::TAU;

use crate::core::transform::Vec2;

/// 一次 burst 的发射参数。曲线在该 burst 的所有粒子上按寿命进度插值。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleEmitter {
    pub count: usize,
    /// 初速上限（世界单位/秒）。每个粒子在 25%–100% 之间随机取。
    pub speed: f32,
    /// 基准寿命（秒）。每个粒子在 50%–100% 之间随机取。
    pub life: f32,
    /// 边长曲线：出生 → 消亡（世界单位）。每个粒子再乘 [0.75, 1.25] 的抖动。
    pub size_start: f32,
    pub size_end: f32,
    /// 颜色曲线：出生 → 消亡（RGBA）。
    pub color_start: [f32; 4],
    pub color_end: [f32; 4],
    /// 世界重力（单位/秒²）。
    pub gravity: Vec2,
    /// 每秒速度衰减比例（`0` = 无阻力）。
    pub drag: f32,
    /// 初始旋转（弧度）。
    pub rotation: f32,
    /// 每秒角速度（弧度）。
    pub rotation_speed: f32,
}

impl Default for ParticleEmitter {
    fn default() -> Self {
        Self {
            count: 8,
            speed: 120.0,
            life: 0.6,
            size_start: 6.0,
            size_end: 6.0,
            color_start: [1.0, 0.6, 0.2, 1.0],
            color_end: [1.0, 0.6, 0.2, 1.0],
            gravity: Vec2::zero(),
            drag: 0.0,
            rotation: 0.0,
            rotation_speed: 0.0,
        }
    }
}

impl ParticleEmitter {
    /// 常用简写：固定颜色与尺寸的一次爆发。
    #[must_use]
    pub fn new(count: usize, speed: f32, life: f32, color: [f32; 4]) -> Self {
        Self {
            count,
            speed,
            life,
            color_start: color,
            color_end: color,
            ..Self::default()
        }
    }

    /// 校验：非有限或退化的参数一律拒绝（调用方应报错而不是静默生成怪粒子）。
    pub fn validate(&self) -> Result<(), String> {
        if !self.speed.is_finite() || !self.life.is_finite() || self.life <= 0.0 {
            return Err("particle speed/life must be finite and life > 0".to_owned());
        }
        if !self.size_start.is_finite()
            || !self.size_end.is_finite()
            || self.size_start < 0.0
            || self.size_end < 0.0
        {
            return Err("particle size must be finite and >= 0".to_owned());
        }
        for value in self.color_start.iter().chain(self.color_end.iter()).chain([
            &self.gravity.x,
            &self.gravity.y,
            &self.drag,
        ]) {
            if !value.is_finite() {
                return Err("particle color/gravity/drag must be finite".to_owned());
            }
        }
        if !self.rotation.is_finite() || !self.rotation_speed.is_finite() {
            return Err("particle rotation must be finite".to_owned());
        }
        if self.drag < 0.0 {
            return Err("particle drag must be >= 0".to_owned());
        }
        Ok(())
    }
}

/// One active particle in screen-space world coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub position: Vec2,
    pub velocity: Vec2,
    /// Seconds remaining before the particle is recycled.
    pub life: f32,
    /// Total life when spawned, used to compute the fade factor.
    pub max_life: f32,
    pub size_start: f32,
    pub size_end: f32,
    pub color_start: [f32; 4],
    pub color_end: [f32; 4],
    pub gravity: Vec2,
    pub drag: f32,
    pub rotation: f32,
    pub rotation_speed: f32,
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

    /// 寿命进度：`0` 出生、`1` 消亡。曲线按它插值。
    #[must_use]
    pub fn progress(&self) -> f32 {
        1.0 - self.fade()
    }

    /// 当前边长（尺寸曲线在寿命进度上的插值）。
    #[must_use]
    pub fn size(&self) -> f32 {
        lerp(self.size_start, self.size_end, self.progress())
    }

    /// 当前颜色（颜色曲线在寿命进度上的插值）。
    #[must_use]
    pub fn color(&self) -> [f32; 4] {
        let progress = self.progress();
        let mut color = [0.0; 4];
        for (index, slot) in color.iter_mut().enumerate() {
            *slot = lerp(self.color_start[index], self.color_end[index], progress);
        }
        color
    }
}

fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount.clamp(0.0, 1.0)
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
    ///
    /// 保留旧签名（固定尺寸/颜色、无重力）：内部转成默认发射参数。
    pub fn burst(&mut self, position: Vec2, count: usize, speed: f32, life: f32, color: [f32; 4]) {
        self.burst_with(position, &ParticleEmitter::new(count, speed, life, color));
    }

    /// 按发射参数生成一次爆发。参数非有限时**不生成任何粒子**（调用方应先用
    /// [`ParticleEmitter::validate`] 报错）。
    pub fn burst_with(&mut self, position: Vec2, emitter: &ParticleEmitter) {
        if emitter.count == 0 || emitter.validate().is_err() {
            return;
        }
        self.particles.reserve(emitter.count);
        for _ in 0..emitter.count {
            // 四次抽取的顺序与旧实现一致：角度 → 速度 → 寿命 → 尺寸抖动。
            // 这样默认参数下的角度/速度/寿命与旧版本逐位相同。
            let angle = self.next_rand() * TAU;
            let speed_unit = self.next_rand();
            let life_unit = self.next_rand();
            let size_unit = self.next_rand();
            let magnitude = emitter.speed * (0.25 + speed_unit * 0.75);
            let max_life = emitter.life * (0.5 + life_unit * 0.5);
            let scale = 0.75 + 0.5 * size_unit;
            self.particles.push(Particle {
                position,
                velocity: Vec2::new(angle.cos() * magnitude, angle.sin() * magnitude),
                life: max_life,
                max_life,
                size_start: emitter.size_start * scale,
                size_end: emitter.size_end * scale,
                color_start: emitter.color_start,
                color_end: emitter.color_end,
                gravity: emitter.gravity,
                drag: emitter.drag,
                rotation: emitter.rotation,
                rotation_speed: emitter.rotation_speed,
            });
        }
    }

    /// Advances particles by `dt` seconds and recycles the finished ones.
    pub fn step(&mut self, delta_seconds: f32) {
        if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return;
        }
        for particle in &mut self.particles {
            particle.velocity = particle.velocity + particle.gravity * delta_seconds;
            if particle.drag > 0.0 {
                // 每秒衰减 `drag` 比例；单步时长超过 1/drag 时钳到 0（不反转速度）。
                particle.velocity =
                    particle.velocity * (1.0 - particle.drag * delta_seconds).max(0.0);
            }
            particle.position = particle.position + particle.velocity * delta_seconds;
            particle.rotation += particle.rotation_speed * delta_seconds;
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

    /// 曲线：尺寸与颜色按寿命进度从起值走到止值。
    #[test]
    fn curves_interpolate_over_lifetime() {
        let mut system = ParticleSystem::new();
        let emitter = ParticleEmitter {
            count: 1,
            speed: 0.0,
            life: 1.0,
            size_start: 10.0,
            size_end: 2.0,
            color_start: [1.0, 0.0, 0.0, 1.0],
            color_end: [0.0, 0.0, 1.0, 0.0],
            ..ParticleEmitter::default()
        };
        system.burst_with(Vec2::zero(), &emitter);
        // 出生：进度 0 → 起值（尺寸抖动在 [0.75,1.25]，故用比例判断）。
        let fresh = system.particles()[0];
        let scale = fresh.size_start / 10.0;
        let lifetime = fresh.max_life;
        assert!((0.7..=1.3).contains(&scale), "scale={scale}");
        assert_eq!(fresh.color(), [1.0, 0.0, 0.0, 1.0]);
        assert!((fresh.size() - 10.0 * scale).abs() < 1e-4);

        // 走到半程：尺寸与颜色都应落在起止之间。寿命按粒子的实际 max_life 推进
        // （每个粒子的寿命在 [50%,100%] 随机，按绝对值步进会直接把它走死）。
        system.step(lifetime * 0.5);
        let mid = system.particles()[0];
        let size = mid.size();
        assert!(size < 10.0 * scale && size > 2.0 * scale, "mid size={size}");
        let color = mid.color();
        assert!(color[0] > 0.0 && color[0] < 1.0, "{color:?}");
        assert!(color[3] > 0.0 && color[3] < 1.0, "{color:?}");

        // 临近消亡（总进度 0.99）：接近止值。
        system.step(lifetime * 0.49);
        let old = system.particles()[0];
        assert!(old.size() < 3.0 * scale, "{}", old.size());
        assert!(old.color()[2] > 0.9, "{:?}", old.color());
    }

    #[test]
    fn gravity_and_drag_change_velocity() {
        let mut system = ParticleSystem::new();
        let emitter = ParticleEmitter {
            count: 1,
            speed: 100.0,
            life: 5.0,
            gravity: Vec2::new(0.0, -100.0),
            ..ParticleEmitter::default()
        };
        system.burst_with(Vec2::zero(), &emitter);
        let initial = system.particles()[0].velocity;
        assert!(initial.y.abs() < 100.0);
        // 纯重力：竖直速度应随步数单调下降（每个粒子的初速方向随机，故只看 y 的下降量）。
        let before = system.particles()[0].velocity;
        system.step(0.1);
        let after = system.particles()[0].velocity;
        assert!((after.y - (before.y - 10.0)).abs() < 1e-3, "{after:?}");
        assert!((after.x - before.x).abs() < 1e-6, "重力不应影响水平速度");

        // 阻力：速度幅值必须下降。
        let mut damped = ParticleSystem::new();
        damped.burst_with(
            Vec2::zero(),
            &ParticleEmitter {
                count: 1,
                speed: 100.0,
                life: 5.0,
                drag: 2.0,
                ..ParticleEmitter::default()
            },
        );
        let speed_before = damped.particles()[0].velocity.length();
        damped.step(0.1);
        let speed_after = damped.particles()[0].velocity.length();
        assert!(speed_after < speed_before, "{speed_before} → {speed_after}");
    }

    #[test]
    fn rotation_advances_and_rejects_bad_emitters() {
        let mut system = ParticleSystem::new();
        let emitter = ParticleEmitter {
            count: 1,
            life: 1.0,
            rotation: 0.5,
            rotation_speed: 2.0,
            ..ParticleEmitter::default()
        };
        system.burst_with(Vec2::zero(), &emitter);
        system.step(0.25);
        let particle = system.particles()[0];
        assert!(
            (particle.rotation - 1.0).abs() < 1e-4,
            "{}",
            particle.rotation
        );

        // 退化参数不生成粒子（调用方应先 validate 报错）。
        let mut rejected = ParticleSystem::new();
        rejected.burst_with(
            Vec2::zero(),
            &ParticleEmitter {
                count: 5,
                life: 0.0,
                ..ParticleEmitter::default()
            },
        );
        assert!(rejected.is_empty());
        assert!(ParticleEmitter::default().validate().is_ok());
        assert!(
            ParticleEmitter {
                drag: -1.0,
                ..ParticleEmitter::default()
            }
            .validate()
            .is_err()
        );
    }
}
