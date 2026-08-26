# game_farm 风格指南

这个文档用于把当前资源库里的“主风格候选”固定下来，避免后续每次做场景或 UI 预览时重新选图。

当前结论：

- 先采用一套以 `Kenney + ScratchIO + Anwar + 若干 CC0 单体物件` 为核心的混合主风格
- 暂不把 `LPC` 系列纳入默认主风格
- 暂不把等距资源作为正式运行时素材，只保留为参考板

## 当前推荐主风格

推荐命名：`主风格候选 A`

定位：

- 2D 正交视角
- 轻松、明亮、偏农场生活感
- 允许来源混合，但要求轮廓清楚、颜色不脏、阅读性优先

## 主风格候选 A 的资源组成

### 1. 场景地形与镇子骨架

优先使用：

- `assets/textures/environment/farm_tileset_main_kenney.png`
- `assets/textures/environment/grass_tile_anwar.png`
- `assets/textures/environment/dirt_tile_anwar.png`
- `assets/textures/town/town_tilemap_kenney_pico8.png`

说明：

- `Kenney` 更适合承担场景大结构，例如农场地表、小镇道路、建筑区块
- `Anwar` 的草地和泥地可以补近景变化，让农田区更像可交互地块

### 2. 作物与农具

优先使用：

- `assets/textures/crops/vegetables_sheet_scratchio.png`
- `assets/textures/crops/fruits_sheet_scratchio.png`
- `assets/textures/crops/cereals_sheet_scratchio.png`
- `assets/textures/crops/tomato_anwar.png`
- `assets/textures/crops/potato_anwar.png`
- `assets/textures/items/hoe_anwar.png`
- `assets/textures/items/watering_can_anwar.png`

说明：

- `ScratchIO` 的总表适合做田地内容密度
- `Anwar` 的单体作物和工具更适合做交互焦点

### 3. 角色与动物

优先使用：

- `assets/textures/characters/farmer_down_sample_hnnnnnnn.png`
- `assets/textures/animals/animals_sheet_16x16_cc0.png`

说明：

- 当前角色资源还偏“样例展示”，足够做原型，但还不是最终角色标准
- 动物表适合占位和氛围补充，后续若进入正式生产，再决定是否换成信息量更高的一套

### 4. 室内、厨房与居家

优先使用：

- `assets/textures/interior/indoor_tileset_kenney_roguelike.png`
- `assets/textures/interior/furniture_and_walls_sheet_cc0.png`
- `assets/textures/kitchen/tableware_sheet_32x32_cc0.png`
- `assets/textures/kitchen/tables_and_misc_props_16x16_cc0.png`

说明：

- 室内主结构优先 `Kenney Roguelike Indoor`
- 其他室内资源以“补充桌椅、餐具和摆设”为主，不反过来替代主结构

### 5. 镇子与店铺

优先使用：

- `assets/textures/town/market_stand_clean_cc0.png`
- `assets/textures/town/market_stand_worn_cc0.png`
- `assets/textures/town_props/lamp_post_32x32_cc0.png`
- `assets/textures/town_props/open_sign_01_32x32_cc0.png`
- `assets/textures/town_props/hanging_sign_01_32x32_cc0.png`
- `assets/textures/shop/bookcase_cc0.png`

说明：

- 镇子气质目前是“小而清楚的摊位 + 灯牌 + 路牌”
- 店铺内饰尽量少而明确，不堆太多高细节物件

### 6. 采集、钓鱼与作坊

优先使用：

- `assets/textures/fish/water_terrain_kenney.png`
- `assets/textures/fish/fish_blue_kenney.png`
- `assets/textures/foraging/plants_sheet_flowers_cc0.png`
- `assets/textures/foraging/mushroom_red_cc0.png`
- `assets/textures/mining/ore_gold_cc0.png`
- `assets/textures/materials/block_texture_tilemap_16x16_cc0.png`
- `assets/textures/storage/pallet_pile_cc0.png`
- `assets/textures/storage/wooden_box_32x32_cc0.png`

说明：

- 这部分优先服务于“可读性”和“玩法识别”
- 一个区域里控制 1 到 2 个主视觉重点，不追求一次把所有物件都堆满

### 7. UI

优先使用：

- `assets/textures/ui/menu_panel_anwar.png`
- `assets/textures/ui/menu_bar_anwar.png`
- `assets/textures/ui/hud_bar_anwar.png`
- `assets/textures/ui/button_start_anwar.png`
- `assets/textures/ui/button_pause_anwar.png`
- `assets/textures/ui/button_ok_anwar.png`
- `assets/textures/ui/icon_dollar_anwar.png`

说明：

- 当前 UI 最成体系的是 `Anwar` 这一组
- 商店和背包展示时，可以混入食物、工具、矿石等单体图标，但面板和按钮皮肤先保持一套

## 暂不推荐纳入默认主风格的资源

### LPC 系列

暂不纳入原因：

- 授权要求更严格
- 动画和信息量更强，容易把当前轻量原型的视觉重心拉偏
- 与现有 `Kenney + Anwar + ScratchIO` 的轻量拼接节奏不完全一致

当前处理：

- 保留在 `assets/third_party/licensed/`
- 只做参考，不作为默认主线素材

### 等距资源

暂不纳入原因：

- 当前项目预览全部建立在 2D 正交视角上
- 等距资源更适合作为“布局参考”，不适合直接混入当前场景

当前处理：

- 保留在 `assets/third_party/environment/kenney_isometric_miniature_farm/`
- 保留在 `assets/third_party/interior/furniture_and_walls_cc0/`

## 当前风格使用规则

为了保持统一感，后续新增资源时优先遵守下面几条：

1. 一个场景先选一个主结构来源。
   - 例如农场主结构用 `Kenney`
   - 作物密度补充用 `ScratchIO`
   - 交互工具和 UI 用 `Anwar`

2. 单个区域最多混 2 到 3 个来源。
   - 超过这个数量后，画面通常会开始散

3. UI 皮肤先不混。
   - 面板、按钮、HUD 条优先保持同一来源

4. 高约束授权资源不直接进入默认组合。

5. 等距资源不直接进入当前运行时预览。

## 已落盘的最终候选层

当前已经按 `主风格候选 A` 建立：

- `assets/final_candidate/`

这层的用途是：

- 作为后续正式接入资源时的第一优先目录
- 避免每次都从 `assets/textures/` 的全量候选里重新筛
- 给后续做图集、命名规范和资源注册时提供一个稳定起点

这一层目前只放“已经决定保留”的主线候选，不再承担探索性收集工作。

## 现阶段可直接参考的预览场景

- `scenes/resource_preview.scene`
  - 看主题分类是否齐
- `scenes/day_slice_preview.scene`
  - 看整体气氛是否顺
- `scenes/farm_day_flow.scene`
  - 看一天流程是否顺
- `scenes/ui_preview.scene`
  - 看 UI 与物品图标是否协调

## 下一步建议

如果继续往前推进，建议按这个顺序做：

1. 继续以 `assets/final_candidate/` 为基础，再做一轮“剔除明显跳风格素材”
2. 开始定义更接近运行时的图集切分、命名和注册规则
3. 再决定是否需要第二套备选风格，例如更偏 `Kenney` 统一感或更偏 `Anwar` UI 感的方案
