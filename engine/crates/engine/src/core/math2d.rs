//! M3-01 coordinate, camera, and color-space conventions.
//!
//! The engine uses a right-handed 2D world convention: +X points right and +Y
//! points up. Screen pixels use a top-left origin, so +Y points down.

use std::{error::Error, fmt};

use crate::core::transform::Vec2;

/// World-space up direction under the engine's right-handed 2D convention.
pub const WORLD_UP: Vec2 = Vec2::new(0.0, 1.0);
/// World-space right direction under the engine's right-handed 2D convention.
pub const WORLD_RIGHT: Vec2 = Vec2::new(1.0, 0.0);
/// 2D 游戏相机默认中心（世界坐标）。相机未显式设置时，游戏视图以该点为屏幕中心，
/// 设计分辨率包围框以它为中心（编辑器 Scene 视图据此定位游戏取景范围）。
pub const DEFAULT_2D_CAMERA_CENTER: Vec2 = Vec2::new(640.0, 360.0);

/// A positive pixel viewport used by [`Camera2D`] conversions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

impl Viewport {
    /// Creates a viewport, rejecting non-finite or non-positive dimensions.
    pub fn new(width: f32, height: f32) -> Result<Self, CoordinateError> {
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Err(CoordinateError::InvalidViewport { width, height });
        }
        Ok(Self { width, height })
    }
}

/// An axis-aligned world rectangle used to constrain a [`Camera2D`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraBounds {
    pub min: Vec2,
    pub max: Vec2,
}

impl CameraBounds {
    pub fn new(min: Vec2, max: Vec2) -> Result<Self, CoordinateError> {
        validate_vec2("camera bounds minimum", min)?;
        validate_vec2("camera bounds maximum", max)?;
        if min.x > max.x || min.y > max.y {
            return Err(CoordinateError::InvalidBounds { min, max });
        }
        Ok(Self { min, max })
    }
}

/// A 2D camera with center, rotation, zoom, and optional world bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2D {
    pub center: Vec2,
    /// Clockwise screen rotation in radians. Positive world rotation appears clockwise on screen.
    pub rotation: f32,
    /// Pixels per world unit.
    pub zoom: f32,
    pub bounds: Option<CameraBounds>,
}

impl Camera2D {
    /// Creates an unrotated camera, where `zoom` is pixels per world unit.
    pub fn new(center: Vec2, zoom: f32) -> Result<Self, CoordinateError> {
        Self::with_settings(center, 0.0, zoom, None)
    }

    pub fn with_rotation(mut self, rotation: f32) -> Result<Self, CoordinateError> {
        self.rotation = rotation;
        self.validate()?;
        Ok(self)
    }

    pub fn with_bounds(mut self, bounds: CameraBounds) -> Result<Self, CoordinateError> {
        self.bounds = Some(bounds);
        self.validate()?;
        Ok(self)
    }

    pub fn without_bounds(mut self) -> Self {
        self.bounds = None;
        self
    }

    fn with_settings(
        center: Vec2,
        rotation: f32,
        zoom: f32,
        bounds: Option<CameraBounds>,
    ) -> Result<Self, CoordinateError> {
        let camera = Self {
            center,
            rotation,
            zoom,
            bounds,
        };
        camera.validate()?;
        Ok(camera)
    }

    /// Converts a world position to top-left-origin screen pixels.
    pub fn world_to_screen(self, world: Vec2, viewport: Viewport) -> Result<Vec2, CoordinateError> {
        self.validate()?;
        validate_vec2("world", world)?;
        validate_viewport(viewport)?;
        let center = self.constrained_center(viewport)?;
        let delta = Vec2::new(world.x - center.x, world.y - center.y);
        let (sin, cos) = self.rotation.sin_cos();
        let view = Vec2::new(
            delta.x * cos + delta.y * sin,
            -delta.x * sin + delta.y * cos,
        );
        Ok(Vec2::new(
            view.x * self.zoom + viewport.width * 0.5,
            viewport.height * 0.5 - view.y * self.zoom,
        ))
    }

    /// Converts a top-left-origin screen pixel position back to world space.
    pub fn screen_to_world(
        self,
        screen: Vec2,
        viewport: Viewport,
    ) -> Result<Vec2, CoordinateError> {
        self.validate()?;
        validate_vec2("screen", screen)?;
        validate_viewport(viewport)?;
        let center = self.constrained_center(viewport)?;
        let view = Vec2::new(
            (screen.x - viewport.width * 0.5) / self.zoom,
            (viewport.height * 0.5 - screen.y) / self.zoom,
        );
        let (sin, cos) = self.rotation.sin_cos();
        Ok(Vec2::new(
            center.x + view.x * cos - view.y * sin,
            center.y + view.x * sin + view.y * cos,
        ))
    }

    fn constrained_center(self, viewport: Viewport) -> Result<Vec2, CoordinateError> {
        let Some(bounds) = self.bounds else {
            return Ok(self.center);
        };
        let half_width = viewport.width * 0.5 / self.zoom;
        let half_height = viewport.height * 0.5 / self.zoom;
        let (sin, cos) = self.rotation.sin_cos();
        let extent_x = half_width * cos.abs() + half_height * sin.abs();
        let extent_y = half_width * sin.abs() + half_height * cos.abs();
        Ok(Vec2::new(
            clamp_center(
                self.center.x,
                bounds.min.x + extent_x,
                bounds.max.x - extent_x,
            ),
            clamp_center(
                self.center.y,
                bounds.min.y + extent_y,
                bounds.max.y - extent_y,
            ),
        ))
    }

    fn validate(self) -> Result<(), CoordinateError> {
        validate_vec2("camera center", self.center)?;
        if !self.rotation.is_finite() {
            return Err(CoordinateError::InvalidRotation(self.rotation));
        }
        if !self.zoom.is_finite() || self.zoom <= 0.0 {
            return Err(CoordinateError::InvalidZoom(self.zoom));
        }
        if let Some(bounds) = self.bounds {
            CameraBounds::new(bounds.min, bounds.max)?;
        }
        Ok(())
    }
}

fn clamp_center(value: f32, minimum: f32, maximum: f32) -> f32 {
    if minimum > maximum {
        (minimum + maximum) * 0.5
    } else {
        value.clamp(minimum, maximum)
    }
}

fn validate_viewport(viewport: Viewport) -> Result<(), CoordinateError> {
    Viewport::new(viewport.width, viewport.height).map(|_| ())
}

fn validate_vec2(name: &'static str, value: Vec2) -> Result<(), CoordinateError> {
    if value.x.is_finite() && value.y.is_finite() {
        Ok(())
    } else {
        Err(CoordinateError::NonFiniteVector { name, value })
    }
}

/// Errors produced by coordinate and camera conversions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoordinateError {
    InvalidViewport { width: f32, height: f32 },
    InvalidZoom(f32),
    NonFiniteVector { name: &'static str, value: Vec2 },
    NonFiniteColor { value: [f32; 4] },
    InvalidRotation(f32),
    InvalidBounds { min: Vec2, max: Vec2 },
}

impl fmt::Display for CoordinateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidViewport { width, height } => {
                write!(formatter, "invalid viewport: {width}x{height}")
            }
            Self::InvalidZoom(zoom) => write!(formatter, "invalid camera zoom: {zoom}"),
            Self::NonFiniteVector { name, value } => write!(
                formatter,
                "{name} vector is not finite: ({}, {})",
                value.x, value.y
            ),
            Self::NonFiniteColor { value } => write!(formatter, "color is not finite: {value:?}"),
            Self::InvalidRotation(rotation) => {
                write!(formatter, "invalid camera rotation: {rotation}")
            }
            Self::InvalidBounds { min, max } => {
                write!(formatter, "invalid camera bounds: {min:?}..{max:?}")
            }
        }
    }
}
impl Error for CoordinateError {}

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
    use super::{
        Camera2D, CameraBounds, CoordinateError, LinearColor, SrgbColor, Viewport, WORLD_RIGHT,
        WORLD_UP,
    };
    use crate::core::transform::Vec2;
    fn assert_vec_close(actual: Vec2, expected: Vec2, epsilon: f32) {
        assert!(
            (actual.x - expected.x).abs() <= epsilon,
            "{actual:?} != {expected:?}"
        );
        assert!(
            (actual.y - expected.y).abs() <= epsilon,
            "{actual:?} != {expected:?}"
        );
    }
    #[test]
    fn coordinate_axes_and_center_match_convention() {
        let viewport = Viewport::new(800.0, 600.0).unwrap();
        let camera = Camera2D::new(Vec2::zero(), 10.0).unwrap();
        assert_eq!(
            camera.world_to_screen(Vec2::zero(), viewport).unwrap(),
            Vec2::new(400.0, 300.0)
        );
        assert_eq!(
            camera.world_to_screen(WORLD_RIGHT, viewport).unwrap(),
            Vec2::new(410.0, 300.0)
        );
        assert_eq!(
            camera.world_to_screen(WORLD_UP, viewport).unwrap(),
            Vec2::new(400.0, 290.0)
        );
    }
    #[test]
    fn coordinate_conversion_round_trips_at_multiple_viewports_and_zooms() {
        for (width, height, zoom) in [
            (800.0, 600.0, 1.0),
            (1920.0, 1080.0, 100.0),
            (320.0, 240.0, 0.25),
        ] {
            let viewport = Viewport::new(width, height).unwrap();
            let camera = Camera2D::new(Vec2::new(3.5, -2.0), zoom).unwrap();
            let world = Vec2::new(-12.25, 8.75);
            let screen = camera.world_to_screen(world, viewport).unwrap();
            assert_vec_close(
                camera.screen_to_world(screen, viewport).unwrap(),
                world,
                0.0001,
            );
        }
    }
    #[test]
    fn invalid_coordinate_inputs_are_rejected() {
        assert!(matches!(
            Viewport::new(0.0, 1.0),
            Err(CoordinateError::InvalidViewport { .. })
        ));
        assert!(matches!(
            Camera2D::new(Vec2::zero(), 0.0),
            Err(CoordinateError::InvalidZoom(_))
        ));
        assert!(matches!(
            Camera2D::new(Vec2::new(f32::NAN, 0.0), 1.0),
            Err(CoordinateError::NonFiniteVector { .. })
        ));
    }

    #[test]
    fn camera_rotation_round_trips_and_rotates_axes() {
        let viewport = Viewport::new(200.0, 100.0).unwrap();
        let camera = Camera2D::new(Vec2::zero(), 10.0)
            .unwrap()
            .with_rotation(std::f32::consts::FRAC_PI_2)
            .unwrap();
        assert_vec_close(
            camera.world_to_screen(WORLD_RIGHT, viewport).unwrap(),
            Vec2::new(100.0, 60.0),
            0.0001,
        );
        let world = Vec2::new(2.5, -3.0);
        assert_vec_close(
            camera
                .screen_to_world(camera.world_to_screen(world, viewport).unwrap(), viewport)
                .unwrap(),
            world,
            0.0001,
        );
    }

    #[test]
    fn camera_bounds_keep_viewport_inside_world_rectangle() {
        let viewport = Viewport::new(100.0, 100.0).unwrap();
        let camera = Camera2D::new(Vec2::new(-100.0, 100.0), 10.0)
            .unwrap()
            .with_bounds(CameraBounds::new(Vec2::new(-5.0, -5.0), Vec2::new(5.0, 5.0)).unwrap())
            .unwrap();
        assert_eq!(
            camera.world_to_screen(Vec2::zero(), viewport).unwrap(),
            Vec2::new(50.0, 50.0)
        );
        assert_eq!(
            camera
                .screen_to_world(Vec2::new(0.0, 0.0), viewport)
                .unwrap(),
            Vec2::new(-5.0, 5.0)
        );
    }

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
