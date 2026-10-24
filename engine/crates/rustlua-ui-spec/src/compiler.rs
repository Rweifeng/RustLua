//! M76-03: deterministic `UiSpec` → command compiler.
//!
//! The compiler is pure: same spec + capability matrix → byte-identical command
//! sequence. The editor maps these semantic commands onto its scene document;
//! AI never writes `.scene` or component strings directly.

use crate::asset::UiAssetPlan;
use crate::capabilities::Capabilities;
use crate::spec::{UiLayoutSpec, UiNode, UiNodeKind, UiSpec, UiStyle};
use crate::validate::{Diagnostic, Severity, validate_spec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One semantic edit. Commands are emitted in a stable pre-order that the
/// editor replays inside a single Undo transaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum UiCommand {
    SetCanvas {
        reference: [f32; 2],
        scale_mode: crate::spec::CanvasScaleMode,
        safe_area: bool,
    },
    CreateNode {
        key: String,
        name: String,
        parent: Option<String>,
    },
    SetWidget {
        key: String,
        widget: UiNodeKind,
    },
    SetLayout {
        key: String,
        layout: UiLayoutSpec,
    },
    SetStyleRef {
        key: String,
        style: Option<String>,
    },
    SetEvents {
        key: String,
        events: std::collections::BTreeMap<String, String>,
    },
    RegisterAsset {
        plan: UiAssetPlan,
    },
    SetBinding {
        target: String,
        property: String,
        source: String,
        mode: crate::spec::BindingMode,
        default: Option<Value>,
    },
    DefineStyle {
        name: String,
        style: UiStyle,
    },
    DefineToken {
        name: String,
        value: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledUi {
    pub document: String,
    pub commands: Vec<UiCommand>,
}

/// Stable FNV-1a content hash used for document revisions.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Revision 位宽上限：`2^53`。`base_revision` 要经 JSON 数字往返，而 JS 侧的
/// `Number` 是 f64，超过这个位宽会被静默改值（外部 MCP 客户端因此永远提交过期
/// revision）。留够碰撞空间仍远大于单工程文档数。
pub const JSON_SAFE_REVISION_LIMIT: u64 = 1_u64 << 53;

/// 文档 revision：内容哈希压进 JSON 安全位宽。
#[must_use]
pub fn revision_hash(bytes: &[u8]) -> u64 {
    content_hash(bytes) % JSON_SAFE_REVISION_LIMIT
}

/// Compile a spec. Validation errors abort; warnings are returned alongside the
/// commands so callers can surface them without blocking the proposal.
pub fn compile(
    spec: &UiSpec,
    caps: &Capabilities,
) -> Result<(CompiledUi, Vec<Diagnostic>), Vec<Diagnostic>> {
    let diagnostics = validate_spec(spec, caps);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(diagnostics);
    }

    let mut commands = vec![UiCommand::SetCanvas {
        reference: spec.document.canvas.reference,
        scale_mode: spec.document.canvas.scale_mode,
        safe_area: spec.document.canvas.safe_area,
    }];
    for (name, style) in &spec.styles {
        commands.push(UiCommand::DefineStyle {
            name: name.clone(),
            style: style.clone(),
        });
    }
    for (name, value) in &spec.tokens {
        commands.push(UiCommand::DefineToken {
            name: name.clone(),
            value: value.clone(),
        });
    }
    emit_node(&spec.root, None, &mut commands);
    for plan in &spec.assets {
        commands.push(UiCommand::RegisterAsset { plan: plan.clone() });
    }
    for binding in &spec.bindings {
        commands.push(UiCommand::SetBinding {
            target: binding.target.clone(),
            property: binding.property.clone(),
            source: binding.source.clone(),
            mode: binding.mode,
            default: binding.default.clone(),
        });
    }
    Ok((
        CompiledUi {
            document: spec.document.name.clone(),
            commands,
        },
        diagnostics,
    ))
}

fn emit_node(node: &UiNode, parent: Option<&str>, out: &mut Vec<UiCommand>) {
    out.push(UiCommand::CreateNode {
        key: node.key.clone(),
        name: node.key.clone(),
        parent: parent.map(str::to_owned),
    });
    out.push(UiCommand::SetWidget {
        key: node.key.clone(),
        widget: node.kind.clone(),
    });
    out.push(UiCommand::SetLayout {
        key: node.key.clone(),
        layout: node.layout.clone(),
    });
    if node.style.is_some() {
        out.push(UiCommand::SetStyleRef {
            key: node.key.clone(),
            style: node.style.clone(),
        });
    }
    if !node.events.is_empty() {
        out.push(UiCommand::SetEvents {
            key: node.key.clone(),
            events: node.events.clone(),
        });
    }
    for child in &node.children {
        emit_node(child, Some(&node.key), out);
    }
}

/// Deepest first-difference between two node serializations, restricted to
/// top-level fields so the proposal is readable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeChange {
    pub key: String,
    pub field: String,
    pub before: Value,
    pub after: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UiSpecDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<NodeChange>,
    pub assets_added: Vec<String>,
    pub assets_removed: Vec<String>,
    /// 值有变化或只在一侧存在的令牌名（新增、删除、改写都算）。
    pub tokens_changed: Vec<String>,
}

impl UiSpecDiff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.assets_added.is_empty()
            && self.assets_removed.is_empty()
            && self.tokens_changed.is_empty()
    }

    #[must_use]
    pub fn to_proposal_diff(&self) -> crate::proposal::ProposalDiff {
        let changed_keys: std::collections::BTreeSet<&str> = self
            .changed
            .iter()
            .map(|change| change.key.as_str())
            .collect();
        crate::proposal::ProposalDiff {
            nodes_added: self.added.len() as u32,
            nodes_changed: changed_keys.len() as u32,
            nodes_removed: self.removed.len() as u32,
            assets_added: self.assets_added.len() as u32,
            tokens_changed: self.tokens_changed.len() as u32,
        }
    }
}

/// Semantic diff. Node order changes are deliberately ignored; key additions
/// and removals are reported, and only fields with different JSON values count
/// as changes.
#[must_use]
pub fn diff_specs(before: &UiSpec, after: &UiSpec) -> UiSpecDiff {
    use std::collections::BTreeMap;

    let before_nodes: BTreeMap<&str, Value> = before
        .walk_nodes()
        .into_iter()
        .map(|(node, _)| {
            (
                node.key.as_str(),
                serde_json::to_value(node).unwrap_or(Value::Null),
            )
        })
        .collect();
    let after_nodes: BTreeMap<&str, Value> = after
        .walk_nodes()
        .into_iter()
        .map(|(node, _)| {
            (
                node.key.as_str(),
                serde_json::to_value(node).unwrap_or(Value::Null),
            )
        })
        .collect();

    let mut diff = UiSpecDiff::default();
    for key in after_nodes.keys() {
        if !before_nodes.contains_key(key) {
            diff.added.push((*key).to_owned());
        }
    }
    for key in before_nodes.keys() {
        if !after_nodes.contains_key(key) {
            diff.removed.push((*key).to_owned());
        }
    }
    for (key, after_value) in &after_nodes {
        let Some(before_value) = before_nodes.get(key) else {
            continue;
        };
        diff_value(key, before_value, after_value, &mut diff.changed);
    }

    let before_assets: std::collections::BTreeSet<&str> = before
        .assets
        .iter()
        .map(|asset| asset.key.as_str())
        .collect();
    let after_assets: std::collections::BTreeSet<&str> = after
        .assets
        .iter()
        .map(|asset| asset.key.as_str())
        .collect();
    diff.assets_added = after_assets
        .difference(&before_assets)
        .map(|key| (*key).to_owned())
        .collect();
    diff.assets_removed = before_assets
        .difference(&after_assets)
        .map(|key| (*key).to_owned())
        .collect();

    let token_names: std::collections::BTreeSet<&str> = before
        .tokens
        .keys()
        .chain(after.tokens.keys())
        .map(String::as_str)
        .collect();
    diff.tokens_changed = token_names
        .into_iter()
        .filter(|name| before.tokens.get(*name) != after.tokens.get(*name))
        .map(str::to_owned)
        .collect();
    diff
}

fn diff_value(key: &str, before: &Value, after: &Value, out: &mut Vec<NodeChange>) {
    if before == after {
        return;
    }
    match (before.as_object(), after.as_object()) {
        (Some(before), Some(after)) => {
            let mut keys: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            keys.extend(before.keys().map(String::as_str));
            keys.extend(after.keys().map(String::as_str));
            for field in keys {
                // Child arrays are represented by node add/remove entries;
                // reporting the parent too would double-count the change.
                if field == "children" {
                    continue;
                }
                let before_value = before.get(field).cloned().unwrap_or(Value::Null);
                let after_value = after.get(field).cloned().unwrap_or(Value::Null);
                if before_value != after_value {
                    out.push(NodeChange {
                        key: key.to_owned(),
                        field: field.to_owned(),
                        before: before_value,
                        after: after_value,
                    });
                }
            }
        }
        _ => out.push(NodeChange {
            key: key.to_owned(),
            field: "node".to_owned(),
            before: before.clone(),
            after: after.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> UiSpec {
        serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "shop"},
            "styles": {"card": {"base": {"bg": "@color.surface"}}},
            "tokens": {"color.surface": "#101014"},
            "root": {"key": "root", "type": "panel", "style": "card", "children": [
                {"key": "title", "type": "text", "text": "Shop",
                 "layout": {"anchor": "top_center", "size": [200, 40]},
                 "events": {"click": "shop.close"}},
                {"key": "list", "type": "scroll_view", "children": [
                    {"key": "row", "type": "container"}
                ]}
            ]},
            "bindings": [{"target": "title", "property": "text", "source": "lua:title"}]
        }))
        .unwrap()
    }

    #[test]
    fn compile_is_deterministic_and_ordered() {
        let caps = Capabilities::planned();
        let (first, warnings) = compile(&spec(), &caps).expect("compiles");
        assert!(
            warnings
                .iter()
                .all(|diagnostic| diagnostic.severity == Severity::Warning)
        );
        let (second, _) = compile(&spec(), &caps).expect("compiles");
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
        let keys: Vec<&str> = first
            .commands
            .iter()
            .filter_map(|command| match command {
                UiCommand::CreateNode { key, .. } => Some(key.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(keys, ["root", "title", "list", "row"]);
        let parents: Vec<Option<&str>> = first
            .commands
            .iter()
            .filter_map(|command| match command {
                UiCommand::CreateNode { parent, .. } => Some(parent.as_deref()),
                _ => None,
            })
            .collect();
        assert_eq!(parents, [None, Some("root"), Some("root"), Some("list")]);
        // Tokens/styles precede nodes; bindings come last.
        assert!(matches!(first.commands[0], UiCommand::SetCanvas { .. }));
        assert!(matches!(
            first.commands.last(),
            Some(UiCommand::SetBinding { .. })
        ));
    }

    #[test]
    fn compile_rejects_invalid_spec_and_capability_gaps() {
        let mut invalid = spec();
        invalid.root.children[0].key = String::new();
        assert!(compile(&invalid, &Capabilities::core()).is_err());

        let mut unsupported = spec();
        unsupported.root.children.push(
            serde_json::from_value(serde_json::json!({"key": "tabs", "type": "tabs"})).unwrap(),
        );
        let diagnostics = compile(&unsupported, &Capabilities::core()).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
        // The same spec compiles once the capability is declared.
        assert!(
            compile(&unsupported, &Capabilities::planned()).is_ok(),
            "{:?}",
            compile(&unsupported, &Capabilities::planned()).err()
        );
    }

    #[test]
    fn diff_reports_added_removed_and_changed_fields() {
        let before = spec();
        let mut after = spec();
        after
            .root
            .children
            .get_mut(0)
            .unwrap()
            .events
            .insert("click".to_owned(), "shop.open".to_owned());
        after.root.children.push(
            serde_json::from_value(
                serde_json::json!({"key": "close", "type": "button", "text": "X"}),
            )
            .unwrap(),
        );
        after.root.children[1].children.clear();
        after.tokens.insert(
            "color.accent".to_owned(),
            Value::String("#ff8800".to_owned()),
        );
        let diff = diff_specs(&before, &after);
        assert_eq!(diff.added, vec!["close".to_owned()]);
        assert_eq!(diff.removed, vec!["row".to_owned()]);
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].key, "title");
        assert_eq!(diff.changed[0].field, "events");
        assert_eq!(diff.tokens_changed, vec!["color.accent".to_owned()]);
        let counts = diff.to_proposal_diff();
        assert_eq!(counts.nodes_added, 1);
        assert_eq!(counts.nodes_removed, 1);
        assert_eq!(counts.nodes_changed, 1);
        assert_eq!(counts.tokens_changed, 1);
        assert!(diff_specs(&before, &before).is_empty());
        assert!(!diff.is_empty());
    }

    #[test]
    fn content_hash_is_stable_and_sensitive() {
        assert_eq!(content_hash(b"abc"), content_hash(b"abc"));
        assert_ne!(content_hash(b"abc"), content_hash(b"abd"));
    }

    /// revision 会被 JSON 客户端按 f64 读；`content_hash` 的全宽 64 位会在那一步
    /// 被静默改值，所以revision 必须落进 2^53 且不同文档仍能区分。
    #[test]
    fn revision_hash_stays_inside_the_json_safe_range() {
        let mut collapsed = 0_u32;
        for index in 0..4096_u32 {
            let bytes = format!("doc-{index}").into_bytes();
            let revision = revision_hash(&bytes);
            assert!(
                revision < JSON_SAFE_REVISION_LIMIT,
                "revision {revision} 超出 2^53"
            );
            assert_eq!(revision as f64 as u64, revision, "revision 过不了 f64 往返");
            // 掩码把全宽哈希压进安全位宽，被压缩掉的位数不该是零头。
            if revision != content_hash(&bytes) {
                collapsed += 1;
            }
        }
        assert!(
            collapsed > 0,
            "掩码形同虚设：4096 份输入没有一个被压进安全位宽"
        );
        assert_ne!(revision_hash(b"abc"), revision_hash(b"abd"));
    }
}
