//! 预设重建的数据收集：从实例根回溯源文件、收集覆盖值、生成新预设、
//! 并把实例元数据写回场景。纯数据搬运，不直接改文档。

use super::*;

/// 重建一个 Prefab 实例所需的全部信息（写回/重新应用共用）。
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
    let root = scene
        .entities
        .get(&instance_root)
        .ok_or_else(|| format!("场景中不存在 Prefab 根实例 {}", instance_root.0))?;
    let instance_entities = collect_scene_subtree(scene, instance_root)?;
    let mut source_to_instance = std::collections::BTreeMap::new();
    for entity_id in &instance_entities {
        let entity = scene
            .entities
            .get(entity_id)
            .ok_or_else(|| format!("场景中不存在实体 {}", entity_id.0))?;
        if entity
            .metadata
            .get("prefab.source")
            .is_none_or(|value| value != prefab_source)
        {
            return Err("Prefab 实例子树包含非同源节点，暂不支持自动重建".to_owned());
        }
        let source_id = entity
            .metadata
            .get("prefab.source_id")
            .ok_or_else(|| format!("实体 {} 缺少 prefab.source_id", entity_id.0))?
            .parse::<u64>()
            .map(SceneEntityId)
            .map_err(|_| format!("实体 {} 的 prefab.source_id 非法", entity_id.0))?;
        if source_to_instance.insert(source_id, *entity_id).is_some() {
            return Err(format!("Prefab 实例中出现重复的源实体 {}", source_id.0));
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
        return Err("Prefab 实例子树和源 Prefab 结构不一致，暂不支持自动重建".to_owned());
    }
    let overrides = collect_prefab_overrides(scene, prefab, &source_to_instance)?;
    Ok(PrefabRebuildState {
        instance_root,
        parent: root.parent,
        instance_entities,
        source_to_instance,
        overrides,
    })
}

pub(crate) fn build_prefab_from_instance_root(
    scene: &rustlua_scene::scene::Scene,
    instance_root: SceneEntityId,
    existing_prefab: &Prefab,
) -> Result<Prefab, String> {
    let mut prefab = Prefab::from_scene(
        scene,
        instance_root,
        existing_prefab.asset_id,
        existing_prefab.name.clone(),
    )
    .map_err(|error| error.to_string())?;
    let mut source_ids = std::collections::BTreeMap::new();
    for entity_id in prefab.entities.keys().copied().collect::<Vec<_>>() {
        let scene_entity = scene
            .entities
            .get(&entity_id)
            .ok_or_else(|| format!("场景中不存在实体 {}", entity_id.0))?;
        let source_id = scene_entity
            .metadata
            .get("prefab.source_id")
            .ok_or_else(|| format!("实体 {} 缺少 prefab.source_id", entity_id.0))?
            .parse::<u64>()
            .map(SceneEntityId)
            .map_err(|_| format!("实体 {} 的 prefab.source_id 非法", entity_id.0))?;
        if source_ids.insert(entity_id, source_id).is_some() {
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
            return Err(format!("Prefab 写回时出现重复源实体 {}", source_id.0));
        }
    }
    prefab.root = root;
    prefab.entities = remapped;
    prefab.metadata = existing_prefab.metadata.clone();
    for entity in prefab.entities.values_mut() {
        entity.metadata.retain(|key, _| !key.starts_with("prefab."));
    }
    Ok(prefab)
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
        })
        .map(|entity| entity.id)
        .collect()
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
        Err(format!("不支持的 Prefab override 字段 {field}"))
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
            entity
                .metadata
                .insert("prefab.source".to_owned(), prefab_source.to_owned());
            entity
                .metadata
                .insert("prefab.source_id".to_owned(), source_id.0.to_string());
            entity.metadata.insert(
                "prefab.root".to_owned(),
                (entity.id == instance.instance_root).to_string(),
            );
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
}
