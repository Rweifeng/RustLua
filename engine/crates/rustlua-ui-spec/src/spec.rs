use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const SPEC_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    #[default]
    Widget,
    Prefab,
    Scene,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CanvasScaleMode {
    None,
    FitWidth,
    FitHeight,
    #[default]
    FitBoth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanvasSpec {
    #[serde(default = "default_reference")]
    pub reference: [f32; 2],
    #[serde(default)]
    pub scale_mode: CanvasScaleMode,
    #[serde(default)]
    pub safe_area: bool,
}

impl Default for CanvasSpec {
    fn default() -> Self {
        Self {
            reference: default_reference(),
            scale_mode: CanvasScaleMode::default(),
            safe_area: false,
        }
    }
}

fn default_reference() -> [f32; 2] {
    [1280.0, 720.0]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiDocumentMeta {
    #[serde(default)]
    pub kind: DocumentKind,
    pub name: String,
    #[serde(default)]
    pub canvas: CanvasSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSpec {
    pub spec_version: u32,
    pub document: UiDocumentMeta,
    #[serde(default)]
    pub tokens: BTreeMap<String, Value>,
    #[serde(default)]
    pub styles: BTreeMap<String, UiStyle>,
    pub root: UiNode,
    #[serde(default)]
    pub assets: Vec<crate::asset::UiAssetPlan>,
    #[serde(default)]
    pub bindings: Vec<UiBindingSpec>,
}

impl UiSpec {
    /// 深度优先遍历，路径形如 `root.children[1]`，供诊断与编译器定位。
    pub fn walk_nodes(&self) -> Vec<(&UiNode, String)> {
        fn visit<'a>(node: &'a UiNode, path: String, out: &mut Vec<(&'a UiNode, String)>) {
            out.push((node, path.clone()));
            for (index, child) in node.children.iter().enumerate() {
                visit(child, format!("{path}.children[{index}]"), out);
            }
        }
        let mut out = Vec::new();
        visit(&self.root, "root".to_owned(), &mut out);
        out
    }

    pub fn find_node(&self, key: &str) -> Option<&UiNode> {
        self.walk_nodes()
            .into_iter()
            .find_map(|(node, _)| (node.key == key).then_some(node))
    }

    pub fn asset_keys(&self) -> BTreeSet<&str> {
        self.assets.iter().map(|asset| asset.key.as_str()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UiStyle {
    #[serde(default)]
    pub base: BTreeMap<String, Value>,
    #[serde(default)]
    pub states: BTreeMap<StyleState, BTreeMap<String, Value>>,
    /// 状态之间的过渡（时长 + 曲线）。缺省 = 瞬时切换（Unity `Selectable` 不勾
    /// Interaction 时的行为）。导出成 `ButtonStates` 的 `transition=秒:曲线`，
    /// 运行时用动画内核的 `UiEasing` 采样，不另造曲线。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<StyleTransition>,
}

/// 状态过渡曲线，与运行时 `rustlua_ui::ui_animation::UiEasing` 同一组词。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StyleEasing {
    #[default]
    Linear,
    SmoothStep,
    Step,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct StyleTransition {
    /// 秒。0 = 瞬时。
    #[serde(default)]
    pub duration: f32,
    #[serde(default)]
    pub easing: StyleEasing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleState {
    Hover,
    Pressed,
    Disabled,
    Focused,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiNode {
    pub key: String,
    #[serde(flatten)]
    pub kind: UiNodeKind,
    #[serde(default, skip_serializing_if = "is_default_layout")]
    pub layout: UiLayoutSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub events: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<UiNode>,
    /// 未知字段不静默丢弃：校验器会把任何残留报成 error，逼 AI 修到契约内。
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn is_default_layout(layout: &UiLayoutSpec) -> bool {
    *layout == UiLayoutSpec::default()
}

/// 手写反序列化：内部标签枚举与 `extra` 都用 flatten，若直接 derive，`type` 会被
/// `extra` 再收一遍，序列化时出现两个 `type`，回读即 "duplicate field `type`"。
/// 这里显式剥掉已知字段，只把真正未知的键留给校验器。
impl<'de> Deserialize<'de> for UiNode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        let value = Value::deserialize(deserializer)?;
        let mut object = match value {
            Value::Object(object) => object,
            _ => return Err(D::Error::custom("ui node must be a json object")),
        };

        let key = object
            .remove("key")
            .ok_or_else(|| D::Error::missing_field("key"))?;
        let key: String = serde_json::from_value(key).map_err(D::Error::custom)?;

        if !object.contains_key("type") {
            return Err(D::Error::missing_field("type"));
        }
        let kind_value = Value::Object(object.clone());
        let kind: UiNodeKind = serde_json::from_value(kind_value).map_err(D::Error::custom)?;
        object.remove("type");
        // Kind fields are consumed by `UiNodeKind`; leaving them in `extra`
        // would make every `text`/`src` node fail validation as "unknown".
        for field in kind_field_names(&kind) {
            object.remove(*field);
        }

        let layout = object
            .remove("layout")
            .map(|value| serde_json::from_value(value).map_err(D::Error::custom))
            .transpose()?
            .unwrap_or_default();
        let style = object
            .remove("style")
            .map(|value| serde_json::from_value(value).map_err(D::Error::custom))
            .transpose()?;
        let events = object
            .remove("events")
            .map(|value| serde_json::from_value(value).map_err(D::Error::custom))
            .transpose()?
            .unwrap_or_default();
        let children = object
            .remove("children")
            .map(|value| serde_json::from_value(value).map_err(D::Error::custom))
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            key,
            kind,
            layout,
            style,
            events,
            children,
            extra: object.into_iter().collect(),
        })
    }
}

fn kind_field_names(kind: &UiNodeKind) -> &'static [&'static str] {
    match kind {
        UiNodeKind::Panel { .. } => &["src"],
        UiNodeKind::Container { .. } => &[
            "direction",
            "padding",
            "spacing",
            "justify",
            "align",
            "columns",
        ],
        UiNodeKind::Text { .. } => &["text", "rich_text"],
        UiNodeKind::Image { .. } => &["src", "nine_slice"],
        UiNodeKind::Button { .. } => &["text", "src"],
        UiNodeKind::Toggle { .. } => &["text", "checked"],
        UiNodeKind::Slider { .. } => &["min", "max", "value"],
        UiNodeKind::Progress { .. } => &["value"],
        UiNodeKind::Scrollbar { .. } => &["value", "size", "direction", "handle"],
        UiNodeKind::Input { .. } => &["placeholder"],
        UiNodeKind::ScrollView { .. } => &["horizontal"],
        UiNodeKind::Mask { .. } => &["padding"],
        UiNodeKind::CanvasGroup { .. } => &["alpha", "interactable", "blocks_raycasts"],
        UiNodeKind::List { .. } => &["virtualized"],
        UiNodeKind::Grid { .. } => &["columns"],
        UiNodeKind::Tabs { .. } => &["selected"],
        UiNodeKind::Modal | UiNodeKind::Spacer | UiNodeKind::SafeArea => &[],
        UiNodeKind::Tooltip { .. } => &["text"],
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiNodeKind {
    Panel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src: Option<String>,
    },
    Container {
        #[serde(default)]
        direction: ContainerDirection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        padding: Option<[f32; 4]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spacing: Option<f32>,
        #[serde(default)]
        justify: Justify,
        #[serde(default)]
        align: Align,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        columns: Option<u32>,
    },
    Text {
        text: String,
        /// `<b>/<color>/<br/>/<icon=…>` 内联标记。运行时按同一套解析出 span，
        /// 未识别的标签原样保留并计入诊断。
        #[serde(default)]
        rich_text: bool,
    },
    Image {
        src: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nine_slice: Option<[f32; 4]>,
    },
    Button {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src: Option<String>,
    },
    Toggle {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default)]
        checked: bool,
    },
    Slider {
        #[serde(default)]
        min: f32,
        #[serde(default = "default_one")]
        max: f32,
        #[serde(default)]
        value: f32,
    },
    Progress {
        #[serde(default)]
        value: f32,
    },
    /// 滚动条（对齐 Unity `Scrollbar`）：`size` 是滑块占轨道的比例，`handle` 指向
    /// 一个子节点键，运行时把它的矩形改写成「长度 = 轨道 × size、位置 = value × 剩余行程」。
    Scrollbar {
        #[serde(default)]
        value: f32,
        #[serde(default = "default_scrollbar_size")]
        size: f32,
        #[serde(default)]
        direction: ScrollbarDirection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handle: Option<String>,
    },
    Input {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
    },
    ScrollView {
        #[serde(default)]
        horizontal: bool,
    },
    /// 通用遮罩：按自身矩形（扣掉 `padding`）裁剪子孙，与 `ScrollView` 同一套裁剪路径。
    Mask {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        padding: Option<[f32; 4]>,
    },
    /// 整组透明度与交互：`alpha` 沿父链相乘，`interactable`/`blocks_raycasts` 直通运行时。
    CanvasGroup {
        #[serde(default = "default_one")]
        alpha: f32,
        #[serde(default = "default_true")]
        interactable: bool,
        #[serde(default = "default_true")]
        blocks_raycasts: bool,
    },
    List {
        #[serde(default)]
        virtualized: bool,
    },
    Grid {
        #[serde(default = "default_grid_columns")]
        columns: u32,
    },
    Tabs {
        #[serde(default)]
        selected: u32,
    },
    Modal,
    Tooltip {
        text: String,
    },
    Spacer,
    SafeArea,
}

impl UiNodeKind {
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Panel { .. } => "panel",
            Self::Container { .. } => "container",
            Self::Text { .. } => "text",
            Self::Image { .. } => "image",
            Self::Button { .. } => "button",
            Self::Toggle { .. } => "toggle",
            Self::Slider { .. } => "slider",
            Self::Progress { .. } => "progress",
            Self::Scrollbar { .. } => "scrollbar",
            Self::Input { .. } => "input",
            Self::ScrollView { .. } => "scroll_view",
            Self::Mask { .. } => "mask",
            Self::CanvasGroup { .. } => "canvas_group",
            Self::List { .. } => "list",
            Self::Grid { .. } => "grid",
            Self::Tabs { .. } => "tabs",
            Self::Modal => "modal",
            Self::Tooltip { .. } => "tooltip",
            Self::Spacer => "spacer",
            Self::SafeArea => "safe_area",
        }
    }

    pub const fn is_leaf(&self) -> bool {
        matches!(
            self,
            Self::Text { .. }
                | Self::Image { .. }
                | Self::Spacer
                | Self::Tooltip { .. }
                | Self::Progress { .. }
        )
    }
}

fn default_one() -> f32 {
    1.0
}

fn default_scrollbar_size() -> f32 {
    0.2
}

fn default_true() -> bool {
    true
}

fn default_grid_columns() -> u32 {
    2
}

/// 滚动条/滑块的行程方向，与运行时 `rustlua_ui::ui_control_direction::ControlDirection`
/// 同一组取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScrollbarDirection {
    #[default]
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
}

impl ScrollbarDirection {
    /// 与运行时 `ControlDirection::as_str` 同一组拼写，导出直接透传。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LeftToRight => "left_to_right",
            Self::RightToLeft => "right_to_left",
            Self::TopToBottom => "top_to_bottom",
            Self::BottomToTop => "bottom_to_top",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContainerDirection {
    #[default]
    VBox,
    HBox,
    Grid,
    Wrap,
    Margin,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiLayoutSpec {
    #[serde(default)]
    pub anchor: AnchorPreset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offsets: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_size: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<[f32; 2]>,
    /// 相对容器的百分比（0..=100，0 表示未设置）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grow: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shrink: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align_self: Option<Align>,
}

impl Default for UiLayoutSpec {
    fn default() -> Self {
        Self {
            anchor: AnchorPreset::Center,
            offsets: None,
            size: None,
            position: None,
            pivot: None,
            min_size: None,
            max_size: None,
            percent: None,
            grow: None,
            shrink: None,
            align_self: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnchorPreset {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    #[default]
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    StretchHorizontal,
    StretchVertical,
    FullRect,
}

impl AnchorPreset {
    /// 九宫格 3×3 加横/纵/全拉伸，面板按这个顺序铺按钮。
    pub const ALL: [Self; 12] = [
        Self::TopLeft,
        Self::TopCenter,
        Self::TopRight,
        Self::CenterLeft,
        Self::Center,
        Self::CenterRight,
        Self::BottomLeft,
        Self::BottomCenter,
        Self::BottomRight,
        Self::StretchHorizontal,
        Self::StretchVertical,
        Self::FullRect,
    ];

    /// 与 JSON 里的取值同一套拼写（`snake_case`），检查器输入框也认这个。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TopLeft => "top_left",
            Self::TopCenter => "top_center",
            Self::TopRight => "top_right",
            Self::CenterLeft => "center_left",
            Self::Center => "center",
            Self::CenterRight => "center_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomCenter => "bottom_center",
            Self::BottomRight => "bottom_right",
            Self::StretchHorizontal => "stretch_horizontal",
            Self::StretchVertical => "stretch_vertical",
            Self::FullRect => "full_rect",
        }
    }

    /// 锚点在父矩形上的比例（拉伸预设取中点，与预览器 `anchor_fraction` 同口径）。
    #[must_use]
    pub const fn fraction(self) -> [f32; 2] {
        match self {
            Self::TopLeft => [0.0, 0.0],
            Self::TopCenter => [0.5, 0.0],
            Self::TopRight => [1.0, 0.0],
            Self::CenterLeft => [0.0, 0.5],
            Self::Center => [0.5, 0.5],
            Self::CenterRight => [1.0, 0.5],
            Self::BottomLeft => [0.0, 1.0],
            Self::BottomCenter => [0.5, 1.0],
            Self::BottomRight => [1.0, 1.0],
            Self::StretchHorizontal | Self::StretchVertical | Self::FullRect => [0.5, 0.5],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiBindingSpec {
    /// 目标节点 key。
    pub target: String,
    pub property: String,
    /// `lua:<path>` / `save:<key>` / `entity:<id>:<component>.<field>`。
    pub source: String,
    #[serde(default)]
    pub mode: BindingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BindingMode {
    #[default]
    OneWay,
    TwoWay,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_spec_round_trips() {
        let raw = r##"{
            "spec_version": 1,
            "document": {"kind": "widget", "name": "Shop"},
            "tokens": {"color.surface": "#1E1E24"},
            "styles": {"card": {"base": {"bg": "@color.surface"}}},
            "root": {
                "key": "root", "type": "panel", "style": "card",
                "children": [
                    {"key": "title", "type": "text", "text": "Shop",
                     "layout": {"anchor": "top_center", "size": [200, 40]}}
                ]
            }
        }"##;
        let spec: UiSpec = serde_json::from_str(raw).unwrap();
        assert_eq!(spec.spec_version, SPEC_VERSION);
        assert_eq!(spec.root.children[0].key, "title");
        assert_eq!(spec.root.children[0].kind.type_name(), "text");
        let encoded = serde_json::to_string(&spec).unwrap();
        let decoded: UiSpec = serde_json::from_str(&encoded).unwrap();
        assert_eq!(spec, decoded);
    }

    #[test]
    fn node_walk_reports_paths() {
        let spec: UiSpec = serde_json::from_str(
            r#"{
                "spec_version": 1,
                "document": {"name": "T"},
                "root": {"key": "r", "type": "panel", "children": [
                    {"key": "a", "type": "panel", "children": [
                        {"key": "b", "type": "text", "text": "x"}
                    ]}
                ]}
            }"#,
        )
        .unwrap();
        let paths: Vec<String> = spec
            .walk_nodes()
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        assert_eq!(
            paths,
            vec!["root", "root.children[0]", "root.children[0].children[0]"]
        );
        assert!(spec.find_node("b").is_some());
        assert!(spec.find_node("missing").is_none());
    }
}
