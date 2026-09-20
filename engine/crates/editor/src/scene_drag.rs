//! Scene View 拖拽 / 框选 / Gizmo 的**提交层**（docs/55 §4.1：View 只发 action）。
//!
//! 面板（`panels/scene_view.rs`）负责绘制、命中测试与事件采集；"预览 → 事务 → 恰好一条 Undo
//! → 保存"这串**提交逻辑**住在这里。它握着 `history.execute` 与文档写入，留在面板文件里会让
//! "View 只发 typed action"这条规则失去可检性——架构棘轮
//! （`panels/architecture_tests.rs`）直接扫描面板源文件，把这种"顺手写文档"钉成欠账。
//!
//! 语义与迁移前完全一致（E07/E08）：
//!
//! - 拖动过程中只把预览推进**运行时 ECS**（不碰文档、不写历史、不落盘）；
//! - 松手时把最后一帧预览写入文档并提交活动事务 → 恰好一条 Undo + 一次保存 + Inspector 同步；
//! - 世界 → 局部的换算交给 `document_space`，**无法无损反解时拒绝写回**（不静默近似）；
//! - Play 中一律拒绝（运行态与编辑态隔离）。

use crate::EditorApp;
use editor::CommandKind;
use engine::core::{
    scene::{SceneEntity, SceneEntityId},
    spatial_transform::LocalTransform,
    transform::Vec2,
    ui_layout::UiRectLayout,
};
use std::collections::BTreeMap;

impl EditorApp {
    /// 拖动预览只改**运行时** ECS：不动文档、不写历史、不落盘。
    /// 真正的提交在 [`Self::finish_scene_drag`]。
    pub(crate) fn apply_drag_preview_to_runtime(&mut self) {
        if self.is_playing() {
            self.scene_state.preview.clear();
            self.scene_state.preview_ui.clear();
            return;
        }
        // 刚打开的场景可能还没有运行时：先用拖动起点的文档建好，再推进第一帧预览。
        self.ensure_scene_runtime();
        if self.scene_runtime.is_none()
            || (self.scene_state.preview.is_empty() && self.scene_state.preview_ui.is_empty())
        {
            return;
        }
        let preview_ui = self.scene_state.preview_ui.clone();
        // docs/54 §8-⑦：预览值是世界变换，但运行时存的是**局部**变换——换算后再推进去，
        // 否则父链下的实体在拖动中会跳到错误的位置（拖完再"弹一下"）。
        let locals = self.scene_drag_locals();
        if let Some(runtime) = self.scene_runtime.as_mut() {
            for (id, local) in &locals {
                runtime.set_scene_entity_transform_preview(
                    *id,
                    Vec2::new(local.translation.x, local.translation.y),
                    2.0 * local.rotation.z.atan2(local.rotation.w),
                    Vec2::new(local.scale.x, local.scale.y),
                );
            }
            for (id, offsets) in preview_ui {
                runtime.set_scene_entity_ui_layout_preview(id, offsets);
            }
        }
    }

    /// E08：拖动松手后把最新 preview 写入编辑文档并 mark_dirty。首次调用时若尚未处于事务中，
    /// 自动 begin（事务 before = 拖动前文档快照，保证 undo 能一次性回到拖动前）。
    /// 若拖动没有净变化，commit 会被 diff 判定为空操作。
    pub(crate) fn apply_drag_preview_to_document(&mut self) {
        if self.is_playing() {
            self.scene_state.preview.clear();
            self.scene_state.preview_ui.clear();
            return;
        }
        // 世界 → 局部**必须在 take 之前算**：`scene_drag_locals()` 读的就是
        // `scene_state.preview`（先 take 会让 locals 恒为空 → 文档写入被静默跳过）。
        // 这个顺序错误由 `scene_view::tests::synthetic_pointer_drag_commits_exactly_one_undo`
        // （帧级合成拖动）当场抓出：拖动全程有预览、松手也提交了事务，但文档一字未改。
        let locals = self.scene_drag_locals();
        let preview = std::mem::take(&mut self.scene_state.preview);
        let preview_ui = std::mem::take(&mut self.scene_state.preview_ui);
        let first = preview
            .keys()
            .next()
            .copied()
            .or_else(|| preview_ui.keys().next().copied());
        let Some(first) = first else {
            return;
        };
        // 写入文档前开启事务（幂等：已开启则直接沿用）。
        self.begin_edit_transaction(
            "Scene Transform",
            CommandKind::Transform(first),
            Some(first),
        );
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        for (id, transform) in &preview {
            // docs/54 §8-⑦：`preview` 是**世界**变换，文档里的 `Transform` 是**局部**值；
            // 直接用世界值覆盖会让父链下的实体拖完跳一下（父的变换被叠加两次）。
            let Some(local) = locals.get(id).copied() else {
                // 无法无损反解（剪切/镜像/退化）：拒绝写回，不静默近似。
                continue;
            };
            let encoded = editor::document_space::encode_transform_2d(local);
            if let Some(entity) = scene.scene.entities.get_mut(id) {
                entity.components.insert("Transform".to_owned(), encoded);
            }
            let _ = transform;
        }
        for (id, position) in &preview_ui {
            if let Some(entity) = scene.scene.entities.get_mut(id) {
                set_ui_component_layout(entity, *position);
            }
        }
        scene.mark_dirty();
    }

    /// E08：2D 拖拽松手 —— 先落最后一帧预览，再提交活动事务（单条 Undo +
    /// 一次保存 + Inspector 同步）。无净变化时为空操作提交，不保存不写历史。
    pub(crate) fn finish_scene_drag(&mut self) {
        if self.is_playing() {
            self.scene_state.preview.clear();
            self.scene_state.preview_ui.clear();
            return;
        }
        self.apply_drag_preview_to_document();
        let first = self
            .scene_state
            .drag
            .as_ref()
            .and_then(|drag| drag.ids.first())
            .copied();
        if self.is_edit_transaction_active() && self.commit_edit_transaction() {
            self.save_scene();
            if let Some(first) = first {
                self.sync_inspector(first);
            }
        }
    }

    /// 提交 3D 平移 Gizmo 拖拽：只改 Transform3D 的 position，保留旋转/缩放。
    ///
    /// docs/54 §8-⑦：Gizmo 的初值与目标都是**世界**变换，而文档里存的是**局部**值。这里按父链
    /// 反解后再写回，因此父链下的实体不再"拖完跳一下"；反解不了（剪切/镜像/退化）时**拒绝写回**
    /// 并给出提示，不静默近似。
    pub(crate) fn commit_gizmo_drag3d(&mut self) {
        if self.is_playing() {
            self.scene_state.preview3d_transform.clear();
            return;
        }
        let preview = std::mem::take(&mut self.scene_state.preview3d_transform);
        let Some((id, transform)) = preview
            .iter()
            .next()
            .map(|(id, transform)| (*id, *transform))
        else {
            return;
        };
        enum Outcome {
            Written,
            Rejected,
            Unchanged,
        }
        let outcome = {
            let Some(scene) = self.document.current_scene.as_mut() else {
                return;
            };
            let mut targets = BTreeMap::new();
            targets.insert(
                id,
                engine::core::spatial_transform::LocalTransform::from_3d(transform),
            );
            match editor::document_space::locals_for_world_targets(&scene.scene, &targets)
                .get(&id)
                .copied()
                .map(editor::document_space::encode_transform_3d)
            {
                Some(encoded) => {
                    let changed = self.history.execute(
                        scene,
                        "3D Transform",
                        CommandKind::Transform3D(id),
                        |scene| {
                            if let Some(entity) = scene.scene.entities.get_mut(&id) {
                                entity
                                    .components
                                    .insert("Transform3D".to_owned(), encoded.clone());
                            }
                            scene.mark_dirty();
                            Ok::<(), ()>(())
                        },
                    );
                    if matches!(changed, Ok(true)) {
                        Outcome::Written
                    } else {
                        Outcome::Unchanged
                    }
                }
                None => Outcome::Rejected,
            }
        };
        match outcome {
            Outcome::Written => {
                self.save_scene();
                self.sync_inspector(id);
            }
            Outcome::Rejected => self.set_scene_view_diagnostic(
                "该实体的世界变换无法在父链下无损反解（剪切/镜像/退化），已拒绝写回".to_owned(),
            ),
            Outcome::Unchanged => {}
        }
    }

    /// 拖动预览 → 文档**局部**变换（世界 → 局部换算，父链下不"跳一下"）。
    pub(crate) fn scene_drag_locals(&self) -> BTreeMap<SceneEntityId, LocalTransform> {
        let Some(scene) = self.document.current_scene.as_ref() else {
            return BTreeMap::new();
        };
        let mut targets = BTreeMap::new();
        for (id, fields) in &self.scene_state.preview {
            let transform = engine::core::transform::Transform {
                position: Vec2::new(fields.position_x, fields.position_y),
                rotation: fields.rotation,
                scale: Vec2::new(fields.scale_x, fields.scale_y),
            };
            if let Ok(local) = LocalTransform::from_2d(transform) {
                targets.insert(*id, local);
            }
        }
        editor::document_space::locals_for_world_targets(&scene.scene, &targets)
    }
}

/// 把 UiLayout 的 offsets 写回组件串；非 UI 实体返回 `false`（不改动）。
pub(crate) fn set_ui_component_layout(entity: &mut SceneEntity, offsets: [f32; 4]) -> bool {
    let Some(raw) = entity.components.get("UiLayout") else {
        return false;
    };
    let Ok(mut layout) = UiRectLayout::from_component_str(raw) else {
        return false;
    };
    layout.offsets = offsets;
    entity
        .components
        .insert("UiLayout".to_owned(), layout.to_component_str());
    true
}
