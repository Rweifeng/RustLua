# 任务：WP5 统一 Camera、Projection 与 RenderView

对应 [docs/40](../../docs/40-引擎全面重构计划.md) §3 WP5。状态：**完成**。相机收敛到
`core/camera.rs`（2D 与 3D 同址）+ 统一 `Projection` / `RenderView` / `FrameViews` /
`CameraSelection`；渲染器消费视图声明（清屏 + 视口）；`Camera3D` 姿态可由 `Transform3D` 提供；
运行时与编辑器（Scene 3D / Scene 2D / 设计分辨率画面，以及 2D 拾取映射）全部走同一套投影。
验收指标逐条对照见文末。

## 裁定

项目负责人于本轮裁定：**合并 2D 与 3D 相机逻辑，统一到 `camera.rs`**（原为待裁定项）。随之要求
`camera2d_controller` 一起跟随相机的新设计变更，已一并完成。

## 已完成

### 1. 两套相机同址

`core/camera.rs` 现在是相机逻辑的唯一住处：

| 原来 | 现在 |
|---|---|
| `math2d::{Viewport, CameraBounds, Camera2D, CoordinateError, DEFAULT_2D_CAMERA_CENTER}` | `camera::` 同名类型 |
| `camera3d::{Camera3D, Ray3, Plane, Frustum, FrustumClassification, Camera3dError, Camera3dComponentError}` | `camera::` 同名类型 |
| `camera2d_controller::Camera2dController` | `camera::Camera2dController` |

`camera3d.rs` 已删除；`math2d.rs` 只留 2D 坐标轴常量与颜色空间（并把 `CoordinateError`
**重新导出**，既有 `math2d::CoordinateError` 引用无需改动）。

### 2. 统一的投影与视图描述

- `Projection::{Orthographic, Perspective}`：正交与透视的唯一说法，带 `from_camera2d` /
  `from_camera3d` / `uses_depth` / `is_orthographic`；**不强制**两者共用 shader 与深度规则
  （与 docs/40 §3 WP5 的要求一致）。
- `ClearPolicy::{None, Color}`、`RenderView { projection, viewport, clear, order, visibility }`：
  一帧渲染所需的相机侧描述，目标是"渲染器消费快照、不回查 ECS"。
- `CameraSelection`：显式选中优先、否则退回**排序序第一个**；候选不存在时退回第一个而不是不画
  （多相机选择从"两处各写一遍"变成一处可测逻辑）。

### 3. 控制器跟随新设计

`Camera2dController` 不再只返回一个中心点，而是给出**本帧应当渲染的相机**：

- `resolve_camera(current, follow_target, viewport, dt) -> Camera2D`：把"请求消费 → 跟随 →
  advance → bounds 钳制"的**次序固定在类型内部**。此前宿主自己拼这套顺序，而且宿主与控制器
  各自夹一次 bounds。
- `resolve_view(...) -> RenderView`：直接给出统一视图描述。

宿主 `step_camera2d` 随之瘦身；**宿主的 `constrained_camera_center` 与 `clamp_center` 两个副本
已删除**（改用 `Camera2D::constrained_center`，该函数因此提升为公开 API），瓦片相机也走同一份
钳制——"夹取规则只有一份"。

## 旧新对拍（行为不变）

迁移不改任何算法，证据是既有的门**原样通过**：

- engine lib 测试 634 项（含 2D 相机 6 项、3D 相机 11 项、控制器 9 项，全部原样）；
- 3D semantic golden（`wp0_3d_golden`）与 3D 阶梯；
- 2D overlay golden（`wp0_2d_golden`：顶点数/像素/剔除等价全部精确断言）；
- `verify.ps1 -Benchmarks` 全绿。

新增的**数值基线**把"约定"钉死，供后续任何投影改动对拍：

- `projection_conventions_are_pinned_for_the_convergence`：垂直 FOV、深度范围 **0..1**、
  右手系 **-Z** 前向、NDC **y 向上**、射线与投影互为逆、视锥前后分类；
- `visible_world_bounds_agrees_with_the_projection`：剔除用的可见域与绘制用的投影必须一致
  （无旋转时四角精确对应，旋转时可见域必须**包含**四角）；
- `unified_projection_and_render_view_match_both_families` 与
  `resolve_camera_owns_request_follow_and_bounds_order`：收敛本身的可测行为。

## 渲染器消费视图描述（本轮完成）

清屏色此前**写死在渲染器的 pass 里**（`0.04, 0.07, 0.12`），视口恒为整张目标，"这块区域
只画 2D overlay"或"这帧不要清屏"宿主都无处可说。现在：

- `FrameViews { main, overlay }`：一帧交给渲染器的两类视图，`clear()` 给出清屏策略，
  `ordered()` 按 `order` 升序给出绘制顺序；
- `Renderer::set_frame_views`：渲染器据此清屏并设视口（`ClearPolicy::None` = 不清屏），
  声明的视口等于整目标时**不调用** `set_viewport`，命令流与收敛前完全一致；
- `DEFAULT_3D_CLEAR` 成为清屏色的唯一定义（渲染器未收到声明时的历史默认 = 宿主声明值）；
- `RenderView::main_2d` 补齐"2D 也可以是这一帧的底"（主/叠加说的是清屏策略，不是 2D/3D）；
- `RuntimeScene::render_view_2d(w, h)`：2D overlay 视图与 `vertices(w, h)` 用**同一个相机**；
- 宿主接线：运行时（`game`）声明 3D 主视图 + 2D overlay；编辑器三个视图（Scene 3D、Scene 2D、
  设计分辨率游戏画面）各自声明 `main_3d` / `main_2d`。

对拍与回滚（`declared_frame_views_own_clear_color_and_overlay_viewport`，真实 GPU 回读）：

| 场景 | 断言 |
|---|---|
| 不声明任何视图 | 读回历史默认清屏色 |
| 声明主视图但用 `DEFAULT_3D_CLEAR` | 与"不声明"的帧**逐字节相同**（回滚档等价） |
| 声明红色清屏 | 整帧都是 `[255,0,0,255]`（清屏确实由声明驱动） |
| 声明 `ClearPolicy::None` | 同一张附件上重画一帧后红色仍在（真的没清屏） |
| overlay 视口 = 左半屏 | 左半屏出现白 quad，右半屏保持清屏色（视口确实生效） |

`clamp_viewport`：声明的视口夹到目标内且至少 1×1（越界的 `set_viewport` 会被 wgpu 判错）。

## Camera3D 姿态来自 Transform（本轮完成）

`Camera3D` 新增 `pose_from_transform`（组件串 `pose=transform`，**默认关闭**）：

- 打开后姿态取自实体的 `Transform3D`：位置 = 变换位置，视线 = 旋转后的 **-Z**，上方向 =
  旋转后的 **+Y**，`fov/near/far/aspect` 仍来自组件；`position`/`target` 退化为回退值；
- 只影响**声明了**的那台相机：同一场景里两台相机（一台 `pose=transform`、一台 look-at）
  的测试证明未声明的那台姿态不变（`runtime_camera_pose_follows_transform_when_declared`）；
- 缺 `Transform3D` 或姿态退化时不静默画错，而是回退组件姿态并在 `scene_extract` 留
  `pose=transform` 诊断（`camera_pose_from_transform_is_extracted_and_diagnosed_when_missing`）；
- 序列化只在打开时写 `pose=transform` ⇒ 既有场景串**逐字节不变**；编辑器 Inspector 有对应
  复选框，读改写不会抹掉该标志；
- 回滚档 = 不写 `pose=transform`（即 `pose=look_at` 的旧行为）。

## 编辑器拾取与运行时同一投影（本轮完成）

编辑器 Scene 视图此前在**五处**各自手写了一遍"`pan + world * zoom` 再除 `ppp`"的面板映射
（`screen_to_world_2d`、`entity_2d_screen_box`、`tilemap_cell_screen_rect`、
`design_resolution_frame`、`design_rect_to_panel`），与运行时 `Camera2D` 是两套说法；docs/40 §5
的输入/拾取目标要求"高 DPI/缩放/旋转下命中误差 <1px；Lua 与编辑器一致"。

现在只有**一份**实现：`editor_scene_camera(pan, zoom)` 造出与运行时同一个 `Camera2D`，
`editor_scene_viewport(rect, ppp)` 把面板逻辑尺寸折成物理像素视口，`scene_view_world_to_panel` /
`scene_view_panel_to_world` 走 `Camera2D::world_to_screen` / `screen_to_world`；上述五处全部改为
调用它。

**旧新对拍**（`scene_view_projection_matches_the_hand_written_pan_zoom_mapping`）：在
`ppp ∈ {1, 1.25, 2} × zoom ∈ {0.5, 1, 2.5, 8} × pan` 的网格上，把收敛后的映射与**收敛前手写
公式的原文**逐点比对（面板→世界、世界→面板两个方向，误差 <1e-3），并断言两者互为逆、设计分辨率
包围框的中心就是游戏相机中心的世界坐标映射。另外 `tilemap_cell_rect_matches_the_hand_written_mapping`
把瓦片格矩形与旧公式逐边比对，并断言**第 0 行画在面板下方**（与运行时 2D golden 的"第 0 行在
底部"行序约定同向）——笔刷格与渲染图不会一个朝上一个朝下。

多相机选择同轮收口：宿主 `active_camera3d` 改用 `CameraSelection`（显式选中优先、否则排序序
第一个），不再自己写一遍判断。

## 验收指标对照（docs/40 §3 WP5）

| 指标 | 证据 |
|---|---|
| 旧 2D `world_to_screen`/`screen_to_world` 等价 | 相机单测原样 + 编辑器映射网格对拍（旧公式原文比对）+ 2D overlay golden |
| 旧 3D projection/ray/frustum 等价 | `projection_conventions_are_pinned_for_the_convergence`（垂直 FOV/深度 0..1/-Z/NDC y 向上/射线互逆/视锥分类）+ 3D golden |
| viewport resize/DPI/离屏一致 | `clamp_viewport` 夹取测试 + 编辑器 ppp 网格对拍 + 离屏回读测试（`declared_frame_views_own_clear_color_and_overlay_viewport`） |
| 多相机选择确定 | `unified_projection_and_render_view_match_both_families`（选中/回退/候选不存在）+ `runtime_camera_pose_follows_transform_when_declared` |
| 3D 画面无回退 | 3D semantic golden 与 3D 阶梯原样通过；`pose=transform` 默认关闭且既有场景串逐字节不变 |
| 姿态冲突给诊断 | `camera_pose_from_transform_is_extracted_and_diagnosed_when_missing` |
| 回滚 | 不声明 `FrameViews`（历史默认清屏色 + 整目标视口，逐字节等价）、不写 `pose=transform`、`ClearPolicy::None` 均可在场景/宿主级逐视图启用 |
