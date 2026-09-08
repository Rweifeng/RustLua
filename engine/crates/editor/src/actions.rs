//! 跨面板共享的编辑命令：撤销/重做、保存、场景切换、实体创建、Inspector 同步、对齐。

use crate::AlignMode;
use crate::EditorApp;
use crate::geometry::{encode_transform, parse_transform};
use editor::{
    Camera3DFields, Collider3dFields, CommandKind, ImageFields, InspectorSnapshot, LightFields,
    SceneDocument, TextFields, Transform3DFields, TransformFields,
};
use eframe::egui;
use engine::core::{
    animation::AnimationStateMachine, scene::SceneEntityId, sprite::SpriteComponent,
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
/// 面板显示层的裸引用/组件对象（`mesh_renderer` 的 `mesh=` 前缀、`sprite_component`
/// 结构体）只作为「编码源」，不直接参与比较，避免显示层噪音造成空操作直写。
///
/// 写入端（`update_inspector_write`）要求写入字段与文档实际字段**逐字节一致**
/// 才能让 `write_inspector` 的 `Ok(*document != before)` 净差异判定正确：
/// - `mesh_renderer`：文档经 `MeshRenderer::to_component_str` 规范化
///   （`mesh=<path>`）。缓冲只存裸引用，比较前拼上 `mesh=` 前缀生成待写值；
///   实体**没有** MeshRenderer 组件时快照为 `None`，write 端会 clear（不会注入）。
/// - `sprite_reference`：文档组件串与缓冲组件的 `SpriteComponent::to_component_str`
///   编码可能因键序/尾分号漂移，因此快照用「面板组件编码串」，与 `write_inspector`
///   判定一致（面板每帧末尾同样把 `SpriteComponent` 编码回引用）。
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
    /// 直写用 mesh 完整组件串（与文档逐字节一致才判等；面板显示用裸引用）。
    pub mesh_renderer_raw: Option<String>,
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
            mesh_renderer_raw: None,
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

    /// 从磁盘打开一个场景/Prefab 文档，并通过统一入口替换当前文档。
    pub(crate) fn open_scene_document(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        let document = SceneDocument::open(path).map_err(|error| error.to_string())?;
        self.set_current_scene_document(document);
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
        let label = {
            let h = &mut self.history;
            let d = &mut self.document;
            d.current_scene
                .as_mut()
                .and_then(|x| h.undo(x).ok())
                .map(str::to_owned)
        };
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
                    transform,
                    sprite_reference,
                    lua_script,
                    transform3d,
                    mesh_renderer: _,
                    camera3d,
                    light,
                    collider3d,
                    text,
                    image,
                    ui_layout,
                    container,
                    ..
                }),
            mesh_renderer_raw,
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
        // 多选批量 Sprite 编辑：其余同带 Sprite 的选中实体一并写入（对齐离散路径
        // apply_inspector 的 selected_sprite_ids 语义）。
        let multi_sprite_ids = self.selection.ids();
        let inspector = self.inspector.clone();
        // 直写快照使用与文档可判等的字段：mesh 用完整组件串、sprite 沿用组件串
        // （面板在帧末把 SpriteComponent 编码回引用），persistent 位由离散路径处理。
        let write = inspector.write_inspector(
            scene,
            entity_id,
            InspectorSnapshot {
                entity_id,
                name,
                transform,
                sprite_reference,
                lua_script,
                transform3d,
                mesh_renderer: mesh_renderer_raw,
                camera3d,
                light,
                collider3d,
                text,
                image,
                ui_layout,
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
                mesh_renderer_raw: None,
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
        let sprite_ref = self.sprite_component.to_component_str();
        let sprite_reference = has("Sprite")
            .then(|| (!self.sprite_component.texture.trim().is_empty()).then_some(sprite_ref))
            .flatten();
        let lua_script = has("LuaScript")
            .then(|| (!self.lua_script.trim().is_empty()).then_some(self.lua_script.clone()))
            .flatten();
        let mesh_renderer = has("MeshRenderer")
            .then(|| (!self.mesh_renderer.trim().is_empty()).then_some(self.mesh_renderer.clone()))
            .flatten();
        let snapshot = InspectorSnapshot {
            entity_id: primary,
            name: self.entity_name.clone(),
            transform: self.transform,
            sprite_reference,
            lua_script,
            transform3d: has("Transform3D").then_some(self.transform3d),
            mesh_renderer,
            camera3d: has("Camera3D").then_some(self.camera3d),
            light: has("Light").then_some(self.light),
            collider3d: has("Collider3d").then_some(self.collider3d.clone()),
            text: has("Text").then_some(self.text.clone()),
            image: has("Image").then_some(self.image.clone()),
            ui_layout: has("UiLayout").then_some(self.ui_layout.clone()),
            container: has("Container").then_some(self.container.clone()),
            persistent: false,
        };
        // 直写用 mesh 完整组件串：缓冲只存 `mesh=` 后的裸引用，直写需拼回前缀
        // 与 `write_inspector`/文档规范化格式逐字节一致。实体无 MeshRenderer 时
        // 快照为 None，write 端 clear；这里同样为 None，diff 不受影响。
        let mesh_renderer_raw = snapshot
            .mesh_renderer
            .as_deref()
            .map(|reference| format!("mesh={reference}"));
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
            mesh_renderer_raw,
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
        self.scene_state.drag = None;
        self.scene_state.gizmo_drag = None;
        self.scene_state.box_select_start = None;
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
        let label = {
            let h = &mut self.history;
            let d = &mut self.document;
            d.current_scene
                .as_mut()
                .and_then(|x| h.redo(x).ok())
                .map(str::to_owned)
        };
        if label.is_some() {
            self.on_document_changed(DocumentChange::Redo);
        }
        self.status = label.map_or_else(|| "Nothing to redo".into(), |x| format!("Redo: {x}"));
    }

    /// 保存当前场景（未命名时写入 scenes/<场景名>.scene）。
    ///
    /// E06：返回是否保存成功。失败（如场景无路径、磁盘写入失败、校验失败）时文档
    /// 保持 dirty 且不变更内容，供「保存并继续」确认流判断是否需要停留在当前文档。
    pub(crate) fn save_scene(&mut self) -> bool {
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
                self.recovery_scan = self.recovery.scan().unwrap_or_default();
                if is_prefab_document {
                    "Prefab 已保存".into()
                } else {
                    "Scene saved".into()
                }
            }
            Err(error) => error,
        };
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
            self.transform = snapshot.transform;
            self.transform3d = snapshot.transform3d.unwrap_or_default();
            self.mesh_renderer = snapshot.mesh_renderer.unwrap_or_default();
            self.camera3d = snapshot.camera3d.unwrap_or_default();
            self.light = snapshot.light.unwrap_or_default();
            self.collider3d = snapshot.collider3d.unwrap_or_default();
            self.text = snapshot.text.unwrap_or_default();
            self.image = snapshot.image.unwrap_or_default();
            self.ui_layout = snapshot.ui_layout.unwrap_or_default();
            self.container = snapshot.container.unwrap_or_default();
            self.sprite_reference = snapshot.sprite_reference.clone().unwrap_or_default();
            self.lua_script = snapshot.lua_script.unwrap_or_default();
            self.refresh_ui_editors(id);
            self.sprite_component = snapshot
                .sprite_reference
                .as_deref()
                .and_then(|value| SpriteComponent::from_component_str(value).ok())
                .unwrap_or_else(|| SpriteComponent {
                    texture: "assets/textures/player.png".to_owned(),
                    size: engine::core::transform::Vec2::new(128.0, 128.0),
                    pivot: engine::core::transform::Vec2::new(0.5, 0.5),
                    tint: [1.0; 4],
                    sorting: engine::core::sprite::SpriteSorting::default(),
                });
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
                            sheet: self.sprite_component.texture.clone(),
                            cols: 1,
                            frame_rate: 1.0,
                            looped: true,
                        }],
                    )
                    .expect("default animation")
                });
        } else {
            // 目标不存在：清空回读字段，避免残留上一实体的快照。
            let fields = (
                &mut self.entity_name,
                &mut self.sprite_reference,
                &mut self.mesh_renderer,
                &mut self.lua_script,
            );
            fields.0.clear();
            fields.1.clear();
            fields.2.clear();
            fields.3.clear();
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
        self.transform = TransformFields::default();
        self.transform3d = Transform3DFields::default();
        self.mesh_renderer.clear();
        self.camera3d = Camera3DFields::default();
        self.light = LightFields::default();
        self.collider3d = Collider3dFields::default();
        self.text = TextFields::default();
        self.image = ImageFields::default();
        self.sprite_reference.clear();
        self.sprite_component = SpriteComponent::default();
        self.tilemap.map = Tilemap::new(engine::core::transform::Vec2::new(32.0, 32.0), 8, 8)
            .expect("default tilemap");
        self.animation.machine = AnimationStateMachine::new(
            "idle",
            vec![engine::core::animation::AnimationClip {
                name: "idle".to_owned(),
                sheet: self.sprite_component.texture.clone(),
                cols: 1,
                frame_rate: 1.0,
                looped: true,
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

    pub(crate) fn add_entity_with_component(&mut self, name: &str, component: &str) {
        self.entity_name = name.to_owned();
        self.add_entity(None);
        if let Some(id) = self.selection.primary() {
            let _ = self.add_component(component);
            self.sync_inspector(id);
        }
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
                self.scene_font_atlas = None;
                self.custom_font_atlases.lock().unwrap().clear();
            }
            Err(error) => self.status = format!("收集字符失败: {error}"),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditorApp, EditorCamera, SceneViewCache2d, SceneViewCache3d};
    use editor::open_project;
    use engine::core::{logging::LogSink, transform::Vec2, transform3d::Transform3D};
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn unique_token() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos()
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("m15-action-{label}-{}", unique_token()))
    }

    fn sample_project_root() -> PathBuf {
        engine::core::games_dir().join("examples")
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

        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));

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

    /// 打开 examples 副本并打开其 scenes/production.scene。
    /// 与 inspector tests 的 sample_app_with_production_scene 行为保持一致
    /// （拷贝 assets/scenes 后不依赖资源索引，Prefab 按相对路径加载）。
    fn open_sample_production(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_sample_project(label);
        let project = open_project(&root).expect("production 副本应可打开");
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));
        (app, root)
    }

    /// 打开 examples 副本并打开其 scenes/main.scene（8 个根实体）。
    fn open_examples_main(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_examples_project(label);
        let project = open_project(&root).expect("examples 副本应可打开");
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.open_scene_resource(PathBuf::from("scenes/main.scene"));
        (app, root)
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
        app.begin_edit_transaction("Inspector", editor::CommandKind::Property, Some(SceneEntityId(2)));
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
        assert!(
            app.scene_view_fit_pending,
            "整表切换应触发视图适配"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn examples_main_open_scene_routes_unified_refresh() {
        let (mut app, root) = open_examples_main("examples-open");
        {
            let scene = app.document.current_scene.clone().expect("已打开场景");
            assert_eq!(scene.scene.entities.len(), 8);
            assert!(scene.scene.entities.contains_key(&SceneEntityId(4)));
        }
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
            8
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
        app.selection
            .set_single(SceneEntityId(4), editor::SelectionSource::Hierarchy);
        app.delete_target = crate::DeleteTarget::Hierarchy;
        poison_scene_view_caches(&mut app);

        // 删除根实体：删除成功后统一入口必须清理悬空选择与陈旧缓存。
        app.delete_selected_entity();
        assert_eq!(app.status, "Entity deleted");
        let scene = app.document.current_scene.as_ref().unwrap();
        assert_eq!(scene.scene.entities.len(), 7);
        assert!(!scene.scene.entities.contains_key(&SceneEntityId(4)));
        assert!(app.selection.is_empty());
        assert_eq!(app.selection.primary(), None);
        assert!(matches!(app.delete_target, crate::DeleteTarget::None));
        assert_scene_view_and_metadata_invalidated(&app);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn examples_main_undo_redo_routes_unified_refresh() {
        let (mut app, root) = open_examples_main("examples-undo");
        let scene = app.document.current_scene.clone().unwrap();
        assert_eq!(scene.scene.entities.len(), 8);
        app.selection
            .set_single(SceneEntityId(4), editor::SelectionSource::Hierarchy);
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
            7
        );
        assert_scene_view_and_metadata_invalidated(&app);

        // Undo 走统一入口：整文档回到删除前快照，Selection/缓存一并收敛。
        app.undo();
        assert_eq!(app.status, "Undo: Delete entities");
        let scene = app.document.current_scene.as_ref().unwrap();
        assert_eq!(scene.scene.entities.len(), 8);
        assert!(scene.scene.entities.contains_key(&SceneEntityId(4)));
        assert!(app.selection.is_empty());
        assert_eq!(app.selection.primary(), None);
        assert_scene_view_and_metadata_invalidated(&app);

        // Redo 同样经统一入口：删除再次生效。
        app.redo();
        assert_eq!(app.status, "Redo: Delete entities");
        let scene = app.document.current_scene.as_ref().unwrap();
        assert_eq!(scene.scene.entities.len(), 7);
        assert!(!scene.scene.entities.contains_key(&SceneEntityId(4)));
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

        app.reapply_selected_prefab();

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
        use crate::PlayState;
        use crate::WorkspaceTab;
        use editor::PlaySession;

        let (mut app, root) = open_sample_production("playback-stop");
        let original = app.document.current_scene.as_ref().unwrap().clone();

        // 模拟运行中：会话持有编辑器快照，运行时文档被脚本改动（如增删实体）。
        app.play_session = Some(PlaySession::start(&original));
        app.play_state = PlayState::Playing;
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

        assert_eq!(app.play_state, PlayState::Editing);
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
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
        assert!(app.pending_document_change.is_none());
        assert_eq!(app.status, "已打开场景：scenes/menu.scene");
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "menu"
        );

        // 打 dirty：请求「打开 production」被挂起，当前文档不被替换。
        app.document.current_scene.as_mut().unwrap().mark_dirty();
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));
        assert!(app.pending_document_change.is_some());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "menu",
            "确认前不得切换文档"
        );

        // 已挂起 pending 时再次请求离开：被忽略，不覆盖也不重复弹窗。
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
        assert!(app.pending_document_change.is_some());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "menu",
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
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
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
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
        assert!(app.pending_document_change.is_some());

        // 「放弃并继续」不保存直接切换，且切换后文档 clean（来自磁盘）。
        let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Discard);
        assert!(resolved);
        assert!(app.pending_document_change.is_none());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "menu",
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
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
        assert!(app.pending_document_change.is_some());

        let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Save);
        assert!(resolved);
        assert!(app.pending_document_change.is_none());
        assert!(!app.pending_save_failed);
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.name,
            "menu",
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
        let id = SceneEntityId(2);
        app.selection
            .set_single(id, editor::SelectionSource::Hierarchy);
        app.sync_inspector(id);
        let pristine = app.document.current_scene.clone().unwrap();
        assert_eq!(app.history.undo_len(), 0);
        let original_x = app.transform.position_x;

        // 帧内拖动/键入：mark 直写文档并置 pending，但不写历史、不落盘。
        app.transform.position_x = original_x + 40.0;
        app.mark_inspector_edit();
        assert!(app.is_edit_transaction_active(), "mark 应已自动开事务");
        assert!(app.inspector_edit.pending);
        assert_eq!(app.history.undo_len(), 0, "直写阶段不产生历史");

        // 同一交互多次 mark：不产生多条历史（一次拖拽=一帧一次 mark 的等价）。
        app.transform.position_x = original_x + 55.0;
        app.mark_inspector_edit();
        app.transform.position_x = original_x + 70.0;
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
        let original_x = app.transform.position_x;

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
                .insert("Transform".to_owned(), "x=999;y=999".to_owned());
            scene.mark_dirty();
        }
        app.scene_state.preview.insert(id, app.transform);
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
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
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
            "menu",
            "修复后保存成功应切到请求的场景"
        );

        let _ = fs::remove_dir_all(root);
    }
}
