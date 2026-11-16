//! 程序化贴图生成（M82-B2）：spec 驱动、seed 确定、跨平台逐字节一致。
//!
//! 四种 kind：`checker`/`noise`/`gradient`/`stripes`。CPU 侧生成 RGBA8，产出
//! [`TextureAsset`]，接现有 `Renderer::upload_texture_rgba8`（2D 贴图缓存按 key 取样）。
//! 与 `terrain_detail.rs` 同一纪律：噪声格点用整数哈希，逐像素只用 `+ - * /` 与
//! `floor`/`round`（IEEE-754 基本运算跨平台正确舍入），**不碰** `sin`/`powf` 等
//! 不保证逐位一致的超越函数——于是同 spec 在任何平台都生成同一串字节。

use crate::texture::{TextureAsset, TextureColorSpace, TextureError};

/// 方形贴图边长下限（含）。
pub const TEXTURE_MIN_SIZE: u32 = 4;
/// 方形贴图边长上限（含）：1024² × 4 = 4 MiB，单张程序化贴图的显存上限。
pub const TEXTURE_MAX_SIZE: u32 = 1024;
/// 频率（格数/条数）下限。
pub const TEXTURE_MIN_SCALE: f32 = 1.0;
/// 频率上限。
pub const TEXTURE_MAX_SCALE: f32 = 256.0;
/// 调色板颜色数下限（四种 kind 都至少需要两色）。
pub const TEXTURE_MIN_COLORS: usize = 2;
/// 调色板颜色数上限。
pub const TEXTURE_MAX_COLORS: usize = 8;

/// 程序化贴图种类。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureKind {
    /// 棋盘格：相邻格交替取调色板色。
    Checker,
    /// 值噪声：seed 确定的周期格点噪声，按调色板插值。
    Noise,
    /// 水平渐变：沿 x 在调色板上插值。
    Gradient,
    /// 竖条纹：沿 x 按条取调色板色。
    Stripes,
}

impl TextureKind {
    /// 规格串 token → 种类；未知 token 返回 `None`（由绑定层报错）。
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "checker" => Some(Self::Checker),
            "noise" => Some(Self::Noise),
            "gradient" => Some(Self::Gradient),
            "stripes" => Some(Self::Stripes),
            _ => None,
        }
    }

    /// 种类 → 规范 token。
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Checker => "checker",
            Self::Noise => "noise",
            Self::Gradient => "gradient",
            Self::Stripes => "stripes",
        }
    }
}

/// 一张程序化贴图的完整规格。字段由 Lua spec 表填入（绑定层负责表→结构）。
#[derive(Clone, Debug, PartialEq)]
pub struct TextureSpec {
    /// 种类。
    pub kind: TextureKind,
    /// 方形边长（texel）。
    pub size: u32,
    /// 噪声种子（仅 `noise` 敏感；其余 kind 与 seed 无关）。
    pub seed: u32,
    /// 频率：checker/noise 的格数、stripes 的条数（gradient 忽略）。
    pub scale: f32,
    /// RGBA 调色板（2..=8 色）。checker/stripes 按序取色，gradient/noise 按序插值。
    pub colors: Vec<[u8; 4]>,
}

impl TextureSpec {
    /// 给定 kind 的缺省规格：64²、seed 0、scale 8、黑白两色。
    #[must_use]
    pub fn default_for(kind: TextureKind) -> Self {
        Self {
            kind,
            size: 64,
            seed: 0,
            scale: 8.0,
            colors: vec![[0, 0, 0, 255], [255, 255, 255, 255]],
        }
    }

    /// 范围校验：尺寸/频率有限且在界内、调色板颜色数合规。
    pub fn validate(&self) -> Result<(), TextureGenError> {
        if !(TEXTURE_MIN_SIZE..=TEXTURE_MAX_SIZE).contains(&self.size) {
            return Err(TextureGenError::SizeOutOfRange { size: self.size });
        }
        if !self.scale.is_finite() || !(TEXTURE_MIN_SCALE..=TEXTURE_MAX_SCALE).contains(&self.scale)
        {
            return Err(TextureGenError::ScaleOutOfRange { scale: self.scale });
        }
        if self.colors.len() < TEXTURE_MIN_COLORS {
            return Err(TextureGenError::TooFewColors {
                count: self.colors.len(),
            });
        }
        if self.colors.len() > TEXTURE_MAX_COLORS {
            return Err(TextureGenError::TooManyColors {
                count: self.colors.len(),
            });
        }
        Ok(())
    }

    /// 生成 RGBA8 贴图。先校验，再逐像素确定性填色。
    pub fn generate(&self) -> Result<TextureAsset, TextureGenError> {
        self.validate()?;
        let size = self.size;
        let mut rgba = vec![0u8; (size as usize) * (size as usize) * 4];
        let cells = self.scale;
        for y in 0..size {
            // 采样在 texel 中心，周期域对称。
            let v = (y as f32 + 0.5) / size as f32;
            for x in 0..size {
                let u = (x as f32 + 0.5) / size as f32;
                let color = match self.kind {
                    TextureKind::Checker => {
                        let cx = (u * cells).floor() as i64;
                        let cy = (v * cells).floor() as i64;
                        let index = (cx + cy).rem_euclid(self.colors.len() as i64) as usize;
                        self.colors[index]
                    }
                    TextureKind::Stripes => {
                        let cx = (u * cells).floor() as i64;
                        let index = cx.rem_euclid(self.colors.len() as i64) as usize;
                        self.colors[index]
                    }
                    TextureKind::Gradient => {
                        let t = x as f32 / (size - 1) as f32;
                        sample_palette(&self.colors, t)
                    }
                    TextureKind::Noise => {
                        let t = value_noise(u, v, cells as u32, self.seed);
                        sample_palette(&self.colors, t)
                    }
                };
                let index = ((y * size + x) * 4) as usize;
                rgba[index..index + 4].copy_from_slice(&color);
            }
        }
        Ok(TextureAsset::from_rgba8(
            size,
            size,
            rgba,
            TextureColorSpace::Srgb,
        )?)
    }
}

/// 在调色板上按 `t∈[0,1]` 线性插值（gradient/noise 共用）。
fn sample_palette(colors: &[[u8; 4]], t: f32) -> [u8; 4] {
    let n = colors.len();
    if n == 1 {
        return colors[0];
    }
    let scaled = t.clamp(0.0, 1.0) * (n - 1) as f32;
    let seg = (scaled.floor() as usize).min(n - 2);
    let frac = scaled - seg as f32;
    lerp_color(colors[seg], colors[seg + 1], frac)
}

/// 两 RGBA 色按 `t` 线性插值，逐通道四舍五入到 u8。
fn lerp_color(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let mut out = [0u8; 4];
    for channel in 0..4 {
        let av = f32::from(a[channel]);
        let bv = f32::from(b[channel]);
        out[channel] = (av + (bv - av) * t).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// 整数格点哈希 → `[0,1]`（与 `terrain_detail.rs` 同族，纯整数运算）。
fn hash(ix: u32, iy: u32, seed: u32) -> f32 {
    let mut h = ix
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(iy.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(seed.wrapping_mul(0xC2B2_AE35));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0x00FF_FFFF) as f32 / 16_777_215.0
}

/// 周期值噪声：`cells×cells` 格点，`u,v∈[0,1)`，格点索引回绕 ⇒ 无缝平铺。
fn value_noise(u: f32, v: f32, cells: u32, seed: u32) -> f32 {
    let cells = cells.max(1);
    let x = u * cells as f32;
    let y = v * cells as f32;
    let x0 = x.floor();
    let y0 = y.floor();
    let tx = smooth(x - x0);
    let ty = smooth(y - y0);
    let ix0 = (x0 as i64).rem_euclid(i64::from(cells)) as u32;
    let iy0 = (y0 as i64).rem_euclid(i64::from(cells)) as u32;
    let ix1 = (ix0 + 1) % cells;
    let iy1 = (iy0 + 1) % cells;
    let a = hash(ix0, iy0, seed);
    let b = hash(ix1, iy0, seed);
    let c = hash(ix0, iy1, seed);
    let d = hash(ix1, iy1, seed);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    (top + (bottom - top) * ty).clamp(0.0, 1.0)
}

/// Hermite 平滑 `3t²-2t³`（只用乘法/减法，跨平台确定）。
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// 程序化贴图生成错误。文本字段统一 `reason`/具名数值，避免 thiserror 把 `source` 当错误源。
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum TextureGenError {
    #[error("texture size {size} out of range {TEXTURE_MIN_SIZE}..={TEXTURE_MAX_SIZE}")]
    SizeOutOfRange { size: u32 },
    #[error("texture scale {scale} out of range {TEXTURE_MIN_SCALE}..={TEXTURE_MAX_SCALE}")]
    ScaleOutOfRange { scale: f32 },
    #[error("texture palette needs at least {TEXTURE_MIN_COLORS} colors, got {count}")]
    TooFewColors { count: usize },
    #[error("texture palette supports at most {TEXTURE_MAX_COLORS} colors, got {count}")]
    TooManyColors { count: usize },
    #[error("texture asset error: {0}")]
    Asset(#[from] TextureError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: TextureKind) -> TextureSpec {
        TextureSpec::default_for(kind)
    }

    #[test]
    fn every_kind_generates_a_square_opaque_texture() {
        for kind in [
            TextureKind::Checker,
            TextureKind::Noise,
            TextureKind::Gradient,
            TextureKind::Stripes,
        ] {
            let asset = spec(kind).generate().expect("generate");
            assert_eq!(asset.width(), 64);
            assert_eq!(asset.height(), 64);
            assert_eq!(asset.pixels().len(), 64 * 64 * 4);
            assert!(
                asset.pixels().iter().skip(3).step_by(4).all(|a| *a == 255),
                "{} 默认调色板 alpha 必须不透明",
                kind.token()
            );
        }
    }

    #[test]
    fn generation_is_deterministic() {
        for kind in [
            TextureKind::Checker,
            TextureKind::Noise,
            TextureKind::Gradient,
            TextureKind::Stripes,
        ] {
            let s = spec(kind);
            assert_eq!(s.generate().unwrap(), s.generate().unwrap());
        }
    }

    #[test]
    fn noise_depends_on_seed_but_checker_does_not() {
        let mut a = spec(TextureKind::Noise);
        a.seed = 1;
        let mut b = spec(TextureKind::Noise);
        b.seed = 2;
        assert_ne!(
            a.generate().unwrap().pixels(),
            b.generate().unwrap().pixels()
        );

        let mut c = spec(TextureKind::Checker);
        c.seed = 1;
        let mut d = spec(TextureKind::Checker);
        d.seed = 999;
        assert_eq!(c.generate().unwrap(), d.generate().unwrap());
    }

    #[test]
    fn checker_alternates_two_colors() {
        let mut s = spec(TextureKind::Checker);
        s.size = 4;
        s.scale = 2.0; // 2×2 格，每格 2 texel
        s.colors = vec![[10, 20, 30, 255], [200, 210, 220, 255]];
        let asset = s.generate().unwrap();
        let px = |x: u32, y: u32| -> [u8; 4] {
            let i = ((y * 4 + x) * 4) as usize;
            [
                asset.pixels()[i],
                asset.pixels()[i + 1],
                asset.pixels()[i + 2],
                asset.pixels()[i + 3],
            ]
        };
        // 左上格 (0,0) 与右下格 (1,1) 同色；右上 (1,0) 与左下 (0,1) 同另一色。
        assert_eq!(px(0, 0), [10, 20, 30, 255]);
        assert_eq!(px(3, 3), [10, 20, 30, 255]);
        assert_eq!(px(3, 0), [200, 210, 220, 255]);
        assert_eq!(px(0, 3), [200, 210, 220, 255]);
    }

    #[test]
    fn stripes_run_along_x() {
        let mut s = spec(TextureKind::Stripes);
        s.size = 4;
        s.scale = 2.0; // 2 条，每条 2 texel
        s.colors = vec![[1, 2, 3, 255], [4, 5, 6, 255]];
        let asset = s.generate().unwrap();
        let px = |x: u32, y: u32| -> [u8; 4] {
            let i = ((y * 4 + x) * 4) as usize;
            [
                asset.pixels()[i],
                asset.pixels()[i + 1],
                asset.pixels()[i + 2],
                asset.pixels()[i + 3],
            ]
        };
        // 同一列上下同色（条纹沿 x，不随 y 变）。
        assert_eq!(px(0, 0), px(0, 3));
        assert_eq!(px(0, 0), [1, 2, 3, 255]);
        assert_eq!(px(3, 0), [4, 5, 6, 255]);
    }

    #[test]
    fn gradient_hits_palette_endpoints() {
        let mut s = spec(TextureKind::Gradient);
        s.size = 8;
        s.colors = vec![[0, 0, 0, 255], [255, 255, 255, 255]];
        let asset = s.generate().unwrap();
        let row0 = &asset.pixels()[0..4];
        assert_eq!(row0, [0, 0, 0, 255]);
        let last = ((8 - 1) * 4) as usize;
        assert_eq!(&asset.pixels()[last..last + 4], [255, 255, 255, 255]);
        // 中点应介于两端之间（单调递增）。
        let mid = (3 * 4) as usize;
        assert!(asset.pixels()[mid] > 0 && asset.pixels()[mid] < 255);
    }

    #[test]
    fn size_bounds_rejected() {
        let mut s = spec(TextureKind::Checker);
        s.size = TEXTURE_MIN_SIZE - 1;
        assert!(matches!(
            s.generate(),
            Err(TextureGenError::SizeOutOfRange { .. })
        ));
        s.size = TEXTURE_MAX_SIZE + 1;
        assert!(matches!(
            s.generate(),
            Err(TextureGenError::SizeOutOfRange { .. })
        ));
        s.size = TEXTURE_MIN_SIZE;
        assert!(s.generate().is_ok());
        s.size = TEXTURE_MAX_SIZE;
        assert!(s.generate().is_ok());
    }

    #[test]
    fn scale_bounds_rejected() {
        let mut s = spec(TextureKind::Checker);
        s.scale = 0.5;
        assert!(matches!(
            s.generate(),
            Err(TextureGenError::ScaleOutOfRange { .. })
        ));
        s.scale = 257.0;
        assert!(matches!(
            s.generate(),
            Err(TextureGenError::ScaleOutOfRange { .. })
        ));
        s.scale = f32::NAN;
        assert!(matches!(
            s.generate(),
            Err(TextureGenError::ScaleOutOfRange { .. })
        ));
        s.scale = 8.0;
        assert!(s.generate().is_ok());
    }

    #[test]
    fn palette_count_bounds_rejected() {
        let mut s = spec(TextureKind::Checker);
        s.colors = vec![[0, 0, 0, 255]];
        assert!(matches!(
            s.generate(),
            Err(TextureGenError::TooFewColors { count: 1 })
        ));
        s.colors = vec![[0, 0, 0, 255]; TEXTURE_MAX_COLORS + 1];
        assert!(matches!(
            s.generate(),
            Err(TextureGenError::TooManyColors { .. })
        ));
        s.colors = vec![[0, 0, 0, 255]; TEXTURE_MAX_COLORS];
        assert!(s.generate().is_ok());
    }

    #[test]
    fn kind_tokens_round_trip() {
        for kind in [
            TextureKind::Checker,
            TextureKind::Noise,
            TextureKind::Gradient,
            TextureKind::Stripes,
        ] {
            assert_eq!(TextureKind::from_token(kind.token()), Some(kind));
        }
        assert_eq!(TextureKind::from_token("bogus"), None);
    }

    #[test]
    fn noise_is_seamless_across_tile_edge() {
        // 周期噪声：u=0 与 u→1 的格点回绕，左右边缘像素应接近（无缝平铺）。
        let mut s = spec(TextureKind::Noise);
        s.size = 16;
        s.scale = 4.0;
        s.seed = 7;
        s.colors = vec![[0, 0, 0, 255], [255, 255, 255, 255]];
        let asset = s.generate().unwrap();
        let lum = |x: u32, y: u32| -> u8 {
            let i = ((y * 16 + x) * 4) as usize;
            asset.pixels()[i]
        };
        // 同一行最左与最右texel 的亮度差应小（周期域对称采样）。
        let left = lum(0, 8);
        let right = lum(15, 8);
        assert!(
            (i16::from(left) - i16::from(right)).abs() < 40,
            "noise 左右边缘应接近无缝：left={left} right={right}"
        );
    }
}
