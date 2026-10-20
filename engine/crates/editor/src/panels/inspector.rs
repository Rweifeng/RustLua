//! 行3 属性面板（Inspector）：实体层级/命名、组件管理、Transform/Camera/Light/Collider 编辑。

use crate::ComponentClipboard;
use crate::EditorApp;
use crate::panels::tilemap_tools;
use crate::util::strip_verbatim_prefix;
use editor::{Collider3dFields, InspectorError, InspectorState};
use eframe::egui;
use rustlua_scene::prefab::Prefab;
use rustlua_scene::scene::{SceneEntity, SceneEntityId};
use rustlua_ui::ui_animation::{UiEasing, UiKeyframe};
use std::{collections::BTreeSet, fs, path::Path, time::SystemTime};

struct ComponentDescriptor {
    name: &'static str,
    /// E13：Add Component 菜单按类别分组显示。
    category: ComponentCategory,
}

/// E13：组件类别（Add Component 分组 + 未来按类别过滤的扩展点）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ComponentCategory {
    /// 变换与相机：2D 与 3D 共用同一套组件（第 57 轮统一后的唯一入口）。
    Spatial,
    Gameplay,
    Rendering,
    ThreeD,
    Ui,
}

impl ComponentCategory {
    /// 分组顺序（菜单与 Add Component 共用一处，避免两处顺序漂移）。
    const ALL: &'static [Self] = &[
        Self::Spatial,
        Self::Gameplay,
        Self::Rendering,
        Self::ThreeD,
        Self::Ui,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Spatial => "变换与相机",
            Self::Gameplay => "玩法",
            Self::Rendering => "渲染",
            Self::ThreeD => "3D",
            Self::Ui => "UI",
        }
    }
}

/// E13：注册表驱动 Add Component——新组件只需在此登记名称与类别，
/// 默认值在 `add_component` 的 match 中补充（类型化 setter 或字面字符串）。
///
/// `菜单栏的「组件」菜单直接由这张表生成`（见 `crate::menu`），因此新增组件既不用改
/// 巨型 match，也不用改菜单。
const COMPONENT_REGISTRY: &[ComponentDescriptor] = &[
    // 第 57 轮：**唯一的**变换与相机（旧名 `Transform3D`/`Camera2D`/`Camera3D` 已从引擎删除，
    // 因此它们既不在本表、也不再能被加载）。默认值见 `add_component` 的对应分支：
    // `Transform` = 引擎的单位变换，`Camera` = 透视相机 + 判别字段。
    ComponentDescriptor {
        name: "Transform",
        category: ComponentCategory::Spatial,
    },
    ComponentDescriptor {
        name: "Camera",
        category: ComponentCategory::Spatial,
    },
    ComponentDescriptor {
        name: "Sprite",
        category: ComponentCategory::Rendering,
    },
    ComponentDescriptor {
        name: "Tilemap",
        category: ComponentCategory::Gameplay,
    },
    // 运行时切场景不销毁：无载荷标记组件，挂在父节点上会级联到整棵子树。
    // 由运行时 `Scene::is_entity_persistent_with_ancestors` 解释；Inspector 不再有复选框。
    ComponentDescriptor {
        name: rustlua_scene::scene::PERSISTENT_COMPONENT,
        category: ComponentCategory::Gameplay,
    },
    ComponentDescriptor {
        name: "Animation",
        category: ComponentCategory::Gameplay,
    },
    ComponentDescriptor {
        name: "MeshRenderer",
        category: ComponentCategory::ThreeD,
    },
    ComponentDescriptor {
        name: "Light",
        category: ComponentCategory::ThreeD,
    },
    ComponentDescriptor {
        name: "Collider3d",
        category: ComponentCategory::ThreeD,
    },
    ComponentDescriptor {
        name: "Text",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "WorldText",
        category: ComponentCategory::Rendering,
    },
    ComponentDescriptor {
        name: "Image",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "UiLayout",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "Container",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "UiEvents",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "UiBindings",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "ButtonStates",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "UiAnimation",
        category: ComponentCategory::Ui,
    },
];

/// 组件菜单的分组视图：`(类别标签, 类别下的组件名)`。
///
/// 菜单栏（`crate::menu` 的「组件」菜单）用它生成子菜单——**注册表是唯一来源**，
/// 因此新增组件只改注册表一处，菜单与 Inspector 的 Add Component 同时生效。
#[must_use]
pub(crate) fn component_menu_groups() -> Vec<(&'static str, Vec<&'static str>)> {
    ComponentCategory::ALL
        .iter()
        .copied()
        .map(|category| {
            let names = COMPONENT_REGISTRY
                .iter()
                .filter(|descriptor| descriptor.category == category)
                .map(|descriptor| descriptor.name)
                .collect();
            (category.label(), names)
        })
        .filter(|(_, names): &(&str, Vec<&str>)| !names.is_empty())
        .collect()
}

/// Inspector Override 摘要用的源 Prefab 缓存条目。
pub(crate) struct CachedPrefabSource {
    /// (文件长度, mtime)。`None` = 读取元数据失败（例如文件不存在）。
    pub(crate) stamp: Option<(u64, Option<SystemTime>)>,
    pub(crate) prefab: Prefab,
}

/// 源 Prefab 的失效戳：长度 + mtime。任一变化即视为需要重载。
fn prefab_stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()))
}

impl EditorApp {
    pub(crate) fn draw_inspector_panel(&mut self, ui: &mut egui::Ui) {
        // E08：Inspector 缓冲（数值拖动/文本键入）的**直写**由控件编辑帧触发
        // （`mark_inspector_edit`：相对基线有真实差异才写，仅驱动 Scene View 刷新，
        // 不提交不落盘）；收敛成一条历史/保存则交给帧末提交边界（本面板内释放、
        // 顶层 release/Enter/Escape 兜底）。面板绘制前不 mark：控件尚未改动缓冲，
        // 净差异必然为空。
        ui.set_min_width(340.0);
        // 内容超出面板高度时可滚动。
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.heading("属性");
// 资源面板选中资源时：属性面板显示资源详情，不显示实体属性。
                if let Some(resource_path) = &self.selected_resource {
                    ui.heading("资源");
                    if let Some(entry) = self.resources.entries().iter().find(|e| &e.path == resource_path).cloned() {
                        let file_name = entry.path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| entry.path_string());
                        ui.strong(&file_name);
                        ui.label(egui::RichText::new(entry.path_string()).monospace().small());
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label("类型：");
                            ui.label(egui::RichText::new(format!("{:?}", entry.kind)).monospace());
                        });
                        let (status_text, status_color) = match entry.status {
                            rustlua_assets::resource_db::ResourceState::Ready => ("已就绪".to_owned(), egui::Color32::from_rgb(120,200,130)),
                            rustlua_assets::resource_db::ResourceState::Discovered => ("待导入".to_owned(), egui::Color32::YELLOW),
                            rustlua_assets::resource_db::ResourceState::Stale => ("已变更需重导".to_owned(), egui::Color32::YELLOW),
                            rustlua_assets::resource_db::ResourceState::Missing => ("文件缺失".to_owned(), egui::Color32::GRAY),
                            rustlua_assets::resource_db::ResourceState::ImportFailed => ("导入失败".to_owned(), egui::Color32::RED),
                            rustlua_assets::resource_db::ResourceState::Invalid => ("依赖断链".to_owned(), egui::Color32::RED),
                            _ => (format!("{:?}", entry.status), ui.visuals().weak_text_color()),
                        };
                        ui.horizontal(|ui| {
                            ui.label("状态：");
                            ui.colored_label(status_color, status_text);
                        });
                        if let Some(error) = &entry.error {
                            ui.colored_label(egui::Color32::RED, egui::RichText::new(error).small());
                        }
                        // 文件大小
                        let abs_path = self.project.root.join(&entry.path);
                        if let Ok(meta) = std::fs::metadata(&abs_path) {
                            let kb = meta.len() as f64 / 1024.0;
                            ui.label(format!("大小：{kb:.1} KB"));
                        }
                        // 纹理：显示大图预览
                        if entry.kind == rustlua_assets::resource_kind::ResourceKind::Texture {
                            ui.separator();
                            let rel = entry.path_string();
                            if let Some(tex_id) = self.texture_thumbnail(&rel, ui.ctx()) {
                                let size = egui::vec2(128.0, 128.0);
                                ui.add(egui::Image::new(egui::load::SizedTexture::new(tex_id, size))
                                    .fit_to_original_size(1.0));
                            } else {
                                ui.label("缩略图加载中…");
                            }
                        }
                        // 场景/Prefab：提示双击打开
                        if matches!(entry.kind, rustlua_assets::resource_kind::ResourceKind::Scene | rustlua_assets::resource_kind::ResourceKind::Prefab) {
                            ui.separator();
                            ui.label(egui::RichText::new("双击资源面板中的条目可打开编辑").small().weak());
                        }
                    } else {
                        ui.label("资源信息不可用");
                    }
                    return;
                }
                if self
                    .current_scene()
                    .is_some_and(|scene| scene.is_prefab_document())
                {
                    ui.label("当前文档：Prefab 编辑模式");
                }
                let Some(selected) = self.selection.primary() else {
                    ui.label("请先在层级中选择实体");
                    return;
                };
                let selected_ids = self.selection.ids();
                if selected_ids.len() > 1 {
                    let mut common_components: Option<BTreeSet<String>> = None;
                    if let Some(scene) = self.current_scene() {
                        for id in &selected_ids {
                            let components = scene
                                .scene
                                .entities
                                .get(id)
                                .map(|entity| entity.components.keys().cloned().collect())
                                .unwrap_or_default();
                            common_components = Some(match common_components {
                                Some(common) => common.intersection(&components).cloned().collect(),
                                None => components,
                            });
                        }
                    }
                    let common_components = common_components.unwrap_or_default();
                    ui.strong(format!("已选择 {} 个实体", selected_ids.len()));
                    ui.label(if common_components.is_empty() {
                        "没有共有组件".to_owned()
                    } else {
                        format!(
                            "共有组件：{}",
                            common_components.into_iter().collect::<Vec<_>>().join("、")
                        )
                    });
                }
                let Some((parent, child_count)) = self.current_scene().and_then(|scene| {
                    scene
                        .scene
                        .entities
                        .get(&selected)
                        .map(|e| (e.parent, e.children.len()))
                }) else {
                    ui.label("实体不存在");
                    return;
                };
                ui.label(format!("实体 ID：{}", selected.0));
                if self.current_scene().is_some_and(|scene| {
                    scene
                        .scene
                        .entities
                        .get(&selected)
                        .and_then(|entity| entity.metadata.get("ui.root"))
                        .is_some_and(|value| value == "true")
                }) {
                    ui.colored_label(egui::Color32::from_rgb(100, 190, 230), "UI Root / Canvas");
                    ui.label("Canvas");
                    ui.horizontal(|ui| {
                        ui.label("参考");
                        ui.add(
                            egui::DragValue::new(&mut self.canvas_config.reference[0])
                                .range(1.0..=7680.0),
                        );
                        ui.add(
                            egui::DragValue::new(&mut self.canvas_config.reference[1])
                                .range(1.0..=4320.0),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("缩放");
                        egui::ComboBox::from_id_salt("canvas_scale_mode")
                            .selected_text(self.canvas_config.scale_mode.as_str())
                            .show_ui(ui, |ui| {
                                for mode in [
                                    rustlua_ui::ui_layout::CanvasScaleMode::None,
                                    rustlua_ui::ui_layout::CanvasScaleMode::FitWidth,
                                    rustlua_ui::ui_layout::CanvasScaleMode::FitHeight,
                                    rustlua_ui::ui_layout::CanvasScaleMode::FitBoth,
                                ] {
                                    ui.selectable_value(
                                        &mut self.canvas_config.scale_mode,
                                        mode,
                                        mode.as_str(),
                                    );
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Safe Area");
                        for inset in &mut self.canvas_config.safe_area {
                            ui.add(egui::DragValue::new(inset).range(0.0..=512.0));
                        }
                    });
                    if ui.button("应用 Canvas 配置").clicked() {
                        self.apply_canvas_config();
                    }
                }
                ui.label(format!(
                    "父实体：{}",
                    parent.map_or_else(|| "无".to_owned(), |id| id.0.to_string())
                ));
                ui.label(format!("子实体数量：{child_count}"));
                if let Some((prefab_source, is_prefab_root)) =
                    self.current_scene().and_then(|scene| {
                        scene.scene.entities.get(&selected).and_then(|entity| {
                            entity.metadata.get("prefab.source").map(|source| {
                                (
                                    source.clone(),
                                    entity
                                        .metadata
                                        .get("prefab.root")
                                        .is_some_and(|value| value == "true"),
                                )
                            })
                        })
                    })
                {
                    ui.label(format!(
                        "Prefab 来源：{}{}",
                        prefab_source,
                        if is_prefab_root {
                            "（根实例）"
                        } else {
                            ""
                        }
                    ));
                    // docs/55 §4.1：面板只发 typed action（目标显式携带），不直接调用文档写入方法。
                    if is_prefab_root && ui.button("重新应用源 Prefab").clicked() {
                        self.dispatch(editor::EditorCommand::ReapplyPrefab { entity: selected });
                    }
                    if ui.button("打开源 Prefab").clicked() {
                        self.open_scene_resource(std::path::PathBuf::from(&prefab_source));
                    }
                    if is_prefab_root && ui.button("写回源并更新实例").clicked() {
                        self.dispatch(editor::EditorCommand::SavePrefabToSource { entity: selected });
                    }
                    if is_prefab_root && ui.button("Unpack").clicked() {
                        self.dispatch(editor::EditorCommand::UnpackPrefab { entity: selected });
                    }
                    if is_prefab_root && ui.button("Revert Overrides").clicked() {
                        self.dispatch(editor::EditorCommand::RevertPrefabOverrides {
                            entity: selected,
                        });
                    }
                }
                if let Some(prefab_state) = self.prefab_override_summary(selected) {
                    match prefab_state {
                        Ok(prefab_overrides) => {
                            if prefab_overrides.is_empty() {
                                ui.label("Prefab Override：无");
                            } else {
                                // 逐字段选择性 Apply / Revert（docs/55 阶段 3）：
                                // 每行显示「源 → 实例」，并可单独把该字段推回源 Prefab 或还原为源取值。
                                let mut revert_field: Option<String> = None;
                                let mut apply_field: Option<String> = None;
                                ui.collapsing(
                                    format!("Prefab Override（{}）", prefab_overrides.len()),
                                    |ui| {
                                        for row in &prefab_overrides {
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .small_button("应用")
                                                    .on_hover_text(format!(
                                                        "把实例取值写回源 Prefab 并更新全部实例：{}",
                                                        compact_value(&row.instance_value)
                                                    ))
                                                    .clicked()
                                                {
                                                    apply_field = Some(row.field.clone());
                                                }
                                                if ui
                                                    .small_button("回退")
                                                    .on_hover_text(format!(
                                                        "把这个字段还原为源 Prefab 的取值：{}",
                                                        row.source_value
                                                    ))
                                                    .clicked()
                                                {
                                                    revert_field = Some(row.field.clone());
                                                }
                                                ui.monospace(&row.field);
                                                ui.weak("源=");
                                                ui.monospace(&row.source_value);
                                                ui.weak("实例=");
                                                ui.monospace(compact_value(&row.instance_value));
                                            });
                                        }
                                    },
                                );
                                if let Some(field) = revert_field {
                                    self.revert_prefab_override_field(selected, &field);
                                }
                                if let Some(field) = apply_field {
                                    self.apply_prefab_override_field(selected, &field);
                                }
                            }
                        }
                        Err(error) => {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 140, 80),
                                format!("Prefab 诊断：{error}"),
                            );
                        }
                    }
                }
                // 运行时切场景不销毁改由实体组件 `Persistent` 表达（Add Component ▸ 玩法）。
                // 属性面板不再提供复选框：是否持久是**实体数据**，应当随场景/预制体一起保存，
                // 并且父节点的组件会级联到整棵子树（见 `Scene::is_entity_persistent_with_ancestors`）。
                // M19-03 PersistId：实体自由 meta（`entity_meta persist_id`），是运行期
                // `entity.spawn(name, persist_id)` 的稳定锚点。增删改都并入 CommandHistory
                // （一次提交 = 单条 Undo），保存后场景文本只增删该 entity_meta 行，
                // **不改场景格式版本**。合法性（重名/跨文件冲突）由领域数据面板的
                // PersistId 注册表与 Lua data.npcs 校验器负责，编辑器不硬编码领域规则。
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("PersistId");
                    if self.persist_id_editing != Some(selected) {
                        self.reload_persist_id_buffer(selected);
                    }
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.persist_id_buffer)
                            .desired_width(150.0)
                            .hint_text("npc_shen_qingluo"),
                    );
                    let mut commit: Option<Option<String>> = None;
                    if ui.button("设置").clicked() {
                        commit = Some(Some(self.persist_id_buffer.clone()));
                    }
                    // 失焦（Enter / 点击别处）即提交：一次提交一条 Undo，避免逐键
                    // 刷屏式历史。
                    if response.lost_focus() {
                        commit = Some(Some(self.persist_id_buffer.clone()));
                    }
                    if ui.button("清除").clicked() {
                        commit = Some(None);
                    }
                    if let Some(value) = commit {
                        match self.set_entity_persist_id(selected, value) {
                            Ok(true) => {
                                self.reload_persist_id_buffer(selected);
                                self.status = if self.persist_id_buffer.is_empty() {
                                    "PersistId 已清除".to_owned()
                                } else {
                                    format!("PersistId 已设置：{}", self.persist_id_buffer)
                                };
                            }
                            Ok(false) => {
                                self.reload_persist_id_buffer(selected);
                                self.status = "PersistId 无变化".to_owned();
                            }
                            Err(error) => self.status = error,
                        }
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("名称");
                    ui.add(egui::TextEdit::singleline(&mut self.entity_name).desired_width(150.0));
                    if ui.button("重新命名").clicked() {
                        self.rename_selected();
                    }
                });
                // M10-06：组件管理（添加/移除）。
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("添加组件");
                    egui::ComboBox::from_id_salt("inspector_add_component")
                        .selected_text(&self.inspector_add_component)
                        .show_ui(ui, |ui| {
                            // E13：按类别分组展示注册表（顺序固定，避免漂移）。
                            for category in ComponentCategory::ALL.iter().copied() {
                                ui.weak(category.label());
                                for descriptor in
                                    COMPONENT_REGISTRY.iter().filter(|d| d.category == category)
                                {
                                    ui.selectable_value(
                                        &mut self.inspector_add_component,
                                        descriptor.name.to_owned(),
                                        descriptor.name,
                                    );
                                }
                                ui.separator();
                            }
                        });
                    if ui.button("添加").clicked() {
                        let component = self.inspector_add_component.clone();
                        if let Err(error) = self.add_component(&component) {
                            self.status = error;
                        }
                    }
                });
                let components = self.current_components();
                // 只显示实体实际拥有的组件编辑器。
                let has = |name: &str| components.iter().any(|component| component == name);
                if !components.is_empty() {
                    ui.label("组件：");
                    for key in &components {
                        ui.horizontal(|ui| {
                            ui.label(key);
                            if ui.small_button("移除").clicked() {
                                let key = key.clone();
                                if let Err(error) = self.remove_component(selected, &key) {
                                    self.status = error;
                                }
                            }
                        });
                    }
                }
                ui.separator();
                // `Transform` / `Transform3D` 的手写面板**已删除**（第 36 轮）：两个组件的字段
                // 各自由 `draw_schema_driven_components` 按 schema 生成（x/y/rotation/scale = DragValue、
                // 四元数 4 个 DragValue），**等比锁**由编辑器态开关 + 字段镜像承担、
                // **重置**由"重置为默认值"动作承担（值来自引擎 `identity()`）。
                //
                // 迁移后刻意不再有"2D/3D 空间切换"（WP9）：两个组件**同时**在字段区里显示，
                // 不存在"被切走的那一种看不见"的问题，也就不用再存一个编辑器态；混合值仍由
                // 通用编辑器逐字段标注（`SchemaFieldRow.mixed`）。
                // `Copy/Paste Components` 按钮随面板一并删除——它们在**编辑菜单**里仍然可用
                // （typed 命令 `CopyComponents`/`PasteComponents`）。
                // `MeshRenderer` 的手写面板**已删除**（第 21 轮）：与 `Light` 同样由
                // `draw_schema_driven_components` 按 schema 生成（`mesh` 字段，带
                // "工程内相对路径 / .glb|.gltf / 内置哨兵" 规则提示）。
                // `Camera3D` 的手写面板**已删除**（第 24 轮）：字段由
                // `draw_schema_driven_components` 按 schema 生成（position/target = DragValue×3、
                // fov = 以角度为单位的 DragValue、near/far、pose = 下拉、enabled = 勾选框）；
                // 只读摘要（统一 `Projection` 的投影说明与 `pose=transform` 的生效姿态）由
                // `schema_fields::component_notes` 在字段行下方呈现。
                // `Light` 的手写面板**已删除**（docs/55 阶段 2）：该组件在
                // `schema_fields::HANDWRITTEN_PANEL_COMPONENTS` 之外，因此由
                // `draw_schema_driven_components` 按 schema 生成字段行
                // （方向 = DragValue×3、颜色 = 取色器、强度 = DragValue），
                // 提交走 `EditorCommand::SetComponentField`。
                if has("Collider3d") {
                    let collider3d_header = if self.inspector_collider3d_open {
                        "Collider3d  v"
                    } else {
                        "Collider3d  >"
                    };
                    if ui.button(collider3d_header).clicked() {
                        self.inspector_collider3d_open = !self.inspector_collider3d_open;
                    }
                    if self.inspector_collider3d_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("形状");
                                egui::ComboBox::from_id_salt("collider3d_shape")
                                    .selected_text(&self.collider3d.shape)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.collider3d.shape,
                                            "ball".to_owned(),
                                            "Ball",
                                        );
                                        ui.selectable_value(
                                            &mut self.collider3d.shape,
                                            "cuboid".to_owned(),
                                            "Cuboid",
                                        );
                                        ui.selectable_value(
                                            &mut self.collider3d.shape,
                                            "capsule".to_owned(),
                                            "Capsule",
                                        );
                                    });
                                ui.checkbox(&mut self.collider3d.sensor, "sensor");
                                ui.checkbox(&mut self.collider3d.dynamic, "dynamic");
                            });
                            match self.collider3d.shape.as_str() {
                                "cuboid" => {
                                    ui.horizontal(|ui| {
                                        ui.label("半宽/半高/半深");
                                        ui.add(egui::DragValue::new(
                                            &mut self.collider3d.half_width,
                                        ));
                                        ui.add(egui::DragValue::new(
                                            &mut self.collider3d.half_height,
                                        ));
                                        ui.add(egui::DragValue::new(&mut self.collider3d.half_z));
                                    });
                                }
                                "capsule" => {
                                    ui.horizontal(|ui| {
                                        ui.label("半高/半径");
                                        ui.add(egui::DragValue::new(
                                            &mut self.collider3d.half_height,
                                        ));
                                        ui.add(egui::DragValue::new(&mut self.collider3d.radius));
                                    });
                                }
                                _ => {
                                    ui.horizontal(|ui| {
                                        ui.label("半径");
                                        ui.add(egui::DragValue::new(&mut self.collider3d.radius));
                                    });
                                }
                            }
                            ui.horizontal(|ui| {
                                ui.label("偏移");
                                ui.add(egui::DragValue::new(&mut self.collider3d.offset_x));
                                ui.add(egui::DragValue::new(&mut self.collider3d.offset_y));
                                ui.add(egui::DragValue::new(&mut self.collider3d.offset_z));
                            });
                            ui.horizontal(|ui| {
                                if ui.button("添加 Collider3d").clicked()
                                    && let Err(error) = self.add_collider3d_component(selected)
                                {
                                    self.status = error;
                                }
                                if ui.button("移除 Collider3d").clicked()
                                    && let Err(error) = self.remove_collider3d_component(selected)
                                {
                                    self.status = error;
                                }
                            });
                        });
                    }
                }
                // UI 组件：Text / Image / Button（屏幕空间）。
                // `Text` 的手写面板**已删除**（第 25 轮）：文本=输入框、字号=DragValue、
                // **字体=项目字体下拉**（`ChoiceSource::ProjectFonts`）、颜色=取色器，
                // 全部由 `draw_schema_driven_components` 按 schema 生成；文本里的 `;`/`=`
                // 由 schema 的转义字段（`FieldSchema::escaped`）负责，串仍与引擎编码器一致。
                // `WorldText` 的手写面板**已删除**（第 23 轮）：文本/字号/颜色/偏移/居中由
                // schema 生成（颜色=取色器、偏移=两个 DragValue、居中=勾选框），
                // **字体字段用项目字体下拉**（`ChoiceSource::ProjectFonts`）。
                // `Image` 的手写面板**已删除**（第 22 轮）：纹理 = 文本框、颜色 = **取色器**、
                // 九宫格四边 = 4 个 DragValue（非负规则由 schema 表达式），全部由
                // `draw_schema_driven_components` 生成。
                // `Sprite` 的手写面板**已删除**（第 34 轮）：`texture` = 文本框（带图片扩展名规则）、
                // `size` = `宽x高`、`pivot` = 两个 DragValue、`tint` = **取色器**、
                // **排序层 = 引擎已知层名下拉**（`ChoiceSource::SortingLayers`）、`order` = DragValue、
                // `flip_x/flip_y` = 勾选框，全部由 `draw_schema_driven_components` 生成；
                // 多选批量与"混合"显示由通用编辑器的批量命令承担（第 33 轮）。
                if has("Tilemap") {
                    let tilemap_header = if self.inspector_tilemap_open {
                        "Tilemap  v"
                    } else {
                        "Tilemap  >"
                    };
                    if ui.button(tilemap_header).clicked() {
                        self.inspector_tilemap_open = !self.inspector_tilemap_open;
                    }
                    if self.inspector_tilemap_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("纹理");
                                ui.text_edit_singleline(&mut self.tilemap.map.texture);
                            });
                            ui.horizontal(|ui| {
                                ui.label("Tile 尺寸");
                                ui.add(egui::DragValue::new(&mut self.tilemap.map.tile_size.x));
                                ui.add(egui::DragValue::new(&mut self.tilemap.map.tile_size.y));
                            });
                            ui.horizontal(|ui| {
                                ui.label("网格");
                                ui.add(
                                    egui::DragValue::new(&mut self.tilemap.map.width)
                                        .range(1..=256),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.tilemap.map.height)
                                        .range(1..=256),
                                );
                            });
                            let target_len = self.tilemap.map.width * self.tilemap.map.height;
                            if self.tilemap.map.tiles.len() != target_len {
                                self.tilemap.map.tiles.resize(target_len, 0);
                            }
                            if self.tilemap.map.solid.len() != target_len {
                                self.tilemap.map.solid.resize(target_len, false);
                            }
                            // M72-02：per-tile tint 与 tiles 同长（0 = 不染色）。
                            // 漏了这一句，改网格尺寸会让序列化端的长度校验直接报错。
                            if self.tilemap.map.tint_ids.len() != target_len {
                                self.tilemap.map.tint_ids.resize(target_len, 0);
                            }
                            // M72-01：图集布局。列/行数**显式**填写，不从贴图尺寸反推——
                            // 反推会让"图里有几列"变成美术文件的隐式契约，重排一次贴图
                            // 就整张地图静默错位（宁可编辑器里多敲两个数字）。
                            ui.horizontal(|ui| {
                                ui.label("图集");
                                ui.add(
                                    egui::DragValue::new(&mut self.tilemap.map.atlas_cols)
                                        .range(1..=64)
                                        .suffix(" 列"),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.tilemap.map.atlas_rows)
                                        .range(1..=64)
                                        .suffix(" 行"),
                                );
                            });
                            // M72-02：逻辑瓦模式。关掉时格子存图集条目号（旧图的行为），
                            // 打开后存 kind id，由邻居掩码 + 确定性哈希现算条目。
                            ui.horizontal(|ui| {
                                ui.label("逻辑瓦");
                                ui.add(
                                    egui::DragValue::new(&mut self.tilemap.map.tile_kinds)
                                        .range(1..=64)
                                        .suffix(" 种"),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.tilemap.map.variant_count)
                                        .range(1..=16)
                                        .suffix(" 变体"),
                                );
                            });
                            let mut layout_error: Option<String> = None;
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut self.tilemap.map.autotile, "自动瓦片(4-bit)");
                                if ui
                                    .button(
                                        "重排变体种子",
                                    )
                                    .clicked()
                                {
                                    // 种子决定同一 kind 下选哪个变体；换种子 = 换一批
                                    // 摆放，但**同一种子永远得到同一张图**（见 tilemap.rs）。
                                    self.tilemap.map.variant_seed = self
                                        .tilemap
                                        .map
                                        .variant_seed
                                        .wrapping_mul(1_664_525)
                                        .wrapping_add(1_013_904_223);
                                }
                                ui.label(format!("seed={}", self.tilemap.map.variant_seed));
                            });
                            if let Err(error) = self.tilemap.map.validate_layout() {
                                layout_error = Some(error.to_string());
                            }
                            if let Some(error) = &layout_error {
                                ui.label(
                                    egui::RichText::new(format!("布局不合法：{error}"))
                                        .color(egui::Color32::from_rgb(240, 130, 90)),
                                );
                            }
                            // RustLua-②a + M72-01/02：Tile Palette——槽位数随布局走，
                            // 不再是写死的 6 色。逻辑瓦模式下槽位是 kind（见 brush_label），
                            // 让美术刷的是"草地"而不是"图集第 3 格"，autotile 才不会被绕过。
                            let brush_max = tilemap_tools::max_brush(&self.tilemap.map);
                            if self.tilemap.brush > brush_max {
                                self.tilemap.brush = brush_max;
                            }
                            ui.horizontal_wrapped(|ui| {
                                ui.label("调色板");
                                for tile in 0..=brush_max {
                                    let [r, g, b, _] = tilemap_tools::debug_swatch(tile);
                                    let color = if tile == 0 {
                                        egui::Color32::from_gray(42)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(
                                            (r * 255.0) as u8,
                                            (g * 255.0) as u8,
                                            (b * 255.0) as u8,
                                            255,
                                        )
                                    };
                                    let selected = self.tilemap.brush == tile;
                                    let swatch = egui::Button::new(
                                        egui::RichText::new(
                                            tilemap_tools::brush_label(&self.tilemap.map, tile),
                                        )
                                        .size(10.0)
                                        .color(egui::Color32::from_gray(220)),
                                    )
                                    .fill(color)
                                    .min_size(egui::vec2(30.0, 22.0))
                                    .stroke(if selected {
                                        egui::Stroke::new(2.0_f32, egui::Color32::WHITE)
                                    } else {
                                        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(90))
                                    });
                                    if ui.add(swatch).clicked() {
                                        self.tilemap.brush = tile;
                                    }
                                }
                                ui.add(
                                    egui::DragValue::new(&mut self.tilemap.brush)
                                        .range(0..=brush_max),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut self.tilemap.paint_collision, "画碰撞层");
                                if ui.button("预览碰撞层").clicked() {
                                    self.tilemap.preview_collision = !self.tilemap.preview_collision;
                                }
                                if self.tilemap.preview_collision {
                                    ui.label("（碰撞层半透明覆盖）");
                                }
                            });
                            ui.label("点击下方网格可直接编辑视觉层或碰撞层。");
                            let preview_width = self.tilemap.map.width.min(24);
                            let preview_height = self.tilemap.map.height.min(24);
                            egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
                                egui::Grid::new("tilemap_grid")
                                    .spacing(egui::vec2(2.0, 2.0))
                                    .show(ui, |ui| {
                                        for y in 0..preview_height {
                                            for x in 0..preview_width {
                                                let index = y * self.tilemap.map.width + x;
                                                let value = self.tilemap.map.tiles[index];
                                                let solid = self.tilemap.map.solid[index];
                                                // 悬停说明"存的是什么 → 画的是图集第几格"：
                                                // autotile/变体下这两个数不同，不写清楚美术
                                                // 没法判断边界瓦摆得对不对（M72-02 §设计要点 1）。
                                                let resolved =
                                                    self.tilemap.map.resolve_atlas_index(x, y);
                                                let label = if self.tilemap.paint_collision {
                                                    if solid { "■".to_owned() } else { "·".to_owned() }
                                                } else if value == 0 {
                                                    "·".to_owned()
                                                } else {
                                                    value.to_string()
                                                };
                                                let cell = ui.add_sized(
                                                    [18.0, 18.0],
                                                    egui::Button::new(label),
                                                );
                                                // `on_hover_text` 吃掉 Response，先拿到
                                                // 点击用的那一份。
                                                let cell = if value != 0 {
                                                    cell.on_hover_text(format!(
                                                        "格 ({x},{y})：存 {value} → 图集条目 {resolved}"
                                                    ))
                                                } else {
                                                    cell
                                                };
                                                if cell.clicked() {
                                                    if self.tilemap.paint_collision {
                                                        self.tilemap.map.solid[index] =
                                                            !self.tilemap.map.solid[index];
                                                    } else {
                                                        self.tilemap.map.tiles[index] =
                                                            tilemap_tools::clamp_brush(
                                                                &self.tilemap.map,
                                                                self.tilemap.brush,
                                                            );
                                                    }
                                                    // E08：连续点击/按住连画只在帧内直写文档，
                                                    // 松手由帧尾提交边界收敛为一条 Undo。
                                                    self.mark_inspector_edit();
                                                }
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                            if ui.button("移除 Tilemap").clicked()
                                && let Err(error) = self.remove_component(selected, "Tilemap")
                            {
                                self.status = error;
                            }
                        });
                    }
                }
                if has("Animation") {
                    let animation_header = if self.inspector_animation_open {
                        "Animation  v"
                    } else {
                        "Animation  >"
                    };
                    if ui.button(animation_header).clicked() {
                        self.inspector_animation_open = !self.inspector_animation_open;
                    }
                    if self.inspector_animation_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("默认状态");
                                ui.text_edit_singleline(&mut self.animation.machine.default);
                            });
                            let mut remove_index = None;
                            for (index, clip) in self.animation.machine.clips.iter_mut().enumerate()
                            {
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.label(format!("Clip {}", index + 1));
                                    if ui.small_button("删除").clicked() {
                                        remove_index = Some(index);
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("名称");
                                    ui.text_edit_singleline(&mut clip.name);
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Sheet");
                                    ui.text_edit_singleline(&mut clip.sheet);
                                });
                                ui.horizontal(|ui| {
                                    ui.label("列数/FPS");
                                    ui.add(egui::DragValue::new(&mut clip.cols).range(1..=128));
                                    ui.add(
                                        egui::DragValue::new(&mut clip.frame_rate)
                                            .speed(0.1)
                                            .range(0.1..=120.0),
                                    );
                                    ui.checkbox(&mut clip.looped, "循环");
                                });
                            }
                            if let Some(index) = remove_index {
                                self.animation.machine.clips.remove(index);
                                if self.animation.machine.clips.is_empty() {
                                    self.animation.machine.clips.push(
                                        rustlua_animation::animation::AnimationClip {
                                            name: "idle".to_owned(),
                                            sheet: self.selected_sprite_texture(),
                                            cols: 1,
                                            frame_rate: 1.0,
                                            looped: true,
                                            events: Vec::new(),
                                        },
                                    );
                                }
                                if !self
                                    .animation
                                    .machine
                                    .clips
                                    .iter()
                                    .any(|clip| clip.name == self.animation.machine.default)
                                {
                                    self.animation.machine.default =
                                        self.animation.machine.clips[0].name.clone();
                                }
                            }
                            if ui.button("添加 Clip").clicked() {
                                self.animation.machine.clips.push(
                                    rustlua_animation::animation::AnimationClip {
                                        name: format!(
                                            "clip{}",
                                            self.animation.machine.clips.len() + 1
                                        ),
                                        sheet: self.selected_sprite_texture(),
                                        cols: 1,
                                        frame_rate: 8.0,
                                        looped: true,
                                        events: Vec::new(),
                                    },
                                );
                            }
                            ui.label("动画数据通过 Inspector 编辑，运行时仍由 Lua 控制切换。");
                            if ui.button("移除 Animation").clicked()
                                && let Err(error) = self.remove_component(selected, "Animation")
                            {
                                self.status = error;
                            }
                        });
                    }
                }
                // `UiLayout` 的手写面板**已删除**（第 31 轮）：`anchors`/`offsets`/`min_size`/
                // `max_size`/`pivot` 由 schema 生成（向量 = DragValue 连排），
                // **锚点预设**由组件级动作下拉接管（`SchemaComponentAction`，12 个预设直接来自
                // 引擎 `UiLayoutPreset::ALL`，一次改 `anchors`+`pivot` = 一条 Undo）；
                // anchors/pivot 的 `0..=1` 约束由**引擎解析器兜底门**保证
                // （`UiRectLayout::from_component_str` 里就是 `validate_anchors`/`validate_unit`）。
                // E17：Container（自动布局容器）编辑组。
                if has("Container") {
                    ui.horizontal(|ui| {
                        ui.label("Container");
                        egui::ComboBox::from_id_salt("container_kind")
                            .selected_text(&self.container.kind)
                            .show_ui(ui, |ui| {
                                for (token, label) in [
                                    ("vbox", "垂直排列 VBox"),
                                    ("hbox", "水平排列 HBox"),
                                    ("grid", "网格 Grid"),
                                    ("margin", "边距 Margin"),
                                    ("center", "居中 Center"),
                                ] {
                                    ui.selectable_value(
                                        &mut self.container.kind,
                                        token.to_owned(),
                                        label,
                                    );
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("内边距");
                        for padding in [
                            &mut self.container.padding_l,
                            &mut self.container.padding_t,
                            &mut self.container.padding_r,
                            &mut self.container.padding_b,
                        ] {
                            ui.add(egui::DragValue::new(padding));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("间距");
                        ui.add(egui::DragValue::new(&mut self.container.spacing));
                        ui.label("列数");
                        ui.add(egui::DragValue::new(&mut self.container.columns));
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.container.stretch, "拉伸主轴");
                        ui.checkbox(&mut self.container.fill, "填满交叉轴");
                    });
                    ui.horizontal(|ui| {
                        ui.label("对齐");
                        egui::ComboBox::from_id_salt("container_align")
                            .selected_text(&self.container.align)
                            .show_ui(ui, |ui| {
                                for (token, label) in
                                    [("start", "起始"), ("center", "居中"), ("end", "末尾")]
                                {
                                    ui.selectable_value(
                                        &mut self.container.align,
                                        token.to_owned(),
                                        label,
                                    );
                                }
                            });
                    });
                    if ui.button("移除 Container").clicked()
                        && let Err(error) = self.remove_component(selected, "Container")
                    {
                        self.status = error;
                    }
                }
                if has("UiEvents") {
                    self.draw_ui_events_editor(ui);
                }
                if has("UiBindings") {
                    self.draw_ui_bindings_editor(ui);
                }
                if has("ButtonStates") {
                    self.draw_button_states_editor(ui);
                }
                if has("UiAnimation") {
                    self.draw_ui_animation_editor(ui);
                }
                if has("Script") {
                    ui.horizontal(|ui| {
                        ui.label("Lua");
                        ui.text_edit_singleline(&mut self.lua_script);
                    });
                }
                // docs/55 阶段 2：有 schema、没有手写面板的组件由通用字段编辑器接管
                // （Camera2D/Quad/RenderMode/Visibility/EntityScript/Particle），
                // 提交通通走 typed action，不在面板里直接写组件串。
                self.draw_schema_driven_components(ui);
            });
        // E08 帧末边界：缓冲差异已在控件编辑帧随 `mark_inspector_edit` 直写文档，
        // 这里只做提交决策。
        // - 本帧 primary 指针在 Inspector 面板区域内释放 → 一次拖动/交互结束 →
        //   收敛为一条历史（无真实差异时 no-op）；
        // - 否则（拖动/键入进行中）保持直写（mark）但不提交，防止一次拖动被
        //   逐帧拆成多条历史。
        // 释放发生在面板外（拖出面板、点击 Scene/其它窗口）或文本 Enter 提交的
        // 兜底由顶层 `flush_frame_edit_boundary` 统一收敛；本面板先于中央 Scene 面板
        // 绘制，该顺序保证 Inspector 提交不会打断 Scene View 自身的拖拽收尾。
        if !self.is_playing() {
            let released_in_panel = ui.input(|input| input.pointer.primary_released())
                && ui
                    .input(|input| input.pointer.latest_pos())
                    .is_some_and(|pos| ui.min_rect().contains(pos));
            if released_in_panel {
                self.finish_inspector_edits_if_dirty();
            } else {
                self.mark_inspector_edit();
            }
        }
    }

    fn draw_ui_events_editor(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("UI Events");
        let mut changed = false;
        for index in 0..self.events_editor.bindings.len() {
            let mut binding = self.events_editor.bindings[index].clone();
            egui::Grid::new(format!("ui_event_{index}")).show(ui, |ui| {
                let mut event = binding
                    .event
                    .unwrap_or(rustlua_ui::ui_events::UiEventType::Click);
                if egui::ComboBox::from_id_salt(format!("ui_event_type_{index}"))
                    .selected_text(event.as_str())
                    .show_ui(ui, |ui| {
                        for candidate in [
                            rustlua_ui::ui_events::UiEventType::Click,
                            rustlua_ui::ui_events::UiEventType::PointerEnter,
                            rustlua_ui::ui_events::UiEventType::PointerExit,
                            rustlua_ui::ui_events::UiEventType::PointerDown,
                            rustlua_ui::ui_events::UiEventType::PointerUp,
                            rustlua_ui::ui_events::UiEventType::Focus,
                            rustlua_ui::ui_events::UiEventType::Submit,
                            rustlua_ui::ui_events::UiEventType::ValueChanged,
                            rustlua_ui::ui_events::UiEventType::Drag,
                        ] {
                            ui.selectable_value(&mut event, candidate, candidate.as_str());
                        }
                    })
                    .response
                    .changed()
                {
                    binding.event = Some(event);
                    changed = true;
                }
                if ui
                    .add(
                        egui::TextEdit::singleline(binding.target.get_or_insert_with(String::new))
                            .hint_text("target"),
                    )
                    .changed()
                {
                    if binding.target.as_deref().is_none_or(str::is_empty) {
                        binding.target = None;
                    }
                    changed = true;
                }
                if ui
                    .add(egui::TextEdit::singleline(&mut binding.function).hint_text("function"))
                    .changed()
                {
                    changed = true;
                }
                let mut args = binding
                    .arguments
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(",");
                if ui
                    .add(egui::TextEdit::singleline(&mut args).hint_text("k=v,k2=v2"))
                    .changed()
                {
                    binding.arguments = args
                        .split([',', ';'])
                        .filter_map(|item| item.trim().split_once('='))
                        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
                        .filter(|(key, _)| !key.is_empty())
                        .collect();
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui.button("移除").clicked() {
                    self.events_editor.bindings.remove(index);
                    changed = true;
                }
            });
            if changed {
                self.events_editor.bindings[index] = binding;
            }
        }
        ui.horizontal(|ui| {
            if ui.button("添加 Event").clicked() {
                self.events_editor
                    .bindings
                    .push(rustlua_ui::ui_events::UiEventBinding {
                        event: Some(rustlua_ui::ui_events::UiEventType::Click),
                        target: None,
                        function: "on_click".to_owned(),
                        arguments: Default::default(),
                    });
            }
            if ui.button("应用 Events").clicked() {
                self.apply_events_editor();
            }
        });
    }

    fn draw_ui_bindings_editor(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("Property Bindings");
        let mut changed = false;
        for index in 0..self.bindings_editor.bindings.len() {
            let mut binding = self.bindings_editor.bindings[index].clone();
            egui::Grid::new(format!("ui_binding_{index}")).show(ui, |ui| {
                let property = binding
                    .property
                    .unwrap_or(rustlua_ui::ui_binding::BoundProperty::TextText);
                if egui::ComboBox::from_id_salt(format!("binding_property_{index}"))
                    .selected_text(property.as_str())
                    .show_ui(ui, |ui| {
                        for candidate in [
                            rustlua_ui::ui_binding::BoundProperty::TextText,
                            rustlua_ui::ui_binding::BoundProperty::ImageVisible,
                            rustlua_ui::ui_binding::BoundProperty::ProgressValue,
                        ] {
                            ui.selectable_value(
                                &mut binding.property,
                                Some(candidate),
                                candidate.as_str(),
                            );
                        }
                    })
                    .response
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .add(egui::TextEdit::singleline(&mut binding.target).hint_text("target"))
                    .changed()
                {
                    changed = true;
                }
                let mut source = binding.source.0.join(".");
                if ui
                    .add(egui::TextEdit::singleline(&mut source).hint_text("data.path"))
                    .changed()
                    && let Ok(path) = rustlua_ui::ui_binding::BindingPath::parse(&source)
                {
                    binding.source = path;
                    changed = true;
                }
                let mut default = binding.default.clone().unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut default).hint_text("default"))
                    .changed()
                {
                    binding.default = (!default.trim().is_empty()).then_some(default);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui.button("移除").clicked() {
                    self.bindings_editor.bindings.remove(index);
                    changed = true;
                }
            });
            if changed {
                self.bindings_editor.bindings[index] = binding;
            }
        }
        ui.horizontal(|ui| {
            if ui.button("添加 Binding").clicked() {
                self.bindings_editor
                    .bindings
                    .push(rustlua_ui::ui_binding::PropertyBinding {
                        property: Some(rustlua_ui::ui_binding::BoundProperty::TextText),
                        target: String::new(),
                        source: rustlua_ui::ui_binding::BindingPath(vec![
                            "data".to_owned(),
                            "value".to_owned(),
                        ]),
                        default: None,
                    });
            }
            if ui.button("应用 Bindings").clicked() {
                self.apply_bindings_editor();
            }
        });
    }

    fn draw_button_states_editor(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("Button States");
        let states = &mut self.button_states_editor;
        for (label, value) in [
            ("Normal", &mut states.normal),
            ("Hover", &mut states.hover),
            ("Pressed", &mut states.pressed),
            ("Disabled", &mut states.disabled),
            ("Focused", &mut states.focused),
        ] {
            let mut text = value.clone().unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut text).hint_text(label))
                .changed()
            {
                *value = (!text.trim().is_empty()).then(|| text.trim().to_owned());
            }
        }
        if ui
            .checkbox(&mut self.button_states_editor.disabled_state, "禁用状态")
            .changed()
        {
            self.status = "ButtonStates 已在缓冲中修改".to_owned();
        }
        if ui.button("应用 Button States").clicked() {
            self.apply_button_states_editor();
        }
    }

    fn draw_ui_animation_editor(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("UI Animation");
        for (property, track) in self.ui_animation_editor.tracks.clone() {
            ui.label(format!("轨道：{property}"));
            for (index, mut keyframe) in track.keyframes.into_iter().enumerate() {
                ui.horizontal(|ui| {
                    let animation_changed = ui
                        .add(egui::DragValue::new(&mut keyframe.time).speed(0.01))
                        .changed()
                        || ui
                            .add(egui::DragValue::new(&mut keyframe.value).speed(0.01))
                            .changed();
                    if animation_changed
                        && let Some(track) = self.ui_animation_editor.tracks.get_mut(&property)
                    {
                        track.keyframes[index] = keyframe;
                    }
                    let mut easing = keyframe.easing;
                    egui::ComboBox::from_id_salt(format!("animation_easing_{property}_{index}"))
                        .selected_text(easing.as_str())
                        .show_ui(ui, |ui| {
                            for candidate in
                                [UiEasing::Linear, UiEasing::Step, UiEasing::SmoothStep]
                            {
                                ui.selectable_value(&mut easing, candidate, candidate.as_str());
                            }
                        });
                    if easing != keyframe.easing
                        && let Some(track) = self.ui_animation_editor.tracks.get_mut(&property)
                    {
                        track.keyframes[index].easing = easing;
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.button("加关键帧").clicked()
                    && let Some(track) = self.ui_animation_editor.tracks.get_mut(&property)
                {
                    let time = track.keyframes.last().map_or(0.0, |frame| frame.time + 0.5);
                    track.keyframes.push(UiKeyframe {
                        time,
                        value: 0.0,
                        easing: UiEasing::Linear,
                    });
                }
            });
        }
        ui.horizontal(|ui| {
            if ui.button("添加 opacity 轨道").clicked() {
                let track = rustlua_ui::ui_animation::UiAnimationTrack::new(
                    "opacity",
                    vec![UiKeyframe {
                        time: 0.0,
                        value: 0.0,
                        easing: UiEasing::SmoothStep,
                    }],
                );
                if let Ok(track) = track {
                    let _ = self.ui_animation_editor.add_track(track);
                }
            }
            if ui.button("播放").clicked() && !self.ui_animation_playing {
                self.toggle_ui_animation_preview();
            }
            if ui.button("停止").clicked() && self.ui_animation_playing {
                self.toggle_ui_animation_preview();
            }
            if ui.button("归零").clicked() {
                self.reset_ui_animation_preview();
            }
            if ui.button("应用动画").clicked() {
                self.apply_ui_animation_editor();
            }
        });
        if let Some(preview) = &self.ui_animation_preview {
            ui.label(format!("预览 t={:.2}", preview.time));
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn apply_inspector(&mut self) -> bool {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
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

    /// 选中实体的组件键列表（Inspector 组件管理用）。
    fn current_components(&self) -> Vec<String> {
        self.current_scene()
            .and_then(|s| {
                self.selection.primary().map(|id| {
                    s.scene
                        .entities
                        .get(&id)
                        .map(|e| e.components.keys().cloned().collect())
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default()
    }

    /// 注册表里是否有这个组件（Add Component 的两个入口共用同一条判定与文案）。
    pub(crate) fn component_registered(name: &str) -> bool {
        COMPONENT_REGISTRY
            .iter()
            .any(|descriptor| descriptor.name == name)
    }

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
        self.status = "Components copied".to_owned();
    }

    // 粘贴组件（`EditorCommand::PasteComponents`）与重命名（`RenameSelection`）的实现体
    // 都已搬到 shell（`crate::actions`）：面板按钮只发 typed action。

    /// 确保源 Prefab 已缓存（必要时重新读盘）。返回后不再持有 `&mut self` 借用，
    /// 调用方随后从 `prefab_source_cache` 取不可变引用即可。
    pub(crate) fn ensure_prefab_source_cached(&mut self, path: &Path) -> Result<(), String> {
        let stamp = prefab_stamp(path);
        let cached_fresh = stamp.is_some()
            && self
                .prefab_source_cache
                .get(path)
                .is_some_and(|cached| cached.stamp == stamp);
        if cached_fresh {
            return Ok(());
        }
        let prefab = Prefab::load(path).map_err(|error| format!("无法加载源 Prefab：{error}"))?;
        self.prefab_source_cache
            .insert(path.to_path_buf(), CachedPrefabSource { stamp, prefab });
        Ok(())
    }

    fn prefab_override_summary(
        &mut self,
        selected: SceneEntityId,
    ) -> Option<Result<Vec<PrefabOverrideRow>, String>> {
        // 先把需要的元数据拷出来，结束对场景的不可变借用。
        let (prefab_source, source_id) = {
            let scene = self.current_scene()?;
            let entity = scene.scene.entities.get(&selected)?;
            let prefab_source = entity.metadata.get("prefab.source")?.clone();
            let source_id = entity
                .metadata
                .get("prefab.source_id")?
                .parse::<u64>()
                .ok()
                .map(SceneEntityId)?;
            (prefab_source, source_id)
        };
        let path = strip_verbatim_prefix(&self.project.root).join(&prefab_source);
        if let Err(error) = self.ensure_prefab_source_cached(&path) {
            return Some(Err(error));
        }
        // 之后全是不可变借用：缓存里的 Prefab 与当前场景可以同时持有。
        let prefab = self
            .prefab_source_cache
            .get(&path)
            .map(|cached| &cached.prefab)?;
        let Some(source_entity) = prefab.entities.get(&source_id) else {
            return Some(Err(format!("源 Prefab 中缺少实体 {}", source_id.0)));
        };
        let scene = self.current_scene()?;
        let entity = scene.scene.entities.get(&selected)?;
        Some(Ok(prefab_override_rows(source_entity, entity)))
    }

    // 选择性 Revert（`EditorCommand::RevertPrefabOverride`）的实现体已搬到 shell
    // （`crate::actions` 的 `revert_prefab_override_field`）：面板只发 typed action。

    // 重新应用源 Prefab（`EditorCommand::ReapplyPrefab`）的实现体已搬到 shell
    // （`crate::actions` 的 `reapply_prefab`）：目标由 typed action 显式携带。

    /// 把实例写回源 Prefab 文件（`EditorCommand::SavePrefabToSource`）与逐字段 Apply
    /// （`EditorCommand::ApplyPrefabOverride`）的实现体都已搬到 shell（`crate::actions` 的
    /// `save_prefab_to_source` / `apply_prefab_override_field`）：面板只发 typed action。
    /// 是否是 Prefab 实例（有 `prefab.source`）。typed action 的前置条件检查用它，
    /// 避免"面板与命令层各写一遍元数据判断"。
    pub(crate) fn is_prefab_instance(&self, selected: SceneEntityId) -> bool {
        self.document
            .current_scene
            .as_ref()
            .and_then(|document| document.scene.entities.get(&selected))
            .is_some_and(|entity| entity.metadata.contains_key("prefab.source"))
    }

    /// 是否是 Prefab **根**实例（`prefab.root=true`）：只有根实例支持重新应用/写回/解包。
    pub(crate) fn is_prefab_root(&self, selected: SceneEntityId) -> bool {
        self.document
            .current_scene
            .as_ref()
            .and_then(|document| document.scene.entities.get(&selected))
            .is_some_and(|entity| {
                entity
                    .metadata
                    .get("prefab.root")
                    .is_some_and(|value| value == "true")
            })
    }

    /// Prefab 实例的 `(来源路径, 源实体 ID)`；非实例返回 `None`。
    pub(crate) fn prefab_instance_source(
        &self,
        selected: SceneEntityId,
    ) -> Option<(String, SceneEntityId)> {
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
        ))
    }

    /// 覆盖字段的**实例侧**原始文本（应用回源时用的就是这个值）。
    pub(crate) fn prefab_override_instance_value(
        &self,
        selected: SceneEntityId,
        field: &str,
    ) -> Option<String> {
        let scene = self.document.current_scene.as_ref()?;
        let entity = scene.scene.entities.get(&selected)?;
        Some(prefab_override_side_value(entity, field))
    }

    // 「落盘 Prefab + 重建实例」整条路径（`write_prefab_and_rebuild_instances` 及其
    // Prefab 写路径辅助）已整体搬到 shell（`crate::actions`）：面板不再持有提交边界。

    // 解包 Prefab（`EditorCommand::UnpackPrefab`）的实现体已搬到 shell
    // （`crate::actions` 的 `unpack_prefab`）：面板只发 typed action。
}

// 混合值判定（`has_mixed_transform`/`has_mixed_transform3d`）**已删除**（第 36 轮）：
// `Transform`/`Transform3D` 迁到通用编辑器后，逐字段的 `SchemaFieldRow.mixed` 就是混合值来源
// （按字段而不是按组件判定，比原来更细）。

// Prefab 写路径辅助（`PrefabRebuildState`、`collect_prefab_rebuild_state`、
// `build_prefab_from_instance_root`、`prefab_root_ids`、`collect_prefab_overrides`、
// `prefab_override_value`、`collect_scene_subtree`、`apply_prefab_instance_metadata`、
// `preserve_prefab_editor_flags`、`PrefabWriteRequest`）已整体搬到 shell（`crate::actions`）：
// 它们只在"写回/重新应用/解包"这条提交路径上使用。面板侧只保留**显示用**的
// Prefab diff（`diff_prefab_entity` / `prefab_override_rows`）与源缓存。

/// Prefab 覆盖列表的一行：字段 + 源取值 + 实例取值（都用于显示与选择性回退）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct PrefabOverrideRow {
    field: String,
    source_value: String,
    instance_value: String,
}

/// 由覆盖字段串计算某一侧的原始文本（源或实例）。
fn prefab_override_side_value(entity: &SceneEntity, field: &str) -> String {
    if field == "name" {
        return entity.name.clone();
    }
    if let Some(key) = field.strip_prefix("component:") {
        return entity
            .components
            .get(key)
            .cloned()
            .unwrap_or_else(|| "（缺失）".to_owned());
    }
    if let Some(raw_path) = field.strip_prefix("property:") {
        let Ok(path) = rustlua_ecs::property_path::PropertyPath::parse(raw_path) else {
            return "（非法路径）".to_owned();
        };
        let Some(payload) = entity.components.get(&path.component) else {
            return "（缺失）".to_owned();
        };
        return rustlua_ecs::property_path::read_field_text(payload, &path)
            .unwrap_or_else(|_| "（缺失）".to_owned());
    }
    if let Some(key) = field.strip_prefix("metadata:") {
        return entity
            .metadata
            .get(key)
            .cloned()
            .unwrap_or_else(|| "（缺失）".to_owned());
    }
    "（未知字段）".to_owned()
}

/// 由 diff 产生的字段串计算「源 / 实例」两侧的原始文本。
fn prefab_override_rows(source: &SceneEntity, instance: &SceneEntity) -> Vec<PrefabOverrideRow> {
    diff_prefab_entity(source, instance)
        .into_iter()
        .map(|field| PrefabOverrideRow {
            source_value: prefab_override_side_value(source, &field),
            instance_value: prefab_override_side_value(instance, &field),
            field,
        })
        .collect()
}

/// 列表里只显示前若干字符，避免长组件串把行撑爆。
fn compact_value(value: &str) -> String {
    const MAX: usize = 48;
    if value.chars().count() <= MAX {
        return value.to_owned();
    }
    let head: String = value.chars().take(MAX).collect();
    format!("{head}…")
}

pub(crate) fn diff_prefab_entity(source: &SceneEntity, instance: &SceneEntity) -> Vec<String> {
    let mut overrides = Vec::new();
    if source.name != instance.name {
        overrides.push("name".to_owned());
    }

    let component_keys = source
        .components
        .keys()
        .chain(instance.components.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let registry = rustlua_ecs::component_schema::builtin_registry();
    for key in component_keys {
        let source_value = source.components.get(&key);
        let instance_value = instance.components.get(&key);
        if source_value == instance_value {
            continue;
        }
        // docs/55 阶段 3：**逐字段** Override。两边都在、且该组件有 schema 时，
        // 只把真正变化的字段列为覆盖（其余字段不动），这样编辑一个字段不会冻结整个组件串；
        // 组件新增/删除或没有 schema 时退回组件级覆盖（整串替换）。
        let field_level = match (source_value, instance_value, registry.get(key.as_str())) {
            (Some(source_payload), Some(instance_payload), Some(schema)) => {
                let (Ok(source_data), Ok(instance_data)) =
                    (schema.parse(source_payload), schema.parse(instance_payload))
                else {
                    overrides.push(format!("component:{key}"));
                    continue;
                };
                let mut fields = std::collections::BTreeSet::new();
                for (field, source_field) in &source_data {
                    match instance_data.get(field) {
                        Some(instance_field) if instance_field == source_field => {}
                        Some(_) => {
                            fields.insert(field.clone());
                        }
                        None => {
                            fields.insert(field.clone());
                        }
                    }
                }
                for field in instance_data.keys() {
                    if !source_data.contains_key(field) {
                        fields.insert(field.clone());
                    }
                }
                // 字段级集合可能为空（例如仅格式不同）：此时不产生噪音覆盖。
                for field in fields {
                    overrides.push(format!("property:{key}.{field}"));
                }
                true
            }
            _ => false,
        };
        if !field_level && source_value != instance_value {
            overrides.push(format!("component:{key}"));
        }
    }

    let metadata_keys = source
        .metadata
        .keys()
        .filter(|key| !key.starts_with("prefab."))
        .chain(
            instance
                .metadata
                .keys()
                .filter(|key| !key.starts_with("prefab.")),
        )
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for key in metadata_keys {
        if source.metadata.get(&key) != instance.metadata.get(&key) {
            overrides.push(format!("metadata:{key}"));
        }
    }

    overrides
}

#[cfg(test)]
mod tests {
    use super::*;
    // Prefab 写路径的纯函数住在 shell（`crate::actions`）：测试直接调它们做单元级断言。
    use crate::EditorApp;
    use crate::actions::{
        apply_prefab_instance_metadata, build_prefab_from_instance_root,
        collect_prefab_rebuild_state,
    };
    use editor::open_project;
    use rustlua_animation::animation::AnimationStateMachine;
    use rustlua_assets::resource_db::AssetId;
    use rustlua_infra::logging::LogSink;
    use rustlua_scene::prefab::Prefab;
    use rustlua_scene::scene::{Scene, SceneEntity, SceneEntityId};
    use rustlua_ui::ui::UiText;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_token() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos()
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rustlua-m14-01-inspector-{label}-{}",
            unique_token()
        ))
    }

    fn sample_project_root() -> PathBuf {
        rustlua_infra::games_dir().join("examples").join("game_api")
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

    /// 打开示例工程副本并打开 **Prefab 实例场景**（`api_prefab.scene`）。
    ///
    /// 该场景里两个 Prefab 源（`assets/prefabs/slime_green.prefab` / `slime_red.prefab`）
    /// 各有实例，实例实体带 `prefab.root` / `prefab.source` / `prefab.source_id` 元数据——
    /// Prefab 的 Apply / Revert / 逐字段 Override 要有真实实例才测得动。
    fn sample_app_with_production_scene(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_sample_project(label);
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/api_prefab.scene"));
        (app, root)
    }

    /// 打开示例工程副本并打开 2D 综合场景（`main.scene`）：动画/精灵/物理这些
    /// "玩法侧"组件的样本都在这里。
    fn sample_app_with_main_scene(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_sample_project(label);
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/main.scene"));
        (app, root)
    }

    /// 打开示例工程副本并打开 **场景 UI 场景**（`ui_test.scene`）。
    fn sample_app_with_menu_scene(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_sample_project(label);
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
        (app, root)
    }

    fn select_entity(app: &mut EditorApp, id: SceneEntityId) {
        app.selection
            .set_single(id, editor::SelectionSource::Hierarchy);
        app.sync_inspector(id);
    }

    fn prefab_root(scene: &Scene, prefab_source: &str) -> SceneEntityId {
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
                        .is_some_and(|value| value == prefab_source)
            })
            .map(|entity| entity.id)
            .expect("sample scene should contain prefab root")
    }

    fn entity() -> SceneEntity {
        SceneEntity {
            id: SceneEntityId(1),
            name: "Enemy".to_owned(),
            parent: None,
            children: Vec::new(),
            components: std::collections::BTreeMap::from([
                (
                    "Transform".to_owned(),
                    "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
                ),
                ("Sprite".to_owned(), "texture=enemy.png".to_owned()),
            ]),
            resources: std::collections::BTreeMap::new(),
            metadata: std::collections::BTreeMap::from([("team".to_owned(), "red".to_owned())]),
        }
    }

    #[test]
    fn diff_prefab_entity_reports_name_component_and_metadata_changes() {
        let source = entity();
        let mut instance = entity();
        instance.name = "Enemy Boss".to_owned();
        // 两边都是**合法载荷** ⇒ 走字段级覆盖（`position` 这一个字段变了）。
        instance.components.insert(
            "Transform".to_owned(),
            "position=10,4,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
        );
        // 实例上多出来的组件没有字段级对拍对象 ⇒ 仍是组件级覆盖（整串替换）。
        instance.components.insert(
            "Light".to_owned(),
            "direction=0,0,-1;color=1,1,1;intensity=1".to_owned(),
        );
        instance
            .metadata
            .insert("team".to_owned(), "blue".to_owned());
        instance.metadata.insert(
            "prefab.source".to_owned(),
            "assets/prefabs/enemy.prefab".to_owned(),
        );
        let overrides = diff_prefab_entity(&source, &instance);
        assert_eq!(
            overrides,
            vec![
                "name".to_owned(),
                // 组件按键名排序：`Light` 在 `Transform` 之前。
                "component:Light".to_owned(),
                "property:Transform.position".to_owned(),
                "metadata:team".to_owned()
            ]
        );
    }

    #[test]
    fn collect_prefab_rebuild_state_preserves_detected_overrides() {
        let mut source = Scene::new("prefab");
        source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
        source
            .add_entity(SceneEntityId(2), "HitBox", Some(SceneEntityId(1)))
            .unwrap();
        source
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Transform".to_owned(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
            );
        let prefab = Prefab::from_scene(
            &source,
            SceneEntityId(1),
            AssetId::from_seed("enemy.prefab"),
            "Enemy",
        )
        .unwrap();

        let mut scene = Scene::new("runtime");
        let instance = prefab
            .instantiate(&mut scene, None, &std::collections::BTreeMap::new())
            .unwrap();
        apply_prefab_instance_metadata(&mut scene, "assets/prefabs/enemy.prefab", &instance);
        scene
            .entities
            .get_mut(&instance.instance_root)
            .unwrap()
            .name = "Boss".to_owned();
        let child = instance.source_to_instance[&SceneEntityId(2)];
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform".to_owned(),
            "position=9,4,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
        );

        let rebuild = collect_prefab_rebuild_state(
            &scene,
            instance.instance_root,
            "assets/prefabs/enemy.prefab",
            &prefab,
        )
        .unwrap();
        assert_eq!(
            rebuild
                .overrides
                .get(&(SceneEntityId(1), "name".to_owned())),
            Some(&"Boss".to_owned())
        );
        // 逐字段 Override：只记录真正变化的字段（原始文本），不再冻结整个组件串。
        // 统一 `Transform` 之后位置的字段名是 `position`（Vec3），不再是 `x`/`y` 两个字段。
        assert_eq!(
            rebuild
                .overrides
                .get(&(SceneEntityId(2), "property:Transform.position".to_owned())),
            Some(&"9,4,0".to_owned())
        );
        assert!(
            !rebuild
                .overrides
                .contains_key(&(SceneEntityId(2), "component:Transform".to_owned())),
            "有 schema 的组件必须走字段级覆盖，而不是整串覆盖"
        );
    }

    #[test]
    fn collect_prefab_rebuild_state_rejects_mixed_subtree() {
        let mut source = Scene::new("prefab");
        source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
        let prefab = Prefab::from_scene(
            &source,
            SceneEntityId(1),
            AssetId::from_seed("enemy.prefab"),
            "Enemy",
        )
        .unwrap();

        let mut scene = Scene::new("runtime");
        let instance = prefab
            .instantiate(&mut scene, None, &std::collections::BTreeMap::new())
            .unwrap();
        apply_prefab_instance_metadata(&mut scene, "assets/prefabs/enemy.prefab", &instance);
        scene
            .add_entity(
                SceneEntityId(99),
                "LooseChild",
                Some(instance.instance_root),
            )
            .unwrap();

        let error = collect_prefab_rebuild_state(
            &scene,
            instance.instance_root,
            "assets/prefabs/enemy.prefab",
            &prefab,
        )
        .unwrap_err();
        assert!(error.contains("非同源节点") || error.contains("结构不一致"));
    }

    #[test]
    fn build_prefab_from_instance_root_strips_prefab_metadata() {
        let mut source = Scene::new("prefab");
        source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
        let prefab = Prefab::from_scene(
            &source,
            SceneEntityId(1),
            AssetId::from_seed("enemy.prefab"),
            "Enemy",
        )
        .unwrap();

        let mut instance_scene = Scene::new("runtime");
        let instance = prefab
            .instantiate(
                &mut instance_scene,
                None,
                &std::collections::BTreeMap::new(),
            )
            .unwrap();
        apply_prefab_instance_metadata(
            &mut instance_scene,
            "assets/prefabs/enemy.prefab",
            &instance,
        );
        instance_scene
            .entities
            .get_mut(&instance.instance_root)
            .unwrap()
            .metadata
            .insert("designer.note".to_owned(), "boss".to_owned());

        let rebuilt =
            build_prefab_from_instance_root(&instance_scene, instance.instance_root, &prefab)
                .unwrap();
        assert!(
            !rebuilt.entities[&rebuilt.root]
                .metadata
                .contains_key("prefab.source")
        );
        assert_eq!(
            rebuilt.entities[&rebuilt.root]
                .metadata
                .get("designer.note"),
            Some(&"boss".to_owned())
        );
    }

    /// 源 Prefab 缓存：命中时不再读盘；文件被改动后必须重载，不能显示过期差异。
    #[test]
    fn prefab_source_cache_hits_and_invalidates_on_file_change() {
        let (mut app, root) = sample_app_with_production_scene("prefab-cache");
        let target = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            prefab_root(scene, "assets/prefabs/slime_green.prefab")
        };
        // 制造一个 override（改名），让摘要非空。
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            let entity = scene.scene.entities.get_mut(&target).unwrap();
            entity.name = "CachedGreen".to_owned();
            scene.mark_dirty();
        }
        select_entity(&mut app, target);

        let first = app
            .prefab_override_summary(target)
            .expect("prefab instance must produce a summary")
            .expect("summary must load");
        assert!(first.iter().any(|row| row.field == "name"), "{first:?}");
        assert_eq!(app.prefab_source_cache.len(), 1, "首次读取应填充缓存");

        // 第二次读取应命中缓存（内容一致、不重新读盘）。
        let second = app
            .prefab_override_summary(target)
            .expect("summary again")
            .expect("cached summary must load");
        assert_eq!(first, second);
        assert_eq!(app.prefab_source_cache.len(), 1);

        // 外部改动源 Prefab：mtime/长度变化后必须重新加载并反映新内容。
        let prefab_path = root.join("assets/prefabs/slime_green.prefab");
        let mut prefab = Prefab::load(&prefab_path).unwrap();
        prefab.name = "renamed-by-tool".to_owned();
        prefab.save(&prefab_path).unwrap();
        let third = app
            .prefab_override_summary(target)
            .expect("summary after external edit")
            .expect("reloaded summary must load");
        assert_eq!(app.prefab_source_cache.len(), 1, "同一路径只保留一条缓存");
        // 缓存键是去掉 `\\?\` 前缀后的绝对路径，这里按文件名断言（两种路径写法等价）。
        let (cached_path, cached) = app.prefab_source_cache.iter().next().unwrap();
        assert!(
            cached_path.ends_with("slime_green.prefab"),
            "unexpected cache key: {}",
            cached_path.display()
        );
        assert_eq!(
            cached.prefab.name, "renamed-by-tool",
            "文件改动后缓存必须重载"
        );
        // 差异集合仍然来自源 Prefab 的真实内容（改名 override 依旧列出）。
        assert!(third.iter().any(|row| row.field == "name"), "{third:?}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_reapply_prefab_preserves_overrides() {
        let (mut app, root) = sample_app_with_production_scene("reapply");
        let target = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            prefab_root(scene, "assets/prefabs/slime_green.prefab")
        };
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            let entity = scene.scene.entities.get_mut(&target).unwrap();
            entity.name = "BossGreen".to_owned();
            entity.components.insert(
                "Transform".to_owned(),
                "position=77,19,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
            );
            scene.mark_dirty();
        }
        select_entity(&mut app, target);

        // typed action（面板按钮走的就是这条）：目标显式携带。
        assert_eq!(
            app.dispatch(editor::EditorCommand::ReapplyPrefab { entity: target }),
            editor::CommandResult::Applied
        );
        assert!(
            app.status.contains("已重新应用 Prefab"),
            "status should confirm prefab reapply"
        );
        let selected = app.selection.primary().unwrap();
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let entity = scene.entities.get(&selected).unwrap();
        assert_eq!(entity.name, "BossGreen");
        // 逐字段语义：被覆盖的字段保留实例取值（原始文本 `77`/`19` 不被规范化），
        // 其余字段跟随源 Prefab。这里用类型化读取断言，避免绑定源文件的格式细节。
        let payload = entity
            .components
            .get("Transform")
            .expect("Transform")
            .clone();
        let registry = rustlua_ecs::component_schema::builtin_registry();
        let read = |field: &str| {
            let path =
                rustlua_ecs::property_path::PropertyPath::parse(&format!("Transform.{field}"))
                    .expect("path");
            rustlua_ecs::property_path::read_field(&registry, &payload, &path).expect("field")
        };
        assert_eq!(
            read("position"),
            rustlua_ecs::component_schema::FieldValue::Tuple(vec![77.0, 19.0, 0.0])
        );
        assert_eq!(
            read("scale"),
            rustlua_ecs::component_schema::FieldValue::Tuple(vec![1.0, 1.0, 1.0])
        );
        assert!(
            payload.contains("position=77"),
            "覆盖字段保留原始文本：{payload}"
        );
        assert_eq!(
            entity.metadata.get("prefab.source").map(String::as_str),
            Some("assets/prefabs/slime_green.prefab")
        );
        let _ = fs::remove_dir_all(root);
    }

    /// Revert = 丢弃全部覆盖：实例字段回到源 Prefab 取值（逐字），名字覆盖同样被清掉。
    #[test]
    fn sample_project_revert_prefab_overrides_restores_source_values() {
        let (mut app, root) = sample_app_with_production_scene("revert");
        let (target, instance_count) = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            let target = prefab_root(scene, "assets/prefabs/slime_green.prefab");
            let count = scene
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
                            .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
                })
                .count();
            (target, count)
        };
        // 期望值 = 同一 Prefab 在空场景里**全新实例化**的结果：
        // 「Revert Overrides」的语义就是"回到没有覆盖的源状态"。
        let expected = {
            let prefab = rustlua_scene::prefab::Prefab::load(
                crate::util::strip_verbatim_prefix(&app.project.root)
                    .join("assets/prefabs/slime_green.prefab"),
            )
            .expect("source prefab loads");
            let mut scratch = Scene::new("scratch");
            let instance = prefab
                .instantiate(&mut scratch, None, &std::collections::BTreeMap::new())
                .expect("fresh instance");
            scratch.entities[&instance.instance_root]
                .components
                .get("Transform")
                .cloned()
                .expect("fresh instance has Transform")
        };
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            let entity = scene.scene.entities.get_mut(&target).unwrap();
            entity.name = "RevertedName".to_owned();
            entity.components.insert(
                "Transform".to_owned(),
                "position=123,456,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
            );
            scene.mark_dirty();
        }
        select_entity(&mut app, target);

        assert_eq!(
            app.dispatch(editor::EditorCommand::RevertPrefabOverrides { entity: target }),
            editor::CommandResult::Applied
        );
        assert!(
            app.status.contains("已还原 Prefab 覆盖"),
            "status should confirm the revert: {}",
            app.status
        );
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        // 生产示例场景里同一个 Prefab 有多个同级实例，且 revert 会重建实例（新实体 ID），
        // 因此这里做**差分**断言：实例数量不变、名字覆盖消失、且恰好出现一个
        // 取值等于"全新实例化"的实例根（即刚被 revert 的那个）。
        let roots: Vec<&rustlua_scene::scene::SceneEntity> = scene
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
                        .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
            })
            .collect();
        assert_eq!(roots.len(), instance_count, "revert 不得增删实例");
        assert!(
            scene
                .entities
                .values()
                .all(|candidate| candidate.name != "RevertedName"),
            "名字覆盖必须被还原"
        );
        let reverted: Vec<&&rustlua_scene::scene::SceneEntity> = roots
            .iter()
            .filter(|entity| entity.components.get("Transform") == Some(&expected))
            .collect();
        assert_eq!(
            reverted.len(),
            1,
            "恰有一个实例根回到源 Prefab 取值（逐字）：{expected}"
        );
        assert_eq!(
            reverted[0]
                .metadata
                .get("prefab.source")
                .map(String::as_str),
            Some("assets/prefabs/slime_green.prefab"),
            "revert 后必须保留 Prefab 来源元数据"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 选择性 Revert（docs/55 阶段 3）：只回退一个字段，其它覆盖必须原样保留；
    /// 且回退是**一条** Undo（可整体收回）。
    #[test]
    fn selective_revert_restores_one_field_and_keeps_other_overrides() {
        let (mut app, root) = sample_app_with_production_scene("selective-revert");
        let target = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            prefab_root(scene, "assets/prefabs/slime_green.prefab")
        };
        // 制造两个独立覆盖：名字 + Transform.x。
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            let entity = scene.scene.entities.get_mut(&target).unwrap();
            entity.name = "SelectiveGreen".to_owned();
            let payload = entity
                .components
                .get("Transform")
                .cloned()
                .expect("instance has Transform");
            let path =
                rustlua_ecs::property_path::PropertyPath::parse("Transform.position").unwrap();
            let updated =
                rustlua_ecs::property_path::write_builtin(&payload, &path, "999,0,0").unwrap();
            entity.components.insert("Transform".to_owned(), updated);
            scene.mark_dirty();
        }
        select_entity(&mut app, target);

        // 覆盖列表里应同时出现字段级条目。
        let rows = app
            .prefab_override_summary(target)
            .expect("summary")
            .expect("loads");
        assert!(
            rows.iter()
                .any(|row| row.field == "property:Transform.position"),
            "{rows:?}"
        );
        assert!(rows.iter().any(|row| row.field == "name"), "{rows:?}");

        let undo_before = app.history.undo_len();
        app.revert_prefab_override_field(target, "property:Transform.position");

        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let entity = scene.entities.get(&target).expect("instance");
        let payload = entity.components.get("Transform").expect("Transform");
        let path = rustlua_ecs::property_path::PropertyPath::parse("Transform.position").unwrap();
        let registry = rustlua_ecs::component_schema::builtin_registry();
        let source_position = {
            let prefab = Prefab::load(root.join("assets/prefabs/slime_green.prefab")).unwrap();
            let source_id = entity
                .metadata
                .get("prefab.source_id")
                .and_then(|value| value.parse::<u64>().ok())
                .map(SceneEntityId)
                .expect("source id");
            let source_payload = prefab.entities[&source_id]
                .components
                .get("Transform")
                .expect("source Transform");
            rustlua_ecs::property_path::read_field(&registry, source_payload, &path)
                .expect("source position")
        };
        assert_eq!(
            rustlua_ecs::property_path::read_field(&registry, payload, &path).expect("position"),
            source_position,
            "被回退字段必须等于源取值"
        );
        assert_eq!(entity.name, "SelectiveGreen", "其它覆盖（名字）必须保留");
        assert_eq!(
            app.history.undo_len(),
            undo_before + 1,
            "选择性回退必须只产生一条历史"
        );

        // 一次 Undo 收回这次回退。
        app.dispatch(editor::EditorCommand::Undo);
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities[&target]
            .components
            .get("Transform")
            .expect("Transform")
            .clone();
        let position =
            rustlua_ecs::property_path::read_field(&registry, &payload, &path).expect("position");
        assert_eq!(
            position,
            rustlua_ecs::component_schema::FieldValue::Tuple(vec![999.0, 0.0, 0.0]),
            "Undo 后覆盖字段应恢复"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 逐字段 Apply（docs/55 阶段 3）：把实例取值写回**源 Prefab 文件**并更新全部实例；
    /// 应用后该字段不再是覆盖，其它覆盖保持不变。
    #[test]
    fn apply_prefab_override_field_writes_source_and_drops_that_override() {
        let (mut app, root) = sample_app_with_production_scene("selective-apply");
        let target = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            prefab_root(scene, "assets/prefabs/slime_green.prefab")
        };
        // 制造两个覆盖：Transform.position=999,0,0 与名字。
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            let entity = scene.scene.entities.get_mut(&target).unwrap();
            entity.name = "ApplyGreen".to_owned();
            let payload = entity
                .components
                .get("Transform")
                .cloned()
                .expect("instance has Transform");
            let path =
                rustlua_ecs::property_path::PropertyPath::parse("Transform.position").unwrap();
            let updated =
                rustlua_ecs::property_path::write_builtin(&payload, &path, "999,0,0").unwrap();
            entity.components.insert("Transform".to_owned(), updated);
            scene.mark_dirty();
        }
        select_entity(&mut app, target);
        let source_id = app.document.current_scene.as_ref().unwrap().scene.entities[&target]
            .metadata
            .get("prefab.source_id")
            .and_then(|value| value.parse::<u64>().ok())
            .map(SceneEntityId)
            .expect("source id");

        let undo_before = app.history.undo_len();
        app.apply_prefab_override_field(target, "property:Transform.position");
        assert!(
            app.status.contains("已把字段应用到源 Prefab"),
            "status: {}",
            app.status
        );

        // 1) 源 Prefab 文件里该字段已更新。
        let prefab = Prefab::load(root.join("assets/prefabs/slime_green.prefab"))
            .expect("source prefab reloads");
        let registry = rustlua_ecs::component_schema::builtin_registry();
        let path = rustlua_ecs::property_path::PropertyPath::parse("Transform.position").unwrap();
        assert_eq!(
            rustlua_ecs::property_path::read_field(
                &registry,
                prefab.entities[&source_id]
                    .components
                    .get("Transform")
                    .expect("source Transform"),
                &path
            )
            .expect("source position"),
            rustlua_ecs::component_schema::FieldValue::Tuple(vec![999.0, 0.0, 0.0]),
            "源 Prefab 文件必须被更新"
        );

        // 2) 该字段不再是覆盖（实例 == 源），名字覆盖仍在。
        let rows = app
            .prefab_override_summary(app.selection.primary().unwrap())
            .expect("summary")
            .expect("loads");
        assert!(
            !rows
                .iter()
                .any(|row| row.field == "property:Transform.position"),
            "应用后该字段不应再是覆盖：{rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.field == "name"),
            "其它覆盖必须保留：{rows:?}"
        );

        // 3) 场景更新是**一条**历史（文件写入本身不属于文档历史，这点与整体写回一致）。
        assert_eq!(app.history.undo_len(), undo_before + 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_save_prefab_to_source_updates_all_instances() {
        let (mut app, root) = sample_app_with_production_scene("save-source");
        let (target, original_names) = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            let target = prefab_root(scene, "assets/prefabs/slime_green.prefab");
            let names = scene
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
                            .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
                })
                .map(|entity| entity.name.clone())
                .collect::<Vec<_>>();
            (target, names)
        };
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            let entity = scene.scene.entities.get_mut(&target).unwrap();
            entity.name = "SlimeGreenElite".to_owned();
            scene.mark_dirty();
        }
        select_entity(&mut app, target);

        assert_eq!(
            app.dispatch(editor::EditorCommand::SavePrefabToSource { entity: target }),
            editor::CommandResult::Applied
        );
        assert!(
            app.status.contains("已写回 Prefab 源并更新实例"),
            "status should confirm prefab save, got: {}",
            app.status
        );
        let saved = Prefab::load(root.join("assets/prefabs/slime_green.prefab")).unwrap();
        assert_eq!(saved.entities[&saved.root].name, "SlimeGreenElite");
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let names: Vec<_> = scene
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
                        .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
            })
            .map(|entity| entity.name.as_str())
            .collect();
        assert!(
            !names.is_empty(),
            "sample scene should still contain green slimes"
        );
        assert!(
            names.contains(&"SlimeGreenElite"),
            "selected instance should rebuild from updated source"
        );
        assert!(
            original_names
                .iter()
                .filter(|name| name.as_str() != "SlimeGreen1")
                .all(|name| names.contains(&name.as_str())),
            "other instance name overrides should be preserved"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// `Animation` 面板（仍在缓冲里的组件）的编辑要能在保存 / 重开后**逐字段保留**。
    ///
    /// 实体与剪辑名都不写死：从场景里取第一个带 `Animation` 的实体与它的第一个剪辑名，
    /// 只改"帧率 + 是否循环"这两个数值字段，再比对重开后的**数值**。
    #[test]
    fn sample_project_animation_edit_persists_after_save_and_reopen() {
        let (mut app, root) = sample_app_with_main_scene("animation-edit");
        let (id, clip_name) = {
            let scene = &app.document.current_scene.as_ref().unwrap().scene;
            let entity = scene
                .entities
                .values()
                .find(|entity| entity.components.contains_key("Animation"))
                .expect("示例 main.scene 应当有带 Animation 的实体");
            let machine =
                AnimationStateMachine::from_component_str(&entity.components["Animation"])
                    .expect("Animation 组件串应当可解析");
            let clip = machine.clips.first().expect("动画实体应当至少有一个 clip");
            (entity.id, clip.name.clone())
        };
        select_entity(&mut app, id);
        app.animation.machine.default = clip_name.clone();
        let clip = app
            .animation
            .machine
            .clips
            .iter_mut()
            .find(|clip| clip.name == clip_name)
            .expect("选中的剪辑应当还在面板缓冲里");
        clip.frame_rate = 12.0;
        clip.looped = false;

        app.apply_inspector();
        app.save_scene();
        app.open_scene_resource(PathBuf::from("scenes/main.scene"));

        let reopened = app.document.current_scene.as_ref().unwrap();
        let animation = AnimationStateMachine::from_component_str(
            &reopened.scene.entities[&id].components["Animation"],
        )
        .unwrap();
        assert_eq!(animation.default, clip_name);
        let clip = animation
            .clips
            .iter()
            .find(|clip| clip.name == clip_name)
            .expect("重开后应当保留同一个剪辑");
        assert_eq!(clip.frame_rate, 12.0);
        assert!(!clip.looped);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_ui_adjustments_persist_after_save_and_reopen() {
        let (mut app, root) = sample_app_with_menu_scene("ui-adjust");
        select_entity(&mut app, SceneEntityId(3));
        // `Text` 已迁到 schema 驱动的通用字段编辑器（第 25 轮）：这里按**生产路径**
        // （typed 命令）提交字段，`UiLayout` 仍走它自己的缓冲面板。
        for (field, value) in [("text", "Production Sample Title"), ("font_size", "33.6")] {
            assert_eq!(
                app.dispatch(editor::EditorCommand::SetComponentField {
                    entity: SceneEntityId(3),
                    component: "Text".to_owned(),
                    field: field.to_owned(),
                    value: value.to_owned(),
                }),
                editor::CommandResult::Applied,
                "Text.{field} 必须能通过 typed 命令提交"
            );
        }
        // `UiLayout` 也已迁到 schema 字段编辑器（第 31 轮）：4 个 anchors + 4 个 offsets
        // 走 typed 命令，`apply_inspector` 不再参与（它只负责剩下的缓冲面板）。
        assert_eq!(
            app.dispatch(editor::EditorCommand::SetComponentFields {
                entity: SceneEntityId(3),
                component: "UiLayout".to_owned(),
                fields: vec![
                    ("anchors".to_owned(), "1,0,1,0".to_owned()),
                    ("offsets".to_owned(), "-48,24,-48,24".to_owned()),
                ],
            }),
            editor::CommandResult::Applied,
            "UiLayout 的锚点/偏移必须能通过 typed 命令提交"
        );

        app.save_scene();
        app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
        let reopened = app.document.current_scene.as_ref().unwrap();
        let title = UiText::from_component_str(
            &reopened.scene.entities[&SceneEntityId(3)].components["Text"],
        )
        .unwrap();
        assert_eq!(title.text, "Production Sample Title");
        assert_eq!(title.font_size, 33.6);
        let layout = rustlua_ui::ui_layout::UiRectLayout::from_component_str(
            &reopened.scene.entities[&SceneEntityId(3)].components["UiLayout"],
        )
        .unwrap();
        assert_eq!(layout.anchors, [1.0, 0.0, 1.0, 0.0]);
        assert_eq!(layout.offsets, [-48.0, 24.0, -48.0, 24.0]);
        let _ = fs::remove_dir_all(root);
    }
}
