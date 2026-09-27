//! Schema 驱动的通用组件字段（docs/55 阶段 2「组件族 Inspector / Schema 驱动」）。
//!
//! 目标是计划里那条硬要求：**新增组件不再修改巨型 match**。带手写面板的组件保持现状，
//! 而"有 schema、没有手写面板"的组件由这里统一生成
//! 「`Component.field` 路径 + 当前原始文本 + 输入种类 + 可选值」的行；视图只负责画输入框，
//! 并把用户输入当作 typed action 发出去（提交边界、Undo、Play 锁定都在 shell 的 dispatch 里）。
//!
//! 接管集合是**推导**的（注册了 schema 且不在 [`HANDWRITTEN_PANEL_COMPONENTS`] 里），
//! 所以给引擎新增组件 + 注册 schema 之后，编辑器自动获得它的字段编辑器。

use engine::core::camera::Projection;
use engine::core::component_schema::{FieldKind, builtin_registry};
use engine::core::math3d::Vec3;
use engine::core::property_path::{
    PropertyError, PropertyPath, read_field_display_text, write_field,
};
use engine::core::scene::{Scene, SceneEntity, SceneEntityId};
use engine::core::transform::Transform3D;
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
    // 注意：`Light`(20) / `MeshRenderer`(21) / `Image`(22) / `WorldText`(23) / `Camera`(24) /
    // `Text`(25) / `UiLayout`(31) / `Sprite`(34) / `Transform`(36) **都不在**
    // 这张表里——它们的手写面板与整套缓冲管线已删除，改由通用编辑器按 schema 生成字段行
    // （见 `panels/schema_inspector.rs` 的 `field_widget`）。
    // 这就是"组件迁移到 schema"的样板：删面板 + 删缓冲管线、从本表移除即可。
    "Collider3d",
    "Tilemap",
    "Animation",
    "Container",
    "UiEvents",
    "UiBindings",
    "ButtonStates",
    "UiAnimation",
    // 实体级脚本：Inspector 有专门的"Lua 路径"输入（含工程内相对路径校验），
    // 不走通用字段行——但它写的**名字与载荷**必须是运行时那份（`Script` + `file=…`）。
    "Script",
];

/// 该组件是否由通用（schema 驱动）编辑器接管：有 schema 且没有手写面板。
#[must_use]
pub fn is_schema_driven(component: &str) -> bool {
    !HANDWRITTEN_PANEL_COMPONENTS.contains(&component) && builtin_registry().contains_key(component)
}

/// 字段的**显示单位**：编辑器显示与输入用什么单位，以及组件串里存的是什么。
///
/// 为什么放在编辑器而不是引擎 schema：schema 描述的是"字符串契约"，而"人看着是多少度"
/// 是界面问题；把显示单位塞进引擎会让审计/序列化也背上界面语义。将来 SDK 需要插件声明
/// 自定义显示时，再把这张表升级成 schema 元数据也不迟。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldUnit {
    /// 原样显示（与组件串同一个数）。
    Raw,
    /// 组件串里**就是角度**（`Camera3D.fov`：引擎编码器写 `fov_y.to_degrees()`、
    /// 解析器读进来再 `to_radians()`）：显示与存储一致，单位只进提示文本、不做换算。
    Degrees,
    /// 组件串里是**弧度**、编辑器按角度显示与输入（`to_degrees`/`to_radians` 换算）。
    Radians,
}

impl FieldUnit {
    /// 存储值 → 显示值。
    #[must_use]
    pub fn to_display(self, stored: f64) -> f64 {
        match self {
            // `Degrees` 也在这里：**组件串里存的就是角度**，没有任何换算。
            // （这一条曾经被写成"组件串存弧度"，如果照那样迁移 `Camera3D`，手写面板里的
            // 60° 会在通用编辑器里显示成 3437.75°——见 `field_units_agree_with_the_engine_encoder`。）
            Self::Raw | Self::Degrees => stored,
            Self::Radians => stored.to_degrees(),
        }
    }

    /// 显示值 → 存储值。
    #[must_use]
    pub fn from_display(self, shown: f64) -> f64 {
        match self {
            Self::Raw | Self::Degrees => shown,
            Self::Radians => shown.to_radians(),
        }
    }

    /// 提示后缀（挂在 schema 的 hint 后面，让用户知道单位）。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Raw => "",
            Self::Degrees => "（单位：角度）",
            Self::Radians => "（编辑器用角度，组件串存弧度）",
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
    /// **引擎已知的排序层**：`sorting_layer`/`layer` 字段用下拉给出 6 个已知层名。
    ///
    /// 为什么用下拉而不是文本框：旧手写 Sprite 面板就是下拉（`background/world/default/
    /// actor/foreground/ui`）；schema 侧仍保持 `Text`（引擎的 `sorting_layer_rank` 接受自定义层名
    /// 并回退到 default），因此**审计口径不变**，下拉只是编辑器的候选来源。
    SortingLayers,
}

/// 以**角度**为单位的字段（组件, 字段）。
///
/// `Camera3D.fov` 的组件串里存的就是角度：`Camera3D::from_component_str` 读进来时
/// `to_radians()`、`Camera3D::to_component_str` 写出去时 `to_degrees()`（见 `core/camera.rs`）。
/// 这条事实由 `field_units_agree_with_the_engine_encoder` 直接拿引擎编码器对拍：
/// 编辑器显示的角度必须等于引擎解析出来的 `fov_y` 换算成的角度。
const DEGREE_FIELDS: &[(&str, &str)] = &[("Camera", "fov")];

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

/// 候选值来自**排序层**的字段名（`Sprite.sorting_layer` / `Quad.layer`）。
const SORTING_LAYER_FIELDS: &[&str] = &["sorting_layer", "layer"];

/// 引擎已知的排序层（与旧手写 Sprite 面板的下拉一致）。
pub const SORTING_LAYERS: &[&str] = &[
    "background",
    "world",
    "default",
    "actor",
    "foreground",
    "ui",
];

/// 该字段的候选值来源。
#[must_use]
pub fn field_choice_source(field: &str) -> ChoiceSource {
    if PROJECT_FONT_FIELDS.contains(&field) {
        ChoiceSource::ProjectFonts
    } else if SORTING_LAYER_FIELDS.contains(&field) {
        ChoiceSource::SortingLayers
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
    /// **多选混合值**：其它选中实体的同一字段取值不同（视图显示"混合"，提交仍写全部目标）。
    pub mixed: bool,
    /// 该字段是**资源引用**（带 `ProjectPath`/`OptionalProjectPath` 规则）：视图据此做
    /// "工程里找不到这个资源"的**上下文诊断**（规则本身是纯字段校验，查不了资源库）。
    pub path_rule: bool,
    /// 引用的资源在当前工程的资源索引里**找不到**（视图填充；只是诊断，不拒绝写入——
    /// 作者可能先写引用、后补资源）。
    pub missing_asset: bool,
    /// 资源引用的**扩展名白名单**（空 = 不限制）：视图的"选择资源"下拉用它筛候选
    /// （与 `FieldRule::ProjectPath` 的扩展名口径同一份事实源）。
    pub path_extensions: &'static [&'static str],
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
    /// **引擎解析器读不了**这次编辑产生的新串：schema 的字段契约与声明式规则都通过了，
    /// 但不变量只有引擎解析器说了算（相机 `near < far`、look-at 非退化、缩放非零……）。
    EngineRejected { component: String, reason: String },
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
            Self::EngineRejected { component, reason } => write!(
                f,
                "组件 {component} 的新取值引擎无法解析：{reason}\
                 （schema 表达不了的运行时约束由引擎解析器拦下）"
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
        .filter(|component| {
            entity.components.contains_key(*component) && is_schema_driven(component)
        })
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
        // **显示文本**：自由文本字段（`Text.text`/`font`）在这里已经反转义，因此输入框里
        // 显示的是真实文字；提交时由 `write_field` 重新转义（两个方向只有 schema 一处实现）。
        let text = read_field_display_text(&registry, payload, &path).unwrap_or_default();
        let choices_source = field_choice_source(field.name);
        // 排序层的候选来自引擎已知层名（schema 侧仍是 Text，审计口径不变）。
        let choices = if choices_source == ChoiceSource::SortingLayers {
            SORTING_LAYERS
        } else {
            field.choices
        };
        rows.push(SchemaFieldRow {
            component: schema.name,
            field: field.name,
            path: path.to_string(),
            kind: field.kind,
            required: field.required,
            choices,
            present: payload_has_field(payload, field.name),
            text,
            hint: format!(
                "{}{}",
                field.describe(),
                field_unit(component, field.name).label()
            ),
            unit: field_unit(component, field.name),
            choices_source,
            mixed: false,
            path_rule: field.rules.iter().any(|rule| {
                matches!(
                    rule,
                    engine::core::component_schema::FieldRule::ProjectPath { .. }
                        | engine::core::component_schema::FieldRule::OptionalProjectPath { .. }
                )
            }),
            missing_asset: false,
            path_extensions: path_extensions_of(field.rules),
        });
    }
    Some(rows)
}

/// **多选**的通用字段行：以第一个实体（主选中）的取值为准，`mixed=true` 表示其它选中实体的
/// 同一字段取值不同。
///
/// "哪些实体会被写"由调用方决定（与旧手写面板一致：只写**确实有这个组件**的选中实体），
/// 因此缺少组件的实体不参与 mixed 判定。
#[must_use]
pub fn rows_for_selection(entities: &[&SceneEntity]) -> Vec<SchemaFieldRow> {
    let Some(primary) = entities.first() else {
        return Vec::new();
    };
    let registry = builtin_registry();
    let mut rows = rows_for_entity(primary);
    for row in &mut rows {
        row.mixed = entities.iter().skip(1).any(|entity| {
            entity
                .components
                .get(row.component)
                .and_then(|payload| {
                    read_field_display_text(
                        &registry,
                        payload,
                        &PropertyPath::new(row.component, row.field),
                    )
                    .ok()
                })
                .is_some_and(|text| text != row.text)
        });
    }
    rows
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

/// 组件级的**只读说明**：迁移后的组件里那些"不是字段"的信息（相机的投影摘要与生效姿态）。
///
/// 视图只呈现、不编辑。`warning` 为真表示"组件参数与渲染实际行为不一致"
/// （如 `pose=transform` 但实体没有 `Transform`），需要用醒目颜色显示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaComponentNote {
    pub text: String,
    pub warning: bool,
}

impl SchemaComponentNote {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            warning: false,
        }
    }

    fn warning(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            warning: true,
        }
    }
}

/// 只读说明的提供者：给定实体，返回这个组件的说明行。
type NoteProvider = fn(&SceneEntity) -> Vec<SchemaComponentNote>;

/// 只读说明的**提供者表**：把"面板里那些不是字段的信息"按组件名登记，通用编辑器照单呈现。
///
/// 这是一张数据表而不是巨型 match，而且**不是**字段编辑器的一部分：字段仍完全由 schema 生成；
/// 只有当一个组件确实有"字段之外的信息"时才在这里加一项（没有就不加）。
const NOTE_PROVIDERS: &[(&str, NoteProvider)] = &[("Camera", camera_notes)];

/// 该组件在当前实体上的只读说明（组件不在实体上 / 没有登记说明时为空）。
#[must_use]
pub fn component_notes(entity: &SceneEntity, component: &str) -> Vec<SchemaComponentNote> {
    NOTE_PROVIDERS
        .iter()
        .find(|(name, _)| *name == component)
        .map_or_else(Vec::new, |(_, provider)| provider(entity))
}

/// **引用必须存在**的上下文诊断（纯函数，便于单测）：把"带路径规则、取值非空、但资源索引里
/// 找不到"的行标成 `missing_asset`。
///
/// 为什么是诊断而不是拒绝：`FieldRule` 是纯字段规则、查不了资源库；而且作者经常会
/// "先写引用、后补资源"。用 `BTreeSet<String>`（而不是 `ResourceBrowserState`）当输入，
/// 是为了让这条逻辑与编辑器状态解耦、可单测。
pub fn mark_missing_assets(
    rows: &mut [SchemaFieldRow],
    assets: &std::collections::BTreeSet<String>,
) {
    if assets.is_empty() {
        // 资源索引还没建立（例如刚打开工程）：不做判断，避免整屏假警告。
        return;
    }
    for row in rows {
        let text = row.text.trim().replace('\\', "/");
        if row.path_rule && !text.is_empty() && !assets.contains(&text) {
            row.missing_asset = true;
        }
    }
}

/// 取出路径规则的扩展名白名单（第一条路径规则为准；没有路径规则就是空 = 不限制）。
fn path_extensions_of(
    rules: &[engine::core::component_schema::FieldRule],
) -> &'static [&'static str] {
    use engine::core::component_schema::FieldRule;
    rules
        .iter()
        .find_map(|rule| match rule {
            FieldRule::ProjectPath { extensions, .. }
            | FieldRule::OptionalProjectPath { extensions, .. } => Some(*extensions),
            FieldRule::NonEmpty | FieldRule::NonNegative => None,
        })
        .unwrap_or(&[])
}

/// **Inspector 字段搜索**（第 42 轮，docs/55 P1）：查询词按空白切分，**每个词**都必须在
/// 该行的"组件名 / 字段名 / 路径 / 当前值"里出现（大小写不敏感的子串），行才保留。
///
/// 为什么按"全部词 AND"：Inspector 的用法几乎都是"把某类字段找出来"
/// （`texture`、`sprite texture`、`camera fov`），AND 是唯一不会越搜越多的语义。
/// 空查询原样返回（视图的默认状态）。
#[must_use]
pub fn filter_field_rows(rows: &[SchemaFieldRow], query: &str) -> Vec<SchemaFieldRow> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return rows.to_vec();
    }
    rows.iter()
        .filter(|row| {
            let haystack = format!(
                "{} {} {} {}",
                row.component.to_lowercase(),
                row.field.to_lowercase(),
                row.path.to_lowercase(),
                row.text.to_lowercase()
            );
            terms.iter().all(|term| haystack.contains(term.as_str()))
        })
        .cloned()
        .collect()
}

/// **引用选择器**的候选：从工程资源路径里筛出符合该字段扩展名白名单的那些
/// （排序、去重；`extensions` 为空 = 全部资源都可选）。
///
/// 纯函数（输入是路径字符串）——选择器是视图，但"哪些资源能选"这条规则必须可单测，
/// 否则它就会悄悄与 `FieldRule::ProjectPath` 的扩展名漂移。
#[must_use]
pub fn picker_candidates(paths: &[String], extensions: &[&str], current: &str) -> Vec<String> {
    let mut candidates: Vec<String> = paths
        .iter()
        .filter(|path| {
            extensions.is_empty()
                || std::path::Path::new(path)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        let extension = extension.to_ascii_lowercase();
                        extensions.iter().any(|allowed| *allowed == extension)
                    })
        })
        .cloned()
        .collect();
    candidates.sort();
    candidates.dedup();
    // 当前值若不在候选里也放进来（否则下拉会显示不出当前选中的东西）。
    let current = current.trim().replace('\\', "/");
    if !current.is_empty() && !candidates.contains(&current) {
        candidates.insert(0, current);
    }
    candidates
}

/// 组件级的**动作**：不是字段，而是"一次改多个字段/需要引擎语义"的整块操作
/// （如 `UiLayout` 的锚点预设——它会同时改 `anchors` 与 `pivot`）。
///
/// 视图把它渲染成下拉/按钮，点击后发**一条** typed action：多字段写入必须原子
/// （一条 Undo），因此走 `EditorCommand::SetComponentFields`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaComponentAction {
    /// 下拉项/按钮的显示名。
    pub label: String,
    /// 该动作当前是否已经生效（下拉的选中态；全部字段与当前值一致时为真）。
    pub selected: bool,
    /// 要写入的字段（顺序即写入顺序）。
    pub fields: Vec<(String, String)>,
}

/// 组件级的**编辑器态开关**（不写文档）：如变换的"锁定比例"（统一后 `Transform` 已撤掉它）。
///
/// 开关本身不是字段，也不进组件串；打开后做**字段镜像**：改 `mirrors` 里的 `from` 字段时，
/// 同一次提交里把 `to` 也写成同一个值（等比锁的语义——迁移前手写面板就是这么做的）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaComponentToggle {
    /// 编辑器态键（按 `实体 + key` 存在 egui 临时数据里，Undo/重开不会污染文档）。
    pub key: &'static str,
    /// 勾选框文案。
    pub label: &'static str,
    /// 默认是否打开。
    pub default: bool,
    /// 打开时的字段镜像：`(改这个字段, 同步这个字段)`。
    pub mirrors: Vec<(String, String)>,
}

/// 编辑器态开关的提供者（不依赖实体，纯静态配置）。
type ToggleProvider = fn() -> Vec<SchemaComponentToggle>;

/// 编辑器态开关的表（与 `NOTE_PROVIDERS`/`ACTION_PROVIDERS` 同一思路）。
///
/// **第 56 轮起是空表**：唯一登记过的"等比锁"随变换统一一起撤掉了——统一后的 `scale` 是
/// `Vec3`，而等比锁的机制是"把这个字段的**整个值**复制到另一个字段"，对向量字段没有可复制对象。
const TOGGLE_PROVIDERS: &[(&str, ToggleProvider)] = &[];

/// 该组件的编辑器态开关。
#[must_use]
pub fn component_toggles(component: &str) -> Vec<SchemaComponentToggle> {
    TOGGLE_PROVIDERS
        .iter()
        .find(|(name, _)| *name == component)
        .map_or_else(Vec::new, |(_, provider)| provider())
}

/// 打开的开关在改 `field` 时要一起写的**镜像字段**（等比锁的展开规则，纯函数）。
///
/// 视图在提交前用它把"单字段提交"展开成"多字段原子提交"（仍然只有一条 Undo）。
#[must_use]
pub fn mirror_fields(
    toggles: &[SchemaComponentToggle],
    field: &str,
    value: &str,
) -> Vec<(String, String)> {
    toggles
        .iter()
        .flat_map(|toggle| toggle.mirrors.iter())
        .filter(|(from, _)| from == field)
        .map(|(_, to)| (to.clone(), value.to_owned()))
        .collect()
}

/// 组件级动作的提供者：给定实体**与组件名**，返回这个组件的动作列表。
///
/// 组件名要传进来：同一个实现可能服务多个组件名，动作必须从**实体上那个名字**的载荷里读当前值。
type ActionProvider = fn(&SceneEntity, &str) -> Vec<SchemaComponentAction>;

/// 组件级动作的**提供者表**（与 `NOTE_PROVIDERS` 同一思路：数据表，不是巨型 match）。
const ACTION_PROVIDERS: &[(&str, ActionProvider)] = &[
    ("UiLayout", ui_layout_preset_actions),
    ("Transform", transform_reset_action),
];

/// 该组件在当前实体上的动作（没有登记/组件不在实体上时为空）。
#[must_use]
pub fn component_actions(entity: &SceneEntity, component: &str) -> Vec<SchemaComponentAction> {
    ACTION_PROVIDERS
        .iter()
        .find(|(name, _)| *name == component)
        .map_or_else(Vec::new, |(_, provider)| provider(entity, component))
}

/// `Transform` 的"Reset"动作：把**引擎默认值**写回去（一次提交 = 一条 Undo）。
///
/// 值来自引擎类型（`Transform3D::identity()`），编辑器不手写字面量。
/// **只认统一形态**：变化后的载荷必须是统一写法，否则动作不提供（写一半的串比没有按钮更糟）。
fn transform_reset_action(entity: &SceneEntity, component: &str) -> Vec<SchemaComponentAction> {
    let Some(payload) = entity.components.get(component) else {
        return Vec::new();
    };
    let Ok(current) = engine::core::transform::Transform3D::from_component_str(payload) else {
        return Vec::new();
    };
    let identity = engine::core::transform::Transform3D::identity();
    if current == identity {
        // 已经等于默认值时不提供动作（视图据此隐藏"重置"）。
        return Vec::new();
    }
    let defaults: Vec<(String, String)> = vec![
        (
            "position".to_owned(),
            format!(
                "{},{},{}",
                identity.position.x, identity.position.y, identity.position.z
            ),
        ),
        (
            "rotation".to_owned(),
            format!(
                "{},{},{},{}",
                identity.rotation.x, identity.rotation.y, identity.rotation.z, identity.rotation.w
            ),
        ),
        (
            "scale".to_owned(),
            format!(
                "{},{},{}",
                identity.scale.x, identity.scale.y, identity.scale.z
            ),
        ),
    ];
    vec![SchemaComponentAction {
        label: "重置为默认值".to_owned(),
        selected: false,
        fields: defaults,
    }]
}

/// `UiLayout` 的锚点预设：与手写面板里的下拉**同一份语义**——预设来自引擎的/// `UiLayoutPreset::ALL`，写入值是引擎自己的编码（`anchors` 四元组 + `pivot` 二元组），
/// 选中态用"字段值与预设一致"判定（不是另存一份"当前预设"状态，因此外部改动也会立刻反映）。
fn ui_layout_preset_actions(entity: &SceneEntity, component: &str) -> Vec<SchemaComponentAction> {
    let Some(payload) = entity.components.get(component) else {
        return Vec::new();
    };
    let Ok(layout) = engine::core::ui_layout::UiRectLayout::from_component_str(payload) else {
        return Vec::new();
    };
    engine::core::ui_layout::UiLayoutPreset::ALL
        .into_iter()
        .map(|preset| {
            let mut applied = layout.clone();
            applied.apply_preset(preset);
            SchemaComponentAction {
                label: preset.label().to_owned(),
                selected: applied.anchors == layout.anchors && applied.pivot == layout.pivot,
                fields: vec![
                    (
                        "anchors".to_owned(),
                        applied
                            .anchors
                            .iter()
                            .map(|value| value.to_string())
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                    (
                        "pivot".to_owned(),
                        applied
                            .pivot
                            .iter()
                            .map(|value| value.to_string())
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                ],
            }
        })
        .collect()
}

/// `Camera` 的只读说明（手写面板里"不是字段"的那部分，随迁移搬到通用编辑器）。
///
/// 统一之后只有**一个**相机组件，说明按**判别字段给出的投影分支**分两套：正交（2D 视口）与
/// 透视（当前这套：`Projection::from_camera3d` 是"这台相机怎么投影"的唯一说法，
/// `Camera3D::with_pose` 与运行时用的是同一条姿态换算）。因此这里显示的不是编辑器的推测，
/// 而是渲染器真正会用的值。
fn camera_notes(entity: &SceneEntity) -> Vec<SchemaComponentNote> {
    let Some(raw) = entity.components.get("Camera") else {
        return Vec::new();
    };
    let camera = match engine::core::camera::Camera::from_component_str(raw) {
        Ok(camera) => camera,
        Err(error) => {
            return vec![SchemaComponentNote::warning(format!(
                "组件串无法解析：{error}"
            ))];
        }
    };
    let mut notes = Vec::new();
    // docs/54 §4.2：`enabled=false` 的相机不参与选帧。必须说明，否则用户会看到一台
    // "参数正常"的相机却完全不参与渲染。
    if !camera.enabled() {
        notes.push(SchemaComponentNote::warning(
            "已禁用（enabled=false）—— 不参与相机选帧，渲染回退到默认相机",
        ));
    }
    let camera = match camera.as_perspective() {
        Some(camera) => camera,
        None => {
            // 正交（2D）相机：字段体是 `center`/`zoom`/`bounds`，说明只有"投到多大"这一条实质信息。
            let orthographic = camera.as_orthographic().expect("已确认不是透视分支");
            notes.push(SchemaComponentNote::info(format!(
                "投影：正交（{:.3} px/单位，中心 ({:.1}, {:.1})）",
                orthographic.zoom, orthographic.center.x, orthographic.center.y
            )));
            return notes;
        }
    };
    // WP5：`pose=transform` 时组件的 position/target 退化为"没有 Transform 时的回退值"，
    // 所以先说清楚这一点，再给出**实际生效**的姿态。
    if camera.pose_from_transform {
        notes.push(SchemaComponentNote::info(
            "位置/目标仅在实体没有 Transform 时生效（pose=transform）",
        ));
    }
    notes.push(SchemaComponentNote::info(
        match Projection::from_camera3d(camera) {
            Projection::Perspective { fov_y, near, far } => format!(
                "投影：透视（垂直 FOV {:.1}°，近 {near:.3}，远 {far:.3}；深度 0..1，前向 -Z）",
                fov_y.to_degrees()
            ),
            Projection::Orthographic { zoom } => format!("投影：正交（{zoom:.3} px/单位）"),
        },
    ));
    if !camera.pose_from_transform {
        return notes;
    }
    notes.push(match entity.components.get("Transform") {
        None => SchemaComponentNote::warning(
            "警告：实体没有 Transform —— 渲染回退到上面的组件姿态",
        ),
        Some(raw) => match Transform3D::from_component_str(raw)
            .map_err(|error| error.to_string())
            .and_then(|pose| camera.with_pose(pose).map_err(|error| error.to_string()))
        {
            Ok(posed) => {
                let forward = (posed.target - posed.position)
                    .normalized()
                    .unwrap_or(Vec3::new(0.0, 0.0, -1.0));
                SchemaComponentNote::info(format!(
                    "生效姿态（来自 Transform）：位置 ({:.3}, {:.3}, {:.3})，视线 -Z → ({:.3}, {:.3}, {:.3})",
                    posed.position.x,
                    posed.position.y,
                    posed.position.z,
                    forward.x,
                    forward.y,
                    forward.z
                ))
            }
            Err(error) => SchemaComponentNote::warning(format!(
                "警告：Transform 不可用（{error}）—— 渲染回退到组件姿态"
            )),
        },
    });
    notes
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
    edit_component_payload_with(&registry, component, payload, field, raw_value)
}

/// 在**组件串**上按字段做一次编辑（不碰场景）：多字段原子编辑与预演共用这条路径。
///
/// 返回 `Ok(None)` = 取值没变（调用方据此不产生历史）。校验顺序与 [`edit_component_field`]
/// 完全一致：先整串 schema 解析（翻译"旧串本身不合法"），再按字段写，最后过引擎解析器兜底门。
pub fn edit_component_payload(
    component: &str,
    payload: &str,
    field: &str,
    raw_value: &str,
) -> Result<Option<String>, SchemaFieldError> {
    let registry = builtin_registry();
    edit_component_payload_with(&registry, component, payload, field, raw_value)
}

fn edit_component_payload_with(
    registry: &std::collections::BTreeMap<
        &'static str,
        engine::core::component_schema::ComponentSchema,
    >,
    component: &str,
    payload: &str,
    field: &str,
    raw_value: &str,
) -> Result<Option<String>, SchemaFieldError> {
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
    let updated = write_field(registry, payload, &path, raw_value).map_err(map_property_error)?;
    if updated == *payload {
        return Ok(None);
    }
    // **引擎解析器兜底门**：schema 管字段契约（类型 + 声明式规则），"运行时读不读得了"
    // 由引擎自己的 typed 解析器说了算。声明式规则表达不了的**跨字段/几何不变量**
    // （相机的 `near < far`、look-at 非退化、Transform 的缩放非零……）就靠这一道拦住——
    // 校验逻辑只有引擎解析器一处定义，编辑器不另写一份，也不会随引擎演进而漂移。
    if let Some(Err(reason)) =
        engine::core::typed_scene::validate_component_payload(component, &updated)
    {
        return Err(SchemaFieldError::EngineRejected {
            component: component.to_owned(),
            reason,
        });
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
    use engine::core::camera::Camera3D;
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
                // 第 39 轮新增的三个：`Animation3D`/`Collider` 是"注册 schema 即自动接管"的
                // 又一次实证（此前它们在编辑器里**没有任何字段行**）；
                // `ParticleEmitter` 是**改名**换来的——注册表原来写的是 `Particle`，
                // 与运行时读的名字不一致，所以真实工程里的粒子组件一直没被接管。
                "Animation3D",
                // 第 57 轮：统一之后的**唯一**相机组件（旧名 `Camera2D`/`Camera3D` 已从注册表删除）。
                "Camera",
                "Collider",
                "Image",
                "Light",
                "MeshRenderer",
                "ParticleEmitter",
                "Quad",
                "RenderMode",
                "Sprite",
                "Text",
                // 唯一的变换组件（旧名 `Transform3D` 已从注册表删除）。
                "Transform",
                "UiLayout",
                "Visibility",
                "WorldText"
            ]
        );
        // 有手写面板但**还没有 schema** 的组件：第 26 轮补齐了
        // `Collider3d`/`Tilemap`/`Container`/`ButtonStates`；第 38 轮补齐了
        // `Animation`/`UiEvents`/`UiBindings`（此前卡在"位置式段/重复键/动态键"上，
        // 由引擎的 `ListModel` 解决）；第 40 轮补齐了最后一个 `UiAnimation`
        // （先把 `track.<属性>=…` 这份格式搬进引擎 `ui_animation.rs`，schema 才有事实源可对拍）。
        // **这张清单现在是空的**——所有有手写面板的组件都同时有 typed schema，
        // 只是面板不动（同一字段不能有两个编辑器）。
        let without_schema: Vec<&str> = HANDWRITTEN_PANEL_COMPONENTS
            .iter()
            .copied()
            .filter(|component| !registry.contains_key(component))
            .collect();
        assert!(
            without_schema.is_empty(),
            "有手写面板却没有 schema 的组件（补一个就会红一次）：{without_schema:?}"
        );
        // 反向：所有有手写面板的组件都在注册表里，且注册表里没有"没人消费"的孤键
        // （后者由引擎侧的 `every_component_id_the_runtime_knows_has_a_schema` 与
        // `script_schema_uses_the_runtime_component_name` 机械化守住）。
        for component in HANDWRITTEN_PANEL_COMPONENTS {
            assert!(
                registry.contains_key(component),
                "{component} 有手写面板，必须同时有 typed schema"
            );
        }
    }

    /// **Inspector 字段搜索**（第 42 轮）：多词 AND、大小写不敏感、匹配组件名/字段名/路径/当前值；
    /// 空查询原样返回。
    #[test]
    fn field_search_is_and_over_component_field_path_and_value() {
        let entity = entity_with(&[
            (
                "Sprite",
                "texture=assets/textures/player.png;size=1x1;tint=1,1,1,1;sorting_layer=default",
            ),
            ("MeshRenderer", "mesh=assets/models/nope.glb"),
            ("Light", "direction=0,1,0;color=1,1,1;intensity=1"),
        ]);
        let rows = rows_for_entity(&entity);

        // 空查询 = 全部行（视图的默认状态）。
        assert_eq!(filter_field_rows(&rows, "").len(), rows.len());
        assert_eq!(filter_field_rows(&rows, "   ").len(), rows.len());

        // 单词：字段名子串（`texture` 同时命中 Sprite.texture 与 Physics 之外的任何 texture 字段）。
        let texture = filter_field_rows(&rows, "texture");
        assert!(!texture.is_empty());
        assert!(texture.iter().all(|row| row.field.contains("texture")));

        // 多词 AND：`sprite texture` 只留 Sprite.texture；调换顺序结果相同。
        let sprite_texture = filter_field_rows(&rows, "sprite texture");
        assert_eq!(
            sprite_texture
                .iter()
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Sprite.texture"]
        );
        assert_eq!(
            filter_field_rows(&rows, "TEXTURE Sprite").len(),
            sprite_texture.len(),
            "大小写与词序都不影响结果"
        );

        // 命中**当前值**：按资源路径搜索能定位到引用它的字段。
        let by_value = filter_field_rows(&rows, "player.png");
        assert_eq!(
            by_value
                .iter()
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Sprite.texture"]
        );

        // 不存在的词 → 空；AND 里有一个词不命中 → 空。
        // （注意 `nope` 这种词会命中 `MeshRenderer.mesh` 的**值** `assets/models/nope.glb`——
        // 搜索覆盖当前值，所以这里用一个任何字段都不含的词。）
        assert!(filter_field_rows(&rows, "zzz-nothing").is_empty());
        assert!(filter_field_rows(&rows, "sprite zzz-nothing").is_empty());
    }

    /// **引用选择器的候选**（第 42 轮）：扩展名白名单过滤 + 排序 + 当前值兜底。
    #[test]
    fn picker_candidates_follow_the_extension_whitelist() {
        let paths = vec![
            "assets/textures/b.png".to_owned(),
            "assets/textures/a.png".to_owned(),
            "assets/models/hero.glb".to_owned(),
            "assets/audio/click.ogg".to_owned(),
            "assets/textures/photo.JPG".to_owned(),
            "assets/textures/a.png".to_owned(),
        ];
        // 纹理字段：只要图片扩展名（大小写不敏感），排序去重。
        assert_eq!(
            picker_candidates(&paths, &["png", "jpg", "jpeg", "webp"], ""),
            vec![
                "assets/textures/a.png".to_owned(),
                "assets/textures/b.png".to_owned(),
                "assets/textures/photo.JPG".to_owned(),
            ]
        );
        // 空白名单 = 不限制（例如 `Script.file` 之外的自定义路径字段）。
        assert_eq!(picker_candidates(&paths, &[], "").len(), 5);
        // 当前值不在候选里也要放进来（否则下拉显示不出当前选中的东西）；
        // 反斜杠写法按组件串的约定归一成正斜杠。
        let with_current = picker_candidates(&paths, &["png"], "assets\\textures\\missing.png");
        assert_eq!(with_current[0], "assets/textures/missing.png");
        assert_eq!(with_current.len(), 3, "两个 png 候选 + 兜底的当前值");
        // 当前值已经在候选里就不重复插入。
        assert_eq!(
            picker_candidates(&paths, &["png"], "assets/textures/a.png").len(),
            2
        );
    }

    /// **引用必须存在**（第 37 轮）：路径规则的字段在资源索引里找不到时标诊断，
    /// 但**不拒绝写入**（作者可以先写引用、后补资源）。
    #[test]
    fn missing_asset_paths_are_flagged_not_rejected() {
        let entity = entity_with(&[
            (
                "Sprite",
                "texture=assets/textures/player.png;size=1x1;tint=1,1,1,1;sorting_layer=default",
            ),
            ("MeshRenderer", "mesh=assets/models/nope.glb"),
            ("Light", "direction=0,1,0;color=1,1,1;intensity=1"),
        ]);
        let mut rows = rows_for_entity(&entity);
        // 带路径规则的字段：Sprite.texture / MeshRenderer.mesh；颜色字段不是。
        assert!(
            rows.iter()
                .any(|row| row.path == "Sprite.texture" && row.path_rule)
        );
        assert!(
            rows.iter()
                .any(|row| row.path == "MeshRenderer.mesh" && row.path_rule)
        );
        assert!(
            rows.iter()
                .any(|row| row.path == "Light.color" && !row.path_rule)
        );
        // 资源索引为空 = 还没建立 → 不做判断（不假警告）。
        mark_missing_assets(&mut rows, &std::collections::BTreeSet::new());
        assert!(rows.iter().all(|row| !row.missing_asset), "空索引不判缺失");
        // 索引里有 player.png：它不算缺失；nope.glb 算缺失。
        let assets: std::collections::BTreeSet<String> = ["assets/textures/player.png".to_owned()]
            .into_iter()
            .collect();
        mark_missing_assets(&mut rows, &assets);
        let missing: Vec<&str> = rows
            .iter()
            .filter(|row| row.missing_asset)
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(missing, vec!["MeshRenderer.mesh"], "{missing:?}");
        // 写入仍然被允许（诊断不是拒绝）：缺失的引用照样能改。
        let scene = scene_with(&[("MeshRenderer", "mesh=assets/models/nope.glb")]);
        assert!(
            edit_component_field(
                &scene,
                SceneEntityId(1),
                "MeshRenderer",
                "mesh",
                "assets/models/other.glb"
            )
            .is_ok(),
            "缺失资源的引用仍可编辑（先写引用、后补资源）"
        );
    }

    /// **编辑器态开关 + 重置动作**（第 35 轮，为 `Transform`/`Transform3D` 迁移铺路）：
    /// 等比锁是"编辑器态 + 字段镜像"（不写文档），重置是"写回引擎默认值"的动作。
    #[test]
    fn transform_toggles_and_reset_actions_come_from_the_engine_types() {
        // 第 56 轮：变换统一成单一 `Transform`（`scale` 是 `Vec3`），**等比锁开关随之撤掉**——
        // 旧机制是"复制整个字段值"，对向量字段没有可复制对象。这条断言把"它是空表"钉住，
        // 免得有人又加回一个点了没反应的开关。
        assert!(
            component_toggles("Transform").is_empty(),
            "统一后的 Transform 不再有等比锁开关（见 ACTION_PROVIDERS 的说明）"
        );
        assert!(component_toggles("Light").is_empty());
        assert!(
            mirror_fields(&[], "scale", "1,1,1").is_empty(),
            "没开锁就不镜像"
        );

        // 重置动作：字段值来自引擎的 `identity()`。
        let entity = entity_with(&[(
            "Transform",
            "position=3.000000000,4.000000000,0.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=2.000000000,2.000000000,1.000000000",
        )]);
        let reset = component_actions(&entity, "Transform");
        assert_eq!(reset.len(), 1);
        let fields: Vec<(&str, &str)> = reset[0]
            .fields
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("position", "0,0,0"),
                ("rotation", "0,0,0,1"),
                ("scale", "1,1,1"),
            ]
        );
        // 写回去之后必须真的等于引擎的单位变换。
        let mut payload = entity.components["Transform"].clone();
        for (field, value) in &reset[0].fields {
            payload = edit_component_payload("Transform", &payload, field, value)
                .expect("重置写入不该失败")
                .unwrap_or(payload);
        }
        assert_eq!(
            engine::core::transform::Transform3D::from_component_str(&payload).unwrap(),
            engine::core::transform::Transform3D::identity()
        );
        // 已经是单位变换 → 没有"重置"动作可点。
        let identity = entity_with(&[(
            "Transform",
            engine::core::transform::Transform3D::identity()
                .to_component_str()
                .as_str(),
        )]);
        assert!(component_actions(&identity, "Transform").is_empty());

        // 畸形的载荷同样不给动作（宁可没有按钮，也不写出更坏的串）。
        let broken = entity_with(&[("Transform", "position=oops")]);
        assert!(component_actions(&broken, "Transform").is_empty());
    }

    /// **组件级动作**（第 30 轮）：`UiLayout` 的 12 个锚点预设来自引擎的
    /// `UiLayoutPreset::ALL`，写的是引擎编码的 `anchors` + `pivot` 两个字段，
    /// 选中态由"字段值与预设一致"判定（外部改动也会立刻反映，不另存一份状态）。
    #[test]
    fn component_actions_come_from_the_engine_presets() {
        let entity = entity_with(&[(
            "UiLayout",
            "anchors=0,0,0,0;offsets=0,0,0,0;min_size=0,0;max_size=100,100;pivot=0.5,0.5",
        )]);
        let actions = component_actions(&entity, "UiLayout");
        assert_eq!(
            actions.len(),
            engine::core::ui_layout::UiLayoutPreset::ALL.len(),
            "动作列表必须来自引擎的预设表（12 个）"
        );
        // 每个动作都写 anchors + pivot 两个字段（预设会同时改它们）。
        for action in &actions {
            let names: Vec<&str> = action
                .fields
                .iter()
                .map(|(name, _)| name.as_str())
                .collect();
            assert_eq!(names, vec!["anchors", "pivot"], "{}", action.label);
        }
        // 默认布局（anchors=0,0,0,0 + pivot=0.5,0.5）**不匹配**任何预设 → 视图显示"自定义"。
        assert!(
            actions.iter().all(|action| !action.selected),
            "默认布局不该匹配某个预设"
        );
        // 把某个预设写回后，**恰好那一个**变成选中态（选中态由字段值推导，不另存状态）。
        let full_rect = actions
            .iter()
            .find(|action| {
                action.label == engine::core::ui_layout::UiLayoutPreset::FullRect.label()
            })
            .expect("FullRect 动作");
        let mut payload = entity.components["UiLayout"].clone();
        for (field, value) in &full_rect.fields {
            payload = edit_component_payload("UiLayout", &payload, field, value)
                .expect("字段写入不该失败")
                .unwrap_or(payload);
        }
        // 写回后布局必须真的等于该预设（写的是引擎语义，不是文本猜的）。
        let layout = engine::core::ui_layout::UiRectLayout::from_component_str(&payload).unwrap();
        let mut applied = layout.clone();
        applied.apply_preset(engine::core::ui_layout::UiLayoutPreset::FullRect);
        assert_eq!(applied.anchors, layout.anchors);
        assert_eq!(applied.pivot, layout.pivot);
        // 且这一份布局的"选中态"只有 FullRect 一个。
        let written = entity_with(&[("UiLayout", payload.as_str())]);
        let selected: Vec<String> = component_actions(&written, "UiLayout")
            .into_iter()
            .filter(|action| action.selected)
            .map(|action| action.label)
            .collect();
        assert_eq!(selected, vec![full_rect.label.clone()], "{selected:?}");
        // 没有登记动作的组件 / 没有该组件的实体：空。
        assert!(component_actions(&entity, "Quad").is_empty());
        assert!(component_actions(&entity_with(&[("Quad", "size=8,8")]), "UiLayout").is_empty());
    }

    /// **字段单位必须与引擎编码器一致**（第 24 轮抓到并修掉的漂移）。统一 `Camera` 的组件串里
    /// `fov` 存的就是**角度**（`Camera3D::to_component_str` 写 `fov_y.to_degrees()`、
    /// `from_component_str` 读进来再 `to_radians()`），所以编辑器显示时**不能**做换算。
    /// 曾经这里被当成"组件串存弧度"：照那样迁移面板，手写面板里的 60° 会在通用编辑器里
    /// 显示成 3437.75°。这条测试直接拿引擎编码器对拍，杜绝同类漂移。
    #[test]
    fn field_units_agree_with_the_engine_encoder() {
        use engine::core::property_path::read_field_text;
        let camera = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            60.0_f32.to_radians(),
            0.1,
            100.0,
        )
        .expect("合法相机");
        let payload = engine::core::camera::Camera::Perspective(camera).to_component_str();
        let raw = read_field_text(&payload, &PropertyPath::new("Camera", "fov")).expect("fov 原文");
        let stored: f64 = raw.trim().parse().expect("fov 是数字");
        let shown = field_unit("Camera", "fov").to_display(stored);
        let engine_degrees = f64::from(camera.fov_y.to_degrees());
        assert!(
            (shown - engine_degrees).abs() < 1e-3,
            "编辑器显示 {shown}°，引擎里是 {engine_degrees}°"
        );
        assert!(
            (shown - 60.0).abs() < 1e-3,
            "显示值就是组件串里的角度：{shown}"
        );
        // 写回路径同样不换算。
        assert!((field_unit("Camera", "fov").from_display(60.0) - 60.0).abs() < 1e-9);
        // 只有"组件串存弧度"的字段（`Radians`，如 `Transform.rotation`）才乘/除 π/180。
        assert!((FieldUnit::Radians.to_display(std::f64::consts::FRAC_PI_3) - 60.0).abs() < 1e-9);
        assert!(
            (FieldUnit::Radians.from_display(60.0) - std::f64::consts::FRAC_PI_3).abs() < 1e-12
        );
    }

    /// **自由文本转义**（第 25 轮）：`Text.text`/`font` 里可以出现 `;`/`=`/`\`，
    /// 组件串按 `UiText` 的约定转义。这条测试走的是**通用编辑器的真实路径**：
    /// 显示文本是反转义后的真实文字 → 提交 → 写出的串仍能被引擎解析器原样读回。
    ///
    /// 没有它，迁移 `Text` 会静默丢掉旧手写面板的这一属性（`text=a;b` 会被写成两个字段，
    /// 或直接被第 24 轮的引擎兜底门拒绝）。
    #[test]
    fn escaped_text_fields_survive_the_generic_edit_path() {
        let scene = scene_with(&[("Text", "text=Hi;font_size=16;color=1,1,1")]);
        // 显示文本：真实文字（不是转义形式）。
        let rows_before = rows_for_component(&scene.entities[&SceneEntityId(1)], "Text")
            .expect("Text 由通用编辑器接管");
        assert!(rows_before.iter().any(|row| row.path == "Text.text"));
        // 写入含特殊字符的文本：一次编辑后引擎必须能原样读回。
        let updated =
            edit_component_field(&scene, SceneEntityId(1), "Text", "text", "a;b=c\\d 中文")
                .expect("转义写入不该失败")
                .expect("取值有变化");
        assert!(
            updated.contains("text=a\\;b\\=c\\\\d 中文"),
            "组件串里必须是转义形式：{updated}"
        );
        let parsed = engine::core::ui::UiText::from_component_str(&updated)
            .expect("引擎解析器必须能读回转义后的串");
        assert_eq!(parsed.text, "a;b=c\\d 中文");
        // 再读显示文本：回到真实文字（读 → 写两个方向互为逆运算）。
        let scene_after = scene_with(&[("Text", updated.as_str())]);
        let rows = rows_for_component(&scene_after.entities[&SceneEntityId(1)], "Text").unwrap();
        let text_row = rows.iter().find(|row| row.path == "Text.text").unwrap();
        assert_eq!(text_row.text, "a;b=c\\d 中文");
        // 幂等：把读到的显示文本再提交一次是"无变化"（不产生历史、不改字节）。
        assert_eq!(
            edit_component_field(
                &scene_after,
                SceneEntityId(1),
                "Text",
                "text",
                &text_row.text
            )
            .expect("ok"),
            None
        );
        // 字体字段同理（旧手写面板的 `font=assets/fonts/a\;b\=数字.ttf` 写法）。
        let font_updated = edit_component_field(
            &scene_after,
            SceneEntityId(1),
            "Text",
            "font",
            "assets/fonts/a;b=数字.ttf",
        )
        .expect("ok")
        .expect("changed");
        let parsed = engine::core::ui::UiText::from_component_str(&font_updated).unwrap();
        assert_eq!(parsed.font, "assets/fonts/a;b=数字.ttf");
        assert_eq!(parsed.text, "a;b=c\\d 中文", "写一个字段不得改动其它字段");
    }

    /// 迁移过来的**只读说明**（第 24 轮）：投影来自统一 `Projection`，`pose=transform` 时给出
    /// **生效**姿态——缺 `Transform` 必须给警告，而不是只显示"其实不生效"的组件值。
    #[test]
    fn camera_notes_show_projection_and_effective_pose() {
        let mut entity = entity_with(&[(
            "Camera",
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100",
        )]);
        let notes = component_notes(&entity, "Camera");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].text.contains("透视") && notes[0].text.contains("60.0°"),
            "投影行应给出统一 Projection 的参数：{}",
            notes[0].text
        );
        assert!(!notes[0].warning);

        // 声明 pose=transform 但没有 Transform：必须给出"回退到组件姿态"的警告。
        entity.components.insert(
            "Camera".to_owned(),
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=transform".to_owned(),
        );
        let notes = component_notes(&entity, "Camera");
        assert!(
            notes
                .iter()
                .any(|note| note.text.contains("仅在实体没有 Transform 时生效")),
            "{notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|note| note.warning && note.text.contains("没有 Transform")),
            "{notes:?}"
        );

        // 补上绕 Y 转 90° 的 Transform：生效姿态必须显示变换后的位置与视线（-X）。
        entity.components.insert(
            "Transform".to_owned(),
            "position=20,5,0;rotation=0,0.7071068,0,0.7071068;scale=1,1,1".to_owned(),
        );
        let notes = component_notes(&entity, "Camera");
        let pose = notes
            .iter()
            .find(|note| note.text.contains("生效姿态"))
            .expect("生效姿态行");
        assert!(!pose.warning, "{}", pose.text);
        assert!(
            pose.text.contains("20.000") && pose.text.contains("-1.000"),
            "生效姿态应来自 Transform 且视线为 -X：{}",
            pose.text
        );

        // 非法的 Transform 串同样是警告（不静默当作可用）。
        entity
            .components
            .insert("Transform".to_owned(), "position=oops".to_owned());
        let notes = component_notes(&entity, "Camera");
        assert!(
            notes
                .iter()
                .any(|note| note.warning && note.text.contains("不可用")),
            "{notes:?}"
        );

        // docs/54 §4.2：禁用相机必须说明"不参与选帧"。
        entity.components.remove("Transform");
        entity.components.insert(
            "Camera".to_owned(),
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;enabled=false".to_owned(),
        );
        let notes = component_notes(&entity, "Camera");
        assert!(
            notes
                .iter()
                .any(|note| note.warning && note.text.contains("不参与相机选帧")),
            "{notes:?}"
        );

        // 无法解析的串：只给一条警告，不猜内容。
        entity
            .components
            .insert("Camera".to_owned(), "position=oops".to_owned());
        let notes = component_notes(&entity, "Camera");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].warning && notes[0].text.contains("无法解析"));

        // 正交分支：同一个组件、按判别字段给出 2D 那套说明（不再有 `Camera2D` 组件名）。
        let orthographic =
            entity_with(&[("Camera", "projection=orthographic;center=640,360;zoom=2")]);
        let notes = component_notes(&orthographic, "Camera");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].text.contains("正交") && notes[0].text.contains("2.000"),
            "{notes:?}"
        );
        assert!(!notes[0].warning);

        // 没有登记说明的组件 / 没有该组件的实体：空。
        assert!(component_notes(&entity, "Quad").is_empty());
        let empty = entity_with(&[("Quad", "size=8,8")]);
        assert!(component_notes(&empty, "Camera").is_empty());
    }

    /// **引擎解析器兜底门**（第 24 轮）：schema 的字段规则通过、但引擎读不了的编辑必须被拒绝，
    /// 且拒绝原因来自引擎解析器原文——这正是迁移 `Camera3D` 之前手写面板在做的校验
    /// （`validate_camera3d` 直接调 `Camera3D::from_component_str`），迁移不得把它弄丢。
    #[test]
    fn edits_the_engine_cannot_read_are_rejected() {
        let scene = scene_with(&[(
            "Camera",
            "projection=perspective;position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100",
        )]);
        // 合法编辑：通过。
        assert!(
            edit_component_field(&scene, SceneEntityId(1), "Camera", "fov", "60").is_ok_and(
                |updated| updated.is_some_and(|payload| payload.contains("fov=60.000000"))
            )
        );
        // `near >= far`：schema 只做类型校验，引擎解析器拒绝（`InvalidPerspective`）。
        match edit_component_field(&scene, SceneEntityId(1), "Camera", "near", "500") {
            Err(SchemaFieldError::EngineRejected { component, reason }) => {
                assert_eq!(component, "Camera");
                assert!(!reason.is_empty(), "拒绝原因不能是空串");
            }
            other => panic!("near >= far 必须被引擎解析器拦下，实际 {other:?}"),
        }
        // 落点退化（position == target）同理。
        match edit_component_field(&scene, SceneEntityId(1), "Camera", "target", "0,0,5") {
            Err(SchemaFieldError::EngineRejected { .. }) => {}
            other => panic!("position == target 必须被拦下，实际 {other:?}"),
        }
        // 判别字段说"正交"，但字段体里没有正交必填的 `center`：schema 的并集表放行（可选），
        // 由**引擎解析器**拦下——"哪种投影必须有哪些字段"只有一处定义（`Camera`）。
        let orthographic = scene_with(&[("Camera", "projection=orthographic;zoom=1")]);
        match edit_component_field(&orthographic, SceneEntityId(1), "Camera", "zoom", "2") {
            Err(SchemaFieldError::EngineRejected { component, reason }) => {
                assert_eq!(component, "Camera");
                assert!(!reason.is_empty(), "拒绝原因不能是空串");
            }
            other => panic!("缺 center 的正交相机必须被引擎解析器拦下，实际 {other:?}"),
        }
        // 引擎不认识的组件名（自定义/Lua 组件）不受这道门影响：没有 schema 就报 `NoSchema`，
        // 不会因为"引擎不认识"而被当成非法载荷（`validate_component_payload` 返回 `None`）。
        let custom = scene_with(&[("MyGameHealth", "hp=10")]);
        match edit_component_field(&custom, SceneEntityId(1), "MyGameHealth", "hp", "12") {
            Err(SchemaFieldError::NoSchema(component)) => assert_eq!(component, "MyGameHealth"),
            other => panic!("expected NoSchema, got {other:?}"),
        }
    }

    /// 迁移后的路径**不放松**校验：`near < far`、look-at 退化都被拒绝，而且拒绝**不产生历史**
    /// （由 shell 的 dispatch 层保证，这里验证预览层返回的是错误而不是一个非法新串）。
    #[test]
    fn rejected_engine_edits_do_not_produce_a_new_payload() {
        let scene = scene_with(&[(
            "Camera",
            "projection=perspective;position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100",
        )]);
        assert!(matches!(
            preview_field_edit(&scene, SceneEntityId(1), "Camera", "far", "0.05"),
            Err(SchemaFieldError::EngineRejected { .. })
        ));
        // 与旧手写面板同级：`far <= near` 也不行。
        assert!(matches!(
            preview_field_edit(&scene, SceneEntityId(1), "Camera", "far", "0.1"),
            Err(SchemaFieldError::EngineRejected { .. })
        ));
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
            (
                "ParticleEmitter",
                "count=8;speed=20;life=1;size=2;color=1,1,1,1",
            ),
            ("Transform", "position=1,2,0;rotation=0,0,0,1;scale=1,1,1"),
            ("Camera", "projection=orthographic;center=0,0;zoom=1"),
        ]);
        // `Transform` 已在第 36 轮迁到通用编辑器，因此它现在**在**接管集合里；
        // 手写面板的组件（如 `Container`）与未知组件仍然不出现。
        assert_eq!(
            driven_components(&entity),
            vec!["Camera", "ParticleEmitter", "Transform"]
        );
        assert!(rows_for_component(&entity, "Container").is_none());
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
        let scene2 = scene_with(&[("Camera", "projection=orthographic;center=0,0;zoom=1.000000")]);
        assert_eq!(
            edit_component_field(&scene2, SceneEntityId(1), "Camera", "zoom", "1").expect("ok"),
            None
        );
    }

    /// 现有组件串本身不满足 schema 时，报错必须指向**整串**问题，而不是用户正在改的字段。
    ///
    /// `write_field` 要先整串解析才能比较当前值，因此"坏字段在别处"的串会以那个字段的
    /// 名字冒出来——用户改的是 `pivot`，消息却指向 `anchors`，这正是把人带到错误字段上的形态。
    ///
    /// 注意**统一 `Camera` 不在这条路上**：它的字段表刻意宽松（`center`/`position` 等都可选），
    /// "哪种投影必须有哪些字段"由引擎解析器强制，因此缺 `center` 得到的是
    /// `EngineRejected`（见 `edits_the_engine_cannot_read_are_rejected`）。
    #[test]
    fn unparsable_payload_is_reported_as_a_payload_problem() {
        let scene = scene_with(&[("UiLayout", "anchors=1,2,3;offsets=0,0,0,0")]);
        match edit_component_field(&scene, SceneEntityId(1), "UiLayout", "pivot", "0.5,0.5") {
            Err(SchemaFieldError::PayloadUnparsable {
                component,
                field,
                reason,
            }) => {
                assert_eq!(component, "UiLayout");
                assert_eq!(field, "anchors", "指出的是整串里出问题的那个字段");
                assert!(!reason.is_empty(), "{reason}");
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
    /// （`Camera/Quad/RenderMode/Visibility/Script/Particle` 全为 0 次命中），
    /// 所以真实工程证据不存在，这里用引擎自己的 `Scene::save`/`Scene::load` 做往返。
    #[test]
    fn driven_components_survive_a_scene_save_load_round_trip() {
        let root =
            std::env::temp_dir().join(format!("rustlua-schema-fields-{}", crate::test_token()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let path = root.join("roundtrip.scene");
        let mut scene = Scene::new("roundtrip");
        scene.add_entity(SceneEntityId(1), "E", None).unwrap();
        {
            let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
            entity.components.insert(
                "Camera".to_owned(),
                "projection=orthographic;center=0,0;zoom=1.5".to_owned(),
            );
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
                "ParticleEmitter".to_owned(),
                "count=8;speed=20;life=1;size=2;color=1,1,1,1".to_owned(),
            );
        }
        scene.save(&path).expect("scene saves");
        let loaded = Scene::load(&path).expect("scene reloads");
        let entity = &loaded.entities[&SceneEntityId(1)];
        assert_eq!(
            driven_components(entity),
            vec![
                "Camera",
                "ParticleEmitter",
                "Quad",
                "RenderMode",
                "Visibility"
            ],
            "通用编辑器接管的组件（顺序来自注册表，稳定）；`Script` 有专用输入，不算在内"
        );
        // `Script`（实体级脚本）虽然不由通用编辑器接管，但它必须能**按运行时格式**读回：
        // 这条同时覆盖了"三个名字收敛成一个"的修复。
        let script = engine::core::entity_script::EntityScript::from_component_str(
            &entity.components["Script"],
        )
        .expect("运行时解析器必须认得保存下来的 Script");
        assert_eq!(script.file, "scripts/a.lua");
        assert!(!script.enabled);
        let rows = rows_for_entity(entity);
        assert!(!rows.is_empty());
        for row in &rows {
            if !row.present {
                // 可选字段可以合法缺省（`Camera.rotation` 等）：缺省就没有"原文"可提交。
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
