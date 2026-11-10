//! 池化粒子系统：burst 生成粒子，按速度积分、按寿命淡出，死亡后回收槽位。
//!
//! 曲线用**起止值线性插值**表达（尺寸、颜色），物理支持重力与阻力，另有自转。
//! 没有曲线参数时行为与旧的"固定尺寸/固定颜色"一致，因此既有调用方零改动。

use std::f32::consts::TAU;

use crate::transform::Vec2;

/// 一次 burst 的发射参数。曲线在该 burst 的所有粒子上按寿命进度插值。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleEmitter {
    pub count: usize,
    /// **连续发射速率**（粒子/秒，`0` = 只按 burst 发射）。
    ///
    /// 场景里声明的发射器用这个字段表达"持续冒烟/冒火"；Lua 的 `particles.burst` 不读它，
    /// 因此默认值 `0` 让既有脚本与场景的行为逐字节不变。`mode=gpu` 且渲染器支持连续流时，
    /// 这个速率由渲染器的环形槽按同一 `rate × dt` 抽样。
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
    /// Z 轴重力（单位/秒²）。默认 `0`，2D 发射器逐位不变。
    pub gravity_z: f32,
    /// 浮力（单位/秒²）：按剩余寿命比例向上推（新粒子更热/更轻），默认 `0`。
    pub buoyancy: f32,
    /// 逐粒子颤动幅度（速度，单位/秒）：每颗粒子用自己稳定的随机相位在 x/y/z 上摆动，
    /// 同类粒子不再像一整块随风平移。默认 `0`：不进入该分支，旧粒子逐位不变。
    pub flutter: f32,
    /// 逐粒子颤动频率（Hz，`0..=20`）。与 `flutter` 一起生效。
    pub flutter_hz: f32,
    /// 沿寿命的路径弯曲（角速度，弧度/秒）：水平面 (x,y) 上每秒把速度方向转过这么多，
    /// 转角按剩余寿命比例衰减（新颗粒转得最快，`buoyancy` 同一 ramp 口径）。
    /// 只转方向不改速率，因此弧线不会把粒子甩成轨道。默认 `0`：旧粒子逐位不变。
    pub curve: f32,
    /// 程序化 curl 噪声风场强度（速度，单位/秒）。
    ///
    /// 场取三维势函数 `Ψ=(sin a·cos b, sin b·cos c, sin c·cos a)` 的旋度，因此**严格无散**
    /// （`∇·w = 0`）：局部只有旋转与剪切、没有源和汇，粒子不会被吸进几个点或从点里喷出。
    /// 相位 `a/b/c` 由世界坐标乘空间频率再叠时间推进，所以相邻粒子共享方向（空间相干），
    /// 整片流场会随时间翻滚。默认 `0`：不进入该分支，旧粒子逐位不变。
    pub curl: f32,
    /// curl 噪声的空间频率（1/世界单位）：涡旋尺度约 `2π/scale`。
    pub curl_scale: f32,
    /// curl 噪声的时间频率（Hz）：涡旋翻滚的速度。
    pub curl_hz: f32,
    /// Z 轴初速绝对值上限（单位/秒）。默认 `0`：不抽 Z 随机数，旧序列不变。
    pub speed_z: f32,
    /// 出生点 Z 抖动半径（单位）。默认 `0`：不抽随机数。
    pub spread_z: f32,
    /// 出生盒半边长 `[x,y,z]`（世界单位）。每个粒子在盒内均匀落点；默认 `[0,0,0]`
    /// 时**不抽随机数**，旧 burst/连续流的初始状态逐位不变。
    ///
    /// 与 `spread_z` 的区别：`spread_z` 抖的是**初速的 z 分量**，这里抖的是**出生位置**。
    /// 雨幕/雪幕/雾团需要的是后者；一个连续流就能覆盖相机周围的一片体积。
    pub spawn_area: [f32; 3],
    /// 每秒速度衰减比例（`0` = 无阻力）。
    pub drag: f32,
    /// 初始旋转（弧度）。
    pub rotation: f32,
    /// 每秒角速度（弧度）。
    pub rotation_speed: f32,
    /// 模拟落在哪一侧（P3-R07）。默认 [`ParticleMode::Cpu`]：**旧场景不带这个字段**，
    /// 而缺席就是 cpu，因此既有场景与粒子 golden 逐字节不变。
    pub mode: ParticleMode,
    /// 随机种子覆盖。`None` = 沿用粒子系统自己的 LCG 状态（旧行为）。
    ///
    /// 给定时，**这一次 burst** 的抽样序列从该 seed 起，于是同一份 emitter 参数在
    /// CPU 与 GPU 两侧能生成同一份初始状态（P3-R07 的对拍前提）。
    pub seed: Option<u64>,
}

/// 粒子模拟落在哪一侧（P3-R07）。
///
/// 这不是"性能开关"而是**语义边界**：`Gpu` 的粒子只存在于显存里，玩法与 Lua **读不到**
/// 它们的位置（读回要卡帧）。需要参与玩法（命中、遮挡、音效触发）的粒子必须用 `Cpu`。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ParticleMode {
    /// CPU 池模拟，玩法可读（默认）。
    #[default]
    Cpu,
    /// compute 模拟，只做视觉层。
    Gpu,
}

impl ParticleMode {
    /// 全部合法取值（错误信息与健康检查都从这里取，避免两处文案漂移）。
    pub const ALL: [Self; 2] = [Self::Cpu, Self::Gpu];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
        }
    }

    /// 允许取值列表，供错误信息用（`cpu|gpu`）。
    #[must_use]
    pub const fn choices() -> &'static str {
        "cpu|gpu"
    }

    /// 解析组件串/Lua 选项里的 mode。取值必须**逐字**命中 [`ParticleMode::ALL`]
    /// （`GPU` 不行）：schema 的 `FieldKind::Enum` 就是精确匹配，两边口径不一致会让
    /// "引擎能加载、审计报红"的漂移永远查不出来。拼错也不许静默退回默认值——
    /// `mode=gpuu` 悄悄走 CPU 是本包最难查的一类 bug。
    ///
    /// # Errors
    ///
    /// 取值不在 [`ParticleMode::ALL`] 里。
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == raw)
            .ok_or_else(|| format!("particle mode must be one of {}", Self::choices()))
    }
}

/// CPU 侧模拟的原因（P3-R07 的可诊断落点）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticleCpuReason {
    /// 作者要的就是 cpu。
    Requested,
    /// 要了 gpu，但这个设备/构建没有 compute 能力。
    NoCompute,
    /// 要了 gpu，但渲染器没有连续流（`rate > 0` 需要环形槽），这一批回 CPU。
    ContinuousRate,
    /// 要了 gpu、compute 也可用，但这一批没能走到派发通道（宿主没来取 / 队列保底）。
    NotWired,
    /// 派发通道在，但渲染器拒了这批 burst（池满、槽位预算、参数校验）。
    DispatchRejected,
}

impl ParticleCpuReason {
    /// 宿主"每种原因只喊一层日志"要遍历的取值（不含 [`Self::Requested`]：作者选 cpu 不是回退）。
    pub const WARNABLE: [Self; 4] = [
        Self::NoCompute,
        Self::ContinuousRate,
        Self::NotWired,
        Self::DispatchRejected,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::NoCompute => "no_compute",
            Self::ContinuousRate => "continuous_rate",
            Self::NotWired => "not_wired",
            Self::DispatchRejected => "dispatch_rejected",
        }
    }

    /// "作者要了 gpu、这一帧却跑在 cpu" —— 只有这种需要在日志/Profiler 上标出来。
    #[must_use]
    pub const fn is_fallback(self) -> bool {
        !matches!(self, Self::Requested)
    }

    /// 稳定编号（宿主用它做"同一原因只喊一层"的位图；新增取值请保持在末尾）。
    #[must_use]
    pub const fn index(self) -> u32 {
        match self {
            Self::Requested => 0,
            Self::NoCompute => 1,
            Self::ContinuousRate => 2,
            Self::NotWired => 3,
            Self::DispatchRejected => 4,
        }
    }
}

/// [`ParticleEmitter::placement`] 的判定结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticlePlacement {
    /// 参数与能力都允许 GPU 模拟。
    Gpu,
    /// 用**同一份**参数在 CPU 模拟（不丢粒子、不黑屏）。
    Cpu(ParticleCpuReason),
}

impl ParticlePlacement {
    #[must_use]
    pub const fn cpu_reason(self) -> Option<ParticleCpuReason> {
        match self {
            Self::Gpu => None,
            Self::Cpu(reason) => Some(reason),
        }
    }
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
            gravity_z: 0.0,
            buoyancy: 0.0,
            flutter: 0.0,
            flutter_hz: 0.0,
            curve: 0.0,
            curl: 0.0,
            curl_scale: 0.3,
            curl_hz: 0.5,
            speed_z: 0.0,
            spread_z: 0.0,
            spawn_area: [0.0; 3],
            drag: 0.0,
            rotation: 0.0,
            rotation_speed: 0.0,
            mode: ParticleMode::Cpu,
            seed: None,
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
            &self.gravity_z,
            &self.buoyancy,
            &self.flutter,
            &self.flutter_hz,
            &self.curve,
            &self.curl,
            &self.curl_scale,
            &self.curl_hz,
            &self.speed_z,
            &self.spread_z,
            &self.spawn_area[0],
            &self.spawn_area[1],
            &self.spawn_area[2],
            &self.drag,
        ]) {
            if !value.is_finite() {
                return Err("particle color/gravity/drag/area must be finite".to_owned());
            }
        }
        if self.speed_z < 0.0 || self.spread_z < 0.0 {
            return Err("particle speed_z/spread_z must be >= 0".to_owned());
        }
        if self.buoyancy < 0.0 {
            return Err("particle buoyancy must be >= 0".to_owned());
        }
        if self.flutter < 0.0 {
            return Err("particle flutter must be >= 0".to_owned());
        }
        if !(0.0..=20.0).contains(&self.flutter_hz) {
            return Err("particle flutter_hz must be 0..=20".to_owned());
        }
        if !(-12.0..=12.0).contains(&self.curve) {
            return Err("particle curve must be -12..=12".to_owned());
        }
        if !(0.0..=50.0).contains(&self.curl) {
            return Err("particle curl must be 0..=50".to_owned());
        }
        if !(0.0..=4.0).contains(&self.curl_scale) {
            return Err("particle curl_scale must be 0..=4".to_owned());
        }
        if !(0.0..=5.0).contains(&self.curl_hz) {
            return Err("particle curl_hz must be 0..=5".to_owned());
        }
        if self.spawn_area.iter().any(|value| *value < 0.0) {
            return Err("particle spawn_area must be >= 0".to_owned());
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
    /// `gravity`、`gravity_z`、`buoyancy`、`speed_z`、`spread_z`、`drag`、`rotation`、`rotation_speed`、`mode`、`seed`。
    /// `size`/`color` 同时设置曲线的**起止**
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
                "gravity_z" => emitter.gravity_z = parse_number(raw, field)?,
                "buoyancy" => emitter.buoyancy = parse_number(raw, field)?,
                "speed_z" => emitter.speed_z = parse_number(raw, field)?,
                "spread_z" => emitter.spread_z = parse_number(raw, field)?,
                "area_x" => emitter.spawn_area[0] = parse_number(raw, field)?,
                "area_y" => emitter.spawn_area[1] = parse_number(raw, field)?,
                "area_z" => emitter.spawn_area[2] = parse_number(raw, field)?,
                "drag" => emitter.drag = parse_number(raw, field)?,
                "rotation" => emitter.rotation = parse_number(raw, field)?,
                "rotation_speed" => emitter.rotation_speed = parse_number(raw, field)?,
                "mode" => {
                    emitter.mode = ParticleMode::parse(raw).map_err(|reason| {
                        ParticleComponentError::InvalidChoice {
                            field: field.to_owned(),
                            reason,
                        }
                    })?
                }
                "seed" => {
                    emitter.seed = Some(
                        raw.parse::<u64>()
                            .map_err(|_| ParticleComponentError::InvalidField(field.to_owned()))?,
                    )
                }
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
        if self.gravity_z != 0.0 {
            out.push_str(&format!(";gravity_z={:.6}", self.gravity_z));
        }
        if self.speed_z != 0.0 {
            out.push_str(&format!(";speed_z={:.6}", self.speed_z));
        }
        if self.spread_z != 0.0 {
            out.push_str(&format!(";spread_z={:.6}", self.spread_z));
        }
        for (name, value) in [
            ("area_x", self.spawn_area[0]),
            ("area_y", self.spawn_area[1]),
            ("area_z", self.spawn_area[2]),
        ] {
            if value != 0.0 {
                out.push_str(&format!(";{name}={value:.6}"));
            }
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
        // `mode=cpu` 与 `seed` 缺席同义，所以两者都只在**偏离默认**时才写——旧场景的规范串
        // 一个字符都不变（粒子 golden 零 diff 的前提）。
        if self.mode != ParticleMode::default() {
            out.push_str(&format!(";mode={}", self.mode.as_str()));
        }
        if let Some(seed) = self.seed {
            out.push_str(&format!(";seed={seed}"));
        }
        out
    }

    /// 这份发射参数**实际**落在哪一侧模拟（P3-R07 的落点判定）。
    ///
    /// 判定条件与渲染器 burst 入口的拒绝条件一致：要 `mode=gpu`、设备有 compute、
    /// 且是 burst 语义（`rate == 0`）。任何一条不满足都用**同一份参数**回 CPU——回退不丢
    /// 粒子，也不黑屏。要判断连续流请用 [`Self::placement_with_streams`]。
    ///
    /// `compute_available` 由宿主从渲染器能力（或 wasm/release 的"根本没有 compute 接缝"）
    /// 传来；ECS 自己不知道设备的事，这是这层不依赖渲染器的边界。
    #[must_use]
    pub fn placement(&self, compute_available: bool) -> ParticlePlacement {
        self.placement_with_streams(compute_available, false)
    }

    /// 与 [`Self::placement`] 同一套规则，额外告诉判定"渲染器有没有连续粒子流"。
    ///
    /// `rate > 0` 只有在渲染器明确支持连续流时才落 GPU；否则仍回
    /// [`ParticleCpuReason::ContinuousRate`]，所以旧调用方（默认 `false`）的行为逐位不变。
    #[must_use]
    pub fn placement_with_streams(
        &self,
        compute_available: bool,
        streams_available: bool,
    ) -> ParticlePlacement {
        if self.mode != ParticleMode::Gpu {
            return ParticlePlacement::Cpu(ParticleCpuReason::Requested);
        }
        if !compute_available {
            return ParticlePlacement::Cpu(ParticleCpuReason::NoCompute);
        }
        if self.rate > 0.0 && !streams_available {
            return ParticlePlacement::Cpu(ParticleCpuReason::ContinuousRate);
        }
        ParticlePlacement::Gpu
    }
}

/// `ParticleEmitter` 组件串的解析错误。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ParticleComponentError {
    /// 空串不是一个发射器。
    #[error("particle emitter component is empty")]
    Empty,
    /// 字段不是 `key=value` 或数值非法。
    #[error("malformed particle field: {0}")]
    InvalidField(String),
    /// 未知键：**报错**而不是忽略，避免"写了 `rate` 却拼成 `reate`"被静默吞掉。
    #[error("unknown particle field: {0}")]
    UnknownField(String),
    /// 枚举键的取值不在允许列表里（`mode=gpuu` 拼错不许悄悄退回 `cpu`）。
    #[error("invalid value for particle field {field}: {reason}")]
    InvalidChoice { field: String, reason: String },
    /// 数值通过文法但语义非法（非有限、`life <= 0`、`drag < 0` 等）。
    #[error("invalid particle emitter: {0}")]
    Invalid(String),
}

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
    /// 世界 Z 坐标。2D burst 恒为 `0`，因此旧渲染路径逐位不变。
    pub position_z: f32,
    pub velocity: Vec2,
    /// 世界 Z 速度。2D burst 恒为 `0`。
    pub velocity_z: f32,
    /// Seconds remaining before the particle is recycled.
    pub life: f32,
    /// Total life when spawned, used to compute the fade factor.
    pub max_life: f32,
    pub size_start: f32,
    pub size_end: f32,
    pub color_start: [f32; 4],
    pub color_end: [f32; 4],
    pub gravity: Vec2,
    pub gravity_z: f32,
    /// 浮力（单位/秒²），随剩余寿命比例衰减。
    pub buoyancy: f32,
    /// 逐粒子颤动幅度/频率（单位/秒、Hz）；`0` = 不颤动。
    pub flutter: f32,
    pub flutter_hz: f32,
    /// 沿寿命的路径弯曲（弧度/秒）；`0` = 直线。
    pub curve: f32,
    /// 程序化 curl 噪声风场：强度（单位/秒）、空间频率、时间频率；`curl=0` = 关。
    pub curl: f32,
    pub curl_scale: f32,
    pub curl_hz: f32,
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
    /// 全局风加速度（世界单位/秒²）；默认零，旧行为逐位不变。
    wind: Vec2,
    /// 全局风的 Z 分量。只有 3D 发射器会读它；默认 `0`。
    wind_z: f32,
    /// 风场：随高度线性增强的剪切系数（每世界单位 z），默认 0。
    wind_shear: f32,
    /// 风场：空间阵风强度（0..=4），默认 0。
    wind_gust: f32,
    /// 风场：阵风的空间频率（每世界单位），默认 0。
    wind_gust_hz: f32,
    /// 最多 4 个局部风源（火柱上升、酸雾外扩等）。
    wind_sources: [WindSource; MAX_WIND_SOURCES],
    /// 风场时钟（秒）；只用于全局湍流相位，CPU/GPU 对齐。
    time: f32,
    /// 网格体积风场 + 温度浮力场（P3-R233）；`None` 时逐粒子积分逐位不变。
    wind_grid: Option<crate::particle_wind_grid::WindGrid>,
}

/// 一个局部风源，坐标与 `particles.burst3d` 一致：`x`/`y` 是水平面（`y`=高度），
/// `z` 是深度；`lift` 沿世界高度方向向上推，`radial` 沿从源指向粒子的三维方向外推。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindSource {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// 影响半径（世界单位）；`0` 表示该槽未启用。
    pub radius: f32,
    /// 沿“源 → 粒子”三维方向的外推力（负值向内），世界单位/秒²。
    pub radial: f32,
    /// 沿世界高度方向（`y`）的上升力，世界单位/秒²。
    pub lift: f32,
    /// 绕世界高度轴的切向旋流强度（正=逆时针），世界单位/秒²。
    pub swirl: f32,
}

/// 局部风源槽位数；Lua/CPU/GPU 共用这一上限。
pub const MAX_WIND_SOURCES: usize = 4;

impl ParticleSystem {
    /// Creates an empty particle system with a fixed seed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            particles: Vec::new(),
            seed: 0x1234_5678_9abc_def0,
            wind: Vec2::zero(),
            wind_z: 0.0,
            wind_shear: 0.0,
            wind_gust: 0.0,
            wind_gust_hz: 0.0,
            wind_sources: [WindSource::default(); MAX_WIND_SOURCES],
            time: 0.0,
            wind_grid: None,
        }
    }

    /// 设置全局风加速度；只作用于 CPU 粒子池（GPU burst 的 GPU 参数暂未带风）。
    /// 非有限值拒绝，避免 NaN 污染整池。
    pub fn set_wind(&mut self, wind: Vec2) -> Result<(), String> {
        self.set_wind_3d(wind.x, wind.y, 0.0)
    }

    /// 设置三维全局风。Z 分量默认 `0`，2D 发射器的积分结果不变。
    pub fn set_wind_3d(&mut self, x: f32, y: f32, z: f32) -> Result<(), String> {
        if !x.is_finite() || !y.is_finite() || !z.is_finite() {
            return Err("particle wind must be finite".to_owned());
        }
        self.wind = Vec2::new(x, y);
        self.wind_z = z;
        Ok(())
    }

    #[must_use]
    pub fn wind(&self) -> Vec2 {
        self.wind
    }

    /// 当前风的 Z 分量。
    #[must_use]
    pub fn wind_z(&self) -> f32 {
        self.wind_z
    }

    /// 设置风场：高度剪切 + 空间阵风。
    ///
    /// - `shear`：每世界单位 z 的风速系数（`-1..=1`；正数表示越高风越大）；
    /// - `gust`：阵风强度（`0..=4`）；
    /// - `gust_hz`：阵风空间频率（每世界单位，`0..=20`）。
    ///
    /// CPU 池与 GPU 参数用**同一个公式**（见 [`Self::wind_factor_at`]），因此同一阵
    /// 风在 CPU/GPU 粒子上的表现一致。默认全 0，旧行为逐位不变。
    pub fn set_wind_profile(&mut self, shear: f32, gust: f32, gust_hz: f32) -> Result<(), String> {
        if !shear.is_finite() || !gust.is_finite() || !gust_hz.is_finite() {
            return Err("particle wind profile must be finite".to_owned());
        }
        if !(-1.0..=1.0).contains(&shear) {
            return Err("particle wind shear must be -1..=1".to_owned());
        }
        if !(0.0..=4.0).contains(&gust) {
            return Err("particle wind gust must be 0..=4".to_owned());
        }
        if !(0.0..=20.0).contains(&gust_hz) {
            return Err("particle wind gust_hz must be 0..=20".to_owned());
        }
        self.wind_shear = shear;
        self.wind_gust = gust;
        self.wind_gust_hz = gust_hz;
        Ok(())
    }

    /// 当前风场 `(shear, gust, gust_hz)`。
    #[must_use]
    pub fn wind_profile(&self) -> (f32, f32, f32) {
        (self.wind_shear, self.wind_gust, self.wind_gust_hz)
    }

    /// 设置一个局部风源；`radius = 0` 关闭该槽。越界/非有限值拒绝且保留旧值。
    pub fn set_wind_source(&mut self, index: usize, source: WindSource) -> Result<(), String> {
        if index >= MAX_WIND_SOURCES {
            return Err(format!("wind source index must be 0..{MAX_WIND_SOURCES}"));
        }
        if !source.x.is_finite()
            || !source.y.is_finite()
            || !source.z.is_finite()
            || !source.radius.is_finite()
            || !source.radial.is_finite()
            || !source.lift.is_finite()
        {
            return Err("wind source must be finite".to_owned());
        }
        if !(0.0..=512.0).contains(&source.radius) {
            return Err("wind source radius must be 0..=512".to_owned());
        }
        if !(-64.0..=64.0).contains(&source.radial) || !(-64.0..=64.0).contains(&source.lift) {
            return Err("wind source radial/lift must be -64..=64".to_owned());
        }
        if !(-64.0..=64.0).contains(&source.swirl) {
            return Err("wind source swirl must be -64..=64".to_owned());
        }
        self.wind_sources[index] = source;
        Ok(())
    }

    /// 清空全部局部风源。
    pub fn clear_wind_sources(&mut self) {
        self.wind_sources = [WindSource::default(); MAX_WIND_SOURCES];
    }

    /// 当前 4 个局部风源（`radius = 0` 的槽未启用）。
    #[must_use]
    pub fn wind_sources(&self) -> &[WindSource; MAX_WIND_SOURCES] {
        &self.wind_sources
    }

    /// 安装/替换网格体积风场（P3-R233）。此后每步先推进场、再让粒子按所在格采样。
    pub fn set_wind_grid(&mut self, grid: crate::particle_wind_grid::WindGrid) {
        self.wind_grid = Some(grid);
    }

    /// 移除网格风场，回到无场行为（逐粒子积分逐位不变）。
    pub fn clear_wind_grid(&mut self) {
        self.wind_grid = None;
    }

    /// 可变借用当前风场，用来注入速度/温度或改系数；无场时 `None`。
    pub fn wind_grid_mut(&mut self) -> Option<&mut crate::particle_wind_grid::WindGrid> {
        self.wind_grid.as_mut()
    }

    /// 只读借用当前风场。
    #[must_use]
    pub fn wind_grid(&self) -> Option<&crate::particle_wind_grid::WindGrid> {
        self.wind_grid.as_ref()
    }

    /// 风场在 `(x, height)` 处的倍率；CPU 与 GPU 共用这一条公式。
    ///
    /// 高度剪切 + 一条空间正弦阵风；结果下限 0，避免反向风把粒子吹得比无风还慢。
    #[must_use]
    pub fn wind_factor_at(&self, x: f32, height: f32) -> f32 {
        let phase = height * self.wind_gust_hz + x * 0.11;
        (1.0 + self.wind_shear * height + self.wind_gust * phase.sin()).max(0.0)
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
    ///
    /// `emitter.seed` 给定时先把 LCG 复位到该 seed：同一份参数的两次 burst 逐位相同，
    /// CPU 与 GPU 两侧也才能对拍。缺席时沿用系统的历史状态（旧行为，一个 bit 都不变）。
    pub fn burst_with(&mut self, position: Vec2, emitter: &ParticleEmitter) {
        self.burst_at(position, 0.0, emitter);
    }

    /// 在三维世界坐标 `[x, y, z]` 生成一次爆发。
    ///
    /// 与 2D `burst_with` 共用同一抽样序列；只有 `speed_z`/`spread_z` 非零时才额外抽
    /// Z 随机数，所以既有 2D 调用一个 bit 都不变。
    pub fn burst_with_3d(&mut self, position: [f32; 3], emitter: &ParticleEmitter) {
        self.burst_at(Vec2::new(position[0], position[1]), position[2], emitter);
    }

    fn burst_at(&mut self, position: Vec2, position_z: f32, emitter: &ParticleEmitter) {
        if emitter.count == 0 || emitter.validate().is_err() {
            return;
        }
        if let Some(seed) = emitter.seed {
            self.seed = seed;
        }
        self.particles.reserve(emitter.count);
        for _ in 0..emitter.count {
            // 四次抽取的顺序与旧实现一致：角度 → 速度 → 寿命 → 尺寸抖动。
            // 这样默认参数下的角度/速度/寿命与旧版本逐位相同。
            let angle = self.next_rand() * TAU;
            let speed_unit = self.next_rand();
            let life_unit = self.next_rand();
            let size_unit = self.next_rand();
            // 只有 3D 发射器才多抽两次；默认 0 时分支不进，旧序列不变。
            let (z_offset, velocity_z) = if emitter.speed_z > 0.0 || emitter.spread_z > 0.0 {
                let speed_z_unit = if emitter.speed_z > 0.0 {
                    self.next_rand()
                } else {
                    0.5
                };
                let spread_z_unit = if emitter.spread_z > 0.0 {
                    self.next_rand()
                } else {
                    0.5
                };
                (
                    (spread_z_unit * 2.0 - 1.0) * emitter.spread_z,
                    (speed_z_unit * 2.0 - 1.0) * emitter.speed_z,
                )
            } else {
                (0.0, 0.0)
            };
            // 出生盒：只有任一边长 > 0 时才抽三次，默认参数下随机序列与旧版逐位一致。
            let spawn = if emitter.spawn_area.iter().any(|value| *value > 0.0) {
                [
                    (self.next_rand() * 2.0 - 1.0) * emitter.spawn_area[0],
                    (self.next_rand() * 2.0 - 1.0) * emitter.spawn_area[1],
                    (self.next_rand() * 2.0 - 1.0) * emitter.spawn_area[2],
                ]
            } else {
                [0.0; 3]
            };
            let magnitude = emitter.speed * (0.25 + speed_unit * 0.75);
            let max_life = emitter.life * (0.5 + life_unit * 0.5);
            let scale = 0.75 + 0.5 * size_unit;
            self.particles.push(Particle {
                position: Vec2::new(position.x + spawn[0], position.y + spawn[1]),
                position_z: position_z + z_offset + spawn[2],
                velocity: Vec2::new(angle.cos() * magnitude, angle.sin() * magnitude),
                velocity_z,
                life: max_life,
                max_life,
                size_start: emitter.size_start * scale,
                size_end: emitter.size_end * scale,
                color_start: emitter.color_start,
                color_end: emitter.color_end,
                gravity: emitter.gravity,
                gravity_z: emitter.gravity_z,
                buoyancy: emitter.buoyancy,
                flutter: emitter.flutter,
                flutter_hz: emitter.flutter_hz,
                curve: emitter.curve,
                curl: emitter.curl,
                curl_scale: emitter.curl_scale,
                curl_hz: emitter.curl_hz,
                drag: emitter.drag,
                rotation: emitter.rotation,
                rotation_speed: emitter.rotation_speed,
            });
        }
    }

    /// Advances particles by `dt` seconds and recycles the finished ones.
    pub fn step(&mut self, delta_seconds: f32) {
        self.step_with_parallelism(delta_seconds, rustlua_infra::par::Parallelism::SEQUENTIAL)
    }

    /// P1-③：并行推进粒子（系统级数据并行）。逐粒子积分彼此独立（无浮点归约），
    /// 按块并行；回收压缩与串行完全一致（只依赖 `life > 0`，与更新顺序无关）——
    /// 因此**串/并行结果逐位相同**（docs/40 WP8：系统内纯数据子任务并行；调度器
    /// 保持确定性单线程）。`Parallelism::SEQUENTIAL` 完全回滚到单线程。
    pub fn step_with_parallelism(
        &mut self,
        delta_seconds: f32,
        parallelism: rustlua_infra::par::Parallelism,
    ) {
        if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return;
        }
        self.time += delta_seconds;
        let time = self.time;
        let wind = self.wind;
        let wind_z = self.wind_z;
        let (shear, gust, gust_hz) = (self.wind_shear, self.wind_gust, self.wind_gust_hz);
        let sources = self.wind_sources;
        // 网格风场每池步只推进一次（在所有粒子采样之前），随后只读共享给各并行块——
        // 采样不改场，故串/并行仍逐位一致。无场时 `None`，逐粒子积分逐位不变。
        if let Some(grid) = self.wind_grid.as_mut() {
            grid.step(delta_seconds);
        }
        let grid = self.wind_grid.as_ref();
        let particles = &mut self.particles;
        let threads = parallelism.effective_threads(particles.len());
        if threads < 2 {
            for particle in particles.iter_mut() {
                step_particle(
                    particle,
                    wind,
                    wind_z,
                    shear,
                    gust,
                    gust_hz,
                    &sources,
                    time,
                    delta_seconds,
                    grid,
                );
            }
            particles.retain(|particle| particle.life > 0.0);
            return;
        }
        let chunk = particles.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for batch in particles.chunks_mut(chunk) {
                scope.spawn(move || {
                    for particle in batch.iter_mut() {
                        step_particle(
                            particle,
                            wind,
                            wind_z,
                            shear,
                            gust,
                            gust_hz,
                            &sources,
                            time,
                            delta_seconds,
                            grid,
                        );
                    }
                });
            }
        });
        particles.retain(|particle| particle.life > 0.0);
    }
}

/// 单粒子一步积分：`step` 与 `step_with_parallelism` 共用同一份，保证串/并行逐位一致。
#[allow(clippy::too_many_arguments)]
fn step_particle(
    particle: &mut Particle,
    wind: Vec2,
    wind_z: f32,
    shear: f32,
    gust: f32,
    gust_hz: f32,
    sources: &[WindSource; MAX_WIND_SOURCES],
    time: f32,
    delta_seconds: f32,
    grid: Option<&crate::particle_wind_grid::WindGrid>,
) {
    // 与 GPU compute 同一公式：高度剪切 + 空间正弦阵风。粒子坐标系是
    // (x, height) 平面 + position_z 深度，所以高度取 `position.y`。
    let height = particle.position.y;
    let phase = height * gust_hz + particle.position.x * 0.11;
    let factor = (1.0 + shear * height + gust * phase.sin()).max(0.0);
    let mut wind = wind * factor;
    let mut wind_z = wind_z * factor;
    // 全局三维湍流：用同一个 gust 强度，在三个轴上错相位；time 让流场随时间翻滚。
    let amp = gust * 0.5;
    if amp > 0.0 {
        wind.x += amp * (phase * 1.7 + particle.position_z * 0.13 + time * 0.7).sin();
        wind_z += amp * (phase * 1.3 - particle.position_z * 0.09 + time * 0.5).cos();
        wind.y += amp * 0.5 * (phase * 2.1 + particle.position.x * 0.2 + time * 0.9).sin();
    }
    // 局部风源：径向 + 上升。四个槽逐个看，未启用的 radius=0 直接跳过。
    for source in sources {
        if source.radius <= 0.0 {
            continue;
        }
        let dx = particle.position.x - source.x;
        let dy = height - source.y;
        let dz = particle.position_z - source.z;
        let distance_sq = dx * dx + dy * dy + dz * dz;
        if distance_sq >= source.radius * source.radius {
            continue;
        }
        let distance = distance_sq.sqrt();
        let falloff = (1.0 - distance / source.radius).powi(2);
        if source.radial != 0.0 && distance > 1e-4 {
            let radial = source.radial * falloff / distance;
            wind = wind + Vec2::new(dx, dy) * radial;
            wind_z += dz * radial;
        }
        // 旋流：水平面 (x, z) 上绕源的切线，正值为逆时针。
        if source.swirl != 0.0 && distance > 1e-4 {
            let swirl = source.swirl * falloff / distance;
            wind.x += -dz * swirl;
            wind_z += dx * swirl;
        }
        wind.y += source.lift * falloff;
    }
    // 程序化 curl 噪声：势函数 `Ψ=(sin a·cos b, sin b·cos c, sin c·cos a)` 的旋度，
    // 严格无散（∇·w = 0）——局部只有旋转/剪切，没有源汇。a/b/c 由世界坐标 × 空间频率
    // 叠时间推进：相邻粒子共享方向（空间相干），整片场随时间翻滚。除以 k 归一化后
    // 每个分量幅度 ≤ `curl`，所以强度就是"涡旋风速上限"。
    if particle.curl > 0.0 {
        let k = particle.curl_scale.max(0.001);
        let w = time * TAU * particle.curl_hz;
        let a = particle.position.x * k + w;
        let b = height * k + w * 0.7;
        let c = particle.position_z * k + w * 1.3;
        wind.x += particle.curl * b.sin() * c.sin();
        wind.y += particle.curl * c.cos() * a.sin();
        wind_z += particle.curl * a.sin() * b.sin();
    }
    // 浮力按剩余寿命比例向上推：新粒子最热，随 life 衰减到 0。
    let buoyancy = particle.buoyancy * (particle.life / particle.max_life).max(0.0);
    // 逐粒子颤动：相位来自粒子自己的稳定随机量（寿命/尺寸抖动），CPU 与 GPU 同一公式。
    // 全局湍流强度取决于 gust（无风时为零），这条不依赖风，让同类粒子各自走出微弯路径。
    if particle.flutter > 0.0 {
        // 相位只做乘加与取小数：不经过 sin 的大倍数放大，GPU 与 CPU 的 sin 精度差
        // 不会被放大成整相位跳变（对拍门的 1e-4 要求下必须逐位同相）。
        let phase = (particle.max_life * 0.37 + particle.size_start * 0.61).fract();
        let wave = time * TAU * particle.flutter_hz + phase * TAU;
        wind.x += particle.flutter * wave.sin();
        wind_z += particle.flutter * (wave * 0.87 - phase * TAU * 1.7).cos();
        wind.y += particle.flutter * 0.45 * (wave * 1.13 + phase * TAU * 0.6).sin();
    }
    // 网格体积风场 + 温度浮力（P3-R233）：按粒子世界位置三线性采样成加速度，与 wind/curl
    // 同口径直接叠加。场已在本次 step 开头推进过；无场（None）时这一整块跳过，逐位不变。
    // 采样点 y 取世界高度（与 wind_sources 的 dy 一致），x/z 取平面与深度。
    if let Some(grid) = grid {
        let sample = grid.sample_accel(rustlua_math::math3d::Vec3::new(
            particle.position.x,
            height,
            particle.position_z,
        ));
        wind.x += sample.x;
        wind.y += sample.y;
        wind_z += sample.z;
    }
    particle.velocity = particle.velocity + (particle.gravity + wind) * delta_seconds;
    particle.velocity_z += (particle.gravity_z + wind_z + buoyancy) * delta_seconds;
    if particle.drag > 0.0 {
        // 每秒衰减 `drag` 比例；单步时长超过 1/drag 时钳到 0（不反转速度）。
        let damp = (1.0 - particle.drag * delta_seconds).max(0.0);
        particle.velocity = particle.velocity * damp;
        particle.velocity_z *= damp;
    }
    if particle.curve != 0.0 {
        // 沿寿命弯曲：与 `buoyancy` 同一 ramp（新颗粒转得最快）。只转水平面 (x, y)
        // 的速度方向、不改速率，所以累计转角有限、不会绕成轨道；z 是竖直轴，不参与。
        let ramp = (particle.life / particle.max_life.max(1e-6)).clamp(0.0, 1.0);
        let angle = particle.curve * ramp * delta_seconds;
        let (sin, cos) = angle.sin_cos();
        let v = particle.velocity;
        particle.velocity = Vec2::new(v.x * cos - v.y * sin, v.x * sin + v.y * cos);
    }
    particle.position = particle.position + particle.velocity * delta_seconds;
    particle.position_z += particle.velocity_z * delta_seconds;
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

    /// 丢掉所有粒子但保留 LCG 状态与风。
    ///
    /// 连续粒子流只借这个系统做**初始抽样**：采样一批、读 `particles()`、再 `clear()`，
    /// 下一批继续推进同一条随机序列，不会像新建系统那样每帧从同一 seed 重来。
    pub fn clear(&mut self) {
        self.particles.clear();
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
        let parallel_degree = rustlua_infra::par::Parallelism::with_threads(4);
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

    /// P3-R233：无网格风场时逐粒子积分逐位不变（新字段/新参数不能动旧路径）。
    #[test]
    fn wind_grid_absent_is_bit_identical_to_baseline() {
        let emitter = ParticleEmitter::new(256, 8.0, 2.0, [1.0, 0.5, 0.25, 1.0]);
        let mut baseline = ParticleSystem::new();
        let mut cleared = ParticleSystem::new();
        baseline.burst_with_3d([1.0, 2.0, 3.0], &emitter);
        cleared.burst_with_3d([1.0, 2.0, 3.0], &emitter);
        // 装了又清空，回到无场——与从未装过逐位一致。
        cleared.set_wind_grid(
            crate::particle_wind_grid::WindGrid::new(
                [4, 4, 4],
                rustlua_math::math3d::Vec3::ZERO,
                1.0,
            )
            .unwrap(),
        );
        cleared.clear_wind_grid();
        assert!(cleared.wind_grid().is_none());
        for _ in 0..60 {
            baseline.step(0.016);
            cleared.step(0.016);
        }
        assert_eq!(baseline.particles(), cleared.particles());
    }

    /// P3-R233：均匀速度网格给粒子精确加上那份加速度（无重力/无其它风的受控场景）。
    #[test]
    fn wind_grid_accelerates_particles_by_sampled_field() {
        use crate::particle_wind_grid::WindGrid;
        use rustlua_math::math3d::Vec3;
        let mut system = ParticleSystem::new();
        system.burst_with_3d(
            [0.5, 0.5, 0.5],
            &ParticleEmitter::new(1, 0.0, 4.0, [1.0; 4]),
        );
        let mut grid = WindGrid::new([2, 2, 2], Vec3::ZERO, 1.0).unwrap();
        // 全格速度 (4, 0, 2)，无扩散/衰减/浮力 ⇒ 采样恒为 (4,0,2)。
        for x in 0..2 {
            for y in 0..2 {
                for z in 0..2 {
                    grid.add_velocity(x, y, z, Vec3::new(4.0, 0.0, 2.0))
                        .unwrap();
                }
            }
        }
        system.set_wind_grid(grid);
        system.step(0.5);
        let p = system.particles()[0];
        assert!(
            (p.velocity.x - 2.0).abs() < 1e-5,
            "vx = 4*0.5, got {}",
            p.velocity.x
        );
        assert!(
            (p.velocity_z - 1.0).abs() < 1e-5,
            "vz = 2*0.5, got {}",
            p.velocity_z
        );
        assert!(p.velocity.y.abs() < 1e-5);
    }

    /// P3-R233：装了网格风场后，串/并行仍逐位一致（场在 step 开头推进一次、之后只读共享）。
    #[test]
    fn wind_grid_parallel_step_matches_sequential_bit_for_bit() {
        use crate::particle_wind_grid::WindGrid;
        use rustlua_math::math3d::Vec3;
        let make_grid = || {
            let mut grid = WindGrid::new([6, 6, 6], Vec3::new(-3.0, -3.0, -3.0), 1.0).unwrap();
            grid.set_params(0.08, 0.5, 1.0, 4.0, 2).unwrap();
            grid.set_temperature(3, 1, 3, 20.0).unwrap();
            grid.add_velocity(2, 2, 2, Vec3::new(3.0, 0.0, -2.0))
                .unwrap();
            grid
        };
        let emitter = ParticleEmitter::new(4000, 10.0, 3.0, [1.0, 0.5, 0.25, 1.0]);
        let mut sequential = ParticleSystem::new();
        let mut parallel = ParticleSystem::new();
        sequential.burst_with_3d([0.0, 0.0, 0.0], &emitter);
        parallel.burst_with_3d([0.0, 0.0, 0.0], &emitter);
        sequential.set_wind_grid(make_grid());
        parallel.set_wind_grid(make_grid());
        let degree = rustlua_infra::par::Parallelism::with_threads(4);
        for _ in 0..100 {
            sequential.step(0.016);
            parallel.step_with_parallelism(0.016, degree);
            assert_eq!(sequential.particles(), parallel.particles());
        }
        assert_eq!(sequential.wind_grid(), parallel.wind_grid());
    }

    #[test]
    fn wind_accelerates_particles_and_rejects_non_finite() {
        let mut system = ParticleSystem::new();
        system.burst(Vec2::zero(), 1, 0.0, 2.0, [1.0, 1.0, 1.0, 1.0]);
        system.set_wind(Vec2::new(10.0, -4.0)).unwrap();
        system.step(0.5);
        assert!((system.particles()[0].velocity.x - 5.0).abs() < 1e-6);
        assert!((system.particles()[0].velocity.y + 2.0).abs() < 1e-6);
        assert!(system.set_wind(Vec2::new(f32::NAN, 0.0)).is_err());
        assert!(system.set_wind(Vec2::new(0.0, f32::INFINITY)).is_err());
    }

    /// 3D burst 的 z 起点/速度/重力都真实参与积分；2D 路径仍逐位不变。
    #[test]
    fn burst_with_3d_integrates_z_and_2d_path_is_bit_identical() {
        let emitter = ParticleEmitter::new(4, 0.0, 1.0, [1.0, 1.0, 1.0, 1.0]);
        let mut flat = ParticleSystem::new();
        let mut flat_3d_api = ParticleSystem::new();
        flat.burst_with(Vec2::new(3.0, 4.0), &emitter);
        flat_3d_api.burst_with_3d([3.0, 4.0, 0.0], &emitter);
        assert_eq!(flat.particles(), flat_3d_api.particles());

        let elevated = ParticleEmitter {
            count: 4,
            speed: 0.0,
            life: 2.0,
            speed_z: 6.0,
            spread_z: 2.0,
            gravity_z: -10.0,
            ..ParticleEmitter::default()
        };
        let mut system = ParticleSystem::new();
        system.burst_with_3d([1.0, 2.0, 5.0], &elevated);
        assert_eq!(system.len(), 4);
        assert!(
            system
                .particles()
                .iter()
                .all(|particle| (3.0..=7.0).contains(&particle.position_z)),
            "spread_z must jitter around the emitter z"
        );
        let before = system.particles()[0].position_z;
        system.set_wind_3d(0.0, 0.0, 20.0).unwrap();
        system.step(0.5);
        let after = system.particles()[0];
        assert!(after.position_z > before, "z wind/gravity must move z");
        assert!(after.velocity_z.is_finite());
        assert!(system.set_wind_3d(0.0, 0.0, f32::NAN).is_err());
    }

    #[test]
    fn wind_sources_push_radially_and_lift() {
        let emitter = ParticleEmitter {
            count: 1,
            speed: 0.0,
            life: 10.0,
            spread_z: 0.0,
            ..ParticleEmitter::default()
        };
        let mut lift_system = ParticleSystem::new();
        lift_system
            .set_wind_source(
                0,
                WindSource {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    radius: 10.0,
                    radial: 0.0,
                    lift: 5.0,
                    swirl: 0.0,
                },
            )
            .unwrap();
        lift_system.burst_with_3d([0.0, 1.0, 0.0], &emitter);
        lift_system.step(1.0);
        let expected_lift = 5.0 * (1.0 - 0.1f32).powi(2);
        assert!((lift_system.particles()[0].velocity.y - expected_lift).abs() < 1e-4);

        let mut radial_system = ParticleSystem::new();
        radial_system
            .set_wind_source(
                1,
                WindSource {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    radius: 10.0,
                    radial: 2.0,
                    lift: 0.0,
                    swirl: 0.0,
                },
            )
            .unwrap();
        radial_system.burst_with_3d([5.0, 0.0, 0.0], &emitter);
        radial_system.step(1.0);
        assert!((radial_system.particles()[0].velocity.x - 0.5).abs() < 1e-4);

        let mut swirl_system = ParticleSystem::new();
        swirl_system
            .set_wind_source(
                2,
                WindSource {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    radius: 10.0,
                    radial: 0.0,
                    lift: 0.0,
                    swirl: 2.0,
                },
            )
            .unwrap();
        swirl_system.burst_with_3d([5.0, 0.0, 0.0], &emitter);
        swirl_system.step(1.0);
        assert!(
            (swirl_system.particles()[0].velocity_z - 0.5).abs() < 1e-4,
            "swirl must push tangentially on world z: {:?}",
            swirl_system.particles()[0].velocity_z
        );

        radial_system.clear_wind_sources();
        assert_eq!(radial_system.wind_sources()[1].radius, 0.0);
        assert!(
            radial_system
                .set_wind_source(
                    4,
                    WindSource {
                        radius: 1.0,
                        ..WindSource::default()
                    }
                )
                .is_err()
        );
        assert!(
            radial_system
                .set_wind_source(
                    0,
                    WindSource {
                        radius: 600.0,
                        ..WindSource::default()
                    }
                )
                .is_err()
        );
        assert!(
            radial_system
                .set_wind_source(
                    0,
                    WindSource {
                        radius: 1.0,
                        lift: f32::NAN,
                        ..WindSource::default()
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn buoyancy_lifts_particles_and_is_validated() {
        let emitter = ParticleEmitter {
            count: 1,
            speed: 0.0,
            life: 1.0,
            buoyancy: 4.0,
            seed: Some(1),
            ..ParticleEmitter::default()
        };
        let mut system = ParticleSystem::new();
        system.burst_with_3d([0.0, 0.0, 0.0], &emitter);
        system.step(0.25);
        assert!(
            system.particles()[0].velocity_z > 0.0,
            "buoyancy must push upward: {}",
            system.particles()[0].velocity_z
        );
        let bad = ParticleEmitter {
            buoyancy: -1.0,
            ..ParticleEmitter::default()
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn wind_profile_shears_with_height_and_guards_inputs() {
        let mut system = ParticleSystem::new();
        assert_eq!(system.wind_profile(), (0.0, 0.0, 0.0));
        assert!((system.wind_factor_at(0.0, 0.0) - 1.0).abs() < 1e-6);

        system.set_wind_profile(0.1, 0.0, 0.0).unwrap();
        assert!((system.wind_factor_at(0.0, 10.0) - 2.0).abs() < 1e-5);

        system.set_wind_profile(0.0, 0.5, 1.0).unwrap();
        let phase = std::f32::consts::FRAC_PI_2;
        let expected = (1.0 + 0.5 * phase.sin()).max(0.0);
        assert!((system.wind_factor_at(0.0, phase) - expected).abs() < 1e-5);

        for (shear, gust, hz) in [
            (1.5, 0.0, 0.0),
            (0.0, 5.0, 0.0),
            (0.0, 0.0, 30.0),
            (f32::NAN, 0.0, 0.0),
        ] {
            assert!(system.set_wind_profile(shear, gust, hz).is_err());
        }

        // Same wind, same profile: the particle at z=20 gets twice the acceleration.
        let emitter = ParticleEmitter {
            count: 1,
            speed: 0.0,
            life: 10.0,
            spread_z: 0.0,
            ..ParticleEmitter::default()
        };
        let mut low = ParticleSystem::new();
        let mut high = ParticleSystem::new();
        low.burst_with_3d([0.0, 0.0, 0.0], &emitter);
        high.burst_with_3d([0.0, 20.0, 0.0], &emitter);
        for system in [&mut low, &mut high] {
            system.set_wind_3d(10.0, 0.0, 0.0).unwrap();
            system.set_wind_profile(0.1, 0.0, 0.0).unwrap();
            system.step(0.5);
        }
        assert!(
            high.particles()[0].velocity.x > low.particles()[0].velocity.x + 1.0,
            "height shear must accelerate the higher particle"
        );
    }

    #[test]
    fn spawn_area_fills_a_box_without_changing_default_positions() {
        let emitter = ParticleEmitter {
            count: 64,
            speed: 0.0,
            life: 1.0,
            spawn_area: [2.0, 3.0, 4.0],
            seed: Some(11),
            ..ParticleEmitter::default()
        };
        let mut system = ParticleSystem::new();
        system.burst_with_3d([10.0, 0.0, 5.0], &emitter);
        assert_eq!(system.len(), 64);
        for particle in system.particles() {
            assert!(
                (8.0..=12.0).contains(&particle.position.x)
                    && (-3.0..=3.0).contains(&particle.position.y)
                    && (1.0..=9.0).contains(&particle.position_z),
                "particle outside spawn box: {particle:?}"
            );
        }
        let mut default = ParticleSystem::new();
        default.burst_with_3d(
            [10.0, 0.0, 5.0],
            &ParticleEmitter::new(4, 0.0, 1.0, [1.0; 4]),
        );
        assert!(
            default
                .particles()
                .iter()
                .all(|particle| particle.position == Vec2::new(10.0, 0.0)
                    && particle.position_z == 5.0),
            "default spawn_area must keep every particle at the origin"
        );
        let mut invalid = ParticleEmitter {
            spawn_area: [0.0, -1.0, 0.0],
            ..ParticleEmitter::default()
        };
        assert!(invalid.validate().is_err());
        invalid.spawn_area = [f32::NAN, 0.0, 0.0];
        assert!(invalid.validate().is_err());
        let text = ParticleEmitter {
            spawn_area: [1.0, 2.0, 3.0],
            ..ParticleEmitter::default()
        }
        .to_component_str();
        assert!(text.contains("area_x=1.000000"));
        assert!(text.contains("area_z=3.000000"));
        let parsed = ParticleEmitter::from_component_str(&text).unwrap();
        assert_eq!(parsed.spawn_area, [1.0, 2.0, 3.0]);
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

    /// P3-R07-1：`mode`/`seed` 是**可选**字段——旧场景串的规范输出必须一字符不变，
    /// 拼错的 `mode` 不许静默退回 cpu。
    #[test]
    fn mode_and_seed_round_trip_without_touching_legacy_strings() {
        // 默认（cpu、无 seed）的规范串里不出现新字段：粒子 golden 的字节前提。
        let legacy = ParticleEmitter::default().to_component_str();
        assert!(
            !legacy.contains("mode") && !legacy.contains("seed"),
            "默认串不该变长: {legacy}"
        );
        // 既有场景串的**规范形**逐字节不变（编码器一直会把数值写成 6 位小数，
        // 这里钉的是"加了 mode/seed 之后规范形没变长/没变味"）。
        let text = "count=4;rate=30;speed=90;life=0.5;size=3;color=1,0.7,0.3,1";
        assert_eq!(
            ParticleEmitter::from_component_str(text)
                .expect("legacy parse")
                .to_component_str(),
            "count=4;rate=30.000000;speed=90.000000;life=0.500000;size=3.000000;color=1.000,0.700,0.300,1.000"
        );

        let emitter =
            ParticleEmitter::from_component_str("count=4;mode=gpu;seed=42").expect("parse mode");
        assert_eq!(emitter.mode, ParticleMode::Gpu);
        assert_eq!(emitter.seed, Some(42));
        assert_eq!(
            emitter.to_component_str(),
            "count=4;speed=120.000000;life=0.600000;size=6.000000;color=1.000,0.600,0.200,1.000;mode=gpu;seed=42"
        );
        // 显式写 cpu 与缺席同义，因此写回时省略。
        assert!(
            !ParticleEmitter::from_component_str("count=4;mode=cpu")
                .expect("explicit cpu")
                .to_component_str()
                .contains("mode")
        );

        // 取值必须逐字命中（与 schema 的 `FieldKind::Enum` 同一口径），拼错要报出允许列表。
        assert!(matches!(
            ParticleEmitter::from_component_str("count=4;mode=GPU"),
            Err(ParticleComponentError::InvalidChoice { .. })
        ));
        let error = ParticleEmitter::from_component_str("count=4;mode=gpuu")
            .expect_err("bad mode must fail");
        assert!(matches!(
            error,
            ParticleComponentError::InvalidChoice { .. }
        ));
        assert!(error.to_string().contains("cpu|gpu"), "{error}");
        assert!(matches!(
            ParticleEmitter::from_component_str("count=4;seed=-1"),
            Err(ParticleComponentError::InvalidField(_))
        ));
        // seed=0 是合法种子（LCG 从 0 也能推出确定序列）。
        assert_eq!(
            ParticleEmitter::from_component_str("count=4;seed=0")
                .expect("zero seed")
                .seed,
            Some(0)
        );
    }

    /// `seed` 让同一份参数在两个系统里生成**逐位相同**的 burst；不给则沿用历史状态（旧行为）。
    #[test]
    fn seed_replays_a_burst_and_absence_keeps_the_stream() {
        let seeded = ParticleEmitter {
            count: 8,
            seed: Some(0xCAFE),
            ..ParticleEmitter::default()
        };
        let mut first = ParticleSystem::new();
        let mut second = ParticleSystem::new();
        first.burst_with(Vec2::zero(), &seeded);
        second.burst_with(Vec2::new(3.0, 3.0), &seeded);
        let offset = Vec2::new(3.0, 3.0);
        for (a, b) in first.particles().iter().zip(second.particles()) {
            assert_eq!(a.velocity, b.velocity, "同 seed 的角度/速度抽样必须一致");
            assert_eq!(a.life, b.life);
            assert_eq!(a.position + offset, b.position, "位置只随发射点平移");
        }
        // 无 seed 的第二次 burst 接着上一次的序列走 ⇒ 与第一次不同（旧行为，没有被 seed 钉住）。
        let unseeded = ParticleEmitter {
            count: 8,
            ..ParticleEmitter::default()
        };
        let mut system = ParticleSystem::new();
        system.burst_with(Vec2::zero(), &unseeded);
        let before: Vec<_> = system.particles().to_vec();
        system.burst_with(Vec2::zero(), &unseeded);
        let after: Vec<_> = system.particles()[8..].to_vec();
        assert_ne!(before, after, "不带 seed 时抽样序列应继续前进");
        // mode 单独出现**不**改变 CPU 抽样（回退/派发都不动数据）。
        let mut gpu_mode = ParticleSystem::new();
        gpu_mode.burst_with(
            Vec2::zero(),
            &ParticleEmitter {
                count: 8,
                mode: ParticleMode::Gpu,
                ..ParticleEmitter::default()
            },
        );
        let mut cpu_mode = ParticleSystem::new();
        cpu_mode.burst_with(Vec2::zero(), &ParticleEmitter::default());
        assert_eq!(gpu_mode.particles(), cpu_mode.particles());
    }

    /// 连续流的抽样器：`clear` 只丢粒子，LCG 继续前进（不是每帧从同一 seed 重来）。
    #[test]
    fn clear_keeps_the_sampling_sequence_advancing() {
        let emitter = ParticleEmitter {
            count: 2,
            speed: 0.0,
            ..ParticleEmitter::default()
        };
        let mut sampler = ParticleSystem::new();
        sampler.burst_with(Vec2::zero(), &emitter);
        assert_eq!(sampler.len(), 2);
        let first = sampler.particles()[0];
        sampler.clear();
        assert!(sampler.is_empty());
        sampler.burst_with(Vec2::zero(), &emitter);
        assert_eq!(sampler.len(), 2);
        assert_ne!(
            sampler.particles()[0],
            first,
            "clear 后的下一批必须继续推进随机序列"
        );
    }

    /// 落点判定与渲染器 `configure_gpu_particles` 的拒绝条件一致；回退用同一份参数走 CPU。
    #[test]
    fn placement_falls_back_to_cpu_whenever_gpu_cannot_take_it() {
        let burst = ParticleEmitter {
            count: 8,
            ..ParticleEmitter::default()
        };
        assert_eq!(
            burst.placement(false),
            ParticlePlacement::Cpu(ParticleCpuReason::Requested),
            "默认 cpu：不是回退"
        );
        let continuous = ParticleEmitter {
            mode: ParticleMode::Gpu,
            rate: 30.0,
            ..burst
        };
        let gpu = ParticleEmitter {
            mode: ParticleMode::Gpu,
            ..burst
        };
        assert_eq!(
            gpu.placement(false),
            ParticlePlacement::Cpu(ParticleCpuReason::NoCompute)
        );
        assert_eq!(
            gpu.placement(true),
            ParticlePlacement::Gpu,
            "compute 可用 + burst 语义才落 GPU"
        );
        assert_eq!(
            continuous.placement(true),
            ParticlePlacement::Cpu(ParticleCpuReason::ContinuousRate),
            "渲染器没声明连续流时，rate 必须回 CPU 而不是静默忽略"
        );
        assert_eq!(
            continuous.placement_with_streams(true, true),
            ParticlePlacement::Gpu,
            "渲染器支持连续流后，rate 才允许落 GPU"
        );
        assert_eq!(
            continuous.placement_with_streams(true, false),
            ParticlePlacement::Cpu(ParticleCpuReason::ContinuousRate)
        );
        assert_eq!(
            continuous.placement_with_streams(false, true),
            ParticlePlacement::Cpu(ParticleCpuReason::NoCompute),
            "能力缺失先于参数不支持上报"
        );
        // "回退"只属于"要了 gpu 却落在 cpu"；作者选 cpu 不算回退。
        assert!(
            !ParticlePlacement::Cpu(ParticleCpuReason::Requested)
                .cpu_reason()
                .is_some_and(ParticleCpuReason::is_fallback)
        );
        for reason in [
            ParticleCpuReason::NoCompute,
            ParticleCpuReason::ContinuousRate,
            ParticleCpuReason::NotWired,
        ] {
            assert!(
                ParticlePlacement::Cpu(reason)
                    .cpu_reason()
                    .is_some_and(ParticleCpuReason::is_fallback),
                "{reason:?} 是可诊断的回退"
            );
            assert_ne!(reason.as_str(), "requested");
        }
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

    /// 逐粒子颤动：`flutter=0` 时同一 burst 的两颗粒子同位同速（旧行为不变）；
    /// 开启后各自相位走出不同路径，同 seed 复跑逐位相同。
    #[test]
    fn flutter_sways_particles_independently() {
        let emitter = ParticleEmitter {
            count: 2,
            speed: 0.0,
            life: 2.0,
            seed: Some(7),
            flutter: 1.5,
            flutter_hz: 1.0,
            ..ParticleEmitter::default()
        };
        assert!(emitter.validate().is_ok());

        let mut still = ParticleSystem::new();
        still.burst_with(
            Vec2::zero(),
            &ParticleEmitter {
                flutter: 0.0,
                flutter_hz: 0.0,
                ..emitter
            },
        );
        still.step(0.5);
        assert_eq!(still.particles[0].position, still.particles[1].position);
        assert_eq!(still.particles[0].velocity, still.particles[1].velocity);

        let mut sway = ParticleSystem::new();
        sway.burst_with(Vec2::zero(), &emitter);
        sway.step(0.5);
        let dx = (sway.particles[0].position.x - sway.particles[1].position.x).abs();
        let dy = (sway.particles[0].position.y - sway.particles[1].position.y).abs();
        assert!(
            dx > 1e-3 || dy > 1e-3,
            "两颗粒子应有不同位移：dx={dx} dy={dy}"
        );

        let mut again = ParticleSystem::new();
        again.burst_with(Vec2::zero(), &emitter);
        again.step(0.5);
        assert_eq!(again.particles[0].position, sway.particles[0].position);
        assert_eq!(again.particles[1].position, sway.particles[1].position);
    }

    #[test]
    fn curve_bends_the_horizontal_path() {
        let emitter = ParticleEmitter {
            count: 1,
            speed: 100.0,
            life: 200.0,
            gravity: Vec2::zero(),
            curve: 2.0,
            ..ParticleEmitter::default()
        };
        let mut sys = ParticleSystem::new();
        sys.burst_with(Vec2::zero(), &emitter);
        let start = sys.particles()[0].velocity;
        let start_speed = start.length();
        for _ in 0..30 {
            sys.step(1.0 / 60.0);
        }
        let particle = &sys.particles()[0];
        let speed = particle.velocity.length();
        assert!(
            (speed - start_speed).abs() < 1e-2,
            "弯曲只转方向、不该改速率：start={start_speed} now={speed}",
        );
        let cos_turn = start.dot(particle.velocity) / (start_speed * speed);
        assert!(cos_turn < 0.99, "0.5 s 后方向应被转过一截：cos={cos_turn}",);
        assert_eq!(particle.velocity_z, 0.0, "z 是竖直轴，弯曲不许漏进去");

        let mut straight = ParticleSystem::new();
        straight.burst_with(
            Vec2::zero(),
            &ParticleEmitter {
                curve: 0.0,
                ..emitter
            },
        );
        let straight_start = straight.particles()[0].velocity;
        for _ in 0..30 {
            straight.step(1.0 / 60.0);
        }
        assert_eq!(straight.particles()[0].velocity, straight_start);
    }

    #[test]
    fn curve_rejects_out_of_range() {
        let mut emitter = ParticleEmitter {
            curve: 13.0,
            ..ParticleEmitter::default()
        };
        assert!(emitter.validate().is_err());
        emitter.curve = -12.0;
        assert!(emitter.validate().is_ok());
        emitter.curve = f32::NAN;
        assert!(emitter.validate().is_err());
    }

    #[test]
    fn flutter_rejects_invalid_range() {
        let mut emitter = ParticleEmitter {
            flutter: -0.1,
            ..ParticleEmitter::default()
        };
        assert!(emitter.validate().is_err());
        emitter.flutter = 1.0;
        emitter.flutter_hz = 20.1;
        assert!(emitter.validate().is_err());
        emitter.flutter_hz = 20.0;
        assert!(emitter.validate().is_ok());
        emitter.flutter = f32::NAN;
        assert!(emitter.validate().is_err());
    }

    /// curl 噪声场：真的推粒子、近邻同向（空间相干）、半个波长反向，
    /// 且**严格无散**（数值中央差分；有源汇时这一条必红）。
    #[test]
    fn curl_field_is_coherent_and_divergence_free() {
        let emitter = ParticleEmitter {
            count: 1,
            speed: 0.0,
            life: 5.0,
            gravity: Vec2::zero(),
            curl: 50.0,
            curl_scale: 0.3,
            curl_hz: 0.0,
            ..ParticleEmitter::default()
        };
        assert!(emitter.validate().is_ok());
        // 单步 1 ms：位移可忽略，速度 ≈ 场矢量 × dt（所有采样同一 time）。
        let sample = |position: [f32; 3]| {
            let mut sys = ParticleSystem::new();
            sys.burst_with_3d(position, &emitter);
            sys.step(1e-3);
            let p = &sys.particles()[0];
            [p.velocity.x, p.velocity.y, p.velocity_z]
        };
        let dot = |u: [f32; 3], v: [f32; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
        let norm = |u: [f32; 3]| dot(u, u).sqrt();
        let cos_between = |u: [f32; 3], v: [f32; 3]| dot(u, v) / (norm(u) * norm(v));

        let center = sample([0.7, 0.3, 0.4]);
        assert!(norm(center) > 1e-3, "curl 场应真的产生速度：{center:?}");
        let near = sample([0.9, 0.3, 0.4]);
        assert!(
            cos_between(center, near) > 0.99,
            "近邻应共享方向（空间相干）：{}",
            cos_between(center, near)
        );
        let far = sample([0.7 + std::f32::consts::PI / 0.3, 0.3, 0.4]);
        assert!(
            cos_between(center, far) < -0.9,
            "半个波长应反向：{}",
            cos_between(center, far)
        );

        let h = 0.05;
        let div = (sample([0.7 + h, 0.3, 0.4])[0] - sample([0.7 - h, 0.3, 0.4])[0]
            + sample([0.7, 0.3 + h, 0.4])[1]
            - sample([0.7, 0.3 - h, 0.4])[1]
            + sample([0.7, 0.3, 0.4 + h])[2]
            - sample([0.7, 0.3, 0.4 - h])[2])
            / (2.0 * h);
        assert!(div.abs() < 1e-4, "curl 噪声应无源汇：div={div}");

        let mut off = ParticleSystem::new();
        off.burst_with_3d(
            [0.7, 0.3, 0.4],
            &ParticleEmitter {
                curl: 0.0,
                ..emitter
            },
        );
        off.step(1e-3);
        assert_eq!(off.particles()[0].velocity, Vec2::zero(), "curl=0 不许动");
        assert_eq!(off.particles()[0].velocity_z, 0.0);
    }

    #[test]
    fn curl_rejects_out_of_range() {
        let mut emitter = ParticleEmitter {
            curl: 50.1,
            ..ParticleEmitter::default()
        };
        assert!(emitter.validate().is_err());
        emitter.curl = 50.0;
        assert!(emitter.validate().is_ok());
        emitter.curl_scale = 4.1;
        assert!(emitter.validate().is_err());
        emitter.curl_scale = 4.0;
        emitter.curl_hz = 5.1;
        assert!(emitter.validate().is_err());
        emitter.curl_hz = 5.0;
        assert!(emitter.validate().is_ok());
        emitter.curl = f32::NAN;
        assert!(emitter.validate().is_err());
    }
}
