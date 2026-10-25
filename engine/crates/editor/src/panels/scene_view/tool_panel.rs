//! 场景工具面板：网格/吸附/视图开关，以及碰撞体叠加显示。

use super::*;

impl EditorApp {
    /// Scene 视图右上角的小工具面板：选择/移动/旋转/缩放（+2D 的 Tile 画笔）。
    ///
    /// E11：面板只**收集**用户意图，最后一次性交给状态机（`set_scene_tool`），
    /// 因此这里不再有 5 处 `self.scene_tool = …` 的散落赋值。
    ///
    /// docs/55 阶段 2：按钮集合与 tooltip 由当前视图模式的**能力表**
    /// （`crate::scene_tools::ToolMode`）给出——3D/Widget 模式下不再显示一个"点了没反应"的
    /// Tile 画笔，Move 的提示在 Widget 模式里也说明了它改的是锚点/偏移。
    pub(crate) fn draw_scene_tool_panel(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let width = 430.0;
        // 画布矩形在窄叶子下可能溢出本标签的裁剪框（见 `dock::show` 里钉宽的那段）：
        // 工具条若贴着 `rect.right()` 摆放，右侧的 缩放/▦ 会落到裁剪框外、被属性面板盖住。
        // 贴右边界改按裁剪框取，保证整条工具条始终落在可见区内。
        let right_edge = rect.right().min(ui.clip_rect().right());
        let tools_rect = egui::Rect::from_min_size(
            egui::pos2(right_edge - width - 4.0, rect.top() + 4.0),
            egui::vec2(width, 26.0),
        );
        // 本帧工具条的屏幕矩形：性能面板的默认位置与它**不得相交**（用户报的重叠就是这两块
        // 文字/控件叠在同一角落）。生产路径每帧一次 `Rect` 写入，与 `scene_canvas_rect` 同源。
        self.scene_tools_rect = tools_rect;
        let tool = self.scene_tool();
        let mode = self.tool_mode();
        let mut requested: Option<SceneTool> = None;
        let tools_content_width = ui
            .scope_builder(egui::UiBuilder::new().max_rect(tools_rect), |ui| {
                // right_to_left：先画的 widget 贴右边缘，保证工具条紧贴视图右边界。
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 世界/轴心不在这里——它们在左上角状态胶囊那一行（见下方）。
                    // 工具按钮：rev() 后视觉顺序仍是 选择|移动|旋转|缩放|▦（从左到右）
                    for candidate in mode.tools().iter().rev() {
                        let label = match candidate {
                            SceneTool::TilePaint => "▦",
                            other => match other {
                                SceneTool::Select => "选择",
                                SceneTool::Move => "移动",
                                SceneTool::Rotate => "旋转",
                                SceneTool::Scale => "缩放",
                                SceneTool::TilePaint => "▦",
                            },
                        };
                        if icon_toggle(ui, tool == *candidate, label, mode.tool_hint(*candidate))
                            .clicked()
                        {
                            requested = Some(*candidate);
                        }
                    }
                });
                ui.min_rect().width()
            })
            .inner;
        if let Some(tool) = requested {
            self.set_scene_tool(tool);
        }
        let tools_row_width = tools_content_width;
        // RustLua-②c：左上角状态胶囊——2D 下回显视图/吸附/像素网格的**显式**状态，
        // 外加 Gizmo 空间/轴心切换（原先贴在右上角工具条尾部，挪到这里与状态同侧）。
        let gizmo_toggle_visible = matches!(
            self.scene_tool(),
            SceneTool::Move | SceneTool::Rotate | SceneTool::Scale | SceneTool::Select
        );
        let is_2d = self.scene_view_mode == crate::SceneViewMode::View2D;
        if is_2d || gizmo_toggle_visible {
            // 两条带子一个贴左一个贴右、宽度按内容走：画布窄到装不下两者时，
            // 「轴心」会和「选择」叠成乱码（1024 窗实测）。装不下就把状态行下移一行
            // （与 TilePaint 第二行的 +32 同构）。工具条宽度本帧量得到，状态行宽度用
            // 上一帧量到的值（首帧按 0 算、走单行，下一帧收敛）。
            let needed = 4.0 + self.scene_status_row_width + 8.0 + tools_row_width + 4.0;
            let status_top = if rect.width() < needed {
                rect.top() + 32.0
            } else {
                rect.top() + 4.0
            };
            let status_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 4.0, status_top),
                egui::vec2(380.0, 26.0),
            );
            let status_content_width = ui
                .scope_builder(egui::UiBuilder::new().max_rect(status_rect), |ui| {
                    // `horizontal_centered` 而不是 `horizontal`：后者只按 `interact_size.y`（18）
                    // 起行，18 高的标签会贴在这条 26 高带子的顶上，比右侧真正居中的 24 高
                    // 图标按钮高 4px。前者按整条带子居中，两组才在同一条水平线上。
                    ui.horizontal_centered(|ui| {
                        if is_2d {
                            ui.label(egui::RichText::new("2D").strong());
                            ui.separator();
                            if self.snap_enabled {
                                // 位移步长与**角度**步长各报各的（M27-02）：一个 0.5 不代表旋转也吸附。
                                ui.label(format!(
                                    "吸附 {} · {:.0}°",
                                    self.snap_step, self.snap_angle_step
                                ));
                            } else {
                                ui.label("无吸附");
                            }
                            ui.separator();
                            if self.show_pixel_grid {
                                ui.label("像素网格");
                            } else {
                                ui.label("无网格");
                            }
                            if gizmo_toggle_visible {
                                ui.separator();
                            }
                        }
                        if gizmo_toggle_visible {
                            if ui
                                .selectable_label(
                                    self.gizmo_space == crate::scene_interaction::GizmoSpace::Local,
                                    self.gizmo_space.label(),
                                )
                                .on_hover_text("变换控件空间：世界轴 / 实体局部轴（点击切换）")
                                .clicked()
                            {
                                self.gizmo_space = self.gizmo_space.toggled();
                            }
                            if ui
                                .selectable_label(
                                    self.gizmo_pivot
                                        == crate::scene_interaction::GizmoPivot::Center,
                                    self.gizmo_pivot.label(),
                                )
                                .on_hover_text(
                                    "变换控件轴心：实体原点 / 选中内容包围盒中心（点击切换）",
                                )
                                .clicked()
                            {
                                self.gizmo_pivot = self.gizmo_pivot.toggled();
                            }
                        }
                        // P3-R03-4：3D 状态行尾挂 IBL 档位（计划 §6.4「场景视图显示当前 tier」）。
                        if !is_2d {
                            let label = self
                                .scene_view_ibl_label()
                                .unwrap_or_else(|| "IBL 未就绪".to_owned());
                            ui.separator();
                            ui.label(egui::RichText::new(label).size(11.0)).on_hover_text(
                                "环境 IBL 的实际档位：Tier A=浮点 cubemap+线性过滤，\
                                 B=浮点无线性过滤，C=只有 SH9 漫反射；模式是策略×环境×能力解析的结果",
                            );
                        }
                    });
                    ui.min_rect().width()
                })
                .inner;
            self.scene_status_row_width = status_content_width;
            self.scene_overlay_bottom_offset = status_top - rect.top() + 26.0;
        } else {
            self.scene_overlay_bottom_offset = 0.0;
        }
        // M19-03 Tile 生产工具第二行：编辑模式 + 区域剪贴板状态（仅 TilePaint 工具下显示）。
        if self.scene_tool() != SceneTool::TilePaint {
            return;
        }
        let tile_tools_rect = egui::Rect::from_min_size(
            egui::pos2(right_edge - width - 4.0, rect.top() + 32.0),
            egui::vec2(width, 26.0),
        );
        self.scene_overlay_bottom_offset = self.scene_overlay_bottom_offset.max(58.0);
        ui.scope_builder(egui::UiBuilder::new().max_rect(tile_tools_rect), |ui| {
            ui.horizontal(|ui| {
                for mode in [
                    TileEditMode::Point,
                    TileEditMode::RectFill,
                    TileEditMode::RegionCopy,
                    TileEditMode::RegionPaste,
                ] {
                    if icon_toggle(ui, self.tilemap.mode == mode, mode.label(), mode.label())
                        .clicked()
                    {
                        self.tilemap.mode = mode;
                        self.scene_state.tile_rect_anchor = None;
                        self.scene_state.tile_rect_current = None;
                    }
                }
                let clipboard = self.tilemap.clipboard.as_ref().map_or_else(
                    || "剪贴板：空".to_owned(),
                    |region| format!("剪贴板：{}", region.label()),
                );
                ui.label(egui::RichText::new(clipboard).size(11.0));
                let has_clipboard = self.tilemap.clipboard.is_some();
                if ui
                    .add_enabled(has_clipboard, egui::Button::new("×"))
                    .on_hover_text("清空瓦片剪贴板")
                    .clicked()
                {
                    self.tilemap.clipboard = None;
                }
            });
        });
    }

    /// 调试覆盖层：把当前场景里**所有**碰撞区域画在 2D Scene 视图上。
    ///
    /// 与只画选中图、且要开 `preview_collision` 的 [`Self::draw_tilemap_overlay`] 不同，
    /// 这里遍历整张场景、与选中无关，一次看清"哪里会挡人"：
    /// ① 每个 `Tilemap` 实体的 solid 格（红色实心）；
    /// ② 每个带 `Collider` 组件实体的 2D 碰撞体轮廓（sensor 蓝、普通橙）。
    /// 碰撞体轮廓按实体姿态（平移 + 旋转）变换到世界，与运行时 debug_draw 同口径
    /// （不叠加 scale——和 `ColliderDebugShape::wireframe_segments` 一致）。
    pub(crate) fn draw_collision_overlay(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        if !self.show_collision_overlay {
            return;
        }
        let Some(scene) = self.current_scene() else {
            return;
        };
        let pan = self.scene_state.pan;
        let zoom = self.scene_zoom;
        let ppp = ppp.max(1.0e-3);

        let solid_fill = egui::Color32::from_rgba_unmultiplied(220, 70, 70, 70);
        let solid_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 110, 110));

        for entity in scene.scene.entities.values() {
            let transform = self
                .scene_state
                .preview
                .get(&entity.id)
                .copied()
                .or_else(|| {
                    entity
                        .components
                        .get("Transform")
                        .and_then(|value| parse_transform(value))
                })
                .unwrap_or_default();

            // ① Tilemap 的 solid 层（地图碰撞格）。旋转的 Tilemap 暂不画（与 TilePaint 同限制）。
            if let Some(raw) = entity.components.get("Tilemap")
                && let Ok(map) = Tilemap::from_component_str(raw)
                && transform.rotation.abs() <= 0.001
            {
                for y in 0..map.height {
                    for x in 0..map.width {
                        if !map.solid[y * map.width + x] {
                            continue;
                        }
                        let cell =
                            tilemap_cell_screen_rect(rect, pan, zoom, ppp, transform, &map, x, y);
                        painter.rect_filled(cell, 0.0, solid_fill);
                        painter.rect_stroke(cell, 0.0, solid_stroke, egui::StrokeKind::Inside);
                    }
                }
            }

            // ② 实体 Collider（2D 物理碰撞体轮廓）。
            if let Some(raw) = entity.components.get("Collider")
                && let Ok(desc) = ColliderDesc::from_component_str(raw)
            {
                let (sin, cos) = transform.rotation.to_radians().sin_cos();
                // 世界点 = 实体平移 + 旋转 ·（碰撞体 offset + 局部轮廓点）。只取 f32 分量重建
                // 本地 `Vec2`，避免 physics 与 engine 两处 `Vec2` 别名在签名上打架。
                let to_world = |lx: f32, ly: f32| -> Vec2 {
                    let ox = desc.offset.x + lx;
                    let oy = desc.offset.y + ly;
                    Vec2::new(
                        transform.position_x + ox * cos - oy * sin,
                        transform.position_y + ox * sin + oy * cos,
                    )
                };
                let stroke = egui::Stroke::new(
                    1.5_f32,
                    if desc.sensor {
                        egui::Color32::from_rgb(120, 200, 240)
                    } else {
                        egui::Color32::from_rgb(240, 170, 80)
                    },
                );
                for (a, b) in desc.shape.wireframe_edges() {
                    let pa = scene_view_world_to_panel(rect, pan, zoom, ppp, to_world(a.x, a.y));
                    let pb = scene_view_world_to_panel(rect, pan, zoom, ppp, to_world(b.x, b.y));
                    painter.line_segment([pa, pb], stroke);
                }
            }
        }
    }
}
