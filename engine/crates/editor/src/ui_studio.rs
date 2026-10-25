//! M76-04：UI Studio 的无头状态层。
//!
//! 画布/层级/检查器/预览/提案箱的**数据与动作**集中在这里，egui 面板只做绘制与转调。
//! 所有写入仍走 [`UiSpecStore`]：提案 → 接受/拒绝/拆分接受，UI Studio 不维护第二份
//! 文档状态。多分辨率预览与安全区只影响绘制参数，不写回 Spec。

use crate::ui_spec_host::{UiHostError, UiHostErrorCode, UiHostResult, UiSpecStore};
use rustlua_ui_spec::{
    ResizeHandle, StyleState, UiNode, UiNodeKind, UiPatch, UiPatchOp, UiProposal, UiSpec,
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

/// 检查器字段的写回通道：面板只按它生成提案，不自己拼 JSON Pointer。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectorEdit {
    None,
    /// 整段 `layout` 对象（面板里按 JSON 编辑）。
    Layout,
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
    selection: Option<String>,
    section: InspectorSection,
    resolution: PreviewResolution,
    safe_area: bool,
    undo: Vec<SnapshotEntry>,
    redo: Vec<SnapshotEntry>,
}

impl UiStudio {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            store: UiSpecStore::new(root),
            document: None,
            spec: None,
            revision: 0,
            selection: None,
            section: InspectorSection::Layout,
            resolution: PreviewResolution::default(),
            safe_area: false,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn open(&mut self, document: &str) -> UiHostResult<()> {
        let loaded = self.store.load(document)?;
        self.document = Some(loaded.document);
        self.revision = loaded.revision;
        self.selection = Some(loaded.spec.root.key.clone());
        self.spec = Some(loaded.spec);
        Ok(())
    }

    /// 重新读取当前文档（接受提案后刷新 revision 与内容）。
    pub fn reload(&mut self) -> UiHostResult<()> {
        let Some(document) = self.document.clone() else {
            return Ok(());
        };
        // 接受提案后保留选中；节点被删掉了才退回根节点。
        let previous = self.selection.clone();
        self.open(&document)?;
        if let Some(key) = previous {
            self.select(&key);
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
        }
        found
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
            InspectorRow {
                section: InspectorSection::Layout,
                field: "layout".to_owned(),
                value: serde_json::to_string(&node.layout).unwrap_or_default(),
                read_only: false,
                edit: InspectorEdit::Layout,
            },
        ];
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
        let (spec, key) = self.spec.as_ref().zip(self.selection.as_deref())?;
        let rows = self.layout_rects();
        let selected = rows.iter().find(|row| row.key == key)?;
        let parent_rect = selected
            .parent
            .as_ref()
            .and_then(|parent| rows.iter().find(|row| &row.key == parent))
            .map_or(
                {
                    let reference = spec.document.canvas.reference;
                    [0.0, 0.0, reference[0], reference[1]]
                },
                |row| row.rect,
            );
        Some((selected.key.clone(), selected.rect, parent_rect))
    }

    /// 画布拖动：把选中节点按设计像素平移（改 `offsets`，绝对定位）。
    pub fn move_selection_by(&self, delta: [f32; 2]) -> UiHostResult<UiProposal> {
        let (key, rect, _) = self
            .selection_frame()
            .ok_or_else(|| invalid_argument("没有选中节点"))?;
        self.set_absolute_rect(
            &key,
            [rect[0] + delta[0], rect[1] + delta[1], rect[2], rect[3]],
        )
    }

    /// 画布手柄：按 `handle` 拖动 `delta` 改变矩形，转成 offsets 提案。
    pub fn resize_selection(
        &self,
        handle: ResizeHandle,
        delta: [f32; 2],
    ) -> UiHostResult<UiProposal> {
        let (key, rect, _) = self
            .selection_frame()
            .ok_or_else(|| invalid_argument("没有选中节点"))?;
        let next = resize_rect(rect, handle, delta, MIN_CANVAS_SIZE);
        self.set_absolute_rect(&key, next)
    }

    fn set_absolute_rect(&self, key: &str, rect: [f32; 4]) -> UiHostResult<UiProposal> {
        let (_, _, parent) = self
            .selection_frame()
            .ok_or_else(|| invalid_argument("没有选中节点"))?;
        let mut layout = self
            .require_spec()?
            .find_node(key)
            .map(|node| node.layout.clone())
            .unwrap_or_default();
        layout.offsets = Some(offsets_for_rect(parent, rect));
        // offsets 与 size/position 互斥（`spec.layout_conflict`），画布拖拽是绝对定位。
        layout.size = None;
        layout.position = None;
        self.set_field(
            key,
            "/layout",
            serde_json::to_value(layout).map_err(|error| invalid_argument(error.to_string()))?,
        )
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
        self.reload()?;
        Ok(accepted)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
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
    use rustlua_ui_spec::{SliceRegion, UiPatchOp};

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
