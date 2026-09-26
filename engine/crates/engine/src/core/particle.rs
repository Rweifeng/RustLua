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
    /// **连续发射速率**（粒子/秒，`0` = 只按 burst 发射）。
    ///
    /// 场景里声明的发射器用这个字段表达"持续冒烟/冒火"；Lua 的 `particles.burst` 不读它，
    /// 因此默认值 `0` 让既有脚本与场景的行为逐字节不变。
    pub rate: f32,
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
            rate: 0.0,
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
        if !self.rate.is_finite() || self.rate < 0.0 {
            return Err("particle rate must be finite and >= 0".to_owned());
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

    /// 解析场景里的 `ParticleEmitter` 组件串。
    ///
    /// 文法（`;` 分隔、`key=value`，未知键一律报错而不是静默忽略）：
    /// `count`、`rate`、`speed`、`life`、`size`、`size_end`、`color`、`color_end`、
    /// `gravity`、`drag`、`rotation`、`rotation_speed`。`size`/`color` 同时设置曲线的**起止**
    /// 值，`size_end`/`color_end` 覆盖终点（与书写顺序无关，见下方 `seen_*_end`）。
    pub fn from_component_str(value: &str) -> Result<Self, ParticleComponentError> {
        let mut emitter = ParticleEmitter::default();
        let mut seen = false;
        let mut seen_size_end = false;
        let mut seen_color_end = false;
        for field in value
            .trim()
            .split(';')
            .filter(|field| !field.trim().is_empty())
        {
            seen = true;
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| ParticleComponentError::InvalidField(field.to_owned()))?;
            let raw = raw.trim();
            match key.trim() {
                "count" => {
                    emitter.count = raw
                        .parse::<usize>()
                        .map_err(|_| ParticleComponentError::InvalidField(field.to_owned()))?
                }
                "rate" => emitter.rate = parse_number(raw, field)?,
                "speed" => emitter.speed = parse_number(raw, field)?,
                "life" => emitter.life = parse_number(raw, field)?,
                "size" => {
                    let value = parse_number(raw, field)?;
                    emitter.size_start = value;
                    if !seen_size_end {
                        emitter.size_end = value;
                    }
                }
                "size_end" => {
                    seen_size_end = true;
                    emitter.size_end = parse_number(raw, field)?;
                }
                "color" => {
                    let value = parse_color(raw, field)?;
                    emitter.color_start = value;
                    if !seen_color_end {
                        emitter.color_end = value;
                    }
                }
                "color_end" => {
                    seen_color_end = true;
                    emitter.color_end = parse_color(raw, field)?;
                }
                "gravity" => {
                    let (x, y) = parse_pair(raw, field)?;
                    emitter.gravity = Vec2::new(x, y);
                }
                "drag" => emitter.drag = parse_number(raw, field)?,
                "rotation" => emitter.rotation = parse_number(raw, field)?,
                "rotation_speed" => emitter.rotation_speed = parse_number(raw, field)?,
                _ => return Err(ParticleComponentError::UnknownField(field.to_owned())),
            }
        }
        if !seen {
            return Err(ParticleComponentError::Empty);
        }
        emitter
            .validate()
            .map_err(ParticleComponentError::Invalid)?;
        Ok(emitter)
    }

    /// 写回组件串。只写**非默认**的可选字段，因此 `to_component_str` → 解析 → 写回是稳定的，
    /// 且既有场景的规范字符串不会因为新增字段而变长。
    #[must_use]
    pub fn to_component_str(self) -> String {
        let mut out = format!("count={}", self.count);
        if self.rate > 0.0 {
            out.push_str(&format!(";rate={:.6}", self.rate));
        }
        out.push_str(&format!(";speed={:.6};life={:.6}", self.speed, self.life));
        out.push_str(&format!(";size={:.6}", self.size_start));
        if self.size_end != self.size_start {
            out.push_str(&format!(";size_end={:.6}", self.size_end));
        }
        out.push_str(&format!(";color={}", format_color(self.color_start)));
        if self.color_end != self.color_start {
            out.push_str(&format!(";color_end={}", format_color(self.color_end)));
        }
        if self.gravity.x != 0.0 || self.gravity.y != 0.0 {
            out.push_str(&format!(
                ";gravity={:.6},{:.6}",
                self.gravity.x, self.gravity.y
            ));
        }
        if self.drag > 0.0 {
            out.push_str(&format!(";drag={:.6}", self.drag));
        }
        if self.rotation != 0.0 {
            out.push_str(&format!(";rotation={:.6}", self.rotation));
        }
        if self.rotation_speed != 0.0 {
            out.push_str(&format!(";rotation_speed={:.6}", self.rotation_speed));
        }
        out
    }
}

/// `ParticleEmitter` 组件串的解析错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParticleComponentError {
    /// 空串不是一个发射器。
    Empty,
    /// 字段不是 `key=value` 或数值非法。
    InvalidField(String),
    /// 未知键：**报错**而不是忽略，避免"写了 `rate` 却拼成 `reate`"被静默吞掉。
    UnknownField(String),
    /// 数值通过文法但语义非法（非有限、`life <= 0`、`drag < 0` 等）。
    Invalid(String),
}

impl std::fmt::Display for ParticleComponentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(formatter, "particle emitter component is empty"),
            Self::InvalidField(field) => write!(formatter, "malformed particle field: {field}"),
            Self::UnknownField(field) => write!(formatter, "unknown particle field: {field}"),
            Self::Invalid(message) => write!(formatter, "invalid particle emitter: {message}"),
        }
    }
}

impl std::error::Error for ParticleComponentError {}

fn parse_number(raw: &str, field: &str) -> Result<f32, ParticleComponentError> {
    raw.parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| ParticleComponentError::InvalidField(field.to_owned()))
}

fn parse_pair(raw: &str, field: &str) -> Result<(f32, f32), ParticleComponentError> {
    let (x, y) = raw
        .split_once(',')
        .ok_or_else(|| ParticleComponentError::InvalidField(field.to_owned()))?;
    Ok((
        parse_number(x.trim(), field)?,
        parse_number(y.trim(), field)?,
    ))
}

fn parse_color(raw: &str, field: &str) -> Result<[f32; 4], ParticleComponentError> {
    let parts = raw
        .split(',')
        .map(|part| parse_number(part.trim(), field))
        .collect::<Result<Vec<_>, _>>()?;
    if parts.len() != 4 {
        return Err(ParticleComponentError::InvalidField(field.to_owned()));
    }
    Ok([parts[0], parts[1], parts[2], parts[3]])
}

fn format_color(color: [f32; 4]) -> String {
    format!(
        "{:.3},{:.3},{:.3},{:.3}",
        color[0], color[1], color[2], color[3]
    )
}

/// 场景声明的**连续**发射器：组件串给出发射参数，运行时按速率累计发射。
///
/// `accumulator` 是"还没攒够一个粒子"的分数部分，**只在内存里**（不序列化），因此
/// 帧率变化不会改变平均发射量，也不会因为一次长帧就整批补发。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneParticleEmitter {
    pub emitter: ParticleEmitter,
    pub accumulator: f32,
}

/// 单步最多补发多少粒子：长暂停/卡顿后不允许一步倾泻成千上万个粒子
/// （那会把帧时间尖峰变成雪崩）。超出部分丢弃，并在调用方留可诊断计数。
pub const MAX_PARTICLES_PER_STEP: usize = 512;

impl SceneParticleEmitter {
    #[must_use]
    pub const fn new(emitter: ParticleEmitter) -> Self {
        Self {
            emitter,
            accumulator: 0.0,
        }
    }

    /// 本步应发射多少粒子。`rate <= 0`（只有 burst）或 `delta_seconds` 非法时返回 0。
    pub fn take_steps(&mut self, delta_seconds: f32) -> usize {
        if self.emitter.rate <= 0.0 || !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return 0;
        }
        self.accumulator += self.emitter.rate * delta_seconds;
        if !self.accumulator.is_finite() {
            self.accumulator = 0.0;
            return 0;
        }
        let whole = self.accumulator.floor();
        let count = (whole as usize).min(MAX_PARTICLES_PER_STEP);
        self.accumulator -= whole;
        count
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
        self.step_with_parallelism(delta_seconds, crate::core::par::Parallelism::SEQUENTIAL)
    }

    /// P1-③：并行推进粒子（系统级数据并行）。逐粒子积分彼此独立（无浮点归约），
    /// 按块并行；回收压缩与串行完全一致（只依赖 `life > 0`，与更新顺序无关）——
    /// 因此**串/并行结果逐位相同**（docs/40 WP8：系统内纯数据子任务并行；调度器
    /// 保持确定性单线程）。`Parallelism::SEQUENTIAL` 完全回滚到单线程。
    pub fn step_with_parallelism(
        &mut self,
        delta_seconds: f32,
        parallelism: crate::core::par::Parallelism,
    ) {
        if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return;
        }
        let particles = &mut self.particles;
        let threads = parallelism.effective_threads(particles.len());
        if threads < 2 {
            for particle in particles.iter_mut() {
                step_particle(particle, delta_seconds);
            }
            particles.retain(|particle| particle.life > 0.0);
            return;
        }
        let chunk = particles.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for batch in particles.chunks_mut(chunk) {
                scope.spawn(move || {
                    for particle in batch.iter_mut() {
                        step_particle(particle, delta_seconds);
                    }
                });
            }
        });
        particles.retain(|particle| particle.life > 0.0);
    }
}

/// 单粒子一步积分：`step` 与 `step_with_parallelism` 共用同一份，保证串/并行逐位一致。
fn step_particle(particle: &mut Particle, delta_seconds: f32) {
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

impl ParticleSystem {
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

    /// P1-③：串/并行 step 逐位一致（同 seed、同 burst、同 dt；压缩只依赖 life>0）。
    #[test]
    fn parallel_step_matches_sequential_bit_for_bit() {
        let emitter = ParticleEmitter::new(10_000, 12.0, 3.0, [1.0, 0.5, 0.25, 1.0]);
        let mut sequential = ParticleSystem::new();
        let mut parallel = ParticleSystem::new();
        sequential.burst_with(Vec2::zero(), &emitter);
        parallel.burst_with(Vec2::zero(), &emitter);
        let parallel_degree = crate::core::par::Parallelism::with_threads(4);
        sequential.step(0.016);
        parallel.step_with_parallelism(0.016, parallel_degree);
        assert_eq!(sequential.particles(), parallel.particles());
        // 多步后死亡粒子的压缩结果同样一致。
        for _ in 0..90 {
            sequential.step(0.016);
            parallel.step_with_parallelism(0.016, parallel_degree);
        }
        assert_eq!(sequential.particles(), parallel.particles());
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
        assert!(
            ParticleEmitter {
                rate: -1.0,
                ..ParticleEmitter::default()
            }
            .validate()
            .is_err()
        );
    }

    /// docs/54 §8-①：粒子发射器是**场景组件**，因此组件串必须能往返、且拼错要报错。
    #[test]
    fn emitter_component_string_round_trips_and_rejects_bad_input() {
        let emitter = ParticleEmitter::from_component_str(
            "count=4;rate=30;speed=90;life=0.5;size=3;color=1,0.7,0.3,1",
        )
        .expect("parse");
        assert_eq!(emitter.count, 4);
        assert_eq!(emitter.rate, 30.0);
        assert_eq!(emitter.size_start, 3.0);
        assert_eq!(emitter.size_end, 3.0, "只给 size 时起止同值");
        assert_eq!(
            emitter.color_end, emitter.color_start,
            "只给 color 时起止同色"
        );

        // 往返稳定：解析 → 写回 → 再解析得到同一个发射器。
        let text = emitter.to_component_str();
        assert_eq!(
            ParticleEmitter::from_component_str(&text).expect("reparse"),
            emitter
        );
        assert_eq!(
            ParticleEmitter::from_component_str(&ParticleEmitter::default().to_component_str())
                .expect("default round trip"),
            ParticleEmitter::default()
        );

        // 曲线终点可以被单独指定，且与书写顺序无关。
        for text in ["count=1;size=2;size_end=8", "count=1;size_end=8;size=2"] {
            let parsed = ParticleEmitter::from_component_str(text).expect(text);
            assert_eq!(parsed.size_start, 2.0, "{text}");
            assert_eq!(parsed.size_end, 8.0, "{text}");
        }
        assert_eq!(
            ParticleEmitter::from_component_str("count=1;color=1,0,0,1;color_end=0,0,0,0")
                .expect("color curve")
                .color_end,
            [0.0, 0.0, 0.0, 0.0]
        );

        // 拼错/未知键/退化数值一律报错，不静默忽略。
        assert_eq!(
            ParticleEmitter::from_component_str(""),
            Err(ParticleComponentError::Empty)
        );
        assert!(matches!(
            ParticleEmitter::from_component_str("count=1;reate=30"),
            Err(ParticleComponentError::UnknownField(_))
        ));
        assert!(matches!(
            ParticleEmitter::from_component_str("count=abc"),
            Err(ParticleComponentError::InvalidField(_))
        ));
        assert!(matches!(
            ParticleEmitter::from_component_str("count=1;life=0"),
            Err(ParticleComponentError::Invalid(_))
        ));
        assert!(matches!(
            ParticleEmitter::from_component_str("count=1;color=1,0,0"),
            Err(ParticleComponentError::InvalidField(_))
        ));
    }

    /// 连续发射按**速率 × 时间**累计：帧率变化不改变平均发射量，长帧也不会整批补发。
    #[test]
    fn scene_emitter_accumulates_fractional_steps() {
        let emitter = ParticleEmitter {
            count: 1,
            rate: 30.0,
            ..ParticleEmitter::default()
        };
        let mut scene = SceneParticleEmitter::new(emitter);
        // 60 Hz：每步 0.5 个 ⇒ 两步发 1 个。
        assert_eq!(scene.take_steps(1.0 / 60.0), 0);
        assert_eq!(scene.take_steps(1.0 / 60.0), 1);
        // 30 Hz：每步 1 个。
        assert_eq!(scene.take_steps(1.0 / 30.0), 1);
        // 长帧被上限夹住，不会一步倾泻。
        assert_eq!(scene.take_steps(60.0), MAX_PARTICLES_PER_STEP);
        // burst-only（rate = 0）与非法 dt 都不发射，且不动累计器。
        let mut burst_only = SceneParticleEmitter::new(ParticleEmitter::default());
        assert_eq!(burst_only.take_steps(1.0), 0);
        assert_eq!(burst_only.accumulator, 0.0);
        assert_eq!(scene.take_steps(f32::NAN), 0);
        assert_eq!(scene.take_steps(0.0), 0);
    }
}
