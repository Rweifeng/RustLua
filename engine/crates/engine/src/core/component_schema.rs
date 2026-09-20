//! E21: typed schemas for high-value scene components encoded as
//! `BTreeMap<String, String>`.

use std::{collections::BTreeMap, fmt};

/// 字段的**语义规则**：在类型正确之后还要满足的领域约束（docs/55 阶段 2「字段校验」）。
///
/// 动机是一个真实缺口：`FieldKind::Text` 只表示"任意文本"，于是 `Script.file`（必须是
/// **工程根内的相对 `.lua` 路径**）这类规则只存在于编辑器的输入校验与引擎解析器里，
/// schema 与审计层面表达不了——"给引擎加个组件就自动获得字段编辑器"因此只保证**类型正确**。
/// 规则是**声明式**的（没有闭包），所以 `FieldSchema` 仍然是 `const`/`Eq`，schema 表照旧是
/// 静态常量表；引擎解析器的同一条规则由 `field_rules_match_engine_validators` 对拍守住。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldRule {
    /// 去空白后必须非空。
    NonEmpty,
    /// **工程根内的相对路径**：拒绝绝对路径与含 `..` 的路径；`extensions` 非空时要求扩展名
    /// （小写比较）在集合里；`sentinels` 里列出的**内置值**（如网格的 `__ground_plane__`）
    /// 直接放行——它们不是路径，但引擎解析器接受。
    ProjectPath {
        extensions: &'static [&'static str],
        sentinels: &'static [&'static str],
    },
    /// 整数/数字必须非负；**向量要求每个分量都非负**（如 `Image.nine_slice` 的四条边框）。
    NonNegative,
}

impl FieldRule {
    /// 人类可读的规则说明（编辑器把它作为输入提示，审计把它作为诊断理由）。
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::NonEmpty => "不能为空".to_owned(),
            Self::ProjectPath {
                extensions: [],
                sentinels: [],
            } => "工程内相对路径".to_owned(),
            Self::ProjectPath {
                extensions,
                sentinels,
            } => {
                let mut text = if extensions.is_empty() {
                    "工程内相对路径".to_owned()
                } else {
                    format!("工程内相对路径（扩展名 {}）", extensions.join("/"))
                };
                if !sentinels.is_empty() {
                    text.push_str(&format!("，或内置值 {}", sentinels.join("/")));
                }
                text
            }
            Self::NonNegative => "不能为负数".to_owned(),
        }
    }

    /// 检查一条规则；`Ok(())` 表示通过。
    fn check(self, value: &FieldValue, raw: &str) -> Result<(), String> {
        match self {
            Self::NonEmpty => {
                if raw.trim().is_empty() {
                    return Err("不能为空".to_owned());
                }
                Ok(())
            }
            Self::ProjectPath {
                extensions,
                sentinels,
            } => check_project_path(raw.trim(), extensions, sentinels),
            Self::NonNegative => {
                let negative = match value {
                    FieldValue::Int(value) => *value < 0,
                    FieldValue::Number(value) => *value < 0.0,
                    // 向量：任一分量为负即拒绝（`nine_slice` 的四条边框必须非负）。
                    FieldValue::Tuple(values) => values.iter().any(|value| *value < 0.0),
                    // 其它类型不适用（规则写错了也不会误报）。
                    _ => false,
                };
                if negative {
                    return Err(format!("不能为负数（{raw}）"));
                }
                Ok(())
            }
        }
    }
}

/// 工程内相对路径检查：绝对路径、`..`、以及不在白名单里的扩展名都拒绝。
///
/// 与 `entity_script::EntityScript::validate` 同一口径（那条路径上脚本是可执行代码，
/// 场景文件不能让它去读工程外的文件）。
fn check_project_path(
    raw: &str,
    extensions: &'static [&'static str],
    sentinels: &'static [&'static str],
) -> Result<(), String> {
    if sentinels.contains(&raw) {
        // 内置值（哨兵）不是路径，引擎解析器直接接受。
        return Ok(());
    }
    if raw.is_empty() {
        return Err("不能为空".to_owned());
    }
    // 与 `entity_script::EntityScript::validate` 共用 `core::paths` 的权威判定
    // （曾经这里只用 `is_absolute()`，于是 Windows 上的 `/etc/passwd` 被放行）。
    if crate::core::paths::is_absolute_path(raw) {
        return Err(format!("必须是工程内相对路径（{raw} 是绝对路径）"));
    }
    if crate::core::paths::escapes_project_root(raw) {
        return Err(format!("必须是工程内相对路径（{raw} 含 ..）"));
    }
    if !extensions.is_empty() {
        let extension = std::path::Path::new(raw)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !extensions.iter().any(|allowed| *allowed == extension) {
            return Err(format!("扩展名必须是 {}（{raw}）", extensions.join("/")));
        }
    }
    Ok(())
}

/// A validated component instance decoded from a serialized component string.
pub type ComponentData = BTreeMap<String, FieldValue>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    Bool,
    Int,
    Number,
    Text,
    Vec2,
    Vec3,
    Vec4,
    /// `宽x高`（如 `size=16x16`）：Sprite/Tilemap 的尺寸编码与逗号分隔的 Vec2 不同。
    Size,
    Enum,
}

impl FieldKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Number => "number",
            Self::Text => "string",
            Self::Vec2 => "vec2",
            Self::Vec3 => "vec3",
            Self::Vec4 => "vec4",
            Self::Size => "size",
            Self::Enum => "enum",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Bool(bool),
    Int(i64),
    Number(f32),
    Text(String),
    Tuple(Vec<f32>),
    Choice(String),
}

impl FieldValue {
    #[must_use]
    pub fn to_component_value(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::Number(value) => format!("{value:.6}"),
            Self::Text(value) => value.clone(),
            Self::Tuple(values) => values
                .iter()
                .map(|value| format!("{value:.6}"))
                .collect::<Vec<_>>()
                .join(","),
            Self::Choice(value) => value.clone(),
        }
    }
}

/// Parse an integer that may use `0x`/`0b` prefixes (Visibility masks encode as `0x{:X}`).
fn parse_int(raw: &str) -> Result<i64, String> {
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
        return Err(format!("invalid {raw:?}; expected integer"));
    }
    i64::from_str_radix(digits, radix).map_err(|_| format!("invalid {raw:?}; expected integer"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldSchema {
    pub name: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    pub choices: &'static [&'static str],
    /// 类型之外的领域规则（路径/非空/非负）。空 = 只做类型校验。
    pub rules: &'static [FieldRule],
}

impl FieldSchema {
    #[must_use]
    pub const fn new(name: &'static str, kind: FieldKind, required: bool) -> Self {
        Self {
            name,
            kind,
            required,
            choices: &[],
            rules: &[],
        }
    }

    #[must_use]
    pub const fn with_choices(mut self, choices: &'static [&'static str]) -> Self {
        self.choices = choices;
        self
    }

    #[must_use]
    pub const fn with_rules(mut self, rules: &'static [FieldRule]) -> Self {
        self.rules = rules;
        self
    }

    /// 编辑器/审计用的一行字段说明（类型 + 规则）。
    #[must_use]
    pub fn describe(&self) -> String {
        if self.rules.is_empty() {
            return self.kind.label().to_owned();
        }
        let rules = self
            .rules
            .iter()
            .map(|rule| rule.describe())
            .collect::<Vec<_>>()
            .join("，");
        format!("{}（{rules}）", self.kind.label())
    }

    /// 校验**单个字段**的原始文本（不要求同组件的其它字段在场）。
    ///
    /// 供 `property_path` 的按字段写入使用：写一个字段时只有它需要被校验，
    /// 其余必填字段的存在性由整串解析负责。
    pub fn validate(&self, raw: &str, component: &str) -> Result<FieldValue, SchemaError> {
        self.validate_raw(raw, component)
    }

    fn validate_raw(&self, raw: &str, component: &str) -> Result<FieldValue, SchemaError> {
        let fail = |reason: String| SchemaError {
            component: component.to_owned(),
            field: self.name.to_owned(),
            reason,
        };
        let parse_number = |value: &str| -> Result<f32, SchemaError> {
            value
                .trim()
                .parse::<f32>()
                .map_err(|_| fail(format!("invalid {value:?}; expected number")))
                .and_then(|parsed| {
                    parsed
                        .is_finite()
                        .then_some(parsed)
                        .ok_or_else(|| fail(format!("invalid {value:?}; number must be finite")))
                })
        };
        match self.kind {
            FieldKind::Bool => match raw.trim() {
                "true" | "1" => Ok(FieldValue::Bool(true)),
                "false" | "0" => Ok(FieldValue::Bool(false)),
                value => Err(fail(format!("invalid {value:?}; expected bool"))),
            },
            FieldKind::Int => parse_int(raw).map(FieldValue::Int).map_err(fail),
            FieldKind::Number => parse_number(raw).map(FieldValue::Number),
            FieldKind::Text => Ok(FieldValue::Text(raw.to_owned())),
            FieldKind::Size => {
                // `宽x高`（Sprite/Tilemap 约定）。也接受 `宽,高`？不接受：
                // 编码只写 `x` 形式，schema 与编码保持一一对应才能被漂移门守住。
                let (width, height) = raw
                    .trim()
                    .split_once(['x', 'X'])
                    .ok_or_else(|| fail(format!("invalid {raw:?}; expected widthxheight")))?;
                let width = parse_number(width)?;
                let height = parse_number(height)?;
                Ok(FieldValue::Tuple(vec![width, height]))
            }
            FieldKind::Vec2 | FieldKind::Vec3 | FieldKind::Vec4 => {
                let expected = match self.kind {
                    FieldKind::Vec2 => 2,
                    FieldKind::Vec3 => 3,
                    _ => 4,
                };
                let parts: Vec<&str> = raw.split(',').collect();
                if parts.len() != expected {
                    return Err(fail(format!(
                        "expected {expected} comma-separated numbers, found {}",
                        parts.len()
                    )));
                }
                let mut values = Vec::with_capacity(expected);
                for part in parts {
                    values.push(parse_number(part)?);
                }
                Ok(FieldValue::Tuple(values))
            }
            FieldKind::Enum => {
                let normalized = raw.trim();
                if self.choices.contains(&normalized) {
                    Ok(FieldValue::Choice(normalized.to_owned()))
                } else {
                    Err(fail(format!(
                        "invalid {normalized:?}; expected one of {:?}",
                        self.choices
                    )))
                }
            }
        }
        // 类型通过之后再跑领域规则：顺序固定为"先类型后语义"，错误信息也就固定了
        // （类型错报到类型上，语义错报到规则上）。
        .and_then(|value| {
            for rule in self.rules {
                rule.check(&value, raw).map_err(&fail)?;
            }
            Ok(value)
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentSchema {
    pub name: &'static str,
    pub version: u32,
    pub fields: &'static [FieldSchema],
}

impl ComponentSchema {
    #[must_use]
    pub const fn new(name: &'static str, version: u32, fields: &'static [FieldSchema]) -> Self {
        Self {
            name,
            version,
            fields,
        }
    }

    /// Parses and validates a component payload. Missing optional fields remain
    /// absent, so round-trips do not add synthetic defaults.
    pub fn parse(&self, raw: &str) -> Result<ComponentData, SchemaError> {
        let mut data = ComponentData::new();
        for field in raw.split(';') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, value)) = field.split_once('=') else {
                return Err(SchemaError {
                    component: self.name.to_owned(),
                    field: field.to_owned(),
                    reason: "component field must be key=value".to_owned(),
                });
            };
            let schema = self
                .fields
                .iter()
                .find(|schema| schema.name == key)
                .ok_or_else(|| SchemaError {
                    component: self.name.to_owned(),
                    field: key.to_owned(),
                    reason: format!("unknown field {key:?}"),
                })?;
            data.insert(key.to_owned(), schema.validate_raw(value, self.name)?);
        }
        for schema in self.fields {
            if schema.required && !data.contains_key(schema.name) {
                return Err(SchemaError {
                    component: self.name.to_owned(),
                    field: schema.name.to_owned(),
                    reason: "required field is missing".to_owned(),
                });
            }
        }
        Ok(data)
    }

    /// serialize 是**按 schema 的字段类型**编码的：`Size` 必须写回 `宽x高`，
    /// 否则「解析 → 序列化 → 解析」会把 `128x128` 变成 `128,128` 而无法再解析。
    /// 该缺陷同样是漂移门第一次跑通前抓出来的。
    #[must_use]
    pub fn serialize(&self, data: &ComponentData) -> String {
        data.iter()
            .map(|(key, value)| {
                let kind = self
                    .fields
                    .iter()
                    .find(|field| field.name == key)
                    .map(|field| field.kind);
                match (kind, value) {
                    (Some(FieldKind::Size), FieldValue::Tuple(values)) if values.len() == 2 => {
                        format!("{key}={}x{}", values[0], values[1])
                    }
                    _ => format!("{key}={}", value.to_component_value()),
                }
            })
            .collect::<Vec<_>>()
            .join(";")
    }

    /// 不依赖 schema 的按值序列化（字段顺序为字典序，`Size` 会写成逗号形式）。
    /// 仅用于已知没有 `Size` 字段的场景；结构化组件请用 [`Self::serialize`]。
    #[must_use]
    pub fn serialize_values(data: &ComponentData) -> String {
        data.iter()
            .map(|(key, value)| format!("{key}={}", value.to_component_value()))
            .collect::<Vec<_>>()
            .join(";")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaError {
    pub component: String,
    pub field: String,
    pub reason: String,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "component {} field {}: {}",
            self.component, self.field, self.reason
        )
    }
}

impl std::error::Error for SchemaError {}

const TRANSFORM_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("x", FieldKind::Number, true),
    FieldSchema::new("y", FieldKind::Number, true),
    FieldSchema::new("rotation", FieldKind::Number, true),
    FieldSchema::new("scale_x", FieldKind::Number, true),
    FieldSchema::new("scale_y", FieldKind::Number, true),
];

const TEXT_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("text", FieldKind::Text, true),
    FieldSchema::new("font_size", FieldKind::Number, true),
    FieldSchema::new("color", FieldKind::Vec3, false),
    // 可选自定义字体（项目相对路径，如 assets/fonts/x.ttf）；空 = 项目默认字体。
    FieldSchema::new("font", FieldKind::Text, false),
];

const IMAGE_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("texture", FieldKind::Text, true),
    FieldSchema::new("color", FieldKind::Vec3, false),
    // 九宫格四条边框（纹理像素）：与 `UiImage::from_component_str` 同一口径，负值拒绝。
    // 全零与"省略该字段"**语义相同**（渲染侧用 `any(|v| *v > 0.0)` 过滤，
    // 见 `runtime_scene.rs` 的九宫格分支），因此编辑器把零写出来后引擎再保存会自然收敛回省略。
    FieldSchema::new("nine_slice", FieldKind::Vec4, false)
        .with_rules(&[FieldRule::NonNegative]),
];

const UI_LAYOUT_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("anchors", FieldKind::Vec4, false),
    FieldSchema::new("offsets", FieldKind::Vec4, false),
    FieldSchema::new("min_size", FieldKind::Vec2, false),
    FieldSchema::new("max_size", FieldKind::Vec2, false),
    FieldSchema::new("pivot", FieldKind::Vec2, false),
];

// ---------------------------------------------------------------------------
// 以下 schema 与引擎各自的 `to_component_str` 一一对应；
// `builtin_schemas_match_engine_encoders` 用引擎自己的编码器逐条验证，
// 因此字段名/类型漂移会直接让这个门判红（docs/55 §1 第 3 条）。
// ---------------------------------------------------------------------------

/// `Transform3D::to_component_str`：`position=..;rotation=..;scale=..`
const TRANSFORM_3D_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("position", FieldKind::Vec3, true),
    FieldSchema::new("rotation", FieldKind::Vec4, true),
    FieldSchema::new("scale", FieldKind::Vec3, true),
];

/// `Camera2dComponent::to_component_str`：`center=..;zoom=.. [+rotation/bounds/enabled]`
const CAMERA_2D_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("center", FieldKind::Vec2, true),
    FieldSchema::new("zoom", FieldKind::Number, true),
    FieldSchema::new("rotation", FieldKind::Number, false),
    FieldSchema::new("bounds", FieldKind::Vec4, false),
    FieldSchema::new("enabled", FieldKind::Bool, false),
];

/// `Camera3dComponent::to_component_str`：位置/目标/视场/远近 [+pose/enabled]
const CAMERA_3D_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("position", FieldKind::Vec3, true),
    FieldSchema::new("target", FieldKind::Vec3, true),
    FieldSchema::new("fov", FieldKind::Number, true),
    FieldSchema::new("near", FieldKind::Number, true),
    FieldSchema::new("far", FieldKind::Number, true),
    FieldSchema::new("pose", FieldKind::Enum, false).with_choices(&["transform"]),
    FieldSchema::new("enabled", FieldKind::Bool, false),
];

/// `DirectionalLight::to_component_str`：`direction=..;color=..;intensity=..`
const LIGHT_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("direction", FieldKind::Vec3, true),
    FieldSchema::new("color", FieldKind::Vec3, true),
    FieldSchema::new("intensity", FieldKind::Number, true),
];

/// `MeshRenderer::to_component_str`：`mesh=<项目相对路径>`
///
/// 规则与 `mesh3d::validate_mesh_path` **同一口径**：非空、工程内相对、
/// 扩展名 `.glb`/`.gltf`，外加内置哨兵 `__ground_plane__`。
const MESH_RENDERER_FIELDS: &[FieldSchema] = &[FieldSchema::new("mesh", FieldKind::Text, true)
    .with_rules(&[
        FieldRule::NonEmpty,
        FieldRule::ProjectPath {
            extensions: &["glb", "gltf"],
            sentinels: &[crate::core::mesh3d::GROUND_PLANE_SENTINEL],
        },
    ])];

/// `SpriteComponent::to_component_str`：尺寸用 `宽x高`，可选 flip/z/flip 等。
///
/// `pivot`/`order` 标为**可选**：编码器当前总会写出它们，但仓库里的真实旧场景
/// （`examples/game_jhyt/scenes/*.scene`）没有这两个字段，解析器会取默认值。
/// 把它们标成 required 会让"旧项目无损打开"这一条在审计里直接判红——
/// 这正是 `schema_audit` 在真实工程上第一次运行就抓出来的问题。
const SPRITE_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("texture", FieldKind::Text, true),
    FieldSchema::new("size", FieldKind::Size, true),
    FieldSchema::new("pivot", FieldKind::Vec2, false),
    FieldSchema::new("tint", FieldKind::Vec4, true),
    // 分层名是**字符串**（`default`/`world`/`actor`/自定义），由 `sorting_layer_rank`
    // 解析——不是整数。这一点由漂移门首次运行时当场抓出。
    FieldSchema::new("sorting_layer", FieldKind::Text, true),
    FieldSchema::new("order", FieldKind::Int, false),
    FieldSchema::new("flip_x", FieldKind::Bool, false),
    FieldSchema::new("flip_y", FieldKind::Bool, false),
    FieldSchema::new("z", FieldKind::Number, false),
];

/// `QuadComponent::to_component_str`：size 是逗号分隔（与 Sprite 的 `x` 形式不同）。
const QUAD_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("size", FieldKind::Vec2, true),
    FieldSchema::new("pivot", FieldKind::Vec2, false),
    FieldSchema::new("color", FieldKind::Vec4, false),
    FieldSchema::new("texture", FieldKind::Text, false),
    FieldSchema::new("z", FieldKind::Number, false),
    // 与 Sprite 一致：分层名是字符串。
    FieldSchema::new("layer", FieldKind::Text, false),
    FieldSchema::new("order", FieldKind::Int, false),
    FieldSchema::new("flip_x", FieldKind::Bool, false),
    FieldSchema::new("flip_y", FieldKind::Bool, false),
];

/// `RenderMode::to_component_str`：`mode=world|overlay|screen`
const RENDER_MODE_FIELDS: &[FieldSchema] =
    &[FieldSchema::new("mode", FieldKind::Enum, true)
        .with_choices(&["world", "overlay", "screen"])];

/// `Visibility::to_component_str`：`layers=0x{:X}`
const VISIBILITY_FIELDS: &[FieldSchema] = &[FieldSchema::new("layers", FieldKind::Int, true)];

/// `EntityScript::to_component_str`：`file=<相对路径> [;enabled=false]`
///
/// 注册表键取 [`crate::core::entity_script::SCRIPT_COMPONENT`]（= `Script`）——**运行时读的
/// 就是这个名字**。此前键写成 `EntityScript`，于是 schema 描述了一个没人消费的组件，
/// 而真实场景里的 `Script` 反而被审计算作"没有 schema"。
const SCRIPT_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("file", FieldKind::Text, true).with_rules(&[
        // 与 `EntityScript::validate` **同一口径**：非空 + 工程根内的相对路径。
        // 脚本是可执行代码，场景文件不能让它去读工程外的文件。
        //
        // 刻意**不**写 `extensions: &["lua"]`：运行时是 `root.join(file)` 后直接当 Lua 源码
        // 编译，扩展名不参与加载判定；把 `.lua` 写进这里的后果是"审计误报运行时能正常跑的
        // 场景"。`.lua` 是**编辑器输入**层面的项目约定（见 `inspector_state::validate_lua_script`），
        // 比运行时更严，属于有意为之，两边差异由编辑器侧的测试写明。
        FieldRule::NonEmpty,
        FieldRule::ProjectPath {
            extensions: &[],
            sentinels: &[],
        },
    ]),
    FieldSchema::new("enabled", FieldKind::Bool, false),
];

/// `WorldText::to_component_str`：复用 UiText 字段 + 世界侧偏移。
///
/// `offset` 标为**可选**：`WorldText::from_component_str` 在字段缺席时取默认 `0,0`
/// （`world_text.rs` 里 `parse_offset` 之前的初始值），而仓库里第二个示例工程
/// `examples/game_api` 的真实场景（`scenes/unified.scene` 的实体 7）就**没有**写这个字段。
/// 标成 required 会让审计把"能正常加载的真实场景"判红——与 `Sprite.pivot/order`
/// 是同一类漂移，这次由**真实工程审计**（把审计跑到第二个示例工程上）抓出来。
const WORLD_TEXT_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("text", FieldKind::Text, true),
    FieldSchema::new("font_size", FieldKind::Number, true),
    FieldSchema::new("color", FieldKind::Vec3, false),
    FieldSchema::new("font", FieldKind::Text, false),
    FieldSchema::new("offset", FieldKind::Vec2, false),
    FieldSchema::new("centered", FieldKind::Bool, false),
];

/// `ParticleComponent::to_component_str`：以 `count` 起头，其余按需输出。
const PARTICLE_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("count", FieldKind::Int, true),
    FieldSchema::new("speed", FieldKind::Number, true),
    FieldSchema::new("life", FieldKind::Number, true),
    FieldSchema::new("size", FieldKind::Number, true),
    FieldSchema::new("color", FieldKind::Vec4, true),
    FieldSchema::new("rate", FieldKind::Number, false),
    FieldSchema::new("size_end", FieldKind::Number, false),
    FieldSchema::new("color_end", FieldKind::Vec4, false),
    FieldSchema::new("gravity", FieldKind::Vec2, false),
    FieldSchema::new("drag", FieldKind::Number, false),
];

/// 内置 schema 注册表：新增结构化组件应加在这里，而不是继续长 ad hoc 解析分支。
#[must_use]
pub fn builtin_registry() -> BTreeMap<&'static str, ComponentSchema> {
    let schemas = [
        ComponentSchema::new("Transform", 1, TRANSFORM_FIELDS),
        ComponentSchema::new("Text", 2, TEXT_FIELDS),
        ComponentSchema::new("Image", 2, IMAGE_FIELDS),
        ComponentSchema::new("UiLayout", 1, UI_LAYOUT_FIELDS),
        ComponentSchema::new("Transform3D", 1, TRANSFORM_3D_FIELDS),
        ComponentSchema::new("Camera2D", 1, CAMERA_2D_FIELDS),
        ComponentSchema::new("Camera3D", 1, CAMERA_3D_FIELDS),
        ComponentSchema::new("Light", 1, LIGHT_FIELDS),
        ComponentSchema::new("MeshRenderer", 1, MESH_RENDERER_FIELDS),
        ComponentSchema::new("Sprite", 1, SPRITE_FIELDS),
        ComponentSchema::new("Quad", 1, QUAD_FIELDS),
        ComponentSchema::new("RenderMode", 1, RENDER_MODE_FIELDS),
        ComponentSchema::new("Visibility", 1, VISIBILITY_FIELDS),
        ComponentSchema::new(
            crate::core::entity_script::SCRIPT_COMPONENT,
            1,
            SCRIPT_FIELDS,
        ),
        ComponentSchema::new("WorldText", 1, WORLD_TEXT_FIELDS),
        ComponentSchema::new("Particle", 1, PARTICLE_FIELDS),
    ];
    schemas
        .into_iter()
        .map(|schema| (schema.name, schema))
        .collect()
}

/// Validates a component without materializing its values.
pub fn validate_component(
    registry: &BTreeMap<&'static str, ComponentSchema>,
    name: &str,
    raw: &str,
) -> Result<(), SchemaError> {
    registry
        .get(name)
        .ok_or_else(|| SchemaError {
            component: name.to_owned(),
            field: "<component>".to_owned(),
            reason: "no schema is registered".to_owned(),
        })
        .and_then(|schema| schema.parse(raw).map(|_| ()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        camera::{Camera2D, Camera3D},
        entity_script::EntityScript,
        light::Light,
        mesh3d::MeshRenderer,
        particle::ParticleEmitter,
        quad::Quad,
        render_mode::RenderMode,
        sprite::SpriteComponent,
        transform::{Transform, Transform3D},
        visibility::Visibility,
        world_text::WorldText,
    };

    /// 漂移门：每个 schema 都必须能解析**引擎自己编码器**写出的字符串，
    /// 且解析 → 再序列化 → 再解析 保持不变。
    ///
    /// 这是 docs/55 §1 第 3 条（"Inspector、校验、迁移和运行时 schema 容易漂移"）
    /// 的机械化拦截：字段改名/改类型/改分隔符都会让这个测试直接判红。
    #[test]
    fn builtin_schemas_match_engine_encoders() {
        let registry = builtin_registry();
        let cases: Vec<(&str, Vec<String>)> = vec![
            ("Transform", vec![Transform::default().to_component_str()]),
            (
                "Transform3D",
                vec![Transform3D::default().to_component_str()],
            ),
            (
                "Camera2D",
                vec![
                    Camera2D::new(crate::core::transform::Vec2::new(0.0, 0.0), 1.0)
                        .expect("valid camera")
                        .to_component_str(),
                    Camera2D::new(crate::core::transform::Vec2::new(0.0, 0.0), 2.0)
                        .expect("valid camera")
                        .to_component_str(),
                ],
            ),
            (
                "Camera3D",
                vec![
                    Camera3D::perspective(
                        crate::core::math3d::Vec3::new(0.0, 1.0, 3.0),
                        crate::core::math3d::Vec3::new(0.0, 0.0, 0.0),
                        60.0_f32.to_radians(),
                        16.0 / 9.0,
                        0.1,
                        100.0,
                    )
                    .expect("valid camera")
                    .to_component_str(),
                ],
            ),
            ("Light", vec![Light::default().to_component_str()]),
            (
                "MeshRenderer",
                vec![
                    MeshRenderer {
                        mesh: "assets/meshes/cube.glb".to_owned(),
                    }
                    .to_component_str(),
                ],
            ),
            (
                "Sprite",
                vec![SpriteComponent::default().to_component_str()],
            ),
            ("Quad", vec![Quad::default().to_component_str()]),
            ("RenderMode", vec![RenderMode::default().to_component_str()]),
            ("Visibility", vec![Visibility::default().to_component_str()]),
            (
                "Script",
                vec![EntityScript {
                    // 用**合法**载荷：`EntityScript::default()` 是 `file=`（空路径），
                    // 引擎自己也不会接受它，拿它当漂移样本只会自欺欺人。
                    file: "scripts/ai/patrol.lua".to_owned(),
                    enabled: true,
                }
                .to_component_str()],
            ),
            (
                "WorldText",
                vec![
                    WorldText::from_component_str("text=hello;font_size=16;offset=0,0")
                        .expect("valid world text")
                        .to_component_str(),
                    WorldText::from_component_str("text=hi;font_size=18;offset=1,2;centered=true")
                        .expect("valid world text")
                        .to_component_str(),
                ],
            ),
            (
                "Particle",
                vec![ParticleEmitter::default().to_component_str()],
            ),
        ];

        for (component, encoded_variants) in cases {
            let schema = registry
                .get(component)
                .unwrap_or_else(|| panic!("{component} 缺少 schema"));
            for encoded in encoded_variants {
                let parsed = schema.parse(&encoded).unwrap_or_else(|error| {
                    panic!("{component}: schema 无法解析引擎编码 {encoded:?}：{error}")
                });
                // 再序列化 → 再解析：必须稳定（不丢字段、不引入新字段）。
                let reserialized = schema.serialize(&parsed);
                let reparsed = schema.parse(&reserialized).unwrap_or_else(|error| {
                    panic!("{component}: 再序列化后无法解析 {reserialized:?}：{error}")
                });
                assert_eq!(
                    parsed, reparsed,
                    "{component}: 解析 → 序列化 → 解析 必须保持不变（encoded={encoded:?}）"
                );
            }
        }
    }

    /// 脚本组件的 schema 键必须是**运行时读的那个名字**。
    ///
    /// 这条守卫来自一个真实缺陷：schema 注册表里写的是 `EntityScript`，运行时与场景加载器
    /// 读的是 `Script`，编辑器写的是 `LuaScript`——于是 schema 在描述一个没人消费的组件，
    /// 真实场景里的 `Script` 反而被审计报成"没有 schema"。
    #[test]
    fn script_schema_uses_the_runtime_component_name() {
        let registry = builtin_registry();
        let name = crate::core::entity_script::SCRIPT_COMPONENT;
        assert_eq!(name, "Script");
        assert!(
            registry.contains_key(name),
            "schema 必须注册运行时读的那个名字"
        );
        assert!(
            !registry.contains_key("EntityScript"),
            "旧键名不得复活（否则又会出现'描述了没人消费的组件'）"
        );
        // 引擎编码器的输出必须能被这个 schema 解析（与漂移门同一口径，但这条专盯脚本组件）。
        let encoded = EntityScript {
            file: "scripts/ai/patrol.lua".to_owned(),
            enabled: false,
        }
        .to_component_str();
        let parsed = registry[name].parse(&encoded).expect("engine encoder output");
        let reserialized = registry[name].serialize(&parsed);
        let reparsed = registry[name]
            .parse(&reserialized)
            .expect("re-serialized payload parses");
        // 语义稳定即可，**不要求逐字节相同**：schema 的序列化按字段名字典序输出
        // （`ComponentData` 是 `BTreeMap`），而引擎编码器按自己的书写顺序
        // （这里是 `file=...;enabled=false`）。字段写入走的是
        // `property_path::write_field` 的"最小改动"路径，因此编辑器编辑字段
        // **不会**重排组件串；这条差异只影响"整串重新编码"的场景。
        assert_eq!(parsed, reparsed, "解析 → 序列化 → 解析必须语义不变");
        assert_eq!(
            reserialized, "enabled=false;file=scripts/ai/patrol.lua",
            "schema 序列化按字典序（与引擎编码器书写顺序不同，见上）"
        );
    }

    /// 声明式规则必须与**引擎解析器**的语义校验一致：两边对同一组样本给出同样的接受/拒绝。
    ///
    /// 这是"语义规则也会漂移"的机械化拦截：`Script.file` 的规则在引擎里是
    /// `EntityScript::validate`，在 schema 里是 `FieldRule::ProjectPath{extensions:["lua"]}`，
    /// 任何一边放宽都会让这条测试判红。`MeshRenderer.mesh` 同理（含内置哨兵）。
    #[test]
    fn field_rules_match_engine_validators() {
        let registry = builtin_registry();
        let script = &registry[crate::core::entity_script::SCRIPT_COMPONENT];
        let samples = [
            ("scripts/ai/patrol.lua", true),
            ("player.lua", true),
            ("scripts/deep/nested/x.lua", true),
            // 引擎只 `root.join(file)` 后编译，扩展名不参与判定 → 运行时能跑，schema 也必须放行
            // （`.lua` 是编辑器输入层的更严约定，不在这里表达）。
            ("scripts/player.txt", true),
            ("scripts/player", true),
            // 绝对路径
            ("/etc/passwd", false),
            ("C:/windows/x.lua", false),
            // 穿越
            ("../outside.lua", false),
            ("scripts/../../outside.lua", false),
            // 空
            ("", false),
        ];
        for (sample, expected_ok) in samples {
            let payload = format!("file={sample}");
            let engine = crate::core::entity_script::EntityScript::from_component_str(&payload);
            let schema = script.parse(&payload);
            assert_eq!(
                engine.is_ok(),
                expected_ok,
                "引擎解析器对 {sample:?} 的判定与预期不符"
            );
            assert_eq!(
                schema.is_ok(),
                expected_ok,
                "schema 对 {sample:?} 的判定必须与引擎一致（reason={:?}）",
                schema.as_ref().err().map(|error| error.reason.clone())
            );
        }

        // 网格：非空 / 工程内相对 / `.glb`/`.gltf` / 内置哨兵。
        let mesh = &registry["MeshRenderer"];
        let mesh_samples = [
            ("assets/models/cube.glb", true),
            ("assets/models/cube.gltf", true),
            (crate::core::mesh3d::GROUND_PLANE_SENTINEL, true),
            ("assets/models/cube.obj", false),
            ("assets/models/cube", false),
            ("../escape.glb", false),
            ("/etc/x.glb", false),
            ("", false),
        ];
        for (sample, expected_ok) in mesh_samples {
            let payload = format!("mesh={sample}");
            let engine = crate::core::mesh3d::MeshRenderer::from_component_str(&payload);
            let schema = mesh.parse(&payload);
            assert_eq!(
                engine.is_ok(),
                expected_ok,
                "引擎解析器对 {sample:?} 的判定与预期不符"
            );
            assert_eq!(
                schema.is_ok(),
                expected_ok,
                "schema 对 {sample:?} 的判定必须与引擎一致（reason={:?}）",
                schema.as_ref().err().map(|error| error.reason.clone())
            );
        }

        // UI 贴图：`nine_slice` 四条边框必须非负（渲染侧只接受非负；见下面的"刻意更严"说明）。
        let image = &registry["Image"];
        let image_samples = [
            ("texture=a.png;color=1,1,1;nine_slice=8,8,16,16", true),
            // 全零 = 不启用九宫格（渲染侧 `any(|v| *v > 0.0)` 过滤，语义等同省略）。
            ("texture=a.png;color=1,1,1;nine_slice=0,0,0,0", true),
            ("texture=a.png", true),
        ];
        for (payload, expected_ok) in image_samples {
            let engine = crate::core::ui::UiImage::from_component_str(payload);
            let schema = image.parse(payload);
            assert_eq!(
                engine.is_ok(),
                expected_ok,
                "引擎解析器对 {payload:?} 的判定与预期不符"
            );
            assert_eq!(
                schema.is_ok(),
                expected_ok,
                "schema 对 {payload:?} 的判定必须与引擎一致（reason={:?}）",
                schema.as_ref().err().map(|error| error.reason.clone())
            );
        }

        // **刻意更严**（与 `Script.file` 的 `.lua` 约定同一类）：负的九宫格边框引擎解析器接受，
        // 但渲染侧会 `*v >= 0.0` 过滤掉（等于不启用），所以那是"写了也没用"的输入。
        // 编辑器侧原本就拦（旧的 `validate_image`），现在由 schema 规则拦，因此
        // **审计与通用字段编辑器都获得这条输入守卫**；引擎行为不变。
        let negative = "texture=a.png;color=1,1,1;nine_slice=-1,0,0,0";
        assert!(
            crate::core::ui::UiImage::from_component_str(negative).is_ok(),
            "引擎解析器仍然接受它（不改运行时行为）"
        );
        assert!(
            image.parse(negative).is_err(),
            "schema 必须拒绝负边框（比引擎更严，与原编辑器一致）"
        );
    }

    /// 类型规则与语义规则都要能单独触发，且错误信息指向正确的层次。
    #[test]
    fn field_rules_reject_semantic_problems_after_type_checks() {
        let registry = builtin_registry();
        let script = &registry[crate::core::entity_script::SCRIPT_COMPONENT];
        // 语义错（类型对）：绝对路径。
        let error = script.parse("file=/abs/player.lua").unwrap_err();
        assert_eq!(error.field, "file");
        assert!(error.reason.contains("绝对路径"), "{}", error.reason);
        // 语义错：穿越。
        let error = script.parse("file=../player.lua").unwrap_err();
        assert!(error.reason.contains(".."), "{}", error.reason);
        // 语义错：空（规则，而不是"字段缺失"）。
        let error = script.parse("file=").unwrap_err();
        assert!(error.reason.contains("不能为空"), "{}", error.reason);
        // 类型错：enabled 不是 bool（语义规则不该抢在类型前面报错）。
        let error = script.parse("file=ok.lua;enabled=maybe").unwrap_err();
        assert_eq!(error.field, "enabled");
        // 扩展名白名单规则单独用一个人造字段验证（`Script.file` 刻意不限扩展名）。
        let texture = FieldSchema::new("texture", FieldKind::Text, true).with_rules(&[
            FieldRule::ProjectPath {
                extensions: &["png", "jpg"],
                sentinels: &[],
            },
        ]);
        assert!(texture.validate("assets/a.png", "Image").is_ok());
        assert!(texture.validate("assets/a.JPG", "Image").is_ok());
        let error = texture.validate("assets/a.gif", "Image").unwrap_err();
        assert!(error.reason.contains("扩展名"), "{}", error.reason);
        // 非负规则。
        let particle = FieldSchema::new("count", FieldKind::Int, true)
            .with_rules(&[FieldRule::NonNegative]);
        assert!(particle.validate("4", "Particle").is_ok());
        assert!(particle.validate("-1", "Particle").is_err());
        assert!(particle.validate("0", "Particle").is_ok());
        assert!(
            FieldSchema::new("zoom", FieldKind::Number, true)
                .with_rules(&[FieldRule::NonNegative])
                .validate("-0.5", "Camera2D")
                .is_err()
        );
        // 非空规则对文本生效。
        let text =
            FieldSchema::new("mesh", FieldKind::Text, true).with_rules(&[FieldRule::NonEmpty]);
        assert!(text.validate("  ", "MeshRenderer").is_err());
        assert!(text.validate("assets/x.glb", "MeshRenderer").is_ok());
    }

    /// 字段说明要能同时表达类型与规则（编辑器用它做输入提示）。
    #[test]
    fn field_describe_mentions_kind_and_rules() {
        let registry = builtin_registry();
        let file = registry[crate::core::entity_script::SCRIPT_COMPONENT]
            .fields
            .iter()
            .find(|field| field.name == "file")
            .expect("file 字段");
        let described = file.describe();
        assert!(described.contains("string"), "{described}");
        assert!(described.contains("相对路径"), "{described}");
        let plain = FieldSchema::new("x", FieldKind::Number, true);
        assert_eq!(plain.describe(), "number");
    }

    /// 真实工程审计抓出来的漂移：`examples/game_api` 的场景里 `WorldText` **没有** `offset`
    /// 字段（引擎解析器默认 `0,0`）。把 `offset` 标成 required 会让审计把能正常加载的真实
    /// 场景判红——与 `Sprite.pivot/order` 同一类问题，这次是"把审计跑到第二个示例工程"才发现的。
    #[test]
    fn optional_fields_missing_from_real_scenes_stay_optional() {
        let registry = builtin_registry();
        // 真实场景里的原样负载（`scenes/unified.scene` 实体 7 的形态）。
        let payload = "text=hello;font_size=18";
        let parsed = registry["WorldText"]
            .parse(payload)
            .expect("offset 缺席必须仍然可解析");
        assert!(
            !parsed.contains_key("offset"),
            "缺省字段不该被补出合成默认值"
        );
        // 显式给出时仍被校验。
        assert!(registry["WorldText"]
            .parse("text=hello;font_size=18;offset=1,2")
            .is_ok());
        assert!(registry["WorldText"]
            .parse("text=hello;font_size=18;offset=1")
            .is_err());
    }

    /// 反向漂移：schema 里写错字段名会让引擎编码解析失败（防止 schema 单方面"发明"字段）。
    #[test]
    fn schemas_reject_fields_the_encoder_never_writes() {        let registry = builtin_registry();
        let error = registry["Sprite"]
            .parse("texture=a.png;size=16x16;pivot=0.5,0.5;tint=1,1,1,1;sorting_layer=default;order=0;bogus=1")
            .unwrap_err();
        assert_eq!(error.component, "Sprite");
        assert_eq!(error.field, "bogus");
    }

    /// 十六进制掩码（Visibility `layers=0x{:X}`）必须被 Int 接受。
    #[test]
    fn int_fields_accept_radix_prefixes() {
        let registry = builtin_registry();
        let data = registry["Visibility"].parse("layers=0xFF").unwrap();
        assert_eq!(data["layers"], FieldValue::Int(255));
        let data = registry["Visibility"].parse("layers=0b1010").unwrap();
        assert_eq!(data["layers"], FieldValue::Int(10));
    }

    /// 尺寸字段：`宽x高` 两种数值都必须是有限数。
    #[test]
    fn size_fields_require_two_numbers() {
        let registry = builtin_registry();
        let error = registry["Sprite"]
            .parse("texture=a.png;size=16;pivot=0,0;tint=1,1,1,1;sorting_layer=default;order=0")
            .unwrap_err();
        assert_eq!(error.field, "size");
        assert!(error.reason.contains("widthxheight"), "{}", error.reason);
    }

    #[test]
    fn parses_transform_and_round_trips() {
        let registry = builtin_registry();
        let data = registry["Transform"]
            .parse("x=1;y=-2;rotation=0;scale_x=2;scale_y=3")
            .unwrap();
        assert_eq!(data["x"], FieldValue::Number(1.0));
        assert_eq!(data["scale_y"], FieldValue::Number(3.0));
        assert!(!registry["Transform"].serialize(&data).is_empty());
    }

    #[test]
    fn errors_include_component_and_field_path() {
        let registry = builtin_registry();
        let error = registry["Transform"]
            .parse("x=bad;y=0;rotation=0;scale_x=1;scale_y=1")
            .unwrap_err();
        assert_eq!(error.component, "Transform");
        assert_eq!(error.field, "x");
        assert!(error.to_string().contains("component Transform field x"));
    }

    #[test]
    fn validates_builtin_components() {
        let registry = builtin_registry();
        assert!(validate_component(&registry, "Unknown", "x=1").is_err());
        assert!(
            registry["UiLayout"]
                .parse("anchors=0,0,1,1;offsets=0,0,0,0")
                .is_ok()
        );
    }

    /// 回归：Text/Image schema 必须与新组件解析器（ui.rs）字段一致——
    /// 视觉属性 + color/nine_slice；旧内联几何（x/y/scale/width/height）
    /// 已随 UiLayout 迁移删除，schema 白名单不得再接受。
    #[test]
    fn text_and_image_schemas_match_current_visual_only_format() {
        let registry = builtin_registry();
        assert!(
            registry["Text"]
                .parse("text=Hello;font_size=24;color=1,0.5,0")
                .is_ok()
        );
        assert!(registry["Text"].parse("text=Hello;font_size=24").is_ok());
        assert!(
            registry["Text"]
                .parse("text=Hello;font_size=24;font=assets/fonts/x.ttf")
                .is_ok()
        );
        assert!(
            registry["Text"]
                .parse("text=Hello;font_size=24;x=1;y=2;scale=1")
                .is_err()
        );
        assert!(
            registry["Image"]
                .parse("texture=ui/button.png;color=1,1,1;nine_slice=8,8,8,8")
                .is_ok()
        );
        assert!(
            registry["Image"]
                .parse("texture=ui/button.png;width=64;height=32")
                .is_err()
        );
        // 字段类型：color 必须 3 个数，nine_slice 必须 4 个数。
        assert!(
            registry["Text"]
                .parse("text=Hello;font_size=24;color=1,1")
                .is_err()
        );
        assert!(
            registry["Image"]
                .parse("texture=ui/button.png;nine_slice=8,8,8")
                .is_err()
        );
    }
}
