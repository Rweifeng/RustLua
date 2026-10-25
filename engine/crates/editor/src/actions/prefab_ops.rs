//! 预设实例命令：重应用、写回源文件、逐字段覆盖与还原、解包为普通实体。
//! 重建实例所需的数据收集在 `prefab_rebuild`。

use super::prefab_rebuild::{
    PrefabWriteRequest, collect_scene_subtree, prefab_root_ids, preserve_prefab_editor_flags,
};
use super::*;

impl EditorApp {
    /// 把**单个字段**还原为源预制体 的取值（docs/55 阶段 3：选择性 Revert）。
    ///
    /// 走统一的 history 网关：一条 Undo 即可收回；Play 中拒绝；不触碰其它覆盖字段。
    pub(crate) fn revert_prefab_override_field(&mut self, selected: SceneEntityId, field: &str) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some((prefab_source, source_id, parent)) = (|| {
            let scene = self.document.current_scene.as_ref()?;
            let entity = scene.scene.entities.get(&selected)?;
            Some((
                entity.metadata.get("prefab.source")?.clone(),
                entity
                    .metadata
                    .get("prefab.source_id")?
                    .parse::<u64>()
                    .ok()
                    .map(SceneEntityId)?,
                entity.parent,
            ))
        })() else {
            self.status = "当前实体不是预制体实例".to_owned();
            return;
        };
        let path = strip_verbatim_prefix(&self.project.root).join(&prefab_source);
        if let Err(error) = self.ensure_prefab_source_cached(&path) {
            self.status = format!("加载源预制体失败：{error}");
            return;
        }
        let Some(source_entity) = self
            .prefab_source_cache
            .get(&path)
            .and_then(|cached| cached.prefab.entities.get(&source_id))
            .cloned()
        else {
            self.status = format!("源预制体中缺少实体 {}", source_id.0);
            return;
        };
        let field = field.to_owned();
        let field_label = field.clone();
        let Some(doc) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self.history.execute(
            doc,
            format!("还原覆盖 {field_label}"),
            CommandKind::Property,
            move |scene| {
                let entity = scene
                    .scene
                    .entities
                    .get_mut(&selected)
                    .ok_or_else(|| format!("场景中不存在实体 {}", selected.0))?;
                if field == "name" {
                    entity.name = source_entity.name.clone();
                } else if let Some(key) = field.strip_prefix("component:") {
                    let payload = source_entity
                        .components
                        .get(key)
                        .cloned()
                        .ok_or_else(|| format!("源预制体缺少组件 {key}"))?;
                    entity.components.insert(key.to_owned(), payload);
                } else if let Some(raw_path) = field.strip_prefix("property:") {
                    let path = rustlua_ecs::property_path::PropertyPath::parse(raw_path)
                        .map_err(|error| error.to_string())?;
                    let source_payload = source_entity
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
                    let updated =
                        rustlua_ecs::property_path::write_builtin(&current, &path, &value)
                            .map_err(|error| error.to_string())?;
                    entity.components.insert(path.component.clone(), updated);
                } else if let Some(key) = field.strip_prefix("metadata:") {
                    match source_entity.metadata.get(key) {
                        Some(value) => {
                            entity.metadata.insert(key.to_owned(), value.clone());
                        }
                        None => {
                            entity.metadata.remove(key);
                        }
                    }
                } else {
                    return Err(format!("不支持的预制体覆盖字段 {field}"));
                }
                scene.mark_dirty();
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(true) => {
                self.on_document_changed(DocumentChange::PrefabApply);
                self.status = match parent {
                    Some(parent) => {
                        format!("已回退覆盖字段：{field_label}（父节点 {}）", parent.0)
                    }
                    None => format!("已回退覆盖字段：{field_label}（根节点）"),
                };
            }
            Ok(false) => self.status = format!("字段已是源取值：{field_label}"),
            Err(error) => self.status = format!("回退覆盖失败：{error}"),
        }
    }

    /// 重新应用源预制体（docs/55 §4.1：**目标由 typed action 显式给出**，不再偷偷读选区）。
    ///
    /// 之前这里读 `selection.primary()`，于是 Hierarchy 的右键菜单必须先"设为单选"才能调用——
    /// 一个隐藏的选区依赖。现在目标作为参数传进来，视图侧只负责发命令。
    pub(crate) fn reapply_prefab(&mut self, selected: SceneEntityId) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&selected) else {
            self.status = "预制体根实例不存在".to_owned();
            return;
        };
        if entity
            .metadata
            .get("prefab.root")
            .is_none_or(|value| value != "true")
        {
            self.status = "只有预制体根实例支持重新应用源".to_owned();
            return;
        }
        let Some(prefab_source) = entity.metadata.get("prefab.source").cloned() else {
            self.status = "当前实体没有预制体来源".to_owned();
            return;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        let prefab = match Prefab::load(root.join(&prefab_source)) {
            Ok(prefab) => prefab,
            Err(error) => {
                self.status = format!("重新加载预制体失败：{error}");
                return;
            }
        };
        let rebuild =
            match collect_prefab_rebuild_state(&scene.scene, selected, &prefab_source, &prefab) {
                Ok(rebuild) => rebuild,
                Err(error) => {
                    self.status = error;
                    return;
                }
            };
        let new_root = std::rc::Rc::new(std::cell::Cell::new(None));
        let new_root_result = new_root.clone();
        let prefab_source_for_apply = prefab_source.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self.history.execute(
            scene,
            "重新应用预制体源",
            CommandKind::Property,
            move |scene| {
                if let Some(parent_id) = rebuild.parent
                    && let Some(parent) = scene.scene.entities.get_mut(&parent_id)
                {
                    parent
                        .children
                        .retain(|child| *child != rebuild.instance_root);
                }
                for entity_id in &rebuild.instance_entities {
                    scene.scene.entities.remove(entity_id);
                }
                let instance = prefab
                    .instantiate(&mut scene.scene, rebuild.parent, &rebuild.overrides)
                    .map_err(|error| error.to_string())?;
                apply_prefab_instance_metadata(
                    &mut scene.scene,
                    &prefab_source_for_apply,
                    &instance,
                );
                preserve_prefab_editor_flags(
                    scene,
                    &rebuild.source_to_instance,
                    &instance.source_to_instance,
                );
                new_root.set(Some(instance.instance_root));
                scene.mark_dirty();
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(_) => {
                if let Some(new_root) = new_root_result.get() {
                    self.selection
                        .set_single(new_root, editor::SelectionSource::Inspector);
                    self.on_document_changed(DocumentChange::PrefabApply);
                }
                self.status = format!("已重新应用预制体：{prefab_source}");
            }
            Err(error) => self.status = format!("重新应用预制体失败：{error}"),
        }
    }

    /// 把实例写回源预制体 文件（目标由 typed action 显式给出）。
    pub(crate) fn save_prefab_to_source(&mut self, selected: SceneEntityId) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&selected) else {
            self.status = "预制体根实例不存在".to_owned();
            return;
        };
        if entity
            .metadata
            .get("prefab.root")
            .is_none_or(|value| value != "true")
        {
            self.status = "只有预制体根实例支持写回源预制体".to_owned();
            return;
        }
        let Some(prefab_source) = entity.metadata.get("prefab.source").cloned() else {
            self.status = "当前实体没有预制体来源".to_owned();
            return;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        let prefab_path = root.join(&prefab_source);
        let existing_prefab = match Prefab::load(&prefab_path) {
            Ok(prefab) => prefab,
            Err(error) => {
                self.status = format!("加载源预制体失败：{error}");
                return;
            }
        };
        let new_prefab =
            match build_prefab_from_instance_root(&scene.scene, selected, &existing_prefab) {
                Ok(prefab) => prefab,
                Err(error) => {
                    self.status = format!("生成源预制体失败：{error}");
                    return;
                }
            };
        let new_selected_root = std::rc::Rc::new(std::cell::Cell::new(None));
        let selected_result = new_selected_root.clone();
        let prefab_source_for_apply = prefab_source.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = Self::write_prefab_and_rebuild_instances(
            &mut self.history,
            scene,
            PrefabWriteRequest {
                source: &prefab_source_for_apply,
                path: &prefab_path,
                prefab: new_prefab,
                selected,
                label: "保存预制体源",
            },
            &new_selected_root,
        );
        match result {
            Ok(()) => {
                self.refresh_resources();
                if let Some(new_root) = selected_result.get() {
                    self.selection
                        .set_single(new_root, editor::SelectionSource::Inspector);
                    self.on_document_changed(DocumentChange::PrefabApply);
                }
                self.status = format!("已写回预制体源并更新实例：{prefab_source}");
            }
            Err(error) => self.status = format!("更新预制体实例失败：{error}"),
        }
    }

    /// 把一个字段的**实例取值**应用回源预制体 文件，并同步更新场景里的全部实例
    /// （docs/55 阶段 3：逐字段 Apply）。应用后该字段不再是覆盖。
    pub(crate) fn apply_prefab_override_field(&mut self, selected: SceneEntityId, field: &str) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some((prefab_source, source_id)) = self.prefab_instance_source(selected) else {
            self.status = "当前实体不是预制体实例".to_owned();
            return;
        };
        let Some(instance_value) = self.prefab_override_instance_value(selected, field) else {
            self.status = format!("无法读取实例取值：{field}");
            return;
        };
        let prefab_path = strip_verbatim_prefix(&self.project.root).join(&prefab_source);
        let mut prefab = match Prefab::load(&prefab_path) {
            Ok(prefab) => prefab,
            Err(error) => {
                self.status = format!("加载源预制体失败：{error}");
                return;
            }
        };
        // 只改源预制体 里的目标字段；失败则**不落盘、不改场景**。
        let Some(source_entity) = prefab.entities.get_mut(&source_id) else {
            self.status = format!("源预制体中缺少实体 {}", source_id.0);
            return;
        };
        if field == "name" {
            source_entity.name = instance_value;
        } else if let Some(key) = field.strip_prefix("component:") {
            source_entity
                .components
                .insert(key.to_owned(), instance_value);
        } else if let Some(raw_path) = field.strip_prefix("property:") {
            let Ok(path) = rustlua_ecs::property_path::PropertyPath::parse(raw_path) else {
                self.status = format!("非法属性路径：{raw_path}");
                return;
            };
            let Some(payload) = source_entity.components.get(&path.component).cloned() else {
                self.status = format!("源预制体 缺少组件 {}", path.component);
                return;
            };
            match rustlua_ecs::property_path::write_builtin(&payload, &path, &instance_value) {
                Ok(updated) => {
                    source_entity.components.insert(path.component, updated);
                }
                Err(error) => {
                    self.status = format!("应用覆盖失败：{error}");
                    return;
                }
            }
        } else if let Some(key) = field.strip_prefix("metadata:") {
            source_entity
                .metadata
                .insert(key.to_owned(), instance_value);
        } else {
            self.status = format!("不支持的预制体覆盖字段 {field}");
            return;
        }

        let new_selected_root = std::rc::Rc::new(std::cell::Cell::new(None));
        let selected_result = new_selected_root.clone();
        let field_label = field.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = Self::write_prefab_and_rebuild_instances(
            &mut self.history,
            scene,
            PrefabWriteRequest {
                source: &prefab_source,
                path: &prefab_path,
                prefab,
                selected,
                label: &format!("应用覆盖 {field_label}"),
            },
            &new_selected_root,
        );
        match result {
            Ok(()) => {
                self.refresh_resources();
                if let Some(new_root) = selected_result.get() {
                    self.selection
                        .set_single(new_root, editor::SelectionSource::Inspector);
                    self.on_document_changed(DocumentChange::PrefabApply);
                }
                self.status = format!("已把字段应用到源预制体并更新全部实例：{field_label}");
            }
            Err(error) => self.status = format!("应用覆盖失败：{error}"),
        }
    }

    /// 解包 预制体实例（目标由 typed action 显式给出）。
    pub(crate) fn unpack_prefab(&mut self, selected: SceneEntityId) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&selected) else {
            self.status = "预制体根实例不存在".to_owned();
            return;
        };
        if entity
            .metadata
            .get("prefab.root")
            .is_none_or(|value| value != "true")
        {
            self.status = "只有预制体根实例支持解包".to_owned();
            return;
        }
        let Some(doc) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self
            .history
            .execute(doc, "解包预制体", CommandKind::Property, |scene| {
                let ids = collect_scene_subtree(&scene.scene, selected)?;
                for id in ids {
                    let Some(entity) = scene.scene.entities.get_mut(&id) else {
                        continue;
                    };
                    entity.metadata.retain(|key, _| !key.starts_with("prefab."));
                }
                scene.mark_dirty();
                Ok::<(), String>(())
            });
        match result {
            Ok(_) => {
                self.on_document_changed(DocumentChange::PrefabApply);
                self.status = "预制体实例已解包为普通实体".to_owned();
            }
            Err(error) => self.status = format!("解包失败：{error}"),
        }
    }

    /// 把（可能已改动的）Prefab 落盘，并在**一条历史**里重建场景内全部同源实例。
    ///
    /// 整体写回与逐字段应用共用这条路径：覆盖集合由「实例 vs 新源」重新 diff 得出，
    /// 因此刚被应用的字段会自动从覆盖列表中消失。
    ///
    /// 作为关联函数接收 `history` 与 `scene` 两个**互不相干**的可变借用，
    /// 避免「持有 current_scene 借用时再调用 &mut self 方法」的借用冲突。
    pub(crate) fn write_prefab_and_rebuild_instances(
        history: &mut editor::CommandService,
        scene: &mut editor::SceneDocument,
        request: PrefabWriteRequest<'_>,
        new_selected_root: &std::rc::Rc<std::cell::Cell<Option<SceneEntityId>>>,
    ) -> Result<(), String> {
        let PrefabWriteRequest {
            source: prefab_source,
            path: prefab_path,
            prefab: new_prefab,
            selected,
            label,
        } = request;
        let roots = prefab_root_ids(&scene.scene, prefab_source);
        let mut rebuilds = Vec::new();
        for root_id in roots {
            rebuilds.push(collect_prefab_rebuild_state(
                &scene.scene,
                root_id,
                prefab_source,
                &new_prefab,
            )?);
        }
        new_prefab
            .save(prefab_path)
            .map_err(|error| format!("写回源预制体失败：{error}"))?;

        let prefab_source_owned = prefab_source.to_owned();
        let selected_root = new_selected_root.clone();
        let new_prefab_for_apply = new_prefab.clone();
        let result = history.execute(scene, label, CommandKind::Property, move |scene| {
            for rebuild in &rebuilds {
                if let Some(parent_id) = rebuild.parent
                    && let Some(parent) = scene.scene.entities.get_mut(&parent_id)
                {
                    parent
                        .children
                        .retain(|child| *child != rebuild.instance_root);
                }
                for entity_id in &rebuild.instance_entities {
                    scene.scene.entities.remove(entity_id);
                }
                let instance = new_prefab_for_apply
                    .instantiate(&mut scene.scene, rebuild.parent, &rebuild.overrides)
                    .map_err(|error| error.to_string())?;
                apply_prefab_instance_metadata(&mut scene.scene, &prefab_source_owned, &instance);
                preserve_prefab_editor_flags(
                    scene,
                    &rebuild.source_to_instance,
                    &instance.source_to_instance,
                );
                if rebuild.instance_root == selected {
                    selected_root.set(Some(instance.instance_root));
                }
            }
            scene.mark_dirty();
            Ok::<(), String>(())
        });
        result.map(|_| ())
    }
}
