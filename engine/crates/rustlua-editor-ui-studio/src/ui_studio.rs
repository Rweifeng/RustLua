//! M76-04：UI Studio 的无头状态层。
//!
//! 画布/层级/检查器/预览/提案箱的**数据与动作**集中在这里，egui 面板只做绘制与转调。
//! 所有写入仍走 [`UiSpecStore`]：提案 → 接受/拒绝/拆分接受，UI Studio 不维护第二份
//! 文档状态。多分辨率预览与安全区只影响绘制参数，不写回 Spec。

use crate::ui_spec_host::{UiHostError, UiHostErrorCode, UiHostResult, UiSpecStore};
use rustlua_ui_spec::{
    Align, AnchorPreset, DocumentKind, ResizeHandle, ScrollbarDirection, StyleState,
    UiDocumentMeta, UiLayoutSpec, UiNode, UiNodeKind, UiPatch, UiPatchOp, UiProposal, UiSpec,
    offsets_for_rect, render_preview, resize_rect, validate_spec,
};
use std::collections::{BTreeMap, BTreeSet};

/// UI Studio 自己的撤销栈上限：只覆盖「接受提案」，与场景 History 互不共享。
const UNDO_LIMIT: usize = 16;

/// 画布拖拽的最小边长（设计像素）：夹住反向拖过头，避免 0 尺寸节点。
const MIN_CANVAS_SIZE: f32 = 8.0;

fn invalid_argument(message: impl Into<String>) -> UiHostError {
    UiHostError::new(UiHostErrorCode::InvalidArgument, message)
}

/// 样式状态的中文显示名（检查器只展示这个，不上屏 `hover/pressed/...`）。
fn style_state_label(state: StyleState) -> &'static str {
    match state {
        StyleState::Hover => "悬停",
        StyleState::Pressed => "按下",
        StyleState::Disabled => "禁用",
        StyleState::Focused => "聚焦",
    }
}

/// 找到 `key` 的父级 key 与它在父级子表里的下标；根节点返回 `None`。
fn parent_slot(root: &UiNode, key: &str) -> Option<(String, usize)> {
    fn visit(node: &UiNode, key: &str, parent: &str, out: &mut Option<(String, usize)>) -> bool {
        for (index, child) in node.children.iter().enumerate() {
            if child.key == key {
                *out = Some((parent.to_owned(), index));
                return true;
            }
            if visit(child, key, &child.key, out) {
                return true;
            }
        }
        false
    }
    let mut out = None;
    visit(root, key, &root.key, &mut out);
    out
}

/// 复制子树：每个 key 找一个不冲突的 `_copy` 名，避免整份重校验报重复。
fn rename_copy(node: &mut UiNode, taken: &mut BTreeSet<String>) {
    node.key = free_key(taken, &format!("{}_copy", node.key));
    for child in &mut node.children {
        rename_copy(child, taken);
    }
}

fn free_key(taken: &mut BTreeSet<String>, base: &str) -> String {
    if taken.insert(base.to_owned()) {
        return base.to_owned();
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}{suffix}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

/// 检查器数值行的显示值：空串 = 该字段未设置（面板显示「未设置」占位）。
fn layout_row_values(layout: &UiLayoutSpec) -> Vec<(LayoutField, String)> {
    vec![
        (LayoutField::Anchor, layout.anchor.as_str().to_owned()),
        (
            LayoutField::Offsets,
            format_values(layout.offsets.map(|v| v.to_vec())),
        ),
        (
            LayoutField::Size,
            format_values(layout.size.map(|v| v.to_vec())),
        ),
        (
            LayoutField::Position,
            format_values(layout.position.map(|v| v.to_vec())),
        ),
        (
            LayoutField::Pivot,
            format_values(layout.pivot.map(|v| v.to_vec())),
        ),
        (
            LayoutField::Percent,
            format_values(layout.percent.map(|v| v.to_vec())),
        ),
        (
            LayoutField::Grow,
            format_values(layout.grow.map(|v| vec![v])),
        ),
        (
            LayoutField::Shrink,
            format_values(layout.shrink.map(|v| vec![v])),
        ),
        (
            LayoutField::MinSize,
            format_values(layout.min_size.map(|v| v.to_vec())),
        ),
        (
            LayoutField::MaxSize,
            format_values(layout.max_size.map(|v| v.to_vec())),
        ),
        (
            LayoutField::AlignSelf,
            layout
                .align_self
                .map(align_as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
    ]
}

fn format_values(values: Option<Vec<f32>>) -> String {
    values
        .map(|values| {
            values
                .into_iter()
                .map(format_number)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn format_number(value: f32) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let rounded = value.round();
    if (value - rounded).abs() < 1e-6 {
        format!("{rounded}")
    } else {
        let text = format!("{value:.3}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

fn align_as_str(align: Align) -> &'static str {
    match align {
        Align::Start => "start",
        Align::Center => "center",
        Align::End => "end",
        Align::Stretch => "stretch",
    }
}

fn parse_align(text: &str) -> Result<Align, String> {
    match text.trim() {
        "start" => Ok(Align::Start),
        "center" => Ok(Align::Center),
        "end" => Ok(Align::End),
        "stretch" => Ok(Align::Stretch),
        other => Err(format!(
            "align_self 只能是 start/center/end/stretch，收到 `{other}`"
        )),
    }
}

fn parse_anchor(text: &str) -> Result<AnchorPreset, String> {
    AnchorPreset::ALL
        .into_iter()
        .find(|preset| preset.as_str() == text.trim())
        .ok_or_else(|| {
            let names = AnchorPreset::ALL
                .iter()
                .map(|preset| preset.as_str())
                .collect::<Vec<_>>()
                .join("/");
            format!("anchor 只能是 {names}，收到 `{}`", text.trim())
        })
}

/// 逗号或空白分隔的数字；空串 = 清除。
fn parse_values(text: &str) -> Result<Vec<f32>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for part in trimmed.split([',', ' ', '\t']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let value: f32 = part.parse().map_err(|_| format!("`{part}` 不是数字"))?;
        if !value.is_finite() {
            return Err("数值必须是有限数".to_owned());
        }
        values.push(value);
    }
    Ok(values)
}

fn parse_exact<const N: usize>(text: &str, field: &str) -> Result<Option<[f32; N]>, String> {
    let values = parse_values(text)?;
    if values.is_empty() {
        return Ok(None);
    }
    let values: [f32; N] = values.try_into().map_err(|values: Vec<f32>| {
        format!(
            "{field} 需要 {N} 个数字，收到 {} 个（空串 = 清除）",
            values.len()
        )
    })?;
    Ok(Some(values))
}

fn parse_scalar(text: &str, field: &str) -> Result<Option<f32>, String> {
    let values = parse_values(text)?;
    match values.as_slice() {
        [] => Ok(None),
        [value] => Ok(Some(*value)),
        _ => Err(format!("{field} 需要 1 个数字（空串 = 清除）")),
    }
}

fn parse_non_negative(text: &str, field: &str) -> Result<Option<f32>, String> {
    let value = parse_scalar(text, field)?;
    if value.is_some_and(|value| value < 0.0) {
        return Err(format!("{field} 不能为负"));
    }
    Ok(value)
}

fn parse_positive_pair(text: &str, field: &str) -> Result<Option<[f32; 2]>, String> {
    let pair = parse_exact::<2>(text, field)?;
    if pair.is_some_and(|pair| pair.iter().any(|value| *value <= 0.0)) {
        return Err(format!("{field} 必须为正数"));
    }
    Ok(pair)
}

/// 在候选线里找离三条移动边最近的一条；返回 (修正量, 命中的线)。
fn best_snap(lines: &[f32], edges: [f32; 3], threshold: f32) -> (f32, Option<f32>) {
    let mut best: Option<(f32, f32)> = None;
    for &line in lines {
        for &edge in &edges {
            let correction = line - edge;
            if correction.abs() <= threshold
                && best.is_none_or(|(current, _)| correction.abs() < current.abs())
            {
                best = Some((correction, line));
            }
        }
    }
    best.map_or((0.0, None), |(correction, line)| (correction, Some(line)))
}

/// 子树 → 独立的 `prefab` 文档（「导出为 UI 预设」的纯函数部分，与落盘无关）。
///
/// 保留的是**这棵子树真正引用到的**东西：节点自己的 `style`、样式里出现的 `@token`、
/// 节点 `src` 里以 `asset:` 声明的资产、目标落在子树内的绑定。画布口径照搬源文档——
/// 预设要按作者当时的设计分辨率预览才有意义。
fn prefab_document(spec: &UiSpec, node: &UiNode, document: &str) -> UiSpec {
    let mut keys = BTreeSet::new();
    let mut styles = BTreeSet::new();
    let mut srcs = BTreeSet::new();
    collect_prefab_refs(node, &mut keys, &mut styles, &mut srcs);
    UiSpec {
        spec_version: spec.spec_version,
        document: UiDocumentMeta {
            kind: DocumentKind::Prefab,
            name: document_name(document),
            canvas: spec.document.canvas.clone(),
        },
        tokens: prefab_tokens(spec, &styles),
        styles: spec
            .styles
            .iter()
            .filter(|(name, _)| styles.contains(name.as_str()))
            .map(|(name, style)| (name.clone(), style.clone()))
            .collect(),
        root: node.clone(),
        assets: spec
            .assets
            .iter()
            .filter(|asset| {
                srcs.iter()
                    .any(|src| src.strip_prefix("asset:") == Some(asset.key.as_str()))
            })
            .cloned()
            .collect(),
        bindings: spec
            .bindings
            .iter()
            .filter(|binding| keys.contains(binding.target.as_str()))
            .cloned()
            .collect(),
    }
}

fn collect_prefab_refs(
    node: &UiNode,
    keys: &mut BTreeSet<String>,
    styles: &mut BTreeSet<String>,
    srcs: &mut BTreeSet<String>,
) {
    keys.insert(node.key.clone());
    if let Some(style) = &node.style {
        styles.insert(style.clone());
    }
    if let Some(src) = node_src(&node.kind) {
        srcs.insert(src.to_owned());
    }
    for child in &node.children {
        collect_prefab_refs(child, keys, styles, srcs);
    }
}

fn node_src(kind: &UiNodeKind) -> Option<&str> {
    match kind {
        UiNodeKind::Panel { src } | UiNodeKind::Button { src, .. } => src.as_deref(),
        UiNodeKind::Image { src, .. } => Some(src.as_str()),
        _ => None,
    }
}

/// 被保留的样式里引用到的令牌（值形如 `@color.text`，也认数组/对象里嵌套的引用）。
fn prefab_tokens(spec: &UiSpec, styles: &BTreeSet<String>) -> BTreeMap<String, serde_json::Value> {
    let mut names = BTreeSet::new();
    for (name, style) in &spec.styles {
        if !styles.contains(name.as_str()) {
            continue;
        }
        for values in std::iter::once(&style.base).chain(style.states.values()) {
            for value in values.values() {
                collect_token_ref(value, &mut names);
            }
        }
    }
    spec.tokens
        .iter()
        .filter(|(name, _)| names.contains(name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn collect_token_ref(value: &serde_json::Value, names: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::String(raw) => {
            for word in raw.split_whitespace() {
                if let Some(name) = word.strip_prefix('@') {
                    names.insert(name.to_owned());
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_token_ref(item, names);
            }
        }
        serde_json::Value::Object(fields) => {
            for item in fields.values() {
                collect_token_ref(item, names);
            }
        }
        _ => {}
    }
}

/// `ui/prefabs/health_bar.ui.json` → `health_bar`（文档名进 `UiDocumentMeta.name`）。
fn document_name(document: &str) -> String {
    let file = document.rsplit('/').next().unwrap_or(document);
    file.strip_suffix(".ui.json").unwrap_or(file).to_owned()
}

/// 控件创建模板（M80-07）：契约要求子节点的控件一次建齐，别让用户建出校验不过的形状。
///
/// 目前只有 `Scrollbar` 需要：`handle` 必须指名一个真实子节点，宿主按名字把它的矩形改写成
/// "长度 = 轨道 × size、位置 = value 行程"（见 `rustlua_ui::ui_scrollbar`）。漏了这条就是
/// "编辑器能编、运行时不动"。轨道按初始尺寸 12×160 是竖的，方向因此一并落成
/// `top_to_bottom`，与尺寸同一口径。
fn with_creation_template(
    kind: UiNodeKind,
    key: &str,
    taken: &mut BTreeSet<String>,
) -> (UiNodeKind, Vec<UiNode>) {
    let UiNodeKind::Scrollbar {
        value,
        size,
        handle,
        ..
    } = kind
    else {
        return (kind, Vec::new());
    };
    let handle_key = free_key(taken, &format!("{key}_handle"));
    // 滑块用 panel 而不是 image：`Image.src` 为空会被校验判红，刚建出来的控件不该带错。
    let knob = UiNode {
        key: handle_key.clone(),
        kind: UiNodeKind::Panel { src: None },
        layout: UiLayoutSpec {
            size: Some([12.0, 48.0]),
            ..UiLayoutSpec::default()
        },
        style: None,
        events: BTreeMap::new(),
        children: Vec::new(),
        extra: BTreeMap::new(),
    };
    (
        UiNodeKind::Scrollbar {
            value,
            size,
            direction: ScrollbarDirection::TopToBottom,
            handle: handle.or(Some(handle_key)),
        },
        vec![knob],
    )
}

/// 新控件的初始布局：参照 Unity 新建 UI 元素的口径——面板/遮罩/组铺满父级，
/// 文本/按钮等取常用尺寸并居中。
fn default_layout_for(kind: &UiNodeKind) -> UiLayoutSpec {
    if matches!(
        kind,
        UiNodeKind::Panel { .. }
            | UiNodeKind::Mask { .. }
            | UiNodeKind::CanvasGroup { .. }
            | UiNodeKind::ScrollView { .. }
            | UiNodeKind::SafeArea
    ) {
        return UiLayoutSpec {
            offsets: Some([0.0; 4]),
            ..UiLayoutSpec::default()
        };
    }
    let size = match kind {
        UiNodeKind::Container { .. } => [200.0, 200.0],
        UiNodeKind::Text { .. } => [160.0, 30.0],
        UiNodeKind::Image { .. } => [100.0, 100.0],
        UiNodeKind::Button { .. } | UiNodeKind::Input { .. } => [160.0, 30.0],
        UiNodeKind::Toggle { .. } | UiNodeKind::Slider { .. } => [160.0, 20.0],
        UiNodeKind::Progress { .. } => [160.0, 8.0],
        UiNodeKind::Scrollbar { .. } => [12.0, 160.0],
        UiNodeKind::Spacer => [40.0, 8.0],
        UiNodeKind::Tooltip { .. } => [160.0, 24.0],
        UiNodeKind::List { .. }
        | UiNodeKind::Grid { .. }
        | UiNodeKind::Tabs { .. }
        | UiNodeKind::Modal => [240.0, 160.0],
        UiNodeKind::Panel { .. }
        | UiNodeKind::Mask { .. }
        | UiNodeKind::CanvasGroup { .. }
        | UiNodeKind::ScrollView { .. }
        | UiNodeKind::SafeArea => unreachable!("上面已按铺满父级返回"),
    };
    UiLayoutSpec {
        size: Some(size),
        ..UiLayoutSpec::default()
    }
}

/// 候选图卡片（M76-05）：来源边车、是否已登记进某份 `.ui.json` 的 `assets[]`。
#[derive(Debug, Clone)]
pub struct AssetCandidate {
    pub key: String,
    pub path: Option<String>,
    pub provenance: Option<rustlua_ui_spec::AssetProvenance>,
    pub bytes: Option<u64>,
    pub registered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewResolution {
    #[default]
    Hd720,
    FullHd1080,
    Phone375x667,
}

impl PreviewResolution {
    pub const ALL: [Self; 3] = [Self::Hd720, Self::FullHd1080, Self::Phone375x667];

    #[must_use]
    pub fn size(self) -> [u32; 2] {
        match self {
            Self::Hd720 => [1280, 720],
            Self::FullHd1080 => [1920, 1080],
            Self::Phone375x667 => [375, 667],
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Hd720 => "1280×720",
            Self::FullHd1080 => "1920×1080",
            Self::Phone375x667 => "375×667",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectorSection {
    Layout,
    Style,
    State,
    Event,
    Binding,
}

impl InspectorSection {
    pub const ALL: [Self; 5] = [
        Self::Layout,
        Self::Style,
        Self::State,
        Self::Event,
        Self::Binding,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Layout => "布局",
            Self::Style => "样式",
            Self::State => "状态",
            Self::Event => "事件",
            Self::Binding => "绑定",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HierarchyRow {
    pub key: String,
    pub depth: usize,
    pub kind: &'static str,
    pub children: usize,
}

/// 多选对齐的基准边/线（以整组选中框为基准，与 Unity「Align to selection」同语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignMode {
    Left,
    CenterX,
    Right,
    Top,
    CenterY,
    Bottom,
}

impl AlignMode {
    pub const ALL: [Self; 6] = [
        Self::Left,
        Self::CenterX,
        Self::Right,
        Self::Top,
        Self::CenterY,
        Self::Bottom,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Left => "左对齐",
            Self::CenterX => "水平居中",
            Self::Right => "右对齐",
            Self::Top => "顶对齐",
            Self::CenterY => "垂直居中",
            Self::Bottom => "底对齐",
        }
    }
}

/// 分布轴向：首尾不动，中间项等间距。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistributeAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SnapGuides {
    /// 吸附到的竖直参考线（设计坐标 x）；没吸上为 `None`。
    pub x: Option<f32>,
    /// 吸附到的水平参考线（设计坐标 y）。
    pub y: Option<f32>,
}

/// 检查器数值行的目标字段。面板只按这个枚举生成提案，不自己拼 JSON Pointer。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutField {
    Anchor,
    Offsets,
    Size,
    Position,
    Pivot,
    Percent,
    Grow,
    Shrink,
    MinSize,
    MaxSize,
    AlignSelf,
}

impl LayoutField {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Anchor => "anchor",
            Self::Offsets => "offsets",
            Self::Size => "size",
            Self::Position => "position",
            Self::Pivot => "pivot",
            Self::Percent => "percent",
            Self::Grow => "grow",
            Self::Shrink => "shrink",
            Self::MinSize => "min_size",
            Self::MaxSize => "max_size",
            Self::AlignSelf => "align_self",
        }
    }
}

/// 检查器字段的写回通道：面板只按它生成提案，不自己拼 JSON Pointer。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectorEdit {
    None,
    /// 单个布局数值/枚举字段（锚点、四边内缩、尺寸、pivot…）。
    LayoutField(LayoutField),
    /// 样式类名，空串 = 清除。
    Style,
    /// 事件名（值是新函数名）。
    Event(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectorRow {
    pub section: InspectorSection,
    pub field: String,
    pub value: String,
    pub read_only: bool,
    pub edit: InspectorEdit,
}

#[derive(Debug, Clone)]
struct SnapshotEntry {
    document: String,
    /// 这一项要写回的字节。
    bytes: Vec<u8>,
    /// 写回前文档必须仍然等于这些字节；不等说明有外部编辑，拒绝覆盖。
    expected: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct UiStudio {
    store: UiSpecStore,
    document: Option<String>,
    spec: Option<UiSpec>,
    revision: u64,
    /// 主选中：检查器与画布手柄都作用在它身上（多选时 = 最后点中的那个）。
    selection: Option<String>,
    /// 其余选中项（多选对齐/分布/整组拖动用）；主选中永远不在这里重复。
    extra_selection: BTreeSet<String>,
    section: InspectorSection,
    resolution: PreviewResolution,
    safe_area: bool,
    undo: Vec<SnapshotEntry>,
    redo: Vec<SnapshotEntry>,
    /// 累计**接受**次数（撤销/重做的挪动不计数）。跨面板全局 Undo 的签名用它区分
    /// "UI 工作室刚刚又接受了一步"，见 [`UiStudio::undo_signature`]。
    accepts: u64,
}

impl UiStudio {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            store: UiSpecStore::new(root),
            document: None,
            spec: None,
            revision: 0,
            selection: None,
            extra_selection: BTreeSet::new(),
            section: InspectorSection::Layout,
            resolution: PreviewResolution::default(),
            safe_area: false,
            undo: Vec::new(),
            redo: Vec::new(),
            accepts: 0,
        }
    }

    pub fn open(&mut self, document: &str) -> UiHostResult<()> {
        let loaded = self.store.load(document)?;
        self.document = Some(loaded.document);
        self.revision = loaded.revision;
        self.selection = Some(loaded.spec.root.key.clone());
        self.extra_selection.clear();
        self.spec = Some(loaded.spec);
        Ok(())
    }

    /// 重新读取当前文档（接受提案后刷新 revision 与内容）。
    pub fn reload(&mut self) -> UiHostResult<()> {
        let Some(document) = self.document.clone() else {
            return Ok(());
        };
        // 接受提案后保留整组选中；节点被删掉了才从选中集里消失。
        let previous_primary = self.selection.clone();
        let previous_extras = std::mem::take(&mut self.extra_selection);
        self.open(&document)?;
        let mut kept: Vec<String> = Vec::new();
        for key in previous_primary.into_iter().chain(previous_extras) {
            let exists = self
                .spec
                .as_ref()
                .is_some_and(|spec| spec.find_node(&key).is_some());
            if exists && !kept.contains(&key) {
                kept.push(key);
            }
        }
        if let Some(first) = kept.first().cloned() {
            self.selection = Some(first);
            self.extra_selection = kept.into_iter().skip(1).collect();
        }
        Ok(())
    }

    pub fn document(&self) -> Option<&str> {
        self.document.as_deref()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn spec(&self) -> Option<&UiSpec> {
        self.spec.as_ref()
    }

    pub fn selection(&self) -> Option<&str> {
        self.selection.as_deref()
    }

    pub fn select(&mut self, key: &str) -> bool {
        let found = self
            .spec
            .as_ref()
            .is_some_and(|spec| spec.find_node(key).is_some());
        if found {
            self.selection = Some(key.to_owned());
            self.extra_selection.clear();
        }
        found
    }

    /// 多选切换（Ctrl/Shift 点选）：新点中的项成为主选中，之前的主选中降为附加项；
    /// 再次点中已选中项则把它移出选中集（移出主选中时由下一个附加项顶上）。
    pub fn toggle_select(&mut self, key: &str) {
        if self.selection.as_deref() == Some(key) {
            if let Some(next) = self.extra_selection.iter().next().cloned() {
                self.extra_selection.remove(&next);
                self.selection = Some(next);
            } else {
                self.selection = None;
            }
            return;
        }
        if self.extra_selection.remove(key) {
            return;
        }
        let found = self
            .spec
            .as_ref()
            .is_some_and(|spec| spec.find_node(key).is_some());
        if found {
            if let Some(previous) = self.selection.take() {
                self.extra_selection.insert(previous);
            }
            self.selection = Some(key.to_owned());
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.extra_selection.clear();
    }

    /// 整组选中，按层级顺序（对齐/分布的输入顺序必须是确定的）。
    #[must_use]
    pub fn selected_keys(&self) -> Vec<String> {
        if self.selection.is_none() && self.extra_selection.is_empty() {
            return Vec::new();
        }
        self.hierarchy()
            .into_iter()
            .map(|row| row.key)
            .filter(|key| {
                self.selection.as_deref() == Some(key.as_str())
                    || self.extra_selection.contains(key)
            })
            .collect()
    }

    #[must_use]
    pub fn selection_count(&self) -> usize {
        self.selected_keys().len()
    }

    pub fn set_section(&mut self, section: InspectorSection) {
        self.section = section;
    }

    pub fn section(&self) -> InspectorSection {
        self.section
    }

    pub fn set_resolution(&mut self, resolution: PreviewResolution) {
        self.resolution = resolution;
    }

    pub fn resolution(&self) -> PreviewResolution {
        self.resolution
    }

    pub fn set_safe_area(&mut self, enabled: bool) {
        self.safe_area = enabled;
    }

    pub fn safe_area(&self) -> bool {
        self.safe_area
    }

    /// 设计像素坐标系下的安全区。手机档预留刘海与底部手势区，其余档为全屏。
    #[must_use]
    pub fn safe_area_rect(&self) -> [f32; 4] {
        let [w, h] = self.resolution.size();
        let (w, h) = (w as f32, h as f32);
        if !self.safe_area {
            return [0.0, 0.0, w, h];
        }
        match self.resolution {
            PreviewResolution::Phone375x667 => [0.0, 44.0, w, h - 44.0 - 34.0],
            _ => [0.0, 0.0, w, h],
        }
    }

    /// 当前预览档位的设计→像素适应参数（与 `render_preview` 同一规则）。
    #[must_use]
    pub fn preview_fit(&self) -> (f32, [f32; 2]) {
        let [width, height] = self.resolution.size();
        let reference = self
            .spec
            .as_ref()
            .map_or([1280.0, 720.0], |spec| spec.document.canvas.reference);
        rustlua_ui_spec::preview_fit(reference, width, height, 1.0)
    }

    /// 画布叠层用的节点矩形（设计坐标，含滚动裁剪）。
    #[must_use]
    pub fn layout_rects(&self) -> Vec<rustlua_ui_spec::LayoutRect> {
        self.spec
            .as_ref()
            .map(rustlua_ui_spec::layout_rects)
            .unwrap_or_default()
    }

    /// 选中节点的九宫格内缩（设计像素）；非 image 或未配置时为 `None`。
    #[must_use]
    pub fn selected_nine_slice(&self) -> Option<[f32; 4]> {
        let node = self
            .spec
            .as_ref()
            .zip(self.selection.as_deref())
            .and_then(|(spec, key)| spec.find_node(key))?;
        match &node.kind {
            UiNodeKind::Image { nine_slice, .. } => *nine_slice,
            _ => None,
        }
    }

    /// 语义层级（深度优先，含容器/控件），与检查器选中同步用。
    #[must_use]
    pub fn hierarchy(&self) -> Vec<HierarchyRow> {
        fn visit(node: &rustlua_ui_spec::UiNode, depth: usize, rows: &mut Vec<HierarchyRow>) {
            rows.push(HierarchyRow {
                key: node.key.clone(),
                depth,
                kind: node.kind.type_name(),
                children: node.children.len(),
            });
            for child in &node.children {
                visit(child, depth + 1, rows);
            }
        }
        let mut rows = Vec::new();
        if let Some(spec) = &self.spec {
            visit(&spec.root, 0, &mut rows);
        }
        rows
    }

    /// 五段检查器；选中节点的只读字段（key/type）标 `read_only`。
    #[must_use]
    pub fn inspector_rows(&self) -> Vec<InspectorRow> {
        let Some(node) = self
            .spec
            .as_ref()
            .zip(self.selection.as_deref())
            .and_then(|(spec, key)| spec.find_node(key))
        else {
            return Vec::new();
        };
        let mut rows = vec![
            InspectorRow {
                section: InspectorSection::Layout,
                field: "key".to_owned(),
                value: node.key.clone(),
                read_only: true,
                edit: InspectorEdit::None,
            },
            InspectorRow {
                section: InspectorSection::Layout,
                field: "type".to_owned(),
                value: node.kind.type_name().to_owned(),
                read_only: true,
                edit: InspectorEdit::None,
            },
        ];
        for (field, value) in layout_row_values(&node.layout) {
            rows.push(InspectorRow {
                section: InspectorSection::Layout,
                field: field.label().to_owned(),
                value,
                read_only: false,
                edit: InspectorEdit::LayoutField(field),
            });
        }
        rows.push(InspectorRow {
            section: InspectorSection::Style,
            field: "style".to_owned(),
            value: node.style.clone().unwrap_or_default(),
            read_only: false,
            edit: InspectorEdit::Style,
        });
        let states = node
            .style
            .as_deref()
            .and_then(|name| self.spec.as_ref()?.styles.get(name))
            .map(|style| {
                style
                    .states
                    .keys()
                    .map(|state| style_state_label(*state).to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        rows.push(InspectorRow {
            section: InspectorSection::State,
            field: "states".to_owned(),
            value: if states.is_empty() {
                "<无状态覆盖>".to_owned()
            } else {
                states.join(", ")
            },
            read_only: true,
            edit: InspectorEdit::None,
        });
        for (event, function) in &node.events {
            rows.push(InspectorRow {
                section: InspectorSection::Event,
                field: event.clone(),
                value: function.clone(),
                read_only: false,
                edit: InspectorEdit::Event(event.clone()),
            });
        }
        if let Some(spec) = &self.spec {
            for binding in spec
                .bindings
                .iter()
                .filter(|binding| binding.target == node.key)
            {
                rows.push(InspectorRow {
                    section: InspectorSection::Binding,
                    field: binding.property.clone(),
                    value: format!("{} ({:?})", binding.source, binding.mode),
                    // 绑定表不在节点补丁的作用域里，先只读展示，避免假装可写。
                    read_only: true,
                    edit: InspectorEdit::None,
                });
            }
        }
        rows.retain(|row| row.section == self.section || row.read_only);
        rows
    }

    /// 空检查器段的占位说明，避免面板看起来“丢字段”。
    #[must_use]
    pub fn section_placeholder(&self) -> &'static str {
        match self.section {
            InspectorSection::Layout => "选择节点后编辑锚点/尺寸",
            InspectorSection::Style => "未指定样式类",
            InspectorSection::State => "没有状态覆盖",
            InspectorSection::Event => "没有事件绑定",
            InspectorSection::Binding => "没有数据绑定",
        }
    }

    /// 当前档位的离屏预览（PNG）。预览失败不影响文档。
    pub fn preview_png(&self) -> Result<Vec<u8>, String> {
        let spec = self.spec.as_ref().ok_or("未打开 UI 文档")?;
        let [width, height] = self.resolution.size();
        render_preview(spec, width, height, 1.0)
    }

    /// 当前文档的结构诊断（预览/报告共用）。
    #[must_use]
    pub fn diagnostics(&self) -> Vec<rustlua_ui_spec::Diagnostic> {
        self.spec
            .as_ref()
            .map(|spec| validate_spec(spec, self.store.capabilities()))
            .unwrap_or_default()
    }

    fn require_document(&self) -> UiHostResult<String> {
        self.document.clone().ok_or_else(|| {
            crate::ui_spec_host::UiHostError::new(
                crate::ui_spec_host::UiHostErrorCode::InvalidArgument,
                "UI 工作室尚未打开文档",
            )
        })
    }

    pub fn submit_patch(&self, patch: UiPatch) -> UiHostResult<UiProposal> {
        self.store.submit_patch(&self.require_document()?, patch)
    }

    pub fn submit_patch_subset(
        &self,
        patch: UiPatch,
        indices: &[usize],
    ) -> UiHostResult<UiProposal> {
        self.store
            .submit_patch_subset(&self.require_document()?, patch, indices)
    }

    /// 令牌增删改也走提案通道：调用只生成提案，接受后才写回文档。
    pub fn set_token(&self, name: &str, value: serde_json::Value) -> UiHostResult<UiProposal> {
        self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops: vec![UiPatchOp::SetToken {
                name: name.to_owned(),
                value,
            }],
        })
    }

    pub fn remove_token(&self, name: &str) -> UiHostResult<UiProposal> {
        self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops: vec![UiPatchOp::RemoveToken {
                name: name.to_owned(),
            }],
        })
    }

    /// 候选图列表（M76-05）：已登记资产 + 只有来源边车的生成候选，按 key 合并排序。
    pub fn asset_candidates(&self) -> Vec<AssetCandidate> {
        let mut by_key: BTreeMap<String, AssetCandidate> = BTreeMap::new();
        for listing in self.store.list_assets(None).unwrap_or_default() {
            by_key.insert(
                listing.key.clone(),
                AssetCandidate {
                    key: listing.key,
                    path: listing.path,
                    provenance: listing.provenance,
                    bytes: listing.bytes,
                    registered: true,
                },
            );
        }
        for provenance in self.store.list_provenance() {
            let entry = by_key
                .entry(provenance.asset_id.clone())
                .or_insert_with(|| AssetCandidate {
                    key: provenance.asset_id.clone(),
                    path: Some(format!(
                        "assets/generated/ui/{}.png",
                        crate::ui_spec_host::sanitize_asset_key(&provenance.asset_id)
                    )),
                    provenance: None,
                    bytes: None,
                    registered: false,
                });
            entry.provenance = Some(provenance);
        }
        by_key.into_values().collect()
    }

    /// 工程级资产审查（来源/体积/未审阅引用），面板上显示 error/warning 计数。
    pub fn asset_review(&self) -> UiHostResult<rustlua_ui_spec::AssetReviewReport> {
        self.store
            .review_assets(None, &rustlua_ui_spec::AssetLimits::default())
    }

    /// 人工审阅通过：写 provenance 边车（`reviewed=true` + 许可），不动 `.ui.json`。
    pub fn review_asset(
        &self,
        key: &str,
        license: Option<String>,
    ) -> UiHostResult<rustlua_ui_spec::AssetProvenance> {
        let existing = self
            .store
            .list_provenance()
            .into_iter()
            .find(|provenance| provenance.asset_id == key);
        // 只点「更新审阅」不重填许可时不能把已有许可抹掉。
        let license = license.or_else(|| existing.as_ref().and_then(|p| p.license.clone()));
        let used_by = existing.map(|p| p.used_by).unwrap_or_default();
        self.store.promote_asset(key, true, license, used_by)
    }

    /// 读工程内资产原始字节（缩略图用）；拒绝绝对路径与 `..`。
    pub fn read_asset_bytes(&self, path: &str) -> Option<Vec<u8>> {
        let relative = std::path::Path::new(path);
        if path.trim().is_empty()
            || relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return None;
        }
        std::fs::read(self.store.root().join(relative)).ok()
    }

    /// 切图落盘：宿主做 IO，边界与像素拷贝由 `rustlua-ui-spec` 的纯函数决定。
    pub fn slice_asset(
        &self,
        source: &str,
        regions: Vec<rustlua_ui_spec::SliceRegion>,
        output_dir: &str,
    ) -> UiHostResult<crate::ui_spec_host::SliceManifest> {
        self.store.slice_asset(source, regions, output_dir)
    }

    /// 检查器/画布的通用写回：落到节点的 JSON Pointer 字段，仍然只生成提案。
    pub fn set_field(
        &self,
        key: &str,
        path: &str,
        value: serde_json::Value,
    ) -> UiHostResult<UiProposal> {
        self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops: vec![UiPatchOp::Replace {
                key: key.to_owned(),
                path: path.to_owned(),
                value,
            }],
        })
    }

    pub fn set_style(&self, key: &str, style: Option<&str>) -> UiHostResult<UiProposal> {
        self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops: vec![UiPatchOp::SetStyle {
                key: key.to_owned(),
                style: style.map(str::to_owned),
            }],
        })
    }

    pub fn set_event(&self, key: &str, event: &str, function: &str) -> UiHostResult<UiProposal> {
        self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops: vec![UiPatchOp::SetEvent {
                key: key.to_owned(),
                event: event.to_owned(),
                function: function.to_owned(),
            }],
        })
    }

    /// 检查器数值行 → 布局提案。空串统一表示「清除该字段」，不是 0。
    ///
    /// `offsets`（四边内缩）与 `size/position/percent`（锚点定位）互斥：写一侧就清另一侧，
    /// 与画布拖拽的语义一致，不会让校验器替作者猜哪套坐标该生效。
    pub fn set_layout_field(
        &self,
        key: &str,
        field: LayoutField,
        text: &str,
    ) -> UiHostResult<UiProposal> {
        if field == LayoutField::Anchor {
            return self.apply_anchor_preset(key, parse_anchor(text).map_err(invalid_argument)?);
        }
        let node = self
            .require_spec()?
            .find_node(key)
            .ok_or_else(|| invalid_argument(format!("节点不存在：{key}")))?;
        let mut layout = node.layout.clone();
        match field {
            LayoutField::Anchor => unreachable!("anchor 已在上面分派"),
            LayoutField::Offsets => {
                layout.offsets = parse_exact::<4>(text, "offsets").map_err(invalid_argument)?;
                if layout.offsets.is_some() {
                    layout.size = None;
                    layout.position = None;
                    layout.percent = None;
                }
            }
            LayoutField::Size => {
                layout.size = parse_positive_pair(text, "size").map_err(invalid_argument)?;
                if layout.size.is_some() {
                    layout.offsets = None;
                }
            }
            LayoutField::Position => {
                layout.position = parse_exact::<2>(text, "position").map_err(invalid_argument)?;
                if layout.position.is_some() {
                    layout.offsets = None;
                }
            }
            LayoutField::Pivot => {
                layout.pivot = parse_exact::<2>(text, "pivot").map_err(invalid_argument)?;
            }
            LayoutField::Percent => {
                let percent = parse_exact::<2>(text, "percent").map_err(invalid_argument)?;
                if percent.is_some_and(|percent| {
                    percent.iter().any(|value| *value < 0.0 || *value > 100.0)
                }) {
                    return Err(invalid_argument("percent 必须在 0..=100 之间"));
                }
                layout.percent = percent;
                if layout.percent.is_some() {
                    layout.offsets = None;
                }
            }
            LayoutField::Grow => {
                layout.grow = parse_non_negative(text, "grow").map_err(invalid_argument)?;
            }
            LayoutField::Shrink => {
                layout.shrink = parse_non_negative(text, "shrink").map_err(invalid_argument)?;
            }
            LayoutField::MinSize => {
                layout.min_size = parse_exact::<2>(text, "min_size").map_err(invalid_argument)?;
            }
            LayoutField::MaxSize => {
                layout.max_size = parse_exact::<2>(text, "max_size").map_err(invalid_argument)?;
            }
            LayoutField::AlignSelf => {
                let trimmed = text.trim();
                layout.align_self = if trimmed.is_empty() {
                    None
                } else {
                    Some(parse_align(trimmed).map_err(invalid_argument)?)
                };
            }
        }
        self.write_layout(key, layout)
    }

    /// 锚点预设：换锚点但**不动画面**——把当前绝对矩形改写成 `size + position` 形式，
    /// 位置按新锚点反解。`offsets` 是不看锚点的四边内缩，留着它会得到「换了锚点却没变」。
    pub fn apply_anchor_preset(&self, key: &str, preset: AnchorPreset) -> UiHostResult<UiProposal> {
        let (rect, parent) = self
            .node_frame(key)
            .ok_or_else(|| invalid_argument(format!("节点不存在或没有布局：{key}")))?;
        let node = self
            .require_spec()?
            .find_node(key)
            .ok_or_else(|| invalid_argument(format!("节点不存在：{key}")))?
            .clone();
        let mut layout = node.layout.clone();
        let pivot = layout.pivot.unwrap_or(layout.anchor.fraction());
        let [ax, ay] = preset.fraction();
        // 拉伸轴解出来的宽度就是父宽：位置必须按父宽反解，按旧矩形宽会横移。
        let stretch_x = matches!(
            preset,
            AnchorPreset::StretchHorizontal | AnchorPreset::FullRect
        );
        let stretch_y = matches!(
            preset,
            AnchorPreset::StretchVertical | AnchorPreset::FullRect
        );
        let width = if stretch_x { parent[2] } else { rect[2] };
        let height = if stretch_y { parent[3] } else { rect[3] };
        layout.anchor = preset;
        layout.offsets = None;
        layout.percent = None;
        layout.size = Some([width, height]);
        layout.position = Some([
            rect[0] - parent[0] - parent[2] * ax + width * pivot[0],
            rect[1] - parent[1] - parent[3] * ay + height * pivot[1],
        ]);
        layout.pivot = Some(pivot);
        self.write_layout(key, layout)
    }

    fn write_layout(&self, key: &str, layout: UiLayoutSpec) -> UiHostResult<UiProposal> {
        let value = serde_json::to_value(layout)
            .map_err(|error| invalid_argument(format!("布局序列化失败：{error}")))?;
        self.set_field(key, "/layout", value)
    }

    /// 在 `parent` 下追加一个控件；key 由控件类型自动取名（冲突加序号）。
    /// 返回提案与新 key：接受之后面板用它把选中落到新节点上。
    pub fn add_node(&self, parent: &str, kind: UiNodeKind) -> UiHostResult<(UiProposal, String)> {
        let spec = self.require_spec()?;
        if spec.find_node(parent).is_none() {
            return Err(invalid_argument(format!("父节点不存在：{parent}")));
        }
        if !self.accepts_children(parent) {
            return Err(invalid_argument(format!("{parent} 不能接收子节点")));
        }
        let mut taken: BTreeSet<String> = spec
            .walk_nodes()
            .into_iter()
            .map(|(node, _)| node.key.clone())
            .collect();
        let key = free_key(&mut taken, kind.type_name());
        let layout = default_layout_for(&kind);
        let (kind, children) = with_creation_template(kind, &key, &mut taken);
        let node = UiNode {
            key: key.clone(),
            kind,
            layout,
            style: None,
            events: BTreeMap::new(),
            children,
            extra: BTreeMap::new(),
        };
        let proposal = self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops: vec![UiPatchOp::Insert {
                parent: parent.to_owned(),
                index: None,
                node: Box::new(node),
            }],
        })?;
        Ok((proposal, key))
    }

    /// 「导出为 UI 预设」（M80-07）：把 `key` 为根的子树落成一份**独立**的 `prefab` 文档。
    ///
    /// 与 Unity 把 UI 子树拖进 Project 窗口同口径：新文档只带这棵子树和它真正用到的
    /// 样式/令牌/资产/绑定。整份文档照抄会让预设拖着兄弟节点与别人的绑定，那样就不叫预设了。
    ///
    /// 只接受尚不存在的目标：整份 Spec 提案的 `base_revision` 是 0，目标已存在时
    /// [`UiSpecStore::accept`] 会判过期——这里提前报清楚，别让用户等到接受那一步才发现。
    pub fn export_prefab(
        &self,
        key: &str,
        document: &str,
    ) -> UiHostResult<crate::ui_spec_host::AcceptedUi> {
        let spec = self.require_spec()?;
        let node = spec
            .find_node(key)
            .ok_or_else(|| invalid_argument(format!("节点不存在：{key}")))?;
        if self.store.resolve(document)?.exists() {
            return Err(invalid_argument(format!(
                "目标预设已存在：{document}；换一个文件名再导出"
            )));
        }
        let prefab = prefab_document(spec, node, document);
        let proposal = self.store.submit_spec(document, 0, prefab)?;
        self.store.accept(&proposal.proposal_id)
    }

    /// 层级操作：删除 / 复制 / 移到新父级 / 包裹进新容器。全部走提案。
    pub fn remove_node(&self, key: &str) -> UiHostResult<UiProposal> {
        self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops: vec![UiPatchOp::Remove {
                key: key.to_owned(),
            }],
        })
    }

    pub fn move_node(
        &self,
        key: &str,
        parent: &str,
        index: Option<i32>,
    ) -> UiHostResult<UiProposal> {
        self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops: vec![UiPatchOp::Move {
                key: key.to_owned(),
                parent: parent.to_owned(),
                index,
            }],
        })
    }

    /// 复制节点为同父级兄弟，副本及子孙的 key 依次加 `_copy` 后缀。
    pub fn duplicate_node(&self, key: &str) -> UiHostResult<UiProposal> {
        let spec = self.require_spec()?;
        let node = spec
            .find_node(key)
            .ok_or_else(|| invalid_argument(format!("节点不存在：{key}")))?;
        let (parent, index) = parent_slot(&spec.root, key)
            .ok_or_else(|| invalid_argument(format!("不能复制文档根节点：{key}")))?;
        let mut taken: BTreeSet<String> = spec
            .walk_nodes()
            .into_iter()
            .map(|(node, _)| node.key.clone())
            .collect();
        let mut copy = node.clone();
        rename_copy(&mut copy, &mut taken);
        self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops: vec![UiPatchOp::Insert {
                parent,
                index: Some(index as i32 + 1),
                node: Box::new(copy),
            }],
        })
    }

    /// 包裹选中节点：先在原位置插一个 `Container`，再把节点移进去。
    /// 两个 op 在同一个补丁里，校验按“已执行到这一步”的状态判父级是否存在。
    pub fn wrap_node(&self, key: &str, container_key: &str) -> UiHostResult<UiProposal> {
        let spec = self.require_spec()?;
        if spec.find_node(key).is_none() {
            return Err(invalid_argument(format!("节点不存在：{key}")));
        }
        if container_key.trim().is_empty() {
            return Err(invalid_argument("容器 key 不能为空"));
        }
        let (parent, index) = parent_slot(&spec.root, key)
            .ok_or_else(|| invalid_argument(format!("不能包裹文档根节点：{key}")))?;
        let container = UiNode {
            key: container_key.to_owned(),
            kind: UiNodeKind::Container {
                direction: rustlua_ui_spec::ContainerDirection::default(),
                padding: None,
                spacing: None,
                justify: rustlua_ui_spec::Justify::default(),
                align: rustlua_ui_spec::Align::default(),
                columns: None,
            },
            layout: rustlua_ui_spec::UiLayoutSpec {
                offsets: Some([0.0, 0.0, 0.0, 0.0]),
                ..Default::default()
            },
            style: None,
            events: BTreeMap::new(),
            children: Vec::new(),
            extra: BTreeMap::new(),
        };
        self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops: vec![
                UiPatchOp::Insert {
                    parent,
                    index: Some(index as i32),
                    node: Box::new(container),
                },
                UiPatchOp::Move {
                    key: key.to_owned(),
                    parent: container_key.to_owned(),
                    index: None,
                },
            ],
        })
    }

    /// 节点的父级与下标（层级拖动算落点用）；根节点返回 `None`。
    #[must_use]
    pub fn parent_and_index(&self, key: &str) -> Option<(String, usize)> {
        self.spec
            .as_ref()
            .and_then(|spec| parent_slot(&spec.root, key))
    }

    /// 该节点能否接收子节点。只给真正的容器类控件放行——补丁校验只挡
    /// `is_leaf()`（文字/图片等），按钮/开关这类能通过校验但不该有子节点。
    #[must_use]
    pub fn accepts_children(&self, key: &str) -> bool {
        self.spec
            .as_ref()
            .and_then(|spec| spec.find_node(key))
            .is_some_and(|node| {
                matches!(
                    node.kind,
                    UiNodeKind::Panel { .. }
                        | UiNodeKind::Container { .. }
                        | UiNodeKind::ScrollView { .. }
                        | UiNodeKind::List { .. }
                        | UiNodeKind::Grid { .. }
                        | UiNodeKind::Tabs { .. }
                        | UiNodeKind::Modal
                        | UiNodeKind::SafeArea
                )
            })
    }

    /// 画布用：选中节点的绝对矩形与父矩形（根节点的父就是画布参考分辨率）。
    #[must_use]
    pub fn selection_frame(&self) -> Option<(String, [f32; 4], [f32; 4])> {
        let key = self.selection.as_deref()?;
        let (rect, parent) = self.node_frame(key)?;
        Some((key.to_owned(), rect, parent))
    }

    /// 整组选中的矩形（画布叠层/对齐分布共用），按层级顺序。
    #[must_use]
    pub fn selection_frames(&self) -> Vec<(String, [f32; 4])> {
        self.selected_node_frames()
            .into_iter()
            .map(|(key, rect, _)| (key, rect))
            .collect()
    }

    /// 整组选中 + 各自父矩形（对齐/分布/整组拖动都要用它换算 offsets）。
    fn selected_node_frames(&self) -> Vec<(String, [f32; 4], [f32; 4])> {
        let selected: BTreeSet<String> = self.selected_keys().into_iter().collect();
        self.node_frames()
            .into_iter()
            .filter(|(key, _, _)| selected.contains(key))
            .collect()
    }

    /// 每个节点的绝对矩形 + 父矩形。父矩形取节点声明父级的实际矩形，
    /// 根节点的父就是画布参考分辨率（与预览器同一口径）。
    fn node_frames(&self) -> Vec<(String, [f32; 4], [f32; 4])> {
        let Some(spec) = self.spec.as_ref() else {
            return Vec::new();
        };
        let rows = self.layout_rects();
        let reference = spec.document.canvas.reference;
        let canvas = [0.0, 0.0, reference[0], reference[1]];
        let hierarchy = self.hierarchy();
        let order: BTreeMap<&str, usize> = hierarchy
            .iter()
            .enumerate()
            .map(|(index, row)| (row.key.as_str(), index))
            .collect();
        let mut frames: Vec<(String, [f32; 4], [f32; 4])> = rows
            .iter()
            .map(|row| {
                let parent = row
                    .parent
                    .as_ref()
                    .and_then(|parent| rows.iter().find(|candidate| &candidate.key == parent))
                    .map_or(canvas, |candidate| candidate.rect);
                (row.key.clone(), row.rect, parent)
            })
            .collect();
        frames.sort_by_key(|(key, _, _)| order.get(key.as_str()).copied().unwrap_or(usize::MAX));
        frames
    }

    fn node_frame(&self, key: &str) -> Option<([f32; 4], [f32; 4])> {
        self.node_frames()
            .into_iter()
            .find(|(candidate, _, _)| candidate == key)
            .map(|(_, rect, parent)| (rect, parent))
    }

    /// 画布拖动：整组选中按设计像素平移（改 `offsets`，绝对定位）。
    pub fn move_selection_by(&self, delta: [f32; 2]) -> UiHostResult<UiProposal> {
        let items: Vec<(String, [f32; 4], [f32; 4])> = self
            .selected_node_frames()
            .into_iter()
            .map(|(key, rect, parent)| {
                (
                    key,
                    [rect[0] + delta[0], rect[1] + delta[1], rect[2], rect[3]],
                    parent,
                )
            })
            .collect();
        if items.is_empty() {
            return Err(invalid_argument("没有选中节点"));
        }
        self.submit_layout_ops(items)
    }

    /// 画布手柄：按 `handle` 拖动 `delta` 改变矩形，转成 offsets 提案。
    pub fn resize_selection(
        &self,
        handle: ResizeHandle,
        delta: [f32; 2],
    ) -> UiHostResult<UiProposal> {
        let (key, rect, parent) = self
            .selection_frame()
            .ok_or_else(|| invalid_argument("没有选中节点"))?;
        let next = resize_rect(rect, handle, delta, MIN_CANVAS_SIZE);
        self.submit_layout_ops(vec![(key, next, parent)])
    }

    /// 多选对齐：以整组选中框为基准（左/中/右、上/中/下），一次提案写入全部节点。
    pub fn align_selection(&self, mode: AlignMode) -> UiHostResult<UiProposal> {
        let frames = self.selected_node_frames();
        if frames.len() < 2 {
            return Err(invalid_argument("对齐至少需要选中 2 个节点"));
        }
        // 基准框在改写前一次算齐：逐项重算会让第二项跟着第一项动。
        let min_x = frames
            .iter()
            .map(|(_, rect, _)| rect[0])
            .fold(f32::INFINITY, f32::min);
        let max_right = frames
            .iter()
            .map(|(_, rect, _)| rect[0] + rect[2])
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = frames
            .iter()
            .map(|(_, rect, _)| rect[1])
            .fold(f32::INFINITY, f32::min);
        let max_bottom = frames
            .iter()
            .map(|(_, rect, _)| rect[1] + rect[3])
            .fold(f32::NEG_INFINITY, f32::max);
        let items = frames
            .into_iter()
            .map(|(key, rect, parent)| {
                let [x, y, w, h] = rect;
                let target = match mode {
                    AlignMode::Left => [min_x, y, w, h],
                    AlignMode::CenterX => [(min_x + max_right) * 0.5 - w * 0.5, y, w, h],
                    AlignMode::Right => [max_right - w, y, w, h],
                    AlignMode::Top => [x, min_y, w, h],
                    AlignMode::CenterY => [x, (min_y + max_bottom) * 0.5 - h * 0.5, w, h],
                    AlignMode::Bottom => [x, max_bottom - h, w, h],
                };
                (key, target, parent)
            })
            .collect();
        self.submit_layout_ops(items)
    }

    /// 多选分布：首尾不动，中间项按等间距排开（Unity「Distribute Spacing」口径）。
    pub fn distribute_selection(&self, axis: DistributeAxis) -> UiHostResult<UiProposal> {
        let mut frames = self.selected_node_frames();
        if frames.len() < 3 {
            return Err(invalid_argument("分布至少需要选中 3 个节点"));
        }
        let horizontal = axis == DistributeAxis::Horizontal;
        let start = |rect: &[f32; 4]| if horizontal { rect[0] } else { rect[1] };
        let size = |rect: &[f32; 4]| if horizontal { rect[2] } else { rect[3] };
        frames.sort_by(|a, b| start(&a.1).total_cmp(&start(&b.1)));
        let last_index = frames.len() - 1;
        let span = start(&frames[last_index].1) + size(&frames[last_index].1) - start(&frames[0].1);
        let total: f32 = frames.iter().map(|(_, rect, _)| size(rect)).sum();
        let gap = (span - total) / last_index as f32;
        let last_start = start(&frames[last_index].1);
        let mut cursor = start(&frames[0].1);
        let mut items = Vec::with_capacity(frames.len());
        for (index, (key, rect, parent)) in frames.into_iter().enumerate() {
            // 最后一项直接用它的原位置：浮点累加误差不能把"首尾不动"变成"尾动一点"。
            let position = if index == last_index {
                last_start
            } else {
                cursor
            };
            cursor += size(&rect) + gap;
            let target = if horizontal {
                [position, rect[1], rect[2], rect[3]]
            } else {
                [rect[0], position, rect[2], rect[3]]
            };
            items.push((key, target, parent));
        }
        self.submit_layout_ops(items)
    }

    /// 拖动吸附：把增量夹到「选中框边/中线」与「父级边/中线、兄弟边/中线」重合的位置。
    /// 返回修正后的位移与命中的参考线（画布画十字线用）；没吸上时增量原样返回。
    #[must_use]
    pub fn snap_delta(&self, delta: [f32; 2], threshold: f32) -> ([f32; 2], SnapGuides) {
        let Some(primary) = self.selection.clone() else {
            return (delta, SnapGuides::default());
        };
        if threshold.is_nan() || threshold <= 0.0 {
            return (delta, SnapGuides::default());
        }
        let rows = self.layout_rects();
        let Some(row) = rows.iter().find(|row| row.key == primary) else {
            return (delta, SnapGuides::default());
        };
        let Some(parent_rect) = row.parent.as_ref().map_or_else(
            || {
                let reference = self
                    .spec
                    .as_ref()
                    .map_or([1280.0, 720.0], |spec| spec.document.canvas.reference);
                Some([0.0, 0.0, reference[0], reference[1]])
            },
            |parent| {
                rows.iter()
                    .find(|candidate| &candidate.key == parent)
                    .map(|candidate| candidate.rect)
            },
        ) else {
            return (delta, SnapGuides::default());
        };
        let mut xs = vec![
            parent_rect[0],
            parent_rect[0] + parent_rect[2] * 0.5,
            parent_rect[0] + parent_rect[2],
        ];
        let mut ys = vec![
            parent_rect[1],
            parent_rect[1] + parent_rect[3] * 0.5,
            parent_rect[1] + parent_rect[3],
        ];
        for sibling in rows
            .iter()
            .filter(|candidate| candidate.parent == row.parent && candidate.key != primary)
        {
            xs.extend([
                sibling.rect[0],
                sibling.rect[0] + sibling.rect[2] * 0.5,
                sibling.rect[0] + sibling.rect[2],
            ]);
            ys.extend([
                sibling.rect[1],
                sibling.rect[1] + sibling.rect[3] * 0.5,
                sibling.rect[1] + sibling.rect[3],
            ]);
        }
        let moved = [
            row.rect[0] + delta[0],
            row.rect[1] + delta[1],
            row.rect[2],
            row.rect[3],
        ];
        let (dx, guide_x) = best_snap(
            &xs,
            [moved[0], moved[0] + moved[2] * 0.5, moved[0] + moved[2]],
            threshold,
        );
        let (dy, guide_y) = best_snap(
            &ys,
            [moved[1], moved[1] + moved[3] * 0.5, moved[1] + moved[3]],
            threshold,
        );
        (
            [delta[0] + dx, delta[1] + dy],
            SnapGuides {
                x: guide_x,
                y: guide_y,
            },
        )
    }

    /// 绝对矩形 → 一批 `Replace /layout` 操作（每个节点一条，一次提案）。
    /// 写 offsets 会清掉 size/position/percent：画布上看到的矩形就是落盘的定位口径。
    fn submit_layout_ops(
        &self,
        items: Vec<(String, [f32; 4], [f32; 4])>,
    ) -> UiHostResult<UiProposal> {
        let spec = self.require_spec()?;
        let mut ops = Vec::with_capacity(items.len());
        for (key, target, parent) in items {
            let mut layout = spec
                .find_node(&key)
                .ok_or_else(|| invalid_argument(format!("节点不存在：{key}")))?
                .layout
                .clone();
            layout.offsets = Some(offsets_for_rect(parent, target));
            layout.size = None;
            layout.position = None;
            layout.percent = None;
            let value = serde_json::to_value(layout)
                .map_err(|error| invalid_argument(format!("布局序列化失败：{error}")))?;
            ops.push(UiPatchOp::Replace {
                key,
                path: "/layout".to_owned(),
                value,
            });
        }
        self.submit_patch(UiPatch {
            base_revision: self.revision(),
            ops,
        })
    }

    fn require_spec(&self) -> UiHostResult<&UiSpec> {
        self.spec
            .as_ref()
            .ok_or_else(|| invalid_argument("UI 工作室尚未打开文档"))
    }

    /// 接受提案并重新载入文档；接受前的原始字节进撤销栈（一次 Undo 复原）。
    /// 新接受会让已撤销的分支作废（和通用编辑器的 redo 语义一致）。
    pub fn accept(&mut self, proposal_id: &str) -> UiHostResult<crate::ui_spec_host::AcceptedUi> {
        let document = self.require_document()?;
        let before = self.store.snapshot(&document)?;
        let accepted = self.store.accept(proposal_id)?;
        let after = self.store.snapshot(&document)?;
        self.undo.push(SnapshotEntry {
            document,
            bytes: before,
            expected: after,
        });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.accepts = self.accepts.wrapping_add(1);
        self.reload()?;
        Ok(accepted)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// 跨面板全局 Undo 的**编辑签名**：`(累计接受次数, undo+redo 深度)`。
    ///
    /// 计数只在 [`UiStudio::accept`] 真正入栈时变大（撤销/重做只挪栈）；深度项在换工程
    /// 清空两条栈后回落到 0，负责把归属从账本上摘走。只用深度会在"撤销一步后紧接着
    /// 再接受一步"时回到同一个数，也会在栈到达 `UNDO_LIMIT` 后停止变化。
    /// 消费方见 editor bin 的 `actions/undo_ledger.rs`。
    #[must_use]
    pub fn undo_signature(&self) -> (u64, usize) {
        (self.accepts, self.undo.len() + self.redo.len())
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// 写回前确认文档仍是快照后的样子；外部编辑不能被撤销/重做静默覆盖。
    fn ensure_unchanged(&self, document: &str, expected: &[u8], action: &str) -> UiHostResult<()> {
        let current = self.store.snapshot(document)?;
        if current != expected {
            return Err(UiHostError::new(
                UiHostErrorCode::StaleRevision,
                format!(
                    "{action}被拒绝：{document} 在快照之后被外部修改（当前 revision {}，期望 {}）；请先刷新",
                    rustlua_ui_spec::revision_hash(&current),
                    rustlua_ui_spec::revision_hash(expected)
                ),
            ));
        }
        Ok(())
    }

    /// 撤销最近一次接受：写回接受前的原始字节并刷新 revision。
    /// 快照校验失败时保留撤销栈项，工作区不变；成功时当前字节进重做栈。
    pub fn undo(&mut self) -> UiHostResult<bool> {
        let Some(entry) = self.undo.last().cloned() else {
            return Ok(false);
        };
        self.ensure_unchanged(&entry.document, &entry.expected, "撤销")?;
        self.store.restore(&entry.document, &entry.bytes)?;
        self.undo.pop();
        self.redo.push(SnapshotEntry {
            document: entry.document.clone(),
            bytes: entry.expected,
            expected: entry.bytes,
        });
        if self.redo.len() > UNDO_LIMIT {
            self.redo.remove(0);
        }
        if self.document.as_deref() == Some(entry.document.as_str()) {
            self.reload()?;
        }
        Ok(true)
    }

    /// 重做最近一次被撤销的接受：恢复撤销前的字节。空栈是 no-op。
    pub fn redo(&mut self) -> UiHostResult<bool> {
        let Some(entry) = self.redo.last().cloned() else {
            return Ok(false);
        };
        self.ensure_unchanged(&entry.document, &entry.expected, "重做")?;
        self.store.restore(&entry.document, &entry.bytes)?;
        self.redo.pop();
        self.undo.push(SnapshotEntry {
            document: entry.document.clone(),
            bytes: entry.expected,
            expected: entry.bytes,
        });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        if self.document.as_deref() == Some(entry.document.as_str()) {
            self.reload()?;
        }
        Ok(true)
    }

    pub fn reject(&self, proposal_id: &str) -> UiHostResult<()> {
        self.store.reject(proposal_id)
    }

    /// 补丁提案的原始操作（提案箱逐条勾选）。整份 Spec 提案返回空列表。
    pub fn proposal_ops(&self, proposal_id: &str) -> UiHostResult<Vec<UiPatchOp>> {
        self.store.proposal_ops(proposal_id)
    }

    /// op 级拆分接受：只把 `indices` 选中的操作落盘。
    ///
    /// 复用 `submit_patch_subset` 的 revision/能力/整份重校验，再按普通提案接受，
    /// 因此撤销/重做、过期校验与审计读数与整体接受完全同一条路径。
    pub fn accept_subset(
        &mut self,
        proposal_id: &str,
        indices: &[usize],
    ) -> UiHostResult<crate::ui_spec_host::AcceptedUi> {
        let stored = self.store.stored_proposal(proposal_id)?;
        let Some(patch) = stored.patch else {
            return Err(UiHostError::new(
                UiHostErrorCode::InvalidArgument,
                "整份 Spec 提案没有可拆分的操作",
            ));
        };
        let subset = self
            .store
            .submit_patch_subset(&stored.document, patch, indices)?;
        self.accept(&subset.proposal_id)
    }

    pub fn proposals(&self) -> UiHostResult<Vec<UiProposal>> {
        Ok(self
            .store
            .list_proposals()?
            .into_iter()
            .map(|stored| stored.proposal)
            .collect())
    }

    /// 各状态覆盖矩阵（样式面板用）：类 → 状态 → 属性键。
    #[must_use]
    pub fn state_matrix(&self) -> BTreeMap<String, BTreeMap<StyleState, Vec<String>>> {
        let mut matrix = BTreeMap::new();
        if let Some(spec) = &self.spec {
            for (name, style) in &spec.styles {
                let states = style
                    .states
                    .iter()
                    .map(|(state, values)| (*state, values.keys().cloned().collect()))
                    .collect();
                matrix.insert(name.clone(), states);
            }
        }
        matrix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_ui_spec::{Capabilities, Severity, SliceRegion, UiPatchOp};

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("rustlua-m76-studio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_spec(root: &std::path::Path) -> u64 {
        std::fs::create_dir_all(root.join("ui")).unwrap();
        let spec: UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "shop"},
            "tokens": {"color.surface": "#101014"},
            "styles": {"card": {"base": {"bg": "@color.surface"},
                                 "states": {"hover": {"bg": "#202028"}}}},
            "root": {
                "key": "root",
                "type": "panel",
                "style": "card",
                "children": [
                    {"key": "title", "type": "text", "text": "Shop"},
                    {"key": "buy", "type": "button", "text": "Buy"}
                ]
            },
            "bindings": [{"target": "title", "property": "text", "source": "lua:shop.title"}]
        }))
        .unwrap();
        let encoded = crate::ui_spec_host::encode_spec(&spec).unwrap();
        std::fs::write(root.join("ui/shop.ui.json"), &encoded).unwrap();
        rustlua_ui_spec::revision_hash(&encoded)
    }

    #[test]
    fn hierarchy_and_inspector_follow_selection() {
        let root = temp_root("tree");
        let revision = write_spec(&root);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();
        assert_eq!(studio.revision(), revision);
        let rows = studio.hierarchy();
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[1].key.as_str(), rows[1].depth), ("title", 1));
        assert!(studio.select("buy"));
        assert_eq!(studio.selection(), Some("buy"));

        studio.set_section(InspectorSection::Style);
        let rows = studio.inspector_rows();
        assert!(
            rows.iter()
                .any(|row| row.field == "style" && row.value.is_empty()),
            "无样式时字段为空串，面板才会给出可编辑输入框"
        );
        studio.set_section(InspectorSection::Binding);
        assert!(
            !studio
                .inspector_rows()
                .iter()
                .any(|row| row.section == InspectorSection::Binding),
            "buy 没有绑定，只保留只读 key/type"
        );

        studio.select("root");
        studio.set_section(InspectorSection::Style);
        let rows = studio.inspector_rows();
        assert!(
            rows.iter()
                .any(|row| row.field == "style" && row.value == "card")
        );
        studio.set_section(InspectorSection::State);
        let rows = studio.inspector_rows();
        assert!(rows.iter().any(|row| row.value.contains("悬停")));

        studio.select("title");
        studio.set_section(InspectorSection::Binding);
        assert!(
            studio
                .inspector_rows()
                .iter()
                .any(|row| row.field == "text" && row.value.contains("lua:shop.title"))
        );
    }

    #[test]
    fn previews_and_safe_area_are_per_resolution() {
        let root = temp_root("preview");
        write_spec(&root);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();
        for resolution in PreviewResolution::ALL {
            studio.set_resolution(resolution);
            let png = studio.preview_png().unwrap();
            assert!(png.starts_with(b"\x89PNG"), "{resolution:?}");
            assert_eq!(
                studio.safe_area_rect(),
                [
                    0.0,
                    0.0,
                    resolution.size()[0] as f32,
                    resolution.size()[1] as f32
                ]
            );
        }
        studio.set_resolution(PreviewResolution::Phone375x667);
        studio.set_safe_area(true);
        assert_eq!(
            studio.safe_area_rect(),
            [0.0, 44.0, 375.0, 667.0 - 44.0 - 34.0]
        );
    }

    #[test]
    fn proposals_are_accepted_through_the_store() {
        let root = temp_root("proposal");
        write_spec(&root);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();
        let patch = UiPatch {
            base_revision: studio.revision(),
            ops: vec![UiPatchOp::SetEvent {
                key: "buy".to_owned(),
                event: "click".to_owned(),
                function: "shop.buy".to_owned(),
            }],
        };
        let proposal = studio.submit_patch(patch.clone()).unwrap();
        assert_eq!(proposal.diff.nodes_changed, 1);
        assert_eq!(
            studio.revision(),
            patch.base_revision,
            "接受前 revision 不变"
        );
        let accepted = studio.accept(&proposal.proposal_id).unwrap();
        assert_eq!(accepted.revision, studio.revision());
        assert_eq!(accepted.document, "ui/shop.ui.json");
        assert_ne!(studio.revision(), patch.base_revision, "接受后必须刷新");
        let buy = studio.spec().unwrap().find_node("buy").unwrap();
        assert_eq!(
            buy.events.get("click").map(String::as_str),
            Some("shop.buy")
        );

        let stale = studio
            .submit_patch(patch)
            .expect_err("旧 revision 必须拒绝");
        assert!(stale.is(crate::ui_spec_host::UiHostErrorCode::StaleRevision));
    }

    #[test]
    fn accepting_a_proposal_undoes_once_to_the_previous_bytes() {
        let root = temp_root("undo");
        let original_revision = write_spec(&root);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();
        let patch = UiPatch {
            base_revision: studio.revision(),
            ops: vec![UiPatchOp::SetEvent {
                key: "buy".to_owned(),
                event: "click".to_owned(),
                function: "shop.buy".to_owned(),
            }],
        };
        let proposal = studio.submit_patch(patch).unwrap();
        assert!(!studio.can_undo(), "提交提案不进撤销栈");
        studio.accept(&proposal.proposal_id).unwrap();
        assert!(studio.can_undo());
        assert_ne!(studio.revision(), original_revision);
        assert_eq!(
            studio
                .spec()
                .unwrap()
                .find_node("buy")
                .unwrap()
                .events
                .get("click"),
            Some(&"shop.buy".to_owned())
        );

        assert!(studio.undo().unwrap());
        assert_eq!(
            studio.revision(),
            original_revision,
            "撤销必须回到原始 revision"
        );
        assert!(
            studio
                .spec()
                .unwrap()
                .find_node("buy")
                .unwrap()
                .events
                .is_empty()
        );
        assert!(!studio.can_undo());
        assert!(!studio.undo().unwrap(), "空栈撤销是 no-op");
    }

    #[test]
    fn redo_replays_the_acceptance_and_a_new_accept_clears_it() {
        let root = temp_root("redo");
        let original_revision = write_spec(&root);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();
        let patch = UiPatch {
            base_revision: studio.revision(),
            ops: vec![UiPatchOp::SetEvent {
                key: "buy".to_owned(),
                event: "click".to_owned(),
                function: "shop.buy".to_owned(),
            }],
        };
        let proposal = studio.submit_patch(patch).unwrap();
        let accepted_revision = studio.accept(&proposal.proposal_id).unwrap().revision;
        assert!(!studio.can_redo(), "新接受必须清空重做栈");

        assert!(studio.undo().unwrap());
        assert_eq!(studio.revision(), original_revision);
        assert!(studio.can_redo());
        assert!(studio.redo().unwrap());
        assert_eq!(
            studio.revision(),
            accepted_revision,
            "重做必须回到接受后的 revision"
        );
        assert_eq!(
            studio
                .spec()
                .unwrap()
                .find_node("buy")
                .unwrap()
                .events
                .get("click"),
            Some(&"shop.buy".to_owned())
        );
        assert!(!studio.can_redo());
        assert!(!studio.redo().unwrap(), "空栈重做是 no-op");

        // 撤销后换一条新分支：旧的重做项必须作废。
        assert!(studio.undo().unwrap());
        assert!(studio.can_redo());
        let next = studio
            .submit_patch(UiPatch {
                base_revision: studio.revision(),
                ops: vec![UiPatchOp::SetEvent {
                    key: "buy".to_owned(),
                    event: "click".to_owned(),
                    function: "shop.cancel".to_owned(),
                }],
            })
            .unwrap();
        studio.accept(&next.proposal_id).unwrap();
        assert!(!studio.can_redo(), "接受新提案后旧重做分支必须丢弃");
    }

    #[test]
    fn split_accept_applies_only_selected_ops() {
        let root = temp_root("split");
        write_spec(&root);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();
        let proposal = studio
            .submit_patch(UiPatch {
                base_revision: studio.revision(),
                ops: vec![
                    UiPatchOp::SetEvent {
                        key: "buy".to_owned(),
                        event: "click".to_owned(),
                        function: "shop.buy".to_owned(),
                    },
                    UiPatchOp::SetEvent {
                        key: "title".to_owned(),
                        event: "click".to_owned(),
                        function: "shop.title".to_owned(),
                    },
                ],
            })
            .unwrap();
        assert_eq!(
            studio.proposal_ops(&proposal.proposal_id).unwrap().len(),
            2,
            "补丁提案必须保留原始 ops 供逐条勾选"
        );
        studio
            .accept_subset(&proposal.proposal_id, &[0])
            .expect("拆分接受");
        let spec = studio.spec().unwrap();
        assert_eq!(
            spec.find_node("buy")
                .unwrap()
                .events
                .get("click")
                .map(String::as_str),
            Some("shop.buy")
        );
        assert!(
            spec.find_node("title").unwrap().events.is_empty(),
            "未选中的 op 不得落盘"
        );
        assert!(studio.undo().unwrap(), "拆分接受同样进撤销栈");
    }

    #[test]
    fn token_edits_are_proposals_and_keep_referenced_tokens_safe() {
        let root = temp_root("tokens");
        let original = write_spec(&root);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();

        let proposal = studio
            .set_token("color.accent", serde_json::json!("#ff8800"))
            .expect("新增令牌应生成提案");
        assert_eq!(proposal.diff.tokens_changed, 1, "令牌改动要进提案读数");
        assert_eq!(studio.revision(), original, "提案阶段不写文档");
        assert!(!studio.spec().unwrap().tokens.contains_key("color.accent"));

        studio.accept(&proposal.proposal_id).unwrap();
        assert_eq!(
            studio.spec().unwrap().tokens.get("color.accent"),
            Some(&serde_json::json!("#ff8800"))
        );
        assert!(studio.undo().unwrap());
        assert!(!studio.spec().unwrap().tokens.contains_key("color.accent"));

        let error = studio
            .remove_token("color.surface")
            .expect_err("仍被样式引用的令牌不能删");
        assert!(
            error.to_string().contains("color.surface"),
            "错误要指出令牌名：{error}"
        );
        assert!(
            studio.spec().unwrap().tokens.contains_key("color.surface"),
            "拒绝后文档必须原样"
        );
    }

    #[test]
    fn undo_and_redo_refuse_to_overwrite_external_edits() {
        let root = temp_root("external");
        write_spec(&root);
        let path = root.join("ui/shop.ui.json");
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();
        let proposal = studio
            .set_token("color.accent", serde_json::json!("#ff8800"))
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let accepted = std::fs::read(&path).unwrap();

        let external = [accepted.as_slice(), b"\n"].concat();
        std::fs::write(&path, &external).unwrap();
        let error = studio.undo().expect_err("外部编辑后撤销必须拒绝");
        assert!(error.is(UiHostErrorCode::StaleRevision), "{error}");
        assert!(studio.can_undo(), "拒绝时不能丢撤销栈");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            external,
            "外部内容不能被覆盖"
        );

        std::fs::write(&path, &accepted).unwrap();
        assert!(studio.undo().unwrap());
        assert!(studio.can_redo());
        let external = [std::fs::read(&path).unwrap().as_slice(), b"\n"].concat();
        std::fs::write(&path, &external).unwrap();
        let error = studio.redo().expect_err("外部编辑后重做必须拒绝");
        assert!(error.is(UiHostErrorCode::StaleRevision), "{error}");
        assert!(studio.can_redo(), "拒绝时不能丢重做栈");
        assert_eq!(std::fs::read(&path).unwrap(), external);
    }

    fn assert_close(actual: [f32; 4], expected: [f32; 4]) {
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-3, "{actual:?} != {expected:?}");
        }
    }

    /// 三个尺寸/位置各不相同的按钮：对齐、分布、吸附都靠它们算出可判定的读数。
    fn write_alignment_spec(root: &std::path::Path) -> u64 {
        std::fs::create_dir_all(root.join("ui")).unwrap();
        let spec: UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "align"},
            "root": {
                "key": "root",
                "type": "panel",
                "layout": {"offsets": [0, 0, 0, 0]},
                "children": [
                    {"key": "a", "type": "button", "text": "A",
                     "layout": {"anchor": "top_left", "size": [50, 40], "position": [10, 10]}},
                    {"key": "b", "type": "button", "text": "B",
                     "layout": {"anchor": "top_left", "size": [70, 20], "position": [100, 30]}},
                    {"key": "c", "type": "button", "text": "C",
                     "layout": {"anchor": "top_left", "size": [30, 30], "position": [200, 60]}}
                ]
            }
        }))
        .unwrap();
        let encoded = crate::ui_spec_host::encode_spec(&spec).unwrap();
        std::fs::write(root.join("ui/align.ui.json"), &encoded).unwrap();
        rustlua_ui_spec::revision_hash(&encoded)
    }

    fn open_alignment_studio(name: &str) -> (std::path::PathBuf, UiStudio) {
        let root = temp_root(name);
        write_alignment_spec(&root);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/align.ui.json").unwrap();
        (root, studio)
    }

    fn rect_of(studio: &UiStudio, key: &str) -> [f32; 4] {
        studio
            .selection_frames()
            .into_iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, rect)| rect)
            .or_else(|| {
                studio
                    .layout_rects()
                    .into_iter()
                    .find(|row| row.key == key)
                    .map(|row| row.rect)
            })
            .unwrap()
    }

    #[test]
    fn numeric_layout_rows_edit_the_spec_and_keep_modes_exclusive() {
        let (_root, mut studio) = open_alignment_studio("layout-rows");
        studio.select("b");
        let rows = studio.inspector_rows();
        for field in [
            "anchor",
            "offsets",
            "size",
            "position",
            "pivot",
            "percent",
            "grow",
            "shrink",
            "min_size",
            "max_size",
            "align_self",
        ] {
            assert!(
                rows.iter().any(|row| row.field == field),
                "布局段缺少数值行 {field}"
            );
        }
        assert!(
            rows.iter().all(|row| row.field != "layout"),
            "整段 JSON 行已被数值行取代"
        );
        let row = rows.iter().find(|row| row.field == "size").unwrap();
        assert_eq!(row.value, "70, 20");
        assert_eq!(row.edit, InspectorEdit::LayoutField(LayoutField::Size));

        // 写 offsets 会清掉 size/position（两套坐标口径互斥）。
        let proposal = studio
            .set_layout_field("b", LayoutField::Offsets, "8, 9, 10, 11")
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let layout = &studio.spec().unwrap().find_node("b").unwrap().layout;
        assert_eq!(layout.offsets, Some([8.0, 9.0, 10.0, 11.0]));
        assert_eq!(layout.size, None);
        assert_eq!(layout.position, None);

        // 再写 size 又清回锚点定位；空串 = 清除字段。
        let proposal = studio
            .set_layout_field("b", LayoutField::Size, "70, 20")
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let layout = &studio.spec().unwrap().find_node("b").unwrap().layout;
        assert_eq!(layout.offsets, None);
        assert_eq!(layout.size, Some([70.0, 20.0]));
        let proposal = studio.set_layout_field("b", LayoutField::Size, "").unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        assert_eq!(
            studio.spec().unwrap().find_node("b").unwrap().layout.size,
            None
        );

        let error = studio
            .set_layout_field("b", LayoutField::Percent, "120, 50")
            .expect_err("percent 超过 100 必须拒绝");
        assert!(error.to_string().contains("0..=100"), "{error}");
        let error = studio
            .set_layout_field("b", LayoutField::Grow, "-1")
            .expect_err("grow 不能为负");
        assert!(error.to_string().contains("不能为负"), "{error}");
        let error = studio
            .set_layout_field("b", LayoutField::Anchor, "middle")
            .expect_err("未知锚点必须报错");
        assert!(error.to_string().contains("anchor"), "{error}");
    }

    #[test]
    fn anchor_presets_change_the_anchor_without_moving_the_node() {
        let (_root, mut studio) = open_alignment_studio("anchor-preset");
        studio.select("b");
        let before = rect_of(&studio, "b");
        assert_close(before, [100.0, 30.0, 70.0, 20.0]);

        let proposal = studio
            .apply_anchor_preset("b", AnchorPreset::BottomRight)
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let layout = &studio.spec().unwrap().find_node("b").unwrap().layout;
        assert_eq!(layout.anchor, AnchorPreset::BottomRight);
        assert_eq!(layout.offsets, None, "换锚点必须改成锚点定位，否则画面不动");
        assert_eq!(layout.position, Some([-1180.0, -690.0]));
        assert_close(rect_of(&studio, "b"), before);

        // 拉伸锚点：左边界保持不动，宽度交给父级。
        let proposal = studio
            .apply_anchor_preset("b", AnchorPreset::FullRect)
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let stretched = rect_of(&studio, "b");
        assert_close(
            [stretched[0], stretched[1], stretched[2], stretched[3]],
            [100.0, 30.0, 1280.0, 720.0],
        );
    }

    #[test]
    fn adding_a_child_uses_kind_defaults_and_a_free_key() {
        let (_root, mut studio) = open_alignment_studio("add-node");
        studio.select("root");
        let (proposal, key) = studio
            .add_node(
                "root",
                UiNodeKind::Text {
                    rich_text: false,
                    text: "Hi".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(key, "text");
        studio.accept(&proposal.proposal_id).unwrap();
        let node = studio.spec().unwrap().find_node("text").unwrap();
        assert_eq!(node.layout.size, Some([160.0, 30.0]));
        assert_eq!(node.layout.anchor, AnchorPreset::Center);
        assert_eq!(
            studio.parent_and_index("text"),
            Some(("root".to_owned(), 3)),
            "新节点追加在末尾"
        );

        let (_proposal, key) = studio
            .add_node(
                "root",
                UiNodeKind::Text {
                    rich_text: false,
                    text: "Again".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(key, "text2", "同名控件自动加序号");

        let error = studio
            .add_node(
                "a",
                UiNodeKind::Text {
                    rich_text: false,
                    text: "No".to_owned(),
                },
            )
            .expect_err("按钮不能接收子节点");
        assert!(error.to_string().contains("不能接收子节点"), "{error}");

        let (proposal, key) = studio
            .add_node("root", UiNodeKind::Mask { padding: None })
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let mask = studio.spec().unwrap().find_node(&key).unwrap();
        assert_eq!(
            mask.layout.offsets,
            Some([0.0; 4]),
            "遮罩默认铺满父级，裁剪区才有意义"
        );
    }

    /// 控件创建模板（M80-07）：契约要求子节点的控件一次建齐，建出来就得能过校验。
    #[test]
    fn scrollbar_creation_template_builds_its_handle_child() {
        let (_root, mut studio) = open_alignment_studio("scrollbar-template");
        studio.select("root");
        let (proposal, key) = studio
            .add_node(
                "root",
                UiNodeKind::Scrollbar {
                    value: 0.0,
                    size: 0.4,
                    direction: ScrollbarDirection::LeftToRight,
                    handle: None,
                },
            )
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();

        let spec = studio.spec().unwrap();
        let bar = spec.find_node(&key).unwrap();
        assert_eq!(bar.layout.size, Some([12.0, 160.0]));
        let UiNodeKind::Scrollbar {
            direction, handle, ..
        } = &bar.kind
        else {
            panic!("新建的应是 Scrollbar");
        };
        assert_eq!(
            *direction,
            ScrollbarDirection::TopToBottom,
            "竖轨道 12×160 的方向必须与尺寸同一口径"
        );
        let handle = handle.clone().expect("模板必须补上 handle 子节点");
        assert_eq!(bar.children.len(), 1, "只补一个滑块，不多建视觉节点");
        assert_eq!(bar.children[0].key, handle);
        assert_eq!(bar.children[0].layout.size, Some([12.0, 48.0]));

        let errors: Vec<String> = validate_spec(spec, &Capabilities::core())
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .map(|diagnostic| diagnostic.message.clone())
            .collect();
        assert!(errors.is_empty(), "模板建出的形状过不了校验：{errors:?}",);
    }

    /// 「导出为 UI 预设」：只带走这棵子树和它真正引用的样式/令牌/资产/绑定。
    #[test]
    fn export_prefab_keeps_the_subtree_and_its_references() {
        let root = temp_root("export-prefab");
        std::fs::create_dir_all(root.join("ui")).unwrap();
        let spec: UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "shop",
                          "canvas": {"reference": [960.0, 540.0]}},
            "tokens": {"color.card": "#101014", "color.unused": "#ff0000"},
            "styles": {
                "card": {"base": {"bg": "@color.card"}},
                "spare": {"base": {"bg": "@color.unused"}}
            },
            "root": {
                "key": "root", "type": "panel",
                "children": [
                    {"key": "buy", "type": "button", "text": "Buy", "style": "card",
                     "src": "asset:buy_art",
                     "children": [{"key": "buy_label", "type": "text", "text": "Buy",
                                   "style": "card"}]},
                    {"key": "other", "type": "panel", "style": "spare"}
                ]
            },
            "assets": [
                {"key": "buy_art", "role": "button", "path": "ui/buy.png"},
                {"key": "other_art", "role": "icon", "path": "ui/other.png"}
            ],
            "bindings": [
                {"target": "buy_label", "property": "text", "source": "lua:shop.buy"},
                {"target": "other", "property": "visible", "source": "lua:shop.other"}
            ]
        }))
        .unwrap();
        let encoded = crate::ui_spec_host::encode_spec(&spec).unwrap();
        std::fs::write(root.join("ui/shop.ui.json"), &encoded).unwrap();
        let source_revision = rustlua_ui_spec::revision_hash(&encoded);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();

        let accepted = studio
            .export_prefab("buy", "ui/prefabs/buy.ui.json")
            .unwrap();
        assert_eq!(accepted.document, "ui/prefabs/buy.ui.json");
        let prefab = accepted.spec;
        assert_eq!(prefab.document.kind, DocumentKind::Prefab);
        assert_eq!(prefab.document.name, "buy");
        assert_eq!(
            prefab.document.canvas.reference,
            [960.0, 540.0],
            "画布口径照搬源文档"
        );
        assert_eq!(prefab.root.key, "buy", "预设的根就是选中的子树");
        assert_eq!(prefab.walk_nodes().len(), 2, "预设只带这棵子树");
        assert_eq!(
            prefab.styles.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["card"]
        );
        assert_eq!(
            prefab.tokens.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["color.card"]
        );
        assert_eq!(
            prefab
                .assets
                .iter()
                .map(|asset| asset.key.as_str())
                .collect::<Vec<_>>(),
            vec!["buy_art"]
        );
        assert_eq!(
            prefab
                .bindings
                .iter()
                .map(|binding| binding.target.as_str())
                .collect::<Vec<_>>(),
            vec!["buy_label"]
        );
        assert!(
            root.join("ui/prefabs/buy.ui.json").is_file(),
            "导出必须真的落盘"
        );
        assert_eq!(studio.revision(), source_revision, "源文档不得被改动");

        let error = studio
            .export_prefab("buy", "ui/prefabs/buy.ui.json")
            .expect_err("同名目标必须拒绝");
        assert!(error.to_string().contains("已存在"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn multi_selection_aligns_and_distributes_in_one_proposal() {
        let (_root, mut studio) = open_alignment_studio("align-distribute");
        studio.select("a");
        assert!(
            studio.align_selection(AlignMode::Left).is_err(),
            "单节点对齐没有基准，必须拒绝"
        );
        studio.toggle_select("b");
        studio.toggle_select("c");
        assert_eq!(studio.selected_keys(), vec!["a", "b", "c"]);
        assert_eq!(studio.selection_count(), 3);

        let proposal = studio.align_selection(AlignMode::Left).unwrap();
        assert_eq!(
            studio.proposal_ops(&proposal.proposal_id).unwrap().len(),
            3,
            "对齐是一次提案里的三条 Replace，不能分批落盘"
        );
        studio.accept(&proposal.proposal_id).unwrap();
        for key in ["a", "b", "c"] {
            assert!(
                (rect_of(&studio, key)[0] - 10.0).abs() < 1e-3,
                "{key} 未左对齐"
            );
        }
        assert_eq!(
            studio.selected_keys(),
            vec!["a", "b", "c"],
            "接受后保留多选"
        );

        // 先把三项横向拉开，再验证等间距分布。
        let proposal = studio
            .set_layout_field("b", LayoutField::Position, "200, 30")
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let proposal = studio
            .set_layout_field("c", LayoutField::Position, "400, 60")
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let proposal = studio
            .distribute_selection(DistributeAxis::Horizontal)
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let a = rect_of(&studio, "a");
        let b = rect_of(&studio, "b");
        let c = rect_of(&studio, "c");
        assert!((a[0] - 10.0).abs() < 1e-3);
        assert!((c[0] - 400.0).abs() < 1e-3, "尾项不许被浮点误差推走");
        let gap = b[0] - (a[0] + a[2]);
        let tail_gap = c[0] - (b[0] + b[2]);
        assert!(
            (gap - tail_gap).abs() < 1e-3,
            "间距不等：{gap} vs {tail_gap}"
        );
    }

    #[test]
    fn multi_selection_drag_moves_every_selected_node() {
        let (_root, mut studio) = open_alignment_studio("multi-drag");
        studio.select("a");
        studio.toggle_select("b");
        let before_a = rect_of(&studio, "a");
        let before_b = rect_of(&studio, "b");
        let proposal = studio.move_selection_by([5.0, -4.0]).unwrap();
        assert_eq!(studio.proposal_ops(&proposal.proposal_id).unwrap().len(), 2);
        studio.accept(&proposal.proposal_id).unwrap();
        assert_close(
            rect_of(&studio, "a"),
            [
                before_a[0] + 5.0,
                before_a[1] - 4.0,
                before_a[2],
                before_a[3],
            ],
        );
        assert_close(
            rect_of(&studio, "b"),
            [
                before_b[0] + 5.0,
                before_b[1] - 4.0,
                before_b[2],
                before_b[3],
            ],
        );
        assert_close(rect_of(&studio, "c"), [200.0, 60.0, 30.0, 30.0]);
    }

    #[test]
    fn snapping_reports_the_guide_line_it_used() {
        let (_root, mut studio) = open_alignment_studio("snap");
        studio.select("b");
        // 纵向拖 5：移动后的边 35/45/55 离现有水平线（30/50/60…）都超过阈值。
        let (delta, guides) = studio.snap_delta([97.0, 5.0], 4.0);
        assert_eq!(guides.y, None, "竖直方向没有可吸的线时不能凭空画一条");
        assert!((delta[1] - 5.0).abs() < 1e-3, "纵向不该被改写");
        let guide = guides.x.expect("水平方向应吸到兄弟节点的边/中线");
        // b 的左边 100 + 97 = 197、中线 232、右边 267；最近的可吸线是 c 的右边 230。
        assert!((guide - 230.0).abs() < 1e-3, "吸附线 {guide}");
        let moved_left = 100.0 + delta[0];
        assert!(
            (moved_left + 70.0 * 0.5 - guide).abs() < 1e-3,
            "命中线必须真的落在移动后的边/中线上"
        );

        let (delta, guides) = studio.snap_delta([0.5, 0.5], 0.1);
        assert_eq!(delta, [0.5, 0.5], "超出阈值不许改位移");
        assert_eq!(guides, SnapGuides::default());
    }

    #[test]
    fn inspector_edits_and_hierarchy_ops_go_through_proposals() {
        let root = temp_root("edits");
        write_spec(&root);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();
        studio.select("buy");

        let proposal = studio
            .set_field(
                "buy",
                "/layout",
                serde_json::json!({"anchor": "top_left", "offsets": [10, 20, 30, 40]}),
            )
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        assert_eq!(
            studio
                .spec()
                .unwrap()
                .find_node("buy")
                .unwrap()
                .layout
                .offsets,
            Some([10.0, 20.0, 30.0, 40.0])
        );

        let proposal = studio.set_style("buy", Some("card")).unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        assert_eq!(
            studio
                .spec()
                .unwrap()
                .find_node("buy")
                .unwrap()
                .style
                .as_deref(),
            Some("card")
        );
        let proposal = studio.set_event("buy", "click", "shop.buy").unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        assert_eq!(
            studio
                .spec()
                .unwrap()
                .find_node("buy")
                .unwrap()
                .events
                .get("click")
                .map(String::as_str),
            Some("shop.buy")
        );

        let proposal = studio.duplicate_node("buy").unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        assert!(studio.spec().unwrap().find_node("buy_copy").is_some());
        assert_eq!(studio.hierarchy().len(), 4);
        assert_eq!(
            studio.parent_and_index("buy_copy"),
            Some(("root".to_owned(), 2))
        );
        assert!(!studio.accepts_children("buy"), "按钮是叶子");
        assert!(studio.accepts_children("root"));

        let error = studio
            .move_node("buy_copy", "title", None)
            .expect_err("叶子控件不能当父级");
        let message = error.to_string();
        assert!(
            message.contains("parent_leaf") || message.contains("叶子"),
            "{message}"
        );
        let proposal = studio.move_node("buy_copy", "root", Some(0)).unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        assert_eq!(studio.spec().unwrap().root.children[0].key, "buy_copy");

        let proposal = studio.wrap_node("buy_copy", "wrap").unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        assert_eq!(studio.spec().unwrap().root.children[0].key, "wrap");
        let wrap = studio.spec().unwrap().find_node("wrap").unwrap();
        assert_eq!(wrap.children.len(), 1);
        assert_eq!(wrap.children[0].key, "buy_copy");

        let proposal = studio.remove_node("buy_copy").unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        assert!(studio.spec().unwrap().find_node("buy_copy").is_none());
        assert!(studio.spec().unwrap().root.children[0].children.is_empty());
    }

    #[test]
    fn canvas_drag_writes_offsets_relative_to_the_parent() {
        let root = temp_root("canvas");
        write_spec(&root);
        let mut studio = UiStudio::new(&root);
        studio.open("ui/shop.ui.json").unwrap();
        studio.select("buy");
        let (_, before, _) = studio.selection_frame().expect("选中框");

        let proposal = studio.move_selection_by([5.0, -3.0]).unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let (_, moved, _) = studio.selection_frame().unwrap();
        assert_close(
            moved,
            [before[0] + 5.0, before[1] - 3.0, before[2], before[3]],
        );

        let proposal = studio
            .resize_selection(ResizeHandle::BottomRight, [20.0, 10.0])
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let (_, resized, _) = studio.selection_frame().unwrap();
        assert_close(
            resized,
            [moved[0], moved[1], moved[2] + 20.0, moved[3] + 10.0],
        );

        // 反向拖过头：夹到 8 设计像素而不是负尺寸。
        let proposal = studio
            .resize_selection(ResizeHandle::Left, [9999.0, 0.0])
            .unwrap();
        studio.accept(&proposal.proposal_id).unwrap();
        let (_, clamped, _) = studio.selection_frame().unwrap();
        assert_close(
            clamped,
            [resized[0] + resized[2] - 8.0, resized[1], 8.0, resized[3]],
        );
    }

    #[test]
    fn generated_candidate_can_be_reviewed_before_it_enters_assets() {
        let root = temp_root("assets");
        write_spec(&root);
        let provenance = rustlua_ui_spec::AssetProvenance {
            asset_id: "ui/button".to_owned(),
            source: rustlua_ui_spec::AssetSource::Generated,
            provider: Some("local".to_owned()),
            model: Some("checker".to_owned()),
            prompt_hash: Some("abc123".to_owned()),
            seed: Some(7),
            license: None,
            reviewed: false,
            used_by: vec!["ui/shop.ui.json".to_owned()],
            created_at: None,
        };
        std::fs::create_dir_all(root.join("ui/provenance")).unwrap();
        std::fs::write(
            root.join("ui/provenance/ui_button.json"),
            serde_json::to_vec_pretty(&provenance).unwrap(),
        )
        .unwrap();
        let studio = UiStudio::new(&root);

        let candidates = studio.asset_candidates();
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.key, "ui/button");
        assert!(!candidate.registered, "还没写进任何 .ui.json 的 assets[]");
        assert_eq!(
            candidate.path.as_deref(),
            Some("assets/generated/ui/ui_button.png")
        );
        assert_eq!(candidate.provenance.as_ref().map(|p| p.seed), Some(Some(7)));

        // 只写边车：reviewed=true + 许可；不要求先登记进 assets[]。
        let reviewed = studio
            .review_asset("ui/button", Some("CC0".to_owned()))
            .unwrap();
        assert!(reviewed.reviewed);
        assert_eq!(reviewed.license.as_deref(), Some("CC0"));
        assert_eq!(reviewed.used_by, vec!["ui/shop.ui.json".to_owned()]);
        assert_eq!(
            studio.asset_candidates()[0]
                .provenance
                .as_ref()
                .map(|p| p.reviewed),
            Some(true)
        );

        // 不重填许可时保留旧值，不能把已填的许可抹成 None。
        let again = studio.review_asset("ui/button", None).unwrap();
        assert_eq!(again.license.as_deref(), Some("CC0"));

        let missing = studio
            .review_asset("ui/missing", None)
            .expect_err("既无登记也无边车必须 NotFound");
        assert!(missing.is(crate::ui_spec_host::UiHostErrorCode::NotFound));
    }

    #[test]
    fn asset_io_stays_inside_the_project_and_slices_through_the_store() {
        let root = temp_root("asset-io");
        write_spec(&root);
        let studio = UiStudio::new(&root);
        let rgba = [255u8, 0, 0, 255];
        let png = rustlua_ui_spec::encode_rgba_png(16, 16, &rgba.repeat(16 * 16));
        std::fs::create_dir_all(root.join("assets/ui")).unwrap();
        std::fs::write(root.join("assets/ui/sheet.png"), &png).unwrap();

        assert_eq!(
            studio.read_asset_bytes("assets/ui/sheet.png").as_deref(),
            Some(png.as_slice())
        );
        assert!(studio.read_asset_bytes("../outside.png").is_none());
        assert!(studio.read_asset_bytes("").is_none());
        let absolute = root.join("assets/ui/sheet.png");
        assert!(
            studio
                .read_asset_bytes(absolute.to_string_lossy().as_ref())
                .is_none(),
            "绝对路径不得直读"
        );

        let manifest = studio
            .slice_asset(
                "assets/ui/sheet.png",
                vec![SliceRegion {
                    name: "buy".to_owned(),
                    rect: [0, 0, 8, 8],
                }],
                "assets/ui/sliced",
            )
            .unwrap();
        assert_eq!(manifest.source_width, 16);
        assert_eq!(manifest.slices.len(), 1);
        assert_eq!(manifest.slices[0].path, "assets/ui/sliced/buy.png");
        assert!(root.join("assets/ui/sliced/slices.json").is_file());
    }
}
