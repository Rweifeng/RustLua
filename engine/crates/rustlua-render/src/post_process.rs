//! docs/54 §4.3 / P3-R01：后处理与 Tonemap 的参数（`RenderPlan` 的最后两条通道）。
//!
//! §4.3 的 `RenderPlan` 以 `PostProcess` 收尾。P3-R01 在它之前插入了 `Tonemap`：
//!
//! ```text
//! 3D / 世界 Sprite / 粒子 → Tonemap（HDR→LDR）→ 2D/UI/矢量文字 → PostProcess → 目标
//! ```
//!
//! 两个通道的参数都放在本模块，但**归属不同**：
//!
//! - `Tonemap` 曲线与 `exposure_ev` 只属于 HDR 的 `Tonemap` pass；HDR 模式下
//!   `PostProcess.exposure` 必须为 `1.0`，否则就是双重曝光（文档与校验见
//!   [`PostProcess::hdr_compatible`]）。
//! - LDR 模式没有 `Tonemap` pass，`exposure_ev` 折进后处理的曝光增益（`exposure * 2^ev`），
//!   因此"我调了曝光但没生效"不会发生。
//! - `tint`/`vignette` 始终属于后处理，不管 HDR 还是 LDR。
//!
//! 参数校验沿用本仓库的做法：非有限值与越界值**报错**而不是静默夹紧——"我调了参数但画面没
//! 变"是最难查的一类问题（可能是值非法被静默丢掉），宁可当场拒绝。

/// 色调映射曲线。`Aces` 是默认值（P3-R01 验收），`Reinhard` 是可选项。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tonemap {
    /// ACES 的 Narkowicz 拟合：高光肩部平滑，8.0 线性值仍能压回 LDR。
    #[default]
    Aces,
    /// Reinhard：`x / (1 + x)`，单调、无参数，作为对照曲线。
    Reinhard,
}

impl Tonemap {
    /// 稳定短名（Profiler / 日志 / golden 共用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aces => "aces",
            Self::Reinhard => "reinhard",
        }
    }

    /// 上传给 WGSL 的曲线代码（`0` = ACES，`1` = Reinhard）。
    #[must_use]
    pub const fn code(self) -> f32 {
        match self {
            Self::Aces => 0.0,
            Self::Reinhard => 1.0,
        }
    }

    /// CPU 参考实现（单测/编辑器预览用），必须与 `TONEMAP_SHADER` 的公式逐行对应。
    ///
    /// 输入是**线性** RGB，先乘 `2^exposure_ev`，再走曲线，最后夹到 `0..=1`。
    #[must_use]
    pub fn apply_rgb(self, rgb: [f32; 3], exposure_ev: f32) -> [f32; 3] {
        let gain = 2.0_f32.powf(exposure_ev);
        let curve = |x: f32| -> f32 {
            let x = (x * gain).max(0.0);
            let mapped = match self {
                Self::Aces => {
                    let (a, b, c, d, e) = (2.51, 0.03, 2.43, 0.59, 0.14);
                    (x * (a * x + b)) / (x * (c * x + d) + e)
                }
                Self::Reinhard => x / (1.0 + x),
            };
            mapped.clamp(0.0, 1.0)
        };
        [curve(rgb[0]), curve(rgb[1]), curve(rgb[2])]
    }
}

/// `exposure_ev` 的合法范围（±20 档 ≈ 约 100 万倍增益）。超出范围几乎一定是单位写错
/// （把 EV 当成线性倍数），因此报错而不是接受一个会变成 `inf` 的值。
pub const EXPOSURE_EV_RANGE: std::ops::RangeInclusive<f32> = -20.0..=20.0;

/// 一帧后处理的参数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PostProcess {
    /// 线性曝光倍数（`1.0` = 不变，`0.0` = 全黑）。必须有限且非负。
    ///
    /// HDR 模式下必须为 `1.0`——曝光只走 [`Self::exposure_ev`]，避免同一份曝光乘两次。
    pub exposure: f32,
    /// RGBA 乘法着色（`[1.0; 4]` = 不变）。分量必须有限且非负。
    pub tint: [f32; 4],
    /// 暗角强度（`0.0` = 无暗角，`1.0` = 四角全黑）。必须有限且落在 `0.0..=1.0`。
    pub vignette: f32,
    /// HDR 的 Tonemap 曲线；LDR 模式不产生 Tonemap pass，因此该字段不参与像素。
    pub tonemap: Tonemap,
    /// 曝光档位（EV）：线性增益 = `2^exposure_ev`。HDR 由 Tonemap pass 消费；LDR 折进后处理。
    pub exposure_ev: f32,
}

impl Default for PostProcess {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            tint: [1.0; 4],
            vignette: 0.0,
            tonemap: Tonemap::Aces,
            exposure_ev: 0.0,
        }
    }
}

impl PostProcess {
    /// 兼容旧 3 字段构造：Tonemap 用默认 ACES，`exposure_ev = 0`。
    pub fn new(exposure: f32, tint: [f32; 4], vignette: f32) -> Result<Self, PostProcessError> {
        Self::with_tonemap(exposure, tint, vignette, Tonemap::default(), 0.0)
    }

    /// HDR 模式的标准构造：`exposure` 固定为 `1.0`，曝光只走 `exposure_ev`。
    pub fn new_hdr(
        tonemap: Tonemap,
        exposure_ev: f32,
        tint: [f32; 4],
        vignette: f32,
    ) -> Result<Self, PostProcessError> {
        Self::with_tonemap(1.0, tint, vignette, tonemap, exposure_ev)
    }

    /// 全字段构造 + 校验。
    pub fn with_tonemap(
        exposure: f32,
        tint: [f32; 4],
        vignette: f32,
        tonemap: Tonemap,
        exposure_ev: f32,
    ) -> Result<Self, PostProcessError> {
        if !exposure.is_finite() {
            return Err(PostProcessError::NonFiniteExposure(exposure));
        }
        if exposure < 0.0 {
            return Err(PostProcessError::NegativeExposure(exposure));
        }
        if let Some(value) = tint.iter().copied().find(|value| !value.is_finite()) {
            return Err(PostProcessError::NonFiniteTint(value));
        }
        if let Some(value) = tint.iter().copied().find(|value| *value < 0.0) {
            return Err(PostProcessError::NegativeTint(value));
        }
        if !vignette.is_finite() {
            return Err(PostProcessError::NonFiniteVignette(vignette));
        }
        if !(0.0..=1.0).contains(&vignette) {
            return Err(PostProcessError::VignetteOutOfRange(vignette));
        }
        if !exposure_ev.is_finite() {
            return Err(PostProcessError::NonFiniteExposureEv(exposure_ev));
        }
        if !EXPOSURE_EV_RANGE.contains(&exposure_ev) {
            return Err(PostProcessError::ExposureEvOutOfRange(exposure_ev));
        }
        Ok(Self {
            exposure,
            tint,
            vignette,
            tonemap,
            exposure_ev,
        })
    }

    /// 参数是否"什么都不做"。
    ///
    /// 注意：**恒等参数不等于回滚档**——只要开了后处理就要走一次离屏 + 全屏 pass（多一次
    /// 8 位量化往返）。真正的回滚档是 `set_post_process(None)`。
    #[must_use]
    pub fn is_identity(self) -> bool {
        self == Self::default()
    }

    /// HDR 模式的曝光归属校验：`exposure` 必须是 `1.0`（曝光只走 EV）。
    ///
    /// 渲染器在 HDR 帧会调用它；不满足时**告警并强制按 1.0 执行**，而不是把曝光乘两次
    /// 让画面突然变暗——同时留下迁移提示，调用方可以改成 [`Self::new_hdr`]。
    pub fn hdr_compatible(self) -> Result<(), PostProcessError> {
        if self.exposure == 1.0 {
            Ok(())
        } else {
            Err(PostProcessError::HdrExposureNotUnity(self.exposure))
        }
    }

    /// 上传给后处理着色器的 uniform（`[f32; 8]`：
    /// exposure, vignette, exposure_ev, tonemap_code, tint.rgba）。
    ///
    /// WGSL 的 uniform 结构要求 `vec4` 成员 16 字节对齐，因此前四个 f32 正好填满一个 vec4，
    /// 随后是 tint。`tonemap_code` 在 LDR 后处理里不参与像素，但保持字段位置固定，避免
    /// 以后加参数时静默错位。
    #[must_use]
    pub fn uniform(self) -> [f32; 8] {
        [
            self.exposure,
            self.vignette,
            self.exposure_ev,
            self.tonemap.code(),
            self.tint[0],
            self.tint[1],
            self.tint[2],
            self.tint[3],
        ]
    }
}

/// `PostProcess` 参数的校验错误。
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum PostProcessError {
    #[error("exposure must be finite: {0}")]
    NonFiniteExposure(f32),
    #[error("exposure must be >= 0: {0}")]
    NegativeExposure(f32),
    #[error("tint must be finite: {0}")]
    NonFiniteTint(f32),
    #[error("tint must be >= 0: {0}")]
    NegativeTint(f32),
    #[error("vignette must be finite: {0}")]
    NonFiniteVignette(f32),
    #[error("vignette must be in 0.0..=1.0: {0}")]
    VignetteOutOfRange(f32),
    #[error("exposure_ev must be finite: {0}")]
    NonFiniteExposureEv(f32),
    #[error("exposure_ev must be in {min}..={max}: {0}", min = EXPOSURE_EV_RANGE.start(), max = EXPOSURE_EV_RANGE.end())]
    ExposureEvOutOfRange(f32),
    #[error("HDR mode requires PostProcess.exposure == 1.0 (exposure belongs to exposure_ev): {0}")]
    HdrExposureNotUnity(f32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let settings = PostProcess::default();
        assert!(settings.is_identity());
        assert_eq!(
            settings.uniform(),
            [1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
            "恒等参数上传后必须真的是恒等（前两位是 exposure/vignette）"
        );
        assert_eq!(settings.tonemap, Tonemap::Aces);
        assert_eq!(settings.exposure_ev, 0.0);
    }

    /// 非法值必须当场报错：静默夹紧会让"我调了参数但没变化"变成无从查起的问题。
    #[test]
    fn rejects_non_finite_and_out_of_range_values() {
        assert_eq!(
            PostProcess::new(-1.0, [1.0; 4], 0.0),
            Err(PostProcessError::NegativeExposure(-1.0))
        );
        assert!(matches!(
            PostProcess::new(f32::NAN, [1.0; 4], 0.0),
            Err(PostProcessError::NonFiniteExposure(_))
        ));
        assert!(matches!(
            PostProcess::new(1.0, [1.0, f32::INFINITY, 1.0, 1.0], 0.0),
            Err(PostProcessError::NonFiniteTint(_))
        ));
        assert!(matches!(
            PostProcess::new(1.0, [1.0, 1.0, -0.5, 1.0], 0.0),
            Err(PostProcessError::NegativeTint(_))
        ));
        assert_eq!(
            PostProcess::new(1.0, [1.0; 4], 1.5),
            Err(PostProcessError::VignetteOutOfRange(1.5))
        );
        assert!(matches!(
            PostProcess::new(1.0, [1.0; 4], f32::NAN),
            Err(PostProcessError::NonFiniteVignette(_))
        ));
        assert!(matches!(
            PostProcess::new_hdr(Tonemap::Aces, f32::NAN, [1.0; 4], 0.0),
            Err(PostProcessError::NonFiniteExposureEv(_))
        ));
        assert!(matches!(
            PostProcess::new_hdr(Tonemap::Aces, 21.0, [1.0; 4], 0.0),
            Err(PostProcessError::ExposureEvOutOfRange(21.0))
        ));
    }

    #[test]
    fn uniform_carries_tint_after_the_parameter_vec4() {
        let settings = PostProcess::new(0.5, [1.0, 0.25, 0.0, 1.0], 0.75).expect("valid settings");
        assert_eq!(
            settings.uniform(),
            [0.5, 0.75, 0.0, 0.0, 1.0, 0.25, 0.0, 1.0]
        );
        assert!(!settings.is_identity());
    }

    #[test]
    fn hdr_constructor_keeps_exposure_at_unity_and_carries_ev() {
        let settings =
            PostProcess::new_hdr(Tonemap::Reinhard, 1.5, [1.0; 4], 0.0).expect("valid HDR");
        assert_eq!(settings.exposure, 1.0);
        assert_eq!(settings.exposure_ev, 1.5);
        assert_eq!(settings.uniform(), [1.0, 0.0, 1.5, 1.0, 1.0, 1.0, 1.0, 1.0]);
        assert!(settings.hdr_compatible().is_ok());
        assert_eq!(
            PostProcess::new_hdr(Tonemap::Aces, 0.0, [1.0; 4], 0.0)
                .expect("valid")
                .hdr_compatible(),
            Ok(())
        );
    }

    /// 曝光归属：HDR 模式下非 1.0 的 `exposure` 必须被识别出来，而不是乘两次。
    #[test]
    fn hdr_rejects_double_exposure() {
        let settings = PostProcess::new(2.0, [1.0; 4], 0.0).expect("valid");
        assert_eq!(
            settings.hdr_compatible(),
            Err(PostProcessError::HdrExposureNotUnity(2.0))
        );
    }

    /// ACES 与 Reinhard 都必须把 8.0 的线性高光压回 LDR，且保持单调/非负。
    #[test]
    fn tonemap_curves_map_bright_linear_values_into_ldr() {
        for curve in [Tonemap::Aces, Tonemap::Reinhard] {
            let zero = curve.apply_rgb([0.0; 3], 0.0);
            assert_eq!(zero, [0.0; 3], "{curve:?} 在 0 处必须为 0");
            let bright = curve.apply_rgb([8.0; 3], 0.0);
            for channel in bright {
                assert!(
                    (0.0..=1.0).contains(&channel),
                    "{curve:?} 必须把 8.0 压回 LDR：{bright:?}"
                );
            }
            assert!(bright[0] > 0.8, "{curve:?} 不该把 8.0 压得太黑：{bright:?}");
            let mut previous = -1.0;
            for step in 0..=100 {
                let value = step as f32 * 0.1;
                let mapped = curve.apply_rgb([value; 3], 0.0)[0];
                assert!(
                    mapped >= previous,
                    "{curve:?} 在 {value} 处不再单调：{mapped} < {previous}"
                );
                previous = mapped;
            }
        }
        let aces = Tonemap::Aces.apply_rgb([0.5; 3], 0.0)[0];
        assert!((aces - 0.6163).abs() < 0.01, "ACES(0.5) = {aces}");
        let reinhard = Tonemap::Reinhard.apply_rgb([0.5; 3], 0.0)[0];
        assert!(
            (reinhard - 1.0 / 3.0).abs() < 1e-6,
            "Reinhard(0.5) = {reinhard}"
        );
    }

    #[test]
    fn exposure_ev_is_a_linear_stop() {
        let base = Tonemap::Reinhard.apply_rgb([0.25; 3], 0.0)[0];
        let one_stop = Tonemap::Reinhard.apply_rgb([0.25; 3], 1.0)[0];
        let expected = (0.5 / 1.5_f32).clamp(0.0, 1.0);
        assert!((one_stop - expected).abs() < 1e-6, "{base} -> {one_stop}");
        assert!(one_stop > base);
    }
}
