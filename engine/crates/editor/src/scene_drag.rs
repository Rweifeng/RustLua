//! Scene View 拖拽 / 框选 / Gizmo 的**提交层**（docs/55 §4.1：View 只发 action）。
//!
//! 面板（`panels/scene_view/`）负责绘制、命中测试与事件采集；"预览 → 事务 → 恰好一条 Undo
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
use rustlua_ecs::spatial_transform::LocalTransform;
use rustlua_ecs::transform::Vec2;
use rustlua_scene::scene::{SceneEntity, SceneEntityId};
use rustlua_ui::ui_layout::UiRectLayout;
use std::collections::BTreeMap;

impl EditorApp {
    /// 拖动预览只改**运行时** ECS：不动文档、不写历史、不落盘。
    /// 真正的提交在 [`Self::finish_scene_drag`]。
    pub(crate) fn apply_drag_preview_to_runtime(&mut self) {
        if self.is_playing() {
            self.scene_state.preview.clear();
            self.scene_state.preview_ui.clear();
            self.begin_drag_preview();
            return;
        }
        // 刚打开的场景可能还没有运行时：先用拖动起点的文档建好，再推进第一帧预览。
        self.ensure_scene_runtime();
        if self.scene_runtime.is_none()
            || (self.scene_state.preview.is_empty() && self.scene_state.preview_ui.is_empty())
        {
            // 拖动已结束（preview 清空）：下一次拖动应当能重新报错，清掉去重记录。
            self.begin_drag_preview();
            return;
        }
        let preview_ui = self.scene_state.preview_ui.clone();
        // docs/54 §8-⑦：预览值是世界变换，但运行时存的是**局部**变换——换算后再推进去，
        // 否则父链下的实体在拖动中会跳到错误的位置（拖完再"弹一下"）。
        let locals = self.scene_drag_locals();
        // 本帧写失败的实体（返回 `false` = 映射表里没它，或它没有 `Transform` 组件）。
        // 只收集、先不改 `self`：下面还要借 `self.scene_runtime`。
        let mut failed: Vec<SceneEntityId> = Vec::new();
        if let Some(runtime) = self.scene_runtime.as_mut() {
            for (id, local) in &locals {
                let applied = runtime.set_scene_entity_transform_preview(
                    *id,
                    Vec2::new(local.translation.x, local.translation.y),
                    2.0 * local.rotation.z.atan2(local.rotation.w),
                    Vec2::new(local.scale.x, local.scale.y),
                );
                if !applied {
                    failed.push(*id);
                }
            }
            for (id, offsets) in preview_ui {
                if !runtime.set_scene_entity_ui_layout_preview(id, offsets) {
                    failed.push(id);
                }
            }
        }
        // 失败必须让用户看见：否则表现是"拖了没反应"，而日志里一个字都没有。
        // 这条路径每帧都跑，所以按实体去重（`preview_failures_reported`），
        // 一次拖动最多每个实体报一条。
        for id in failed {
            if self.scene_state.preview_failures_reported.insert(id) {
                self.push_console_notice(format!(
                    "拖动预览未能写入 {id:?}：运行时的这个实体已不存在或没有 Transform 组件\
                     （拖完松手时文档仍会按拖动结果提交）"
                ));
            }
        }
    }

    /// 开始一次新的拖动预览：清空"已报过失败"的记录。
    ///
    /// 用户重新拖一次时，如果上一次的失败原因已经变了（例如运行时重建过），应当再报一次。
    /// 由 `apply_drag_preview_to_runtime` 在"上一帧没有预览、这一帧有了"时调用——
    /// 那正是拖动的第一帧，不需要 panel 侧再记一个状态。
    pub(crate) fn begin_drag_preview(&mut self) {
        self.scene_state.preview_failures_reported.clear();
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
        self.begin_edit_transaction("场景变换", CommandKind::Transform(first), Some(first));
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
            let encoded = editor::document_space::encode_transform(local);
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

    /// 提交 3D Gizmo 拖拽：把所有目标的 `Transform3D` 一次写入（**一条 Undo 覆盖全部**）。
    ///
    /// docs/55 阶段 2 收口：拖拽目标是整个选中集合，因此这里不再是"只取 preview 的第一条"。
    /// 只要有一个目标的世界变换无法在父链下无损反解，就**整批拒绝**——写一半会让多选拖拽
    /// 变成"部分实体留在原地"的静默错误，这比不写更糟。
    ///
    /// docs/54 §8-⑦：Gizmo 的初值与目标都是**世界**变换，而文档里存的是**局部**值。这里按父链
    /// 反解后再写回，因此父链下的实体不再"拖完跳一下"。
    pub(crate) fn commit_gizmo_drag3d(&mut self) {
        if self.is_playing() {
            self.scene_state.preview3d_transform.clear();
            return;
        }
        let primary = self
            .scene_state
            .gizmo_drag
            .as_ref()
            .map(crate::gizmo::GizmoDrag::primary);
        let preview = std::mem::take(&mut self.scene_state.preview3d_transform);
        if preview.is_empty() {
            return;
        }
        let primary = primary.unwrap_or(preview.keys().next().copied().expect("非空"));
        enum Outcome {
            Written(usize),
            Rejected(Vec<SceneEntityId>),
            Unchanged,
        }
        let outcome = {
            let Some(scene) = self.document.current_scene.as_mut() else {
                return;
            };
            let mut targets = BTreeMap::new();
            for (id, transform) in &preview {
                targets.insert(
                    *id,
                    rustlua_ecs::spatial_transform::LocalTransform::from_3d(*transform),
                );
            }
            let locals = editor::document_space::locals_for_world_targets(&scene.scene, &targets);
            let mut encoded = BTreeMap::new();
            let mut rejected = Vec::new();
            for id in preview.keys() {
                match locals
                    .get(id)
                    .copied()
                    .map(editor::document_space::encode_transform)
                {
                    Some(value) => {
                        encoded.insert(*id, value);
                    }
                    None => rejected.push(*id),
                }
            }
            if !rejected.is_empty() {
                Outcome::Rejected(rejected)
            } else {
                let written = encoded.len();
                let changed = self.history.execute(
                    scene,
                    if written > 1 {
                        "3D 变换（多选）"
                    } else {
                        "3D Transform"
                    },
                    CommandKind::Transform3D(primary),
                    |scene| {
                        for (id, value) in &encoded {
                            if let Some(entity) = scene.scene.entities.get_mut(id) {
                                entity
                                    .components
                                    .insert("Transform".to_owned(), value.clone());
                            }
                        }
                        scene.mark_dirty();
                        Ok::<(), ()>(())
                    },
                );
                if matches!(changed, Ok(true)) {
                    Outcome::Written(written)
                } else {
                    Outcome::Unchanged
                }
            }
        };
        match outcome {
            Outcome::Written(written) => {
                self.status = if written > 1 {
                    format!("已移动 {written} 个实体（一条撤销可全部回退）")
                } else {
                    "已移动实体".to_owned()
                };
                self.save_scene();
                self.sync_inspector(primary);
            }
            Outcome::Rejected(rejected) => self.set_scene_view_diagnostic(format!(
                "{} 个选中实体的世界变换无法在父链下无损反解（剪切/镜像/退化），已整批拒绝写回",
                rejected.len()
            )),
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
            let transform = rustlua_ecs::transform::Transform {
                position: Vec2::new(fields.position_x, fields.position_y),
                // 预览里的旋转是编辑器口径的**度**，引擎的平面 Transform 按**弧度**建四元数：
                // 这里漏一次换算，"拖到 45°"就会落成"绕 Z 转 45 弧度"（画面跳回约 1°）。
                rotation: fields.rotation.to_radians(),
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

#[cfg(test)]
mod tests {
    use crate::EditorApp;
    use crate::gizmo::{GizmoDrag, GizmoDragTargets};
    use editor::{CommandResult, EditorCommand, ProjectInfo, SceneDocument};
    use eframe::egui;
    use rustlua_ecs::transform::Transform3D;
    use rustlua_infra::logging::LogSink;
    use rustlua_math::math3d::{Quat, Vec3};
    use rustlua_scene::scene::SceneEntityId;
    use std::collections::BTreeSet;

    fn app_with_two_entities() -> EditorApp {
        let root = std::env::temp_dir().join(format!(
            "rustlua-drag-commit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        let mut document = SceneDocument::new_scene("multi_gizmo");
        for index in 0..2_u64 {
            let id = SceneEntityId(index + 1);
            document
                .scene
                .add_entity(id, format!("Node{index}"), None)
                .expect("加实体");
            document
                .scene
                .entities
                .get_mut(&id)
                .expect("实体存在")
                .components
                .insert(
                    "Transform".to_owned(),
                    "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
                );
        }
        let mut app = EditorApp::new(
            ProjectInfo {
                root,
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(32),
        );
        app.document.current_scene = Some(document);
        app
    }

    fn transform_of(app: &EditorApp, id: SceneEntityId) -> String {
        app.document.current_scene.as_ref().unwrap().scene.entities[&id].components["Transform"]
            .clone()
    }

    /// docs/55 阶段 2 收口：**3D 多选拖拽 = 一条 Undo 覆盖全部目标**。
    ///
    /// 这条测试钉住的是"多选推广"这件事本身：提交前 `commit_gizmo_drag3d` 只写 preview 的第一条，
    /// 于是"选了 5 个、只动 1 个"却没有任何报错——正是阶段 2 要收掉的那类静默错误。
    #[test]
    fn gizmo_drag3d_commits_every_selected_entity_as_one_undo() {
        let mut app = app_with_two_entities();
        let (first, second) = (SceneEntityId(1), SceneEntityId(2));
        let before_first = transform_of(&app, first);
        let before_second = transform_of(&app, second);

        let initial = Transform3D {
            position: Vec3::new(0.0, 0.0, 0.0),
            rotation: Quat::identity(),
            scale: Vec3::new(1.0, 1.0, 1.0),
        };
        let targets = GizmoDragTargets::new(
            first,
            [(first, initial), (second, initial)],
            &BTreeSet::new(),
        )
        .expect("两个实体都可拖");
        assert_eq!(targets.targets.len(), 2);
        app.scene_state.gizmo_drag = Some(GizmoDrag {
            targets,
            axis: crate::GizmoAxis::X,
            start: egui::Pos2::ZERO,
            origin: initial,
            pivot: None,
            start_ray: None,
        });
        // 两个目标都推进了预览（模拟拖拽中的同一份世界增量）。
        for (id, offset) in [(first, 3.0_f32), (second, 3.0_f32)] {
            app.scene_state.preview3d_transform.insert(
                id,
                Transform3D {
                    position: Vec3::new(offset, 0.0, 0.0),
                    ..initial
                },
            );
        }

        app.commit_gizmo_drag3d();

        assert_eq!(app.history.undo_len(), 1, "一次拖拽只能有一条 Undo");
        assert_ne!(
            transform_of(&app, first),
            before_first,
            "第一个目标必须落盘"
        );
        assert_ne!(
            transform_of(&app, second),
            before_second,
            "第二个目标也必须落盘"
        );
        let after_first = transform_of(&app, first);
        let after_second = transform_of(&app, second);

        // 一条 Undo 把两个实体一起还原。
        assert_eq!(
            app.dispatch(EditorCommand::Undo),
            CommandResult::Applied,
            "撤销必须成功"
        );
        assert_eq!(
            transform_of(&app, first),
            before_first,
            "第一个目标要回到原值"
        );
        assert_eq!(
            transform_of(&app, second),
            before_second,
            "第二个目标要回到原值"
        );
        assert_ne!(after_first, before_first);
        assert_ne!(after_second, before_second);
    }

    /// 锁定实体不进入拖拽目标集合（与"锁定不响应手柄"一致）。
    #[test]
    fn locked_entities_are_excluded_from_the_drag_targets() {
        let first = SceneEntityId(1);
        let second = SceneEntityId(2);
        let initial = Transform3D {
            position: Vec3::new(0.0, 0.0, 0.0),
            rotation: Quat::identity(),
            scale: Vec3::new(1.0, 1.0, 1.0),
        };
        let locked: BTreeSet<SceneEntityId> = [second].into_iter().collect();
        let targets = GizmoDragTargets::new(first, [(first, initial), (second, initial)], &locked)
            .expect("至少有一个可拖目标");
        assert_eq!(targets.targets.len(), 1);
        assert_eq!(targets.targets[0].0, first);
        // 全被锁定 → 不给拖拽集合（调用方据此提前返回，不制造空 Undo）。
        let none = GizmoDragTargets::new(first, [(second, initial)], &locked);
        assert!(none.is_none());
    }
    /// 正常拖动：预览写成功，**不该**产生任何提示。
    ///
    /// 这是这次改动的反向守卫——上报逻辑一加上，最容易犯的错是"条件写反"，
    /// 于是每次拖动都刷一条"预览失败"，比不报还糟。
    #[test]
    fn a_successful_drag_preview_reports_nothing() {
        let mut app = app_with_two_entities();
        let id = SceneEntityId(1);
        // 先把运行时建出来（拖动第一帧会 ensure，这里显式做一次让状态确定）。
        app.ensure_scene_runtime();
        assert!(app.scene_runtime.is_some(), "测试前提：运行时应当能建起来");

        app.scene_state
            .preview
            .insert(id, crate::TransformFields::default());
        app.apply_drag_preview_to_runtime();

        assert!(
            app.console_notices.is_empty(),
            "正常拖动不该产生提示，实际：{:?}",
            app.console_notices
        );
    }

    /// 失效实体：预览写失败必须**上报一次**，且连续帧不重复。
    ///
    /// 两条要求缺一不可：
    /// - 不报 → 表现是"拖了没反应"，日志里一个字都没有（改这条之前的现状）；
    /// - 每帧都报 → 拖动一次刷几百条，日志反而不可读。
    ///
    /// 怎么造"预览写失败"：文档里有这个实体（`scene_drag_locals` 会把它算进去），
    /// 但把运行时 `scene_entities` 映射表清空——这正是"运行时已失效但文档还在"的真实形态
    /// （例如场景重载之后、或实体刚被脚本 destroy）。
    #[test]
    fn a_failed_drag_preview_is_reported_once_not_every_frame() {
        let mut app = app_with_two_entities();
        app.ensure_scene_runtime();
        let id = SceneEntityId(1);
        let Some(runtime) = app.scene_runtime.as_mut() else {
            panic!("测试前提：运行时应当能建起来");
        };
        // 清空 id → 句柄映射：`set_scene_entity_transform_preview` 从此对这个 id 返回 false。
        runtime.with_script(|script| {
            let entities = script.entity_state_handle();
            entities.with_mut(|state| state.scene_entities.clear());
        });

        app.scene_state
            .preview
            .insert(id, crate::TransformFields::default());

        app.apply_drag_preview_to_runtime();
        let after_first = app.console_notices.len();
        assert_eq!(
            after_first, 1,
            "失效实体的预览失败必须报一条，实际 {} 条：{:?}",
            after_first, app.console_notices
        );
        assert!(
            app.console_notices[0].contains("1"),
            "提示里应当点出是哪个实体，实际：{}",
            app.console_notices[0]
        );

        // 模拟后续帧：同一个失效实体，一次都不该再报。
        for frame in 1..=5 {
            // 每帧都要重新插入 preview——真实拖动里它一直在（被 pointer move 覆盖写）。
            app.scene_state
                .preview
                .insert(id, crate::TransformFields::default());
            app.apply_drag_preview_to_runtime();
            assert_eq!(
                app.console_notices.len(),
                after_first,
                "第 {frame} 帧又报了一次——拖动会刷屏（去重没生效）"
            );
        }

        // 拖动结束后重新开始：应当能再报一次（用户换了情况要能看到）。
        app.scene_state.preview.clear();
        app.apply_drag_preview_to_runtime(); // preview 空 → 清掉去重记录
        app.scene_state
            .preview
            .insert(id, crate::TransformFields::default());
        app.apply_drag_preview_to_runtime();
        assert_eq!(
            app.console_notices.len(),
            after_first + 1,
            "新一次拖动应当能重新报错，实际仍是 {} 条",
            app.console_notices.len()
        );
    }
}
