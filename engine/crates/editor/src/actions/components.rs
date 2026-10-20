//! 组件与实体的结构编辑：字段批改、组件增删、改名、父子关系、删除、粘贴。
//! 每次改动都经 `on_document_changed` 记一条撤销，并在编辑事务里合并。

use super::*;

impl EditorApp {
    /// 应用一次字段编辑（docs/55 阶段 2 typed action 的目标实现）。
    ///
    /// 语义：
    /// - Play 中拒绝（运行态与编辑态隔离）；
    /// - 取值非法 / 目标不存在 → `Rejected` + 状态栏原因，**不改文档、不产生历史**；
    /// - 取值与当前等价（含 `1` vs `1.000000`）→ `NoOp`，**不产生历史**（幂等提交不污染 Undo）；
    /// - 真正改了就写进文档并产生**恰好一条** `CommandKind::Property` 历史。
    ///
    /// 放在 shell（而不是面板模块）是刻意的：它握着提交边界（`history.execute`），
    /// 属于"命令执行"而不是"绘制"。`panels/architecture_tests.rs` 的棘轮测试要求
    /// 面板里不出现直接写文档的调用点。
    ///
    /// 多选批量：把**一个字段**写到所有选中且拥有该组件的实体上（一条 Undo）。
    pub(crate) fn apply_component_field_edit_on_selection(
        &mut self,
        component: &str,
        field: &str,
        value: &str,
    ) -> CommandResult {
        self.apply_component_edit_on_selection(component, &[(field.to_owned(), value.to_owned())])
    }

    /// 多选批量：把**一组字段原子地**写到所有选中且拥有该组件的实体上（一条 Undo）。
    pub(crate) fn apply_component_fields_edit_on_selection(
        &mut self,
        component: &str,
        fields: &[(String, String)],
    ) -> CommandResult {
        self.apply_component_edit_on_selection(component, fields)
    }

    /// 多选批量的共同实现：
    ///
    /// - 目标 = 选中集合里**确实拥有该组件**的实体（与旧手写面板一致；缺组件的实体不动）；
    /// - 每个目标独立走 schema/PropertyPath 校验（`edit_component_payload`），
    ///   **任一目标失败就整体不写**（不会写一半实体）；
    /// - 全部无净变化 → `NoOp`；否则**一条**历史（多选一次操作 = 一条 Undo）。
    pub(crate) fn apply_component_edit_on_selection(
        &mut self,
        component: &str,
        fields: &[(String, String)],
    ) -> CommandResult {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        let selected = self.selection.ids();
        if selected.is_empty() {
            self.status = "请先选择实体".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        let Some(document) = self.document.current_scene.as_ref() else {
            self.status = "没有打开的场景".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        // 目标集合：顺序与选择一致（主选中在前），只保留拥有该组件的实体。
        let targets: Vec<SceneEntityId> = selected
            .iter()
            .copied()
            .filter(|id| {
                document
                    .scene
                    .entities
                    .get(id)
                    .is_some_and(|entity| entity.components.contains_key(component))
            })
            .collect();
        if targets.is_empty() {
            self.status = format!("选中的实体都没有组件 {component}");
            return CommandResult::Rejected(self.status.clone());
        }
        let mut updated: Vec<(SceneEntityId, String)> = Vec::new();
        for id in &targets {
            let Some(original) = document
                .scene
                .entities
                .get(id)
                .and_then(|entity| entity.components.get(component))
                .cloned()
            else {
                continue;
            };
            let mut payload = original.clone();
            for (field, value) in fields {
                match editor::edit_component_payload(component, &payload, field, value) {
                    Ok(Some(next)) => payload = next,
                    Ok(None) => {}
                    Err(error) => {
                        self.status = error.to_string();
                        return CommandResult::Rejected(self.status.clone());
                    }
                }
            }
            if payload != original {
                updated.push((*id, payload));
            }
        }
        if updated.is_empty() {
            self.status = format!("{component} 无变化");
            return CommandResult::NoOp;
        }
        let label = format!("Set {component}（{} 个实体）", updated.len());
        let written = updated.len();
        let key = component.to_owned();
        let document = self
            .document
            .current_scene
            .as_mut()
            .expect("已确认存在当前文档");
        let result =
            self.history
                .execute(document, label, CommandKind::Property, move |document| {
                    for (id, payload) in &updated {
                        if let Some(instance) = document.scene.entities.get_mut(id) {
                            instance.components.insert(key.clone(), payload.clone());
                        }
                    }
                    document.mark_dirty();
                    Ok::<(), String>(())
                });
        match result {
            Ok(true) => {
                self.status = format!("已批量写入 {component}（{written} 个实体）");
                CommandResult::Applied
            }
            Ok(false) => {
                self.status = format!("{component} 无变化");
                CommandResult::NoOp
            }
            Err(error) => {
                self.status = error.clone();
                CommandResult::Rejected(error)
            }
        }
    }

    /// **原子多字段**写入（组件级动作的落点，如 `UiLayout` 的锚点预设）：
    ///
    /// - 逐字段走同一条 schema/PropertyPath 校验路径（`edit_component_payload`），
    ///   中间结果只在内存里滚动，**任何一步失败就整体不写**（不会留下半套取值）；
    /// - 全部无变化 → `NoOp`（不产生历史）；
    /// - 有变化 → **恰好一条** `CommandKind::Property` 历史（逐字段提交会留下多条）。
    pub(crate) fn apply_component_fields_edit(
        &mut self,
        entity: SceneEntityId,
        component: &str,
        fields: &[(String, String)],
    ) -> CommandResult {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        let Some(document) = self.document.current_scene.as_ref() else {
            self.status = "没有打开的场景".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        let Some(instance) = document.scene.entities.get(&entity) else {
            self.status = format!("实体不存在：{}", entity.0);
            return CommandResult::Rejected(self.status.clone());
        };
        let Some(original) = instance.components.get(component).cloned() else {
            self.status = format!("实体 {} 没有组件 {component}", entity.0);
            return CommandResult::Rejected(self.status.clone());
        };
        let mut payload = original.clone();
        let mut changed_fields = Vec::new();
        for (field, value) in fields {
            match editor::edit_component_payload(component, &payload, field, value) {
                Ok(Some(updated)) => {
                    payload = updated;
                    changed_fields.push(field.clone());
                }
                Ok(None) => {}
                Err(error) => {
                    self.status = error.to_string();
                    return CommandResult::Rejected(self.status.clone());
                }
            }
        }
        if payload == original {
            self.status = format!("{component} 无变化");
            return CommandResult::NoOp;
        }
        let label = format!("Set {component}.{}", changed_fields.join("+"));
        let key = component.to_owned();
        let document = self
            .document
            .current_scene
            .as_mut()
            .expect("已确认存在当前文档");
        let result =
            self.history
                .execute(document, label, CommandKind::Property, move |document| {
                    let Some(instance) = document.scene.entities.get_mut(&entity) else {
                        return Err(format!("实体不存在：{}", entity.0));
                    };
                    instance.components.insert(key, payload);
                    document.mark_dirty();
                    Ok::<(), String>(())
                });
        match result {
            Ok(true) => {
                self.status = format!(
                    "已写入 {component}（{}）",
                    if changed_fields.is_empty() {
                        "无字段变化".to_owned()
                    } else {
                        changed_fields.join("+")
                    }
                );
                CommandResult::Applied
            }
            Ok(false) => {
                self.status = format!("{component} 无变化");
                CommandResult::NoOp
            }
            Err(error) => {
                self.status = error.clone();
                CommandResult::Rejected(error)
            }
        }
    }

    /// 应用一次字段编辑（单字段入口；多字段原子写入见 [`Self::apply_component_fields_edit`]）。
    pub(crate) fn apply_component_field_edit(
        &mut self,
        entity: SceneEntityId,
        component: &str,
        field: &str,
        value: &str,
    ) -> CommandResult {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        let Some(document) = self.document.current_scene.as_mut() else {
            self.status = "没有打开的场景".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        // 先在**不动文档**的前提下算出新组件串：校验失败与"无变化"都在这里收口。
        let preview =
            match editor::preview_field_edit(&document.scene, entity, component, field, value) {
                Ok(preview) => preview,
                Err(error) => {
                    self.status = error.to_string();
                    return CommandResult::Rejected(self.status.clone());
                }
            };
        let editor::FieldEdit::Changed { path, payload } = preview else {
            self.status = format!("{component}.{field} 无变化");
            return CommandResult::NoOp;
        };
        let label = format!("Set {path}");
        let key = component.to_owned();
        let result =
            self.history
                .execute(document, label, CommandKind::Property, move |document| {
                    let Some(instance) = document.scene.entities.get_mut(&entity) else {
                        return Err(format!("实体不存在：{}", entity.0));
                    };
                    instance.components.insert(key, payload);
                    document.mark_dirty();
                    Ok::<(), String>(())
                });
        match result {
            Ok(true) => {
                self.status = format!("已写入 {component}.{field}");
                CommandResult::Applied
            }
            Ok(false) => {
                self.status = format!("{component}.{field} 无变化");
                CommandResult::NoOp
            }
            Err(error) => {
                self.status = error.clone();
                CommandResult::Rejected(error)
            }
        }
    }
}
/// Inspector 的**实体级 typed action**实现（shell 层，docs/55 §4.1 规则 1）。
///
/// 面板只发 `EditorCommand`：提交边界（`history.execute`）、Play 锁定、状态文案都住在这里。
/// `panels/architecture_tests.rs` 的棘轮测试扫描 `panels/*.rs` 的**生产代码**，面板里再出现
/// `self.history.execute(` / `self.document.current_scene.as_mut()` 就会判红。
///
/// 命令一律携带**显式目标实体**（不偷偷读选区）；菜单/工具栏/快捷键用的"选中实体"入口
/// 保留 `*_selected` 包装，行为与迁移前逐字一致。
impl EditorApp {
    /// 用当前 Inspector 的 Collider3d 字段缓冲为实体添加/更新 Collider3d（一条历史）。
    pub(crate) fn add_collider3d_component(&mut self, entity: SceneEntityId) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let inspector = self.inspector.clone();
        let collider3d = self.collider3d.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            "Add Collider3d",
            CommandKind::Property,
            move |scene| {
                inspector.set_collider3d(scene, entity, collider3d)?;
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// 移除实体的 Collider3d 组件（经 history 可撤销）。
    pub(crate) fn remove_collider3d_component(
        &mut self,
        entity: SceneEntityId,
    ) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let inspector = self.inspector.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            "Remove Collider3d",
            CommandKind::Property,
            move |scene| {
                inspector.clear_collider3d(scene, entity)?;
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// 为**当前选中**实体添加组件（菜单/工具栏/场景视图的既有入口）。
    ///
    /// 判定顺序与迁移前一致：Play 锁定 → 注册表校验 → 选区；真正的提交在
    /// [`Self::add_component_to`]（typed action 的落点）。
    pub(crate) fn add_component(&mut self, name: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if !Self::component_registered(name) {
            return Err(format!("未注册组件：{name}"));
        }
        let Some(selected) = self.selection.primary() else {
            return Ok(());
        };
        self.add_component_to(selected, name)
    }

    /// 为 `entity` 添加组件（默认值，经 history 可撤销）。目标由 typed action 显式给出。
    pub(crate) fn add_component_to(
        &mut self,
        entity: SceneEntityId,
        name: &str,
    ) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if !Self::component_registered(name) {
            return Err(format!("未注册组件：{name}"));
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return Ok(());
        };
        if scene.scene.entities[&entity].components.contains_key(name) {
            return Err(format!("实体已有 {name} 组件"));
        }
        let inspector = self.inspector.clone();
        let name = name.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            format!("Add {name}"),
            CommandKind::Property,
            move |scene| Self::insert_default_component(scene, &inspector, entity, &name),
        );
        match result {
            Ok(_) => {
                self.sync_inspector(entity);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// 移除 `entity` 上的组件（经 history 可撤销）。
    pub(crate) fn remove_component(
        &mut self,
        entity: SceneEntityId,
        name: &str,
    ) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let name = name.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            format!("Remove {name}"),
            CommandKind::Property,
            move |scene| {
                if let Some(entity) = scene.scene.entities.get_mut(&entity) {
                    entity.components.remove(&name);
                    scene.mark_dirty();
                }
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => {
                self.sync_inspector(entity);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// 用 Inspector 的名称缓冲重命名**当前选中**实体（菜单「重命名」入口）。
    pub(crate) fn rename_selected(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let Some(selected) = self.selection.primary() else {
            self.status = "请先在层级中选择实体".into();
            return;
        };
        let name = self.entity_name.trim().to_owned();
        self.rename_entity(selected, name);
    }

    /// 重命名 `entity`（目标与名称都由 typed action 显式携带）。
    pub(crate) fn rename_entity(&mut self, entity: SceneEntityId, name: String) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "Rename entity", CommandKind::Rename, move |scene| {
                    tree.rename(scene, entity, name).map_err(|e| e.to_string())
                });
        match result {
            Ok(true) => {
                self.status = "Entity renamed".into();
                self.on_document_changed(DocumentChange::EntityRenamed);
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    /// 把**当前选中**实体重挂到 `parent` 下（Hierarchy 拖放与命令层共用）。
    pub(crate) fn reparent_selected(&mut self, parent: Option<SceneEntityId>) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let Some(selected) = self.selection.primary() else {
            self.status = "请先在层级中选择实体".into();
            return;
        };
        let Some(scene) = self.document.current_scene.as_ref() else {
            self.status = "Create a scene first".into();
            return;
        };
        if let Err(error) = self.validate_reparent_target(scene, selected, parent) {
            self.status = error;
            return;
        }

        if !self.begin_edit_transaction("Reparent entity", CommandKind::Reparent, Some(selected)) {
            self.status = "重挂载事务已激活，无法重复开始".into();
            return;
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.cancel_edit_transaction();
            self.status = "Create a scene first".into();
            return;
        };
        let result = self.tree.reparent(scene, selected, parent);
        match result {
            Ok(()) => {
                self.commit_edit_transaction();
                self.on_document_changed(DocumentChange::EntityReparented);
                self.status = "Parent updated".into();
            }
            Err(e) => {
                self.cancel_edit_transaction();
                self.status = e.to_string();
            }
        }
    }

    pub(crate) fn validate_reparent_target(
        &self,
        scene: &editor::SceneDocument,
        moving: SceneEntityId,
        parent: Option<SceneEntityId>,
    ) -> Result<(), String> {
        let Some(target) = parent else {
            return Ok(());
        };
        if target == moving {
            return Err("不能将实体挂到自己下面".to_owned());
        }
        let mut candidate = Some(target);
        while let Some(current) = candidate {
            if current == moving {
                return Err("不能把实体移动到其后代节点下".to_owned());
            }
            candidate = scene
                .scene
                .entities
                .get(&current)
                .and_then(|entity| entity.parent);
        }
        Ok(())
    }

    /// 删除当前选中的实体集合（仅保留顶层根，整批一条历史）。
    pub(crate) fn delete_selected_entity(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        // H03：按选中集合删除（仅保留顶层根，子孙随根整棵移除），整批一条历史。
        let targets = self.top_most_selected_ids();
        if targets.is_empty() {
            self.status = "请先在层级中选择实体".into();
            return;
        }
        // 删除前记录父节点，供删除后选择回退（SelectionModel::on_entities_removed）。
        let parent = self.current_scene().and_then(|scene| {
            self.selection
                .primary()
                .and_then(|primary| scene.scene.entities.get(&primary))
                .and_then(|entity| entity.parent)
        });
        let tree = self.tree.clone();
        let removed = std::rc::Rc::new(std::cell::Cell::new(
            None::<std::collections::BTreeSet<SceneEntityId>>,
        ));
        let removed_out = removed.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result = self.history.execute(
            scene,
            "Delete entities",
            CommandKind::Delete,
            move |scene| {
                let mut deleted_all = std::collections::BTreeSet::new();
                for target in targets {
                    let deleted = tree.delete(scene, target).map_err(|e| e.to_string())?;
                    deleted_all.extend(deleted);
                }
                removed.set(Some(deleted_all));
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(true) => {
                if let Some(deleted) = removed_out.take() {
                    self.selection.on_entities_removed(&deleted, parent);
                }
                // E03：删除入口直接复位删除目标（此时选区可能已被清空，on_document_changed
                // 的 `had_selection` 无法感知“删除前有选中实体”，故在此显式复位）。
                self.delete_target = crate::DeleteTarget::None;
                self.on_document_changed(DocumentChange::EntityDeleted);
                self.status = "Entity deleted".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    /// 粘贴编辑器剪贴板里的组件到**当前选中**实体（菜单「编辑 ▸ 粘贴组件」）。
    pub(crate) fn paste_selected_components(&mut self) {
        let Some(id) = self.selection.primary() else {
            return;
        };
        self.paste_components_to(id);
    }

    /// 粘贴编辑器剪贴板里的组件到 `entity`（目标由 typed action 显式携带）。
    pub(crate) fn paste_components_to(&mut self, entity: SceneEntityId) {
        if self.is_playing() {
            return;
        }
        let Some(clipboard) = self.inspector_clipboard.clone() else {
            self.status = "Component clipboard is empty".to_owned();
            return;
        };
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ =
                self.history
                    .execute(scene, "Paste Components", CommandKind::Property, |scene| {
                        let target = scene.scene.entities.get_mut(&entity).ok_or(())?;
                        target.components = clipboard.components.clone();
                        target.resources = clipboard.resources.clone();
                        target.metadata = clipboard.metadata.clone();
                        scene.mark_dirty();
                        Ok::<(), ()>(())
                    });
        }
        self.sync_inspector(entity);
        self.status = "Components pasted".to_owned();
    }
}
