//! Editor wiring for the structured UI data layer (E20–E27).

use crate::EditorApp;
use editor::{CommandKind, InspectorError};
use eframe::egui;
use engine::core::{
    component_schema::{builtin_registry, validate_component},
    editor_plugins::EditorPlugin,
    preview_diagnostics::{DiagnosticCollector, PreviewDiagnostic, PreviewErrorKind},
    scene::SceneEntityId,
    ui_animation::{UiAnimationPreview, UiEasing, UiKeyframe},
    ui_binding::BindingSet,
    ui_events::UiEvents,
    ui_layout::{CanvasConfig, ContainerStyle, UiRectLayout},
    ui_scene_layout::{UiSceneNode, resolve_scene_ui_layout},
    ui_states::ButtonStateSet,
};
use std::collections::{BTreeMap, BTreeSet};

/// A tiny built-in plugin proves that the registry is owned by the app and
/// survives through the whole editor lifetime without touching scene data.
struct CoreEditorPlugin;

impl EditorPlugin for CoreEditorPlugin {
    fn name(&self) -> &str {
        "core"
    }
}

impl EditorApp {
    pub(crate) fn initialize_ui_extensions(&mut self) {
        if self.plugin_registry.names().is_empty() {
            match self.plugin_registry.register(Box::new(CoreEditorPlugin)) {
                Ok(()) => match self.plugin_registry.load("core") {
                    Ok(()) => self
                        .plugin_status
                        .push("插件 core 已加载（API v1）".to_owned()),
                    Err(error) => self
                        .plugin_status
                        .push(format!("插件 core 加载失败：{error}")),
                },
                Err(error) => self.plugin_status.push(format!("插件注册失败：{error}")),
            }
        }
    }

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
            .unwrap_or_else(|| engine::core::ui_animation::UiAnimationClip::new("preview"));
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
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
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
            format!("Set {key}"),
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

    pub(crate) fn apply_canvas_config(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
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
            "Set Canvas Config",
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
                        "Canvas 配置只适用于 UI Root".to_owned(),
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
        let mut fields = Vec::new();
        for (property, track) in &self.ui_animation_editor.tracks {
            let keyframes = track
                .keyframes
                .iter()
                .map(|keyframe| {
                    format!(
                        "{:.3}:{:.3}:{}",
                        keyframe.time,
                        keyframe.value,
                        keyframe.easing.as_str()
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            fields.push(format!("track.{property}={keyframes}"));
        }
        self.set_ui_component("UiAnimation", Some(fields.join("|")));
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
                    && let Ok(image) = engine::core::ui::UiImage::from_component_str(reference)
                    && !image.texture.is_empty()
                    && !self.project.root.join(&image.texture).is_file()
                {
                    diagnostics.push(PreviewDiagnostic {
                        kind: PreviewErrorKind::AssetMissing,
                        entity: Some(entity.id.0),
                        component: Some("Image".to_owned()),
                        message: format!("texture {} was not found", image.texture),
                    });
                }
            }
            diagnostics.extend(layout_diagnostics(scene));
        }
        let collector = DiagnosticCollector::default();
        self.preview_diagnostics = collector;
        self.preview_diagnostics.replace_all(diagnostics);
    }

    pub(crate) fn revert_selected_prefab_overrides(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(selected) = self.selection.primary() else {
            self.status = "请先选中 Prefab 根实例".to_owned();
            return;
        };
        let Some(scene) = self.current_scene().cloned() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&selected) else {
            self.status = "Prefab 根实例不存在".to_owned();
            return;
        };
        let Some(prefab_source) = entity.metadata.get("prefab.source").cloned() else {
            self.status = "当前实体没有 Prefab 来源".to_owned();
            return;
        };
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        let prefab = match engine::core::prefab::Prefab::load(root.join(&prefab_source)) {
            Ok(prefab) => prefab,
            Err(error) => {
                self.status = format!("加载源 Prefab 失败：{error}");
                return;
            }
        };
        let Some(doc) = self.document.current_scene.as_mut() else {
            return;
        };
        let result = self.history.execute(
            doc,
            "Revert Prefab Overrides",
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
                self.status = "已还原 Prefab 覆盖".to_owned();
            }
            Err(error) => self.status = format!("还原 Prefab 覆盖失败：{error}"),
        }
    }
}

pub(crate) fn parse_ui_animation_component(
    raw: &str,
) -> Result<engine::core::ui_animation::UiAnimationClip, String> {
    let mut clip = engine::core::ui_animation::UiAnimationClip::new("preview");
    for field in raw.split('|') {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        let Some((key, value)) = field.split_once('=') else {
            return Err(format!("malformed animation field: {field}"));
        };
        let Some(property) = key.strip_prefix("track.") else {
            return Err(format!("unknown animation field: {key}"));
        };
        let mut keyframes = Vec::new();
        for item in value.split(',') {
            let parts = item.split(':').collect::<Vec<_>>();
            if parts.len() != 3 {
                return Err(format!("malformed keyframe: {item}"));
            }
            keyframes.push(UiKeyframe {
                time: parts[0]
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad time: {item}"))?,
                value: parts[1]
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad value: {item}"))?,
                easing: UiEasing::parse(parts[2]).ok_or_else(|| format!("bad easing: {item}"))?,
            });
        }
        let track = engine::core::ui_animation::UiAnimationTrack::new(property, keyframes)?;
        clip.add_track(track)?;
    }
    Ok(clip)
}

fn layout_diagnostics(scene: &editor::SceneDocument) -> Vec<PreviewDiagnostic> {
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
        let preferred_size = if entity.components.contains_key("Image") {
            layout
                .as_ref()
                .map(|layout| {
                    let rect = layout.resolve([0.0, 0.0]);
                    [rect.w, rect.h]
                })
                .unwrap_or([64.0, 64.0])
        } else if entity.components.contains_key("Text") {
            [128.0, 32.0]
        } else {
            [64.0, 64.0]
        };
        nodes.insert(
            entity.id.0,
            UiSceneNode {
                parent: entity.parent.map(|id| id.0),
                layout,
                container,
                preferred_size,
            },
        );
    }
    let (_, diagnostics) = resolve_scene_ui_layout(&nodes, [1280.0, 720.0]);
    diagnostics
        .into_iter()
        .map(|diagnostic| PreviewDiagnostic {
            kind: PreviewErrorKind::Layout,
            entity: Some(diagnostic.entity),
            component: Some("UiLayout".to_owned()),
            message: format!("{:?}", diagnostic.reason),
        })
        .collect()
}

fn collect_subtree_ids(
    scene: &engine::core::scene::Scene,
    root: SceneEntityId,
) -> Result<Vec<SceneEntityId>, String> {
    fn visit(
        scene: &engine::core::scene::Scene,
        id: SceneEntityId,
        out: &mut Vec<SceneEntityId>,
        seen: &mut BTreeSet<SceneEntityId>,
    ) -> Result<(), String> {
        if !seen.insert(id) {
            return Err("Prefab subtree contains a cycle".to_owned());
        }
        let entity = scene
            .entities
            .get(&id)
            .ok_or_else(|| format!("missing entity {}", id.0))?;
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

#[allow(dead_code)]
fn unused_egui_marker(_: &egui::Context) {}
