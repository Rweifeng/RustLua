//! 瓦片绘制：叠加显示、笔刷/矩形/粘贴三种落笔方式、格子与屏幕坐标互换。

use super::*;

impl EditorApp {
    pub(crate) fn selected_tilemap_context(&self) -> Option<TilemapPaintContext> {
        let selected = self.selection.primary()?;
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

    pub(crate) fn draw_tilemap_overlay(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        context: &TilemapPaintContext,
        hovered: Option<(usize, usize)>,
        pending_rect: Option<CellRect>,
    ) {
        let ppp = painter.ctx().pixels_per_point().max(1.0e-3);
        if context.transform.rotation.abs() > 0.001 {
            painter.text(
                rect.left_bottom() + egui::vec2(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                "瓦片画笔暂不支持旋转的瓦片地图",
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
                    ppp,
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
                if solid && self.tilemap.preview_collision {
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
        // M19-03：矩形工具的拖拽预览（矩形填充/区域复制的待选区）。
        if let Some(pending) = pending_rect {
            let fill = match self.tilemap.mode {
                TileEditMode::RegionCopy => egui::Color32::from_rgba_unmultiplied(90, 180, 230, 60),
                _ => egui::Color32::from_rgba_unmultiplied(230, 190, 80, 70),
            };
            let stroke = egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 230, 120));
            for y in pending.min_y..=pending.max_y {
                for x in pending.min_x..=pending.max_x {
                    let screen_rect = tilemap_cell_screen_rect(
                        rect,
                        self.scene_state.pan,
                        self.scene_zoom,
                        ppp,
                        context.transform,
                        &self.tilemap.map,
                        x,
                        y,
                    );
                    painter.rect_filled(screen_rect.shrink(1.0), 0.0, fill);
                    painter.rect_stroke(screen_rect, 0.0, stroke, egui::StrokeKind::Inside);
                }
            }
            painter.text(
                rect.left_top() + egui::vec2(12.0, 34.0),
                egui::Align2::LEFT_TOP,
                format!(
                    "{} · {}×{}（{} 格）",
                    self.tilemap.mode.label(),
                    pending.width(),
                    pending.height(),
                    pending.cell_count()
                ),
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(255, 230, 120),
            );
        }
    }

    /// M19-03：Tilemap 编辑的鼠标处理，按 `TileEditMode` 分派：
    /// - `Point`：原有单格画笔（左键画 / 右键擦，拖拽连续画）；
    /// - `RectFill`：拖拽框选 → 释放时**一次**矩形填充（单条 Undo）；
    /// - `RegionCopy`：拖拽框选 → 释放时把区域复制到编辑器剪贴板（不写文档）；
    /// - `RegionPaste`：单击 → 把剪贴板粘贴到该格（越界裁剪，单条 Undo）。
    pub(crate) fn handle_tilemap_paint(
        &mut self,
        response: &egui::Response,
        rect: egui::Rect,
        context: &TilemapPaintContext,
    ) {
        let ppp = response.ctx.pixels_per_point().max(1.0e-3);
        let pointer = response.interact_pointer_pos();
        let mode = self.tilemap.mode;
        let cell_at = |app: &Self, pos: egui::Pos2| app.tilemap_cell_at(rect, ppp, pos, context);
        if response.drag_started_by(egui::PointerButton::Primary) {
            self.scene_state.tile_paint_active = true;
            self.scene_state.tile_paint_last_cell = None;
            if matches!(mode, TileEditMode::RectFill | TileEditMode::RegionCopy) {
                let anchor = pointer.and_then(|pos| cell_at(self, pos));
                self.scene_state.tile_rect_anchor = anchor;
                self.scene_state.tile_rect_current = anchor;
            }
        }
        match mode {
            TileEditMode::Point => {
                if response.clicked_by(egui::PointerButton::Primary)
                    && let Some((x, y)) = pointer.and_then(|pos| cell_at(self, pos))
                {
                    self.apply_tile_paint_cell(x, y, false);
                    self.commit_tilemap_paint(context.entity);
                    self.scene_state.tile_paint_last_cell = None;
                }
                if response.clicked_by(egui::PointerButton::Secondary)
                    && let Some((x, y)) = pointer.and_then(|pos| cell_at(self, pos))
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
                        && let Some((x, y)) = pointer.and_then(|pos| cell_at(self, pos))
                        && self.scene_state.tile_paint_last_cell != Some((x, y, erase))
                    {
                        // E11：每个新格 = 一次笔触（`Painting` 相位下的 `TileStroke`）。
                        self.interaction.handle(InteractionEvent::TileStroke {
                            cell: [x as i32, y as i32],
                        });
                        self.apply_tile_paint_cell(x, y, erase);
                        self.scene_state.tile_paint_last_cell = Some((x, y, erase));
                    }
                }
            }
            TileEditMode::RectFill | TileEditMode::RegionCopy => {
                if response.dragged_by(egui::PointerButton::Primary)
                    && let Some(cell) = pointer.and_then(|pos| cell_at(self, pos))
                {
                    // 指针移出地图时保留最后一个有效格，避免预览跳回起点。
                    self.scene_state.tile_rect_current = Some(cell);
                }
                // 单击（未形成拖拽）等价于 1×1 选区。
                if response.clicked_by(egui::PointerButton::Primary)
                    && let Some(cell) = pointer.and_then(|pos| cell_at(self, pos))
                {
                    self.scene_state.tile_rect_anchor = Some(cell);
                    self.scene_state.tile_rect_current = Some(cell);
                }
                if response.drag_stopped_by(egui::PointerButton::Primary)
                    || response.clicked_by(egui::PointerButton::Primary)
                {
                    let copy_only = mode == TileEditMode::RegionCopy;
                    self.commit_tilemap_rect(context.entity, copy_only);
                }
            }
            TileEditMode::RegionPaste => {
                if response.clicked_by(egui::PointerButton::Primary)
                    && let Some(cell) = pointer.and_then(|pos| cell_at(self, pos))
                {
                    self.paste_tilemap_region(context.entity, cell);
                }
            }
        }
        // 单击（无拖拽）提交后也要清掉待选矩形，否则预览会一直留在画面上。
        if response.clicked_by(egui::PointerButton::Primary) {
            self.scene_state.tile_rect_anchor = None;
            self.scene_state.tile_rect_current = None;
        }
        if response.drag_stopped_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Secondary)
        {
            if mode == TileEditMode::Point && self.scene_state.tile_paint_active {
                self.commit_tilemap_paint(context.entity);
            }
            self.scene_state.tile_paint_active = false;
            self.scene_state.tile_paint_last_cell = None;
            self.scene_state.tile_rect_anchor = None;
            self.scene_state.tile_rect_current = None;
        }
    }

    /// 指针 → 当前 tilemap 格（越界/旋转返回 `None`）。
    pub(crate) fn tilemap_cell_at(
        &self,
        rect: egui::Rect,
        pixels_per_point: f32,
        pointer: egui::Pos2,
        context: &TilemapPaintContext,
    ) -> Option<(usize, usize)> {
        tilemap_cell_from_pointer(
            rect,
            self.scene_state.pan,
            self.scene_zoom,
            pixels_per_point,
            pointer,
            context.transform,
            &self.tilemap.map,
        )
    }

    /// M19-03：矩形填充或区域复制（一次操作 = 单条 Undo / 只更新剪贴板）。
    pub(crate) fn commit_tilemap_rect(&mut self, entity_id: SceneEntityId, copy_only: bool) {
        let (Some(anchor), Some(current)) = (
            self.scene_state.tile_rect_anchor,
            self.scene_state.tile_rect_current,
        ) else {
            return;
        };
        let rect = CellRect::normalize(anchor, current);
        if copy_only {
            match copy_region(&self.tilemap.map, rect) {
                Some(region) => {
                    self.status = format!(
                        "已复制区域 {}（{} 格）到 Tile 剪贴板",
                        region.label(),
                        rect.cell_count()
                    );
                    self.tilemap.clipboard = Some(region);
                }
                None => self.status = "区域完全越界，未复制".to_owned(),
            }
            return;
        }
        // 与单格画笔一致的写入语义：勾选「画碰撞层」时写 solid，否则写 tile 值。
        let fill = if self.tilemap.paint_collision {
            TileFill {
                value: None,
                solid: Some(true),
            }
        } else {
            TileFill {
                value: Some(clamp_brush(&self.tilemap.map, self.tilemap.brush)),
                solid: None,
            }
        };
        if !fill_rect(&mut self.tilemap.map, rect, fill) {
            self.status = format!("矩形填充无变化（{} 格）", rect.cell_count());
            return;
        }
        self.commit_tilemap_paint_labelled(
            entity_id,
            &format!(
                "Tilemap 矩形填充 {}（{}×{}）",
                if self.tilemap.paint_collision {
                    "solid"
                } else {
                    "tile"
                },
                rect.width(),
                rect.height()
            ),
        );
    }

    /// M19-03：把剪贴板区域粘贴到指定格（越界裁剪），单条 Undo。
    pub(crate) fn paste_tilemap_region(&mut self, entity_id: SceneEntityId, cell: (usize, usize)) {
        let Some(region) = self.tilemap.clipboard.clone() else {
            self.status = "瓦片剪贴板为空：先用「区域复制」框选".to_owned();
            return;
        };
        let changed = paste_region(
            &mut self.tilemap.map,
            &region,
            cell.0 as isize,
            cell.1 as isize,
        );
        if !changed {
            self.status = format!(
                "粘贴 {} 到 ({}, {}) 无变化（越界或内容相同）",
                region.label(),
                cell.0,
                cell.1
            );
            return;
        }
        self.status = format!("粘贴 {} 到 ({}, {})", region.label(), cell.0, cell.1);
        self.commit_tilemap_paint_labelled(entity_id, &format!("瓦片地图粘贴 {}", region.label()));
    }

    pub(crate) fn apply_tile_paint_cell(&mut self, x: usize, y: usize, erase: bool) {
        let index = y * self.tilemap.map.width + x;
        if self.tilemap.paint_collision {
            self.tilemap.map.solid[index] = !erase;
        } else {
            self.tilemap.map.tiles[index] = if erase {
                0
            } else {
                clamp_brush(&self.tilemap.map, self.tilemap.brush)
            };
        }
    }

    pub(crate) fn commit_tilemap_paint(&mut self, entity_id: SceneEntityId) {
        self.commit_tilemap_paint_labelled(entity_id, "瓦片绘制");
    }

    // `commit_tilemap_paint_labelled` **已搬到 shell**（`crate::scene_ops`，第 29 轮）：
    // Tile 绘制落盘是文档写入，提交边界不在面板里。

    // 拖拽/框选/Gizmo 的**提交层**（预览 → 事务 → 一条 Undo → 保存）已搬到
    // `crate::scene_drag`：
    //
    // - `apply_drag_preview_to_runtime`：拖动中只推进运行时 ECS；
    // - `apply_drag_preview_to_document` / `finish_scene_drag`：松手时落文档并提交事务；
    // - `commit_gizmo_drag3d`：3D Gizmo 的世界 → 局部反解后写回；
    // - `scene_drag_locals`：世界变换 → 文档局部值的换算。
    //
    // 这样面板文件里不再出现 `history.execute` / `current_scene.as_mut()`，
    // "View 只发 action"由 `panels/architecture_tests.rs` 的棘轮守住。
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct TilemapPaintContext {
    pub(crate) entity: SceneEntityId,
    pub(crate) transform: TransformFields,
}

pub(crate) fn tilemap_cell_from_pointer(
    rect: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    ppp: f32,
    pointer: egui::Pos2,
    transform: TransformFields,
    tilemap: &Tilemap,
) -> Option<(usize, usize)> {
    if transform.rotation.abs() > 0.001 || zoom <= 0.0 {
        return None;
    }
    let world = screen_to_world_2d(rect, pan, zoom, ppp, pointer);
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

#[allow(clippy::too_many_arguments)] // tilemap cell rect math keeps its params flat
pub(crate) fn tilemap_cell_screen_rect(
    rect: egui::Rect,
    pan: egui::Vec2,
    zoom: f32,
    ppp: f32,
    transform: TransformFields,
    tilemap: &Tilemap,
    x: usize,
    y: usize,
) -> egui::Rect {
    // 格子的世界 AABB 两角走统一映射，再取外接矩形（世界 y 向上、面板 y 向下，所以
    // 两个角映射后顺序会翻转，用 min/max 归一）。
    let cell_w = tilemap.tile_size.x * transform.scale_x.abs();
    let cell_h = tilemap.tile_size.y * transform.scale_y.abs();
    let min_world = Vec2::new(
        transform.position_x + x as f32 * cell_w,
        transform.position_y + y as f32 * cell_h,
    );
    let max_world = Vec2::new(min_world.x + cell_w, min_world.y + cell_h);
    let a = scene_view_world_to_panel(rect, pan, zoom, ppp, min_world);
    let b = scene_view_world_to_panel(rect, pan, zoom, ppp, max_world);
    egui::Rect::from_min_max(
        egui::pos2(a.x.min(b.x), a.y.min(b.y)),
        egui::pos2(a.x.max(b.x), a.y.max(b.y)),
    )
}
