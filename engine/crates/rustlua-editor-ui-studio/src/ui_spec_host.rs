//! M76-03：UI 文档（`.ui.json`）宿主。
//!
//! AI 只产出 [`UiSpec`] / [`UiPatch`]；本模块负责**唯一**的落盘口径：
//! 读取时 revision = 文件内容哈希，写入前必须校验能力矩阵，写动作先成为提案，
//! 人工接受后才原子落盘。任何失败都不产生部分修改。
//!
//! 这里刻意不依赖 `EditorApp` / egui：编辑器 shell、MCP 适配层与测试共用同一份实现。

use rustlua_ui_spec::{
    AssetEntry, AssetLimits, AssetProvenance, AssetReviewReport, AssetRole, AssetSource,
    Capabilities, Diagnostic, ProposalCost, ProposalDiff, ProposalKind, Severity, SliceRegion,
    UiAssetPlan, UiNodeKind, UiPatch, UiProposal, UiSpec, apply_patch, compile, content_hash,
    diff_specs, encode_rgba_png, render_preview, review_assets, revision_hash, slice_manual,
    slice_rgba, validate_spec,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rustlua_assets_import::{atlas_plan, decode_png_rgba};

pub const PROPOSAL_DIR: &str = ".cache/ui_proposals";
/// 资产来源边车**必须入库**：放进 `.cache` 会被 gitignore，干净克隆上的生成资产
/// 就只剩计划没有来源，发布门会直接红。提案目录才是临时态。
pub const ASSET_REGISTRY_DIR: &str = "ui/provenance";

/// 当前运行时真实支持的能力面（与 M76-02 对齐）。
pub fn capabilities() -> Capabilities {
    Capabilities::core()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiHostErrorCode {
    InvalidArgument,
    StaleRevision,
    InvalidSpec,
    Capability,
    NotFound,
    ConfirmationRequired,
    BudgetExceeded,
    Unavailable,
    Io,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiHostError {
    pub code: UiHostErrorCode,
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl UiHostError {
    pub fn new(code: UiHostErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            diagnostics: Vec::new(),
        }
    }

    pub fn with_diagnostics(mut self, diagnostics: Vec<Diagnostic>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    pub fn is(&self, code: UiHostErrorCode) -> bool {
        self.code == code
    }
}

impl std::fmt::Display for UiHostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for UiHostError {}

pub type UiHostResult<T> = Result<T, UiHostError>;

/// `asset_details` 的四元读数：来源边车、单件字节、是否缺文件、图集页成本。
type AssetDetails = (
    Option<AssetProvenance>,
    Option<u64>,
    bool,
    Option<(String, u64)>,
);

fn io_error(path: &Path, error: std::io::Error) -> UiHostError {
    let code = if error.kind() == std::io::ErrorKind::NotFound {
        UiHostErrorCode::NotFound
    } else {
        UiHostErrorCode::Io
    };
    UiHostError::new(code, format!("{} 失败：{error}", path.display()))
}

fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

fn capability_diagnostic(diagnostic: &Diagnostic) -> bool {
    matches!(
        diagnostic.code.as_str(),
        "spec.node_unsupported"
            | "spec.event_unsupported"
            | "spec.state_unsupported"
            | "spec.binding_unsupported"
            | "spec.clipping_unsupported"
            | "spec.virtual_list_unsupported"
            | "spec.rich_text_unsupported"
            | "spec.transition_unsupported"
    )
}

fn spec_error(diagnostics: Vec<Diagnostic>) -> UiHostError {
    let code = if diagnostics.iter().any(capability_diagnostic) {
        UiHostErrorCode::Capability
    } else {
        UiHostErrorCode::InvalidSpec
    };
    let message = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
        .collect::<Vec<_>>()
        .join("；");
    let message = if message.is_empty() {
        "Spec 校验失败".to_owned()
    } else {
        message
    };
    UiHostError::new(code, message).with_diagnostics(diagnostics)
}

/// 已落盘的 UI 文档 + 内容哈希 revision。
#[derive(Debug, Clone, PartialEq)]
pub struct UiSpecDocument {
    pub document: String,
    pub revision: u64,
    pub spec: UiSpec,
}

/// 提案的持久化形状：接受时不再信任外部输入，直接用落盘时算好的完整 Spec。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredProposal {
    pub proposal: UiProposal,
    pub document: String,
    pub base_revision: u64,
    pub new_spec: UiSpec,
    /// 补丁提案的原始操作，供提案箱做 op 级勾选拆分接受；整份 Spec 提案没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<UiPatch>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AcceptedUi {
    pub document: String,
    pub revision: u64,
    pub spec: UiSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiAssetListing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<String>,
    pub key: String,
    pub role: AssetRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<AssetProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// 切图产物清单里的一项。`path` 是工程内相对路径，`rect` 是源图上的像素区域。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlicedAsset {
    pub name: String,
    pub path: String,
    pub rect: [u32; 4],
    pub bytes: u64,
}

/// 一次切图落盘的完整读数：源图尺寸 + 每个区域写出的文件。
///
/// 清单最后写，所以「清单存在」蕴含「清单里的文件都已落盘」；中途失败会清掉本次
/// 新建的文件，不留下半套产物。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceManifest {
    pub source: String,
    pub source_width: u32,
    pub source_height: u32,
    pub output_dir: String,
    pub slices: Vec<SlicedAsset>,
}

/// 工程内的 UI 文档仓库。`root` 是工程根目录；所有相对路径都限制在它里面。
#[derive(Debug, Clone)]
pub struct UiSpecStore {
    root: PathBuf,
    capabilities: Capabilities,
}

impl UiSpecStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            capabilities: capabilities(),
        }
    }

    pub fn with_capabilities(root: impl Into<PathBuf>, capabilities: Capabilities) -> Self {
        Self {
            root: root.into(),
            capabilities,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// 只接受工程内、以 `.ui.json` 结尾的相对路径。
    pub fn resolve(&self, document: &str) -> UiHostResult<PathBuf> {
        let path = Path::new(document);
        if document.trim().is_empty() {
            return Err(UiHostError::new(
                UiHostErrorCode::InvalidArgument,
                "document 不能为空",
            ));
        }
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(UiHostError::new(
                UiHostErrorCode::InvalidArgument,
                format!("只接受工程内相对路径（不得为绝对路径或含 ..）：{document}"),
            ));
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_none_or(|name| !name.ends_with(".ui.json"))
        {
            return Err(UiHostError::new(
                UiHostErrorCode::InvalidArgument,
                format!("只接受 .ui.json 文档：{document}"),
            ));
        }
        Ok(self.root.join(path))
    }

    pub fn load(&self, document: &str) -> UiHostResult<UiSpecDocument> {
        let path = self.resolve(document)?;
        let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
        let spec: UiSpec = serde_json::from_slice(&bytes).map_err(|error| {
            UiHostError::new(
                UiHostErrorCode::InvalidSpec,
                format!("解析 {} 失败：{error}", path.display()),
            )
        })?;
        Ok(UiSpecDocument {
            document: normalize_document(document),
            revision: revision_hash(&bytes),
            spec,
        })
    }

    /// 读取文档原始字节：撤销快照必须保留原字节，重编码会让 revision 漂移。
    pub fn snapshot(&self, document: &str) -> UiHostResult<Vec<u8>> {
        let path = self.resolve(document)?;
        fs::read(&path).map_err(|error| io_error(&path, error))
    }

    /// 把快照写回文档（接受提案后的一次 Undo）。仍走解析 + 校验，失败不改工作区。
    pub fn restore(&self, document: &str, bytes: &[u8]) -> UiHostResult<AcceptedUi> {
        let spec: UiSpec = serde_json::from_slice(bytes).map_err(|error| {
            UiHostError::new(
                UiHostErrorCode::InvalidSpec,
                format!("撤销快照解析失败：{error}"),
            )
        })?;
        self.validate(&spec)?;
        let path = self.resolve(document)?;
        write_atomic(&path, bytes)?;
        Ok(AcceptedUi {
            document: normalize_document(document),
            revision: revision_hash(bytes),
            spec,
        })
    }

    pub fn validate(&self, spec: &UiSpec) -> UiHostResult<Vec<Diagnostic>> {
        let diagnostics = validate_spec(spec, &self.capabilities);
        if has_errors(&diagnostics) {
            return Err(spec_error(diagnostics));
        }
        Ok(diagnostics)
    }

    pub fn preview(
        &self,
        spec: &UiSpec,
        width: u32,
        height: u32,
        scale: f32,
    ) -> UiHostResult<Vec<u8>> {
        render_preview(spec, width, height, scale)
            .map_err(|message| UiHostError::new(UiHostErrorCode::InvalidArgument, message))
    }

    /// 整文档替换：只生成提案，不改工作区。新文档允许 `base_revision = 0`。
    pub fn submit_spec(
        &self,
        document: &str,
        base_revision: u64,
        spec: UiSpec,
    ) -> UiHostResult<UiProposal> {
        let path = self.resolve(document)?;
        let current = if path.is_file() {
            Some(self.load(document)?)
        } else {
            None
        };
        let current_revision = current.as_ref().map_or(0, |document| document.revision);
        if base_revision != current_revision {
            return Err(UiHostError::new(
                UiHostErrorCode::StaleRevision,
                format!(
                    "base_revision 过期：请求 {base_revision}，当前 {current_revision}；请重新 ui_export_spec"
                ),
            ));
        }
        self.validate(&spec)?;
        compile(&spec, &self.capabilities).map_err(spec_error)?;

        let diff = current.as_ref().map_or_else(
            || ProposalDiff {
                nodes_added: spec.walk_nodes().len() as u32,
                assets_added: spec.assets.len() as u32,
                ..ProposalDiff::default()
            },
            |current| diff_specs(&current.spec, &spec).to_proposal_diff(),
        );
        self.store_proposal(
            document,
            base_revision,
            spec,
            ProposalKind::Spec,
            diff,
            None,
        )
    }

    /// 增量补丁：`patch.base_revision` 必须等于当前内容哈希，否则整批拒绝。
    pub fn submit_patch(&self, document: &str, patch: UiPatch) -> UiHostResult<UiProposal> {
        let current = self.load(document)?;
        if patch.base_revision != current.revision {
            return Err(UiHostError::new(
                UiHostErrorCode::StaleRevision,
                format!(
                    "base_revision 过期：补丁基于 {}，当前 {}；请重新 ui_export_spec",
                    patch.base_revision, current.revision
                ),
            ));
        }
        let next = apply_patch(&current.spec, &patch, &self.capabilities).map_err(spec_error)?;
        self.validate(&next)?;
        let diff = diff_specs(&current.spec, &next).to_proposal_diff();
        self.store_proposal(
            document,
            patch.base_revision,
            next,
            ProposalKind::Patch,
            diff,
            Some(patch),
        )
    }

    /// 拆分接受：只把 `indices` 选中的操作落成提案。选中集合为空时返回空补丁的
    /// 提案（no-op）；越界索引按参数错误拒绝。与整体接受共用同一套 revision 与
    /// 能力校验，被排除的操作不会进入落盘内容。
    pub fn submit_patch_subset(
        &self,
        document: &str,
        patch: UiPatch,
        indices: &[usize],
    ) -> UiHostResult<UiProposal> {
        let selected = rustlua_ui_spec::select_ops(&patch, indices)
            .map_err(|message| UiHostError::new(UiHostErrorCode::InvalidArgument, message))?;
        self.submit_patch(document, selected)
    }

    fn store_proposal(
        &self,
        document: &str,
        base_revision: u64,
        new_spec: UiSpec,
        kind: ProposalKind,
        diff: ProposalDiff,
        patch: Option<UiPatch>,
    ) -> UiHostResult<UiProposal> {
        let document = normalize_document(document);
        let encoded = encode_spec(&new_spec)?;
        let proposal_id = proposal_id(&document, base_revision, &encoded);
        let proposal = UiProposal {
            proposal_id: proposal_id.clone(),
            kind,
            base_revision,
            diagnostics: Vec::new(),
            diff,
            preview_png: None,
            cost: ProposalCost::default(),
            expires_at: None,
        };
        let stored = StoredProposal {
            proposal: proposal.clone(),
            document,
            base_revision,
            new_spec,
            patch,
        };
        let path = self.proposal_path(&proposal_id)?;
        write_atomic(
            &path,
            &serde_json::to_vec_pretty(&stored).map_err(|error| {
                UiHostError::new(UiHostErrorCode::Io, format!("序列化提案失败：{error}"))
            })?,
        )?;
        Ok(proposal)
    }

    pub fn list_proposals(&self) -> UiHostResult<Vec<StoredProposal>> {
        let dir = self.root.join(PROPOSAL_DIR);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_error(&dir, error)),
        };
        let mut proposals = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&dir, error))?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            match serde_json::from_slice::<StoredProposal>(&bytes) {
                Ok(stored) => proposals.push(stored),
                Err(_) => continue,
            }
        }
        proposals.sort_by(|left, right| left.proposal.proposal_id.cmp(&right.proposal.proposal_id));
        Ok(proposals)
    }

    /// 读单个提案的持久化形状（提案箱做 op 级勾选时要用原始补丁）。
    pub fn stored_proposal(&self, proposal_id: &str) -> UiHostResult<StoredProposal> {
        let path = self.proposal_path(proposal_id)?;
        let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
        serde_json::from_slice(&bytes).map_err(|error| {
            UiHostError::new(
                UiHostErrorCode::InvalidArgument,
                format!("提案 {} 解析失败：{error}", path.display()),
            )
        })
    }

    /// 补丁提案的原始操作；整份 Spec 提案没有 op，返回空列表。
    pub fn proposal_ops(&self, proposal_id: &str) -> UiHostResult<Vec<rustlua_ui_spec::UiPatchOp>> {
        Ok(self
            .stored_proposal(proposal_id)?
            .patch
            .map(|patch| patch.ops)
            .unwrap_or_default())
    }

    /// 接受提案：复核 revision → 再校验 → 原子写入文档 → 删除提案。
    /// 任何一步失败都不会改动工作区。
    pub fn accept(&self, proposal_id: &str) -> UiHostResult<AcceptedUi> {
        let path = self.proposal_path(proposal_id)?;
        let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
        let stored: StoredProposal = serde_json::from_slice(&bytes).map_err(|error| {
            UiHostError::new(
                UiHostErrorCode::InvalidArgument,
                format!("提案 {} 解析失败：{error}", path.display()),
            )
        })?;
        let document_path = self.resolve(&stored.document)?;
        // 新文档提案（base_revision=0 且不是补丁）允许文件尚不存在；文件一旦出现，
        // 哈希就必须仍为 0 才能落盘，避免把基于空文档的提案覆盖到别人的文件上。
        let current_revision = match fs::read(&document_path) {
            Ok(bytes) => revision_hash(&bytes),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && stored.base_revision == 0
                    && stored.patch.is_none() =>
            {
                0
            }
            Err(error) => return Err(io_error(&document_path, error)),
        };
        if current_revision != stored.base_revision {
            return Err(UiHostError::new(
                UiHostErrorCode::StaleRevision,
                format!(
                    "提案已过期：基于 {}，当前 {}；请重新生成提案",
                    stored.base_revision, current_revision
                ),
            ));
        }
        self.validate(&stored.new_spec)?;
        let encoded = encode_spec(&stored.new_spec)?;
        write_atomic(&document_path, &encoded)?;
        let _ = fs::remove_file(&path);
        Ok(AcceptedUi {
            document: stored.document,
            revision: revision_hash(&encoded),
            spec: stored.new_spec,
        })
    }

    /// 拒绝提案只删提案文件，不碰文档。
    pub fn reject(&self, proposal_id: &str) -> UiHostResult<()> {
        let path = self.proposal_path(proposal_id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(&path, error)),
        }
    }

    fn proposal_path(&self, proposal_id: &str) -> UiHostResult<PathBuf> {
        if proposal_id.is_empty()
            || !proposal_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            return Err(UiHostError::new(
                UiHostErrorCode::InvalidArgument,
                format!("非法 proposal_id：{proposal_id}"),
            ));
        }
        Ok(self
            .root
            .join(PROPOSAL_DIR)
            .join(format!("{proposal_id}.json")))
    }

    /// 列出资产：给定文档只读那一份，否则扫描工程内所有 `.ui.json`。
    /// 跳过 VCS/构建目录，避免把缓存里的旧文档当作事实源。
    pub fn list_assets(&self, document: Option<&str>) -> UiHostResult<Vec<UiAssetListing>> {
        let documents = match document {
            Some(document) => vec![self.load(document)?],
            None => self.scan_documents()?,
        };
        let mut listings = Vec::new();
        for document in documents {
            for asset in &document.spec.assets {
                let (provenance, bytes, _, _) = self.asset_details(asset, &Default::default())?;
                listings.push(UiAssetListing {
                    document: Some(document.document.clone()),
                    key: asset.key.clone(),
                    role: asset.role,
                    path: asset.path.clone(),
                    provenance,
                    bytes,
                });
            }
        }
        listings.sort_by(|left, right| {
            left.key
                .cmp(&right.key)
                .then_with(|| left.document.cmp(&right.document))
        });
        listings.dedup_by(|left, right| left.key == right.key && left.document == right.document);
        Ok(listings)
    }

    /// 发布前的资产审查（M76-05）：来源可追溯、未审阅不得被引用、单件/图集/总包体积。
    ///
    /// 跨文档声明同一 key 是合法的（共享资产）；这里按 key 合并引用与字节，只审一次。
    pub fn review_assets(
        &self,
        document: Option<&str>,
        limits: &AssetLimits,
    ) -> UiHostResult<AssetReviewReport> {
        let documents = match document {
            Some(document) => vec![self.load(document)?],
            None => self.scan_documents()?,
        };
        let mut entries: std::collections::BTreeMap<String, AssetEntry> =
            std::collections::BTreeMap::new();
        let mut missing_files = std::collections::BTreeSet::new();
        // 图集区域引用没有独立文件，体积按**页**算；装箱一次、整批共享。
        let atlas_costs = self.atlas_page_costs();
        for document in documents {
            for asset in &document.spec.assets {
                let (mut provenance, bytes, file_missing, atlas_page) =
                    self.asset_details(asset, &atlas_costs)?;
                if file_missing {
                    missing_files.insert(asset.key.clone());
                }
                let references = asset_references(&document.spec, &asset.key);
                if let Some(provenance) = provenance.as_mut() {
                    for key in references {
                        if !provenance.used_by.contains(&key) {
                            provenance.used_by.push(key);
                        }
                    }
                }
                match entries.get_mut(&asset.key) {
                    Some(existing) => {
                        if existing.provenance.is_none() {
                            existing.provenance = provenance;
                        } else if let (Some(existing), Some(incoming)) =
                            (existing.provenance.as_mut(), provenance)
                        {
                            for key in incoming.used_by {
                                if !existing.used_by.contains(&key) {
                                    existing.used_by.push(key);
                                }
                            }
                        }
                        if existing.size_bytes.is_none() {
                            existing.size_bytes = bytes;
                        }
                        if existing.atlas_page.is_none() {
                            existing.atlas_page = atlas_page;
                        }
                    }
                    None => {
                        entries.insert(
                            asset.key.clone(),
                            AssetEntry {
                                plan: asset.clone(),
                                provenance,
                                size_bytes: bytes,
                                atlas_page,
                            },
                        );
                    }
                }
            }
        }
        let entries: Vec<AssetEntry> = entries.into_values().collect();
        let mut report = review_assets(&entries, limits);
        for key in missing_files {
            report.diagnostics.push(
                Diagnostic::error(
                    "asset.file_missing",
                    format!("asset_plan.{key}"),
                    "声明的资产文件不存在：发布包会缺图",
                )
                .with_fix("重新出图/切图，或修正 assets[].path"),
            );
        }
        Ok(report)
    }

    /// 工程内全部 `.ui.json` 的相对路径（稳定排序，跳过 VCS/构建目录）。
    ///
    /// 发布门要逐个加载而不是像 [`Self::list_assets`] 那样静默跳过坏文档，
    /// 所以这里只给路径，解析失败由调用方显式报错。
    pub fn document_paths(&self) -> UiHostResult<Vec<String>> {
        let mut found = Vec::new();
        self.scan_dir(&self.root, 0, &mut found)?;
        found.sort();
        Ok(found)
    }

    fn scan_documents(&self) -> UiHostResult<Vec<UiSpecDocument>> {
        let mut found = Vec::new();
        for document in self.document_paths()? {
            if let Ok(document) = self.load(&document) {
                found.push(document);
            }
        }
        Ok(found)
    }

    fn scan_dir(&self, dir: &Path, depth: usize, found: &mut Vec<String>) -> UiHostResult<()> {
        if depth > 8 {
            return Ok(());
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error(dir, error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| io_error(dir, error))?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || name == "builds"
            {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                self.scan_dir(&path, depth + 1, found)?;
            } else if name.ends_with(".ui.json") {
                let relative = path
                    .strip_prefix(&self.root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                found.push(relative);
            }
        }
        Ok(())
    }

    /// 资产的实际位置：显式 `path` 优先；生成计划按出图落盘约定
    /// `assets/generated/ui/<key>.png`；图集区域引用（`x.atlas#region`）查图集本体。
    fn asset_file_path(&self, asset: &UiAssetPlan) -> Option<PathBuf> {
        let declared = asset.path.clone().or_else(|| {
            asset
                .generate
                .as_ref()
                .map(|_| format!("assets/generated/ui/{}.png", sanitize_asset_key(&asset.key)))
        })?;
        let file = declared.split('#').next().unwrap_or(&declared);
        if file.trim().is_empty() {
            return None;
        }
        Some(self.root.join(file))
    }

    /// 图集区域 → (页 id, 页字节)。按 `atlas.json` 真实装箱一次；装不出来就返回空表，
    /// 让区域引用退化成"没有体积数据"而不是随意估一个数。
    fn atlas_page_costs(&self) -> std::collections::BTreeMap<(String, String), (String, u64)> {
        use atlas_plan::AtlasFormat;
        let mut costs = std::collections::BTreeMap::new();
        let Ok(Some(config)) =
            atlas_plan::AtlasConfig::load(&self.root.join(atlas_plan::AtlasConfig::FILE))
        else {
            return costs;
        };
        let Ok(files) = atlas_plan::collect_png_files(&self.root, "assets") else {
            return costs;
        };
        let excluded = atlas_plan::collect_excluded(&self.root).unwrap_or_default();
        let Ok(set) = atlas_plan::plan_pages(&config, &files, &excluded) else {
            return costs;
        };
        for page in &set.pages {
            // `auto` 落盘取两种编码里更小的那一个；这里按同一口径量（不估 zstd，只比编码字节）。
            let Ok(png) = atlas_plan::encode_page_png(page) else {
                continue;
            };
            let bytes = match page.format {
                AtlasFormat::Png => png.len(),
                AtlasFormat::Rltex => atlas_plan::encode_page_rltex(page)
                    .map(|raw| raw.len())
                    .unwrap_or(png.len()),
                AtlasFormat::Auto => atlas_plan::encode_page_rltex(page)
                    .map(|raw| raw.len().min(png.len()))
                    .unwrap_or(png.len()),
            } as u64;
            // 页 id 必须与 cook 写出的文件名一致（sanitize 过的 `<spec>-<index>`），
            // 否则 `assets/atlases/<id>.atlas#region` 永远查不到页字节。
            let id = format!("{}-{}", atlas_plan::sanitize(&page.spec_name), page.index);
            for region in page.by_name.keys() {
                costs.insert((id.clone(), region.clone()), (id.clone(), bytes));
            }
        }
        costs
    }

    fn asset_details(
        &self,
        asset: &UiAssetPlan,
        atlas_costs: &std::collections::BTreeMap<(String, String), (String, u64)>,
    ) -> UiHostResult<AssetDetails> {
        let provenance = self.read_provenance(&asset.key);
        // 图集区域引用（`x.atlas#region`）：体积归到所属页，单件门不重复计。
        let atlas_page = asset.path.as_deref().and_then(|path| {
            let (file, region) = path.split_once('#')?;
            let name = Path::new(file).file_stem()?.to_str()?;
            atlas_costs
                .get(&(name.to_owned(), region.to_owned()))
                .cloned()
        });
        // 区域引用没有独立文件：查不到页成本时也不能把 `.atlas` 本体算成单件体积。
        let is_region = asset.path.as_deref().is_some_and(|path| path.contains('#'));
        let Some(file) = self.asset_file_path(asset) else {
            return Ok((provenance, None, false, atlas_page));
        };
        match fs::metadata(&file) {
            Ok(metadata) => {
                let bytes = (atlas_page.is_none() && !is_region).then_some(metadata.len());
                Ok((provenance, bytes, false, atlas_page))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok((provenance, None, true, atlas_page))
            }
            Err(_) => Ok((provenance, None, true, atlas_page)),
        }
    }

    fn read_provenance(&self, asset_id: &str) -> Option<AssetProvenance> {
        let path = self.asset_registry_path(asset_id).ok()?;
        let bytes = fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn asset_registry_path(&self, asset_id: &str) -> UiHostResult<PathBuf> {
        if asset_id.trim().is_empty() {
            return Err(UiHostError::new(
                UiHostErrorCode::InvalidArgument,
                "asset_id 不能为空",
            ));
        }
        let safe: String = asset_id
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        Ok(self
            .root
            .join(ASSET_REGISTRY_DIR)
            .join(format!("{safe}.json")))
    }

    /// 人工审阅后的资产提升：写 provenance 边车文件；未审阅一律拒绝。
    /// 生成资产（M76-05）必须在已有 provenance 上补充许可，不允许降级成 human。
    pub fn promote_asset(
        &self,
        asset_id: &str,
        reviewed: bool,
        license: Option<String>,
        used_by: Vec<String>,
    ) -> UiHostResult<AssetProvenance> {
        if !reviewed {
            return Err(UiHostError::new(
                UiHostErrorCode::ConfirmationRequired,
                "资产尚未人工审阅（reviewed=false），不得进入发布包",
            ));
        }
        let listings = self.list_assets(None)?;
        let existing = self.read_provenance(asset_id);
        // 生成候选在写进 `assets[]` 之前就必须能审阅，否则「未审阅不得引用」没有入口。
        if !listings.iter().any(|listing| listing.key == asset_id) && existing.is_none() {
            return Err(UiHostError::new(
                UiHostErrorCode::NotFound,
                format!("工程内没有登记该资产：{asset_id}"),
            ));
        }
        let provenance = AssetProvenance {
            asset_id: asset_id.to_owned(),
            source: existing
                .as_ref()
                .map_or(AssetSource::Human, |provenance| provenance.source),
            provider: existing.as_ref().and_then(|value| value.provider.clone()),
            model: existing.as_ref().and_then(|value| value.model.clone()),
            prompt_hash: existing
                .as_ref()
                .and_then(|value| value.prompt_hash.clone()),
            seed: existing.as_ref().and_then(|value| value.seed),
            license,
            reviewed,
            used_by,
            created_at: existing
                .and_then(|value| value.created_at)
                .or_else(|| Some(rfc3339_now())),
        };
        let diagnostics = provenance.validate();
        if provenance.has_errors() {
            return Err(spec_error(diagnostics));
        }
        let path = self.asset_registry_path(asset_id)?;
        let bytes = serde_json::to_vec_pretty(&provenance).map_err(|error| {
            UiHostError::new(
                UiHostErrorCode::Io,
                format!("序列化 provenance 失败：{error}"),
            )
        })?;
        write_atomic(&path, &bytes)?;
        Ok(provenance)
    }

    /// 全部来源边车（含尚未写进 `assets[]` 的生成候选），按 asset_id 稳定排序。
    ///
    /// 坏 JSON 静默跳过：这里是面板列表，发布门走 [`Self::review_assets`] 会显式报错。
    pub fn list_provenance(&self) -> Vec<AssetProvenance> {
        let Ok(entries) = fs::read_dir(self.root.join(ASSET_REGISTRY_DIR)) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = fs::read(entry.path()) else {
                continue;
            };
            if let Ok(provenance) = serde_json::from_slice::<AssetProvenance>(&bytes) {
                out.push(provenance);
            }
        }
        out.sort_by(|left, right| left.asset_id.cmp(&right.asset_id));
        out
    }

    /// 生图产物落盘。只接受工程内 `.png` 相对路径，父目录自动创建。
    pub fn write_asset(&self, path: &str, bytes: &[u8]) -> UiHostResult<PathBuf> {
        let relative = Path::new(path);
        if path.trim().is_empty()
            || relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
            || !path.ends_with(".png")
        {
            return Err(UiHostError::new(
                UiHostErrorCode::InvalidArgument,
                format!("生成资产路径必须是工程内 .png 相对路径：{path}"),
            ));
        }
        let target = self.root.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        write_atomic(&target, bytes)?;
        Ok(target)
    }

    /// 切图落盘：读源 PNG → 解码 → 按区域裁像素 → 逐张写 PNG + `slices.json`。
    ///
    /// 边界、命名与像素拷贝都由 `rustlua-ui-spec` 的纯函数决定，宿主只做 IO：同一张图
    /// 同一组区域重跑得到同样的文件与字节。清单最后写，中途失败会清掉本次新建的文件，
    /// 不留「清单说有、磁盘没有」的半套产物。
    pub fn slice_asset(
        &self,
        source: &str,
        regions: Vec<SliceRegion>,
        output_dir: &str,
    ) -> UiHostResult<SliceManifest> {
        let source_relative = validate_relative_path(source, ".png", "切图源")?;
        let output_relative = validate_relative_path(output_dir, "", "输出目录")?;
        if regions.is_empty() {
            return Err(UiHostError::new(
                UiHostErrorCode::InvalidArgument,
                "切图区域不能为空",
            ));
        }
        let source_path = self.root.join(&source_relative);
        let bytes = fs::read(&source_path).map_err(|error| io_error(&source_path, error))?;
        let (width, height, rgba) = decode_png_rgba(&bytes)
            .map_err(|message| UiHostError::new(UiHostErrorCode::InvalidArgument, message))?;
        let regions = slice_manual(width, height, regions).map_err(|error| {
            UiHostError::new(UiHostErrorCode::InvalidArgument, error.to_string())
        })?;
        let mut files = BTreeSet::new();
        for region in &regions {
            let file = sanitize_asset_key(&region.name);
            if !files.insert(file.clone()) {
                return Err(UiHostError::new(
                    UiHostErrorCode::InvalidArgument,
                    format!(
                        "切图区域名字清洗后重复：{}（{file}.png 会互相覆盖）",
                        region.name
                    ),
                ));
            }
        }
        let crops = slice_rgba(width, height, &rgba, &regions).map_err(|error| {
            UiHostError::new(UiHostErrorCode::InvalidArgument, error.to_string())
        })?;

        let mut slices = Vec::with_capacity(regions.len());
        let mut encoded = Vec::with_capacity(regions.len());
        for (region, crop) in regions.iter().zip(crops) {
            let [_, _, crop_width, crop_height] = region.rect;
            let png = encode_rgba_png(crop_width, crop_height, &crop);
            let path = format!("{output_relative}/{}.png", sanitize_asset_key(&region.name));
            slices.push(SlicedAsset {
                name: region.name.clone(),
                path: path.clone(),
                rect: region.rect,
                bytes: png.len() as u64,
            });
            encoded.push((path, png));
        }
        let manifest = SliceManifest {
            source: source_relative,
            source_width: width,
            source_height: height,
            output_dir: output_relative.clone(),
            slices,
        };
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
            UiHostError::new(UiHostErrorCode::Io, format!("序列化切图清单失败：{error}"))
        })?;
        manifest_bytes.push(b'\n');

        let manifest_path = self.root.join(format!("{output_relative}/slices.json"));
        let mut written: Vec<PathBuf> = Vec::new();
        let result = (|| -> UiHostResult<()> {
            for (path, png) in &encoded {
                written.push(self.write_asset(path, png)?);
            }
            write_atomic(&manifest_path, &manifest_bytes)
        })();
        if let Err(error) = result {
            for target in written {
                let _ = fs::remove_file(target);
            }
            return Err(error);
        }
        Ok(manifest)
    }

    /// 写入来源边车（生成资产默认 `reviewed=false`）。有 error 级诊断时拒绝落盘，
    /// 避免把不可追溯的资产登记成事实。
    pub fn register_provenance(&self, provenance: &AssetProvenance) -> UiHostResult<()> {
        let diagnostics = provenance.validate();
        if provenance.has_errors() {
            return Err(spec_error(diagnostics));
        }
        let path = self.asset_registry_path(&provenance.asset_id)?;
        let bytes = serde_json::to_vec_pretty(provenance).map_err(|error| {
            UiHostError::new(
                UiHostErrorCode::Io,
                format!("序列化 provenance 失败：{error}"),
            )
        })?;
        write_atomic(&path, &bytes)
    }
}

fn normalize_document(document: &str) -> String {
    document.replace('\\', "/")
}

/// Spec 里直接引用 `asset:{key}` 的节点 key。来源记录里的 `used_by` 与这里取并集，
/// 因为 AI 可能预先填了 `used_by`，而真正的引用事实在节点上。
fn asset_references(spec: &UiSpec, key: &str) -> Vec<String> {
    let needle = format!("asset:{key}");
    spec.walk_nodes()
        .into_iter()
        .filter_map(|(node, _)| {
            let source = match &node.kind {
                UiNodeKind::Image { src, .. } => Some(src.as_str()),
                UiNodeKind::Button { src: Some(src), .. } => Some(src.as_str()),
                _ => None,
            };
            (source == Some(needle.as_str())).then(|| node.key.clone())
        })
        .collect()
}

fn proposal_id(document: &str, base_revision: u64, encoded: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(document.len() + encoded.len() + 8);
    bytes.extend_from_slice(document.as_bytes());
    bytes.extend_from_slice(&base_revision.to_le_bytes());
    bytes.extend_from_slice(encoded);
    format!("uip_{:016x}", content_hash(&bytes))
}

/// 资产 key → 文件名。key 只用于拼路径，必须保证不含分隔符/控制字符。
/// 出图落盘与发布门查文件必须是同一个约定，否则生成资产会被判成“缺图”。
pub fn sanitize_asset_key(key: &str) -> String {
    let sanitized: String = key
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "asset".to_owned()
    } else {
        sanitized
    }
}

pub fn encode_spec(spec: &UiSpec) -> UiHostResult<Vec<u8>> {
    let mut encoded = serde_json::to_vec_pretty(spec).map_err(|error| {
        UiHostError::new(UiHostErrorCode::Io, format!("序列化 UiSpec 失败：{error}"))
    })?;
    encoded.push(b'\n');
    Ok(encoded)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> UiHostResult<()> {
    let Some(parent) = path.parent() else {
        return Err(UiHostError::new(
            UiHostErrorCode::InvalidArgument,
            format!("路径没有父目录：{}", path.display()),
        ));
    };
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let temporary = path.with_extension(format!(
        "tmp{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    fs::write(&temporary, bytes).map_err(|error| io_error(&temporary, error))?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        io_error(path, error)
    })
}

/// 工程内相对路径校验：切图源/输出目录都不能逃出 `root`。
fn validate_relative_path(path: &str, suffix: &str, label: &str) -> UiHostResult<String> {
    let relative = Path::new(path);
    let requirement = if suffix.is_empty() {
        "工程内相对路径".to_owned()
    } else {
        format!("工程内 {suffix} 相对路径")
    };
    if path.trim().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || (!suffix.is_empty() && !path.ends_with(suffix))
    {
        return Err(UiHostError::new(
            UiHostErrorCode::InvalidArgument,
            format!("{label}必须是{requirement}：{path}"),
        ));
    }
    Ok(path.replace('\\', "/"))
}

/// `created_at` 只需要可排序与可读；不引入日期库，按 UTC 手算。
fn rfc3339_now() -> String {
    format_rfc3339(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
    )
}

fn format_rfc3339(seconds: u64) -> String {
    let days = seconds / 86_400;
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    let hour = remainder / 3_600;
    let minute = (remainder % 3_600) / 60;
    let second = remainder % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant 的 civil_from_days：把 1970-01-01 起的天数换成公历日期。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// 预览图走 JSON-RPC 时用 base64，避免把几十万字节展开成数字数组。
pub fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(a >> 2) as usize] as char);
        output.push(TABLE[(((a & 0b11) << 4) | (b >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[(((b & 0b1111) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(c & 0b11_1111) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_ui_spec::UiPatchOp;
    use serde_json::json;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-ui-spec-host-{label}-{}-{}",
            std::process::id(),
            crate::test_token()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp root");
        root
    }

    fn spec(name: &str) -> UiSpec {
        serde_json::from_value(json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": name},
            "tokens": {"color.surface": "#101014"},
            "styles": {"card": {"base": {"bg": "@color.surface"}}},
            "root": {
                "key": "root",
                "type": "panel",
                "style": "card",
                "children": [
                    {"key": "title", "type": "text", "text": "Shop"}
                ]
            }
        }))
        .expect("spec")
    }

    fn write_document(store: &UiSpecStore, document: &str, spec: &UiSpec) -> u64 {
        let path = store.resolve(document).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let encoded = encode_spec(spec).unwrap();
        fs::write(&path, &encoded).unwrap();
        revision_hash(&encoded)
    }

    #[test]
    fn revision_is_content_hash_and_export_round_trips() {
        let root = temp_root("revision");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let loaded = store.load("ui/shop.ui.json").expect("load");
        assert_eq!(loaded.revision, revision);
        assert_eq!(loaded.spec, spec("shop"));
        let _ = fs::remove_dir_all(root);
    }

    /// revision 要走完整 JSON 数字往返：外部 MCP 客户端把 JSON 数字读成 f64，
    /// 超过 2^53 会被静默改值，客户端于是永远提交过期 `base_revision`。
    #[test]
    fn revision_survives_a_json_number_round_trip() {
        let root = temp_root("revision_json_number");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        assert!(
            revision < rustlua_ui_spec::JSON_SAFE_REVISION_LIMIT,
            "revision {revision} 超出 JSON 数字安全位宽"
        );
        let frame = json!({ "revision": revision }).to_string();
        let over_wire = serde_json::from_str::<serde_json::Value>(&frame).expect("解析回执帧");
        assert_eq!(
            over_wire["revision"].as_f64().expect("f64") as u64,
            revision,
            "客户端按 f64 读回的 revision 已经变了：{frame}"
        );
        // 按客户端读回的值再提交一次，必须仍被判为当前 revision。
        let echoed = over_wire["revision"].as_u64().expect("u64");
        store
            .submit_spec("ui/shop.ui.json", echoed, spec("shop"))
            .expect("回读 revision 的提案");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_zero_base_proposal_can_create_a_new_document() {
        let root = temp_root("create_document");
        let store = UiSpecStore::new(&root);
        let proposal = store
            .submit_spec("ui/tavern.ui.json", 0, spec("tavern"))
            .expect("新文档提案");
        assert_eq!(proposal.base_revision, 0);
        let accepted = store.accept(&proposal.proposal_id).expect("接受新文档");
        assert_eq!(accepted.document, "ui/tavern.ui.json");
        assert!(root.join("ui/tavern.ui.json").is_file());
        assert_eq!(
            store.load("ui/tavern.ui.json").unwrap().spec,
            spec("tavern")
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 提案已生成、文件随后被别人创建：accept 必须判过期，不能覆盖。
    #[test]
    fn accepting_a_new_document_proposal_refuses_a_file_that_appeared_meanwhile() {
        let root = temp_root("create_document_race");
        let store = UiSpecStore::new(&root);
        let proposal = store
            .submit_spec("ui/tavern.ui.json", 0, spec("tavern"))
            .expect("新文档提案");
        write_document(&store, "ui/tavern.ui.json", &spec("tavern"));
        let error = store
            .accept(&proposal.proposal_id)
            .expect_err("文件已出现必须判过期");
        assert!(error.is(UiHostErrorCode::StaleRevision), "{error:?}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn apply_spec_stores_a_proposal_without_touching_the_document() {
        let root = temp_root("proposal");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let mut next = spec("shop");
        next.root.children.push(
            serde_json::from_value(json!({"key": "buy", "type": "button", "text": "Buy"})).unwrap(),
        );
        let proposal = store
            .submit_spec("ui/shop.ui.json", revision, next.clone())
            .expect("proposal");
        assert_eq!(proposal.kind, ProposalKind::Spec);
        assert_eq!(proposal.base_revision, revision);
        assert_eq!(proposal.diff.nodes_added, 1);
        assert_eq!(
            store.load("ui/shop.ui.json").unwrap().spec,
            spec("shop"),
            "提案未接受前文档不得改变"
        );
        let accepted = store.accept(&proposal.proposal_id).expect("accept");
        assert_eq!(accepted.spec, next);
        assert_eq!(
            store.load("ui/shop.ui.json").unwrap().revision,
            accepted.revision
        );
        assert!(store.list_proposals().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reject_leaves_the_document_untouched() {
        let root = temp_root("reject");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let proposal = store
            .submit_spec("ui/shop.ui.json", revision, spec("changed"))
            .expect("proposal");
        store.reject(&proposal.proposal_id).expect("reject");
        assert_eq!(
            store.load("ui/shop.ui.json").unwrap().spec,
            spec("shop"),
            "拒绝不得改动文档"
        );
        assert!(store.accept(&proposal.proposal_id).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn proposal_ids_are_deterministic() {
        let root = temp_root("deterministic");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let mut next = spec("shop");
        next.root.children[0].style = None;
        let first = store
            .submit_spec("ui/shop.ui.json", revision, next.clone())
            .unwrap();
        let second = store
            .submit_spec("ui/shop.ui.json", revision, next)
            .unwrap();
        assert_eq!(first.proposal_id, second.proposal_id);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_revision_is_rejected() {
        let root = temp_root("stale");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let error = store
            .submit_spec("ui/shop.ui.json", revision + 1, spec("new"))
            .expect_err("过期 revision 必须拒绝");
        assert!(error.is(UiHostErrorCode::StaleRevision), "{error}");
        assert!(!root.join(PROPOSAL_DIR).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let root = temp_root("unknown-key");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let error = store
            .submit_patch(
                "ui/shop.ui.json",
                UiPatch {
                    base_revision: revision,
                    ops: vec![rustlua_ui_spec::UiPatchOp::SetStyle {
                        key: "ghost".to_owned(),
                        style: None,
                    }],
                },
            )
            .expect_err("未知 key 必须拒绝");
        assert!(error.is(UiHostErrorCode::InvalidSpec), "{error}");
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.code == "patch.key_missing")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_key_is_rejected() {
        let root = temp_root("duplicate-key");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let error = store
            .submit_patch(
                "ui/shop.ui.json",
                UiPatch {
                    base_revision: revision,
                    ops: vec![rustlua_ui_spec::UiPatchOp::Insert {
                        parent: "root".to_owned(),
                        index: None,
                        node: Box::new(
                            serde_json::from_value(
                                json!({"key": "title", "type": "text", "text": "Again"}),
                            )
                            .unwrap(),
                        ),
                    }],
                },
            )
            .expect_err("重复 key 必须拒绝");
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.code == "patch.key_duplicate")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn removing_root_is_rejected() {
        let root = temp_root("remove-root");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let error = store
            .submit_patch(
                "ui/shop.ui.json",
                UiPatch {
                    base_revision: revision,
                    ops: vec![rustlua_ui_spec::UiPatchOp::Remove {
                        key: "root".to_owned(),
                    }],
                },
            )
            .expect_err("删除根必须拒绝");
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.code == "patch.remove_root")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn modifying_key_is_rejected() {
        let root = temp_root("modify-key");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let error = store
            .submit_patch(
                "ui/shop.ui.json",
                UiPatch {
                    base_revision: revision,
                    ops: vec![rustlua_ui_spec::UiPatchOp::Replace {
                        key: "title".to_owned(),
                        path: "/key".to_owned(),
                        value: json!("other"),
                    }],
                },
            )
            .expect_err("修改 key 必须拒绝");
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.code == "patch.path_forbidden")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unsupported_capability_is_rejected() {
        let root = temp_root("capability");
        let store = UiSpecStore::new(&root);
        let mut next = spec("shop");
        next.root.kind = rustlua_ui_spec::UiNodeKind::Tabs { selected: 0 };
        let error = store
            .submit_spec("ui/shop.ui.json", 0, next)
            .expect_err("未支持控件必须拒绝");
        assert!(error.is(UiHostErrorCode::Capability), "{error}");
        assert!(
            error
                .diagnostics
                .iter()
                .any(|d| d.code == "spec.node_unsupported")
        );
        assert!(!root.join("ui/shop.ui.json").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn patch_apply_accepts_legal_changes_once() {
        let root = temp_root("patch-apply");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let proposal = store
            .submit_patch(
                "ui/shop.ui.json",
                UiPatch {
                    base_revision: revision,
                    ops: vec![
                        rustlua_ui_spec::UiPatchOp::Replace {
                            key: "title".to_owned(),
                            path: "/text".to_owned(),
                            value: json!("Boutique"),
                        },
                        rustlua_ui_spec::UiPatchOp::SetEvent {
                            key: "title".to_owned(),
                            event: "click".to_owned(),
                            function: "shop.open".to_owned(),
                        },
                    ],
                },
            )
            .expect("patch proposal");
        let accepted = store.accept(&proposal.proposal_id).expect("accept");
        let title = accepted.spec.find_node("title").unwrap();
        assert_eq!(
            title.kind,
            rustlua_ui_spec::UiNodeKind::Text {
                rich_text: false,
                text: "Boutique".to_owned()
            }
        );
        assert_eq!(
            title.events.get("click").map(String::as_str),
            Some("shop.open")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn split_accept_applies_only_selected_ops() {
        let root = temp_root("split-accept");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let patch = UiPatch {
            base_revision: revision,
            ops: vec![
                rustlua_ui_spec::UiPatchOp::Replace {
                    key: "title".to_owned(),
                    path: "/text".to_owned(),
                    value: json!("Boutique"),
                },
                rustlua_ui_spec::UiPatchOp::SetEvent {
                    key: "title".to_owned(),
                    event: "click".to_owned(),
                    function: "shop.open".to_owned(),
                },
            ],
        };
        let proposal = store
            .submit_patch_subset("ui/shop.ui.json", patch.clone(), &[1])
            .expect("拆分接受应生成提案");
        assert_eq!(proposal.diff.nodes_changed, 1, "只应有一个节点被改动");
        let accepted = store.accept(&proposal.proposal_id).expect("accept");
        let title = accepted.spec.find_node("title").unwrap();
        assert_eq!(
            title.kind,
            rustlua_ui_spec::UiNodeKind::Text {
                rich_text: false,
                text: "Shop".to_owned()
            },
            "未选中的文本替换不能落盘"
        );
        assert_eq!(
            title.events.get("click").map(String::as_str),
            Some("shop.open")
        );

        let error = store
            .submit_patch_subset("ui/shop.ui.json", patch, &[2])
            .expect_err("越界索引必须拒绝");
        assert!(error.is(UiHostErrorCode::InvalidArgument), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn asset_listing_and_promotion_require_review() {
        let root = temp_root("assets");
        let store = UiSpecStore::new(&root);
        let mut with_asset = spec("shop");
        with_asset.assets.push(UiAssetPlan {
            key: "shop.bg".to_owned(),
            role: AssetRole::Background,
            path: Some("assets/ui/shop_bg.png".to_owned()),
            generate: None,
            nine_slice: None,
            budget: Default::default(),
        });
        write_document(&store, "ui/shop.ui.json", &with_asset);
        let listings = store.list_assets(None).expect("list");
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].key, "shop.bg");

        let error = store
            .promote_asset("shop.bg", false, Some("CC0".to_owned()), Vec::new())
            .expect_err("未审阅必须拒绝");
        assert!(error.is(UiHostErrorCode::ConfirmationRequired), "{error}");

        let provenance = store
            .promote_asset(
                "shop.bg",
                true,
                Some("CC0".to_owned()),
                vec!["shop.root".to_owned()],
            )
            .expect("promote");
        assert!(provenance.reviewed);
        assert_eq!(provenance.license.as_deref(), Some("CC0"));
        assert_eq!(
            store.list_assets(None).unwrap()[0]
                .provenance
                .as_ref()
                .unwrap()
                .asset_id,
            "shop.bg"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn review_gate_blocks_unreviewed_generated_assets_and_size() {
        let root = temp_root("review");
        let store = UiSpecStore::new(&root);
        let mut with_asset = spec("shop");
        with_asset.assets.push(UiAssetPlan {
            key: "shop.bg".to_owned(),
            role: AssetRole::Background,
            path: None,
            generate: Some(rustlua_ui_spec::ImageGeneration {
                prompt: "wooden shop background".to_owned(),
                negative: None,
                size: Some([64, 32]),
                variants: Some(1),
                seed: Some(7),
            }),
            nine_slice: None,
            budget: Default::default(),
        });
        with_asset.assets.push(UiAssetPlan {
            key: "shop.frame".to_owned(),
            role: AssetRole::Decoration,
            path: Some("assets/ui/frame.png".to_owned()),
            generate: None,
            nine_slice: Some([4, 4, 4, 4]),
            budget: Default::default(),
        });
        with_asset.root.children.push(
            serde_json::from_value(json!({
                "key": "shop_bg",
                "type": "image",
                "src": "asset:shop.bg"
            }))
            .unwrap(),
        );
        write_document(&store, "ui/shop.ui.json", &with_asset);

        let png_path = root.join("assets/ui/frame.png");
        fs::create_dir_all(png_path.parent().unwrap()).unwrap();
        fs::write(&png_path, vec![0u8; 32]).unwrap();
        // 生成计划按落盘约定查找产物；缺图本身也是发布门 error。
        let generated_path = root.join("assets/generated/ui/shop_bg.png");
        fs::create_dir_all(generated_path.parent().unwrap()).unwrap();
        fs::write(&generated_path, vec![0u8; 32]).unwrap();
        let provenance_path = store.asset_registry_path("shop.bg").unwrap();
        fs::create_dir_all(provenance_path.parent().unwrap()).unwrap();
        fs::write(
            &provenance_path,
            serde_json::to_vec(&AssetProvenance {
                asset_id: "shop.bg".to_owned(),
                source: AssetSource::Generated,
                provider: Some("local".to_owned()),
                model: Some("local-checker".to_owned()),
                prompt_hash: Some("hash".to_owned()),
                seed: Some(7),
                license: Some("CC0".to_owned()),
                reviewed: false,
                used_by: Vec::new(),
                created_at: None,
            })
            .unwrap(),
        )
        .unwrap();

        let limits = AssetLimits {
            max_single_bytes: 8,
            max_atlas_bytes: 1024,
            max_total_bytes: 1024,
        };
        let report = store.review_assets(None, &limits).expect("review");
        for code in ["asset.unreviewed_used", "asset.size_exceeded"] {
            assert!(
                report.diagnostics.iter().any(|d| d.code == code),
                "缺少 {code}: {:?}",
                report.diagnostics
            );
        }
        assert_eq!(report.generated, vec!["shop.bg".to_owned()]);

        store
            .promote_asset("shop.bg", true, Some("CC0".to_owned()), Vec::new())
            .expect("promote");
        let report = store
            .review_assets(None, &AssetLimits::default())
            .expect("review after promote");
        assert!(!report.has_errors(), "{:?}", report.diagnostics);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn atlas_region_references_count_each_page_once() {
        let root = temp_root("atlas-page");
        for (name, color) in [
            ("button", [200u8, 80, 40, 255]),
            ("frame", [40u8, 80, 200, 255]),
        ] {
            let path = root.join(format!("assets/ui/{name}.png"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                rustlua_ui_spec::encode_rgba_png(8, 8, &color.repeat(64)),
            )
            .unwrap();
        }
        fs::write(
            root.join(atlas_plan::AtlasConfig::FILE),
            br#"{"schema":1,"atlases":[{"name":"ui","include":["assets/ui/"],"max_size":64,"padding":0,"format":"png"}]}"#,
        )
        .unwrap();
        // 区域引用检查的是 `.atlas` 本体存在，不读内容；编辑期还没有 cook 产物时
        // 发布门只该报“缺图”，体积则按装箱结果算。
        fs::create_dir_all(root.join("assets/atlases")).unwrap();
        fs::write(
            root.join("assets/atlases/ui-0.atlas"),
            "texture=assets/atlases/ui-0.png\n",
        )
        .unwrap();

        let mut document = spec("shop");
        for (key, role, region) in [
            ("ui/button", "button", "button"),
            ("ui/frame", "decoration", "frame"),
        ] {
            let path = format!("assets/atlases/ui-0.atlas#{region}");
            document.assets.push(
                serde_json::from_value(json!({
                    "key": key,
                    "role": role,
                    "path": path
                }))
                .unwrap(),
            );
        }
        let store = UiSpecStore::new(&root);
        write_document(&store, "ui/shop.ui.json", &document);

        let report = store
            .review_assets(None, &AssetLimits::default())
            .expect("review");
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|d| d.code == "asset.file_missing"),
            "{:?}",
            report.diagnostics
        );

        // 同一页被两个区域引用：页字节只能进总包一次。用 atlas_plan 同一口径复算页长。
        let config = atlas_plan::AtlasConfig::load(&root.join(atlas_plan::AtlasConfig::FILE))
            .unwrap()
            .expect("atlas.json");
        let files = atlas_plan::collect_png_files(&root, "assets").unwrap();
        let excluded = atlas_plan::collect_excluded(&root).unwrap();
        let set = atlas_plan::plan_pages(&config, &files, &excluded).unwrap();
        let page_bytes: u64 = set
            .pages
            .iter()
            .map(|page| atlas_plan::encode_page_png(page).unwrap().len() as u64)
            .sum();
        assert!(page_bytes > 0, "两张 8×8 图必须装出一页");
        assert_eq!(
            report.total_bytes, page_bytes,
            "两处区域引用只计一页；单件门也不应为区域再计一次"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preview_returns_a_png() {
        let root = temp_root("preview");
        let store = UiSpecStore::new(&root);
        let png = store
            .preview(&spec("shop"), 320, 180, 1.0)
            .expect("preview");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn rfc3339_is_utc_and_stable() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_rfc3339(1_790_000_000), "2026-09-21T14:13:20Z");
    }

    #[test]
    fn document_paths_cannot_escape_the_project() {
        let root = temp_root("paths");
        let store = UiSpecStore::new(&root);
        for document in ["../x.ui.json", "/abs/x.ui.json", "ui/x.json", ""] {
            assert!(
                store.load(document).is_err(),
                "{document} 必须被路径校验拒绝"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn patch_proposals_keep_their_ops_for_split_acceptance() {
        let root = temp_root("proposal-ops");
        let store = UiSpecStore::new(&root);
        let revision = write_document(&store, "ui/shop.ui.json", &spec("shop"));
        let patch = UiPatch {
            base_revision: revision,
            ops: vec![UiPatchOp::SetEvent {
                key: "title".to_owned(),
                event: "click".to_owned(),
                function: "shop.open".to_owned(),
            }],
        };
        let proposal = store
            .submit_patch("ui/shop.ui.json", patch.clone())
            .unwrap();
        assert_eq!(
            store.proposal_ops(&proposal.proposal_id).unwrap(),
            patch.ops,
            "补丁提案必须能读回原始 ops"
        );
        assert_eq!(
            store.stored_proposal(&proposal.proposal_id).unwrap().patch,
            Some(patch)
        );

        let spec_proposal = store
            .submit_spec("ui/shop2.ui.json", 0, spec("shop2"))
            .unwrap();
        assert!(
            store
                .proposal_ops(&spec_proposal.proposal_id)
                .unwrap()
                .is_empty(),
            "整份 Spec 提案没有可拆分的 op"
        );
        assert!(
            store
                .stored_proposal(&spec_proposal.proposal_id)
                .unwrap()
                .patch
                .is_none()
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 4x2 源图：每个像素颜色不同，裁错行列会直接有别。
    fn write_sheet(root: &Path) -> Vec<u8> {
        let mut rgba = Vec::new();
        for y in 0..2u8 {
            for x in 0..4u8 {
                rgba.extend_from_slice(&[x * 10, y * 100, 7, 255]);
            }
        }
        fs::create_dir_all(root.join("assets/ui")).unwrap();
        fs::write(
            root.join("assets/ui/sheet.png"),
            encode_rgba_png(4, 2, &rgba),
        )
        .unwrap();
        rgba
    }

    #[test]
    fn slice_asset_writes_crops_and_manifest_deterministically() {
        let root = temp_root("slice");
        let store = UiSpecStore::new(&root);
        write_sheet(&root);
        let regions = vec![
            SliceRegion::new("top_left", [0, 0, 2, 1]),
            SliceRegion::new("bottom right", [2, 1, 2, 1]),
        ];
        let manifest = store
            .slice_asset(
                "assets/ui/sheet.png",
                regions.clone(),
                "assets/generated/ui/sheet",
            )
            .expect("切图落盘");
        assert_eq!(manifest.source, "assets/ui/sheet.png");
        assert_eq!((manifest.source_width, manifest.source_height), (4, 2));
        assert_eq!(manifest.slices.len(), 2);
        let first = &manifest.slices[0];
        assert_eq!(first.path, "assets/generated/ui/sheet/top_left.png");
        // 名字里的空格清洗成下划线，但清单保留原始名字用于回指源区域。
        assert_eq!(manifest.slices[1].name, "bottom right");
        assert_eq!(
            manifest.slices[1].path,
            "assets/generated/ui/sheet/bottom_right.png"
        );

        // 裁出来的像素必须正好是源图对应行。
        let source = fs::read(root.join("assets/generated/ui/sheet/top_left.png")).unwrap();
        let crop = encode_rgba_png(2, 1, &[0, 0, 7, 255, 10, 0, 7, 255]);
        assert_eq!(source, crop);

        // 清单最后写，且能和磁盘上的文件对上。
        let stored: SliceManifest = serde_json::from_slice(
            &fs::read(root.join("assets/generated/ui/sheet/slices.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored, manifest);
        for slice in &stored.slices {
            let bytes = fs::read(root.join(&slice.path)).unwrap();
            assert_eq!(bytes.len() as u64, slice.bytes);
        }

        // 同输入重跑：清单与文件字节完全一致。
        let again = store
            .slice_asset("assets/ui/sheet.png", regions, "assets/generated/ui/sheet")
            .expect("重跑");
        assert_eq!(again, manifest);
        assert_eq!(
            fs::read(root.join("assets/generated/ui/sheet/top_left.png")).unwrap(),
            source
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn slice_asset_rejects_bad_inputs_without_writing_anything() {
        let root = temp_root("slice-negative");
        let store = UiSpecStore::new(&root);
        let rgba = write_sheet(&root);
        assert!(!rgba.is_empty());
        let output = "assets/generated/ui/bad";

        let cases: Vec<(Vec<SliceRegion>, &str, UiHostErrorCode, &str)> = vec![
            (
                vec![SliceRegion::new("escape", [3, 0, 2, 1])],
                "assets/ui/sheet.png",
                UiHostErrorCode::InvalidArgument,
                "越界区域",
            ),
            (
                vec![
                    SliceRegion::new("a b", [0, 0, 1, 1]),
                    SliceRegion::new("a:b", [1, 0, 1, 1]),
                ],
                "assets/ui/sheet.png",
                UiHostErrorCode::InvalidArgument,
                "清洗后重名",
            ),
            (
                Vec::new(),
                "assets/ui/sheet.png",
                UiHostErrorCode::InvalidArgument,
                "空区域",
            ),
            (
                vec![SliceRegion::new("ok", [0, 0, 1, 1])],
                "../sheet.png",
                UiHostErrorCode::InvalidArgument,
                "源图逃出工程",
            ),
            (
                vec![SliceRegion::new("ok", [0, 0, 1, 1])],
                "assets/ui/missing.png",
                UiHostErrorCode::NotFound,
                "源图不存在",
            ),
        ];
        for (regions, source, code, label) in cases {
            let error = store.slice_asset(source, regions, output).expect_err(label);
            assert_eq!(error.code, code, "{label}");
        }

        // 输出目录逃出工程同样拒绝。
        let error = store
            .slice_asset(
                "assets/ui/sheet.png",
                vec![SliceRegion::new("ok", [0, 0, 1, 1])],
                "../escape",
            )
            .expect_err("输出目录逃出工程");
        assert_eq!(error.code, UiHostErrorCode::InvalidArgument);

        // 非 PNG 内容：解码失败，且不留输出目录。
        fs::write(root.join("assets/ui/broken.png"), b"not a png").unwrap();
        let error = store
            .slice_asset(
                "assets/ui/broken.png",
                vec![SliceRegion::new("ok", [0, 0, 1, 1])],
                "assets/generated/ui/broken",
            )
            .expect_err("坏 PNG");
        assert_eq!(error.code, UiHostErrorCode::InvalidArgument);
        assert!(!root.join("assets/generated/ui/broken").exists());
        assert!(!root.join("assets/generated/ui/bad").exists());
        let _ = fs::remove_dir_all(root);
    }
}
