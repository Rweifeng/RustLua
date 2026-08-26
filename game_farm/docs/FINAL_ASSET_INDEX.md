# game_farm 最终候选索引

这个文档用于给 `assets/final_candidate/` 中的素材建立“预期用途索引”。

目标：

- 让后续接入运行时资源时，不需要重新判断每张图打算做什么
- 给图集拆分、资源注册和命名规范提供一份稳定清单
- 保持 `主风格候选 A` 的用途边界清楚

## 使用方式

- 优先从 `assets/final_candidate/` 取图
- 如果这里缺图，再回到 `assets/textures/` 或 `assets/curated/` 补充
- 若某张图用途发生变化，先更新这里，再改后续运行时资源规划

## 索引清单

| 文件 | 预期用途 | 备注 |
|---|---|---|
| `assets/final_candidate/environment/farm_tileset_main_kenney.png` | 农场主地表与基础地图骨架 | 当前默认农场主 tileset |
| `assets/final_candidate/environment/grass_tile_anwar.png` | 农田外圈草地补片 | 用于补近景可交互地块边缘 |
| `assets/final_candidate/environment/dirt_tile_anwar.png` | 可耕地/农田土壤补片 | 用于田地与路径过渡 |
| `assets/final_candidate/town/town_tilemap_kenney_pico8.png` | 小镇主道路与建筑骨架 | 当前默认镇子主 tilemap |
| `assets/final_candidate/town/market_stand_clean_cc0.png` | 商店摊位主图 | 用于集市或杂货店外摆 |
| `assets/final_candidate/town/market_stand_worn_cc0.png` | 次级摊位或旧摊位变体 | 用于做新旧层次变化 |
| `assets/final_candidate/crops/vegetables_sheet_scratchio.png` | 蔬菜作物总表 | 用于田地区大面积内容填充 |
| `assets/final_candidate/crops/fruits_sheet_scratchio.png` | 水果作物总表 | 用于果园或高价值作物区 |
| `assets/final_candidate/crops/cereals_sheet_scratchio.png` | 谷物作物总表 | 用于田地区主种植内容 |
| `assets/final_candidate/crops/tomato_anwar.png` | 番茄单体焦点作物 | 用于交互高亮或图标展示 |
| `assets/final_candidate/crops/potato_anwar.png` | 土豆单体焦点作物 | 用于交互高亮或图标展示 |
| `assets/final_candidate/items/hoe_anwar.png` | 默认锄头图标 | 农耕工具主图 |
| `assets/final_candidate/items/watering_can_anwar.png` | 默认浇水壶图标 | 农耕工具主图 |
| `assets/final_candidate/characters/farmer_down_sample_hnnnnnnn.png` | 玩家农夫原型角色图 | 当前角色基准样例 |
| `assets/final_candidate/animals/animals_sheet_16x16_cc0.png` | 牧场动物占位总表 | 当前用于原型期动物展示 |
| `assets/final_candidate/interior/indoor_tileset_kenney_roguelike.png` | 室内主结构 tileset | 当前默认室内主图集 |
| `assets/final_candidate/interior/furniture_and_walls_sheet_cc0.png` | 家具与墙体参考总表 | 用于补室内摆设类型 |
| `assets/final_candidate/kitchen/tableware_sheet_32x32_cc0.png` | 厨房餐具主表 | 用于料理、餐桌和室内交互补充 |
| `assets/final_candidate/kitchen/tables_and_misc_props_16x16_cc0.png` | 桌椅与小摆件表 | 用于厨房和屋内摆设补充 |
| `assets/final_candidate/fish/water_terrain_kenney.png` | 水域主地表图 | 用于河边、池塘和钓鱼区 |
| `assets/final_candidate/fish/fish_blue_kenney.png` | 默认鱼类样例图 | 用于钓鱼系统和背包展示 |
| `assets/final_candidate/foraging/plants_sheet_flowers_cc0.png` | 花草采集物总表 | 用于野外拾取点和地表点缀 |
| `assets/final_candidate/foraging/mushroom_red_cc0.png` | 蘑菇采集物单体 | 用于林地采集焦点物 |
| `assets/final_candidate/mining/ore_gold_cc0.png` | 默认矿石识别图 | 用于采矿物和背包展示 |
| `assets/final_candidate/materials/block_texture_tilemap_16x16_cc0.png` | 作坊/建材主 tilemap | 用于木料、石料、干草等材料区 |
| `assets/final_candidate/storage/pallet_pile_cc0.png` | 仓储货盘堆主图 | 用于作坊和仓库后场 |
| `assets/final_candidate/storage/wooden_box_32x32_cc0.png` | 仓储木箱主图 | 用于农场和店铺储物点 |
| `assets/final_candidate/shop/bookcase_cc0.png` | 店内陈列架主图 | 用于杂货店或工坊陈列 |
| `assets/final_candidate/town_props/lamp_post_32x32_cc0.png` | 村镇路灯主图 | 用于街道识别和氛围照明 |
| `assets/final_candidate/town_props/open_sign_01_32x32_cc0.png` | 店铺营业牌主图 | 用于商店开业提示 |
| `assets/final_candidate/town_props/hanging_sign_01_32x32_cc0.png` | 店招悬挂牌主图 | 用于店面识别 |
| `assets/final_candidate/ui/menu_panel_anwar.png` | 默认菜单面板皮肤 | UI 主面板基准 |
| `assets/final_candidate/ui/menu_bar_anwar.png` | 默认菜单顶部条 | UI 标题条基准 |
| `assets/final_candidate/ui/hud_bar_anwar.png` | 默认 HUD 条 | HUD 主样式基准 |
| `assets/final_candidate/ui/button_start_anwar.png` | 默认开始按钮 | 主按钮样式 |
| `assets/final_candidate/ui/button_pause_anwar.png` | 默认暂停按钮 | 次级按钮样式 |
| `assets/final_candidate/ui/button_ok_anwar.png` | 默认确认按钮 | 确认操作按钮 |
| `assets/final_candidate/ui/icon_dollar_anwar.png` | 货币图标主图 | 商店和 HUD 货币识别图 |

## 当前接入优先级

1. `environment/`、`town/`、`interior/`
   - 先建立可走动空间和主要区域骨架
2. `crops/`、`items/`、`characters/`
   - 再补农场核心循环
3. `fish/`、`foraging/`、`mining/`、`materials/`、`storage/`
   - 继续补采集和作坊循环
4. `ui/`、`shop/`、`town_props/`
   - 最后统一 HUD、商店和交互表现

## 后续更新规则

1. 新增到 `assets/final_candidate/` 的图，必须同步登记到这里
2. 如果某张图被替换，优先更新本索引中的“预期用途”和“备注”
3. 如果后面出现 `主风格候选 B`，建议新增独立索引文件，不和这一份混写
