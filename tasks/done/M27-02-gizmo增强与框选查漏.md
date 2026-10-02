# 任务：M27-02 Gizmo 增强与框选查漏

- 任务 ID：M27-02
- 状态：待验收
- 关联里程碑：M27（可在 M27-00 合入后并行插队；与 M27-01 无依赖）
- 创建日期：2026-10-01

## 目标

对 Scene View 可视化编辑做**查漏增强**（现状已覆盖大半：拖动、框选、多选、
Local/World、Pivot/Center 均已实现），补齐交互缺项并统一 2D/3D 手感。

## 现状基线（先核对再动手）

- `gizmo.rs:536`：gizmo 绘制与命中测试；`scene_drag.rs:480`：提交层
  （预览只改运行时 ECS，松手恰好一条 Undo，见文件头注释）。
- `selection.rs:336`：统一选择模型，`SelectionSource::SceneView2D` 已含框选；
  Shift/Ctrl 增减选需按代码核实（`selection.rs` 的 toggle 路径）。
- `layout.rs`：`gizmo_space/gizmo_pivot` 已持久化；M10-07 已做吸附。
- **注意**：`tasks/done/M10-07`、`M10-08` 已交付过一轮 gizmo/多选，本任务开工前
  先按其验收清单对照现码，剔掉已存在项，避免重复建设。

## 影响文件

- `gizmo.rs`、`scene_tools.rs`、`panels/scene_view.rs`（2D 与 3D 两条绘制路径）
- `selection.rs`（仅当缺少框选"相交 vs 包含"切换时）
- `scene_drag.rs`（提交层语义不变，只扩 action 种类）
- `layout.rs`（新增交互开关的持久化键，v2 顺带）

## 缺项清单（逐项先验证"是否真的缺"，已存在则划掉并记录证据）

1. **3D：rotate 三环独立命中 + 屏幕空间圆环拾取 —— 本轮新增**。
   原码只有一支绕 Y 的环：`pick_gizmo_axis` 的 Rotate 分支对整个圆环一律返回
   `GizmoAxis::X`，`rotation_delta` 也恒用 `Vec3::Y`，于是"看到三支环、只有一支能用"。
   现在 X/Y/Z 各按**真实三维圆采样再投影**（`ring_radius_world` + `ring_polyline`，
   `gizmo.rs:189/210`），画的是真椭圆；命中按**到折线的最近距离**决胜负
   （`pick_rotation_ring`，`gizmo.rs:245`），角度用射线与环面的交点量
   （`ray_plane_point` → `ring_angle`），射线接近平行时退回水平增量而不是 NaN。
2. **3D：scale 立方体把手与轴向平面块 —— 本轮新增**。
   原码只有一个对角线上的等比把手。现在 `scale_handles`（`gizmo.rs:116`）给出
   三支臂端（ScaleX/Y/Z）+ 三块平面（ScaleXY/XZ/YZ），绘制与命中读同一份数据；
   等比搬到手柄**中心方框**（`GizmoAxis::Scale`）。缩放支持轴掩码
   （`scale_axis_mask`）：单轴把手只改那一轴，**位置偏移同样只按掩码缩放**，
   否则拖 X 会把实体顺着 Y/Z 一起推走。
3. **2D：旋转显示为单把手圆环 + 角度吸附 —— 本轮新增**。
   此前 2D 的旋转只是"横着拖就转"，画面上没有任何对应物，且 Rotate 分支从不看吸附开关。
   现在 `draw_scene_2d_rotation_ring` 画的圆心就是这一趟真实的转轴
   （`scene_drag_pivot` 的 Pivot/Center 语义），环随轴心模式移动；
   新增 `snap_angle`（`gizmo.rs:289`，单位=度）与工具栏的 `snap_angle_step`（默认 15°），
   轨道角与写入文档的角是同一个数。
4. **框选模式切换（相交/包含切换键 + 持久化）—— 已存在（方向规则）/ 显式开关不做**。
   `entities_in_selection_rect`（`panels/scene_view.rs:3363`）已按**拖动方向**分流：
   左→右=完全包含、右→左=相交（Unity 同款），由
   `panels::scene_view::tests::box_selection_uses_directional_containment_rules` 钉住。
   再做一个是/非开关会与这条既有语义打架（同一个手势两套规则），故不做；
   3D 视图没有框选（走射线点选），不在本任务范围。
5. **框选反选 / 起手势整组拖动 —— 已存在**。
   `scene_pointer_down`（`panels/scene_view.rs:615`）在"按在已选实体上"时不清空选中集合，
   拖动 id 取自 `selection.ids()`，`SceneDrag.ids` 全量携带 → 整组一起动；
   3D 侧按设计是"点选重建选中集合"，多选整体移动经由 `GizmoDragTargets::new`
   把整个选中集合喂给一次拖拽（M10-08 已交付，本轮补了提交层的对照测试）。
6. **多选 gizmo 聚合轴心 —— 已存在，但 2D/3D 口径不同（记录为已知偏差）**。
   3D 的 `Center` = 选中集合**并集包围盒中心**（`selection_bounds_center`）；
   2D 的 `Center` = 拖动集合**位置平均**（`scene_drag_pivot`）。
   本轮**不做统一**：2D 要算并集就得逐实体解析 Sprite 的 size/pivot，
   把圆心挪到贴图上去，反而偏离"绕纹理中心转"的直觉；两边各自稳定且有测试。
7. **拖动中坐标浮标 —— 本轮新增**。
   `readout_2d` / `readout_3d`（`gizmo.rs:301/323`）给出纯文案（位置=当前值、
   旋转=转了多少度、3D 旋转还报绕的是哪根轴），2D 挂在指针旁、3D 挂在手柄旁，
   两侧同一份字号（`FontId::proportional(13.0)`）。

## 风险

- 每加一种命中形态都在增大 `scene_view.rs`（已 5000+ 行）：新增命中逻辑一律进 `gizmo.rs`，
  面板文件只做事件转发（对齐架构棘轮，见 `panels/architecture_tests.rs`）。
  **实测结果**：本轮全部纯逻辑（环几何、射线求交、角度量、吸附、轴掩码、文案）落在
  `gizmo.rs`，`scene_view.rs` 只多了两段绘制与一个 `gizmo_ring_angle` 事件转发；
  棘轮测试仍要求面板对文档零写入，未破。
- 2D/3D 双路径手感漂移：每条缺项做完补一个"同一操作 2D/3D 各一"的对照单测。

## 验收方式

- [x] 缺项清单逐条标注"已存在（证据）/ 本轮新增 / 不做（理由）"，不允许空表
- [x] 每条新增交互有对应单测（命中测试纯函数化，不依赖 GPU）
- [x] Undo 步数纪律：每种新拖动松手后恰好一条历史（`scene_drag.rs` 提交层复用，
      3D 走 `gizmo_drag3d_commits_every_selected_entity_as_one_undo`，
      2D 走 `a_2d_rotation_drag_snaps_to_the_angle_step_and_commits_once`
      + 真实帧 `scene_view/rotation_drag_commits_exactly_one_undo`）
- [x] Play 模式全部拒绝；`cargo test -p editor` 全绿

## 实现记录

**纯逻辑层（`gizmo.rs`）**

- 环：`ring_basis`（挑一条不共线的手柄轴）→ `ring_radius_world`（用**面内那条可见投影**
  量屏幕→世界尺度，两条投影都被压扁时返回 `None`，调用方跳过这一环）→
  `ring_polyline`（`GIZMO_RING_SEGMENTS=32` 采样）→ `distance_point_to_ring` /
  `pick_rotation_ring`（三支环靠最近距离决胜负，阈值 `GIZMO_RING_HIT_RADIUS`）。
- 角度：`ray_plane_point`（射线与环面求交；`|dot| < 0.02` 视为侧视、无解）→
  `ring_angle`（`atan2(offset·(u×first), offset·first)`，交点与环心重合时返回 `None`
  而不是 NaN）→ `snap_angle`（度为步长）。
- 缩放：`scale_handles` / `scale_axis_mask` / `scale_handle_direction` / `scale_factor`
  （0.01 每像素、下限 0.05）。
- 签名变化：`rotate_about` / `rotate_targets` / `rotation_delta` 多一个 `slot: Vec3`
  （绕哪根轴，世界模式直用、局部模式先按实体朝向转过去）；
  `scale_about` / `scale_axis_targets` 多一个 `mask: [bool; 3]`。
- 文案：`readout_2d` / `readout_3d` / `axis_name`。
- 测试：`rotation_center_still_grabs_and_rings_are_picked_separately`、
  `rotation_ring_picking_prefers_the_nearest_ring`、
  `ring_geometry_projects_a_real_ellipse_not_a_circle`、
  `ring_angle_is_signed_and_degrades_to_none`、
  `ray_plane_point_refuses_an_edge_on_view`、`angle_snap_rounds_to_the_step_in_radians`、
  `the_drag_readout_reports_the_number_the_user_just_changed`、
  `scale_handles_are_grabbable_and_drawn_from_the_same_numbers`、
  `the_scale_handle_moves_the_value_along_its_own_screen_direction`、
  `a_single_axis_scale_leaves_the_other_axes_alone`、
  `a_ring_rotates_about_its_own_axis_not_always_y`、
  `a_local_ring_follows_each_entitys_own_axis`。

**面板层（`panels/scene_view.rs`）**

- 3D：环与把手的绘制与命中**共用同一份函数返回值**（这是本轮的硬约束，否则又回到
  "画一整圈只有一点能抓"）；hover 高亮取最近的那支环；拖动中读 `gizmo_ring_angle`，
  吸附后同一个角度既喂轨道也写进预览；浮标画在手柄右上方。
- 2D：`draw_scene_2d_rotation_ring`（环心=真实转轴，半径包住最远的被拖实体）、
  `draw_scene_2d_drag_readout`、`design_to_panel_pos`（`panel_to_design_pos` 的逆，
  走 WP5 同一份映射，不另写 `pan + world*zoom`）。
- `scene_pointer_drag_apply`：转角与轨道共用吸附后的 `angle`。

**工具栏 / 会话状态**

- `snap_angle_step: f32`（`main.rs`，默认 15°）+ 工具栏 DragValue（1..=90°，后缀 °），
  与 `snap_enabled` 共用开关；2D 状态条同时显示"吸附 0.5 · 15°"。
- **与原计划的偏差**：`snap_angle_step` 与整族吸附状态一样**不落 layout.json**
  （`layout.rs` 历史上只持久化 `gizmo_space`/`gizmo_pivot`），会话内生效。

**顺带修掉的真实缺陷（不是本轮功能，但不修则第 3 项不可用）**

- 2D 提交层的**度/弧度**错位：`TransformFields.rotation` 是编辑器口径的**度**，
  而引擎的平面 `Transform.rotation` 与 `Quat::from_axis_angle` 一律按**弧度**。
  `geometry::encode_transform`/`parse_transform` 与 `scene_drag::scene_drag_locals`
  三个桥接点都没有换算，于是"绕 Z 拖到 45°"被存成"绕 Z 转 45 弧度"
  （四元数折回主值 ≈58.3°，读回来又显示成 1.0177），画面松手即跳。
  这个洞自 M10-07 起就在，此前没人拿真实角度去对拍，是新加的
  `a_2d_rotation_drag_snaps_to_the_angle_step_and_commits_once` 当场抓出的
  （断言"文档里存的必须就是画面转到的角度"）。
  换算现在只发生在这一对编解码函数里，由
  `geometry::tests::rotation_crosses_the_degree_radian_boundary_only_in_the_codec` 钉住。
- 真实帧证据：`--gui-smoke` 新增 3 条用例
  （`scene_view/rotation_drag_snaps_to_the_angle_step` = 文档里的旋转 45.000004°、
  `rotation_drag_commits_exactly_one_undo`、`undo_restores_the_rotation`），
  用例数 35 → 38，全绿。

## 未完成项 / 已知限制

- **环的角度只在"射线打得到环面"时精确**：相机侧视环面（`|dot| < 0.02`）时退回
  水平像素增量 ×0.01，与旧行为一致。此时画面上的环本身就快成一条线，吸附仍按角度步长跳。
- **3D 环的半径来自屏幕臂长**：没有透视除法，远端实体的环在世界尺度上偏大。
  命中与绘制用的是同一份，所以"点得到"没问题，只是绝对世界半径不精确。
- **2D 的 `Center` 轴心仍是位置平均**（见缺项 6）；多选时圆心可能落在贴图的空白处。
- **未做框选模式的显式开关**，理由见缺项 4；如果后续要"框选时按 Tab 切包含/相交"，
  需要先把方向规则改成可配置项，属于行为变更而非补洞。
- **浮标只在主选中实体上显示**（多选时其余实体不各标一份），
  3D 侧浮在锚点、2D 侧浮在指针旁，位置口径不同但字号一致。
- **`snap_angle_step` 不持久化**，与整族吸附开关口径一致；重启回 15°。
- 环/把手的**真实手感未经人眼验收**：本轮证据是纯逻辑单测 + 合成指针 + 真实帧文档对拍，
  连续拖动的观感（环是否好抓、三点重叠时的取舍）留给 M27-03 的窗口浸泡。
