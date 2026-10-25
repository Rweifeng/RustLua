//! 3D 视图：类型化场景的懒构建、相机与视角绘制、gizmo 环与 3D 变换取用。

use super::*;

impl EditorApp {
    /// 确保 3D 视图的 typed 提取与当前文档 revision 一致（WP1-4）。
    ///
    /// 组件字符串只在文档变化时解析一次；用宽容编译，坏组件只丢自己并留下诊断，
    /// 不会让整个 3D 视图空白。
    pub(crate) fn ensure_scene_view_3d_typed(&mut self) {
        let revision =
            panel_scene_document(&self.play_session, &self.document).map(|doc| doc.revision);
        if self.scene_view_3d_typed.revision == revision {
            return;
        }
        let (entities, diagnostics) = match panel_scene_document(&self.play_session, &self.document)
        {
            Some(scene) => {
                // 表现层组件的 codec 由组合根装一次表（P1 ④）：scene 层只剩"名字 + 原文"，
                // `Sprite`/`Text` 的文法归 render/ui。编辑器是场景文本的编译方，必须在这里装。
                engine::host::scene_codecs::install();
                let (typed, diagnostics) = TypedScene::compile_v2_lenient(&scene.scene);
                let entities = typed
                    .entities
                    .iter()
                    .filter_map(|entity| {
                        let Some(TypedComponent::Transform(transform)) =
                            entity.components.get(&ComponentId::Transform)
                        else {
                            return None;
                        };
                        let Some(TypedComponent::Opaque { assets, .. }) =
                            entity.components.get(&ComponentId::MeshRenderer)
                        else {
                            return None;
                        };
                        Some(SceneView3dTypedEntity {
                            id: entity.id,
                            name: entity.name.clone(),
                            transform: *transform,
                            // mesh 路径由 render 的 codec 从载荷里报出来（P1 ④），
                            // 编辑器不自己再解析一遍 `MeshRenderer` 的文法。
                            mesh_path: assets.first()?.0.clone(),
                        })
                    })
                    .collect::<Vec<_>>();
                // 只报 3D 相关组件的诊断：其余组件的问题归 Inspector / Console，
                // 混进 3D 视图提示会误导（旧 serialized 提取也只覆盖这几个组件）。
                let diagnostics = diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        matches!(
                            diagnostic.component.as_str(),
                            "Transform" | "Camera" | "MeshRenderer"
                        )
                    })
                    .map(|diagnostic| {
                        format!(
                            "{} / {}：{}",
                            diagnostic.name, diagnostic.component, diagnostic.message
                        )
                    })
                    .collect::<Vec<_>>();
                (entities, diagnostics)
            }
            None => (Vec::new(), Vec::new()),
        };
        self.scene_view_3d_typed.revision = revision;
        self.scene_view_3d_typed.entities = entities;
        self.scene_view_3d_typed.diagnostics = diagnostics;
    }

    /// 3D 场景视图（M9-03）：XZ 俯视投影绘制 Transform3D 实体，选中实体显示
    /// 平移 Gizmo（X/Z 轴手柄、Y 高度手柄、自由拖拽），拖拽写回 Transform3D。
    pub(crate) fn draw_scene_view_3d(&mut self, ui: &mut egui::Ui) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let rect = response.rect;
        let scale = ui.ctx().pixels_per_point();
        let width = (rect.width() * scale) as u32;
        let height = (rect.height() * scale) as u32;
        // M27-01：3D 视图同样是放置区（把 .glb / 预制体拖进透视口是游戏编辑器的常态动作）。
        // 这里的落点只有"建实体"一种语义：控件属于 Widget 模式，重排属于层级。
        let (_, dropped) = ui
            .dnd_drop_zone::<crate::dnd::DropPayload, _>(egui::Frame::default(), |ui| {
                ui.allocate_rect(rect, egui::Sense::hover())
            });
        if let Some(payload) = dropped {
            let source = crate::dnd::DragSource::Internal((*payload).clone());
            let action = crate::dnd::plan(&source, crate::dnd::DropZone::SceneCanvas);
            self.status = self.commit_spawn_drop(&action, None);
        }
        if let Some((action, drop)) =
            self.claim_external_drop(crate::dnd::DropZone::SceneCanvas, rect)
        {
            let text = self.commit_spawn_drop(&action, None);
            self.status = format!("{text}{}", drop.notes());
        }

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
            "3D 场景视图  -  中键旋转 / 右键平移 / 滚轮缩放",
            egui::FontId::proportional(15.0),
            egui::Color32::LIGHT_GRAY,
        );

        // 收集 3D 实体（Transform3D），投影到屏幕。typed 提取按文档 revision 缓存，
        // 不再逐帧解析组件字符串（WP1-4）。
        if self.current_scene().is_none() {
            return;
        }
        self.ensure_scene_view_3d_typed();
        let filter = crate::panels::hierarchy::HierarchyFilter::parse(&self.hierarchy_filter);
        let diagnostic = if self.scene_view_3d_typed.diagnostics.is_empty() {
            None
        } else {
            Some(format!(
                "{} 个 3D 组件无效：{}",
                self.scene_view_3d_typed.diagnostics.len(),
                self.scene_view_3d_typed
                    .diagnostics
                    .iter()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("；")
            ))
        };
        match diagnostic {
            Some(message) => self.set_scene_view_diagnostic(message),
            None => self.clear_scene_view_diagnostic(),
        }
        let entities3d: Vec<_> = self
            .scene_view_3d_typed
            .entities
            .iter()
            .filter(|entity| filter.matches_name(&entity.name.to_lowercase()))
            .map(|entity| {
                let bounds = rustlua_math::math3d::Aabb3 {
                    min: Vec3::new(-0.5, -0.5, -0.5),
                    max: Vec3::new(0.5, 0.5, 0.5),
                };
                // docs/54 §8-⑦：标记 / 点选 / Gizmo 初值必须与**3D 画面同源**（世界变换）。
                // 用文档里的局部值会让父链下的实体"手柄在人不在的地方"。
                let transform = self
                    .scene_view_3d_world_transform(entity.id)
                    .unwrap_or(entity.transform);
                (entity.id, entity.name.clone(), transform, bounds)
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
            let selected = self.selection.contains(*id);
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
                    pick_scene_3d_entity(
                        ray,
                        &entities3d,
                        &self.scene_state.locked,
                        &self.scene_state.preview3d_transform,
                    )
                });
            if let Some(id) = hit {
                let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                if additive {
                    self.selection.add(id, SelectionSource::SceneView3D);
                } else {
                    self.selection.set_single(id, SelectionSource::SceneView3D);
                }
                self.sync_inspector(id);
            } else {
                self.selection.clear(SelectionSource::SceneView3D);
                self.clear_inspector();
            }
        }

        // Gizmo：按工具绘制手柄并拖拽（Move 平移 / Rotate 绕 Y / Scale 等比缩放）。
        //
        // docs/55 阶段 2 收口：手柄作用于**整个选中集合**——锚点按轴心模式取"主实体原点"
        // 或"选中集合并集包围盒中心"，拖拽时对每个目标算各自的新世界变换（见 `crate::gizmo`）。
        let selected: Vec<(SceneEntityId, Transform3D, rustlua_math::math3d::Aabb3)> = entities3d
            .iter()
            .filter(|(id, _, _, _)| self.selection.contains(*id))
            .map(|(id, _, transform, bounds)| (*id, *transform, *bounds))
            .collect();
        let primary_selected = self
            .selection
            .primary()
            .filter(|id| selected.iter().any(|(candidate, _, _)| candidate == id))
            .or_else(|| selected.first().map(|(id, _, _)| *id));
        if let Some(primary) = primary_selected
            && let Some((_, primary_transform, _)) =
                selected.iter().find(|(id, _, _)| *id == primary).copied()
        {
            let tool = self.scene_tool();
            if matches!(
                tool,
                SceneTool::Move | SceneTool::Select | SceneTool::Rotate | SceneTool::Scale
            ) {
                // 轴心模式决定手柄画在哪里（Pivot = 主实体原点；Center = 选中集合的包围盒中心）。
                let origin = crate::gizmo::gizmo_anchor(
                    self.gizmo_pivot,
                    primary_transform.position,
                    selected
                        .iter()
                        .map(|(_, transform, bounds)| (transform.position, *bounds)),
                );
                let Some(screen) = self.project_to_screen(origin, rect) else {
                    return;
                };
                let len = 40.0;
                // 手柄方向来自**真实轴的投影**（第 45 轮）：局部模式因此看得见（实体转多少、
                // 手柄转多少），相机转动时手柄也跟着走。命中检测用同一份方向，
                // 所以"看到哪里就点哪里"成立。
                let axes = gizmo_screen_axes(
                    origin,
                    screen,
                    len,
                    self.gizmo_space,
                    primary_transform.rotation,
                    |point| self.project_to_screen(point, rect),
                );
                let pointer = response.interact_pointer_pos();
                // 旋转的三支环：把 3D 圆**投影**成屏幕椭圆（不是画三个同心圆——那样三条环
                // 会完全重叠，"哪一条"无从判断）。绘制与命中读同一份折线。
                let rings: Vec<(GizmoAxis, Vec<egui::Pos2>)> = if tool == SceneTool::Rotate {
                    [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z]
                        .into_iter()
                        .filter_map(|slot| {
                            let normal = gizmo_axis_world(
                                self.gizmo_space,
                                primary_transform.rotation,
                                slot,
                            )?;
                            let radius =
                                crate::gizmo::ring_radius_world(origin, normal, len, |point| {
                                    self.project_to_screen(point, rect)
                                })?;
                            let polyline =
                                crate::gizmo::ring_polyline(origin, normal, radius, |point| {
                                    self.project_to_screen(point, rect)
                                });
                            (!polyline.is_empty()).then_some((slot, polyline))
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                // 命中检测：按工具判定手柄。
                let axis = if response.drag_started_by(egui::PointerButton::Primary)
                    && let Some(pos) = drag_start_pos(ui, pointer)
                {
                    crate::gizmo::pick_gizmo_axis(
                        tool,
                        screen,
                        pos,
                        selected
                            .iter()
                            .any(|(id, _, _)| self.scene_state.locked.contains(id)),
                        axes,
                    )
                    .or_else(|| {
                        // 环上的点：只有旋转换到这一层才判（三条环要按到椭圆的距离比大小）。
                        if tool == SceneTool::Rotate {
                            crate::gizmo::pick_rotation_ring(pos, &rings)
                        } else {
                            None
                        }
                    })
                } else {
                    None
                };
                // 绘制手柄。
                match tool {
                    SceneTool::Move | SceneTool::Select => {
                        painter.line_segment(
                            [screen, screen + axes[0]],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(220, 80, 80)),
                        );
                        painter.circle_filled(
                            screen + axes[0],
                            5.0,
                            egui::Color32::from_rgb(220, 80, 80),
                        );
                        painter.line_segment(
                            [screen, screen + axes[2]],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(80, 120, 220)),
                        );
                        painter.circle_filled(
                            screen + axes[2],
                            5.0,
                            egui::Color32::from_rgb(80, 120, 220),
                        );
                        painter.line_segment(
                            [screen, screen + axes[1]],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 200, 90)),
                        );
                        painter.circle_filled(
                            screen + axes[1],
                            5.0,
                            egui::Color32::from_rgb(90, 200, 90),
                        );
                    }
                    SceneTool::Rotate => {
                        // 三支环各按自己的轴上色（X 红 / Y 绿 / Z 蓝），指针正压着的那条加粗——
                        // "先看清会绕哪根轴，再决定按下去"。
                        let hovered =
                            ui.input(|input| input.pointer.hover_pos())
                                .and_then(|position| {
                                    crate::gizmo::pick_rotation_ring(position, &rings)
                                });
                        for (slot, polyline) in &rings {
                            if polyline.len() < 2 {
                                continue;
                            }
                            let color = match slot {
                                GizmoAxis::X => egui::Color32::from_rgb(230, 100, 100),
                                GizmoAxis::Y => egui::Color32::from_rgb(120, 210, 120),
                                _ => egui::Color32::from_rgb(110, 150, 235),
                            };
                            let width = if hovered == Some(*slot) {
                                3.5_f32
                            } else {
                                2.0_f32
                            };
                            painter.add(egui::Shape::closed_line(
                                polyline.clone(),
                                egui::Stroke::new(width, color),
                            ));
                        }
                    }
                    SceneTool::Scale => {
                        // 中心 = 等比（旧行为），六个把手 = 单轴 / 双轴；位置与命中读同一份。
                        painter.rect_filled(
                            egui::Rect::from_center_size(
                                screen,
                                egui::Vec2::splat(crate::gizmo::GIZMO_CENTER_HALF * 1.6),
                            ),
                            2.0,
                            egui::Color32::from_rgb(180, 100, 220),
                        );
                        for (handle, offset) in crate::gizmo::scale_handles(axes) {
                            let color = match handle {
                                GizmoAxis::ScaleX => egui::Color32::from_rgb(220, 80, 80),
                                GizmoAxis::ScaleY => egui::Color32::from_rgb(90, 200, 90),
                                GizmoAxis::ScaleZ => egui::Color32::from_rgb(80, 120, 220),
                                _ => egui::Color32::from_rgb(180, 100, 220),
                            };
                            let point = screen + offset;
                            if matches!(
                                handle,
                                GizmoAxis::ScaleXY | GizmoAxis::ScaleXZ | GizmoAxis::ScaleYZ
                            ) {
                                painter.rect_filled(
                                    egui::Rect::from_center_size(point, egui::Vec2::splat(7.0)),
                                    1.0,
                                    color,
                                );
                            } else {
                                painter.line_segment(
                                    [screen, point],
                                    egui::Stroke::new(1.5_f32, color),
                                );
                                painter.circle_filled(point, 4.5, color);
                            }
                        }
                    }
                    SceneTool::TilePaint => {}
                }
                if let Some(axis) = axis {
                    // E11：Gizmo 抓取也走同一个交互状态机（命中手柄 = 拖动开始）。
                    // 相机导航（未命中手柄的拖动）**不是**文档交互，因此不喂事件。
                    let Some(start) = drag_start_pos(ui, pointer) else {
                        return;
                    };
                    self.interaction.handle(InteractionEvent::PointerDown {
                        pos: [start.x, start.y],
                        hit: true,
                    });
                    let targets = crate::gizmo::GizmoDragTargets::new(
                        primary,
                        selected.iter().map(|(id, transform, _)| (*id, *transform)),
                        &self.scene_state.locked,
                    );
                    let Some(targets) = targets else {
                        return;
                    };
                    self.scene_state.gizmo_drag = Some(crate::gizmo::GizmoDrag {
                        targets,
                        axis,
                        start,
                        // 锚点作为"参考变换"存下来：位置的射线求交用它，
                        // 旋转/缩放不需要它的旋转（各自用自己的）。
                        origin: Transform3D {
                            position: origin,
                            ..primary_transform
                        },
                        // 轴心模式 `Center` 时记下手柄原点：拖动过程中"绕这个点公转"
                        // （`Pivot` 模式下为 `None` = 原地旋转/缩放，与迁移前一致）。
                        pivot: (self.gizmo_pivot == crate::scene_interaction::GizmoPivot::Center)
                            .then_some(origin),
                        start_ray: {
                            let px = (start.x - rect.left()) * ui.ctx().pixels_per_point();
                            let py = (start.y - rect.top()) * ui.ctx().pixels_per_point();
                            self.editor_camera_ray(px, py, width as f32, height as f32)
                        },
                    });
                }
                // 拖拽更新：按工具修改 Transform3D（position/rotation/scale）。
                if response.dragged_by(egui::PointerButton::Primary)
                    && let (Some(drag), Some(pos)) = (self.scene_state.gizmo_drag.as_ref(), pointer)
                {
                    self.interaction.handle(InteractionEvent::PointerDrag {
                        pos: [pos.x, pos.y],
                    });
                    let delta = pos - drag.start;
                    let anchor = drag.origin.position;
                    // 世界增量只算一次（射线与轴求交），然后**整批**应用——多选平移时
                    // 每个目标用同一份增量，不会各算一份产生相对漂移。
                    let world_delta = drag.start_ray.and_then(|start_ray| {
                        let px = (pos.x - rect.left()) * ui.ctx().pixels_per_point();
                        let py = (pos.y - rect.top()) * ui.ctx().pixels_per_point();
                        let current_ray =
                            self.editor_camera_ray(px, py, width as f32, height as f32)?;
                        let axis = gizmo_axis_world(
                            self.gizmo_space,
                            drag.primary_initial().rotation,
                            drag.axis,
                        )?;
                        let start = start_ray.closest_point_on_line(anchor, axis)?.1;
                        let current = current_ray.closest_point_on_line(anchor, axis)?.1;
                        Some(axis * (current - start))
                    });
                    let primary = drag.primary();
                    // 旋转角优先按**环面求交**算（指针在环平面上的落点绕环心转了多少度），
                    // 几何退化时退回水平增量；吸附步长用**角度**单位，与位置的 `snap_step` 分开。
                    let angle_step = self.snap_enabled.then_some(self.snap_angle_step);
                    let ring = if tool == SceneTool::Rotate {
                        let (raw, slot) = self.gizmo_ring_angle(
                            drag,
                            pos,
                            rect,
                            ui.ctx().pixels_per_point(),
                            (width as f32, height as f32),
                            delta,
                        );
                        Some((crate::gizmo::snap_angle(raw, angle_step), slot))
                    } else {
                        None
                    };
                    let previews = match tool {
                        SceneTool::Move | SceneTool::Select => {
                            let d = self.editor_camera.dist * 0.002;
                            let world = world_delta.unwrap_or_else(|| match drag.axis {
                                // 没有射线（退化视角）时退回屏幕近似：与迁移前一致的兜底。
                                GizmoAxis::X | GizmoAxis::Y | GizmoAxis::Z => {
                                    Vec3::new(delta.x * d, -delta.y * d, 0.0)
                                }
                                GizmoAxis::Free => {
                                    let fx = -self.editor_camera.yaw.sin();
                                    let fz = -self.editor_camera.yaw.cos();
                                    let rx = self.editor_camera.yaw.cos();
                                    let rz = -self.editor_camera.yaw.sin();
                                    Vec3::new(
                                        (delta.x * rx - delta.y * fx) * d,
                                        0.0,
                                        (delta.x * rz - delta.y * fz) * d,
                                    )
                                }
                                // 缩放手柄不会走到这条分支（工具不同）；留个兜底不为零增量。
                                _ => Vec3::ZERO,
                            });
                            crate::gizmo::translate_targets(
                                &drag.targets.targets,
                                world,
                                self.snap_enabled.then_some(self.snap_step),
                            )
                        }
                        SceneTool::Rotate => crate::gizmo::rotate_targets(
                            self.gizmo_space,
                            ring.map_or(Vec3::Y, |(_, slot)| slot),
                            &drag.targets.targets,
                            drag.pivot,
                            ring.unwrap_or((delta.x * 0.01, Vec3::Y)).0,
                        ),
                        SceneTool::Scale => {
                            let direction = crate::gizmo::scale_handle_direction(drag.axis, axes);
                            crate::gizmo::scale_axis_targets(
                                &drag.targets.targets,
                                drag.pivot,
                                crate::gizmo::scale_axis_mask(drag.axis),
                                crate::gizmo::scale_factor(delta, direction),
                            )
                        }
                        SceneTool::TilePaint => Vec::new(),
                    };
                    // 即时数值浮标：拖动中把"现在是多少 / 转了多少"直接标在手柄旁边，
                    // 省掉"拖一下、再看 Inspector"的来回（M27-02 缺项 7）。
                    if let Some((_, preview)) = previews.iter().find(|(id, _)| *id == primary) {
                        let text = crate::gizmo::readout_3d(
                            tool,
                            drag.axis,
                            preview,
                            ring.unwrap_or((0.0, Vec3::Y)).0,
                        );
                        if !text.is_empty() {
                            painter.text(
                                screen + egui::vec2(14.0, -len - 16.0),
                                egui::Align2::LEFT_TOP,
                                text,
                                // 与 2D 浮标同一份字号：两侧"拖动手上的数字"必须看起来是同一个东西。
                                egui::FontId::proportional(13.0),
                                egui::Color32::from_rgb(240, 220, 120),
                            );
                        }
                    }
                    for (id, next) in previews {
                        self.scene_state.preview3d_transform.insert(id, next);
                    }
                }
                if response.drag_stopped_by(egui::PointerButton::Primary) {
                    self.commit_gizmo_drag3d();
                    self.scene_state.gizmo_drag = None;
                    self.interaction.handle(InteractionEvent::PointerUp {
                        pos: pointer.map_or([0.0, 0.0], |pos| [pos.x, pos.y]),
                    });
                }
            }
        }
        // 工具小面板（Select/Move/Rotate/Scale）置于场景右上角。
        self.draw_scene_tool_panel(ui, rect);
    }

    /// 环拖动的**转角**：两条视线与环平面求交，取交点绕环心的带符号夹角。
    ///
    /// 返回 `(转角, 槽位轴)`，槽位轴喂给 `gizmo::rotate_targets`：**测量**用的是画出来的
    /// 那条环的法向（局部模式下 = 主实体自己的轴），**应用**用的是槽位（X/Y/Z）——局部模式
    /// 下每个目标绕它自己的同名轴转，两者不是同一个向量，混用会让多选旋转歪掉。
    ///
    /// 相机几乎贴着环面看（交点跑到无穷远）、指针指到转轴上、或没有射线时，退回水平增量
    /// `delta.x * 0.01`——就是迁移前那条公式，作为退化路径继续存在。
    pub(crate) fn gizmo_ring_angle(
        &self,
        drag: &crate::gizmo::GizmoDrag,
        pointer: egui::Pos2,
        rect: egui::Rect,
        pixels_per_point: f32,
        size: (f32, f32),
        delta: egui::Vec2,
    ) -> (f32, Vec3) {
        let rotation = drag.primary_initial().rotation;
        let slot = gizmo_axis_world(
            crate::scene_interaction::GizmoSpace::World,
            rotation,
            drag.axis,
        );
        let measured = || {
            let normal = gizmo_axis_world(self.gizmo_space, rotation, drag.axis)?;
            let start_ray = drag.start_ray?;
            let current_ray = self.editor_camera_ray(
                (pointer.x - rect.left()) * pixels_per_point,
                (pointer.y - rect.top()) * pixels_per_point,
                size.0,
                size.1,
            )?;
            let center = drag.origin.position;
            let start = crate::gizmo::ray_plane_point(start_ray, center, normal)?;
            let current = crate::gizmo::ray_plane_point(current_ray, center, normal)?;
            crate::gizmo::ring_angle(center, normal, start, current)
        };
        match (measured(), slot) {
            (Some(angle), Some(slot)) => (angle, slot),
            _ => (delta.x * 0.01, slot.unwrap_or(Vec3::Y)),
        }
    }

    /// 3D 实体的**世界**变换：与 3D 画面同源——运行时可用时取它（渲染用的就是它），否则按文档
    /// 层级求解（dev 文档与运行时是同一套父先序规则）。
    ///
    /// docs/54 §8-⑦：标记、点选与 Gizmo 的**初值**都必须用它；用文档里的局部值会让父链下的实体
    /// "手柄在人不在的地方"（画面用世界值、覆层用局部值）。
    pub(crate) fn scene_view_3d_world_transform(
        &self,
        id: rustlua_scene::scene::SceneEntityId,
    ) -> Option<rustlua_ecs::transform::Transform3D> {
        if let Some(runtime) = &self.scene_runtime
            && let Some(world) = runtime.world_transform_3d_of(id)
        {
            return Some(world);
        }
        let scene = panel_scene_document(&self.play_session, &self.document)?;
        editor::document_space::world_transform_3d(&scene.scene, id)
    }

    // 本帧 2D 拖拽预览（`scene_state.preview` 里存的是**世界**值）换算成**局部**值：
    // 实现搬到 `crate::scene_drag::scene_drag_locals`（运行时预览与文档写回共用同一条换算，
    // 因此"看到的"和"写进去的"不会再各算一套；父与子同批移动时由
    // `locals_for_world_targets` 的父先序处理，不会二次叠加）。
}
