use crate::validate::{Diagnostic, Severity};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiAssetPlan {
    pub key: String,
    pub role: AssetRole,
    /// 已有资产路径或图集引用；与 `generate` 二选一。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generate: Option<ImageGeneration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nine_slice: Option<[u32; 4]>,
    #[serde(default)]
    pub budget: AssetBudget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetRole {
    Background,
    Button,
    Icon,
    Decoration,
    Font,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageGeneration {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AssetBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_atlas_pages: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetProvenance {
    pub asset_id: String,
    pub source: AssetSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default)]
    pub reviewed: bool,
    #[serde(default)]
    pub used_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetSource {
    Generated,
    Human,
    Imported,
}

impl AssetProvenance {
    /// 发布前的资产审计。生成资产必须有可追溯的模型/prompt；许可缺失只告警，
    /// 由发布门决定是否阻断。
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let path = format!("asset_provenance.{}", self.asset_id);
        if self.asset_id.trim().is_empty() {
            out.push(Diagnostic::error(
                "asset.id_empty",
                &path,
                "asset_id 不能为空",
            ));
        }
        if self.source == AssetSource::Generated {
            for (field, value, code) in [
                (
                    "provider",
                    self.provider.as_deref(),
                    "asset.provider_missing",
                ),
                ("model", self.model.as_deref(), "asset.model_missing"),
                (
                    "prompt_hash",
                    self.prompt_hash.as_deref(),
                    "asset.prompt_hash_missing",
                ),
            ] {
                if value.is_none_or(|value| value.trim().is_empty()) {
                    out.push(
                        Diagnostic::error(code, &path, format!("生成资产缺少 {field}"))
                            .with_fix("记录模型来源后才能进入发布包"),
                    );
                }
            }
        }
        if self
            .license
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            out.push(
                Diagnostic::warning("asset.license_missing", &path, "资产未记录许可")
                    .with_fix("补 license 字段，或在上架前替换为已授权资产"),
            );
        }
        if self.source == AssetSource::Generated && !self.reviewed {
            out.push(
                Diagnostic::warning("asset.unreviewed", &path, "AI 生成资产尚未人工审核")
                    .with_fix("在资产浏览器里标记 reviewed=true"),
            );
        }
        out
    }

    pub fn has_errors(&self) -> bool {
        self.validate()
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

impl UiAssetPlan {
    /// 计划本身的自洽性检查（不依赖出图结果）。
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let path = format!("asset_plan.{}", self.key);
        if self.key.trim().is_empty() {
            out.push(Diagnostic::error(
                "asset.key_empty",
                &path,
                "资产 key 不能为空（它是引用与回放的稳定标识）",
            ));
        }
        match (&self.path, &self.generate) {
            (None, None) => out.push(Diagnostic::error(
                "asset.source_missing",
                &path,
                "资产计划必须给 `path` 或 `generate`",
            )),
            (Some(_), Some(_)) => out.push(Diagnostic::error(
                "asset.source_conflict",
                &path,
                "`path` 与 `generate` 二选一，不能同时给",
            )),
            _ => {}
        }
        if let Some(generate) = &self.generate {
            if generate.prompt.trim().is_empty() {
                out.push(Diagnostic::error(
                    "asset.prompt_empty",
                    &path,
                    "生成计划缺少 prompt",
                ));
            }
            if generate.variants == Some(0) {
                out.push(Diagnostic::error(
                    "asset.variants_zero",
                    &path,
                    "variants 至少为 1",
                ));
            }
            if let Some([width, height]) = generate.size
                && let Some([left, right, top, bottom]) = self.nine_slice
                && (left + right >= width || top + bottom >= height)
            {
                out.push(Diagnostic::error(
                    "asset.nine_slice_margins",
                    &path,
                    format!(
                        "九宫格边距和必须小于尺寸：x {left}+{right} < {width}，y {top}+{bottom} < {height}"
                    ),
                ));
            }
        }
        out
    }
}

/// 发布体积上限。单个/图集/总包分别判，超限都是 error（发布门直接红）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetLimits {
    pub max_single_bytes: u64,
    pub max_atlas_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for AssetLimits {
    fn default() -> Self {
        Self {
            max_single_bytes: 4 * 1024 * 1024,
            max_atlas_bytes: 16 * 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
        }
    }
}

/// 一件待发布资产：计划 + 来源 + 实测字节数（`None` = 尚未出图/未测量）。
#[derive(Debug, Clone, PartialEq)]
pub struct AssetEntry {
    pub plan: UiAssetPlan,
    pub provenance: Option<AssetProvenance>,
    pub size_bytes: Option<u64>,
    /// 所属图集页 `(页 id, 页字节数)`；同一页只按 `max_atlas_bytes` 判一次，
    /// 页字节也只计入总包一次（区域引用本身没有独立文件大小）。
    pub atlas_page: Option<(String, u64)>,
}

/// 资产审查结果：诊断 + 发布清单里必须列出的生成资产。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssetReviewReport {
    pub diagnostics: Vec<Diagnostic>,
    pub generated: Vec<String>,
    pub total_bytes: u64,
}

impl AssetReviewReport {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    #[must_use]
    pub fn warnings(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Warning)
            .count()
    }
}

/// 发布前审查：计划自洽、来源可追溯、未审阅资产不得被引用、体积上限。
///
/// 生成资产即使未进当前界面也会进 `generated` 清单——发布包要能答出"哪些图是 AI 出的"。
pub fn review_assets(entries: &[AssetEntry], limits: &AssetLimits) -> AssetReviewReport {
    let mut report = AssetReviewReport::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut atlas_totals: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();
    for entry in entries {
        let key = entry.plan.key.clone();
        report.diagnostics.extend(entry.plan.validate());
        if key.trim().is_empty() {
            continue;
        }
        if !seen.insert(key.clone()) {
            report.diagnostics.push(Diagnostic::error(
                "asset.key_duplicate",
                format!("asset_plan.{key}"),
                "资产 key 重复（引用会指向不确定的那一件）",
            ));
        }
        if entry.plan.generate.is_some()
            && !entry
                .provenance
                .as_ref()
                .is_some_and(|provenance| provenance.source == AssetSource::Generated)
        {
            report.diagnostics.push(Diagnostic::error(
                "asset.provenance_missing",
                format!("asset_plan.{key}"),
                "生成计划缺少 Generated 来源记录",
            ));
        }
        if let Some(provenance) = &entry.provenance {
            report.diagnostics.extend(provenance.validate());
            if provenance.source == AssetSource::Generated {
                report.generated.push(key.clone());
                if !provenance.reviewed && !provenance.used_by.is_empty() {
                    report.diagnostics.push(Diagnostic::error(
                        "asset.unreviewed_used",
                        format!("asset_plan.{key}"),
                        "未审阅的 AI 资产被界面引用，发布门必须阻断",
                    ));
                }
            }
        }
        // 体积门与来源无关：人工导入的图同样占发布包。
        if let Some(size) = entry.size_bytes {
            report.total_bytes = report.total_bytes.saturating_add(size);
            if size > limits.max_single_bytes {
                report.diagnostics.push(Diagnostic::error(
                    "asset.size_exceeded",
                    format!("asset_plan.{key}"),
                    format!("单件资产 {size} B 超过上限 {} B", limits.max_single_bytes),
                ));
            }
        }
        if let Some((page, page_bytes)) = &entry.atlas_page {
            atlas_totals.entry(page.clone()).or_insert(*page_bytes);
        }
        if let Some(budget) = entry.plan.budget.max_bytes
            && entry.size_bytes.is_some_and(|size| size > budget)
        {
            report.diagnostics.push(Diagnostic::error(
                "asset.budget_exceeded",
                format!("asset_plan.{key}"),
                format!("资产超过计划预算 {budget} B"),
            ));
        }
    }
    for (page, bytes) in &atlas_totals {
        // 图集页是发布包里真实存在的产物，页字节同样要计入总包（每页一次）。
        report.total_bytes = report.total_bytes.saturating_add(*bytes);
        if *bytes > limits.max_atlas_bytes {
            report.diagnostics.push(Diagnostic::error(
                "asset.atlas_size_exceeded",
                format!("atlas.page.{page}"),
                format!(
                    "图集页 {page} 共 {bytes} B 超过上限 {} B",
                    limits.max_atlas_bytes
                ),
            ));
        }
    }
    if report.total_bytes > limits.max_total_bytes {
        report.diagnostics.push(Diagnostic::error(
            "asset.total_size_exceeded",
            "asset_manifest",
            format!(
                "资产总计 {} B 超过上限 {} B",
                report.total_bytes, limits.max_total_bytes
            ),
        ));
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_asset_requires_provenance() {
        let provenance = AssetProvenance {
            asset_id: "ui/shop_bg".to_owned(),
            source: AssetSource::Generated,
            provider: None,
            model: None,
            prompt_hash: None,
            seed: None,
            license: None,
            reviewed: false,
            used_by: vec!["shop.root".to_owned()],
            created_at: None,
        };
        let diagnostics = provenance.validate();
        assert!(provenance.has_errors());
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "asset.provider_missing")
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "asset.license_missing")
        );
        assert!(diagnostics.iter().any(|d| d.code == "asset.unreviewed"));
    }

    #[test]
    fn reviewed_generated_asset_with_license_passes() {
        let provenance = AssetProvenance {
            asset_id: "ui/shop_bg".to_owned(),
            source: AssetSource::Generated,
            provider: Some("local".to_owned()),
            model: Some("sd-xl".to_owned()),
            prompt_hash: Some("abc123".to_owned()),
            seed: Some(42),
            license: Some("CC0".to_owned()),
            reviewed: true,
            used_by: vec!["shop.root".to_owned()],
            created_at: Some("2026-10-24T00:00:00Z".to_owned()),
        };
        assert!(provenance.validate().is_empty());
    }

    fn plan(key: &str, size: [u32; 2], nine_slice: Option<[u32; 4]>) -> UiAssetPlan {
        UiAssetPlan {
            key: key.to_owned(),
            role: AssetRole::Button,
            path: None,
            generate: Some(ImageGeneration {
                prompt: "a green button".to_owned(),
                negative: None,
                size: Some(size),
                variants: Some(1),
                seed: Some(7),
            }),
            nine_slice,
            budget: AssetBudget::default(),
        }
    }

    fn provenance(key: &str, reviewed: bool, used_by: Vec<String>) -> AssetProvenance {
        AssetProvenance {
            asset_id: key.to_owned(),
            source: AssetSource::Generated,
            provider: Some("recorded".to_owned()),
            model: Some("image-model".to_owned()),
            prompt_hash: Some("hash".to_owned()),
            seed: Some(7),
            license: Some("CC0".to_owned()),
            reviewed,
            used_by,
            created_at: None,
        }
    }

    #[test]
    fn nine_slice_margins_must_fit_size() {
        let bad = plan("ui/button", [64, 32], Some([40, 24, 4, 4]));
        assert!(
            bad.validate()
                .iter()
                .any(|d| d.code == "asset.nine_slice_margins")
        );
        let good = plan("ui/button", [64, 32], Some([16, 16, 4, 4]));
        assert!(good.validate().is_empty(), "{:?}", good.validate());
    }

    #[test]
    fn unreviewed_generated_asset_cannot_be_referenced() {
        let mut source = provenance("ui/button", false, vec!["shop.buy".to_owned()]);
        source.license = None;
        let entry = AssetEntry {
            plan: plan("ui/button", [64, 32], None),
            provenance: Some(source),
            size_bytes: Some(1024),
            atlas_page: None,
        };
        let report = review_assets(&[entry], &AssetLimits::default());
        assert!(report.has_errors());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "asset.unreviewed_used")
        );
        assert!(report.warnings() >= 2, "缺许可/未审阅都要有 warning");
    }

    #[test]
    fn size_gates_cover_single_atlas_and_total() {
        let limits = AssetLimits {
            max_single_bytes: 100,
            max_atlas_bytes: 150,
            max_total_bytes: 200,
        };
        let entries = vec![
            AssetEntry {
                plan: plan("ui/a", [64, 32], None),
                provenance: Some(provenance("ui/a", true, vec![])),
                size_bytes: Some(120),
                atlas_page: Some(("ui-0".to_owned(), 400)),
            },
            AssetEntry {
                plan: plan("ui/b", [64, 32], None),
                provenance: Some(provenance("ui/b", true, vec![])),
                size_bytes: Some(100),
                atlas_page: None,
            },
        ];
        let report = review_assets(&entries, &limits);
        // 总包 = 单件 120 + 100 + 图集页 400 = 620。
        assert_eq!(report.total_bytes, 620);
        for code in [
            "asset.size_exceeded",
            "asset.atlas_size_exceeded",
            "asset.total_size_exceeded",
        ] {
            assert!(
                report.diagnostics.iter().any(|d| d.code == code),
                "缺少 {code}: {:?}",
                report.diagnostics
            );
        }
    }

    #[test]
    fn one_atlas_page_counts_once_even_with_many_regions() {
        let limits = AssetLimits {
            max_single_bytes: 1000,
            max_atlas_bytes: 150,
            max_total_bytes: 1000,
        };
        let region = |key: &str, page: &str, bytes: u64| AssetEntry {
            plan: plan(key, [64, 32], None),
            provenance: Some(provenance(key, true, vec![])),
            size_bytes: None,
            atlas_page: Some((page.to_owned(), bytes)),
        };
        let report = review_assets(
            &[
                region("ui/a", "ui-0", 120),
                region("ui/b", "ui-0", 120),
                region("ui/c", "ui-1", 130),
            ],
            &limits,
        );
        assert!(
            !report.has_errors(),
            "同页多区域只按页字节判一次、总包也只计一次：{:?}",
            report.diagnostics
        );
        assert_eq!(report.total_bytes, 250, "两页 120 + 130");

        let over = review_assets(&[region("ui/a", "ui-0", 160)], &limits);
        assert!(
            over.diagnostics
                .iter()
                .any(|d| d.code == "asset.atlas_size_exceeded")
        );
    }

    #[test]
    fn review_report_lists_every_generated_asset() {
        let entries = vec![
            AssetEntry {
                plan: plan("ui/a", [64, 32], None),
                provenance: Some(provenance("ui/a", true, vec![])),
                size_bytes: Some(10),
                atlas_page: None,
            },
            AssetEntry {
                plan: plan("ui/b", [64, 32], None),
                provenance: Some(provenance("ui/b", true, vec![])),
                size_bytes: Some(10),
                atlas_page: None,
            },
        ];
        let report = review_assets(&entries, &AssetLimits::default());
        assert_eq!(report.generated, vec!["ui/a".to_owned(), "ui/b".to_owned()]);
        assert!(!report.has_errors());
        assert_eq!(report.total_bytes, 20);
    }

    #[test]
    fn plan_requires_exactly_one_source() {
        let mut missing = plan("ui/a", [64, 32], None);
        missing.generate = None;
        assert!(
            missing
                .validate()
                .iter()
                .any(|d| d.code == "asset.source_missing")
        );
        let mut both = plan("ui/a", [64, 32], None);
        both.path = Some("assets/ui/a.png".to_owned());
        assert!(
            both.validate()
                .iter()
                .any(|d| d.code == "asset.source_conflict")
        );
    }
}
