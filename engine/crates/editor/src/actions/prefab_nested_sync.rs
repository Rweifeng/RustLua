//! 嵌套预制体：内层 → 外层的显式同步（#9 的第三块）。
//!
//! 四条口径：
//!
//! 1. **外层文件里的内层实例是一份取值快照**。内层文件改了，外层不会自动变：必须显式同步。
//!    这避免"打开一个场景就悄悄改写资产"。
//! 2. **记录才代表外层的选择**。`prefab.nested_override.<外层编号>.<字段>` 记录外层相对
//!    内层显式改过的字段；没有记录 = 内层默认值，同步时跟着新内层走。
//!    `prefab.nested_stamp.<内层路径>` 记下这份快照来自哪个内层版本。
//! 3. **记录只在写回外层时产生**（场景写回、in-context 退出、创建预制体、预制体文档保存）。
//!    那一刻外层快照就是作者的当前状态，与内层 diff 即"外层改了什么"。内层若在上次同步后
//!    已经变化，写回会被拒绝——否则内层的新值会被记成外层自己的覆盖，永久钉在快照里。
//! 4. **v1 只做字段级**。内层增删节点、改层级、改更深嵌套的结构都明确拒绝，并提示改用手工
//!    重建；外层自己加在内层实例**外**的节点不受影响（同步只替换内层子树）。
//!
//! 同步 = 新内层 + 旧记录：逐节点替换子树内容、保外层实体编号与父子关系，再把记录按
//! 内层编号套用。外层实体编号必须保住——其它文件里的 `prefab.outer_id` 场景引用、
//! 以及"下次同步按编号对号"都依赖它。

use super::prefab_rebuild::{PrefabWriteRequest, enclosing_prefab_source, prefab_override_value};
use super::prefab_variant::variant_write_back_refusal;
use super::*;
use editor::file_watcher::FileSignature;
use rustlua_scene::prefab::{
    PREFAB_NESTED_OVERRIDE_META_PREFIX, PREFAB_NESTED_STAMP_META_PREFIX, PREFAB_OUTER_ID_META_KEY,
    PREFAB_SOURCE_META_KEY, PrefabInstance,
};
use rustlua_scene::scene::Scene;
use std::collections::{BTreeMap, BTreeSet};

/// 嵌套覆盖/签名记录是否属于"内层谱系"（文档层往返、变体保存都要原样保留的键）。
pub(crate) fn is_nested_lineage_key(key: &str) -> bool {
    key.starts_with(PREFAB_NESTED_OVERRIDE_META_PREFIX)
        || key.starts_with(PREFAB_NESTED_STAMP_META_PREFIX)
}

/// 写回外层时记录的结果（状态栏用它说明"第一次建立了基线"之类的副作用）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NestedLineageReport {
    /// 本次重新建立基线的内层来源（之前没有签名记录）。
    pub(crate) seeded: Vec<String>,
    /// 内层文件读不到、保留旧记录的来源。
    pub(crate) missing: Vec<String>,
    /// 写回时随快照重算的嵌套实例数。
    pub(crate) instances: usize,
}

impl NestedLineageReport {
    /// 追加到状态栏的提示；没有副作用时为 `None`。
    pub(crate) fn status_note(&self) -> Option<String> {
        let mut notes = Vec::new();
        if !self.seeded.is_empty() {
            notes.push(format!("已建立内层同步基线：{}", self.seeded.join("、")));
        }
        if !self.missing.is_empty() {
            notes.push(format!(
                "内层预制体读取失败、保留旧记录：{}",
                self.missing.join("、")
            ));
        }
        (!notes.is_empty()).then(|| notes.join("；"))
    }
}

/// 同步结果（状态栏用）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NestedSyncOutcome {
    pub(crate) instances: usize,
    pub(crate) overrides: usize,
    /// 旧外层文件没有签名记录：本次按内层内容整体重建，外层自定义字段需重新调整。
    pub(crate) legacy: bool,
}

/// 这份文件里**直接挂在这一层**的嵌套实例根及其内层来源。
///
/// 从文件根按树走，碰到带 `prefab.source` 的节点就记下并停止下钻：更深一层的节点属于
/// 内层自己的快照，由内层文件的同步负责，不能算作本文件的直接实例。
pub(crate) fn direct_nested_roots(prefab: &Prefab) -> Vec<(SceneEntityId, String)> {
    fn visit(prefab: &Prefab, id: SceneEntityId, out: &mut Vec<(SceneEntityId, String)>) {
        let Some(entity) = prefab.entities.get(&id) else {
            return;
        };
        if let Some(source) = entity
            .metadata
            .get(PREFAB_SOURCE_META_KEY)
            .map(String::as_str)
            .filter(|source| !source.trim().is_empty())
        {
            out.push((id, source.to_owned()));
            return;
        }
        for child in &entity.children {
            visit(prefab, *child, out);
        }
    }
    let mut out = Vec::new();
    visit(prefab, prefab.root, &mut out);
    out
}

/// 某个直接嵌套实例根下的全部外层节点（含更深嵌套的快照节点）。
fn nested_subtree(prefab: &Prefab, root: SceneEntityId) -> BTreeSet<SceneEntityId> {
    let mut out = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !out.insert(id) {
            continue;
        }
        if let Some(entity) = prefab.entities.get(&id) {
            stack.extend(entity.children.iter().copied());
        }
    }
    out
}

/// 结构配对：内层实体编号 → 外层实体编号，按树形逐层对应。
///
/// v1 不允许结构差异，所以失败就是拒绝；配对与差异清单来自同一个比较器
/// （`prefab_structure_diff::pair_and_diff_subtrees`），拒绝文案点名的是
/// 「资源 ▸ 查看结构差异…」窗口里会列出的同一批条目。
fn pair_subtrees(
    outer: &Prefab,
    outer_root: SceneEntityId,
    inner: &Prefab,
    inner_root: SceneEntityId,
) -> Result<BTreeMap<SceneEntityId, SceneEntityId>, String> {
    let (pairs, changes) =
        super::prefab_structure_diff::pair_and_diff_subtrees(outer, outer_root, inner, inner_root)?;
    if changes.is_empty() {
        Ok(pairs)
    } else {
        Err(format!(
            "{}（v1 只支持字段级同步）",
            super::prefab_structure_diff::summarize(&changes)
        ))
    }
}

/// 嵌套实例根记录的内层实体编号。
pub(crate) fn nested_root_inner_id(
    prefab: &Prefab,
    outer_root: SceneEntityId,
) -> Result<SceneEntityId, String> {
    prefab
        .entities
        .get(&outer_root)
        .and_then(|entity| entity.metadata.get("prefab.source_id"))
        .and_then(|value| value.parse::<u64>().ok())
        .map(SceneEntityId)
        .ok_or_else(|| format!("嵌套实例根 {} 缺少内层实体编号", outer_root.0))
}

/// 写回外层前，把嵌套实例的"外层改动"重算成覆盖记录，并跟上内层签名。
///
/// 只处理**这一层**直接挂的嵌套实例；更深层由内层文件自己的记录负责。
/// 内层读不到时保留旧记录（不阻断写回——快照本身仍是完整的）。
pub(crate) fn prepare_nested_lineage(
    prefab: &mut Prefab,
    project_root: &Path,
) -> Result<NestedLineageReport, String> {
    let root = strip_verbatim_prefix(project_root);
    let mut by_source: BTreeMap<String, Vec<SceneEntityId>> = BTreeMap::new();
    for (id, source) in direct_nested_roots(prefab) {
        by_source.entry(source).or_default().push(id);
    }
    if by_source.is_empty() && prefab.nested_stamps().is_empty() {
        return Ok(NestedLineageReport::default());
    }

    let mut records = prefab
        .nested_overrides()
        .map_err(|error| format!("嵌套覆盖记录损坏：{error}"))?;
    let mut stamps = BTreeMap::new();
    let mut report = NestedLineageReport::default();

    for (source, roots) in &by_source {
        let path = root.join(source);
        let inner = match Prefab::load(&path) {
            Ok(inner) => inner,
            Err(_) => {
                report.missing.push(source.clone());
                continue;
            }
        };
        let stamp = FileSignature::stamp_of(&path)
            .ok_or_else(|| format!("无法读取内层预制体签名：{source}"))?;
        if let Some(previous) = prefab.nested_stamp(source)
            && previous != stamp
        {
            return Err(format!(
                "内层预制体 {source} 在上次同步后已经被修改；请先用「资源 ▸ 从内层预制体同步」\
                 再写回外层，否则内层的新值会被记成外层自己的覆盖"
            ));
        }
        if prefab.nested_stamp(source).is_none() {
            report.seeded.push(source.clone());
        }
        for &outer_root in roots {
            let inner_root = nested_root_inner_id(prefab, outer_root)?;
            let pairs = pair_subtrees(prefab, outer_root, &inner, inner_root)?;
            for (inner_id, outer_id) in &pairs {
                records.retain(|(entity_id, _), _| entity_id != outer_id);
                let inner_entity = inner
                    .entities
                    .get(inner_id)
                    .ok_or_else(|| format!("内层预制体缺少实体 {}", inner_id.0))?;
                let outer_entity = prefab
                    .entities
                    .get(outer_id)
                    .ok_or_else(|| format!("外层文件缺少实体 {}", outer_id.0))?;
                for field in diff_prefab_entity(inner_entity, outer_entity) {
                    let value = prefab_override_value(outer_entity, &field)?;
                    records.insert((*outer_id, field), value);
                }
            }
            report.instances += 1;
        }
        stamps.insert(source.clone(), stamp);
    }

    // 只保留仍然存在的嵌套节点：删掉的内层实例不能把记录留给下一个同号实体。
    records.retain(|(entity_id, _), _| {
        prefab
            .entities
            .get(entity_id)
            .is_some_and(|entity| entity.metadata.contains_key(PREFAB_SOURCE_META_KEY))
    });
    prefab
        .set_nested_overrides(&records)
        .map_err(|error| format!("嵌套覆盖记录无法写回：{error}"))?;
    let stale: Vec<String> = prefab
        .nested_stamps()
        .into_keys()
        .filter(|source| !by_source.contains_key(source))
        .collect();
    for source in stale {
        prefab
            .set_nested_stamp(&source, None)
            .map_err(|error| error.to_string())?;
    }
    for (source, stamp) in stamps {
        prefab
            .set_nested_stamp(&source, Some(&stamp))
            .map_err(|error| error.to_string())?;
    }
    Ok(report)
}

/// 预制体**文档**保存前的嵌套准备：重算记录并回写文档元数据，实体本身不动。
///
/// 与变体保存同一条口径：内层源在上次同步后动过就拒绝保存，否则内层的新值会以
/// "外层默认值"的名义写进快照，而记录里没有它——下次同步就会被内层再覆盖一次。
pub(crate) fn prepare_nested_for_document_save(
    document: &mut editor::SceneDocument,
    project_root: &Path,
) -> Result<(), String> {
    if !document.is_prefab_document() {
        return Ok(());
    }
    let mut prefab = document
        .prefab_asset()
        .map_err(|error| format!("读取预制体文档失败：{error}"))?;
    if direct_nested_roots(&prefab).is_empty() && prefab.nested_stamps().is_empty() {
        return Ok(());
    }
    prepare_nested_lineage(&mut prefab, project_root)?;
    document
        .scene
        .metadata
        .retain(|key, _| !is_nested_lineage_key(key));
    for (key, value) in prefab
        .metadata
        .iter()
        .filter(|(key, _)| is_nested_lineage_key(key))
    {
        document.scene.metadata.insert(key.clone(), value.clone());
    }
    Ok(())
}

/// 显式同步：新快照 = 新内层 + 旧记录，保外层编号与父子关系。
pub(crate) fn bake_nested_from_inner(
    outer: &mut Prefab,
    inner: &Prefab,
    source: &str,
    stamp: &str,
) -> Result<NestedSyncOutcome, String> {
    let records = outer
        .nested_overrides()
        .map_err(|error| format!("嵌套覆盖记录损坏：{error}"))?;
    let roots: Vec<SceneEntityId> = direct_nested_roots(outer)
        .into_iter()
        .filter(|(_, root_source)| root_source == source)
        .map(|(id, _)| id)
        .collect();
    if roots.is_empty() {
        return Err(format!("外层文件里没有内层预制体 {source} 的实例"));
    }
    let mut outcome = NestedSyncOutcome::default();
    for outer_root in roots {
        let inner_root = nested_root_inner_id(outer, outer_root)?;
        let pairs = pair_subtrees(outer, outer_root, inner, inner_root)?;
        let mut staged = Scene::new(outer.name.clone());
        for (inner_id, outer_id) in &pairs {
            let inner_entity = inner
                .entities
                .get(inner_id)
                .ok_or_else(|| format!("内层预制体缺少实体 {}", inner_id.0))?;
            let old = outer
                .entities
                .get(outer_id)
                .ok_or_else(|| format!("外层文件缺少实体 {}", outer_id.0))?;
            let mut entity = inner_entity.clone();
            entity.id = *outer_id;
            entity.parent = old.parent;
            entity.children = old.children.clone();
            entity.metadata.remove(PREFAB_OUTER_ID_META_KEY);
            if !inner_entity.metadata.contains_key(PREFAB_SOURCE_META_KEY) {
                // 内层自己的节点：在外层文件里盖上"我属于这张内层"的身份。
                // 更深嵌套的节点保留它自己的标记（它指向更内层文件）。
                entity
                    .metadata
                    .insert(PREFAB_SOURCE_META_KEY.to_owned(), source.to_owned());
                entity
                    .metadata
                    .insert("prefab.source_id".to_owned(), inner_id.0.to_string());
                entity.metadata.insert(
                    "prefab.root".to_owned(),
                    (*outer_id == outer_root).to_string(),
                );
            }
            staged.entities.insert(*outer_id, entity);
        }
        let inner_to_outer = pairs.clone();
        let mut overrides = BTreeMap::new();
        for ((outer_id, field), value) in &records {
            let Some(inner_id) = pairs
                .iter()
                .find(|(_, mapped)| *mapped == outer_id)
                .map(|(inner_id, _)| *inner_id)
            else {
                continue;
            };
            overrides.insert((inner_id, field.clone()), value.clone());
            outcome.overrides += 1;
        }
        PrefabInstance {
            prefab_id: inner.asset_id,
            instance_root: outer_root,
            source_to_instance: inner_to_outer,
            overrides,
        }
        .apply_overrides(&mut staged)
        .map_err(|error| format!("嵌套覆盖记录无法套到新的 {source}：{error}"))?;
        for (id, entity) in staged.entities {
            outer.entities.insert(id, entity);
        }
        outcome.instances += 1;
    }
    outer
        .set_nested_stamp(source, Some(stamp))
        .map_err(|error| error.to_string())?;
    Ok(outcome)
}

impl EditorApp {
    /// 菜单门禁：当前主选是不是一个可直接同步的嵌套实例根。
    pub(crate) fn nested_sync_available(&self) -> bool {
        let Some(selected) = self.selection.primary() else {
            return false;
        };
        let Some(document) = self.document.current_scene.as_ref() else {
            return false;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        document
            .scene
            .entities
            .get(&selected)
            .and_then(|entity| document_nested_root_source(document, entity, &root))
            .is_some()
    }

    /// 「从内层预制体同步」：按当前选中的嵌套实例根判断是场景实例还是预制体文档。
    pub(crate) fn sync_nested_prefab_from_inner(
        &mut self,
        selected: SceneEntityId,
    ) -> Result<(), String> {
        if self.is_playing() {
            return Err("播放模式下不能同步嵌套预制体，请先点击停止".to_owned());
        }
        let Some(document) = self.document.current_scene.as_ref() else {
            return Err("没有打开的文档".to_owned());
        };
        if document.is_prefab_document() {
            self.sync_nested_in_prefab_document(selected)
        } else {
            self.sync_nested_scene_instance(selected)
        }
    }

    /// 打开的是外层预制体文档：就地重烘内层子树，进一条 Undo，等 Ctrl+S 落盘。
    fn sync_nested_in_prefab_document(&mut self, selected: SceneEntityId) -> Result<(), String> {
        let root = strip_verbatim_prefix(&self.project.root).clone();
        let Some(document) = self.document.current_scene.as_ref() else {
            return Err("没有打开的文档".to_owned());
        };
        let outer_source = prefab_document_source(document, &root)
            .ok_or_else(|| "无法确定当前预制体文档的工程内路径".to_owned())?;
        let Some(inner_source) = document
            .scene
            .entities
            .get(&selected)
            .and_then(|entity| file_nested_root_source(entity, &outer_source))
        else {
            return Err("请先选中一个嵌套预制体实例根".to_owned());
        };
        let inner_path = root.join(&inner_source);
        let inner = Prefab::load(&inner_path)
            .map_err(|error| format!("加载内层预制体 {inner_source} 失败：{error}"))?;
        let stamp = FileSignature::stamp_of(&inner_path)
            .ok_or_else(|| format!("无法读取内层预制体签名：{inner_source}"))?;
        let mut outer = document
            .prefab_asset()
            .map_err(|error| format!("读取当前预制体文档失败：{error}"))?;
        let previous = outer.nested_stamp(&inner_source).map(str::to_owned);
        if previous.as_deref() == Some(stamp.as_str()) {
            self.status = format!("内层预制体 {inner_source} 没有新变化");
            return Ok(());
        }
        let legacy = previous.is_none();
        let affected: BTreeSet<SceneEntityId> = direct_nested_roots(&outer)
            .into_iter()
            .filter(|(_, source)| *source == inner_source)
            .flat_map(|(id, _)| nested_subtree(&outer, id))
            .collect();
        let outcome = bake_nested_from_inner(&mut outer, &inner, &inner_source, &stamp)?;
        let entities: BTreeMap<SceneEntityId, SceneEntity> = affected
            .into_iter()
            .filter_map(|id| outer.entities.get(&id).cloned().map(|entity| (id, entity)))
            .collect();
        let lineage: BTreeMap<String, String> = outer
            .metadata
            .iter()
            .filter(|(key, _)| is_nested_lineage_key(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let Some(document) = self.document.current_scene.as_mut() else {
            return Err("没有打开的文档".to_owned());
        };
        let label = format!("从内层同步预制体：{inner_source}");
        self.history
            .execute(
                document,
                label.as_str(),
                CommandKind::Property,
                move |document| {
                    for (id, entity) in entities {
                        document.scene.entities.insert(id, entity);
                    }
                    document
                        .scene
                        .metadata
                        .retain(|key, _| !is_nested_lineage_key(key));
                    document.scene.metadata.extend(lineage);
                    document.mark_dirty();
                    Ok::<(), ()>(())
                },
            )
            .map_err(|()| "同步嵌套预制体失败".to_owned())?;
        self.on_document_changed(DocumentChange::PrefabApply);
        self.status = nested_sync_status(&inner_source, &outcome, legacy, "Ctrl+S 保存");
        Ok(())
    }

    /// 选中场景里某个外层实例中的嵌套节点：更新外层文件并重建外层实例（一条 Undo）。
    fn sync_nested_scene_instance(&mut self, selected: SceneEntityId) -> Result<(), String> {
        let root = strip_verbatim_prefix(&self.project.root).clone();
        let Some(document) = self.current_scene() else {
            return Err("没有打开的场景".to_owned());
        };
        let Some(inner_source) = document
            .scene
            .entities
            .get(&selected)
            .and_then(scene_nested_root_source)
        else {
            return Err("请先选中一个嵌套预制体实例根".to_owned());
        };
        let Some(outer_source) = enclosing_prefab_source(&document.scene, selected) else {
            return Err("选中的实体不在外层预制体实例里".to_owned());
        };
        let Some(direct_root) =
            enclosing_direct_instance_root(&document.scene, selected, &outer_source)
        else {
            return Err(format!("找不到 {outer_source} 的场景实例根"));
        };
        let outer_path = root.join(&outer_source);
        let old_outer =
            Prefab::load(&outer_path).map_err(|error| format!("加载外层预制体失败：{error}"))?;
        if let Some(reason) = variant_write_back_refusal(&old_outer) {
            return Err(reason);
        }
        let inner_path = root.join(&inner_source);
        let inner = Prefab::load(&inner_path)
            .map_err(|error| format!("加载内层预制体 {inner_source} 失败：{error}"))?;
        let stamp = FileSignature::stamp_of(&inner_path)
            .ok_or_else(|| format!("无法读取内层预制体签名：{inner_source}"))?;
        let previous = old_outer.nested_stamp(&inner_source).map(str::to_owned);
        if previous.as_deref() == Some(stamp.as_str()) {
            self.status = format!("内层预制体 {inner_source} 没有新变化");
            return Ok(());
        }
        let legacy = previous.is_none();
        let mut new_outer = old_outer.clone();
        let outcome = bake_nested_from_inner(&mut new_outer, &inner, &inner_source, &stamp)?;

        let cell = std::rc::Rc::new(std::cell::Cell::new(None));
        let selected_cell = cell.clone();
        let Some(document) = self.document.current_scene.as_mut() else {
            return Err("没有打开的场景".to_owned());
        };
        Self::write_prefab_and_rebuild_instances(
            &mut self.history,
            document,
            PrefabWriteRequest {
                source: &outer_source,
                path: &outer_path,
                prefab: new_outer,
                selected: direct_root,
                label: "从内层同步预制体",
                baseline: Some(&old_outer),
            },
            &selected_cell,
        )
        .map_err(|error| format!("更新外层实例失败：{error}"))?;
        self.refresh_resources();
        if let Some(new_root) = cell.get() {
            self.selection
                .set_single(new_root, editor::SelectionSource::Command);
            self.on_document_changed(DocumentChange::PrefabApply);
        }
        self.status = nested_sync_status(&inner_source, &outcome, legacy, "已写回外层并更新实例");
        Ok(())
    }
}

fn nested_sync_status(
    inner_source: &str,
    outcome: &NestedSyncOutcome,
    legacy: bool,
    tail: &str,
) -> String {
    let mut status = format!(
        "已从内层预制体 {inner_source} 同步 {} 个实例（保留 {} 个字段覆盖），{tail}",
        outcome.instances, outcome.overrides
    );
    if legacy {
        status
            .push_str("。旧外层文件没有内层基线，本次按内层内容整体重建，外层自定义字段请重新调整");
    }
    status
}

/// 预制体文档里一个**本层**嵌套实例根的来源：有 `prefab.root=true`、来源不是本文件。
fn file_nested_root_source(entity: &SceneEntity, outer_source: &str) -> Option<String> {
    let source = entity.metadata.get(PREFAB_SOURCE_META_KEY)?;
    (entity.metadata.get("prefab.root").map(String::as_str) == Some("true")
        && source != outer_source
        && !source.trim().is_empty())
    .then(|| source.clone())
}

/// 场景里一个**本层**嵌套实例根的来源：还带 `prefab.outer_id`（外层文件里的位置）。
fn scene_nested_root_source(entity: &SceneEntity) -> Option<String> {
    let source = entity.metadata.get(PREFAB_SOURCE_META_KEY)?;
    (entity.metadata.get("prefab.root").map(String::as_str) == Some("true")
        && entity.metadata.contains_key(PREFAB_OUTER_ID_META_KEY)
        && !source.trim().is_empty())
    .then(|| source.clone())
}

/// 从嵌套节点往上找"直接实例化在场景里的外层根"（没有 `prefab.outer_id` 的那个）。
fn enclosing_direct_instance_root(
    scene: &Scene,
    entity: SceneEntityId,
    outer_source: &str,
) -> Option<SceneEntityId> {
    let mut current = scene.entities.get(&entity)?.parent;
    while let Some(id) = current {
        let node = scene.entities.get(&id)?;
        if node
            .metadata
            .get(PREFAB_SOURCE_META_KEY)
            .map(String::as_str)
            == Some(outer_source)
            && node.metadata.get("prefab.root").map(String::as_str) == Some("true")
            && !node.metadata.contains_key(PREFAB_OUTER_ID_META_KEY)
        {
            return Some(id);
        }
        current = node.parent;
    }
    None
}

/// 当前文档里一个可同步的嵌套实例根来源。
fn document_nested_root_source(
    document: &editor::SceneDocument,
    entity: &SceneEntity,
    project_root: &Path,
) -> Option<String> {
    if document.is_prefab_document() {
        file_nested_root_source(entity, &prefab_document_source(document, project_root)?)
    } else {
        scene_nested_root_source(entity)
    }
}

/// 预制体文档的工程内相对路径（`.prefab` 文件名与 `prefab.source` 同一口径）。
fn prefab_document_source(document: &editor::SceneDocument, project_root: &Path) -> Option<String> {
    let path = strip_verbatim_prefix(document.path.as_deref()?);
    let relative = path.strip_prefix(project_root).ok()?;
    editor::normalize_relative(relative).map(|path| path.to_string_lossy().replace('\\', "/"))
}
