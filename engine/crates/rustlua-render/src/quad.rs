//! docs/54 §4.3：**一等 Quad**（Sprite 的几何本体）。
//!
//! §4.3 把两种可绘制对象写成同一套分解：
//!
//! ```text
//! Sprite = Quad Geometry + TextureRegion + Material + Transform + Depth/Sorting
//! Mesh   = Mesh Geometry + Material + Transform + Depth/Sorting
//! ```
//!
//! 此前这条分解只体现在代码里：`Sprite` 组件**必须**给贴图（缺 `texture=` 直接报
//! `MissingTexture`），于是"一个纯色矩形"只能靠 `texture=__white__` 这种内部哨兵来写，而
//! 那个哨兵在文档里根本没出现过。几何（size/pivot）、材质（tint）与深度/排序虽然都在
//! `SpriteComponent` 里，却和贴图绑在同一个类型上，分不开。
//!
//! `Quad` 把"几何 + 材质"独立成一等组件：**贴图可选**，不给就是内置 1×1 白纹理上的纯色矩形。
//! 两条刻意的边界：
//!
//! - **不是第二条渲染通道**。Quad 在提取阶段降级成与 Sprite 完全相同的
//!   `SpriteBatchItem`（同一个批处理、同一套排序/剔除/深度/可见遮罩），因此 §4.3 那句
//!   "必须进入相同的 RenderWorld 语义"是结构上成立的，而不是靠约定。
//! - **不参与 `Animation`**。帧动画动的是**纹理区域**（UV），而 Quad 恰恰是"没有区域"的那种
//!   几何；给一个矩形播精灵表没有意义。需要帧动画就用 `Sprite`（差别写进 docs/16）。
//!
//! 旋转/缩放/位置来自实体的 `Transform`（与 Sprite 同源），Quad 只描述几何与材质。

use crate::sprite::Sprite;
use crate::sprite::SpriteError;
use crate::sprite::SpriteSorting;
use rustlua_ecs::transform::Vec2;

/// 内置白纹理哨兵：没有贴图的 Quad 采样它，于是 tint 就是最终颜色。
///
/// 与 [`crate::host::frame::WHITE_TEXTURE_PATH`] 是同一个字符串（那边是渲染器解析用的定义，
/// 这里给出无贴图 Quad 的默认值）。刻意不引用 host 层：`core` 不该依赖 `host`。
pub const QUAD_DEFAULT_TEXTURE: &str = "__white__";

/// 一等 Quad：几何（`size`/`pivot`）+ 材质（`color`/可选 `texture`）+ 深度与排序（`z`/`sorting`）。
#[derive(Clone, Debug, PartialEq)]
pub struct Quad {
    /// 四边形尺寸（局部单位，受实体 `Transform::scale` 再缩放）。
    pub size: Vec2,
    /// 轴心（0..=1，`0.5,0.5` = 居中）。
    pub pivot: Vec2,
    /// 颜色（RGBA 乘法，`[1.0; 4]` = 采样原色）。没有贴图时它就是最终颜色。
    pub color: [f32; 4],
    /// 可选贴图；`None` = 用内置白纹理，也就得到一个纯色矩形。
    pub texture: Option<String>,
    /// 世界 z（深度通道用；overlay 路径下只影响同层排序）。
    pub z: f32,
    pub sorting: SpriteSorting,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl Default for Quad {
    fn default() -> Self {
        Self {
            size: Vec2::new(64.0, 64.0),
            pivot: Vec2::new(0.5, 0.5),
            color: [1.0; 4],
            texture: None,
            z: 0.0,
            sorting: SpriteSorting::default(),
            flip_x: false,
            flip_y: false,
        }
    }
}

impl Quad {
    /// 解析场景 `Quad` 组件串：`size=w,h`（必填）、`pivot=x,y`、`color=r,g,b[,a]`、
    /// `texture=<路径>`、`z=<f32>`、`layer=<名>`、`order=<i32>`、`flip_x`/`flip_y`。
    ///
    /// 未知键、缺 `size`、非法数值一律报错：静默忽略会让"我写了 `colour=`"变成画面不动的
    /// 谜题（拼错 `color` 与"颜色没生效"在画面上完全一样）。
    pub fn from_component_str(value: &str) -> Result<Self, QuadError> {
        let mut quad = Quad::default();
        let mut has_size = false;
        let mut seen = false;
        for field in value
            .trim()
            .split(';')
            .filter(|field| !field.trim().is_empty())
        {
            seen = true;
            let (key, raw) = field
                .split_once('=')
                .map(|(key, raw)| (key.trim(), raw.trim()))
                .ok_or_else(|| QuadError::MalformedField(field.to_owned()))?;
            match key {
                "size" => {
                    quad.size = parse_pair(raw, "size")?;
                    has_size = true;
                }
                "pivot" => quad.pivot = parse_pair(raw, "pivot")?,
                "color" => quad.color = parse_color(raw)?,
                "texture" => {
                    if raw.is_empty() {
                        return Err(QuadError::EmptyTexture);
                    }
                    quad.texture = Some(raw.to_owned());
                }
                "z" => {
                    quad.z = raw
                        .parse::<f32>()
                        .ok()
                        .filter(|value| value.is_finite())
                        .ok_or_else(|| QuadError::InvalidNumber(format!("z={raw}")))?;
                }
                "layer" => {
                    if raw.is_empty() {
                        return Err(QuadError::InvalidField(format!("layer={raw}")));
                    }
                    quad.sorting.layer = raw.to_owned();
                }
                "order" => {
                    quad.sorting.order = raw
                        .parse::<i32>()
                        .map_err(|_| QuadError::InvalidField(format!("order={raw}")))?;
                }
                "flip_x" => quad.flip_x = parse_flag(raw, "flip_x")?,
                "flip_y" => quad.flip_y = parse_flag(raw, "flip_y")?,
                other => return Err(QuadError::UnknownField(other.to_owned())),
            }
        }
        if !seen {
            return Err(QuadError::Empty);
        }
        if !has_size {
            return Err(QuadError::MissingField("size"));
        }
        quad.validate()?;
        Ok(quad)
    }

    /// 写回组件串。默认值**不写出来**（`size` 必填、其余只在非默认时追加），因此
    /// "读进来再写回去"对既有写法是稳定的。
    #[must_use]
    pub fn to_component_str(&self) -> String {
        let mut out = format!("size={},{}", self.size.x, self.size.y);
        if self.pivot != Vec2::new(0.5, 0.5) {
            out.push_str(&format!(";pivot={},{}", self.pivot.x, self.pivot.y));
        }
        if self.color != [1.0; 4] {
            out.push_str(&format!(
                ";color={},{},{},{}",
                self.color[0], self.color[1], self.color[2], self.color[3]
            ));
        }
        if let Some(texture) = &self.texture {
            out.push_str(&format!(";texture={texture}"));
        }
        if self.z != 0.0 {
            out.push_str(&format!(";z={}", self.z));
        }
        if self.sorting != SpriteSorting::default() {
            out.push_str(&format!(";layer={}", self.sorting.layer));
            out.push_str(&format!(";order={}", self.sorting.order));
        }
        if self.flip_x {
            out.push_str(";flip_x=true");
        }
        if self.flip_y {
            out.push_str(";flip_y=true");
        }
        out
    }

    /// 采样用的贴图路径：显式贴图优先，否则内置白纹理。
    #[must_use]
    pub fn texture_path(&self) -> &str {
        self.texture.as_deref().unwrap_or(QUAD_DEFAULT_TEXTURE)
    }

    /// 校验（与 [`Sprite`] 同一套规则：尺寸非零有限、轴心有限且落在 0..=1、颜色有限、z 有限）。
    pub fn validate(&self) -> Result<(), QuadError> {
        if !(self.size.x.is_finite() && self.size.y.is_finite())
            || self.size.x <= 0.0
            || self.size.y <= 0.0
        {
            return Err(QuadError::InvalidSize(format!(
                "{},{}",
                self.size.x, self.size.y
            )));
        }
        if !(self.pivot.x.is_finite() && self.pivot.y.is_finite())
            || !(0.0..=1.0).contains(&self.pivot.x)
            || !(0.0..=1.0).contains(&self.pivot.y)
        {
            return Err(QuadError::InvalidPivot(format!(
                "{},{}",
                self.pivot.x, self.pivot.y
            )));
        }
        if let Some(value) = self.color.iter().copied().find(|value| !value.is_finite()) {
            return Err(QuadError::InvalidColor(value.to_string()));
        }
        if !self.z.is_finite() {
            return Err(QuadError::InvalidNumber(format!("z={}", self.z)));
        }
        Ok(())
    }

    /// 降级成渲染用的 [`Sprite`]：**这就是 §4.3 的 "Quad 几何 + 纹理区域"**，纹理区域取整张
    /// 贴图（`0..1`），因此 Quad 与 Sprite 在批处理里是同一个东西，只有区域与动画不同。
    pub fn to_sprite(&self) -> Result<Sprite, QuadError> {
        self.validate()?;
        Ok(Sprite::new(self.size)
            .and_then(|sprite| sprite.with_pivot(self.pivot))
            .and_then(|sprite| sprite.with_tint(self.color))
            .map(|sprite| sprite.with_z(self.z).with_flip(self.flip_x, self.flip_y))?)
    }
}

fn parse_pair(raw: &str, field: &str) -> Result<Vec2, QuadError> {
    let (x, y) = raw
        .split_once(',')
        .ok_or_else(|| QuadError::InvalidField(format!("{field}={raw}")))?;
    let x = x
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| QuadError::InvalidField(format!("{field}={raw}")))?;
    let y = y
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| QuadError::InvalidField(format!("{field}={raw}")))?;
    Ok(Vec2::new(x, y))
}

fn parse_color(raw: &str) -> Result<[f32; 4], QuadError> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() != 3 && parts.len() != 4 {
        return Err(QuadError::InvalidColor(raw.to_owned()));
    }
    let mut color = [1.0f32; 4];
    for (index, part) in parts.iter().enumerate() {
        color[index] = part
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| QuadError::InvalidColor(raw.to_owned()))?;
    }
    Ok(color)
}

fn parse_flag(raw: &str, field: &str) -> Result<bool, QuadError> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(QuadError::InvalidField(format!("{field}={other}"))),
    }
}

/// `Quad` 组件串的解析/校验错误。
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum QuadError {
    #[error("quad component is empty")]
    Empty,
    #[error("quad is missing field {0}")]
    MissingField(&'static str),
    #[error("malformed quad field: {0}")]
    MalformedField(String),
    #[error("unknown quad field: {0}")]
    UnknownField(String),
    #[error("invalid quad field: {0}")]
    InvalidField(String),
    #[error("invalid quad size: {0}")]
    InvalidSize(String),
    #[error("invalid quad pivot: {0}")]
    InvalidPivot(String),
    #[error("invalid quad color: {0}")]
    InvalidColor(String),
    #[error("invalid quad number: {0}")]
    InvalidNumber(String),
    #[error("quad texture must not be empty")]
    EmptyTexture,
    #[error(transparent)]
    Sprite(#[from] SpriteError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_field_and_round_trips() {
        let quad = Quad::from_component_str(
            "size=32,16;pivot=0,1;color=1,0.5,0,0.25;texture=a.png;z=-2;layer=actor;order=3;flip_x=true",
        )
        .expect("valid quad");
        assert_eq!(quad.size, Vec2::new(32.0, 16.0));
        assert_eq!(quad.pivot, Vec2::new(0.0, 1.0));
        assert_eq!(quad.color, [1.0, 0.5, 0.0, 0.25]);
        assert_eq!(quad.texture.as_deref(), Some("a.png"));
        assert_eq!(quad.z, -2.0);
        assert_eq!(quad.sorting.layer, "actor");
        assert_eq!(quad.sorting.order, 3);
        assert!(quad.flip_x && !quad.flip_y);

        let encoded = quad.to_component_str();
        assert_eq!(Quad::from_component_str(&encoded).unwrap(), quad);
    }

    /// 默认值不写进组件串（既有写法"读进来再写回去"必须稳定），而 `size` 永远写出来。
    #[test]
    fn writes_only_non_default_fields() {
        let quad = Quad::from_component_str("size=64,64").expect("valid quad");
        assert_eq!(quad.to_component_str(), "size=64,64");
        assert!(quad.texture.is_none());
        assert_eq!(
            quad.texture_path(),
            QUAD_DEFAULT_TEXTURE,
            "没有贴图时采样内置白纹理 ⇒ tint 就是最终颜色"
        );
    }

    /// 拼错字段必须报错：`colour=` 被静默忽略与"颜色没生效"在画面上完全一样。
    #[test]
    fn rejects_malformed_components() {
        for raw in [
            "",
            "colour=1,0,0",
            "size",
            "size=64",
            "size=0,64",
            "size=64,nan",
            "size=64,64;pivot=2,0.5",
            "size=64,64;color=1,1",
            "size=64,64;color=1,1,1,nan",
            "size=64,64;texture=",
            "size=64,64;z=deep",
            "size=64,64;flip_x=maybe",
            "size=64,64;order=first",
        ] {
            assert!(Quad::from_component_str(raw).is_err(), "{raw:?} 应当被拒绝");
        }
        assert_eq!(
            Quad::from_component_str("size=64,64;colour=1,0,0"),
            Err(QuadError::UnknownField("colour".to_owned()))
        );
        assert_eq!(
            Quad::from_component_str("pivot=0.5,0.5"),
            Err(QuadError::MissingField("size"))
        );
    }

    /// 降级路径就是 §4.3 的 "Quad 几何 + 纹理区域"：几何/材质/深度原样带过去，区域取整张贴图。
    #[test]
    fn lowers_to_a_sprite_with_the_full_texture_region() {
        let quad =
            Quad::from_component_str("size=10,20;pivot=0,0;color=0.5,0.5,0.5,1;z=1.5;flip_y=true")
                .expect("valid quad");
        let sprite = quad.to_sprite().expect("lowered sprite");
        assert_eq!(sprite.size, Vec2::new(10.0, 20.0));
        assert_eq!(sprite.pivot, Vec2::new(0.0, 0.0));
        assert_eq!(sprite.tint, [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(sprite.z, 1.5);
        assert!(sprite.flip_y);
        assert_eq!(sprite.uv_min, Vec2::zero());
        assert_eq!(sprite.uv_max, Vec2::new(1.0, 1.0));
    }
}
