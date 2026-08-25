//! 跨面板共享的编辑命令：撤销/重做、保存、场景切换、实体创建、Inspector 同步、对齐。

use crate::AlignMode;
use crate::EditorApp;
use crate::geometry::{encode_transform, parse_transform};
use editor::{CommandKind, TransformFields};
use engine::core::{
    animation::AnimationStateMachine, scene::SceneEntityId, sprite::SpriteComponent,
    tilemap::Tilemap,
};
use std::path::PathBuf;

impl EditorApp {
    pub(crate) fn undo(&mut self) {
        let label = {
            let h = &mut self.history;
            let d = &mut self.document;
            d.current_scene
                .as_mut()
                .and_then(|x| h.undo(x).ok())
                .map(str::to_owned)
        };
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
        match self.document.open_scene(&path) {
            Ok(()) => {
                self.history.clear();
                self.autosave.reset();
                self.play_session = None;
                self.tree.clear_selection();
                self.inspector.clear_selection();
                self.scene_state.preview.clear();
                self.scene_state.selected.clear();
                self.scene_state.collapsed.clear();
                self.sync_hierarchy_metadata();
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

    /// 同步 InspectorState ↔ EditorApp 快照字段。
    pub(crate) fn sync_inspector(&mut self, id: SceneEntityId) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let scene = scene.clone();
        if self.inspector.select(&scene, id).is_ok()
            && let Ok(snapshot) = self.inspector.snapshot(&scene)
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
            self.button = snapshot.button.unwrap_or_default();
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

    /// 对齐选中实体：网格吸附 / 水平 / 垂直。
    pub(crate) fn align_selected(&mut self, mode: AlignMode) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let ids: Vec<_> = self.scene_state.selected.iter().copied().collect();
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
        let ids: Vec<_> = self.scene_state.selected.iter().copied().collect();
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
        let ids: Vec<_> = self.scene_state.selected.iter().copied().collect();
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
        if let Some(selected) = self.tree.selected()
            && self.scene_state.selected.contains(&selected)
        {
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
        let ids: Vec<_> = self.scene_state.selected.iter().copied().collect();
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
        if let Some(selected) = self.tree.selected()
            && self.scene_state.selected.contains(&selected)
        {
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
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "Create entity", CommandKind::Create, move |scene| {
                    let mut tree = tree;
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
                    Ok::<(), String>(())
                });
        match result {
            Ok(true) => {
                if let Some(scene) = self.document.current_scene.as_ref()
                    && let Some(id) = scene.scene.entities.keys().max_by_key(|id| id.0).copied()
                {
                    let _ = self.tree.select(scene, id);
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
        if let Some(id) = self.tree.selected() {
            let _ = self.add_component(component);
            self.sync_inspector(id);
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
    use engine::core::transform::Vec2;

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
}
