//! docs/54 §4.3：后处理（`RenderPlan` 的最后一条通道）。
//!
//! §4.3 的 `RenderPlan` 以 `PostProcess` 收尾。此前渲染器**没有**这条通道：所有内容直接画进
//! 目标附件，宿主想说"这一帧整体压暗一点 / 叠一层色调 / 加暗角"没有地方可说。
//!
//! 本模块只定义**设置**（哪些参数、怎么校验），通道本身在 `host::renderer` 里执行：
//!
//! - 宿主用 `Renderer::set_post_process(Some(settings))` 打开；**不调用**（`None`）就是回滚档
//!   ——场景直接画进目标附件，不产生离屏纹理、不产生额外 pass，像素与收敛前逐字节相同。
//! - 打开后场景先画进一张离屏颜色纹理，再由一条全屏 pass 采样它写进目标附件；因此
//!   **`PostProcess` 一定会出现在 `RenderPlan` 里**，且永远是最后一条。
//! - 参数校验沿用本仓库的做法：非有限值与越界值**报错**而不是静默夹紧——"我调了参数但画面没
//!   变"是最难查的一类问题（可能是值非法被静默丢掉），宁可当场拒绝。

/// 一帧后处理的参数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PostProcess {
    /// 线性曝光倍数（`1.0` = 不变，`0.0` = 全黑）。必须有限且非负。
    pub exposure: f32,
    /// RGBA 乘法着色（`[1.0; 4]` = 不变）。分量必须有限且非负。
    pub tint: [f32; 4],
    /// 暗角强度（`0.0` = 无暗角，`1.0` = 四角全黑）。必须有限且落在 `0.0..=1.0`。
    pub vignette: f32,
}

impl Default for PostProcess {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            tint: [1.0; 4],
            vignette: 0.0,
        }
    }
}

impl PostProcess {
    /// 校验并构造。
    pub fn new(exposure: f32, tint: [f32; 4], vignette: f32) -> Result<Self, PostProcessError> {
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
        Ok(Self {
            exposure,
            tint,
            vignette,
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

    /// 上传给着色器的 uniform（`[f32; 8]`：exposure, vignette, pad, pad, tint.rgba）。
    ///
    /// WGSL 的 uniform 结构要求 `vec4` 成员 16 字节对齐，因此前四个 f32 用两个填充位补齐。
    #[must_use]
    pub fn uniform(self) -> [f32; 8] {
        [
            self.exposure,
            self.vignette,
            0.0,
            0.0,
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
    }

    #[test]
    fn uniform_carries_tint_after_two_padding_slots() {
        let settings = PostProcess::new(0.5, [1.0, 0.25, 0.0, 1.0], 0.75).expect("valid settings");
        assert_eq!(
            settings.uniform(),
            [0.5, 0.75, 0.0, 0.0, 1.0, 0.25, 0.0, 1.0]
        );
        assert!(!settings.is_identity());
    }
}
