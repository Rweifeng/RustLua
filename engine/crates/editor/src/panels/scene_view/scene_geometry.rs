//! 2D 命中与包围盒：实体/UI 控件的屏幕矩形、框选、拾取、gizmo 轴与拖动换算。
//! 纯几何计算，不读面板状态，便于单独回归（`tests` 里有手推映射对照）。

use super::*;

impl EditorApp {
    /// 某实体在 2D Scene 画布上的**包围框中心与尺寸**（面板逻辑坐标 / 点）。
    ///
    /// docs/55 阶段 0：GUI smoke 的合成指针事件必须打在实体真正被绘制的位置上；
    /// 这里复用面板自身的坐标换算（`collect_scene_2d_entities` + 本帧画布矩形），
    /// 因此 smoke 不会自己写一套会漂移的投影。画布本帧没画（`Rect::NOTHING`）时返回 `None`，
    /// 让调用方显式失败而不是拿一个假坐标去打事件。
    pub(crate) fn scene_2d_entity_box(
        &mut self,
        id: SceneEntityId,
        pixels_per_point: f32,
    ) -> Option<(egui::Pos2, egui::Vec2)> {
        let rect = self.scene_canvas_rect;
        if !(rect.width() > 1.0 && rect.height() > 1.0) {
            return None;
        }
        let filter = crate::panels::hierarchy::HierarchyFilter::default();
        self.collect_scene_2d_entities(rect, &filter, pixels_per_point)
            .into_iter()
            .find(|entity| entity.id == id)
            .map(|entity| (entity.center, entity.size))
    }

    /// 收集 2D 场景视图要绘制的实体包围框（字段级借用，避免与 &mut scene_runtime
    /// 冲突）。UI 组件实体用组件自身的屏幕空间坐标解析，其余实体按 Transform 世界
    /// 坐标 + pan/zoom 映射。
    pub(crate) fn collect_scene_2d_entities(
        &mut self,
        rect: egui::Rect,
        filter: &crate::panels::hierarchy::HierarchyFilter,
        pixels_per_point: f32,
    ) -> Vec<SceneBoxedEntity> {
        self.ensure_scene_runtime();
        let scene = panel_scene_document(&self.play_session, &self.document);
        let scene_state = &self.scene_state;
        let scene_zoom = self.scene_zoom;
        let pan = scene_state.pan;
        // 游戏相机中心（设计分辨率包围框世界中心），在借用前读取为局部值。
        let camera_center = egui::vec2(self.game_camera_2d_center.x, self.game_camera_2d_center.y);
        let Some(scene) = scene else {
            return Vec::new();
        };
        let canvas_size = [
            self.preview_resolution.0 as f32,
            self.preview_resolution.1 as f32,
        ];
        let nodes = scene_ui_nodes(&scene.scene);
        let (mut resolved_ui_layouts, _) = resolve_scene_ui_layout(&nodes, canvas_size);
        // Keep only explicit Anchor+Offset layouts and container placements;
        // the resolver's default origin rects must not shift the UI.
        resolved_ui_layouts.retain(|id, resolved| {
            resolved.container_controlled || nodes.get(id).is_some_and(|node| node.layout.is_some())
        });
        // The game-screen overlay resolves layouts inside RuntimeScene. Reuse
        // that exact result so selection boxes cannot diverge from rendered UI
        // when hierarchy/key mappings or in-flight previews differ.
        let runtime_ui_layouts = self
            .scene_runtime
            .as_mut()
            .map(|runtime| {
                runtime.scene_entity_ui_layout_rects(
                    nodes.keys().map(|id| SceneEntityId(*id)),
                    canvas_size,
                )
            })
            .unwrap_or_default();
        // Scene View boxes and hit tests must follow the in-flight UI drag even
        // while the authoritative document still holds the drag-start layout.
        for (id, offsets) in &scene_state.preview_ui {
            if let Some(resolved) = resolved_ui_layouts.get_mut(&id.0) {
                resolved.rect = UiRect::new(
                    offsets[0],
                    offsets[1],
                    (offsets[2] - offsets[0]).max(0.0),
                    (offsets[3] - offsets[1]).max(0.0),
                );
            }
        }
        let mut entities = Vec::with_capacity(scene.scene.entities.len());
        for e in scene.scene.entities.values() {
            if !filter.is_empty() && !filter.matches_name(&e.name.to_lowercase()) {
                continue;
            }
            // H11：父实体隐藏时整棵子树一并隐藏；子实体自身标记不变。
            let mut hidden_by_ancestor = false;
            let mut visibility_cursor = Some(e.id);
            for _ in 0..=scene.scene.entities.len() {
                let Some(cursor_id) = visibility_cursor else {
                    break;
                };
                if scene_state.hidden.contains(&cursor_id) {
                    hidden_by_ancestor = true;
                    break;
                }
                let Some(cursor_entity) = scene.scene.entities.get(&cursor_id) else {
                    break;
                };
                visibility_cursor = cursor_entity.parent;
            }
            if hidden_by_ancestor {
                continue;
            }
            let transform = scene_state
                .preview
                .get(&e.id)
                .copied()
                .or_else(|| {
                    e.components
                        .get("Transform")
                        .and_then(|v| parse_transform(v))
                })
                .unwrap_or_default();
            // WP2：包围框必须跟**渲染**用同一份位置。场景视图渲染走 `RuntimeScene::vertices()`
            // （父链组合后的世界变换），而文档里存的是局部值——带父子关系的实体若继续用局部值，
            // 会出现"框在人不在的地方"。继承未生效（没有父子关系）时这里返回的就是局部值，
            // 因此两条路径的结果一致；拖动预览（`scene_state.preview`）优先，保持所见即所得。
            let transform = if scene_state.preview.contains_key(&e.id) {
                transform
            } else {
                self.scene_runtime
                    .as_ref()
                    .and_then(|runtime| runtime.world_transform_2d_of(e.id))
                    .map(|world| TransformFields {
                        position_x: world.position.x,
                        position_y: world.position.y,
                        rotation: world.rotation.to_degrees(),
                        scale_x: world.scale.x,
                        scale_y: world.scale.y,
                    })
                    .unwrap_or(transform)
            };
            let measured_text_bounds = e
                .components
                .get("Text")
                .and_then(|raw| rustlua_ui::ui::UiText::from_component_str(raw).ok())
                .map(|text| {
                    if let Some(runtime) = self.scene_runtime.as_mut() {
                        runtime.measure_scene_ui_text(&text).1
                    } else {
                        measure_text_without_runtime(&text.text, text.effective_scale()).1
                    }
                });
            // UI 组件在设计分辨率包围框内渲染，包围框按 UiLayout 四边
            // Anchor+Offset 解析后映射进包围框（随 pan/zoom）；
            // 其余实体（Sprite/图块）按 Transform 世界坐标 + pan/zoom 映射。空实体
            // 回退默认框。
            if let Some((center, size, ui)) = ui_entity_2d_box(
                e,
                rect,
                runtime_ui_layouts.get(&e.id.0).copied().or_else(|| {
                    resolved_ui_layouts
                        .get(&e.id.0)
                        .map(|resolved| resolved.rect)
                }),
                measured_text_bounds,
                scene_state.pan,
                scene_zoom,
                camera_center,
                self.preview_resolution,
                pixels_per_point,
            ) {
                let effective = TransformFields {
                    rotation: 0.0,
                    ..transform
                };
                entities.push(SceneBoxedEntity {
                    id: e.id,
                    name: e.name.clone(),
                    transform: effective,
                    center,
                    size,
                    ui: Some(ui),
                });
            } else {
                // 运行期生成的 UI 行没有可解析的版面：镜像只回灌结构与姿态，`UiLayout` 的
                // anchors/offsets 与 `Text`/`Image` 的贴合都不在其中，按姿态画出来的框会把
                // 整屏 UI 全叠在世界原点——那是**假框**（18 个节点叠成一坨），比不画更误导。
                // 这一行的真值看设计分辨率框里那张游戏画面（Play 期间就是 Game 视图那一帧），
                // 行本身照样在层级里可选、属性面板里可读姿态。计数不为零即"这里确实少画了"。
                if e.metadata
                    .contains_key(editor::runtime_sync::RUNTIME_UI_META)
                {
                    self.profiler
                        .bump("scene_2d_runtime_ui_rows_without_payload");
                    continue;
                }
                let (center, size) =
                    entity_2d_screen_box(e, rect, pan, scene_zoom, transform, pixels_per_point);
                entities.push(SceneBoxedEntity {
                    id: e.id,
                    name: e.name.clone(),
                    transform,
                    center,
                    size,
                    ui: None,
                });
            }
        }
        entities
    }
}
/// 2D 场景视图里一个实体的包围框记录（选中轮廓 / 命中 / 框选 / 拖拽共用）。
pub(crate) struct SceneBoxedEntity {
    pub(crate) id: SceneEntityId,
    pub(crate) name: String,
    /// 生效变换：UI 实体旋转恒为 0（组件在屏幕空间，不受 Transform 旋转影响）。
    pub(crate) transform: TransformFields,
    /// 包围框中心（面板逻辑坐标）。
    pub(crate) center: egui::Pos2,
    pub(crate) size: egui::Vec2,
    /// 该实体是否为 UI 组件实体（Image/Button/Text），附组件改写信息。
    pub(crate) ui: Option<UiEntityInfo>,
}

/// 拖动**起点**：优先用「按下时指针所在处」，而不是本帧的指针位置。
///
/// `egui` 的 `drag_started` 是在指针已经移动超过点击阈值（默认 6 点）那一帧才置位的，
/// 而 `interact_pointer_pos()` 给的是**移动之后**的位置。小尺寸精灵（设计分辨率缩放到
/// 画布后可能只有 11 点宽）在"按在中心、拖 12 点"时，用本帧位置做命中测试会落在包围框
/// 之外——表现为"抓着精灵拖，结果变成了框选"。
///
/// 这条不是理论问题：docs/55 阶段 0 的 GUI smoke（`crate::framegate`）用真实指针事件复现
/// 了它（`scene_view/drag_commits_exactly_one_undo` 一开始报 `phase=Marquee` 而不是
/// `Dragging`）。商业引擎的语义也是"抓住按下时手里的东西"，所以这里改用 `press_origin`。
pub(crate) fn drag_start_pos(ui: &egui::Ui, current: Option<egui::Pos2>) -> Option<egui::Pos2> {
    ui.input(|input| input.pointer.press_origin()).or(current)
}

pub(crate) fn pick_scene_2d_entity(
    entities: &[SceneBoxedEntity],
    position: egui::Pos2,
    locked: &std::collections::BTreeSet<SceneEntityId>,
) -> Option<SceneEntityId> {
    entities.iter().rev().find_map(|entity| {
        (rotated_contains(
            entity.center,
            entity.size,
            entity.transform.rotation,
            position,
        ) && !locked.contains(&entity.id))
        .then_some(entity.id)
    })
}

pub(crate) fn entities_in_selection_rect(
    entities: &[SceneBoxedEntity],
    selection_rect: egui::Rect,
    start: egui::Pos2,
    current: egui::Pos2,
) -> Vec<SceneEntityId> {
    let left_to_right = current.x >= start.x;
    entities
        .iter()
        .filter_map(|entity| {
            let corners = rotated_corners(entity.center, entity.size, entity.transform.rotation);
            let contained = corners
                .iter()
                .all(|corner| selection_rect.contains(*corner));
            let intersects = polygon_intersects_rect(&corners, selection_rect);
            (left_to_right && contained || !left_to_right && intersects).then_some(entity.id)
        })
        .collect()
}

/// 旋转的**世界空间增量**：世界模式绕世界 Y；局部模式绕实体**自己的 Y**
/// （两者都能写成"世界轴增量左乘"，所以轨道运算只有一份实现）。
/// 2D 拖拽的**轴心**：`Pivot` = 主选中实体自身位置；`Center` = 拖动集合的位置平均
/// （多选时这才是"绕整体中心转"）。
///
/// 返回的 `(x, y)` 会喂给 `scene_drag_transform` 的 Rotate/Scale 分支。
pub(crate) fn scene_drag_pivot(
    pivot: crate::scene_interaction::GizmoPivot,
    primary: Option<SceneEntityId>,
    initial: &std::collections::BTreeMap<SceneEntityId, TransformFields>,
) -> (f32, f32) {
    if pivot == crate::scene_interaction::GizmoPivot::Pivot
        && let Some(position) = primary.and_then(|id| initial.get(&id))
    {
        return (position.position_x, position.position_y);
    }
    let count = initial.len().max(1) as f32;
    let x = initial.values().map(|value| value.position_x).sum::<f32>() / count;
    let y = initial.values().map(|value| value.position_y).sum::<f32>() / count;
    (x, y)
}

/// Gizmo 轴在**世界空间**的方向（第 43 轮：世界/局部两种变换空间）。
///
/// 局部模式下轴跟随实体的旋转；四元数非法（非单位/非有限）时退回世界轴——
/// 一个坏掉的旋转不应该让手柄整根消失、连修都修不了。
pub(crate) fn gizmo_axis_world(
    space: crate::scene_interaction::GizmoSpace,
    rotation: Quat,
    axis: GizmoAxis,
) -> Option<Vec3> {
    let unit = match axis {
        GizmoAxis::X => Vec3::X,
        GizmoAxis::Y => Vec3::Y,
        GizmoAxis::Z => Vec3::Z,
        // Free / Scale 不是轴。
        _ => return None,
    };
    match space {
        crate::scene_interaction::GizmoSpace::World => Some(unit),
        crate::scene_interaction::GizmoSpace::Local => Some(rotation.rotate(unit).unwrap_or(unit)),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn scene_drag_transform(
    initial: TransformFields,
    pivot: (f32, f32),
    delta: egui::Vec2,
    tool: SceneTool,
    snap_enabled: bool,
    snap_step: f32,
    pixels_per_point: f32,
    zoom: f32,
    angle: f32,
    factor: f32,
) -> TransformFields {
    let mut next = initial;
    match tool {
        SceneTool::Move | SceneTool::Select => {
            next.position_x += delta.x * pixels_per_point / zoom;
            next.position_y -= delta.y * pixels_per_point / zoom;
            if snap_enabled {
                next.position_x = (next.position_x / snap_step).round() * snap_step;
                next.position_y = (next.position_y / snap_step).round() * snap_step;
            }
        }
        SceneTool::Rotate => {
            let relative_x = initial.position_x - pivot.0;
            let relative_y = initial.position_y - pivot.1;
            next.position_x = pivot.0 + relative_x * angle.cos() - relative_y * angle.sin();
            next.position_y = pivot.1 + relative_x * angle.sin() + relative_y * angle.cos();
            // 写入的角度必须是**轨道用的那一个**（`angle`），不是未吸附的原始增量：
            // 否则吸附时画面转到位、文档里存的却是另一个数。
            next.rotation += angle.to_degrees();
        }
        SceneTool::Scale => {
            next.position_x = pivot.0 + (initial.position_x - pivot.0) * factor;
            next.position_y = pivot.1 + (initial.position_y - pivot.1) * factor;
            next.scale_x = (initial.scale_x * factor).max(0.05);
            next.scale_y = (initial.scale_y * factor).max(0.05);
        }
        SceneTool::TilePaint => {}
    }
    next
}

pub(crate) fn pick_scene_3d_entity(
    ray: Ray3,
    entities: &[(
        SceneEntityId,
        String,
        Transform3D,
        rustlua_math::math3d::Aabb3,
    )],
    locked: &std::collections::BTreeSet<SceneEntityId>,
    preview: &std::collections::BTreeMap<SceneEntityId, Transform3D>,
) -> Option<SceneEntityId> {
    entities
        .iter()
        .filter(|(id, _, _, _)| !locked.contains(id))
        .filter_map(|(id, _, transform, bounds)| {
            let position = preview
                .get(id)
                .map(|value| value.position)
                .unwrap_or(transform.position);
            let world_bounds = rustlua_math::math3d::Aabb3 {
                min: bounds.min + position,
                max: bounds.max + position,
            };
            ray.intersect_aabb(world_bounds)
                .map(|distance| (*id, distance))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(id, _)| id)
}

/// Gizmo 三条臂在**屏幕**上的向量（长度 = `len`），由真实世界轴的投影算出来。
///
/// 为什么必须投影而不是画固定方向（第 45 轮补的缺口）：局部模式的意义就是"手柄跟着实体转"，
/// 固定方向的手柄让这个模式在画面上完全看不出来；而且相机绕转时世界轴在屏幕上的方向本来就会变。
/// 退化情况（轴几乎与视线平行 → 投影长度趋零、或投影失败）时退回一条固定的屏幕方向，
/// 否则那根手柄会缩成一个点、抓不到也看不见。
pub(crate) fn gizmo_screen_axes(
    position: Vec3,
    screen: egui::Pos2,
    len: f32,
    space: crate::scene_interaction::GizmoSpace,
    rotation: Quat,
    project: impl Fn(Vec3) -> Option<egui::Pos2>,
) -> [egui::Vec2; 3] {
    // 默认方向：X 向右、Y 向上、Z 斜向左上（与旧版的观感一致，只在投影不可用时兜底）。
    let mut axes = [
        egui::vec2(len, 0.0),
        egui::vec2(0.0, -len),
        egui::vec2(-len * 0.7, -len * 0.7),
    ];
    for (index, axis) in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z]
        .into_iter()
        .enumerate()
    {
        let Some(world) = gizmo_axis_world(space, rotation, axis) else {
            continue;
        };
        let Some(point) = project(position + world) else {
            continue;
        };
        let delta = point - screen;
        if delta.length() >= 1.0 {
            axes[index] = delta.normalized() * len;
        }
    }
    axes
}

/// UI 组件实体的拖拽改写信息。
#[derive(Clone)]
pub(crate) struct UiEntityInfo {
    pub(crate) layout: UiRectLayout,
}

/// 计算 UI 组件实体（Image/Button/Text）在面板坐标系的真实包围框。
///
/// `UiLayout` 是唯一几何事实来源；`Image` / `Text` 只提供视觉属性。这里用与
/// 运行时相同的四边 Anchor+Offset 解析出设计像素矩形，再映射进 Scene 视图。
#[allow(clippy::too_many_arguments)]
pub(crate) fn ui_entity_2d_box(
    entity: &SceneEntity,
    rect: egui::Rect,
    override_layout: Option<UiRect>,
    measured_text_bounds: Option<[f32; 4]>,
    pan: egui::Vec2,
    zoom: f32,
    center: egui::Vec2,
    design_resolution: (u32, u32),
    pixels_per_point: f32,
) -> Option<(egui::Pos2, egui::Vec2, UiEntityInfo)> {
    // UI 坐标是设计分辨率像素（包围框内）；经 design_rect_to_panel 映射到面板逻辑坐标。
    let viewport = [design_resolution.0 as f32, design_resolution.1 as f32];
    // 直接返回期望尺寸（`egui::Rect::size()` 是 max-min，rect 远离原点时会有浮点
    // 抵消误差），中心沿用 `design_rect_to_panel` 的映射。
    let box_of = |origin: [f32; 2], size: [f32; 2]| {
        let frame = design_rect_to_panel(
            rect,
            pan,
            zoom,
            pixels_per_point,
            center,
            design_resolution,
            origin,
            size,
        );
        let ppp = pixels_per_point.max(1.0e-3);
        (
            frame.center(),
            egui::vec2(size[0] * zoom / ppp, size[1] * zoom / ppp),
        )
    };
    let raw_layout = entity.components.get("UiLayout")?;
    let layout = UiRectLayout::from_component_str(raw_layout).ok()?;
    if !(entity.components.contains_key("Image")
        || entity.components.contains_key("Button")
        || entity.components.contains_key("Text"))
    {
        return None;
    }
    let resolved = override_layout.unwrap_or_else(|| layout.resolve(viewport));
    let mut resolved = resolved;
    if entity.components.contains_key("Text") {
        let bounds = measured_text_bounds
            .or_else(|| {
                let raw = entity.components.get("Text")?;
                let text = rustlua_ui::ui::UiText::from_component_str(raw).ok()?;
                Some(measure_text_without_runtime(&text.text, text.effective_scale()).1)
            })
            .unwrap_or([0.0; 4]);
        // Runtime rendering uses the layout rect only as the text origin; the
        // glyph bbox supplies visible width, height and ascent offset.
        resolved.x += bounds[0];
        resolved.y += bounds[1];
        resolved.w = (bounds[2] - bounds[0]).max(0.0);
        resolved.h = (bounds[3] - bounds[1]).max(0.0);
    }
    let (center, size) = box_of([resolved.x, resolved.y], [resolved.w, resolved.h]);
    Some((center, size, UiEntityInfo { layout }))
}

pub(crate) fn scene_ui_nodes(
    scene: &rustlua_scene::scene::Scene,
) -> std::collections::BTreeMap<u64, UiSceneNode> {
    scene
        .entities
        .values()
        .map(|entity| {
            let layout = entity
                .components
                .get("UiLayout")
                .and_then(|raw| rustlua_ui::ui_layout::UiRectLayout::from_component_str(raw).ok());
            let container = entity.components.get("Container").and_then(|raw| {
                rustlua_ui::ui_layout::ContainerStyle::from_component_str(raw).ok()
            });
            let layout_item = entity
                .components
                .get("LayoutItem")
                .and_then(|raw| rustlua_ui::ui_layout::LayoutItem::from_component_str(raw).ok());
            let preferred_size = rustlua_ui::ui_scene_layout::preferred_size_for(
                &entity.components,
                layout.as_ref(),
                &mut |text, scale| {
                    rustlua_render::text::measure_builtin_text(text, scale).map(|(size, _)| size)
                },
            );
            (
                entity.id.0,
                UiSceneNode {
                    parent: entity.parent.map(|id| id.0),
                    layout,
                    container,
                    layout_item,
                    preferred_size,
                },
            )
        })
        .collect()
}

/// 无运行时时度量文本：用内置 5x7 位图指标（与运行时无字体回退一致）。
/// 返回 `(size, pixel_bounds)`，语义同 `RuntimeScene::measure_ui_text`。
pub(crate) fn measure_text_without_runtime(text: &str, scale: f32) -> ([f32; 2], [f32; 4]) {
    rustlua_render::text::measure_builtin_text(text, scale).unwrap_or(([0.0, 0.0], [0.0; 4]))
}
