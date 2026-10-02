//! docs/54 §4.2：视图**可见遮罩**的实体侧。
//!
//! `RenderView` 从很早起就有 `visibility: u32` 字段（"可见层遮罩，`u32::MAX` = 全可见"），
//! 但**没有任何消费者**——字段是空的。本模块把这条契约接上：
//!
//! ```text
//! 实体 `Visibility { layers }`  ∩  `RenderView::visibility`  != 0  ⇒  本视图画它
//! ```
//!
//! 语义（刻意的几条边界）：
//!
//! - 实体**没有** `Visibility` 组件 ⇒ 视为全可见（`u32::MAX`）。既有场景因此一字不改、像素不变。
//! - 位怎么分配由项目自己决定（引擎不预设"第 0 位是世界内容"这种约定）；需要的是"视图与实体
//!   用同一套位"。
//! - 被遮掉的实体是**不存在于这个视图**，不是"透明"：它不产顶点、不占默认绘制层的序号、
//!   也不参与计数。
//! - 遮罩只作用于**有实体身份**的内容。编辑器里那些"画布 UI"（按名字注册的面板/血条/网格）
//!   不是实体，因此永远照画——把非实体内容也塞进遮罩只会让"谁被谁挡住"更难查。
//! - 被遮掉的实体数会记进 `RuntimeScene` 的每帧统计（`last_view_filtered`），
//!   因此"东西没画出来"能一眼看出是遮罩拦的还是根本没提取到。

/// 全可见（视图遮罩与实体层的默认值）。
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::World;

pub const ALL_LAYERS: u32 = u32::MAX;

/// 实体属于哪些"可见层"。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Visibility {
    /// 位遮罩：位由项目分配。
    pub layers: u32,
}

impl Default for Visibility {
    fn default() -> Self {
        Self::all()
    }
}

impl Visibility {
    #[must_use]
    pub const fn new(layers: u32) -> Self {
        Self { layers }
    }

    /// 全可见。
    #[must_use]
    pub const fn all() -> Self {
        Self { layers: ALL_LAYERS }
    }

    /// 与视图遮罩是否相交（视图看得见本实体）。
    #[must_use]
    pub const fn visible_in(self, view_mask: u32) -> bool {
        self.layers & view_mask != 0
    }

    /// 解析场景 `Visibility` 组件串：`layers=<n>`，`n` 接受十进制、`0x` 十六进制与 `0b` 二进制
    /// （位遮罩写十六进制最可读）；未知键、空串、非整数**报错**，不静默当全可见。
    pub fn from_component_str(value: &str) -> Result<Self, VisibilityError> {
        let mut layers: Option<u32> = None;
        let mut seen = false;
        for field in value
            .trim()
            .split(';')
            .filter(|field| !field.trim().is_empty())
        {
            seen = true;
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| VisibilityError::MalformedField(field.to_owned()))?;
            let raw = raw.trim();
            match key.trim() {
                "layers" => {
                    layers = Some(parse_layers(raw).ok_or_else(|| {
                        VisibilityError::InvalidValue(format!("layers={raw} (需要 0..=4294967295)"))
                    })?);
                }
                other => return Err(VisibilityError::UnknownField(other.to_owned())),
            }
        }
        if !seen {
            return Err(VisibilityError::Empty);
        }
        layers
            .map(|layers| Self { layers })
            .ok_or(VisibilityError::MissingField("layers"))
    }

    /// 写回组件串：`layers` 用十六进制（位遮罩可读性最好）。全可见的实体**通常不声明**这个组件
    /// ——缺组件就是全可见，所以场景里不写它才是"零改动"的表达方式。
    #[must_use]
    pub fn to_component_str(self) -> String {
        format!("layers=0x{:X}", self.layers)
    }
}

fn parse_layers(raw: &str) -> Option<u32> {
    let raw = raw.trim();
    let (digits, radix) =
        if let Some(rest) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
            (rest, 16)
        } else if let Some(rest) = raw.strip_prefix("0b").or_else(|| raw.strip_prefix("0B")) {
            (rest, 2)
        } else {
            (raw, 10)
        };
    if digits.is_empty() {
        return None;
    }
    u32::from_str_radix(digits, radix).ok()
}

/// 实体在这个视图遮罩下是否可见。没有 `Visibility` 组件的实体视为全可见。
#[must_use]
pub fn entity_visible_in(world: &World, entity: EntityId, view_mask: u32) -> bool {
    match world.get::<Visibility>(entity) {
        Ok(Some(visibility)) => visibility.visible_in(view_mask),
        // 组件缺失（绝大多数实体）与读取错误都按"全可见"处理：遮罩是**可选**的收窄手段，
        // 不该因为一个存储错误把内容整体藏掉。视图遮罩本身为 0 仍是"什么都不画"。
        _ => view_mask != 0,
    }
}

/// `Visibility` 组件串的解析错误。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum VisibilityError {
    #[error("visibility component is empty")]
    Empty,
    #[error("visibility is missing field {0}")]
    MissingField(&'static str),
    #[error("malformed visibility field: {0}")]
    MalformedField(String),
    #[error("unknown visibility field: {0}")]
    UnknownField(String),
    #[error("invalid visibility value: {0}")]
    InvalidValue(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decimal_hex_and_binary_masks() {
        assert_eq!(
            Visibility::from_component_str("layers=1").unwrap().layers,
            1
        );
        assert_eq!(
            Visibility::from_component_str("layers=0x0F")
                .unwrap()
                .layers,
            0x0F
        );
        assert_eq!(
            Visibility::from_component_str("layers=0b1010")
                .unwrap()
                .layers,
            0b1010
        );
        // 全可见可以显式写出来，且写回用十六进制。
        let all = Visibility::from_component_str("layers=4294967295").unwrap();
        assert_eq!(all.layers, ALL_LAYERS);
        assert_eq!(all.to_component_str(), "layers=0xFFFFFFFF");
    }

    /// 拼错必须报错：静默当"全可见"会让遮罩看起来失效，静默当 0 会让内容整体消失——
    /// 两种都只在真机上才看得出来。
    #[test]
    fn rejects_malformed_masks() {
        for raw in [
            "",
            "layers",
            "layers=",
            "layers=abc",
            "layers=0x",
            "layers=-1",
            "layers=4294967296",
            "mask=1",
        ] {
            assert!(
                Visibility::from_component_str(raw).is_err(),
                "{raw:?} 应当被拒绝"
            );
        }
    }

    #[test]
    fn mask_intersection_decides_visibility() {
        let world_only = Visibility::new(0b01);
        let ui_only = Visibility::new(0b10);
        assert!(world_only.visible_in(0b01 | 0b10));
        assert!(world_only.visible_in(0b01));
        assert!(!world_only.visible_in(0b10));
        assert!(!ui_only.visible_in(0b01));
        assert!(Visibility::all().visible_in(0b10));
        // 视图遮罩为 0 ⇒ 什么都不画（这是"关掉整个视图"的表达方式）。
        assert!(!Visibility::all().visible_in(0));
    }
}
