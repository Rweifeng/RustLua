//! 行3 属性面板（Inspector）：实体层级/命名、组件管理、Transform/Camera/Light/Collider 编辑。

use crate::ComponentClipboard;
use crate::EditorApp;
use crate::panels::tilemap_tools;
use crate::util::{component_display_name, strip_verbatim_prefix};
use editor::{Collider3dFields, ContainerPreset, InspectorError, InspectorState};
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
    // P3-E01-2：`.anim` 时间线播放器（与状态机 `Animation`、骨骼 `Animation3D` 并列）。
    ComponentDescriptor {
        name: "AnimationPlayer",
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
        name: "LayoutItem",
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

/// Inspector Override 摘要用的源预制体 缓存条目。
pub(crate) struct CachedPrefabSource {
    /// (文件长度, mtime)。`None` = 读取元数据失败（例如文件不存在）。
    pub(crate) stamp: Option<(u64, Option<SystemTime>)>,
    pub(crate) prefab: Prefab,
}

/// 源预制体 的失效戳：长度 + mtime。任一变化即视为需要重载。
fn prefab_stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()))
}

mod helpers;
mod prefab;
#[cfg(test)]
mod tests;
mod ui_editors;

// Prefab diff / 字段取值这些纯函数住在 `helpers`（只算文本，不碰文档）。
use helpers::{
    canvas_scale_mode_label, collider3d_shape_label, compact_value, container_align_label,
    container_justify_label, container_kind_label,
};
// 外部调用点（`crate::actions`）沿用 `panels::inspector::…` 路径。
pub(crate) use helpers::{diff_prefab_entity, prefab_override_side_value};

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
                            ui.label(egui::RichText::new(entry.kind.label()).monospace());
                        });
                        let (status_text, status_color) = match entry.status {
                            rustlua_assets::resource_db::ResourceState::Ready => ("已就绪".to_owned(), egui::Color32::from_rgb(120,200,130)),
                            rustlua_assets::resource_db::ResourceState::Discovered => ("待导入".to_owned(), egui::Color32::YELLOW),
                            rustlua_assets::resource_db::ResourceState::Stale => ("已变更需重导".to_owned(), egui::Color32::YELLOW),
                            rustlua_assets::resource_db::ResourceState::Missing => ("文件缺失".to_owned(), egui::Color32::GRAY),
                            rustlua_assets::resource_db::ResourceState::ImportFailed => ("导入失败".to_owned(), egui::Color32::RED),
                            rustlua_assets::resource_db::ResourceState::Invalid => ("依赖断链".to_owned(), egui::Color32::RED),
                            _ => ("未知状态".to_owned(), ui.visuals().weak_text_color()),
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
                    ui.label("当前文档：预制体编辑模式");
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
                    ui.colored_label(egui::Color32::from_rgb(100, 190, 230), "界面根节点 / 画布");
                    ui.label("画布");
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
                            .selected_text(canvas_scale_mode_label(self.canvas_config.scale_mode))
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
                                        canvas_scale_mode_label(mode),
                                    );
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("安全区");
                        for inset in &mut self.canvas_config.safe_area {
                            ui.add(egui::DragValue::new(inset).range(0.0..=512.0));
                        }
                    });
                    if ui.button("应用画布配置").clicked() {
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
                        "预制体来源：{}{}",
                        prefab_source,
                        if is_prefab_root {
                            "（根实例）"
                        } else {
                            ""
                        }
                    ));
                    // docs/55 §4.1：面板只发 typed action（目标显式携带），不直接调用文档写入方法。
                    if is_prefab_root && ui.button("重新应用源预制体").clicked() {
                        self.dispatch(editor::EditorCommand::ReapplyPrefab { entity: selected });
                    }
                    if ui.button("打开源预制体").clicked() {
                        self.open_scene_resource(std::path::PathBuf::from(&prefab_source));
                    }
                    // P1-4：in-context 编辑（对标 Unity 的 Prefab Mode）——不打断场景，
                    // 场景文档连同未保存改动一起暂存在 shell 的 `PrefabStage` 口袋里。
                    if is_prefab_root
                        && self.prefab_stage().is_none()
                        && ui.button("在上下文中编辑").clicked()
                    {
                        self.dispatch(editor::EditorCommand::EnterPrefabStage { entity: selected });
                    }
                    if is_prefab_root && ui.button("写回源并更新实例").clicked() {
                        self.dispatch(editor::EditorCommand::SavePrefabToSource { entity: selected });
                    }
                    if is_prefab_root && ui.button("解包预制体").clicked() {
                        self.dispatch(editor::EditorCommand::UnpackPrefab { entity: selected });
                    }
                    if is_prefab_root && ui.button("还原覆盖").clicked() {
                        self.dispatch(editor::EditorCommand::RevertPrefabOverrides {
                            entity: selected,
                        });
                    }
                }
                if let Some(prefab_state) = self.prefab_override_summary(selected) {
                    match prefab_state {
                        Ok(prefab_overrides) => {
                            if prefab_overrides.is_empty() {
                                ui.label("预制体覆盖：无");
                            } else {
                                // 逐字段选择性 Apply / Revert（docs/55 阶段 3）：
                                // 每行显示「源 → 实例」，并可单独把该字段推回源预制体 或还原为源取值。
                                let mut revert_field: Option<String> = None;
                                let mut apply_field: Option<String> = None;
                                ui.collapsing(
                                    format!("预制体覆盖（{}）", prefab_overrides.len()),
                                    |ui| {
                                        for row in &prefab_overrides {
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .small_button("应用")
                                                    .on_hover_text(format!(
                                                        "把实例取值写回源预制体并更新全部实例：{}",
                                                        compact_value(&row.instance_value)
                                                    ))
                                                    .clicked()
                                                {
                                                    apply_field = Some(row.field.clone());
                                                }
                                                if ui
                                                    .small_button("回退")
                                                    .on_hover_text(format!(
                                                        "把这个字段还原为源预制体的取值：{}",
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
                                format!("预制体诊断：{error}"),
                            );
                        }
                    }
                }
                // M79 P2：**整棵实例**的覆盖一览。上面那段是"选中谁看谁"，深层子节点被改过
                // 就得逐个点开找；这里补上 Unity 在实例根上给的那一眼——按节点路径列出全部
                // 字段级覆盖，逐条可「回退」，也可一次「逐项还原全部」（一条 Undo、不重建实例）。
                if self.is_prefab_root(selected) {
                    match self.prefab_instance_overrides(selected) {
                        Some(Ok(overrides)) => {
                            if overrides.rows.is_empty() && overrides.notes.is_empty() {
                                ui.label("整个实例的覆盖：无");
                            } else {
                                let mut revert_row: Option<(SceneEntityId, String)> = None;
                                let mut focus_entity: Option<SceneEntityId> = None;
                                let mut revert_all = false;
                                ui.collapsing(
                                    format!(
                                        "整个实例的覆盖（{} 项 / {} 个节点）",
                                        overrides.rows.len(),
                                        overrides.entity_count()
                                    ),
                                    |ui| {
                                        for row in &overrides.rows {
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .small_button("回退")
                                                    .on_hover_text(format!(
                                                        "把 {}/{} 还原为源取值：{}",
                                                        row.entity_label, row.field, row.source_value
                                                    ))
                                                    .clicked()
                                                {
                                                    revert_row =
                                                        Some((row.entity, row.field.clone()));
                                                }
                                                if ui
                                                    .small_button("选中")
                                                    .on_hover_text("在层级树里选中这个节点")
                                                    .clicked()
                                                {
                                                    focus_entity = Some(row.entity);
                                                }
                                                ui.monospace(&row.entity_label);
                                                ui.weak("源=");
                                                ui.monospace(compact_value(&row.source_value));
                                                ui.weak("实例=");
                                                ui.monospace(compact_value(&row.instance_value));
                                            });
                                        }
                                        // 还原不了的那部分也要说出来，否则"列表里没有"会被读成"没覆盖"。
                                        for note in &overrides.notes {
                                            ui.weak(note.display());
                                        }
                                        if !overrides.rows.is_empty()
                                            && ui
                                                .button("逐项还原全部字段覆盖")
                                                .on_hover_text(
                                                    "一条 Undo 还原上面列出的全部字段覆盖；\
                                                     不重建实例、实体 id 不变。\n\
                                                     结构性差异（源里已无对应实体的子节点）不在范围内，\
                                                     那种请用「还原覆盖」重建。",
                                                )
                                                .clicked()
                                        {
                                            revert_all = true;
                                        }
                                    },
                                );
                                if let Some((entity, field)) = revert_row {
                                    self.dispatch(editor::EditorCommand::RevertPrefabOverride {
                                        entity,
                                        field,
                                    });
                                }
                                if let Some(entity) = focus_entity {
                                    self.dispatch(editor::EditorCommand::SelectEntity { id: entity });
                                }
                                if revert_all {
                                    self.dispatch(
                                        editor::EditorCommand::RevertAllPrefabFieldOverrides {
                                            entity: selected,
                                        },
                                    );
                                }
                            }
                        }
                        Some(Err(error)) => {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 140, 80),
                                format!("整实例覆盖扫描：{error}"),
                            );
                        }
                        None => {}
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
                    ui.label("持久化 ID");
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
                                    "持久化 ID 已清除".to_owned()
                                } else {
                                    format!("持久化 ID 已设置：{}", self.persist_id_buffer)
                                };
                            }
                            Ok(false) => {
                                self.reload_persist_id_buffer(selected);
                                self.status = "持久化 ID 无变化".to_owned();
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
                        .selected_text(component_display_name(&self.inspector_add_component))
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
                                        component_display_name(descriptor.name),
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
                            ui.label(component_display_name(key));
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
                // `Copy/粘贴组件` 按钮随面板一并删除——它们在**编辑菜单**里仍然可用
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
                        "3D 碰撞体  v"
                    } else {
                        "3D 碰撞体  >"
                    };
                    if ui.button(collider3d_header).clicked() {
                        self.inspector_collider3d_open = !self.inspector_collider3d_open;
                    }
                    if self.inspector_collider3d_open {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("形状");
                                egui::ComboBox::from_id_salt("collider3d_shape")
                                    .selected_text(collider3d_shape_label(&self.collider3d.shape))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.collider3d.shape,
                                            "ball".to_owned(),
                                            "球体",
                                        );
                                        ui.selectable_value(
                                            &mut self.collider3d.shape,
                                            "cuboid".to_owned(),
                                            "长方体",
                                        );
                                        ui.selectable_value(
                                            &mut self.collider3d.shape,
                                            "capsule".to_owned(),
                                            "胶囊",
                                        );
                                    });
                                ui.checkbox(&mut self.collider3d.sensor, "传感器");
                                ui.checkbox(&mut self.collider3d.dynamic, "动态");
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
                                if ui.button("添加 3D 碰撞体").clicked()
                                    && let Err(error) = self.add_collider3d_component(selected)
                                {
                                    self.status = error;
                                }
                                if ui.button("移除 3D 碰撞体").clicked()
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
                        "瓦片地图  v"
                    } else {
                        "瓦片地图  >"
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
                                ui.label("瓦片尺寸");
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
                                ui.checkbox(&mut self.tilemap.map.autotile, "自动瓦片（4 位）");
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
                                ui.label(format!("种子={}", self.tilemap.map.variant_seed));
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
                            if ui.button("移除瓦片地图").clicked()
                                && let Err(error) = self.remove_component(selected, "Tilemap")
                            {
                                self.status = error;
                            }
                        });
                    }
                }
                if has("Animation") {
                    let animation_header = if self.inspector_animation_open {
                        "动画状态机  v"
                    } else {
                        "动画状态机  >"
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
                                    ui.label(format!("动画片段 {}", index + 1));
                                    if ui.small_button("删除").clicked() {
                                        remove_index = Some(index);
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("名称");
                                    ui.text_edit_singleline(&mut clip.name);
                                });
                                ui.horizontal(|ui| {
                                    ui.label("图集");
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
                            if ui.button("添加动画片段").clicked() {
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
                            ui.label("动画数据通过属性面板编辑，运行时仍由 Lua 脚本控制切换。");
                            if ui.button("移除动画状态机").clicked()
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
                        ui.label("容器");
                        egui::ComboBox::from_id_salt("container_kind")
                            .selected_text(container_kind_label(&self.container.kind))
                            .show_ui(ui, |ui| {
                                for (token, label) in [
                                    ("vbox", "垂直排列"),
                                    ("hbox", "水平排列"),
                                    ("grid", "网格"),
                                    ("wrap", "换行"),
                                    ("margin", "边距"),
                                    ("center", "居中"),
                                ] {
                                    ui.selectable_value(
                                        &mut self.container.kind,
                                        token.to_owned(),
                                        label,
                                    );
                                }
                            });
                    });
                    // E05-4：常用预设只改缓冲（kind/spacing/columns），提交仍走 E08 管线。
                    ui.horizontal(|ui| {
                        ui.label("预设");
                        for preset in ContainerPreset::ALL {
                            if ui.small_button(preset.label()).clicked() {
                                preset.apply(&mut self.container);
                            }
                        }
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
                            .selected_text(container_align_label(&self.container.align))
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
                    if matches!(self.container.kind.as_str(), "hbox" | "vbox" | "grid" | "wrap") {
                        ui.horizontal(|ui| {
                            ui.label("主轴分布");
                            egui::ComboBox::from_id_salt("container_justify")
                                .selected_text(container_justify_label(&self.container.justify))
                                .show_ui(ui, |ui| {
                                    for (token, label) in [
                                        ("start", "起始"),
                                        ("center", "居中"),
                                        ("end", "末尾"),
                                        ("space_between", "两端对齐"),
                                        ("space_around", "环绕分布"),
                                        ("space_evenly", "均匀分布"),
                                    ] {
                                        ui.selectable_value(
                                            &mut self.container.justify,
                                            token.to_owned(),
                                            label,
                                        );
                                    }
                                });
                        });
                    }
                    if ui.button("移除容器").clicked()
                        && let Err(error) = self.remove_component(selected, "Container")
                    {
                        self.status = error;
                    }
                }
                // E05-4：LayoutItem（父容器对该子项的约束）。
                if has("LayoutItem") {
                    ui.label("布局项（子项约束）");
                    for (label, pair) in [
                        ("最小", &mut self.layout_item.min_size),
                        ("最大", &mut self.layout_item.max_size),
                        ("首选", &mut self.layout_item.preferred_size),
                        ("百分比", &mut self.layout_item.size_percent),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            ui.add(egui::DragValue::new(&mut pair[0]).range(0.0..=100_000.0));
                            ui.add(egui::DragValue::new(&mut pair[1]).range(0.0..=100_000.0));
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.label("伸展");
                        ui.add(egui::DragValue::new(&mut self.layout_item.grow).range(0.0..=100.0));
                        ui.label("收缩");
                        ui.add(
                            egui::DragValue::new(&mut self.layout_item.shrink).range(0.0..=100.0),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("自对齐");
                        let selected_text = if self.layout_item.align_self.is_empty() {
                            "继承容器".to_owned()
                        } else {
                            self.layout_item.align_self.clone()
                        };
                        egui::ComboBox::from_id_salt("layout_item_align")
                            .selected_text(selected_text)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.layout_item.align_self,
                                    String::new(),
                                    "继承容器",
                                );
                                for (token, label) in
                                    [("start", "起始"), ("center", "居中"), ("end", "末尾")]
                                {
                                    ui.selectable_value(
                                        &mut self.layout_item.align_self,
                                        token.to_owned(),
                                        label,
                                    );
                                }
                            });
                        ui.label("宽高比");
                        ui.add(
                            egui::DragValue::new(&mut self.layout_item.aspect_ratio)
                                .range(0.0..=10.0),
                        );
                        ui.checkbox(&mut self.layout_item.visible, "可见");
                    });
                    if ui.button("移除布局项").clicked()
                        && let Err(error) = self.remove_component(selected, "LayoutItem")
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
                        ui.label("Lua 脚本");
                        ui.text_edit_singleline(&mut self.lua_script);
                    });
                }
                // docs/55 阶段 2：有 schema、没有手写面板的组件由通用字段编辑器接管
                // （Camera2D/Quad/RenderMode/Visibility/EntityScript/Particle），
                // 提交通通走 typed action，不在面板里直接写组件串。
                self.draw_schema_driven_components(ui);
                // P3-E02：`.material` 覆盖的字段/贴图编辑（只发 typed action）。
                self.draw_material_section(ui);
                // P3-E04-3：插件检查器段（无选中实体时由方法内部跳过）。
                self.draw_plugin_inspector_sections(ui);
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
