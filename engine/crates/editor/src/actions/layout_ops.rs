//! 布局类命令：对齐、等距分布、精灵绘制顺序（前移/后移/按 Y 排序）。
//! 目标集合与分布轴的计算是自由函数，便于单独回归。

use super::*;

impl EditorApp {
    /// 对齐选中实体：网格吸附 / 水平 / 垂直。
    pub(crate) fn align_selected(&mut self, mode: AlignMode) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let ids: Vec<_> = self.selection.ids();
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
            .execute(scene, "对齐", CommandKind::Property, move |scene| {
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

    /// **资源重命名/移动 + 引用保持**（第 44 轮，docs/55 P1 的 Project Browser 一条）。
    ///
    /// 顺序是刻意的：**先校验、再搬文件、最后改引用**。
    /// - 目标已存在 / 越出工程根 / 正在 Play / 目标是当前打开的文档 → 直接拒绝（不做半截操作）；
    /// - 文件搬完再改引用：即使改引用失败，磁盘上也是"文件已改名 + 部分文档仍指向旧路径"
    ///   这种**可诊断**的状态（审计会把断链报出来），而不是"文档指向一个不存在的文件却没人知道"；
    /// - 其它文档的磁盘改写由 `editor::rewrite_project_references` 负责：**解析成功才写回**，
    ///   坏文件原样保留并进报告；
    /// - 当前打开的文档**不写磁盘**（保住用户未保存的编辑），而是在内存里改写并进 history
    ///   （一条 Undo 可回退引用改写；文件改名本身不回退，与 Unity 的取舍一致）。
    pub(crate) fn perform_resource_move(
        &mut self,
        from: &std::path::Path,
        to: &std::path::Path,
    ) -> Result<String, String> {
        if self.is_playing() {
            return Err("播放模式下不能移动资源，请先点击停止".to_owned());
        }
        let root = self.project.root.clone();
        let from_relative = editor::normalize_relative(from)
            .ok_or_else(|| format!("资源路径必须是工程内相对路径：{}", from.display()))?;
        let to_relative = editor::normalize_relative(to)
            .ok_or_else(|| format!("目标路径必须是工程内相对路径：{}", to.display()))?;
        if from_relative == to_relative {
            return Err("源与目标是同一个路径".to_owned());
        }
        let from_absolute = root.join(&from_relative);
        let to_absolute = root.join(&to_relative);
        if !from_absolute.is_file() {
            return Err(format!("源资源不存在：{}", from_relative.display()));
        }
        if to_absolute.exists() {
            return Err(format!("目标已存在：{}", to_relative.display()));
        }
        let open_document = self
            .document
            .current_scene
            .as_ref()
            .and_then(|document| document.path.clone());
        if open_document.as_ref().is_some_and(|path| {
            editor::paths_match(path, &from_absolute) || editor::paths_match(path, &to_absolute)
        }) {
            return Err(format!(
                "当前打开的文档不能重命名或移动：{}",
                from_relative.display()
            ));
        }
        if let Some(parent) = to_absolute.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建目标目录失败：{error}"))?;
        }
        std::fs::rename(&from_absolute, &to_absolute)
            .map_err(|error| format!("移动资源失败：{error}"))?;

        let from_text = from_relative.to_string_lossy().replace('\\', "/");
        let to_text = to_relative.to_string_lossy().replace('\\', "/");
        let report = editor::rewrite_project_references(
            &root,
            &from_text,
            &to_text,
            open_document.as_deref(),
        );

        // 当前打开的文档：内存改写 + 一条 Undo（不写盘）。
        let mut open_document_components = 0;
        if let Some(scene) = self.document.current_scene.as_mut() {
            let label = format!("资源引用：{from_text} → {to_text}");
            let counter = &mut open_document_components;
            let _ = self
                .history
                .execute(scene, label, CommandKind::Rename, |scene| {
                    let changed =
                        editor::rewrite_scene_references(&mut scene.scene, &from_text, &to_text);
                    if changed > 0 {
                        scene.mark_dirty();
                    }
                    *counter = changed;
                    Ok::<(), ()>(())
                });
        }

        self.thumbnails.remove(&from_text);
        // 预览窗指着刚被搬走的那张图：当场作废，否则窗里会留着一张"工程里已经不存在"的图。
        if self.texture_preview.relative.as_deref() == Some(from_text.as_str()) {
            self.texture_preview.invalidate();
        }
        self.refresh_resources();
        self.selected_resource = Some(to_relative.clone());
        let mut status = format!(
            "已重命名资源：{from_text} → {to_text}（改写 {} 个文档 / {} 处组件引用）",
            report.rewritten_files.len(),
            report.rewritten_components
        );
        if open_document_components > 0 {
            status.push_str(&format!(
                "；当前文档内 {open_document_components} 处（未保存，可 Undo）"
            ));
        }
        if !report.failures.is_empty() {
            status.push_str(&format!(
                "；{} 个文档未改写：{}",
                report.failures.len(),
                report.failures.join("、")
            ));
        }
        Ok(status)
    }

    /// 让选中的 2D 实体在水平或垂直方向上按首末锚点等间距分布。
    pub(crate) fn distribute_selected(&mut self, horizontal: bool) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let ids: Vec<_> = self.selection.ids();
        let targets = collect_selected_transforms(scene, &ids);
        if targets.len() < 3 {
            self.status = "等间距至少需要 3 个带变换组件的实体".to_owned();
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
            .execute(scene, "分布", CommandKind::Property, move |scene| {
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
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let ids: Vec<_> = self.selection.ids();
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
            self.status = "请先选中带精灵组件的实体".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let _ = self
            .history
            .execute(scene, "精灵排序", CommandKind::Property, move |scene| {
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
        if let Some(selected) = self.selection.primary() {
            self.sync_inspector(selected);
        }
        self.status = format!("已批量调整精灵排序：{delta:+}");
    }

    /// 按实体 Y 坐标重新生成选中 Sprite 的 sorting.order，适合 2D 俯视/伪等轴生产流程。
    pub(crate) fn sort_selected_sprites_by_y(&mut self, descending: bool) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let ids: Vec<_> = self.selection.ids();
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
            self.status = "至少选择 2 个带精灵组件的实体才能批量排序".to_owned();
            return;
        }
        let updates = sorted_sprite_order_updates(&targets, descending);
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let _ = self.history.execute(
            scene,
            "重排精灵顺序",
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
        if let Some(selected) = self.selection.primary() {
            self.sync_inspector(selected);
        }
        self.status = if descending {
            "已按 Y 从高到低重排精灵顺序".to_owned()
        } else {
            "已按 Y 从低到高重排精灵顺序".to_owned()
        };
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DistributionAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug)]
pub(crate) struct SpriteOrderTarget {
    pub(crate) id: SceneEntityId,
    pub(crate) position_x: f32,
    pub(crate) position_y: f32,
    pub(crate) order: i32,
    pub(crate) sprite: SpriteComponent,
}

pub(crate) fn collect_selected_transforms(
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

pub(crate) fn distribute_targets(
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

pub(crate) fn sorted_sprite_order_updates(
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

// ---------------------------------------------------------------------------
// Prefab typed action（shell 层）：面板只发命令，写盘与重建实例的提交边界在这里。
// ---------------------------------------------------------------------------
