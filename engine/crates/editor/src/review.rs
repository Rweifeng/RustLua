//! M76-06：可归档的发布前审查报告。
//!
//! 报告只汇总既有事实：Spec 校验诊断、资产来源/许可/体积门、三宿主证据状态与
//! 未收口项。这里不产生新的“绿”：没跑过的宿主一律标 `untested`，调用方要用
//! `ReviewReport::record_host` 显式写入实测证据才能改成 `tested`。

use crate::ui_spec_host::{UiHostResult, UiSpecStore};
use rustlua_ui_spec::{AssetLimits, Diagnostic, Severity, validate_spec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostStatus {
    Tested,
    Untested,
}

/// 一个宿主的证据状态。`checks` 写实际跑过的检查名，未测时为空。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEvidence {
    pub status: HostStatus,
    pub note: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<String>,
}

impl HostEvidence {
    pub fn untested(reason: impl Into<String>) -> Self {
        Self {
            status: HostStatus::Untested,
            note: reason.into(),
            checks: Vec::new(),
        }
    }

    pub fn tested(checks: Vec<String>) -> Self {
        Self {
            status: HostStatus::Tested,
            note: String::new(),
            checks,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreviewedAsset {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetLicense {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub reviewed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewReport {
    pub document: String,
    pub revision: u64,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    pub errors: usize,
    pub warnings: usize,
    #[serde(default)]
    pub unreviewed: Vec<UnreviewedAsset>,
    #[serde(default)]
    pub licenses: Vec<AssetLicense>,
    pub total_bytes: u64,
    #[serde(default)]
    pub generated: Vec<String>,
    /// 固定三宿主键：`pc` / `web` / `wechat`。默认全部 `untested`。
    pub hosts: BTreeMap<String, HostEvidence>,
    #[serde(default)]
    pub unresolved: Vec<String>,
}

pub const HOST_KEYS: [&str; 3] = ["pc", "web", "wechat"];

impl ReviewReport {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.errors == 0
            && self.unreviewed.is_empty()
            && self
                .hosts
                .values()
                .all(|evidence| evidence.status == HostStatus::Tested)
    }

    /// 只有调用方拿到实测证据时才改状态；重复记录以最后一次为准。
    pub fn record_host(&mut self, host: &str, evidence: HostEvidence) {
        self.hosts.insert(host.to_owned(), evidence);
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("ReviewReport 序列化")
    }

    /// 导出到调用方给的路径；父目录不存在时报错而不是静默丢报告。
    pub fn export(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_json_pretty())
    }
}

/// 工程级审查：逐文档报告 + 跨文档资产门，供发布门与归档脚本使用。
///
/// 与 [`review_document`] 一样，这里不会把没跑过的宿主写成已测；`is_green`
/// 只表示“没有 error”，不等于三宿主一致。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectReview {
    pub scanned: usize,
    #[serde(default)]
    pub documents: Vec<ReviewReport>,
    /// 跨文档资产门的诊断（重复 key、总包体积、图集页等）。
    #[serde(default)]
    pub asset_diagnostics: Vec<Diagnostic>,
    /// 证据清单本身的问题（未知文档键、坏文件等）。
    #[serde(default)]
    pub host_diagnostics: Vec<Diagnostic>,
    pub total_bytes: u64,
}

impl ProjectReview {
    /// 文档与资产门的诊断按 (code, path, message) 去重：同一件资产在单文档报告
    /// 与工程资产门里各出现一次，归档时不应被算两遍。
    #[must_use]
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut seen = std::collections::BTreeSet::new();
        self.documents
            .iter()
            .flat_map(|document| document.diagnostics.iter())
            .chain(self.asset_diagnostics.iter())
            .chain(self.host_diagnostics.iter())
            .filter(|diagnostic| {
                seen.insert((
                    diagnostic.code.clone(),
                    diagnostic.path.clone(),
                    diagnostic.message.clone(),
                ))
            })
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn errors(&self) -> usize {
        self.diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .count()
    }

    #[must_use]
    pub fn warnings(&self) -> usize {
        self.diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Warning)
            .count()
    }

    /// 尚未拿到实测证据的宿主数量；发布门必须把它带在读数里而不是省略。
    #[must_use]
    pub fn untested_hosts(&self) -> usize {
        self.documents
            .iter()
            .flat_map(|document| document.hosts.values())
            .filter(|evidence| evidence.status == HostStatus::Untested)
            .count()
    }

    #[must_use]
    pub fn is_green(&self) -> bool {
        self.errors() == 0
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("ProjectReview 序列化")
    }

    pub fn export(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_json_pretty())
    }
}

/// 三宿主证据清单的固定位置（工程相对路径）。
///
/// 清单只声明“哪份文档在哪个宿主上跑过哪条命令、产物在哪”，真正的判定要读产物：
/// 产物的 `document` / `host` / `revision` / `checks` 必须与清单和当前文档一致，
/// 产物缺失或过期一律退回 `untested`，不允许只凭清单自称已测。
pub const HOST_EVIDENCE_PATH: &str = "ui/hosts.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostProof {
    /// 已实际执行、可复现的命令。
    pub command: String,
    /// 产物里必须逐项出现的检查名。
    #[serde(default)]
    pub checks: Vec<String>,
    /// 工程相对路径的 JSON 产物。
    pub artifact: String,
    /// 跑这条证据时文档的 revision。
    pub revision: u64,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct HostEvidenceFile {
    version: u32,
    #[serde(default)]
    documents: BTreeMap<String, BTreeMap<String, HostProof>>,
}

fn load_host_evidence(root: &Path) -> Result<Option<HostEvidenceFile>, String> {
    let path = root.join(HOST_EVIDENCE_PATH);
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!("读取 {HOST_EVIDENCE_PATH} 失败：{error}"));
        }
    };
    let file: HostEvidenceFile = serde_json::from_slice(&raw)
        .map_err(|error| format!("{HOST_EVIDENCE_PATH} 不是合法 JSON：{error}"))?;
    if file.version != 1 {
        return Err(format!(
            "{HOST_EVIDENCE_PATH} 只支持 version=1，实际为 {}",
            file.version
        ));
    }
    Ok(Some(file))
}

fn resolve_evidence_artifact(root: &Path, relative: &str) -> Option<PathBuf> {
    let normalized = relative.replace('\\', "/");
    let path = Path::new(&normalized);
    if normalized.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(path))
}

/// 校验一条宿主证据并转成报告项。失败原因一律进 `Err`，调用方降级为 `untested`。
fn verified_host_evidence(
    root: &Path,
    document: &str,
    revision: u64,
    host: &str,
    proof: &HostProof,
) -> Result<HostEvidence, String> {
    if proof.command.trim().is_empty() {
        return Err("command 为空".to_owned());
    }
    if proof.checks.is_empty() {
        return Err("checks 为空".to_owned());
    }
    let artifact = resolve_evidence_artifact(root, &proof.artifact)
        .ok_or_else(|| format!("artifact 必须是工程内相对路径：{}", proof.artifact))?;
    let raw = std::fs::read(&artifact)
        .map_err(|error| format!("产物 {} 读不到：{error}", proof.artifact))?;
    let value: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|error| format!("产物 {} 不是合法 JSON：{error}", proof.artifact))?;
    let declared = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .replace('\\', "/")
    };
    let artifact_document = declared("document");
    if !same_document(&artifact_document, document) {
        return Err(format!(
            "产物 document={artifact_document:?} 与当前文档 {document:?} 不符"
        ));
    }
    if declared("host") != host {
        return Err(format!(
            "产物 host={:?} 与清单宿主 {host:?} 不符",
            declared("host")
        ));
    }
    let artifact_revision = value
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "产物缺少 revision".to_owned())?;
    if artifact_revision != revision {
        return Err(format!(
            "产物 revision={artifact_revision} 已过期（当前 {revision}）"
        ));
    }
    let artifact_checks: Vec<String> = value
        .get("checks")
        .and_then(serde_json::Value::as_array)
        .map(|checks| {
            checks
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    for check in &proof.checks {
        if !artifact_checks.iter().any(|actual| actual == check) {
            return Err(format!("产物缺少检查项 {check:?}"));
        }
    }
    Ok(HostEvidence {
        status: HostStatus::Tested,
        note: if proof.note.trim().is_empty() {
            format!("{}：{}", proof.command, proof.artifact)
        } else {
            proof.note.clone()
        },
        checks: proof.checks.clone(),
    })
}

fn same_document(left: &str, right: &str) -> bool {
    left.replace('\\', "/").trim_start_matches("./")
        == right.replace('\\', "/").trim_start_matches("./")
}

/// 按证据清单填一份文档的三宿主状态；没有清单/条目/有效产物都保持 `untested`。
fn host_evidence_for(
    store: &UiSpecStore,
    document: &str,
    revision: u64,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<String, HostEvidence> {
    let mut hosts = BTreeMap::new();
    let loaded = load_host_evidence(store.root());
    let file = match loaded {
        Ok(file) => file,
        Err(message) => {
            diagnostics.push(Diagnostic::error(
                "review.host_evidence_invalid",
                HOST_EVIDENCE_PATH,
                message.clone(),
            ));
            for host in HOST_KEYS {
                hosts.insert(
                    host.to_owned(),
                    HostEvidence::untested(format!("{host} 宿主证据清单不可用：{message}")),
                );
            }
            return hosts;
        }
    };
    let proofs = file.as_ref().and_then(|file| {
        file.documents
            .iter()
            .find(|(key, _)| same_document(key, document))
            .map(|(_, proofs)| proofs)
    });
    for host in HOST_KEYS {
        let evidence = match proofs.and_then(|proofs| proofs.get(host)) {
            None => HostEvidence::untested(format!("{host} 宿主没有 {HOST_EVIDENCE_PATH} 证据")),
            Some(proof) => {
                match verified_host_evidence(store.root(), document, revision, host, proof) {
                    Ok(evidence) => evidence,
                    Err(message) => {
                        diagnostics.push(Diagnostic::warning(
                            "review.host_proof_invalid",
                            format!("{HOST_EVIDENCE_PATH}#{document}/{host}"),
                            message.clone(),
                        ));
                        HostEvidence::untested(format!("{host} 宿主证据无效：{message}"))
                    }
                }
            }
        };
        hosts.insert(host.to_owned(), evidence);
    }
    if let Some(proofs) = proofs {
        for host in proofs
            .keys()
            .filter(|host| !HOST_KEYS.contains(&host.as_str()))
        {
            diagnostics.push(Diagnostic::warning(
                "review.host_unknown",
                format!("{HOST_EVIDENCE_PATH}#{document}/{host}"),
                format!("未知宿主键 {host:?}，只认 pc/web/wechat"),
            ));
        }
    }
    hosts
}

/// 扫描工程内全部 `.ui.json` 并跑发布门。任何一份文档解析失败都直接报错，
/// 不允许坏文档被静默跳过。
pub fn review_project(store: &UiSpecStore, limits: &AssetLimits) -> UiHostResult<ProjectReview> {
    let paths = store.document_paths()?;
    let mut documents = Vec::with_capacity(paths.len());
    for document in &paths {
        documents.push(review_document(store, document, limits)?);
    }
    let assets = store.review_assets(None, limits)?;
    let mut host_diagnostics = Vec::new();
    match load_host_evidence(store.root()) {
        Ok(Some(file)) => {
            for key in file.documents.keys() {
                if !paths.iter().any(|path| same_document(path, key)) {
                    host_diagnostics.push(Diagnostic::warning(
                        "review.host_document_unknown",
                        HOST_EVIDENCE_PATH,
                        format!("证据清单里的 {key:?} 不在任何 .ui.json 中"),
                    ));
                }
            }
        }
        Ok(None) => {}
        Err(message) => host_diagnostics.push(Diagnostic::error(
            "review.host_evidence_invalid",
            HOST_EVIDENCE_PATH,
            message,
        )),
    }
    Ok(ProjectReview {
        scanned: paths.len(),
        documents,
        asset_diagnostics: assets.diagnostics,
        host_diagnostics,
        total_bytes: assets.total_bytes,
    })
}

/// 汇总一个文档的审查报告。三宿主默认 `untested`；只有 `ui/hosts.json` 里的
/// 证据条目通过产物校验（文档/宿主/revision/检查项全部对得上）时才记为已测。
pub fn review_document(
    store: &UiSpecStore,
    document: &str,
    limits: &AssetLimits,
) -> UiHostResult<ReviewReport> {
    let loaded = store.load(document)?;
    let mut diagnostics = validate_spec(&loaded.spec, store.capabilities());
    let asset_report = store.review_assets(Some(document), limits)?;
    let hosts = host_evidence_for(store, &loaded.document, loaded.revision, &mut diagnostics);

    let mut unreviewed = Vec::new();
    let mut licenses = Vec::new();
    for listing in store.list_assets(Some(document))? {
        let reviewed = listing
            .provenance
            .as_ref()
            .is_some_and(|provenance| provenance.reviewed);
        let license = listing
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.license.clone());
        let provider = listing
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.provider.clone());
        if !reviewed {
            unreviewed.push(UnreviewedAsset {
                key: listing.key.clone(),
                path: listing.path.clone(),
                source: listing
                    .provenance
                    .as_ref()
                    .map(|provenance| format!("{:?}", provenance.source).to_lowercase()),
            });
        }
        if license.is_none() {
            diagnostics.push(Diagnostic::warning(
                "review.license_missing",
                format!("asset:{}", listing.key),
                "资产没有许可字段，发布前必须补齐或写明豁免理由",
            ));
        }
        licenses.push(AssetLicense {
            key: listing.key,
            license,
            provider,
            reviewed,
        });
    }

    let mut unresolved = Vec::new();
    if !unreviewed.is_empty() {
        unresolved.push(format!("{} 个资产未审阅", unreviewed.len()));
    }
    let host_untested = hosts
        .values()
        .filter(|evidence| evidence.status == HostStatus::Untested)
        .count();
    if host_untested > 0 {
        unresolved.push(format!(
            "{host_untested} 个宿主未测，不得据此宣称跨宿主一致"
        ));
    }

    let errors = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .count();
    let warnings = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Warning)
        .count();

    Ok(ReviewReport {
        document: loaded.document,
        revision: loaded.revision,
        diagnostics,
        errors,
        warnings,
        unreviewed,
        licenses,
        total_bytes: asset_report.total_bytes,
        generated: asset_report.generated,
        hosts,
        unresolved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_spec_host::UiHostErrorCode;
    use rustlua_ui_spec::{AssetLimits, AssetProvenance, AssetSource, UiSpec};

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("rustlua-m76-review-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn spec_with_asset() -> UiSpec {
        serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "shop"},
            "tokens": {"color.surface": "#101014"},
            "styles": {},
            "root": {
                "key": "root",
                "type": "panel",
                "children": [{"key": "banner", "type": "image", "src": "asset:shop.bg"}]
            },
            "assets": [{
                "key": "shop.bg",
                "role": "background",
                "path": "assets/ui/shop_bg.png"
            }]
        }))
        .unwrap()
    }

    fn provenance(license: Option<&str>, reviewed: bool) -> AssetProvenance {
        AssetProvenance {
            asset_id: "shop.bg".to_owned(),
            source: AssetSource::Generated,
            provider: Some("local-checker".to_owned()),
            model: Some("local-checker".to_owned()),
            prompt_hash: Some("0123456789abcdef".to_owned()),
            seed: Some(7),
            license: license.map(str::to_owned),
            reviewed,
            used_by: vec!["banner".to_owned()],
            created_at: None,
        }
    }

    fn write_fixture(root: &std::path::Path, license: Option<&str>, reviewed: bool) -> UiSpecStore {
        std::fs::create_dir_all(root.join("ui")).unwrap();
        let store = UiSpecStore::new(root);
        let encoded = crate::ui_spec_host::encode_spec(&spec_with_asset()).unwrap();
        std::fs::write(root.join("ui/shop.ui.json"), encoded).unwrap();
        store
            .register_provenance(&provenance(license, reviewed))
            .expect("写 provenance");
        std::fs::create_dir_all(root.join("assets/ui")).unwrap();
        std::fs::write(root.join("assets/ui/shop_bg.png"), vec![0u8; 32]).unwrap();
        store
    }

    #[test]
    fn project_review_goes_red_on_size_unreviewed_and_broken_documents() {
        let root = temp_root("project");
        let store = write_fixture(&root, Some("CC0"), true);
        let limits = AssetLimits::default();
        let report = review_project(&store, &limits).expect("扫描工程");
        assert_eq!(report.scanned, 1);
        assert!(report.is_green(), "{}", report.to_json_pretty());
        assert_eq!(report.untested_hosts(), 3);
        assert!(!report.to_json_pretty().contains("\"tested\""));

        let tight = AssetLimits {
            max_single_bytes: 8,
            ..limits
        };
        let report = review_project(&store, &tight).expect("体积门");
        assert!(!report.is_green());
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "asset.size_exceeded")
        );

        store
            .register_provenance(&provenance(Some("CC0"), false))
            .expect("覆盖 provenance");
        let report = review_project(&store, &limits).expect("未审阅门");
        assert!(!report.is_green());
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "asset.unreviewed_used")
        );

        std::fs::write(root.join("ui/broken.ui.json"), b"{").unwrap();
        let error = review_project(&store, &limits).expect_err("坏文档必须报错而不是跳过");
        assert!(error.is(UiHostErrorCode::InvalidSpec), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn generated_asset_without_provenance_sidecar_blocks_the_project_gate() {
        let root = temp_root("generate");
        std::fs::create_dir_all(root.join("ui")).unwrap();
        let store = UiSpecStore::new(&root);
        let mut spec = spec_with_asset();
        spec.assets[0].path = None;
        spec.assets[0].generate = Some(rustlua_ui_spec::ImageGeneration {
            prompt: "a shop banner".to_owned(),
            negative: None,
            size: Some([256, 128]),
            variants: None,
            seed: Some(3),
        });
        let encoded = crate::ui_spec_host::encode_spec(&spec).unwrap();
        std::fs::write(root.join("ui/shop.ui.json"), encoded).unwrap();

        let report = review_project(&store, &AssetLimits::default()).expect("扫描工程");
        assert!(!report.is_green());
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "asset.provenance_missing"),
            "{}",
            report.to_json_pretty()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn report_marks_unreviewed_assets_and_untested_hosts() {
        let root = temp_root("unreviewed");
        let store = write_fixture(&root, None, false);
        let report = review_document(&store, "ui/shop.ui.json", &AssetLimits::default()).unwrap();
        assert_eq!(report.unreviewed.len(), 1);
        assert_eq!(report.unreviewed[0].key, "shop.bg");
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "review.license_missing")
        );
        assert_eq!(report.hosts["pc"].status, HostStatus::Untested);
        assert_eq!(report.hosts["wechat"].status, HostStatus::Untested);
        assert!(!report.is_clean());
        assert!(report.unresolved.iter().any(|item| item.contains("未审阅")));

        let json = report.to_json_pretty();
        assert!(json.contains("\"untested\""));
        let _ = std::fs::remove_dir_all(root);
    }

    fn write_host_artifact(
        root: &std::path::Path,
        relative: &str,
        document: &str,
        host: &str,
        revision: u64,
        checks: &[&str],
    ) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "document": document,
                "host": host,
                "revision": revision,
                "checks": checks,
                "draw_ops": 12,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_host_list(root: &std::path::Path, documents: serde_json::Value) {
        let path = root.join(HOST_EVIDENCE_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "documents": documents,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn proof(artifact: &str, revision: u64) -> serde_json::Value {
        serde_json::json!({
            "command": "cargo run -q -p game --example m76_ui_matrix -- --host pc",
            "checks": ["layout_rects", "draw_order"],
            "artifact": artifact,
            "revision": revision,
            "note": "本机实测",
        })
    }

    #[test]
    fn host_evidence_turns_tested_only_with_a_matching_artifact() {
        let root = temp_root("hosts");
        let store = write_fixture(&root, Some("CC0"), true);
        let revision = review_document(&store, "ui/shop.ui.json", &AssetLimits::default())
            .unwrap()
            .revision;
        for host in HOST_KEYS {
            write_host_artifact(
                &root,
                &format!("artifacts/ui-matrix/{host}.json"),
                "ui/shop.ui.json",
                host,
                revision,
                &["layout_rects", "draw_order"],
            );
        }
        write_host_list(
            &root,
            serde_json::json!({
                "ui/shop.ui.json": {
                    "pc": proof("artifacts/ui-matrix/pc.json", revision),
                    "web": proof("artifacts/ui-matrix/web.json", revision),
                    "wechat": proof("artifacts/ui-matrix/wechat.json", revision),
                }
            }),
        );

        let report = review_document(&store, "ui/shop.ui.json", &AssetLimits::default()).unwrap();
        for host in HOST_KEYS {
            assert_eq!(report.hosts[host].status, HostStatus::Tested, "{host}");
            assert_eq!(
                report.hosts[host].checks,
                vec!["layout_rects".to_owned(), "draw_order".to_owned()]
            );
        }
        assert!(report.is_clean(), "{}", report.to_json_pretty());

        // 产物 revision 过期 → 退回未测并给 warning，不能只凭清单自称已测。
        write_host_artifact(
            &root,
            "artifacts/ui-matrix/pc.json",
            "ui/shop.ui.json",
            "pc",
            revision + 1,
            &["layout_rects", "draw_order"],
        );
        let report = review_document(&store, "ui/shop.ui.json", &AssetLimits::default()).unwrap();
        assert_eq!(report.hosts["pc"].status, HostStatus::Untested);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "review.host_proof_invalid" && d.message.contains("过期")),
            "{}",
            report.to_json_pretty()
        );
        assert!(!report.is_clean());

        // 产物缺一个声明过的检查项 → 同样不算已测。
        write_host_artifact(
            &root,
            "artifacts/ui-matrix/pc.json",
            "ui/shop.ui.json",
            "pc",
            revision,
            &["layout_rects"],
        );
        let report = review_document(&store, "ui/shop.ui.json", &AssetLimits::default()).unwrap();
        assert_eq!(report.hosts["pc"].status, HostStatus::Untested);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "review.host_proof_invalid" && d.message.contains("draw_order"))
        );

        // 产物缺失 → 未测；未知宿主键 → warning；坏清单 → error 并全退未测。
        std::fs::remove_file(root.join("artifacts/ui-matrix/wechat.json")).unwrap();
        write_host_list(
            &root,
            serde_json::json!({
                "ui/shop.ui.json": {
                    "pc": proof("artifacts/ui-matrix/pc.json", revision),
                    "web": proof("artifacts/ui-matrix/web.json", revision),
                    "wechat": proof("artifacts/ui-matrix/wechat.json", revision),
                    "switch": proof("artifacts/ui-matrix/pc.json", revision),
                }
            }),
        );
        let report = review_document(&store, "ui/shop.ui.json", &AssetLimits::default()).unwrap();
        assert_eq!(report.hosts["wechat"].status, HostStatus::Untested);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "review.host_unknown")
        );

        std::fs::write(root.join(HOST_EVIDENCE_PATH), b"{").unwrap();
        let report = review_document(&store, "ui/shop.ui.json", &AssetLimits::default()).unwrap();
        assert!(
            report
                .hosts
                .values()
                .all(|host| host.status == HostStatus::Untested)
        );
        assert_eq!(report.errors, 1);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "review.host_evidence_invalid")
        );

        // 项目级：清单里写过但不存在的文档同样要警告。
        write_host_list(
            &root,
            serde_json::json!({
                "ui/gone.ui.json": {
                    "pc": proof("artifacts/ui-matrix/pc.json", revision),
                }
            }),
        );
        let project = review_project(&store, &AssetLimits::default()).unwrap();
        assert!(
            project
                .diagnostics()
                .iter()
                .any(|d| d.code == "review.host_document_unknown"),
            "{}",
            project.to_json_pretty()
        );
        assert!(project.is_green(), "未知文档键只是 warning，不该挡发布门");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reviewed_asset_with_license_leaves_only_host_gap() {
        let root = temp_root("reviewed");
        let store = write_fixture(&root, Some("CC0"), true);
        let mut report =
            review_document(&store, "ui/shop.ui.json", &AssetLimits::default()).unwrap();
        assert!(report.unreviewed.is_empty());
        assert_eq!(report.licenses[0].license.as_deref(), Some("CC0"));
        assert_eq!(report.errors, 0);
        assert!(
            report
                .unresolved
                .iter()
                .any(|item| item.contains("宿主未测"))
        );

        for host in HOST_KEYS {
            report.record_host(host, HostEvidence::tested(vec!["layout_rects".to_owned()]));
        }
        assert!(report.is_clean());
        assert!(
            report
                .hosts
                .values()
                .all(|evidence| evidence.status == HostStatus::Tested)
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
