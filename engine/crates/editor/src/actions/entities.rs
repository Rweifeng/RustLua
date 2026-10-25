//! 实体创建：场景实体、UI 实体与控件的落位、稳定 ID（persist_id）的维护、
//! 以及字体字符集的收集。

use super::*;

impl EditorApp {
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
            return Err("播放模式下不能编辑场景，请先点击停止".to_owned());
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
                return Err(format!("本场景已存在持久化 ID「{value}」"));
            }
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Err("没有打开的场景".to_owned());
        };
        let label = match value.as_deref() {
            Some(value) => format!("持久化 ID {value}"),
            None => "清除持久化 ID".to_owned(),
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
            self.status = "播放模式下不能编辑场景，请先点击停止".into();
            return;
        }
        let name = self.entity_name.trim().to_owned();
        if name.is_empty() {
            self.status = "实体名不能为空".into();
            return;
        }
        let tree = self.tree.clone();
        let created = std::cell::Cell::new(None::<SceneEntityId>);
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "请先创建场景".into();
            return;
        };
        let result = self
            .history
            .execute(scene, "创建实体", CommandKind::Create, |scene| {
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
                self.status = "实体已创建".into();
            }
            Ok(false) => self.status = "没有变化".into(),
            Err(e) => self.status = e,
        }
    }

    /// Unity 风格：新建 UI 实体时自动确保场景里有一个 Canvas 根（全屏 UiLayout），
    /// 新 UI 实体挂到 Canvas 下。已有 Canvas 就直接复用。
    pub(crate) fn create_ui_entity(&mut self) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".into();
            return;
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "请先创建场景".into();
            return;
        };
        let tree = self.tree.clone();
        let created = std::cell::Cell::new(None::<SceneEntityId>);
        let result = self
            .history
            .execute(scene, "创建界面实体", CommandKind::Create, |scene| {
                // 1. 找或建 UI 根节点（metadata ui.root=true）。
                let root_id = scene.scene.entities.values()
                    .find(|e| e.metadata.get("ui.root").map(|v| v == "true").unwrap_or(false))
                    .map(|e| e.id);
                let root_id = if let Some(id) = root_id {
                    id
                } else {
                    let id = tree.create_entity(scene, "界面根节点", None).map_err(|e| e.to_string())?;
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
                let id = tree.create_entity(scene, "界面元素", Some(root_id))
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
                self.status = "界面实体已创建到界面根节点下".into();
            }
            Ok(false) => self.status = "没有变化".into(),
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
        let _ = self
            .history
            .execute(scene, "摆放控件", CommandKind::Property, move |scene| {
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    crate::scene_drag::set_ui_component_layout(entity, offsets);
                }
                scene.mark_dirty();
                Ok::<(), InspectorError>(())
            });
    }

    /// 5b：在设计分辨率指定位置创建 widget（面板 → 设计像素已换算）。
    /// 默认 200×100 尺寸，中心锚点，按 widget 类型附加视觉/事件组件。
    pub(crate) fn create_widget_at(&mut self, widget: &str, design_pos: egui::Pos2) {
        let (name, component) = match widget {
            "Text" => ("新建文本", "Text"),
            "Image" => ("新建图片", "Image"),
            "Button" => ("新建按钮", "Image"),
            "Panel" => ("新建面板", "Image"),
            "Container" => ("新建容器", "Container"),
            _ => {
                self.status = format!("未知控件类型：{widget}");
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
        if widget == "Button"
            && let Err(error) = self.add_component("UiEvents")
        {
            self.status = error;
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
        let chars = rustlua_assets_import::char_collect::collect_project_chars(&root);
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
