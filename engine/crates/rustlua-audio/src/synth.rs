//! 合成音效原语（M82 C 批）：规格表驱动，渲染成 PCM 采样，喂现有音频总线。
//!
//! 只做**合成原语**（振荡器 + 噪声 + ADSR 包络 + biquad 滤波），不含"调得像枪声/引擎"的预设——
//! 那部分是人耳定标，不进机器门（计划 §范围）。本模块**平台无关**（不碰 rodio/设备）：
//! 桌面设备把 [`render_samples`] 的采样包成 rodio `SamplesBuffer` 挂上 Sink；平台出口（web/微信）
//! 的合成另走各自的音频 buffer（本批不宣称真机绿）。
//!
//! 确定性：同 spec + 同采样率 → 同采样（噪声用整数 LCG，正弦用 `sin`——音频门只断言 RMS/峰值，
//! 不做跨平台字节恒等）。

/// 频率下限（Hz）。
pub const SYNTH_MIN_FREQ: f32 = 20.0;
/// 频率上限（Hz）。
pub const SYNTH_MAX_FREQ: f32 = 20_000.0;
/// 时长下限（秒）。
pub const SYNTH_MIN_DUR: f32 = 0.01;
/// 时长上限（秒）。
pub const SYNTH_MAX_DUR: f32 = 10.0;
/// 包络各段时长上限（秒）。
pub const SYNTH_MAX_ENV: f32 = 10.0;
/// 增益上限。
pub const SYNTH_MAX_GAIN: f32 = 1.0;
/// 滤波器截止频率下限（Hz）。
pub const SYNTH_MIN_CUTOFF: f32 = 20.0;
/// 滤波器截止频率上限（Hz）。
pub const SYNTH_MAX_CUTOFF: f32 = 20_000.0;
/// 合成输出采样率（桌面设备与渲染缓冲门同口径）。
pub const SYNTH_SAMPLE_RATE: u32 = 44_100;

/// 振荡器波形。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthWave {
    Sine,
    Square,
    Saw,
    Triangle,
    Noise,
}

impl SynthWave {
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "sine" => Some(Self::Sine),
            "square" => Some(Self::Square),
            "saw" => Some(Self::Saw),
            "triangle" => Some(Self::Triangle),
            "noise" => Some(Self::Noise),
            _ => None,
        }
    }

    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Sine => "sine",
            Self::Square => "square",
            Self::Saw => "saw",
            Self::Triangle => "triangle",
            Self::Noise => "noise",
        }
    }
}

/// 滤波器类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterKind {
    LowPass,
    HighPass,
}

impl FilterKind {
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "lowpass" => Some(Self::LowPass),
            "highpass" => Some(Self::HighPass),
            _ => None,
        }
    }

    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::LowPass => "lowpass",
            Self::HighPass => "highpass",
        }
    }
}

/// 一个 biquad 滤波档。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterSpec {
    pub kind: FilterKind,
    pub cutoff: f32,
}

/// 一次合成的完整规格。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SynthSpec {
    pub wave: SynthWave,
    /// 基频（Hz；噪声忽略）。
    pub freq: f32,
    /// 按键持续时长（秒）。
    pub dur: f32,
    /// 起音（秒）。
    pub attack: f32,
    /// 衰减（秒）。
    pub decay: f32,
    /// 持续电平（0..=1）。
    pub sustain: f32,
    /// 释放（秒）。
    pub release: f32,
    /// 峰值增益（0..=1）。
    pub gain: f32,
    /// 可选滤波（截止频率 Hz）。
    pub filter: Option<FilterSpec>,
}

impl Default for SynthSpec {
    fn default() -> Self {
        Self {
            wave: SynthWave::Sine,
            freq: 440.0,
            dur: 0.3,
            attack: 0.01,
            decay: 0.05,
            sustain: 0.7,
            release: 0.1,
            gain: 0.5,
            filter: None,
        }
    }
}

impl SynthSpec {
    /// 范围校验。越界一律报错（不静默成功）。
    pub fn validate(&self) -> Result<(), SynthError> {
        if self.wave != SynthWave::Noise
            && (!self.freq.is_finite() || !(SYNTH_MIN_FREQ..=SYNTH_MAX_FREQ).contains(&self.freq))
        {
            return Err(SynthError::FreqOutOfRange { freq: self.freq });
        }
        if !self.dur.is_finite() || !(SYNTH_MIN_DUR..=SYNTH_MAX_DUR).contains(&self.dur) {
            return Err(SynthError::DurOutOfRange { dur: self.dur });
        }
        for (label, value) in [
            ("attack", self.attack),
            ("decay", self.decay),
            ("release", self.release),
        ] {
            if !value.is_finite() || !(0.0..=SYNTH_MAX_ENV).contains(&value) {
                return Err(SynthError::EnvOutOfRange { label, value });
            }
        }
        if !self.sustain.is_finite() || !(0.0..=1.0).contains(&self.sustain) {
            return Err(SynthError::SustainOutOfRange {
                sustain: self.sustain,
            });
        }
        if !self.gain.is_finite() || !(0.0..=SYNTH_MAX_GAIN).contains(&self.gain) {
            return Err(SynthError::GainOutOfRange { gain: self.gain });
        }
        if let Some(filter) = self.filter
            && (!filter.cutoff.is_finite()
                || !(SYNTH_MIN_CUTOFF..=SYNTH_MAX_CUTOFF).contains(&filter.cutoff))
        {
            return Err(SynthError::CutoffOutOfRange {
                cutoff: filter.cutoff,
            });
        }
        Ok(())
    }
}

/// 合成规格校验错误。
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum SynthError {
    #[error("synth freq {freq} out of range {SYNTH_MIN_FREQ}..={SYNTH_MAX_FREQ}")]
    FreqOutOfRange { freq: f32 },
    #[error("synth dur {dur} out of range {SYNTH_MIN_DUR}..={SYNTH_MAX_DUR}")]
    DurOutOfRange { dur: f32 },
    #[error("synth {label} {value} out of range 0..={SYNTH_MAX_ENV}")]
    EnvOutOfRange { label: &'static str, value: f32 },
    #[error("synth sustain {sustain} out of range 0..=1")]
    SustainOutOfRange { sustain: f32 },
    #[error("synth gain {gain} out of range 0..=1")]
    GainOutOfRange { gain: f32 },
    #[error("synth filter cutoff {cutoff} out of range {SYNTH_MIN_CUTOFF}..={SYNTH_MAX_CUTOFF}")]
    CutoffOutOfRange { cutoff: f32 },
}

/// 把规格渲染成单声道 f32 采样（`-1..=1`）。长度 = `(dur + release) * sample_rate`。
pub fn render_samples(spec: &SynthSpec, sample_rate: u32) -> Result<Vec<f32>, SynthError> {
    spec.validate()?;
    let rate = sample_rate.max(1) as f32;
    let total = (((spec.dur + spec.release) * rate).round() as usize).max(1);
    let mut out = Vec::with_capacity(total);
    let mut phase = 0.0f32;
    let step = if spec.wave == SynthWave::Noise {
        0.0
    } else {
        spec.freq / rate
    };
    let mut lcg = 0x1234_5678u32;
    let mut filter = spec.filter.map(|filter| Biquad::new(filter, rate));
    for index in 0..total {
        let t = index as f32 / rate;
        let envelope = envelope(spec, t);
        let osc = if spec.wave == SynthWave::Noise {
            lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (lcg >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        } else {
            oscillator(spec.wave, phase)
        };
        phase = (phase + step).fract();
        let mut sample = osc * envelope * spec.gain;
        if let Some(filter) = filter.as_mut() {
            sample = filter.process(sample);
        }
        out.push(sample.clamp(-1.0, 1.0));
    }
    Ok(out)
}

/// 振荡器：相位 `0..1` → `-1..=1`（噪声不走这里）。
fn oscillator(wave: SynthWave, phase: f32) -> f32 {
    match wave {
        SynthWave::Sine => (std::f32::consts::TAU * phase).sin(),
        SynthWave::Square => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        SynthWave::Saw => 2.0 * phase - 1.0,
        SynthWave::Triangle => 4.0 * (phase - 0.5).abs() - 1.0,
        SynthWave::Noise => 0.0,
    }
}

/// ADSR 包络：起音→衰减→持续→释放。
fn envelope(spec: &SynthSpec, t: f32) -> f32 {
    let sustain = spec.sustain.clamp(0.0, 1.0);
    if spec.attack > 0.0 && t < spec.attack {
        return (t / spec.attack).clamp(0.0, 1.0);
    }
    let decay_end = spec.attack + spec.decay;
    if spec.decay > 0.0 && t < decay_end {
        let progress = ((t - spec.attack) / spec.decay).clamp(0.0, 1.0);
        return 1.0 - (1.0 - sustain) * progress;
    }
    if t < spec.dur {
        return sustain;
    }
    if spec.release > 0.0 {
        let progress = ((t - spec.dur) / spec.release).clamp(0.0, 1.0);
        return sustain * (1.0 - progress);
    }
    0.0
}

/// RBJ cookbook 的单段 biquad（低通/高通），Direct Form I。
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn new(spec: FilterSpec, sample_rate: f32) -> Self {
        let w0 = std::f32::consts::TAU * (spec.cutoff / sample_rate).clamp(0.0, 0.49);
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let alpha = sin_w0 / (2.0 * q);
        let a0 = 1.0 + alpha;
        let (b0, b1, b2) = match spec.kind {
            FilterKind::LowPass => ((1.0 - cos_w0) / 2.0, 1.0 - cos_w0, (1.0 - cos_w0) / 2.0),
            FilterKind::HighPass => ((1.0 + cos_w0) / 2.0, -(1.0 + cos_w0), (1.0 + cos_w0) / 2.0),
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: -2.0 * cos_w0 / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// 峰值（最大绝对值）。
#[must_use]
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()))
}

/// 均方根（RMS）。
#[must_use]
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32) -> SynthSpec {
        SynthSpec {
            wave: SynthWave::Sine,
            freq,
            dur: 0.5,
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.0,
            gain: 1.0,
            filter: None,
        }
    }

    #[test]
    fn sine_zero_crossings_match_frequency() {
        let spec = sine(440.0);
        let samples = render_samples(&spec, 8_000).expect("render");
        let crossings = samples
            .windows(2)
            .filter(|pair| (pair[0] >= 0.0) != (pair[1] >= 0.0))
            .count();
        // 0.5s × 440Hz = 220 周期 ⇒ ~440 次过零。
        assert!(
            (435..=445).contains(&crossings),
            "440Hz 半秒应约有 440 次过零，实得 {crossings}"
        );
    }

    #[test]
    fn envelope_peaks_then_releases_to_silence() {
        let spec = SynthSpec {
            wave: SynthWave::Sine,
            freq: 441.0,
            dur: 0.2,
            attack: 0.01,
            decay: 0.02,
            sustain: 0.5,
            release: 0.05,
            gain: 1.0,
            filter: None,
        };
        let samples = render_samples(&spec, 44_100).expect("render");
        let top = peak(&samples);
        assert!((0.4..=1.0).contains(&top), "峰值应接近 1（起音段）：{top}");
        // 释放尾：最后几个采样应趋近 0。
        let tail = &samples[samples.len() - 8..];
        assert!(
            tail.iter().all(|s| s.abs() < 0.05),
            "释放尾应收敛到静音：{tail:?}"
        );
        assert!(rms(&samples) > 0.05, "整体 RMS 应非 0");
    }

    #[test]
    fn gain_scales_output() {
        let loud = sine(220.0);
        let quiet = SynthSpec { gain: 0.5, ..loud };
        let loud_peak = peak(&render_samples(&loud, 16_000).unwrap());
        let quiet_peak = peak(&render_samples(&quiet, 16_000).unwrap());
        let ratio = quiet_peak / loud_peak;
        assert!((0.45..=0.55).contains(&ratio), "半增益应约减半：{ratio}");
    }

    #[test]
    fn noise_is_bounded_and_deterministic() {
        let spec = SynthSpec {
            wave: SynthWave::Noise,
            filter: None,
            ..SynthSpec::default()
        };
        let a = render_samples(&spec, 44_100).unwrap();
        let b = render_samples(&spec, 44_100).unwrap();
        assert_eq!(a, b, "噪声也必须确定（整数 LCG）");
        assert!(a.iter().all(|s| s.abs() <= 1.0), "噪声不得越界");
        assert!(rms(&a) > 0.05, "噪声应有能量");
    }

    #[test]
    fn lowpass_reduces_high_frequency_energy() {
        let square = SynthSpec {
            wave: SynthWave::Square,
            freq: 2_000.0,
            dur: 0.3,
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.0,
            gain: 0.8,
            filter: None,
        };
        let filtered = SynthSpec {
            filter: Some(FilterSpec {
                kind: FilterKind::LowPass,
                cutoff: 300.0,
            }),
            ..square
        };
        let raw_rms = rms(&render_samples(&square, 44_100).unwrap());
        let filtered_rms = rms(&render_samples(&filtered, 44_100).unwrap());
        assert!(
            filtered_rms < raw_rms * 0.6,
            "300Hz 低通应显著压低 2kHz 方波能量：raw={raw_rms} filtered={filtered_rms}"
        );
    }

    #[test]
    fn validate_rejects_out_of_range() {
        let bad_freq = SynthSpec {
            freq: 5.0,
            ..SynthSpec::default()
        };
        assert!(matches!(
            bad_freq.validate(),
            Err(SynthError::FreqOutOfRange { .. })
        ));
        let bad_dur = SynthSpec {
            dur: 0.0,
            ..SynthSpec::default()
        };
        assert!(matches!(
            bad_dur.validate(),
            Err(SynthError::DurOutOfRange { .. })
        ));
        let bad_gain = SynthSpec {
            gain: 1.5,
            ..SynthSpec::default()
        };
        assert!(matches!(
            bad_gain.validate(),
            Err(SynthError::GainOutOfRange { .. })
        ));
        let bad_cutoff = SynthSpec {
            filter: Some(FilterSpec {
                kind: FilterKind::LowPass,
                cutoff: 10.0,
            }),
            ..SynthSpec::default()
        };
        assert!(matches!(
            bad_cutoff.validate(),
            Err(SynthError::CutoffOutOfRange { .. })
        ));
        // 噪声忽略 freq，非法 freq 也不该被拒。
        let noise_any_freq = SynthSpec {
            wave: SynthWave::Noise,
            freq: 1.0,
            ..SynthSpec::default()
        };
        assert!(noise_any_freq.validate().is_ok());
    }
}
