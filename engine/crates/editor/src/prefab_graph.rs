//! **嵌套 Prefab 的检测与环检测**（docs/55 P1 的 Prefab 一条）。
//!
//! 编辑器**不支持**嵌套 Prefab（docs/56 §5 记的诚实缺口）。"不支持"和"悄悄生成一份坏数据"
//! 是两件事：本模块把"这份 Prefab 里是否嵌了别的 Prefab、会不会绕成环"变成可查、可测的事实，
//! 让调用方能在**创建**与**实例化**两个边界上给出明确拒绝或警告。
//!
//! 为什么用加载闭包而不是直接读盘：扫描逻辑（DFS + 环检测 + 深度上限）与文件系统无关，
//! 用闭包当输入就能在没有工程的单测里把环、菱形依赖、超深链一次跑完。

use std::collections::{BTreeMap, BTreeSet};

/// 嵌套扫描的深度上限：超过就当"链太深"报出来，而不是无限递归。
pub const MAX_NESTING_DEPTH: usize = 16;

/// 一次嵌套扫描的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrefabNesting {
    /// 该 Prefab 直接嵌的其它 Prefab 源（去重、排序）。
    pub nested: Vec<String>,
    /// 传递闭包里出现的所有 Prefab 源（不含起点；排序）。
    pub transitive: Vec<String>,
    /// 检测到的环（每个环从字典序最小的节点开始记录，去重后排序）。
    pub cycles: Vec<Vec<String>>,
    /// 最长嵌套链的深度（起点算 1；无嵌套时为 1）。
    pub max_depth: usize,
    /// 因为超过 [`MAX_NESTING_DEPTH`] 而**没有继续展开**的源（诊断用）。
    pub truncated_at: Vec<String>,
}

impl PrefabNesting {
    /// 这份 Prefab 是"平的"（没有嵌任何别的 Prefab，也没有环）。
    #[must_use]
    pub fn is_flat(&self) -> bool {
        self.nested.is_empty() && self.cycles.is_empty()
    }

    /// 一行人类可读的诊断；平的返回 `None`（调用方不必显示任何东西）。
    #[must_use]
    pub fn diagnostic(&self, source: &str) -> Option<String> {
        if self.is_flat() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.nested.is_empty() {
            parts.push(format!(
                "嵌套 Prefab 尚不支持：{} 里引用了 {}",
                source,
                self.nested.join("、")
            ));
        }
        for cycle in &self.cycles {
            parts.push(format!("检测到 Prefab 引用环：{}", cycle.join(" → ")));
        }
        if !self.truncated_at.is_empty() {
            parts.push(format!(
                "嵌套链超过 {MAX_NESTING_DEPTH} 层，未继续展开：{}",
                self.truncated_at.join("、")
            ));
        }
        Some(parts.join("；"))
    }
}

/// 扫描 `root_source` 的嵌套结构。
///
/// `load(source)` 返回该 Prefab **直接**引用的其它 Prefab 源（`Prefab::nested_sources` 的语义）；
/// 源读不到（文件缺失/解析失败）时返回空——"读不到"不是"没有嵌套"，因此调用方在
/// **创建**边界上要另外确认源存在（那里本来就要求实体在场景里）。
#[must_use]
pub fn scan_prefab_nesting<F>(root_source: &str, load: F) -> PrefabNesting
where
    F: Fn(&str) -> Vec<String>,
{
    let mut report = PrefabNesting {
        nested: load(root_source),
        max_depth: 1,
        ..PrefabNesting::default()
    };
    report.nested.sort();
    report.nested.dedup();

    // 显式栈 DFS：栈里同时带"当前路径"，因此环检测不需要额外的父指针。
    let mut stack: Vec<(String, usize, Vec<String>)> = report
        .nested
        .iter()
        .rev()
        .map(|source| (source.clone(), 2usize, vec![root_source.to_owned(), source.clone()]))
        .collect();
    let mut seen: BTreeSet<String> = report.nested.iter().cloned().collect();
    let mut cycles: BTreeSet<Vec<String>> = BTreeSet::new();
    let mut truncated: BTreeSet<String> = BTreeSet::new();
    // 每个源最长见过的深度（同一个源可能从不同路径到达：菱形依赖不该被当成环）。
    let mut deepest: BTreeMap<String, usize> = BTreeMap::new();

    while let Some((source, depth, path)) = stack.pop() {
        report.max_depth = report.max_depth.max(depth);
        if depth > MAX_NESTING_DEPTH {
            truncated.insert(source);
            continue;
        }
        if deepest.get(&source).is_some_and(|seen| *seen >= depth) {
            continue;
        }
        deepest.insert(source.clone(), depth);
        for child in load(&source) {
            if child.trim().is_empty() {
                continue;
            }
            if let Some(position) = path.iter().position(|item| *item == child) {
                // 环：把从重复节点开始的这一段记下来（起点取字典序最小者，便于去重）。
                let mut cycle: Vec<String> = path[position..].to_vec();
                let start = cycle
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, item)| item.as_str())
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                cycle.rotate_left(start);
                cycles.insert(cycle);
                continue;
            }
            seen.insert(child.clone());
            let mut next_path = path.clone();
            next_path.push(child.clone());
            stack.push((child, depth + 1, next_path));
        }
    }

    report.transitive = seen.into_iter().collect();
    report.cycles = cycles.into_iter().collect();
    report.truncated_at = truncated.into_iter().collect();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 平的 Prefab：没有嵌套、没有环，`diagnostic` 返回 `None`（调用方不用显示任何东西）。
    #[test]
    fn flat_prefabs_report_nothing() {
        let report = scan_prefab_nesting("assets/prefabs/hero.prefab", |_| Vec::new());
        assert!(report.is_flat());
        assert_eq!(report.max_depth, 1);
        assert_eq!(report.diagnostic("hero.prefab"), None);
    }

    /// 一层嵌套：扫描要报出"嵌了谁"，深度 = 2，诊断文本里带上源名。
    #[test]
    fn direct_nesting_is_reported_with_its_source() {
        let report = scan_prefab_nesting("a.prefab", |source| {
            if source == "a.prefab" {
                vec!["b.prefab".to_owned(), "b.prefab".to_owned()]
            } else {
                Vec::new()
            }
        });
        assert!(!report.is_flat());
        assert_eq!(report.nested, vec!["b.prefab".to_owned()], "重复引用要去重");
        assert_eq!(report.transitive, vec!["b.prefab".to_owned()]);
        assert_eq!(report.max_depth, 2);
        let text = report.diagnostic("a.prefab").expect("有诊断");
        assert!(text.contains("嵌套 Prefab 尚不支持"), "{text}");
        assert!(text.contains("b.prefab"), "{text}");
    }

    /// **环**：A → B → A 必须被判出来（这正是"悄悄递归到栈溢出"的那类数据）。
    #[test]
    fn cycles_are_detected_instead_of_recursing_forever() {
        let links = |source: &str| match source {
            "a.prefab" => vec!["b.prefab".to_owned()],
            "b.prefab" => vec!["a.prefab".to_owned()],
            _ => Vec::new(),
        };
        let report = scan_prefab_nesting("a.prefab", links);
        assert_eq!(report.cycles.len(), 1, "{report:?}");
        assert_eq!(
            report.cycles[0],
            vec!["a.prefab".to_owned(), "b.prefab".to_owned()]
        );
        let text = report.diagnostic("a.prefab").expect("有诊断");
        assert!(text.contains("引用环"), "{text}");
        // 自环（自己嵌自己）同样要判出来。
        let self_cycle = scan_prefab_nesting("self.prefab", |source| {
            if source == "self.prefab" {
                vec!["self.prefab".to_owned()]
            } else {
                Vec::new()
            }
        });
        assert_eq!(self_cycle.cycles.len(), 1, "{self_cycle:?}");
    }

    /// **菱形依赖不是环**：A → (B, C) → D 只应报一次、深度 = 3，且不会把 D 判成环。
    #[test]
    fn diamond_dependencies_are_not_cycles() {
        let links = |source: &str| match source {
            "a.prefab" => vec!["b.prefab".to_owned(), "c.prefab".to_owned()],
            "b.prefab" | "c.prefab" => vec!["d.prefab".to_owned()],
            _ => Vec::new(),
        };
        let report = scan_prefab_nesting("a.prefab", links);
        assert!(report.cycles.is_empty(), "{report:?}");
        assert_eq!(report.max_depth, 3);
        assert_eq!(
            report.transitive,
            vec![
                "b.prefab".to_owned(),
                "c.prefab".to_owned(),
                "d.prefab".to_owned()
            ]
        );
    }

    /// 超深链不会无界展开：超过上限的源进 `truncated_at`，且给出可读诊断。
    #[test]
    fn deep_chains_are_truncated_at_the_depth_limit() {
        let links = |source: &str| {
            let index: usize = source
                .trim_start_matches("p")
                .trim_end_matches(".prefab")
                .parse()
                .unwrap_or(0);
            vec![format!("p{}.prefab", index + 1)]
        };
        let report = scan_prefab_nesting("p0.prefab", links);
        assert!(report.max_depth <= MAX_NESTING_DEPTH + 1, "{report:?}");
        assert!(!report.truncated_at.is_empty(), "必须有截断记录：{report:?}");
        let text = report.diagnostic("p0.prefab").expect("有诊断");
        assert!(text.contains("未继续展开"), "{text}");
    }
}
