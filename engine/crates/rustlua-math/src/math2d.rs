//! 2D 向量、坐标轴常量与颜色空间约定。
//!
//! `Vec2` 是纯数学原语，定义在本模块；ECS 的 `transform` 与相机的 `CoordinateError`
//! 均从这里导入并重新导出，因此既有 `transform::Vec2` / `camera::CoordinateError` 的
//! 引用无需改动。

/// A two-dimensional vector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    /// Creates a vector from its two coordinates.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Returns the zero vector.
    #[must_use]
    pub const fn zero() -> Self {
        Self::new(0.0, 0.0)
    }

    /// Length (magnitude).
    #[must_use]
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    /// Dot product.
    #[must_use]
    pub const fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
    }

    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl std::ops::Neg for Vec2 {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self::new(-self.x, -self.y)
    }
}

/// Errors produced by coordinate and camera conversions.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum CoordinateError {
    #[error("invalid viewport: {width}x{height}")]
    InvalidViewport { width: f32, height: f32 },
    #[error("invalid camera zoom: {0}")]
    InvalidZoom(f32),
    #[error("{name} vector is not finite: ({}, {})", .value.x, .value.y)]
    NonFiniteVector { name: &'static str, value: Vec2 },
    #[error("color is not finite: {value:?}")]
    NonFiniteColor { value: [f32; 4] },
    #[error("invalid camera rotation: {0}")]
    InvalidRotation(f32),
    #[error("invalid camera bounds: {min:?}..{max:?}")]
    InvalidBounds { min: Vec2, max: Vec2 },
    /// 深度范围非法（`far <= near` 或含非有限值）。
    #[error("invalid depth range: near={near} far={far}")]
    InvalidDepthRange { near: f32, far: f32 },
}

/// World-space up direction under the engine's right-handed 2D convention.
pub const WORLD_UP: Vec2 = Vec2::new(0.0, 1.0);
/// World-space right direction under the engine's right-handed 2D convention.
pub const WORLD_RIGHT: Vec2 = Vec2::new(1.0, 0.0);

/// A color whose RGB channels are in linear light. Alpha is always linear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl LinearColor {
    /// Creates a linear color, rejecting NaN and infinity.
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Result<Self, CoordinateError> {
        let color = Self { r, g, b, a };
        color.validate()?;
        Ok(color)
    }
    /// Converts RGB to sRGB. Values outside `[0, 1]` are clamped.
    #[must_use]
    pub fn to_srgb(self) -> SrgbColor {
        SrgbColor {
            r: linear_to_srgb(self.r),
            g: linear_to_srgb(self.g),
            b: linear_to_srgb(self.b),
            a: self.a.clamp(0.0, 1.0),
        }
    }
    fn validate(self) -> Result<(), CoordinateError> {
        validate_color([self.r, self.g, self.b, self.a])
    }
}

/// A color encoded in the standard sRGB transfer curve. Alpha is linear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SrgbColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl SrgbColor {
    /// Creates an sRGB color, rejecting NaN and infinity.
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Result<Self, CoordinateError> {
        let color = Self { r, g, b, a };
        color.validate()?;
        Ok(color)
    }
    /// Converts RGB to linear light. Values outside `[0, 1]` are clamped.
    #[must_use]
    pub fn to_linear(self) -> LinearColor {
        LinearColor {
            r: srgb_to_linear(self.r),
            g: srgb_to_linear(self.g),
            b: srgb_to_linear(self.b),
            a: self.a.clamp(0.0, 1.0),
        }
    }
    fn validate(self) -> Result<(), CoordinateError> {
        validate_color([self.r, self.g, self.b, self.a])
    }
}

fn validate_color(value: [f32; 4]) -> Result<(), CoordinateError> {
    if value.iter().all(|channel| channel.is_finite()) {
        Ok(())
    } else {
        Err(CoordinateError::NonFiniteColor { value })
    }
}
fn srgb_to_linear(channel: f32) -> f32 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}
fn linear_to_srgb(channel: f32) -> f32 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}
#[cfg(test)]
mod tests {
    use super::{LinearColor, SrgbColor};

    #[test]
    fn srgb_reference_values_are_correct() {
        let linear = SrgbColor::new(0.0, 0.5, 1.0, 0.25).unwrap().to_linear();
        assert!((linear.g - 0.21404114).abs() < 0.0000001);
        assert_eq!(linear.r, 0.0);
        assert!((linear.b - 1.0).abs() < 0.0000001);
        assert_eq!(linear.a, 0.25);
        let srgb = LinearColor::new(0.0, 0.21404114, 1.0, 0.25)
            .unwrap()
            .to_srgb();
        assert!((srgb.g - 0.5).abs() < 0.0000001);
        assert_eq!(srgb.r, 0.0);
        assert!((srgb.b - 1.0).abs() < 0.0000001);
        assert_eq!(srgb.a, 0.25);
    }
    #[test]
    fn color_round_trip_preserves_channels_and_alpha_is_not_gamma_converted() {
        let srgb = SrgbColor::new(0.1, 0.4, 0.9, 0.37).unwrap();
        let round_trip = srgb.to_linear().to_srgb();
        assert!((round_trip.r - srgb.r).abs() < 0.00001);
        assert!((round_trip.g - srgb.g).abs() < 0.00001);
        assert!((round_trip.b - srgb.b).abs() < 0.00001);
        assert_eq!(round_trip.a, srgb.a);
        assert!(SrgbColor::new(f32::NAN, 0.0, 0.0, 1.0).is_err());
    }
    #[test]
    fn out_of_range_color_channels_are_clamped_during_conversion() {
        let color = SrgbColor::new(-1.0, 2.0, 0.5, 3.0).unwrap().to_linear();
        assert_eq!(color.r, 0.0);
        assert_eq!(color.g, 1.0);
        assert!((color.b - 0.21404114).abs() < 0.0000001);
        assert_eq!(color.a, 1.0);
    }
}
