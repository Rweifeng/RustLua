//! 无障碍音色控制：单声道 + 低/高频衰减（M74 欠项）。
//!
//! 这里只放**纯 DSP/参数**，不碰 rodio：桌面播放器在 `device.rs` 里用
//! [`crate::tone::ToneFilter`] 包住解码后的 f32 采样；wasm / 平台出口继续走自己的
//! `AudioSinkProvider`，不会因为本模块把 rodio 拉进 wasm。
//!
//! 口径：
//! - `bass_db` / `treble_db` 只允许 `[-24.0, 0.0]`，0 表示不切；**不做 boost**，
//!   避免用户把已经接近满幅的素材推到削波；
//! - 单声道是等权平均（不是只取左声道），因此不会丢右声道的音效；
//! - 默认 [`crate::tone::ToneControl::OFF`] 是严格直通，播放器不包 filter，旧声音逐位不变。

use std::f32::consts::PI;

use thiserror::Error;

/// 低/高频最多切 24 dB。
pub const MAX_CUT_DB: f32 = 24.0;
/// 低/高频分界的低通截止频率（Hz）。一阶分频，足够做“降低高频/低频”的无障碍档。
pub const TONE_SPLIT_HZ: f32 = 400.0;

/// 非法音色参数。
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ToneError {
    #[error("tone bass_db must be finite and in [-24, 0], got {0}")]
    BassDb(f32),
    #[error("tone treble_db must be finite and in [-24, 0], got {0}")]
    TrebleDb(f32),
}

/// 播放期音色控制。`Default` 与 [`Self::OFF`] 相同：立体声、零衰减。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneControl {
    mono: bool,
    bass_db: f32,
    treble_db: f32,
}

impl Default for ToneControl {
    fn default() -> Self {
        Self::OFF
    }
}

impl ToneControl {
    /// 严格直通。
    pub const OFF: Self = Self {
        mono: false,
        bass_db: 0.0,
        treble_db: 0.0,
    };

    /// 构造并校验。`mono` 与两段衰减可独立组合。
    ///
    /// # Errors
    /// 任一分贝值非有限或不在 `[-24, 0]`：不静默 clamp，避免设置面板显示
    /// 一个值、实际播放另一个值。
    pub fn new(mono: bool, bass_db: f32, treble_db: f32) -> Result<Self, ToneError> {
        if !bass_db.is_finite() || !(-MAX_CUT_DB..=0.0).contains(&bass_db) {
            return Err(ToneError::BassDb(bass_db));
        }
        if !treble_db.is_finite() || !(-MAX_CUT_DB..=0.0).contains(&treble_db) {
            return Err(ToneError::TrebleDb(treble_db));
        }
        Ok(Self {
            mono,
            bass_db,
            treble_db,
        })
    }

    #[must_use]
    pub const fn mono(self) -> bool {
        self.mono
    }

    #[must_use]
    pub const fn bass_db(self) -> f32 {
        self.bass_db
    }

    #[must_use]
    pub const fn treble_db(self) -> f32 {
        self.treble_db
    }

    /// 是否严格直通（不需要包 filter）。
    #[must_use]
    pub fn is_neutral(self) -> bool {
        !self.mono && self.bass_db == 0.0 && self.treble_db == 0.0
    }

    /// 只改单声道开关，保留两段衰减。
    #[must_use]
    pub fn with_mono(self, mono: bool) -> Self {
        Self { mono, ..self }
    }

    /// 只改两段衰减，保留单声道开关。
    ///
    /// # Errors
    /// 见 [`Self::new`]。
    pub fn with_cut(self, bass_db: f32, treble_db: f32) -> Result<Self, ToneError> {
        Self::new(self.mono, bass_db, treble_db)
    }
}

/// 一阶低/高频分频器；`mono` 的降混在播放器层做，本类型只处理一个声道。
#[derive(Debug, Clone)]
pub struct ToneFilter {
    alpha: f32,
    bass_gain: f32,
    treble_gain: f32,
    low: [f32; 2],
    channels: usize,
}

impl ToneFilter {
    /// `channels` 是**输入**声道数；状态按声道奇偶各存一份（2 声道足够，
    /// 多声道时相邻声道复用同一只低通，仍然稳定且无爆音）。
    #[must_use]
    pub fn new(sample_rate: u32, channels: u16, control: ToneControl) -> Self {
        let sr = sample_rate.max(1) as f32;
        let alpha = 1.0 - (-2.0 * PI * TONE_SPLIT_HZ / sr).exp();
        Self {
            alpha,
            bass_gain: db_to_gain(control.bass_db),
            treble_gain: db_to_gain(control.treble_db),
            low: [0.0; 2],
            channels: usize::from(channels.max(1)),
        }
    }

    /// 对一个采样做低/高频分频与增益。输出被钳在 `[-1, 1]`，防止两段叠加削波。
    pub fn process(&mut self, channel: usize, sample: f32) -> f32 {
        let slot = channel % self.channels.max(1) % 2;
        let low = &mut self.low[slot];
        *low += self.alpha * (sample - *low);
        let high = sample - *low;
        (*low * self.bass_gain + high * self.treble_gain).clamp(-1.0, 1.0)
    }
}

fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_strict_passthrough() {
        assert!(ToneControl::default().is_neutral());
        assert_eq!(ToneControl::OFF, ToneControl::default());
        let control = ToneControl::new(false, -6.0, -12.0).unwrap();
        assert!(!control.is_neutral());
        assert!(control.with_mono(true).mono());
    }

    #[test]
    fn rejects_out_of_range_and_non_finite() {
        assert!(ToneControl::new(false, 1.0, 0.0).is_err());
        assert!(ToneControl::new(false, -25.0, 0.0).is_err());
        assert!(ToneControl::new(false, f32::NAN, 0.0).is_err());
        assert!(ToneControl::new(false, 0.0, f32::INFINITY).is_err());
    }

    #[test]
    fn bass_cut_attenuates_dc() {
        let control = ToneControl::new(false, -24.0, 0.0).unwrap();
        let mut filter = ToneFilter::new(48_000, 2, control);
        let mut last = 0.0;
        for _ in 0..4_000 {
            last = filter.process(0, 1.0);
        }
        assert!(last < 0.08, "DC after -24 dB bass cut was {last}");
    }

    #[test]
    fn neutral_filter_is_identity_for_dc() {
        let control = ToneControl::OFF;
        let mut filter = ToneFilter::new(48_000, 2, control);
        let mut last = 0.0;
        for _ in 0..4_000 {
            last = filter.process(0, 0.5);
        }
        assert!((last - 0.5).abs() < 1e-4, "neutral DC was {last}");
    }
}
