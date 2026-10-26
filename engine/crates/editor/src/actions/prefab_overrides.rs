//! M79 P2：预制体实例**整棵子树**的覆盖聚合与就地还原。
//!
//! 逐条覆盖视图（"选中哪个节点就看那个节点的覆盖"）在 docs/55 阶段 3 就有了
//! （`panels::inspector::prefab_override_summary`）。它买不到的是 Unity 在实例根上给的那一眼：
//! **"这个实例到底被改了什么"**——改的多是深层子节点，逐节点点开找等于把层级树重走一遍。
//!
//! 于是这里做三件事：
//!
//! * [`collect_instance_overrides`]：从实例根往下走一遍，按"节点路径 + 字段"给出**全部字段级覆盖**
//!   （含源/实例两侧取值，纯只读、不碰文档）；走不完的部分（源里已没有对应实体的节点、
//!   中途混进来的另一张预制体的节点）以 [`InstanceOverrideNote`] 单独列出，**不假装能还原**；
//! * [`revert_prefab_field_from_source`]：把"一个字段回到源取值"这套写法语义收在**一处**
//!   （原先内联在 `prefab_ops::revert_prefab_override_field` 里，聚合视图再抄一遍就是第三份）；
//! * 每帧成本：[`EditorApp::prefab_instance_overrides`] 带缓存，命中时是一次 `fs::metadata`
//!   （源预制体那侧本来就要 stat，见 `prefab_stamp`）加四次整数比较，不是整棵子树的 diff。
//!
//! 与既有那条「还原覆盖」（`RevertPrefabOverrides`：删掉子树后按空覆盖重新实例化）的区别是刻意保留的：
//! 那条顺带能摆平结构性差异，代价是实例里**所有实体的 id 重排**（场景里任何指向它们的引用都可能断），
//! 而它在历史里是整文档快照形态（结构变更压不动，见 docs/25 §24）。本批新增的「逐项还原全部字段覆盖」
//! 不换 id，且是一条 O(改动) 的补丁历史。

use super::prefab_rebuild::collect_scene_subtree;
use super::*;

/// 整实例覆盖列表的一行（显示与逐条还原共用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstanceOverrideRow {
    /// 场景里那个实例实体的 id（逐条还原与"选中该节点"都用它）。
    pub(crate) entity: SceneEntityId,
    /// 源预制体里的对应实体 id。
    pub(crate) source: SceneEntityId,
    /// 从实例根数下来的名字路径（`Root/Body/Head`）；名字重复时不唯一，只作显示。
    pub(crate) entity_label: String,
    /// 覆盖字段串（`name` / `component:X` / `property:Comp.field` / `metadata:X`）。
    pub(crate) field: String,
    /// 源侧取值（还原后要变成的样子）。
    pub(crate) source_value: String,
    /// 实例侧取值（现在这个样子）。
    pub(crate) instance_value: String,
}

/// 一条"看得见但还原不了"的差异。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstanceOverrideNote {
    /// 实例里有这个节点，源预制体里没有对应的实体（源删过节点，或这是手工加进去的子节点）。
    SourceEntityMissing {
        entity: SceneEntityId,
        label: String,
    },
    /// 子树里混进了**另一张**预制体的节点（嵌套预制体后置，docs/25 §23），本列表不展开它。
    ForeignPrefabNode {
        entity: SceneEntityId,
        label: String,
    },
}

impl InstanceOverrideNote {
    pub(crate) fn display(&self) -> String {
        match self {
            Self::SourceEntityMissing { label, .. } => {
                format!("{label}：源预制体里没有对应实体（新增的子节点，无法按字段还原）")
            }
            Self::ForeignPrefabNode { label, .. } => {
                format!("{label}：属于另一张预制体，本列表不展开")
            }
        }
    }
}

/// 一次收集的结果：可还原的字段覆盖 + 还原不了的结构性说明。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InstanceOverrides {
    pub(crate) rows: Vec<InstanceOverrideRow>,
    pub(crate) notes: Vec<InstanceOverrideNote>,
}

impl InstanceOverrides {
    /// 覆盖涉及了多少个节点（列表标题用："N 项 / M 个节点"）。
    pub(crate) fn entity_count(&self) -> usize {
        self.rows
            .iter()
            .map(|row| row.entity)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }
}

/// 从实例根收集**整棵子树**的字段级覆盖（纯函数：只吃显式传入的场景与源预制体）。
pub(crate) fn collect_instance_overrides(
    scene: &rustlua_scene::scene::Scene,
    instance_root: SceneEntityId,
    prefab_source: &str,
    prefab: &Prefab,
) -> Result<InstanceOverrides, String> {
    // 先借重建路径的同一条遍历拿到"子树里有哪些节点"（它带循环检测），
    // 之后按父子顺序再走一遍只为生成可读路径。
    let subtree = collect_scene_subtree(scene, instance_root)?;
    let members = subtree
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let mut out = InstanceOverrides::default();
    let mut visited = std::collections::BTreeSet::new();
    let mut stack = vec![(instance_root, String::new())];
    while let Some((id, prefix)) = stack.pop() {
        if !visited.insert(id) || !members.contains(&id) {
            continue;
        }
        let entity = scene
            .entities
            .get(&id)
            .ok_or_else(|| format!("场景中不存在实体 {}", id.0))?;
        let label = if prefix.is_empty() {
            entity.name.clone()
        } else {
            format!("{prefix}/{}", entity.name)
        };
        if entity
            .metadata
            .get("prefab.source")
            .is_some_and(|value| value != prefab_source)
        {
            // 另一张预制体的根：不展开它的子树，也不把它的字段算进这张实例。
            out.notes
                .push(InstanceOverrideNote::ForeignPrefabNode { entity: id, label });
            continue;
        }
        let source_id = entity
            .metadata
            .get("prefab.source_id")
            .and_then(|value| value.parse::<u64>().ok())
            .map(SceneEntityId);
        let Some(source_entity) = source_id.and_then(|sid| prefab.entities.get(&sid)).cloned()
        else {
            out.notes.push(InstanceOverrideNote::SourceEntityMissing {
                entity: id,
                label: label.clone(),
            });
            // 源里都没有对应实体，它下面的子树同样无法逐字段对拍：整段跳过。
            continue;
        };
        for field in diff_prefab_entity(&source_entity, entity) {
            out.rows.push(InstanceOverrideRow {
                entity: id,
                source: source_id.unwrap_or(SceneEntityId(0)),
                entity_label: label.clone(),
                source_value: crate::panels::inspector::prefab_override_side_value(
                    &source_entity,
                    &field,
                ),
                instance_value: crate::panels::inspector::prefab_override_side_value(
                    entity, &field,
                ),
                field,
            });
        }
        // 逆序压栈，弹出后即为 children 的正序。
        for child in entity.children.iter().rev() {
            if !visited.contains(child) {
                stack.push((*child, label.clone()));
            }
        }
    }
    out.rows.sort_by(|left, right| {
        (&left.entity_label, &left.field).cmp(&(&right.entity_label, &right.field))
    });
    Ok(out)
}

/// 把**一个字段**还原为源预制体的取值（`name` / `component:` / `property:` / `metadata:`）。
///
/// 这是"还原覆盖"唯一的写法语义：单条（`prefab_ops::revert_prefab_override_field`）与
/// 整实例批量（`prefab_ops::revert_all_prefab_field_overrides`）都调它，
/// 避免两处各抄一遍 `property_path` 的最小改写规则。
pub(crate) fn revert_prefab_field_from_source(
    entity: &mut SceneEntity,
    field: &str,
    source: &SceneEntity,
) -> Result<(), String> {
    if field == "name" {
        entity.name = source.name.clone();
        return Ok(());
    }
    if let Some(key) = field.strip_prefix("component:") {
        let payload = source
            .components
            .get(key)
            .cloned()
            .ok_or_else(|| format!("源预制体缺少组件 {key}"))?;
        entity.components.insert(key.to_owned(), payload);
        return Ok(());
    }
    if let Some(raw_path) = field.strip_prefix("property:") {
        let path = rustlua_ecs::property_path::PropertyPath::parse(raw_path)
            .map_err(|error| error.to_string())?;
        let source_payload = source
            .components
            .get(&path.component)
            .ok_or_else(|| format!("源预制体缺少组件 {}", path.component))?;
        let value = rustlua_ecs::property_path::read_field_text(source_payload, &path)
            .map_err(|error| error.to_string())?;
        let current = entity
            .components
            .get(&path.component)
            .cloned()
            .ok_or_else(|| format!("实例缺少组件 {}", path.component))?;
        let updated = rustlua_ecs::property_path::write_builtin(&current, &path, &value)
            .map_err(|error| error.to_string())?;
        entity.components.insert(path.component.clone(), updated);
        return Ok(());
    }
    if let Some(key) = field.strip_prefix("metadata:") {
        match source.metadata.get(key) {
            Some(value) => {
                entity.metadata.insert(key.to_owned(), value.clone());
            }
            None => {
                entity.metadata.remove(key);
            }
        }
        return Ok(());
    }
    Err(format!("不支持的预制体覆盖字段 {field}"))
}

/// 整实例覆盖列表的缓存键。
///
/// 这四个整数就是"这份列表还可能因为什么变"的完整答案：
///
/// * `revision`——文档内容变（`mark_dirty` 递增，网关里每次编辑都会调它）；
/// * `(edits, depth)`——与 §22 全局账本**同一口径**的历史签名：撤销/重做不动 `revision`
///   （撤销不标脏），但一定动深度；换文档或换实例时 `edits` 归零或选中变化；
/// * `generation`——磁盘上的源预制体被重载过（别的会话存了源、in-context 阶段保存写回磁盘），
///   此时文档一个字没改，但"源取值"变了；
/// * `entity`——选中换了节点。
///
/// 键相同就直接复用。**查磁盘（`prefab_stamp`）排在比键之前**：这样"文档一个字没改、源被别的
/// 会话存了盘"也能被 `generation` 抓到。代价是每帧一次 `fs::metadata`，换掉的是整棵子树的 diff。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrefabOverrideCacheKey {
    pub(crate) revision: u64,
    pub(crate) edits: u64,
    pub(crate) depth: usize,
    pub(crate) generation: u64,
    pub(crate) entity: SceneEntityId,
}

/// 缓存条目：键 + 上次算出来的列表。
#[derive(Clone, Debug)]
pub(crate) struct PrefabOverrideCache {
    pub(crate) key: PrefabOverrideCacheKey,
    pub(crate) overrides: InstanceOverrides,
}

impl EditorApp {
    /// 选中节点所属的**实例根**（沿 `parent` 往上找第一个 `prefab.root=true`）。
    ///
    /// 不是预制体实例则 `None`。带循环保护（脏数据里 parent 成环不会把这一帧挂住）。
    pub(crate) fn prefab_instance_root(&self, selected: SceneEntityId) -> Option<SceneEntityId> {
        let scene = self.current_scene()?;
        let mut cursor = Some(selected);
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = cursor {
            if !seen.insert(id) {
                return None;
            }
            let entity = scene.scene.entities.get(&id)?;
            if entity
                .metadata
                .get("prefab.root")
                .is_some_and(|value| value == "true")
            {
                return Some(id);
            }
            cursor = entity.parent;
        }
        None
    }

    /// 整棵实例的覆盖列表（带缓存；面板每帧调它）。
    ///
    /// * `None`——选中节点不在预制体实例里（面板因此不渲染这一段）；
    /// * `Some(Err(..))`——源预制体读不出来，或子树里有指向不存在实体的坏数据。
    pub(crate) fn prefab_instance_overrides(
        &mut self,
        selected: SceneEntityId,
    ) -> Option<Result<InstanceOverrides, String>> {
        let root = self.prefab_instance_root(selected)?;
        let prefab_source = {
            let document = self.document.current_scene.as_ref()?;
            let entity = document.scene.entities.get(&root)?;
            entity.metadata.get("prefab.source")?.clone()
        };
        // 先查磁盘（一次 `fs::metadata`，不重读文件内容），再算缓存键：
        // 顺序反过来会让"源被外部改了"永远命中旧缓存——键里的代际只有在真重载时才变。
        let path = strip_verbatim_prefix(&self.project.root).join(&prefab_source);
        if let Err(error) = self.ensure_prefab_source_cached(&path) {
            return Some(Err(error));
        }
        let key = {
            let document = self.document.current_scene.as_ref()?;
            let (edits, depth) = self.history.undo_signature();
            PrefabOverrideCacheKey {
                revision: document.revision,
                edits,
                depth,
                generation: self.prefab_source_generation,
                entity: root,
            }
        };
        if let Some(cached) = &self.prefab_override_cache
            && cached.key == key
        {
            return Some(Ok(cached.overrides.clone()));
        }
        let prefab = self
            .prefab_source_cache
            .get(&path)
            .map(|cached| cached.prefab.clone())?;
        let document = self.document.current_scene.as_ref()?;
        match collect_instance_overrides(&document.scene, root, &prefab_source, &prefab) {
            Ok(overrides) => {
                self.prefab_override_cache = Some(PrefabOverrideCache {
                    key,
                    overrides: overrides.clone(),
                });
                Some(Ok(overrides))
            }
            Err(error) => Some(Err(error)),
        }
    }

    /// 让整实例覆盖缓存作废（结构性写路径与换文档处调用；其余靠缓存键自动失效）。
    pub(crate) fn invalidate_prefab_override_cache(&mut self) {
        self.prefab_override_cache = None;
    }
}
