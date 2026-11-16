//! 属性面板的支撑方法：Apply 入口、默认组件取值、组件复制（M79 结构切分，零行为变化）。
use super::*;

impl EditorApp {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn apply_inspector(&mut self) -> bool {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return false;
        }
        if self.selection.primary().is_none() {
            self.status = "应用修改".to_owned();
            return false;
        }
        self.finish_inspector_edits_if_dirty()
    }
    // E08：整面板在每帧末尾统一经帧尾提交边界收敛缓冲并刷新基线；apply_inspector
    // 保留给测试直接调用（写入 + 收敛为一条历史的一步语义）。

    // 添加/移除 Collider3d 的按钮走 `EditorCommand::AddCollider3d` /
    // `RemoveCollider3d`：实现体住在 shell（`crate::actions`），面板不再直接写文档。

    /// 选中实体 `Sprite` 组件的纹理路径（Animation 面板新建 Clip 的默认 sheet 用它）。
    ///
    /// `Sprite` 迁到 schema 编辑器后不再有缓冲组件，这里直接从**文档**读（语义与迁移前一致：
    /// 面板每帧都会把缓冲写回文档）；没有 Sprite/纹理为空时回退到"添加组件"的默认纹理。
    pub(crate) fn selected_sprite_texture(&self) -> String {
        self.current_scene()
            .and_then(|document| {
                let id = self.selection.primary()?;
                let payload = document.scene.entities.get(&id)?.components.get("Sprite")?;
                rustlua_render::sprite::SpriteComponent::from_component_str(payload)
                    .ok()
                    .map(|sprite| sprite.texture)
            })
            .filter(|texture| !texture.trim().is_empty())
            .unwrap_or_else(|| "assets/textures/player.png".to_owned())
    }

    /// 选中实体的组件键列表（Inspector 组件管理用），按 `editor.component_order` 排序。
    pub(super) fn current_components(&self) -> Vec<String> {
        self.current_scene()
            .and_then(|s| {
                self.selection.primary().map(|id| {
                    s.scene
                        .entities
                        .get(&id)
                        .map(editor::display_components)
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default()
    }

    // 注册表里是否有这个组件（Add Component 的两个入口共用同一条判定与文案）。

    /// 为实体写入 `name` 组件的默认取值。
    ///
    /// 这是**纯函数**：只吃显式传入的场景与 Inspector 状态，不碰 `self.document` /
    /// `self.history` / 选区——提交边界住在 shell 的 `EditorApp::add_component_to`
    /// （`crate::actions`）。这里留下的只有"注册表 → 默认值"这张表。
    pub(crate) fn insert_default_component(
        scene: &mut editor::SceneDocument,
        inspector: &InspectorState,
        selected: SceneEntityId,
        name: &str,
    ) -> Result<(), InspectorError> {
        match name {
            // `Sprite` 的手写面板已删除（第 34 轮）：默认值取**引擎类型**的编码
            // （纹理/尺寸/排序层都是引擎与"添加组件"共用的默认），编辑由通用字段编辑器接管。
            "Sprite" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "Sprite".to_owned(),
                        rustlua_render::sprite::SpriteComponent {
                            texture: "assets/textures/player.png".to_owned(),
                            size: rustlua_ecs::transform::Vec2::new(128.0, 128.0),
                            pivot: rustlua_ecs::transform::Vec2::new(0.5, 0.5),
                            tint: [1.0; 4],
                            sorting: rustlua_render::sprite::SpriteSorting::default(),
                            flip_x: false,
                            flip_y: false,
                            z: 0.0,
                            nine_slice: None,
                        }
                        .to_component_str(),
                    );
                }
                scene.mark_dirty();
            }
            "Tilemap" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "Tilemap".to_owned(),
                        rustlua_ecs::tilemap::Tilemap::new(
                            rustlua_ecs::transform::Vec2::new(32.0, 32.0),
                            8,
                            8,
                        )
                        .map_err(|error| InspectorError::InvalidAssetReference(error.to_string()))?
                        .to_component_str(),
                    );
                    scene.mark_dirty();
                }
            }
            "Animation" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "Animation".to_owned(),
                        rustlua_animation::animation::AnimationStateMachine::new(
                            "idle",
                            vec![rustlua_animation::animation::AnimationClip {
                                name: "idle".to_owned(),
                                sheet: "assets/textures/player.png".to_owned(),
                                cols: 1,
                                frame_rate: 1.0,
                                looped: true,
                                events: Vec::new(),
                            }],
                        )
                        .map_err(|error| InspectorError::InvalidAssetReference(error.to_string()))?
                        .to_component_str(),
                    );
                    scene.mark_dirty();
                }
            }
            // P3-E01-2：默认指向工程内的 `.anim` 约定目录；用户在通用字段编辑器里改成
            // 实际片段。运行时会读文件并在缺失/坏格式时点名报错，不会静默空转。
            "AnimationPlayer" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "AnimationPlayer".to_owned(),
                        rustlua_animation::animation_player::AnimationPlayer::from_component_str(
                            "clip=assets/anim/hero.anim",
                        )
                        .map_err(|error| InspectorError::InvalidAssetReference(error.to_string()))?
                        .to_component_str(),
                    );
                    scene.mark_dirty();
                }
            }
            // 唯一的 `Transform`：默认值取**引擎类型**的编码（单位变换），
            // 编辑由通用字段编辑器接管。
            "Transform" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "Transform".to_owned(),
                        rustlua_ecs::transform::Transform3D::identity().to_component_str(),
                    );
                }
                scene.mark_dirty();
            }
            // 唯一的 `Camera`：默认值是**透视**相机（`Camera3D::default()` = 位置 (0,0,5)
            // 看向原点、FOV 90°、near 0.1、far 100）+ 显式判别字段；要正交就把
            // `projection` 改成 orthographic，字段编辑器会接着给正交那套字段。
            "Camera" => {
                let camera = rustlua_camera::camera::Camera::Perspective(
                    rustlua_camera::camera::Camera3D::default(),
                );
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity
                        .components
                        .insert("Camera".to_owned(), camera.to_component_str());
                }
                scene.mark_dirty();
            }
            // Light 没有手写面板了，但"添加组件"仍要能加：默认值直接取**引擎类型**
            // 的编码（不在这里手写字面串），之后由通用字段编辑器接管编辑。
            "Light" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "Light".to_owned(),
                        rustlua_render::light::Light::default().to_component_str(),
                    );
                }
                scene.mark_dirty();
            }
            "Collider3d" => {
                inspector.set_collider3d(scene, selected, Collider3dFields::default())?
            }
            // MeshRenderer 同样没有手写面板了：默认值取引擎类型 + 内置哨兵的编码。
            "MeshRenderer" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "MeshRenderer".to_owned(),
                        rustlua_render::mesh3d::MeshRenderer {
                            mesh: rustlua_render::mesh3d::GROUND_PLANE_SENTINEL.to_owned(),
                            material: None,
                        }
                        .to_component_str(),
                    );
                }
                scene.mark_dirty();
            }
            "Text" => {
                let text = rustlua_ui::ui::UiText::default();
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity
                        .components
                        .insert("Text".to_owned(), text.to_component_str());
                }
                scene.mark_dirty();
            }
            // WorldText 没有手写面板了：默认值取引擎类型的编码
            // （`text=文字;font_size=…;offset=0,-20;centered=true`）。
            "WorldText" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "WorldText".to_owned(),
                        rustlua_ui::world_text::WorldText {
                            text: rustlua_ui::ui::UiText {
                                text: "文字".to_owned(),
                                font_size: rustlua_ui::ui::UI_TEXT_BASE_PX,
                                color: [1.0, 1.0, 1.0],
                                font: String::new(),
                                ..Default::default()
                            },
                            offset: [0.0, -20.0],
                            centered: true,
                        }
                        .to_component_str(),
                    );
                }
                scene.mark_dirty();
            }
            // Image 同样没有手写面板了：默认值取引擎类型的编码（空纹理 + 白 + 无九宫格）。
            "Image" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "Image".to_owned(),
                        rustlua_ui::ui::UiImage {
                            texture: String::new(),
                            color: [1.0, 1.0, 1.0, 1.0],
                            nine_slice: None,
                        }
                        .to_component_str(),
                    );
                }
                scene.mark_dirty();
            }
            // E16：UiLayout 默认 = 引擎的 `default_element()`（中心锚 200×100 元素），
            // 由布局统一承担所有 UI 位置与尺寸；编辑器不再自己拼预设 + 手写 offsets。
            "UiLayout" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "UiLayout".to_owned(),
                        rustlua_ui::ui_layout::UiRectLayout::default_element().to_component_str(),
                    );
                    scene.mark_dirty();
                }
            }
            // E17：Container 默认 VBox 无内边距。
            "Container" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "Container".to_owned(),
                        rustlua_ui::ui_layout::ContainerStyle::default().to_component_str(),
                    );
                    scene.mark_dirty();
                }
            }
            // E05-4：子项约束默认全零 + shrink=1（引擎 `LayoutItem::default()`）。
            "LayoutItem" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "LayoutItem".to_owned(),
                        rustlua_ui::ui_layout::LayoutItem::default().to_component_str(),
                    );
                    scene.mark_dirty();
                }
            }
            "UiEvents" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "UiEvents".to_owned(),
                        rustlua_ui::ui_events::UiEvents::click("on_click").to_component_str(),
                    );
                    scene.mark_dirty();
                }
            }
            "UiBindings" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "UiBindings".to_owned(),
                        "property=text.text;target=self;source=data.value".to_owned(),
                    );
                    scene.mark_dirty();
                }
            }
            "ButtonStates" => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        "ButtonStates".to_owned(),
                        "normal=assets/textures/player.png".to_owned(),
                    );
                    scene.mark_dirty();
                }
            }
            // 标记组件没有字段：载荷就是空串，存在即开启。
            rustlua_scene::scene::PERSISTENT_COMPONENT => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(
                        rustlua_scene::scene::PERSISTENT_COMPONENT.to_owned(),
                        String::new(),
                    );
                    scene.mark_dirty();
                }
            }
            other => {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.insert(other.to_owned(), String::new());
                    scene.mark_dirty();
                }
            }
        }
        Ok(())
    }

    /// 复制选中实体的组件到编辑器剪贴板（菜单「编辑 ▸ 复制组件」）。
    pub(crate) fn copy_selected_components(&mut self) {
        let Some(id) = self.selection.primary() else {
            return;
        };
        let Some(entity) = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&id))
        else {
            return;
        };
        self.inspector_clipboard = Some(ComponentClipboard {
            components: entity.components.clone(),
            resources: entity.resources.clone(),
            metadata: entity.metadata.clone(),
        });
        self.status = "组件已复制".to_owned();
    }
}
