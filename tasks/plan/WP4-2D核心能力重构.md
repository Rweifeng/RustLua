# 任务：WP4 2D 核心能力重构

对应 [docs/40](../../docs/40-引擎全面重构计划.md) §3 WP4。状态：**①–⑨ 全部已做——⑨ World2D 分层
已于 2026-09-26 裁定达成**（三显式层由 WP6 的 2D/3D 统一批次落地，像素级门兜底，见下「已裁定」段）。

## 已完成（按 docs/40 §3 WP4 的九条实施要点）

| 要点 | 状态 | 证据 |
|---|---|---|
| ① 统一 Transform 与 XY 便利 API | 已做 | WP2 的 `Local/GlobalTransform` + Lua 统一 `transform` 表（[WP9](WP9-编辑器Lua迁移收口.md)） |
| ② Camera2D 正交控制器 | 已做 | `core/camera2d_controller.rs`：follow/dead-zone/帧率无关平滑/确定性抖动；9 个测试 |
| ③ Sprite pivot/nine-slice/区域/flip/颜色/层/排序键 | **已做** | pivot 早已有、flip 已补（docs/08）、层与 order 已有；**nine_slice 已补（P2-②b，2026-09）**——`Sprite`/`SpriteComponent` 增加 `nine_slice`（组件串 `nine_slice=l,r,t,b`、校验 l+r/t+b≤1），`quad_sliced_with_terms`/`quad_sliced_world` 产出 3×3 九 quad，`SpriteBatch` 三条构建路径混有九宫格项时整体降级串行（普通精灵仍并行、输出逐字节不变）；schema 注册后由通用字段编辑器接管；引擎 lib 880 测试全绿（876 基线 + 4 个九宫格测试） |
| ④ Tilemap chunk dirty / 可见 chunk 提取 | **已做** | 可见瓦片剔除（docs/53 §3.9，−17%/−71%）+ **chunk 级脏重建（2026-09，触发条件未出现、按指示提前实现）**：32×32 格切块行主序缓存（`TileChunk`），编辑时逐块比对 `tiles` 行段只重建脏块（`rebuild_dirty_chunks`），append 按块行主序消费；`solid`/`meta` 变化与 `position`/`rotation` 变化零重建（不参与 sprite 生成），`scale` 变化全块重建（`center_offset` 预乘 scale）；1000×1000 改一格只重建 1 块（O(块) 而非 O(100 万格)，单测 `million_cell_edit_is_chunk_bounded`）；像素等价由 wp0_2d_golden `tilemap_culling.pixels_identical=true` 兜住；引擎测试 887 全绿（另有 3 个失败为并行会话 title_menu 未提交改动引入，与本项无关） |
| ⑤ 粒子 SoA/池化/曲线 | 部分已做 | 曲线已补（尺寸/颜色/自转/重力/阻力）；SoA 未做——docs/40 §8 的"1 万粒子 <4 ms"实测 **1.88 ms 已达标**，缺动力 |
| ⑥ 屏幕 UI 与世界文字分离 | 已做 | `WorldText`（docs/12 §世界文字），走同一字形管线 |
| ⑦ 动画 clip/state/事件边界 | 已做 | `AnimationClip::events` + Lua `animation.events()`（帧事件语义逐条有测试） |
| ⑧ 输入统一 screen→world | 已做 | `camera2d.screen_to_world/world_to_screen/mouse_world`，与渲染共用同一份投影 |
| ⑨ World2D / Legacy2DOverlay / ScreenUI 三种显式层 | ✅ 已做（WP6 2D/3D 统一批次落地，2026-09-26 裁定达成） | `core/layer.rs` 的 `RenderLayer::{World2D,Overlay2D,ScreenUI}` + `OVERLAY_DRAW_ORDER` 分桶（桶内原序，层序由渲染器保证）；`runtime_scene.rs` 的 `mode=overlay/screen/world` 显式显示模式路由；`RenderPass::Legacy2DOverlay`（World2D+Overlay2D 两桶）+ ScreenUI 独立通道；旧项目默认走 Legacy2DOverlay——`wp0_2d_golden` 泄漏/区域/行序/剔除逐字节不变；`wp0-hud-golden-v1` HUD 隔离门进 verify.ps1；详见 docs/54 批 B/J、阶段 E 与本文件「已裁定」段 |

## ~~待办：Tilemap chunk 级脏重建~~（已做，2026-09——**触发条件未出现，按指示提前实现**）

**为什么提前做**：文档口径的触发条件是"出现 1000×1000 量级的真实地图"——核查 game_jhyt 全部真实
.scene，最大 40×22（village/mountain_path/combat_arena/manor），interior 系列 20×12，引擎源码/测试
无 1000×1000 → **条件未触发**。但整层重建代价是 O(总格数)，1000×1000 时会是十几毫秒、远超 docs/40 §5
的 <4ms 目标，为未来大图铺路而提前实现（本段因此保留原量尺数字作为基线：
32×32 编辑 +2 µs、100×100 +107 µs，docs/53 §3.9）。

**已实现**（引擎 lib 单测 `tilemap_chunk_tests` 7 项全绿）：
1. 每层按 32×32 格切分（`TILE_CHUNK_SIZE`），缓存块内 sprite 列表（`TileChunk`，边缘块不满）；
2. 编辑时**逐块比对** `tiles` **行段**（块区域在 row-major `tiles` 里不是连续切片——块宽 < 层宽时跨行，
   逐行段比较才覆盖真实区域），只重建变化的块（`rebuild_dirty_chunks` 返回重建块数）；
3. 块序拼接按块行主序（`chunks` 按 `(oy, ox)` 字典序），块内行主序，append 消费顺序稳定；
   像素结果不变——wp0_2d_golden 的 `tilemap_culling.pixels_identical=true`（剔除开/关逐字节相同）兜住。
   注意：块行主序 ≠ 整层行主序（层宽 > 块宽时顺序不同），瓦片互不重叠 → 像素不变。

**语义边界**（有单测）：`solid`/`meta` 变化与 `position`/`rotation` 变化 → 零块重建（它们不参与
sprite 生成；`center_offset` 相对层原点）；`scale` 变化 → 全块重建（`center_offset` 预乘 scale）。
1000×1000 改一格只重建 1 块（`million_cell_edit_is_chunk_bounded`）——编辑代价从 O(N) 降为 O(块)。

量尺已在门里（`wp0-v4` 的 `tilemap_edit_stairs` + `tilemap_stress` 100×100），数字见 docs/53 §3.9。

## ~~待办：世界 Sprite 的 nine_slice~~（P2-②b 已做，2026-09）

原缺口：UI `Image` 已有九宫格，世界 `Sprite` 没有。P2-②b 以编辑器 2D 验收（docs/40 编辑器 2D 行
「锚点/九宫格/碰撞 overlay」）作为真实用例补齐：见上表 ③ 的证据行。

## 已裁定：⑨ World2D / Legacy2DOverlay / ScreenUI 三种显式层（2026-09-26）

docs/40 §3 WP4 ⑨ 要求引入三种显式层，旧项目默认走 `Legacy2DOverlay`。**现场核对结论：已达成**——
三显式层在 WP6 的 2D/3D 统一批次（docs/54 批 B/J、阶段 E）已落地，并有像素级门兜底，WP4 无需再实现：

- **层是类型不是注释**：`core/layer.rs` 定义 `RenderLayer::{World2D, Overlay2D, ScreenUI}`
  （默认 World2D），渲染器的 2D 叠加通道按 `OVERLAY_DRAW_ORDER` 分桶绘制（`layered_indices`，桶内原序）；
  层的先后由渲染器保证而非调用点排版（单测：错序输入 → 按层次重排且桶内相对顺序不变）。
- **显式显示模式**：`runtime_scene.rs` 中 `mode=overlay`/`mode=screen` 的实体分别落 `Overlay2D`/`ScreenUI`
  桶（画在世界内容之上、ScreenUI 不被其它 2D 内容盖住）；`mode=world` 请求世界通道；未声明 = 默认
  World2D 桶 = 走 `Legacy2DOverlay` 通道（World2D + Overlay2D 两个桶）——**旧项目默认行为不变**
  （`wp0_2d_golden` 的泄漏/区域/行序/剔除断言逐字节保持）。
- **ScreenUI 独立通道**：`RenderPass::Legacy2DOverlay` 与 `RenderPass::ScreenUI` 两条独立通道；
  `wp0-hud-golden-v1`（关掉屏幕 UI 的像素是调试线青色、打开后必须是 HUD 品红）进 `verify.ps1` 逐条判红。
- 渲染器另有 `world_2d_draws/overlay_2d_draws/screen_ui_draws` 三层计数（诊断/量尺）。

**世界 Sprite 深度通道不在 ⑨ 范围**：它是 WP6 的独立待办（地基：Sprite 世界 z、
`Camera2D::view_projection` 已完成；通道本体进行中），⑨ 的分层语义已为其留好位置
（World2D 可参与深度、Overlay/Screen 不参与）。

**同源核对（本裁定一并落定）**：

- **Tile 画笔方向：一致**。运行时约定（docs/53 §3.6，golden 像素固定）`tiles[0]` 行 = 世界 Y 最小 =
  屏幕最下方；编辑器画笔 `tilemap_cell_from_pointer`（scene_view.rs）走 `Camera2D::screen_to_world`
  （Y 向上）→ `row = floor((world.y − pos.y)/cell_h)`，行 0 同样 = 世界 Y 最小 = 屏幕最下方；
  落盘 `apply_tile_paint_cell` 的 `index = y*width + x` 与运行时行序一致；网格预览
  `tilemap_cell_screen_rect` 同一 Y 向上映射。⇒ 画笔所见 = 运行时所得，无翻转。
- **编辑器 picking 投影：同源**。编辑器 2D 场景视图的 pan/zoom/ppp → 世界坐标已收敛到
  `editor_scene_camera(pan, zoom)`（即 `Camera2D` 实例）的 `world_to_screen`/`screen_to_world`
  （scene_view.rs 注释明说"同一份投影"，此前各处手写 `pan + world*zoom` 再除 ppp 已统一）——
  docs/40 ⑧"编辑器 picking 使用同一投影"在 2D 侧达成。计划此前引用的 docs/25 §14"两套映射"出处已变
  （docs/25 §14 现为生产工具章节，其 §14.4 写明共用同一套 world→panel 映射）。

**影响面**：纯裁定，不改这三处的语义与代码；仅更新本文件与 docs/53 / docs/54 / docs/00 / docs/README
的状态表述。
