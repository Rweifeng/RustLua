//! P3-E02：Inspector 的「材质（.material）」段。
//!
//! 选中带 `MeshRenderer` 的实体时：
//!
//! - 没有 `material=` 覆盖：显示 glTF 内嵌材质提示，可一键「创建覆盖」——写入默认
//!   `.material`（原子替换），再通过 typed action 把路径写进 `MeshRenderer.material`；
//! - 有覆盖：读写工程内 `.material`，显示 base color / metallic / roughness / emissive /
//!   normal scale / occlusion / alpha / double-sided / IBL 与五个贴图槽；
//! - 每次修改都走 [`MaterialEditSession`]：与导入器/运行时同一台解析器回读，非法值即时
//!   标红且**不提交**；拖动数值合并成一条撤销；
//! - 保存由 `editor::material_ops` 原子落盘（先写 `*.material.tmp`，失败保留旧文件）；
//! - 未保存的修改通过 [`MaterialState::preview`] 交给 Scene 视图：渲染器把内存材质
//!   放进与磁盘覆盖相同的 `GpuMaterial` 缓存（不写盘、不改文档），保存前即可见。
//!
//! 边界：面板不写场景文档。创建/移除覆盖只发 `EditorCommand::SetComponentField`；
//! `.material` 文件由面板经 `material_ops` 写盘（与时间线写 `.anim` 同一口径）。
//! 有未保存修改时切换选中实体不会丢掉会话（停在旧材质并提示）；「重新加载」在脏时禁用。
//!
//! 明确不做：另存为、预览球、资源浏览器双击/拖拽材质、UV 变换、材质节点图。

use std::path::{Path, PathBuf};

use eframe::egui;
use rustlua_assets::mat_asset::{MaterialAlphaMode, MaterialAsset, MaterialTexture};
use rustlua_assets::mat_edit::{MaterialEditError, MaterialEditSession};
use rustlua_render::mesh3d::MeshRenderer;
use rustlua_scene::scene::SceneEntityId;

use crate::EditorApp;

#[cfg(test)]
#[path = "../material_editor_tests.rs"]
mod material_editor;
use editor::EditorCommand;

const COMPONENT: &str = "MeshRenderer";
const MAX_TEXTURES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
struct MaterialTarget {
    entity: SceneEntityId,
    name: String,
    /// `MeshRenderer` 组件原文；`None` = 实体没有这个组件。
    payload: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MaterialKey {
    entity: SceneEntityId,
    payload: String,
}

#[derive(Debug, Default)]
pub(crate) enum MaterialStatus {
    #[default]
    NoSelection,
    NoMeshRenderer {
        name: String,
    },
    /// 有 `MeshRenderer`，但没有 `.material` 覆盖（使用 glTF 内嵌材质）。
    NoOverride {
        entity: SceneEntityId,
        name: String,
        mesh: String,
        suggested: String,
    },
    Error(String),
    Ready(Box<MaterialPreview>),
}

/// 面板只发的两类场景动作；真正提交在 shell 的 `dispatch`。
#[derive(Clone, Debug, PartialEq, Eq)]
enum MaterialIntent {
    AssignOverride { entity: SceneEntityId, path: String },
    ClearOverride { entity: SceneEntityId },
}

#[derive(Debug)]
pub(crate) struct MaterialPreview {
    entity: SceneEntityId,
    entity_name: String,
    material_path: String,
    project_root: PathBuf,
    session: MaterialEditSession,
    textures: Vec<String>,
    name_buffer: String,
    message: Option<(bool, String)>,
    switch_blocked: bool,
    reload_requested: bool,
    /// 「另存为」展开时正在编辑的目标路径；`None` = 折叠。
    save_as_path: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct MaterialState {
    key: Option<MaterialKey>,
    status: MaterialStatus,
    /// 上一帧交给 Scene 视图的预览快照。用它检测「这一帧里发生了修改」，
    /// 从而只在内容真的变化时让 3D 缓存失效。
    last_preview: Option<(String, MaterialAsset)>,
}

impl MaterialState {
    /// 交给渲染器的内存预览：`Some` 时该路径的 `.material` 由会话内容替代（不写盘）。
    pub(crate) fn preview(&self) -> Option<(&str, &MaterialAsset)> {
        match &self.status {
            MaterialStatus::Ready(preview) => {
                Some((preview.material_path.as_str(), preview.session.material()))
            }
            _ => None,
        }
    }
}

/// 「创建覆盖」的副作用：先原子写默认材质，成功后由调用方发写回路径的 action。
fn create_override(
    entity: SceneEntityId,
    name: &str,
    suggested: &str,
    project_root: &Path,
) -> Result<MaterialIntent, String> {
    let material = MaterialAsset {
        name: Some(name.to_owned()),
        ..MaterialAsset::default()
    };
    editor::material_ops::save_material(project_root, suggested, &material)
        .map(|_| MaterialIntent::AssignOverride {
            entity,
            path: suggested.to_owned(),
        })
        .map_err(|error| error.to_string())
}

impl MaterialState {
    fn sync(&mut self, target: Option<MaterialTarget>, project_root: &Path) {
        let Some(target) = target else {
            self.key = None;
            self.status = MaterialStatus::NoSelection;
            return;
        };
        let next_key = target.payload.as_ref().map(|payload| MaterialKey {
            entity: target.entity,
            payload: payload.clone(),
        });
        if let Some(next_key) = &next_key
            && self.key.as_ref() == Some(next_key)
        {
            match &mut self.status {
                MaterialStatus::NoMeshRenderer { name } => {
                    *name = target.name.clone();
                    return;
                }
                MaterialStatus::NoOverride { name, .. } => {
                    *name = target.name.clone();
                    return;
                }
                MaterialStatus::Ready(preview) => {
                    preview.entity_name = target.name.clone();
                    preview.switch_blocked = false;
                    return;
                }
                // 上次加载失败：同一载荷也允许重试（用户可能刚修好文件）。
                MaterialStatus::Error(_) => {}
                MaterialStatus::NoSelection => {}
            }
        }
        if let MaterialStatus::Ready(preview) = &mut self.status
            && preview.session.is_dirty()
        {
            preview.switch_blocked = true;
            return;
        }
        let Some(payload) = target.payload else {
            self.key = None;
            self.status = MaterialStatus::NoMeshRenderer { name: target.name };
            return;
        };
        self.key = Some(MaterialKey {
            entity: target.entity,
            payload: payload.clone(),
        });
        self.status = load_status(target.entity, &target.name, &payload, project_root);
    }

    fn draw(&mut self, ui: &mut egui::Ui, project_root: &Path) -> Option<MaterialIntent> {
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("材质");
            ui.weak(".material 覆盖；不改场景文档");
        });
        let intent = match &mut self.status {
            MaterialStatus::NoSelection => {
                ui.weak("在层级或场景视图里选中一个带网格渲染器的实体。");
                None
            }
            MaterialStatus::NoMeshRenderer { name } => {
                ui.weak(format!("实体 `{name}` 没有网格渲染器组件。"));
                None
            }
            MaterialStatus::NoOverride {
                entity,
                name,
                mesh,
                suggested,
            } => {
                ui.label(format!(
                    "实体 `{name}` 使用 glTF 内嵌材质（网格 `{mesh}`）。"
                ));
                ui.weak(
                    "创建覆盖后会把材质参数保存成工程内 .material，并写回网格渲染器的材质字段。",
                );
                ui.label(format!("建议路径：{suggested}"));
                if ui.button("创建覆盖").clicked() {
                    match create_override(*entity, name, suggested, project_root) {
                        Ok(intent) => return Some(intent),
                        Err(error) => {
                            ui.colored_label(
                                egui::Color32::from_rgb(226, 120, 92),
                                format!("创建覆盖失败：{error}"),
                            );
                        }
                    }
                }
                None
            }
            MaterialStatus::Error(error) => {
                ui.colored_label(egui::Color32::from_rgb(226, 148, 92), error);
                None
            }
            MaterialStatus::Ready(preview) => {
                let intent = preview.draw(ui, project_root);
                if preview.reload_requested {
                    preview.reload_requested = false;
                    self.key = None;
                }
                intent
            }
        };
        if let MaterialStatus::Ready(preview) = &mut self.status {
            preview.draw_message(ui);
        }
        intent
    }
}

fn load_status(
    entity: SceneEntityId,
    entity_name: &str,
    payload: &str,
    project_root: &Path,
) -> MaterialStatus {
    let renderer = match MeshRenderer::from_component_str(payload) {
        Ok(renderer) => renderer,
        Err(error) => return MaterialStatus::Error(format!("网格渲染器配置无效：{error}")),
    };
    let Some(material_path) = renderer.material.clone() else {
        return MaterialStatus::NoOverride {
            entity,
            name: entity_name.to_owned(),
            mesh: renderer.mesh,
            suggested: editor::material_ops::suggest_override_path(project_root, entity_name),
        };
    };
    let material = match editor::material_ops::load_material(project_root, &material_path) {
        Ok(material) => material,
        Err(error) => return MaterialStatus::Error(format!("材质加载失败：{error}")),
    };
    let name_buffer = material.name.clone().unwrap_or_default();
    MaterialStatus::Ready(Box::new(MaterialPreview {
        entity,
        entity_name: entity_name.to_owned(),
        material_path,
        project_root: project_root.to_path_buf(),
        session: MaterialEditSession::new(material),
        textures: collect_textures(project_root),
        name_buffer,
        message: None,
        switch_blocked: false,
        reload_requested: false,
        save_as_path: None,
    }))
}

impl EditorApp {
    fn sync_material_from_selection(&mut self) {
        let target = self.selection.primary().and_then(|id| {
            let document = self.current_scene()?;
            let entity = document.scene.entities.get(&id)?;
            Some(MaterialTarget {
                entity: id,
                name: entity.name.clone(),
                payload: entity.components.get(COMPONENT).cloned(),
            })
        });
        let project_root = self.project.root.clone();
        self.material.sync(target, &project_root);
    }

    /// Inspector 末尾的材质段（`DockTab::Inspector` 内调用）。
    pub(crate) fn draw_material_section(&mut self, ui: &mut egui::Ui) {
        self.sync_material_from_selection();
        let project_root = self.project.root.clone();
        let intent = self.material.draw(ui, &project_root);
        self.refresh_material_preview();
        let Some(intent) = intent else {
            return;
        };
        let command = match intent {
            MaterialIntent::AssignOverride { entity, path } => EditorCommand::SetComponentField {
                entity,
                component: COMPONENT.to_owned(),
                field: "material".to_owned(),
                value: path,
            },
            MaterialIntent::ClearOverride { entity } => {
                EditorCommand::ClearMeshRendererMaterial { entity }
            }
        };
        self.dispatch(command);
    }

    /// 本帧的材质会话内容与上帧不同（编辑/撤销/重做/还原/换选中）时，
    /// 让 3D 视图缓存失效并重绘；渲染侧每帧会读取 [`MaterialState::preview`]。
    fn refresh_material_preview(&mut self) {
        let next = self
            .material
            .preview()
            .map(|(path, material)| (path.to_owned(), material.clone()));
        if next == self.material.last_preview {
            return;
        }
        self.material.last_preview = next;
        self.invalidate_scene_view_state();
    }
}

impl MaterialPreview {
    fn draw(&mut self, ui: &mut egui::Ui, project_root: &Path) -> Option<MaterialIntent> {
        let mut intent = None;
        ui.horizontal(|ui| {
            ui.label(format!("实体 `{}`", self.entity_name));
            ui.weak(&self.material_path);
            if self.session.is_dirty() {
                ui.colored_label(egui::Color32::from_rgb(226, 190, 92), "● 未保存");
            }
        });
        if self.switch_blocked {
            ui.colored_label(
                egui::Color32::from_rgb(226, 148, 92),
                "选中已切换：当前材质有未保存修改，先保存或还原。",
            );
        }
        ui.horizontal(|ui| {
            let can_undo = self.session.can_undo();
            if ui
                .add_enabled(can_undo, egui::Button::new("撤销"))
                .on_disabled_hover_text("没有可撤销的修改")
                .clicked()
            {
                self.session.undo();
            }
            let can_redo = self.session.can_redo();
            if ui
                .add_enabled(can_redo, egui::Button::new("重做"))
                .on_disabled_hover_text("没有可重做的修改")
                .clicked()
            {
                self.session.redo();
            }
            if ui.button("保存").clicked() {
                self.save_to_disk();
            }
            if ui
                .button("另存为")
                .on_hover_text("写入新的 .material 并把网格渲染器的材质切换到它")
                .clicked()
            {
                self.save_as_path = Some(editor::material_ops::suggest_override_path(
                    &self.project_root,
                    &self.entity_name,
                ));
            }
            let dirty = self.session.is_dirty();
            if ui
                .add_enabled(dirty, egui::Button::new("还原"))
                .on_disabled_hover_text("没有未保存修改")
                .clicked()
                && let Err(error) = self.session.revert_to_saved()
            {
                self.message = Some((false, format!("还原失败：{error}")));
            }
            if ui
                .add_enabled(!dirty, egui::Button::new("重新加载"))
                .on_disabled_hover_text("先保存或还原未保存修改")
                .clicked()
            {
                self.reload_requested = true;
            }
            if ui
                .add_enabled(!dirty, egui::Button::new("移除覆盖"))
                .on_disabled_hover_text("先保存或还原未保存修改")
                .clicked()
            {
                intent = Some(MaterialIntent::ClearOverride {
                    entity: self.entity,
                });
            }
        });
        if let Some(initial) = self.save_as_path.clone() {
            let mut path = initial;
            ui.horizontal(|ui| {
                ui.label("另存为");
                ui.add(
                    egui::TextEdit::singleline(&mut path)
                        .desired_width(220.0)
                        .hint_text("assets/materials/....material"),
                );
                let confirm = ui.button("写入并切换").clicked();
                let cancel = ui.button("取消").clicked();
                if cancel {
                    self.save_as_path = None;
                } else if confirm {
                    match self.save_as(&path) {
                        Ok(next) => intent = Some(next),
                        Err(error) => {
                            self.message = Some((false, format!("另存为失败：{error}")));
                        }
                    }
                } else {
                    self.save_as_path = Some(path);
                }
            });
        }

        let current = self.session.material().clone();
        let mut name = self.name_buffer.clone();
        let mut base_color = current.base_color;
        let mut metallic = current.metallic;
        let mut roughness = current.roughness;
        let mut emissive = [
            current.emissive[0],
            current.emissive[1],
            current.emissive[2],
        ];
        let mut emissive_strength = current.emissive_strength;
        let mut normal_scale = current.normal_scale;
        let mut occlusion_strength = current.occlusion_strength;
        let mut alpha_mode = current.alpha_mode;
        let mut alpha_cutoff = current.alpha_cutoff;
        let mut double_sided = current.double_sided;
        let mut ibl_intensity = current.ibl_intensity;

        egui::Grid::new("material_fields")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("名称");
                let response = ui.text_edit_singleline(&mut name);
                let submitted =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if submitted && name != self.name_buffer {
                    self.name_buffer = name.clone();
                    self.apply(false, |material| material.name = Some(name.clone()));
                }
                ui.end_row();

                ui.label("基础色");
                let response = ui.color_edit_button_rgba_unmultiplied(&mut base_color);
                if response.changed() {
                    self.apply(false, |material| material.base_color = base_color);
                }
                ui.end_row();

                ui.label("金属度");
                let response = number(ui, &mut metallic, 0.0..=1.0, 0.01);
                if response.drag_started() {
                    self.session.break_coalescing();
                }
                if response.changed() {
                    self.apply(response.dragged(), |material| material.metallic = metallic);
                }
                ui.end_row();

                ui.label("粗糙度");
                let response = number(ui, &mut roughness, 0.0..=1.0, 0.01);
                if response.drag_started() {
                    self.session.break_coalescing();
                }
                if response.changed() {
                    self.apply(response.dragged(), |material| {
                        material.roughness = roughness
                    });
                }
                ui.end_row();

                ui.label("自发光");
                let response = ui.color_edit_button_rgb(&mut emissive);
                if response.changed() {
                    self.apply(false, |material| material.emissive = emissive);
                }
                ui.end_row();

                ui.label("发光强度");
                let response = number(ui, &mut emissive_strength, 0.0..=100.0, 0.05);
                if response.drag_started() {
                    self.session.break_coalescing();
                }
                if response.changed() {
                    self.apply(response.dragged(), |material| {
                        material.emissive_strength = emissive_strength;
                    });
                }
                ui.end_row();

                ui.label("法线强度");
                let response = number(ui, &mut normal_scale, 0.0..=10.0, 0.01);
                if response.drag_started() {
                    self.session.break_coalescing();
                }
                if response.changed() {
                    self.apply(response.dragged(), |material| {
                        material.normal_scale = normal_scale
                    });
                }
                ui.end_row();

                ui.label("遮挡强度");
                let response = number(ui, &mut occlusion_strength, 0.0..=1.0, 0.01);
                if response.drag_started() {
                    self.session.break_coalescing();
                }
                if response.changed() {
                    self.apply(response.dragged(), |material| {
                        material.occlusion_strength = occlusion_strength;
                    });
                }
                ui.end_row();

                ui.label("透明模式");
                egui::ComboBox::from_id_salt("material_alpha_mode")
                    .selected_text(alpha_label(alpha_mode))
                    .show_ui(ui, |ui| {
                        for mode in [
                            MaterialAlphaMode::Opaque,
                            MaterialAlphaMode::Mask,
                            MaterialAlphaMode::Blend,
                        ] {
                            ui.selectable_value(&mut alpha_mode, mode, alpha_label(mode));
                        }
                    });
                if alpha_mode != current.alpha_mode {
                    self.apply(false, |material| material.alpha_mode = alpha_mode);
                }
                ui.end_row();

                ui.label("透明阈值");
                let response = ui.add_enabled(
                    current.alpha_mode == MaterialAlphaMode::Mask,
                    egui::DragValue::new(&mut alpha_cutoff)
                        .speed(0.01)
                        .range(0.0..=1.0),
                );
                if response.changed() {
                    self.apply(true, |material| material.alpha_cutoff = alpha_cutoff);
                }
                ui.end_row();

                ui.label("双面");
                if ui.checkbox(&mut double_sided, "").changed() {
                    self.apply(false, |material| material.double_sided = double_sided);
                }
                ui.end_row();

                ui.label("IBL 强度");
                let response = number(ui, &mut ibl_intensity, 0.0..=10.0, 0.01);
                if response.drag_started() {
                    self.session.break_coalescing();
                }
                if response.changed() {
                    self.apply(response.dragged(), |material| {
                        material.ibl_intensity = ibl_intensity
                    });
                }
                ui.end_row();
            });

        ui.separator();
        ui.strong("贴图");
        for (index, label) in ["基础色", "金属度/粗糙度", "法线", "遮挡", "自发光"]
            .into_iter()
            .enumerate()
        {
            self.draw_texture_slot(ui, index, label, project_root);
        }

        intent
    }

    fn draw_message(&mut self, ui: &mut egui::Ui) {
        let Some((ok, message)) = &self.message else {
            return;
        };
        let color = if *ok {
            egui::Color32::from_rgb(120, 190, 130)
        } else {
            egui::Color32::from_rgb(226, 120, 92)
        };
        ui.colored_label(color, message);
    }

    fn draw_texture_slot(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        label: &str,
        project_root: &Path,
    ) {
        let current =
            texture(self.session.material(), index).and_then(|texture| texture.path.clone());
        let mut picked: Option<Option<String>> = None;
        ui.horizontal(|ui| {
            ui.label(label);
            match &current {
                Some(path) => {
                    let exists = project_root.join(path).is_file();
                    if exists {
                        ui.label(path);
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(226, 120, 92),
                            format!("{path}（文件缺失）"),
                        );
                    }
                }
                None => {
                    ui.weak("（无）");
                }
            }
            egui::ComboBox::from_id_salt(format!("material_texture_{index}"))
                .selected_text("选择…")
                .show_ui(ui, |ui| {
                    let has_current = current.is_some();
                    if ui.selectable_label(!has_current, "（无）").clicked() {
                        picked = Some(None);
                    }
                    for path in &self.textures {
                        let selected = current.as_deref() == Some(path.as_str());
                        if ui.selectable_label(selected, path).clicked() {
                            picked = Some(Some(path.clone()));
                        }
                    }
                });
            if current.is_some() && ui.small_button("清除").clicked() {
                picked = Some(None);
            }
        });
        if let Some(path) = picked {
            self.set_texture(index, path);
        }
    }

    fn set_texture(&mut self, index: usize, path: Option<String>) {
        self.apply(false, |material| {
            let slot = texture_mut(material, index);
            *slot = path.map(|path| MaterialTexture {
                path: Some(path),
                image: None,
            });
        });
    }

    fn apply(&mut self, coalesce: bool, mutate: impl FnOnce(&mut MaterialAsset)) {
        let result = if coalesce {
            self.session.edit_coalesced(mutate)
        } else {
            self.session.edit(mutate)
        };
        match result {
            Ok(()) => self.message = None,
            Err(error) => {
                let detail = match error {
                    MaterialEditError::Invalid(error) => error.detail,
                    other => other.to_string(),
                };
                self.message = Some((false, format!("未提交：{detail}")));
            }
        }
    }

    /// 另存为：目标必须是工程内、尚不存在的 `.material`；写入成功后把绑定切过去。
    /// 不覆盖已有文件（那是「保存」的职责），失败时磁盘与绑定都不变。
    fn save_as(&mut self, relative: &str) -> Result<MaterialIntent, String> {
        let relative = relative.trim().replace('\\', "/");
        if relative == self.material_path {
            return Err("目标与当前路径相同：直接点「保存」即可".to_owned());
        }
        let target = editor::material_ops::resolve_material_path(&self.project_root, &relative)
            .map_err(|error| error.to_string())?;
        if target.exists() {
            return Err(format!("`{relative}` 已存在：另存为不会覆盖已有文件"));
        }
        editor::material_ops::save_material(&self.project_root, &relative, self.session.material())
            .map_err(|error| error.to_string())?;
        self.material_path = relative.clone();
        self.name_buffer = self.session.material().name.clone().unwrap_or_default();
        self.session.mark_saved();
        self.save_as_path = None;
        // 让下一次 sync 重新按文档里的 material= 加载：命令成功则读新文件，
        // 被拒绝（如 Play 锁定）则回到旧路径，不会留下半切换的预览。
        self.reload_requested = true;
        self.message = Some((true, format!("已另存为 `{relative}` 并切换绑定")));
        Ok(MaterialIntent::AssignOverride {
            entity: self.entity,
            path: relative,
        })
    }

    fn save_to_disk(&mut self) -> bool {
        match editor::material_ops::save_material(
            &self.project_root,
            &self.material_path,
            self.session.material(),
        ) {
            Ok(path) => {
                self.session.mark_saved();
                self.message = Some((true, format!("已保存 {}", path.display())));
                true
            }
            Err(error) => {
                self.message = Some((false, format!("保存失败：{error}")));
                false
            }
        }
    }
}

fn number(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    speed: f32,
) -> egui::Response {
    ui.add(
        egui::DragValue::new(value)
            .speed(speed)
            .range(range)
            .max_decimals(4),
    )
}

fn texture(material: &MaterialAsset, index: usize) -> Option<&MaterialTexture> {
    match index {
        0 => material.base_color_texture.as_ref(),
        1 => material.metallic_roughness_texture.as_ref(),
        2 => material.normal_texture.as_ref(),
        3 => material.occlusion_texture.as_ref(),
        4 => material.emissive_texture.as_ref(),
        _ => None,
    }
}

fn texture_mut(material: &mut MaterialAsset, index: usize) -> &mut Option<MaterialTexture> {
    match index {
        0 => &mut material.base_color_texture,
        1 => &mut material.metallic_roughness_texture,
        2 => &mut material.normal_texture,
        3 => &mut material.occlusion_texture,
        4 => &mut material.emissive_texture,
        _ => &mut material.base_color_texture,
    }
}

fn alpha_label(mode: MaterialAlphaMode) -> &'static str {
    match mode {
        MaterialAlphaMode::Opaque => "不透明",
        MaterialAlphaMode::Mask => "遮罩",
        MaterialAlphaMode::Blend => "混合",
    }
}

/// 扫描工程 `assets/` 下的 png/jpg/jpeg（面板加载时做一次，最多 512 个）。
fn collect_textures(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        if out.len() >= MAX_TEXTURES {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        for path in paths {
            if out.len() >= MAX_TEXTURES {
                return;
            }
            if path.is_dir() {
                walk(&path, root, out);
                continue;
            }
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
                continue;
            }
            if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join("assets"), root, &mut out);
    out.sort();
    out
}
