//! CPU-side Sprite geometry generation for the 2D renderer.

use std::{error::Error, fmt};

use crate::core::{
    camera::{Camera2D, CoordinateError, OrthoTerms, Viewport},
    transform::{Transform, Vec2},
};

/// A textured, tinted quad description. `pivot` and UVs are normalized.
///
/// `flip_x` / `flip_y` 只翻转**采样方向**（UV 在四边形上的走向），不改变 UV 范围本身。
/// 这样 `validate` 的 `uv_min < uv_max` 约束仍然成立，也能直接复用 UV 来生成翻转后的
/// 顶点；如果靠交换 uv_min/uv_max 来表达翻转，就会被该校验拒绝。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    pub size: Vec2,
    pub pivot: Vec2,
    pub uv_min: Vec2,
    pub uv_max: Vec2,
    pub tint: [f32; 4],
    pub flip_x: bool,
    pub flip_y: bool,
    /// 世界 z（WP6）：四边形所在的世界平面深度。默认 0（世界平面），只在使用
    /// 世界 Sprite 深度通道时参与深度测试；overlay 路径忽略它。
    pub z: f32,
}

/// One CPU-side vertex in screen-pixel space.
///
/// `Default` 存在的唯一理由是让并行合批能**一次开好输出缓冲**再分块就写入（WP8）；
/// 全零顶点不表示任何合法几何，只是占位。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
    /// 水平翻转（面向左右）。默认关闭。
    pub flip_x: bool,
    /// 垂直翻转。默认关闭。
    pub flip_y: bool,
    /// 世界 z（组件串可选 `z=`，默认 0）。非 0 时写回组件串，因此既有场景字符串不变。
    pub z: f32,
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

/// 符号层名 → 批处理分层桶。
///
/// **不要用 `to_ascii_lowercase()`**：它每个精灵每帧都会造一个 `String`，10k 精灵就是
/// 1 万次分配/帧（`wp0-v4` 实测的 1.0 次分配/精灵正是这一处）。大小写不敏感比较用
/// `eq_ignore_ascii_case`，零分配。
fn sorting_layer_rank(layer: &str) -> i32 {
    let layer = layer.trim();
    if layer.eq_ignore_ascii_case("background") {
        return -2;
    }
    if layer.eq_ignore_ascii_case("world") {
        return -1;
    }
    if layer.eq_ignore_ascii_case("actor") {
        return 1;
    }
    if layer.eq_ignore_ascii_case("foreground") {
        return 2;
    }
    if layer.eq_ignore_ascii_case("ui") {
        return 3;
    }
    // `default` 与未知值同归 0（与旧实现一致）。
    0
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpriteComponentError {
    MissingTexture,
    InvalidField(String),
    InvalidSize(String),
    InvalidPivot(String),
    InvalidTint(String),
    InvalidOrder(String),
    /// 世界 z 不是有限数（WP6：深度必须可比较，NaN 会让整帧深度测试失效）。
    InvalidDepth(String),
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
            Self::InvalidDepth(value) => write!(f, "invalid sprite z: {value}"),
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
            flip_x: false,
            flip_y: false,
            z: 0.0,
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
                "flip_x" => component.flip_x = parse_flag(raw, "flip_x")?,
                "flip_y" => component.flip_y = parse_flag(raw, "flip_y")?,
                "z" => {
                    let value: f32 = raw
                        .trim()
                        .parse()
                        .map_err(|_| SpriteComponentError::InvalidDepth(raw.to_owned()))?;
                    if !value.is_finite() {
                        return Err(SpriteComponentError::InvalidDepth(raw.to_owned()));
                    }
                    component.z = value;
                }
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
        let mut text = format!(
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
        );
        // 只在开启时输出：既有场景的规范字符串保持不变，避免无谓的存档改动。
        if self.flip_x {
            text.push_str(";flip_x=true");
        }
        if self.flip_y {
            text.push_str(";flip_y=true");
        }
        if self.z != 0.0 {
            text.push_str(&format!(";z={}", trim_float(self.z)));
        }
        text
    }

    pub fn sprite(&self) -> Result<Sprite, SpriteComponentError> {
        Sprite::new(self.size)?
            .with_pivot(self.pivot)?
            .with_tint(self.tint)
            .map(|sprite| sprite.with_flip(self.flip_x, self.flip_y))
            .map(|sprite| sprite.with_z(self.z))
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

/// 解析 `flip_x` / `flip_y`。只接受明确的 true/false（含 1/0）；拼错的值必须报错，
/// 不能静默当成 false——那会让"翻转变成了看不懂就不翻"。
fn parse_flag(raw: &str, field: &str) -> Result<bool, SpriteComponentError> {
    match raw.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(SpriteComponentError::InvalidField(format!(
            "{field}={other}"
        ))),
    }
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
            flip_x: false,
            flip_y: false,
            z: 0.0,
        };
        sprite.validate()?;
        Ok(sprite)
    }

    /// 设置世界 z（WP6 世界 Sprite 深度通道用）。
    #[must_use]
    pub const fn with_z(mut self, z: f32) -> Self {
        self.z = z;
        self
    }

    /// 设置水平/垂直翻转。翻转只影响 UV 走向，不需要额外校验。
    #[must_use]
    pub const fn with_flip(mut self, flip_x: bool, flip_y: bool) -> Self {
        self.flip_x = flip_x;
        self.flip_y = flip_y;
        self
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
    ///
    /// 便捷入口：每次调用现烘焙一次相机。**热路径请用 [`Sprite::quad_with_terms`]**——
    /// 逐精灵重复烘焙正是"每精灵 4 次校验 + 三角函数"的来源。
    pub fn quad(
        &self,
        transform: Transform,
        camera: Camera2D,
        viewport: Viewport,
    ) -> Result<SpriteQuad, SpriteError> {
        let terms = OrthoTerms::new(camera, viewport).map_err(SpriteError::Coordinate)?;
        self.quad_with_terms(transform, terms)
    }

    /// Generates screen-space vertices from **pre-baked** camera terms.
    ///
    /// 每帧烘焙一次（[`OrthoTerms::new`]），之后逐顶点只剩乘加：不校验相机、不算三角函数。
    /// 投影结果与 [`Sprite::quad`] 完全一致（`OrthoTerms` 与 `Camera2D::world_to_screen` 同源）。
    pub fn quad_with_terms(
        &self,
        transform: Transform,
        camera: OrthoTerms,
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
            let screen = camera.world_to_screen(world);
            positions[index] = [screen.x, screen.y];
        }
        let (u0, u1) = if self.flip_x {
            (self.uv_max.x, self.uv_min.x)
        } else {
            (self.uv_min.x, self.uv_max.x)
        };
        let (v0, v1) = if self.flip_y {
            (self.uv_max.y, self.uv_min.y)
        } else {
            (self.uv_min.y, self.uv_max.y)
        };
        let uv = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
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

    /// 世界空间四边形的 4 个角（WP6 世界 Sprite 深度通道用）。
    ///
    /// 与 [`Sprite::quad`] 共用同一份局部角点与变换数学，区别只是**不做投影**：交给顶点着色器
    /// 用 `Camera2D::view_projection` 去投影，路径因此只有一份（不会出现"CPU 投影"与
    /// "GPU 投影"两套约定）。返回顺序与 `quad` 一致（左上、右上、右下、左下）。
    pub fn quad_world(&self, transform: Transform) -> Result<[[f32; 2]; 4], SpriteError> {
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
            positions[index] = [
                transform.position.x + scaled.x * cos - scaled.y * sin,
                transform.position.y + scaled.x * sin + scaled.y * cos,
            ];
        }
        Ok(positions)
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
            flip_x: false,
            flip_y: false,
            z: 0.0,
        };
        let encoded = component.to_component_str();
        let decoded = SpriteComponent::from_component_str(&encoded).unwrap();
        assert_eq!(decoded, component);
        assert_eq!(decoded.sorting.effective_layer(), 1012);
    }

    /// WP6：Sprite 的世界 z（`z=`）。默认 0 且**不写回**（既有场景字符串逐字节不变）；
    /// 非有限值报错——NaN 会让整帧深度测试失效，属于"必须拒绝"而不是"静默当 0"。
    #[test]
    fn sprite_z_parses_round_trips_and_rejects_non_finite() {
        let plain =
            SpriteComponent::from_component_str("texture=a.png;size=32x32;pivot=0.5,0.5").unwrap();
        assert_eq!(plain.z, 0.0);
        assert!(
            !plain.to_component_str().contains(";z="),
            "默认 z 不得写回组件串：{}",
            plain.to_component_str()
        );

        let with_z =
            SpriteComponent::from_component_str("texture=a.png;size=32x32;z=-2.5").unwrap();
        assert_eq!(with_z.z, -2.5);
        let encoded = with_z.to_component_str();
        assert!(encoded.contains(";z=-2.5"), "{encoded}");
        assert_eq!(
            SpriteComponent::from_component_str(&encoded).unwrap(),
            with_z,
            "z 必须能原样往返"
        );
        // z 传递到运行时 `Sprite`（世界 Sprite 深度通道读它）。
        assert_eq!(with_z.sprite().unwrap().z, -2.5);

        assert!(SpriteComponent::from_component_str("texture=a.png;z=nan").is_err());
        assert!(SpriteComponent::from_component_str("texture=a.png;z=inf").is_err());
        assert!(SpriteComponent::from_component_str("texture=a.png;z=deep").is_err());
    }

    #[test]
    fn sprite_flip_parses_round_trips_and_flips_sampled_uvs() {
        let component = SpriteComponent::from_component_str(
            "texture=assets/textures/player.png;size=32x32;pivot=0.5,0.5;flip_x=true",
        )
        .unwrap();
        assert!(component.flip_x);
        assert!(!component.flip_y);

        // 只在开启时输出，且能原样读回。
        let encoded = component.to_component_str();
        assert!(encoded.ends_with(";flip_x=true"), "{encoded}");
        assert_eq!(
            SpriteComponent::from_component_str(&encoded).unwrap(),
            component
        );

        // 未开启时不得改变既有规范字符串（避免存档 churn）。
        let plain =
            SpriteComponent::from_component_str("texture=a.png;size=32x32;pivot=0.5,0.5").unwrap();
        assert!(!plain.to_component_str().contains("flip"));

        // 拼错的布尔值必须报错，不能静默当成 false。
        assert!(
            SpriteComponent::from_component_str("texture=a.png;size=32x32;flip_x=maybe").is_err()
        );

        // 顶点取样方向必须真的翻转，而 UV 范围本身不变。
        let sprite = component
            .sprite()
            .unwrap()
            .with_uv(Vec2::new(0.25, 0.0), Vec2::new(0.75, 1.0))
            .unwrap();
        assert_eq!(sprite.uv_min, Vec2::new(0.25, 0.0));
        assert_eq!(sprite.uv_max, Vec2::new(0.75, 1.0));

        let quad = sprite
            .quad(
                Transform::default(),
                Camera2D::new(Vec2::new(320.0, 180.0), 1.0).unwrap(),
                Viewport::new(320.0, 180.0).unwrap(),
            )
            .unwrap();
        // 四边形首顶点是左上角：翻转前取 uv_min.x，翻转后取 uv_max.x。
        assert_eq!(quad.vertices[0].uv, [0.75, 0.0]);
        assert_eq!(quad.vertices[1].uv, [0.25, 0.0]);

        let vertical = sprite
            .with_flip(false, true)
            .quad(
                Transform::default(),
                Camera2D::new(Vec2::new(320.0, 180.0), 1.0).unwrap(),
                Viewport::new(320.0, 180.0).unwrap(),
            )
            .unwrap();
        assert_eq!(vertical.vertices[0].uv, [0.25, 1.0]);
        assert_eq!(vertical.vertices[2].uv, [0.75, 0.0]);
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
