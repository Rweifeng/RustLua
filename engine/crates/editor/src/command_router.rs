//! Typed command entry point for the editor shell (docs/55 §4.1 rule 1).
//!
//! Views and shortcuts emit an [`EditorCommand`] instead of calling document-mutating
//! methods directly. `EditorApp::dispatch` is the single place that sequences the shared
//! commit boundaries (Inspector flush, transaction guard, unsaved-change guard, status
//! reporting), so behaviour stays identical no matter which widget triggered it.

use crate::EditorApp;
use editor::{CommandResult, EditorCommand, SceneDocument};
use rustlua_scene::scene::SceneEntityId;

#[cfg(test)]
use editor::PlaySession;

/// Prefab action 需要目标是什么（用于统一前置条件检查）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrefabTarget {
    /// 任意实例（字段级 Apply/Revert、整体 Revert）。
    Instance,
    /// 只有根实例（重新应用 / 写回源 / Unpack）。
    Root,
}

impl EditorApp {
    /// Dispatch one typed editor command and report what happened.
    ///
    /// Every persistent change still flows through the existing guarded paths
    /// (`save_scene` / `undo_with_transaction_guard` / `open_scene_resource` / Play-Stop),
    /// so this is a routing layer rather than a second implementation.
    pub(crate) fn dispatch(&mut self, command: EditorCommand) -> CommandResult {
        let started = std::time::Instant::now();
        let result = self.dispatch_inner(command);
        self.profiler.record_command("dispatch", started.elapsed());
        result
    }

    fn dispatch_inner(&mut self, command: EditorCommand) -> CommandResult {
        match command {
            EditorCommand::NewScene { name } => {
                // 与面板「新建场景」同一语义：先收敛 Inspector 挂起编辑，避免脏缓冲跨文档。
                self.commit_inspector_edits_if_dirty();
                let name = if name.trim().is_empty() {
                    "main".to_owned()
                } else {
                    name.trim().to_owned()
                };
                self.document.current_scene = Some(SceneDocument::new_scene(name.clone()));
                self.history.clear();
                self.selection.clear(editor::SelectionSource::Command);
                self.on_document_changed(crate::actions::DocumentChange::Created);
                self.status = format!("已新建场景：{name}（未保存）");
                CommandResult::Applied
            }
            EditorCommand::OpenDocument { path } => {
                // 走既有「未保存切换保护」网关：dirty 时挂起确认，确认前不改动任何状态。
                self.open_scene_resource(path);
                self.command_result_from_status("打开文档被挂起或失败")
            }
            EditorCommand::Save => {
                // E08：保存前先把 Inspector 挂起缓冲收敛为一条历史，落盘内容才能被单次
                // Undo 收回。
                self.commit_inspector_edits_if_dirty();
                if self.save_scene() {
                    CommandResult::Applied
                } else {
                    CommandResult::Rejected(self.status.clone())
                }
            }
            EditorCommand::SaveAs { path } => {
                self.commit_inspector_edits_if_dirty();
                let Some(document) = self.document.current_scene.as_mut() else {
                    return CommandResult::Rejected("没有可保存的场景".to_owned());
                };
                match document.save_as(&path) {
                    Ok(()) => {
                        self.status = format!("场景已另存为：{}", crate::util::display_path(&path));
                        CommandResult::Applied
                    }
                    Err(error) => {
                        self.status = format!("另存失败：{error}");
                        CommandResult::Rejected(self.status.clone())
                    }
                }
            }
            EditorCommand::Undo => {
                // M79 P1-2：全局单栈。不再问"哪个标签聚焦"，而是问账本"最近一次提交落在哪个
                // 归属"（场景 / UI 工作室 / 图 / 材质 / 动画），撤的动作仍由那个归属自己执行。
                self.undo_global()
            }
            EditorCommand::Redo => self.redo_global(),
            EditorCommand::StartPlay => {
                if self.play_state().is_in_play_mode() {
                    return CommandResult::NoOp;
                }
                // M79 P1-4：阶段文档是一份预制体。把它当场景播放，跑的是"预制体根当场景根"，
                // 与用户在场景里看到的东西不是一回事——先返回场景再播放（Unity 同口径）。
                if self.prefab_stage.is_some() {
                    self.status = "预制体编辑中不能播放，请先返回场景".to_owned();
                    return CommandResult::Rejected(self.status.clone());
                }
                self.play();
                match self.play_state() {
                    editor::PlayState::Playing => CommandResult::Applied,
                    _ => CommandResult::Rejected(self.status.clone()),
                }
            }
            EditorCommand::StopPlay => {
                // 暂停态也要能停止（`has_session` 口径），否则用户会被卡在暂停里。
                if !self.play_state().is_in_play_mode() {
                    return CommandResult::NoOp;
                }
                self.stop();
                match self.play_state() {
                    editor::PlayState::Editing => CommandResult::Applied,
                    _ => CommandResult::Rejected(self.status.clone()),
                }
            }
            // ER-10：暂停/继续/单帧步进/重启都只经由 `PlayController` 迁移状态，
            // 不是运行中的会话时统一 no-op（不制造伪状态迁移）。
            EditorCommand::PausePlay => match self.play_state() {
                editor::PlayState::Playing => {
                    self.toggle_play_pause();
                    match self.play_state() {
                        // 暂停必须真的冻结逻辑：状态没落到 `Paused` 就不算成功。
                        editor::PlayState::Paused => CommandResult::Applied,
                        _ => CommandResult::Rejected(self.status.clone()),
                    }
                }
                _ => CommandResult::NoOp,
            },
            EditorCommand::ResumePlay => match self.play_state() {
                editor::PlayState::Paused => {
                    self.toggle_play_pause();
                    match self.play_state() {
                        editor::PlayState::Playing => CommandResult::Applied,
                        _ => CommandResult::Rejected(self.status.clone()),
                    }
                }
                _ => CommandResult::NoOp,
            },
            EditorCommand::StepPlay { frames } => {
                let frames = frames.max(1);
                // 运行中或暂停中都可以步进：运行中步进 = 先冻结再走这一帧（Unity 习惯）。
                if !matches!(
                    self.play_state(),
                    editor::PlayState::Playing | editor::PlayState::Paused
                ) {
                    return CommandResult::NoOp;
                }
                self.step_play_frames(frames);
                match self.play_state() {
                    // 步进保持暂停语义（画面停在步进后的那一帧）。
                    editor::PlayState::Paused => CommandResult::Applied,
                    _ => CommandResult::Rejected(self.status.clone()),
                }
            }
            EditorCommand::RestartPlay => match self.play_state() {
                editor::PlayState::Playing | editor::PlayState::Paused => {
                    self.restart_play();
                    match self.play_state() {
                        editor::PlayState::Playing => CommandResult::Applied,
                        _ => CommandResult::Rejected(self.status.clone()),
                    }
                }
                _ => CommandResult::NoOp,
            },
            // P1：Play 期调参的"显式保留"入口（Unity 里改完不点 Apply 就被 Stop 吞掉）。
            // 写的是**编辑文档**并合并成一条历史，所以必须在 Stop 之前点。
            EditorCommand::ApplyPlayEdits => self.apply_play_edits(),
            EditorCommand::CreateEntity {
                name,
                parent,
                transform_3d,
            } => {
                if !self.project_loaded || self.is_playing() {
                    return CommandResult::Rejected(self.status.clone());
                }
                // 实体创建沿用面板的既有实现：名称缓冲 + 统一创建入口（含默认组件）。
                self.entity_name = if name.trim().is_empty() {
                    if transform_3d {
                        "3D 实体"
                    } else {
                        "新实体"
                    }
                    .to_owned()
                } else {
                    name.trim().to_owned()
                };
                self.document_edit(|app| {
                    if transform_3d {
                        app.add_3d_entity(parent);
                    } else {
                        app.add_entity(parent);
                    }
                })
            }
            EditorCommand::DuplicateSelection => {
                self.document_edit(EditorApp::duplicate_selected_entity)
            }
            EditorCommand::DeleteSelection => self.document_edit(EditorApp::delete_selected_entity),
            EditorCommand::ReparentSelection { target, position } => {
                let Some(primary) = self.selection.primary() else {
                    self.status = "请先选择要移动的实体".to_owned();
                    return CommandResult::NoOp;
                };
                match target {
                    None => self.document_edit(|app| app.reparent_selected(None)),
                    Some(target) => {
                        if target == primary {
                            return CommandResult::NoOp;
                        }
                        self.document_edit(|app| {
                            app.handle_hierarchy_drop(primary, target, position)
                        })
                    }
                }
            }
            EditorCommand::SetComponentField {
                entity,
                component,
                field,
                value,
            } => self.apply_component_field_edit(entity, &component, &field, &value),
            EditorCommand::ClearMeshRendererMaterial { entity } => {
                self.clear_mesh_renderer_material(entity)
            }
            EditorCommand::SetComponentFields {
                entity,
                component,
                fields,
            } => self.apply_component_fields_edit(entity, &component, &fields),
            EditorCommand::SetComponentFieldOnSelection {
                component,
                field,
                value,
            } => self.apply_component_field_edit_on_selection(&component, &field, &value),
            EditorCommand::SetComponentFieldsOnSelection { component, fields } => {
                self.apply_component_fields_edit_on_selection(&component, &fields)
            }
            // Inspector 的组件管理：目标实体随命令携带，实现体住在 shell（`crate::actions`），
            // 面板只发命令。失败原因仍按既有惯例落回 `status`（与面板原来的直调路径一致）。
            EditorCommand::AddComponent { entity, component } => self.document_edit(|app| {
                if let Err(error) = app.add_component_to(entity, &component) {
                    app.status = error;
                }
            }),
            EditorCommand::RemoveComponent { entity, component } => self.document_edit(|app| {
                if let Err(error) = app.remove_component(entity, &component) {
                    app.status = error;
                }
            }),
            EditorCommand::AddCollider3d { entity } => self.document_edit(|app| {
                if let Err(error) = app.add_collider3d_component(entity) {
                    app.status = error;
                }
            }),
            EditorCommand::RemoveCollider3d { entity } => self.document_edit(|app| {
                if let Err(error) = app.remove_collider3d_component(entity) {
                    app.status = error;
                }
            }),
            EditorCommand::RenameSelection { entity, name } => {
                self.document_edit(|app| app.rename_entity(entity, name))
            }
            EditorCommand::PasteComponents { entity } => {
                self.document_edit(|app| app.paste_components_to(entity))
            }
            // Prefab 的 action：目标实体由命令显式携带，视图不再依赖"当前选区"。
            //
            // 前置条件在**命令层**统一判定（实体在场、是实例/根实例、非 Play），
            // 因此拒绝原因精确到"为什么不能做"，而不是让下层方法静默地只改状态栏文字。
            EditorCommand::ApplyPrefabOverride { entity, field } => {
                if let Some(rejected) = self.prefab_precondition(entity, PrefabTarget::Instance) {
                    return rejected;
                }
                self.document_edit(|app| app.apply_prefab_override_field(entity, &field))
            }
            EditorCommand::RevertPrefabOverride { entity, field } => {
                if let Some(rejected) = self.prefab_precondition(entity, PrefabTarget::Instance) {
                    return rejected;
                }
                self.document_edit(|app| app.revert_prefab_override_field(entity, &field))
            }
            EditorCommand::ReapplyPrefab { entity } => {
                if let Some(rejected) = self.prefab_precondition(entity, PrefabTarget::Root) {
                    return rejected;
                }
                self.document_edit(|app| app.reapply_prefab(entity))
            }
            EditorCommand::SavePrefabToSource { entity } => {
                if let Some(rejected) = self.prefab_precondition(entity, PrefabTarget::Root) {
                    return rejected;
                }
                self.document_edit(|app| app.save_prefab_to_source(entity))
            }
            EditorCommand::UnpackPrefab { entity } => {
                if let Some(rejected) = self.prefab_precondition(entity, PrefabTarget::Root) {
                    return rejected;
                }
                self.document_edit(|app| app.unpack_prefab(entity))
            }
            EditorCommand::RevertPrefabOverrides { entity } => {
                if let Some(rejected) = self.prefab_precondition(entity, PrefabTarget::Instance) {
                    return rejected;
                }
                self.document_edit(|app| app.revert_prefab_overrides(entity))
            }
            // M79 P2：逐项还原整棵实例的字段级覆盖。目标允许是实例里的任意节点
            // （命令内部沿 `parent` 找到实例根），所以前置条件按 Instance 判。
            EditorCommand::RevertAllPrefabFieldOverrides { entity } => {
                if let Some(rejected) = self.prefab_precondition(entity, PrefabTarget::Instance) {
                    return rejected;
                }
                self.document_edit(|app| app.revert_all_prefab_field_overrides(entity))
            }
            // M79 P1-4：in-context 预制体编辑。前置条件写在动作自己里（进入要判"当前文档
            // 是不是已经是预制体"、退出要判"有没有口袋"），因此不走 `prefab_precondition`，
            // 也不走 `document_edit`——进入/退出**不产生**场景历史条目，用"undo_len 变长"
            // 判成功会把正常的进入误报成 NoOp。
            EditorCommand::EnterPrefabStage { entity } => self.enter_prefab_stage(entity),
            EditorCommand::ExitPrefabStage { save } => self.close_prefab_stage(save),
            // P1-3b：断链审计只读文档（Play 期间也能扫），改指则在自己内部按
            // `perform_resource_move` 的同一套闸门拒绝 Play 态写入。
            EditorCommand::ScanBrokenReferences => self.scan_broken_references(),
            EditorCommand::RepairBrokenReference { from, to } => {
                match self.repair_broken_reference(&from, &to) {
                    Ok(status) => {
                        self.status = status;
                        CommandResult::Applied
                    }
                    Err(reason) => CommandResult::Rejected(reason),
                }
            }
            EditorCommand::SelectEntity { id } => {
                let exists = self
                    .document
                    .current_scene
                    .as_ref()
                    .is_some_and(|document| document.scene.entities.contains_key(&id));
                if !exists {
                    self.status = format!("实体不存在：{}", id.0);
                    return CommandResult::Rejected(self.status.clone());
                }
                if self.selection.primary() == Some(id) && self.selection.len() == 1 {
                    return CommandResult::NoOp;
                }
                self.select_single_hierarchy(id);
                CommandResult::Applied
            }
        }
    }

    /// Prefab 类 action 的前置条件：不满足时返回带原因的 `Rejected`。
    ///
    /// 三条检查按"最具体"的顺序给出原因：Play 锁定 → 没有打开的场景 → 实体不存在 →
    /// 目标不是实例/根实例。这样测试就能对每条拒绝理由断言，而不是只看"没成功"。
    fn prefab_precondition(
        &mut self,
        entity: SceneEntityId,
        target: PrefabTarget,
    ) -> Option<CommandResult> {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return Some(CommandResult::Rejected(self.status.clone()));
        }
        let Some(document) = self.document.current_scene.as_ref() else {
            self.status = "没有打开的场景".to_owned();
            return Some(CommandResult::Rejected(self.status.clone()));
        };
        if !document.scene.entities.contains_key(&entity) {
            self.status = format!("实体不存在：{}", entity.0);
            return Some(CommandResult::Rejected(self.status.clone()));
        }
        let ready = match target {
            PrefabTarget::Instance => self.is_prefab_instance(entity),
            PrefabTarget::Root => self.is_prefab_root(entity),
        };
        if !ready {
            self.status = match target {
                PrefabTarget::Instance => "当前实体不是预制体实例".to_owned(),
                PrefabTarget::Root => "只有预制体根实例支持该操作".to_owned(),
            };
            return Some(CommandResult::Rejected(self.status.clone()));
        }
        None
    }

    /// Run an entity/document-level edit and report the outcome uniformly.    ///
    /// Entity and Prefab operations funnel through the existing guarded paths (each produces at
    /// most one undo entry), so success is observable as "a history entry appeared", and failures
    /// are reported through `status`. This keeps menus, shortcuts and panels consistent without
    /// each call site inventing its own success detection.
    fn document_edit(&mut self, edit: impl FnOnce(&mut Self)) -> CommandResult {
        let undo_before = self.history.undo_len();
        edit(self);
        let undo_after = self.history.undo_len();
        if undo_after > undo_before {
            return CommandResult::Applied;
        }
        if self.status.contains("失败")
            || self.status.contains("错误")
            || self.status.contains("不能")
            || self.status.contains("锁定")
        {
            return CommandResult::Rejected(self.status.clone());
        }
        CommandResult::NoOp
    }

    /// Turn a guarded action's status text into a result: the guard paths report failures
    /// through `status`, so a failure is recognised by the status still describing a problem.
    fn command_result_from_status(&self, _context: &str) -> CommandResult {
        if self.pending_document_change.is_some() {
            // 未保存切换保护已挂起：用户还没确认，什么都没改，不算失败。
            CommandResult::NoOp
        } else if self.status.contains("失败") || self.status.contains("错误") {
            CommandResult::Rejected(self.status.clone())
        } else {
            CommandResult::Applied
        }
    }

    /// Whether a Play session is currently owning a runtime world (used by tests/HUD).
    #[cfg(test)]
    pub(crate) fn play_session_active(&self) -> bool {
        self.play_session.is_some()
    }

    /// Jump to a location parsed out of a console line or diagnostic (docs/55 §5 P1).
    ///
    /// Four behaviours, in priority order:
    /// 1. a referenced entity that exists in the open scene - select it (Inspector follows);
    /// 2. a scene/prefab path that exists in the project - open it through the 常规
    ///    未保存保护网关（dirty 时挂起确认，不静默丢弃编辑）。
    /// 3. a script/text file that exists on disk - hand `file:line` to the user's external
    ///    editor (no in-editor text view, docs/55 §2.2) and narrow the console filter to it;
    /// 4. otherwise - narrow the console filter to that file name and report the location,
    ///    so a script error at least lands on the file's log lines.
    pub(crate) fn jump_to_diagnostic(
        &mut self,
        location: &editor::DiagnosticLocation,
    ) -> CommandResult {
        if location.is_empty() {
            return CommandResult::NoOp;
        }
        if let Some(entity) = location.entity
            && self
                .document
                .current_scene
                .as_ref()
                .is_some_and(|document| document.scene.entities.contains_key(&entity))
        {
            let selected = self.dispatch(editor::EditorCommand::SelectEntity { id: entity });
            if matches!(selected, CommandResult::Applied) {
                self.status = format!("已定位到实体 {}（{}）", entity.0, location.describe());
            }
            return selected;
        }
        if location.is_document_path()
            && let Some(path) = &location.path
        {
            let absolute = self.project.root.join(path);
            if absolute.exists() {
                self.dispatch(editor::EditorCommand::OpenDocument { path: path.clone() });
                self.status = format!("已定位到文档：{}", location.describe());
                return CommandResult::Applied;
            }
        }
        if let Some(path) = &location.path {
            let name = path
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            self.console_filter = name;
            // P0（脚本迭代闭环）：脚本类文件在编辑器里**没有视图**，"定位到它"唯一有意义的
            // 实现就是把 `file:line` 交给用户自己的编辑器（不内置文本编辑器，见 docs/55 §2.2）。
            // 顺带把控制台过滤收窄到该文件——多条报错堆在一起时，这一步才是能读的那一条。
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                self.project.root.join(path)
            };
            if crate::external_editor::is_text_source(&absolute) && absolute.is_file() {
                return match crate::external_editor::open_in_external_editor(
                    &absolute,
                    location.line,
                    self.external_editor_dry_run,
                ) {
                    Ok(plan) => {
                        self.status = format!(
                            "{}（已按来源过滤控制台）",
                            crate::external_editor::describe(&plan, &absolute, location.line)
                        );
                        CommandResult::Applied
                    }
                    Err(error) => {
                        self.status = format!("已按来源过滤控制台，但外部编辑器没起来：{error}");
                        CommandResult::Rejected(error)
                    }
                };
            }
            self.status = format!("已按来源过滤控制台：{}", location.describe());
            return CommandResult::Applied;
        }
        if let Some(line) = location.line {
            self.status = format!("诊断位置：第 {line} 行（无可定位文件）");
            return CommandResult::NoOp;
        }
        CommandResult::NoOp
    }

    /// Runtime document of the active Play session, if any.
    #[cfg(test)]
    pub(crate) fn play_session_runtime(&self) -> Option<&SceneDocument> {
        self.play_session
            .as_ref()
            .map(PlaySession::runtime_document)
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandResult, EditorCommand, SceneEntityId};
    use crate::EditorApp;
    use editor::PlayState;
    use editor::{CommandKind, ProjectInfo, SceneDocument};
    use rustlua_infra::logging::LogSink;

    fn test_app() -> EditorApp {
        let mut app = EditorApp::new(
            ProjectInfo {
                root: std::env::temp_dir()
                    .join(format!("rustlua-command-router-{}", std::process::id())),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        let mut document = SceneDocument::new_scene("main");
        document
            .scene
            .add_entity(SceneEntityId(1), "Root", None)
            .unwrap();
        app.document.current_scene = Some(document);
        app
    }

    /// UI Studio 已打开 `ui/shop.ui.json`，且 UI Studio 标签处于聚焦状态。
    fn ui_studio_app(label: &str) -> (EditorApp, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "rustlua-command-router-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("ui")).unwrap();
        let spec: rustlua_ui_spec::UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "shop"},
            "root": {"key": "root", "type": "panel",
                     "children": [{"key": "title", "type": "text", "text": "Shop"}]}
        }))
        .unwrap();
        let encoded = editor::ui_spec_host::encode_spec(&spec).unwrap();
        std::fs::write(root.join("ui/shop.ui.json"), &encoded).unwrap();

        let mut app = test_app();
        app.project = ProjectInfo {
            root: root.clone(),
            version: "test".to_owned(),
            directories: Vec::new(),
        };
        app.project_loaded = true;
        let mut studio = editor::ui_studio::UiStudio::new(root.clone());
        studio.open("ui/shop.ui.json").expect("打开 UI 文档");
        app.ui_studio_panel.studio = Some(studio);
        app.show_ui_studio = true;
        let mut dock = crate::dock::default_dock_state();
        dock.main_surface_mut()
            .push_to_first_leaf(crate::dock::DockTab::UiStudio);
        assert!(crate::dock::focus_tab(
            &mut dock,
            crate::dock::DockTab::UiStudio
        ));
        app.dock_state = Some(dock);
        (app, root)
    }

    /// 打开一张最小 `.vgraph` 的工程（可视化脚本归属在账本上有自己的栈）。
    fn graph_app(label: &str) -> (EditorApp, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "rustlua-command-router-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("graphs")).unwrap();
        std::fs::write(
            root.join("graphs/flow.vgraph"),
            r#"{"version":1,"name":"flow","nodes":[]}"#,
        )
        .unwrap();
        let mut app = test_app();
        app.project = ProjectInfo {
            root: root.clone(),
            version: "test".to_owned(),
            directories: Vec::new(),
        };
        app.project_loaded = true;
        app.graph
            .open(&root, std::path::Path::new("graphs/flow.vgraph"))
            .expect("打开图");
        (app, root)
    }

    /// **一次 Ctrl+Z 只消费一步**（M79 P1-2 的回归）：图与场景各有历史时，一次撤销只
    /// 挪其中一条栈，另一条纹丝不动。历史上画布私读快捷键会让同一次按键吃两步。
    #[test]
    fn one_undo_consumes_exactly_one_owner_step() {
        let (mut app, root) = graph_app("undo-one-step");
        // 场景一步。
        assert_eq!(
            app.dispatch(EditorCommand::CreateEntity {
                name: "Root2".to_owned(),
                parent: None,
                transform_3d: false,
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1);
        app.poll_undo_owners();
        // 图一步（加一个节点）。
        app.graph
            .doc_mut()
            .expect("文档已打开")
            .add_node("on_start", [0.0, 0.0])
            .expect("加节点");
        assert!(app.graph.undo_can_undo());
        app.poll_undo_owners();

        // 撤销：只动图，场景历史原地不动。
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        assert!(!app.graph.undo_can_undo(), "图的那一步应被撤掉");
        assert_eq!(
            app.history.undo_len(),
            1,
            "同一次按键不得顺手撤掉场景的一步"
        );
        assert_eq!(app.history.redo_len(), 0);

        // 再撤一次：这时才轮到场景。
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        assert_eq!(app.history.undo_len(), 0);
        assert_eq!(app.history.redo_len(), 1);

        // 两条栈都空了：第三次按键必须是 no-op，而不是继续吃别的面板。
        assert_eq!(
            app.dispatch(EditorCommand::Undo),
            CommandResult::NoOp,
            "没有历史时不得凭空报成功：{}",
            app.status
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 全局 Undo 按「最近提交的归属」分流，**与哪个标签聚焦无关**（M79 P1-2）：
    /// UI 工作室刚接受过一步就先撤它，栈空后才自动落回场景历史。两条栈互不共享
    /// （UI Undo 不得吞掉场景历史，反之亦然）。
    #[test]
    fn ui_studio_owner_is_undone_before_scene_history_regardless_of_focus() {
        use rustlua_ui_spec::{UiPatch, UiPatchOp};

        let (mut app, root) = ui_studio_app("ui-undo");
        let original = app.ui_studio_panel.studio.as_ref().unwrap().revision();
        // 场景侧先造一条历史：UI Studio 聚焦时它必须原地不动。
        assert_eq!(
            app.dispatch(EditorCommand::CreateEntity {
                name: "SceneChild".to_owned(),
                parent: Some(SceneEntityId(1)),
                transform_3d: false,
            }),
            CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), 1);
        // 帧首轮询：场景那一步属于"上一帧"，账本先看到它。
        app.poll_undo_owners();

        let accepted_revision = {
            let studio = app.ui_studio_panel.studio.as_mut().unwrap();
            let proposal = studio
                .submit_patch(UiPatch {
                    base_revision: original,
                    ops: vec![UiPatchOp::SetEvent {
                        key: "title".to_owned(),
                        event: "click".to_owned(),
                        function: "shop.open".to_owned(),
                    }],
                })
                .expect("提交提案");
            studio
                .accept(&proposal.proposal_id)
                .expect("接受提案")
                .revision
        };
        assert_ne!(accepted_revision, original);

        // 前台故意切到场景标签：分流只看账本，不看谁在前台。
        let dock = app.dock_state.as_mut().unwrap();
        assert!(crate::dock::focus_tab(dock, crate::dock::DockTab::Scene));
        assert_eq!(
            app.ui_studio_panel.studio.as_ref().unwrap().undo_len(),
            1,
            "前置：接受一次后必须有可撤销项"
        );
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        assert_eq!(
            app.ui_studio_panel.studio.as_ref().unwrap().revision(),
            original,
            "撤销接受必须把 revision 还原"
        );
        assert_eq!(app.history.undo_len(), 1, "场景历史不得被 UI Undo 消费");
        // UI 栈已空 → 同一个焦点下继续 Undo 才落回场景历史。
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        assert_eq!(app.history.undo_len(), 0);
        assert_eq!(
            app.ui_studio_panel.studio.as_ref().unwrap().revision(),
            original,
            "已空的 UI 栈不因场景 Undo 改变"
        );

        // 场景侧又提交一步 → 最近归属换回场景，撤的是它，不是 UI 工作室。
        assert_eq!(
            app.dispatch(EditorCommand::CreateEntity {
                name: "Second".to_owned(),
                parent: None,
                transform_3d: false,
            }),
            CommandResult::Applied
        );
        app.poll_undo_owners();
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        assert_eq!(app.history.undo_len(), 0, "刚提交的场景那步被撤掉了");
        assert_eq!(app.history.redo_len(), 1, "重做栈只应有这一步");
        assert_eq!(
            app.ui_studio_panel.studio.as_ref().unwrap().revision(),
            original,
            "UI 工作室的历史不得被场景 Undo 挪动"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 全局 Redo 与 Undo 对称：先回到**刚才撤销掉的那个归属**（Unity 的 Redo 是上一次
    /// Undo 的逆），它没有可重做的了才按账本顺序找下一个；两条栈互不吞。
    #[test]
    fn redo_goes_back_to_the_owner_that_was_just_undone() {
        use rustlua_ui_spec::{UiPatch, UiPatchOp};

        let (mut app, root) = ui_studio_app("ui-redo");
        let original = app.ui_studio_panel.studio.as_ref().unwrap().revision();
        // 场景侧先造一条「可重做」：创建实体再撤销。
        assert_eq!(
            app.dispatch(EditorCommand::CreateEntity {
                name: "SceneChild".to_owned(),
                parent: Some(SceneEntityId(1)),
                transform_3d: false,
            }),
            CommandResult::Applied
        );
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        assert!(app.history.can_redo());

        let accepted_revision = {
            let studio = app.ui_studio_panel.studio.as_mut().unwrap();
            let proposal = studio
                .submit_patch(UiPatch {
                    base_revision: original,
                    ops: vec![UiPatchOp::SetEvent {
                        key: "title".to_owned(),
                        event: "click".to_owned(),
                        function: "shop.open".to_owned(),
                    }],
                })
                .expect("提交提案");
            studio
                .accept(&proposal.proposal_id)
                .expect("接受提案")
                .revision
        };
        let dock = app.dock_state.as_mut().unwrap();
        assert!(crate::dock::focus_tab(dock, crate::dock::DockTab::UiStudio));
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        assert_eq!(
            app.ui_studio_panel.studio.as_ref().unwrap().revision(),
            original
        );
        assert!(app.ui_studio_panel.studio.as_ref().unwrap().can_redo());

        // UI Studio 聚焦 → Redo 消费 UI 栈，场景重做栈原地不动。
        assert_eq!(app.dispatch(EditorCommand::Redo), CommandResult::Applied);
        assert_eq!(
            app.ui_studio_panel.studio.as_ref().unwrap().revision(),
            accepted_revision,
            "重做接受必须回到接受后的 revision"
        );
        assert!(app.history.can_redo(), "场景重做栈不得被 UI Redo 消费");
        assert!(!app.ui_studio_panel.studio.as_ref().unwrap().can_redo());

        // UI 栈已空 → 同一条命令落回场景历史。
        assert_eq!(app.dispatch(EditorCommand::Redo), CommandResult::Applied);
        assert!(!app.history.can_redo());
        assert_eq!(
            app.ui_studio_panel.studio.as_ref().unwrap().revision(),
            accepted_revision,
            "已空的 UI 栈不因场景 Redo 改变"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// **唯一的变换与相机组件**（第 57 轮）：从"添加组件"入口加进来的默认载荷必须是
    /// **统一形态**，并且能被引擎解析器读回——这是"旧名删掉之后，新实体仍然合法"的常驻门。
    ///
    /// 两条默认值都来自**引擎类型**（`Transform3D::identity()` / `Camera3D::default()`），
    /// 编辑器里没有第二份手写字面量。
    #[test]
    fn adding_transform_and_camera_writes_unified_payloads() {
        let mut app = test_app();
        app.project_loaded = true;
        let entity = SceneEntityId(1);
        for component in ["Transform", "Camera"] {
            assert_eq!(
                app.dispatch(EditorCommand::AddComponent {
                    entity,
                    component: component.to_owned(),
                }),
                CommandResult::Applied,
                "{component} 应能作为组件加进来"
            );
        }
        let components =
            &app.document.current_scene.as_ref().unwrap().scene.entities[&entity].components;
        assert_eq!(
            rustlua_ecs::transform::Transform3D::from_component_str(&components["Transform"])
                .expect("统一形态的 Transform"),
            rustlua_ecs::transform::Transform3D::identity()
        );
        let camera = rustlua_camera::camera::Camera::from_component_str(&components["Camera"])
            .expect("统一形态的 Camera");
        assert_eq!(
            camera.projection(),
            rustlua_camera::camera::CameraProjection::Perspective,
            "默认添加的是透视相机（要正交就把判别字段改成 orthographic）"
        );
        // 旧名不得再被认识：加进文档只会得到"未知组件"错误，而不是悄悄写一个没人读的键。
        assert!(
            app.dispatch(EditorCommand::AddComponent {
                entity,
                component: "Transform3D".to_owned(),
            }) != CommandResult::Applied
        );
        assert!(
            !app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
                .components
                .contains_key("Transform3D")
        );
    }

    /// 持久实体是**组件**（无载荷标记）：从 Add Component 加进来后，文档里出现的是
    /// `Persistent` 键而不是旧的 `entity_meta persistent`；移除组件即关闭。
    #[test]
    fn adding_persistent_component_writes_a_marker_component() {
        let mut app = test_app();
        app.project_loaded = true;
        let entity = SceneEntityId(1);
        assert_eq!(
            app.dispatch(EditorCommand::AddComponent {
                entity,
                component: rustlua_scene::scene::PERSISTENT_COMPONENT.to_owned(),
            }),
            CommandResult::Applied
        );
        let stored = &app.document.current_scene.as_ref().unwrap().scene.entities[&entity];
        assert_eq!(
            stored
                .components
                .get(rustlua_scene::scene::PERSISTENT_COMPONENT),
            Some(&String::new()),
            "标记组件必须存在且载荷为空"
        );
        assert!(
            !stored.metadata.contains_key("persistent"),
            "组件路径不得再写旧的 metadata 标志"
        );
        assert_eq!(
            app.dispatch(EditorCommand::RemoveComponent {
                entity,
                component: rustlua_scene::scene::PERSISTENT_COMPONENT.to_owned(),
            }),
            CommandResult::Applied
        );
        assert!(
            !app.document.current_scene.as_ref().unwrap().scene.entities[&entity]
                .components
                .contains_key(rustlua_scene::scene::PERSISTENT_COMPONENT)
        );
    }

    /// 实体级命令：创建 / 复制 / 删除 / 换父 / 选中都走同一入口，且各自只产生一条历史。
    #[test]
    fn entity_commands_apply_through_dispatch_and_undo_as_one_step() {
        let mut app = test_app();
        app.project_loaded = true;
        let root = SceneEntityId(1);

        // 创建：作为 Root 的子节点，产生 1 条历史，并自动选中新实体。
        assert_eq!(
            app.dispatch(EditorCommand::CreateEntity {
                name: "Child".to_owned(),
                parent: Some(root),
                transform_3d: false,
            }),
            editor::CommandResult::Applied
        );
        let created = app.selection.primary().expect("新实体应被选中");
        assert_eq!(app.history.undo_len(), 1);
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.entities[&created].parent,
            Some(root)
        );

        // 第 57 轮：变换统一成一个组件，因此"新建 3D 实体"与"新建实体"写的是**同一个**
        // `Transform`（单位变换）；空间由实体随后挂上的组件推断（`MeshRenderer`/`Light`/… ⇒ 3D）。
        // 这条断言把"写入的载荷是统一形态"钉住（旧名 `Transform3D` 不会再出现）。
        assert_eq!(
            app.dispatch(EditorCommand::CreateEntity {
                name: "Box".to_owned(),
                parent: None,
                transform_3d: true,
            }),
            editor::CommandResult::Applied
        );
        let boxed = app.selection.primary().expect("3D 实体应被选中");
        let payload = &app.document.current_scene.as_ref().unwrap().scene.entities[&boxed]
            .components["Transform"];
        assert_eq!(
            rustlua_ecs::transform::Transform3D::from_component_str(payload).expect("统一形态"),
            rustlua_ecs::transform::Transform3D::identity()
        );

        // 复制：在选中实体旁生成副本，历史 +1。
        let before_duplicate = app.history.undo_len();
        assert_eq!(
            app.dispatch(EditorCommand::DuplicateSelection),
            editor::CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), before_duplicate + 1);

        // 删除：整批删除只产生一条历史。
        let before_delete = app.history.undo_len();
        assert_eq!(
            app.dispatch(EditorCommand::DeleteSelection),
            editor::CommandResult::Applied
        );
        assert_eq!(app.history.undo_len(), before_delete + 1);

        // 逐步撤销可回到最初：只剩 Root，且没有任何子节点。
        while app.history.can_undo() {
            app.dispatch(EditorCommand::Undo);
        }
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        assert_eq!(scene.entities.len(), 1);
        assert!(scene.entities[&root].children.is_empty());
    }

    /// 换父：拖放落点与「移动到根节点」是同一条命令的两种参数。
    #[test]
    fn reparent_command_covers_drop_and_move_to_root() {
        let mut app = test_app();
        app.project_loaded = true;
        let root = SceneEntityId(1);
        app.dispatch(EditorCommand::CreateEntity {
            name: "Child".to_owned(),
            parent: Some(root),
            transform_3d: false,
        });
        let child = app.selection.primary().expect("child");
        app.dispatch(EditorCommand::CreateEntity {
            name: "Other".to_owned(),
            parent: None,
            transform_3d: false,
        });
        let other = app.selection.primary().expect("other");

        // 移动到根节点：child 从 root 下移出。
        app.dispatch(EditorCommand::SelectEntity { id: child });
        assert_eq!(
            app.dispatch(EditorCommand::ReparentSelection {
                target: None,
                position: editor::ReparentPosition::Child,
            }),
            editor::CommandResult::Applied
        );
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.entities[&child].parent,
            None
        );

        // 拖到 other 之下。
        assert_eq!(
            app.dispatch(EditorCommand::ReparentSelection {
                target: Some(other),
                position: editor::ReparentPosition::Child,
            }),
            editor::CommandResult::Applied
        );
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.entities[&child].parent,
            Some(other)
        );

        // 自己拖到自己：无操作，不产生历史。
        let undo_before = app.history.undo_len();
        assert_eq!(
            app.dispatch(EditorCommand::ReparentSelection {
                target: Some(child),
                position: editor::ReparentPosition::Child,
            }),
            editor::CommandResult::NoOp
        );
        assert_eq!(app.history.undo_len(), undo_before);
    }

    /// 选中命令：不存在实体被拒绝，重复单选是 no-op。
    #[test]
    fn select_entity_command_validates_and_is_idempotent() {
        let mut app = test_app();
        assert_eq!(
            app.dispatch(EditorCommand::SelectEntity {
                id: SceneEntityId(1)
            }),
            editor::CommandResult::Applied
        );
        assert_eq!(app.selection.primary(), Some(SceneEntityId(1)));
        assert_eq!(
            app.dispatch(EditorCommand::SelectEntity {
                id: SceneEntityId(1)
            }),
            editor::CommandResult::NoOp
        );
        match app.dispatch(EditorCommand::SelectEntity {
            id: SceneEntityId(999),
        }) {
            editor::CommandResult::Rejected(reason) => assert!(reason.contains("不存在")),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    /// Play 状态下实体创建必须被拒绝（不产生历史、不改文档）。
    #[test]
    fn entity_commands_are_rejected_while_playing() {
        let mut app = test_app();
        app.project_loaded = true;
        app.play.set_state_for_test(PlayState::Playing);
        let before = app.document.current_scene.clone();
        match app.dispatch(EditorCommand::CreateEntity {
            name: "Child".to_owned(),
            parent: None,
            transform_3d: false,
        }) {
            editor::CommandResult::Rejected(_) => {}
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 0);
        assert_eq!(app.document.current_scene, before);
    }

    /// ER-10：Pause / Resume / Step / Restart 四条命令的状态机语义。
    ///
    /// 未运行（或启动失败）时全部 no-op；运行中暂停后步进必须保持暂停；继续运行会
    /// 清空待步进帧，避免"暂停期间的步进请求"在恢复后突然快进。
    #[test]
    fn play_control_commands_follow_the_state_machine() {
        let mut app = test_app();

        // 未运行：四条命令都不产生伪状态迁移。
        assert_eq!(
            app.dispatch(EditorCommand::PausePlay),
            editor::CommandResult::NoOp
        );
        assert_eq!(
            app.dispatch(EditorCommand::ResumePlay),
            editor::CommandResult::NoOp
        );
        assert_eq!(
            app.dispatch(EditorCommand::StepPlay { frames: 1 }),
            editor::CommandResult::NoOp
        );
        assert_eq!(
            app.dispatch(EditorCommand::RestartPlay),
            editor::CommandResult::NoOp
        );
        assert_eq!(app.play_state(), PlayState::Editing);

        // 启动失败态同样不接受运行控制命令。
        app.play.set_state_for_test(PlayState::StartFailed);
        assert_eq!(
            app.dispatch(EditorCommand::PausePlay),
            editor::CommandResult::NoOp
        );
        assert_eq!(
            app.dispatch(EditorCommand::RestartPlay),
            editor::CommandResult::NoOp
        );
        assert_eq!(app.play_state(), PlayState::StartFailed);

        // 运行中 → 暂停。
        app.play.set_state_for_test(PlayState::Playing);
        assert_eq!(
            app.dispatch(EditorCommand::PausePlay),
            editor::CommandResult::Applied
        );
        assert_eq!(app.play_state(), PlayState::Paused);
        // 重复暂停是 no-op（不报错，也不改变状态）。
        assert_eq!(
            app.dispatch(EditorCommand::PausePlay),
            editor::CommandResult::NoOp
        );
        assert_eq!(app.play_state(), PlayState::Paused);

        // 暂停态单帧步进：请求被记录，状态保持暂停。
        assert_eq!(
            app.dispatch(EditorCommand::StepPlay { frames: 3 }),
            editor::CommandResult::Applied
        );
        assert_eq!(app.play.pending_steps(), 3);
        assert_eq!(app.play_state(), PlayState::Paused);

        // 继续运行：回到 Playing，并清空待步进帧。
        assert_eq!(
            app.dispatch(EditorCommand::ResumePlay),
            editor::CommandResult::Applied
        );
        assert_eq!(app.play_state(), PlayState::Playing);
        assert_eq!(app.play.pending_steps(), 0);
        // 运行中再"继续"是 no-op。
        assert_eq!(
            app.dispatch(EditorCommand::ResumePlay),
            editor::CommandResult::NoOp
        );

        // 运行中直接步进：先冻结再走这一帧（不会在步进后继续跑）。
        assert_eq!(
            app.dispatch(EditorCommand::StepPlay { frames: 1 }),
            editor::CommandResult::Applied
        );
        assert_eq!(app.play_state(), PlayState::Paused);
        assert_eq!(app.play.pending_steps(), 1);
    }

    /// Prefab action 的前置条件：目标不存在 / 不是实例 / 不是根实例都必须 `Rejected`
    /// 且**不产生历史**（拒绝理由要能被断言，而不是只看"没成功"）。
    #[test]
    fn prefab_commands_reject_invalid_targets_with_precise_reasons() {
        let mut app = test_app();
        app.project_loaded = true;
        // 场景里有 Root（普通实体，无 Prefab 元数据）。
        let root = SceneEntityId(1);

        // 1) 实体不存在 → 拒绝理由带 ID。
        match app.dispatch(EditorCommand::ReapplyPrefab {
            entity: SceneEntityId(42),
        }) {
            CommandResult::Rejected(reason) => assert!(reason.contains("42"), "{reason}"),
            other => panic!("expected Rejected, got {other:?}"),
        }
        // 2) 普通实体不是 预制体实例 → 字段级操作被拒绝。
        match app.dispatch(EditorCommand::ApplyPrefabOverride {
            entity: root,
            field: "property:Transform.x".to_owned(),
        }) {
            CommandResult::Rejected(reason) => {
                assert!(reason.contains("不是预制体实例"), "{reason}")
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        match app.dispatch(EditorCommand::RevertPrefabOverride {
            entity: root,
            field: "property:Transform.x".to_owned(),
        }) {
            CommandResult::Rejected(reason) => {
                assert!(reason.contains("不是预制体实例"), "{reason}")
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        match app.dispatch(EditorCommand::RevertPrefabOverrides { entity: root }) {
            CommandResult::Rejected(reason) => {
                assert!(reason.contains("不是预制体实例"), "{reason}")
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        // 3) 根实例限定操作同样被拒绝（同一个普通实体）。
        for command in [
            EditorCommand::ReapplyPrefab { entity: root },
            EditorCommand::SavePrefabToSource { entity: root },
            EditorCommand::UnpackPrefab { entity: root },
        ] {
            match app.dispatch(command) {
                CommandResult::Rejected(reason) => {
                    assert!(reason.contains("根实例"), "{reason}")
                }
                other => panic!("expected Rejected, got {other:?}"),
            }
        }
        assert_eq!(app.history.undo_len(), 0, "拒绝必须不产生历史");
        assert_eq!(
            app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .len(),
            1,
            "拒绝不得改动文档"
        );
    }

    /// Prefab action 在 Play 中被拒绝（运行态与编辑态隔离）。
    #[test]
    fn prefab_commands_are_rejected_while_playing() {
        let mut app = test_app();
        app.project_loaded = true;
        app.play.set_state_for_test(PlayState::Playing);
        let before = app.document.current_scene.clone();
        match app.dispatch(EditorCommand::ReapplyPrefab {
            entity: SceneEntityId(1),
        }) {
            CommandResult::Rejected(reason) => assert!(reason.contains("播放模式下"), "{reason}"),
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(app.history.undo_len(), 0);
        assert_eq!(app.document.current_scene, before);
    }

    /// 诊断跳转：实体定位优先选中该实体；未知实体则退化为按文件过滤控制台。
    #[test]
    fn diagnostic_jump_selects_entity_then_falls_back_to_filter() {
        let mut app = test_app();
        app.project_loaded = true;

        // 1) `entity=1` 命中当前场景：选中它并把说明写进状态栏。
        let location = editor::parse_location("layout overflow entity=1").expect("entity location");
        assert_eq!(
            app.jump_to_diagnostic(&location),
            editor::CommandResult::Applied
        );
        assert_eq!(app.selection.primary(), Some(SceneEntityId(1)));
        assert!(app.status.contains("实体 1"), "{}", app.status);

        // 2) 不存在的实体 + 脚本路径：退化为按文件名过滤控制台（不报错、不切文档）。
        let before = app.document.current_scene.clone();
        let location = editor::parse_location("ERROR script: load failed scripts/player.lua:12")
            .expect("file");
        assert_eq!(
            app.jump_to_diagnostic(&location),
            editor::CommandResult::Applied
        );
        assert_eq!(app.console_filter, "player.lua");
        assert_eq!(app.document.current_scene, before);
        assert!(app.status.contains("player.lua:12"), "{}", app.status);

        // 3) 空位置是 no-op（不误改过滤词）。
        let empty = editor::DiagnosticLocation::default();
        assert_eq!(app.jump_to_diagnostic(&empty), editor::CommandResult::NoOp);
    }

    /// P0（脚本迭代闭环）：脚本文件**在盘上**时，点击诊断行就是把它连同行号交给外部编辑器
    /// （编辑器内不做文本视图），同时把控制台过滤收窄到该文件。
    ///
    /// 走 dry-run：只验证"命令已交给外部编辑器这一层"，不在验收门里真的起进程。
    #[test]
    fn diagnostic_jump_hands_an_existing_script_to_the_external_editor() {
        let mut app = test_app();
        app.project_loaded = true;
        app.external_editor_dry_run = true;
        let root = app.project.root.clone();
        std::fs::create_dir_all(root.join("scripts")).expect("scripts dir");
        std::fs::write(
            root.join("scripts/player.lua"),
            "function on_spawn(id) end\n",
        )
        .expect("write script");

        let location = editor::parse_location("ERROR script: load failed scripts/player.lua:12")
            .expect("file");
        assert_eq!(
            app.jump_to_diagnostic(&location),
            editor::CommandResult::Applied
        );
        assert!(app.status.contains("player.lua"), "{}", app.status);
        assert!(
            app.status.contains("12"),
            "状态栏要带上行号：{}",
            app.status
        );
        assert_eq!(app.console_filter, "player.lua", "同时收窄控制台过滤");
    }

    /// 盘上没有的脚本路径**不许**起外部编辑器：只按来源过滤（旧行为保持）。
    #[test]
    fn diagnostic_jump_does_not_launch_an_editor_for_a_missing_script() {
        let mut app = test_app();
        app.project_loaded = true;
        app.external_editor_dry_run = true;
        app.status = "哨兵".to_owned();
        let location =
            editor::parse_location("ERROR script: load failed scripts/gone.lua:3").expect("file");
        assert_eq!(
            app.jump_to_diagnostic(&location),
            editor::CommandResult::Applied
        );
        assert!(
            app.status.contains("已按来源过滤控制台") && !app.status.contains("打开"),
            "缺文件时只过滤，不声称打开了编辑器：{}",
            app.status
        );
    }

    /// 诊断跳转到**项目内已存在**的文档时走统一打开入口（含未保存保护）。
    #[test]
    fn diagnostic_jump_opens_existing_project_document() {
        let mut app = test_app();
        app.project_loaded = true;
        // 在项目根下准备一个真实场景文件（跳转的目标）。
        let root = app.project.root.clone();
        std::fs::create_dir_all(root.join("scenes")).expect("scenes dir");
        let scene_path = root.join("scenes/target.scene");
        let mut target = SceneDocument::new_scene("target");
        target
            .scene
            .add_entity(SceneEntityId(1), "Target", None)
            .unwrap();
        target.save_as(&scene_path).expect("scene saves");

        // 当前文档是 dirty 的：跳转必须走未保存保护（挂起确认），而不是静默丢弃编辑。
        app.document.current_scene.as_mut().unwrap().mark_dirty();
        let location =
            editor::parse_location("scene validation failed scenes/target.scene:1").expect("doc");
        assert!(location.is_document_path());
        assert_eq!(
            app.jump_to_diagnostic(&location),
            editor::CommandResult::Applied
        );
        assert!(
            app.pending_document_change.is_some(),
            "dirty 文档跳转必须挂起未保存确认"
        );
        let _ = std::fs::remove_file(&scene_path);
    }

    /// NewScene 清空历史与选择，并留下一个 dirty 的未保存文档。
    #[test]
    fn new_scene_resets_history_and_selection() {
        let mut app = test_app();
        app.selection
            .set_single(SceneEntityId(1), editor::SelectionSource::Hierarchy);
        assert_eq!(
            app.dispatch(EditorCommand::NewScene {
                name: "level2".to_owned()
            }),
            editor::CommandResult::Applied
        );
        let document = app.current_scene().expect("new scene exists");
        assert_eq!(document.scene.name, "level2");
        assert!(document.is_dirty(), "新建场景必须标记为未保存");
        assert_eq!(app.history.undo_len(), 0);
        assert!(app.selection.is_empty());
    }

    /// Undo/Redo 通过 typed 命令往返，且无历史时是 no-op。
    #[test]
    fn undo_and_redo_roundtrip_through_dispatch() {
        let mut app = test_app();
        assert_eq!(
            app.dispatch(EditorCommand::Undo),
            editor::CommandResult::NoOp
        );
        assert_eq!(
            app.dispatch(EditorCommand::Redo),
            editor::CommandResult::NoOp
        );

        let id = SceneEntityId(1);
        let renamed = {
            let scene = app.document.current_scene.as_mut().expect("scene");
            app.history
                .execute(scene, "Rename", CommandKind::Rename, |scene| {
                    let entity = scene.scene.entities.get_mut(&id).expect("entity");
                    entity.name = "Renamed".to_owned();
                    scene.mark_dirty();
                    Ok::<(), String>(())
                })
        };
        assert_eq!(renamed, Ok(true));

        assert_eq!(
            app.dispatch(EditorCommand::Undo),
            editor::CommandResult::Applied
        );
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.entities[&id].name,
            "Root"
        );
        assert_eq!(
            app.dispatch(EditorCommand::Redo),
            editor::CommandResult::Applied
        );
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.entities[&id].name,
            "Renamed"
        );
    }

    /// Save 保存到项目 scenes/ 目录并把文档置为 clean；再次 Save 仍是 Applied（幂等落盘）。
    #[test]
    fn save_command_writes_document_and_clears_dirty() {
        let mut app = test_app();
        app.project_loaded = true;
        assert_eq!(
            app.dispatch(EditorCommand::Save),
            editor::CommandResult::Applied
        );
        let document = app.current_scene().expect("scene");
        assert!(!document.is_dirty(), "保存后必须置为 clean");
        let path = document.path.clone().expect("保存后应有路径");
        assert!(path.exists(), "场景文件必须落盘：{}", path.display());
        assert!(path.ends_with("scenes/main.scene"));
        assert_eq!(
            app.dispatch(EditorCommand::Save),
            editor::CommandResult::Applied
        );
        let _ = std::fs::remove_file(&path);
    }

    /// SaveAs 写入指定路径并把该路径作为当前文档路径。
    #[test]
    fn save_as_command_writes_to_requested_path() {
        let mut app = test_app();
        let target = std::env::temp_dir().join(format!(
            "rustlua-command-router-saveas-{}.scene",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&target);
        assert_eq!(
            app.dispatch(EditorCommand::SaveAs {
                path: target.clone()
            }),
            editor::CommandResult::Applied
        );
        assert!(target.exists());
        assert_eq!(
            app.current_scene().unwrap().path.as_deref(),
            Some(target.as_path())
        );
        let _ = std::fs::remove_file(&target);
    }

    /// 没有场景时 Save 被拒绝且状态说明原因，不产生文件。
    #[test]
    fn save_without_scene_is_rejected() {
        let mut app = test_app();
        app.document.current_scene = None;
        match app.dispatch(EditorCommand::Save) {
            editor::CommandResult::Rejected(reason) => assert!(!reason.is_empty()),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    /// Play 在无 GPU 环境下必须被拒绝：进入 `StartFailed`（M11-01 状态机），
    /// 不持有运行会话，也不改动编辑器文档（"运行异常不退出编辑器"）。
    #[test]
    fn start_play_without_gpu_is_rejected_and_keeps_document_isolated() {
        let mut app = test_app();
        let before = app.document.current_scene.clone();
        match app.dispatch(EditorCommand::StartPlay) {
            editor::CommandResult::Rejected(reason) => {
                assert!(reason.contains("失败"), "拒绝原因应说明启动失败：{reason}")
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(app.play_state(), PlayState::StartFailed);
        assert!(!app.play_session_active());
        assert!(app.play_session_runtime().is_none());
        assert_eq!(
            app.document.current_scene, before,
            "启动失败不得改动编辑器文档"
        );
    }

    /// Stop 在未 Play 时是 no-op（不产生伪状态迁移）。
    #[test]
    fn stop_without_play_is_noop() {
        let mut app = test_app();
        assert_eq!(
            app.dispatch(EditorCommand::StopPlay),
            editor::CommandResult::NoOp
        );
        assert_eq!(app.play_state(), PlayState::Editing);
    }
}
