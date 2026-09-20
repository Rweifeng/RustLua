//! Schema 驱动的通用组件字段（docs/55 阶段 2「组件族 Inspector / Schema 驱动」）。
//!
//! 目标是计划里那条硬要求：**新增组件不再修改巨型 match**。带手写面板的组件保持现状，
//! 而"有 schema、没有手写面板"的组件由这里统一生成
//! 「`Component.field` 路径 + 当前原始文本 + 输入种类 + 可选值」的行；视图只负责画输入框，
//! 并把用户输入当作 typed action 发出去（提交边界、Undo、Play 锁定都在 shell 的 dispatch 里）。
//!
//! 接管集合是**推导**的（注册了 schema 且不在 [`HANDWRITTEN_PANEL_COMPONENTS`] 里），
//! 所以给引擎新增组件 + 注册 schema 之后，编辑器自动获得它的字段编辑器。

use engine::core::component_schema::{FieldKind, builtin_registry};
use engine::core::property_path::{PropertyError, PropertyPath, read_field_text, write_field};
use engine::core::scene::{Scene, SceneEntity, SceneEntityId};
use std::fmt;

/// **有手写面板**的组件：这些组件仍由各自的专用面板编辑，通用字段编辑器不碰它们
/// （否则同一字段会出现两个编辑器，互相覆盖）。
///
/// 名单的方向很重要：通用编辑器的接管集合是**推导**出来的
/// （= 注册了 schema 且不在本名单里），不是另一张白名单。因此
/// **给引擎新增一个组件 + 注册 schema 之后，编辑器自动就能按字段编辑它**，
/// 一行 match/白名单都不用改——这正是 docs/55 阶段 2 验收条款
/// "新增组件不再修改巨型 match" 的可检形式。
pub const HANDWRITTEN_PANEL_COMPONENTS: &[&str] = &[
    "Transform",
    "Transform3D",
    "Camera3D",
    // 注意：`Light`（第 20 轮）、`MeshRenderer`（第 21 轮）、`Image`（第 22 轮）**不在**这张表里——
    // 它们的手写面板与整套缓冲管线已删除，改由通用编辑器按 schema 生成字段行
    // （见 `panels/schema_inspector.rs` 的 `field_widget`）。
    // 这就是"组件迁移到 schema"的样板：删面板 + 删缓冲管线、从本表移除即可。
    "Collider3d",
    "Text",
    "Sprite",
    "Tilemap",
    "Animation",
    "UiLayout",
    "Container",
    "UiEvents",
    "UiBindings",
    "ButtonStates",
    "UiAnimation",
    // 实体级脚本：Inspector 有专门的"Lua 路径"输入（含工程内相对路径校验），
    // 不走通用字段行——但它写的**名字与载荷**必须是运行时那份（见 `component_migration`）。
    "Script",
];

/// 该组件是否由通用（schema 驱动）编辑器接管：有 schema 且没有手写面板。
#[must_use]
pub fn is_schema_driven(component: &str) -> bool {
    !HANDWRITTEN_PANEL_COMPONENTS.contains(&component)
        && builtin_registry().contains_key(component)
}

/// 字段的**显示单位**：只影响编辑器的显示与输入，组件串**始终**用引擎单位。
///
/// 为什么放在编辑器而不是引擎 schema：schema 描述的是"字符串契约"，而"人看着是多少度"
/// 是界面问题；把显示单位塞进引擎会让审计/序列化也背上界面语义。将来 SDK 需要插件声明
/// 自定义显示时，再把这张表升级成 schema 元数据也不迟。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldUnit {
    /// 原样显示（与组件串同一个数）。
    Raw,
    /// 弧度 ↔ 角度（引擎存弧度，人看角度）。
    Degrees,
}

impl FieldUnit {
    /// 存储值 → 显示值。
    #[must_use]
    pub fn to_display(self, stored: f64) -> f64 {
        match self {
            Self::Raw => stored,
            Self::Degrees => stored.to_degrees(),
        }
    }

    /// 显示值 → 存储值。
    #[must_use]
    pub fn from_display(self, shown: f64) -> f64 {
        match self {
            Self::Raw => shown,
            Self::Degrees => shown.to_radians(),
        }
    }

    /// 提示后缀（挂在 schema 的 hint 后面，让用户知道单位）。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Raw => "",
            Self::Degrees => "（编辑器用角度，组件串存弧度）",
        }
    }
}

/// 字段候选值的来源。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChoiceSource {
    /// schema 里的静态 `choices`（`FieldKind::Enum`）。
    Static,
    /// **项目资源**：如字体字段的候选来自项目里的字体文件（编辑器侧数据，schema 里没有）。
    ProjectFonts,
}

/// 需要按角度显示的字段（组件, 字段）。
const DEGREE_FIELDS: &[(&str, &str)] = &[("Camera3D", "fov")];

/// 候选值来自项目字体的字段名（`Text.font` / `WorldText.font`）。
const PROJECT_FONT_FIELDS: &[&str] = &["font"];

/// 该字段的显示单位。
#[must_use]
pub fn field_unit(component: &str, field: &str) -> FieldUnit {
    if DEGREE_FIELDS
        .iter()
        .any(|(name, candidate)| *name == component && *candidate == field)
    {
        FieldUnit::Degrees
    } else {
        FieldUnit::Raw
    }
}

/// 该字段的候选值来源。
#[must_use]
pub fn field_choice_source(field: &str) -> ChoiceSource {
    if PROJECT_FONT_FIELDS.contains(&field) {
        ChoiceSource::ProjectFonts
    } else {
        ChoiceSource::Static
    }
}

/// 一行通用字段：路径、当前文本、输入种类。
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaFieldRow {
    pub component: &'static str,
    pub field: &'static str,
    /// `Component.field`——与 Prefab 覆盖、诊断跳转共用同一套路径语法。
    pub path: String,
    pub kind: FieldKind,
    pub required: bool,
    pub choices: &'static [&'static str],
    /// 当前取值的**原始文本**（保留原格式，未改动就不回写）。
    pub text: String,
    /// 组件串里是否**真的**存在这个字段；可选字段缺省时为 `false`。
    pub present: bool,
    /// 类型 + 领域规则的一行说明（编辑器拿来做输入提示，例如"string（工程内相对路径）"）。
    pub hint: String,
    /// 显示单位（只影响编辑器）。
    pub unit: FieldUnit,
    /// 候选值来源（静态 choices 还是项目资源）。
    pub choices_source: ChoiceSource,
}

impl SchemaFieldRow {
    /// 是否是枚举（视图画下拉而不是输入框）。
    #[must_use]
    pub fn is_choice(&self) -> bool {
        matches!(self.kind, FieldKind::Enum)
    }

    /// 可选且当前缺省：输入框应显示"未设置"而不是空串。
    #[must_use]
    pub fn is_unset(&self) -> bool {
        !self.present && !self.required
    }
}

/// 通用字段编辑的失败原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaFieldError {
    /// 实体不在当前场景里。
    EntityMissing(u64),
    /// 实体没有这个组件。
    ComponentMissing { entity: u64, component: String },
    /// 组件没有注册 schema（写不了字段，只能整体替换）。
    NoSchema(String),
    /// 组件没有这个字段。
    UnknownField { component: String, field: String },
    /// 取值不合法（类型/范围/枚举）。
    InvalidValue { field: String, reason: String },
    /// 越界下标（通用编辑器按字段行提交，暂不会产生；保留以完整映射引擎错误）。
    UnknownIndex {
        field: String,
        index: usize,
        len: usize,
    },
    /// **现有的组件串本身不满足 schema**（例如缺必填字段）：逐字段写入会先整串解析，
    /// 因此这种情况下必须整串修正后再按字段编辑，不能报成"你要改的那个字段非法"。
    PayloadUnparsable {
        component: String,
        field: String,
        reason: String,
    },
}

impl fmt::Display for SchemaFieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityMissing(id) => write!(f, "实体不存在：{id}"),
            Self::ComponentMissing { entity, component } => {
                write!(f, "实体 {entity} 没有组件 {component}")
            }
            Self::NoSchema(component) => write!(f, "组件 {component} 没有注册 schema"),
            Self::UnknownField { component, field } => {
                write!(f, "组件 {component} 没有字段 {field}")
            }
            Self::InvalidValue { field, reason } => write!(f, "字段 {field} 取值非法：{reason}"),
            Self::UnknownIndex { field, index, len } => {
                write!(f, "字段 {field} 只有 {len} 个分量，下标 {index} 越界")
            }
            Self::PayloadUnparsable {
                component,
                field,
                reason,
            } => write!(
                f,
                "组件 {component} 的现有串无法按 schema 解析（字段 {field}：{reason}）；\
                 请先用整串/手写面板修正后再按字段编辑"
            ),
        }
    }
}

impl std::error::Error for SchemaFieldError {}

fn map_property_error(error: PropertyError) -> SchemaFieldError {
    match error {
        PropertyError::NoSchema(component) => SchemaFieldError::NoSchema(component),
        PropertyError::UnknownField { component, field } => {
            SchemaFieldError::UnknownField { component, field }
        }
        PropertyError::UnknownIndex { field, index, len } => {
            SchemaFieldError::UnknownIndex { field, index, len }
        }
        PropertyError::InvalidValue { field, reason } => {
            SchemaFieldError::InvalidValue { field, reason }
        }
        PropertyError::Schema(error) => SchemaFieldError::InvalidValue {
            field: error.field,
            reason: error.reason,
        },
    }
}

/// 该实体上由通用编辑器接管的组件名。
///
/// 顺序取自注册表（`BTreeMap` → 按组件名字典序），因此与实体上的插入顺序无关，稳定可测。
#[must_use]
pub fn driven_components(entity: &SceneEntity) -> Vec<&'static str> {
    builtin_registry()
        .keys()
        .copied()
        .filter(|component| entity.components.contains_key(*component) && is_schema_driven(component))
        .collect()
}

/// 一个实体的全部通用字段行（按组件、再按 schema 字段声明顺序）。
#[must_use]
pub fn rows_for_entity(entity: &SceneEntity) -> Vec<SchemaFieldRow> {
    driven_components(entity)
        .into_iter()
        .flat_map(|component| {
            rows_for_component(entity, component).unwrap_or_else(|| {
                // 组件在实体上、schema 里也一定有（`driven_components` 已过滤两者），
                // 所以这里只可能是编程错误；退化为空行集而不是 panic。
                debug_assert!(false, "{component} 被判定为通用组件却没有 schema");
                Vec::new()
            })
        })
        .collect()
}

/// 单个组件的通用字段行。组件没有 schema、有手写面板，或实体没有它时返回 `None`。
#[must_use]
pub fn rows_for_component(entity: &SceneEntity, component: &str) -> Option<Vec<SchemaFieldRow>> {
    if !is_schema_driven(component) {
        return None;
    }
    let payload = entity.components.get(component)?;
    let registry = builtin_registry();
    let schema = registry.get(component)?;
    let mut rows = Vec::with_capacity(schema.fields.len());
    for field in schema.fields {
        let path = PropertyPath::new(component, field.name);
        let text = read_field_text(payload, &path).unwrap_or_default();
        rows.push(SchemaFieldRow {
            component: schema.name,
            field: field.name,
            path: path.to_string(),
            kind: field.kind,
            required: field.required,
            choices: field.choices,
            present: payload_has_field(payload, field.name),
            text,
            hint: format!(
                "{}{}",
                field.describe(),
                field_unit(component, field.name).label()
            ),
            unit: field_unit(component, field.name),
            choices_source: field_choice_source(field.name),
        });
    }
    Some(rows)
}

/// 组件串里是否有该 key（`read_field_text` 对缺省可选字段返回 `Ok("")`，无法区分
/// "缺省"与"显式空串"，所以这里再按 key 查一次）。
fn payload_has_field(payload: &str, field: &str) -> bool {
    payload.split(';').any(|entry| {
        entry
            .split_once('=')
            .is_some_and(|(key, _)| key.trim() == field)
    })
}

/// 算出一次字段编辑后的**新组件串**，不修改场景、不产生历史。
///
/// 返回 `Ok(None)` 表示"取值没有变化"（调用方必须据此不产生历史条目——
/// 这是"整串覆盖 vs 逐字段覆盖"之外的第二条等价性来源：幂等提交不污染 Undo 栈）。
pub fn edit_component_field(
    scene: &Scene,
    entity: SceneEntityId,
    component: &str,
    field: &str,
    raw_value: &str,
) -> Result<Option<String>, SchemaFieldError> {
    let Some(instance) = scene.entities.get(&entity) else {
        return Err(SchemaFieldError::EntityMissing(entity.0));
    };
    let Some(payload) = instance.components.get(component) else {
        return Err(SchemaFieldError::ComponentMissing {
            entity: entity.0,
            component: component.to_owned(),
        });
    };
    let registry = builtin_registry();
    // 先确认**现有组件串**能被 schema 解析：`write_field` 内部要整串解析来比较当前值，
    // 缺必填字段的旧串会在这里以"另一个字段非法"的形式冒出来，必须先翻译成人话。
    if let Some(schema) = registry.get(component)
        && let Err(error) = schema.parse(payload)
    {
        return Err(SchemaFieldError::PayloadUnparsable {
            component: component.to_owned(),
            field: error.field,
            reason: error.reason,
        });
    }
    let path = PropertyPath::new(component, field);
    let updated =
        write_field(&registry, payload, &path, raw_value).map_err(map_property_error)?;
    if updated == *payload {
        return Ok(None);
    }
    Ok(Some(updated))
}

/// 一次编辑的预览结果：给"Mixed Value / 批量改 1000 对象"这类后续能力留出的返回形状。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldEdit {
    /// 取值有变化，携带新组件串与格式化后的路径。
    Changed { path: String, payload: String },
    /// 取值没变（含"输入与当前等价但格式不同"，如 `1.0` vs `1.000000`）。
    Unchanged,
}

/// `edit_component_field` 的便捷包装：把结果整理成 [`FieldEdit`]。
pub fn preview_field_edit(
    scene: &Scene,
    entity: SceneEntityId,
    component: &str,
    field: &str,
    raw_value: &str,
) -> Result<FieldEdit, SchemaFieldError> {
    let path = PropertyPath::new(component, field).to_string();
    match edit_component_field(scene, entity, component, field, raw_value)? {
        Some(payload) => Ok(FieldEdit::Changed { path, payload }),
        None => Ok(FieldEdit::Unchanged),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::scene::SceneEntity;
    use std::collections::BTreeMap;

    fn entity_with(components: &[(&str, &str)]) -> SceneEntity {
        let mut entity = SceneEntity {
            id: SceneEntityId(1),
            name: "E".to_owned(),
            parent: None,
            children: Vec::new(),
            components: BTreeMap::new(),
            resources: BTreeMap::new(),
            metadata: BTreeMap::new(),
        };
        for (name, payload) in components {
            entity
                .components
                .insert((*name).to_owned(), (*payload).to_owned());
        }
        entity
    }

    fn scene_with(components: &[(&str, &str)]) -> Scene {
        let mut scene = Scene::new("s");
        scene.add_entity(SceneEntityId(1), "E", None).unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        for (name, payload) in components {
            entity
                .components
                .insert((*name).to_owned(), (*payload).to_owned());
        }
        scene
    }

    #[test]
    fn handwritten_panels_are_disjoint_from_the_generic_editor() {
        let registry = builtin_registry();
        for handwritten in HANDWRITTEN_PANEL_COMPONENTS {
            // 真正的不变量：有手写面板的组件**绝不**被通用编辑器接管，
            // 否则同一个字段会有两个编辑器互相覆盖。
            assert!(
                !is_schema_driven(handwritten),
                "{handwritten} 已有手写面板，不应由通用编辑器接管"
            );
        }
        // 没有手写面板的 schema 组件**自动**进入通用编辑器：新增组件只需注册 schema，
        // 不用改 match，也不用改这份名单（这就是"新增组件不改巨型 match"的可检形式）。
        let driven: Vec<&str> = registry
            .keys()
            .copied()
            .filter(|component| is_schema_driven(component))
            .collect();
        assert_eq!(
            driven,
            vec![
                "Camera2D",
                // 从手写面板迁移过来的组件（删面板 → 进这里）：Light 20 轮、MeshRenderer 21 轮、
                // Image 22 轮。
                "Image",
                "Light",
                "MeshRenderer",
                "Particle",
                "Quad",
                "RenderMode",
                "Visibility",
                "WorldText"
            ]
        );
        // 有手写面板但**还没有 schema** 的组件（docs/55 §7 的已知调度缺口）：
        // 这张清单钉在这里，补齐一个 schema 就会红一次，逼着顺手更新文档。
        let without_schema: Vec<&str> = HANDWRITTEN_PANEL_COMPONENTS
            .iter()
            .copied()
            .filter(|component| !registry.contains_key(component))
            .collect();
        assert_eq!(
            without_schema,
            vec![
                "Collider3d",
                "Tilemap",
                "Animation",
                "Container",
                "UiEvents",
                "UiBindings",
                "ButtonStates",
                "UiAnimation"
            ]
        );
    }

    #[test]
    fn rows_keep_raw_text_and_report_missing_optional_fields() {
        let entity = entity_with(&[
            ("Quad", "size=64,32;color=1,1,1,1"),
            ("Visibility", "layers=0x3"),
        ]);
        let rows = rows_for_entity(&entity);
        let size = rows
            .iter()
            .find(|row| row.path == "Quad.size")
            .expect("Quad.size 行");
        assert_eq!(size.text, "64,32", "原始文本必须原样读回（不回写格式）");
        assert!(size.present);
        // Visibility 的位掩码用 `0x` 形式：通用编辑器读的是原文，不做二次格式化。
        let layers = rows
            .iter()
            .find(|row| row.path == "Visibility.layers")
            .unwrap();
        assert_eq!(layers.text, "0x3");
        // 缺省的可选字段：present=false、text 为空 → 视图显示"未设置"，不显示空串。
        let entity = entity_with(&[("Quad", "size=8,8")]);
        let rows = rows_for_entity(&entity);
        let optional = rows
            .iter()
            .find(|row| row.path == "Quad.layer")
            .expect("Quad.layer 行");
        if !optional.required {
            assert!(!optional.present);
            assert!(optional.is_unset());
        }
    }

    #[test]
    fn driven_components_follow_the_declared_order_and_ignore_unknown_ones() {
        let entity = entity_with(&[
            ("Particle", "count=10;lifetime=1"),
            ("Transform", "x=1;y=2"),
            ("Camera2D", "zoom=1"),
        ]);
        assert_eq!(driven_components(&entity), vec!["Camera2D", "Particle"]);
        assert!(rows_for_component(&entity, "Transform").is_none());
        assert!(rows_for_component(&entity, "Nope").is_none());
    }

    #[test]
    fn editing_writes_one_field_and_leaves_the_rest_byte_identical() {
        let scene = scene_with(&[("Quad", "size=64,32;color=1,1,1,1")]);
        let updated = edit_component_field(&scene, SceneEntityId(1), "Quad", "size", "128,64")
            .expect("edits")
            .expect("changed");
        // 数字按**字段类型**归一化为 `{:.6}`（`FieldValue::to_component_value`）：
        // 只有被编辑的那个字段的文本变了，其余字段逐字节保持原样。
        assert_eq!(updated, "size=128.000000,64.000000;color=1,1,1,1");
        // 归一化后的文本再提交同样的取值：判定为"无变化"（幂等），历史不会持续增长。
        let normalized = scene_with(&[("Quad", "size=128.000000,64.000000;color=1,1,1,1")]);
        assert_eq!(
            edit_component_field(&normalized, SceneEntityId(1), "Quad", "size", "128,64")
                .expect("ok"),
            None
        );
    }

    #[test]
    fn unchanged_value_is_reported_as_no_change() {
        let scene = scene_with(&[("Quad", "size=64,32")]);
        // 逐字符相同
        assert_eq!(
            edit_component_field(&scene, SceneEntityId(1), "Quad", "size", "64,32").expect("ok"),
            None
        );
        // 类型等价但格式不同（`1` vs `1.000000`）：`write_field` 做类型归一比较，
        // 因此这里也必须是"无变化"，否则每次失焦提交都会多出一条历史。
        let scene2 = scene_with(&[("Camera2D", "center=0,0;zoom=1.000000")]);
        assert_eq!(
            edit_component_field(&scene2, SceneEntityId(1), "Camera2D", "zoom", "1").expect("ok"),
            None
        );
    }

    /// 现有组件串本身不满足 schema 时，报错必须指向**整串**问题，而不是用户正在改的字段。
    ///
    /// `write_field` 要先整串解析才能比较当前值，因此缺必填字段的旧串原本会以
    /// "字段 center 取值非法：required field is missing" 冒出来——用户改的是 `zoom`，
    /// 这条消息会把人带到错误的字段上。
    #[test]
    fn unparsable_payload_is_reported_as_a_payload_problem() {
        let scene = scene_with(&[("Camera2D", "zoom=1")]);
        match edit_component_field(&scene, SceneEntityId(1), "Camera2D", "zoom", "2") {
            Err(SchemaFieldError::PayloadUnparsable {
                component,
                field,
                reason,
            }) => {
                assert_eq!(component, "Camera2D");
                assert_eq!(field, "center");
                assert!(reason.contains("required"), "{reason}");
            }
            other => panic!("expected PayloadUnparsable, got {other:?}"),
        }
    }

    #[test]
    fn invalid_value_unknown_field_and_missing_targets_are_rejected() {
        let scene = scene_with(&[("Quad", "size=64,32")]);
        match edit_component_field(&scene, SceneEntityId(1), "Quad", "size", "64") {
            Err(SchemaFieldError::InvalidValue { field, .. }) => assert_eq!(field, "size"),
            other => panic!("expected InvalidValue, got {other:?}"),
        }
        match edit_component_field(&scene, SceneEntityId(1), "Quad", "nope", "1") {
            Err(SchemaFieldError::UnknownField { field, .. }) => assert_eq!(field, "nope"),
            other => panic!("expected UnknownField, got {other:?}"),
        }
        match edit_component_field(&scene, SceneEntityId(9), "Quad", "size", "1x1") {
            Err(SchemaFieldError::EntityMissing(9)) => {}
            other => panic!("expected EntityMissing, got {other:?}"),
        }
        match edit_component_field(&scene, SceneEntityId(1), "Sprite", "order", "1") {
            Err(SchemaFieldError::ComponentMissing { component, .. }) => {
                assert_eq!(component, "Sprite")
            }
            other => panic!("expected ComponentMissing, got {other:?}"),
        }
    }

    #[test]
    fn preview_wraps_the_edit_for_view_consumption() {
        let scene = scene_with(&[("Quad", "size=64,32")]);
        assert_eq!(
            preview_field_edit(&scene, SceneEntityId(1), "Quad", "size", "8,8").expect("ok"),
            FieldEdit::Changed {
                path: "Quad.size".to_owned(),
                payload: "size=8.000000,8.000000".to_owned()
            }
        );
        assert_eq!(
            preview_field_edit(&scene, SceneEntityId(1), "Quad", "size", "64,32").expect("ok"),
            FieldEdit::Unchanged
        );
    }

    /// 六个接管组件**同时在场**时的端到端证据：保存 → 重新打开后字段仍能原样读出，
    /// 且"把读到的原文再提交一次"必须是无变化（幂等）——这条同时守住"场景格式不变"。
    ///
    /// 诚实边界：仓库里的示例工程 `examples/game_jhyt` **没有任何**这六个组件
    /// （`Camera2D/Quad/RenderMode/Visibility/EntityScript/Particle` 全为 0 次命中），
    /// 所以真实工程证据不存在，这里用引擎自己的 `Scene::save`/`Scene::load` 做往返。
    #[test]
    fn driven_components_survive_a_scene_save_load_round_trip() {
        let root = std::env::temp_dir().join(format!("p2-schema-fields-{}", crate::test_token()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let path = root.join("roundtrip.scene");
        let mut scene = Scene::new("roundtrip");
        scene.add_entity(SceneEntityId(1), "E", None).unwrap();
        {
            let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
            entity
                .components
                .insert("Camera2D".to_owned(), "center=0,0;zoom=1.5".to_owned());
            entity
                .components
                .insert("Quad".to_owned(), "size=64,32;color=1,1,1,1".to_owned());
            entity
                .components
                .insert("RenderMode".to_owned(), "mode=overlay".to_owned());
            entity
                .components
                .insert("Visibility".to_owned(), "layers=0x3".to_owned());
            entity.components.insert(
                "Script".to_owned(),
                "file=scripts/a.lua;enabled=false".to_owned(),
            );
            entity.components.insert(
                "Particle".to_owned(),
                "count=8;speed=20;life=1;size=2;color=1,1,1,1".to_owned(),
            );
        }
        scene.save(&path).expect("scene saves");
        let loaded = Scene::load(&path).expect("scene reloads");
        let entity = &loaded.entities[&SceneEntityId(1)];
        assert_eq!(
            driven_components(entity),
            vec!["Camera2D", "Particle", "Quad", "RenderMode", "Visibility"],
            "通用编辑器接管的组件（顺序来自注册表，稳定）；`Script` 有专用输入，不算在内"
        );
        // `Script`（实体级脚本）虽然不由通用编辑器接管，但它必须能**按运行时格式**读回：
        // 这条同时覆盖了"三个名字收敛成一个"的修复。
        let script =
            engine::core::entity_script::EntityScript::from_component_str(&entity.components["Script"])
                .expect("运行时解析器必须认得保存下来的 Script");
        assert_eq!(script.file, "scripts/a.lua");
        assert!(!script.enabled);
        let rows = rows_for_entity(entity);
        assert!(!rows.is_empty());
        for row in &rows {
            if !row.present {
                // 可选字段可以合法缺省（`Camera2D.rotation` 等）：缺省就没有"原文"可提交。
                assert!(
                    !row.required,
                    "{}.{} 是必填字段，保存/加载后必须仍然在场",
                    row.component, row.field
                );
                continue;
            }
            // 每个在场字段读得到原文，且原文再提交一次是"无变化"（不产生历史、不改文本）。
            assert_eq!(
                edit_component_field(
                    &loaded,
                    SceneEntityId(1),
                    row.component,
                    row.field,
                    &row.text
                )
                .unwrap_or_else(|error| panic!("{}.{}: {error}", row.component, row.field)),
                None,
                "{}.{} 的原文再提交一次必须是无变化",
                row.component,
                row.field
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
