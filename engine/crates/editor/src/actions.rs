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
    pub(crate) fn save_scene(&mut self) {
        if !self.project_loaded {
            self.status = "Import a game project before editing".to_owned();
            return;
        }
        let Some(doc) = self.document.current_scene.as_mut() else {
            self.status = "No scene".into();
            return;
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
    }

    /// 打开场景或 Prefab 资源（资源面板 / 层级面板）。
    pub(crate) fn open_scene_resource(&mut self, relative_path: PathBuf) {
        if !self.project_loaded {
            return;
        }
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
}
