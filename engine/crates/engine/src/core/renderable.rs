//! docs/54 §4.3：**单一 `Renderable` 与单一 `Material`**。
//!
//! §4.3 把可绘制对象写成同一套分解：
//!
//! ```text
//! Sprite = Quad Geometry + TextureRegion + Material + Transform + Depth/Sorting
//! Mesh   = Mesh Geometry + Material + Transform + Depth/Sorting
//! ```
//!
//! Quad 这一半已经落地（批 D）。剩下两件事：
//!
//! - **`Renderable` 是单一的**：一个实体对渲染器来说只有**一种**可绘制表达。"Sprite + Quad 都挂
//!   在同一个实体上"此前会**画两遍**（两条收集路径各自 push），既没有诊断也没有优先级——本模块把
//!   分类写成一个类型，冲突因此可以被点名（文档侧与加载期都点）。
//! - **`Material` 是单一的**：精灵/Quad 的"贴图 + 颜色"与网格的 glTF PBR 此前是两套说法
//!   （`Sprite::tint` vs `GpuMaterial`）。现在都用一个 [`Material`] 描述，渲染器的 `GpuMaterial`
//!   由它构造，于是"材质是什么"只有一处定义。
//!
//! 边界（刻意）：`Renderable` 是**分类视图**，不是第二份真源——它从组件推出来，不参与序列化、
//! 不参与渲染决策；渲染仍走原来的批处理/网格路径（那是性能所在）。

use crate::core::transform::Vec2;

/// 一个实体的可绘制表达的种类。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RenderableKind {
    /// 2D 精灵（贴图 + 纹理区域 + 可被帧动画改 UV）。
    Sprite,
    /// 一等矩形（几何 + 材质，贴图可选）。
    Quad,
    /// 3D 网格。
    Mesh,
    /// 世界空间文字。
    WorldText,
    /// 屏幕空间文字。
    Text,
    /// 屏幕空间图片。
    Image,
    /// 粒子发射器。
    Particle,
}

impl RenderableKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sprite => "sprite",
            Self::Quad => "quad",
            Self::Mesh => "mesh",
            Self::WorldText => "world_text",
            Self::Text => "text",
            Self::Image => "image",
            Self::Particle => "particle",
        }
    }

    /// 场景组件名 → 种类。非可绘制组件返回 `None`。
    #[must_use]
    pub fn from_component_name(name: &str) -> Option<Self> {
        match name {
            "Sprite" => Some(Self::Sprite),
            "Quad" => Some(Self::Quad),
            "MeshRenderer" => Some(Self::Mesh),
            "WorldText" => Some(Self::WorldText),
            "Text" => Some(Self::Text),
            "Image" => Some(Self::Image),
            "ParticleEmitter" => Some(Self::Particle),
            _ => None,
        }
    }

    /// 冲突时的优先级（小的赢）。顺序按"越专门的表达越优先"排：
    /// `Sprite`（带纹理区域与动画）→ `Quad`（纯几何 + 材质）→ `Mesh` → 文字 → 图片 → 粒子。
    ///
    /// 这不是"随便挑一个"：它写下来是为了让"两者都挂时画哪一个"可预测、可断言，而不是取决于
    /// 收集循环的先后。
    #[must_use]
    pub const fn precedence(self) -> u8 {
        match self {
            Self::Sprite => 0,
            Self::Quad => 1,
            Self::Mesh => 2,
            Self::WorldText => 3,
            Self::Text => 4,
            Self::Image => 5,
            Self::Particle => 6,
        }
    }
}

/// 一个实体的可绘制分类结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Renderable {
    /// 恰好一种可绘制组件。
    Single(RenderableKind),
    /// 没有任何可绘制组件（纯逻辑/容器实体）。
    None,
    /// 挂了多种：这是数据错误，必须点名——不然"画了两遍"或"只有一个生效"都无从解释。
    Conflict {
        /// 冲突的组件名（按固定顺序给出，便于断言）。
        kinds: [Option<RenderableKind>; 3],
        /// 按优先级实际生效的那一种。
        resolved: RenderableKind,
    },
}

impl Renderable {
    /// 从组件名集合分类。
    #[must_use]
    pub fn classify<'a>(components: impl IntoIterator<Item = &'a str>) -> Self {
        let mut kinds: Vec<RenderableKind> = components
            .into_iter()
            .filter_map(RenderableKind::from_component_name)
            .collect();
        kinds.sort_by_key(|kind| (kind.precedence(), kind.as_str()));
        kinds.dedup();
        match kinds.len() {
            0 => Self::None,
            1 => Self::Single(kinds[0]),
            _ => {
                let mut slots = [None; 3];
                for (index, kind) in kinds.iter().take(3).enumerate() {
                    slots[index] = Some(*kind);
                }
                Self::Conflict {
                    kinds: slots,
                    resolved: kinds[0],
                }
            }
        }
    }

    /// 实际生效的种类（没有可绘制组件 ⇒ `None`）。
    #[must_use]
    pub const fn effective(self) -> Option<RenderableKind> {
        match self {
            Self::Single(kind) => Some(kind),
            Self::None => None,
            Self::Conflict { resolved, .. } => Some(resolved),
        }
    }

    /// 是否需要在加载期告警（只有冲突需要）。
    #[must_use]
    pub const fn is_conflict(self) -> bool {
        matches!(self, Self::Conflict { .. })
    }

    /// 冲突的可读描述（诊断用）。
    #[must_use]
    pub fn describe(self) -> Option<String> {
        match self {
            Self::Conflict { kinds, resolved } => {
                let names: Vec<&str> = kinds.iter().flatten().map(|kind| kind.as_str()).collect();
                Some(format!(
                    "一个实体挂了多种可绘制组件（{}），实际按优先级只画 {}",
                    names.join(" + "),
                    resolved.as_str()
                ))
            }
            _ => None,
        }
    }
}

/// 材质：精灵/Quad 的"贴图 + 颜色"与网格的 glTF PBR 共用同一套说法（docs/54 §4.3）。
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// 基色（RGBA 乘法；网格下即 glTF 的 `baseColorFactor`）。
    pub base_color: [f32; 4],
    /// 贴图路径（精灵/Quad 可缺省 = 内置白纹理；网格下缺省 = 白底）。
    pub texture: Option<String>,
    /// glTF 金属度（精灵/Quad 恒为 0）。
    pub metallic: f32,
    /// glTF 粗糙度（精灵/Quad 恒为 1）。
    pub roughness: f32,
    /// 是否走无光照路径（精灵/Quad 是；网格否）。
    pub unlit: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self::unlit([1.0; 4], None)
    }
}

impl Material {
    /// 无光照材质（精灵/Quad 用）：`base_color` 就是最终颜色（缺贴图时）。
    #[must_use]
    pub fn unlit(base_color: [f32; 4], texture: Option<String>) -> Self {
        Self {
            base_color,
            texture,
            metallic: 0.0,
            roughness: 1.0,
            unlit: true,
        }
    }

    /// glTF 的 PBR 材质（网格用）。
    #[must_use]
    pub fn pbr(
        base_color: [f32; 4],
        texture: Option<String>,
        metallic: f32,
        roughness: f32,
    ) -> Self {
        Self {
            base_color,
            texture,
            metallic,
            roughness,
            unlit: false,
        }
    }

    /// 校验：颜色/数值必须有限，金属度与粗糙度在 `0..=1`。
    pub fn validate(&self) -> Result<(), MaterialError> {
        if let Some(value) = self
            .base_color
            .iter()
            .copied()
            .find(|value| !value.is_finite())
        {
            return Err(MaterialError::NonFiniteBaseColor(value));
        }
        if !self.metallic.is_finite() || !(0.0..=1.0).contains(&self.metallic) {
            return Err(MaterialError::MetallicOutOfRange(self.metallic));
        }
        if !self.roughness.is_finite() || !(0.0..=1.0).contains(&self.roughness) {
            return Err(MaterialError::RoughnessOutOfRange(self.roughness));
        }
        Ok(())
    }

    /// 精灵/Quad 的尺寸与色调之外，颜色是不是"什么都不做"。
    #[must_use]
    pub fn is_identity_tint(&self) -> bool {
        self.base_color == [1.0; 4] && self.unlit
    }
}

/// `Material` 的校验错误。
#[derive(Clone, Debug, PartialEq)]
pub enum MaterialError {
    NonFiniteBaseColor(f32),
    MetallicOutOfRange(f32),
    RoughnessOutOfRange(f32),
}

impl std::fmt::Display for MaterialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFiniteBaseColor(value) => {
                write!(formatter, "material base color must be finite: {value}")
            }
            Self::MetallicOutOfRange(value) => {
                write!(formatter, "material metallic must be in 0..=1: {value}")
            }
            Self::RoughnessOutOfRange(value) => {
                write!(formatter, "material roughness must be in 0..=1: {value}")
            }
        }
    }
}

impl std::error::Error for MaterialError {}

/// 精灵的 `tint` + 贴图路径 → 统一材质。
#[must_use]
pub fn sprite_material(tint: [f32; 4], texture: Option<String>) -> Material {
    Material::unlit(tint, texture)
}

/// Quad 的几何之外的材质部分（颜色 + 可选贴图）。
#[must_use]
pub fn quad_material(color: [f32; 4], texture: Option<String>) -> Material {
    Material::unlit(color, texture)
}

/// 屏幕空间图片（`Image` 组件）的材质：`Image` 自带颜色与贴图，语义与精灵一致。
#[must_use]
pub fn image_material(color: [f32; 4], texture: Option<String>) -> Material {
    Material::unlit(color, texture)
}

/// 一个 2D 尺寸（精灵/Quad 的几何）——列在这里是为了让"几何与材质分开"在类型上可见。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuadGeometry {
    pub size: Vec2,
    pub pivot: Vec2,
}

impl QuadGeometry {
    #[must_use]
    pub const fn new(size: Vec2, pivot: Vec2) -> Self {
        Self { size, pivot }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_a_single_renderable_and_ignores_non_renderables() {
        assert_eq!(
            Renderable::classify(["Transform", "Sprite", "Collider"]),
            Renderable::Single(RenderableKind::Sprite)
        );
        assert_eq!(
            Renderable::classify(["Transform", "MeshRenderer", "Transform3D"]),
            Renderable::Single(RenderableKind::Mesh)
        );
        assert_eq!(
            Renderable::classify(["Transform", "Script", "Visibility"]),
            Renderable::None
        );
        assert_eq!(Renderable::None.effective(), None);
        assert!(!Renderable::None.is_conflict());
    }

    /// 冲突必须点名，并且"实际画哪一个"是**确定的**（按写下来的优先级），不是收集顺序的巧合。
    #[test]
    fn conflicting_renderables_resolve_by_documented_precedence() {
        let conflict = Renderable::classify(["Sprite", "Quad"]);
        assert_eq!(
            conflict,
            Renderable::Conflict {
                kinds: [
                    Some(RenderableKind::Sprite),
                    Some(RenderableKind::Quad),
                    None
                ],
                resolved: RenderableKind::Sprite,
            }
        );
        assert!(conflict.is_conflict());
        assert_eq!(conflict.effective(), Some(RenderableKind::Sprite));
        let message = conflict.describe().expect("conflict description");
        assert!(message.contains("sprite + quad"), "{message}");
        assert!(message.contains("只画 sprite"), "{message}");

        // 参数顺序不影响结论（分类先按优先级排序）。
        assert_eq!(
            Renderable::classify(["Quad", "Sprite"]),
            Renderable::classify(["Sprite", "Quad"])
        );
    }

    #[test]
    fn material_validates_and_carries_the_shared_description() {
        let material = sprite_material([0.5, 0.25, 1.0, 1.0], Some("a.png".into()));
        assert!(material.unlit);
        assert_eq!(material.metallic, 0.0);
        assert_eq!(material.roughness, 1.0);
        assert!(material.validate().is_ok());
        assert!(!material.is_identity_tint());

        let pbr = Material::pbr([1.0; 4], None, 0.25, 0.8);
        assert!(!pbr.unlit);
        assert!(pbr.validate().is_ok());
        assert!(Material::default().is_identity_tint());
        assert_eq!(
            Material::pbr([1.0; 4], None, 1.5, 0.5),
            Material::pbr([1.0; 4], None, 1.5, 0.5)
        );
        assert_eq!(
            Material::pbr([1.0; 4], None, 1.5, 0.5).validate(),
            Err(MaterialError::MetallicOutOfRange(1.5))
        );
        assert!(matches!(
            Material::unlit([f32::NAN, 0.0, 0.0, 1.0], None).validate(),
            Err(MaterialError::NonFiniteBaseColor(_))
        ));
        assert_eq!(
            Material::pbr([1.0; 4], None, 0.0, 2.0).validate(),
            Err(MaterialError::RoughnessOutOfRange(2.0))
        );
    }
}
