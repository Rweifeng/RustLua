//! 指针交互：按下/拖动/松开的状态机、拖动读数、2D 旋转环、选中框与容器内边距命中。
//! 交互结果写回 `scene_interaction::ToolPhase`，文档改动统一经 `actions` 提交。

use super::*;

impl EditorApp {
    /// 指针按下：喂状态机 → 按相位决定"开始拖动"（命中实体）还是"开始框选"（空白处）。
    ///
    /// 返回按下后的相位，便于调用方/测试直接断言。
    pub(crate) fn scene_pointer_down(
        &mut self,
        pos: egui::Pos2,
        entities: &[SceneBoxedEntity],
        additive: bool,
    ) -> ToolPhase {
        let hit = pick_scene_2d_entity(entities, pos, &self.scene_state.locked);
        // E11：把指针事件交给交互状态机，由它决定阶段（拖动 / 框选）。
        self.interaction.handle(InteractionEvent::PointerDown {
            pos: [pos.x, pos.y],
            hit: hit.is_some(),
        });
        // 阶段语义由状态机给出：`affects_selection()` = 框选（起点在阶段里），
        // 否则就是拖动（按在实体上）。这里不再自己判断"有没有 box_select_start"。
        if self.interaction.phase().affects_selection() {
            if !additive {
                self.selection.clear(SelectionSource::SceneView2D);
            }
        } else if let Some(hit_id) = hit {
            let source = SelectionSource::SceneView2D;
            if additive {
                self.selection.add(hit_id, source);
            } else if !self.selection.contains(hit_id) {
                self.selection.set_single(hit_id, source);
            }
            self.sync_inspector(hit_id);
            let ids: Vec<_> = self
                .selection
                .ids()
                .into_iter()
                .filter(|selected| !self.scene_state.locked.contains(selected))
                .collect();
            let initial = entities
                .iter()
                .filter(|entity| ids.contains(&entity.id))
                .map(|entity| (entity.id, entity.transform))
                .collect();
            let ui_initial = entities
                .iter()
                .filter(|entity| ids.contains(&entity.id))
                .filter_map(|entity| {
                    entity.ui.as_ref().map(|ui| {
                        (
                            entity.id,
                            UiDragInitial {
                                offsets: ui.layout.offsets,
                                anchors: ui.layout.anchors,
                            },
                        )
                    })
                })
                .collect();
            self.scene_state.drag = Some(SceneDrag {
                ids,
                start: pos,
                initial,
                ui_initial,
            });
        }
        self.interaction.phase()
    }

    /// 指针拖动：推进状态机（相位里的 `current` 就是框选矩形的另一端）。
    pub(crate) fn scene_pointer_drag(&mut self, pos: egui::Pos2) -> ToolPhase {
        self.interaction.handle(InteractionEvent::PointerDrag {
            pos: [pos.x, pos.y],
        });
        self.interaction.phase()
    }

    /// 拖动中的**预览推进**（从 `draw_scene_2d` 抽出）：按拖动起点与当前指针算出每个选中
    /// 实体的预览变换 / UiLayout offsets，然后**只**推进运行时 ECS——文档与历史不动，
    /// 提交在 [`Self::scene_pointer_up`]。
    ///
    /// 抽出来的理由与按下/拖动/释放三个入口相同（docs/55 阶段 2 验收："绘制与交互可独立
    /// 测试"）：整条拖动序列可以在没有 egui 帧、没有 GPU 的情况下合成出来并断言。
    /// `scale` = `ui.ctx().pixels_per_point()`。
    pub(crate) fn scene_pointer_drag_apply(&mut self, pos: egui::Pos2, scale: f32) {
        let Some(drag) = self.scene_state.drag.as_ref() else {
            return;
        };
        let delta = pos - drag.start;
        let mut preview = std::collections::BTreeMap::new();
        let mut preview_ui = std::collections::BTreeMap::new();
        let mut preview_anchors = std::collections::BTreeMap::new();
        let mut preview_padding = std::collections::BTreeMap::new();
        let pivot = scene_drag_pivot(self.gizmo_pivot, self.selection.primary(), &drag.initial);
        // 转角与轨道**共用同一个数**：吸附生效时两者一起跳，不会出现"框在转、人没转到那个角度"。
        // 单位是**度**的步长（`snap_angle_step`），不是位置的 `snap_step`——把 0.5 个世界单位
        // 当成 0.5° 用不叫吸附。
        let raw_angle = (delta.x * 0.5).to_radians();
        let angle =
            crate::gizmo::snap_angle(raw_angle, self.snap_enabled.then_some(self.snap_angle_step));
        let factor = crate::gizmo::scale_factor(delta, egui::vec2(1.0, 0.0));
        // Phase 5d：padding 模式——单独处理，不依赖 ui_initial。
        if let Some(handle) = self.scene_state.padding_handle {
            if let Some((initial_padding, outer_rect)) = self.scene_state.padding_drag_initial
                && let Some(primary) = self.selection.primary()
            {
                let design_w = self.preview_resolution.0 as f32;
                let panel_w = outer_rect.width();
                let design_to_panel_scale = panel_w / design_w.max(1.0);
                let new_padding = scene_resize::compute_padding_values(
                    initial_padding,
                    delta,
                    design_to_panel_scale,
                    handle,
                );
                preview_padding.insert(primary, new_padding);
            }
            self.scene_state.preview_padding = preview_padding;
            self.apply_drag_preview_to_runtime();
            return;
        }
        for id in &drag.ids {
            // UI 组件拖拽：只预览 UiLayout offsets/anchors，不写 Transform。
            if let Some(ui) = drag.ui_initial.get(id) {
                if let Some(handle) = self.scene_state.anchor_handle {
                    // Phase 5c：anchor 模式——按手柄修改 anchors。
                    let parent_panel_size = egui::vec2(
                        self.preview_resolution.0 as f32 * self.scene_zoom / scale,
                        self.preview_resolution.1 as f32 * self.scene_zoom / scale,
                    );
                    let new_anchors = scene_resize::compute_anchor_values(
                        ui.anchors,
                        delta,
                        parent_panel_size,
                        handle,
                    );
                    preview_anchors.insert(*id, new_anchors);
                    // anchor 模式下 offsets 保持不变。
                    preview_ui.insert(*id, ui.offsets);
                } else {
                    let per_axis = egui::vec2(self.scene_zoom / scale, self.scene_zoom / scale);
                    let design_delta =
                        scene_resize::panel_delta_to_design_delta(delta, self.scene_zoom, scale);
                    let new_offsets = if let Some(handle) = self.scene_state.resize_handle {
                        // Phase 5a：resize 模式——按手柄只改对应边的 offset。
                        scene_resize::compute_resize_offsets(ui.offsets, design_delta, handle)
                    } else {
                        // 正常平移模式——四边同移。
                        [
                            ui.offsets[0] + delta.x / per_axis.x,
                            ui.offsets[1] + delta.y / per_axis.y,
                            ui.offsets[2] + delta.x / per_axis.x,
                            ui.offsets[3] + delta.y / per_axis.y,
                        ]
                    };
                    preview_ui.insert(*id, new_offsets);
                    // 非 anchor 模式下 anchors 保持不变。
                    preview_anchors.insert(*id, ui.anchors);
                }
                continue;
            }
            let initial = drag.initial[id];
            let next = scene_drag_transform(
                initial,
                pivot,
                delta,
                self.scene_tool(),
                self.snap_enabled,
                self.snap_step,
                scale,
                self.scene_zoom,
                angle,
                factor,
            );
            preview.insert(*id, next);
        }
        self.scene_state.preview = preview;
        self.scene_state.preview_ui = preview_ui;
        self.scene_state.preview_anchors = preview_anchors;
        // 拖动中只把预览写入运行时 ECS，避免每帧重建 Lua/ECS；松手时由
        // finish_scene_drag 事务化写入文档，生成单条 Undo + 单次保存。
        self.apply_drag_preview_to_runtime();
    }

    /// 指针释放：提交拖动（一条 Undo）并让状态机回到 Idle。
    pub(crate) fn scene_pointer_up(&mut self, pos: egui::Pos2) -> ToolPhase {
        self.finish_scene_drag();
        self.scene_state.drag = None;
        self.scene_state.resize_handle = None;
        self.scene_state.anchor_handle = None;
        self.scene_state.padding_handle = None;
        self.scene_state.padding_drag_initial = None;
        self.interaction.handle(InteractionEvent::PointerUp {
            pos: [pos.x, pos.y],
        });
        self.interaction.phase()
    }

    /// M27-02（缺项 3）：绘制 2D 旋转工具的**圆环 + 朝向把手**。
    ///
    /// 圆心不是另挑的一个参考点，而是这一趟拖拽**真实的转轴**（[`scene_drag_pivot`]：
    /// `Pivot` = 主选实体自身，`Center` = 拖动集合的均值），所以切换轴心模式时环会跟着
    /// 挪动，用户看到的圆心就是角度绕着转的那个点。把手挂在主选实体当前朝向上，
    /// "往把手方向拖"与"横着拖多少像素"因此是同一件事。
    pub(crate) fn draw_scene_2d_rotation_ring(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        pixels_per_point: f32,
        entities: &[SceneBoxedEntity],
    ) {
        // 空闲时环跟着当前选中集合走；拖动中必须用**拖拽初始**变换——预览后的位置会绕
        // 圆心公转，用预览值算圆心会让环自己转着跑。
        let idle: std::collections::BTreeMap<SceneEntityId, TransformFields> =
            if self.scene_state.drag.is_none() {
                entities
                    .iter()
                    .filter(|entity| {
                        self.selection.contains(entity.id)
                            && !self.scene_state.locked.contains(&entity.id)
                    })
                    .map(|entity| (entity.id, entity.transform))
                    .collect()
            } else {
                std::collections::BTreeMap::new()
            };
        let initial: &std::collections::BTreeMap<SceneEntityId, TransformFields> =
            match &self.scene_state.drag {
                Some(drag) => &drag.initial,
                None => &idle,
            };
        if initial.is_empty() {
            return;
        }
        let (px, py) = scene_drag_pivot(self.gizmo_pivot, self.selection.primary(), initial);
        let center = design_to_panel_pos(
            rect,
            self.scene_state.pan,
            self.scene_zoom,
            pixels_per_point,
            self.game_camera_center_egui(),
            self.preview_resolution,
            egui::pos2(px, py),
        );
        let radius = entities
            .iter()
            .filter(|entity| initial.contains_key(&entity.id))
            .map(|entity| (entity.center - center).length() + entity.size.length() * 0.5)
            .fold(28.0_f32, f32::max);
        let color = egui::Color32::from_rgb(230, 160, 60);
        painter.circle_stroke(center, radius, egui::Stroke::new(2.0_f32, color));
        let knob = self
            .selection
            .primary()
            .and_then(|id| entities.iter().find(|entity| entity.id == id))
            .or_else(|| {
                entities
                    .iter()
                    .find(|entity| initial.contains_key(&entity.id))
            });
        if let Some(knob) = knob {
            // 设计分辨率 y 向上、面板 y 向下，所以屏幕角取负。
            let theta = knob.transform.rotation.to_radians();
            let point = center + egui::vec2(theta.cos(), -theta.sin()) * radius;
            painter.line_segment([center, point], egui::Stroke::new(1.0_f32, color));
            painter.circle_filled(point, 5.0, color);
        }
    }

    /// M27-02（缺项 7）：在指针旁绘制本次拖动的即时数值（位置 / 角度 / 缩放）。
    ///
    /// 只读**已生效**的包围框变换（`collect_scene_2d_entities` 已经把 in-flight 预览叠加
    /// 进去了），所以浮标和画面上的框永远一致；差值相对拖拽初值，与 Inspector 里的绝对值
    /// 互补——这里回答的是"我刚才转了多少度"。
    pub(crate) fn draw_scene_2d_drag_readout(
        &self,
        painter: &egui::Painter,
        pointer: egui::Pos2,
        entities: &[SceneBoxedEntity],
    ) {
        let Some(drag) = self.scene_state.drag.as_ref() else {
            return;
        };
        let Some(primary) = self.selection.primary() else {
            return;
        };
        // 主选实体不在这一趟拖拽集合里（按下空白后框选到别的实体）→ 没有差值可报。
        let Some(initial) = drag.initial.get(&primary) else {
            return;
        };
        let Some(entity) = entities.iter().find(|entity| entity.id == primary) else {
            return;
        };
        let current = entity.transform;
        let text = crate::gizmo::readout_2d(
            self.scene_tool(),
            (current.position_x, current.position_y),
            current.rotation - initial.rotation,
            (current.scale_x, current.scale_y),
        );
        if text.is_empty() {
            return;
        }
        painter.text(
            pointer + egui::vec2(14.0, 14.0),
            egui::Align2::LEFT_TOP,
            text,
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(240, 220, 120),
        );
    }

    /// Phase 5a：返回当前唯一选中 UI 实体的面板包围框（仅 Widget 模式、单选时有意义）。
    pub(crate) fn selected_ui_entity_rect(
        &self,
        pointer: egui::Pos2,
        entities: &[SceneBoxedEntity],
    ) -> Option<egui::Rect> {
        if self.scene_view_mode != SceneViewMode::Widget {
            return None;
        }
        let primary = self.selection.primary()?;
        let entity = entities.iter().find(|e| e.id == primary)?;
        entity.ui.as_ref()?;
        let rect = egui::Rect::from_center_size(entity.center, entity.size);
        // 指针必须在包围框附近（留出手柄大小的余量）。
        let margin = scene_resize::HANDLE_HALF_SIZE + 4.0;
        let expanded = rect.expand(margin);
        expanded.contains(pointer).then_some(rect)
    }

    /// Phase 5c：返回主选 UI 实体的锚点矩形（面板坐标），用于锚点手柄命中测试。
    pub(crate) fn selected_ui_anchor_rect(
        &self,
        _pointer: egui::Pos2,
        entities: &[SceneBoxedEntity],
    ) -> Option<egui::Rect> {
        if self.scene_view_mode != SceneViewMode::Widget {
            return None;
        }
        let primary = self.selection.primary()?;
        let entity = entities.iter().find(|e| e.id == primary)?;
        let ui_entity = entity.ui.as_ref()?;
        // 需要面板矩形来调用 design_rect_to_panel。
        let panel_rect = self.scene_canvas_rect;
        if panel_rect.is_negative() {
            return None;
        }
        let parent_w = self.preview_resolution.0 as f32;
        let parent_h = self.preview_resolution.1 as f32;
        let anchor_origin = [
            ui_entity.layout.anchors[0] * parent_w,
            ui_entity.layout.anchors[1] * parent_h,
        ];
        let anchor_size = [
            (ui_entity.layout.anchors[2] - ui_entity.layout.anchors[0]) * parent_w,
            (ui_entity.layout.anchors[3] - ui_entity.layout.anchors[1]) * parent_h,
        ];
        Some(design_rect_to_panel(
            panel_rect,
            self.scene_state.pan,
            self.scene_zoom,
            self.scene_canvas_rect.width().max(1.0) / panel_rect.width().max(1.0),
            self.game_camera_center_egui(),
            self.preview_resolution,
            anchor_origin,
            anchor_size,
        ))
    }

    /// Phase 5d：命中测试选中 Container 实体的 padding 手柄。
    /// 返回 (手柄, 外框矩形, 当前 padding, 设计→面板缩放)。
    pub(crate) fn selected_container_padding_hit(
        &self,
        pointer: egui::Pos2,
        entities: &[SceneBoxedEntity],
    ) -> Option<scene_resize::PaddingHandle> {
        if self.scene_view_mode != SceneViewMode::Widget {
            return None;
        }
        let primary = self.selection.primary()?;
        let entity = entities.iter().find(|e| e.id == primary)?;
        let scene = panel_scene_document(&self.play_session, &self.document)?;
        let scene_entity = scene.scene.entities.get(&primary)?;
        let container_raw = scene_entity.components.get("Container")?;
        let style =
            rustlua_ui::ui_layout::ContainerStyle::from_component_str(container_raw).ok()?;
        let padding = self
            .scene_state
            .preview_padding
            .get(&primary)
            .copied()
            .unwrap_or(style.padding);
        let outer_rect = egui::Rect::from_center_size(entity.center, entity.size);
        let design_w = self.preview_resolution.0 as f32;
        let panel_w = outer_rect.width();
        let design_to_panel_scale = panel_w / design_w.max(1.0);
        scene_resize::hit_test_padding_handle(outer_rect, padding, design_to_panel_scale, pointer)
    }
}
