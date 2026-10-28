//! 场景视图主绘制：面板骨架、工具条、对齐参考线、设计分辨率框。
//! 这里只负责"画"，拖动与拾取在 `pointer` / `scene_geometry`，渲染在 `render`。

use super::*;

impl EditorApp {
    pub(crate) fn draw_scene_view(&mut self, ui: &mut egui::Ui) {
        if self.scene_view_mode == SceneViewMode::View3D {
            self.draw_scene_view_3d(ui);
            return;
        }
        // E05-4：布局诊断按当前预览分辨率解算；切分辨率后先刷新，避免「溢出」角标沿用旧几何。
        self.refresh_layout_diagnostics_if_resolution_changed();
        // 画布宽度钉回本标签裁剪框：`available_size` 在窄叶子下可能被上一帧的横向溢出撑大
        // （egui_dock 的 `expand_to_include_rect` 会把溢出宽度回灌进可用矩形，见 `dock::show`
        // 里给工具条套 clip scope 的那段）。若照原样 allocate，画布右边界越出裁剪框、并把
        // 更大的可用宽度喂给下一帧，形成正反馈——变换工具条/zoom 数值随之被属性面板盖住。
        let clip_width = ui.clip_rect().width();
        let avail = ui.available_size();
        let desired = egui::vec2(avail.x.min(clip_width), avail.y);
        let (response, painter) = ui.allocate_painter(desired, egui::Sense::click_and_drag());
        let rect = response.rect;
        // docs/55 阶段 0：把本帧画布矩形公开给 GUI smoke（合成指针事件要打在真实画布上，
        // 并且要用面板自己的坐标换算把实体中心变成屏幕点）。
        self.scene_canvas_rect = rect;
        // 打开场景/切入 2D 后，把视图适配到设计分辨率包围框：完整显示并居中，
        // 避免大包围框在窄面板下被裁剪到属性面板边缘。
        if self.scene_view_fit_pending {
            self.scene_view_fit_pending = false;
            self.fit_scene_view_to_design(rect);
        }
        // 拖放：把资源浏览器中的纹理拖到场景创建精灵。
        // M27-01：载荷类型统一成 `DropPayload` 后，"落在哪 ⇒ 做什么"由 `dnd::plan` 一处判定。
        let (_, dropped) = ui.dnd_drop_zone::<crate::dnd::DropPayload, egui::Response>(
            egui::Frame::default(),
            |ui| ui.allocate_rect(rect, egui::Sense::hover()),
        );
        if let Some(payload) = dropped {
            let source = crate::dnd::DragSource::Internal((*payload).clone());
            let action = crate::dnd::plan(&source, crate::dnd::DropZone::SceneCanvas);
            self.status = self.commit_spawn_drop(&action, None);
        }
        // 系统文件直接拖进窗口：落在画布上就是"导入 + 建实体"（可生成的那批）。
        if let Some((action, drop)) =
            self.claim_external_drop(crate::dnd::DropZone::SceneCanvas, rect)
        {
            let text = self.commit_spawn_drop(&action, None);
            self.status = format!("{text}{}", drop.notes());
        }
        let drop_hint = self.external_hover_action(
            crate::dnd::DropZone::SceneCanvas,
            rect,
            ui.input(|input| input.pointer.hover_pos()),
        );
        // 仅当鼠标悬停在 Scene 视图内时才响应滚轮缩放（避免在其它面板滚动误触发）。
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if response.contains_pointer() && scroll.abs() > 0.0 {
            self.set_scene_zoom_centered(self.scene_zoom, self.scene_zoom * (1.0 + scroll * 0.001));
        }
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(28, 32, 40));
        // 实时渲染编辑场景（精灵/图块/文本）作为背景。
        let scale = ui.ctx().pixels_per_point();
        // 中键拖拽平移（内容跟随鼠标）：面板增量（points）× ppp = 世界像素位移。
        if response.dragged_by(egui::PointerButton::Middle) {
            let d = response.drag_delta();
            self.scene_state.pan.x += d.x * scale;
            self.scene_state.pan.y -= d.y * scale;
        }
        let view_w = (rect.width() * scale) as u32;
        let view_h = (rect.height() * scale) as u32;
        self.apply_drag_preview_to_runtime();
        self.render_scene_view_2d(view_w, view_h, ui.ctx());
        if let Some(tex) = &self.scene_2d_tex {
            painter.image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        // 设计分辨率包围框（Unity Camera Preview 风格）：标注游戏取景范围。
        self.draw_design_resolution_box(ui, &painter, rect, scale);
        // 拖放反馈画在场景渲染之后（否则会被整张离屏纹理盖掉）。
        let pointer = ui.input(|input| input.pointer.hover_pos());
        let drop_hint = drop_hint.or_else(|| {
            crate::dnd::internal_drag(ui.ctx())
                .filter(|_| pointer.is_some_and(|pointer| rect.contains(pointer)))
                .map(|source| crate::dnd::plan(&source, crate::dnd::DropZone::SceneCanvas))
        });
        if let Some(action) = drop_hint {
            crate::dnd::paint_drop_feedback(ui, rect, 2, &action);
        }
        // E18：Safe Area 由 Canvas 配置驱动（insets 像素按设计分辨率钳制），
        // 不再用固定的 5% 估算——后者与 Inspector 里的 `canvas.safe_area` 无关。
        if self.show_safe_area
            && let Some(safe_frame) = design_safe_frame(
                rect,
                self.scene_state.pan,
                self.scene_zoom,
                scale,
                self.game_camera_center_egui(),
                self.preview_resolution,
                &self.scene_canvas_config(),
            )
        {
            painter.rect_stroke(
                safe_frame,
                0.0_f32,
                egui::Stroke::new(
                    1.0_f32,
                    egui::Color32::from_rgba_unmultiplied(100, 220, 160, 180),
                ),
                egui::StrokeKind::Inside,
            );
        }
        if self.show_pixel_grid {
            let grid_size = 16.0 * self.scene_zoom / scale;
            if grid_size >= 4.0 {
                let grid_stroke = egui::Stroke::new(
                    1.0_f32,
                    egui::Color32::from_rgba_unmultiplied(150, 170, 190, 45),
                );
                let x_origin = rect.center().x + self.scene_state.pan.x / scale;
                let y_origin = rect.center().y - self.scene_state.pan.y / scale;
                let mut x = rect.left() + (x_origin - rect.left()).rem_euclid(grid_size);
                while x < rect.right() {
                    painter.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        grid_stroke,
                    );
                    x += grid_size;
                }
                let mut y = rect.top() + (y_origin - rect.top()).rem_euclid(grid_size);
                while y < rect.bottom() {
                    painter.line_segment(
                        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                        grid_stroke,
                    );
                    y += grid_size;
                }
            }
        }
        if self.show_scene_guides {
            let guide_stroke = egui::Stroke::new(
                1.0_f32,
                egui::Color32::from_rgba_unmultiplied(120, 190, 220, 110),
            );
            painter.line_segment(
                [
                    egui::pos2(rect.center().x, rect.top()),
                    egui::pos2(rect.center().x, rect.bottom()),
                ],
                guide_stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(rect.left(), rect.center().y),
                    egui::pos2(rect.right(), rect.center().y),
                ],
                guide_stroke,
            );
        }
        // 在包围框内显示游戏画面（设计分辨率 + 游戏相机 + UI 覆盖层）：
        // 实际 UI 随包围框缩放/平移（包围框 world 映射）。
        if let Some(id) = self.game_screen_texture_id() {
            let frame = design_resolution_frame(
                rect,
                self.scene_state.pan,
                self.scene_zoom,
                scale,
                self.game_camera_center_egui(),
                self.preview_resolution,
            );
            painter.image(
                id,
                frame,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        // 性能数字**不在这里画**：它们是独立弹窗（`panels::performance`），
        // 因为本视图右上角是工具小面板（见 `draw_scene_tool_panel`），两段文字叠在一起时
        // 工具按钮被盖住、数字也读不全。
        // Scene View 的过滤只按**名称词**（`type:` 词管 Hierarchy 的行可见性，
        // 不该让视口里的对象凭空消失——见 `HierarchyFilter::matches_name`）。
        let filter = crate::panels::hierarchy::HierarchyFilter::parse(&self.hierarchy_filter);
        let entities = self.collect_scene_2d_entities(rect, &filter, scale);
        let tilemap_context = self.selected_tilemap_context();
        if self.show_scene_guides
            && let Some(scene) = self.current_scene()
        {
            let connection_stroke = egui::Stroke::new(
                1.0_f32,
                egui::Color32::from_rgba_unmultiplied(170, 170, 190, 130),
            );
            for entity in &entities {
                let Some(parent_id) = scene
                    .scene
                    .entities
                    .get(&entity.id)
                    .and_then(|value| value.parent)
                else {
                    continue;
                };
                let Some(parent) = entities.iter().find(|value| value.id == parent_id) else {
                    continue;
                };
                painter.line_segment([parent.center, entity.center], connection_stroke);
            }
        }
        for entity in entities.iter() {
            let box_rect = egui::Rect::from_center_size(entity.center, entity.size);
            let selected = self.selection.contains(entity.id);
            // 场景已实时渲染（含实际 UI 覆盖层），实体只画选中轮廓 + 名称。
            if selected {
                painter.rect_stroke(
                    box_rect,
                    0.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(230, 170, 60)),
                    egui::StrokeKind::Outside,
                );
                painter.text(
                    entity.center + egui::vec2(0.0, -entity.size.y * 0.5 - 6.0),
                    egui::Align2::CENTER_BOTTOM,
                    &entity.name,
                    egui::FontId::proportional(14.0),
                    egui::Color32::LIGHT_YELLOW,
                );
                painter.circle_stroke(
                    entity.center,
                    4.0,
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 220, 90)),
                );
                painter.line_segment(
                    [
                        entity.center - egui::vec2(7.0, 0.0),
                        entity.center + egui::vec2(7.0, 0.0),
                    ],
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 220, 90)),
                );
                painter.line_segment(
                    [
                        entity.center - egui::vec2(0.0, 7.0),
                        entity.center + egui::vec2(0.0, 7.0),
                    ],
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 220, 90)),
                );
            }
            if response.clicked()
                && response
                    .interact_pointer_pos()
                    .is_some_and(|pos| box_rect.contains(pos))
            {
                let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                let source = SelectionSource::SceneView2D;
                if additive {
                    self.selection.toggle(entity.id, source);
                } else {
                    self.selection.set_single(entity.id, source);
                }
                self.sync_inspector(entity.id);
            }
        }

        let pointer = response.interact_pointer_pos();
        let hovered_tile = pointer.and_then(|pos| {
            tilemap_context.as_ref().and_then(|context| {
                tilemap_cell_from_pointer(
                    rect,
                    self.scene_state.pan,
                    self.scene_zoom,
                    scale,
                    pos,
                    context.transform,
                    &self.tilemap.map,
                )
            })
        });
        let pending_tile_rect = self.scene_state.tile_rect_anchor.and_then(|anchor| {
            self.scene_state
                .tile_rect_current
                .map(|current| CellRect::normalize(anchor, current))
        });
        if let Some(context) = tilemap_context.as_ref() {
            self.draw_tilemap_overlay(&painter, rect, context, hovered_tile, pending_tile_rect);
        }
        // M19-03：领域数据只读覆盖层（当前地图的出口 / 地点 / 出生格）。绘制在
        // Tile 网格之上、实体选中框之下，与既有 UI 布局/网格共用同一坐标换算。
        self.draw_domain_overlay(&painter, rect, scale);
        // 调试覆盖层：画出场景里**所有**碰撞区域（Tilemap solid 格 + 实体 Collider），
        // 与选中无关，供跑地图调试用。开关在 View ▸ 碰撞区域(调试)。
        self.draw_collision_overlay(&painter, rect, scale);
        if let Some(context) = tilemap_context.as_ref()
            && self.scene_tool() == SceneTool::TilePaint
        {
            // E11：TilePaint 同样走交互状态机（按下 → `Painting`，笔触 → `TileStroke`，
            // 松手 → `Idle`），这样"连续绘制"的相位也有单一事实源。
            if let Some(pos) = pointer
                && response.drag_started_by(egui::PointerButton::Primary)
            {
                self.interaction.handle(InteractionEvent::PointerDown {
                    pos: [pos.x, pos.y],
                    hit: true,
                });
            }
            self.handle_tilemap_paint(&response, rect, context);
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                self.interaction.handle(InteractionEvent::PointerUp {
                    pos: pointer.map_or([0.0, 0.0], |pos| [pos.x, pos.y]),
                });
            }
            self.draw_scene_tool_panel(ui, rect);
            return;
        }
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pos) = drag_start_pos(ui, pointer)
        {
            let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
            self.scene_pointer_down(pos, &entities, additive);
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(current) = pointer
        {
            self.scene_pointer_drag(current);
            // 预览推进（从绘制里抽出来的方法）：合成拖动测试也走这条路径。
            self.scene_pointer_drag_apply(current, scale);
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            self.scene_pointer_up(pointer.unwrap_or(egui::Pos2::ZERO));
        }
        if let (ToolPhase::Marquee { start, .. }, Some(current)) =
            (self.interaction.phase(), pointer)
            && response.dragged_by(egui::PointerButton::Primary)
        {
            let start = egui::pos2(start[0], start[1]);
            let selection_rect = egui::Rect::from_two_pos(start, current);
            painter.rect_stroke(
                selection_rect,
                0.0,
                egui::Stroke::new(1.0_f32, egui::Color32::LIGHT_BLUE),
                egui::StrokeKind::Outside,
            );
            for id in entities_in_selection_rect(&entities, selection_rect, start, current) {
                // 拖动过程中持续累加；非加选时集合已在框选开始时清空。
                self.selection.add(id, SelectionSource::SceneView2D);
            }
        }
        // M27-02（缺项 3）：旋转工具在 2D 里画成**圆环 + 朝向把手**。此前"横着拖就转"在画面上
        // 没有任何对应物，用户不知道该往哪儿拖；环心取的是这一趟**真实的转轴**，所以轴心模式
        // （Pivot/Center）一改，环就跟着挪。
        if self.scene_tool() == SceneTool::Rotate {
            self.draw_scene_2d_rotation_ring(&painter, rect, scale, &entities);
        }
        // M27-02（缺项 7）：拖动中的即时数值浮标，省掉"拖一下、再扭头看 Inspector"的来回。
        if let Some(current_pointer) = pointer {
            self.draw_scene_2d_drag_readout(&painter, current_pointer, &entities);
        }
        // 工具小面板（Select/Move/Rotate/Scale）置于场景右上角。
        self.draw_scene_tool_panel(ui, rect);
    }

    // ---- E11：2D 交互的三个入口（从 `draw_scene_2d` 抽出来，便于脱离绘制单测） ----
    //
    // docs/55 阶段 2 的验收条款之一是"绘制与交互可独立测试"。把"指针按下/拖动/释放"
    // 抽成方法后，交互决策可以在**没有 egui 帧、没有 GPU** 的情况下直接调用与断言；
    // 绘制函数只负责把输入喂进来、并把相位画出来。

    /// 在 2D Scene 视图绘制设计分辨率包围框（仿 Unity Camera Preview）：以游戏 2D
    /// 相机中心为世界中心、设计分辨率为尺寸的半透明矩形，按编辑器 pan/zoom 映射到
    /// 面板坐标，让设计者在编辑时看到游戏镜头能拍到的取景范围。分辨率标注固定显示
    /// 在视图左上角标题下方（包围框可能被缩放/平移推出可视区域，固定标注始终可见）。
    pub(crate) fn draw_design_resolution_box(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let (w, h) = self.preview_resolution;
        if w == 0 || h == 0 {
            return;
        }
        let frame = design_resolution_frame(
            rect,
            self.scene_state.pan,
            self.scene_zoom,
            ppp,
            self.game_camera_center_egui(),
            (w, h),
        );
        painter.rect_filled(
            frame,
            0.0,
            egui::Color32::from_rgba_unmultiplied(90, 170, 255, 16),
        );
        painter.rect_stroke(
            frame,
            0.0,
            egui::Stroke::new(
                1.5_f32,
                egui::Color32::from_rgba_unmultiplied(120, 190, 255, 210),
            ),
            egui::StrokeKind::Outside,
        );
        let base = rect.left_top() + egui::vec2(4.0, self.scene_overlay_bottom_offset + 4.0);
        // E18 的完整体检读数默认收起：常驻只留「分辨率 · 缩放」一行，悬停看
        // 预览/显示/裁剪/安全区/锚点框，视口角落不再被三行等宽文本占住。
        let hud = crate::ui_editor::CanvasHud {
            design: (w, h),
            display: (
                (frame.width() * ppp).round().max(0.0) as u32,
                (frame.height() * ppp).round().max(0.0) as u32,
            ),
            scale: self.scene_zoom,
            crop: design_frame_crop(rect, frame, self.scene_zoom, ppp),
            safe: self.scene_canvas_config().safe_area,
        };
        let hud_rect = painter.text(
            base,
            egui::Align2::LEFT_TOP,
            hud.compact_line(),
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgb(150, 195, 235),
        );
        ui.interact(
            hud_rect.expand(2.0),
            ui.id().with("design_resolution_hud"),
            egui::Sense::hover(),
        )
        .on_hover_text(hud.details());
    }
}
