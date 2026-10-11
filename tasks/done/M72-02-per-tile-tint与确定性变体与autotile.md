# 任务：per-tile tint、确定性变体与 4-bit autotile

- 任务 ID：M72-02
- 状态：已完成（2026-10-08）
- 关联里程碑：M72（P0 引擎地基）
- 创建日期：2026-10-08
- 依赖：M72-01
- 阻塞：M72-04 → M72-05 → M72-06

## 目标

补掉"地面没有变体、地形交界是直线、没有 per-tile 颜色"这三条，让地表能画出
星露谷那种"相邻两块永不完全相同 + 不规则犬牙边"的质感。今天这三条**一条都没有**。

## 现状（实测）

- **变体 / autotile / per-cell UV 偏移：完全没有。** `tile_uv` 是存储值的纯函数，
  每个非空格恰好映射一个图集位置。
- `meta: BTreeMap` 存在（`tilemap.rs:38`）但**不序列化**（`:27-28`）且渲染器从不读 → 不能拿它承载。
- **per-tile tint 结构上已经免费**：顶点格式带 `color:[f32;4]`
  （`engine/crates/engine/src/host/frame.rs:22`（OverlayVertex）、`:56`（WorldSpriteVertex）），
  但 tilemap 侧把 tint 硬编码成不透明白：`runtime_scene/tilemap.rs:202`（overlay）、`:297`（world）。
- 合批：一层一个 `SpriteDrawBatch`，同贴图连续段合并（`:221-236` overlay、`:319-335` world），
  每 tile 6 顶点。
- 容量：`tilemap_stress.rs:37` 默认 100×100=10,000 瓦 / 30 帧；编辑器网格维度 clamp 1..=256
  （`inspector.rs:716,720`）。`manor` 是 40×23=920 格 → **加变体和 autotile 不碰性能红线**。

## 影响文件

- `engine/crates/rustlua-ecs/src/tilemap.rs`（逻辑瓦 → 图集索引的解析、变体哈希、邻居掩码）
- `engine/crates/engine/src/host/runtime_scene/tilemap.rs:202,297`（接 per-tile tint）
- `engine/crates/editor/src/panels/`（笔刷要能刷"逻辑瓦"而不是图集索引，否则美术没法画）
- `examples/game_jhyt/tools/pixel_kit.py` / 新增 `terrain_kit.py`（生成器侧要能出同构的 autotile 表，
  否则预览图与实机不一致）

## 设计要点

1. **两层语义分开**：格子里存「逻辑瓦 id」（草/土/石/水…），渲染时再由
   「邻居掩码 + 确定性哈希」解析成「图集索引」。直接把图集索引写进格子 = 美术要手工摆 16 个变体，
   等于没有 autotile。
2. **autotile 首版只做 4-bit 掩码 = 16 变体**（上/下/左/右四个邻居在不在）。
   **明确做不到**星露谷那种 47 变体全表与不规则犬牙岸线——那需要 8-bit 掩码或手工装饰层。
   岸线的不规则感由 M72-05 的 sprite 装饰层压边凑，不假装 autotile 能解决。
3. **变体哈希必须确定性**：`hash(cell_x, cell_y, layer_seed) % variant_count`。
   用 `std::collections::hash_map::DefaultHasher` 或任何带随机种子的实现都是错的——
   每帧抖一次，所有画面类门必红。种子要进组件串并序列化，编辑器与运行时读同一个值。
4. per-tile tint 首版只接两个来源：**季节/湿润的整体色调** 与 **阴影压暗**。
   不做逐格手涂（没有编辑器 UI 支撑，做了也没人能用）。

## 风险

- autotile 让"格子值"的语义变了 → M72-01 的 `.scene` 迁移与 M72-02 的解析层要划清边界，
  别把两件事写进同一个函数。
- 确定性哈希一旦进黄金表，改哈希函数 = 全图重排 = 画面类门全红。**哈希函数与种子要钉进门里的常量**。
- 编辑器笔刷若仍刷图集索引，美术会绕过 autotile 手工摆位，规范立刻失效。这条要在验收里显式测。

## 验收方式

- `jhyt_m72_tile_variant_determinism`：同一种子重跑 100 帧，逐帧取 tile→图集索引序列，断言**全等**。
- `jhyt_m72_autotile_mask`：造一块 8×8 草地嵌 4×4 土，断言 12 个交界格取到转角/T 形瓦、
  内部格不出现压边瓦。
- **反面对照**：把邻居掩码里的"上"一位去掉，门必须红在顶部那一排交界格上。
- per-tile tint：断言 `tint != [1,1,1,1]` 的格子在顶点色里真的带下去了（读 mesh 顶点，不看像素）。
- **反面对照**：把 `:202`/`:297` 的 tint 改回硬编码白，tint 断言必须红。
- 登记进 `verify.ps1` MVP 清单。

## 实现记录

- 状态：已完成（2026-10-08）。**引擎侧解算 + 三条门 + 跨宿主 parity**；数据侧启用属于 M72-04 起。
- 落地内容（按"谁说了算"分层）：
  - **组件层**（`engine/crates/rustlua-ecs/src/tilemap.rs`）：两层语义在此汇成一处
    `resolve_atlas_index(x, y)`——恒等档（`!autotile && variant_count<=1`）存储 id **就是**图集条目，
    旧图零改动；变体档 `entry = 1 + (kind-1)*span + variant_for(x, y)`；autotile 档
    `entry = 1 + kinds*span + (kind-1)*16 + mask%16`；容量装不下时解出 `0`（什么都不画）而不是
    静默取错格。`variant_for` = FNV-1a（`x,y,seed,MIX_AXIS_X,MIX_AXIS_Y` 逐字节混入）
    **再接 splitmix64 的雪崩收尾**取模；`autotile_mask` 的位序按 **index** 命名
    （bit0=`y-1`、bit1=`x+1`、bit2=`y+1`、bit3=`x-1`），越界当"没有邻居"，只有同 kind 的非零邻居才算。
    `register_tint` 幂等（同色复用同一 id，上限 255 色）、`tint_at` 未染色读回 `[1,1,1,1]`、
    载荷用稀疏两段 `tints=<cellIndex>:<tintId>,…` + `tints_rgb=r,g,b,a|…`。
  - **渲染层**（`engine/crates/engine/src/host/runtime_scene/tilemap.rs`）：chunk 展平时把
    `tilemap.tint_at(x,y)` 写进 `CachedTileSprite.tint`，**overlay 与世界深度两个赋值点各接一次**
    （`append_tilemap_layer` 与 `append_tilemap_layer_world`）；脏重建同时比较 `tiles` 与
    `tint_ids` 切片，因此 `clear_tint` 当帧生效。
  - **脚本层**（`engine/crates/rustlua-scripting/src/script/api/tilemap.rs`）：
    `atlas_index` / `autotile_mask` / `set_tint` / `clear_tint` / `tint` 把解算与染色暴露给门与
    数据脚本（`tilemap.get` 一并带出七个布局字段，跨宿主比对不必再猜）。
  - **编辑器层**（`engine/crates/editor/src/panels/tilemap_tools.rs`、`inspector.rs`）：
    画笔上界改由 `max_brush`/`clamp_brush` 按模式算——直接寻址档跟图集容量（96），
    逻辑瓦档跟 `tile_kinds`（刷到条目号等于绕过 autotile 手工摆变体，正是 §风险 3）；
    `tint_ids` 随 `tiles` 一起 resize，改网格维度不会错位。三条单测钉住
    （`brush_bounds_follow_the_atlas_layout_not_the_six_color_palette`、
    `brush_is_bounded_by_kind_count_in_logical_tile_mode`、`brush_labels_distinguish_kind_from_atlas_entry`）。
  - **第三宿主层**（`plugins/webgl/web-runtime.js`）：JS 解算与**内嵌 Lua 垫片**两份镜像都补上雪崩
    收尾（Lua 侧另需 `lshr` 做逻辑右移——`>>` 对负数补符号位，与 Rust 的 u64 零填充右移不是同一串位；
    末位取模也要 `u64_mod` 纠正，否则 `variant_count` 非 2 的幂时两侧各选各的变体）。
  - **预览工具层**（`examples/game_jhyt/tools/preview_scene.py`）：`variant_for` 同样补
    `_avalanche`，工具与实机预览的地表不再是点阵。
- 与计划不同 / 计划没写清的三处：
  1. §验收方式 只说"把掩码里的'上'一位去掉门必须红"。**实现里这条做成门内建的参考实现开关**
     （`jhyt_m72_autotile_mask` 的 `reference_mask(.., ignore)`），因为"红在哪一排"需要一份
     不参与解算的独立算式才能判定；另外补了"抹掉 `y+1` 顶行必须**不**红"与"互换两个列位顶行必须
     恰好 2 格"两条，防止门对某一位其实是瞎的。
  2. §影响文件 提到的 `tools/terrain_kit.py` **本包没有新建**：autotile 图集的**排版**（每种 kind
     占 16 格过渡块、块内顺序 = 掩码值）与地表色阶表是同一件事，落在 M72-04 的
     `gen_terrain_tiles.py` 一侧才有输入；本包只钉"运行时怎么解"，先造一个只有模板的生成器会
     在 M72-04 被重写。边界已写进 §未完成项。
  3. §现状 里的行号（`:202`/`:297`）随 M72-01 的改动已漂移；现分别为
     `runtime_scene/tilemap.rs:215`（overlay）与 `:310`（world），复现命令按函数名而非行号写。
- 本包踩到并修掉的三处实现细节（记下来是因为它们都会"看着绿"）：
  - 原始 FNV 直接取模在 `variant_count=4` 时第一行就是 `3,2,1,0,3,2,1,0` 的**周期点阵**：
    确定性有、质感没有。雪崩收尾不是美化，门为此自带一份**不做收尾**的参考实现并断言它就是那个点阵。
  - `SceneDraw.texture` 是 **1 基**（0 = 内建白图，见 `runtime_scene/frame_data.rs:816` 的 `+1`），
    而世界通道的 `SpriteDrawBatch.texture_id` 是 `paths` 的 **0 基**下标。门里两套编号混用过一次，
    表现是"世界通道一格都没画出来"——已按通道分别解析并在门内注释钉住。
  - autotile 的位序若按屏幕方位命名（把"上邻居"当 bit0），地图 row 0（在最底）与图集 row 0
    （在最顶）会指向相反方向，交界会整体错一个方向。三份实现（Rust/JS/Lua）与 Python 预览都用
    index 命名。

## 验收证据

全部读数取于 2026-10-08，工作目录 `engine/`（cargo）与仓库根（node / py）。

- [x] `cargo fmt --all -- --check` ⇒ 退出码 0（本批 `cargo fmt --all` 后复查，无输出）。
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings` ⇒ 退出码 0
  （文档回写之后**又跑了一遍**，`Finished dev profile in 34.97s`）。
  首跑红两条，都是本包门里的 `manual_contains`（`entries.iter().any(|e| *e == 0)` ⇒
  `entries.contains(&0)`），改写法而不是加 `allow`。
  日志里另有三条 `warning: <crate>:` 前缀的行是 build script 的 `cargo:warning=`（本机没有
  `rc.exe` 也没有 `engine/.tools/zig`，跳过 exe 文件图标），与 `-D warnings` 的 lint 口径无关，
  不会把门判红。
- [x] `cargo test -p rustlua-ecs --lib` ⇒ 151 passed / 0 failed，其中本包新增
  `variant_indices_are_pinned_by_the_hash_constants`（把 8×4 的 32 个变体序号写成金标
  `[[0,2,2,1,3,1,0,3],[3,2,2,0,2,1,3,2],[1,1,1,0,3,1,1,3],[3,1,2,0,3,2,1,1]]`，
  并内建"未混洗即点阵"的对照断言）与既有的 `autotile_mask_tracks_same_kind_neighbours`、
  `autotile_resolution_reaches_transition_blocks`。
- [x] `jhyt_m72_tile_variant_determinism` 绿（`cargo run -p game --example jhyt_m72_tile_variant_determinism`）：
  末行 `frames=100 cells=32 seed=0x12345678 raw_lattice=[3, 2, 1, 0, 3, 2, 1, 0] lua_mirror_cells=32`。
  读数：§① 32 格逐格等于金标（第一行 `0,2,2,1,3,1,0,3`）；§② 100 帧条目序列与 32 个 quad 的 UV
  序列逐元素全等（视口剔除关闭，且每帧复验 `scene.reload() == "main"`，防止"全等"其实是探针已不在场）；
  §③ 格子值 4 在恒等档条目全为 4，切 4 变体后落进块 13..16；§④ 4 个变体在 32 格里分布
  `[5,11,8,8]`（引擎非周期）而参考实现是周期 4 的点阵；§⑤ 换 seed 改 21/32 格、换回逐格复原；
  §⑥ 把 web 宿主的 Lua 镜像喂进真 VM 重解，32 格与引擎同值，镜像种子 +1 后 25/32 格与引擎不同。
- [x] `jhyt_m72_autotile_mask` 绿：末行 `cells=64 dirt_border=12 dirt_interior=4 mask_kinds=9`。
  读数：§① 64 格掩码逐格等于手写位序表，土块 16 格取到 9 种掩码
  `[3,6,7,9,11,12,13,14,15]`、四角 `[6,12,3,9]` 各不同；§② 内芯 4 格 `0b1111`、交界 12 格、
  草侧 16 格各**只缺指向土块那一位**、另 4 格纯内部草格 `0b1111`，满块瓦条目草 18 / 土 34；
  §③ kind 1 实测 6..=18（块界 3..=18）、kind 2 实测 22..=34（块界 19..=34），逐格满足布局式；
  §⑤ 地图四角 `0110/1100/0011/1001`；§⑥ 挖空 (0,0) 后本格条目 0、右邻 `1110→0110`、
  上邻 `0111→0110`，填回后 64 格条目序列与挖空之前逐格全等；§⑦ 64 格 UV 逐格等于
  「手写掩码 → 条目 → 12×8 子矩形」；§⑧ 垫片 64 格掩码与条目同值 + 全草图 36 个内部格读到
  `0b1111`/条目 18。
  口径修正一处：§② 首版统计的是"所有 3 位草格"（40 格），那把地图边的草格也算成"贴着土块"，
  数字读起来像证据其实是噪声；现按"缺的正好是指向土块那一位"逐格判，实测 16 格。
- [x] `jhyt_m72_tile_tint` 绿：末行 `cells=32 tinted=3 non_white=3 alpha_pinned=true`。
  读数：§① 载荷
  `tints=1:1,21:2;tints_rgb=0.72,0.84,1,1|0.94,0.78,0.42,0.35`（32 格只写 2 项，不是逐格定长串），
  同色二次注册复用 id、表长仍 1，往返后 32 格逐格同色；§② 3 格 × 6 顶点逐点等于注册色、
  非白格数 3、其余 29 格为白；§③ 世界深度通道 32 格全部带色；§④ `(5,2)` 六个顶点 alpha 全 0.35；
  §⑤ `clear_tint` 当帧回白、另两格颜色不动、非白格数 3→2，且 `tilemap.tint` 组件读回与顶点色同一份；
  §⑥ `(5,2)` 与 `(7,3)` 读出两种颜色（id 没在解算里并成一种）。
- [x] **反面对照：全部是真跑出来的红**（每次只改一处，跑完即还原；`git diff` 已确认还原干净）。
  | 破坏点 | 门 | 首个红点（原样读数） |
  | --- | --- | --- |
  | `variant_for` 去掉雪崩收尾（`hash % count`） | `jhyt_m72_tile_variant_determinism` §① | `第一行变体 = [3, 2, 1, 0, 3, 2, 1, 0]，金标是 [0, 2, 2, 1, 3, 1, 0, 3]`；`cargo test -p rustlua-ecs variant` 同步红：`variant_for(0,0) 漂了`（left 3 / right 0） |
  | `autotile_mask` 的 bit0/bit2 互换 | `jhyt_m72_autotile_mask` §① | `32/64 格的邻居掩码与手写表不符，(x,y) 引擎/预期 = [(0, 0, 3, 6), (1, 0, 11, 14), …]`；`autotile_mask_tracks_same_kind_neighbours`、`autotile_resolution_reaches_transition_blocks` 同步红 |
  | overlay 那一处 tint 改回硬编码白 | `jhyt_m72_tile_tint` §② | `格子 (1,0) 的第 0 个顶点色 = [[1.0, 1.0, 1.0, 1.0], …]，6 个顶点都应为 [0.72, 0.84, 1.0, 1.0]`（"非白格数 = 染色格数"这条同时归 0） |
  | **只**把世界通道那一处 tint 改回白 | `jhyt_m72_tile_tint` §③ | §② 仍绿（非白格数 3），§③ 红：`世界深度通道里 (1,0) 的顶点色 = [1.0, 1.0, 1.0, 1.0]，应为 [0.72, 0.84, 1.0, 1.0]` ⇒ 两个赋值点是分开被看着的，只接一条会红在另一条 |
  | web 的 Lua 垫片 `AVALANCHE_MUL_A` 末位改 1 | 门 §⑥ + node 门 | §⑥ 红：`(0,0) 引擎解出条目 1，web 的 Lua 垫片解出 2`；`node scripts/check-tile-hash-parity.mjs` 红：`Lua 垫片里 AVALANCHE_MUL_A 不是 -4658895280553007687` |
  | web 的 Lua 垫片 offsets 上下互换 | 门 §⑧ | §①–⑦ 全绿，§⑧ 红：`(0,0) 引擎给掩码 0110/条目 9，web 的 Lua 垫片给 0011/条目 6` |
- [x] **四份实现同值**（Rust / web 的 JS / web 内嵌 Lua 垫片 / `preview_scene.py`）：
  - 门内自动重测的两条：`jhyt_m72_tile_variant_determinism` §⑥ 与 `jhyt_m72_autotile_mask` §⑧
    ——它们**原样切出宿主发给 Lua 的那段文本**（起止暗号 `__rustlua_cell_index = function(m, x, y)`
    到 `tilemap = {`）喂进引擎真 Lua 5.4 VM，地图数据仍从引擎读（`tilemap.get` + 逐格 `get_cell`），
    所以判的是两份**解算实现**，不是两份抄本互相印证。
  - JS 那一份：仓库根 `node scripts/check-tile-hash-parity.mjs` ⇒
    `tile_hash_parity_ok js_cells=56 lua_constants=6/6 slots=16`（32 格变体金标 + 16 格土 +
    8 格草 pinned + 掩码/容量守卫；并把 Lua 垫片的 6 个带符号常数与 JS 的 BigInt 逐条对上）。
  - Python 那一份：仓库根
    `python -c "import sys;sys.path.insert(0,'examples/game_jhyt/tools');import preview_scene as p;print([p.variant_for([],8,4,x,0,0x12345678,4) for x in range(8)])"`
    ⇒ `[0, 2, 2, 1, 3, 1, 0, 3]`，与 Rust 金标第一行相同。
  - 本包早些时候还做过一次**离线全量对拍**（临时脚本，未入库）：从 `web-runtime.js` 切出 JS helper
    在 node 跑、把 Lua 垫片在真 Lua VM 跑，两侧对 60 行变体（1920 格）与 4 种布局（128 个条目）
    逐格 diff = 0 字节；`preview_scene.py` 与两侧逐行 0 处不同。入库形态即上面三条。
- [x] 三条门与 parity 门已登记：`scripts/verify.ps1` 的 MVP 清单
  （`jhyt_m72_tile_variant_determinism`、`jhyt_m72_autotile_mask`、`jhyt_m72_tile_tint`，
  紧跟 `jhyt_m72_tileset_atlas`）与 node 步 `Tile atlas hash parity (web JS mirror vs Rust golden)`
  （缺 node 时按本仓库口径打印跳过原因，不写成通过）。
- [x] `node --check plugins/webgl/web-runtime.js` 通过；L2 覆盖清单
  `node scripts/check-web-api-overrides.mjs` ⇒ `web_api_overrides_ok overrides=56 modules=11`
  （Lua 垫片只在既有覆盖点内加函数，没有新增整表替换）。
- [x] 文档回写两处：`docs/16-scene-format.md` 的 `Tilemap` 一节补上**容量对账式**
  （`tile_kinds * (max(variant_count,1) + 16*autotile) <= atlas_cols*atlas_rows`，恒等档占用记 0
  因而不检查）、解算越界返回 `0` 的口径、雪崩收尾为什么不是可选美化、tint 调色表三条边界
  （同色去重、上限 255、未染色读回 `[1,1,1,1]`），"相关的验证入口"列出三条门 + parity 脚本；
  `CHANGELOG.md` 新增 M72-02 一笔（症状 → 为什么必须把格子值分成两层语义 → 哈希为什么不能省
  混洗 → 4-bit 明确做不到什么 → 回滚档）。
- [x] `node scripts/gen-lua-api-docs.js --check` ⇒ `lua_api_docs_ok 再生成是 no-op、桩数未超标、
  无死绑定/重复注册`（`tables=42 funcs=290 stubs=275 documented=15`）。
  **这条开工时是红的，且红在本包之外**：报 17 个文件与再生成不一致 + 待补桩 `282 > 基线 275`。
  拆开是两笔债：① `282-275 = 7` 正好是 M72-01 新增的七个 `tilemap.*` API——`f89790f0` 更新了
  运行期快照 `lua_api_surface.json` 却没重跑生成器；② 另外 16 个模块的差异**只有**
  `绑定：<file>:<line>` 的行号漂移，来自 `684c01e7`（104 处 Lua API 借用改写）改了 `script/api/*.rs`
  的行数。两笔都在 HEAD 上：生成器的全部输入（`script/api/`、`lua_api.rs`、`lua_api/doc/`、
  `baseline.json`）在工作区都是干净的，本包未提交的改动碰不到它们。
  修法按门自己写的口径来——**补文档而不是放宽基线**：七个新函数逐条写清参数/返回/失败串与
  为什么这么设计（例如 `atlas_index` 的返回是 1 基条目号、越界返回 0 而不是报错；
  `tint` 返回 `{r=,g=,b=,a=}` 命名字段而不是数组），桩数落回 275 = 基线，再重跑生成器把行号
  漂移并进来。判别口径：`--check` 里"再生成是 no-op"才算闭环，只看桩数会漏掉 16 个陈旧文件。
- [x] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --lib` ⇒ 退出码 0。
  **首跑红，且红在本包自己的代码注释上**：
  `error: public documentation for 'variant_for' links to private item 'avalanche'`
  （`engine/crates/rustlua-ecs/src/tilemap.rs:283`）——intra-doc 链接指向私有函数，在
  `-D warnings` 下是错误而不是提醒。改成普通代码串（不建链接）后复跑通过。
  这条 clippy 抓不到（`cargo clippy` 两轮都绿），只有 `cargo doc` 抓得到；口径记下来：
  **M72 的门清单里 `cargo doc` 不是可省的一步**。
- [x] `node scripts/engine-pub-surface.mjs --check` ⇒ `pub_surface_ok items=2192 moved_into_submodules=0`。
  开工时红：`added=9 removed=1`。逐条归因——**8 条是本批的**：`AUTOTILE_SLOTS`、
  `DEFAULT_ATLAS_COLS` / `DEFAULT_ATLAS_ROWS`、`TILES_FORMAT_ATLAS` / `TILES_FORMAT_LEGACY`、
  `VARIANT_HASH_OFFSET` / `VARIANT_HASH_PRIME`、`TileNeighbor`（M72-01 的 `f89790f0` 加了公开符号
  却没跑 `--update`，M72-02 的 `TileNeighbor` 又叠了一层）；**另外 2 条不是**：
  `+ fn engine::core::project_config::release_flag` 与 `- constant …::RELEASE_TOML` 是同一件事，
  来自 `68ffcdef`（那次把 `RELEASE_TOML` 换成 `release_flag`，基线里却还留着旧名——
  `git show 68ffcdef:engine/crates/rustlua-infra/src/project_config.rs` 里已经没有 `RELEASE_TOML`）。
  跑 `--update` 后基线与 HEAD 相符，diff 只有 11 增 3 删、全部落在上面这 10 条上。
  顺手钉一条这道门的**能力边界**：它只记模块级路径，`impl Tilemap` 上新增的 `set_tint` /
  `register_tint` / `resolve_atlas_index` 等方法**不在符号面里**（新旧基线里 `tilemap::Tilemap::set_tint`
  都是 0 处），所以它抓不到"方法语义变了"，只能抓"符号被搬走或多/少一个"。
- [ ] **本批留一条仍然红的门，说清不是本包造成的**：`powershell scripts/check-docs.ps1` ⇒
  `issues=1`，红在 `examples/game_jhyt/docs/32-江湖有田-开发计划.md:43` 引用了不存在的
  `../scripts/README.md`（该文件从未写过）。**这一条 M72-01 §验收证据 已经登记过并归口 M72-13**
  （那时也是在 HEAD 的干净 worktree 里复算得到同一条 `issues=1`），本包不动别人的文档，
  也不把这条门写成通过。本包改口的两个文件（`docs/16-scene-format.md`、`CHANGELOG.md`）
  在同一道门里未报任何问题——判据是同一次运行里只有那一条 issue。

## 未完成项 / 已知限制

- **数据侧一条都没启用**：7 张场景的 `Tilemap` 载荷里没有 `autotile=1` / `variant_count>=2` /
  `tints=`（实测 `grep` 为空），游戏脚本里也没有 `set_tint` / `set_kinds` 调用。本包交付的是
  引擎能力 + 门；地表真正长出水田与林地的样子属于 M72-04（图集与色阶表）→ M72-05（散布）。
- `tools/terrain_kit.py` 未新建（见 §实现记录 与计划不同的一处 2）：autotile 过渡块的**排版**
  与 M72-04 的地表图集同源，边界在 M72-04 交付时一并钉；届时 `gen_terrain_tiles.py` 必须按
  `entry = 1 + kinds*span + (kind-1)*16 + mask` 与同一套位序出图，否则预览图与实机不一致。
- 4-bit 掩码做不到不规则岸线与 47 变体全表（设计内，见 §设计要点 2）；岸线的不规则感由
  M72-05 的 sprite 装饰层压边凑。
- 逐格手涂 tint **没有编辑器 UI**（设计内，§设计要点 4）：编辑器只做了解算与 `tint_ids` 随
  `tiles` 同步 resize，染色入口目前是 Lua `tilemap.set_tint` 与场景载荷。
- web 宿主的乘法染色是**近似**（`multiply` + `destination-in` 离屏合成模拟 per-vertex color），
  alpha 半透明在 web 与原生之间不保证逐像素相同；三宿主逐像素比对属于 M72-12。
- `variant_count > 1` 且 `autotile` 同时开启时，过渡块紧跟在**所有** kind 的变体块之后
  （`1 + kinds*span + …`）：容量按 `kinds*(span+16)` 校验，超出当场报错而不是画错。
  首版地表只需 `span=1`（kind 各占 16 格过渡块），两者叠加的图集尚未有美术输入。
