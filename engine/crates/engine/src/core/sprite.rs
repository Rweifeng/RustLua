//! CPU-side Sprite geometry generation for the 2D renderer.

use std::{error::Error, fmt};

use crate::core::{
    math2d::{Camera2D, CoordinateError, Viewport},
    transform::{Transform, Vec2},
};

/// A textured, tinted quad description. `pivot` and UVs are normalized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    pub size: Vec2,
    pub pivot: Vec2,
    pub uv_min: Vec2,
    pub uv_max: Vec2,
    pub tint: [f32; 4],
}

/// One CPU-side vertex in screen-pixel space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// The asset path of the texture a sprite renders with, from the scene's
/// `Sprite` component value. The host renderer resolves it against the project
/// root and samples it with the sprite's UVs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpriteAsset(pub String);

impl SpriteAsset {
    /// Returns the asset path this sprite samples.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.0
    }
}

/// The two triangles making up a Sprite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteQuad {
    pub vertices: [SpriteVertex; 6],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpriteError {
    InvalidSize(Vec2),
    InvalidPivot(Vec2),
    InvalidUv { min: Vec2, max: Vec2 },
    NonFiniteTint([f32; 4]),
    Coordinate(CoordinateError),
}

impl fmt::Display for SpriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize(v) => write!(f, "invalid sprite size: ({}, {})", v.x, v.y),
            Self::InvalidPivot(v) => write!(f, "invalid sprite pivot: ({}, {})", v.x, v.y),
            Self::InvalidUv { min, max } => {
                write!(f, "invalid sprite UV: min={min:?}, max={max:?}")
            }
            Self::NonFiniteTint(c) => write!(f, "sprite tint is not finite: {c:?}"),
            Self::Coordinate(error) => error.fmt(f),
        }
    }
}
impl Error for SpriteError {}
impl From<CoordinateError> for SpriteError {
    fn from(value: CoordinateError) -> Self {
        Self::Coordinate(value)
    }
}

impl Sprite {
    /// Creates a sprite centered on its transform position, covering the full texture.
    pub fn new(size: Vec2) -> Result<Self, SpriteError> {
        let sprite = Self {
            size,
            pivot: Vec2::new(0.5, 0.5),
            uv_min: Vec2::zero(),
            uv_max: Vec2::new(1.0, 1.0),
            tint: [1.0; 4],
        };
        sprite.validate()?;
        Ok(sprite)
    }

    pub fn with_uv(mut self, uv_min: Vec2, uv_max: Vec2) -> Result<Self, SpriteError> {
        self.uv_min = uv_min;
        self.uv_max = uv_max;
        self.validate()?;
        Ok(self)
    }

    pub fn with_pivot(mut self, pivot: Vec2) -> Result<Self, SpriteError> {
        self.pivot = pivot;
        self.validate()?;
        Ok(self)
    }

    pub fn with_tint(mut self, tint: [f32; 4]) -> Result<Self, SpriteError> {
        self.tint = tint;
        self.validate()?;
        Ok(self)
    }

    /// Generates screen-space vertices using the M3-01 top-left screen convention.
    pub fn quad(
        &self,
        transform: Transform,
        camera: Camera2D,
        viewport: Viewport,
    ) -> Result<SpriteQuad, SpriteError> {
        self.validate()?;
        if !transform.position.x.is_finite()
            || !transform.position.y.is_finite()
            || !transform.rotation.is_finite()
            || !transform.scale.x.is_finite()
            || !transform.scale.y.is_finite()
        {
            return Err(SpriteError::Coordinate(CoordinateError::NonFiniteVector {
                name: "transform",
                value: transform.position,
            }));
        }
        let cos = transform.rotation.cos();
        let sin = transform.rotation.sin();
        let local = [
            Vec2::new(
                -self.pivot.x * self.size.x,
                (1.0 - self.pivot.y) * self.size.y,
            ),
            Vec2::new(
                (1.0 - self.pivot.x) * self.size.x,
                (1.0 - self.pivot.y) * self.size.y,
            ),
            Vec2::new(
                (1.0 - self.pivot.x) * self.size.x,
                -self.pivot.y * self.size.y,
            ),
            Vec2::new(-self.pivot.x * self.size.x, -self.pivot.y * self.size.y),
        ];
        let mut positions = [[0.0; 2]; 4];
        for (index, point) in local.into_iter().enumerate() {
            let scaled = Vec2::new(point.x * transform.scale.x, point.y * transform.scale.y);
            let world = Vec2::new(
                transform.position.x + scaled.x * cos - scaled.y * sin,
                transform.position.y + scaled.x * sin + scaled.y * cos,
            );
            let screen = camera.world_to_screen(world, viewport)?;
            positions[index] = [screen.x, screen.y];
        }
        let uv = [
            [self.uv_min.x, self.uv_min.y],
            [self.uv_max.x, self.uv_min.y],
            [self.uv_max.x, self.uv_max.y],
            [self.uv_min.x, self.uv_max.y],
        ];
        let vertex = |index: usize| SpriteVertex {
            position: positions[index],
            uv: uv[index],
            color: self.tint,
        };
        Ok(SpriteQuad {
            vertices: [
                vertex(0),
                vertex(1),
                vertex(2),
                vertex(0),
                vertex(2),
                vertex(3),
            ],
        })
    }

    fn validate(&self) -> Result<(), SpriteError> {
        if !self.size.x.is_finite()
            || !self.size.y.is_finite()
            || self.size.x <= 0.0
            || self.size.y <= 0.0
        {
            return Err(SpriteError::InvalidSize(self.size));
        }
        if !self.pivot.x.is_finite()
            || !self.pivot.y.is_finite()
            || self.pivot.x < 0.0
            || self.pivot.x > 1.0
            || self.pivot.y < 0.0
            || self.pivot.y > 1.0
        {
            return Err(SpriteError::InvalidPivot(self.pivot));
        }
        if !self.uv_min.x.is_finite()
            || !self.uv_min.y.is_finite()
            || !self.uv_max.x.is_finite()
            || !self.uv_max.y.is_finite()
            || self.uv_min.x < 0.0
            || self.uv_min.y < 0.0
            || self.uv_max.x > 1.0
            || self.uv_max.y > 1.0
            || self.uv_min.x >= self.uv_max.x
            || self.uv_min.y >= self.uv_max.y
        {
            return Err(SpriteError::InvalidUv {
                min: self.uv_min,
                max: self.uv_max,
            });
        }
        if self.tint.iter().any(|channel| !channel.is_finite()) {
            return Err(SpriteError::NonFiniteTint(self.tint));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn camera() -> (Camera2D, Viewport) {
        (
            Camera2D::new(Vec2::zero(), 10.0).unwrap(),
            Viewport::new(100.0, 100.0).unwrap(),
        )
    }
    fn near(a: f32, b: f32) {
        assert!((a - b).abs() < 0.0001, "{a} != {b}");
    }

    #[test]
    fn defaults_generate_full_quad() {
        let (camera, viewport) = camera();
        let quad = Sprite::new(Vec2::new(2.0, 4.0))
            .unwrap()
            .quad(Transform::identity(), camera, viewport)
            .unwrap();
        assert_eq!(quad.vertices.len(), 6);
        assert_eq!(quad.vertices[0].uv, [0.0, 0.0]);
        assert_eq!(quad.vertices[2].uv, [1.0, 1.0]);
        near(quad.vertices[0].position[0], 40.0);
        near(quad.vertices[0].position[1], 30.0);
        near(quad.vertices[2].position[0], 60.0);
        near(quad.vertices[2].position[1], 70.0);
    }
    #[test]
    fn scale_rotation_and_pivot_apply() {
        let (camera, viewport) = camera();
        let sprite = Sprite::new(Vec2::new(2.0, 2.0))
            .unwrap()
            .with_pivot(Vec2::new(0.0, 0.0))
            .unwrap()
            .with_tint([0.2, 0.3, 0.4, 0.5])
            .unwrap();
        let quad = sprite
            .quad(
                Transform {
                    position: Vec2::new(1.0, 0.0),
                    rotation: std::f32::consts::FRAC_PI_2,
                    scale: Vec2::new(2.0, 1.0),
                },
                camera,
                viewport,
            )
            .unwrap();
        assert_eq!(quad.vertices[0].color, [0.2, 0.3, 0.4, 0.5]);
        near(quad.vertices[0].position[0], 40.0);
        near(quad.vertices[0].position[1], 50.0);
        near(quad.vertices[1].position[0], 40.0);
        near(quad.vertices[1].position[1], 10.0);
    }
    #[test]
    fn rejects_invalid_input() {
        assert!(Sprite::new(Vec2::new(0.0, 1.0)).is_err());
        assert!(
            Sprite::new(Vec2::new(1.0, 1.0))
                .unwrap()
                .with_uv(Vec2::new(-0.1, 0.0), Vec2::new(1.0, 1.0))
                .is_err()
        );
        assert!(
            Sprite::new(Vec2::new(1.0, 1.0))
                .unwrap()
                .with_tint([f32::NAN; 4])
                .is_err()
        );
    }
}
