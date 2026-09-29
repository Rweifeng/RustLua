//! 跨面板共享的编辑命令：撤销/重做、保存、场景切换、实体创建、Inspector 同步、对齐。

use crate::AlignMode;
use crate::EditorApp;
use crate::geometry::{encode_transform, parse_transform};
use crate::panels::inspector::diff_prefab_entity;
use crate::util::strip_verbatim_prefix;
use editor::{
    Collider3dFields, CommandKind, CommandResult, InspectorError, InspectorSnapshot, SceneDocument,
    TransformFields,
};
use eframe::egui;
use engine::core::{
    animation::AnimationStateMachine,
    prefab::Prefab,
    scene::{SceneEntity, SceneEntityId},
    sprite::SpriteComponent,
    tilemap::Tilemap,
};
use std::path::{Path, PathBuf};

/// E08 Inspector「脏编辑」标记：用户在一次连续交互中改动了缓冲字段，但
/// 尚未到提交边界（松手/文本提交/失焦）。编辑帧由 `mark_inspector_edit`
/// 把最新缓冲同步进一条自动事务并直写文档（驱动 Scene View revision 刷新），
/// 真正的历史/保存延迟到提交点。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EditState {
    pub pending: bool,
    /// 这条脏编辑是否已被消费进活动事务（防同一次拖动逐帧重复 begin）。
    pub consumed: bool,
}

/// E08 Inspector 缓冲基线：与文档上次同步（`sync_inspector`）或提交成功后的
/// 「已提交快照」。Inspector 面板编辑帧把整组缓冲字段与基线做差异比较，
/// 有差异才把整组字段直写文档（自动开事务、不写历史、不落盘）。
///
/// `InspectorSnapshot` 覆盖 `write_inspector` 的主要参数（name/transform/sprite/
/// lua/3D/UI 组件），其字段全部从 **EditorApp 当前缓冲**（`entity_name`/
/// `transform`/`transform3d`/`mesh_renderer`/`camera3d`/…/`text`/`image`）编码而来，
/// 与 `write_inspector` 写回的内容逐字段一致 —— 缓冲改动因此产生真实净 diff。
/// 面板显示层的裸引用/组件对象（`mesh_renderer` 的 `mesh=` 前缀）只作为「编码源」，
/// 不直接参与比较，避免显示层噪音造成空操作直写。
///
/// 写入端（`update_inspector_write`）要求写入字段与文档实际字段**逐字节一致**
/// 才能让 `write_inspector` 的 `Ok(*document != before)` 净差异判定正确：
/// - `mesh_renderer`：文档经 `MeshRenderer::to_component_str` 规范化
///   （`mesh=<path>`）。缓冲只存裸引用，比较前拼上 `mesh=` 前缀生成待写值；
///   实体**没有** MeshRenderer 组件时快照为 `None`，write 端会 clear（不会注入）。
/// - `Sprite` / `Light` / `Text` / `Image` / `UiLayout` / `Camera3D` / `WorldText` 都已迁到
///   schema 生成的字段编辑器（第 20–34 轮），因此**不在**这条缓冲直写路径里。
/// - `persistent`：`write_inspector` 不写该位，diff 由 `persistent`（文档真值）
///   单独负责，避免面板残留的 false 在每次直写时把「未变更」判成变更。
/// - `tilemap`/`animation`：从面板 `TilemapEditorState`/`AnimationEditorState`
///   实时编码的整串（`Some` 表示应保留/写回组件；None = 实体无该组件时不写）。
///
/// 附带实体存在性代理：`editing_entities` 在缓冲编辑期间携带全部被编辑实体
/// （主选 + Sprite 多选从属），以免目标实体被删除/撤销掉后误伤。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditSnapshot {
    pub doc_id: SceneEntityId,
    /// 目标（快照来源）实体是否仍存在于当前编辑文档。
    pub doc_present: bool,
    /// 面板主选要写回的实体 ID（可能已被撤销到不存在 —— `editing_entities` 兜底）。
    pub editing_entities: Vec<SceneEntityId>,
    pub snapshot: Option<InspectorSnapshot>,
    /// 面板 buffer 的 persistent 与文档真值一致后才不会误写。
    pub persistent: bool,
    pub tilemap: Option<String>,
    pub animation: Option<String>,
}

impl EditSnapshot {
    /// “空”占位：无文档/无主选时使用（doc_id 取 max，保证与任何真实快照不等）。
    pub fn empty() -> Self {
        Self {
            doc_id: SceneEntityId(u64::MAX),
            doc_present: false,
            editing_entities: Vec::new(),
            snapshot: None,
            persistent: false,
            tilemap: None,
            animation: None,
        }
    }

    pub fn clear(&mut self) {
        *self = Self::empty();
    }
}

/// 一次已提交的文档变更类别（E02）：驱动 `on_document_changed` 的统一刷新流程。
///
/// 打开/新建/恢复、Undo/Redo、运行时 Stop 会把 `current_scene` 整个替换为新快照；
/// 实体删除/重命名/Prefab 重建在 history 闭包内写克隆文档后同样需要统一收尾，
/// 确保 Hierarchy / Inspector / Scene View / Game 预览显示同一份数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocumentChange {
    /// 打开磁盘场景/Prefab 文档（替换当前编辑文档）。
    Opened,
    /// 新建空白场景（未保存）。
    Created,
    /// 删除实体（含其子树）。
    EntityDeleted,
    /// 重命名实体。
    EntityRenamed,
    /// 变更 UI Canvas 根节点标记。
    UiRootChanged,
    /// 重挂载实体到新父节点。
    EntityReparented,
    /// 重新应用 / 写回 Prefab 源后重建实例（实体整体被替换）。
    PrefabApply,
    /// Undo：整文档回退到历史快照。
    Undo,
    /// Redo：整文档前进到历史快照。
    Redo,
    /// 退出 Play：编辑文档恢复为 Stop 时取回的编辑器快照。
    PlaybackStopped,
}

/// E06：一次待确认的文档离开操作。任何可能离开当前编辑文档的入口
/// （场景/Prefab 打开、新建场景、崩溃快照恢复、切换项目、关闭编辑器）
/// 都会经 `request_document_change` 请求。若当前编辑文档 dirty，该操作被挂起，
/// 等顶层「保存并继续 / 放弃并继续 / 取消」弹窗裁决后再执行对应 perform；
/// 若 clean 则立即执行，行为与 E06 之前完全一致。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PendingChange {
    /// 打开另一份场景 / Prefab 文档（相对项目根路径）。
    OpenResource { relative_path: PathBuf },
    /// 新建空白场景（尚未保存）。
    CreateScene { name: String },
    /// E19: create an isolated Widget document.
    CreateWidget { name: String },
    /// 恢复一条崩溃恢复快照。
    Recover {
        candidate: editor::RecoveryCandidate,
    },
    /// 切换游戏项目（新建 / 打开 / 最近项目；走后台导入）。
    SwitchProject { path: String },
    /// 关闭编辑器窗口（用户点窗口 X / 菜单退出）。
    QuitEditor,
}

impl PendingChange {
    /// 弹窗标题下对离开动作的短描述。
    pub(crate) fn action_label(&self) -> String {
        match self {
            Self::OpenResource { relative_path } => {
                format!("打开 {}", relative_path.display())
            }
            Self::CreateScene { name } => format!("新建场景 {name}"),
            Self::CreateWidget { name } => format!("新建 Widget {name}"),
            Self::Recover { .. } => "恢复崩溃快照".to_owned(),
            Self::SwitchProject { path } => format!("切换到项目：{}", path),
            Self::QuitEditor => "关闭编辑器".to_owned(),
        }
    }
}

/// E06 确认窗的用户决策（由顶层弹窗的按钮触发；独立成枚举便于回归测试）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingDecision {
    /// 「保存并继续」：保存成功才执行离开操作；失败则停留并保留 dirty。
    Save,
    /// 「放弃并继续」：不保存，直接执行离开操作。
    Discard,
    /// 「取消」：取消挂起的离开操作，文档 / 选择 / 视图 / 历史保持不变。
    Cancel,
}

impl EditorApp {
    /// 停止 Play：丢弃运行态变更，恢复编辑器快照。
    ///
    /// M11-01 状态机：`Playing → Stopping → Editing`，Stop 期间释放临时世界/GPU 纹理/Lua 状态。
    /// **实现住在 shell**（第 28 轮从 `panels/game_view.rs` 搬来）：Stop 会替换当前文档，
    /// 属于提交边界；面板只发 `EditorCommand::StopPlay`（docs/55 §4.1 规则 1）。
    pub(crate) fn stop(&mut self) {
        let Some(session) = self.play_session.take() else {
            self.status = "Not currently in Play mode".to_owned();
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
        self.status = "Play stopped; runtime changes were discarded".to_owned();
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
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                self.perform_pending_change(change);
            }
        }
    }
}

impl EditorApp {
    /// 用统一入口替换当前编辑场景文档，确保 Scene View 相关状态同步失效。
    pub(crate) fn set_current_scene_document(&mut self, document: SceneDocument) {
        self.document.current_scene = Some(document);
        // H02/H03：文档切换后行内重命名会话与 Shift 锚点立即失效。
        self.hierarchy_rename = None;
        self.selection_anchor = None;
        self.invalidate_scene_view_state();
        // 仅在文档整表切换（打开/新建/恢复/Stop 取回）时把 2D 视图重新适配到
        // 设计分辨率包围框；同文档编辑提交不走这里，不会重置用户 zoom/pan。
        self.scene_view_fit_pending = true;
    }

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
    fn apply_component_edit_on_selection(
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

    /// 从磁盘打开一个场景/Prefab 文档，并通过统一入口替换当前文档。
    pub(crate) fn open_scene_document(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        let display = path.as_ref().display().to_string();
        let document = SceneDocument::open(path).map_err(|error| error.to_string())?;
        self.set_current_scene_document(document);
        self.status = format!("已打开场景：{display}");
        Ok(())
    }

    /// 场景文档切换后，强制让 Scene View 在下一帧重建运行时并重新渲染。
    /// 否则两个刚打开的文档都处于 `revision=0` 时，旧缓存可能被误命中。
    ///
    /// 除 2D/3D 回读缓存外，还须把「游戏画面」离屏纹理的缓存 key 与上传状态一并清掉：
    /// `scene_screen_cache_key`/`scene_screen_tex_id` 不在渲染失败分支的清理范围内，
    /// 若留着旧 key，新文档（revision 仍为 0）会被 `render_scene_screen` 判定为命中，
    /// 画布会继续显示上一文档的游戏画面。
    pub(crate) fn invalidate_scene_view_state(&mut self) {
        self.scene_runtime = None;
        self.scene_runtime_revision = None;
        self.scene_view_cache_2d = crate::SceneViewCache2d::default();
        self.scene_view_cache_3d = crate::SceneViewCache3d::default();
        // WP1-4：typed 提取也按 revision 缓存，切文档时必须一并失效，否则新文档会沿用
        // 上一份文档的 3D 实体列表（两个文档 revision 可能都是 0）。
        self.scene_view_3d_typed = crate::SceneView3dTyped::default();
        self.scene_state.preview.clear();
        self.scene_state.preview3d_transform.clear();
        self.scene_state.gizmo_drag = None;
        self.preview_diagnostic = None;
        self.scene_screen_cache_key = None;
        self.scene_screen_tex_id = None;
        self.scene_view_repaint_pending = true;
        // 注意：这里不置 `scene_view_fit_pending`。本函数在**同文档编辑提交**
        // （Inspector 属性/删除/撤销等）后也会执行，若触发视图适配会把用户
        // 滚轮/拖拽设置的 zoom/pan 重置回 fit 值。重新适配只在文档整表切换时
        // 做，见 `set_current_scene_document`。
    }

    /// 字体资产/文本渲染策略变化后，丢弃 CPU 图集和 Scene View GPU 端的旧
    /// 字体状态。Play 预览天然新建渲染器；Scene View 必须显式重置，否则版本号
    /// 相同的新图集可能被旧纹理缓存误判为未变化。
    pub(crate) fn invalidate_font_render_resources(&mut self) {
        self.scene_font_atlas = None;
        self.scene_vector_atlas = None;
        self.custom_font_atlases.lock().unwrap().clear();
        self.custom_vector_atlases.lock().unwrap().clear();
        self.reset_scene_render_resources();
        self.invalidate_scene_view_state();
    }

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
    fn update_inspector_write(&mut self) -> bool {
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
                persistent: false,
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
    fn current_inspector_edit_snapshot(&self) -> Option<EditSnapshot> {
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
                persistent: false,
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
            persistent: false,
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
            persistent: entity
                .metadata
                .get("persistent")
                .is_some_and(|value| value == "true"),
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

    /// 保存当前场景（未命名时写入 scenes/<场景名>.scene）。
    ///
    /// E06：返回是否保存成功。失败（如场景无路径、磁盘写入失败、校验失败）时文档
    /// 保持 dirty 且不变更内容，供「保存并继续」确认流判断是否需要停留在当前文档。
    ///
    /// 磁盘文件被外部改动过时**拒绝覆盖**（见下面 `disk_status` 的说明）；
    /// 显式覆盖走 [`EditorApp::save_scene_force`]。
    pub(crate) fn save_scene(&mut self) -> bool {
        self.save_scene_with_policy(false)
    }

    /// 强制保存：明知磁盘被外部改过也覆盖（菜单里的显式动作）。
    pub(crate) fn save_scene_force(&mut self) -> bool {
        self.save_scene_with_policy(true)
    }

    fn save_scene_with_policy(&mut self, force: bool) -> bool {
        let started = std::time::Instant::now();
        // E07：保存前若正处于活动事务，先把最后一条 preview 提交为一条历史命令
        // （快照来自事务 before），保证落盘内容可被单条 Undo 收回。
        if self.edit_transaction.is_active() && self.history.is_transaction_active() {
            self.history.finish_transaction(
                self.document
                    .current_scene
                    .as_ref()
                    .expect("edit requires a scene"),
            );
            self.edit_transaction.active = false;
            self.edit_transaction.consumed = true;
        }
        if !self.project_loaded {
            self.status = "Import a game project before editing".to_owned();
            return false;
        }
        let Some(doc) = self.document.current_scene.as_mut() else {
            self.status = "No scene".into();
            return false;
        };
        // docs/55 阶段 3 的数据安全半边：**磁盘上的文件在外部被改过就不要静默覆盖**。
        //
        // `scenes/` 与 `scripts/` 本来就是外部工具会写的目录（生成器、git checkout、另一个编辑器），
        // 而这里过去是无条件覆盖——用户的 Ctrl+S 会悄悄吃掉别人的改动，正是计划 §8
        // 把"数据损坏"列为直接阻断项要防的那类事故。默认拒绝并给出可执行的下一步
        // （`save_scene_force` 提供显式覆盖出口，用户不会被卡死）。
        let disk_status = doc.disk_status();
        if disk_status.blocks_overwrite() && !force {
            self.status = format!(
                "已阻止保存：{}（{}）。如需覆盖请用「强制保存」，或先用「重新加载」放弃内存改动。",
                disk_status.describe(),
                doc.path
                    .as_deref()
                    .map(crate::util::display_path)
                    .unwrap_or_else(|| "<未命名>".to_owned())
            );
            self.set_scene_view_diagnostic(self.status.clone());
            self.profiler
                .record_command("save_scene_blocked", started.elapsed());
            return false;
        }
        let is_prefab_document = doc.is_prefab_document();
        let r = if doc.path.is_some() {
            self.recovery.save_document(doc).map_err(|e| e.to_string())
        } else {
            doc.save_as(
                self.project
                    .root
                    .join("scenes")
                    .join(format!("{}.scene", doc.scene.name)),
            )
            .map_err(|e| e.to_string())
        };
        let saved = r.is_ok();
        if saved {
            // 未命名场景第一次 save_as 后才获得稳定 key；这里补写当前折叠状态。
            self.save_hierarchy_collapsed();
        }
        self.status = match r {
            Ok(()) => {
                if let Some(doc) = self.document.current_scene.as_ref() {
                    let _ = self.recovery.clear_for(doc);
                }
                self.autosave.reset();
                self.refresh_recovery_scan();
                if is_prefab_document {
                    "Prefab 已保存".into()
                } else {
                    "Scene saved".into()
                }
            }
            Err(error) => error,
        };
        self.profiler
            .record_command("save_scene", started.elapsed());
        saved
    }

    /// 打开场景或 Prefab 资源（资源面板 / 层级面板 / Inspector Prefab 来源链接）。
    ///
    /// E06：统一经「未保存切换保护」网关。当前编辑文档 dirty 时挂起 pending 并弹确认
    /// 窗（确认前不切换）；clean 时直接执行 `perform_open_scene_resource`。
    pub(crate) fn open_scene_resource(&mut self, relative_path: PathBuf) {
        if !self.project_loaded {
            return;
        }
        self.request_document_change(PendingChange::OpenResource { relative_path });
    }

    /// 实际打开场景/Prefab（仅由 request 流程在确认后调用，或 clean 时直接调用）。
    fn perform_open_scene_resource(&mut self, relative_path: PathBuf) {
        let path = self.project.root.join(&relative_path);
        match self.open_scene_document(&path) {
            Ok(()) => {
                self.history.clear();
                self.autosave.reset();
                let opened_prefab = self
                    .current_scene()
                    .is_some_and(|document| document.is_prefab_document());
                self.scene_view_mode = if opened_prefab {
                    crate::SceneViewMode::Prefab
                } else {
                    crate::SceneViewMode::View2D
                };
                self.scene_view_fit_pending = true;
                // 打开文档本身已完成 Scene View 失效；整面板状态收敛到统一入口
                // （选择修复 / Inspector / Game 预览清理 / Hierarchy 元数据重扫）。
                self.on_document_changed(DocumentChange::Opened);
                // 打开场景文档后不再指向资源面板的选中项（Delete 快捷键回到 Hierarchy 外）。
                self.clear_resource_selection();
                self.status = if relative_path.extension().and_then(|value| value.to_str())
                    == Some("prefab")
                {
                    format!("已打开 Prefab：{}", relative_path.display())
                } else {
                    format!("已打开场景：{}", relative_path.display())
                };
            }
            Err(error) => self.status = format!("打开资源失败：{error}"),
        }
    }

    /// 同步 InspectorState ↔ EditorApp 快照字段（以显式 `id` 为准；选择状态在 `self.selection`）。
    ///
    /// E08：完成后把缓冲与文档对齐（面板主选为 `id` 的多选目标已与文档写回对齐，
    /// 无需重读），刷新基线并复位脏标记 —— 下一次缓冲编辑从「与文档一致」开始比较。
    pub(crate) fn sync_inspector(&mut self, id: SceneEntityId) {
        // E08：同步是「文档 → 缓冲」回读，任何未提交的挂起编辑都已被提交点收敛；
        // 若仍有未决缓冲（异常路径），先丢弃以免回读后被旧基线误判为脏而反复直写。
        self.clear_inspector_edit_pending();
        let Some(scene) = self.current_scene() else {
            self.inspector_baseline.clear();
            return;
        };
        // E08：目标实体必须存在于当前编辑文档。只有场景而目标不存在时回读会残留
        // 上一实体的快照字段并被误判为“挂起编辑”，故整组清空（与 `clear_inspector`
        // 行为一致）并清基线，返回空态。
        if !scene.scene.entities.contains_key(&id) {
            self.clear_inspector();
            return;
        }
        let scene = scene.clone();
        self.focus_hierarchy_delete_target();
        if let Ok(snapshot) = self.inspector.snapshot(&scene, id) {
            self.entity_name = snapshot.name;
            self.collider3d = snapshot.collider3d.unwrap_or_default();
            self.container = snapshot.container.unwrap_or_default();
            self.lua_script = snapshot.lua_script.unwrap_or_default();
            self.refresh_ui_editors(id);
            self.tilemap.map = scene
                .scene
                .entities
                .get(&id)
                .and_then(|entity| entity.components.get("Tilemap"))
                .and_then(|value| Tilemap::from_component_str(value).ok())
                .unwrap_or_else(|| {
                    Tilemap::new(engine::core::transform::Vec2::new(32.0, 32.0), 8, 8)
                        .expect("default tilemap")
                });
            self.animation.machine = scene
                .scene
                .entities
                .get(&id)
                .and_then(|entity| entity.components.get("Animation"))
                .and_then(|value| AnimationStateMachine::from_component_str(value).ok())
                .unwrap_or_else(|| {
                    AnimationStateMachine::new(
                        "idle",
                        vec![engine::core::animation::AnimationClip {
                            name: "idle".to_owned(),
                            sheet: self.selected_sprite_texture(),
                            cols: 1,
                            frame_rate: 1.0,
                            looped: true,
                            events: Vec::new(),
                        }],
                    )
                    .expect("default animation")
                });
        } else {
            // 目标不存在：清空回读字段，避免残留上一实体的快照。
            let fields = (
                &mut self.entity_name,
                &mut self.mesh_renderer,
                &mut self.lua_script,
            );
            fields.0.clear();
            fields.1.clear();
            fields.2.clear();
        }
        // 同步完成后刷新基线：当前缓冲（刚刚从 `id` 回读）即“已提交”状态。
        self.refresh_inspector_baseline();
    }

    /// E03：清空 Inspector 的 buffered 显示字段（无主选中 / 点击空白处时调用）。
    /// Inspector 面板下一帧以“请先在 Hierarchy 中选择实体”的空态呈现，
    /// 避免把上一个实体的快照残留显示给空选择。
    pub(crate) fn clear_inspector(&mut self) {
        self.focus_hierarchy_delete_target();
        self.entity_name.clear();
        self.mesh_renderer.clear();
        self.collider3d = Collider3dFields::default();
        self.tilemap.map = Tilemap::new(engine::core::transform::Vec2::new(32.0, 32.0), 8, 8)
            .expect("default tilemap");
        self.animation.machine = AnimationStateMachine::new(
            "idle",
            vec![engine::core::animation::AnimationClip {
                name: "idle".to_owned(),
                sheet: self.selected_sprite_texture(),
                cols: 1,
                frame_rate: 1.0,
                looped: true,
                events: Vec::new(),
            }],
        )
        .expect("default animation");
        self.lua_script.clear();
        // E08：清空显示缓冲后，基线/脏标记一并复位，防止旧快照残留参与比较。
        self.inspector_edit = EditState::default();
        self.inspector_baseline.clear();
    }

    /// 对齐选中实体：网格吸附 / 水平 / 垂直。
    pub(crate) fn align_selected(&mut self, mode: AlignMode) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let ids: Vec<_> = self.selection.ids();
        if ids.is_empty() {
            self.status = "请先选中实体".to_owned();
            return;
        }
        let mut targets: Vec<(SceneEntityId, TransformFields)> = Vec::new();
        for id in &ids {
            if let Some(entity) = scene.scene.entities.get(id)
                && let Some(transform) = entity
                    .components
                    .get("Transform")
                    .and_then(|value| parse_transform(value))
            {
                targets.push((*id, transform));
            }
        }
        if targets.is_empty() {
            return;
        }
        let pivot = targets[0].1;
        let step = self.snap_step;
        let new_targets = match mode {
            AlignMode::Grid => targets
                .iter()
                .map(|(id, transform)| {
                    let mut next = *transform;
                    next.position_x = (next.position_x / step).round() * step;
                    next.position_y = (next.position_y / step).round() * step;
                    (*id, next)
                })
                .collect::<Vec<_>>(),
            AlignMode::H => targets
                .iter()
                .map(|(id, transform)| {
                    let mut next = *transform;
                    next.position_y = pivot.position_y;
                    (*id, next)
                })
                .collect::<Vec<_>>(),
            AlignMode::V => targets
                .iter()
                .map(|(id, transform)| {
                    let mut next = *transform;
                    next.position_x = pivot.position_x;
                    (*id, next)
                })
                .collect::<Vec<_>>(),
        };
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let _ = self
            .history
            .execute(scene, "Align", CommandKind::Property, move |scene| {
                for (id, transform) in &new_targets {
                    if let Some(entity) = scene.scene.entities.get_mut(id) {
                        entity
                            .components
                            .insert("Transform".to_owned(), encode_transform(*transform));
                    }
                }
                scene.mark_dirty();
                Ok::<(), ()>(())
            });
        self.status = "已完成对齐".to_owned();
    }

    /// **资源重命名/移动 + 引用保持**（第 44 轮，docs/55 P1 的 Project Browser 一条）。
    ///
    /// 顺序是刻意的：**先校验、再搬文件、最后改引用**。
    /// - 目标已存在 / 越出工程根 / 正在 Play / 目标是当前打开的文档 → 直接拒绝（不做半截操作）；
    /// - 文件搬完再改引用：即使改引用失败，磁盘上也是"文件已改名 + 部分文档仍指向旧路径"
    ///   这种**可诊断**的状态（审计会把断链报出来），而不是"文档指向一个不存在的文件却没人知道"；
    /// - 其它文档的磁盘改写由 `editor::rewrite_project_references` 负责：**解析成功才写回**，
    ///   坏文件原样保留并进报告；
    /// - 当前打开的文档**不写磁盘**（保住用户未保存的编辑），而是在内存里改写并进 history
    ///   （一条 Undo 可回退引用改写；文件改名本身不回退，与 Unity 的取舍一致）。
    pub(crate) fn perform_resource_move(
        &mut self,
        from: &std::path::Path,
        to: &std::path::Path,
    ) -> Result<String, String> {
        if self.is_playing() {
            return Err("Play mode locks resource moves; click Stop first".to_owned());
        }
        let root = self.project.root.clone();
        let from_relative = editor::normalize_relative(from)
            .ok_or_else(|| format!("资源路径必须是工程内相对路径：{}", from.display()))?;
        let to_relative = editor::normalize_relative(to)
            .ok_or_else(|| format!("目标路径必须是工程内相对路径：{}", to.display()))?;
        if from_relative == to_relative {
            return Err("源与目标是同一个路径".to_owned());
        }
        let from_absolute = root.join(&from_relative);
        let to_absolute = root.join(&to_relative);
        if !from_absolute.is_file() {
            return Err(format!("源资源不存在：{}", from_relative.display()));
        }
        if to_absolute.exists() {
            return Err(format!("目标已存在：{}", to_relative.display()));
        }
        let open_document = self
            .document
            .current_scene
            .as_ref()
            .and_then(|document| document.path.clone());
        if open_document.as_ref().is_some_and(|path| {
            editor::paths_match(path, &from_absolute) || editor::paths_match(path, &to_absolute)
        }) {
            return Err(format!(
                "当前打开的文档不能重命名或移动：{}",
                from_relative.display()
            ));
        }
        if let Some(parent) = to_absolute.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建目标目录失败：{error}"))?;
        }
        std::fs::rename(&from_absolute, &to_absolute)
            .map_err(|error| format!("移动资源失败：{error}"))?;

        let from_text = from_relative.to_string_lossy().replace('\\', "/");
        let to_text = to_relative.to_string_lossy().replace('\\', "/");
        let report = editor::rewrite_project_references(
            &root,
            &from_text,
            &to_text,
            open_document.as_deref(),
        );

        // 当前打开的文档：内存改写 + 一条 Undo（不写盘）。
        let mut open_document_components = 0;
        if let Some(scene) = self.document.current_scene.as_mut() {
            let label = format!("资源引用：{from_text} → {to_text}");
            let counter = &mut open_document_components;
            let _ = self
                .history
                .execute(scene, label, CommandKind::Rename, |scene| {
                    let changed =
                        editor::rewrite_scene_references(&mut scene.scene, &from_text, &to_text);
                    if changed > 0 {
                        scene.mark_dirty();
                    }
                    *counter = changed;
                    Ok::<(), ()>(())
                });
        }

        self.thumbnails.remove(&from_text);
        self.refresh_resources();
        self.selected_resource = Some(to_relative.clone());
        let mut status = format!(
            "已重命名资源：{from_text} → {to_text}（改写 {} 个文档 / {} 处组件引用）",
            report.rewritten_files.len(),
            report.rewritten_components
        );
        if open_document_components > 0 {
            status.push_str(&format!(
                "；当前文档内 {open_document_components} 处（未保存，可 Undo）"
            ));
        }
        if !report.failures.is_empty() {
            status.push_str(&format!(
                "；{} 个文档未改写：{}",
                report.failures.len(),
                report.failures.join("、")
            ));
        }
        Ok(status)
    }

    /// 让选中的 2D 实体在水平或垂直方向上按首末锚点等间距分布。
    pub(crate) fn distribute_selected(&mut self, horizontal: bool) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let ids: Vec<_> = self.selection.ids();
        let targets = collect_selected_transforms(scene, &ids);
        if targets.len() < 3 {
            self.status = "等间距至少需要 3 个带 Transform 的实体".to_owned();
            return;
        }
        let axis = if horizontal {
            DistributionAxis::Horizontal
        } else {
            DistributionAxis::Vertical
        };
        let new_targets = distribute_targets(&targets, axis);
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let _ = self
            .history
            .execute(scene, "Distribute", CommandKind::Property, move |scene| {
                for (id, transform) in &new_targets {
                    if let Some(entity) = scene.scene.entities.get_mut(id) {
                        entity
                            .components
                            .insert("Transform".to_owned(), encode_transform(*transform));
                    }
                }
                scene.mark_dirty();
                Ok::<(), ()>(())
            });
        self.status = if horizontal {
            "已完成水平等间距".to_owned()
        } else {
            "已完成垂直等间距".to_owned()
        };
    }

    /// 批量调整选中 Sprite 的 sorting.order，便于快速前后排层。
    pub(crate) fn nudge_selected_sprite_order(&mut self, delta: i32) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let ids: Vec<_> = self.selection.ids();
        let mut updates = Vec::new();
        for id in ids {
            let Some(entity) = scene.scene.entities.get(&id) else {
                continue;
            };
            let Some(raw) = entity.components.get("Sprite") else {
                continue;
            };
            let Ok(mut sprite) = SpriteComponent::from_component_str(raw) else {
                continue;
            };
            sprite.sorting.order += delta;
            updates.push((id, sprite.to_component_str()));
        }
        if updates.is_empty() {
            self.status = "请先选中带 Sprite 的实体".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let _ = self
            .history
            .execute(scene, "Sprite Order", CommandKind::Property, move |scene| {
                for (id, sprite) in &updates {
                    if let Some(entity) = scene.scene.entities.get_mut(id) {
                        entity
                            .components
                            .insert("Sprite".to_owned(), sprite.clone());
                    }
                }
                scene.mark_dirty();
                Ok::<(), ()>(())
            });
        if let Some(selected) = self.selection.primary() {
            self.sync_inspector(selected);
        }
        self.status = format!("已批量调整 Sprite 排序：{delta:+}");
    }

    /// 按实体 Y 坐标重新生成选中 Sprite 的 sorting.order，适合 2D 俯视/伪等轴生产流程。
    pub(crate) fn sort_selected_sprites_by_y(&mut self, descending: bool) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let ids: Vec<_> = self.selection.ids();
        let mut targets = Vec::new();
        for id in ids {
            let Some(entity) = scene.scene.entities.get(&id) else {
                continue;
            };
            let Some(raw_sprite) = entity.components.get("Sprite") else {
                continue;
            };
            let Ok(sprite) = SpriteComponent::from_component_str(raw_sprite) else {
                continue;
            };
            let transform = entity
                .components
                .get("Transform")
                .and_then(|value| parse_transform(value))
                .unwrap_or_default();
            targets.push(SpriteOrderTarget {
                id,
                position_x: transform.position_x,
                position_y: transform.position_y,
                order: sprite.sorting.order,
                sprite,
            });
        }
        if targets.len() < 2 {
            self.status = "至少选择 2 个带 Sprite 的实体才能批量排序".to_owned();
            return;
        }
        let updates = sorted_sprite_order_updates(&targets, descending);
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let _ = self.history.execute(
            scene,
            "Sort Sprite Order",
            CommandKind::Property,
            move |scene| {
                for (id, sprite) in &updates {
                    if let Some(entity) = scene.scene.entities.get_mut(id) {
                        entity
                            .components
                            .insert("Sprite".to_owned(), sprite.clone());
                    }
                }
                scene.mark_dirty();
                Ok::<(), ()>(())
            },
        );
        if let Some(selected) = self.selection.primary() {
            self.sync_inspector(selected);
        }
        self.status = if descending {
            "已按 Y 从高到低重排 Sprite 顺序".to_owned()
        } else {
            "已按 Y 从低到高重排 Sprite 顺序".to_owned()
        };
    }

    /// 创建根实体（层级面板 / Inspector）。
    /// M19-03：写入 / 清除实体的自由 meta `persist_id`（一次提交 = 单条 Undo）。
    ///
    /// - `None` 或空白值 → 删除 `entity_meta persist_id` 行；否则写入去空白后的值。
    /// - 同场景内已存在同名 persist_id 时**拒绝**（编辑器侧防重复；跨场景重复与
    ///   `data.npcs.persist_id` 冲突由领域数据面板的 PersistId 注册表报告，因为那
    ///   需要跨文件视野，规则来源仍是 Lua 数据）。
    /// - 返回值：`Ok(true)` 产生了改动（已入 History），`Ok(false)` 无净变化，
    ///   `Err` 被拒绝（Play 中 / 无场景 / 重名）。
    pub(crate) fn set_entity_persist_id(
        &mut self,
        entity: SceneEntityId,
        persist_id: Option<String>,
    ) -> Result<bool, String> {
        if self.is_playing() {
            return Err("Play 模式锁定场景编辑，请先停止运行".to_owned());
        }
        let value = persist_id
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if let Some(value) = value.as_deref() {
            let duplicate = self.document.current_scene.as_ref().is_some_and(|scene| {
                scene.scene.entities.values().any(|candidate| {
                    candidate.id != entity
                        && candidate.metadata.get("persist_id").map(String::as_str) == Some(value)
                })
            });
            if duplicate {
                return Err(format!("本场景已存在 persist_id：「{value}」"));
            }
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Err("没有打开的场景".to_owned());
        };
        let label = match value.as_deref() {
            Some(value) => format!("PersistId {value}"),
            None => "Clear PersistId".to_owned(),
        };
        let changed = self
            .history
            .execute(scene, label, CommandKind::Property, move |scene| {
                let Some(entity) = scene.scene.entities.get_mut(&entity) else {
                    return Ok::<(), ()>(());
                };
                match value {
                    Some(value) => {
                        if entity.metadata.get("persist_id") != Some(&value) {
                            entity.metadata.insert("persist_id".to_owned(), value);
                            scene.mark_dirty();
                        }
                    }
                    None => {
                        if entity.metadata.remove("persist_id").is_some() {
                            scene.mark_dirty();
                        }
                    }
                }
                Ok::<(), ()>(())
            })
            .unwrap_or(false);
        Ok(changed)
    }

    /// M19-03：把 Inspector 的 persist_id 编辑缓冲回读为实体当前 meta 值。
    pub(crate) fn reload_persist_id_buffer(&mut self, entity: SceneEntityId) {
        self.persist_id_editing = Some(entity);
        self.persist_id_buffer = self
            .document
            .current_scene
            .as_ref()
            .and_then(|scene| scene.scene.entities.get(&entity))
            .and_then(|entity| entity.metadata.get("persist_id").cloned())
            .unwrap_or_default();
    }

    pub(crate) fn add_entity(&mut self, parent: Option<SceneEntityId>) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let name = self.entity_name.trim().to_owned();
        if name.is_empty() {
            self.status = "Entity name cannot be empty".into();
            return;
        }
        let tree = self.tree.clone();
        let created = std::cell::Cell::new(None::<SceneEntityId>);
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result = self
            .history
            .execute(scene, "Create entity", CommandKind::Create, |scene| {
                let id = tree
                    .create_entity(scene, name.clone(), parent)
                    .map_err(|e| e.to_string())?;
                // 2D 实体默认只带 Transform 组件。
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    entity.components.insert(
                        "Transform".into(),
                        encode_transform(TransformFields::default()),
                    );
                }
                created.set(Some(id));
                Ok::<(), String>(())
            });
        match result {
            Ok(true) => {
                if let Some(id) = created.get() {
                    self.selection.add(id, editor::SelectionSource::Command);
                    self.sync_inspector(id);
                }
                self.status = "Entity created".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    /// Unity 风格：新建 UI 实体时自动确保场景里有一个 Canvas 根（全屏 UiLayout），
    /// 新 UI 实体挂到 Canvas 下。已有 Canvas 就直接复用。
    pub(crate) fn create_ui_entity(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let tree = self.tree.clone();
        let created = std::cell::Cell::new(None::<SceneEntityId>);
        let result = self
            .history
            .execute(scene, "Create UI entity", CommandKind::Create, |scene| {
                // 1. 找或建 UI 根节点（metadata ui.root=true）。
                let root_id = scene.scene.entities.values()
                    .find(|e| e.metadata.get("ui.root").map(|v| v == "true").unwrap_or(false))
                    .map(|e| e.id);
                let root_id = if let Some(id) = root_id {
                    id
                } else {
                    let id = tree.create_entity(scene, "UIRoot", None).map_err(|e| e.to_string())?;
                    if let Some(root) = scene.scene.entities.get_mut(&id) {
                        root.metadata.insert("ui.root".into(), "true".into());
                        root.components.insert(
                            "UiLayout".into(),
                            "anchors=0.0000,0.0000,1.0000,1.0000;offsets=0.00,0.00,0.00,0.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000".into(),
                        );
                    }
                    id
                };
                // 2. 在 UI 根下建 UI 实体。
                let id = tree.create_entity(scene, "UI Element", Some(root_id))
                    .map_err(|e| e.to_string())?;
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    entity.components.insert(
                        "Transform".into(),
                        encode_transform(TransformFields::default()),
                    );
                }
                created.set(Some(id));
                Ok::<(), String>(())
            });
        match result {
            Ok(true) => {
                if let Some(id) = created.get() {
                    self.selection.add(id, editor::SelectionSource::Command);
                    self.sync_inspector(id);
                }
                self.status = "UI entity created under UI root".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    pub(crate) fn add_entity_with_component(&mut self, name: &str, component: &str) {
        self.entity_name = name.to_owned();
        self.add_entity(None);
        if let Some(id) = self.selection.primary() {
            let _ = self.add_component(component);
            self.sync_inspector(id);
        }
    }

    /// 5b：覆盖主选实体的 UiLayout offsets（一条历史，可撤销）。
    /// 拖放创建后用来把默认中心定位改成落点位置。
    pub(crate) fn set_ui_layout_offsets(&mut self, offsets: [f32; 4]) {
        let Some(id) = self.selection.primary() else {
            return;
        };
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let _ = self.history.execute(
            scene,
            "Position widget",
            CommandKind::Property,
            move |scene| {
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    crate::scene_drag::set_ui_component_layout(entity, offsets);
                }
                scene.mark_dirty();
                Ok::<(), InspectorError>(())
            },
        );
    }

    /// 5c：覆盖主选实体的 UiLayout anchors（一条历史，可撤销）。
    pub(crate) fn set_ui_layout_anchors(&mut self, anchors: [f32; 4]) {
        let Some(id) = self.selection.primary() else {
            return;
        };
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let _ = self.history.execute(
            scene,
            "Adjust anchors",
            CommandKind::Property,
            move |scene| {
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    if let Some(raw) = entity.components.get("UiLayout") {
                        if let Ok(mut layout) =
                            engine::core::ui_layout::UiRectLayout::from_component_str(raw)
                        {
                            layout.anchors = anchors;
                            entity.components.insert(
                                "UiLayout".to_owned(),
                                layout.to_component_str(),
                            );
                        }
                    }
                }
                scene.mark_dirty();
                Ok::<(), InspectorError>(())
            },
        );
    }

    /// 5b：在设计分辨率指定位置创建 widget（面板 → 设计像素已换算）。
    /// 默认 200×100 尺寸，中心锚点，按 widget 类型附加视觉/事件组件。
    pub(crate) fn create_widget_at(&mut self, widget: &str, design_pos: egui::Pos2) {
        let (name, component) = match widget {
            "Text" => ("New Text", "Text"),
            "Image" => ("New Image", "Image"),
            "Button" => ("New Button", "Image"),
            "Panel" => ("New Panel", "Image"),
            "Container" => ("New Container", "Container"),
            _ => {
                self.status = format!("未知 Widget 类型：{widget}");
                return;
            }
        };
        self.add_entity_with_component(name, component);
        // 默认 default_element() 是 200×100 中心锚点：offsets = [-100, -50, 100, 50]。
        // 按落点平移：offsets += (design_pos - design_center)。
        let design_center = egui::pos2(
            self.preview_resolution.0 as f32 * 0.5,
            self.preview_resolution.1 as f32 * 0.5,
        );
        let dx = design_pos.x - design_center.x;
        let dy = design_pos.y - design_center.y;
        let offsets = [-100.0 + dx, -50.0 + dy, 100.0 + dx, 50.0 + dy];
        self.set_ui_layout_offsets(offsets);
        if widget == "Button" {
            if let Err(error) = self.add_component("UiEvents") {
                self.status = error;
            }
        }
    }

    /// 5b：在设计分辨率中心创建 widget（无落点时的回退路径）。
    pub(crate) fn create_widget_at_center(&mut self, widget: &str) {
        let design_center = egui::pos2(
            self.preview_resolution.0 as f32 * 0.5,
            self.preview_resolution.1 as f32 * 0.5,
        );
        self.create_widget_at(widget, design_center);
    }

    /// 收集项目用到的字符并写入项目根 `chars.txt`（首包字体预热）。场景视图下次
    /// 重建图集时读到它并预生成字形，避免首帧文字卡顿。
    pub(crate) fn collect_font_chars(&mut self) {
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        let chars = engine::core::char_collect::collect_project_chars(&root);
        match std::fs::write(root.join("chars.txt"), chars.as_bytes()) {
            Ok(()) => {
                self.status = format!(
                    "已收集 {} 个字符写入 chars.txt（场景视图重建后预热）",
                    chars.chars().count()
                );
                // 让场景视图下次重建图集时按新清单预热。
                self.invalidate_font_render_resources();
            }
            Err(error) => self.status = format!("收集字符失败: {error}"),
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

    /// 设置实体的"运行时切场景不销毁"标记（Inspector 复选框；一次提交 = 一条历史）。
    ///
    /// 与迁移前的面板内联实现一致：**不做** Play 锁定（复选框本来就没有这道前置判断），
    /// 组件写入失败也不改状态栏。
    pub(crate) fn set_entity_persistent(&mut self, entity: SceneEntityId, persistent: bool) {
        let inspector = self.inspector.clone();
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ =
                self.history
                    .execute(scene, "Toggle Persistent", CommandKind::Property, |scene| {
                        inspector.set_persistent(scene, entity, persistent)?;
                        Ok::<(), InspectorError>(())
                    });
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DistributionAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug)]
struct SpriteOrderTarget {
    id: SceneEntityId,
    position_x: f32,
    position_y: f32,
    order: i32,
    sprite: SpriteComponent,
}

fn collect_selected_transforms(
    scene: &editor::SceneDocument,
    ids: &[SceneEntityId],
) -> Vec<(SceneEntityId, TransformFields)> {
    let mut targets = Vec::new();
    for id in ids {
        if let Some(entity) = scene.scene.entities.get(id)
            && let Some(transform) = entity
                .components
                .get("Transform")
                .and_then(|value| parse_transform(value))
        {
            targets.push((*id, transform));
        }
    }
    targets
}

fn distribute_targets(
    targets: &[(SceneEntityId, TransformFields)],
    axis: DistributionAxis,
) -> Vec<(SceneEntityId, TransformFields)> {
    let mut sorted = targets.to_vec();
    sorted.sort_by(|left, right| {
        let left_value = match axis {
            DistributionAxis::Horizontal => left.1.position_x,
            DistributionAxis::Vertical => left.1.position_y,
        };
        let right_value = match axis {
            DistributionAxis::Horizontal => right.1.position_x,
            DistributionAxis::Vertical => right.1.position_y,
        };
        left_value
            .total_cmp(&right_value)
            .then_with(|| left.0.0.cmp(&right.0.0))
    });
    let first = sorted
        .first()
        .map(|(_, transform)| match axis {
            DistributionAxis::Horizontal => transform.position_x,
            DistributionAxis::Vertical => transform.position_y,
        })
        .unwrap_or(0.0);
    let last = sorted
        .last()
        .map(|(_, transform)| match axis {
            DistributionAxis::Horizontal => transform.position_x,
            DistributionAxis::Vertical => transform.position_y,
        })
        .unwrap_or(first);
    let step = if sorted.len() > 1 {
        (last - first) / (sorted.len() - 1) as f32
    } else {
        0.0
    };
    sorted
        .into_iter()
        .enumerate()
        .map(|(index, (id, mut transform))| {
            let value = first + step * index as f32;
            match axis {
                DistributionAxis::Horizontal => transform.position_x = value,
                DistributionAxis::Vertical => transform.position_y = value,
            }
            (id, transform)
        })
        .collect()
}

fn sorted_sprite_order_updates(
    targets: &[SpriteOrderTarget],
    descending: bool,
) -> Vec<(SceneEntityId, String)> {
    let mut sorted = targets.to_vec();
    sorted.sort_by(|left, right| {
        let primary = if descending {
            right.position_y.total_cmp(&left.position_y)
        } else {
            left.position_y.total_cmp(&right.position_y)
        };
        primary
            .then_with(|| left.position_x.total_cmp(&right.position_x))
            .then_with(|| left.id.0.cmp(&right.id.0))
    });
    let base = sorted.iter().map(|target| target.order).min().unwrap_or(0);
    sorted
        .into_iter()
        .enumerate()
        .map(|(index, mut target)| {
            target.sprite.sorting.order = base + index as i32;
            (target.id, target.sprite.to_component_str())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Prefab typed action（shell 层）：面板只发命令，写盘与重建实例的提交边界在这里。
// ---------------------------------------------------------------------------

impl EditorApp {
    /// 把**单个字段**还原为源 Prefab 的取值（docs/55 阶段 3：选择性 Revert）。
    ///
    /// 走统一的 history 网关：一条 Undo 即可收回；Play 中拒绝；不触碰其它覆盖字段。
    pub(crate) fn revert_prefab_override_field(&mut self, selected: SceneEntityId, field: &str) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
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
            self.status = "当前实体不是 Prefab 实例".to_owned();
            return;
        };
        let path = strip_verbatim_prefix(&self.project.root).join(&prefab_source);
        if let Err(error) = self.ensure_prefab_source_cached(&path) {
            self.status = format!("加载源 Prefab 失败：{error}");
            return;
        }
        let Some(source_entity) = self
            .prefab_source_cache
            .get(&path)
            .and_then(|cached| cached.prefab.entities.get(&source_id))
            .cloned()
        else {
            self.status = format!("源 Prefab 中缺少实体 {}", source_id.0);
            return;
        };
        let field = field.to_owned();
        let field_label = field.clone();
        let Some(doc) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self.history.execute(
            doc,
            format!("Revert Override {field_label}"),
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
                        .ok_or_else(|| format!("源 Prefab 缺少组件 {key}"))?;
                    entity.components.insert(key.to_owned(), payload);
                } else if let Some(raw_path) = field.strip_prefix("property:") {
                    let path = engine::core::property_path::PropertyPath::parse(raw_path)
                        .map_err(|error| error.to_string())?;
                    let source_payload = source_entity
                        .components
                        .get(&path.component)
                        .ok_or_else(|| format!("源 Prefab 缺少组件 {}", path.component))?;
                    let value = engine::core::property_path::read_field_text(source_payload, &path)
                        .map_err(|error| error.to_string())?;
                    let current = entity
                        .components
                        .get(&path.component)
                        .cloned()
                        .ok_or_else(|| format!("实例缺少组件 {}", path.component))?;
                    let updated =
                        engine::core::property_path::write_builtin(&current, &path, &value)
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
                    return Err(format!("不支持的 Prefab override 字段 {field}"));
                }
                scene.mark_dirty();
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(true) => {
                self.on_document_changed(DocumentChange::PrefabApply);
                self.status = format!("已回退覆盖字段：{field_label}（父节点 {parent:?}）");
            }
            Ok(false) => self.status = format!("字段已是源取值：{field_label}"),
            Err(error) => self.status = format!("回退覆盖失败：{error}"),
        }
    }

    /// 重新应用源 Prefab（docs/55 §4.1：**目标由 typed action 显式给出**，不再偷偷读选区）。
    ///
    /// 之前这里读 `selection.primary()`，于是 Hierarchy 的右键菜单必须先"设为单选"才能调用——
    /// 一个隐藏的选区依赖。现在目标作为参数传进来，视图侧只负责发命令。
    pub(crate) fn reapply_prefab(&mut self, selected: SceneEntityId) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&selected) else {
            self.status = "Prefab 根实例不存在".to_owned();
            return;
        };
        if entity
            .metadata
            .get("prefab.root")
            .is_none_or(|value| value != "true")
        {
            self.status = "只有 Prefab 根实例支持重新应用源".to_owned();
            return;
        }
        let Some(prefab_source) = entity.metadata.get("prefab.source").cloned() else {
            self.status = "当前实体没有 Prefab 来源".to_owned();
            return;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        let prefab = match Prefab::load(root.join(&prefab_source)) {
            Ok(prefab) => prefab,
            Err(error) => {
                self.status = format!("重新加载 Prefab 失败：{error}");
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
            "Reapply Prefab Source",
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
                self.status = format!("已重新应用 Prefab：{prefab_source}");
            }
            Err(error) => self.status = format!("重新应用 Prefab 失败：{error}"),
        }
    }

    /// 把实例写回源 Prefab 文件（目标由 typed action 显式给出）。
    pub(crate) fn save_prefab_to_source(&mut self, selected: SceneEntityId) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&selected) else {
            self.status = "Prefab 根实例不存在".to_owned();
            return;
        };
        if entity
            .metadata
            .get("prefab.root")
            .is_none_or(|value| value != "true")
        {
            self.status = "只有 Prefab 根实例支持写回源 Prefab".to_owned();
            return;
        }
        let Some(prefab_source) = entity.metadata.get("prefab.source").cloned() else {
            self.status = "当前实体没有 Prefab 来源".to_owned();
            return;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        let prefab_path = root.join(&prefab_source);
        let existing_prefab = match Prefab::load(&prefab_path) {
            Ok(prefab) => prefab,
            Err(error) => {
                self.status = format!("加载源 Prefab 失败：{error}");
                return;
            }
        };
        let new_prefab =
            match build_prefab_from_instance_root(&scene.scene, selected, &existing_prefab) {
                Ok(prefab) => prefab,
                Err(error) => {
                    self.status = format!("生成源 Prefab 失败：{error}");
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
                label: "Save Prefab Source",
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
                self.status = format!("已写回 Prefab 源并更新实例：{prefab_source}");
            }
            Err(error) => self.status = format!("更新 Prefab 实例失败：{error}"),
        }
    }

    /// 把一个字段的**实例取值**应用回源 Prefab 文件，并同步更新场景里的全部实例
    /// （docs/55 阶段 3：逐字段 Apply）。应用后该字段不再是覆盖。
    pub(crate) fn apply_prefab_override_field(&mut self, selected: SceneEntityId, field: &str) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some((prefab_source, source_id)) = self.prefab_instance_source(selected) else {
            self.status = "当前实体不是 Prefab 实例".to_owned();
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
                self.status = format!("加载源 Prefab 失败：{error}");
                return;
            }
        };
        // 只改源 Prefab 里的目标字段；失败则**不落盘、不改场景**。
        let Some(source_entity) = prefab.entities.get_mut(&source_id) else {
            self.status = format!("源 Prefab 中缺少实体 {}", source_id.0);
            return;
        };
        if field == "name" {
            source_entity.name = instance_value;
        } else if let Some(key) = field.strip_prefix("component:") {
            source_entity
                .components
                .insert(key.to_owned(), instance_value);
        } else if let Some(raw_path) = field.strip_prefix("property:") {
            let Ok(path) = engine::core::property_path::PropertyPath::parse(raw_path) else {
                self.status = format!("非法属性路径：{raw_path}");
                return;
            };
            let Some(payload) = source_entity.components.get(&path.component).cloned() else {
                self.status = format!("源 Prefab 缺少组件 {}", path.component);
                return;
            };
            match engine::core::property_path::write_builtin(&payload, &path, &instance_value) {
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
            self.status = format!("不支持的 Prefab override 字段 {field}");
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
                label: &format!("Apply Override {field_label}"),
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
                self.status = format!("已把字段应用到源 Prefab 并更新全部实例：{field_label}");
            }
            Err(error) => self.status = format!("应用覆盖失败：{error}"),
        }
    }

    /// 解包 Prefab 实例（目标由 typed action 显式给出）。
    pub(crate) fn unpack_prefab(&mut self, selected: SceneEntityId) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&selected) else {
            self.status = "Prefab 根实例不存在".to_owned();
            return;
        };
        if entity
            .metadata
            .get("prefab.root")
            .is_none_or(|value| value != "true")
        {
            self.status = "只有 Prefab 根实例支持 Unpack".to_owned();
            return;
        }
        let Some(doc) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self
            .history
            .execute(doc, "Unpack Prefab", CommandKind::Property, |scene| {
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
                self.status = "Prefab 实例已解包为普通实体".to_owned();
            }
            Err(error) => self.status = format!("Unpack 失败：{error}"),
        }
    }

    /// 把（可能已改动的）Prefab 落盘，并在**一条历史**里重建场景内全部同源实例。
    ///
    /// 整体写回与逐字段应用共用这条路径：覆盖集合由「实例 vs 新源」重新 diff 得出，
    /// 因此刚被应用的字段会自动从覆盖列表中消失。
    ///
    /// 作为关联函数接收 `history` 与 `scene` 两个**互不相干**的可变借用，
    /// 避免「持有 current_scene 借用时再调用 &mut self 方法」的借用冲突。
    fn write_prefab_and_rebuild_instances(
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
            .map_err(|error| format!("写回源 Prefab 失败：{error}"))?;

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
    scene: &engine::core::scene::Scene,
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
    scene: &engine::core::scene::Scene,
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

fn prefab_root_ids(scene: &engine::core::scene::Scene, prefab_source: &str) -> Vec<SceneEntityId> {
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

fn collect_prefab_overrides(
    scene: &engine::core::scene::Scene,
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

fn prefab_override_value(instance: &SceneEntity, field: &str) -> Result<String, String> {
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
        let path = engine::core::property_path::PropertyPath::parse(path)
            .map_err(|error| format!("{field}: {error}"))?;
        let payload = instance
            .components
            .get(&path.component)
            .ok_or_else(|| format!("实例缺少组件 {}", path.component))?;
        engine::core::property_path::read_field_text(payload, &path)
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

fn collect_scene_subtree(
    scene: &engine::core::scene::Scene,
    root: SceneEntityId,
) -> Result<Vec<SceneEntityId>, String> {
    fn visit(
        scene: &engine::core::scene::Scene,
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
    scene: &mut engine::core::scene::Scene,
    prefab_source: &str,
    instance: &engine::core::prefab::PrefabInstance,
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

fn preserve_prefab_editor_flags(
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
struct PrefabWriteRequest<'a> {
    source: &'a str,
    path: &'a std::path::Path,
    prefab: Prefab,
    /// 需要在重建后重新选中的实例根（通常是用户当前选中的那个）。
    selected: SceneEntityId,
    /// Undo/Redo 菜单里显示的命令名。
    label: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditorApp, EditorCamera, SceneViewCache2d, SceneViewCache3d};
    use editor::open_project;
    use engine::core::{logging::LogSink, transform::Transform3D, transform::Vec2};
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    fn unique_token() -> u128 {
        // 进程内唯一：避免并行测试临时目录互撞（详见 editor::test_token）。
        crate::util::test_token()
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("m15-action-{label}-{}", unique_token()))
    }

    fn sample_project_root() -> PathBuf {
        engine::core::games_dir().join("examples").join("game_api")
    }

    fn copy_dir(source: &Path, destination: &Path) {
        if !source.is_dir() {
            return;
        }
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let source_path = entry.path();
            let target_path = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                fs::create_dir_all(&target_path).unwrap();
                copy_dir(&source_path, &target_path);
            } else {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::copy(&source_path, &target_path).unwrap();
            }
        }
    }

    fn copy_sample_project(label: &str) -> PathBuf {
        let source = sample_project_root();
        assert!(
            source.join("project.toml").is_file(),
            "sample project missing: {}",
            source.display()
        );
        let root = temp_root(label);
        fs::create_dir_all(&root).unwrap();
        fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
        copy_dir(&source.join("scripts"), &root.join("scripts"));
        copy_dir(&source.join("scenes"), &root.join("scenes"));
        copy_dir(&source.join("assets"), &root.join("assets"));
        root
    }

    /// docs/55 阶段 3 的数据安全半边：**磁盘文件被外部改动时保存必须被拒绝**。
    ///
    /// 这条守的是一类静默数据损坏：`scenes/` 是外部工具（生成器、git checkout、另一个编辑器）
    /// 会写的目录，而过去 `Ctrl+S` 无条件覆盖——别人的改动被悄悄吃掉，且**没有任何提示**。
    #[test]
    fn save_refuses_to_overwrite_an_externally_modified_scene() {
        let root = copy_sample_project("external-change");
        let project = open_project(&root).expect("打开工程");
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.project_loaded = true;
        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
        let path = app
            .document
            .current_scene
            .as_ref()
            .and_then(|document| document.path.clone())
            .expect("场景应有磁盘路径");

        // ① 干净状态下保存：正常通过（不制造假冲突）。
        assert!(app.save_scene(), "无外部改动时保存必须成功：{}", app.status);

        // ② 外部改写磁盘文件（模拟生成器/git checkout）。
        let external = "scene\texternally\twritten\n";
        fs::write(&path, external).expect("外部写入");
        assert_eq!(
            app.document
                .current_scene
                .as_ref()
                .expect("场景")
                .disk_status(),
            editor::DiskStatus::Modified,
            "外部写入之后必须被识别为已修改"
        );

        // ③ 普通保存被拒绝，且**磁盘内容原样保留**（这才是关键：拒绝不是提示，而是不写）。
        app.document
            .current_scene
            .as_mut()
            .expect("场景")
            .mark_dirty();
        assert!(!app.save_scene(), "外部改动后普通保存必须被拒绝");
        assert_eq!(
            fs::read_to_string(&path).expect("磁盘内容"),
            external,
            "被拒绝的保存不得改动磁盘文件"
        );
        assert!(
            app.status.contains("已阻止保存"),
            "状态栏必须说明原因：{}",
            app.status
        );
        assert!(
            app.document
                .current_scene
                .as_ref()
                .expect("场景")
                .is_dirty(),
            "被拒绝之后文档必须保持 dirty"
        );

        // ④ 显式强制保存：覆盖成功，磁盘内容变成编辑器里的场景。
        assert!(app.save_scene_force(), "强制保存必须成功：{}", app.status);
        let written = fs::read_to_string(&path).expect("磁盘内容");
        assert_ne!(written, external, "强制保存必须真的覆盖");
        assert!(
            written.contains(
                &app.document
                    .current_scene
                    .as_ref()
                    .expect("场景")
                    .scene
                    .name
            ),
            "覆盖后的文件应当是被保存的场景"
        );
        // ⑤ 覆盖之后签名刷新：再保存一次不会再报冲突（否则用户会陷在"永远冲突"里）。
        assert_eq!(
            app.document
                .current_scene
                .as_ref()
                .expect("场景")
                .disk_status(),
            editor::DiskStatus::Unchanged
        );
        app.document
            .current_scene
            .as_mut()
            .expect("场景")
            .mark_dirty();
        assert!(app.save_scene(), "覆盖之后应恢复正常保存：{}", app.status);
        let _ = fs::remove_dir_all(root);
    }

    /// 未命名文档没有磁盘基准，`disk_status()` 必须是 `Untracked` 而不是"冲突"。
    #[test]
    fn unnamed_document_never_reports_a_disk_conflict() {
        let root = copy_sample_project("unnamed-disk");
        let project = open_project(&root).expect("打开工程");
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.project_loaded = true;
        app.create_scene();
        let document = app.document.current_scene.as_ref().expect("新场景");
        assert!(document.path.is_none());
        assert_eq!(document.disk_status(), editor::DiskStatus::Untracked);
        assert!(app.save_scene(), "新建场景必须能保存：{}", app.status);
        assert!(
            app.document
                .current_scene
                .as_ref()
                .expect("场景")
                .path
                .is_some(),
            "保存之后应获得路径"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn distribute_targets_preserves_endpoints_and_even_spacing() {
        let targets = vec![
            (
                SceneEntityId(1),
                TransformFields {
                    position_x: 40.0,
                    position_y: 5.0,
                    ..TransformFields::default()
                },
            ),
            (
                SceneEntityId(2),
                TransformFields {
                    position_x: 0.0,
                    position_y: 2.0,
                    ..TransformFields::default()
                },
            ),
            (
                SceneEntityId(3),
                TransformFields {
                    position_x: 10.0,
                    position_y: 9.0,
                    ..TransformFields::default()
                },
            ),
        ];
        let distributed = distribute_targets(&targets, DistributionAxis::Horizontal);
        let xs = distributed
            .iter()
            .map(|(_, transform)| transform.position_x)
            .collect::<Vec<_>>();
        assert_eq!(xs, vec![0.0, 20.0, 40.0]);
        let ys = distributed
            .iter()
            .map(|(_, transform)| transform.position_y)
            .collect::<Vec<_>>();
        assert_eq!(ys, vec![2.0, 9.0, 5.0]);
    }

    #[test]
    fn sorted_sprite_order_updates_keep_min_order_and_follow_y() {
        let sprite = |order| SpriteComponent {
            texture: "assets/test.png".to_owned(),
            size: Vec2::new(32.0, 32.0),
            pivot: Vec2::new(0.5, 0.5),
            tint: [1.0; 4],
            sorting: engine::core::sprite::SpriteSorting {
                layer: "world".to_owned(),
                order,
            },

            flip_x: false,
            flip_y: false,
            z: 0.0,
            nine_slice: None,
        };
        let targets = vec![
            SpriteOrderTarget {
                id: SceneEntityId(1),
                position_x: 0.0,
                position_y: 20.0,
                order: 8,
                sprite: sprite(8),
            },
            SpriteOrderTarget {
                id: SceneEntityId(2),
                position_x: 0.0,
                position_y: 10.0,
                order: 5,
                sprite: sprite(5),
            },
            SpriteOrderTarget {
                id: SceneEntityId(3),
                position_x: 0.0,
                position_y: 30.0,
                order: 12,
                sprite: sprite(12),
            },
        ];
        let updates = sorted_sprite_order_updates(&targets, false);
        let orders = updates
            .iter()
            .map(|(_, value)| {
                SpriteComponent::from_component_str(value)
                    .unwrap()
                    .sorting
                    .order
            })
            .collect::<Vec<_>>();
        assert_eq!(orders, vec![5, 6, 7]);
    }

    #[test]
    fn open_scene_resource_invalidates_scene_view_state() {
        let root = copy_sample_project("scene-switch");
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.scene_runtime_revision = Some(0);
        app.scene_view_cache_2d = SceneViewCache2d {
            revision: Some(0),
            size: (320, 180),
            camera: (eframe::egui::vec2(12.0, -8.0), 1.5),
        };
        app.scene_view_cache_3d = SceneViewCache3d {
            revision: Some(0),
            size: (640, 360),
            camera: EditorCamera::default(),
        };
        app.scene_state
            .preview
            .insert(SceneEntityId(1), TransformFields::default());
        app.scene_state
            .preview3d_transform
            .insert(SceneEntityId(1), Transform3D::default());
        app.preview_diagnostic = Some("stale frame".to_owned());

        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));

        assert_eq!(app.scene_runtime_revision, None);
        assert_eq!(app.scene_view_cache_2d.revision, None);
        assert_eq!(app.scene_view_cache_2d.size, (0, 0));
        assert_eq!(app.scene_view_cache_3d.revision, None);
        assert_eq!(app.scene_view_cache_3d.size, (0, 0));
        assert!(app.scene_state.preview.is_empty());
        assert!(app.scene_state.preview3d_transform.is_empty());
        assert_eq!(app.preview_diagnostic, None);
        assert!(app.scene_view_repaint_pending);

        let _ = fs::remove_dir_all(root);
    }

    /// WP1-4：3D 视图的 typed 提取按文档 revision 缓存，并且切文档后必须失效——
    /// 否则新文档会沿用上一份文档的 3D 实体列表（两个文档 revision 都可能为 0）。
    #[test]
    fn scene_view_3d_typed_cache_follows_document_and_resets_on_switch() {
        let root = copy_sample_project("wp1-04-typed");
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/demo3d.scene"));

        app.ensure_scene_view_3d_typed();
        assert!(
            !app.scene_view_3d_typed.entities.is_empty(),
            "demo3d 应提取出 3D 实体"
        );
        assert!(app.scene_view_3d_typed.diagnostics.is_empty());
        assert_eq!(
            app.scene_view_3d_typed.revision,
            app.document.current_scene.as_ref().map(|doc| doc.revision)
        );

        // 同一 revision 下不重算：清空缓存内容当探针，再调用必须保持清空。
        app.scene_view_3d_typed.entities.clear();
        app.ensure_scene_view_3d_typed();
        assert!(
            app.scene_view_3d_typed.entities.is_empty(),
            "revision 未变时必须命中缓存，不得重新提取"
        );

        // 切换文档 → 缓存整体失效。文档此时是干净的，否则会被 E06 未保存保护拦下、
        // 实际不切文档（那样断言的就是另一回事）。
        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
        assert_eq!(app.scene_view_3d_typed.revision, None);
        assert!(app.scene_view_3d_typed.entities.is_empty());

        // 重新打开 demo3d 并编辑：revision 自增后必须重新提取（同样清空当探针）。
        app.open_scene_resource(PathBuf::from("scenes/demo3d.scene"));
        app.ensure_scene_view_3d_typed();
        assert!(!app.scene_view_3d_typed.entities.is_empty());
        let before = app.scene_view_3d_typed.revision;
        app.scene_view_3d_typed.entities.clear();
        app.document.current_scene.as_mut().unwrap().mark_dirty();
        assert_ne!(
            app.document.current_scene.as_ref().map(|doc| doc.revision),
            before
        );
        app.ensure_scene_view_3d_typed();
        assert!(
            !app.scene_view_3d_typed.entities.is_empty(),
            "revision 变化后必须重新提取"
        );

        let _ = fs::remove_dir_all(root);
    }

    // E02/E07 回归测试：使用仓库内 examples 工程（project.toml +
    // scripts/scenes；不复制巨大的 assets，也绝不调用 refresh_resources）。
    fn copy_examples_project(label: &str) -> PathBuf {
        let source = sample_project_root();
        assert!(
            source.join("project.toml").is_file(),
            "examples 工程缺失: {}",
            source.display()
        );
        assert!(
            source.join("scenes/main.scene").is_file(),
            "examples 缺少 scenes/main.scene"
        );
        let root = temp_root(label);
        fs::create_dir_all(&root).unwrap();
        fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
        copy_dir(&source.join("scripts"), &root.join("scripts"));
        copy_dir(&source.join("scenes"), &root.join("scenes"));
        root
    }

    /// 打开示例工程副本并打开 **Prefab 实例场景** `scenes/api_prefab.scene`。
    ///
    /// 与 inspector tests 的 `sample_app_with_production_scene` 指向同一个场景：
    /// 它带两个 Prefab 源（slime_green / slime_red）的真实实例，Apply / Revert / 逐字段
    /// Override 这些路径都需要"实例实体上真的有 `prefab.source` 元数据"才测得动。
    /// （拷贝 assets/scenes 后不依赖资源索引，Prefab 按相对路径加载。）
    fn open_sample_production(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_sample_project(label);
        let project = open_project(&root).expect("production 副本应可打开");
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.open_scene_resource(PathBuf::from("scenes/api_prefab.scene"));
        (app, root)
    }

    /// 打开示例工程副本并打开 2D 综合场景 `scenes/main.scene`。
    ///
    /// 场景名与实体结构见 `examples/game_api/README.md`；断言**不写死实体数量**，
    /// 而是在运行期从文档里取（示例工程的场景会随演示增删而变，写死数量只会制造假失败）。
    fn open_examples_main(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_examples_project(label);
        let project = open_project(&root).expect("examples 副本应可打开");
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.open_scene_resource(PathBuf::from("scenes/main.scene"));
        (app, root)
    }

    /// 文档里第一个**没有父实体**的实体 id（删除/撤销这类测试需要一个真有意义的根实体）。
    fn first_root_entity(app: &EditorApp) -> SceneEntityId {
        app.document
            .current_scene
            .as_ref()
            .expect("已打开场景")
            .scene
            .entities
            .values()
            .find(|entity| entity.parent.is_none())
            .expect("示例场景应当有根实体")
            .id
    }

    /// 制造一份“上次文档”遗留的陈旧状态：运行时 revision、2D/3D 缓存、
    /// 预览变换、画面纹理 key、Hierarchy 元数据 key。
    fn poison_scene_view_caches(app: &mut EditorApp) {
        app.scene_runtime_revision = Some(0);
        app.scene_view_cache_2d = SceneViewCache2d {
            revision: Some(0),
            size: (320, 180),
            camera: (eframe::egui::vec2(12.0, -8.0), 1.5),
        };
        app.scene_view_cache_3d = SceneViewCache3d {
            revision: Some(0),
            size: (640, 360),
            camera: EditorCamera::default(),
        };
        app.scene_state
            .preview
            .insert(SceneEntityId(1), TransformFields::default());
        app.scene_state
            .preview3d_transform
            .insert(SceneEntityId(1), Transform3D::default());
        app.preview_diagnostic = Some("stale frame".to_owned());
        app.scene_screen_cache_key = Some((0, (1280, 720)));
        app.scene_screen_tex_id = Some(eframe::egui::TextureId::User(0));
        app.last_metadata_key = Some((Some(PathBuf::from("stale/path")), 5));
    }

    fn assert_scene_view_and_metadata_invalidated(app: &EditorApp) {
        assert_eq!(app.scene_runtime_revision, None);
        assert_eq!(app.scene_view_cache_2d.revision, None);
        assert_eq!(app.scene_view_cache_2d.size, (0, 0));
        assert_eq!(app.scene_view_cache_3d.revision, None);
        assert_eq!(app.scene_view_cache_3d.size, (0, 0));
        assert!(app.scene_state.preview.is_empty());
        assert!(app.scene_state.preview3d_transform.is_empty());
        assert_eq!(app.preview_diagnostic, None);
        assert_eq!(app.scene_screen_cache_key, None);
        assert_eq!(app.scene_screen_tex_id, None);
        assert!(app.scene_view_repaint_pending);
        assert_eq!(app.last_metadata_key, None);
    }

    /// 回归：同文档编辑提交（属性直写的事务提交）不得重置 Scene 视图的 zoom/pan，
    /// 也不得置 fit_pending 把滚轮设置的视图状态覆盖回 fit 值；文档整表切换
    /// （重新打开场景）则必须重新适配。
    #[test]
    fn edit_commit_preserves_scene_view_camera_but_reopen_refits() {
        let (mut app, root) = open_examples_main("zoom-preserve");
        // 清掉打开场景时的初始适配请求，聚焦「编辑提交」是否再次触发。
        app.scene_view_fit_pending = false;
        // 用户滚轮/拖拽后的视图状态。
        app.scene_zoom = 1.6;
        app.scene_state.pan = egui::vec2(42.0, -17.0);

        // 属性编辑提交路径：开事务 → 直写 → 提交（等价 Inspector 数值编辑松手）。
        app.selection
            .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);
        app.begin_edit_transaction(
            "Inspector",
            editor::CommandKind::Property,
            Some(SceneEntityId(2)),
        );
        if let Some(entity) = app
            .document
            .current_scene
            .as_mut()
            .and_then(|doc| doc.scene.entities.get_mut(&SceneEntityId(2)))
        {
            entity.name = "ZoomPreserved".to_owned();
        }
        assert!(app.commit_edit_transaction(), "应有净差异提交");
        assert!(
            !app.scene_view_fit_pending,
            "同文档编辑提交不得触发视图适配"
        );
        assert_eq!(app.scene_zoom, 1.6, "编辑提交不得重置 zoom");
        assert_eq!(app.scene_state.pan, egui::vec2(42.0, -17.0));

        // 文档整表切换（重新打开场景）：必须重新适配视图。先撤销恢复 clean，
        // 否则 open 会走 dirty 挂起确认而不执行。
        app.undo();
        app.scene_zoom = 1.6;
        app.open_scene_resource(std::path::PathBuf::from("scenes/main.scene"));
        assert!(app.scene_view_fit_pending, "整表切换应触发视图适配");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn examples_main_open_scene_routes_unified_refresh() {
        let (mut app, root) = open_examples_main("examples-open");
        let entity_count = {
            let scene = app.document.current_scene.clone().expect("已打开场景");
            let count = scene.scene.entities.len();
            assert!(
                count >= 8,
                "示例 main.scene 应当有足够多的实体，实际 {count}"
            );
            assert!(
                scene
                    .scene
                    .entities
                    .values()
                    .any(|entity| entity.parent.is_some()),
                "示例 main.scene 应当有父子关系（Hierarchy 面板与传播路径的样本）"
            );
            count
        };
        assert_eq!(app.status, "已打开场景：scenes/main.scene");
        assert!(app.selection.is_empty());

        // 在已打开的文档上制造悬空选择、陈旧缓存与旧的 Hierarchy 元数据 key。
        app.selection
            .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);
        app.delete_target = crate::DeleteTarget::Hierarchy;
        poison_scene_view_caches(&mut app);

        // 再次打开同一场景：Opened 属整表切换，必须无条件清空选择并全量刷新。
        app.open_scene_resource(PathBuf::from("scenes/main.scene"));
        assert_eq!(
            app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .len(),
            entity_count,
            "重开同一场景应当得到同一份实体集合"
        );
        assert!(app.selection.is_empty());
        assert_eq!(app.selection.primary(), None);
        assert!(matches!(app.delete_target, crate::DeleteTarget::None));
        assert_scene_view_and_metadata_invalidated(&app);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn examples_main_delete_selected_root_routes_unified_refresh() {
        let (mut app, root) = open_examples_main("examples-delete");
        let target = first_root_entity(&app);
        let before = app
            .document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .len();
        app.selection
            .set_single(target, editor::SelectionSource::Hierarchy);
        app.delete_target = crate::DeleteTarget::Hierarchy;
        poison_scene_view_caches(&mut app);

        // 删除根实体：删除成功后统一入口必须清理悬空选择与陈旧缓存。
        app.delete_selected_entity();
        assert_eq!(app.status, "Entity deleted");
        let scene = app.document.current_scene.as_ref().unwrap();
        assert_eq!(
            scene.scene.entities.len(),
            before - 1,
            "删除根实体（连同子树）"
        );
        assert!(!scene.scene.entities.contains_key(&target));
        assert!(app.selection.is_empty());
        assert_eq!(app.selection.primary(), None);
        assert!(matches!(app.delete_target, crate::DeleteTarget::None));
        assert_scene_view_and_metadata_invalidated(&app);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn examples_main_undo_redo_routes_unified_refresh() {
        let (mut app, root) = open_examples_main("examples-undo");
        let target = first_root_entity(&app);
        let before = app
            .document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .len();
        app.selection
            .set_single(target, editor::SelectionSource::Hierarchy);
        poison_scene_view_caches(&mut app);

        app.delete_selected_entity();
        assert_eq!(app.status, "Entity deleted");
        assert_eq!(
            app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .len(),
            before - 1
        );
        assert_scene_view_and_metadata_invalidated(&app);

        // Undo 走统一入口：整文档回到删除前快照，Selection/缓存一并收敛。
        app.undo();
        assert_eq!(app.status, "Undo: Delete entities");
        let scene = app.document.current_scene.as_ref().unwrap();
        assert_eq!(scene.scene.entities.len(), before);
        assert!(scene.scene.entities.contains_key(&target));
        assert!(app.selection.is_empty());
        assert_eq!(app.selection.primary(), None);
        assert_scene_view_and_metadata_invalidated(&app);

        // Redo 同样经统一入口：删除再次生效。
        app.redo();
        assert_eq!(app.status, "Redo: Delete entities");
        let scene = app.document.current_scene.as_ref().unwrap();
        assert_eq!(scene.scene.entities.len(), before - 1);
        assert!(!scene.scene.entities.contains_key(&target));
        assert!(app.selection.is_empty());
        assert_eq!(app.selection.primary(), None);
        assert_scene_view_and_metadata_invalidated(&app);

        let _ = fs::remove_dir_all(root);
    }

    /// E02 Rename 收敛：重命名走统一入口（同一文档内编辑，不整表切换），
    /// 选中实体保持有效并随后同步 Inspector，同时清理陈旧 Scene View 缓存与
    /// Hierarchy 元数据 key。
    #[test]
    fn game_production_rename_selected_routes_unified_refresh() {
        let (mut app, root) = open_sample_production("rename");
        let scene = app.document.current_scene.clone().unwrap();
        assert!(scene.scene.entities.contains_key(&SceneEntityId(2)));
        app.selection
            .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);
        app.sync_inspector(SceneEntityId(2));
        poison_scene_view_caches(&mut app);

        app.entity_name = "HeroRenamed".to_owned();
        app.rename_selected();

        assert_eq!(app.status, "Entity renamed");
        let scene = app.document.current_scene.as_ref().unwrap();
        assert_eq!(
            scene.scene.entities.get(&SceneEntityId(2)).unwrap().name,
            "HeroRenamed"
        );
        // 选中实体仍在同一文档内 → 选择保留而非清空，Inspector 主源已同步新名。
        assert_eq!(app.selection.primary(), Some(SceneEntityId(2)));
        assert_eq!(app.entity_name, "HeroRenamed");
        assert_scene_view_and_metadata_invalidated(&app);

        let _ = fs::remove_dir_all(root);
    }

    /// E02 Prefab Apply 收敛：重新应用源 Prefab 会把整棵实例替换为重建的新实体，
    /// 统一入口必须让选择指向新的根 ID、保持其仍有效，并清理陈旧缓存与元数据。
    #[test]
    fn game_production_reapply_prefab_routes_unified_refresh() {
        let (mut app, root) = open_sample_production("prefab-apply");
        let old_root = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            scene
                .entities
                .values()
                .find(|entity| {
                    entity
                        .metadata
                        .get("prefab.root")
                        .is_some_and(|value| value == "true")
                        && entity
                            .metadata
                            .get("prefab.source")
                            .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
                })
                .unwrap()
                .id
        };
        app.selection
            .set_single(old_root, editor::SelectionSource::Inspector);
        app.sync_inspector(old_root);
        poison_scene_view_caches(&mut app);

        assert_eq!(
            app.dispatch(editor::EditorCommand::ReapplyPrefab { entity: old_root }),
            editor::CommandResult::Applied
        );
        assert!(
            app.status.contains("已重新应用 Prefab"),
            "status should confirm prefab reapply, got: {}",
            app.status
        );
        let new_root = app.selection.primary().unwrap();
        assert_ne!(
            new_root, old_root,
            "reapply rebuilds instance so selection must point at the new root"
        );
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        assert!(scene.entities.contains_key(&new_root));
        assert_eq!(
            scene
                .entities
                .get(&new_root)
                .unwrap()
                .metadata
                .get("prefab.source")
                .map(String::as_str),
            Some("assets/prefabs/slime_green.prefab")
        );
        assert!(
            !scene.entities.contains_key(&old_root),
            "old instance root must be removed"
        );
        assert_scene_view_and_metadata_invalidated(&app);

        let _ = fs::remove_dir_all(root);
    }

    /// E02 运行时修改应用（PlaybackStopped）收敛：Stop 丢弃运行态改动并恢复编辑器
    /// 快照，选择无条件清空、delete_target 复位，场景/Game 缓存与 Hierarchy 元数据
    /// 一并失效。
    #[test]
    fn game_production_playback_stop_routes_unified_refresh() {
        use crate::WorkspaceTab;
        use editor::PlaySession;
        use editor::PlayState;

        let (mut app, root) = open_sample_production("playback-stop");
        let original = app.document.current_scene.as_ref().unwrap().clone();

        // 模拟运行中：会话持有编辑器快照，运行时文档被脚本改动（如增删实体）。
        app.play_session = Some(PlaySession::start(&original));
        app.play.set_state_for_test(PlayState::Playing);
        app.workspace_tab = WorkspaceTab::Game;
        app.preview_input_state.clear_all();
        // 运行期脚本删掉一个实例（20），并选中一个实例（10）作为运行期选区。
        app.play_session
            .as_mut()
            .unwrap()
            .runtime_document_mut()
            .scene
            .entities
            .remove(&SceneEntityId(20));
        app.selection
            .set_single(SceneEntityId(10), editor::SelectionSource::Playback);
        poison_scene_view_caches(&mut app);

        app.stop();

        assert_eq!(app.play_state(), PlayState::Editing);
        assert!(app.play_session.is_none());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap(),
            &original,
            "stop must restore the untouched editor snapshot"
        );
        assert!(app.selection.is_empty());
        assert_eq!(app.selection.primary(), None);
        assert!(matches!(app.delete_target, crate::DeleteTarget::None));
        assert_scene_view_and_metadata_invalidated(&app);
        assert_eq!(app.status, "Play stopped; runtime changes were discarded");

        let _ = fs::remove_dir_all(root);
    }

    // ---- E06 未保存切换保护 ----

    /// 弹窗决策仅在 QuitEditor 路径会向原生窗口发送 ViewportCommand；
    /// OpenResource 等路径不依赖真实窗口，测试用裸 Context 即可驱动。
    fn test_context() -> egui::Context {
        egui::Context::default()
    }

    #[test]
    fn e06_dirty_open_is_pending_clean_open_is_immediate() {
        let (mut app, root) = open_sample_production("e06-pending");
        // 打开 production 时文档 clean：再次请求打开 menu 应立即执行，不挂起。
        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
        assert!(app.pending_document_change.is_none());
        assert_eq!(app.status, "已打开场景：scenes/ui_test.scene");
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "ui_test"
        );

        // 打 dirty：请求「打开 production」被挂起，当前文档不被替换。
        app.document.current_scene.as_mut().unwrap().mark_dirty();
        app.open_scene_resource(PathBuf::from("scenes/api_gameplay.scene"));
        assert!(app.pending_document_change.is_some());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "ui_test",
            "确认前不得切换文档"
        );

        // 已挂起 pending 时再次请求离开：被忽略，不覆盖也不重复弹窗。
        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
        assert!(app.pending_document_change.is_some());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "ui_test",
            "重复请求不得覆盖挂起的离开操作"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn e06_cancel_keeps_document_and_state() {
        let (mut app, root) = open_sample_production("e06-cancel");
        app.document.current_scene.as_mut().unwrap().mark_dirty();
        // 快照必须在 mark_dirty 之后：取消 = 保留这份 dirty 文档继续编辑。
        let doc_dirty = app.document.current_scene.as_ref().unwrap().clone();
        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
        assert!(app.pending_document_change.is_some());

        let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Cancel);
        assert!(resolved);
        assert!(app.pending_document_change.is_none());
        assert!(!app.pending_save_failed);
        assert_eq!(
            app.document.current_scene.as_ref().unwrap(),
            &doc_dirty,
            "取消后应保留 dirty 文档内容与状态（revision/dirty 均不变）"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn e06_discard_leaves_without_saving() {
        let (mut app, root) = open_sample_production("e06-discard");
        app.document.current_scene.as_mut().unwrap().mark_dirty();
        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
        assert!(app.pending_document_change.is_some());

        // 「放弃并继续」不保存直接切换，且切换后文档 clean（来自磁盘）。
        let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Discard);
        assert!(resolved);
        assert!(app.pending_document_change.is_none());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "ui_test",
            "放弃后应切到请求的场景"
        );
        assert!(!app.document.current_scene.as_ref().unwrap().is_dirty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn e06_save_success_saves_then_leaves() {
        let (mut app, root) = open_sample_production("e06-save-ok");
        // 打开 production（prefab 文档，path 指向磁盘）→ 正常「保存并继续」：
        // 保存成功 → 原文档 clean 并离开到 menu。
        let current_path = app
            .document
            .current_scene
            .as_ref()
            .unwrap()
            .path
            .clone()
            .expect("磁盘打开的文档应有路径");
        app.document.current_scene.as_mut().unwrap().mark_dirty();
        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
        assert!(app.pending_document_change.is_some());

        let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Save);
        assert!(resolved);
        assert!(app.pending_document_change.is_none());
        assert!(!app.pending_save_failed);
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "ui_test",
            "保存成功后应切到请求的场景"
        );

        // 原文档落盘且不再是 dirty（重新加载磁盘文件应等于保存时的内容）。
        let reloaded = editor::SceneDocument::open(&current_path).unwrap();
        assert!(!reloaded.is_dirty());
        let _ = fs::remove_dir_all(root);
    }

    /// E07：App 层事务网关。直接对历史提交事务一次连续编辑（模拟多帧预览写入），
    /// 最终只产生一条 Undo、dirty 一次、提交/取消状态机正确。
    #[test]
    fn e07_commit_transaction_produces_single_undo_and_dirty_once() {
        let (mut app, root) = open_examples_main("e07-commit");
        let id = SceneEntityId(2);
        let began = app.begin_edit_transaction(
            "Scene Transform",
            editor::CommandKind::Transform(id),
            Some(id),
        );
        assert!(began, "事务应成功开始");
        assert!(app.is_edit_transaction_active());
        let before = app.document.current_scene.clone().unwrap();

        // 多次更新（模拟拖拽帧），仅更新文档内容，不调用 save_scene。
        for frame in 0..5 {
            let scene = app.document.current_scene.as_mut().unwrap();
            scene
                .scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert("Transform".to_owned(), format!("position={frame},0"));
            scene.mark_dirty();
        }
        assert_eq!(app.history.undo_len(), 0, "事务期间不得追加历史（未提交）");

        app.commit_edit_transaction();
        assert!(!app.is_edit_transaction_active());
        assert_eq!(app.history.undo_len(), 1, "一次连续编辑只产生一条 Undo");

        // Undo：整文档回到事务开始前（单条快照模型 = before）。
        app.undo();
        assert_eq!(
            app.document.current_scene.as_ref().unwrap(),
            &before,
            "Undo 应回到事务开始前内容"
        );
        // Redo：回到事务结束内容。
        app.redo();
        let after = app.document.current_scene.as_ref().unwrap();
        assert_eq!(
            after.scene.entities[&id].components["Transform"],
            "position=4,0"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// E07：取消事务 → 无历史、无 dirty（干净文档上取消后仍是 clean），
    /// 文档内容回退到 begin 前。
    #[test]
    fn e07_cancel_transaction_no_history_no_dirty() {
        let (mut app, root) = open_examples_main("e07-cancel");
        let id = SceneEntityId(2);
        // 打开文档为 clean，作为 dirty 判定基线。
        assert!(!app.document.current_scene.as_ref().unwrap().is_dirty());
        let began = app.begin_edit_transaction(
            "Scene Transform",
            editor::CommandKind::Transform(id),
            Some(id),
        );
        assert!(began, "事务应成功开始");
        let before = app.document.current_scene.clone().unwrap();
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            scene
                .scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert("Transform".to_owned(), "position=99,99".to_owned());
            scene.mark_dirty();
        }

        app.cancel_edit_transaction();
        assert!(!app.is_edit_transaction_active());
        assert_eq!(app.history.undo_len(), 0, "取消不产生历史");
        assert!(
            !app.document.current_scene.as_ref().unwrap().is_dirty(),
            "取消不标记 Dirty"
        );
        assert_eq!(
            app.document.current_scene.as_ref().unwrap(),
            &before,
            "取消恢复事务开始前状态"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// E07：Undo 快捷键先取消活动事务（不产生历史/不改 dirty），再撤回到上一条
    /// 历史；活动事务不把预览内容泄漏成一条 Undo。
    #[test]
    fn e07_undo_guard_cancels_active_transaction_then_undoes() {
        let (mut app, root) = open_examples_main("e07-undo-guard");
        let id = SceneEntityId(2);
        let pristine = app.document.current_scene.clone().unwrap();
        // 先做一条离散编辑（产生 1 条历史、dirty）。
        app.selection
            .set_single(id, editor::SelectionSource::Hierarchy);
        app.entity_name = "RenamedByTest".to_owned();
        app.rename_selected();
        assert_eq!(app.history.undo_len(), 1);
        let after_rename = app.document.current_scene.clone().unwrap();
        assert_ne!(&after_rename, &pristine);

        // Begin 事务并写入内容。
        let began = app.begin_edit_transaction(
            "Scene Transform",
            editor::CommandKind::Transform(id),
            Some(id),
        );
        assert!(began, "事务应成功开始");
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            scene
                .scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert("Transform".to_owned(), "position=5,5".to_owned());
            scene.mark_dirty();
        }
        assert_eq!(app.history.undo_len(), 1, "事务内容尚未成为历史");

        // 撤销：先取消事务（回到 rename 后），再撤 rename。
        app.undo_with_transaction_guard();
        assert!(!app.is_edit_transaction_active());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap(),
            &pristine,
            "取消事务并撤销 rename 后应回到最开始的文档"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// E07：保存时若仍有活动事务，自动以单条历史落盘（save_scene 前兜底提交）。
    #[test]
    fn e07_save_flushes_active_transaction() {
        let (mut app, root) = open_examples_main("e07-save-flush");
        let id = SceneEntityId(2);
        let began = app.begin_edit_transaction(
            "Scene Transform",
            editor::CommandKind::Transform(id),
            Some(id),
        );
        assert!(began, "事务应成功开始");
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            scene
                .scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert("Transform".to_owned(), "position=3,3".to_owned());
            scene.mark_dirty();
        }
        assert!(app.is_edit_transaction_active());
        assert!(app.save_scene());
        assert!(!app.is_edit_transaction_active(), "保存后事务必须被消费");
        assert_eq!(app.history.undo_len(), 1, "保存前自动提交单条历史");
        let _ = fs::remove_dir_all(root);
    }

    // ── E08：Inspector 直写/提交边界 ─────────────────────────────────────────
    // 体验标准：拖动中磁盘不持续变化、一次拖拽一条历史；Inspector 输入提交后
    // 可撤销、不产生空操作；Escape 取消后不留拖拽残留。

    /// E08：Inspector 缓冲编辑先 `mark`（直写不写历史）再 `finish`（收敛一条
    /// 历史）；再次 finish 无差异 → no-op 不产生空历史；撤销一次回到编辑前。
    #[test]
    fn e08_inspector_mark_then_finish_yields_single_history_and_undo() {
        let (mut app, root) = open_sample_production("e08-inspector-mark-finish");
        // `Animation` 面板是仍在**缓冲**里的组件（Transform/Text/UiLayout 已迁到 schema 字段
        // 编辑器），因此这条测试挑一个**真的有 Animation 组件**的实体，而不是写死 id 2：
        // 示例工程的场景会随演示增删而变化，"id 2 必须是动画实体"只是历史巧合。
        let (id, default_clip) = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            let entity = scene
                .entities
                .values()
                .find(|entity| entity.components.contains_key("Animation"))
                .expect("示例场景应当至少有一个带 Animation 的实体");
            let machine =
                AnimationStateMachine::from_component_str(&entity.components["Animation"])
                    .expect("Animation 组件串应当可解析");
            (entity.id, machine.default)
        };
        app.selection
            .set_single(id, editor::SelectionSource::Hierarchy);
        app.sync_inspector(id);
        let pristine = app.document.current_scene.clone().unwrap();
        assert_eq!(app.history.undo_len(), 0);
        assert_eq!(app.animation.machine.default, default_clip);
        assert!(
            !app.animation.machine.clips.is_empty(),
            "动画实体应当至少有一个 clip"
        );

        // 帧内拖动/键入：mark 直写文档并置 pending，但不写历史、不落盘。
        app.animation.machine.default = "move".to_owned();
        app.mark_inspector_edit();
        assert!(app.is_edit_transaction_active(), "mark 应已自动开事务");
        assert!(app.inspector_edit.pending);
        assert_eq!(app.history.undo_len(), 0, "直写阶段不产生历史");

        // 同一交互多次 mark：不产生多条历史（一次拖拽=一帧一次 mark 的等价）。
        app.animation.machine.default = "attack".to_owned();
        app.mark_inspector_edit();
        app.animation.machine.default = "move".to_owned();
        app.mark_inspector_edit();
        assert_eq!(app.history.undo_len(), 0, "多次直写仍不产生历史");

        // 提交边界：收敛为一条历史。
        let changed = app.finish_inspector_edits_if_dirty();
        assert!(changed, "有真实 diff 时应提交一条历史");
        assert_eq!(app.history.undo_len(), 1, "一次交互收敛为一条历史");
        assert!(!app.inspector_edit.pending);
        assert!(!app.is_edit_transaction_active(), "提交后事务关闭");
        let committed = app.document.current_scene.clone().unwrap();
        assert_ne!(&committed, &pristine);

        // 无净差异的再次提交：no-op，不产生空历史。
        assert!(!app.finish_inspector_edits_if_dirty());
        assert_eq!(app.history.undo_len(), 1, "空操作不产生空历史");

        // 撤销一步回到编辑前。
        app.undo_with_transaction_guard();
        assert_eq!(
            app.document.current_scene.as_ref().unwrap(),
            &pristine,
            "撤销应回到 Inspector 编辑前"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// E08：Scene 拖拽（拖动中多帧直写文档）在松手提交前不写历史；提交后恰
    /// 一条历史、一次 undo 整体回退（不留逐帧碎片）。
    #[test]
    fn e08_scene_drag_direct_writes_commit_once_on_release() {
        let (mut app, root) = open_sample_production("e08-scene-drag-writes");
        let id = SceneEntityId(2);
        app.selection
            .set_single(id, editor::SelectionSource::Hierarchy);
        app.sync_inspector(id);
        let pristine = app.document.current_scene.clone().unwrap();
        assert_eq!(app.history.undo_len(), 0);
        // `Transform` 缓冲已随迁移删除（第 36 轮）：初值直接从**文档**读。
        let original_x = crate::geometry::parse_transform(
            &app.document.current_scene.as_ref().unwrap().scene.entities[&id].components
                ["Transform"],
        )
        .expect("Transform 可解析")
        .position_x;

        // 拖动首帧：begin 事务；随后多帧在事务内直写（等效 apply_drag_preview 逐帧写）。
        assert!(app.begin_edit_transaction(
            "Scene Transform",
            editor::CommandKind::Transform(id),
            Some(id),
        ));
        for offset in [10.0_f32, 25.0, 42.0] {
            let scene = app.document.current_scene.as_mut().unwrap();
            scene
                .scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert(
                    "Transform".to_owned(),
                    format!("x={};y=0", original_x + offset),
                );
            scene.mark_dirty();
            assert!(app.is_edit_transaction_active());
            assert_eq!(
                app.history.undo_len(),
                0,
                "拖动直写期间不产生历史（不写盘）"
            );
        }

        // 松手：提交为一条历史，dirty（交由 autosave / 保存点落盘）。
        let committed = app.commit_edit_transaction();
        assert!(committed, "拖拽有净变化应提交成功");
        assert_eq!(app.history.undo_len(), 1, "一次拖拽 = 一条历史");
        assert!(!app.is_edit_transaction_active());

        // 撤销一次 → 全部拖拽位移一次性回退（无碎片）。
        app.undo_with_transaction_guard();
        assert_eq!(
            app.document.current_scene.as_ref().unwrap(),
            &pristine,
            "单次 undo 应撤销整段拖动"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// E08：Escape 取消活动事务后清空 Scene View 拖拽残留 —— stale preview /
    /// gizmo 不留在状态里，避免下一帧把已取消的拖动重新写回文档。
    #[test]
    fn e08_escape_cancel_clears_scene_drag_residue() {
        let (mut app, root) = open_sample_production("e08-escape-clear");
        let id = SceneEntityId(2);
        app.selection
            .set_single(id, editor::SelectionSource::Hierarchy);
        app.sync_inspector(id);
        let pristine = app.document.current_scene.clone().unwrap();

        // 模拟一个已开事务的拖拽 + preview / gizmo 残留。
        assert!(app.begin_edit_transaction(
            "Scene Transform",
            editor::CommandKind::Transform(id),
            Some(id),
        ));
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            scene
                .scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert(
                    "Transform".to_owned(),
                    "position=999.000000000,999.000000000,0.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000"
                        .to_owned(),
                );
            scene.mark_dirty();
        }
        // 拖拽预览值从**文档**读（`Transform` 缓冲已随迁移删除，第 36 轮）。
        let preview_transform = crate::geometry::parse_transform(
            &app.document.current_scene.as_ref().unwrap().scene.entities[&id].components
                ["Transform"],
        )
        .expect("Transform 可解析");
        app.scene_state.preview.insert(id, preview_transform);
        app.scene_state
            .preview_ui
            .insert(id, [3.0, -5.0, 3.0, -5.0]);
        app.scene_state
            .preview3d_transform
            .insert(id, Transform3D::default());
        assert!(app.is_edit_transaction_active());

        // Escape 取消：文档回滚 + 残留清理。
        app.cancel_edit_transaction();
        app.clear_active_scene_drag();

        assert!(!app.is_edit_transaction_active());
        assert_eq!(app.history.undo_len(), 0, "取消不产生历史");
        assert_eq!(
            app.document.current_scene.as_ref().unwrap(),
            &pristine,
            "取消回滚到事务前"
        );
        assert!(app.scene_state.preview.is_empty());
        assert!(app.scene_state.preview_ui.is_empty());
        assert!(app.scene_state.preview3d_transform.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn e06_save_failure_keeps_dirty_and_pending() {
        let (mut app, root) = open_sample_production("e06-save-fail");
        app.document.current_scene.as_mut().unwrap().mark_dirty();
        // 请求离开到 menu（dirty → pending；只挂起，不碰磁盘）。
        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
        assert!(app.pending_document_change.is_some());
        let doc_before = app.document.current_scene.as_ref().unwrap().clone();

        // 破坏保存目标：scenes 目录替换为同名普通文件 → 原子写 create_dir_all 必失败。
        let scenes_dir = app.project.root.join("scenes");
        let _ = fs::remove_dir_all(&scenes_dir);
        fs::write(&scenes_dir, "not a directory").unwrap();

        // 「保存并继续」保存失败：pending 保留、save_failed 置位、文档停在原地且保持 dirty。
        let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Save);
        assert!(!resolved, "保存失败时不得消费 pending");
        assert!(app.pending_document_change.is_some());
        assert!(app.pending_save_failed);
        assert!(app.document.current_scene.as_ref().unwrap().is_dirty());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap(),
            &doc_before,
            "保存失败后文档不得被改动"
        );

        // 修复磁盘（恢复 scenes 目录与 menu.scene 副本）后重试「保存并继续」：
        // 保存成功 → pending 消费、切到 menu、save_failed 复位。
        let _ = fs::remove_file(&scenes_dir);
        fs::create_dir_all(&scenes_dir).unwrap();
        let sample_scenes = sample_project_root().join("scenes");
        for entry in fs::read_dir(&sample_scenes).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), scenes_dir.join(entry.file_name())).unwrap();
        }
        let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Save);
        assert!(resolved);
        assert!(app.pending_document_change.is_none());
        assert!(!app.pending_save_failed);
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "ui_test",
            "修复后保存成功应切到请求的场景"
        );

        let _ = fs::remove_dir_all(root);
    }
}
