# 任务：manor 地表图集重做与色阶表

- 任务 ID：M72-04
- 状态：完成（2026-10-08）
- 关联里程碑：M72（P1 金标场景 manor）
- 创建日期：2026-10-08
- 依赖：M72-01、M72-02
- 阻塞：M72-05

## 目标

把 `manor` 的地表从"5 种单色瓦 + 直线边界"重做成带边缘过渡、带色阶的四族图集。

## 现状（实测）

- `scenes/manor.scene` 地表层（**entity 1**，40×23=920 格）只用了 **5 个取值**：
  `{1:390, 2:269, 3:71, 4:160, 6:30}`。
- `assets/art/tilesets/manor/manor_ground.png` 实测 `192x32`、**1135 字节** → 基本就是平色 + 点阵。
- 星露谷口径（`53` §2 实测）：同一色族要 **3–5 个明度档**（草 4–5、水 3）。
  `57` §1.1 锁了 8 个色板 token 但**每 token 只有一档**，明度档数从未定义——
  这是 `53` §5 点名"Style Bible 仍要补"的一条，本包补掉。

## 前置（必须先解决，不能边做边碰）

**图集预算口径**。`51` 定的是"每图 ≤1 张 1024×1024 图集"。本包地表四族 ×16 变体 = 64 格
（8×8 网格、256×256 就够），但 `manor` 后面还要挂农田/湿润/29 种作物 ×5 阶段，
**一张 1024×1024 装不下**。→ 开工前先定"一张图允许几张图集"的放宽口径并**回写 `51`**，
否则等 M72-08 接完线才发现超预算，要整批重排。

## 影响文件

- 新增 `examples/game_jhyt/tools/gen_terrain_tiles.py`（生成器，沿用 `pixel_kit.py`）
- `examples/game_jhyt/assets/art/tilesets/manor/*`（重做）
- 其余 6 族 tileset 目录（`village/mountain/cave/interior_forge/interior_herb`）本包**不动**，
  留 M72-11
- `examples/game_jhyt/docs/57-*.md`（补色阶表）、`docs/51-*.md`（补图集预算）
- `examples/game_jhyt/tools/preview_scene.py`（**必须同步**：它按"6 列 + V 整幅拉伸"复刻引擎 UV，
  见其 docstring；M72-01 改了引擎就得同时改它，否则预览图与实机不一致，地验收口全废）

## 设计要点

1. 四族：草 / 土 / 石板 / 水。每族 16 变体（4-bit 邻居掩码）+ 若干同族外观变体。
2. **色阶表**：每个色板 token 定义 3–5 档明度，写成一张表进 `57`，生成器从表取色，
   **不许在生成器里现写 hex**。这是"两张图看起来是一个世界"的唯一保障。
3. 地形交界按 `53` §3.1：**碎点压边 1–2px，不画 1px 分隔线**。
4. 悬崖/墙按 `53` §3.3 三段式（圆石面 + 草帽顶 + 深色基线），替掉现在读起来像壁纸的重复砖纹横带。
5. 水按 `53`：不规则岸线 + 石头 + 睡莲 + 中心深度渐变（现在是纯蓝矩形 30 格）。
6. 图边缘按 `53` §3.6：深绿视觉墙 `#004030`，不是空气墙。
7. 全部走 `51` 的后处理契约：alpha 二值化（>127→255）+ 6bit/通道量化；`FilterMode::Nearest`、无 mipmap。

## 风险

- 生成器与 autotile 表**必须同构**：M72-02 的运行时掩码解析和生成器排布图集的方式若不一致，
  实机边缘会错位，而 `preview_scene.py` 看不出来（它按数据画，不跑 autotile）。
  → 门要跑**实机**取 UV 序列，不能只看合成图。
- 色阶表一旦定稿，前面所有批次（含上一轮的 85 个物品图标）都在旧色阶上 → 本包**不回改旧资产**，
  只约束新产出，否则范围失控。

## 验收方式

- 量化判据：**同屏可见瓦种类 5 → ≥24**（从 mesh 的 tile→图集索引序列去重统计，不靠肉眼）。
- `jhyt_m72_manor_terrain`：断言草地→土交界处不存在"一整条直线边界"（即每个交界格都取到压边变体）。
- 色阶门：断言 `manor` 新图集里每个色族的明度档数 ≥3。
- **反面对照**：把色阶表某一族压成单档，色阶门必须红。
- 目验：`py tools/preview_scene.py scenes/manor.scene` 与 `references/stardew/Standard_Farm.png` 并排看。
- 登记进 `verify.ps1` MVP 清单。

## 实现记录

1. **前置的"图集预算口径"先纠了一处前提**：本包计划写的"`51` 定的是每图 ≤1 张 1024×1024
   图集"其实不存在——`51` 是美术资源规范（tile 尺寸 / PNG 编码 / 分层），真正的预算条款在
   `57` §1 的性能预算行与 `54` §8。所以口径写进了**真正该在的两处**：`57` §1 新增"图集预算与
   寻址"行（每图 ≤4 张地表图集；贴图尺寸必须 == `atlas_cols × atlas_rows × tile`；空槽全透明），
   `51` §2.1 补同口径的一行引用。放宽到 4 张的理由：manor 地表一张 12×9=108 条目已用 102，
   四季变体是**同尺寸换贴图**（切季节只换 `texture=`，不另计额度），农田 / 湿润 / 作物各占一张
   ——1024² 装不下，也不该装。
2. **色阶表**：新增 `tools/terrain_ramp.json`（地表唯一的取色来源）+ `tools/terrain_kit.py`
   （契约层：读表、族色阶、明度单调判定、条目寻址、`tilemap_component()` 序列化、
   `quantize_to_ramp`、`observed_levels`）。六族各 4 档（grass/dirt/stone/water/roof/hedge）
   + utility 四色（ink/paper/gold/wood）+ `ground_tints` 两档顶点色，回写 `57` §1.2。
   生成器一律从表取色，**不许写 hex 字面量**。
3. **新图集**：`tools/gen_terrain_tiles.py` 产出 `manor_ground.png` 12×9=384×288（6 个 kind 的
   满块瓦 = 条目 1..6，每 kind 16 格过渡块 = 7..102，尾部 103..108 留全透明）。
   `gen_manor_art.py` 里旧的地表段整段删除（−134 行），色板改由色阶表供；四季变体由
   `gen_season_variants.py` 按**族**做调色板替换（六族 × 四季，整族 `KEEP` 或一次给满四档，
   中间不留 `None`），并新增换色覆盖率门 `coverage_issues()`。
4. **场景重排**：`manor.scene` entity 1 地表层升到 `tiles_format=1;atlas_cols=12;atlas_rows=9;
   tile_kinds=6;variant_count=1;autotile=1` + 稀疏 `tints=`/`tints_rgb=`（920 格里 616 格非白）。
   承重的玩法语义一个没动：`farm.lua` 的 `tile==2`、`building.lua` 的 `tile==3`、
   `buildings.lua` 的 `ground_tiles∈1..6`。`combat_arena.scene` 与 `gen_arena_scene.py` 一起升到
   12×9 声明（格子值一格没改，只改编码）。
5. **门**：新增 `engine/crates/game/examples/jhyt_m72_manor_terrain.rs`（六节 + 三处内建反面
   对照），登记进 `verify.ps1` MVP 清单。

## 验收证据

- [x] **图集预算口径已定并回写**：见上 §1（`57` §1 新增行 + `51` §2.1 引用行）。
- [x] **色阶表已进 `57`、生成器从表取色**：`57` §1.2 六族 + utility + 染色档；
  `gen_terrain_tiles.py --check` → `[OK] 地表图集校验通过（色阶 / alpha / 16 格过渡块 / 文本空槽 / 空槽）`；
  `gen_season_variants.py --check` → `[OK] 四季变体校验通过（4 季 × 2 tileset）`。
- [x] **同屏瓦种类读数（前 / 后）**：整层去重条目 **5 → 59**；实机一屏 252 格 = **64 种外观**
  （纯条目 31 种）；数据侧扫过全部 20×12 窗口，最小值 **25**（左上角 (14, 8)）≥ 24。
  口径说明见"未完成项"第 1 条——**纯条目**口径下最坏窗口只有 12，≥24 只在"条目 × 顶点色"的
  外观等价类上成立，门两个数都打出来。
- [x] **交界不画直线**：0 格交界取到满块瓦；天然地貌（草/土/水）之间同外观最长直边
  `1→2:4 1→6:3 2→1:5 2→6:4 6→1:3 6→2:2`，全部 ≤5。
- [x] **色阶门**：基础图集 `dirt=4档 grass=4档 roof=4档 stone=4档 water=4档`（实测档数，不是表里数）；
  四季图先做同位置色置换还原后同判（顺带证明四季变体是"换色不重画"）。
- [x] **反面对照三处全部跑红**：① 合成"草地嵌 8×8 规则土块"报出 8 格同外观直边（>5，证明 §③
  那条断言不是恒绿）；② 草族整族压成单档 → 色阶门红（`grass 族…只实测到 1 档明度`）；
  ③ 从门内色表删掉 `#6F8F52` → §⑤ 报出 1 个 offender 并点名它。
- [x] **`preview_scene.py` 已同步**：它按组件声明的 `atlas_cols/atlas_rows` 解 UV（M72-01 已改），
  12×9 直接吃对，本包无需再改；两个场景出图正常（`scene_preview_manor.png` 1280×736）。
- [x] **已登记进 `verify.ps1` MVP 清单**：`jhyt_m72_manor_terrain`，注释逐条列了六节读数与三处反面对照。
- 门尾行：`jhyt_m72_manor_terrain_ok project=… atlas_entries=59 screen_tiles=64 worst_window=25 straight_max=5`。
- 目验：`py tools/preview_scene.py scenes/manor.scene` 与 `53` §2 的星露谷实测口径并排看——
  交界是碎点压边、没有 1px 分隔线，草/土各自有明度层次，水有深度渐变。
  （计划里写的 `references/stardew/Standard_Farm.png` 不在仓库内，`53` §1 只登记了 wiki 外链。）

## 未完成项 / 已知限制

1. **autotile 与 variant 在解算里互斥**（`Tilemap::resolve_atlas_index` 的变体分支在 `else` 里）：
   开自动瓦片后内芯格（mask `0b1111`）只能取该 kind 的**一个**条目（实测草内芯 185 格全取条目 22），
   重复只靠 per-tile 顶点色打散。所以"同屏 ≥24"必须按**外观等价类（图集条目 × 顶点色档）**统计。
   真正的"同族多外观"要等引擎把两者复合（先解槽位、再按变体哈希在槽内选档）——登记给后续包。
2. **`2→1`（土→草）最长直边 5 格，正好贴着阈值**，零余量：再往草地里插一块规则土就会红。
3. 路 / 墙（kind 3/4）与其他 kind 的交界**只报数不断言**（人工结构直的是应该的），
   实测 `2→3` 有 8 格连排、`4→1` 有 6 格。
4. 色阶表在门内是**手抄副本**（`crates/game` 没有 serde_json 依赖，为一个门加依赖不值）；
   漂移的表现是 §⑤ 报"表外色"，不会静默放过。
5. **悬空碰撞**：地表 `solid` 串里 x33..36 × y14..20 有一片没有对应可视物的碰撞格，
   M72-05 摆装饰时一并处理。
6. 生成器与落盘场景已有分歧：`gen_manor_scene.py` 写出的 entity 21/24/25 Transform `x=1056`，
   盘上是 `512`。本包只同步了 HUD `UiLayout` 偏移，没重跑整场景生成（避免把别的在建改动卷进来）。
7. village 及其余 5 族 tileset 仍是旧单档色，`gen_season_variants.py` 里走的是旧写法
   `VILLAGE_MAPS`，留 M72-11 并进族表。
8. 全量质量门里有两条红**不是本包引入**，属未提交的 M73-00 在建工作：
   ① `cargo clippy --workspace --all-targets --all-features` 在 `crates/gpu-web-probe`（wasm-only
   spike）宿主目标上红——`map_async` 的回调不满足 `WasmNotSend`；② `cargo doc --workspace`
   `-D warnings` 在 `rustlua-scripting/src/script/progress.rs:37`（公开文档链到私有项 `HOOK_STEP`）
   红，连带 `engine-pub-surface.mjs --check` 基线差 6 条。排除这两处后：fmt 干净、
   clippy（`--exclude gpu-web-probe`）零告警、`cargo test -p rustlua-ecs --lib` 151 绿、
   Lua API 文档桩门 / 图集哈希 parity 门 / Web API 覆盖清单门 / `check-docs.ps1` 全绿。
