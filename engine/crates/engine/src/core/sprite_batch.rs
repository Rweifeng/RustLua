//! CPU-side batching for textured 2D Sprites.

use std::{error::Error, fmt};

use crate::core::{
    math2d::{Camera2D, CoordinateError, Viewport},
    sprite::{Sprite, SpriteError, SpriteVertex},
    transform::Transform,
};

/// One Sprite submitted to a batch. Sprites with the same texture and layer are
/// emitted as one draw range; lower layers are always emitted first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteBatchItem {
    pub texture_id: u32,
    pub layer: i32,
    pub sprite: Sprite,
    pub transform: Transform,
}

impl SpriteBatchItem {
    #[must_use]
    pub const fn new(texture_id: u32, layer: i32, sprite: Sprite, transform: Transform) -> Self {
        Self {
            texture_id,
            layer,
            sprite,
            transform,
        }
    }
}

/// A contiguous range in the flattened Sprite vertex buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpriteDrawBatch {
    pub texture_id: u32,
    pub layer: i32,
    pub vertex_start: usize,
    pub vertex_count: usize,
}

/// Result of building a Sprite batch for one camera and viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteBatchMesh {
    pub vertices: Vec<SpriteVertex>,
    pub draws: Vec<SpriteDrawBatch>,
}

impl SpriteBatchMesh {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
    #[must_use]
    pub const fn sprite_count(&self) -> usize {
        self.vertices.len() / 6
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpriteBatchError {
    Sprite { index: usize, error: SpriteError },
    Coordinate(CoordinateError),
    InvalidTransform { index: usize },
}

impl fmt::Display for SpriteBatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sprite { index, error } => {
                write!(f, "sprite batch item {index} is invalid: {error}")
            }
            Self::Coordinate(error) => error.fmt(f),
            Self::InvalidTransform { index } => {
                write!(f, "sprite batch item {index} has a non-finite transform")
            }
        }
    }
}
impl Error for SpriteBatchError {}
impl From<CoordinateError> for SpriteBatchError {
    fn from(value: CoordinateError) -> Self {
        Self::Coordinate(value)
    }
}

/// Collects Sprite submissions and converts them into sorted draw batches.
#[derive(Clone, Debug, Default)]
pub struct SpriteBatch {
    items: Vec<SpriteBatchItem>,
}

impl SpriteBatch {
    #[must_use]
    pub const fn new() -> Self {
        Self { items: Vec::new() }
    }
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            items: Vec::with_capacity(capacity),
        }
    }
    pub fn clear(&mut self) {
        self.items.clear();
    }
    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn push(&mut self, item: SpriteBatchItem) {
        self.items.push(item);
    }

    /// Builds stable layer-major, texture-minor batches.
    pub fn build(
        &self,
        camera: Camera2D,
        viewport: Viewport,
    ) -> Result<SpriteBatchMesh, SpriteBatchError> {
        let mut ordered: Vec<(usize, &SpriteBatchItem)> = self.items.iter().enumerate().collect();
        ordered.sort_by(|(left_index, left), (right_index, right)| {
            left.layer
                .cmp(&right.layer)
                .then_with(|| left.texture_id.cmp(&right.texture_id))
                .then_with(|| left_index.cmp(right_index))
        });
        let mut vertices = Vec::with_capacity(ordered.len() * 6);
        let mut draws: Vec<SpriteDrawBatch> = Vec::new();
        for (index, item) in ordered {
            if !item.transform.position.x.is_finite()
                || !item.transform.position.y.is_finite()
                || !item.transform.rotation.is_finite()
                || !item.transform.scale.x.is_finite()
                || !item.transform.scale.y.is_finite()
            {
                return Err(SpriteBatchError::InvalidTransform { index });
            }
            let quad = item
                .sprite
                .quad(item.transform, camera, viewport)
                .map_err(|error| SpriteBatchError::Sprite { index, error })?;
            let start = vertices.len();
            vertices.extend_from_slice(&quad.vertices);
            if let Some(previous) = draws.last_mut()
                && previous.texture_id == item.texture_id
                && previous.layer == item.layer
                && previous.vertex_start + previous.vertex_count == start
            {
                previous.vertex_count += 6;
            } else {
                draws.push(SpriteDrawBatch {
                    texture_id: item.texture_id,
                    layer: item.layer,
                    vertex_start: start,
                    vertex_count: 6,
                });
            }
        }
        Ok(SpriteBatchMesh { vertices, draws })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        math2d::{Camera2D, Viewport},
        sprite::Sprite,
        transform::{Transform, Vec2},
    };

    fn setup() -> (Camera2D, Viewport) {
        (
            Camera2D::new(Vec2::zero(), 10.0).unwrap(),
            Viewport::new(100.0, 100.0).unwrap(),
        )
    }
    fn item(texture_id: u32, layer: i32, x: f32) -> SpriteBatchItem {
        SpriteBatchItem::new(
            texture_id,
            layer,
            Sprite::new(Vec2::new(1.0, 1.0)).unwrap(),
            Transform::from_position(Vec2::new(x, 0.0)),
        )
    }

    #[test]
    fn groups_same_texture_and_layer() {
        let (camera, viewport) = setup();
        let mut batch = SpriteBatch::new();
        batch.push(item(2, 0, 0.0));
        batch.push(item(1, 0, 1.0));
        batch.push(item(2, 0, 2.0));
        let mesh = batch.build(camera, viewport).unwrap();
        assert_eq!(mesh.sprite_count(), 3);
        assert_eq!(
            mesh.draws,
            vec![
                SpriteDrawBatch {
                    texture_id: 1,
                    layer: 0,
                    vertex_start: 0,
                    vertex_count: 6
                },
                SpriteDrawBatch {
                    texture_id: 2,
                    layer: 0,
                    vertex_start: 6,
                    vertex_count: 12
                }
            ]
        );
    }
    #[test]
    fn sorts_layers_before_textures_and_is_stable() {
        let (camera, viewport) = setup();
        let mut batch = SpriteBatch::new();
        batch.push(item(9, 1, 0.0));
        batch.push(item(2, -1, 1.0));
        batch.push(item(3, -1, 2.0));
        batch.push(item(2, -1, 3.0));
        let mesh = batch.build(camera, viewport).unwrap();
        assert_eq!(
            mesh.draws
                .iter()
                .map(|draw| (draw.layer, draw.texture_id, draw.vertex_count))
                .collect::<Vec<_>>(),
            vec![(-1, 2, 12), (-1, 3, 6), (1, 9, 6)]
        );
    }
    #[test]
    fn preserves_atlas_uvs_and_tint() {
        let (camera, viewport) = setup();
        let sprite = Sprite::new(Vec2::new(2.0, 2.0))
            .unwrap()
            .with_uv(Vec2::new(0.25, 0.5), Vec2::new(0.75, 1.0))
            .unwrap()
            .with_tint([0.1, 0.2, 0.3, 0.4])
            .unwrap();
        let mut batch = SpriteBatch::new();
        batch.push(SpriteBatchItem::new(4, 0, sprite, Transform::identity()));
        let mesh = batch.build(camera, viewport).unwrap();
        assert_eq!(mesh.vertices[0].uv, [0.25, 0.5]);
        assert_eq!(mesh.vertices[2].uv, [0.75, 1.0]);
        assert_eq!(mesh.vertices[0].color, [0.1, 0.2, 0.3, 0.4]);
    }
    #[test]
    fn rejects_invalid_transform_with_original_index() {
        let (camera, viewport) = setup();
        let mut batch = SpriteBatch::new();
        batch.push(item(1, 0, 0.0));
        batch.push(SpriteBatchItem::new(
            1,
            0,
            Sprite::new(Vec2::new(1.0, 1.0)).unwrap(),
            Transform {
                position: Vec2::new(f32::NAN, 0.0),
                ..Transform::identity()
            },
        ));
        assert_eq!(
            batch.build(camera, viewport),
            Err(SpriteBatchError::InvalidTransform { index: 1 })
        );
    }
}
