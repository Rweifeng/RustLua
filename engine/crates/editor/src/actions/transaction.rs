//! 编辑事务与撤销栈：开/合/取消一次连续编辑，Inspector 脏编辑的提交边界，
//! 以及撤销/重做的唯一入口。所有写文档的路径最终都收敛到这里。

use super::*;

impl EditorApp {
    /// 统一的文档变更入口（E02）：任意一次成功提交的文档变更后调用，
    /// 按「选择修复 → Inspector 同步 → Scene View 失效 → Game Preview 失效 →
    /// Hierarchy 元数据重扫」的固定顺序收敛各面板状态，并清掉可能误命中的画面缓存。
    ///
    /// 约定：
    /// - 调用点先写入/替换 `document.current_scene`（或内部变更），再调本入口；
    /// - 面板选择修复与 Inspector 同步都以**编辑文档**（`document.current_scene`）为准；
    /// - `DocumentChange::PlaybackStopped` 时使用 Play 会话的编辑器快照（此时已取回
    ///   到 `current_scene`），其余变更类别由各入口保持等价行为。
    pub(crate) fn on_document_changed(&mut self, change: DocumentChange) {
        // 1. 选择修复。
        //
        // 整表切换路径（打开/新建/恢复/Stop）把 `current_scene` 替换为**另一份文档**，
        // 必须无条件清空全部选择（与各入口历史行为一致：恢复/Stop 后即使实体 ID 仍
        // “合法”也不得把旧选区保留到新文档上），并复位 delete_target。
        //
        // 其余（Undo/Redo/删除/重命名/Prefab）是**同一文档内的编辑变更**：只清除指向
        // 已不存在实体的悬空/僵尸选择；仍在原文档中的选中实体保持选中并随后同步。
        let document_swapped = matches!(
            change,
            DocumentChange::Opened | DocumentChange::Created | DocumentChange::PlaybackStopped
        );
        if document_swapped {
            self.selection.clear(editor::SelectionSource::Playback);
            self.delete_target = crate::DeleteTarget::None;
        } else {
            // 以当前编辑文档为准，仅清除指向不存在实体的悬空/僵尸选择。
            let active = self.document.current_scene.clone();
            let valid = |id: SceneEntityId| {
                active
                    .as_ref()
                    .is_none_or(|scene| scene.scene.entities.contains_key(&id))
            };
            let had_selection = !self.selection.is_empty();
            self.selection.retain_valid(valid);
            if had_selection && self.selection.is_empty() {
                self.delete_target = crate::DeleteTarget::None;
            }
        }

        // 2. Inspector 同步：selection 的主选中是 Inspector 内容的主来源（无主选中则保持
        // 空选择，不得把旧快照残留给一个不存在的实体）。
        if let Some(selected) = self.selection.primary()
            && self
                .document
                .current_scene
                .as_ref()
                .is_some_and(|scene| scene.scene.entities.contains_key(&selected))
        {
            self.sync_inspector(selected);
        }

        // 3. Scene View 失效：2D/3D 回读缓存、场景运行时、Game 画面离屏纹理缓存。
        //    （Open/Undo/Stop 等路径在替换 current_scene 后还会单独调用此函数；
        //    这里在纯编辑变更（删除/重命名/Prefab）时也幂等执行，代价可控。）
        self.invalidate_scene_view_state();
        self.invalidate_ui_layout_cache();
        self.refresh_preview_diagnostics();

        // 4. Game Preview 失效：整文档替换类变更（打开/新建/恢复）不允许残留运行时预览。
        if change == DocumentChange::Opened || change == DocumentChange::Created {
            self.play_session = None;
            self.preview = None;
            self.preview_input_state.clear_all();
        }

        // 5. Hierarchy 折叠状态同步：打开读配置、新建清空、同文档编辑剔除悬空 ID。
        self.sync_hierarchy_collapsed(change);

        // 6. Hierarchy 元数据刷新：置空 key 强制下一帧 sync_hierarchy_metadata 全量重扫，
        //    避免 (path, revision) 恰好相同的文档复用上一份 hidden/locked 集合。
        self.last_metadata_key = None;
    }

    pub(crate) fn undo(&mut self) {
        let started = std::time::Instant::now();
        let label = {
            let h = &mut self.history;
            let d = &mut self.document;
            d.current_scene
                .as_mut()
                .and_then(|x| h.undo(x).ok())
                .map(str::to_owned)
        };
        self.profiler.record_command("undo", started.elapsed());
        if label.is_some() {
            self.on_document_changed(DocumentChange::Undo);
        }
        self.status = label.map_or_else(|| "Nothing to undo".into(), |x| format!("Undo: {x}"));
    }

    /// E07 事务网关：撤销必须先取消活动事务（取消不产生历史、不改 dirty）；
    /// 之后 `undo()` 自身走统一入口（on_document_changed(Undo)）。
    pub(crate) fn undo_with_transaction_guard(&mut self) {
        // E08：撤销前先把 Inspector 挂起缓冲收敛（diff 驱动：无论 pending 是否
        // 置位，只要缓冲与文档有差异就直写 + 提交为一条历史），避免残留脏缓冲在
        // 撤销后仍直写文档覆盖刚回退的内容。无差异时 no-op。
        self.finish_inspector_edits_if_dirty();
        if self.edit_transaction.is_active() {
            self.cancel_edit_transaction();
        }
        self.undo();
    }

    /// E07 事务网关：提交活动事务 —— 从历史记录当前文档状态并标记 dirty，
    /// 之后触发与 Undo/Redo 一致的统一刷新（无主选中则不同步 Inspector，与
    /// 现有删除/重命名路径一致）。空操作（内容无差异）自动成为 no-op 提交。
    ///
    /// 返回是否有内容差异被真正提交（`true` 表示追加了一条 Undo 并需保存）。
    pub(crate) fn commit_edit_transaction(&mut self) -> bool {
        let Some(doc) = self.document.current_scene.as_mut() else {
            self.edit_transaction.active = false;
            self.edit_transaction.consumed = true;
            return false;
        };
        let changed = self.history.commit_transaction(doc).unwrap_or(false);
        if !self.edit_transaction.consumed {
            self.edit_transaction.active = false;
            self.edit_transaction.consumed = true;
        }
        if changed {
            doc.mark_dirty();
            self.on_document_changed(DocumentChange::EntityDeleted);
            // 提交点（松手/文本提交/失焦）已把缓冲落成历史；此后缓冲与文档一致，
            // 清空脏标记避免残留直写。若提交是空操作（内容无差异），则缓冲本就与
            // 文档一致，保留 pending 供随后帧的 commit 收敛复位。
            if !self.inspector_edit.pending {
                self.inspector_edit.pending = false;
                self.inspector_edit.consumed = false;
            }
        }
        changed
    }

    /// E07 事务网关：取消活动事务 —— 文档还原到事务开始前快照，不追加历史，
    /// 不标记 dirty；随后走统一入口收敛选择/Inspector/视图。
    pub(crate) fn cancel_edit_transaction(&mut self) {
        if let Some(doc) = self.document.current_scene.as_mut() {
            let _ = self.history.cancel_transaction(doc);
        }
        if !self.edit_transaction.consumed {
            self.edit_transaction.active = false;
            self.edit_transaction.consumed = true;
        }
        self.on_document_changed(DocumentChange::EntityDeleted);
    }

    /// E07：是否处于活动事务（顶层快捷键/帧结束决策用）。
    pub(crate) fn is_edit_transaction_active(&self) -> bool {
        self.edit_transaction.is_active() && self.history.is_transaction_active()
    }

    /// E08：Inspector 面板在帧内记录了一次缓冲字段编辑（拖动/键入/点击），并
    /// 把最新整组缓冲**直写**进活动事务（净差异自动开事务；逐帧直写驱动 Scene
    /// View revision 刷新，但**不写历史、不写盘**）。
    ///
    /// 调用点应只在真正改动缓冲时调用（净差异为空自动空操作）；实际提交（历史 +
    /// 保存 + Inspector 同步 + 基线刷新）由 `commit_inspector_edits_if_dirty` 在
    /// 提交点（松手/文本提交/失焦/顶层快捷键前）统一执行。
    pub(crate) fn mark_inspector_edit(&mut self) {
        let dirty = self.update_inspector_write();
        // E08：只有当缓冲相对基线有真实差异并完成直写，才置位 pending ——
        // 之后由提交点收敛为一条历史。无差异（如仅点击/聚焦）不置位，避免
        // 未提交标记在后续帧把无关活动事务（如 Scene View 拖拽）误收敛。
        if dirty {
            self.inspector_edit.pending = true;
            self.inspector_edit.consumed = false;
        }
    }

    /// E08：把当前 Inspector 缓冲相对基线有差异的部分直写文档。若尚未处于活动
    /// 事务则自动开一条「Inspector」Property 事务（before = 提交前的文档快照），
    /// 后续提交/取消统一收尾。拖动中每次改动都会进入本路径，因此 net 变化决定
    /// 是否需要写。
    ///
    /// 返回本帧是否有内容差异被直写（区别于“已提交”）。
    pub(crate) fn update_inspector_write(&mut self) -> bool {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return false;
        }
        let Some(snapshot) = self.current_inspector_edit_snapshot() else {
            return false;
        };
        // 目标实体已不在编辑文档（被删除/撤销）：无可写对象，直接放弃，避免为
        // 一个不存在实体的空模板开一条悬挂事务。
        if !snapshot.doc_present {
            return false;
        }
        // 缓冲与基线一致（无净变化）→ 空操作。
        if self.inspector_baseline == snapshot && !self.inspector_edit.pending {
            return false;
        }
        // 自动开事务（幂等：拖动中已开则沿用）。Scene View 拖拽的事务不同 ——
        // 正常 UI 同一物理指针只能操作一个目标，Escape/Undo 的取消在顶层统一处理。
        let doc_id = snapshot.doc_id;
        self.begin_edit_transaction("Inspector", editor::CommandKind::Property, Some(doc_id));
        let EditSnapshot {
            snapshot:
                Some(InspectorSnapshot {
                    entity_id,
                    name,
                    lua_script,
                    collider3d,
                    container,
                    layout_item,
                    ..
                }),
            tilemap,
            animation,
            ..
        } = snapshot
        else {
            return false;
        };
        let Some(scene) = self.document.current_scene.as_mut() else {
            return false;
        };
        // 多选批量由通用编辑器的批量命令承担（第 33 轮）；这条缓冲直写路径只处理剩下的
        // 手写面板（Transform/Collider3d/Container/Tilemap/Animation/Script）。
        let multi_sprite_ids = self.selection.ids();
        let inspector = self.inspector.clone();
        // 注：`Light` / `MeshRenderer` 已迁到 schema 生成的字段编辑器（走 SetComponentField），
        // 因此不再出现在这条缓冲直写路径里。
        let write = inspector.write_inspector(
            scene,
            entity_id,
            InspectorSnapshot {
                entity_id,
                name,
                lua_script,
                collider3d,
                container,
                layout_item,
            },
            tilemap.as_deref(),
            animation.as_deref(),
            &multi_sprite_ids,
        );
        match write {
            Ok(changed) => {
                self.status = format!("Inspector 已更新：EntityId={}", entity_id.0);
                changed
            }
            Err(error) => {
                self.status = error.to_string();
                false
            }
        }
    }

    /// E08：以「面板当前缓冲」重建一份 `EditSnapshot` —— 即面板要**写回文档**的
    /// 完整内容，与 `write_inspector` 的参数逐字段一致（编码源见各组件 `encode_*`
    /// 与 `to_component_str`）。缓冲与基线比较的是这份待写内容，因此任何数值拖动 /
    /// 文本键入（name/transform/3D/UI/mesh/sprite/tilemap/animation/lua…）都会产生
    /// 真实净 diff；面板未编辑时快照恰与基线相等 → no-op，不误开事务不误写。
    pub(crate) fn current_inspector_edit_snapshot(&self) -> Option<EditSnapshot> {
        let doc = self.document.current_scene.as_ref()?;
        let primary = self.selection.primary()?;
        let mut targets = self.selection.ids();
        if targets.is_empty() {
            targets.push(primary);
        }
        let doc_present = doc.scene.entities.contains_key(&primary);
        if !doc_present {
            return Some(EditSnapshot {
                doc_id: primary,
                doc_present: false,
                editing_entities: targets,
                snapshot: None,
                tilemap: None,
                animation: None,
            });
        }
        // 各 3D/UI 组件的存在性由文档真值决定：实体已有该组件才把缓冲字段编入
        // 快照（写入端因此不会给普通实体注入默认组件）；缓冲编辑面（拖数值/改文本）
        // 只作用于已存在的组件，编辑前后快照相应字段都保持 Some，diff 正常生效。
        //
        // 带引用的组件（Sprite/mesh/lua）在用户清空缓冲时按「空 → clear」规范化成
        // None —— 与旧 apply_inspector 的空值移除语义一致，也避免把空串交给 setter
        //（校验拒绝空引用）而让直写整体失败。
        let entity = &doc.scene.entities[&primary];
        let has = |key: &str| entity.components.contains_key(key);
        let lua_script = has("Script")
            .then(|| (!self.lua_script.trim().is_empty()).then_some(self.lua_script.clone()))
            .flatten();
        let snapshot = InspectorSnapshot {
            entity_id: primary,
            name: self.entity_name.clone(),
            lua_script,
            collider3d: has("Collider3d").then_some(self.collider3d.clone()),
            container: has("Container").then_some(self.container.clone()),
            layout_item: has("LayoutItem").then_some(self.layout_item.clone()),
        };
        // Tilemap / Animation：面板编辑器状态实时编码整串。实体没有该组件时保持
        // None（写入端不写、比较端与基线 None 相等）；面板未编辑时编码串恰与
        // 文档组件串一致 → 与基线相等 → 不误写。
        let tilemap = has("Tilemap").then(|| self.tilemap.map.to_component_str());
        let animation = has("Animation").then(|| self.animation.machine.to_component_str());
        Some(EditSnapshot {
            doc_id: primary,
            doc_present: true,
            editing_entities: targets,
            snapshot: Some(snapshot),
            tilemap,
            animation,
        })
    }

    /// E08：提交后/同步后把基线刷新为「当前缓冲」（直写/回读后缓冲即“已提交”
    /// 状态），供下一轮编辑比较。
    pub(crate) fn refresh_inspector_baseline(&mut self) {
        self.inspector_baseline = self
            .current_inspector_edit_snapshot()
            .unwrap_or_else(EditSnapshot::empty);
    }

    /// E08 提交点：Inspector 缓冲挂起编辑到达松手/文本提交/失焦边界。把挂起缓冲
    /// 直写进活动事务后提交为**一条**历史，净变化才保存；随后同步缓冲回读文档并
    /// 刷新基线。无净变化时为 no-op（不产生空历史、不落盘）。
    pub(crate) fn commit_inspector_edits_if_dirty(&mut self) {
        if !self.inspector_edit.pending {
            return;
        }
        // 先收敛本帧缓冲（同一交互最后一段直写），再提交事务。
        self.update_inspector_write();
        if self.is_edit_transaction_active() {
            self.commit_edit_transaction();
        }
        self.inspector_edit.pending = false;
        self.inspector_edit.consumed = false;
        if self.selection.primary().is_some() {
            // 本提交点已把缓冲落成一条历史：回读文档同步缓冲（修正编码规范化
            // 漂移），下一次编辑从与文档一致的状态重新比较。
            if let Some(selected) = self.selection.primary() {
                self.sync_inspector(selected);
            }
        }
        self.refresh_inspector_baseline();
    }

    /// E08 帧尾提交边界：只要缓冲与基线有**真实差异**就收敛（直写 → 提交 → 同步 →
    /// 刷新基线），供数值拖动松手、TextEdit 失焦/Enter、Escape 等边界调用。
    /// 与 [`EditorApp::commit_inspector_edits_if_dirty`] 的差异：后者要求 `pending`
    /// 已置位（由交互回调同步 mark），而本函数以 diff 为准，即使标记丢失也不会漏提交。
    ///
    /// 返回本边界是否把缓冲收敛为了一条**净差异历史**（`true` = 追加了一条 Undo，
    /// 需保存；空操作/无净差异返回 `false`，不落盘、不产生空历史）。注意该返回值
    /// 是「提交结果」而非「本帧是否直写」：同一拖拽的最后一段差异通常已由先前
    /// 帧的 `mark_inspector_edit` 直写进文档，到达本边界时 `update_inspector_write`
    /// 常为 no-op，但挂起的活动事务仍会被提交为一条历史。
    pub(crate) fn finish_inspector_edits_if_dirty(&mut self) -> bool {
        // 无主选/无场景/播放中 → 无内容可提交。
        if self.is_playing() || self.selection.primary().is_none() {
            return false;
        }
        let pending = self.inspector_edit.pending;
        // 有真实 diff 才需要进入直写（与 `update_inspector_write` 的净差异判定一致）；
        // 仅 pending（末段差异已由前帧 mark 直写）也需进入提交。
        let dirty = self.update_inspector_write();
        if !dirty && !pending {
            return false;
        }
        let committed = if self.is_edit_transaction_active() {
            self.commit_edit_transaction()
        } else {
            false
        };
        self.inspector_edit.pending = false;
        self.inspector_edit.consumed = false;
        if dirty && let Some(selected) = self.selection.primary() {
            self.sync_inspector(selected);
        }
        self.refresh_inspector_baseline();
        committed
    }

    /// E08：活动事务被其它路径（Scene View 拖动、gizmo、删除快捷键…）接管时，
    /// Inspector 缓冲编辑已随 mark 直写进各自事务（未开 Inspector 事务时无残留）——
    /// 调用点负责让 Scene View 侧事务统一提交；本函数只复位缓冲状态供下一条
    /// Inspector 编辑重新比较。
    pub(crate) fn clear_inspector_edit_pending(&mut self) {
        self.inspector_edit.pending = false;
        self.inspector_edit.consumed = false;
    }

    /// E08 帧末统一提交边界（update 最末调用）：Inspector 缓冲提交的兜底。
    /// 只要本帧发生 primary 释放（把 Inspector 编辑拖出面板 / 点击 Scene / 其它
    /// 窗口离开面板）或 Enter（文本显式提交），就把挂起缓冲收敛为一条历史。
    /// 正在键入文本（键盘焦点被占用）时鼠标释放不触发，避免 `sync_inspector`
    /// 回读覆盖仍在编辑的输入框；Enter 是显式提交故除外。无挂起编辑时 no-op。
    pub(crate) fn flush_frame_edit_boundary(&mut self, context: &egui::Context) {
        if self.is_playing() {
            return;
        }
        let releasing =
            context.input(|i| i.pointer.any_released()) && !context.wants_keyboard_input();
        let entering = context.input(|i| i.key_pressed(egui::Key::Enter));
        if releasing || entering {
            self.finish_inspector_edits_if_dirty();
        }
    }

    /// E08：Escape 取消活动事务（拖动 / Inspector 编辑的“取消”语义）。文档回滚
    /// 到事务 before，不追加历史、不保存；由 cancel 的统一入口回读缓冲与视图。
    /// 正在键入文本时 egui 用 Escape 放弃输入焦点，故不在此打断（等下一交互收敛）。
    /// 返回是否执行了取消（调用点据此清理 Scene 拖拽残留等）。
    pub(crate) fn cancel_edit_on_escape(&mut self, context: &egui::Context) -> bool {
        if context.wants_keyboard_input() || !context.input(|i| i.key_pressed(egui::Key::Escape)) {
            return false;
        }
        self.clear_inspector_edit_pending();
        if self.is_edit_transaction_active() {
            self.cancel_edit_transaction();
        } else if let Some(selected) = self.selection.primary() {
            // 无活动事务但有未收敛的缓冲残留：回读文档，丢弃未提交改动。
            self.sync_inspector(selected);
        }
        true
    }

    /// E08：Escape 取消后清理 Scene View 拖拽残留 —— 文档已回滚到事务前，若还
    /// 留着 `drag`/`gizmo_drag`/preview，下一帧会把 stale preview 继续写回文档
    /// （相当于把已取消的拖动又“复活”）。调用点仅在确实取消后调用。
    pub(crate) fn clear_active_scene_drag(&mut self) {
        // E11：交互阶段由状态机持有，取消时必须让它回到 Idle，
        // 否则下一帧还会按"仍在框选/拖动"处理指针。
        self.interaction
            .handle(crate::scene_interaction::InteractionEvent::Cancel);
        self.scene_state.drag = None;
        self.scene_state.gizmo_drag = None;
        self.scene_state.preview.clear();
        self.scene_state.preview_ui.clear();
        self.scene_state.preview3d_transform.clear();
    }

    /// E07 事务网关：开始一次连续编辑事务 —— 记录当前文档为 before 快照并置位
    /// 顶层事务状态。后续多帧预览只更新文档、不写历史，由 commit/cancel 收尾。
    ///
    /// 返回是否成功开始。已处于事务中时返回 false（禁止嵌套）。
    pub(crate) fn begin_edit_transaction(
        &mut self,
        label: &str,
        kind: editor::CommandKind,
        target: Option<SceneEntityId>,
    ) -> bool {
        if self.edit_transaction.is_active() {
            return false;
        }
        let Some(doc) = self.document.current_scene.as_ref() else {
            return false;
        };
        if self
            .history
            .begin_transaction(doc, label, kind, target)
            .is_err()
        {
            return false;
        }
        self.edit_transaction.active = true;
        self.edit_transaction.consumed = false;
        true
    }

    pub(crate) fn redo(&mut self) {
        let started = std::time::Instant::now();
        let label = {
            let h = &mut self.history;
            let d = &mut self.document;
            d.current_scene
                .as_mut()
                .and_then(|x| h.redo(x).ok())
                .map(str::to_owned)
        };
        self.profiler.record_command("redo", started.elapsed());
        if label.is_some() {
            self.on_document_changed(DocumentChange::Redo);
        }
        self.status = label.map_or_else(|| "Nothing to redo".into(), |x| format!("Redo: {x}"));
    }
}
