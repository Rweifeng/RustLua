use crate::validate::Diagnostic;
use serde::{Deserialize, Serialize};

/// AI 改动的唯一交付物：先审阅，接受后才落成一次事务。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiProposal {
    pub proposal_id: String,
    pub kind: ProposalKind,
    pub base_revision: u64,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    #[serde(default)]
    pub diff: ProposalDiff,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_png: Option<String>,
    #[serde(default)]
    pub cost: ProposalCost,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    Spec,
    Patch,
    AssetPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProposalDiff {
    #[serde(default)]
    pub nodes_added: u32,
    #[serde(default)]
    pub nodes_changed: u32,
    #[serde(default)]
    pub nodes_removed: u32,
    #[serde(default)]
    pub assets_added: u32,
    /// 新增 / 删除 / 改写的设计令牌数。
    #[serde(default)]
    pub tokens_changed: u32,
}

impl ProposalDiff {
    pub fn is_empty(&self) -> bool {
        self.nodes_added == 0
            && self.nodes_changed == 0
            && self.nodes_removed == 0
            && self.assets_added == 0
            && self.tokens_changed == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ProposalCost {
    #[serde(default)]
    pub llm_tokens: u64,
    #[serde(default)]
    pub image_calls: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_usd: Option<f64>,
}

impl UiProposal {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == crate::validate::Severity::Error)
    }

    /// RFC3339 UTC timestamps sort lexicographically, so no date parser is
    /// needed; callers pass the same clock they would display.
    #[must_use]
    pub fn is_expired_at(&self, now: &str) -> bool {
        self.expires_at
            .as_deref()
            .is_some_and(|expires| expires <= now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_round_trips() {
        let proposal = UiProposal {
            proposal_id: "uip_test".to_owned(),
            kind: ProposalKind::Patch,
            base_revision: 7,
            diagnostics: Vec::new(),
            diff: ProposalDiff {
                nodes_added: 2,
                nodes_changed: 1,
                nodes_removed: 0,
                assets_added: 0,
                tokens_changed: 1,
            },
            preview_png: Some("preview.png".to_owned()),
            cost: ProposalCost {
                llm_tokens: 100,
                image_calls: 0,
                estimated_usd: Some(0.002),
            },
            expires_at: None,
        };
        let encoded = serde_json::to_string(&proposal).unwrap();
        let decoded: UiProposal = serde_json::from_str(&encoded).unwrap();
        assert_eq!(proposal, decoded);
        assert!(!proposal.has_errors());
    }
}
