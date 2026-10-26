//! 文档切换保护：打开/新建/退出前的"脏文档"闸门，以及待决变更的落盘、取消与恢复。
//! 只决定"要不要问、问完怎么走"，真正的保存与打开在 `document_io`。

use super::*;

impl EditorApp {
    /// 停止 Play：丢弃运行态变更，恢复编辑器快照。
    ///
    /// M11-01 状态机：`Playing → Stopping → Editing`，Stop 期间释放临时世界/GPU 纹理/Lua 状态。
    /// **实现住在 shell**（第 28 轮从 `panels/game_view.rs` 搬来）：Stop 会替换当前文档，
    /// 属于提交边界；面板只发 `EditorCommand::StopPlay`（docs/55 §4.1 规则 1）。
    pub(crate) fn stop(&mut self) {
        let Some(session) = self.play_session.take() else {
            self.status = "当前不在运行模式".to_owned();
            return;
        };
        if let Err(error) = self.play.begin_stop() {
            self.status = error.to_string();
            return;
        }
        self.preview = None;
        self.set_current_scene_document(session.stop());
        // Play/Stop 清空输入状态，避免下一轮 Play 残留边沿（M11-02）。
        self.preview_input_state.clear_all();
        // 统一收尾：选择修复 + Inspector 空同步 + Scene View/Game 预览失效 +
        // Hierarchy 元数据重扫（含 delete_target 复位）。
        self.on_document_changed(DocumentChange::PlaybackStopped);
        let snapshot =
            self.editor_context_snapshot
                .take()
                .unwrap_or_else(|| crate::EditorContextSnapshot {
                    selection: editor::SelectionModel::new(),
                    workspace_tab: crate::WorkspaceTab::Scene,
                    scene_tool: crate::scene_interaction::SceneTool::Select,
                    gizmo_space: crate::scene_interaction::GizmoSpace::default(),
                    gizmo_pivot: crate::scene_interaction::GizmoPivot::default(),
                    scene_view_mode: self.scene_view_mode,
                    scene_zoom: 1.0,
                    scene_pan: egui::Vec2::ZERO,
                    hierarchy_collapsed: self.scene_state.collapsed.clone(),
                    preview_resolution: self.preview_resolution,
                    game_camera_2d_center: self.game_camera_2d_center,
                });
        self.restore_editor_context(snapshot);
        if let Err(error) = self.play.finish_stop() {
            tracing::warn!(target: "editor", "[preview] state machine: {error}");
        }
        self.status = "运行已停止，运行期改动已丢弃".to_owned();
    }

    /// 实际替换为崩溃快照文档（仅由确认流或 clean 快速路径调用）。
    ///
    /// **实现住在 shell**（第 28 轮从 `panels/header.rs` 搬来）：它会替换当前文档。
    pub(crate) fn perform_recover_candidate(&mut self, candidate: editor::RecoveryCandidate) {
        self.set_current_scene_document(self.recovery.recover(&candidate));
        self.history.clear();
        self.autosave.reset();
        // 统一收尾：选择修复 + Inspector 空同步 + Scene View/Game 预览失效 +
        // Hierarchy 元数据重扫（快照替换后强制隐藏/锁定集合重建）。
        self.on_document_changed(DocumentChange::Opened);
        self.refresh_recovery_scan();
        self.status = format!(
            "已恢复崩溃快照：{}",
            crate::util::display_path(&candidate.path)
        );
    }

    /// E06 网关：请求一次文档离开操作。
    ///
    /// - 若当前没有任何编辑文档 → 无 dirty 可言，直接执行。
    /// - 若当前编辑文档 dirty → 挂起 pending（不触碰文档 / 选择 / 视图 / 历史），
    ///   由顶层确认窗裁决；同一时刻已挂起其他 pending 时直接忽略（防重复弹窗）。
    /// - 若当前编辑文档 clean → 立即执行（保持 E06 前各入口行为，测试路径不变）。
    pub(crate) fn request_document_change(&mut self, change: PendingChange) {
        if self.pending_document_change.is_some() {
            // 已有待确认操作：防止重复触发切换请求与重复弹窗。
            return;
        }
        // M79 P1-4：in-context 编辑中换文档会把「回到场景」的口袋整份丢掉（场景文档与它的
        // 撤销栈只存在那儿）。闸门放在**这里**而不是 `perform_pending_change`，因为脏阶段文档
        // 在下一行就会被挂成 pending、弹出「保存并继续」——那时用户已经选了"保存"，再拒绝
        // 只会让他对着一个既不保存也不切换的对话框发懵。
        // 阶段文档 clean 时先静默返回场景（没东西可丢），再让**原来的**闸门重新判一遍：
        // 这时当前文档是那份场景，它若 dirty 就会正常弹窗，而不是把用户的场景改动静默换掉。
        if self.prefab_stage.is_some() {
            if self.prefab_stage_unsaved() {
                self.status = format!(
                    "已阻止{action}：正在预制体编辑上下文中（{source} 有未保存改动）。请先用横幅里的「保存并返回」或「放弃并返回」。",
                    action = change.action_label(),
                    source = self
                        .prefab_stage
                        .as_ref()
                        .map(|stage| stage.source.clone())
                        .unwrap_or_default(),
                );
                self.set_scene_view_diagnostic(self.status.clone());
                return;
            }
            self.close_prefab_stage(false);
        }
        let dirty = self
            .document
            .current_scene
            .as_ref()
            .is_some_and(editor::SceneDocument::is_dirty);
        if dirty {
            self.pending_document_change = Some(change);
            self.pending_save_failed = false;
            return;
        }
        self.perform_pending_change(change);
    }

    /// 执行一条 pending / 已确认的文档离开操作（唯一的实际切换执行点）。
    pub(crate) fn perform_pending_change(&mut self, change: PendingChange) {
        match change {
            PendingChange::OpenResource { relative_path } => {
                self.perform_open_scene_resource(relative_path);
            }
            PendingChange::CreateScene { name } => self.perform_create_scene(name),
            PendingChange::CreateWidget { name } => self.perform_create_widget(name),
            PendingChange::Recover { candidate } => self.perform_recover_candidate(candidate),
            PendingChange::SwitchProject { path } => {
                self.import_path = path;
                self.start_import();
            }
            PendingChange::QuitEditor => {}
        }
    }

    /// 取消挂起的离开操作：清空 pending，文档 / 选择 / 视图 / 历史保持原样。
    pub(crate) fn cancel_pending_change(&mut self) {
        self.pending_document_change = None;
        self.pending_save_failed = false;
    }

    /// E06：按用户决策推进挂起的离开操作（弹窗按钮 → 此处，便于单测三分支语义）。
    ///
    /// - Save：先保存，成功才离开；失败只置 `pending_save_failed`，文档保持 dirty 且不离开。
    /// - Discard：不保存直接离开。
    /// - Cancel：取消挂起操作，文档保持不变。
    ///
    /// 返回是否已把 pending 消费掉（Save 失败返回 false，便于弹窗决定是否重绘）。
    pub(crate) fn resolve_pending_change(
        &mut self,
        context: &egui::Context,
        decision: PendingDecision,
    ) -> bool {
        match decision {
            PendingDecision::Save => {
                if self.save_scene() {
                    self.pending_save_failed = false;
                    self.finish_pending_change(context);
                    true
                } else {
                    // 保存失败：保留 pending，文档保持 dirty，不执行离开操作。
                    self.pending_save_failed = true;
                    false
                }
            }
            PendingDecision::Discard => {
                self.pending_save_failed = false;
                self.finish_pending_change(context);
                true
            }
            PendingDecision::Cancel => {
                self.cancel_pending_change();
                self.status = "已取消：文档保持不变".to_owned();
                true
            }
        }
    }

    /// 单次消费挂起的离开操作并执行（防重复触发）。QuitEditor 确认后发送 Close，
    /// 其余交由 `perform_pending_change` 走各自 perform 路径。
    pub(crate) fn finish_pending_change(&mut self, context: &egui::Context) {
        if let Some(change) = self.pending_document_change.take() {
            self.pending_save_failed = false;
            if matches!(change, PendingChange::QuitEditor) {
                // Close 会再触发一次 CloseRequested（放弃路径文档仍 dirty），
                // 凭此标志让 guard_close_request 放行，否则重新弹窗关不掉。
                self.quit_approved = true;
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                self.perform_pending_change(change);
            }
        }
    }
}
