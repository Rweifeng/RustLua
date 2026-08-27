//! 行3 Scene 视图（2D/3D）：渲染、选择、Gizmo、框选、拖放生成实体。

use crate::EditorApp;
use crate::geometry::{
    encode_transform, parse_transform, polygon_intersects_rect, rotated_contains, rotated_corners,
};
use crate::util::icon_toggle;
use crate::util::strip_verbatim_prefix;
use crate::{
    GizmoAxis, GizmoDrag, SceneDrag, SceneTool, SceneViewCache2d, SceneViewCache3d, SceneViewMode,
};
use editor::{CommandKind, TransformFields};
use eframe::egui;
use engine::core::{
    camera3d::{Camera3D, Ray3},
    light::Light,
    math2d::Camera2D,
    math3d::{Quat, Vec3},
    prefab::Prefab,
    scene::SceneEntityId,
    scene_extract::extract_serialized_scene_3d,
    tilemap::Tilemap,
    transform::Vec2,
    transform3d::Transform3D,
};
use engine::host::{
    BoundsSource, ReadbackStatus, Renderer, RuntimeScene, Scene3d, Scene3dBounds, Scene3dDrawable,
    SceneMesh,
};
use std::path::Path;

impl EditorApp {
    pub(crate) fn draw_scene_view(&mut self, ui: &mut egui::Ui) {
        if self.scene_view_mode == SceneViewMode::View3D {
            self.draw_scene_view_3d(ui);
            return;
        }
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let rect = response.rect;
        // 拖放：把资源浏览器中的纹理拖到场景创建精灵。
        let (_, dropped_texture) = ui
            .dnd_drop_zone::<String, egui::Response>(egui::Frame::default(), |ui| {
                ui.allocate_rect(rect, egui::Sense::hover())
            });
        if let Some(path) = dropped_texture
            && let Err(error) = self.spawn_dropped_resource(&path)
        {
            self.status = error;
        }
        // 仅当鼠标悬停在 Scene 视图内时才响应滚轮缩放（避免在其它面板滚动误触发）。
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if response.contains_pointer() && scroll.abs() > 0.0 {
            self.scene_zoom = (self.scene_zoom * (1.0 + scroll * 0.001)).clamp(0.25, 4.0);
        }
        if response.dragged_by(egui::PointerButton::Middle)
            || (response.dragged_by(egui::PointerButton::Secondary)
                && self.scene_tool != SceneTool::TilePaint)
        {
            self.scene_state.pan += response.drag_delta();
        }
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(28, 32, 40));
        // 实时渲染编辑场景（精灵/图块/文本）作为背景。
        let scale = ui.ctx().pixels_per_point();
        let view_w = (rect.width() * scale) as u32;
        let view_h = (rect.height() * scale) as u32;
        self.render_scene_view_2d(view_w, view_h, ui.ctx());
        if let Some(tex) = &self.scene_2d_tex {
            painter.image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Scene View  -  2D",
            egui::FontId::proportional(16.0),
            egui::Color32::LIGHT_GRAY,
        );
        let filter = self.hierarchy_filter.to_lowercase();
        let entities: Vec<_> = self
            .current_scene()
            .map(|s| {
                s.scene
                    .entities
                    .values()
                    .filter_map(|e| {
                        if !filter.trim().is_empty()
                            && !e.name.to_lowercase().contains(filter.trim())
                        {
                            return None;
                        }
                        if self.scene_state.hidden.contains(&e.id) {
                            return None;
                        }
                        let transform = self
                            .scene_state
                            .preview
                            .get(&e.id)
                            .copied()
                            .or_else(|| {
                                e.components
                                    .get("Transform")
                                    .and_then(|v| parse_transform(v))
                            })
                            .unwrap_or_default();
                        Some((e.id, e.name.clone(), transform))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let tilemap_context = self.selected_tilemap_context();
        for (id, name, transform) in entities.iter() {
            let center = egui::pos2(
                rect.center().x + self.scene_state.pan.x + transform.position_x * self.scene_zoom,
                rect.center().y + self.scene_state.pan.y - transform.position_y * self.scene_zoom,
            );
            let size = egui::vec2(
                (112.0 * transform.scale_x.abs().max(0.25)) * self.scene_zoom,
                (44.0 * transform.scale_y.abs().max(0.25)) * self.scene_zoom,
            );
            let box_rect = egui::Rect::from_center_size(center, size);
            let selected =
                self.tree.selected() == Some(*id) || self.scene_state.selected.contains(id);
            // 场景已实时渲染，实体只画选中轮廓 + 名称。
            if selected {
                painter.rect_stroke(
                    box_rect,
                    0.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(230, 170, 60)),
                    egui::StrokeKind::Outside,
                );
                painter.text(
                    center + egui::vec2(0.0, -size.y * 0.5 - 6.0),
                    egui::Align2::CENTER_BOTTOM,
                    name,
                    egui::FontId::proportional(14.0),
                    egui::Color32::LIGHT_YELLOW,
                );
            }
            if response.clicked()
                && response
                    .interact_pointer_pos()
                    .is_some_and(|pos| box_rect.contains(pos))
            {
                let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                if additive {
                    if self.scene_state.selected.contains(id) {
                        self.scene_state.selected.remove(id);
                    } else {
                        self.scene_state.selected.insert(*id);
                    }
                } else {
                    self.scene_state.selected.clear();
                    self.scene_state.selected.insert(*id);
                }
                if let Some(scene) = self.current_scene() {
                    let scene = scene.clone();
                    let _ = self.tree.select(&scene, *id);
                }
                self.sync_inspector(*id);
            }
        }

        let pointer = response.interact_pointer_pos();
        let hovered_tile = pointer.and_then(|pos| {
            tilemap_context.as_ref().and_then(|context| {
                tilemap_cell_from_pointer(
                    rect,
                    self.scene_state.pan,
                    self.scene_zoom,
                    pos,
                    context.transform,
                    &self.tilemap.map,
                )
            })
        });
        if let Some(context) = tilemap_context.as_ref() {
            self.draw_tilemap_overlay(&painter, rect, context, hovered_tile);
        }
        if let Some(context) = tilemap_context.as_ref()
            && self.scene_tool == SceneTool::TilePaint
        {
            self.handle_tilemap_paint(&response, rect, context);
            self.draw_scene_tool_panel(ui, rect);
            return;
        }
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pos) = pointer
        {
            let hit = entities.iter().rev().find(|(id, _, transform)| {
                let center = egui::pos2(
                    rect.center().x
                        + self.scene_state.pan.x
                        + transform.position_x * self.scene_zoom,
                    rect.center().y + self.scene_state.pan.y
                        - transform.position_y * self.scene_zoom,
                );
                let size = egui::vec2(
                    (112.0 * transform.scale_x.abs().max(0.25)) * self.scene_zoom,
                    (44.0 * transform.scale_y.abs().max(0.25)) * self.scene_zoom,
                );
                rotated_contains(center, size, transform.rotation, pos)
                    && !self.scene_state.locked.contains(id)
            });
            if let Some((id, _, _)) = hit {
                let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                if !additive && !self.scene_state.selected.contains(id) {
                    self.scene_state.selected.clear();
                }
                self.scene_state.selected.insert(*id);
                if let Some(scene) = self.current_scene() {
                    let scene = scene.clone();
                    let _ = self.tree.select(&scene, *id);
                }
                self.sync_inspector(*id);
                let ids: Vec<_> = self
                    .scene_state
                    .selected
                    .iter()
                    .copied()
                    .filter(|selected| !self.scene_state.locked.contains(selected))
                    .collect();
                let initial = entities
                    .iter()
                    .filter(|(entity_id, _, _)| ids.contains(entity_id))
                    .map(|(entity_id, _, value)| (*entity_id, *value))
                    .collect();
                self.scene_state.drag = Some(SceneDrag {
                    ids,
                    start: pos,
                    initial,
                });
            } else {
                self.scene_state.box_select_start = Some(pos);
                if !ui.input(|input| input.modifiers.command || input.modifiers.ctrl) {
                    self.scene_state.selected.clear();
                }
            }
        }
        if let (Some(start), Some(current)) = (self.scene_state.box_select_start, pointer)
            && response.dragged_by(egui::PointerButton::Primary)
        {
            let selection_rect = egui::Rect::from_two_pos(start, current);
            painter.rect_stroke(
                selection_rect,
                0.0,
                egui::Stroke::new(1.0_f32, egui::Color32::LIGHT_BLUE),
                egui::StrokeKind::Outside,
            );
            for (id, _, transform) in &entities {
                let center = egui::pos2(
                    rect.center().x
                        + self.scene_state.pan.x
                        + transform.position_x * self.scene_zoom,
                    rect.center().y + self.scene_state.pan.y
                        - transform.position_y * self.scene_zoom,
                );
                let size = egui::vec2(
                    (112.0 * transform.scale_x.abs().max(0.25)) * self.scene_zoom,
                    (44.0 * transform.scale_y.abs().max(0.25)) * self.scene_zoom,
                );
                let corners = rotated_corners(center, size, transform.rotation);
                let contained = corners
                    .iter()
                    .all(|corner| selection_rect.contains(*corner));
                let intersects = polygon_intersects_rect(&corners, selection_rect);
                let left_to_right = current.x >= start.x;
                if (left_to_right && contained) || (!left_to_right && intersects) {
                    self.scene_state.selected.insert(*id);
                }
            }
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let (Some(drag), Some(pos)) = (self.scene_state.drag.as_ref(), pointer)
        {
            let delta = pos - drag.start;
            let mut preview = std::collections::BTreeMap::new();
            let pivot = {
                let count = drag.initial.len().max(1) as f32;
                let x = drag
                    .initial
                    .values()
                    .map(|value| value.position_x)
                    .sum::<f32>()
                    / count;
                let y = drag
                    .initial
                    .values()
                    .map(|value| value.position_y)
                    .sum::<f32>()
                    / count;
                (x, y)
            };
            let angle = (delta.x * 0.5).to_radians();
            let factor = (1.0 + delta.x * 0.01).max(0.05);
            for id in &drag.ids {
                let initial = drag.initial[id];
                let mut next = initial;
                match self.scene_tool {
                    SceneTool::Move | SceneTool::Select => {
                        next.position_x += delta.x / self.scene_zoom;
                        next.position_y -= delta.y / self.scene_zoom;
                        if self.snap_enabled {
                            next.position_x =
                                (next.position_x / self.snap_step).round() * self.snap_step;
                            next.position_y =
                                (next.position_y / self.snap_step).round() * self.snap_step;
                        }
                    }
                    SceneTool::Rotate => {
                        let relative_x = initial.position_x - pivot.0;
                        let relative_y = initial.position_y - pivot.1;
                        next.position_x =
                            pivot.0 + relative_x * angle.cos() - relative_y * angle.sin();
                        next.position_y =
                            pivot.1 + relative_x * angle.sin() + relative_y * angle.cos();
                        next.rotation += delta.x * 0.5;
                    }
                    SceneTool::Scale => {
                        next.position_x = pivot.0 + (initial.position_x - pivot.0) * factor;
                        next.position_y = pivot.1 + (initial.position_y - pivot.1) * factor;
                        next.scale_x = (initial.scale_x * factor).max(0.05);
                        next.scale_y = (initial.scale_y * factor).max(0.05);
                    }
                    SceneTool::TilePaint => {}
                }
                preview.insert(*id, next);
            }
            self.scene_state.preview = preview;
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            self.commit_scene_drag();
            self.scene_state.drag = None;
            self.scene_state.box_select_start = None;
        }
        // 工具小面板（Select/Move/Rotate/Scale）置于场景右上角。
        self.draw_scene_tool_panel(ui, rect);
    }

    fn set_scene_view_diagnostic(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.preview_diagnostic.as_deref() == Some(message.as_str()) {
            return;
        }
        self.push_console_notice(format!("[scene view] {message}"));
        self.preview_diagnostic = Some(message);
    }

    fn clear_scene_view_diagnostic(&mut self) {
        self.preview_diagnostic = None;
    }

    /// 3D 场景视图（M9-03）：XZ 俯视投影绘制 Transform3D 实体，选中实体显示
    /// 平移 Gizmo（X/Z 轴手柄、Y 高度手柄、自由拖拽），拖拽写回 Transform3D。
    fn draw_scene_view_3d(&mut self, ui: &mut egui::Ui) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let rect = response.rect;
        let scale = ui.ctx().pixels_per_point();
        let width = (rect.width() * scale) as u32;
        let height = (rect.height() * scale) as u32;

        // 导航：滚轮缩放 / 中键轨道 / 右键平移（编辑器相机，与游戏相机分离）。
        // 滚轮缩放仅当鼠标悬停在视图内才响应。
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if response.contains_pointer() && scroll.abs() > 0.0 {
            self.editor_camera.dist =
                (self.editor_camera.dist * (1.0 + scroll * 0.001)).clamp(1.0, 100.0);
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            let d = response.drag_delta();
            self.editor_camera.yaw += d.x * 0.008;
            self.editor_camera.pitch = (self.editor_camera.pitch + d.y * 0.008).clamp(0.05, 1.4);
        } else if response.dragged_by(egui::PointerButton::Secondary) {
            let d = response.drag_delta();
            let fx = -self.editor_camera.yaw.sin();
            let fz = -self.editor_camera.yaw.cos();
            let rx = self.editor_camera.yaw.cos();
            let rz = -self.editor_camera.yaw.sin();
            let s = self.editor_camera.dist * 0.0016;
            self.editor_camera.target.x += (-d.x * rx + d.y * fx) * s;
            self.editor_camera.target.z += (-d.x * rz + d.y * fz) * s;
        }

        // 渲染透视视口（编辑场景 + 编辑器相机）到离屏纹理并显示。
        self.render_scene_view_3d(width, height, ui.ctx());
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(28, 32, 40));
        if let Some(tex) = &self.scene_preview_tex {
            painter.image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Scene View  -  3D  中键旋转 / 右键平移 / 滚轮缩放",
            egui::FontId::proportional(15.0),
            egui::Color32::LIGHT_GRAY,
        );

        // 收集 3D 实体（Transform3D），投影到屏幕。
        let filter = self.hierarchy_filter.to_lowercase();
        let Some(scene) = self.current_scene() else {
            return;
        };
        let extracted = extract_serialized_scene_3d(&scene.scene);
        if !extracted.diagnostics.is_empty() {
            let details = extracted
                .diagnostics
                .iter()
                .take(2)
                .map(|diagnostic| diagnostic.summary())
                .collect::<Vec<_>>();
            self.set_scene_view_diagnostic(format!(
                "{} 个 3D 组件无效：{}",
                extracted.diagnostics.len(),
                details.join("；")
            ));
        } else {
            self.clear_scene_view_diagnostic();
        }
        let entities3d: Vec<_> = extracted
            .entities
            .into_iter()
            .filter(|entity| {
                filter.trim().is_empty() || entity.name.to_lowercase().contains(filter.trim())
            })
            .map(|entity| {
                let bounds = engine::core::math3d::Aabb3 {
                    min: Vec3::new(-0.5, -0.5, -0.5),
                    max: Vec3::new(0.5, 0.5, 0.5),
                };
                (entity.id, entity.name, entity.transform, bounds)
            })
            .collect();

        // 绘制实体标记（投影后的菱形 + 名称；拖拽预览优先）。
        for (id, name, transform, _) in &entities3d {
            let position = self
                .scene_state
                .preview3d_transform
                .get(id)
                .map(|t| t.position)
                .unwrap_or(transform.position);
            let Some(screen) = self.project_to_screen(position, rect) else {
                continue;
            };
            let selected =
                self.tree.selected() == Some(*id) || self.scene_state.selected.contains(id);
            let half = 8.0;
            let stroke = egui::Stroke::new(
                if selected { 2.0_f32 } else { 1.0_f32 },
                egui::Color32::from_rgb(230, 170, 60),
            );
            let corners = [
                screen + egui::vec2(-half, 0.0),
                screen + egui::vec2(0.0, -half),
                screen + egui::vec2(half, 0.0),
                screen + egui::vec2(0.0, half),
            ];
            painter.add(egui::Shape::convex_polygon(
                corners.to_vec(),
                if selected {
                    egui::Color32::from_rgb(120, 90, 40)
                } else {
                    egui::Color32::from_rgb(58, 66, 80)
                },
                stroke,
            ));
            painter.text(
                screen + egui::vec2(0.0, half + 10.0),
                egui::Align2::CENTER_CENTER,
                name,
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
        }

        // 点选：由屏幕坐标构造真实世界射线，按最近 bounds 命中实体。
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let px = (pos.x - rect.left()) * ui.ctx().pixels_per_point();
            let py = (pos.y - rect.top()) * ui.ctx().pixels_per_point();
            let hit = self
                .editor_camera_ray(px, py, width as f32, height as f32)
                .and_then(|ray| {
                    entities3d
                        .iter()
                        .filter(|(id, _, _, _)| !self.scene_state.locked.contains(id))
                        .filter_map(|(id, _, transform, bounds)| {
                            let position = self
                                .scene_state
                                .preview3d_transform
                                .get(id)
                                .map(|value| value.position)
                                .unwrap_or(transform.position);
                            let world_bounds = engine::core::math3d::Aabb3 {
                                min: bounds.min + position,
                                max: bounds.max + position,
                            };
                            ray.intersect_aabb(world_bounds)
                                .map(|distance| (*id, distance))
                        })
                        .min_by(|left, right| left.1.total_cmp(&right.1))
                        .map(|(id, _)| id)
                });
            if let Some(id) = hit {
                let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                if !additive {
                    self.scene_state.selected.clear();
                }
                self.scene_state.selected.insert(id);
                if let Some(scene) = self.current_scene() {
                    let scene = scene.clone();
                    let _ = self.tree.select(&scene, id);
                }
                self.sync_inspector(id);
            } else {
                self.scene_state.selected.clear();
            }
        }

        // Gizmo：按工具绘制手柄并拖拽（Move 平移 / Rotate 绕 Y / Scale 等比缩放）。
        let selected_entity = entities3d.iter().find(|(id, _, _, _)| {
            self.tree.selected() == Some(*id) || self.scene_state.selected.contains(id)
        });
        if let Some((id, _, transform, _)) = selected_entity {
            let tool = self.scene_tool;
            if matches!(
                tool,
                SceneTool::Move | SceneTool::Select | SceneTool::Rotate | SceneTool::Scale
            ) {
                let Some(screen) = self.project_to_screen(transform.position, rect) else {
                    return;
                };
                let len = 40.0;
                let pointer = response.interact_pointer_pos();
                // 命中检测：按工具判定手柄。
                let axis = if response.drag_started_by(egui::PointerButton::Primary)
                    && let Some(pos) = pointer
                {
                    match tool {
                        SceneTool::Move | SceneTool::Select => {
                            let hit_radius = 12.0;
                            if (pos - (screen + egui::vec2(len, 0.0))).length() < hit_radius {
                                Some(GizmoAxis::X)
                            } else if (pos - (screen + egui::vec2(0.0, -len))).length() < hit_radius
                            {
                                Some(GizmoAxis::Z)
                            } else if (pos - (screen + egui::vec2(-len * 0.7, -len * 0.7))).length()
                                < hit_radius
                            {
                                Some(GizmoAxis::Y)
                            } else {
                                let box_rect =
                                    egui::Rect::from_center_size(screen, egui::vec2(24.0, 24.0));
                                if box_rect.contains(pos) && !self.scene_state.locked.contains(id) {
                                    Some(GizmoAxis::Free)
                                } else {
                                    None
                                }
                            }
                        }
                        SceneTool::Rotate => {
                            if (pos - (screen + egui::vec2(len, 0.0))).length() < 14.0
                                || egui::Rect::from_center_size(screen, egui::vec2(24.0, 24.0))
                                    .contains(pos)
                            {
                                Some(GizmoAxis::X)
                            } else {
                                None
                            }
                        }
                        SceneTool::Scale => {
                            if (pos - (screen + egui::vec2(len, len))).length() < 14.0
                                || egui::Rect::from_center_size(screen, egui::vec2(24.0, 24.0))
                                    .contains(pos)
                            {
                                Some(GizmoAxis::Scale)
                            } else {
                                None
                            }
                        }
                        SceneTool::TilePaint => None,
                    }
                } else {
                    None
                };
                // 绘制手柄。
                match tool {
                    SceneTool::Move | SceneTool::Select => {
                        painter.line_segment(
                            [screen, screen + egui::vec2(len, 0.0)],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(220, 80, 80)),
                        );
                        painter.circle_filled(
                            screen + egui::vec2(len, 0.0),
                            5.0,
                            egui::Color32::from_rgb(220, 80, 80),
                        );
                        painter.line_segment(
                            [screen, screen + egui::vec2(0.0, -len)],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(80, 120, 220)),
                        );
                        painter.circle_filled(
                            screen + egui::vec2(0.0, -len),
                            5.0,
                            egui::Color32::from_rgb(80, 120, 220),
                        );
                        painter.line_segment(
                            [screen, screen + egui::vec2(-len * 0.7, -len * 0.7)],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 200, 90)),
                        );
                        painter.circle_filled(
                            screen + egui::vec2(-len * 0.7, -len * 0.7),
                            5.0,
                            egui::Color32::from_rgb(90, 200, 90),
                        );
                    }
                    SceneTool::Rotate => {
                        painter.circle_stroke(
                            screen,
                            len,
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(230, 160, 60)),
                        );
                        painter.circle_filled(
                            screen + egui::vec2(len, 0.0),
                            5.0,
                            egui::Color32::from_rgb(230, 160, 60),
                        );
                    }
                    SceneTool::Scale => {
                        painter.line_segment(
                            [screen, screen + egui::vec2(len, len)],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(180, 100, 220)),
                        );
                        painter.circle_filled(
                            screen + egui::vec2(len, len),
                            5.0,
                            egui::Color32::from_rgb(180, 100, 220),
                        );
                    }
                    SceneTool::TilePaint => {}
                }
                if let Some(axis) = axis {
                    self.scene_state.gizmo_drag = Some(GizmoDrag {
                        id: *id,
                        axis,
                        start: pointer.expect("drag started with a pointer"),
                        initial: *transform,
                        start_ray: pointer.and_then(|position| {
                            let px = (position.x - rect.left()) * ui.ctx().pixels_per_point();
                            let py = (position.y - rect.top()) * ui.ctx().pixels_per_point();
                            self.editor_camera_ray(px, py, width as f32, height as f32)
                        }),
                    });
                }
                // 拖拽更新：按工具修改 Transform3D（position/rotation/scale）。
                if response.dragged_by(egui::PointerButton::Primary)
                    && let (Some(drag), Some(pos)) = (self.scene_state.gizmo_drag.as_ref(), pointer)
                {
                    let delta = pos - drag.start;
                    let mut next = drag.initial;
                    match tool {
                        SceneTool::Move | SceneTool::Select => {
                            let d = self.editor_camera.dist * 0.002;
                            let world_delta = drag.start_ray.and_then(|start_ray| {
                                let px = (pos.x - rect.left()) * ui.ctx().pixels_per_point();
                                let py = (pos.y - rect.top()) * ui.ctx().pixels_per_point();
                                let current_ray =
                                    self.editor_camera_ray(px, py, width as f32, height as f32)?;
                                let axis = match drag.axis {
                                    GizmoAxis::X => Some(Vec3::X),
                                    GizmoAxis::Y => Some(Vec3::Y),
                                    GizmoAxis::Z => Some(Vec3::Z),
                                    _ => None,
                                }?;
                                let start = start_ray
                                    .closest_point_on_line(drag.initial.position, axis)?
                                    .1;
                                let current = current_ray
                                    .closest_point_on_line(drag.initial.position, axis)?
                                    .1;
                                Some(axis * (current - start))
                            });
                            match drag.axis {
                                GizmoAxis::X | GizmoAxis::Y | GizmoAxis::Z => {
                                    if let Some(value) = world_delta {
                                        next.position = drag.initial.position + value;
                                    } else {
                                        next.position = next.position
                                            + Vec3::new(delta.x * d, -delta.y * d, 0.0);
                                    }
                                }
                                GizmoAxis::Free => {
                                    let fx = -self.editor_camera.yaw.sin();
                                    let fz = -self.editor_camera.yaw.cos();
                                    let rx = self.editor_camera.yaw.cos();
                                    let rz = -self.editor_camera.yaw.sin();
                                    next.position.x += (delta.x * rx - delta.y * fx) * d;
                                    next.position.z += (delta.x * rz - delta.y * fz) * d;
                                }
                                _ => {}
                            }
                            if self.snap_enabled {
                                next.position.x =
                                    (next.position.x / self.snap_step).round() * self.snap_step;
                                next.position.y =
                                    (next.position.y / self.snap_step).round() * self.snap_step;
                                next.position.z =
                                    (next.position.z / self.snap_step).round() * self.snap_step;
                            }
                        }
                        SceneTool::Rotate => {
                            if let Ok(rotation) = Quat::from_axis_angle(Vec3::Y, delta.x * 0.01) {
                                next.rotation = rotation * next.rotation;
                            }
                        }
                        SceneTool::Scale => {
                            let factor = (1.0 + delta.x * 0.01).max(0.05);
                            next.scale = Vec3::new(
                                (next.scale.x * factor).max(0.05),
                                (next.scale.y * factor).max(0.05),
                                (next.scale.z * factor).max(0.05),
                            );
                        }
                        SceneTool::TilePaint => {}
                    }
                    self.scene_state.preview3d_transform.insert(drag.id, next);
                }
                if response.drag_stopped_by(egui::PointerButton::Primary) {
                    self.commit_gizmo_drag3d();
                    self.scene_state.gizmo_drag = None;
                }
            }
        }
        // 工具小面板（Select/Move/Rotate/Scale）置于场景右上角。
        self.draw_scene_tool_panel(ui, rect);
    }

    /// Scene 视图右上角的小工具面板：选择/移动/旋转/缩放。
    fn draw_scene_tool_panel(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let width = 190.0;
        let tools_rect = egui::Rect::from_min_size(
            egui::pos2(rect.right() - width - 4.0, rect.top() + 4.0),
            egui::vec2(width, 26.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(tools_rect), |ui| {
            ui.horizontal(|ui| {
                if icon_toggle(ui, self.scene_tool == SceneTool::Select, "⌖", "选择").clicked()
                {
                    self.scene_tool = SceneTool::Select;
                }
                if icon_toggle(ui, self.scene_tool == SceneTool::Move, "✥", "移动").clicked() {
                    self.scene_tool = SceneTool::Move;
                }
                if icon_toggle(ui, self.scene_tool == SceneTool::Rotate, "⟳", "旋转").clicked()
                {
                    self.scene_tool = SceneTool::Rotate;
                }
                if icon_toggle(ui, self.scene_tool == SceneTool::Scale, "⤢", "缩放").clicked() {
                    self.scene_tool = SceneTool::Scale;
                }
                if icon_toggle(
                    ui,
                    self.scene_tool == SceneTool::TilePaint,
                    "▦",
                    "Tile 画笔",
                )
                .clicked()
                {
                    self.scene_tool = SceneTool::TilePaint;
                }
            });
        });
    }

    fn selected_tilemap_context(&self) -> Option<TilemapPaintContext> {
        let selected = self.tree.selected()?;
        let scene = self.current_scene()?;
        let entity = scene.scene.entities.get(&selected)?;
        if !entity.components.contains_key("Tilemap") {
            return None;
        }
        let transform = self
            .scene_state
            .preview
            .get(&selected)
            .copied()
            .or_else(|| {
                entity
                    .components
                    .get("Transform")
                    .and_then(|value| parse_transform(value))
            })?;
        Some(TilemapPaintContext {
            entity: selected,
            transform,
        })
    }

    fn draw_tilemap_overlay(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        context: &TilemapPaintContext,
        hovered: Option<(usize, usize)>,
    ) {
        if context.transform.rotation.abs() > 0.001 {
            painter.text(
                rect.left_bottom() + egui::vec2(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                "TilePaint 暂不支持旋转 Tilemap",
                egui::FontId::proportional(13.0),
                egui::Color32::LIGHT_RED,
            );
            return;
        }
        let cell_w =
            self.tilemap.map.tile_size.x * context.transform.scale_x.abs() * self.scene_zoom;
        let cell_h =
            self.tilemap.map.tile_size.y * context.transform.scale_y.abs() * self.scene_zoom;
        if cell_w <= 0.0 || cell_h <= 0.0 {
            return;
        }
        for y in 0..self.tilemap.map.height {
            for x in 0..self.tilemap.map.width {
                let screen_rect = tilemap_cell_screen_rect(
                    rect,
                    self.scene_state.pan,
                    self.scene_zoom,
                    context.transform,
                    &self.tilemap.map,
                    x,
                    y,
                );
                let index = y * self.tilemap.map.width + x;
                let value = self.tilemap.map.tiles[index];
                let solid = self.tilemap.map.solid[index];
                if value != 0 {
                    painter.rect_filled(
                        screen_rect,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(80, 140, 220, 48),
                    );
                }
                if solid {
                    painter.rect_filled(
                        screen_rect.shrink(2.0),
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(220, 90, 90, 60),
                    );
                }
                let stroke = if hovered == Some((x, y)) {
                    egui::Stroke::new(2.0_f32, egui::Color32::YELLOW)
                } else {
                    egui::Stroke::new(
                        1.0_f32,
                        egui::Color32::from_rgba_unmultiplied(180, 180, 180, 90),
                    )
                };
                painter.rect_stroke(screen_rect, 0.0, stroke, egui::StrokeKind::Inside);
            }
        }
    }

    fn handle_tilemap_paint(
        &mut self,
        response: &egui::Response,
        rect: egui::Rect,
        context: &TilemapPaintContext,
    ) {
        let pointer = response.interact_pointer_pos();
        if response.drag_started_by(egui::PointerButton::Primary)
            || response.drag_started_by(egui::PointerButton::Secondary)
        {
            self.scene_state.tile_paint_active = true;
            self.scene_state.tile_paint_last_cell = None;
        }
        if response.clicked_by(egui::PointerButton::Primary)
            && let Some(pos) = pointer
            && let Some((x, y)) = tilemap_cell_from_pointer(
                rect,
                self.scene_state.pan,
                self.scene_zoom,
                pos,
                context.transform,
                &self.tilemap.map,
            )
        {
            self.apply_tile_paint_cell(x, y, false);
            self.commit_tilemap_paint(context.entity);
            self.scene_state.tile_paint_last_cell = None;
        }
        if response.clicked_by(egui::PointerButton::Secondary)
            && let Some(pos) = pointer
            && let Some((x, y)) = tilemap_cell_from_pointer(
                rect,
                self.scene_state.pan,
                self.scene_zoom,
                pos,
                context.transform,
                &self.tilemap.map,
            )
        {
            self.apply_tile_paint_cell(x, y, true);
            self.commit_tilemap_paint(context.entity);
            self.scene_state.tile_paint_last_cell = None;
        }
        for (button, erase) in [
            (egui::PointerButton::Primary, false),
            (egui::PointerButton::Secondary, true),
        ] {
            if response.dragged_by(button)
                && let Some(pos) = pointer
                && let Some((x, y)) = tilemap_cell_from_pointer(
                    rect,
                    self.scene_state.pan,
                    self.scene_zoom,
                    pos,
                    context.transform,
                    &self.tilemap.map,
                )
                && self.scene_state.tile_paint_last_cell != Some((x, y, erase))
            {
                self.apply_tile_paint_cell(x, y, erase);
                self.scene_state.tile_paint_last_cell = Some((x, y, erase));
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Secondary)
        {
            if self.scene_state.tile_paint_active {
                self.commit_tilemap_paint(context.entity);
            }
            self.scene_state.tile_paint_active = false;
            self.scene_state.tile_paint_last_cell = None;
        }
    }

    fn apply_tile_paint_cell(&mut self, x: usize, y: usize, erase: bool) {
        let index = y * self.tilemap.map.width + x;
        if self.tilemap.paint_collision {
            self.tilemap.map.solid[index] = !erase;
        } else {
            self.tilemap.map.tiles[index] = if erase { 0 } else { self.tilemap.brush.min(6) };
        }
    }

    fn commit_tilemap_paint(&mut self, entity_id: SceneEntityId) {
        if self.is_playing() {
            return;
        }
        let tilemap = self.tilemap.map.clone();
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ = self.history.execute(
                scene,
                "Tilemap Paint",
                CommandKind::Property,
                move |scene| {
                    if let Some(entity) = scene.scene.entities.get_mut(&entity_id) {
                        entity
                            .components
                            .insert("Tilemap".to_owned(), tilemap.to_component_str());
                        scene.mark_dirty();
                    }
                    Ok::<(), ()>(())
                },
            );
        }
    }

    /// 提交 2D 场景拖拽：写回 Transform 组件。
    fn commit_scene_drag(&mut self) {
        if self.is_playing() {
            self.scene_state.preview.clear();
            return;
        }
        let preview = std::mem::take(&mut self.scene_state.preview);
        let Some(first) = preview.keys().next().copied() else {
            return;
        };
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ = self.history.execute(
                scene,
                "Scene Transform",
                CommandKind::Transform(first),
                |scene| {
                    for (id, transform) in &preview {
                        if let Some(entity) = scene.scene.entities.get_mut(id) {
                            entity
                                .components
                                .insert("Transform".to_owned(), encode_transform(*transform));
                        }
                    }
                    scene.mark_dirty();
                    Ok::<(), ()>(())
                },
            );
        }
    }

    /// 提交 3D 平移 Gizmo 拖拽：只改 Transform3D 的 position，保留旋转/缩放。
    fn commit_gizmo_drag3d(&mut self) {
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
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ = self.history.execute(
                scene,
                "3D Transform",
                CommandKind::Transform3D(id),
                |scene| {
                    if let Some(entity) = scene.scene.entities.get_mut(&id) {
                        entity
                            .components
                            .insert("Transform3D".to_owned(), transform.to_component_str());
                    }
                    scene.mark_dirty();
                    Ok::<(), ()>(())
                },
            );
        }
    }

    /// 确保 3D 场景视图的离屏渲染器存在；按视图尺寸重建纹理（M10-04）。
    fn ensure_scene_renderer(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.scene_renderer.is_none()
            && let Some((device, queue)) = &self.gpu
        {
            let root = strip_verbatim_prefix(&self.project.root);
            self.scene_renderer =
                Renderer::new_offscreen(device.clone(), queue.clone(), Some(root), false, false)
                    .ok();
        }
        let size_changed = self
            .scene_texture
            .as_ref()
            .is_none_or(|(texture, _)| texture.width() != width || texture.height() != height);
        if size_changed && let Some((device, _)) = &self.gpu {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("p2-scene-view"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.scene_texture = Some((texture, view));
        }
    }

    /// 从当前编辑场景 + 编辑器轨道相机构建 3D 渲染帧（M10-04）。
    ///
    /// M11-04 / R-05：优先复用 `RuntimeScene` 的 ECS 提取（与运行时同一条
    /// drawable/component 提取路径，实体/资源数量与关键状态一致），编辑器轨道
    /// 相机作为覆盖传入（不修改场景数据）；`scene_runtime` 缺失、实例化失败或
    /// 提取为空时回退到静态 serialized extractor（保持既有诊断行为）。
    fn build_scene3d(&mut self, aspect: f32) -> Option<Scene3d> {
        let camera = self.editor_camera(aspect);
        if let Some(runtime) = &self.scene_runtime
            && let Some(mut scene3d) = runtime.scene_3d_with_camera(aspect, Some(camera))
        {
            // Gizmo/拖拽预览覆盖 transform（不修改场景数据、不写回 ECS）。
            for drawable in &mut scene3d.drawables {
                if let Some(t) = self.scene_state.preview3d_transform.get(&drawable.entity)
                    && let Ok(model) = t.model_matrix()
                {
                    drawable.model = model;
                }
            }
            return Some(scene3d);
        }
        // 回退：serialized extractor（无 ECS 场景 / 实例化失败时保持既有行为与诊断）。
        let scene = self.current_scene()?;
        let extracted = extract_serialized_scene_3d(&scene.scene);
        if !extracted.diagnostics.is_empty() {
            let details = extracted
                .diagnostics
                .iter()
                .take(2)
                .map(|diagnostic| diagnostic.summary())
                .collect::<Vec<_>>();
            self.set_scene_view_diagnostic(format!(
                "{} 个 3D 组件无效：{}",
                extracted.diagnostics.len(),
                details.join("；")
            ));
        }
        let drawables = extracted
            .entities
            .into_iter()
            .filter_map(|entity| {
                let transform = self
                    .scene_state
                    .preview3d_transform
                    .get(&entity.id)
                    .copied()
                    .unwrap_or(entity.transform);
                let model = transform.model_matrix().ok()?;
                Some(Scene3dDrawable {
                    entity: entity.id,
                    name: entity.name,
                    mesh_path: entity.mesh.mesh,
                    model,
                    palette: Vec::new(),
                    bounds: Scene3dBounds {
                        local_aabb: engine::core::math3d::Aabb3 {
                            min: Vec3::new(-0.5, -0.5, -0.5),
                            max: Vec3::new(0.5, 0.5, 0.5),
                        },
                        source: BoundsSource::Fallback,
                    },
                })
            })
            .collect::<Vec<_>>();
        if drawables.is_empty() {
            return None;
        }
        let light = Light {
            direction: Vec3::new(0.4, 0.8, 0.5),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        };
        Some(Scene3d {
            camera,
            light,
            drawables,
        })
    }

    /// 渲染 3D 场景视图（编辑场景 + 编辑器相机）到离屏纹理并更新 egui 预览。
    /// 内容未变化（修订/实际渲染尺寸/相机未动且无活动拖拽预览）时跳过渲染，沿用缓存纹理。
    ///
    /// M11-03：缓存 key 记录实际渲染尺寸（R-03，拖拽结束半分辨率 → 全分辨率必然失效）；
    /// 缓存只在回读成功后提交，失败清缓存并显示错误（R-04，下帧重试而非沿用旧帧）。
    fn render_scene_view_3d(&mut self, width: u32, height: u32, ctx: &egui::Context) {
        if width == 0 || height == 0 {
            return;
        }
        let revision = self.document.current_scene.as_ref().map(|doc| doc.revision);
        // Gizmo/拖拽预览变化需每帧重渲染；否则仅在修订/尺寸/相机变化时重渲染。
        let preview_active = !self.scene_state.preview3d_transform.is_empty();
        // 交互期（拖拽预览活动）降半分辨率渲染，空闲恢复全分辨率。
        let (render_w, render_h) = if preview_active {
            ((width / 2).max(1), (height / 2).max(1))
        } else {
            (width, height)
        };
        let cache = self.scene_view_cache_3d;
        if scene_view_3d_cache_hit(
            cache,
            render_w,
            render_h,
            revision,
            self.editor_camera,
            preview_active,
        ) {
            self.drain_readback_3d(ctx);
            return;
        }
        let started = std::time::Instant::now();
        self.ensure_scene_renderer(render_w, render_h);
        // 前置条件未满足（无 GPU）时不更新缓存，下帧重试。
        if self.scene_renderer.is_none() || self.scene_texture.is_none() {
            return;
        }
        // M11-04 / R-05：确保编辑场景的 ECS 运行时已按当前文档 revision 建立，
        // 让 3D 视图与运行时共用同一条提取路径（相机以编辑器视图作为覆盖）。
        self.ensure_scene_runtime();
        let scene3d = self.build_scene3d(render_w as f32 / render_h.max(1) as f32);
        let mesh = SceneMesh {
            vertices: Vec::new(),
            draws: Vec::new(),
            paths: Vec::new(),
            font_atlas: None,
        };
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        let Some((texture, view)) = &self.scene_texture else {
            return;
        };
        renderer.render_into_texture(&mesh, scene3d.as_ref(), view, render_w, render_h);
        // 非阻塞回读：就绪则上传并提交缓存；Pending 请求重绘等下帧轮询；
        // 失败清缓存（保持不匹配）让下帧重试，并显示用户可见错误（R-04）。
        match renderer.readback_frame(texture, render_w, render_h) {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            } => {
                self.scene_view_cache_3d = SceneViewCache3d {
                    size: (render_w, render_h),
                    revision,
                    camera: self.editor_camera,
                };
                self.set_scene_3d_texture(pixels, width, height, ctx);
            }
            ReadbackStatus::Pending => ctx.request_repaint(),
            ReadbackStatus::Failed(error) => {
                tracing::warn!(target: "editor", "[scene view] 3D readback failed: {error}");
                self.scene_view_cache_3d = SceneViewCache3d::default();
                self.set_scene_view_diagnostic(format!("Scene View 3D 回读失败：{error}"));
                ctx.request_repaint();
            }
        }
        // 性能打点：仅当单帧耗时超过阈值时记录，便于对照优化前后。
        let elapsed = started.elapsed();
        if elapsed >= std::time::Duration::from_millis(8) {
            tracing::info!(
                target: "editor",
                "[editor-perf] scene view 3D frame: {}ms ({render_w}x{render_h})",
                elapsed.as_millis()
            );
        }
    }

    /// 确保编辑场景的 2D 运行时存在，且反映当前文档 revision 的场景内容。
    /// 场景编辑（revision 变化）时重建运行时，让背景随编辑更新。
    fn ensure_scene_runtime(&mut self) {
        let revision = self.document.current_scene.as_ref().map(|doc| doc.revision);
        // 已按此 revision 尝试过（无论成功失败）则不再重建，避免持久失败时每帧重试。
        if self.scene_runtime_revision == revision {
            return;
        }
        self.scene_runtime = None;
        let Some(scene) = self.document.current_scene.as_ref() else {
            self.scene_runtime_revision = revision;
            return;
        };
        let root = strip_verbatim_prefix(&self.project.root);
        match RuntimeScene::from_scene_edit(Some(&root), scene.scene.clone()) {
            Ok(runtime) => self.scene_runtime = Some(runtime),
            Err(error) => {
                tracing::warn!(target: "editor", "[scene view] 2D runtime rebuild failed: {error}");
            }
        }
        self.scene_runtime_revision = revision;
    }

    /// 渲染 2D 场景视图（编辑场景，匹配编辑器 pan/zoom）到离屏纹理并更新 egui 预览。
    /// 内容未变化（修订/尺寸/相机未动）时跳过渲染，沿用缓存纹理。
    fn render_scene_view_2d(&mut self, width: u32, height: u32, ctx: &egui::Context) {
        if width == 0 || height == 0 {
            return;
        }
        let revision = self.document.current_scene.as_ref().map(|doc| doc.revision);
        let camera = (self.scene_state.pan, self.scene_zoom);
        let cache = self.scene_view_cache_2d;
        if cache.size == (width, height) && cache.revision == revision && cache.camera == camera {
            self.drain_readback_2d(ctx);
            return;
        }
        // 前置条件未满足（无场景/无 GPU）时不更新缓存，下帧重试，避免"打开场景后
        // revision 仍为 0 被缓存命中"导致背景永不渲染。
        self.ensure_scene_runtime();
        if self.scene_runtime.is_none() {
            return;
        }
        self.ensure_scene_renderer(width, height);
        if self.scene_renderer.is_none() || self.scene_texture.is_none() {
            return;
        }
        // 本次实际渲染尺寸（2D 无半分辨率，即视口尺寸）；readback 返回的尺寸可能因
        // 滞后帧不同，缓存记录用实际渲染尺寸。
        let render_size = (width, height);
        let started = std::time::Instant::now();

        let zoom = self.scene_zoom;
        let pan = self.scene_state.pan;
        // 2D 相机匹配编辑器视图：屏幕中心对应世界 -pan/zoom。
        let camera2d =
            Camera2D::new(Vec2::new(-pan.x / zoom, -pan.y / zoom), zoom).unwrap_or_else(|_| {
                Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("zero camera is valid")
            });
        let mesh = {
            let Some(runtime) = &mut self.scene_runtime else {
                return;
            };
            runtime.set_camera_2d(camera2d);
            let mesh = match runtime.vertices(width, height) {
                Ok(mesh) => mesh,
                Err(error) => {
                    tracing::error!(target: "editor", "[scene view] 2D render failed: {error}");
                    self.set_scene_view_diagnostic(format!("Scene View 渲染失败：{error}"));
                    return;
                }
            };
            self.clear_scene_view_diagnostic();
            mesh
        };
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        let Some((texture, view)) = &self.scene_texture else {
            return;
        };
        renderer.render_into_texture(&mesh, None, view, width, height);
        // 非阻塞回读：就绪则上传并提交缓存；Pending 请求重绘等下帧轮询；
        // 失败清缓存（保持不匹配）让下帧重试，并显示用户可见错误（R-04）。
        match renderer.readback_frame(texture, render_size.0, render_size.1) {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            } => {
                self.scene_view_cache_2d = SceneViewCache2d {
                    size: render_size,
                    revision,
                    camera,
                };
                self.set_scene_2d_texture(pixels, width, height, ctx);
            }
            ReadbackStatus::Pending => {
                self.set_scene_view_diagnostic("Scene View 等待 GPU 回读……显示上一帧");
                ctx.request_repaint();
            }
            ReadbackStatus::Failed(error) => {
                tracing::warn!(target: "editor", "[scene view] 2D readback failed: {error}");
                self.scene_view_cache_2d = SceneViewCache2d::default();
                self.set_scene_view_diagnostic(format!("Scene View 回读失败：{error}"));
                ctx.request_repaint();
            }
        }
        // 性能打点：仅当单帧耗时超过阈值时记录，便于对照优化前后。
        let elapsed = started.elapsed();
        if elapsed >= std::time::Duration::from_millis(8) {
            tracing::info!(
                target: "editor",
                "[editor-perf] scene view 2D frame: {}ms ({width}x{height})",
                elapsed.as_millis()
            );
        }
    }

    /// 把 2D 回读像素上传到 egui 纹理（NEAREST，像素风）。
    fn set_scene_2d_texture(
        &mut self,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        ctx: &egui::Context,
    ) {
        let image =
            egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &pixels);
        match &mut self.scene_2d_tex {
            Some(tex) => tex.set(image, egui::TextureOptions::NEAREST),
            None => {
                self.scene_2d_tex =
                    Some(ctx.load_texture("scene_2d", image, egui::TextureOptions::NEAREST));
            }
        }
    }

    /// 把 3D 回读像素上传到 egui 纹理（LINEAR 平滑缩放）。
    fn set_scene_3d_texture(
        &mut self,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        ctx: &egui::Context,
    ) {
        let image =
            egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &pixels);
        match &mut self.scene_preview_tex {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => {
                self.scene_preview_tex =
                    Some(ctx.load_texture("scene_view", image, egui::TextureOptions::LINEAR));
            }
        }
    }

    /// 内容未变化时排空 2D 在途回读（补上滞后一帧的纹理），并自终止重绘轮询。
    fn drain_readback_2d(&mut self, ctx: &egui::Context) {
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        if !renderer.readback_in_flight() {
            return;
        }
        match renderer.readback_pending() {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            } => {
                self.set_scene_2d_texture(pixels, width, height, ctx);
            }
            ReadbackStatus::Pending => ctx.request_repaint(),
            ReadbackStatus::Failed(error) => {
                tracing::warn!(target: "editor", "[scene view] 2D readback drain failed: {error}");
            }
        }
    }

    /// 内容未变化时排空 3D 在途回读（补上滞后一帧的纹理），并自终止重绘轮询。
    fn drain_readback_3d(&mut self, ctx: &egui::Context) {
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        if !renderer.readback_in_flight() {
            return;
        }
        match renderer.readback_pending() {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            } => {
                self.set_scene_3d_texture(pixels, width, height, ctx);
            }
            ReadbackStatus::Pending => ctx.request_repaint(),
            ReadbackStatus::Failed(error) => {
                tracing::warn!(target: "editor", "[scene view] 3D readback drain failed: {error}");
            }
        }
    }

    fn editor_camera(&self, aspect_ratio: f32) -> Camera3D {
        let cam = self.editor_camera;
        let position = Vec3::new(
            cam.target.x + cam.dist * cam.pitch.cos() * cam.yaw.sin(),
            cam.target.y + cam.dist * cam.pitch.sin(),
            cam.target.z + cam.dist * cam.pitch.cos() * cam.yaw.cos(),
        );
        Camera3D {
            position,
            target: cam.target,
            up: Vec3::Y,
            fov_y: 60f32.to_radians(),
            aspect_ratio,
            near: 0.1,
            far: 200.0,
        }
    }

    fn editor_camera_ray(&self, x: f32, y: f32, width: f32, height: f32) -> Option<Ray3> {
        let camera = self.editor_camera(width / height.max(1.0));
        let (origin, direction) = camera.ray_from_screen(x, y, width, height).ok()?;
        Ray3::new(origin, direction).ok()
    }

    /// 把世界坐标投影到屏幕（编辑器相机透视投影）。
    fn project_to_screen(&self, world: Vec3, rect: egui::Rect) -> Option<egui::Pos2> {
        let camera = self.editor_camera(rect.width() / rect.height().max(1.0));
        let vp = camera.view_projection_matrix().ok()?;
        let c = vp.columns;
        let x = c[0][0] * world.x + c[1][0] * world.y + c[2][0] * world.z + c[3][0];
        let y = c[0][1] * world.x + c[1][1] * world.y + c[2][1] * world.z + c[3][1];
        let w = c[0][3] * world.x + c[1][3] * world.y + c[2][3] * world.z + c[3][3];
        if w <= 0.0 {
            return None;
        }
        Some(egui::pos2(
            rect.center().x + (x / w) * rect.width() * 0.5,
            rect.center().y - (y / w) * rect.height() * 0.5,
        ))
    }

    /// 从拖放的预制体路径实例化到场景。
    fn spawn_prefab_from_drop(&mut self, path: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let root = strip_verbatim_prefix(&self.project.root);
        let prefab = Prefab::load(root.join(path)).map_err(|error| error.to_string())?;
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let overrides = std::collections::BTreeMap::new();
        let result = self.history.execute(
            scene,
            "Instantiate Prefab",
            CommandKind::Create,
            move |scene| {
                let instance = prefab
                    .instantiate(&mut scene.scene, None, &overrides)
                    .map_err(|error| error.to_string())?;
                for (source_id, entity_id) in &instance.source_to_instance {
                    if let Some(entity) = scene.scene.entities.get_mut(entity_id) {
                        entity
                            .metadata
                            .insert("prefab.source".to_owned(), path.to_owned());
                        entity
                            .metadata
                            .insert("prefab.source_id".to_owned(), source_id.0.to_string());
                        entity.metadata.insert(
                            "prefab.root".to_owned(),
                            (entity.id == instance.instance_root).to_string(),
                        );
                    }
                }
                scene.mark_dirty();
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// 拖放资源分发：预制体 → 实例化；其他（纹理）→ 创建精灵。
    fn spawn_dropped_resource(&mut self, path: &str) -> Result<(), String> {
        if path.ends_with(".prefab") {
            self.spawn_prefab_from_drop(path)
        } else if path.ends_with(".glb") || path.ends_with(".gltf") {
            self.spawn_model_from_drop(path)
        } else {
            self.spawn_sprite_from_drop(path)
        }
    }

    /// 从拖放的模型路径创建 3D 网格实体（Transform3D + MeshRenderer）。
    fn spawn_model_from_drop(&mut self, path: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if self.document.current_scene.is_none() {
            return Err("请先创建场景".to_owned());
        }
        let name = Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Model".to_owned());
        let tree = self.tree.clone();
        let path = path.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result =
            self.history
                .execute(scene, "Create Model", CommandKind::Create, move |scene| {
                    let mut tree = tree;
                    let id = tree
                        .create_entity(scene, name.clone(), None)
                        .map_err(|error| error.to_string())?;
                    if let Some(entity) = scene.scene.entities.get_mut(&id) {
                        entity.components.insert(
                            "Transform3D".into(),
                            Transform3D::default().to_component_str(),
                        );
                        entity
                            .components
                            .insert("MeshRenderer".into(), format!("mesh={path}"));
                    }
                    Ok::<(), String>(())
                });
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// 从拖放的资源路径创建 2D 精灵实体（Transform + Sprite）。
    fn spawn_sprite_from_drop(&mut self, path: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if self.document.current_scene.is_none() {
            return Err("请先创建场景".to_owned());
        }
        let name = Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Sprite".to_owned());
        let tree = self.tree.clone();
        let path = path.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result =
            self.history
                .execute(scene, "Create Sprite", CommandKind::Create, move |scene| {
                    let mut tree = tree;
                    let id = tree
                        .create_entity(scene, name.clone(), None)
                        .map_err(|error| error.to_string())?;
                    if let Some(entity) = scene.scene.entities.get_mut(&id) {
                        entity.components.insert(
                            "Transform".into(),
                            "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
                        );
                        let sprite = engine::core::sprite::SpriteComponent {
                            texture: path.clone(),
                            size: engine::core::transform::Vec2::new(32.0, 32.0),
                            pivot: engine::core::transform::Vec2::new(0.5, 0.5),
                            tint: [1.0; 4],
                            sorting: engine::core::sprite::SpriteSorting::default(),
                        };
                        entity
                            .components
                            .insert("Sprite".into(), sprite.to_component_str());
                    }
                    Ok::<(), String>(())
                });
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct TilemapPaintContext {
    entity: SceneEntityId,
    transform: TransformFields,
}

fn screen_to_world_2d(rect: egui::Rect, pan: egui::Vec2, zoom: f32, pointer: egui::Pos2) -> Vec2 {
    Vec2::new(
        (pointer.x - rect.center().x - pan.x) / zoom,
        -(pointer.y - rect.center().y - pan.y) / zoom,
    )
}

fn tilemap_cell_from_pointer(
    rect: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    pointer: egui::Pos2,
    transform: TransformFields,
    tilemap: &Tilemap,
) -> Option<(usize, usize)> {
    if transform.rotation.abs() > 0.001 || zoom <= 0.0 {
        return None;
    }
    let world = screen_to_world_2d(rect, pan, zoom, pointer);
    let cell_w = tilemap.tile_size.x * transform.scale_x.abs();
    let cell_h = tilemap.tile_size.y * transform.scale_y.abs();
    if cell_w <= 0.0 || cell_h <= 0.0 {
        return None;
    }
    let local_x = (world.x - transform.position_x) / cell_w;
    let local_y = (world.y - transform.position_y) / cell_h;
    if !(0.0..tilemap.width as f32).contains(&local_x)
        || !(0.0..tilemap.height as f32).contains(&local_y)
    {
        return None;
    }
    Some((local_x.floor() as usize, local_y.floor() as usize))
}

fn tilemap_cell_screen_rect(
    rect: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    transform: TransformFields,
    tilemap: &Tilemap,
    x: usize,
    y: usize,
) -> egui::Rect {
    let cell_w = tilemap.tile_size.x * transform.scale_x.abs() * zoom;
    let cell_h = tilemap.tile_size.y * transform.scale_y.abs() * zoom;
    let min_x = rect.center().x
        + pan.x
        + (transform.position_x + x as f32 * tilemap.tile_size.x * transform.scale_x.abs()) * zoom;
    let max_x = min_x + cell_w;
    let max_y = rect.center().y + pan.y
        - (transform.position_y + y as f32 * tilemap.tile_size.y * transform.scale_y.abs()) * zoom;
    let min_y = max_y - cell_h;
    egui::Rect::from_min_max(egui::pos2(min_x, min_y), egui::pos2(max_x, max_y))
}

/// 判断 3D Scene 视图是否可直接命中渲染缓存（M11-03 / R-03）：
/// 无活动拖拽预览，且实际渲染尺寸 / 场景修订 / 编辑器相机都未变化时才命中。
/// `render_w/render_h` 必须是实际渲染尺寸（交互期为半分辨率）——缓存 key 记录
/// 真实渲染尺寸，拖拽结束半分辨率 → 全分辨率必然失效，不会沿用模糊帧。
fn scene_view_3d_cache_hit(
    cache: SceneViewCache3d,
    render_w: u32,
    render_h: u32,
    revision: Option<u64>,
    camera: crate::EditorCamera,
    preview_active: bool,
) -> bool {
    !preview_active
        && cache.size == (render_w, render_h)
        && cache.revision == revision
        && cache.camera == camera
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorApp;
    use crate::EditorCamera;
    use editor::open_project;
    use engine::core::logging::LogSink;
    use engine::core::scene::SceneEntityId;
    use engine::core::transform::Vec2;
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
            "rustlua-m14-01-scene-view-{label}-{}",
            unique_token()
        ))
    }

    fn sample_project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../game_test_2d_production")
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

    /// R-03：3D 缓存 key 记录实际渲染尺寸。拖拽期渲染半分辨率并写入缓存；
    /// 拖拽结束后视口恢复全分辨率，缓存不匹配 → 必然重渲染，不会沿用模糊帧。
    #[test]
    fn scene_view_3d_cache_records_actual_render_size() {
        let camera = EditorCamera::default();
        let revision = Some(7u64);
        let view_w = 1280u32;
        let view_h = 720u32;
        let half = ((view_w / 2).max(1), (view_h / 2).max(1));
        // 拖拽中写入的半分辨率缓存。
        let cache_half = SceneViewCache3d {
            size: half,
            revision,
            camera,
        };
        // 拖拽中（preview_active=true）：即使尺寸匹配也需每帧重渲染 → 不命中。
        assert!(!scene_view_3d_cache_hit(
            cache_half, half.0, half.1, revision, camera, true,
        ));
        // 拖拽结束：preview_active=false，但视口已恢复全分辨率，与半分辨率缓存尺寸不匹配 → 重渲染。
        assert!(!scene_view_3d_cache_hit(
            cache_half, view_w, view_h, revision, camera, false,
        ));
        // 空闲且全分辨率缓存与当前视口/修订/相机匹配 → 命中，可跳过重渲染。
        let cache_full = SceneViewCache3d {
            size: (view_w, view_h),
            revision,
            camera,
        };
        assert!(scene_view_3d_cache_hit(
            cache_full, view_w, view_h, revision, camera, false,
        ));
        // 相机或修订变化 → 不命中。
        let mut moved = EditorCamera::default();
        moved.dist += 1.0;
        assert!(!scene_view_3d_cache_hit(
            cache_full, view_w, view_h, revision, moved, false,
        ));
        assert!(!scene_view_3d_cache_hit(
            cache_full,
            view_w,
            view_h,
            Some(8u64),
            camera,
            false,
        ));
    }

    #[test]
    fn tilemap_pointer_mapping_round_trips_visible_cells() {
        let rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));
        let pan = egui::vec2(10.0, -6.0);
        let zoom = 2.0;
        let transform = TransformFields {
            position_x: 16.0,
            position_y: 8.0,
            rotation: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
        };
        let map = Tilemap::new(Vec2::new(32.0, 32.0), 4, 3).unwrap();
        let cell_rect = tilemap_cell_screen_rect(rect, pan, zoom, transform, &map, 2, 1);
        let pointer = cell_rect.center();
        assert_eq!(
            tilemap_cell_from_pointer(rect, pan, zoom, pointer, transform, &map),
            Some((2, 1))
        );
    }

    #[test]
    fn tilemap_pointer_mapping_rejects_outside_and_rotated_maps() {
        let rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));
        let map = Tilemap::new(Vec2::new(32.0, 32.0), 2, 2).unwrap();
        let transform = TransformFields::default();
        assert_eq!(
            tilemap_cell_from_pointer(
                rect,
                egui::Vec2::ZERO,
                1.0,
                egui::pos2(-20.0, -20.0),
                transform,
                &map,
            ),
            None
        );
        let rotated = TransformFields {
            rotation: 15.0,
            ..transform
        };
        assert_eq!(
            tilemap_cell_from_pointer(
                rect,
                egui::Vec2::ZERO,
                1.0,
                egui::pos2(200.0, 150.0),
                rotated,
                &map,
            ),
            None
        );
    }

    #[test]
    fn sample_project_prefab_drop_instantiates_prefab_metadata() {
        let root = copy_sample_project("prefab-drop");
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/menu.scene"));
        let before = app
            .document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .len();

        app.spawn_prefab_from_drop("assets/prefabs/slime_green.prefab")
            .unwrap();

        let scene = app.document.current_scene.as_ref().unwrap();
        let prefab_roots: Vec<_> = scene
            .scene
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
        assert_eq!(prefab_roots.len(), 1, "drop should create one prefab root");
        assert_eq!(prefab_roots[0].name, "SlimeGreen");
        assert!(
            scene.scene.entities.len() > before,
            "drop should add prefab entities to the scene"
        );
        assert!(
            scene.is_dirty(),
            "instantiating prefab should dirty the scene"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_tile_paint_persists_after_save_and_reopen() {
        let root = copy_sample_project("tilepaint");
        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.refresh_resources();
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));
        app.sync_inspector(SceneEntityId(1));
        let width = app.tilemap.map.width;
        let x = 4usize;
        let y = 6usize;
        let index = y * width + x;

        app.tilemap.brush = 5;
        app.tilemap.paint_collision = false;
        app.apply_tile_paint_cell(x, y, false);
        app.tilemap.paint_collision = true;
        app.apply_tile_paint_cell(x, y, false);
        app.commit_tilemap_paint(SceneEntityId(1));

        let document = app.document.current_scene.as_ref().unwrap();
        let component = document.scene.entities[&SceneEntityId(1)].components["Tilemap"].clone();
        let tilemap = Tilemap::from_component_str(&component).unwrap();
        assert_eq!(tilemap.tiles[index], 5);
        assert!(
            tilemap.solid[index],
            "paint collision should persist to tilemap"
        );
        assert!(document.is_dirty(), "tile paint should dirty the scene");

        app.save_scene();
        assert_eq!(app.status, "Scene saved");
        app.open_scene_resource(PathBuf::from("scenes/production.scene"));
        let reopened = app.document.current_scene.as_ref().unwrap();
        let reopened_component =
            reopened.scene.entities[&SceneEntityId(1)].components["Tilemap"].clone();
        let reopened_tilemap = Tilemap::from_component_str(&reopened_component).unwrap();
        assert_eq!(reopened_tilemap.tiles[index], 5);
        assert!(
            reopened_tilemap.solid[index],
            "reopened scene should preserve collision paint"
        );
        let _ = fs::remove_dir_all(root);
    }
}
