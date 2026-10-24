use crate::capabilities::Capabilities;
use crate::spec::{UiNode, UiSpec};
use crate::validate::{Diagnostic, Severity, validate_patch, validate_spec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// AI 对现有 UI 的增量修改。`base_revision` 不匹配时由应用层拒绝，避免覆盖
/// 人工在提案期间做的编辑。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiPatch {
    pub base_revision: u64,
    pub ops: Vec<UiPatchOp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum UiPatchOp {
    Replace {
        key: String,
        /// JSON Pointer 风格，例如 `/style`、`/layout/anchor`、`/events/click`。
        path: String,
        value: Value,
    },
    Insert {
        parent: String,
        /// `None` 或 `-1` 表示追加到末尾。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<i32>,
        /// 节点树可能很大，装箱避免每个补丁操作都按子树大小占位。
        node: Box<UiNode>,
    },
    Remove {
        key: String,
    },
    /// 把节点从原父级摘下插到 `parent` 下；`index` 以**摘除之后**的目标子表为准，
    /// `None` 或 `-1` 追加。同一补丁里先 `insert` 容器再 `move` 进去是合法编辑
    /// （校验按“已执行到这一步”的存在性判断）。
    Move {
        key: String,
        parent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<i32>,
    },
    SetEvent {
        key: String,
        event: String,
        function: String,
    },
    SetStyle {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    /// 新增或改写一个设计令牌；值必须是标量或标量数组（与 `tokens` 的契约一致）。
    SetToken {
        name: String,
        value: Value,
    },
    /// 删除一个设计令牌；仍被样式或节点引用的令牌会被整份重校验拒绝。
    RemoveToken {
        name: String,
    },
}

impl UiPatch {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// 拆分接受：按原始顺序保留 `indices` 里的操作。空选择产生空补丁（合法的 no-op），
/// 越界索引直接报错——静默丢操作会让“拆分接受”变成不可解释的部分写入。
pub fn select_ops(patch: &UiPatch, indices: &[usize]) -> Result<UiPatch, String> {
    let mut keep = std::collections::BTreeSet::new();
    for index in indices {
        if *index >= patch.ops.len() {
            return Err(format!(
                "补丁只有 {} 个操作，拆分接受引用了第 {index} 个",
                patch.ops.len()
            ));
        }
        keep.insert(*index);
    }
    Ok(UiPatch {
        base_revision: patch.base_revision,
        ops: patch
            .ops
            .iter()
            .enumerate()
            .filter(|(index, _)| keep.contains(index))
            .map(|(_, op)| op.clone())
            .collect(),
    })
}

/// 校验并应用补丁。任何 error 级诊断都直接返回，**不产生部分修改**：
/// 调用方拿到的要么是完整的新 Spec，要么是原样未动的失败结果。
pub fn apply_patch(
    spec: &UiSpec,
    patch: &UiPatch,
    caps: &Capabilities,
) -> Result<UiSpec, Vec<Diagnostic>> {
    let mut diagnostics = validate_patch(spec, patch, caps);
    if has_errors(&diagnostics) {
        return Err(diagnostics);
    }

    let mut next = spec.clone();
    for (index, op) in patch.ops.iter().enumerate() {
        if let Err(message) = apply_op(&mut next, op) {
            diagnostics.push(Diagnostic::error(
                "patch.apply_failed",
                format!("ops[{index}]"),
                message,
            ));
        }
    }
    // 与前置检查同一口径：只有 error 阻断。warning（例如刚插入的空容器会在
    // 随后的 move 里被填上子节点）不该把整个补丁打回去。
    if has_errors(&diagnostics) {
        return Err(diagnostics);
    }

    // 补丁本身合法不代表结果合法（例如删掉被绑定引用的节点）。整份重校验兜底。
    let post = validate_spec(&next, caps);
    if has_errors(&post) {
        return Err(post);
    }
    Ok(next)
}

fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

fn apply_op(spec: &mut UiSpec, op: &UiPatchOp) -> Result<(), String> {
    match op {
        UiPatchOp::Replace { key, path, value } => {
            let node = find_node_mut(&mut spec.root, key)
                .ok_or_else(|| format!("目标节点不存在：{key}"))?;
            replace_field(node, path, value.clone())
        }
        UiPatchOp::Insert {
            parent,
            index,
            node,
        } => {
            let parent = find_node_mut(&mut spec.root, parent)
                .ok_or_else(|| format!("父节点不存在：{parent}"))?;
            let at = match index {
                Some(index) if *index >= 0 => (*index as usize).min(parent.children.len()),
                _ => parent.children.len(),
            };
            parent.children.insert(at, (**node).clone());
            Ok(())
        }
        UiPatchOp::Remove { key } => {
            if remove_child(&mut spec.root, key) {
                Ok(())
            } else {
                Err(format!("目标节点不存在：{key}"))
            }
        }
        UiPatchOp::Move { key, parent, index } => {
            let node =
                take_child(&mut spec.root, key).ok_or_else(|| format!("目标节点不存在：{key}"))?;
            let Some(target) = find_node_mut(&mut spec.root, parent) else {
                return Err(format!("父节点不存在：{parent}"));
            };
            let at = match index {
                Some(index) if *index >= 0 => (*index as usize).min(target.children.len()),
                _ => target.children.len(),
            };
            target.children.insert(at, node);
            Ok(())
        }
        UiPatchOp::SetEvent {
            key,
            event,
            function,
        } => {
            let node = find_node_mut(&mut spec.root, key)
                .ok_or_else(|| format!("目标节点不存在：{key}"))?;
            node.events.insert(event.clone(), function.clone());
            Ok(())
        }
        UiPatchOp::SetStyle { key, style } => {
            let node = find_node_mut(&mut spec.root, key)
                .ok_or_else(|| format!("目标节点不存在：{key}"))?;
            node.style = style.clone();
            Ok(())
        }
        UiPatchOp::SetToken { name, value } => {
            spec.tokens.insert(name.clone(), value.clone());
            Ok(())
        }
        UiPatchOp::RemoveToken { name } => {
            if spec.tokens.remove(name).is_some() {
                Ok(())
            } else {
                Err(format!("令牌不存在：{name}"))
            }
        }
    }
}

fn replace_field(node: &mut UiNode, path: &str, value: Value) -> Result<(), String> {
    let segments = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
    if segments.is_empty() || segments[0].is_empty() {
        return Err(format!("路径必须是 JSON Pointer 风格：{path}"));
    }
    let mut encoded = serde_json::to_value(&*node).map_err(|error| error.to_string())?;
    set_pointer(&mut encoded, &segments, value)?;
    *node = serde_json::from_value(encoded)
        .map_err(|error| format!("补丁值不符合节点契约：{error}"))?;
    Ok(())
}

fn set_pointer(current: &mut Value, segments: &[&str], value: Value) -> Result<(), String> {
    let Some((head, tail)) = segments.split_first() else {
        *current = value;
        return Ok(());
    };
    let object = current
        .as_object_mut()
        .ok_or_else(|| format!("路径前缀不是对象：/{}", segments.join("/")))?;
    if tail.is_empty() {
        object.insert((*head).to_owned(), value);
        return Ok(());
    }
    let entry = object
        .entry((*head).to_owned())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(serde_json::Map::new());
    }
    set_pointer(entry, tail, value)
}

fn find_node_mut<'a>(node: &'a mut UiNode, key: &str) -> Option<&'a mut UiNode> {
    if node.key == key {
        return Some(node);
    }
    for child in &mut node.children {
        if let Some(found) = find_node_mut(child, key) {
            return Some(found);
        }
    }
    None
}

fn take_child(node: &mut UiNode, key: &str) -> Option<UiNode> {
    if let Some(position) = node.children.iter().position(|child| child.key == key) {
        return Some(node.children.remove(position));
    }
    node.children
        .iter_mut()
        .find_map(|child| take_child(child, key))
}

fn remove_child(node: &mut UiNode, key: &str) -> bool {
    if let Some(position) = node.children.iter().position(|child| child.key == key) {
        node.children.remove(position);
        return true;
    }
    node.children
        .iter_mut()
        .any(|child| remove_child(child, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_round_trips() {
        let raw = r#"{
            "base_revision": 42,
            "ops": [
                {"op": "replace", "key": "buy", "path": "/style", "value": "primary"},
                {"op": "insert", "parent": "root", "index": -1,
                 "node": {"key": "close", "type": "button", "text": "Close"}},
                {"op": "set_event", "key": "close", "event": "click", "function": "shop.close"},
                {"op": "remove", "key": "old"}
            ]
        }"#;
        let patch: UiPatch = serde_json::from_str(raw).unwrap();
        assert_eq!(patch.base_revision, 42);
        assert_eq!(patch.ops.len(), 4);
        assert_eq!(
            serde_json::to_value(&patch).unwrap(),
            serde_json::from_str::<Value>(raw).unwrap()
        );
    }

    fn spec() -> UiSpec {
        serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "shop"},
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
        .unwrap()
    }

    fn apply(ops: Vec<UiPatchOp>) -> Result<UiSpec, Vec<Diagnostic>> {
        apply_patch(
            &spec(),
            &UiPatch {
                base_revision: 1,
                ops,
            },
            &Capabilities::core(),
        )
    }

    #[test]
    fn applies_field_event_style_insert_and_remove() {
        let next = apply(vec![
            UiPatchOp::Insert {
                parent: "root".to_owned(),
                index: None,
                node: Box::new(
                    serde_json::from_value(serde_json::json!({
                        "key": "buy",
                        "type": "button",
                        "text": "Buy"
                    }))
                    .unwrap(),
                ),
            },
            UiPatchOp::Replace {
                key: "buy".to_owned(),
                path: "/layout/anchor".to_owned(),
                value: Value::String("center".to_owned()),
            },
            UiPatchOp::SetEvent {
                key: "buy".to_owned(),
                event: "click".to_owned(),
                function: "shop.buy".to_owned(),
            },
            UiPatchOp::SetStyle {
                key: "buy".to_owned(),
                style: None,
            },
            UiPatchOp::Remove {
                key: "title".to_owned(),
            },
        ])
        .expect("合法补丁应应用成功");
        let root = &next.root;
        assert_eq!(root.children.len(), 1);
        let buy = &root.children[0];
        assert_eq!(buy.key, "buy");
        assert_eq!(
            buy.events.get("click").map(String::as_str),
            Some("shop.buy")
        );
        assert_eq!(next.find_node("title"), None);
    }

    #[test]
    fn invalid_op_leaves_spec_untouched() {
        let original = spec();
        let error = apply_patch(
            &original,
            &UiPatch {
                base_revision: 1,
                ops: vec![
                    UiPatchOp::Remove {
                        key: "title".to_owned(),
                    },
                    UiPatchOp::Replace {
                        key: "missing".to_owned(),
                        path: "/style".to_owned(),
                        value: Value::Null,
                    },
                ],
            },
            &Capabilities::core(),
        )
        .expect_err("未知 key 必须整批拒绝");
        assert!(error.iter().any(|d| d.code == "patch.key_missing"));
        assert_eq!(
            original.find_node("title").map(|node| node.key.as_str()),
            Some("title")
        );
    }

    #[test]
    fn select_ops_keeps_original_order_and_dedupes() {
        let patch = UiPatch {
            base_revision: 7,
            ops: vec![
                UiPatchOp::SetStyle {
                    key: "title".to_owned(),
                    style: None,
                },
                UiPatchOp::Remove {
                    key: "title".to_owned(),
                },
                UiPatchOp::SetEvent {
                    key: "root".to_owned(),
                    event: "click".to_owned(),
                    function: "shop.close".to_owned(),
                },
            ],
        };
        let subset = select_ops(&patch, &[2, 0, 2]).expect("索引都在范围内");
        assert_eq!(subset.base_revision, 7);
        assert_eq!(subset.ops.len(), 2);
        assert!(matches!(subset.ops[0], UiPatchOp::SetStyle { .. }));
        assert!(matches!(subset.ops[1], UiPatchOp::SetEvent { .. }));

        let empty = select_ops(&patch, &[]).expect("空选择是合法 no-op");
        assert!(empty.is_empty());
        assert_eq!(empty.base_revision, 7);
    }

    #[test]
    fn select_ops_rejects_out_of_range_index() {
        let patch = UiPatch {
            base_revision: 7,
            ops: vec![UiPatchOp::Remove {
                key: "title".to_owned(),
            }],
        };
        let error = select_ops(&patch, &[1]).expect_err("越界索引必须报错");
        assert!(error.contains("第 1 个"), "{error}");
    }

    #[test]
    fn key_rewrite_is_forbidden() {
        let error = apply(vec![UiPatchOp::Replace {
            key: "title".to_owned(),
            path: "/key".to_owned(),
            value: Value::String("other".to_owned()),
        }])
        .expect_err("修改 key 必须被拒");
        assert!(error.iter().any(|d| d.code == "patch.path_forbidden"));
    }

    #[test]
    fn tokens_can_be_added_rewritten_and_removed() {
        let next = apply(vec![
            UiPatchOp::SetToken {
                name: "color.accent".to_owned(),
                value: Value::String("#ff8800".to_owned()),
            },
            UiPatchOp::SetToken {
                name: "spacing.scale".to_owned(),
                value: serde_json::json!([4, 8, 16]),
            },
            UiPatchOp::RemoveToken {
                name: "unused".to_owned(),
            },
        ])
        .expect_err("删除不存在的令牌必须整批拒绝");
        assert!(next.iter().any(|d| d.code == "patch.token_missing"));

        let mut base = spec();
        base.tokens.insert("unused".to_owned(), Value::from(1));
        let next = apply_patch(
            &base,
            &UiPatch {
                base_revision: 1,
                ops: vec![
                    UiPatchOp::SetToken {
                        name: "color.surface".to_owned(),
                        value: Value::String("#202028".to_owned()),
                    },
                    UiPatchOp::SetToken {
                        name: "spacing.scale".to_owned(),
                        value: serde_json::json!([4, 8, 16]),
                    },
                    UiPatchOp::RemoveToken {
                        name: "unused".to_owned(),
                    },
                ],
            },
            &Capabilities::core(),
        )
        .expect("令牌补丁应应用成功");
        assert_eq!(
            next.tokens.get("color.surface"),
            Some(&Value::String("#202028".to_owned()))
        );
        assert_eq!(
            next.tokens.get("spacing.scale"),
            Some(&serde_json::json!([4, 8, 16]))
        );
        assert!(!next.tokens.contains_key("unused"));
    }

    #[test]
    fn removing_a_referenced_token_leaves_the_spec_untouched() {
        let original = spec();
        let error = apply_patch(
            &original,
            &UiPatch {
                base_revision: 1,
                ops: vec![UiPatchOp::RemoveToken {
                    name: "color.surface".to_owned(),
                }],
            },
            &Capabilities::core(),
        )
        .expect_err("仍被样式引用的令牌不能删");
        assert!(
            error
                .iter()
                .any(|d| d.code == "spec.token_missing" || d.code == "patch.token_missing"),
            "{error:?}"
        );
        assert!(original.tokens.contains_key("color.surface"));
    }

    #[test]
    fn bad_token_payloads_are_rejected_before_apply() {
        let error = apply(vec![UiPatchOp::SetToken {
            name: " ".to_owned(),
            value: serde_json::json!({"nested": 1}),
        }])
        .expect_err("空名与对象值都必须拒绝");
        assert!(error.iter().any(|d| d.code == "spec.token_name"));
        assert!(error.iter().any(|d| d.code == "spec.token_shape"));
    }

    fn nested_spec() -> UiSpec {
        serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "nested"},
            "root": {
                "key": "root",
                "type": "panel",
                "children": [
                    {"key": "left", "type": "container", "children": [
                        {"key": "a", "type": "text", "text": "A"},
                        {"key": "b", "type": "text", "text": "B"}
                    ]},
                    {"key": "right", "type": "container", "children": []}
                ]
            }
        }))
        .unwrap()
    }

    fn apply_nested(ops: Vec<UiPatchOp>) -> Result<UiSpec, Vec<Diagnostic>> {
        apply_patch(
            &nested_spec(),
            &UiPatch {
                base_revision: 1,
                ops,
            },
            &Capabilities::core(),
        )
    }

    fn keys_of(node: &UiNode) -> Vec<String> {
        node.children
            .iter()
            .map(|child| child.key.clone())
            .collect()
    }

    #[test]
    fn moves_nodes_between_parents_and_reorders_siblings() {
        let next = apply_nested(vec![UiPatchOp::Move {
            key: "a".to_owned(),
            parent: "right".to_owned(),
            index: None,
        }])
        .expect("跨父级移动");
        assert_eq!(keys_of(&next.root), ["left", "right"]);
        let left = next.find_node("left").unwrap();
        let right = next.find_node("right").unwrap();
        assert_eq!(keys_of(left), ["b"]);
        assert_eq!(keys_of(right), ["a"]);

        // 同一父级内换序：b 插到 a 前面。
        let next = apply_nested(vec![UiPatchOp::Move {
            key: "b".to_owned(),
            parent: "left".to_owned(),
            index: Some(0),
        }])
        .expect("兄弟换序");
        assert_eq!(keys_of(next.find_node("left").unwrap()), ["b", "a"]);
    }

    #[test]
    fn wrap_shape_inserts_container_then_moves_into_it() {
        // UI Studio 的“包裹”就是这两条 op：新容器先插在原位置，节点再移进去。
        let container: UiNode = serde_json::from_value(serde_json::json!({
            "key": "wrapped",
            "type": "container",
            "layout": {"offsets": [0, 0, 0, 0]}
        }))
        .unwrap();
        let next = apply_nested(vec![
            UiPatchOp::Insert {
                parent: "root".to_owned(),
                index: Some(0),
                node: Box::new(container),
            },
            UiPatchOp::Move {
                key: "left".to_owned(),
                parent: "wrapped".to_owned(),
                index: None,
            },
        ])
        .expect("同批插入容器再移动");
        assert_eq!(keys_of(&next.root), ["wrapped", "right"]);
        assert_eq!(keys_of(next.find_node("wrapped").unwrap()), ["left"]);
    }

    #[test]
    fn rejects_moves_that_would_break_the_tree() {
        let into_self = apply_nested(vec![UiPatchOp::Move {
            key: "left".to_owned(),
            parent: "a".to_owned(),
            index: None,
        }])
        .expect_err("移进自己的子树必须拒绝");
        assert!(
            into_self
                .iter()
                .any(|d| d.code == "patch.move_into_self" || d.code == "patch.parent_leaf"),
            "{into_self:?}"
        );

        let root = apply_nested(vec![UiPatchOp::Move {
            key: "root".to_owned(),
            parent: "right".to_owned(),
            index: None,
        }])
        .expect_err("根节点不能移动");
        assert!(root.iter().any(|d| d.code == "patch.move_root"), "{root:?}");

        let leaf_parent = apply_nested(vec![UiPatchOp::Move {
            key: "left".to_owned(),
            parent: "a".to_owned(),
            index: None,
        }])
        .expect_err("叶子控件不能当父级");
        assert!(
            leaf_parent
                .iter()
                .any(|d| d.code == "patch.move_into_self" || d.code == "patch.parent_leaf"),
            "{leaf_parent:?}"
        );

        let missing = apply_nested(vec![UiPatchOp::Move {
            key: "left".to_owned(),
            parent: "nope".to_owned(),
            index: None,
        }])
        .expect_err("父级不存在");
        assert!(missing.iter().any(|d| d.code == "patch.parent_missing"));
    }

    #[test]
    fn remove_then_insert_same_key_in_one_patch_is_legal() {
        let node: UiNode = serde_json::from_value(serde_json::json!({
            "key": "a",
            "type": "text",
            "text": "A2"
        }))
        .unwrap();
        let next = apply_nested(vec![
            UiPatchOp::Remove {
                key: "a".to_owned(),
            },
            UiPatchOp::Insert {
                parent: "left".to_owned(),
                index: Some(1),
                node: Box::new(node),
            },
        ])
        .expect("同批替换同一 key");
        assert_eq!(keys_of(next.find_node("left").unwrap()), ["b", "a"]);
    }
}
