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

/// Editor/runtime-facing Sprite component payload.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteComponent {
    pub texture: String,
    pub size: Vec2,
    pub pivot: Vec2,
    pub tint: [f32; 4],
    pub sorting: SpriteSorting,
}

/// Stable 2D ordering metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpriteSorting {
    pub layer: String,
    pub order: i32,
}

impl Default for SpriteSorting {
    fn default() -> Self {
        Self {
            layer: "default".to_owned(),
            order: 0,
        }
    }
}

impl SpriteSorting {
    /// Maps a symbolic layer to a stable batch order bucket.
    #[must_use]
    pub fn effective_layer(&self) -> i32 {
        sorting_layer_rank(&self.layer)
            .saturating_mul(1_000)
            .saturating_add(self.order.clamp(-999, 999))
    }
}

fn sorting_layer_rank(layer: &str) -> i32 {
    match layer.trim().to_ascii_lowercase().as_str() {
        "background" => -2,
        "world" => -1,
        "default" => 0,
        "actor" => 1,
        "foreground" => 2,
        "ui" => 3,
        _ => 0,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpriteComponentError {
    MissingTexture,
    InvalidField(String),
    InvalidSize(String),
    InvalidPivot(String),
    InvalidTint(String),
    InvalidOrder(String),
    Sprite(SpriteError),
}

impl fmt::Display for SpriteComponentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTexture => write!(f, "sprite component is missing texture"),
            Self::InvalidField(field) => write!(f, "invalid sprite field: {field}"),
            Self::InvalidSize(value) => write!(f, "invalid sprite size: {value}"),
            Self::InvalidPivot(value) => write!(f, "invalid sprite pivot: {value}"),
            Self::InvalidTint(value) => write!(f, "invalid sprite tint: {value}"),
            Self::InvalidOrder(value) => write!(f, "invalid sprite order: {value}"),
            Self::Sprite(error) => error.fmt(f),
        }
    }
}

impl Error for SpriteComponentError {}

impl From<SpriteError> for SpriteComponentError {
    fn from(value: SpriteError) -> Self {
        Self::Sprite(value)
    }
}

impl Default for SpriteComponent {
    fn default() -> Self {
        Self {
            texture: String::new(),
            size: Vec2::new(128.0, 128.0),
            pivot: Vec2::new(0.5, 0.5),
            tint: [1.0; 4],
            sorting: SpriteSorting::default(),
        }
    }
}

impl SpriteComponent {
    pub fn new(texture: impl Into<String>, size: Vec2) -> Result<Self, SpriteComponentError> {
        let component = Self {
            texture: texture.into(),
            size,
            ..Default::default()
        };
        component.validate()?;
        Ok(component)
    }

    pub fn from_component_str(value: &str) -> Result<Self, SpriteComponentError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(SpriteComponentError::MissingTexture);
        }
        let mut component = SpriteComponent::default();
        let mut seen_texture = false;
        for (index, field) in trimmed.split(';').enumerate() {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| SpriteComponentError::InvalidField(field.to_owned()))?;
            match key {
                "texture" if index == 0 => {
                    component.texture = raw.to_owned();
                    seen_texture = true;
                }
                "texture" => {
                    return Err(SpriteComponentError::InvalidField(field.to_owned()));
                }
                "size" => component.size = parse_size(raw)?,
                "pivot" => component.pivot = parse_pair(raw, "pivot")?,
                "tint" => component.tint = parse_tint(raw)?,
                "sorting_layer" => component.sorting.layer = raw.trim().to_owned(),
                "order" => {
                    component.sorting.order = raw
                        .trim()
                        .parse()
                        .map_err(|_| SpriteComponentError::InvalidOrder(raw.to_owned()))?
                }
                _ => return Err(SpriteComponentError::InvalidField(field.to_owned())),
            }
        }
        if !seen_texture || component.texture.trim().is_empty() {
            return Err(SpriteComponentError::MissingTexture);
        }
        component.validate()?;
        Ok(component)
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "texture={};size={}x{};pivot={:.3},{:.3};tint={:.3},{:.3},{:.3},{:.3};sorting_layer={};order={}",
            self.texture,
            trim_float(self.size.x),
            trim_float(self.size.y),
            self.pivot.x,
            self.pivot.y,
            self.tint[0],
            self.tint[1],
            self.tint[2],
            self.tint[3],
            self.sorting.layer,
            self.sorting.order
        )
    }

    pub fn sprite(&self) -> Result<Sprite, SpriteComponentError> {
        Sprite::new(self.size)?
            .with_pivot(self.pivot)?
            .with_tint(self.tint)
            .map_err(Into::into)
    }

    pub fn validate(&self) -> Result<(), SpriteComponentError> {
        if self.texture.trim().is_empty() {
            return Err(SpriteComponentError::MissingTexture);
        }
        let _ = self.sprite()?;
        Ok(())
    }
}

fn parse_size(value: &str) -> Result<Vec2, SpriteComponentError> {
    if let Some((w, h)) = value.split_once('x') {
        let width = w
            .trim()
            .parse()
            .map_err(|_| SpriteComponentError::InvalidSize(value.to_owned()))?;
        let height = h
            .trim()
            .parse()
            .map_err(|_| SpriteComponentError::InvalidSize(value.to_owned()))?;
        Ok(Vec2::new(width, height))
    } else {
        let size = value
            .trim()
            .parse()
            .map_err(|_| SpriteComponentError::InvalidSize(value.to_owned()))?;
        Ok(Vec2::new(size, size))
    }
}

fn parse_pair(value: &str, name: &str) -> Result<Vec2, SpriteComponentError> {
    let Some((x, y)) = value.split_once(',') else {
        return Err(SpriteComponentError::InvalidPivot(format!(
            "{name}={value}"
        )));
    };
    let x = x
        .trim()
        .parse()
        .map_err(|_| SpriteComponentError::InvalidPivot(value.to_owned()))?;
    let y = y
        .trim()
        .parse()
        .map_err(|_| SpriteComponentError::InvalidPivot(value.to_owned()))?;
    Ok(Vec2::new(x, y))
}

fn parse_tint(value: &str) -> Result<[f32; 4], SpriteComponentError> {
    let parts: Vec<_> = value.split(',').collect();
    if parts.len() != 4 {
        return Err(SpriteComponentError::InvalidTint(value.to_owned()));
    }
    Ok([
        parts[0]
            .trim()
            .parse()
            .map_err(|_| SpriteComponentError::InvalidTint(value.to_owned()))?,
        parts[1]
            .trim()
            .parse()
            .map_err(|_| SpriteComponentError::InvalidTint(value.to_owned()))?,
        parts[2]
            .trim()
            .parse()
            .map_err(|_| SpriteComponentError::InvalidTint(value.to_owned()))?,
        parts[3]
            .trim()
            .parse()
            .map_err(|_| SpriteComponentError::InvalidTint(value.to_owned()))?,
    ])
}

fn trim_float(value: f32) -> String {
    if value.fract().abs() < 0.0001 {
        format!("{:.0}", value)
    } else {
        format!("{value:.3}")
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

    #[test]
    fn sprite_component_round_trips_structured_fields() {
        let component = SpriteComponent {
            texture: "assets/textures/player.png".to_owned(),
            size: Vec2::new(48.0, 64.0),
            pivot: Vec2::new(0.25, 0.75),
            tint: [0.5, 0.6, 0.7, 0.8],
            sorting: SpriteSorting {
                layer: "actor".to_owned(),
                order: 12,
            },
        };
        let encoded = component.to_component_str();
        let decoded = SpriteComponent::from_component_str(&encoded).unwrap();
        assert_eq!(decoded, component);
        assert_eq!(decoded.sorting.effective_layer(), 1012);
    }

    #[test]
    fn sprite_component_rejects_invalid_structured_payload() {
        assert!(SpriteComponent::from_component_str("size=32;texture=x.png").is_err());
        assert!(SpriteComponent::from_component_str("assets/textures/player.png;size=32").is_err());
        assert!(SpriteComponent::from_component_str("texture=;size=32").is_err());
        assert!(SpriteComponent::from_component_str("texture=x.png;size=0x32").is_err());
        assert!(SpriteComponent::from_component_str("texture=x.png;pivot=2,0.5").is_err());
        assert!(SpriteComponent::from_component_str("texture=x.png;tint=1,1,1").is_err());
    }
}
