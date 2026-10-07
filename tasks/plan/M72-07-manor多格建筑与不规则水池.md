# 任务：manor 多格建筑与不规则水池

- 任务 ID：M72-07
- 状态：待办
- 关联里程碑：M72（P1 金标场景 manor · 金标收口）
- 创建日期：2026-10-08
- 依赖：M72-06
- 阻塞：M72-08、M72-11（**P1 规范在此定稿，之后才铺量**）

## 目标

把建筑从"1–2 格锚点的小贴片"重做成有屋顶脊线、墙面窗门、右下投影的多格建筑；
水池从纯蓝矩形改成不规则岸线。**本包收口 = P1 金标完成，产出可复制规范给 M72-11。**

## 现状（实测）

- `assets/art/buildings/` 共 **27 张**，其中 8 张是 `build_*` 建造幽灵（foundation/scaffold/
  training_hall/tea_shed/storehouse/herb_hut/forge_stand…）。
- `manor.scene` 真正常驻的建筑只有 3 个：`manor_house.png` / `manor_shed.png` / `manor_gate.png`。
- 星露谷口径（`53` §3.5、`51` §2.1）：**多格视觉 + 单格根碰撞**；
  屋顶有脊线与瓦片层次、墙面有窗/门/木纹、右下投影。
- 水池：`manor_ground.png` 里 value `6` 共 30 格，纯蓝矩形。

## 影响文件

- `examples/game_jhyt/tools/gen_manor_art.py`（已有，扩展）
- `examples/game_jhyt/assets/art/buildings/manor_*`
- `examples/game_jhyt/scenes/manor.scene`
- `examples/game_jhyt/scripts/data/buildings.lua`（多格占位与单格根碰撞的登记）
- `examples/game_jhyt/docs/44-*.md`（山庄场景重构，回写）

## 设计要点

1. **多格视觉 + 单格根碰撞**：视觉跨 3×3 或 4×3，碰撞只占入口那一格。
   这条 `53` §3.5 已定，落地时最容易忘——忘了就是玩家撞空气。
2. 屋顶/墙/门窗分件，别合成一张死图（分件才能复用、才能换季、才能做 tier 升级）。
3. 建筑投影走 M72-06 的统一光向通道，不在建筑贴图里烤死阴影（否则换季/换光向要重画）。
4. 水池：岸线用 M72-04 的水族 autotile + M72-05 的石头/睡莲装饰压边，
   中心深度渐变走 per-tile tint。

## 风险

- 多格建筑与建造系统（`build` 模式、`buildings.lua`）耦合：占位形状一改，
  建造落位校验、拆除、七日炼药目标都可能受影响。
  注意最近一笔 `e585fec` 刚修过"建造模式看不见自己选的是哪座"，这块是活的。
- 建筑是 sprite 不是 tile，实体数继续涨（M72-05 已把这条列为性能前置）。

## 验收方式

- 目验：`manor` 与 `references/stardew/farmhouse-with-cubby.png` / `house-tier1.png` 并排。
- `jhyt_m72_building_footprint`：断言每个建筑**视觉跨多格、碰撞只有一格**，
  且玩家能走到门口（路径可通）。
- **反面对照**：把某建筑的 `solid` 扩到整个视觉范围，门必须红（证明它真在测"单格根碰撞"）。
- 建造系统回归：`build` 模式落位/拆除/幽灵可见性手测一遍（`e585fec` 那块）。
- **P1 金标收口判据**：`manor` 达到 §M72-00 定的规范，且规范已回写 `57`/`51` →
  才允许 M72-11 开工。

## 实现记录

（待回填）

## 验收证据

- [ ] 每栋建筑"视觉多格 / 碰撞单格"读数
- [ ] `jhyt_m72_building_footprint` 绿 + 反面对照跑红
- [ ] 建造模式落位/拆除/幽灵手测记录
- [ ] 水池非矩形（岸线格数与形状读数）
- [ ] P1 规范已回写 `57`/`51`（金标收口）
- [ ] 已登记进 `verify.ps1` MVP 清单

## 未完成项 / 已知限制

（待回填）
