//! 预设重建的数据收集：从实例根回溯源文件、收集覆盖值、生成新预设、
//! 并把实例元数据写回场景。纯数据搬运，不直接改文档。

use rustlua_scene::prefab::{PREFAB_OUTER_ID_META_KEY, PREFAB_SOURCE_META_KEY};

use super::*;

/// 实例节点所属的**内层**预制体来源（嵌套节点才有；外层自己的节点返回 `None`）。
pub(crate) fn nested_instance_source<'e>(
    entity: &'e SceneEntity,
    outer_source: &str,
) -> Option<&'e str> {
    entity
        .metadata
        .get(PREFAB_SOURCE_META_KEY)
        .map(String::as_str)
        .filter(|source| !source.trim().is_empty() && *source != outer_source)
}

/// 场景里的实例节点对应**外层**预制体文件里的哪个实体编号。
///
/// * 外层自己的节点：`prefab.source` 就是外层来源，编号即 `prefab.source_id`；
/// * 嵌套实例的节点：`prefab.source` 指的是内层文件，它在外层文件里的位置另有其键
///   （`prefab.outer_id`，实例化时由 [`apply_prefab_instance_metadata`] 盖章）；
/// * 两者都对不上（手工挂进来的节点）→ `None`，调用方必须拒绝而不是猜一个编号。
pub(crate) fn envelope_entity_key(
    entity: &SceneEntity,
    outer_source: &str,
) -> Option<SceneEntityId> {
    let source = entity.metadata.get(PREFAB_SOURCE_META_KEY)?;
    let key = if source == outer_source {
        "prefab.source_id"
    } else {
        PREFAB_OUTER_ID_META_KEY
    };
    entity
        .metadata
        .get(key)?
        .parse::<u64>()
        .ok()
        .map(SceneEntityId)
}

/// 重建一个 预制体实例所需的全部信息（写回/重新应用共用）。
///
/// 写路径辅助是**纯函数**：只吃显式传入的场景/Prefab，不持有 `EditorApp`
/// （面板棘轮扫描的是 `self.document` / `self.history` 这类直写）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrefabRebuildState {
    pub(crate) instance_root: SceneEntityId,
    pub(crate) parent: Option<SceneEntityId>,
    pub(crate) instance_entities: Vec<SceneEntityId>,
    pub(crate) source_to_instance: std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
    pub(crate) overrides: std::collections::BTreeMap<(SceneEntityId, String), String>,
}

pub(crate) fn collect_prefab_rebuild_state(
    scene: &rustlua_scene::scene::Scene,
    instance_root: SceneEntityId,
    prefab_source: &str,
    prefab: &Prefab,
) -> Result<PrefabRebuildState, String> {
    collect_prefab_rebuild_state_with_baseline(scene, instance_root, prefab_source, prefab, None)
}

/// 同一条收集路径，但**覆盖值按 `baseline` diff**（结构与实例校验始终按 `prefab`）。
///
/// in-context 编辑必须走这里：阶段里改的是**源**，如果拿"改过的新源"去 diff 实例，
/// 刚被改掉的那个字段会因为"实例还留着老值"而被判成实例自己的覆盖，重建时再按回去
/// ——源改动被实例的老值顶回，等于同步失败（这条正是回归门抓出来的）。
/// 用进入阶段时那份源当基线，覆盖集合才是"实例与源真正的不同"。
pub(crate) fn collect_prefab_rebuild_state_with_baseline(
    scene: &rustlua_scene::scene::Scene,
    instance_root: SceneEntityId,
    prefab_source: &str,
    prefab: &Prefab,
    baseline: Option<&Prefab>,
) -> Result<PrefabRebuildState, String> {
    let root = scene
        .entities
        .get(&instance_root)
        .ok_or_else(|| format!("场景中不存在预制体根实例 {}", instance_root.0))?;
    let instance_entities = collect_scene_subtree(scene, instance_root)?;
    let mut source_to_instance = std::collections::BTreeMap::new();
    for entity_id in &instance_entities {
        let entity = scene
            .entities
            .get(entity_id)
            .ok_or_else(|| format!("场景中不存在实体 {}", entity_id.0))?;
        let inner_source = nested_instance_source(entity, prefab_source);
        let Some(source_id) = envelope_entity_key(entity, prefab_source) else {
            return Err(if entity.metadata.contains_key(PREFAB_SOURCE_META_KEY) {
                format!(
                    "嵌套预制体节点 {} 没有本预制体的实体编号（{PREFAB_OUTER_ID_META_KEY} 缺失或非法），\
                     请用「解包预制体」后重新创建预制体",
                    entity_id.0
                )
            } else {
                "预制体实例子树包含非同源节点，暂不支持自动重建".to_owned()
            });
        };
        if let Some(inner) = inner_source {
            // 嵌套节点在外层文件里也必须被记成**同一张**内层预制体的同一个节点。
            // 对不上就是"手工挂进来的别的预制体"：外层文件根本不知道它，重建会静默丢掉
            // 整棵子树——宁可拒绝，也不写坏数据。
            let file_entity = prefab.entities.get(&source_id).ok_or_else(|| {
                format!(
                    "源预制体中缺少实体 {}（嵌套节点 {} 属于它）",
                    source_id.0, entity_id.0
                )
            })?;
            if file_entity
                .metadata
                .get(PREFAB_SOURCE_META_KEY)
                .map(String::as_str)
                != Some(inner)
                || file_entity.metadata.get("prefab.source_id")
                    != entity.metadata.get("prefab.source_id")
            {
                return Err(format!(
                    "实体 {} 的嵌套来源（{inner}）与源预制体里实体 {} 记录的不一致，暂不支持自动重建",
                    entity_id.0, source_id.0
                ));
            }
        }
        if source_to_instance.insert(source_id, *entity_id).is_some() {
            return Err(format!("预制体实例中出现重复的源实体 {}", source_id.0));
        }
    }
    let prefab_ids = prefab
        .entities
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let instance_ids = source_to_instance
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if prefab_ids != instance_ids {
        return Err("预制体实例子树和源预制体结构不一致，暂不支持自动重建".to_owned());
    }
    let overrides =
        collect_prefab_overrides(scene, baseline.unwrap_or(prefab), &source_to_instance)?;
    Ok(PrefabRebuildState {
        instance_root,
        parent: root.parent,
        instance_entities,
        source_to_instance,
        overrides,
    })
}

/// 由场景里的实例根生成**新的源预制体**，并一并交回"场景节点 → 新文件实体编号"。
///
/// 调用方需要那张映射表是因为写回是两步：先按新源重建场景里的实例，再落盘。而"刚拖进外层
/// 分支、还没有 `prefab.outer_id`"的嵌套节点只有在这里才第一次拿到位置编号——不把它盖回
/// 场景节点，紧接着的重建收集会因对不上号而拒掉整次写回（见 [`stamp_envelope_ids`]）。
pub(crate) fn build_prefab_from_instance_root(
    scene: &rustlua_scene::scene::Scene,
    instance_root: SceneEntityId,
    existing_prefab: &Prefab,
    outer_source: &str,
) -> Result<
    (
        Prefab,
        std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
    ),
    String,
> {
    let mut prefab = Prefab::from_scene(
        scene,
        instance_root,
        existing_prefab.asset_id,
        existing_prefab.name.clone(),
    )
    .map_err(|error| error.to_string())?;
    // 场景节点 → 外层文件实体编号。外层自己的节点沿用它原来的编号（场景里**其它**同源实例
    // 的 `prefab.source_id` 因此继续指得对）；嵌套节点沿用它的 `prefab.outer_id`，
    // 第一次被挂进来的嵌套实例还没有外层编号，按子树顺序分配一个不冲突的新编号——
    // 否则"往预制体里挂一张预制体再写回"这条作者流程会被卡死。
    let mut source_ids = std::collections::BTreeMap::new();
    let mut used = prefab
        .entities
        .values()
        .filter_map(|entity| envelope_entity_key(entity, outer_source))
        .collect::<std::collections::BTreeSet<_>>();
    let mut next = used
        .iter()
        .map(|id| id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    for entity_id in collect_scene_subtree_from_prefab(&prefab, instance_root) {
        let scene_entity = scene
            .entities
            .get(&entity_id)
            .ok_or_else(|| format!("场景中不存在实体 {}", entity_id.0))?;
        let key = match envelope_entity_key(scene_entity, outer_source) {
            Some(key) => key,
            None if entity_id == instance_root => {
                // 文件根属于这张预制体自己（`Prefab::from_scene` 的那条不变量）。
                // 手工或历史数据里它可能还挂着内层标记，这里按外层身份重新认它，
                // 否则这张文件永远写不回去。
                scene_entity
                    .metadata
                    .get("prefab.source_id")
                    .and_then(|value| value.parse::<u64>().ok())
                    .map(SceneEntityId)
                    .ok_or_else(|| {
                        format!("实例根实体 {} 缺少可用的 prefab.source_id", entity_id.0)
                    })?
            }
            None if nested_instance_source(scene_entity, outer_source).is_some() => {
                // 第一次被挂进本预制体的嵌套实例：给它分配一个没被占用的外层编号。
                let mut candidate = next;
                while !used.insert(SceneEntityId(candidate)) {
                    candidate = candidate.checked_add(1).ok_or("预制体实体编号已用尽")?;
                }
                next = candidate + 1;
                SceneEntityId(candidate)
            }
            None => {
                return Err(format!(
                    "实体 {} 既不是本预制体的节点（缺 prefab.source_id），也不是嵌套实例的节点",
                    entity_id.0
                ));
            }
        };
        if source_ids.insert(entity_id, key).is_some() {
            return Err(format!("实例实体 {} 出现重复映射", entity_id.0));
        }
    }
    let root = *source_ids
        .get(&prefab.root)
        .ok_or_else(|| format!("实例根实体 {} 缺少 source_id 映射", prefab.root.0))?;
    let mut remapped = std::collections::BTreeMap::new();
    for (_, mut entity) in prefab.entities {
        let source_id = *source_ids
            .get(&entity.id)
            .ok_or_else(|| format!("实体 {} 缺少 source_id 映射", entity.id.0))?;
        entity.id = source_id;
        entity.parent = entity.parent.and_then(|id| source_ids.get(&id).copied());
        entity.children = entity
            .children
            .into_iter()
            .filter_map(|id| source_ids.get(&id).copied())
            .collect();
        if remapped.insert(source_id, entity).is_some() {
            return Err(format!("预制体写回时出现重复源实体 {}", source_id.0));
        }
    }
    prefab.root = root;
    prefab.entities = remapped;
    prefab.metadata = existing_prefab.metadata.clone();
    for entity in prefab.entities.values_mut() {
        // 只洗掉**外层自己的**标记：那三个键由实例化重新盖章，留在文件里反而是过期数据。
        // 嵌套节点的标记（内层来源、内层编号、内层实例根）就是它的身份证，必须逐字写回文件；
        // 它在外层文件里的位置也不用记——文件里那个实体的编号本身就是位置。
        // 文件根是个例外：它属于这张预制体自己（见 `Prefab::from_scene` 的不变量），
        // 手工数据里残留的内层标记在这里被清掉。
        let nested =
            entity.id != prefab.root && nested_instance_source(entity, outer_source).is_some();
        if nested {
            entity.metadata.remove(PREFAB_OUTER_ID_META_KEY);
        } else {
            entity.metadata.retain(|key, _| !key.starts_with("prefab."));
        }
    }
    Ok((prefab, source_ids))
}

/// 把 [`build_prefab_from_instance_root`] 分配给嵌套节点的外层编号盖回场景节点。
///
/// 只动**还没有** `prefab.outer_id` 的嵌套节点：已有的那个是它在外层文件里的原位置，改写它
/// 会让文件与场景对不上号。外层自己的节点一律不碰（那两个键由实例化重新盖章，不是外层位置）。
pub(crate) fn stamp_envelope_ids(
    scene: &mut rustlua_scene::scene::Scene,
    outer_source: &str,
    envelope_ids: &std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
) {
    for (entity_id, file_id) in envelope_ids {
        let Some(entity) = scene.entities.get_mut(entity_id) else {
            continue;
        };
        if entity.metadata.contains_key(PREFAB_OUTER_ID_META_KEY)
            || nested_instance_source(entity, outer_source).is_none()
        {
            continue;
        }
        entity
            .metadata
            .insert(PREFAB_OUTER_ID_META_KEY.to_owned(), file_id.0.to_string());
    }
}

/// 按**子树顺序**（根先、孩子正序）遍历新预制体的实体。
///
/// 为什么在这儿自己走一遍，而不是复用 [`collect_scene_subtree`]：`Prefab::from_scene` 会把根
/// 的 `parent` 断掉，两侧的孩子顺序一致，但给预制体造一份临时 `Scene` 只是为了让同一个函数
/// 换个输入类型——不如直接按 `entities` 走，省掉一次克隆。
fn collect_scene_subtree_from_prefab(prefab: &Prefab, root: SceneEntityId) -> Vec<SceneEntityId> {
    let mut out = Vec::new();
    let mut visiting = std::collections::BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !visiting.insert(id) {
            continue;
        }
        out.push(id);
        if let Some(entity) = prefab.entities.get(&id) {
            stack.extend(entity.children.iter().rev().copied());
        }
    }
    out
}

pub(crate) fn prefab_root_ids(
    scene: &rustlua_scene::scene::Scene,
    prefab_source: &str,
) -> Vec<SceneEntityId> {
    scene
        .entities
        .values()
        .filter(|entity| {
            entity
                .metadata
                .get("prefab.root")
                .is_some_and(|value| value == "true")
                && entity
                    .metadata
                    .get("prefab.source")
                    .is_some_and(|value| value == prefab_source)
                // **嵌在外层预制体里的内层实例根不算这张源的直接实例**：它的取值快照存在
                // 外层文件中，按内层新源重建会把它的 `prefab.outer_id` 洗掉（内层根会被重写成
                // 外层直接子节点），外层下次写回就对不上号。内层源改动同步的是**直接实例**；
                // 嵌套那一份由外层自己的写回路径负责。
                && !entity.metadata.contains_key(PREFAB_OUTER_ID_META_KEY)
        })
        .map(|entity| entity.id)
        .collect()
}

/// 这个实体被**嵌在哪张**预制体里：沿父链找到最近的"不属于内层"的预制体节点，返回外层来源。
///
/// 只有内层节点带 [`PREFAB_OUTER_ID_META_KEY`]，所以跳过带标记的节点就能穿过整棵内层子树，
/// 停在真正持有这份快照的外层文件上。非嵌套节点返回 `None`。
pub(crate) fn enclosing_prefab_source(
    scene: &rustlua_scene::scene::Scene,
    entity: SceneEntityId,
) -> Option<String> {
    let mut current = scene.entities.get(&entity)?.parent?;
    loop {
        let node = scene.entities.get(&current)?;
        if !node.metadata.contains_key(PREFAB_OUTER_ID_META_KEY)
            && let Some(source) = node.metadata.get(PREFAB_SOURCE_META_KEY)
        {
            return Some(source.clone());
        }
        current = node.parent?;
    }
}

pub(crate) fn collect_prefab_overrides(
    scene: &rustlua_scene::scene::Scene,
    prefab: &Prefab,
    source_to_instance: &std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
) -> Result<std::collections::BTreeMap<(SceneEntityId, String), String>, String> {
    let mut overrides = std::collections::BTreeMap::new();
    for (source_id, source_entity) in &prefab.entities {
        let instance_id = source_to_instance
            .get(source_id)
            .ok_or_else(|| format!("实例缺少源实体 {}", source_id.0))?;
        let instance_entity = scene
            .entities
            .get(instance_id)
            .ok_or_else(|| format!("场景中不存在实体 {}", instance_id.0))?;
        for field in diff_prefab_entity(source_entity, instance_entity) {
            let value = prefab_override_value(instance_entity, &field)?;
            overrides.insert((*source_id, field), value);
        }
    }
    Ok(overrides)
}

pub(crate) fn prefab_override_value(instance: &SceneEntity, field: &str) -> Result<String, String> {
    if field == "name" {
        Ok(instance.name.clone())
    } else if let Some(key) = field.strip_prefix("component:") {
        instance
            .components
            .get(key)
            .cloned()
            .ok_or_else(|| format!("实例缺少组件 {key}"))
    } else if let Some(path) = field.strip_prefix("property:") {
        // 逐字段覆盖：值取该字段（或元组分量）的**原始文本**，
        // 由 `PrefabInstance::apply_overrides` 用 property_path 做最小改写。
        // 取原始文本而不是规范化数值，避免把 `77` 写成 `77.000000`。
        let path = rustlua_ecs::property_path::PropertyPath::parse(path)
            .map_err(|error| format!("{field}: {error}"))?;
        let payload = instance
            .components
            .get(&path.component)
            .ok_or_else(|| format!("实例缺少组件 {}", path.component))?;
        rustlua_ecs::property_path::read_field_text(payload, &path)
            .map_err(|error| format!("{field}: {error}"))
    } else if let Some(key) = field.strip_prefix("metadata:") {
        instance
            .metadata
            .get(key)
            .cloned()
            .ok_or_else(|| format!("实例缺少元数据 {key}"))
    } else {
        Err(format!("不支持的预制体覆盖字段 {field}"))
    }
}

pub(crate) fn collect_scene_subtree(
    scene: &rustlua_scene::scene::Scene,
    root: SceneEntityId,
) -> Result<Vec<SceneEntityId>, String> {
    pub(crate) fn visit(
        scene: &rustlua_scene::scene::Scene,
        id: SceneEntityId,
        out: &mut Vec<SceneEntityId>,
        visiting: &mut std::collections::BTreeSet<SceneEntityId>,
    ) -> Result<(), String> {
        if !visiting.insert(id) {
            return Err(format!("实体 {} 的子树存在循环", id.0));
        }
        let entity = scene
            .entities
            .get(&id)
            .ok_or_else(|| format!("场景中不存在实体 {}", id.0))?;
        for child in &entity.children {
            visit(scene, *child, out, visiting)?;
        }
        out.push(id);
        Ok(())
    }

    let mut out = Vec::new();
    let mut visiting = std::collections::BTreeSet::new();
    visit(scene, root, &mut out, &mut visiting)?;
    Ok(out)
}

pub(crate) fn apply_prefab_instance_metadata(
    scene: &mut rustlua_scene::scene::Scene,
    prefab_source: &str,
    instance: &rustlua_scene::prefab::PrefabInstance,
) {
    for (source_id, entity_id) in &instance.source_to_instance {
        if let Some(entity) = scene.entities.get_mut(entity_id) {
            let is_instance_root = entity.id == instance.instance_root;
            if !is_instance_root && nested_instance_source(entity, prefab_source).is_some() {
                // 嵌套节点：文件里已经带了它自己的内层来源、内层编号与内层实例根标记，
                // 那三个键才是它的身份，外层一个都不许改（改了它就对不回内层文件了）。
                // 外层唯一要补的是"我在外层文件里是几号实体"。
                entity
                    .metadata
                    .insert(PREFAB_OUTER_ID_META_KEY.to_owned(), source_id.0.to_string());
                continue;
            }
            entity
                .metadata
                .insert(PREFAB_SOURCE_META_KEY.to_owned(), prefab_source.to_owned());
            entity
                .metadata
                .insert("prefab.source_id".to_owned(), source_id.0.to_string());
            entity.metadata.insert(
                "prefab.root".to_owned(),
                (entity.id == instance.instance_root).to_string(),
            );
            entity.metadata.remove(PREFAB_OUTER_ID_META_KEY);
        }
    }
}

pub(crate) fn preserve_prefab_editor_flags(
    scene: &mut editor::SceneDocument,
    old_map: &std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
    new_map: &std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
) {
    let mut hidden = std::collections::BTreeSet::new();
    let mut locked = std::collections::BTreeSet::new();
    for (source_id, instance_id) in old_map {
        let hidden_key = format!("editor.hidden.{}", instance_id.0);
        let locked_key = format!("editor.locked.{}", instance_id.0);
        if scene
            .scene
            .metadata
            .remove(&hidden_key)
            .is_some_and(|value| value == "true")
        {
            hidden.insert(*source_id);
        }
        if scene
            .scene
            .metadata
            .remove(&locked_key)
            .is_some_and(|value| value == "true")
        {
            locked.insert(*source_id);
        }
    }
    for source_id in hidden {
        if let Some(instance_id) = new_map.get(&source_id) {
            scene.scene.metadata.insert(
                format!("editor.hidden.{}", instance_id.0),
                "true".to_owned(),
            );
        }
    }
    for source_id in locked {
        if let Some(instance_id) = new_map.get(&source_id) {
            scene.scene.metadata.insert(
                format!("editor.locked.{}", instance_id.0),
                "true".to_owned(),
            );
        }
    }
}

/// 一次「落盘 Prefab + 重建实例」请求（整体写回与逐字段应用共用）。
pub(crate) struct PrefabWriteRequest<'a> {
    pub(crate) source: &'a str,
    pub(crate) path: &'a std::path::Path,
    pub(crate) prefab: Prefab,
    /// 需要在重建后重新选中的实例根（通常是用户当前选中的那个）。
    pub(crate) selected: SceneEntityId,
    /// Undo/Redo 菜单里显示的命令名。
    pub(crate) label: &'a str,
    /// 覆盖 diff 的**基线源**；`None` = 用 `prefab` 自己当基线。
    ///
    /// 整体写回 / 逐字段应用都不需要它（那两条里"实例就是事实源"，新源与实例天然一致）。
    /// in-context 退出必须给出进入时那份源：否则刚在阶段里改掉的字段会被实例的老值
    /// 当成覆盖按回去（见 [`collect_prefab_rebuild_state_with_baseline`]）。
    pub(crate) baseline: Option<&'a Prefab>,
}
