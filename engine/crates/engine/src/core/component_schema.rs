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
    /// 与 [`FieldRule::ProjectPath`] 相同，但**空值放行**——空串在引擎里是"未指定"
    /// （`Sprite.texture=` 就是 Sprite 的默认编码：没有纹理的占位精灵），因此不能要求非空。
    OptionalProjectPath {
        extensions: &'static [&'static str],
        sentinels: &'static [&'static str],
    },
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
            Self::OptionalProjectPath {
                extensions,
                sentinels,
            } => {
                let inner = Self::ProjectPath {
                    extensions,
                    sentinels,
                }
                .describe();
                format!("{inner}（留空表示不指定）")
            }
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
            Self::OptionalProjectPath {
                extensions,
                sentinels,
            } => {
                // 空 = "未指定"（引擎默认编码就是 `texture=`），不参与路径判定。
                if raw.trim().is_empty() {
                    return Ok(());
                }
                check_project_path(raw.trim(), extensions, sentinels)
            }
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

/// 单个字段的编码（[`ComponentSchema::serialize`] 与
/// [`ComponentSchema::serialize_payload`] 共用）：`Size` 写回 `宽x高`、自由文本按
/// `UiText` 约定转义，其余走值自身的编码。schema 为 `None` 表示"没有声明的键"
/// （动态键或历史数据），此时按值编码。
fn encode_field(schema: Option<&FieldSchema>, key: &str, value: &FieldValue) -> String {
    let escaped = schema.is_some_and(|field| field.escaped);
    let kind = schema.map(|field| field.kind);
    match (escaped, kind, value) {
        (true, _, FieldValue::Text(text)) => {
            format!("{key}={}", crate::core::ui::escape_text(text))
        }
        (_, Some(FieldKind::Size), FieldValue::Tuple(values)) if values.len() == 2 => {
            format!("{key}={}x{}", values[0], values[1])
        }
        _ => format!("{key}={}", value.to_component_value()),
    }
}

/// 校验一个**位置式段**的参数（[`PositionalField`]）。
///
/// 与引擎解析器同口径：段按冒号切成至多 `parts.len()` 份（最后一份吃掉剩余文本，
/// 因此 `events=2:step` 里的冒号不会被误切），前 N 份必须齐备且逐个通过类型校验。
fn validate_positional(
    positional: &PositionalField,
    rest: &str,
    component: &str,
) -> Result<(), SchemaError> {
    if positional.parts.is_empty() {
        return Ok(());
    }
    let parts: Vec<&str> = rest.splitn(positional.parts.len(), ':').collect();
    let required = positional.parts.iter().filter(|part| part.required).count();
    if parts.len() < required {
        return Err(SchemaError {
            component: component.to_owned(),
            field: positional.prefix.trim_end_matches(':').to_owned(),
            reason: format!(
                "expected at least {required} colon-separated values, found {}",
                parts.len()
            ),
        });
    }
    for (schema, raw) in positional.parts.iter().zip(parts) {
        schema.validate_raw(raw, component)?;
    }
    Ok(())
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldSchema {
    pub name: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    pub choices: &'static [&'static str],
    /// 类型之外的领域规则（路径/非空/非负）。空 = 只做类型校验。
    pub rules: &'static [FieldRule],
    /// **自由文本转义**：该字段的值里可以出现 `;`/`=`/`\`，组件串按 `UiText` 的约定转义
    /// （`\;` / `\=` / `\\`）。只有**引擎编码器真的转义**的字段才能标这一位；标了之后
    /// `ComponentSchema::parse`/`serialize`、`property_path` 的按字段读写都走同一份转义实现
    /// （见 `ui::escape_text`/`unescape_text`/`split_fields`）。
    ///
    /// 为什么必须显式标注而不是"所有 Text 都转义"：组件串的切分口径由**引擎解析器**决定，
    /// 像 `MeshRenderer.mesh` 这类字段的解析器是**朴素按 `;` 切分**的，schema 若替它转义，
    /// 审计就会接受运行时读不了的串（比引擎更松，是反向漂移）。
    pub escaped: bool,
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
            escaped: false,
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

    /// 标记为**自由文本**（见 [`FieldSchema::escaped`]）。
    #[must_use]
    pub const fn with_escaped(mut self) -> Self {
        self.escaped = true;
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

/// **位置式段**：段里没有 `=`，各参数按冒号位置解码
/// （`Animation` 的 `clip:<名>:<sheet>:<cols>:<fps>:<looped>[:events=..]`）。
///
/// 为什么必须单独建模而不是塞进 `FieldSchema`：位置式段的"键"是**前缀**而不是字段名，
/// 且同一前缀可以在一串里出现多次；扁平 `key=value` 模型两者都表达不了。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PositionalField {
    /// 段前缀（含冒号，如 `"clip:"`）——用 `starts_with` 匹配。
    pub prefix: &'static str,
    /// 冒号分隔的位置参数；**最后一段吃掉剩余文本**
    /// （`events=2:step` 内部的冒号因此不会被误切，与引擎的 `splitn(n, ':')` 同口径）。
    pub parts: &'static [FieldSchema],
    /// 该段在一条记录里是否必须出现。
    pub required: bool,
}

/// **动态键**：键名由作者决定（`UiEvents` 的 `arg.<名>`、`UiAnimation` 的 `track.<属性>`），
/// 只有值的类型可以事先声明。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DynamicField {
    pub prefix: &'static str,
    pub field: FieldSchema,
}

/// **列表/重复字段模型**（第 38 轮）：组件串由一条或多条记录组成，记录之间用
/// `separator` 分隔，记录内部固定用 `;`。
///
/// 出现它的原因：`Animation`/`UiEvents`/`UiBindings`/`UiAnimation` 的组件串不是
/// `BTreeMap<键,值>`——`clip:` 段没有 `=` 且可重复、`|` 分隔的多条绑定里同一个键会出现多次。
/// 旧模型对它们的处理是"静默折叠/报错"，因此它们被**故意**排除在注册表外
/// （见 `components_with_positional_or_repeated_fields_stay_unregistered` 的历史）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListModel {
    /// `None` = 组件串只有**一条**记录（位置式段可在记录内重复，如 `Animation`）；
    /// `Some('|')` = 多条记录（`UiEvents`/`UiBindings`/`UiAnimation`）。
    pub separator: Option<char>,
    /// 记录内的位置式段。
    pub positional: &'static [PositionalField],
    /// 记录内的动态键。
    pub dynamic: &'static [DynamicField],
}

/// 一条记录：`key=value` 字段（含动态键，键名保留原文）+ 按出现顺序的位置式段。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComponentEntry {
    pub fields: ComponentData,
    /// `(前缀, 段原文)`：保留原文是为了**无损**写回（位置式段的数值格式由引擎决定，
    /// schema 不重新格式化它，否则 `2` 会变成 `2.000000`）。
    pub positional: Vec<(String, String)>,
}

/// [`ComponentSchema::parse`] 的完整形态：单条记录 → 字段映射；列表模型 → 多条记录。
#[derive(Clone, Debug, PartialEq)]
pub enum ComponentPayload {
    Fields(ComponentData),
    Entries(Vec<ComponentEntry>),
}

impl ComponentPayload {
    /// 单条记录形态的字段映射（列表模型返回 `None`）。
    #[must_use]
    pub const fn fields(&self) -> Option<&ComponentData> {
        match self {
            Self::Fields(data) => Some(data),
            Self::Entries(_) => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentSchema {
    pub name: &'static str,
    pub version: u32,
    pub fields: &'static [FieldSchema],
    /// `None` = 单条 `key=value` 记录（老模型）；`Some` = 列表/重复字段模型。
    pub list: Option<ListModel>,
}

impl ComponentSchema {
    #[must_use]
    pub const fn new(name: &'static str, version: u32, fields: &'static [FieldSchema]) -> Self {
        Self {
            name,
            version,
            fields,
            list: None,
        }
    }

    /// 声明为**列表/重复字段**模型（见 [`ListModel`]）。
    #[must_use]
    pub const fn with_list(mut self, list: ListModel) -> Self {
        self.list = Some(list);
        self
    }

    /// 解析**任意**组件串（单条记录或列表模型）。
    ///
    /// 列表模型的组件**不能**用 [`Self::parse`]：那个入口的契约是"扁平键值映射"，
    /// 对列表模型会返回一条明确说明的错误（而不是像以前那样报"段里没有 `=`"这种
    /// 误导性的原因）。
    pub fn parse_payload(&self, raw: &str) -> Result<ComponentPayload, SchemaError> {
        let Some(list) = self.list else {
            return self.parse(raw).map(ComponentPayload::Fields);
        };
        let fail = |field: &str, reason: String| SchemaError {
            component: self.name.to_owned(),
            field: field.to_owned(),
            reason,
        };
        let records: Vec<&str> = match list.separator {
            None => vec![raw],
            Some(separator) => raw.split(separator).collect(),
        };
        let mut entries = Vec::new();
        for record in records {
            let mut entry = ComponentEntry::default();
            for segment in record.split(';') {
                let segment = segment.trim();
                if segment.is_empty() {
                    continue;
                }
                if let Some(positional) = list
                    .positional
                    .iter()
                    .find(|positional| segment.starts_with(positional.prefix))
                {
                    validate_positional(
                        positional,
                        &segment[positional.prefix.len()..],
                        self.name,
                    )?;
                    entry
                        .positional
                        .push((positional.prefix.to_owned(), segment.to_owned()));
                    continue;
                }
                let Some((key, value)) = segment.split_once('=') else {
                    return Err(fail(
                        segment,
                        "component field must be key=value".to_owned(),
                    ));
                };
                let schema = self
                    .fields
                    .iter()
                    .find(|schema| schema.name == key)
                    .or_else(|| {
                        list.dynamic
                            .iter()
                            .find(|dynamic| {
                                key.len() > dynamic.prefix.len() && key.starts_with(dynamic.prefix)
                            })
                            .map(|dynamic| &dynamic.field)
                    })
                    .ok_or_else(|| fail(key, format!("unknown field {key:?}")))?;
                entry
                    .fields
                    .insert(key.to_owned(), schema.validate_raw(value, self.name)?);
            }
            for schema in self.fields {
                if schema.required && !entry.fields.contains_key(schema.name) {
                    return Err(fail(schema.name, "required field is missing".to_owned()));
                }
            }
            for positional in list.positional {
                if positional.required
                    && !entry
                        .positional
                        .iter()
                        .any(|(prefix, _)| prefix == positional.prefix)
                {
                    return Err(fail(
                        positional.prefix,
                        "required field is missing".to_owned(),
                    ));
                }
            }
            entries.push(entry);
        }
        Ok(ComponentPayload::Entries(entries))
    }

    /// [`Self::parse_payload`] 的逆运算：按**声明顺序**写回（引擎编码器的书写顺序），
    /// 位置式段保留原文。
    #[must_use]
    pub fn serialize_payload(&self, payload: &ComponentPayload) -> String {
        let entries = match payload {
            ComponentPayload::Fields(data) => return self.serialize(data),
            ComponentPayload::Entries(entries) => entries,
        };
        let list = self.list.unwrap_or(ListModel {
            separator: None,
            positional: &[],
            dynamic: &[],
        });
        let records: Vec<String> = entries
            .iter()
            .map(|entry| {
                let mut segments: Vec<String> = Vec::new();
                for schema in self.fields {
                    if let Some(value) = entry.fields.get(schema.name) {
                        segments.push(encode_field(Some(schema), schema.name, value));
                    }
                }
                // 动态键：`BTreeMap` 字典序——引擎编码器（`UiEventBinding::to_text` 遍历
                // `arguments`、编辑器 UI 动画遍历 `tracks`）也是字典序。
                for (key, value) in &entry.fields {
                    if self.fields.iter().any(|schema| schema.name == key) {
                        continue;
                    }
                    let dynamic = list
                        .dynamic
                        .iter()
                        .find(|dynamic| key.starts_with(dynamic.prefix));
                    segments.push(encode_field(
                        dynamic.map(|dynamic| &dynamic.field),
                        key,
                        value,
                    ));
                }
                for (_, raw) in &entry.positional {
                    segments.push(raw.clone());
                }
                segments.join(";")
            })
            .collect();
        match list.separator {
            None => records.first().cloned().unwrap_or_default(),
            Some(separator) => records.join(&separator.to_string()),
        }
    }

    /// Parses and validates a component payload. Missing optional fields remain
    /// absent, so round-trips do not add synthetic defaults.
    pub fn parse(&self, raw: &str) -> Result<ComponentData, SchemaError> {
        if self.list.is_some() {
            return Err(SchemaError {
                component: self.name.to_owned(),
                field: "<list>".to_owned(),
                reason: "该组件是列表/重复字段模型，没有扁平的键值映射；请用 parse_payload"
                    .to_owned(),
            });
        }
        self.parse_with_fields(self.fields, raw)
    }

    /// 按指定字段表解析的实现。
    fn parse_with_fields(
        &self,
        fields: &'static [FieldSchema],
        raw: &str,
    ) -> Result<ComponentData, SchemaError> {
        let mut data = ComponentData::new();
        // 有转义字段的组件必须用**转义感知**的切分（`text=a\;b` 不是一个分隔符）；
        // 没有转义字段的组件保持朴素切分：口径与它们各自的引擎解析器一致，不放宽审计。
        let escaped = fields.iter().any(|field| field.escaped);
        let segments: Vec<&str> = if escaped {
            crate::core::ui::split_fields(raw)
        } else {
            raw.split(';').collect()
        };
        for field in segments {
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
            let schema = fields
                .iter()
                .find(|schema| schema.name == key)
                .ok_or_else(|| SchemaError {
                    component: self.name.to_owned(),
                    field: key.to_owned(),
                    reason: format!("unknown field {key:?}"),
                })?;
            // 转义字段在 `ComponentData` 里存**反转义后的真实文本**：这样"读 → 改 → 写"
            // 与"当前值比较"都在同一层语义上（`write_field` 的幂等判定靠它）。
            let value = if schema.escaped {
                crate::core::ui::unescape_text(value)
            } else {
                value.to_owned()
            };
            data.insert(key.to_owned(), schema.validate_raw(&value, self.name)?);
        }
        for schema in fields {
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

    /// 该组件是否有**自由文本**字段（决定切分与序列化是否走转义口径）。
    #[must_use]
    pub fn has_escaped_fields(&self) -> bool {
        self.fields.iter().any(|field| field.escaped)
    }

    /// serialize 是**按 schema 的字段类型**编码的：`Size` 必须写回 `宽x高`，
    /// 否则「解析 → 序列化 → 解析」会把 `128x128` 变成 `128,128` 而无法再解析。
    /// 该缺陷同样是漂移门第一次跑通前抓出来的。
    ///
    /// **自由文本字段**（`FieldSchema::escaped`）在这里按 `UiText` 的约定转义，
    /// 与 `ComponentSchema::parse` 互为逆运算。
    #[must_use]
    pub fn serialize(&self, data: &ComponentData) -> String {
        data.iter()
            .map(|(key, value)| {
                encode_field(
                    self.fields.iter().find(|field| field.name == key),
                    key,
                    value,
                )
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

/// **统一 `Transform`**：唯一变换组件的字段表 = 3D 表示。
///
/// 组件串由 `Transform3D::to_component_str` 写出；2D 实体用的是**同一个组件、同一个形态**，
/// 空间差别不体现在字段上（见 `spatial_transform::entity_space`）。
const TRANSFORM_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("position", FieldKind::Vec3, true),
    FieldSchema::new("rotation", FieldKind::Vec4, true),
    FieldSchema::new("scale", FieldKind::Vec3, true),
];

/// 统一 `Camera`：显式投影判别字段 + 两套字段的并集。
///
/// **刻意宽松**："哪套字段必须出现"由引擎读取器（`camera::Camera`）强制——
/// 判别字段必填，字段体必须与它匹配（写 orthographic 却给透视字段会被拒）。
const CAMERA_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("projection", FieldKind::Enum, true)
        .with_choices(&["orthographic", "perspective"]),
    FieldSchema::new("center", FieldKind::Vec2, false),
    FieldSchema::new("zoom", FieldKind::Number, false),
    FieldSchema::new("bounds", FieldKind::Vec4, false),
    FieldSchema::new("position", FieldKind::Vec3, false),
    FieldSchema::new("target", FieldKind::Vec3, false),
    FieldSchema::new("fov", FieldKind::Number, false),
    FieldSchema::new("near", FieldKind::Number, false),
    FieldSchema::new("far", FieldKind::Number, false),
    FieldSchema::new("pose", FieldKind::Enum, false).with_choices(&["transform", "look_at"]),
    FieldSchema::new("enabled", FieldKind::Bool, false),
];

const TEXT_FIELDS: &[FieldSchema] = &[
    // `text`/`font` 是**自由文本**：`UiText::to_component_str` 会把 `;`/`=`/`\` 转义
    // （`split_fields` 也按转义切分），所以 schema 必须用同一口径，否则
    // "含分号的文字"会被审计判成非法、也会被按字段写入写坏。
    FieldSchema::new("text", FieldKind::Text, true).with_escaped(),
    FieldSchema::new("font_size", FieldKind::Number, true),
    FieldSchema::new("color", FieldKind::Vec3, false),
    // 可选自定义字体（项目相对路径，如 assets/fonts/x.ttf）；空 = 项目默认字体。
    FieldSchema::new("font", FieldKind::Text, false).with_escaped(),
];

/// **纹理引用**的共用规则（docs/55 §P0 第 4 条"字段级语义校验"的铺开）。
///
/// 与旧手写 Sprite 面板的 `validate_asset_reference` **同一口径**：工程内相对路径 +
/// 图片扩展名；`__white__` 是引擎内置的 1×1 白色纹理哨兵（真实场景 `Image` 组件里就有它），
/// 因此必须放行。
///
/// **刻意比引擎解析器更严**（与 `Script.file` 的 `.lua` 约定、`Image.nine_slice` 的非负同一类）：
/// `SpriteComponent/UiImage/Quad/Tilemap::from_component_str` 接受任意文本，`.bmp`/绝对路径
/// 都能解析；编辑器与**审计**在这里拦下它们，运行时行为不变（分歧由
/// `field_rules_match_engine_validators` 里的显式断言钉住，别"顺手对齐"）。
const TEXTURE_RULE: FieldRule = FieldRule::OptionalProjectPath {
    extensions: &["png", "jpg", "jpeg", "webp"],
    sentinels: &["__white__"],
};

/// 同 [`TEXTURE_RULE`]，但**空值也拒绝**：`Quad`/`Tilemap` 的引擎解析器明确拒绝空纹理
/// （`quad texture must not be empty`），schema 与它同口径。
const REQUIRED_TEXTURE_RULE: FieldRule = FieldRule::ProjectPath {
    extensions: &["png", "jpg", "jpeg", "webp"],
    sentinels: &["__white__"],
};

const IMAGE_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("texture", FieldKind::Text, true).with_rules(&[TEXTURE_RULE]),
    FieldSchema::new("color", FieldKind::Vec3, false),
    // 九宫格四条边框（纹理像素）：与 `UiImage::from_component_str` 同一口径，负值拒绝。
    // 全零与"省略该字段"**语义相同**（渲染侧用 `any(|v| *v > 0.0)` 过滤，
    // 见 `runtime_scene.rs` 的九宫格分支），因此编辑器把零写出来后引擎再保存会自然收敛回省略。
    FieldSchema::new("nine_slice", FieldKind::Vec4, false).with_rules(&[FieldRule::NonNegative]),
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
    FieldSchema::new("texture", FieldKind::Text, true).with_rules(&[TEXTURE_RULE]),
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
    FieldSchema::new("texture", FieldKind::Text, false).with_rules(&[REQUIRED_TEXTURE_RULE]),
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
    // 与 `TEXT_FIELDS` 同源：`WorldText` 把 text/font 原样转交 `UiText`，转义口径必须一致。
    FieldSchema::new("text", FieldKind::Text, true).with_escaped(),
    FieldSchema::new("font_size", FieldKind::Number, true),
    FieldSchema::new("color", FieldKind::Vec3, false),
    FieldSchema::new("font", FieldKind::Text, false).with_escaped(),
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

/// `ColliderDesc3d::from_component_str`（组件名 `Collider3d`）：3D 碰撞体。
///
/// 与其它 schema 不同，3D 碰撞体在引擎侧**只有解析器、没有 `to_component_str`**
/// （碰撞体只在实体实例化时解析一次；编辑器侧另有 `inspector_state::encode_collider3d`）。
/// 因此这里的事实源是解析器，对拍样本取**真实工程里出现过的形态**
/// （`examples/game_api/scenes/api_3d.scene` 的三种碰撞体形态），并由
/// `builtin_schemas_match_engine_encoders` 用引擎解析器逐条确认"schema 接受的串引擎也接受"。
///
/// required 判定：
/// - `shape` 缺席 → 解析器直接报 `missing shape` → **required**。
/// - `radius`/`half_width`/`half_height`/`half_z` 缺席时**各自有默认值 `0.0`**，随后由
///   "对应形状的尺寸必须为正"这条**跨字段**校验兜底（`shape=ball` 缺 `radius` 才会报
///   `ball radius must be positive`）。当前 schema 模型（`BTreeMap<key,value>`）表达不了
///   "仅当 `shape=ball`/`capsule` 时 `radius` 必填"，把它们标成 required 会让真实场景里的
///   `shape=cuboid;half_width=..;half_height=..;half_z=..`（没有 `radius`）被判红——
///   那是比漏报更糟的**误报**（真实工程审计一跑就红）。所以这四个字段标 optional。
/// - 其余（`offset`/`sensor`/`dynamic`/`layer`/`mask`/`ccd`）缺席都取默认值 → optional。
const COLLIDER_3D_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("shape", FieldKind::Enum, true).with_choices(&["ball", "cuboid", "capsule"]),
    // ball / capsule
    FieldSchema::new("radius", FieldKind::Number, false),
    // cuboid
    FieldSchema::new("half_width", FieldKind::Number, false),
    // cuboid / capsule
    FieldSchema::new("half_height", FieldKind::Number, false),
    // cuboid
    FieldSchema::new("half_z", FieldKind::Number, false),
    FieldSchema::new("offset", FieldKind::Vec3, false),
    FieldSchema::new("sensor", FieldKind::Bool, false),
    FieldSchema::new("dynamic", FieldKind::Bool, false),
    // 碰撞分组掩码（u8 位域）：解析器写 `layer`/`mask`，默认全 1（0xFF）。
    FieldSchema::new("layer", FieldKind::Int, false),
    FieldSchema::new("mask", FieldKind::Int, false),
    FieldSchema::new("ccd", FieldKind::Bool, false),
];

/// `Tilemap::to_component_str`：
/// `tile_size=<宽x高>;width=..;height=..;texture=..;tiles=..;solid=..`
///
/// required 判定：`tile_size`/`width`/`height`/`tiles`/`solid` 缺席时解析器分别报
/// `missing tile_size` / `missing width` / … → **required**；`texture` 缺席时回退
/// [`crate::core::tilemap::DEFAULT_TILESET`] → **optional**。
///
/// `tiles`/`solid` 是**逐格字符序列**（`tiles` 用 `0`-`6`、`solid` 用 `0`/`1`，行优先），
/// 不是整数也不是向量，所以只能是 `Text`；"长度 = `width × height`"是跨字段规则，
/// schema 模型表达不了（由引擎解析器的 `LengthMismatch` 兜底）。
///
/// 规则：
/// - `tile_size`/`width`/`height`/`tiles`/`solid` 的非负：解析器的 `tile_size` 必须是正数、
///   `width`/`height` 解析成 `usize`（负数直接报错），`NonNegative` 是 schema 侧能表达的最强口径。
/// - `tiles`/`solid` 的 `NonEmpty`：解析器要求长度等于 `width × height`，而尺寸必须非零，
///   所以空串必然是 `LengthMismatch` —— 两边同判非法（不是更严）。
/// - `texture` 的 `NonEmpty` 与解析器的 `texture must not be empty` 同一口径；
///   schema 额外拒绝**纯空白**（解析器只查 `is_empty()`），属于 `Image.nine_slice` 那类
///   "写了也没用"的输入守卫。
const TILEMAP_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("tile_size", FieldKind::Size, true).with_rules(&[FieldRule::NonNegative]),
    FieldSchema::new("width", FieldKind::Int, true).with_rules(&[FieldRule::NonNegative]),
    FieldSchema::new("height", FieldKind::Int, true).with_rules(&[FieldRule::NonNegative]),
    FieldSchema::new("texture", FieldKind::Text, false).with_rules(&[REQUIRED_TEXTURE_RULE]),
    FieldSchema::new("tiles", FieldKind::Text, true).with_rules(&[FieldRule::NonEmpty]),
    FieldSchema::new("solid", FieldKind::Text, true).with_rules(&[FieldRule::NonEmpty]),
];

/// `ContainerStyle::to_component_str`：`Container` 自动布局样式（E17）。
///
/// **全字段 optional**：`from_component_str` 从 `Self::default()` 起头，任何字段缺席都取默认值
/// （`kind=vbox`、`padding=0,0,0,0`、`spacing=0`、`columns=0`、`stretch/fill=false`、
/// `align=start`），连空串 `""` 都是合法的默认样式（`ui_layout.rs` 的测试钉住了这一点），
/// 因此一个 required 都没有。
/// `columns` 是 `usize`（解析器对负数报错）→ 挂 `NonNegative`；`0` = 自动推断方形网格。
/// `spacing`/`padding` 允许负数（重叠布局），不加规则。
const CONTAINER_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("kind", FieldKind::Enum, false)
        .with_choices(&["vbox", "hbox", "grid", "margin", "center"]),
    FieldSchema::new("padding", FieldKind::Vec4, false),
    FieldSchema::new("spacing", FieldKind::Number, false),
    FieldSchema::new("columns", FieldKind::Int, false).with_rules(&[FieldRule::NonNegative]),
    FieldSchema::new("stretch", FieldKind::Bool, false),
    FieldSchema::new("fill", FieldKind::Bool, false),
    FieldSchema::new("align", FieldKind::Enum, false).with_choices(&["start", "center", "end"]),
];

/// `ButtonStateSet::to_component_str`：五个视觉状态的贴图覆盖 + `disabled_state`。
///
/// **全字段 optional**：`from_component_str` 从 `Self::default()`（全 `None` / `false`）起头。
/// 空值（`normal=`）被解析器当成"没有覆盖"（`non_empty` 返回 `None`）而不是错误，所以这里
/// **不能**挂 `NonEmpty`——那会让"清空一个状态"这种合法写法变成审计误报。
/// 路径规则同理不挂：引擎只把值当不透明字符串存，真实工程里的 `__white__` 是内置哨兵
/// （`examples/game_api/scenes/ui_test.scene` 的 ButtonStates），扩展名/相对路径判定在别处，schema
/// 替它加码就是"比引擎更松/更严"的反向漂移。
const BUTTON_STATES_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("normal", FieldKind::Text, false),
    FieldSchema::new("hover", FieldKind::Text, false),
    FieldSchema::new("pressed", FieldKind::Text, false),
    FieldSchema::new("disabled", FieldKind::Text, false),
    FieldSchema::new("focused", FieldKind::Text, false),
    FieldSchema::new("disabled_state", FieldKind::Bool, false),
];

// ---------------------------------------------------------------------------
// 列表/重复字段模型（第 38 轮）：`Animation` / `UiEvents` / `UiBindings` / `UiAnimation`
// ---------------------------------------------------------------------------
//
// 这三个（`UiAnimation` 见下）此前**故意不注册**：它们的组件串不是 `BTreeMap<键,值>`
// ——`clip:` 段没有 `=` 且可重复、`|` 分隔的多条绑定里同一个键会出现多次。
// 现在由 `ListModel` 表达，字段表**逐条对着引擎解析器**（而不是对着旧手写面板）核对。

/// `Animation`：一条记录，`default=<名>` + 每个状态一个位置式 `clip:<名>:<sheet>:<cols>:<fps>:<looped>[:events=..]`。
const ANIMATION_FIELDS: &[FieldSchema] = &[FieldSchema::new("default", FieldKind::Text, true)];

const ANIMATION_CLIP_PARTS: &[FieldSchema] = &[
    FieldSchema::new("name", FieldKind::Text, true),
    FieldSchema::new("sheet", FieldKind::Text, true),
    FieldSchema::new("cols", FieldKind::Int, true),
    FieldSchema::new("frame_rate", FieldKind::Number, true),
    // 引擎解析器只认这两个字面量（`clip looped must be true/false`），因此用 `Enum`
    // 而不是 `Bool`——`Bool` 会多接受 `1`/`0`，那比引擎松。
    FieldSchema::new("looped", FieldKind::Enum, true).with_choices(&["true", "false"]),
    // 可选的第 6 段（`events=<帧>:<名>[,<帧>:<名>]`）：**整段**吃剩余文本，
    // 与引擎的 `splitn(6, ':')` 同口径（否则 `2:step` 会被误切）。
    FieldSchema::new("events", FieldKind::Text, false),
];

const ANIMATION_POSITIONAL: &[PositionalField] = &[PositionalField {
    prefix: "clip:",
    parts: ANIMATION_CLIP_PARTS,
    required: false,
}];

const ANIMATION_LIST: ListModel = ListModel {
    // `Animation` 是**一条**记录里的重复段（不是 `|` 分隔的多条记录）。
    separator: None,
    positional: ANIMATION_POSITIONAL,
    dynamic: &[],
};

/// `UiEvents`：`|` 分隔的多条记录，每条是 `event=/target=/function=/arg.<名>=`。
/// 事件名与 `UiEventType::parse` 的白名单一一对应（比引擎松一点都会产生误报）。
const UI_EVENTS_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("event", FieldKind::Enum, false).with_choices(&[
        "pointer_enter",
        "pointer_exit",
        "pointer_down",
        "pointer_up",
        "click",
        "focus",
        "submit",
        "value_changed",
        "drag",
    ]),
    FieldSchema::new("target", FieldKind::Text, false),
    // 引擎的 `UiEventBinding::parse` 在没有 `function` 时报 `MissingFunction`。
    FieldSchema::new("function", FieldKind::Text, true),
];

const UI_EVENTS_DYNAMIC: &[DynamicField] = &[DynamicField {
    prefix: "arg.",
    field: FieldSchema::new("arg", FieldKind::Text, false),
}];

const UI_EVENTS_LIST: ListModel = ListModel {
    separator: Some('|'),
    positional: &[],
    dynamic: UI_EVENTS_DYNAMIC,
};

/// `UiBindings`：`|` 分隔的多条记录，每条 `property=/target=/source=[/default=]`。
const UI_BINDINGS_FIELDS: &[FieldSchema] = &[
    // 与 `BoundProperty::parse` 的三个变体一一对应（未知名在引擎里会退化成
    // `MissingTargetOrProperty`，所以这里用 `Enum` 精确表达）。
    FieldSchema::new("property", FieldKind::Enum, true).with_choices(&[
        "text.text",
        "image.visible",
        "progress.value",
    ]),
    // 引擎对空 `target` 直接报错（`MissingTargetOrProperty`）。
    FieldSchema::new("target", FieldKind::Text, true).with_rules(&[FieldRule::NonEmpty]),
    FieldSchema::new("source", FieldKind::Text, true),
    FieldSchema::new("default", FieldKind::Text, false),
];

const UI_BINDINGS_LIST: ListModel = ListModel {
    separator: Some('|'),
    positional: &[],
    dynamic: &[],
};

/// `ColliderDesc::from_component_str`（组件名 `Collider`）：2D 碰撞体——`Collider3d` 的孪生。
///
/// 事实源同样是**解析器**（2D 也没有 `to_component_str`），样本取真实工程里出现过的形态
/// （`examples/game_api/scenes/api_prefab.scene` 的史莱姆实例：`shape=ball;radius=12`），由
/// `builtin_schemas_match_engine_encoders` 末尾的显式对拍确认"schema 接受的串引擎也接受"。
///
/// required 判定（逐条对照解析器的 `ok_or`）：
/// - `shape` 缺席 → 解析器报缺 shape → **required**；`shape` 只认 `ball`/`cuboid`（→ `Enum`）。
/// - `radius`/`half_width`/`half_height` 缺席各自有默认值，且"`radius` 只跟 `shape=ball` 搭配"
///   是**跨字段**校验（引擎报 `radius needs shape=ball`），扁平模型表达不了 → optional
///   （与 `Collider3d` 同一处理，标 required 会对 `shape=cuboid` 的真实串误报）。
/// - `sensor`/`dynamic`/`ccd` 用 `Bool`（与 `Collider3d` 保持一致；引擎只认 `true`/`false`，
///   因此这里**比引擎松**：多接受 `1`/`0`，偏松只漏报不误报）。
const COLLIDER_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("shape", FieldKind::Enum, true).with_choices(&["ball", "cuboid"]),
    FieldSchema::new("radius", FieldKind::Number, false),
    FieldSchema::new("half_width", FieldKind::Number, false),
    FieldSchema::new("half_height", FieldKind::Number, false),
    FieldSchema::new("offset", FieldKind::Vec2, false),
    FieldSchema::new("sensor", FieldKind::Bool, false),
    FieldSchema::new("dynamic", FieldKind::Bool, false),
    FieldSchema::new("layer", FieldKind::Int, false),
    FieldSchema::new("mask", FieldKind::Int, false),
    FieldSchema::new("ccd", FieldKind::Bool, false),
];

/// `Animation3dDescriptor::from_component_str` / `to_component_str`（组件名 `Animation3D`）：
/// `mesh=<模型路径>;clip=<片段名>;looped=<true|false>`。
///
/// 这是**3D 蒙皮动画**组件，与 2D 的 `Animation`（状态机 + 位置式 `clip:` 段）是两个组件，
/// 只是名字相近——合并它们会让"这个实体播的是精灵图还是骨骼动画"变成隐式推断。
///
/// required 判定：`mesh` 缺席 → `MissingMesh`、`clip` 缺席 → `missing clip` → 都 required；
/// `looped` 缺席时引擎取默认 `true`（编码器总会写出它）→ optional。
/// `looped` 用 `Enum{["true","false"]}`：引擎只认这两个字面量，`Bool` 会多接受 `1`/`0`。
const ANIMATION_3D_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("mesh", FieldKind::Text, true).with_rules(&[
        FieldRule::NonEmpty,
        FieldRule::ProjectPath {
            extensions: &["glb", "gltf"],
            sentinels: &[crate::core::mesh3d::GROUND_PLANE_SENTINEL],
        },
    ]),
    FieldSchema::new("clip", FieldKind::Text, true).with_rules(&[FieldRule::NonEmpty]),
    FieldSchema::new("looped", FieldKind::Enum, false).with_choices(&["true", "false"]),
];

/// `UiAnimation`（第 40 轮）：`|` 分隔的多条轨道，每条是动态键 `track.<属性名>`。
///
/// 事实源是引擎解析器 `UiAnimationClip::tracks_from_component_str`（第 40 轮从编辑器搬进引擎，
/// 编辑器改为调用它）。**schema 只校验"值非空"**：键帧三元组
/// `<时间>:<值>:<缓动>` 的语法与"时间严格递增"由那个解析器负责——写入路径上的
/// `typed_scene::validate_component_payload` 会跑它，因此编辑器不会写出非法串；
/// 但审计层（只跑 schema）对这条是**偏松**的：只漏报，不误报。
/// 为什么不为它加一条 `FieldRule`：那会把三元组语法复制成第二份实现，正是本轮
/// 要把 `UiAnimation` 收进引擎所消除的东西。
const UI_ANIMATION_DYNAMIC: &[DynamicField] = &[DynamicField {
    prefix: "track.",
    field: FieldSchema::new("track", FieldKind::Text, true).with_rules(&[FieldRule::NonEmpty]),
}];

const UI_ANIMATION_LIST: ListModel = ListModel {
    separator: Some('|'),
    positional: &[],
    dynamic: UI_ANIMATION_DYNAMIC,
};

/// 内置 schema 注册表：新增结构化组件应加在这里，而不是继续长 ad hoc 解析分支。
#[must_use]
pub fn builtin_registry() -> BTreeMap<&'static str, ComponentSchema> {
    let schemas = [
        // **唯一的变换组件**：2D 与 3D 用同一个名字、同一个（3D）形态；空间由实体
        // 携带的其它组件推断（`spatial_transform::entity_space`），2D 消费者用
        // `LocalTransform::to_2d` 投影。旧名 `Transform3D` 已删除。
        ComponentSchema::new("Transform", 2, TRANSFORM_FIELDS),
        ComponentSchema::new("Text", 2, TEXT_FIELDS),
        ComponentSchema::new("Image", 2, IMAGE_FIELDS),
        ComponentSchema::new("UiLayout", 1, UI_LAYOUT_FIELDS),
        // **统一相机**：判别字段 + 两套字段并集；旧名 `Camera2D`/`Camera3D` 已删除。
        ComponentSchema::new("Camera", 1, CAMERA_FIELDS),
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
        // **键名必须是运行时读的那个名字**（第 39 轮修的真实缺陷）：这里原来写的是
        // `Particle`，而运行时（`typed_scene` 的编译表、`script.rs`、`renderable.rs`）
        // 读的是 `ParticleEmitter`。后果与当年的 `Script`/`EntityScript` 一模一样：
        // 真实工程里的粒子组件一直被审计报"没有 schema"，而注册表里那份 schema
        // 描述的是**没人消费的组件名**。门见
        // `every_component_id_the_runtime_knows_has_a_schema`（机械化版本）。
        ComponentSchema::new("ParticleEmitter", 1, PARTICLE_FIELDS),
        // 第 39 轮：审计的 `component_no_schema` 明细暴露的两个真实缺口。
        ComponentSchema::new("Collider", 1, COLLIDER_FIELDS),
        ComponentSchema::new("Animation3D", 1, ANIMATION_3D_FIELDS),
        // 第 26 轮：补齐有手写面板、但此前没有 schema 的 4 个组件。
        ComponentSchema::new("Collider3d", 1, COLLIDER_3D_FIELDS),
        ComponentSchema::new("Tilemap", 1, TILEMAP_FIELDS),
        ComponentSchema::new("Container", 1, CONTAINER_FIELDS),
        ComponentSchema::new("ButtonStates", 1, BUTTON_STATES_FIELDS),
        // 第 38 轮：列表/重复字段模型落地后补上的 3 个；第 40 轮补上 `UiAnimation`
        // （先把它的格式搬进引擎 `ui_animation.rs`，schema 才有对拍对象，见 §3.33/§3.35）。
        ComponentSchema::new("Animation", 1, ANIMATION_FIELDS).with_list(ANIMATION_LIST),
        ComponentSchema::new("UiEvents", 1, UI_EVENTS_FIELDS).with_list(UI_EVENTS_LIST),
        ComponentSchema::new("UiBindings", 1, UI_BINDINGS_FIELDS).with_list(UI_BINDINGS_LIST),
        ComponentSchema::new("UiAnimation", 1, &[]).with_list(UI_ANIMATION_LIST),
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
        .and_then(|schema| schema.parse_payload(raw).map(|_| ()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        camera::{Camera, Camera2D, Camera3D},
        entity_script::EntityScript,
        light::Light,
        mesh3d::MeshRenderer,
        particle::ParticleEmitter,
        physics3d::ColliderDesc3d,
        quad::Quad,
        render_mode::RenderMode,
        sprite::SpriteComponent,
        tilemap::Tilemap,
        transform::Transform3D,
        ui_binding::{BindingPath, BindingSet, BoundProperty, PropertyBinding},
        ui_events::{UiEventBinding, UiEventType, UiEvents},
        ui_layout::{ChildAlign, ContainerKind, ContainerStyle},
        ui_states::ButtonStateSet,
        visibility::Visibility,
        world_text::WorldText,
    };

    /// `Collider3d` 的真实工程实例（`examples/game_api/scenes/api_3d.scene`）：
    /// 三种形状各自出现过的完整形态，逐字节照抄。
    ///
    /// 引擎侧**没有** `ColliderDesc3d::to_component_str`（碰撞体只在实例化时解析一次），
    /// 所以漂移门对它的证据分两半：这些**真实串**必须过 schema（下面的 `cases`），
    /// 同时必须过**引擎解析器**（本测试末尾的显式对拍）——"schema 接受 = 引擎接受"。
    const COLLIDER_3D_INSTANCES: &[&str] = &[
        // cuboid 没有 radius；capsule 没有 half_width/half_z；ball 只有 radius。
        "shape=cuboid;half_width=4;half_height=0.1;half_z=4;offset=0,-0.1,0;dynamic=false",
        "shape=cuboid;half_width=0.2;half_height=1;half_z=4;dynamic=false",
        "shape=capsule;half_height=0.8;radius=0.4;dynamic=false;sensor=true",
        "shape=ball;radius=0.4;dynamic=false;sensor=true",
        "shape=ball;radius=0.5;dynamic=true",
    ];

    /// **运行时认识的每个组件名都必须有 schema**（第 39 轮）：机械化地防住"键名漂移"。
    ///
    /// 这条门来自两个**真实缺陷**：`EntityScript`/`Script`（第 3 轮，见
    /// `script_schema_uses_the_runtime_component_name`）与 `Particle`/`ParticleEmitter`
    /// （第 39 轮，审计的 `component_no_schema` 明细才暴露）。两次都是同一个形态：
    /// 注册表描述了一个**没人消费**的名字，而真实工程里的组件被报"没有 schema"——
    /// 前者让 schema 成为空转，后者让真实实例失去校验。
    ///
    /// 逐个写断言挡不住第三例（`Particle` 那次就是漏了），所以这里直接遍历
    /// `typed_scene::ComponentId::ALL`：新增运行时组件却忘了注册 schema，这条立刻判红。
    #[test]
    fn every_component_id_the_runtime_knows_has_a_schema() {
        use crate::core::typed_scene::ComponentId;
        let registry = builtin_registry();
        let missing: Vec<&str> = ComponentId::ALL
            .iter()
            .map(|id| id.as_str())
            .filter(|name| !registry.contains_key(name))
            .collect();
        assert!(
            missing.is_empty(),
            "运行时认识这些组件名，但 schema 注册表里没有：{missing:?}"
        );
        // 名字还必须**唯一**：一个 `ComponentId` 一个名字（重复会让上面那条门形同虚设）。
        let mut names: Vec<&str> = ComponentId::ALL.iter().map(|id| id.as_str()).collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(names.len(), unique, "ComponentId::as_str 出现了重复名字");
        // 第 39 轮修掉的漂移名不得复活。
        assert!(
            !registry.contains_key("Particle"),
            "旧键名 `Particle` 不得复活：运行时读的是 `ParticleEmitter`"
        );
        assert!(registry.contains_key("ParticleEmitter"));
    }

    /// `Collider`（2D）的真实工程实例（`examples/game_jyht/scenes/*.scene`，31+7 个实例、
    /// 形态一致）：与 `COLLIDER_3D_INSTANCES` 同一分工——2D 也没有编码器，所以真实串
    /// 既要过 schema，也要由引擎解析器确认。
    const COLLIDER_INSTANCES: &[&str] = &[
        "shape=ball;radius=12",
        "shape=cuboid;half_width=8;half_height=4;sensor=true",
    ];

    /// `Animation3D` 的真实工程实例（`examples/game_api/scenes/api_3d.scene`、
    /// `assets/models/`）：`mesh` 是 `glb` 工程内相对路径。
    const ANIMATION_3D_INSTANCES: &[&str] = &[
        "mesh=assets/models/kaykit-warrior.glb;clip=Idle;looped=true",
        "mesh=assets/models/kaykit-warrior.glb;clip=Running_A;looped=true",
    ];

    /// 漂移门：每个 schema 都必须能解析**引擎自己编码器**写出的字符串，
    /// 且解析 → 再序列化 → 再解析 保持不变。
    ///
    /// 这是 docs/55 §1 第 3 条（"Inspector、校验、迁移和运行时 schema 容易漂移"）
    /// 的机械化拦截：字段改名/改类型/改分隔符都会让这个测试直接判红。
    #[test]
    fn builtin_schemas_match_engine_encoders() {
        let registry = builtin_registry();
        let cases: Vec<(&str, Vec<String>)> = vec![
            // **唯一的变换组件**：2D 与 3D 都是这一份（3D 形态），编码器只有 `Transform3D`。
            (
                "Transform",
                vec![Transform3D::default().to_component_str()],
            ),
            // **统一相机**：判别字段 + 各自投影的字段体，两种投影都必须过同一份 schema。
            (
                "Camera",
                vec![
                    Camera::Orthographic(
                        Camera2D::new(crate::core::transform::Vec2::new(0.0, 0.0), 1.0)
                            .expect("valid camera"),
                    )
                    .to_component_str(),
                    Camera::Perspective(
                        Camera3D::perspective(
                            crate::core::math3d::Vec3::new(0.0, 1.0, 3.0),
                            crate::core::math3d::Vec3::new(0.0, 0.0, 0.0),
                            60.0_f32.to_radians(),
                            0.1,
                            100.0,
                        )
                        .expect("valid camera"),
                    )
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
                "ParticleEmitter",
                vec![ParticleEmitter::default().to_component_str()],
            ),
            // 第 26 轮补齐的 4 个组件。`Collider3d` 只有解析器（见 `COLLIDER_3D_INSTANCES`），
            // 其余三个用引擎自己的编码器输出。
            (
                "Collider3d",
                COLLIDER_3D_INSTANCES
                    .iter()
                    .map(|payload| (*payload).to_owned())
                    .collect(),
            ),
            (
                "Tilemap",
                vec![
                    {
                        // 引擎编码器：`tiles`/`solid` 的行优先字符序列由它自己生成。
                        let mut map = Tilemap::new(crate::core::transform::Vec2::new(32.0, 32.0), 4, 3)
                            .expect("valid tilemap");
                        map.set_cell(1, 1, 2).expect("in bounds");
                        map.set_solid(1, 1, true).expect("in bounds");
                        map.texture = "assets/custom-tileset.png".to_owned();
                        map.to_component_str()
                    },
                    // 真实工程形态（`examples/game_api/scenes/api_gameplay.scene` 的地块）。
                    "tile_size=32x32;width=8;height=5;texture=assets/textures/tileset.png;tiles=1111111112222221122322211222222111111111;solid=1111111110000001100100011000000111111111"
                        .to_owned(),
                ],
            ),
            (
                "Container",
                vec![
                    ContainerStyle::default().to_component_str(),
                    ContainerStyle {
                        kind: ContainerKind::Grid,
                        padding: [1.0, 2.0, 3.0, 4.0],
                        spacing: 8.0,
                        columns: 3,
                        stretch: true,
                        fill: true,
                        align: ChildAlign::Center,
                    }
                    .to_component_str(),
                    // 真实工程形态（`engine/crates/engine/src/core/typed_scene.rs` 的样本串）。
                    "kind=vbox;padding=0,0,0,0;spacing=4;columns=1;stretch=true;fill=false;align=center"
                        .to_owned(),
                    // 真实工程形态（`examples/game_api/scenes/ui_test.scene` 的面板）：
                    // 编辑器手写面板编码出来的定长小数（`padding=.0000`、`spacing=.000000`）。
                    "kind=vbox;padding=24.0000,24.0000,24.0000,24.0000;spacing=16.000000;columns=0;stretch=false;fill=false;align=center"
                        .to_owned(),
                ],
            ),
            (
                "ButtonStates",
                vec![
                    // 默认值编码是**空串**：没有任何状态覆盖时引擎就写空串（解析器接受）。
                    ButtonStateSet::default().to_component_str(),
                    ButtonStateSet::from_component_str(
                        "normal=assets/ui/btn.png;hover=assets/ui/btn_hover.png;pressed=assets/ui/btn_pressed.png;disabled=assets/ui/btn_disabled.png;focused=assets/ui/btn_focused.png;disabled_state=true",
                    )
                    .expect("valid button states")
                    .to_component_str(),
                    // 真实工程形态（`examples/game_api/scenes/ui_test.scene` 的 ButtonStates）：
                    // `__white__` 是内置白色贴图哨兵，不是路径。
                    "normal=__white__;disabled=__white__;disabled_state=true".to_owned(),
                ],
            ),
            (
                "Animation3D",
                // 引擎编码器写出的两种形态 + 真实工程串（`clip` 名大小写敏感）。
                vec![
                    crate::core::animation3d::Animation3dDescriptor {
                        mesh: "assets/models/kaykit-warrior.glb".to_owned(),
                        clip: "Idle".to_owned(),
                        looped: true,
                    }
                    .to_component_str(),
                    crate::core::animation3d::Animation3dDescriptor {
                        mesh: "assets/models/hero.glb".to_owned(),
                        clip: "Walk".to_owned(),
                        looped: false,
                    }
                    .to_component_str(),
                    ANIMATION_3D_INSTANCES[1].to_owned(),
                ],
            ),
            (
                "Collider",
                COLLIDER_INSTANCES
                    .iter()
                    .map(|instance| (*instance).to_owned())
                    .collect(),
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

        // `Collider3d` 的**另一半证据**：引擎没有编码器，所以上面那些真实串必须由
        // 引擎解析器亲口确认"确实合法"——否则"schema 能接受"可能只是接受了一个
        // 引擎根本不认的串（那正是反向漂移）。
        for payload in COLLIDER_3D_INSTANCES {
            ColliderDesc3d::from_component_str(payload).unwrap_or_else(|error| {
                panic!("Collider3d 样本必须是引擎解析器接受的串：{payload:?} → {error}")
            });
        }
        // 2D `Collider` 同理（第 39 轮补上 schema）。
        for payload in COLLIDER_INSTANCES {
            crate::core::physics::ColliderDesc::from_component_str(payload).unwrap_or_else(
                |error| panic!("Collider 样本必须是引擎解析器接受的串：{payload:?} → {error}"),
            );
        }
        // `Animation3D` 的真实工程串也必须由引擎解析器确认（schema 的 required 判定依赖它）。
        for payload in ANIMATION_3D_INSTANCES {
            crate::core::animation3d::Animation3dDescriptor::from_component_str(payload)
                .unwrap_or_else(|error| {
                    panic!("Animation3D 样本必须是引擎解析器接受的串：{payload:?} → {error}")
                });
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
        let parsed = registry[name]
            .parse(&encoded)
            .expect("engine encoder output");
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

    /// **列表/重复字段模型的正向证据**（第 38 轮）：`Animation`（位置式 `clip:` 段）、
    /// `UiEvents`/`UiBindings`（`|` 分隔的重复记录）都按"引擎编码器写出来的串"对拍：
    /// schema 能解析、能**逐字节**写回、再解析保持不变，且 `validate_component` 放行。
    ///
    /// 为什么逐字节要求在这里是合理的（而单条记录组件只要求语义稳定）：这些串的
    /// 位置式段与记录顺序**本身携带信息**（第 2 个 `clip:` 就是第 2 个状态），
    /// schema 保留原文而不是重新格式化，因此"引擎写什么、schema 就写回什么"成立。
    #[test]
    fn repeated_field_schemas_round_trip_like_their_engines() {
        use crate::core::animation::AnimationStateMachine;

        let registry = builtin_registry();
        let round_trip = |component: &str, encoded: &str| {
            let schema = registry
                .get(component)
                .unwrap_or_else(|| panic!("{component} 缺少 schema"));
            assert!(
                schema.list.is_some(),
                "{component} 必须是列表模型（否则又退回扁平键值映射）"
            );
            let payload = schema.parse_payload(encoded).unwrap_or_else(|error| {
                panic!("{component}: 解析引擎串 {encoded:?} 失败：{error}")
            });
            assert_eq!(
                schema.serialize_payload(&payload),
                encoded,
                "{component}: schema 写回必须与引擎编码器逐字节一致"
            );
            assert_eq!(
                schema
                    .parse_payload(&schema.serialize_payload(&payload))
                    .expect("再解析"),
                payload,
                "{component}: 解析 → 序列化 → 解析 必须保持不变"
            );
            assert!(
                validate_component(&registry, component, encoded).is_ok(),
                "{component}: 审计入口必须放行引擎自己写的串"
            );
            // 列表模型**没有**扁平映射：老入口必须报出明确原因，而不是静默折叠。
            let error = schema
                .parse(encoded)
                .expect_err("列表模型不该有扁平键值映射");
            assert_eq!(error.field, "<list>", "{component}: {error}");
            payload
        };

        // ① `Animation`：真实串来自 `examples/game_api/assets/prefabs/slime_red.prefab`；
        //    第二条覆盖"多个 `clip:` 段 + 可选第 6 段 `events=`（内部还有冒号）"。
        for sample in [
            "default=squish;clip:squish:assets/textures/slime.png:2:3:true",
            "default=idle;clip:idle:a.png:1:1:false;clip:walk:a.png:4:8:true:events=2:step,3:jump",
        ] {
            let machine = AnimationStateMachine::from_component_str(sample)
                .unwrap_or_else(|error| panic!("样本必须是引擎接受的串：{sample:?} → {error}"));
            round_trip("Animation", &machine.to_component_str());
        }
        let animation = registry["Animation"].clone();
        // 位置式段的**否定**证据（都是引擎解析器也会拒的形态）。
        for bad in [
            // 缺 required 的 `default`（引擎：`missing default`）。
            "clip:a:b:2:3:true",
            // `looped` 只有 true/false（引擎：`clip looped must be true/false`）。
            "default=x;clip:a:b:2:3:maybe",
            // 位置参数不够（引擎：`clip needs name:sheet:cols:fps:looped`）。
            "default=x;clip:a:b:2:true",
        ] {
            assert!(
                animation.parse_payload(bad).is_err(),
                "{bad:?} 必须被拒（与引擎解析器同口径）"
            );
        }

        // ② `UiEvents`：两条绑定同名键重复出现，且第二条带动态键 `arg.amount`。
        let events = UiEvents {
            bindings: vec![
                UiEventBinding {
                    event: Some(UiEventType::Click),
                    target: None,
                    function: "on_start".to_owned(),
                    arguments: BTreeMap::new(),
                },
                UiEventBinding {
                    event: Some(UiEventType::ValueChanged),
                    target: Some("Icon".to_owned()),
                    function: "on_settings".to_owned(),
                    arguments: BTreeMap::from([("amount".to_owned(), "2".to_owned())]),
                },
            ],
        };
        let encoded = events.to_component_str();
        assert_eq!(encoded.matches("function=").count(), 2, "{encoded}");
        let payload = round_trip("UiEvents", &encoded);
        assert_eq!(
            payload,
            ComponentPayload::Entries(vec![
                ComponentEntry {
                    fields: ComponentData::from([
                        ("event".to_owned(), FieldValue::Choice("click".to_owned())),
                        (
                            "function".to_owned(),
                            FieldValue::Text("on_start".to_owned())
                        ),
                    ]),
                    positional: Vec::new(),
                },
                ComponentEntry {
                    fields: ComponentData::from([
                        ("arg.amount".to_owned(), FieldValue::Text("2".to_owned())),
                        (
                            "event".to_owned(),
                            FieldValue::Choice("value_changed".to_owned())
                        ),
                        (
                            "function".to_owned(),
                            FieldValue::Text("on_settings".to_owned())
                        ),
                        ("target".to_owned(), FieldValue::Text("Icon".to_owned())),
                    ]),
                    positional: Vec::new(),
                },
            ]),
            "两条绑定必须都在（旧模型的静默折叠就死在这里）"
        );
        // 真实工程串（`runtime_scene.rs` 的样本）。
        round_trip("UiEvents", "event=click;function=on_click");
        for bad in [
            // 引擎：`MissingFunction`。
            "event=click;target=Icon",
            // 引擎：`UnknownEvent`。
            "event=bogus;function=f",
            // 引擎：`UnknownField`。
            "function=f;nope=1",
        ] {
            assert!(
                registry["UiEvents"].parse_payload(bad).is_err(),
                "{bad:?} 必须被拒"
            );
        }

        // ③ `UiBindings`：同一形态，且 `property` 只能是三个已知值之一。
        let bindings = BindingSet {
            bindings: vec![
                PropertyBinding {
                    property: Some(BoundProperty::TextText),
                    target: "Title".to_owned(),
                    source: BindingPath(vec!["vm".to_owned(), "title".to_owned()]),
                    default: None,
                },
                PropertyBinding {
                    property: Some(BoundProperty::ImageVisible),
                    target: "Icon".to_owned(),
                    source: BindingPath(vec!["vm".to_owned(), "visible".to_owned()]),
                    default: Some("true".to_owned()),
                },
            ],
        };
        let encoded = bindings.to_component_str();
        assert_eq!(encoded.matches("target=").count(), 2, "{encoded}");
        round_trip("UiBindings", &encoded);
        for bad in [
            "property=bogus;target=t;source=s",
            "property=text.text;source=s",
            "property=text.text;target=t",
            // 空 target 引擎也拒（`MissingTargetOrProperty`）。
            "property=text.text;target=;source=s",
        ] {
            assert!(
                registry["UiBindings"].parse_payload(bad).is_err(),
                "{bad:?} 必须被拒"
            );
        }

        // ④ `UiAnimation`（第 40 轮）：格式的唯一事实源已搬进引擎
        // （`UiAnimationClip::tracks_from_component_str` / `tracks_to_component_str`），
        // 因此这里可以对拍"引擎写什么 → schema 读什么 → schema 写回什么"。
        use crate::core::ui_animation::{UiAnimationClip, UiAnimationTrack, UiEasing, UiKeyframe};
        let mut tracks = std::collections::BTreeMap::new();
        tracks.insert(
            "opacity".to_owned(),
            UiAnimationTrack::new(
                "opacity",
                vec![
                    UiKeyframe {
                        time: 0.0,
                        value: 0.0,
                        easing: UiEasing::Linear,
                    },
                    UiKeyframe {
                        time: 0.5,
                        value: 1.0,
                        easing: UiEasing::SmoothStep,
                    },
                ],
            )
            .expect("valid track"),
        );
        tracks.insert(
            "scale".to_owned(),
            UiAnimationTrack::new(
                "scale",
                vec![UiKeyframe {
                    time: 0.0,
                    value: 2.0,
                    easing: UiEasing::Step,
                }],
            )
            .expect("valid track"),
        );
        let encoded = UiAnimationClip::tracks_to_component_str(&tracks);
        let payload = round_trip("UiAnimation", &encoded);
        assert_eq!(
            payload,
            ComponentPayload::Entries(vec![
                ComponentEntry {
                    fields: ComponentData::from([(
                        "track.opacity".to_owned(),
                        FieldValue::Text("0.000:0.000:linear,0.500:1.000:smoothstep".to_owned()),
                    )]),
                    positional: Vec::new(),
                },
                ComponentEntry {
                    fields: ComponentData::from([(
                        "track.scale".to_owned(),
                        FieldValue::Text("0.000:2.000:step".to_owned()),
                    )]),
                    positional: Vec::new(),
                },
            ]),
            "两条轨道必须都在（动态键各自成一条记录）"
        );
        // 编辑器一直写的真实形态（`ui_editor` 的预览）。
        round_trip(
            "UiAnimation",
            "track.opacity=0.000:0.000:linear|track.scale=0.000:1.000:smoothstep",
        );
        // schema 这一层是**偏松**的（只查值非空）：语法错误由引擎解析器在写入路径上拦。
        assert!(
            registry["UiAnimation"]
                .parse_payload("track.opacity=0.000:0.000")
                .is_ok(),
            "schema 层只查非空——三元组语法归引擎解析器，这条断言把这个边界钉住"
        );
        assert!(
            UiAnimationClip::tracks_from_component_str("track.opacity=0.000:0.000").is_err(),
            "引擎解析器必须拒绝同一个串（写入路径的兜底门）"
        );
        // 空值（`track.opacity=`）两边都拒：schema 的 `NonEmpty` 与解析器的键帧校验同口径。
        assert!(
            registry["UiAnimation"]
                .parse_payload("track.opacity=")
                .is_err()
        );
    }

    /// **扁平平面的朴素 schema 为什么不行**：这段用临时 schema（不进注册表）演示
    /// "位置式段/重复键/动态键"三种形态在旧模型下的真实失败样子。
    ///
    /// 第 38 轮起 `Animation`/`UiEvents`/`UiBindings`，第 40 轮起 `UiAnimation` 都已由
    /// `ListModel` 接管（正向证据在 `repeated_field_schemas_round_trip_like_their_engines`）；
    /// 本测试保留这些否定对照，防止有人把 `ListModel` 又改回"按 `;` 切 `key=value`"。
    #[test]
    fn flat_model_cannot_express_positional_or_repeated_fields() {
        // 只为演示"表达不了"而临时拼的**朴素** schema（不进注册表，也不导出）。
        const NAIVE_ANIMATION_FIELDS: &[FieldSchema] =
            &[FieldSchema::new("default", FieldKind::Text, true)];
        const NAIVE_UI_EVENTS_FIELDS: &[FieldSchema] = &[
            FieldSchema::new("event", FieldKind::Enum, false).with_choices(&["click"]),
            FieldSchema::new("target", FieldKind::Text, false),
            FieldSchema::new("function", FieldKind::Text, false),
            FieldSchema::new("arg.amount", FieldKind::Text, false),
        ];
        let registry = builtin_registry();
        // 四个"说不出来"的组件**全部**已注册，且都是列表模型（否则又会退回扁平映射）。
        for component in ["Animation", "UiEvents", "UiBindings", "UiAnimation"] {
            let schema = registry
                .get(component)
                .unwrap_or_else(|| panic!("{component} 必须已注册"));
            assert!(
                schema.list.is_some(),
                "{component} 必须是列表模型（动态键/重复键表达不了扁平映射）"
            );
        }

        // ① `Animation`：位置式 `clip:<name>:<sheet>:<cols>:<fps>:<looped>[:events=..]`，
        // 段里根本没有 `=`，而且一个状态机可以带**多个** `clip:` 段（重复键）。
        // 真实串来自 `examples/game_api/assets/prefabs/slime_red.prefab`。
        let animation = "default=squish;clip:squish:assets/textures/slime.png:2:3:true";
        assert!(
            crate::core::animation::AnimationStateMachine::from_component_str(animation).is_ok(),
            "这是引擎接受的真实串"
        );
        let naive_animation = ComponentSchema::new("Animation", 1, NAIVE_ANIMATION_FIELDS);
        let error = naive_animation
            .parse(animation)
            .expect_err("位置式 `clip:` 段没有 `=`，schema 模型读不了");
        assert_eq!(
            error.field,
            "clip:squish:assets/textures/slime.png:2:3:true"
        );

        // ② `UiEvents`：`|` 分隔多条绑定 → `event`/`function`/`target` 各出现多次。
        // `BTreeMap` 会**静默折叠**成一条（后写覆盖先写），往返序列化直接丢绑定——
        // 比"读不了"更危险，因此必须是跳过而不是硬塞。
        let events = UiEvents {
            bindings: vec![
                UiEventBinding {
                    event: Some(UiEventType::Click),
                    target: None,
                    function: "on_start".to_owned(),
                    arguments: BTreeMap::new(),
                },
                UiEventBinding {
                    event: Some(UiEventType::Click),
                    target: Some("Icon".to_owned()),
                    function: "on_settings".to_owned(),
                    // 动态键名（`arg.<任意名字>`）：固定字段表也表达不了。
                    arguments: BTreeMap::from([("amount".to_owned(), "2".to_owned())]),
                },
            ],
        };
        let encoded = events.to_component_str();
        assert_eq!(encoded.matches("function=").count(), 2, "{encoded}");
        let naive_events = ComponentSchema::new("UiEvents", 1, NAIVE_UI_EVENTS_FIELDS);
        let parsed = naive_events
            .parse(&encoded)
            .expect("BTreeMap 会静默折叠重复键（问题正在于此）");
        assert_ne!(
            naive_events.serialize(&parsed),
            encoded,
            "重复键被折叠 → 往返丢绑定"
        );

        // ③ `UiBindings`：同一形态（`property`/`target`/`source` 每条绑定各出现一次）。
        let bindings = BindingSet {
            bindings: vec![
                PropertyBinding {
                    property: Some(BoundProperty::TextText),
                    target: "Title".to_owned(),
                    source: BindingPath(vec!["vm".to_owned(), "title".to_owned()]),
                    default: None,
                },
                PropertyBinding {
                    property: Some(BoundProperty::ImageVisible),
                    target: "Icon".to_owned(),
                    source: BindingPath(vec!["vm".to_owned(), "visible".to_owned()]),
                    default: Some("true".to_owned()),
                },
            ],
        };
        let encoded = bindings.to_component_str();
        assert!(encoded.contains('|'), "{encoded}");
        assert_eq!(encoded.matches("target=").count(), 2, "{encoded}");

        // ④ `UiAnimation`：`track.<属性名>=<time>:<value>:<easing>,…`，键名来自作者内容
        // （`track.opacity`/`track.scale`/…），**动态键集合** + `|` 分隔多条轨道。
        // 第 40 轮起这份格式的唯一事实源在引擎 `UiAnimationClip::tracks_from_component_str`
        // （正向往返证据在 `repeated_field_schemas_round_trip_like_their_engines`）；
        // 这里保留"扁平模型读不了"的否定对照。
        let ui_animation = "track.opacity=0.000:0.000:linear|track.scale=0.000:1.000:smoothstep";
        let naive_ui_animation = ComponentSchema::new("UiAnimation", 1, &[]);
        assert!(
            naive_ui_animation.parse(ui_animation).is_err(),
            "`track.<属性>` 是动态键，固定字段表表达不了"
        );
        // 注册表里那份（动态键 `track.`）能解析，且解析出的记录数 = 轨道数。
        let entries = registry["UiAnimation"]
            .parse_payload(ui_animation)
            .expect("注册的 schema 必须能读引擎格式");
        match entries {
            ComponentPayload::Entries(entries) => assert_eq!(entries.len(), 2),
            ComponentPayload::Fields(_) => panic!("UiAnimation 必须是列表模型"),
        }
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

        // 相机：统一 `Camera` 的 `pose` 两个字面量必须与引擎解析器一致（`transform` / `look_at`）。
        let camera = &registry["Camera"];
        let camera_base =
            "projection=perspective;position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100";
        let camera_samples = [
            (camera_base.to_owned(), true),
            (format!("{camera_base};pose=transform"), true),
            // 与省略同义：编码器在 `pose_from_transform=false` 时不写这个字段，但引擎解析器
            // 认它——通用字段编辑器靠它把 `pose=transform` 改回去（下拉框没有"清空"）。
            (format!("{camera_base};pose=look_at"), true),
            (format!("{camera_base};pose=bogus"), false),
            (format!("{camera_base};enabled=false"), true),
            // 判别字段必填：缺了它 schema 与引擎读取器都必须拒绝。
            (
                "position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100".to_owned(),
                false,
            ),
        ];
        for (payload, expected_ok) in camera_samples {
            let engine = crate::core::camera::Camera::from_component_str(&payload);
            let schema = camera.parse(&payload);
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

        // 纹理引用（第 32 轮铺开的共用规则）：四个组件的 `texture` 都要求
        // "工程内相对路径 + png/jpg/jpeg/webp"，内置哨兵 `__white__` 放行。
        let texture_samples = [
            ("assets/textures/player.png", true),
            ("assets/textures/ui/panel.jpg", true),
            ("assets/art/characters/x.webp", true),
            ("__white__", true),
            ("assets/textures/player.bmp", false),
            ("../escape.png", false),
            ("/etc/x.png", false),
            ("", false),
        ];
        for (sample, expected_ok) in texture_samples {
            for (component, payload) in [
                (
                    "Sprite",
                    format!("texture={sample};size=32x32;tint=1,1,1,1;sorting_layer=default"),
                ),
                (
                    "Image",
                    if sample.is_empty() {
                        "texture=".to_owned()
                    } else {
                        format!("texture={sample};color=1,1,1")
                    },
                ),
            ] {
                let schema = &registry[component];
                // 空值对这两个组件是合法的"未指定"（引擎自己的默认编码就是 `texture=`）。
                let expected = expected_ok || sample.is_empty();
                assert_eq!(
                    schema.parse(&payload).is_ok(),
                    expected,
                    "{component} 对 texture={sample:?} 的判定与预期不符"
                );
            }
        }
        // **刻意更严**：引擎解析器接受任意文本（`.bmp`、绝对路径都能读），schema 拒绝——
        // 与旧手写 Sprite 面板的 `validate_asset_reference` 同一口径，运行时行为不变。
        assert!(
            crate::core::sprite::SpriteComponent::from_component_str(
                "texture=assets/x.bmp;size=1x1;tint=1,1,1,1;sorting_layer=default"
            )
            .is_ok(),
            "引擎解析器仍然接受它（不改运行时行为）"
        );
        assert!(
            registry["Sprite"]
                .parse("texture=assets/x.bmp;size=1x1;tint=1,1,1,1;sorting_layer=default")
                .is_err(),
            "schema 必须拒绝非图片扩展名（比引擎更严，与原编辑器一致）"
        );

        // **刻意更严**（与 `Script.file` 的 `.lua` 约定同一类）：负的九宫格边框引擎解析器接受，        // 但渲染侧会 `*v >= 0.0` 过滤掉（等于不启用），所以那是"写了也没用"的输入。
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

        // 第 26 轮补齐的 4 个组件：声明式规则必须与各自的引擎解析器同判。
        // `Tilemap.texture`：缺席 = 回退默认图集（optional），写了就必须非空。
        let tilemap = &registry["Tilemap"];
        let tilemap_base = "tile_size=32x32;width=2;height=2;tiles=0000;solid=0000";
        let tilemap_samples = [
            (tilemap_base.to_owned(), true),
            (
                format!("{tilemap_base};texture=assets/textures/tileset.png"),
                true,
            ),
            // 解析器：`texture must not be empty`。
            (format!("{tilemap_base};texture="), false),
            // `width`/`height` 解析成 `usize`：负数报错（schema 的 `NonNegative` 同一判定）。
            (
                "tile_size=32x32;width=-2;height=2;tiles=0000;solid=0000".to_owned(),
                false,
            ),
            // `tiles`/`solid` 不能为空：尺寸必须非零，空序列必然 `LengthMismatch`。
            (
                "tile_size=32x32;width=2;height=2;tiles=;solid=0000".to_owned(),
                false,
            ),
        ];
        for (payload, expected_ok) in tilemap_samples {
            let engine = crate::core::tilemap::Tilemap::from_component_str(&payload);
            let schema = tilemap.parse(&payload);
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

        // `Container`：全字段可选（空串 = 默认样式）、枚举字面量与 `columns` 的非负。
        let container = &registry["Container"];
        let container_samples = [
            (String::new(), true),
            ("kind=vbox".to_owned(), true),
            (
                "kind=grid;padding=1,2,3,4;spacing=8;columns=3;stretch=true;fill=false;align=center"
                    .to_owned(),
                true,
            ),
            // 解析器：`unknown Container kind` / `unknown align`。
            ("kind=unknown".to_owned(), false),
            ("align=middle".to_owned(), false),
            // 解析器：`columns` 是 `usize`，`columns=abc` 与负数都报错。
            ("columns=abc".to_owned(), false),
            ("columns=-1".to_owned(), false),
        ];
        for (payload, expected_ok) in container_samples {
            let engine = crate::core::ui_layout::ContainerStyle::from_component_str(&payload);
            let schema = container.parse(&payload);
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

        // `ButtonStates`：空的贴图值 = "没有这个覆盖"（解析器的 `non_empty` 返回 `None`），
        // 所以 schema 也**必须**接受它——这里刻意不加 `NonEmpty`，否则"清空状态"会变成误报。
        let button_states = &registry["ButtonStates"];
        let button_samples = [
            (String::new(), true),
            ("normal=base.png".to_owned(), true),
            // 真实工程形态（`examples/game_api/scenes/ui_test.scene` 的 ButtonStates）。
            (
                "normal=__white__;disabled=__white__;disabled_state=true".to_owned(),
                true,
            ),
            ("normal=".to_owned(), true),
            ("disabled_state=maybe".to_owned(), false),
            ("bad=1".to_owned(), false),
        ];
        for (payload, expected_ok) in button_samples {
            let engine = crate::core::ui_states::ButtonStateSet::from_component_str(&payload);
            let schema = button_states.parse(&payload);
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

        // `Collider3d`：schema 只强制 `shape`（解析器 `missing shape`），形状尺寸字段的
        // "按形状必填"是跨字段规则，两边都不在 schema 表达——这里钉住"两边都不误报
        // cuboid 缺 radius"这一条（真实工程 `game3d.scene` 里全是这种串）。
        let collider = &registry["Collider3d"];
        let collider_samples = [
            (
                "shape=ball;radius=0.5;dynamic=false;sensor=true".to_owned(),
                true,
            ),
            (
                "shape=cuboid;half_width=4;half_height=0.1;half_z=4;dynamic=false".to_owned(),
                true,
            ),
            (
                "shape=capsule;half_height=0.8;radius=0.4;ccd=true".to_owned(),
                true,
            ),
            // 解析器：`unknown shape` / `unknown field` / 缺 `shape`。
            ("shape=pyramid;radius=1".to_owned(), false),
            ("bogus=1".to_owned(), false),
            ("radius=1".to_owned(), false),
        ];
        for (payload, expected_ok) in collider_samples {
            let engine = ColliderDesc3d::from_component_str(&payload);
            let schema = collider.parse(&payload);
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
    }

    /// **自由文本转义**（第 25 轮）：`Text`/`WorldText` 的 `text`/`font` 里可以出现
    /// `;`/`=`/`\`，组件串按 `UiText` 的约定转义。schema 必须与引擎编码器**同一口径**，
    /// 否则含分号的文字会被审计判成漂移（`unknown field`）、也会被按字段写入写坏。
    ///
    /// 这条同时钉住"哪些字段标了 `escaped`"：漏标就会在这里红。
    #[test]
    fn escaped_text_fields_match_the_engine_encoder() {
        let text = crate::core::ui::UiText {
            text: "a;b=c\\d 中文".to_owned(),
            font_size: 16.0,
            color: [1.0, 0.5, 0.2],
            font: "assets/fonts/a;b=数字.ttf".to_owned(),
        };
        let payload = text.to_component_str();
        assert!(
            payload.contains("text=a\\;b\\=c\\\\d 中文"),
            "引擎编码器必须转义：{payload}"
        );
        for component in ["Text", "WorldText"] {
            // WorldText 的串多带 offset/centered；`Text` 的字段是它的子集，两者都能解析。
            let schema = &builtin_registry()[component];
            let parsed = schema.parse(&payload).unwrap_or_else(|error| {
                panic!("{component} schema 必须接受引擎编码的串：{error:?}")
            });
            assert_eq!(
                parsed["text"],
                FieldValue::Text("a;b=c\\d 中文".to_owned()),
                "{component}.text 必须是**反转义后**的真实文本"
            );
            assert_eq!(
                parsed["font"],
                FieldValue::Text("assets/fonts/a;b=数字.ttf".to_owned())
            );
            // 反向：schema 写出的串引擎必须能读回同样的值。
            let reserialized = schema.serialize(&parsed);
            let engine = crate::core::ui::UiText::from_component_str(&reserialized).unwrap_or_else(
                |error| panic!("{component} 的 schema 输出必须能被引擎读回：{error}"),
            );
            assert_eq!(engine.text, text.text);
            assert_eq!(engine.font, text.font);
        }
        // 没标转义的字段不会被转义：路径类字段保持逐字节原样（口径与它们自己的解析器一致）。
        let mesh = &builtin_registry()["MeshRenderer"];
        assert!(!mesh.has_escaped_fields());
        assert_eq!(
            mesh.serialize(&mesh.parse("mesh=assets/models/cube.glb").unwrap()),
            "mesh=assets/models/cube.glb"
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
        let particle =
            FieldSchema::new("count", FieldKind::Int, true).with_rules(&[FieldRule::NonNegative]);
        assert!(particle.validate("4", "ParticleEmitter").is_ok());
        assert!(particle.validate("-1", "ParticleEmitter").is_err());
        assert!(particle.validate("0", "ParticleEmitter").is_ok());
        assert!(
            FieldSchema::new("zoom", FieldKind::Number, true)
                .with_rules(&[FieldRule::NonNegative])
                .validate("-0.5", "Camera")
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
        assert!(
            registry["WorldText"]
                .parse("text=hello;font_size=18;offset=1,2")
                .is_ok()
        );
        assert!(
            registry["WorldText"]
                .parse("text=hello;font_size=18;offset=1")
                .is_err()
        );
    }

    /// 反向漂移：schema 里写错字段名会让引擎编码解析失败（防止 schema 单方面"发明"字段）。
    #[test]
    fn schemas_reject_fields_the_encoder_never_writes() {
        let registry = builtin_registry();
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
        // 统一 `Transform` 只有 3D 形态：`position`/`rotation`/`scale` 三个向量字段。
        let data = registry["Transform"]
            .parse("position=1,-2,0;rotation=0,0,0,1;scale=2,3,1")
            .unwrap();
        assert_eq!(data["position"], FieldValue::Tuple(vec![1.0, -2.0, 0.0]));
        assert_eq!(data["scale"], FieldValue::Tuple(vec![2.0, 3.0, 1.0]));
        assert!(!registry["Transform"].serialize(&data).is_empty());
    }

    #[test]
    fn errors_include_component_and_field_path() {
        let registry = builtin_registry();
        let error = registry["Transform"]
            .parse("position=bad,0,0;rotation=0,0,0,1;scale=1,1,1")
            .unwrap_err();
        assert_eq!(error.component, "Transform");
        assert_eq!(error.field, "position");
        assert!(
            error
                .to_string()
                .contains("component Transform field position")
        );
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
