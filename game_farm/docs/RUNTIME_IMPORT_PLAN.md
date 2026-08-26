# game_farm 运行时资源导入计划

这个文档用于定义 `assets/final_candidate/` 进入正式运行时资源流程的顺序、批次和基本规则。

它和其它文档的关系：

- `docs/STYLE_GUIDE.md`
  - 定义主风格候选 A 的边界
- `docs/FINAL_ASSET_INDEX.md`
  - 定义每张最终候选图的预期用途
- `docs/RUNTIME_IMPORT_PLAN.md`
  - 定义“先导什么、后导什么、导入时要注意什么”

## 总体原则

1. 先导“空间骨架”，再导“玩法核心”，最后导“表现层”。
2. 优先从 `assets/final_candidate/` 取图，不重新回全量候选池。
3. 每一批导入后，都要能形成一个可验证的小闭环，而不是只导入零散贴图。
4. UI 与场景资源分开推进，避免相互阻塞。

## 推荐导入顺序

### 批次 1：空间骨架

目标：

- 先让项目具备最基本的农场、小镇、室内三种空间骨架
- 能够用同一套风格把“可走动区域”搭起来

优先导入：

- `assets/final_candidate/environment/farm_tileset_main_kenney.png`
- `assets/final_candidate/environment/grass_tile_anwar.png`
- `assets/final_candidate/environment/dirt_tile_anwar.png`
- `assets/final_candidate/town/town_tilemap_kenney_pico8.png`
- `assets/final_candidate/interior/indoor_tileset_kenney_roguelike.png`
- `assets/final_candidate/interior/furniture_and_walls_sheet_cc0.png`

验收标准：

- 能拼出一个农场主场景
- 能拼出一个镇子或街道场景
- 能拼出一个基础室内场景

### 批次 2：农场核心循环

目标：

- 让玩家、地块、作物、农具形成最基本的种植循环

优先导入：

- `assets/final_candidate/crops/vegetables_sheet_scratchio.png`
- `assets/final_candidate/crops/fruits_sheet_scratchio.png`
- `assets/final_candidate/crops/cereals_sheet_scratchio.png`
- `assets/final_candidate/crops/tomato_anwar.png`
- `assets/final_candidate/crops/potato_anwar.png`
- `assets/final_candidate/items/hoe_anwar.png`
- `assets/final_candidate/items/watering_can_anwar.png`
- `assets/final_candidate/characters/farmer_down_sample_hnnnnnnn.png`
- `assets/final_candidate/animals/animals_sheet_16x16_cc0.png`

验收标准：

- 场景中可明确区分农田、作物与角色
- 工具图可进入基础 HUD 或背包占位
- 动物资源至少能作为牧场原型占位

### 批次 3：采集与作坊循环

目标：

- 把“农场外”的收集与材料回收补齐

优先导入：

- `assets/final_candidate/fish/water_terrain_kenney.png`
- `assets/final_candidate/fish/fish_blue_kenney.png`
- `assets/final_candidate/foraging/plants_sheet_flowers_cc0.png`
- `assets/final_candidate/foraging/mushroom_red_cc0.png`
- `assets/final_candidate/mining/ore_gold_cc0.png`
- `assets/final_candidate/materials/block_texture_tilemap_16x16_cc0.png`
- `assets/final_candidate/storage/pallet_pile_cc0.png`
- `assets/final_candidate/storage/wooden_box_32x32_cc0.png`

验收标准：

- 能形成水边采集或钓鱼区
- 能形成野外采集点
- 能形成作坊/仓储区

### 批次 4：镇子表现与商店识别

目标：

- 让镇子和店铺从“空间存在”进化为“功能可识别”

优先导入：

- `assets/final_candidate/town/market_stand_clean_cc0.png`
- `assets/final_candidate/town/market_stand_worn_cc0.png`
- `assets/final_candidate/shop/bookcase_cc0.png`
- `assets/final_candidate/town_props/lamp_post_32x32_cc0.png`
- `assets/final_candidate/town_props/open_sign_01_32x32_cc0.png`
- `assets/final_candidate/town_props/hanging_sign_01_32x32_cc0.png`

验收标准：

- 镇子里能明确识别出商店入口、摊位和街道识别物
- 店内外视觉有基本分层

### 批次 5：UI 与交互表现

目标：

- 把当前场景资源接上统一的界面皮肤

优先导入：

- `assets/final_candidate/ui/menu_panel_anwar.png`
- `assets/final_candidate/ui/menu_bar_anwar.png`
- `assets/final_candidate/ui/hud_bar_anwar.png`
- `assets/final_candidate/ui/button_start_anwar.png`
- `assets/final_candidate/ui/button_pause_anwar.png`
- `assets/final_candidate/ui/button_ok_anwar.png`
- `assets/final_candidate/ui/icon_dollar_anwar.png`

验收标准：

- 至少有一版统一的主菜单或暂停菜单
- 至少有一版基础 HUD
- 商店或背包界面具备统一皮肤

## 每批导入时的处理规则

### 1. 图集规则

- 同一批次内部优先做主题图集，不一次性做全项目总图集
- 建议先按：
  - `environment_town`
  - `interior_kitchen`
  - `farmplay`
  - `gathering_workshop`
  - `ui_core`
  这样的粒度拆

### 2. 命名规则

- 运行时资源名优先表达用途，而不是原始来源
- 原始来源信息保留在文档里，不强行塞进最终运行时 id

建议示例：

- `farm.ground.main`
- `farm.ground.soil`
- `town.market.stand_a`
- `ui.panel.main`
- `ui.button.confirm`

### 3. 替换规则

- 如果导入过程中发现某张图不合适，不直接覆盖原图
- 先在 `docs/FINAL_ASSET_INDEX.md` 更新用途或状态
- 再决定是否替换 `assets/final_candidate/` 中对应文件

### 4. 校验规则

每完成一个批次，至少检查：

- 风格是否仍然统一
- 场景中是否已经形成可读的小闭环
- UI 是否和场景主风格冲突

## 当前不建议立即导入的内容

- `LPC` 授权更严格的资源
- 等距参考资源
- `assets/textures/` 中未进入 `final_candidate/` 的边缘候选

这些内容仍然保留价值，但不应阻塞主线导入。

## 建议的首个实施里程碑

建议先完成：

1. 批次 1：空间骨架
2. 批次 2：农场核心循环

完成这两个批次后，项目就会拥有：

- 一个可辨认的农场空间
- 一个可辨认的镇子/室内空间
- 一套可用于原型验证的基础农耕视觉闭环

这会比直接先做 UI 或零散物件更容易形成可见成果。
