# game_farm 批次 1 导入草案

这个文档用于把 `docs/RUNTIME_IMPORT_PLAN.md` 中的“批次 1：空间骨架”进一步细化成可执行草案。

它关注的是第一批真正要动手处理的资源，而不是全局策略。

## 批次目标

先把下面三类空间骨架准备成可进入运行时的第一批资源：

- 农场外部地形
- 小镇道路与建筑骨架
- 室内结构骨架

## 当前暂存目录

本批次已建立：

- `assets/runtime_batch1/`
  - 第一批接入暂存层
- `assets/runtime_batch1/environment_town/`
  - 农场地形与镇子骨架
- `assets/runtime_batch1/interior_space/`
  - 室内骨架
- `assets/runtime_batch1/environment_town.atlas`
  - 农场/镇子 atlas 草案
- `assets/runtime_batch1/interior_space.atlas`
  - 室内 atlas 草案
- `assets/runtime_batch1/tilesets/farm_town_tileset_runtime.png`
  - 外部空间 first pass runtime tileset
- `assets/runtime_batch1/tilesets/interior_tileset_runtime.png`
  - 室内空间 first pass runtime tileset

## 当前纳入的资源

### environment_town

- `assets/runtime_batch1/environment_town/farm_tileset_main_kenney.png`
- `assets/runtime_batch1/environment_town/grass_tile_anwar.png`
- `assets/runtime_batch1/environment_town/dirt_tile_anwar.png`
- `assets/runtime_batch1/environment_town/town_tilemap_kenney_pico8.png`

建议运行时用途：

- `farm.ground.main`
- `farm.ground.grass`
- `farm.ground.soil`
- `town.layout.main`

### interior_space

- `assets/runtime_batch1/interior_space/indoor_tileset_kenney_roguelike.png`
- `assets/runtime_batch1/interior_space/furniture_and_walls_sheet_cc0.png`

建议运行时用途：

- `interior.layout.main`
- `interior.props.reference`

## 当前引擎约束

根据工作区现有实现，当前 `Tilemap` 仍然偏向：

- 单张 tileset 纹理
- 按列选择 `1..=6` 的图块外观

这意味着本批次现在还处于“接入前整理”阶段，而不是“已经可直接喂给 Tilemap”的最终阶段。

换句话说，当前这 6 张图需要继续做一步“运行时化”处理：

1. 从候选大图中选出真正要进入 `Tilemap` 的小块
2. 把这些块重新排成符合当前引擎约束的 tileset
3. 保留不适合进入 `Tilemap` 的部分走 `Sprite` 路线

## 建议拆分方式

### A. 进入 Tilemap 的内容

优先考虑：

- 农场地表基础块
- 道路/墙体/地板等重复性高的块
- 室内主结构中的地板/墙面/边框类块

### B. 保留 Sprite 的内容

优先考虑：

- 摊位、门牌、装饰性强的建筑部件
- 家具和参考表中不规则、非重复的小物件
- 不适合塞进单列 1..=6 工作流的独立物件

## 当前建议的第一步输出

当前已经先生成了第一版原型输出：

1. `assets/runtime_batch1/tilesets/farm_town_tileset_runtime.png`
   - 6 列版本，面向当前 Tilemap 单纹理按列采样约束
2. `assets/runtime_batch1/tilesets/interior_tileset_runtime.png`
   - 6 列版本，优先保留更偏结构性的室内块

但它们当前仍然是 first pass：

- 还没有经过场景实机替换验证
- 还没有针对具体地图布局做二次删改
- 仍可能需要调整列顺序与具体切块

其余对象继续保留在独立 PNG 或 atlas 草案里。

## 批次完成判定

批次 1 可以视为完成，当满足：

- 已经选出农场/镇子/室内三类空间的最小 Tilemap 可用块
- 至少形成 1 套外部 tileset 与 1 套室内 tileset
- 对于不适合 Tilemap 的对象，已经明确走 Sprite 路线

## 下一步建议

下一步最直接的执行项是：

1. 从 `farm_tileset_main_kenney.png`、`town_tilemap_kenney_pico8.png`、`indoor_tileset_kenney_roguelike.png` 中人工挑出最小必需块
2. 用当前 first pass runtime tileset 去试着替换一个 `.scene`
3. 根据场景实际观感再继续重排和删列

当前已经新增：

- `scenes/batch1_tilemap_preview.scene`

这张场景用于把：

- `assets/runtime_batch1/tilesets/farm_town_tileset_runtime.png`
- `assets/runtime_batch1/tilesets/interior_tileset_runtime.png`

直接接进两个 `Tilemap` 组件里，做第一轮运行时验证。

当前这张预览场景也已经额外补了少量 `Sprite` 物件（如路灯、营业牌、书架），用于帮助判断：

- first pass tileset 是否足够支撑“外部院子 / 室内房间”的基础空间感
- 哪些内容应继续留在 `Sprite` 路线，而不强行塞进 Tilemap

当前混合样板又继续补了一批更明确的功能物件：

- 外部：摊位、货盘堆、木箱
- 室内：桌椅小物表、餐具表、书架

因此这张场景现在已经不只是“能否接通”的验证图，而是第一版 `Tilemap + Sprite` 分工样板。
