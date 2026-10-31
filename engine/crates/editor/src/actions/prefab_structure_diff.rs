//! 预制体结构差异：**只读**比较器，先把"差在哪"说清楚，不做任何合并。
//!
//! 为什么先只要读视图：v1 的变体与嵌套同步都只做字段级，遇到结构差异就拒绝；而拒绝文案
//! 原先只有"第一处差异"一句话。作者看不见全貌就无法判断"这是我想要的结构改动，还是
//! 误操作"。合并语义一旦定错很难回退，所以这一步只出差异清单，写盘路径一个字节不动。
//!
//! 三处共用同一份结果（判据只有一处，拒绝文案与窗口不可能各说一套）：
//!
//! 1. 变体保存 / 从基础同步：`基础 ↔ 当前快照`（实体编号相同，按编号对齐）；
//! 2. 嵌套同步 / 写回：`内层源 ↔ 外层快照`（实体编号不同，按位置配对，见
//!    [`pair_and_diff_subtrees`]）；
//! 3. 「资源 ▸ 查看结构差异…」窗口：把 1、2 的结果列出来，点一行定位到当前文档的实体。
//!
//! 只读边界：这里返回的是"有哪些差异"，不提供"接受哪一条"。差异非空时三条写路径仍然拒绝。

use super::prefab_nested_sync::{direct_nested_roots, nested_root_inner_id};
use super::*;
use rustlua_scene::prefab::{PREFAB_SOURCE_META_KEY, Prefab};
use rustlua_scene::scene::SceneEntity;
use std::collections::{BTreeMap, BTreeSet};

/// 一类结构差异（条目种类稳定，文案可变）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StructuralChangeKind {
    /// 文件根不是同一号实体。
    RootMoved,
    /// 参考侧有、候选侧没有的实体。
    MissingEntity,
    /// 候选侧多出的实体。
    AddedEntity,
    /// 父节点不同。
    ParentChanged,
    /// 子节点集合或顺序不同。
    ChildrenChanged,
    /// 嵌套身份证（`prefab.source` / `prefab.source_id` / `prefab.root` 等）不同。
    NestingIdentityChanged,
}

/// 一条结构差异。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StructuralChange {
    pub(crate) kind: StructuralChangeKind,
    /// 候选侧实体编号；当前文档里存在这个实体时，视图可以点它定位。
    pub(crate) candidate: Option<SceneEntityId>,
    /// 面向作者的完整描述（带编号与名字，拒绝文案直接引用）。
    pub(crate) detail: String,
}

impl StructuralChange {
    fn new(
        kind: StructuralChangeKind,
        candidate: Option<SceneEntityId>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            candidate,
            detail: detail.into(),
        }
    }
}

/// 一份 Prefab 的谱系差异分区（窗口里一节 = 一条对比关系）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StructureDiffSection {
    /// 对比关系，例如 `变体 ↔ 基础 assets/prefabs/enemy.prefab`。
    pub(crate) title: String,
    /// 对比没算成的原因（源文件读不到等）；有值时 `changes` 必为空。
    pub(crate) note: Option<String>,
    pub(crate) changes: Vec<StructuralChange>,
    /// `changes` 里的候选侧编号是否就是当前文档的实体编号（决定能不能点中定位）。
    pub(crate) locatable: bool,
}

/// 窗口里"重新计算"要回到的目标。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StructureDiffTarget {
    /// 当前打开的文档。
    Document,
    /// 工程内某份 `.prefab` 文件（资源面板右键进入；文件没打开时不可定位）。
    File(PathBuf),
}

/// 两条写路径共用的单行摘要：前几条点名，其余报数量。
pub(crate) fn summarize(changes: &[StructuralChange]) -> String {
    const SHOWN: usize = 3;
    let mut text = format!("{} 项结构差异", changes.len());
    let shown: Vec<&str> = changes
        .iter()
        .take(SHOWN)
        .map(|change| change.detail.as_str())
        .collect();
    if !shown.is_empty() {
        text.push('：');
        text.push_str(&shown.join("；"));
    }
    if changes.len() > shown.len() {
        text.push_str(&format!("；另有 {} 项", changes.len() - shown.len()));
    }
    text
}

/// 两份**同编号谱系**的预制体（基础 ↔ 变体快照）的结构差异。
///
/// 编号就是对齐依据：变体是完整快照，同名实体在两边是同一号。
pub(crate) fn structural_changes(
    reference: &Prefab,
    candidate: &Prefab,
    reference_side: &str,
    candidate_side: &str,
) -> Vec<StructuralChange> {
    let mut changes = Vec::new();
    if reference.root != candidate.root {
        changes.push(StructuralChange::new(
            StructuralChangeKind::RootMoved,
            Some(candidate.root),
            format!(
                "根实体 #{} → #{}（{reference_side} ↔ {candidate_side}）",
                reference.root.0, candidate.root.0
            ),
        ));
    }
    for (id, reference_entity) in &reference.entities {
        let Some(entity) = candidate.entities.get(id) else {
            changes.push(StructuralChange::new(
                StructuralChangeKind::MissingEntity,
                None,
                format!(
                    "{candidate_side}缺少实体 #{}（{}）",
                    id.0, reference_entity.name
                ),
            ));
            continue;
        };
        if entity.parent != reference_entity.parent {
            changes.push(StructuralChange::new(
                StructuralChangeKind::ParentChanged,
                Some(*id),
                format!(
                    "实体 #{}（{}）的父节点变了：{} → {}",
                    id.0,
                    entity.name,
                    parent_text(reference_entity.parent),
                    parent_text(entity.parent)
                ),
            ));
        }
        if entity.children != reference_entity.children {
            changes.push(StructuralChange::new(
                StructuralChangeKind::ChildrenChanged,
                Some(*id),
                format!("实体 #{}（{}）的子节点或顺序变了", id.0, entity.name),
            ));
        }
        if let Some(field) = nested_marker_difference(reference_entity, entity) {
            changes.push(StructuralChange::new(
                StructuralChangeKind::NestingIdentityChanged,
                Some(*id),
                format!(
                    "实体 #{}（{}）的嵌套预制体标记 {field} 变了",
                    id.0, entity.name
                ),
            ));
        }
    }
    for (id, entity) in &candidate.entities {
        if !reference.entities.contains_key(id) {
            changes.push(StructuralChange::new(
                StructuralChangeKind::AddedEntity,
                Some(*id),
                format!("{candidate_side}多出实体 #{}（{}）", id.0, entity.name),
            ));
        }
    }
    changes
}

/// 内层源 ↔ 外层快照子树：按位置配对，**同时**给出配对表与差异清单。
///
/// 配对规则与 v1 的同步判据完全相同（`prefab.source` / `prefab.source_id` /
/// `prefab.root` 必须逐字对得上，子节点按位置一一对应）；差别只在于不立刻放弃：
/// 差异被记下来，配对继续沿"还对得上的那一条"走，供窗口展示。
///
/// 身份已经对不上的分支**不再下钻**：那棵子树的位置配对没有意义，继续走只会把一处
/// 结构改动放大成一串噪音。子节点数不同时把多出来的那几个点名（哪边多、分别是谁）。
pub(crate) fn pair_and_diff_subtrees(
    outer: &Prefab,
    outer_root: SceneEntityId,
    inner: &Prefab,
    inner_root: SceneEntityId,
) -> Result<
    (
        BTreeMap<SceneEntityId, SceneEntityId>,
        Vec<StructuralChange>,
    ),
    String,
> {
    let mut pairs = BTreeMap::new();
    let mut changes = Vec::new();
    visit(
        outer,
        outer_root,
        outer_root,
        inner,
        inner_root,
        &mut pairs,
        &mut changes,
    )?;
    Ok((pairs, changes))
}

fn visit(
    outer: &Prefab,
    outer_id: SceneEntityId,
    outer_root: SceneEntityId,
    inner: &Prefab,
    inner_id: SceneEntityId,
    pairs: &mut BTreeMap<SceneEntityId, SceneEntityId>,
    changes: &mut Vec<StructuralChange>,
) -> Result<(), String> {
    let outer_entity = outer
        .entities
        .get(&outer_id)
        .ok_or_else(|| format!("外层文件缺少实体 {}", outer_id.0))?;
    let inner_entity = inner
        .entities
        .get(&inner_id)
        .ok_or_else(|| format!("内层预制体缺少实体 {}", inner_id.0))?;
    if pairs.insert(inner_id, outer_id).is_some() {
        return Err(format!("内层实体 {} 对应了多个外层节点", inner_id.0));
    }

    // 嵌套身份证逐项校验：第一处不一致就记下并停止下钻（位置配对已经没有意义）。
    let mut identity_problem: Option<String> = None;
    if let Some(expected) = inner_entity.metadata.get(PREFAB_SOURCE_META_KEY) {
        if outer_entity.metadata.get(PREFAB_SOURCE_META_KEY) != Some(expected) {
            identity_problem = Some(format!(
                "外层节点 #{}（{}）的嵌套来源与内层实体 #{} 不一致（内层结构变了）",
                outer_id.0, outer_entity.name, inner_id.0
            ));
        } else if outer_entity.metadata.get("prefab.source_id")
            != inner_entity.metadata.get("prefab.source_id")
        {
            identity_problem = Some(format!(
                "外层节点 #{}（{}）的内层编号与内层实体 #{} 不一致（内层结构变了）",
                outer_id.0, outer_entity.name, inner_id.0
            ));
        }
    } else if outer_entity
        .metadata
        .get("prefab.source_id")
        .and_then(|value| value.parse::<u64>().ok())
        != Some(inner_id.0)
    {
        // 内层这里是它自己的节点（文件根永远属于这张预制体自己），外层对应的快照节点
        // 必须指回它。**不查**外层的 `prefab.source`：那是"我属于哪张内层"的盖章，
        // 同步时由 `bake_nested_from_inner` 负责写，不构成身份差异。
        identity_problem = Some(format!(
            "外层节点 #{}（{}）没有指向内层实体 #{}（内层结构变了）",
            outer_id.0, outer_entity.name, inner_id.0
        ));
    }
    if identity_problem.is_none() {
        let expected_root = inner_entity
            .metadata
            .get("prefab.root")
            .map(String::as_str)
            .unwrap_or(if outer_id == outer_root {
                "true"
            } else {
                "false"
            });
        if outer_entity.metadata.get("prefab.root").map(String::as_str) != Some(expected_root) {
            identity_problem = Some(format!(
                "外层节点 #{}（{}）的实例根标记与内层实体 #{} 不一致（内层结构变了）",
                outer_id.0, outer_entity.name, inner_id.0
            ));
        }
    }
    if let Some(detail) = identity_problem {
        changes.push(StructuralChange::new(
            StructuralChangeKind::NestingIdentityChanged,
            Some(outer_id),
            detail,
        ));
        return Ok(());
    }

    if outer_entity.children.len() != inner_entity.children.len() {
        changes.push(StructuralChange::new(
            StructuralChangeKind::ChildrenChanged,
            Some(outer_id),
            format!(
                "外层节点 #{}（{}）的子节点数变了（外层 {}，内层 {}）：v1 只支持字段级同步",
                outer_id.0,
                outer_entity.name,
                outer_entity.children.len(),
                inner_entity.children.len()
            ),
        ));
        // 多出来的那几个点名（按位置配对，多出的都在尾部）。
        let common = outer_entity.children.len().min(inner_entity.children.len());
        for inner_child in inner_entity.children.iter().skip(common) {
            if let Some(entity) = inner.entities.get(inner_child) {
                changes.push(StructuralChange::new(
                    StructuralChangeKind::MissingEntity,
                    None,
                    format!(
                        "内层新增了节点 #{}（{}），外层快照里没有",
                        entity.id.0, entity.name
                    ),
                ));
            }
        }
        for outer_child in outer_entity.children.iter().skip(common) {
            if let Some(entity) = outer.entities.get(outer_child) {
                changes.push(StructuralChange::new(
                    StructuralChangeKind::AddedEntity,
                    Some(*outer_child),
                    format!(
                        "外层快照多出节点 #{}（{}），内层没有",
                        entity.id.0, entity.name
                    ),
                ));
            }
        }
        return Ok(());
    }
    for (outer_child, inner_child) in outer_entity.children.iter().zip(&inner_entity.children) {
        visit(
            outer,
            *outer_child,
            outer_root,
            inner,
            *inner_child,
            pairs,
            changes,
        )?;
    }
    Ok(())
}

fn parent_text(parent: Option<SceneEntityId>) -> String {
    parent.map_or_else(|| "无".to_owned(), |id| format!("#{}", id.0))
}

/// 实体上任何 `prefab.*` 标记不同都算嵌套身份变化；返回第一个不同的键名。
fn nested_marker_difference(reference: &SceneEntity, candidate: &SceneEntity) -> Option<String> {
    let keys: BTreeSet<&str> = reference
        .metadata
        .keys()
        .chain(candidate.metadata.keys())
        .filter(|key| key.starts_with("prefab."))
        .map(String::as_str)
        .collect();
    keys.into_iter()
        .find(|key| reference.metadata.get(*key) != candidate.metadata.get(*key))
        .map(str::to_owned)
}

impl EditorApp {
    /// 菜单入口：当前文档的谱系差异（变体基础 / 每个直接嵌套实例的内层源）。
    pub(crate) fn open_prefab_structure_diff(&mut self) {
        let sections = match self
            .current_scene()
            .and_then(|document| document.prefab_asset().ok())
        {
            Some(prefab) => self.prefab_structure_sections(&prefab, true),
            None => Vec::new(),
        };
        self.prefab_structure.show = true;
        self.prefab_structure.target = Some(StructureDiffTarget::Document);
        self.prefab_structure.sections = sections;
        self.prefab_structure.message = self.prefab_structure_message();
    }

    /// 资源面板入口：工程内某份 `.prefab`。文件没打开时差异仍可看，但条目不能定位。
    pub(crate) fn open_prefab_structure_diff_for(&mut self, relative: &Path) {
        let root = strip_verbatim_prefix(&self.project.root).clone();
        let absolute = root.join(relative);
        self.prefab_structure.show = true;
        self.prefab_structure.target = Some(StructureDiffTarget::File(relative.to_path_buf()));
        match Prefab::load(&absolute) {
            Ok(prefab) => {
                let open = self
                    .current_scene()
                    .and_then(|document| document.path.as_deref())
                    .map(strip_verbatim_prefix)
                    .is_some_and(|path| path == absolute.as_path());
                self.prefab_structure.sections = self.prefab_structure_sections(&prefab, open);
                self.prefab_structure.message = self.prefab_structure_message();
            }
            Err(error) => {
                self.prefab_structure.sections.clear();
                self.prefab_structure.message = format!("读取 {relative:?} 失败：{error}");
            }
        }
    }

    /// 「重新计算」：回到打开窗口时的那份文档/文件（文档内容可能已被编辑）。
    pub(crate) fn refresh_prefab_structure_diff(&mut self) {
        match self.prefab_structure.target.clone() {
            Some(StructureDiffTarget::Document) => self.open_prefab_structure_diff(),
            Some(StructureDiffTarget::File(path)) => self.open_prefab_structure_diff_for(&path),
            None => {}
        }
    }

    /// 把差异条目定位到当前文档的实体（只改选区，不碰文档与磁盘）。
    pub(crate) fn locate_structural_change(&mut self, entity: SceneEntityId) {
        let present = self
            .current_scene()
            .is_some_and(|document| document.scene.entities.contains_key(&entity));
        if !present {
            self.status = format!("实体 #{} 不在当前文档里，无法定位", entity.0);
            return;
        }
        self.selection
            .set_single(entity, editor::SelectionSource::Command);
        self.status = format!("已定位到实体 #{}", entity.0);
    }

    fn prefab_structure_message(&self) -> String {
        if self.current_scene().is_none() {
            return "没有打开的文档".to_owned();
        }
        if self
            .prefab_structure
            .sections
            .iter()
            .all(|section| section.note.is_some())
        {
            return "没有可对比的谱系：当前对象既不是变体，也没有直接挂的嵌套实例".to_owned();
        }
        String::new()
    }

    /// 一份 Prefab 的谱系差异：变体基础一节 + 每个直接嵌套实例一节。
    fn prefab_structure_sections(
        &self,
        prefab: &Prefab,
        locatable: bool,
    ) -> Vec<StructureDiffSection> {
        let root = strip_verbatim_prefix(&self.project.root);
        let mut sections = Vec::new();
        if let Some(base_relative) = prefab.variant_base().map(str::to_owned) {
            let title = format!("变体 ↔ 基础 {base_relative}");
            match Prefab::load(root.join(&base_relative)) {
                Ok(base) => sections.push(StructureDiffSection {
                    title,
                    note: None,
                    changes: structural_changes(&base, prefab, "基础", "当前快照"),
                    locatable,
                }),
                Err(error) => sections.push(StructureDiffSection {
                    title,
                    note: Some(format!("基础读取失败：{error}")),
                    ..StructureDiffSection::default()
                }),
            }
        }
        let mut by_source: BTreeMap<String, Vec<SceneEntityId>> = BTreeMap::new();
        for (id, source) in direct_nested_roots(prefab) {
            by_source.entry(source).or_default().push(id);
        }
        for (source, roots) in by_source {
            let title_source = source.clone();
            let inner = match Prefab::load(root.join(&source)) {
                Ok(inner) => inner,
                Err(error) => {
                    sections.push(StructureDiffSection {
                        title: format!("嵌套实例 ↔ 内层 {title_source}"),
                        note: Some(format!("内层读取失败：{error}")),
                        ..StructureDiffSection::default()
                    });
                    continue;
                }
            };
            for outer_root in roots {
                let title = format!("嵌套实例 #{} ↔ 内层 {source}", outer_root.0);
                match nested_root_inner_id(prefab, outer_root).and_then(|inner_root| {
                    pair_and_diff_subtrees(prefab, outer_root, &inner, inner_root)
                }) {
                    Ok((_, changes)) => sections.push(StructureDiffSection {
                        title,
                        note: None,
                        changes,
                        locatable,
                    }),
                    Err(error) => sections.push(StructureDiffSection {
                        title,
                        note: Some(error),
                        ..StructureDiffSection::default()
                    }),
                }
            }
        }
        sections
    }
}
