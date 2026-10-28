use crate::asset::UiAssetPlan;
use crate::capabilities::Capabilities;
use crate::patch::{UiPatch, UiPatchOp};
use crate::spec::{
    BindingMode, CanvasSpec, SPEC_VERSION, StyleState, UiBindingSpec, UiLayoutSpec, UiNode,
    UiNodeKind, UiSpec, UiStyle,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Diagnostic {
    pub fn error(code: &str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: code.to_owned(),
            path: path.into(),
            key: None,
            message: message.into(),
            fix: None,
        }
    }

    pub fn warning(code: &str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: code.to_owned(),
            path: path.into(),
            key: None,
            message: message.into(),
            fix: None,
        }
    }

    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

const STYLE_PROPERTIES: &[&str] = &[
    "bg",
    "background",
    "color",
    "font",
    "font_size",
    "radius",
    "opacity",
    "border",
    "border_width",
    "border_color",
    "shadow",
    "nine_slice",
    "padding",
    "margin",
    "spacing",
    "align",
    "justify",
    "text_align",
    "tint",
    "cursor",
    "transition",
];

const BINDING_PROPERTIES: &[&str] = &[
    "text", "color", "src", "visible", "value", "checked", "enabled", "progress",
];

const PATCH_ROOT_FIELDS: &[&str] = &[
    "style",
    "layout",
    "events",
    "text",
    "src",
    "checked",
    "value",
    "rich_text",
    "handle",
    "placeholder",
    "nine_slice",
    "direction",
    "min",
    "max",
    "columns",
    "selected",
    "virtualized",
    "horizontal",
    "padding",
    "spacing",
    "justify",
    "align",
];

const LAYOUT_FIELDS: &[&str] = &[
    "anchor",
    "offsets",
    "size",
    "position",
    "pivot",
    "min_size",
    "max_size",
    "percent",
    "grow",
    "shrink",
    "align_self",
];

struct Context<'a> {
    caps: &'a Capabilities,
    tokens: &'a BTreeMap<String, Value>,
    styles: &'a BTreeMap<String, UiStyle>,
    assets: &'a BTreeSet<&'a str>,
}

impl Context<'_> {
    fn supported_nodes(&self) -> String {
        self.caps
            .node_types
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn supported_events(&self) -> String {
        self.caps
            .events
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn validate_spec(spec: &UiSpec, caps: &Capabilities) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if spec.spec_version != SPEC_VERSION {
        diagnostics.push(
            Diagnostic::error(
                "spec.version",
                "spec_version",
                format!(
                    "不支持的 spec_version {}（当前 {}）",
                    spec.spec_version, SPEC_VERSION
                ),
            )
            .with_fix(format!("改用 spec_version={SPEC_VERSION}")),
        );
    }
    if spec.document.name.trim().is_empty() {
        diagnostics.push(Diagnostic::error(
            "spec.document_name",
            "document.name",
            "文档名不能为空",
        ));
    }
    validate_canvas(&spec.document.canvas, &mut diagnostics);

    for (name, value) in &spec.tokens {
        validate_token(name, value, &mut diagnostics);
    }

    let mut asset_keys = BTreeSet::new();
    for (index, asset) in spec.assets.iter().enumerate() {
        let path = format!("assets[{index}]");
        if !asset_keys.insert(asset.key.as_str()) {
            diagnostics.push(
                Diagnostic::error(
                    "spec.asset_duplicate",
                    format!("{path}.key"),
                    format!("资产 key 重复：{}", asset.key),
                )
                .with_key(&asset.key),
            );
        }
        validate_asset(asset, &path, &mut diagnostics);
    }

    let context = Context {
        caps,
        tokens: &spec.tokens,
        styles: &spec.styles,
        assets: &asset_keys,
    };

    for (name, style) in &spec.styles {
        validate_style(name, style, &context, &mut diagnostics);
    }

    let mut keys = BTreeMap::new();
    validate_node(&spec.root, "root", &context, &mut keys, &mut diagnostics);

    for (index, binding) in spec.bindings.iter().enumerate() {
        validate_binding(binding, index, &context, &keys, &mut diagnostics);
    }

    diagnostics
}

fn validate_canvas(canvas: &CanvasSpec, diagnostics: &mut Vec<Diagnostic>) {
    for (axis, value) in ["width", "height"].iter().zip(canvas.reference) {
        if !value.is_finite() || value <= 0.0 {
            diagnostics.push(Diagnostic::error(
                "spec.canvas_reference",
                format!("document.canvas.reference.{axis}"),
                format!("参考分辨率 {axis} 必须为正数，实际 {value}"),
            ));
        }
    }
}

fn validate_token(name: &str, value: &Value, diagnostics: &mut Vec<Diagnostic>) {
    let path = format!("tokens.{name}");
    if name.trim().is_empty() {
        diagnostics.push(Diagnostic::error(
            "spec.token_name",
            path.clone(),
            "token 名不能为空",
        ));
    }
    validate_token_value(value, &path, diagnostics);
}

fn validate_token_value(value: &Value, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_token_value(item, &format!("{path}[{index}]"), diagnostics);
            }
        }
        Value::Object(_) => diagnostics.push(
            Diagnostic::error(
                "spec.token_shape",
                path,
                "token 只能是标量或标量数组（不支持对象）",
            )
            .with_fix("拆成多个平铺 token"),
        ),
    }
}

fn validate_style(
    name: &str,
    style: &UiStyle,
    context: &Context<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let path = format!("styles.{name}");
    if name.trim().is_empty() {
        diagnostics.push(Diagnostic::error(
            "spec.style_name",
            path.clone(),
            "样式名不能为空",
        ));
    }
    if style.base.is_empty() && style.states.is_empty() {
        diagnostics.push(Diagnostic::warning(
            "spec.style_empty",
            path.clone(),
            "样式既没有 base 也没有 states",
        ));
    }
    validate_style_map(&style.base, &format!("{path}.base"), context, diagnostics);
    if let Some(transition) = &style.transition {
        if !context.caps.transitions {
            diagnostics.push(
                Diagnostic::error(
                    "spec.transition_unsupported",
                    format!("{path}.transition"),
                    "运行时尚未插值状态过渡（hover/pressed 仍是瞬时切换）",
                )
                .with_fix("先接进轨道运行时，或删除 transition"),
            );
        }
        if !transition.duration.is_finite() || !(0.0..=10.0).contains(&transition.duration) {
            diagnostics.push(
                Diagnostic::error(
                    "spec.transition_duration",
                    format!("{path}.transition.duration"),
                    format!("过渡时长必须在 0..=10 秒，实际 {}", transition.duration),
                )
                .with_fix("Unity 的默认交互过渡是 0.1–0.3 秒"),
            );
        }
    }
    for (state, values) in &style.states {
        if !context.caps.supports_state(*state) {
            diagnostics.push(
                Diagnostic::error(
                    "spec.state_unsupported",
                    format!("{path}.states.{}", state_name(*state)),
                    format!("运行时尚未消费 {} 状态", state_name(*state)),
                )
                .with_fix("先补运行时状态系统，或删除该状态覆盖"),
            );
        }
        validate_style_map(
            values,
            &format!("{path}.states.{}", state_name(*state)),
            context,
            diagnostics,
        );
    }
}

fn validate_style_map(
    values: &BTreeMap<String, Value>,
    path: &str,
    context: &Context<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (property, value) in values {
        if !STYLE_PROPERTIES.contains(&property.as_str()) {
            diagnostics.push(
                Diagnostic::error(
                    "spec.style_property_unknown",
                    format!("{path}.{property}"),
                    format!("未知样式属性：{property}"),
                )
                .with_fix(format!("可用属性：{}", STYLE_PROPERTIES.join(", "))),
            );
        }
        validate_token_refs(value, &format!("{path}.{property}"), context, diagnostics);
    }
}

fn validate_token_refs(
    value: &Value,
    path: &str,
    context: &Context<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match value {
        Value::String(raw) => {
            let Some(name) = raw.strip_prefix('@') else {
                return;
            };
            if name.is_empty() || !context.tokens.contains_key(name) {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.token_missing",
                        path,
                        format!("引用了不存在的 token：{raw}"),
                    )
                    .with_fix("先在 tokens 里定义该令牌，或改用字面量"),
                );
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_token_refs(item, &format!("{path}[{index}]"), context, diagnostics);
            }
        }
        _ => {}
    }
}

fn validate_node(
    node: &UiNode,
    path: &str,
    context: &Context<'_>,
    keys: &mut BTreeMap<String, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if node.key.trim().is_empty() {
        diagnostics.push(Diagnostic::error(
            "spec.node_key_empty",
            format!("{path}.key"),
            "节点 key 不能为空",
        ));
    } else if let Some(previous) = keys.insert(node.key.clone(), path.to_owned()) {
        diagnostics.push(
            Diagnostic::error(
                "spec.node_key_duplicate",
                format!("{path}.key"),
                format!("节点 key 重复：{}（首次出现在 {previous}）", node.key),
            )
            .with_key(&node.key),
        );
    }

    if !node.extra.is_empty() {
        let fields = node.extra.keys().cloned().collect::<Vec<_>>().join(", ");
        diagnostics.push(
            Diagnostic::error(
                "spec.unknown_field",
                path,
                format!("节点 {} 含未知字段：{fields}", node.key),
            )
            .with_key(&node.key)
            .with_fix("删除未知字段，或先在 UiNodeKind / UiLayoutSpec 里定义它"),
        );
    }

    let type_name = node.kind.type_name();
    if !context.caps.supports_node(type_name) {
        diagnostics.push(
            Diagnostic::error(
                "spec.node_unsupported",
                path,
                format!("运行时尚未支持控件类型：{type_name}"),
            )
            .with_key(&node.key)
            .with_fix(format!("当前支持：{}", context.supported_nodes())),
        );
    }

    if node.kind.is_leaf() && !node.children.is_empty() {
        diagnostics.push(
            Diagnostic::error(
                "spec.leaf_has_children",
                path,
                format!("{type_name} 是叶子控件，不能再挂子节点"),
            )
            .with_key(&node.key),
        );
    }

    if let Some(style) = &node.style
        && !context.styles.contains_key(style)
    {
        diagnostics.push(
            Diagnostic::error(
                "spec.style_missing",
                format!("{path}.style"),
                format!("引用了不存在的样式：{style}"),
            )
            .with_key(&node.key)
            .with_fix("先在 styles 里定义该样式"),
        );
    }

    for (event, function) in &node.events {
        if !context.caps.supports_event(event) {
            diagnostics.push(
                Diagnostic::error(
                    "spec.event_unsupported",
                    format!("{path}.events.{event}"),
                    format!("运行时尚未分发事件：{event}"),
                )
                .with_key(&node.key)
                .with_fix(format!("当前支持：{}", context.supported_events())),
            );
        }
        if !is_valid_function_name(function) {
            diagnostics.push(
                Diagnostic::error(
                    "spec.event_function",
                    format!("{path}.events.{event}"),
                    format!("非法的 Lua 函数名：{function}"),
                )
                .with_key(&node.key)
                .with_fix("使用 module.function 或 identifier"),
            );
        }
    }

    validate_layout(&node.layout, path, &node.key, diagnostics);
    validate_node_kind(node, path, context, diagnostics);

    for (index, child) in node.children.iter().enumerate() {
        validate_node(
            child,
            &format!("{path}.children[{index}]"),
            context,
            keys,
            diagnostics,
        );
    }
}

fn validate_layout(
    layout: &UiLayoutSpec,
    path: &str,
    key: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let layout_path = format!("{path}.layout");
    if layout.offsets.is_some() && (layout.size.is_some() || layout.position.is_some()) {
        diagnostics.push(
            Diagnostic::error(
                "spec.layout_conflict",
                &layout_path,
                "offsets 不能与 size/position 同时出现",
            )
            .with_key(key)
            .with_fix("四边锚点用 offsets；中心/角点定位用 size + position"),
        );
    }
    for (field, values) in [
        ("size", layout.size),
        ("position", layout.position),
        ("min_size", layout.min_size),
        ("max_size", layout.max_size),
    ] {
        if let Some(values) = values
            && values.iter().any(|value| !value.is_finite())
        {
            diagnostics.push(
                Diagnostic::error(
                    "spec.layout_nan",
                    format!("{layout_path}.{field}"),
                    format!("{field} 含非有限数"),
                )
                .with_key(key),
            );
        }
    }
    if let Some(size) = layout.size
        && size.iter().any(|value| *value <= 0.0)
    {
        diagnostics.push(
            Diagnostic::error(
                "spec.layout_size",
                format!("{layout_path}.size"),
                "尺寸必须为正数",
            )
            .with_key(key),
        );
    }
    if let (Some(min), Some(max)) = (layout.min_size, layout.max_size) {
        for axis in 0..2 {
            if min[axis] < 0.0 || max[axis] < 0.0 || min[axis] > max[axis] {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.layout_min_max",
                        format!("{layout_path}.min_size"),
                        format!(
                            "min_size/max_size 第 {axis} 轴不合法：{} > {}",
                            min[axis], max[axis]
                        ),
                    )
                    .with_key(key),
                );
            }
        }
    }
    if let Some(percent) = layout.percent
        && percent
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0 || *value > 100.0)
    {
        diagnostics.push(
            Diagnostic::error(
                "spec.layout_percent",
                format!("{layout_path}.percent"),
                "percent 必须在 0..=100 之间",
            )
            .with_key(key),
        );
    }
    if layout.grow.is_some_and(|value| value < 0.0) {
        diagnostics.push(
            Diagnostic::error(
                "spec.layout_grow",
                format!("{layout_path}.grow"),
                "grow 不能为负",
            )
            .with_key(key),
        );
    }
    if layout.shrink.is_some_and(|value| value < 0.0) {
        diagnostics.push(
            Diagnostic::error(
                "spec.layout_shrink",
                format!("{layout_path}.shrink"),
                "shrink 不能为负",
            )
            .with_key(key),
        );
    }
    if let Some(pivot) = layout.pivot
        && pivot
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0 || *value > 1.0)
    {
        diagnostics.push(
            Diagnostic::warning(
                "spec.layout_pivot",
                format!("{layout_path}.pivot"),
                "pivot 通常应在 0..=1 之间",
            )
            .with_key(key),
        );
    }
}

fn validate_node_kind(
    node: &UiNode,
    path: &str,
    context: &Context<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match &node.kind {
        UiNodeKind::Panel { src } => {
            if let Some(src) = src {
                validate_image_ref(src, &format!("{path}.src"), context, &node.key, diagnostics);
            }
        }
        UiNodeKind::Image { src, nine_slice } => {
            validate_image_ref(src, &format!("{path}.src"), context, &node.key, diagnostics);
            if let Some(slice) = nine_slice
                && slice.iter().any(|value| *value < 0.0)
            {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.nine_slice_negative",
                        format!("{path}.nine_slice"),
                        "九宫格边距不能为负",
                    )
                    .with_key(&node.key),
                );
            }
        }
        UiNodeKind::Button { src, .. } => {
            if let Some(src) = src {
                validate_image_ref(src, &format!("{path}.src"), context, &node.key, diagnostics);
            }
        }
        UiNodeKind::Container {
            padding,
            spacing,
            columns,
            ..
        } => {
            if let Some(padding) = padding
                && padding.iter().any(|value| *value < 0.0)
            {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.container_padding",
                        format!("{path}.padding"),
                        "Container padding 不能为负",
                    )
                    .with_key(&node.key),
                );
            }
            if spacing.is_some_and(|value| value < 0.0) {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.container_spacing",
                        format!("{path}.spacing"),
                        "Container spacing 不能为负",
                    )
                    .with_key(&node.key),
                );
            }
            if columns == &Some(0) {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.container_columns",
                        format!("{path}.columns"),
                        "Grid 列数必须大于 0",
                    )
                    .with_key(&node.key),
                );
            }
            if node.children.is_empty() {
                diagnostics.push(
                    Diagnostic::warning("spec.container_empty", path, "Container 没有子节点")
                        .with_key(&node.key),
                );
            }
        }
        UiNodeKind::Slider { min, max, value } => {
            if !min.is_finite() || !max.is_finite() || min >= max {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.slider_range",
                        path,
                        format!("Slider 范围非法：min={min}, max={max}"),
                    )
                    .with_key(&node.key),
                );
            } else if !value.is_finite() || *value < *min || *value > *max {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.slider_value",
                        path,
                        format!("Slider 当前值 {value} 不在 [{min}, {max}]"),
                    )
                    .with_key(&node.key),
                );
            }
        }
        UiNodeKind::Progress { value } => {
            if !value.is_finite() || !(0.0..=1.0).contains(value) {
                diagnostics.push(
                    Diagnostic::warning(
                        "spec.progress_value",
                        path,
                        format!("Progress 值 {value} 不在 0..=1"),
                    )
                    .with_key(&node.key),
                );
            }
        }
        UiNodeKind::Scrollbar {
            value,
            size,
            handle,
            ..
        } => {
            for (field, raw) in [("value", value), ("size", size)] {
                if !raw.is_finite() || !(0.0..=1.0).contains(raw) {
                    diagnostics.push(
                        Diagnostic::error(
                            "spec.scrollbar_range",
                            format!("{path}.{field}"),
                            format!("Scrollbar {field} 必须在 0..=1，实际 {raw}"),
                        )
                        .with_key(&node.key)
                        .with_fix("运行时按轨道比例钳到 0..=1"),
                    );
                }
            }
            // `handle` 指的是子节点键：运行时按名字找那块滑块并改写它的矩形。
            // 引用不存在等于写了一个永不移动的滑块，比不写更难发现。
            if let Some(handle) = handle
                && !node.children.iter().any(|child| &child.key == handle)
            {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.scrollbar_handle_missing",
                        format!("{path}.handle"),
                        format!("Scrollbar 滑块子节点不存在：{handle}"),
                    )
                    .with_key(&node.key)
                    .with_fix("把 handle 指向该 Scrollbar 的一个子节点 key，或删掉 handle"),
                );
            }
        }
        UiNodeKind::Grid { columns } => {
            if *columns == 0 {
                diagnostics.push(
                    Diagnostic::error("spec.grid_columns", path, "Grid 列数必须大于 0")
                        .with_key(&node.key),
                );
            }
        }
        UiNodeKind::List { virtualized } => {
            if *virtualized && !context.caps.virtual_list {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.virtual_list_unsupported",
                        path,
                        "运行时尚未支持虚拟列表",
                    )
                    .with_key(&node.key),
                );
            }
        }
        UiNodeKind::ScrollView { .. } => {
            if !context.caps.clipping {
                diagnostics.push(
                    Diagnostic::error("spec.clipping_unsupported", path, "运行时尚未支持裁剪/滚动")
                        .with_key(&node.key),
                );
            }
        }
        UiNodeKind::Mask { padding } => {
            if !context.caps.clipping {
                diagnostics.push(
                    Diagnostic::error("spec.clipping_unsupported", path, "运行时尚未支持裁剪/滚动")
                        .with_key(&node.key),
                );
            }
            if let Some(padding) = padding
                && padding.iter().any(|value| !value.is_finite())
            {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.mask_padding",
                        format!("{path}.padding"),
                        "Mask padding 含非有限数",
                    )
                    .with_key(&node.key),
                );
            }
        }
        UiNodeKind::CanvasGroup { alpha, .. } => {
            if !alpha.is_finite() || !(0.0..=1.0).contains(alpha) {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.canvas_group_alpha",
                        path,
                        format!("CanvasGroup alpha {alpha} 不在 0..=1"),
                    )
                    .with_key(&node.key),
                );
            }
        }
        UiNodeKind::Text { rich_text, .. } => {
            if *rich_text && !context.caps.rich_text {
                diagnostics.push(
                    Diagnostic::error(
                        "spec.rich_text_unsupported",
                        path,
                        "运行时尚未解析富文本标记",
                    )
                    .with_key(&node.key)
                    .with_fix("先接进文本排版，或去掉 rich_text"),
                );
            }
        }
        UiNodeKind::Toggle { .. }
        | UiNodeKind::Input { .. }
        | UiNodeKind::Tabs { .. }
        | UiNodeKind::Modal
        | UiNodeKind::Tooltip { .. }
        | UiNodeKind::Spacer
        | UiNodeKind::SafeArea => {}
    }
}

fn validate_image_ref(
    raw: &str,
    path: &str,
    context: &Context<'_>,
    key: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let value = raw.trim();
    if value.is_empty() {
        diagnostics.push(
            Diagnostic::error("spec.image_src_empty", path, "图片引用不能为空").with_key(key),
        );
        return;
    }
    if let Some(asset_key) = value.strip_prefix("asset:") {
        if asset_key.is_empty() || !context.assets.contains(asset_key) {
            diagnostics.push(
                Diagnostic::error(
                    "spec.asset_missing",
                    path,
                    format!("引用了未声明的资产：{value}"),
                )
                .with_key(key)
                .with_fix("先在 assets 里声明该 key"),
            );
        }
        return;
    }
    if let Some((atlas, region)) = value.split_once('#') {
        if atlas.trim().is_empty() || region.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "spec.atlas_ref",
                    path,
                    format!("图集引用必须形如 atlas#region：{value}"),
                )
                .with_key(key),
            );
        }
        return;
    }
    if value.starts_with('/')
        || value.starts_with('\\')
        || value.contains("..")
        || value.contains('\\')
        || value.contains(':')
    {
        diagnostics.push(
            Diagnostic::error(
                "spec.asset_path_unsafe",
                path,
                format!("资产路径必须是工程内相对路径（/ 分隔）：{value}"),
            )
            .with_key(key),
        );
    }
}

fn validate_asset(asset: &UiAssetPlan, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    if asset.key.trim().is_empty() {
        diagnostics.push(Diagnostic::error(
            "spec.asset_key_empty",
            format!("{path}.key"),
            "资产 key 不能为空",
        ));
    }
    if asset.path.is_none() && asset.generate.is_none() {
        diagnostics.push(
            Diagnostic::error(
                "spec.asset_source_missing",
                path,
                format!("资产 {} 既没有 path 也没有 generate", asset.key),
            )
            .with_key(&asset.key),
        );
    }
    if let Some(path_value) = &asset.path
        && (path_value.trim().is_empty()
            || path_value.starts_with('/')
            || path_value.starts_with('\\')
            || path_value.contains("..")
            || path_value.contains('\\'))
    {
        diagnostics.push(
            Diagnostic::error(
                "spec.asset_path_unsafe",
                format!("{path}.path"),
                format!("资产路径不安全：{path_value}"),
            )
            .with_key(&asset.key),
        );
    }
    if let Some(generate) = &asset.generate {
        if generate.prompt.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "spec.asset_prompt_empty",
                    format!("{path}.generate.prompt"),
                    "生成 prompt 不能为空",
                )
                .with_key(&asset.key),
            );
        }
        if let Some(size) = generate.size
            && size.contains(&0)
        {
            diagnostics.push(
                Diagnostic::error(
                    "spec.asset_size",
                    format!("{path}.generate.size"),
                    "生成尺寸必须大于 0",
                )
                .with_key(&asset.key),
            );
        }
        if generate.variants == Some(0) {
            diagnostics.push(
                Diagnostic::error(
                    "spec.asset_variants",
                    format!("{path}.generate.variants"),
                    "候选数量至少为 1",
                )
                .with_key(&asset.key),
            );
        }
    }
    if let Some(slice) = asset.nine_slice
        && let Some(size) = asset.generate.as_ref().and_then(|generate| generate.size)
    {
        if slice[0] + slice[2] >= size[0] {
            diagnostics.push(
                Diagnostic::error(
                    "spec.nine_slice_width",
                    format!("{path}.nine_slice"),
                    "九宫格左右边距之和必须小于图片宽度",
                )
                .with_key(&asset.key),
            );
        }
        if slice[1] + slice[3] >= size[1] {
            diagnostics.push(
                Diagnostic::error(
                    "spec.nine_slice_height",
                    format!("{path}.nine_slice"),
                    "九宫格上下边距之和必须小于图片高度",
                )
                .with_key(&asset.key),
            );
        }
    }
    if asset.budget.max_bytes == Some(0) || asset.budget.max_atlas_pages == Some(0) {
        diagnostics.push(
            Diagnostic::error(
                "spec.asset_budget",
                format!("{path}.budget"),
                "资产预算必须大于 0",
            )
            .with_key(&asset.key),
        );
    }
}

fn validate_binding(
    binding: &UiBindingSpec,
    index: usize,
    context: &Context<'_>,
    keys: &BTreeMap<String, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let path = format!("bindings[{index}]");
    if !context.caps.bindings {
        diagnostics.push(
            Diagnostic::error(
                "spec.binding_unsupported",
                &path,
                "运行时尚未消费 UiBindings",
            )
            .with_fix("先补 BindingRuntime，或删除该绑定"),
        );
    }
    if !keys.contains_key(&binding.target) {
        diagnostics.push(
            Diagnostic::error(
                "spec.binding_target",
                format!("{path}.target"),
                format!("绑定目标节点不存在：{}", binding.target),
            )
            .with_key(&binding.target),
        );
    }
    if !BINDING_PROPERTIES.contains(&binding.property.as_str()) {
        diagnostics.push(
            Diagnostic::error(
                "spec.binding_property",
                format!("{path}.property"),
                format!("不支持的绑定属性：{}", binding.property),
            )
            .with_fix(format!("可用属性：{}", BINDING_PROPERTIES.join(", "))),
        );
    }
    if !binding.source.contains(':')
        || binding
            .source
            .split_once(':')
            .is_some_and(|(_, tail)| tail.trim().is_empty())
    {
        diagnostics.push(Diagnostic::error(
            "spec.binding_source",
            format!("{path}.source"),
            format!(
                "绑定来源必须形如 lua:<path>、save:<key> 或 entity:<id>:<component>.<field>：{}",
                binding.source
            ),
        ));
    }
    if binding.mode == BindingMode::TwoWay
        && !matches!(binding.property.as_str(), "text" | "value" | "checked")
    {
        diagnostics.push(Diagnostic::error(
            "spec.binding_two_way",
            format!("{path}.mode"),
            format!("属性 {} 不支持双向绑定", binding.property),
        ));
    }
}

pub fn validate_patch(spec: &UiSpec, patch: &UiPatch, caps: &Capabilities) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if patch.ops.is_empty() {
        diagnostics.push(Diagnostic::error("patch.empty", "ops", "补丁不能为空"));
    }

    let asset_keys = spec.asset_keys();
    let context = Context {
        caps,
        tokens: &spec.tokens,
        styles: &spec.styles,
        assets: &asset_keys,
    };
    let mut keys = BTreeMap::new();
    let mut nodes: BTreeMap<String, UiNode> = BTreeMap::new();
    for (node, path) in spec.walk_nodes() {
        keys.insert(node.key.clone(), path);
        nodes.insert(node.key.clone(), node.clone());
    }
    // 同一补丁里「先删后插」「先插容器再 move 进去」都是合法编辑：存在性与父级
    // 类型按“已执行到这一步”的状态判断，而不是一律对原始 Spec 判断。
    let mut removed: BTreeSet<String> = BTreeSet::new();

    for (index, op) in patch.ops.iter().enumerate() {
        let path = format!("ops[{index}]");
        match op {
            UiPatchOp::Replace {
                key,
                path: target,
                value,
            } => {
                if !patch_key_exists(&keys, &removed, key) {
                    diagnostics.push(
                        Diagnostic::error(
                            "patch.key_missing",
                            format!("{path}.key"),
                            format!("目标节点不存在：{key}"),
                        )
                        .with_key(key),
                    );
                }
                validate_patch_path(key, target, value, &context, &path, &mut diagnostics);
            }
            UiPatchOp::Insert {
                parent,
                index,
                node,
            } => {
                match nodes.get(parent).filter(|_| !removed.contains(parent)) {
                    None => diagnostics.push(
                        Diagnostic::error(
                            "patch.parent_missing",
                            format!("{path}.parent"),
                            format!("父节点不存在：{parent}"),
                        )
                        .with_key(parent),
                    ),
                    Some(parent_node) if parent_node.kind.is_leaf() => diagnostics.push(
                        Diagnostic::error(
                            "patch.parent_leaf",
                            format!("{path}.parent"),
                            format!("{parent} 是叶子控件，不能插入子节点"),
                        )
                        .with_key(parent),
                    ),
                    Some(_) => {}
                }
                if index.is_some_and(|index| index < -1) {
                    diagnostics.push(Diagnostic::error(
                        "patch.insert_index",
                        format!("{path}.index"),
                        "index 必须 >= -1（-1 表示追加）",
                    ));
                }
                if node.key.trim().is_empty() {
                    diagnostics.push(Diagnostic::error(
                        "patch.node_key_empty",
                        format!("{path}.node.key"),
                        "插入节点 key 不能为空",
                    ));
                } else if patch_key_exists(&keys, &removed, &node.key) {
                    diagnostics.push(
                        Diagnostic::error(
                            "patch.key_duplicate",
                            format!("{path}.node.key"),
                            format!("插入节点 key 已存在：{}", node.key),
                        )
                        .with_key(&node.key),
                    );
                }
                // 先登记节点树，后续操作的父级/子级判断才能看到它。同批删除后再插入
                // 同一 key 时，最终树里只有一份，重复检查要按“已删掉”处理。
                for key in subtree_keys(node) {
                    if removed.remove(&key) {
                        keys.remove(&key);
                    }
                }
                nodes.insert(node.key.clone(), (**node).clone());
                validate_node(
                    node,
                    &format!("{path}.node"),
                    &context,
                    &mut keys,
                    &mut diagnostics,
                );
            }
            UiPatchOp::Move { key, parent, index } => {
                if !patch_key_exists(&keys, &removed, key) {
                    diagnostics.push(
                        Diagnostic::error(
                            "patch.key_missing",
                            format!("{path}.key"),
                            format!("目标节点不存在：{key}"),
                        )
                        .with_key(key),
                    );
                }
                if *key == spec.root.key {
                    diagnostics.push(
                        Diagnostic::error(
                            "patch.move_root",
                            format!("{path}.key"),
                            "不能移动文档根节点",
                        )
                        .with_key(key),
                    );
                }
                match nodes.get(parent).filter(|_| !removed.contains(parent)) {
                    None => diagnostics.push(
                        Diagnostic::error(
                            "patch.parent_missing",
                            format!("{path}.parent"),
                            format!("父节点不存在：{parent}"),
                        )
                        .with_key(parent),
                    ),
                    Some(parent_node) => {
                        if parent_node.kind.is_leaf() {
                            diagnostics.push(
                                Diagnostic::error(
                                    "patch.parent_leaf",
                                    format!("{path}.parent"),
                                    format!("{parent} 是叶子控件，不能插入子节点"),
                                )
                                .with_key(parent),
                            );
                        }
                        // 把节点移进自己或自己的子树会成环。
                        if *parent == *key
                            || nodes
                                .get(key)
                                .is_some_and(|node| subtree_contains(node, parent))
                        {
                            diagnostics.push(
                                Diagnostic::error(
                                    "patch.move_into_self",
                                    format!("{path}.parent"),
                                    format!("不能把 {key} 移进它自己或它的子树（{parent}）"),
                                )
                                .with_key(key),
                            );
                        }
                    }
                }
                if index.is_some_and(|index| index < -1) {
                    diagnostics.push(Diagnostic::error(
                        "patch.move_index",
                        format!("{path}.index"),
                        "index 必须 >= -1（-1 表示追加）",
                    ));
                }
            }
            UiPatchOp::Remove { key } => {
                if !patch_key_exists(&keys, &removed, key) {
                    diagnostics.push(
                        Diagnostic::error(
                            "patch.key_missing",
                            format!("{path}.key"),
                            format!("目标节点不存在：{key}"),
                        )
                        .with_key(key),
                    );
                }
                if *key == spec.root.key {
                    diagnostics.push(
                        Diagnostic::error(
                            "patch.remove_root",
                            format!("{path}.key"),
                            "不能删除文档根节点",
                        )
                        .with_key(key),
                    );
                }
                // 子树整段消失，给后续操作（例如同批 Move 的父级判断）看到。
                if let Some(node) = nodes.get(key) {
                    for key in subtree_keys(node) {
                        removed.insert(key);
                    }
                }
            }
            UiPatchOp::SetEvent {
                key,
                event,
                function,
            } => {
                if !patch_key_exists(&keys, &removed, key) {
                    diagnostics.push(
                        Diagnostic::error(
                            "patch.key_missing",
                            format!("{path}.key"),
                            format!("目标节点不存在：{key}"),
                        )
                        .with_key(key),
                    );
                }
                if !context.caps.supports_event(event) {
                    diagnostics.push(
                        Diagnostic::error(
                            "spec.event_unsupported",
                            format!("{path}.event"),
                            format!("运行时尚未分发事件：{event}"),
                        )
                        .with_key(key),
                    );
                }
                if !is_valid_function_name(function) {
                    diagnostics.push(
                        Diagnostic::error(
                            "spec.event_function",
                            format!("{path}.function"),
                            format!("非法的 Lua 函数名：{function}"),
                        )
                        .with_key(key),
                    );
                }
            }
            UiPatchOp::SetStyle { key, style } => {
                if !patch_key_exists(&keys, &removed, key) {
                    diagnostics.push(
                        Diagnostic::error(
                            "patch.key_missing",
                            format!("{path}.key"),
                            format!("目标节点不存在：{key}"),
                        )
                        .with_key(key),
                    );
                }
                if let Some(style) = style
                    && !context.styles.contains_key(style)
                {
                    diagnostics.push(
                        Diagnostic::error(
                            "spec.style_missing",
                            format!("{path}.style"),
                            format!("引用了不存在的样式：{style}"),
                        )
                        .with_key(key),
                    );
                }
            }
            UiPatchOp::SetToken { name, value } => {
                if name.trim().is_empty() {
                    diagnostics.push(Diagnostic::error(
                        "spec.token_name",
                        format!("{path}.name"),
                        "token 名不能为空",
                    ));
                }
                validate_token_value(value, &format!("{path}.value"), &mut diagnostics);
            }
            UiPatchOp::RemoveToken { name } => {
                if !context.tokens.contains_key(name) {
                    diagnostics.push(Diagnostic::error(
                        "patch.token_missing",
                        format!("{path}.name"),
                        format!("令牌不存在：{name}"),
                    ));
                }
            }
        }
    }

    diagnostics
}

fn patch_key_exists(
    keys: &BTreeMap<String, String>,
    removed: &BTreeSet<String>,
    key: &str,
) -> bool {
    keys.contains_key(key) && !removed.contains(key)
}

fn subtree_keys(node: &UiNode) -> Vec<String> {
    let mut out = vec![node.key.clone()];
    for child in &node.children {
        out.extend(subtree_keys(child));
    }
    out
}

fn subtree_contains(node: &UiNode, key: &str) -> bool {
    node.key == key
        || node
            .children
            .iter()
            .any(|child| subtree_contains(child, key))
}

fn validate_patch_path(
    key: &str,
    target: &str,
    value: &Value,
    context: &Context<'_>,
    path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !target.starts_with('/') || target.len() == 1 {
        diagnostics.push(
            Diagnostic::error(
                "patch.path",
                format!("{path}.path"),
                format!("路径必须是 JSON Pointer 风格：{target}"),
            )
            .with_key(key),
        );
        return;
    }
    let segments = target
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let root = segments[0];
    if !PATCH_ROOT_FIELDS.contains(&root) {
        diagnostics.push(
            Diagnostic::error(
                "patch.path_forbidden",
                format!("{path}.path"),
                format!("不允许通过补丁修改字段：{root}"),
            )
            .with_key(key)
            .with_fix(format!("可用根字段：{}", PATCH_ROOT_FIELDS.join(", "))),
        );
        return;
    }
    if root == "layout"
        && let Some(field) = segments.get(1)
        && !LAYOUT_FIELDS.contains(field)
    {
        diagnostics.push(
            Diagnostic::error(
                "patch.layout_field",
                format!("{path}.path"),
                format!("未知布局字段：{field}"),
            )
            .with_key(key),
        );
    }
    if root == "style"
        && let Some(style) = value.as_str()
        && !context.styles.contains_key(style)
    {
        diagnostics.push(
            Diagnostic::error(
                "spec.style_missing",
                format!("{path}.value"),
                format!("引用了不存在的样式：{style}"),
            )
            .with_key(key),
        );
    }
    if root == "events"
        && let Some(event) = segments.get(1)
    {
        if !context.caps.supports_event(event) {
            diagnostics.push(
                Diagnostic::error(
                    "spec.event_unsupported",
                    format!("{path}.path"),
                    format!("运行时尚未分发事件：{event}"),
                )
                .with_key(key),
            );
        }
        if let Some(function) = value.as_str()
            && !is_valid_function_name(function)
        {
            diagnostics.push(
                Diagnostic::error(
                    "spec.event_function",
                    format!("{path}.value"),
                    format!("非法的 Lua 函数名：{function}"),
                )
                .with_key(key),
            );
        }
    }
}

fn is_valid_function_name(name: &str) -> bool {
    !name.is_empty()
        && name.split(['.', ':']).all(|part| {
            let mut chars = part.chars();
            matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
                && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
}

fn state_name(state: StyleState) -> &'static str {
    match state {
        StyleState::Hover => "hover",
        StyleState::Pressed => "pressed",
        StyleState::Disabled => "disabled",
        StyleState::Focused => "focused",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{CanvasSpec, DocumentKind, StyleEasing, StyleTransition, UiDocumentMeta};

    fn minimal_spec() -> UiSpec {
        UiSpec {
            spec_version: SPEC_VERSION,
            document: UiDocumentMeta {
                kind: DocumentKind::Widget,
                name: "Shop".to_owned(),
                canvas: CanvasSpec::default(),
            },
            tokens: [(
                "color.surface".to_owned(),
                Value::String("#1E1E24".to_owned()),
            )]
            .into_iter()
            .collect(),
            styles: [(
                "card".to_owned(),
                UiStyle {
                    base: [("bg".to_owned(), Value::String("@color.surface".to_owned()))]
                        .into_iter()
                        .collect(),
                    states: BTreeMap::new(),
                    transition: None,
                },
            )]
            .into_iter()
            .collect(),
            root: UiNode {
                key: "root".to_owned(),
                kind: UiNodeKind::Panel { src: None },
                layout: UiLayoutSpec::default(),
                style: Some("card".to_owned()),
                events: BTreeMap::new(),
                children: vec![UiNode {
                    key: "title".to_owned(),
                    kind: UiNodeKind::Text {
                        text: "Shop".to_owned(),
                        rich_text: false,
                    },
                    layout: UiLayoutSpec::default(),
                    style: None,
                    events: BTreeMap::new(),
                    children: Vec::new(),
                    extra: BTreeMap::new(),
                }],
                extra: BTreeMap::new(),
            },
            assets: Vec::new(),
            bindings: Vec::new(),
        }
    }

    fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect()
    }

    #[test]
    fn core_spec_is_clean() {
        let diagnostics = validate_spec(&minimal_spec(), &Capabilities::core());
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }

    #[test]
    fn duplicate_node_keys_are_rejected() {
        let mut spec = minimal_spec();
        spec.root.children[0].key = "root".to_owned();
        let diagnostics = validate_spec(&spec, &Capabilities::core());
        assert!(codes(&diagnostics).contains(&"spec.node_key_duplicate"));
    }

    #[test]
    fn unsupported_node_is_rejected_until_runtime_catches_up() {
        let mut spec = minimal_spec();
        spec.root.children.push(UiNode {
            key: "tabs".to_owned(),
            kind: UiNodeKind::Tabs { selected: 0 },
            layout: UiLayoutSpec::default(),
            style: None,
            events: BTreeMap::new(),
            children: Vec::new(),
            extra: BTreeMap::new(),
        });
        let core = validate_spec(&spec, &Capabilities::core());
        assert!(codes(&core).contains(&"spec.node_unsupported"));
        let planned = validate_spec(&spec, &Capabilities::planned());
        assert!(
            !codes(&planned).contains(&"spec.node_unsupported"),
            "{planned:#?}"
        );
    }

    #[test]
    fn list_is_core_but_virtualizing_is_still_gated() {
        let mut spec = minimal_spec();
        spec.root.children.push(UiNode {
            key: "rows".to_owned(),
            kind: UiNodeKind::List { virtualized: false },
            layout: UiLayoutSpec::default(),
            style: None,
            events: BTreeMap::new(),
            children: Vec::new(),
            extra: BTreeMap::new(),
        });
        let core = validate_spec(&spec, &Capabilities::core());
        assert!(
            !codes(&core).contains(&"spec.node_unsupported"),
            "list 的普通形态已能导出成滚动容器：{core:#?}"
        );
        spec.root.children.last_mut().unwrap().kind = UiNodeKind::List { virtualized: true };
        let core = validate_spec(&spec, &Capabilities::core());
        assert!(codes(&core).contains(&"spec.virtual_list_unsupported"));
        let planned = validate_spec(&spec, &Capabilities::planned());
        assert!(!codes(&planned).contains(&"spec.virtual_list_unsupported"));
    }

    #[test]
    fn missing_style_and_token_are_rejected() {
        let mut spec = minimal_spec();
        spec.root.style = Some("missing".to_owned());
        spec.tokens.clear();
        let diagnostics = validate_spec(&spec, &Capabilities::core());
        let codes = codes(&diagnostics);
        assert!(codes.contains(&"spec.style_missing"));
        assert!(codes.contains(&"spec.token_missing"));
    }

    #[test]
    fn unknown_style_property_is_rejected() {
        let mut spec = minimal_spec();
        spec.styles
            .get_mut("card")
            .unwrap()
            .base
            .insert("glow".to_owned(), Value::String("10".to_owned()));
        let diagnostics = validate_spec(&spec, &Capabilities::core());
        assert!(codes(&diagnostics).contains(&"spec.style_property_unknown"));
    }

    #[test]
    fn unsupported_event_is_rejected() {
        let mut spec = minimal_spec();
        spec.root
            .events
            .insert("double_click".to_owned(), "shop.double".to_owned());
        // `value_changed`/`drag` 自 M80-07 起是 core 能力，这里换一个两侧都没有的事件名。
        for caps in [Capabilities::core(), Capabilities::planned()] {
            let diagnostics = validate_spec(&spec, &caps);
            assert!(codes(&diagnostics).contains(&"spec.event_unsupported"));
        }
    }

    #[test]
    fn control_events_and_rich_text_are_core_authored() {
        let mut spec = minimal_spec();
        spec.root
            .events
            .insert("value_changed".to_owned(), "shop.changed".to_owned());
        spec.root
            .events
            .insert("drag".to_owned(), "shop.drag".to_owned());
        spec.root.children[0].kind = UiNodeKind::Text {
            text: "<b>Shop</b>".to_owned(),
            rich_text: true,
        };
        let diagnostics = validate_spec(&spec, &Capabilities::core());
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }

    #[test]
    fn scrollbar_handle_must_name_a_child() {
        let mut spec = minimal_spec();
        spec.root.children.push(UiNode {
            key: "bar".to_owned(),
            kind: UiNodeKind::Scrollbar {
                value: 0.25,
                size: 0.3,
                direction: crate::spec::ScrollbarDirection::TopToBottom,
                handle: Some("missing".to_owned()),
            },
            layout: UiLayoutSpec::default(),
            style: None,
            events: BTreeMap::new(),
            children: Vec::new(),
            extra: BTreeMap::new(),
        });
        let diagnostics = validate_spec(&spec, &Capabilities::core());
        assert!(codes(&diagnostics).contains(&"spec.scrollbar_handle_missing"));
        let codes = codes(&diagnostics);
        assert!(!codes.contains(&"spec.node_unsupported"));
    }

    #[test]
    fn transition_is_gated_by_capabilities() {
        let mut spec = minimal_spec();
        spec.styles.get_mut("card").unwrap().transition = Some(StyleTransition {
            duration: 0.15,
            easing: StyleEasing::SmoothStep,
        });
        // M80-07 起底色过渡是 core 能力：`ButtonStates.transition` 由运行时驱动消费。
        let core = validate_spec(&spec, &Capabilities::core());
        assert!(
            !codes(&core).contains(&"spec.transition_unsupported"),
            "{core:?}"
        );
        let mut no_transitions = Capabilities::core();
        no_transitions.transitions = false;
        let gated = validate_spec(&spec, &no_transitions);
        assert!(codes(&gated).contains(&"spec.transition_unsupported"));
        let mut with_bad_duration = spec.clone();
        with_bad_duration.styles.get_mut("card").unwrap().transition = Some(StyleTransition {
            duration: -1.0,
            easing: StyleEasing::Linear,
        });
        let diagnostics = validate_spec(&with_bad_duration, &Capabilities::core());
        assert!(codes(&diagnostics).contains(&"spec.transition_duration"));
    }

    #[test]
    fn state_override_is_gated_by_capabilities() {
        let mut spec = minimal_spec();
        spec.styles.get_mut("card").unwrap().states.insert(
            StyleState::Hover,
            [("opacity".to_owned(), serde_json::json!(0.8))]
                .into_iter()
                .collect(),
        );
        let mut no_states = Capabilities::core();
        no_states.style_states.clear();
        let gated = validate_spec(&spec, &no_states);
        assert!(codes(&gated).contains(&"spec.state_unsupported"));
        let core = validate_spec(&spec, &Capabilities::core());
        assert!(!codes(&core).contains(&"spec.state_unsupported"));
        let planned = validate_spec(&spec, &Capabilities::planned());
        assert!(!codes(&planned).contains(&"spec.state_unsupported"));
    }

    #[test]
    fn unknown_node_fields_are_reported() {
        let mut spec = minimal_spec();
        spec.root
            .extra
            .insert("mystery".to_owned(), Value::Bool(true));
        let diagnostics = validate_spec(&spec, &Capabilities::core());
        assert!(codes(&diagnostics).contains(&"spec.unknown_field"));
    }

    #[test]
    fn asset_refs_are_checked() {
        let mut spec = minimal_spec();
        spec.root.kind = UiNodeKind::Image {
            src: "asset:shop.bg".to_owned(),
            nine_slice: None,
        };
        let diagnostics = validate_spec(&spec, &Capabilities::core());
        assert!(codes(&diagnostics).contains(&"spec.asset_missing"));

        spec.assets.push(UiAssetPlan {
            key: "shop.bg".to_owned(),
            role: crate::asset::AssetRole::Background,
            path: Some("ui/shop_bg.png".to_owned()),
            generate: None,
            nine_slice: None,
            budget: Default::default(),
        });
        let diagnostics = validate_spec(&spec, &Capabilities::core());
        assert!(!codes(&diagnostics).contains(&"spec.asset_missing"));
    }

    #[test]
    fn bad_layout_is_rejected() {
        let mut spec = minimal_spec();
        spec.root.layout.offsets = Some([0.0, 0.0, 10.0, 10.0]);
        spec.root.layout.size = Some([10.0, 10.0]);
        spec.root.layout.percent = Some([120.0, 50.0]);
        let diagnostics = validate_spec(&spec, &Capabilities::core());
        let codes = codes(&diagnostics);
        assert!(codes.contains(&"spec.layout_conflict"));
        assert!(codes.contains(&"spec.layout_percent"));
    }

    #[test]
    fn binding_is_gated_by_capabilities() {
        let mut spec = minimal_spec();
        spec.bindings.push(UiBindingSpec {
            target: "title".to_owned(),
            property: "text".to_owned(),
            source: "lua:shop.title".to_owned(),
            mode: BindingMode::OneWay,
            default: None,
        });
        let mut no_bindings = Capabilities::core();
        no_bindings.bindings = false;
        let gated = validate_spec(&spec, &no_bindings);
        assert!(codes(&gated).contains(&"spec.binding_unsupported"));
        let core = validate_spec(&spec, &Capabilities::core());
        assert!(!codes(&core).contains(&"spec.binding_unsupported"));
        let planned = validate_spec(&spec, &Capabilities::planned());
        assert!(!codes(&planned).contains(&"spec.binding_unsupported"));
    }

    #[test]
    fn patch_validates_keys_and_paths() {
        let spec = minimal_spec();
        let patch = UiPatch {
            base_revision: 1,
            ops: vec![
                UiPatchOp::Replace {
                    key: "title".to_owned(),
                    path: "/style".to_owned(),
                    value: Value::String("card".to_owned()),
                },
                UiPatchOp::Insert {
                    parent: "root".to_owned(),
                    index: Some(-1),
                    node: Box::new(UiNode {
                        key: "buy".to_owned(),
                        kind: UiNodeKind::Button {
                            text: Some("Buy".to_owned()),
                            src: None,
                        },
                        layout: UiLayoutSpec::default(),
                        style: None,
                        events: BTreeMap::new(),
                        children: Vec::new(),
                        extra: BTreeMap::new(),
                    }),
                },
                UiPatchOp::SetEvent {
                    key: "buy".to_owned(),
                    event: "click".to_owned(),
                    function: "shop.buy".to_owned(),
                },
            ],
        };
        assert!(validate_patch(&spec, &patch, &Capabilities::core()).is_empty());
    }

    #[test]
    fn patch_rejects_unknown_and_duplicate_keys() {
        let spec = minimal_spec();
        let patch = UiPatch {
            base_revision: 1,
            ops: vec![
                UiPatchOp::Replace {
                    key: "nope".to_owned(),
                    path: "/style".to_owned(),
                    value: Value::String("card".to_owned()),
                },
                UiPatchOp::Insert {
                    parent: "root".to_owned(),
                    index: None,
                    node: Box::new(UiNode {
                        key: "title".to_owned(),
                        kind: UiNodeKind::Text {
                            text: "duplicate".to_owned(),
                            rich_text: false,
                        },
                        layout: UiLayoutSpec::default(),
                        style: None,
                        events: BTreeMap::new(),
                        children: Vec::new(),
                        extra: BTreeMap::new(),
                    }),
                },
            ],
        };
        let diagnostics = validate_patch(&spec, &patch, &Capabilities::core());
        let codes = codes(&diagnostics);
        assert!(codes.contains(&"patch.key_missing"));
        assert!(codes.contains(&"patch.key_duplicate"));
    }

    #[test]
    fn patch_rejects_key_and_type_rewrites() {
        let spec = minimal_spec();
        let patch = UiPatch {
            base_revision: 1,
            ops: vec![UiPatchOp::Replace {
                key: "title".to_owned(),
                path: "/key".to_owned(),
                value: Value::String("renamed".to_owned()),
            }],
        };
        let diagnostics = validate_patch(&spec, &patch, &Capabilities::core());
        assert!(codes(&diagnostics).contains(&"patch.path_forbidden"));
    }
}
