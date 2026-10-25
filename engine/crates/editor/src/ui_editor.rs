//! Editor wiring for the structured UI data layer (E20–E27).

use crate::EditorApp;
use editor::{CommandKind, InspectorError};
use eframe::egui;
use rustlua_ecs::component_schema::{builtin_registry, validate_component};
use rustlua_infra::preview_diagnostics::{
    DiagnosticCollector, PreviewDiagnostic, PreviewErrorKind,
};
use rustlua_scene::scene::SceneEntityId;
// `UiEasing`/`UiKeyframe` 不再从这里导入：`UiAnimation` 的解析/编码已搬进引擎

// （`UiAnimationClip::tracks_from_component_str`/`tracks_to_component_str`）。

use rustlua_ui::ui_animation::UiAnimationPreview;
use rustlua_ui::ui_binding::BindingSet;
use rustlua_ui::ui_events::UiEvents;
use rustlua_ui::ui_layout::{CanvasConfig, ContainerStyle, LayoutAxis, UiRectLayout};
use rustlua_ui::ui_scene_layout::{UiLayoutReason, UiSceneNode, resolve_scene_ui_layout};
use rustlua_ui::ui_states::ButtonStateSet;
use std::collections::{BTreeMap, BTreeSet};

impl EditorApp {
    pub(crate) fn refresh_ui_editors(&mut self, id: SceneEntityId) {
        let Some(scene) = self.document.current_scene.clone() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&id) else {
            return;
        };
        self.events_editor = entity
            .components
            .get("UiEvents")
            .and_then(|raw| UiEvents::parse(raw).ok())
            .unwrap_or_default();
        self.bindings_editor = entity
            .components
            .get("UiBindings")
            .and_then(|raw| BindingSet::parse(raw).ok())
            .unwrap_or_default();
        self.button_states_editor = entity
            .components
            .get("ButtonStates")
            .and_then(|raw| ButtonStateSet::from_component_str(raw).ok())
            .unwrap_or_default();
        let animation = entity
            .components
            .get("UiAnimation")
            .and_then(|raw| parse_ui_animation_component(raw).ok())
            .unwrap_or_else(|| rustlua_ui::ui_animation::UiAnimationClip::new("preview"));
        if self.ui_animation_editor != animation {
            self.ui_animation_editor = animation;
            self.ui_animation_preview = None;
        }
        self.canvas_config = entity
            .metadata
            .get("ui.root")
            .filter(|value| *value == "true")
            .map(|_| {
                CanvasConfig::from_metadata(
                    entity.metadata.get("canvas.reference").map(String::as_str),
                    entity.metadata.get("canvas.scale_mode").map(String::as_str),
                    entity.metadata.get("canvas.safe_area").map(String::as_str),
                )
                .unwrap_or_default()
            })
            .unwrap_or_default();
    }

    fn set_ui_component(&mut self, key: &'static str, value: Option<String>) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(selected) = self.selection.primary() else {
            return;
        };
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self.history.execute(
            scene,
            format!("设置{}", crate::util::component_display_name(key)),
            CommandKind::Property,
            move |scene| {
                if let Some(entity) = scene.scene.entities.get_mut(&selected) {
                    match value {
                        Some(value) if entity.components.get(key) != Some(&value) => {
                            entity.components.insert(key.to_owned(), value);
                            scene.mark_dirty();
                        }
                        Some(_) => {}
                        None => {
                            if entity.components.remove(key).is_some() {
                                scene.mark_dirty();
                            }
                        }
                    }
                }
                Ok::<(), InspectorError>(())
            },
        );
        if let Err(error) = result {
            self.status = error.to_string();
        } else {
            self.sync_inspector(selected);
            self.refresh_preview_diagnostics();
        }
    }

    pub(crate) fn apply_events_editor(&mut self) {
        let value = if self.events_editor.bindings.is_empty() {
            None
        } else {
            Some(self.events_editor.to_component_str())
        };
        self.set_ui_component("UiEvents", value);
    }

    pub(crate) fn apply_bindings_editor(&mut self) {
        let value = if self.bindings_editor.bindings.is_empty() {
            None
        } else {
            Some(self.bindings_editor.to_component_str())
        };
        self.set_ui_component("UiBindings", value);
    }

    pub(crate) fn apply_button_states_editor(&mut self) {
        let value = if self.button_states_editor.normal.is_none() {
            None
        } else {
            Some(self.button_states_editor.to_component_str())
        };
        self.set_ui_component("ButtonStates", value);
    }

    /// E18：当前场景 UI Root（`ui.root=true`）的 Canvas 配置。
    /// 与选中项无关——HUD 与 Safe Area 在未选中 Canvas 时也必须读到同一份配置。
    pub(crate) fn scene_canvas_config(&self) -> CanvasConfig {
        let Some(document) = self.document.current_scene.as_ref() else {
            return CanvasConfig::default();
        };
        canvas_config_from_scene(&document.scene)
    }

    pub(crate) fn apply_canvas_config(&mut self) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(selected) = self.selection.primary() else {
            return;
        };
        let canvas = self.canvas_config;
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self.history.execute(
            scene,
            "设置画布配置",
            CommandKind::Property,
            move |scene| {
                let Some(entity) = scene.scene.entities.get_mut(&selected) else {
                    return Err(InspectorError::UnknownEntity(selected));
                };
                if entity
                    .metadata
                    .get("ui.root")
                    .is_none_or(|value| value != "true")
                {
                    return Err(InspectorError::InvalidAssetReference(
                        "画布配置只适用于界面根节点".to_owned(),
                    ));
                }
                entity.metadata.insert(
                    "canvas.reference".to_owned(),
                    format!("{},{}", canvas.reference[0], canvas.reference[1]),
                );
                entity.metadata.insert(
                    "canvas.scale_mode".to_owned(),
                    canvas.scale_mode.as_str().to_owned(),
                );
                entity.metadata.insert(
                    "canvas.safe_area".to_owned(),
                    format!(
                        "{},{},{},{}",
                        canvas.safe_area[0],
                        canvas.safe_area[1],
                        canvas.safe_area[2],
                        canvas.safe_area[3]
                    ),
                );
                scene.mark_dirty();
                Ok::<(), InspectorError>(())
            },
        );
        if let Err(error) = result {
            self.status = error.to_string();
        } else {
            self.sync_inspector(selected);
            self.refresh_preview_diagnostics();
        }
    }

    pub(crate) fn apply_ui_animation_editor(&mut self) {
        if self.ui_animation_editor.tracks.is_empty() {
            self.set_ui_component("UiAnimation", None);
            return;
        }
        // 编码同样来自引擎（唯一事实源）：编辑器不再自己拼 `track.<属性>=…`。
        let payload = rustlua_ui::ui_animation::UiAnimationClip::tracks_to_component_str(
            &self.ui_animation_editor.tracks,
        );
        self.set_ui_component("UiAnimation", Some(payload));
    }

    pub(crate) fn advance_ui_animation_preview(&mut self, delta_seconds: f32) {
        if !self.ui_animation_playing {
            return;
        }
        let preview = self
            .ui_animation_preview
            .get_or_insert_with(|| UiAnimationPreview::new(self.ui_animation_editor.clone(), true));
        preview.advance(delta_seconds);
    }

    pub(crate) fn toggle_ui_animation_preview(&mut self) {
        self.ui_animation_playing = !self.ui_animation_playing;
        if self.ui_animation_playing {
            let preview = self.ui_animation_preview.get_or_insert_with(|| {
                UiAnimationPreview::new(self.ui_animation_editor.clone(), true)
            });
            preview.playing = true;
        } else if let Some(preview) = &mut self.ui_animation_preview {
            preview.playing = false;
        }
    }

    pub(crate) fn reset_ui_animation_preview(&mut self) {
        self.ui_animation_preview = Some(UiAnimationPreview::new(
            self.ui_animation_editor.clone(),
            true,
        ));
    }

    pub(crate) fn invalidate_ui_layout_cache(&mut self) {
        self.ui_layout_cache.invalidate_all();
    }

    pub(crate) fn refresh_preview_diagnostics(&mut self) {
        let mut diagnostics = Vec::new();
        if let Some(scene) = self.document.current_scene.as_ref() {
            let registry = builtin_registry();
            for entity in scene.scene.entities.values() {
                for (key, raw) in &entity.components {
                    if registry.contains_key(key.as_str())
                        && let Err(error) = validate_component(&registry, key, raw)
                    {
                        diagnostics.push(PreviewDiagnostic {
                            kind: PreviewErrorKind::SceneLoad,
                            entity: Some(entity.id.0),
                            component: Some(key.clone()),
                            message: error.to_string(),
                        });
                    }
                    if key == "UiEvents"
                        && let Err(error) = UiEvents::parse(raw)
                    {
                        diagnostics.push(PreviewDiagnostic {
                            kind: PreviewErrorKind::SceneLoad,
                            entity: Some(entity.id.0),
                            component: Some(key.clone()),
                            message: error.to_string(),
                        });
                    }
                    if key == "UiBindings"
                        && let Err(error) = BindingSet::parse(raw)
                    {
                        diagnostics.push(PreviewDiagnostic {
                            kind: PreviewErrorKind::SceneLoad,
                            entity: Some(entity.id.0),
                            component: Some(key.clone()),
                            message: error.to_string(),
                        });
                    }
                    if key == "ButtonStates"
                        && let Err(error) = ButtonStateSet::from_component_str(raw)
                    {
                        diagnostics.push(PreviewDiagnostic {
                            kind: PreviewErrorKind::SceneLoad,
                            entity: Some(entity.id.0),
                            component: Some(key.clone()),
                            message: error,
                        });
                    }
                }
                if let Some(reference) = entity.components.get("Image").map(String::as_str)
                    && let Ok(image) = rustlua_ui::ui::UiImage::from_component_str(reference)
                    && !image.texture.is_empty()
                    && image.texture != rustlua_render::frame::WHITE_TEXTURE_PATH
                    && !self.project.root.join(&image.texture).is_file()
                {
                    diagnostics.push(PreviewDiagnostic {
                        kind: PreviewErrorKind::AssetMissing,
                        entity: Some(entity.id.0),
                        component: Some("Image".to_owned()),
                        message: format!("找不到纹理 {}", image.texture),
                    });
                }
            }
            diagnostics.extend(layout_diagnostics(
                scene,
                [
                    self.preview_resolution.0 as f32,
                    self.preview_resolution.1 as f32,
                ],
            ));
        }
        let collector = DiagnosticCollector::default();
        self.preview_diagnostics = collector;
        self.preview_diagnostics.replace_all(diagnostics);
        self.layout_diagnostics_resolution = self.preview_resolution;
    }

    /// E05-4：布局诊断按**当前预览分辨率**解算，切分辨率后角标不会沿用旧几何。
    ///
    /// 只在分辨率变化时重算（`refresh_preview_diagnostics` 会重新解析所有组件）；
    /// 场景/字段变化仍走各自的既有刷新点。
    pub(crate) fn refresh_layout_diagnostics_if_resolution_changed(&mut self) {
        if self.layout_diagnostics_resolution != self.preview_resolution {
            self.refresh_preview_diagnostics();
        }
    }

    /// 回退 Prefab 实例上的所有覆盖（目标由 typed action 显式给出）。
    pub(crate) fn revert_prefab_overrides(&mut self, selected: SceneEntityId) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(scene) = self.current_scene().cloned() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&selected) else {
            self.status = "预制体根实例不存在".to_owned();
            return;
        };
        let Some(prefab_source) = entity.metadata.get("prefab.source").cloned() else {
            self.status = "当前实体没有预制体来源".to_owned();
            return;
        };
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        let prefab = match rustlua_scene::prefab::Prefab::load(root.join(&prefab_source)) {
            Ok(prefab) => prefab,
            Err(error) => {
                self.status = format!("加载源预制体失败：{error}");
                return;
            }
        };
        let Some(doc) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self.history.execute(
            doc,
            "还原预制体覆盖",
            CommandKind::Property,
            move |scene| {
                if let Some(parent_id) = scene.scene.entities.get(&selected).and_then(|e| e.parent)
                    && let Some(parent) = scene.scene.entities.get_mut(&parent_id)
                {
                    parent.children.retain(|child| *child != selected);
                }
                let ids = collect_subtree_ids(&scene.scene, selected)?;
                for id in ids {
                    scene.scene.entities.remove(&id);
                }
                let empty = std::collections::BTreeMap::new();
                let parent = scene
                    .scene
                    .entities
                    .get(&selected)
                    .and_then(|entity| entity.parent);
                let instance = prefab
                    .instantiate(&mut scene.scene, parent, &empty)
                    .map_err(|error| error.to_string())?;
                for entity_id in instance.source_to_instance.values() {
                    if let Some(entity) = scene.scene.entities.get_mut(entity_id) {
                        entity
                            .metadata
                            .insert("prefab.source".to_owned(), prefab_source.clone());
                        entity.metadata.insert(
                            "prefab.root".to_owned(),
                            (*entity_id == instance.instance_root).to_string(),
                        );
                    }
                }
                scene.mark_dirty();
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(_) => {
                self.on_document_changed(crate::actions::DocumentChange::PrefabApply);
                self.status = "已还原预制体覆盖".to_owned();
            }
            Err(error) => self.status = format!("还原预制体覆盖失败：{error}"),
        }
    }
}

/// `UiAnimation` 组件串 → 预览用的 `UiAnimationClip`。
///
/// **解析本身住在引擎**（`UiAnimationClip::tracks_from_component_str`，第 40 轮搬过去）：
/// 编辑器和审计必须用同一份格式定义，否则又是"两份事实源"。这里只负责把轨道装进
/// 预览用的 clip（`name` 是预览概念，不属于组件契约）。
pub(crate) fn parse_ui_animation_component(
    raw: &str,
) -> Result<rustlua_ui::ui_animation::UiAnimationClip, String> {
    let tracks = rustlua_ui::ui_animation::UiAnimationClip::tracks_from_component_str(raw)?;
    let mut clip = rustlua_ui::ui_animation::UiAnimationClip::new("preview");
    clip.tracks = tracks;
    Ok(clip)
}

fn layout_diagnostics(
    scene: &editor::SceneDocument,
    resolution: [f32; 2],
) -> Vec<PreviewDiagnostic> {
    let mut nodes = BTreeMap::new();
    for entity in scene.scene.entities.values() {
        let layout = entity
            .components
            .get("UiLayout")
            .and_then(|raw| UiRectLayout::from_component_str(raw).ok());
        let container = entity
            .components
            .get("Container")
            .and_then(|raw| ContainerStyle::from_component_str(raw).ok());
        let layout_item = entity
            .components
            .get("LayoutItem")
            .and_then(|raw| rustlua_ui::ui_layout::LayoutItem::from_component_str(raw).ok());
        let preferred_size = rustlua_ui::ui_scene_layout::preferred_size_for(
            &entity.components,
            layout.as_ref(),
            &mut |text, scale| {
                rustlua_render::text::measure_builtin_text(text, scale).map(|(size, _)| size)
            },
        );
        nodes.insert(
            entity.id.0,
            UiSceneNode {
                parent: entity.parent.map(|id| id.0),
                layout,
                container,
                layout_item,
                preferred_size,
            },
        );
    }
    let (_, diagnostics) = resolve_scene_ui_layout(&nodes, resolution);
    diagnostics
        .into_iter()
        .map(|diagnostic| PreviewDiagnostic {
            kind: PreviewErrorKind::Layout,
            entity: Some(diagnostic.entity),
            component: Some("UiLayout".to_owned()),
            message: layout_reason_text(diagnostic.reason),
        })
        .collect()
}

fn layout_reason_text(reason: UiLayoutReason) -> String {
    match reason {
        UiLayoutReason::MissingParent => "父节点缺失".to_owned(),
        UiLayoutReason::CircularParent => "父节点循环引用".to_owned(),
        UiLayoutReason::InvalidLayout => "布局参数无法解析".to_owned(),
        UiLayoutReason::InvalidContainer => "容器样式无法解析".to_owned(),
        UiLayoutReason::Overflow {
            axis,
            needed,
            available,
        } => {
            let axis = match axis {
                LayoutAxis::Horizontal => "水平",
                LayoutAxis::Vertical => "垂直",
            };
            format!("{axis}方向溢出：需要 {needed:.1}，可用 {available:.1}")
        }
    }
}

fn collect_subtree_ids(
    scene: &rustlua_scene::scene::Scene,
    root: SceneEntityId,
) -> Result<Vec<SceneEntityId>, String> {
    fn visit(
        scene: &rustlua_scene::scene::Scene,
        id: SceneEntityId,
        out: &mut Vec<SceneEntityId>,
        seen: &mut BTreeSet<SceneEntityId>,
    ) -> Result<(), String> {
        if !seen.insert(id) {
            return Err("预制体子树存在循环引用".to_owned());
        }
        let entity = scene
            .entities
            .get(&id)
            .ok_or_else(|| format!("缺少实体 {}", id.0))?;
        for child in &entity.children {
            visit(scene, *child, out, seen)?;
        }
        out.push(id);
        Ok(())
    }
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    visit(scene, root, &mut out, &mut seen)?;
    Ok(out)
}

/// E18：从场景的 UI Root（`ui.root=true`）读取 Canvas 配置。
/// 无 Root 或 metadata 解析失败时退回默认值（与 `sync_inspector` 同口径）。
pub(crate) fn canvas_config_from_scene(scene: &rustlua_scene::scene::Scene) -> CanvasConfig {
    scene
        .entities
        .values()
        .find(|entity| {
            entity
                .metadata
                .get("ui.root")
                .is_some_and(|value| value.as_str() == "true")
        })
        .and_then(|entity| {
            CanvasConfig::from_metadata(
                entity.metadata.get("canvas.reference").map(String::as_str),
                entity.metadata.get("canvas.scale_mode").map(String::as_str),
                entity.metadata.get("canvas.safe_area").map(String::as_str),
            )
            .ok()
        })
        .unwrap_or_default()
}

/// E18：Scene/Game 共用的画布 HUD 读数。
/// 全部读数描述「当前屏幕上真实发生的事」，不引入未接线的配置投影。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CanvasHud {
    /// 设计分辨率（= 编辑器预览的 UI 参考尺寸）。
    pub design: (u32, u32),
    /// 画布在面板上真实占用的物理像素。
    pub display: (u32, u32),
    /// 物理像素 / 设计像素。
    pub scale: f32,
    /// 设计框被面板裁掉的设计像素（左/上/右/下）。
    pub crop: [f32; 4],
    /// `CanvasConfig.safe_area` 原始 insets（预览像素）。
    pub safe: [f32; 4],
}

impl CanvasHud {
    /// 常驻单行读数：设计分辨率 + 显示缩放。其余读数收进悬停提示（[`Self::details`]），
    /// 视口角落不再铺三行等宽调试文本。
    pub(crate) fn compact_line(&self) -> String {
        format!(
            "{}×{} · {:.0}%",
            self.design.0,
            self.design.1,
            self.scale * 100.0
        )
    }

    /// 悬停展开的完整体检读数（E18）：预览/显示/裁剪 + Safe Area/锚点框。
    pub(crate) fn details(&self) -> String {
        format!(
            "预览 {}×{} · 显示 {}×{}\n缩放 {:.0}% · 裁剪 左{:.0} 上{:.0} 右{:.0} 下{:.0}\n安全区 左{:.0} 上{:.0} 右{:.0} 下{:.0} · 锚点框 (0,0)–({},{})",
            self.design.0,
            self.design.1,
            self.display.0,
            self.display.1,
            self.scale * 100.0,
            self.crop[0],
            self.crop[1],
            self.crop[2],
            self.crop[3],
            self.safe[0],
            self.safe[1],
            self.safe[2],
            self.safe[3],
            self.design.0,
            self.design.1,
        )
    }
}

#[allow(dead_code)]
fn unused_egui_marker(_: &egui::Context) {}

#[cfg(test)]
mod tests {
    use super::CanvasHud;

    #[test]
    fn canvas_hud_stays_compact_and_expands_details_on_hover() {
        let hud = CanvasHud {
            design: (1280, 720),
            display: (640, 360),
            scale: 0.5,
            crop: [240.0, 60.0, 240.0, 60.0],
            safe: [40.0, 20.0, 40.0, 20.0],
        };
        assert_eq!(hud.compact_line(), "1280×720 · 50%");
        assert!(!hud.compact_line().contains("裁剪"), "常驻行不再带调试读数");
        assert_eq!(
            hud.details(),
            "预览 1280×720 · 显示 640×360\n\
             缩放 50% · 裁剪 左240 上60 右240 下60\n\
             安全区 左40 上20 右40 下20 · 锚点框 (0,0)–(1280,720)"
        );
    }
}
