//! 文档读写：保存（含外部改动冲突策略）、资源打开、文档替换与缓存失效、
//! Inspector 与文档的同步。磁盘格式与冲突判定在 `editor` crate，这里只编排顺序。

use super::*;

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

    pub(crate) fn save_scene_with_policy(&mut self, force: bool) -> bool {
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
    pub(crate) fn perform_open_scene_resource(&mut self, relative_path: PathBuf) {
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
                    Tilemap::new(rustlua_ecs::transform::Vec2::new(32.0, 32.0), 8, 8)
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
                        vec![rustlua_animation::animation::AnimationClip {
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
        self.tilemap.map = Tilemap::new(rustlua_ecs::transform::Vec2::new(32.0, 32.0), 8, 8)
            .expect("default tilemap");
        self.animation.machine = AnimationStateMachine::new(
            "idle",
            vec![rustlua_animation::animation::AnimationClip {
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
}
