//! 跨面板共享的编辑命令：撤销/重做、保存、场景切换、实体创建、Inspector 同步、对齐。

use crate::AlignMode;
use crate::EditorApp;
use crate::geometry::{encode_transform, parse_transform};
use editor::{
    Camera3DFields, Collider3dFields, CommandKind, ImageFields, LightFields, SceneDocument,
    TextFields, Transform3DFields, TransformFields,
};
use engine::core::{
    animation::AnimationStateMachine, scene::SceneEntityId, sprite::SpriteComponent,
    tilemap::Tilemap,
};
use std::path::{Path, PathBuf};
use eframe::egui;

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
        self.invalidate_scene_view_state();
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
        // 新文档打开后把 2D 视图适配到设计分辨率包围框。
        self.scene_view_fit_pending = true;
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

        // 4. Game Preview 失效：整文档替换类变更（打开/新建/恢复）不允许残留运行时预览。
        if change == DocumentChange::Opened || change == DocumentChange::Created {
            self.play_session = None;
            self.preview = None;
            self.preview_input_state.clear_all();
        }

        // 5. Hierarchy 元数据刷新：置空 key 强制下一帧 sync_hierarchy_metadata 全量重扫，
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
                // 打开文档本身已完成 Scene View 失效；整面板状态收敛到统一入口
                // （选择修复 / Inspector / Game 预览清理 / Hierarchy 元数据重扫）。
                self.on_document_changed(DocumentChange::Opened);
                // 打开场景文档后不再指向资源面板的选中项（Delete 快捷键回到 Hierarchy 外）。
                self.clear_resource_selection();
                self.scene_state.collapsed.clear();
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
    pub(crate) fn sync_inspector(&mut self, id: SceneEntityId) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let scene = scene.clone();
        self.focus_hierarchy_delete_target();
        if let Ok(snapshot) = self.inspector.snapshot(&scene, id)
        {
            self.entity_name = snapshot.name;
            self.transform = snapshot.transform;
            self.transform3d = snapshot.transform3d.unwrap_or_default();
            self.mesh_renderer = snapshot.mesh_renderer.unwrap_or_default();
            self.camera3d = snapshot.camera3d.unwrap_or_default();
            self.light = snapshot.light.unwrap_or_default();
            self.collider3d = snapshot.collider3d.unwrap_or_default();
            self.text = snapshot.text.unwrap_or_default();
            self.image = snapshot.image.unwrap_or_default();
            self.sprite_reference = snapshot.sprite_reference.clone().unwrap_or_default();
            self.lua_script = snapshot.lua_script.unwrap_or_default();
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
        }
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
        let result =
            self.history
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
        engine::core::games_dir().join("game_test_2d_production")
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

    // E02 回归测试：使用仓库内 game_jhyt 工程（standalone 游戏，project.toml +
    // scripts/scenes；不复制巨大的 assets，也绝不调用 refresh_resources）。
    fn jhyt_project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .expect("editor crate 上溯三级应为 p2 工作区根")
            .join("game_jhyt")
    }

    fn copy_jhyt_project(label: &str) -> PathBuf {
        let source = jhyt_project_root();
        assert!(
            source.join("project.toml").is_file(),
            "game_jhyt 工程缺失: {}",
            source.display()
        );
        assert!(
            source.join("scenes/main.scene").is_file(),
            "game_jhyt 缺少 scenes/main.scene"
        );
        let root = temp_root(label);
        fs::create_dir_all(&root).unwrap();
        fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
        copy_dir(&source.join("scripts"), &root.join("scripts"));
        copy_dir(&source.join("scenes"), &root.join("scenes"));
        root
    }

    /// 打开 game_test_2d_production 副本并打开其 scenes/production.scene。
    /// 与 inspector tests 的 sample_app_with_production_scene 行为保持一致
    /// （拷贝 assets/scenes 后不依赖资源索引，Prefab 按相对路径加载）。
    fn open_sample_production(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_sample_project(label);
        let project = open_project(&root).expect("production 副本应可打开");
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));
        (app, root)
    }

    /// 打开 game_jhyt 副本并打开其 scenes/main.scene（4 个根实体）。
    fn open_jhyt_main(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_jhyt_project(label);
        let project = open_project(&root).expect("game_jhyt 副本应可打开");
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

    #[test]
    fn game_jhyt_open_scene_routes_unified_refresh() {
        let (mut app, root) = open_jhyt_main("jhyt-open");
        {
            let scene = app.document.current_scene.clone().expect("已打开场景");
            assert_eq!(scene.scene.entities.len(), 4);
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
            4
        );
        assert!(app.selection.is_empty());
        assert_eq!(app.selection.primary(), None);
        assert!(matches!(app.delete_target, crate::DeleteTarget::None));
        assert_scene_view_and_metadata_invalidated(&app);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn game_jhyt_delete_selected_root_routes_unified_refresh() {
        let (mut app, root) = open_jhyt_main("jhyt-delete");
        app.selection
            .set_single(SceneEntityId(4), editor::SelectionSource::Hierarchy);
        app.delete_target = crate::DeleteTarget::Hierarchy;
        poison_scene_view_caches(&mut app);

        // 删除根实体：删除成功后统一入口必须清理悬空选择与陈旧缓存。
        app.delete_selected_entity();
        assert_eq!(app.status, "Entity deleted");
        let scene = app.document.current_scene.as_ref().unwrap();
        assert_eq!(scene.scene.entities.len(), 3);
        assert!(!scene.scene.entities.contains_key(&SceneEntityId(4)));
        assert!(app.selection.is_empty());
        assert_eq!(app.selection.primary(), None);
        assert!(matches!(app.delete_target, crate::DeleteTarget::None));
        assert_scene_view_and_metadata_invalidated(&app);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn game_jhyt_undo_redo_routes_unified_refresh() {
        let (mut app, root) = open_jhyt_main("jhyt-undo");
        let scene = app.document.current_scene.clone().unwrap();
        assert_eq!(scene.scene.entities.len(), 4);
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
            3
        );
        assert_scene_view_and_metadata_invalidated(&app);

        // Undo 走统一入口：整文档回到删除前快照，Selection/缓存一并收敛。
        app.undo();
        assert_eq!(app.status, "Undo: Delete entity");
        let scene = app.document.current_scene.as_ref().unwrap();
        assert_eq!(scene.scene.entities.len(), 4);
        assert!(scene.scene.entities.contains_key(&SceneEntityId(4)));
        assert!(app.selection.is_empty());
        assert_eq!(app.selection.primary(), None);
        assert_scene_view_and_metadata_invalidated(&app);

        // Redo 同样经统一入口：删除再次生效。
        app.redo();
        assert_eq!(app.status, "Redo: Delete entity");
        let scene = app.document.current_scene.as_ref().unwrap();
        assert_eq!(scene.scene.entities.len(), 3);
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
            scene.entities.get(&new_root).unwrap().metadata.get("prefab.source").map(String::as_str),
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
        use editor::PlaySession;
        use crate::PlayState;
        use crate::WorkspaceTab;

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
        app.document
            .current_scene
            .as_mut()
            .unwrap()
            .mark_dirty();
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
        app.document
            .current_scene
            .as_mut()
            .unwrap()
            .mark_dirty();
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
        app.document
            .current_scene
            .as_mut()
            .unwrap()
            .mark_dirty();
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
        assert!(!app
            .document
            .current_scene
            .as_ref()
            .unwrap()
            .is_dirty());

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
        app.document
            .current_scene
            .as_mut()
            .unwrap()
            .mark_dirty();
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

    #[test]
    fn e06_save_failure_keeps_dirty_and_pending() {
        let (mut app, root) = open_sample_production("e06-save-fail");
        app.document
            .current_scene
            .as_mut()
            .unwrap()
            .mark_dirty();
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
