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
    ///
    /// Play 中不锁：改走运行态镜像（见 [`EditorApp::plan_play_edits`]）。
    pub(crate) fn apply_component_edit_on_selection(
        &mut self,
        component: &str,
        fields: &[(String, String)],
    ) -> CommandResult {
        if self.is_playing() {
            return self.apply_play_field_edit_on_selection(component, fields);
        }
        let display = crate::util::component_display_name(component);
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
            self.status = format!("选中的实体都没有组件 {display}");
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
            self.status = format!("{display} 无变化");
            return CommandResult::NoOp;
        }
        let label = format!("设置{display}（{} 个实体）", updated.len());
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
                self.status = format!("已批量写入 {display}（{written} 个实体）");
                CommandResult::Applied
            }
            Ok(false) => {
                self.status = format!("{display} 无变化");
                CommandResult::NoOp
            }
            Err(error) => {
                self.status = error.clone();
                CommandResult::Rejected(error)
            }
        }
    }

    /// **Play 期间的字段写入**（Unity 式"运行中调参"）：只改**运行态镜像**，
    /// 并把用户填进去的取值记进会话的待应用表（见 `PlaySession::apply_runtime_edit`）。
    ///
    /// 三条语义：
    /// - **不产生历史、不标脏**：镜像 Stop 时必然丢弃，撤销它没有意义（想反悔就再填一次）；
    /// - **Inspector 立刻看到新值**：面板在 Play 中读的就是这份镜像
    ///   （`main.rs::current_scene` → `PlaySession::runtime_document`）；被脚本每帧覆盖的
    ///   字段（如 transform 位置）会在下一次回灌时回到活世界的读数，这与 Unity 一致；
    /// - **想留下取值要显式应用**：[`EditorApp::apply_play_edits`]（工具栏「应用到场景」）。
    pub(crate) fn apply_play_field_edit(
        &mut self,
        entity: SceneEntityId,
        component: &str,
        fields: &[(String, String)],
    ) -> CommandResult {
        if fields.is_empty() {
            self.status = "没有要写入的字段".to_owned();
            return CommandResult::NoOp;
        }
        let display = crate::util::component_display_name(component);
        let targets = [entity];
        let planned = match self.plan_play_edits(&targets, component, fields) {
            Ok(planned) => planned,
            Err(error) => {
                self.status = error.clone();
                return CommandResult::Rejected(error);
            }
        };
        let written = self.write_play_edits(component, fields, &planned);
        if written == 0 {
            self.status = format!("实体不存在：{}", entity.0);
            return CommandResult::Rejected(self.status.clone());
        }
        let names = field_names(fields);
        self.status =
            format!("播放模式：已记录 {display}（{names}）·Stop 丢弃，点「应用到场景」保留");
        CommandResult::Applied
    }

    /// 多选批量 Play 写入：目标 = 选区里**在镜像里拥有该组件**的实体（与编辑态同一口径）。
    fn apply_play_field_edit_on_selection(
        &mut self,
        component: &str,
        fields: &[(String, String)],
    ) -> CommandResult {
        let display = crate::util::component_display_name(component);
        let selected = self.selection.ids();
        if selected.is_empty() {
            self.status = "请先选择实体".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        let Some(session) = self.play_session.as_ref() else {
            self.status = "运行态镜像还没准备好，请稍后再试".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        let targets: Vec<SceneEntityId> = selected
            .iter()
            .copied()
            .filter(|id| {
                session
                    .runtime_document()
                    .scene
                    .entities
                    .get(id)
                    .is_some_and(|entity| entity.components.contains_key(component))
            })
            .collect();
        if targets.is_empty() {
            self.status = format!("选中的实体都没有组件 {display}");
            return CommandResult::Rejected(self.status.clone());
        }
        let planned = match self.plan_play_edits(&targets, component, fields) {
            Ok(planned) => planned,
            Err(error) => {
                self.status = error.clone();
                return CommandResult::Rejected(error);
            }
        };
        let written = self.write_play_edits(component, fields, &planned);
        let names = field_names(fields);
        self.status = format!(
            "播放模式：已记录 {display}（{written} 个实体 · {names}）·Stop 丢弃，点「应用到场景」保留"
        );
        CommandResult::Applied
    }

    /// 只读校验：把 `fields` 逐个打在**镜像里各目标的当前组件串**上，算出新串。
    ///
    /// 与编辑态的批量路径一样是"全成才写"：任一目标取值非法就整体返回原因、
    /// 镜像一格不动（半套运行态修改比拒绝更难排查）。
    /// 失败还包括"会话还没建好"（Starting/Stopping 过渡态）与"实体没有该组件"。
    fn plan_play_edits(
        &self,
        targets: &[SceneEntityId],
        component: &str,
        fields: &[(String, String)],
    ) -> Result<Vec<(SceneEntityId, String)>, String> {
        let display = crate::util::component_display_name(component);
        let Some(session) = self.play_session.as_ref() else {
            return Err("运行态镜像还没准备好，请稍后再试".to_owned());
        };
        let mirror = session.runtime_document();
        let mut planned = Vec::new();
        for entity in targets {
            let Some(original) = mirror
                .scene
                .entities
                .get(entity)
                .and_then(|instance| instance.components.get(component))
            else {
                return Err(format!("实体 {} 没有组件 {display}", entity.0));
            };
            let mut payload = original.clone();
            for (field, value) in fields {
                match editor::edit_component_payload(component, &payload, field, value) {
                    Ok(Some(next)) => payload = next,
                    Ok(None) => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            // 即使镜像已经是这个取值也要入列：登记的是"用户提交过的字段"（见
            // `write_play_edits`），写回同一个串对镜像没有影响。
            planned.push((*entity, payload));
        }
        Ok(planned)
    }

    /// 把校验好的新组件串写进镜像并登记取值；返回真正登记下的实体数。
    ///
    /// 登记的永远是**用户提交的那批字段**（即使镜像此刻已经是这个值）：
    /// 场景里 x=5、脚本把它跑到 100，我填 100 是想让 100 留下——按"镜像无变化"跳过
    /// 就会把我刚填的取值忘掉。写回同一个串对镜像本身没有影响。
    /// 返回 0 = 这些实体在这一帧被运行期销毁了（`entity.destroy`），调用方据此拒绝。
    fn write_play_edits(
        &mut self,
        component: &str,
        fields: &[(String, String)],
        planned: &[(SceneEntityId, String)],
    ) -> usize {
        let Some(session) = self.play_session.as_mut() else {
            return 0;
        };
        let mut written = 0;
        for (entity, payload) in planned {
            if session.apply_runtime_edit(*entity, component, payload.clone(), fields) {
                written += 1;
            }
        }
        written
    }

    /// 本次 Play 里记录了几个待应用的字段值（工具栏按钮上的角标）。
    pub(crate) fn pending_play_edit_count(&self) -> usize {
        self.play_session
            .as_ref()
            .map_or(0, |session| session.play_edits().len())
    }

    /// **「应用到场景」**（Unity 的显式保留）：把本次 Play 里填过的取值写回**编辑文档**，
    /// 合并成**恰好一条** Undo；不点它、直接 Stop 就是默认回滚。
    ///
    /// 带走的是"我填的那个数"（会话里那份表），不是游戏此刻的读数——否则会把手忙脚乱跑出来
    /// 的运行时状态写进场景文件。运行期生成的实体（`entity.spawn`）、以及编辑文档里已经不存在
    /// 的组件按实体计数跳过并在状态栏说明，它们本来就不属于这份场景。
    pub(crate) fn apply_play_edits(&mut self) -> CommandResult {
        let Some(edits) = self
            .play_session
            .as_ref()
            .map(|session| session.play_edits().clone())
        else {
            self.status = "没有在运行，没有可应用的运行态修改".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        if edits.is_empty() {
            self.status = "本次运行没有记录到运行态修改".to_owned();
            return CommandResult::NoOp;
        }
        let Some(document) = self.document.current_scene.as_ref() else {
            self.status = "没有打开的场景".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        // 按 (实体, 组件) 归并：一个组件的多个字段要合成一条写入，否则字段会互相覆盖。
        let mut groups: std::collections::BTreeMap<(SceneEntityId, String), Vec<(String, String)>> =
            std::collections::BTreeMap::new();
        for ((entity, component, field), value) in &edits {
            groups
                .entry((*entity, component.clone()))
                .or_default()
                .push((field.clone(), value.clone()));
        }
        let mut updated: Vec<(SceneEntityId, String, String)> = Vec::new();
        let mut skipped = 0usize;
        let mut failure: Option<String> = None;
        for ((entity, component), fields) in groups {
            let Some(original) = document
                .scene
                .entities
                .get(&entity)
                .and_then(|instance| instance.components.get(&component))
                .cloned()
            else {
                // 运行期生成的实体 / 编辑文档里已删除的组件：这份场景里没有落点。
                skipped += 1;
                continue;
            };
            let mut payload = original.clone();
            for (field, value) in &fields {
                match editor::edit_component_payload(&component, &payload, field, value) {
                    Ok(Some(next)) => payload = next,
                    Ok(None) => {}
                    Err(error) => {
                        failure = Some(error.to_string());
                        break;
                    }
                }
            }
            if failure.is_some() {
                break;
            }
            if payload != original {
                updated.push((entity, component, payload));
            }
        }
        if let Some(error) = failure {
            self.status = error.clone();
            return CommandResult::Rejected(error);
        }
        if updated.is_empty() {
            self.status = if skipped == 0 {
                "运行态修改与场景取值已一致，无需应用".to_owned()
            } else {
                format!("可应用的运行态修改为 0（跳过 {skipped} 个不属于本场景的实体/组件）")
            };
            return CommandResult::NoOp;
        }
        let written = updated.len();
        // Stop 会把会话里的**编辑器快照**交还给当前文档（`actions::document_gate::stop`），
        // 所以刚应用的取值必须同时落进那份快照，否则"点了 Apply 再 Stop"照样被覆盖回去。
        // Play 期间除了本路径没有任何写入碰得到编辑文档（结构编辑、组件增删全部锁着），
        // 两份拷贝不会各自漂移。
        let carried = updated.clone();
        let result = self.history.execute(
            self.document
                .current_scene
                .as_mut()
                .expect("已确认存在当前文档"),
            "应用运行态修改到场景",
            CommandKind::Property,
            move |document| {
                for (entity, component, payload) in &carried {
                    let Some(instance) = document.scene.entities.get_mut(entity) else {
                        return Err(format!("实体不存在：{}", entity.0));
                    };
                    instance
                        .components
                        .insert(component.clone(), payload.clone());
                }
                document.mark_dirty();
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(true) => {
                if let Some(session) = self.play_session.as_mut() {
                    let snapshot = session.editor_snapshot_mut();
                    for (entity, component, payload) in &updated {
                        if let Some(instance) = snapshot.scene.entities.get_mut(entity) {
                            instance
                                .components
                                .insert(component.clone(), payload.clone());
                        }
                    }
                    // 标脏：Stop 之后当前文档就是这份快照，脏标记只能由它带回去。
                    snapshot.mark_dirty();
                }
                self.status = if skipped == 0 {
                    format!(
                        "已把 {written} 个实体的运行态修改写入场景（一条撤销，Stop 不再回滚这些字段）"
                    )
                } else {
                    format!(
                        "已把 {written} 个实体的运行态修改写入场景（跳过 {skipped} 个运行期生成的实体/已删组件）"
                    )
                };
                CommandResult::Applied
            }
            Ok(false) => {
                self.status = "运行态修改与场景取值已一致，无需应用".to_owned();
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
            return self.apply_play_field_edit(entity, component, fields);
        }
        let display = crate::util::component_display_name(component);
        let Some(document) = self.document.current_scene.as_ref() else {
            self.status = "没有打开的场景".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        let Some(instance) = document.scene.entities.get(&entity) else {
            self.status = format!("实体不存在：{}", entity.0);
            return CommandResult::Rejected(self.status.clone());
        };
        let Some(original) = instance.components.get(component).cloned() else {
            self.status = format!("实体 {} 没有组件 {display}", entity.0);
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
            self.status = format!("{display} 无变化");
            return CommandResult::NoOp;
        }
        let label = format!("设置{display}.{}", changed_fields.join("+"));
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
                    "已写入 {display}（{}）",
                    if changed_fields.is_empty() {
                        "无字段变化".to_owned()
                    } else {
                        changed_fields.join("+")
                    }
                );
                CommandResult::Applied
            }
            Ok(false) => {
                self.status = format!("{display} 无变化");
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
            return self.apply_play_field_edit(
                entity,
                component,
                &[(field.to_owned(), value.to_owned())],
            );
        }
        let display = crate::util::component_display_name(component);
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
            self.status = format!("{display}.{field} 无变化");
            return CommandResult::NoOp;
        };
        let label = format!("设置{path}");
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
                self.status = format!("已写入 {display}.{field}");
                CommandResult::Applied
            }
            Ok(false) => {
                self.status = format!("{display}.{field} 无变化");
                CommandResult::NoOp
            }
            Err(error) => {
                self.status = error.clone();
                CommandResult::Rejected(error)
            }
        }
    }

    /// P3-E02：移除 `MeshRenderer.material=` 覆盖（一次提交 = 一条 Undo）。
    pub(crate) fn clear_mesh_renderer_material(&mut self, entity: SceneEntityId) -> CommandResult {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        let Some(document) = self.document.current_scene.as_mut() else {
            self.status = "没有打开的场景".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        let Some(payload) = document
            .scene
            .entities
            .get(&entity)
            .and_then(|instance| instance.components.get("MeshRenderer"))
            .cloned()
        else {
            self.status = "实体没有网格渲染器组件".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        let Ok(mut renderer) = rustlua_render::mesh3d::MeshRenderer::from_component_str(&payload)
        else {
            self.status = "网格渲染器配置无效".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        if renderer.material.is_none() {
            self.status = "网格渲染器材质无变化".to_owned();
            return CommandResult::NoOp;
        }
        renderer.material = None;
        let next = renderer.to_component_str();
        let result = self.history.execute(
            document,
            "清除网格渲染器材质",
            CommandKind::Property,
            move |document| {
                let Some(instance) = document.scene.entities.get_mut(&entity) else {
                    return Err(format!("实体不存在：{}", entity.0));
                };
                instance.components.insert("MeshRenderer".to_owned(), next);
                document.mark_dirty();
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(true) => {
                self.status = "已移除材质覆盖（回到 glTF 内嵌材质）".to_owned();
                CommandResult::Applied
            }
            Ok(false) => {
                self.status = "网格渲染器材质无变化".to_owned();
                CommandResult::NoOp
            }
            Err(error) => {
                self.status = error.clone();
                CommandResult::Rejected(error)
            }
        }
    }
}
/// 状态栏里的字段名摘要：最多列 3 个，再多折算成数量（工具栏那一行装不下长串）。
fn field_names(fields: &[(String, String)]) -> String {
    let names: Vec<&str> = fields.iter().map(|(field, _)| field.as_str()).collect();
    match names.len() {
        0 => "无字段".to_owned(),
        1..=3 => names.join("+"),
        more => format!("{more} 个字段"),
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
            return Err("播放模式下不能编辑场景，请先点击停止".to_owned());
        }
        let inspector = self.inspector.clone();
        let collider3d = self.collider3d.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            "添加 3D 碰撞体",
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
            return Err("播放模式下不能编辑场景，请先点击停止".to_owned());
        }
        let inspector = self.inspector.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            "移除 3D 碰撞体",
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
            return Err("播放模式下不能编辑场景，请先点击停止".to_owned());
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
            return Err("播放模式下不能编辑场景，请先点击停止".to_owned());
        }
        if !Self::component_registered(name) {
            return Err(format!("未注册组件：{name}"));
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return Ok(());
        };
        if scene.scene.entities[&entity].components.contains_key(name) {
            return Err(format!(
                "实体已有{}组件",
                crate::util::component_display_name(name)
            ));
        }
        let label = format!("添加{}", crate::util::component_display_name(name));
        let inspector = self.inspector.clone();
        let name = name.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self
            .history
            .execute(scene, label, CommandKind::Property, move |scene| {
                Self::insert_default_component(scene, &inspector, entity, &name)
            });
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
            return Err("播放模式下不能编辑场景，请先点击停止".to_owned());
        }
        let label = format!("移除{}", crate::util::component_display_name(name));
        let name = name.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self
            .history
            .execute(scene, label, CommandKind::Property, move |scene| {
                if let Some(entity) = scene.scene.entities.get_mut(&entity) {
                    entity.components.remove(&name);
                    scene.mark_dirty();
                }
                Ok::<(), InspectorError>(())
            });
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
            self.status = "播放模式下不能编辑场景，请先点击停止".into();
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
            self.status = "播放模式下不能编辑场景，请先点击停止".into();
            return;
        }
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "请先创建场景".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "重命名实体", CommandKind::Rename, move |scene| {
                    tree.rename(scene, entity, name).map_err(|e| e.to_string())
                });
        match result {
            Ok(true) => {
                self.status = "实体已重命名".into();
                self.on_document_changed(DocumentChange::EntityRenamed);
            }
            Ok(false) => self.status = "没有变化".into(),
            Err(e) => self.status = e,
        }
    }

    /// 把**当前选中**实体重挂到 `parent` 下（Hierarchy 拖放与命令层共用）。
    pub(crate) fn reparent_selected(&mut self, parent: Option<SceneEntityId>) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".into();
            return;
        }
        let Some(selected) = self.selection.primary() else {
            self.status = "请先在层级中选择实体".into();
            return;
        };
        let Some(scene) = self.document.current_scene.as_ref() else {
            self.status = "请先创建场景".into();
            return;
        };
        if let Err(error) = self.validate_reparent_target(scene, selected, parent) {
            self.status = error;
            return;
        }

        if !self.begin_edit_transaction("更改父实体", CommandKind::Reparent, Some(selected)) {
            self.status = "重挂载事务已激活，无法重复开始".into();
            return;
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.cancel_edit_transaction();
            self.status = "请先创建场景".into();
            return;
        };
        let result = self.tree.reparent(scene, selected, parent);
        match result {
            Ok(()) => {
                self.commit_edit_transaction();
                self.on_document_changed(DocumentChange::EntityReparented);
                self.status = "父实体已更新".into();
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
            self.status = "播放模式下不能编辑场景，请先点击停止".into();
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
            self.status = "请先创建场景".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "删除实体", CommandKind::Delete, move |scene| {
                    let mut deleted_all = std::collections::BTreeSet::new();
                    for target in targets {
                        let deleted = tree.delete(scene, target).map_err(|e| e.to_string())?;
                        deleted_all.extend(deleted);
                    }
                    removed.set(Some(deleted_all));
                    Ok::<(), String>(())
                });
        match result {
            Ok(true) => {
                if let Some(deleted) = removed_out.take() {
                    self.selection.on_entities_removed(&deleted, parent);
                }
                // E03：删除入口直接复位删除目标（此时选区可能已被清空，on_document_changed
                // 的 `had_selection` 无法感知“删除前有选中实体”，故在此显式复位）。
                self.delete_target = crate::DeleteTarget::None;
                self.on_document_changed(DocumentChange::EntityDeleted);
                self.status = "实体已删除".into();
            }
            Ok(false) => self.status = "没有变化".into(),
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
            self.status = "组件剪贴板为空".to_owned();
            return;
        };
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ = self
                .history
                .execute(scene, "粘贴组件", CommandKind::Property, |scene| {
                    let target = scene.scene.entities.get_mut(&entity).ok_or(())?;
                    target.components = clipboard.components.clone();
                    target.resources = clipboard.resources.clone();
                    target.metadata = clipboard.metadata.clone();
                    scene.mark_dirty();
                    Ok::<(), ()>(())
                });
        }
        self.sync_inspector(entity);
        self.status = "组件已粘贴".to_owned();
    }
}
