# 任务：WP4 2D 核心能力重构

对应 [docs/40](../../docs/40-引擎全面重构计划.md) §3 WP4。状态：**①③④⑤⑥⑦⑧ 已做；②（相机控制器）
已做；⑨ World2D 分层待裁定。**

## 已完成（按 docs/40 §3 WP4 的九条实施要点）

| 要点 | 状态 | 证据 |
|---|---|---|
| ① 统一 Transform 与 XY 便利 API | 已做 | WP2 的 `Local/GlobalTransform` + Lua 统一 `transform` 表（[WP9](WP9-编辑器Lua迁移收口.md)） |
| ② Camera2D 正交控制器 | 已做 | `core/camera2d_controller.rs`：follow/dead-zone/帧率无关平滑/确定性抖动；9 个测试 |
| ③ Sprite pivot/nine-slice/区域/flip/颜色/层/排序键 | **大部分已做** | pivot 早已有、flip 已补（docs/08）、层与 order 已有；**nine_slice 只对 UI Image 有，世界 Sprite 没有**（待办） |
| ④ Tilemap chunk dirty / 可见 chunk 提取 | **部分已做** | 可见瓦片剔除已做（docs/53 §3.9，−17%/−71%）；**chunk 级脏重建未做**（见下） |
| ⑤ 粒子 SoA/池化/曲线 | 部分已做 | 曲线已补（尺寸/颜色/自转/重力/阻力）；SoA 未做——docs/40 §8 的"1 万粒子 <4 ms"实测 **1.88 ms 已达标**，缺动力 |
| ⑥ 屏幕 UI 与世界文字分离 | 已做 | `WorldText`（docs/12 §世界文字），走同一字形管线 |
| ⑦ 动画 clip/state/事件边界 | 已做 | `AnimationClip::events` + Lua `animation.events()`（帧事件语义逐条有测试） |
| ⑧ 输入统一 screen→world | 已做 | `camera2d.screen_to_world/world_to_screen/mouse_world`，与渲染共用同一份投影 |
| ⑨ World2D / Legacy2DOverlay / ScreenUI 三种显式层 | **未做** | 范围决策，见下 |

## 待办：Tilemap chunk 级脏重建（有触发条件，不盲做）

**为什么没做**：先补了量尺才敢决定。实测（docs/53 §3.9）改一格之后的净代价是 32×32 **+2 µs**（噪声）、
100×100 **+107 µs**——代价与**总格数**成正比（整层 sprite 列表重建），但在这个量级上完全不痛；而稳态帧的
大头已经被"视口外瓦片剔除"拿走了。chunk 化在这种规模下只剩很小的净收益，却要引入块划分、逐块比对与
块序拼接三处新复杂度。

**触发条件**：出现 1000×1000 量级的真实地图（1000×1000 时整层重建会是十几毫秒，远超 docs/40 §5 的
"接口更新目标 <4ms"）。到那时要做的是：
1. 每层按固定块（建议 32×32 格）切分，缓存块内 sprite 列表；
2. 编辑时**逐块比对** `tiles` 切片，只重建变化的块；
3. 块序拼接必须保持绘制顺序稳定（块行主序），像素结果不变——由 2D golden 的像素等价证明兜住。

量尺已在门里（`wp0-v4` 的 `tilemap_edit_stairs`），做之前先看数字。

## 待办：世界 Sprite 的 nine_slice（③ 的缺口）

UI `Image` 已有九宫格，世界 `Sprite` 没有。需要先确认真实需求（当前项目里没有用例），
否则属于"为了对齐清单而加功能"。

## 待裁定：⑨ World2D / Legacy2DOverlay / ScreenUI 三种显式层

docs/40 §3 WP4 ⑨ 要求引入三种显式层，旧项目默认走 `Legacy2DOverlay`。这决定了 2D 渲染的语义边界
（世界空间 2D 是否参与深度、屏幕 UI 是否与世界共用顶点缓冲），与 WP6 的 `RenderSnapshot` 直接耦合，
属于范围决策。

**同时需要一并裁定的还有两件"编辑器与运行时对 2D 空间理解是否同源"的事**（它们本质是同一个问题）：

- 瓦片 `tiles` 第 0 行画在地图**最下方**（2D 世界 Y 向上），此约定此前无文档、现已被 2D golden 用像素
  固定；**编辑器 Tile 画笔的网格预览方向是否一致尚未核对**；
- 编辑器场景视图用自己的 pan/zoom/ppp 面板映射，与运行时 `Camera2D` 是两套（docs/25 §14），
  docs/40 §3 WP4 ⑧ 要求"编辑器 picking 使用同一投影"。

裁定前不改这三处的语义。
