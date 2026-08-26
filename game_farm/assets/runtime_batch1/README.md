# 批次 1 运行时暂存层

这里存放的是 `docs/RUNTIME_IMPORT_PLAN.md` 中“批次 1：空间骨架”对应的首批接入暂存资源。

用途：

- 作为第一批正式接入运行时资源前的单独工作区
- 把“空间骨架”相关资源从 `assets/final_candidate/` 再收敛到更小范围
- 方便后续做图集拆分、命名、注册和验证

当前分组：

- `environment_town/`
  - 农场地形与镇子骨架相关资源
- `interior_space/`
  - 室内骨架相关资源
- `tilesets/`
  - 第一版面向当前引擎约束的 runtime tileset 输出

注意：

当前引擎的 `Tilemap` 仍然偏向“一张 tileset 纹理 + 按列采样 1..=6 外观”的工作流。

这意味着这里的图片虽然已经是“批次 1 候选”，但还不是可以直接无损塞进当前 `Tilemap` 的最终格式，后续仍需要：

1. 从大图中抽取或重排真正要进运行时的 tile
2. 决定哪些内容走 `Tilemap`，哪些内容继续走普通 `Sprite`
3. 把最终运行时命名和注册规则落到资源数据库

配套文档：

- `docs/RUNTIME_IMPORT_PLAN.md`
- `docs/BATCH1_IMPORT_SPEC.md`

当前已生成的第一版 runtime tileset：

- `tilesets/farm_town_tileset_runtime.png`
  - 外部空间 first pass，包含草地、泥地与 4 列镇子结构候选
- `tilesets/interior_tileset_runtime.png`
  - 室内空间 first pass，包含墙面、地板和室内分区候选

这两张图目前的定位是：

- 验证“6 列 Tilemap 工作流”是否能承载当前空间骨架
- 给后续场景替换和 tileset 二次重排提供起点
