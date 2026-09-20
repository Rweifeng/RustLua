//! 行3 属性面板（Inspector）：实体层级/命名、组件管理、Transform/Camera/Light/Collider 编辑。

use crate::ComponentClipboard;
use crate::EditorApp;
use crate::util::strip_verbatim_prefix;
use editor::{
    Camera3DFields, Collider3dFields, CommandKind, InspectorError,
    TextFields, Transform3DFields, TransformFields, TransformSpace,
};
use eframe::egui;
use engine::core::{
    camera::{Camera3D, Projection},
    math3d::Vec3,
    prefab::Prefab,
    scene::{Scene, SceneEntity, SceneEntityId},
    transform::Transform3D,
    ui_animation::{UiEasing, UiKeyframe},
};
use std::{collections::BTreeSet, fs, path::Path, time::SystemTime};

struct ComponentDescriptor {
    name: &'static str,
    /// E13：Add Component 菜单按类别分组显示。
    category: ComponentCategory,
}

/// E13：组件类别（Add Component 分组 + 未来按类别过滤的扩展点）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ComponentCategory {
    Gameplay,
    Rendering,
    ThreeD,
    Ui,
}

impl ComponentCategory {
    const fn label(self) -> &'static str {
        match self {
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
    ComponentDescriptor {
        name: "Sprite",
        category: ComponentCategory::Rendering,
    },
    ComponentDescriptor {
        name: "Tilemap",
        category: ComponentCategory::Gameplay,
    },
    ComponentDescriptor {
        name: "Animation",
        category: ComponentCategory::Gameplay,
    },
    ComponentDescriptor {
        name: "Transform3D",
        category: ComponentCategory::ThreeD,
    },
    ComponentDescriptor {
        name: "MeshRenderer",
        category: ComponentCategory::ThreeD,
    },
    ComponentDescriptor {
        name: "Camera3D",
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
    [
        ComponentCategory::Gameplay,
        ComponentCategory::Rendering,
        ComponentCategory::ThreeD,
        ComponentCategory::Ui,
    ]
    .into_iter()
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
    stamp: Option<(u64, Option<SystemTime>)>,
    prefab: Prefab,
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
                                    engine::core::ui_layout::CanvasScaleMode::None,
                                    engine::core::ui_layout::CanvasScaleMode::FitWidth,
                                    engine::core::ui_layout::CanvasScaleMode::FitHeight,
                                    engine::core::ui_layout::CanvasScaleMode::FitBoth,
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
                // 持久实体：运行时切场景不销毁。
                {
                    let mut persistent = self
                        .current_scene()
                        .and_then(|scene| {
                            scene.scene.entities.get(&selected).map(|entity| {
                                entity
                                    .metadata
                                    .get("persistent")
                                    .is_some_and(|value| value == "true")
                            })
                        })
                        .unwrap_or(false);
                    if ui
                        .checkbox(&mut persistent, "运行时切场景不销毁（持久实体）")
                        .changed()
                    {
                        let inspector = self.inspector.clone();
                        if let Some(scene) = self.document.current_scene.as_mut() {
                            let _ = self.history.execute(
                                scene,
                                "Toggle Persistent",
                                CommandKind::Property,
                                |scene| {
                                    inspector.set_persistent(scene, selected, persistent)?;
                                    Ok::<(), InspectorError>(())
                                },
                            );
                        }
                    }
                }
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
                            for category in [
                                ComponentCategory::Gameplay,
                                ComponentCategory::Rendering,
                                ComponentCategory::ThreeD,
                                ComponentCategory::Ui,
                            ] {
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
                let transform_mixed = selected_ids.len() > 1
                    && self
                        .current_scene()
                        .is_some_and(|scene| has_mixed_transform(scene, &selected_ids));
                let transform3d_mixed = selected_ids.len() > 1
                    && self
                        .current_scene()
                        .is_some_and(|scene| has_mixed_transform3d(scene, &selected_ids));
                // 只显示实体实际拥有的组件编辑器（空 2D 实体只有 Transform，3D 只有 Transform3D）。
                let has = |name: &str| components.iter().any(|component| component == name);
                // WP5/WP9：Camera 面板的**只读摘要**——投影来自统一的 `Projection`，`pose=transform`
                // 时给出**生效**姿态（或"回退到组件姿态"的警告）。两边都用引擎自己的解析器读组件串
                // 原文，因此面板显示的就是渲染器真正会用的值，而不是编辑器另算一遍。
                let camera_summary = self.camera3d_panel_summary();
                if !components.is_empty() {
                    ui.label("组件：");
                    for key in &components {
                        ui.horizontal(|ui| {
                            ui.label(key);
                            if ui.small_button("移除").clicked() {
                                let key = key.clone();
                                if let Err(error) = self.remove_component(&key) {
                                    self.status = error;
                                }
                            }
                        });
                    }
                }
                ui.separator();
                // WP9：**一个** Transform 面板。此前 2D 与 3D 是两个各自独立的折叠块，同一件事
                // 要在两个地方编辑；现在按实体实际拥有的空间显示，两种都有时给一个 2D/3D 模式开关，
                // 并且**始终**在标题下方写明存在哪些空间——被切走的那一种不是消失了，只是没在编辑。
                let has_2d = has("Transform");
                let has_3d = has("Transform3D");
                let transform_space =
                    TransformSpace::resolve(has_2d, has_3d, self.inspector_transform_space);
                if let Some(space) = transform_space {
                    let transform_header = if self.inspector_transform_open {
                        "变换（Transform）  v"
                    } else {
                        "变换（Transform）  >"
                    };
                    if ui.button(transform_header).clicked() {
                        self.inspector_transform_open = !self.inspector_transform_open;
                    }
                    if self.inspector_transform_open {
                        let spaces = match (has_2d, has_3d) {
                            (true, true) => "空间：2D + 3D",
                            (true, false) => "空间：2D",
                            _ => "空间：3D",
                        };
                        ui.label(spaces);
                        if TransformSpace::both_present(has_2d, has_3d) {
                            ui.horizontal(|ui| {
                                ui.label("编辑：");
                                for candidate in [TransformSpace::TwoD, TransformSpace::ThreeD] {
                                    ui.radio_value(
                                        &mut self.inspector_transform_space,
                                        candidate,
                                        candidate.label(),
                                    );
                                }
                            });
                        }
                    }
                    if self.inspector_transform_open && space == TransformSpace::TwoD {
                        ui.group(|ui| {
                            ui.label("2D");
                            ui.horizontal(|ui| {
                                ui.label(if transform_mixed {
                                    "位置（混合）"
                                } else {
                                    "位置"
                                });
                                ui.add(egui::DragValue::new(&mut self.transform.position_x));
                                ui.add(egui::DragValue::new(&mut self.transform.position_y));
                            });
                            ui.horizontal(|ui| {
                                ui.label(if transform_mixed {
                                    "旋转（混合）"
                                } else {
                                    "旋转"
                                });
                                ui.add(egui::DragValue::new(&mut self.transform.rotation));
                            });
                            ui.horizontal(|ui| {
                                ui.label(if transform_mixed {
                                    "缩放（混合）"
                                } else {
                                    "缩放"
                                });
                                let x_changed = ui
                                    .add(egui::DragValue::new(&mut self.transform.scale_x))
                                    .changed();
                                let y_changed = ui
                                    .add(egui::DragValue::new(&mut self.transform.scale_y))
                                    .changed();
                                if self.transform_lock_ratio {
                                    if x_changed {
                                        self.transform.scale_y = self.transform.scale_x;
                                    } else if y_changed {
                                        self.transform.scale_x = self.transform.scale_y;
                                    }
                                }
                                ui.checkbox(&mut self.transform_lock_ratio, "锁定比例");
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Reset").clicked() {
                                    self.transform = TransformFields::default();
                                }
                                if ui.button("Copy Components").clicked() {
                                    self.copy_selected_components();
                                }
                                if ui.button("Paste Components").clicked() {
                                    self.paste_selected_components();
                                }
                            });
                        });
                    }
                }
                if transform_space == Some(TransformSpace::ThreeD) {
                    ui.group(|ui| {
                        ui.label("3D");
                        ui.horizontal(|ui| {
                            ui.label(if transform3d_mixed {
                                "位置（混合）"
                            } else {
                                "位置"
                            });
                            ui.add(egui::DragValue::new(&mut self.transform3d.position_x));
                            ui.add(egui::DragValue::new(&mut self.transform3d.position_y));
                            ui.add(egui::DragValue::new(&mut self.transform3d.position_z));
                        });
                        ui.horizontal(|ui| {
                            ui.label(if transform3d_mixed {
                                "旋转 Q（混合）"
                            } else {
                                "旋转 Q"
                            });
                            ui.add(egui::DragValue::new(&mut self.transform3d.rotation_x));
                            ui.add(egui::DragValue::new(&mut self.transform3d.rotation_y));
                            ui.add(egui::DragValue::new(&mut self.transform3d.rotation_z));
                            ui.add(egui::DragValue::new(&mut self.transform3d.rotation_w));
                        });
                        ui.horizontal(|ui| {
                            ui.label(if transform3d_mixed {
                                "缩放（混合）"
                            } else {
                                "缩放"
                            });
                            let x_changed = ui
                                .add(egui::DragValue::new(&mut self.transform3d.scale_x))
                                .changed();
                            let y_changed = ui
                                .add(egui::DragValue::new(&mut self.transform3d.scale_y))
                                .changed();
                            let z_changed = ui
                                .add(egui::DragValue::new(&mut self.transform3d.scale_z))
                                .changed();
                            if self.transform_lock_ratio {
                                if x_changed {
                                    self.transform3d.scale_y = self.transform3d.scale_x;
                                    self.transform3d.scale_z = self.transform3d.scale_x;
                                } else if y_changed {
                                    self.transform3d.scale_x = self.transform3d.scale_y;
                                    self.transform3d.scale_z = self.transform3d.scale_y;
                                } else if z_changed {
                                    self.transform3d.scale_x = self.transform3d.scale_z;
                                    self.transform3d.scale_y = self.transform3d.scale_z;
                                }
                            }
                            ui.checkbox(&mut self.transform_lock_ratio, "锁定比例");
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Reset 3D").clicked() {
                                self.transform3d = Transform3DFields::default();
                            }
                        });
                    });
                }
                // `MeshRenderer` 的手写面板**已删除**（第 21 轮）：与 `Light` 同样由
                // `draw_schema_driven_components` 按 schema 生成（`mesh` 字段，带
                // "工程内相对路径 / .glb|.gltf / 内置哨兵" 规则提示）。
                if has("Camera3D") {
                    ui.group(|ui| {
                        ui.label("Camera3D");
                        ui.horizontal(|ui| {
                            ui.label("位置");
                            ui.add(egui::DragValue::new(&mut self.camera3d.position_x));
                            ui.add(egui::DragValue::new(&mut self.camera3d.position_y));
                            ui.add(egui::DragValue::new(&mut self.camera3d.position_z));
                        });
                        ui.horizontal(|ui| {
                            ui.label("目标");
                            ui.add(egui::DragValue::new(&mut self.camera3d.target_x));
                            ui.add(egui::DragValue::new(&mut self.camera3d.target_y));
                            ui.add(egui::DragValue::new(&mut self.camera3d.target_z));
                        });
                        ui.horizontal(|ui| {
                            ui.label("FOV");
                            ui.add(egui::DragValue::new(&mut self.camera3d.fov_degrees));
                            ui.label("近/远");
                            ui.add(egui::DragValue::new(&mut self.camera3d.near));
                            ui.add(egui::DragValue::new(&mut self.camera3d.far));
                        });
                        // WP5：姿态来源。打开后位置/目标退化为"该实体没有 Transform3D 时的
                        // 回退值"，上方向与视线来自实体的变换——所以在面板里直接说明，
                        // 并给出**实际生效**的姿态（下面的摘要）。
                        ui.checkbox(
                            &mut self.camera3d.pose_from_transform,
                            "姿态来自实体的 Transform3D（pose=transform）",
                        );
                        // docs/54 §4.2：相机启用开关。关掉后这台相机不参与选帧。
                        ui.checkbox(&mut self.camera3d.enabled, "启用（enabled）");
                        if !self.camera3d.enabled {
                            ui.label("已禁用：不参与相机选帧，渲染回退到默认相机");
                        }
                        if self.camera3d.pose_from_transform {
                            ui.label("位置/目标仅在实体没有 Transform3D 时生效");
                        }
                        if let Some(summary) = &camera_summary {
                            ui.separator();
                            ui.label(&summary.projection);
                            if let Some(pose) = &summary.pose {
                                if pose.warning {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(220, 150, 60),
                                        &pose.text,
                                    );
                                } else {
                                    ui.label(&pose.text);
                                }
                            }
                        }
                    });
                }
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
                                    && let Err(error) = self.add_collider3d_component()
                                {
                                    self.status = error;
                                }
                                if ui.button("移除 Collider3d").clicked()
                                    && let Err(error) = self.remove_collider3d_component()
                                {
                                    self.status = error;
                                }
                            });
                        });
                    }
                }
                // UI 组件：Text / Image / Button（屏幕空间）。
                if has("Text") {
                    let text_header = if self.inspector_text_open {
                        "Text  v"
                    } else {
                        "Text  >"
                    };
                    if ui.button(text_header).clicked() {
                        self.inspector_text_open = !self.inspector_text_open;
                    }
                    if self.inspector_text_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("文本");
                                ui.text_edit_singleline(&mut self.text.text);
                            });
                            ui.horizontal(|ui| {
                                ui.label("字号");
                                let mut font_size = self.text.font_size;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut font_size)
                                            .suffix("px")
                                            .speed(1.0),
                                    )
                                    .changed()
                                {
                                    self.text.font_size = font_size.max(1.0);
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("字体");
                                let selected = if self.text.font.is_empty() {
                                    "默认字体（不指定）".to_owned()
                                } else {
                                    self.text.font.clone()
                                };
                                egui::ComboBox::from_id_salt("inspector_text_font")
                                    .selected_text(selected)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.text.font,
                                            String::new(),
                                            "默认字体（不指定）",
                                        );
                                        for path in self.project_font_paths() {
                                            ui.selectable_value(
                                                &mut self.text.font,
                                                path.clone(),
                                                path,
                                            );
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label("颜色");
                                ui.add(
                                    egui::DragValue::new(&mut self.text.color_r).range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.text.color_g).range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.text.color_b).range(0.0..=1.0),
                                );
                            });
                            if ui.button("移除 Text").clicked()
                                && let Err(error) = self.remove_component("Text")
                            {
                                self.status = error;
                            }
                        });
                    }
                }
                // `WorldText` 的手写面板**已删除**（第 23 轮）：文本/字号/颜色/偏移/居中由
                // schema 生成（颜色=取色器、偏移=两个 DragValue、居中=勾选框），
                // **字体字段用项目字体下拉**（`ChoiceSource::ProjectFonts`）。
                // `Image` 的手写面板**已删除**（第 22 轮）：纹理 = 文本框、颜色 = **取色器**、
                // 九宫格四边 = 4 个 DragValue（非负规则由 schema 表达式），全部由
                // `draw_schema_driven_components` 生成。
                if has("Sprite") {
                    let sprite_header = if self.inspector_sprite_open {
                        "Sprite  v"
                    } else {
                        "Sprite  >"
                    };
                    if ui.button(sprite_header).clicked() {
                        self.inspector_sprite_open = !self.inspector_sprite_open;
                    }
                    if self.inspector_sprite_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("纹理");
                                ui.text_edit_singleline(&mut self.sprite_component.texture);
                            });
                            ui.horizontal(|ui| {
                                ui.label("尺寸");
                                ui.add(egui::DragValue::new(&mut self.sprite_component.size.x));
                                ui.add(egui::DragValue::new(&mut self.sprite_component.size.y));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Pivot");
                                ui.add(
                                    egui::DragValue::new(&mut self.sprite_component.pivot.x)
                                        .range(0.0..=1.0),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.sprite_component.pivot.y)
                                        .range(0.0..=1.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("颜色");
                                for channel in &mut self.sprite_component.tint {
                                    ui.add(
                                        egui::DragValue::new(channel).range(0.0..=1.0).speed(0.01),
                                    );
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("排序层");
                                egui::ComboBox::from_id_salt("sprite_sorting_layer")
                                    .selected_text(&self.sprite_component.sorting.layer)
                                    .show_ui(ui, |ui| {
                                        for layer in [
                                            "background",
                                            "world",
                                            "default",
                                            "actor",
                                            "foreground",
                                            "ui",
                                        ] {
                                            ui.selectable_value(
                                                &mut self.sprite_component.sorting.layer,
                                                layer.to_owned(),
                                                layer,
                                            );
                                        }
                                    });
                                ui.label("Order");
                                ui.add(egui::DragValue::new(
                                    &mut self.sprite_component.sorting.order,
                                ));
                            });
                            ui.horizontal(|ui| {
                                ui.label("翻转");
                                ui.checkbox(&mut self.sprite_component.flip_x, "水平");
                                ui.checkbox(&mut self.sprite_component.flip_y, "垂直");
                            });
                            if ui.button("移除 Sprite").clicked()
                                && let Err(error) = self.remove_component("Sprite")
                            {
                                self.status = error;
                            }
                            self.sprite_reference = self.sprite_component.to_component_str();
                        });
                    }
                }
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
                            ui.horizontal(|ui| {
                                ui.label("画笔");
                                ui.add(egui::DragValue::new(&mut self.tilemap.brush).range(0..=6));
                                ui.checkbox(&mut self.tilemap.paint_collision, "画碰撞层");
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
                                                let label = if self.tilemap.paint_collision {
                                                    if solid { "■" } else { "·" }
                                                } else if value == 0 {
                                                    "·"
                                                } else {
                                                    match value {
                                                        1 => "1",
                                                        2 => "2",
                                                        3 => "3",
                                                        4 => "4",
                                                        5 => "5",
                                                        _ => "6",
                                                    }
                                                };
                                                if ui
                                                    .add_sized(
                                                        [18.0, 18.0],
                                                        egui::Button::new(label),
                                                    )
                                                    .clicked()
                                                {
                                                    if self.tilemap.paint_collision {
                                                        self.tilemap.map.solid[index] =
                                                            !self.tilemap.map.solid[index];
                                                    } else {
                                                        self.tilemap.map.tiles[index] =
                                                            self.tilemap.brush.min(6);
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
                                && let Err(error) = self.remove_component("Tilemap")
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
                                        engine::core::animation::AnimationClip {
                                            name: "idle".to_owned(),
                                            sheet: self.sprite_component.texture.clone(),
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
                                    engine::core::animation::AnimationClip {
                                        name: format!(
                                            "clip{}",
                                            self.animation.machine.clips.len() + 1
                                        ),
                                        sheet: self.sprite_component.texture.clone(),
                                        cols: 1,
                                        frame_rate: 8.0,
                                        looped: true,
                                        events: Vec::new(),
                                    },
                                );
                            }
                            ui.label("动画数据通过 Inspector 编辑，运行时仍由 Lua 控制切换。");
                            if ui.button("移除 Animation").clicked()
                                && let Err(error) = self.remove_component("Animation")
                            {
                                self.status = error;
                            }
                        });
                    }
                }
                // E16：UiLayout（四边 Anchor+Offset）编辑组。存在时运行时以
                // 该布局优先解析 Text/Image 位置（视觉属性仍在原组件上）。
                if has("UiLayout") {
                    let header = if self.inspector_ui_layout_open {
                        "UiLayout  v"
                    } else {
                        "UiLayout  >"
                    };
                    if ui.button(header).clicked() {
                        self.inspector_ui_layout_open = !self.inspector_ui_layout_open;
                    }
                    if self.inspector_ui_layout_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("锚点预设");
                                let current = self.ui_layout.to_layout();
                                let matched = engine::core::ui_layout::UiLayoutPreset::ALL
                                    .into_iter()
                                    .find(|preset| preset.anchors() == current.anchors);
                                let selected_label = matched
                                    .map_or("自定义".to_owned(), |preset| {
                                        preset.label().to_owned()
                                    });
                                egui::ComboBox::from_id_salt("ui_layout_preset")
                                    .selected_text(selected_label)
                                    .show_ui(ui, |ui| {
                                        for preset in engine::core::ui_layout::UiLayoutPreset::ALL {
                                            let chosen = ui
                                                .selectable_label(
                                                    matched == Some(preset),
                                                    preset.label(),
                                                )
                                                .clicked();
                                            if chosen {
                                                let mut layout = current.clone();
                                                layout.apply_preset(preset);
                                                self.ui_layout =
                                                    editor::UiLayoutFields::from_layout(layout);
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label("Anchor LTRB");
                                for anchor in [
                                    &mut self.ui_layout.anchor_l,
                                    &mut self.ui_layout.anchor_t,
                                    &mut self.ui_layout.anchor_r,
                                    &mut self.ui_layout.anchor_b,
                                ] {
                                    ui.add(
                                        egui::DragValue::new(anchor).range(0.0..=1.0).speed(0.01),
                                    );
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Offset LTRB");
                                for offset in [
                                    &mut self.ui_layout.offset_l,
                                    &mut self.ui_layout.offset_t,
                                    &mut self.ui_layout.offset_r,
                                    &mut self.ui_layout.offset_b,
                                ] {
                                    ui.add(egui::DragValue::new(offset));
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Min/Max");
                                ui.add(egui::DragValue::new(&mut self.ui_layout.min_w));
                                ui.add(egui::DragValue::new(&mut self.ui_layout.min_h));
                                ui.add(egui::DragValue::new(&mut self.ui_layout.max_w));
                                ui.add(egui::DragValue::new(&mut self.ui_layout.max_h));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Pivot");
                                ui.add(
                                    egui::DragValue::new(&mut self.ui_layout.pivot_x)
                                        .range(0.0..=1.0)
                                        .speed(0.01),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.ui_layout.pivot_y)
                                        .range(0.0..=1.0)
                                        .speed(0.01),
                                );
                            });
                            if ui.button("移除 UiLayout").clicked()
                                && let Err(error) = self.remove_component("UiLayout")
                            {
                                self.status = error;
                            }
                        });
                    }
                }
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
                        && let Err(error) = self.remove_component("Container")
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
                ui.separator();
                if ui.button("添加子实体").clicked() {
                    self.add_entity(Some(selected));
                }
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
                    .unwrap_or(engine::core::ui_events::UiEventType::Click);
                if egui::ComboBox::from_id_salt(format!("ui_event_type_{index}"))
                    .selected_text(event.as_str())
                    .show_ui(ui, |ui| {
                        for candidate in [
                            engine::core::ui_events::UiEventType::Click,
                            engine::core::ui_events::UiEventType::PointerEnter,
                            engine::core::ui_events::UiEventType::PointerExit,
                            engine::core::ui_events::UiEventType::PointerDown,
                            engine::core::ui_events::UiEventType::PointerUp,
                            engine::core::ui_events::UiEventType::Focus,
                            engine::core::ui_events::UiEventType::Submit,
                            engine::core::ui_events::UiEventType::ValueChanged,
                            engine::core::ui_events::UiEventType::Drag,
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
                    .push(engine::core::ui_events::UiEventBinding {
                        event: Some(engine::core::ui_events::UiEventType::Click),
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
                    .unwrap_or(engine::core::ui_binding::BoundProperty::TextText);
                if egui::ComboBox::from_id_salt(format!("binding_property_{index}"))
                    .selected_text(property.as_str())
                    .show_ui(ui, |ui| {
                        for candidate in [
                            engine::core::ui_binding::BoundProperty::TextText,
                            engine::core::ui_binding::BoundProperty::ImageVisible,
                            engine::core::ui_binding::BoundProperty::ProgressValue,
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
                    && let Ok(path) = engine::core::ui_binding::BindingPath::parse(&source)
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
                    .push(engine::core::ui_binding::PropertyBinding {
                        property: Some(engine::core::ui_binding::BoundProperty::TextText),
                        target: String::new(),
                        source: engine::core::ui_binding::BindingPath(vec![
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
                let track = engine::core::ui_animation::UiAnimationTrack::new(
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

    /// 为选中实体添加 Collider3d 组件（用当前字段编码，经 history 可撤销）。
    fn add_collider3d_component(&mut self) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let Some(selected) = self.selection.primary() else {
            return Ok(());
        };
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
                inspector.set_collider3d(scene, selected, collider3d)?;
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// 移除选中实体的 Collider3d 组件（经 history 可撤销）。
    fn remove_collider3d_component(&mut self) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let Some(selected) = self.selection.primary() else {
            return Ok(());
        };
        let inspector = self.inspector.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            "Remove Collider3d",
            CommandKind::Property,
            move |scene| {
                inspector.clear_collider3d(scene, selected)?;
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
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

    /// Camera 面板的只读摘要（WP5/WP9：按统一 `Projection` 展示，并给出**生效**姿态）。
    ///
    /// 两条信息都用引擎自己的解析器从组件串原文得到：`Projection::from_camera3d` 是"这台相机
    /// 怎么投影"的唯一说法，`Camera3D::with_pose` 与运行时用的是同一条姿态换算。因此面板里
    /// 写的不是编辑器的推测，而是渲染器真正会用的值。
    fn camera3d_panel_summary(&self) -> Option<CameraPanelSummary> {
        camera_panel_summary(&self.current_scene()?.scene, self.selection.primary()?)
    }

    /// 为选中实体添加组件（默认值，经 history 可撤销）。
    pub(crate) fn add_component(&mut self, name: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if !COMPONENT_REGISTRY
            .iter()
            .any(|descriptor| descriptor.name == name)
        {
            return Err(format!("未注册组件：{name}"));
        }
        let Some(selected) = self.selection.primary() else {
            return Ok(());
        };
        let Some(scene) = self.document.current_scene.as_ref() else {
            return Ok(());
        };
        if scene.scene.entities[&selected]
            .components
            .contains_key(name)
        {
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
            move |scene| {
                match name.as_str() {
                    "Sprite" => inspector.set_sprite_reference(
                        scene,
                        selected,
                        engine::core::sprite::SpriteComponent {
                            texture: "assets/textures/player.png".to_owned(),
                            size: engine::core::transform::Vec2::new(128.0, 128.0),
                            pivot: engine::core::transform::Vec2::new(0.5, 0.5),
                            tint: [1.0; 4],
                            sorting: engine::core::sprite::SpriteSorting::default(),

                            flip_x: false,
                            flip_y: false,
                            z: 0.0,
                        }
                        .to_component_str(),
                    )?,
                    "Tilemap" => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(
                                "Tilemap".to_owned(),
                                engine::core::tilemap::Tilemap::new(
                                    engine::core::transform::Vec2::new(32.0, 32.0),
                                    8,
                                    8,
                                )
                                .map_err(|error| {
                                    InspectorError::InvalidAssetReference(error.to_string())
                                })?
                                .to_component_str(),
                            );
                            scene.mark_dirty();
                        }
                    }
                    "Animation" => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(
                                "Animation".to_owned(),
                                engine::core::animation::AnimationStateMachine::new(
                                    "idle",
                                    vec![engine::core::animation::AnimationClip {
                                        name: "idle".to_owned(),
                                        sheet: "assets/textures/player.png".to_owned(),
                                        cols: 1,
                                        frame_rate: 1.0,
                                        looped: true,
                                        events: Vec::new(),
                                    }],
                                )
                                .map_err(|error| {
                                    InspectorError::InvalidAssetReference(error.to_string())
                                })?
                                .to_component_str(),
                            );
                            scene.mark_dirty();
                        }
                    }
                    "Transform3D" => {
                        inspector.set_transform3d(scene, selected, Transform3DFields::default())?
                    }
                    "Camera3D" => {
                        inspector.set_camera3d(scene, selected, Camera3DFields::default())?
                    }
                    // Light 没有手写面板了，但"添加组件"仍要能加：默认值直接取**引擎类型**
                    // 的编码（不在这里手写字面串），之后由通用字段编辑器接管编辑。
                    "Light" => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(
                                "Light".to_owned(),
                                engine::core::light::Light::default().to_component_str(),
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
                                engine::core::mesh3d::MeshRenderer {
                                    mesh: engine::core::mesh3d::GROUND_PLANE_SENTINEL.to_owned(),
                                }
                                .to_component_str(),
                            );
                        }
                        scene.mark_dirty();
                    }
                    "Text" => inspector.set_text(scene, selected, TextFields::default())?,
                    // WorldText 没有手写面板了：默认值取引擎类型的编码
                    // （`text=文字;font_size=…;offset=0,-20;centered=true`）。
                    "WorldText" => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(
                                "WorldText".to_owned(),
                                engine::core::world_text::WorldText {
                                    text: engine::core::ui::UiText {
                                        text: "文字".to_owned(),
                                        font_size: engine::core::ui::UI_TEXT_BASE_PX,
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
                                engine::core::ui::UiImage {
                                    texture: String::new(),
                                    color: [1.0, 1.0, 1.0],
                                    nine_slice: None,
                                }
                                .to_component_str(),
                            );
                        }
                        scene.mark_dirty();
                    }
                    // E16：UiLayout 默认为中心点锚 200×100 元素，由布局统一
                    // 承担所有 UI 位置与尺寸。
                    "UiLayout" => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert("UiLayout".to_owned(), {
                                let mut layout = engine::core::ui_layout::UiRectLayout::default();
                                layout
                                    .apply_preset(engine::core::ui_layout::UiLayoutPreset::Center);
                                layout.offsets = [-100.0, -50.0, 100.0, 50.0];
                                layout.to_component_str()
                            });
                            scene.mark_dirty();
                        }
                    }
                    // E17：Container 默认 VBox 无内边距。
                    "Container" => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(
                                "Container".to_owned(),
                                engine::core::ui_layout::ContainerStyle::default()
                                    .to_component_str(),
                            );
                            scene.mark_dirty();
                        }
                    }
                    "UiEvents" => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(
                                "UiEvents".to_owned(),
                                engine::core::ui_events::UiEvents::click("on_click")
                                    .to_component_str(),
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
                    other => {
                        if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                            entity.components.insert(other.to_owned(), String::new());
                            scene.mark_dirty();
                        }
                    }
                }
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => {
                self.sync_inspector(selected);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// 移除选中实体的组件（经 history 可撤销）。
    fn remove_component(&mut self, name: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let Some(selected) = self.selection.primary() else {
            return Ok(());
        };
        let name = name.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result = self.history.execute(
            scene,
            format!("Remove {name}"),
            CommandKind::Property,
            move |scene| {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    entity.components.remove(&name);
                    scene.mark_dirty();
                }
                Ok::<(), InspectorError>(())
            },
        );
        match result {
            Ok(_) => {
                self.sync_inspector(selected);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

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
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "Rename entity", CommandKind::Rename, move |scene| {
                    tree.rename(scene, selected, name)
                        .map_err(|e| e.to_string())
                });
        match result {
            Ok(true) => {
                self.status = "Entity renamed".into();
                self.on_document_changed(crate::actions::DocumentChange::EntityRenamed);
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

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
                self.on_document_changed(crate::actions::DocumentChange::EntityReparented);
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
                self.on_document_changed(crate::actions::DocumentChange::EntityDeleted);
                self.status = "Entity deleted".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
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

    /// 粘贴编辑器剪贴板里的组件到选中实体（菜单「编辑 ▸ 粘贴组件」）。
    pub(crate) fn paste_selected_components(&mut self) {
        if self.is_playing() {
            return;
        }
        let Some(id) = self.selection.primary() else {
            return;
        };
        let Some(clipboard) = self.inspector_clipboard.clone() else {
            self.status = "Component clipboard is empty".to_owned();
            return;
        };
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ =
                self.history
                    .execute(scene, "Paste Components", CommandKind::Property, |scene| {
                        let entity = scene.scene.entities.get_mut(&id).ok_or(())?;
                        entity.components = clipboard.components.clone();
                        entity.resources = clipboard.resources.clone();
                        entity.metadata = clipboard.metadata.clone();
                        scene.mark_dirty();
                        Ok::<(), ()>(())
                    });
        }
        self.sync_inspector(id);
        self.status = "Components pasted".to_owned();
    }

    /// 确保源 Prefab 已缓存（必要时重新读盘）。返回后不再持有 `&mut self` 借用，
    /// 调用方随后从 `prefab_source_cache` 取不可变引用即可。
    fn ensure_prefab_source_cached(&mut self, path: &Path) -> Result<(), String> {
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
                self.on_document_changed(crate::actions::DocumentChange::PrefabApply);
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
                    self.on_document_changed(crate::actions::DocumentChange::PrefabApply);
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
                    self.on_document_changed(crate::actions::DocumentChange::PrefabApply);
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
                    self.on_document_changed(crate::actions::DocumentChange::PrefabApply);
                }
                self.status = format!("已把字段应用到源 Prefab 并更新全部实例：{field_label}");
            }
            Err(error) => self.status = format!("应用覆盖失败：{error}"),
        }
    }

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
    fn prefab_instance_source(&self, selected: SceneEntityId) -> Option<(String, SceneEntityId)> {
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
    fn prefab_override_instance_value(
        &self,
        selected: SceneEntityId,
        field: &str,
    ) -> Option<String> {
        let scene = self.document.current_scene.as_ref()?;
        let entity = scene.scene.entities.get(&selected)?;
        Some(prefab_override_side_value(entity, field))
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
                self.on_document_changed(crate::actions::DocumentChange::PrefabApply);
                self.status = "Prefab 实例已解包为普通实体".to_owned();
            }
            Err(error) => self.status = format!("Unpack 失败：{error}"),
        }
    }
}

/// Camera 面板的只读摘要（WP5/WP9）。
struct CameraPanelSummary {
    /// 统一 `Projection` 的说法（透视/正交 + 参数 + 深度与前向约定）。
    projection: String,
    /// `pose=transform` 时的生效姿态；`warning` 为真表示渲染会回退到组件姿态。
    pose: Option<CameraPanelPose>,
}

/// Camera 面板里的一行姿态说明。
struct CameraPanelPose {
    text: String,
    warning: bool,
}

/// Camera 面板的只读摘要（纯函数，便于单测）。
///
/// 投影来自统一 [`Projection`]，`pose=transform` 时给出**生效**姿态——用的都是引擎自己的
/// 解析器与姿态换算，因此面板显示的就是渲染器真正会用的值。
fn camera_panel_summary(scene: &Scene, id: SceneEntityId) -> Option<CameraPanelSummary> {
    let entity = scene.entities.get(&id)?;
    let raw = entity.components.get("Camera3D")?;
    let camera = match Camera3D::from_component_str(raw) {
        Ok(camera) => camera,
        Err(error) => {
            return Some(CameraPanelSummary {
                projection: format!("组件串无法解析：{error}"),
                pose: None,
            });
        }
    };
    let projection = match Projection::from_camera3d(camera) {
        Projection::Perspective { fov_y, near, far } => format!(
            "投影：透视（垂直 FOV {:.1}°，近 {near:.3}，远 {far:.3}；深度 0..1，前向 -Z）",
            fov_y.to_degrees()
        ),
        Projection::Orthographic { zoom } => format!("投影：正交（{zoom:.3} px/单位）"),
    };
    // docs/54 §4.2：`enabled=false` 的相机不参与选帧。面板必须说明这一点，否则用户会看到
    // 一台"参数正常"的相机却完全不参与渲染。
    let projection = if camera.enabled {
        projection
    } else {
        format!("已禁用（enabled=false）—— 不参与相机选帧，渲染回退到默认相机。{projection}")
    };
    if !camera.pose_from_transform {
        return Some(CameraPanelSummary {
            projection,
            pose: None,
        });
    }
    let pose = match entity.components.get("Transform3D") {
        None => CameraPanelPose {
            text: "警告：实体没有 Transform3D —— 渲染回退到上面的组件姿态".to_owned(),
            warning: true,
        },
        Some(raw) => match Transform3D::from_component_str(raw)
            .map_err(|error| error.to_string())
            .and_then(|pose| camera.with_pose(pose).map_err(|error| error.to_string()))
        {
            Ok(posed) => {
                let forward = (posed.target - posed.position)
                    .normalized()
                    .unwrap_or(Vec3::new(0.0, 0.0, -1.0));
                CameraPanelPose {
                    text: format!(
                        "生效姿态（来自 Transform3D）：位置 ({:.3}, {:.3}, {:.3})，视线 -Z → ({:.3}, {:.3}, {:.3})",
                        posed.position.x,
                        posed.position.y,
                        posed.position.z,
                        forward.x,
                        forward.y,
                        forward.z
                    ),
                    warning: false,
                }
            }
            Err(error) => CameraPanelPose {
                text: format!("警告：Transform3D 不可用（{error}）—— 渲染回退到组件姿态"),
                warning: true,
            },
        },
    };
    Some(CameraPanelSummary {
        projection,
        pose: Some(pose),
    })
}

fn has_mixed_transform(scene: &editor::SceneDocument, ids: &[SceneEntityId]) -> bool {
    let mut transforms = ids.iter().filter_map(|id| {
        scene
            .scene
            .entities
            .get(id)
            .and_then(|entity| entity.components.get("Transform"))
            .and_then(|value| crate::geometry::parse_transform(value))
    });
    let Some(first) = transforms.next() else {
        return false;
    };
    transforms.any(|transform| transform != first)
}

fn has_mixed_transform3d(scene: &editor::SceneDocument, ids: &[SceneEntityId]) -> bool {
    let mut transforms = ids.iter().filter_map(|id| {
        scene
            .scene
            .entities
            .get(id)
            .and_then(|entity| entity.components.get("Transform3D"))
            .and_then(|value| Transform3D::from_component_str(value).ok())
    });
    let Some(first) = transforms.next() else {
        return false;
    };
    transforms.any(|transform| transform != first)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PrefabRebuildState {
    instance_root: SceneEntityId,
    parent: Option<SceneEntityId>,
    instance_entities: Vec<SceneEntityId>,
    source_to_instance: std::collections::BTreeMap<SceneEntityId, SceneEntityId>,
    overrides: std::collections::BTreeMap<(SceneEntityId, String), String>,
}

fn collect_prefab_rebuild_state(
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

fn build_prefab_from_instance_root(
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

fn apply_prefab_instance_metadata(
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
        let Ok(path) = engine::core::property_path::PropertyPath::parse(raw_path) else {
            return "（非法路径）".to_owned();
        };
        let Some(payload) = entity.components.get(&path.component) else {
            return "（缺失）".to_owned();
        };
        return engine::core::property_path::read_field_text(payload, &path)
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

fn diff_prefab_entity(source: &SceneEntity, instance: &SceneEntity) -> Vec<String> {
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
    let registry = engine::core::component_schema::builtin_registry();
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
    use crate::EditorApp;
    use editor::open_project;
    use engine::core::{
        animation::AnimationStateMachine,
        logging::LogSink,
        prefab::Prefab,
        resource_db::AssetId,
        scene::{Scene, SceneEntity, SceneEntityId},
        ui::UiText,
    };
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

    fn sample_app_with_production_scene(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_sample_project(label);
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));
        (app, root)
    }

    fn sample_app_with_menu_scene(label: &str) -> (EditorApp, PathBuf) {
        let root = copy_sample_project(label);
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
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

    /// WP5/WP9：Camera 面板按统一 `Projection` 展示，并在 `pose=transform` 时给出**生效**姿态
    /// （而不是只显示"实际不生效"的组件值）——缺 `Transform3D` 时必须显示会回退的警告。
    #[test]
    fn camera_panel_summary_shows_projection_and_effective_pose() {
        let mut scene = Scene::new("camera-panel");
        let camera = SceneEntityId(1);
        scene.add_entity(camera, "Camera", None).unwrap();
        let components = &mut scene.entities.get_mut(&camera).unwrap().components;
        components.insert(
            "Camera3D".into(),
            "position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
        );

        // 未声明 pose=transform：只有投影一行，没有姿态说明（组件值就是渲染值）。
        let summary = camera_panel_summary(&scene, camera).expect("summary");
        assert!(
            summary.projection.contains("透视") && summary.projection.contains("60.0°"),
            "投影行应给出统一 Projection 的参数：{}",
            summary.projection
        );
        assert!(summary.pose.is_none());

        // 声明 pose=transform 但没有 Transform3D：必须给出"回退到组件姿态"的警告。
        scene.entities.get_mut(&camera).unwrap().components.insert(
            "Camera3D".into(),
            "position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=transform".into(),
        );
        let missing = camera_panel_summary(&scene, camera).expect("summary");
        let pose = missing.pose.expect("pose line");
        assert!(pose.warning, "缺 Transform3D 时应是警告：{}", pose.text);
        assert!(pose.text.contains("没有 Transform3D"), "{}", pose.text);

        // 补上绕 Y 转 90° 的 Transform3D：生效姿态必须显示变换后的位置与视线（-X）。
        scene.entities.get_mut(&camera).unwrap().components.insert(
            "Transform3D".into(),
            "position=20,5,0;rotation=0,0.7071068,0,0.7071068;scale=1,1,1".into(),
        );
        let posed = camera_panel_summary(&scene, camera).expect("summary");
        let pose = posed.pose.expect("pose line");
        assert!(!pose.warning, "{}", pose.text);
        assert!(
            pose.text.contains("20.000") && pose.text.contains("-1.000"),
            "生效姿态应来自 Transform3D 且视线为 -X：{}",
            pose.text
        );

        // 非法的 Transform3D 串同样是警告（不静默当作可用）。
        scene
            .entities
            .get_mut(&camera)
            .unwrap()
            .components
            .insert("Transform3D".into(), "position=oops".into());
        let broken = camera_panel_summary(&scene, camera).expect("summary");
        assert!(broken.pose.expect("pose line").warning);
    }

    fn entity() -> SceneEntity {
        SceneEntity {
            id: SceneEntityId(1),
            name: "Enemy".to_owned(),
            parent: None,
            children: Vec::new(),
            components: std::collections::BTreeMap::from([
                ("Transform".to_owned(), "x=0;y=0".to_owned()),
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
        instance
            .components
            .insert("Transform".to_owned(), "x=10;y=4".to_owned());
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
                "component:Transform".to_owned(),
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
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".to_owned(),
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
            "x=9;y=4;rotation=0;scale_x=1;scale_y=1".to_owned(),
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
        assert_eq!(
            rebuild
                .overrides
                .get(&(SceneEntityId(2), "property:Transform.x".to_owned())),
            Some(&"9".to_owned())
        );
        assert_eq!(
            rebuild
                .overrides
                .get(&(SceneEntityId(2), "property:Transform.y".to_owned())),
            Some(&"4".to_owned())
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
                "x=77;y=19;rotation=0;scale_x=1;scale_y=1".to_owned(),
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
        let registry = engine::core::component_schema::builtin_registry();
        let read = |field: &str| {
            let path =
                engine::core::property_path::PropertyPath::parse(&format!("Transform.{field}"))
                    .expect("path");
            engine::core::property_path::read_field(&registry, &payload, &path).expect("field")
        };
        assert_eq!(
            read("x"),
            engine::core::component_schema::FieldValue::Number(77.0)
        );
        assert_eq!(
            read("y"),
            engine::core::component_schema::FieldValue::Number(19.0)
        );
        assert_eq!(
            read("scale_x"),
            engine::core::component_schema::FieldValue::Number(1.0)
        );
        assert!(payload.contains("x=77"), "覆盖字段保留原始文本：{payload}");
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
            let prefab = engine::core::prefab::Prefab::load(
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
                "x=123;y=456;rotation=0;scale_x=1;scale_y=1".to_owned(),
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
        let roots: Vec<&engine::core::scene::SceneEntity> = scene
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
        let reverted: Vec<&&engine::core::scene::SceneEntity> = roots
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
            let path = engine::core::property_path::PropertyPath::parse("Transform.x").unwrap();
            let updated =
                engine::core::property_path::write_builtin(&payload, &path, "999").unwrap();
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
            rows.iter().any(|row| row.field == "property:Transform.x"),
            "{rows:?}"
        );
        assert!(rows.iter().any(|row| row.field == "name"), "{rows:?}");

        let undo_before = app.history.undo_len();
        app.revert_prefab_override_field(target, "property:Transform.x");

        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let entity = scene.entities.get(&target).expect("instance");
        let payload = entity.components.get("Transform").expect("Transform");
        let path = engine::core::property_path::PropertyPath::parse("Transform.x").unwrap();
        let registry = engine::core::component_schema::builtin_registry();
        let source_x = {
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
            engine::core::property_path::read_field(&registry, source_payload, &path)
                .expect("source x")
        };
        assert_eq!(
            engine::core::property_path::read_field(&registry, payload, &path).expect("x"),
            source_x,
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
        let x = engine::core::property_path::read_field(&registry, &payload, &path).expect("x");
        assert_eq!(
            x,
            engine::core::component_schema::FieldValue::Number(999.0),
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
        // 制造两个覆盖：Transform.x=999 与名字。
        {
            let scene = app.document.current_scene.as_mut().unwrap();
            let entity = scene.scene.entities.get_mut(&target).unwrap();
            entity.name = "ApplyGreen".to_owned();
            let payload = entity
                .components
                .get("Transform")
                .cloned()
                .expect("instance has Transform");
            let path = engine::core::property_path::PropertyPath::parse("Transform.x").unwrap();
            let updated =
                engine::core::property_path::write_builtin(&payload, &path, "999").unwrap();
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
        app.apply_prefab_override_field(target, "property:Transform.x");
        assert!(
            app.status.contains("已把字段应用到源 Prefab"),
            "status: {}",
            app.status
        );

        // 1) 源 Prefab 文件里该字段已更新。
        let prefab = Prefab::load(root.join("assets/prefabs/slime_green.prefab"))
            .expect("source prefab reloads");
        let registry = engine::core::component_schema::builtin_registry();
        let path = engine::core::property_path::PropertyPath::parse("Transform.x").unwrap();
        assert_eq!(
            engine::core::property_path::read_field(
                &registry,
                prefab.entities[&source_id]
                    .components
                    .get("Transform")
                    .expect("source Transform"),
                &path
            )
            .expect("source x"),
            engine::core::component_schema::FieldValue::Number(999.0),
            "源 Prefab 文件必须被更新"
        );

        // 2) 该字段不再是覆盖（实例 == 源），名字覆盖仍在。
        let rows = app
            .prefab_override_summary(app.selection.primary().unwrap())
            .expect("summary")
            .expect("loads");
        assert!(
            !rows.iter().any(|row| row.field == "property:Transform.x"),
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

    #[test]
    fn sample_project_animation_edit_persists_after_save_and_reopen() {
        let (mut app, root) = sample_app_with_production_scene("animation-edit");
        select_entity(&mut app, SceneEntityId(2));
        app.animation.machine.default = "move".to_owned();
        let move_clip = app
            .animation
            .machine
            .clips
            .iter_mut()
            .find(|clip| clip.name == "move")
            .expect("player should have move clip");
        move_clip.frame_rate = 12.0;
        move_clip.looped = false;

        app.apply_inspector();
        app.save_scene();
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));

        let reopened = app.document.current_scene.as_ref().unwrap();
        let animation = AnimationStateMachine::from_component_str(
            &reopened.scene.entities[&SceneEntityId(2)].components["Animation"],
        )
        .unwrap();
        assert_eq!(animation.default, "move");
        let move_clip = animation
            .clips
            .iter()
            .find(|clip| clip.name == "move")
            .expect("reopened animation should keep move clip");
        assert_eq!(move_clip.frame_rate, 12.0);
        assert!(!move_clip.looped);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_ui_adjustments_persist_after_save_and_reopen() {
        let (mut app, root) = sample_app_with_menu_scene("ui-adjust");
        select_entity(&mut app, SceneEntityId(3));
        app.text.text = "Production Sample Title".to_owned();
        app.text.font_size = 33.6;
        app.ui_layout.anchor_l = 1.0;
        app.ui_layout.anchor_t = 0.0;
        app.ui_layout.anchor_r = 1.0;
        app.ui_layout.anchor_b = 0.0;
        app.ui_layout.offset_l = -48.0;
        app.ui_layout.offset_t = 24.0;
        app.ui_layout.offset_r = -48.0;
        app.ui_layout.offset_b = 24.0;
        app.apply_inspector();

        app.save_scene();
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
        let reopened = app.document.current_scene.as_ref().unwrap();
        let title = UiText::from_component_str(
            &reopened.scene.entities[&SceneEntityId(3)].components["Text"],
        )
        .unwrap();
        assert_eq!(title.text, "Production Sample Title");
        assert_eq!(title.font_size, 33.6);
        let layout = engine::core::ui_layout::UiRectLayout::from_component_str(
            &reopened.scene.entities[&SceneEntityId(3)].components["UiLayout"],
        )
        .unwrap();
        assert_eq!(layout.anchors, [1.0, 0.0, 1.0, 0.0]);
        assert_eq!(layout.offsets, [-48.0, 24.0, -48.0, 24.0]);
        let _ = fs::remove_dir_all(root);
    }
}
