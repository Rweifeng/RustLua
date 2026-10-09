# 任务：tileset 多行多列寻址与 `.scene` 编码迁移

- 任务 ID：M72-01
- 状态：已完成（2026-10-08，验收证据见文末）
- 关联里程碑：M72（P0 引擎地基）
- 创建日期：2026-10-08
- 上游：`examples/game_jhyt/docs/71-*.md` §4-A
- 阻塞：M72-02 → M72-04 → … → M72-11（整条场景线）

## 目标

让 tilemap 能寻址一张 N 列 × M 行的图集，而不是今天写死的 **6 列 × 1 行、V 恒拉伸**。
这是场景质感的天花板：不解开，重画多少瓦片图集都塞不进去。

## 现状（实测，非推断）

- `engine/crates/rustlua-ecs/src/tilemap.rs:91 tile_uv(column)`：`count = TILE_PALETTE.len()`，
  `start = (column-1)/count`、`end = column/count`，**y 恒为 `0.0..1.0`**。
- `tilemap.rs:9 TILE_PALETTE: [[f32;4];6]` 那个字面 `6` 就是列数真源。
- `tilemap.rs:111 set_cell` 与 `:446` 解析器都拒绝 `value > 6`。
- **`tilemap.rs:343 to_component_str` 每格只写一个 ASCII 数字**，`:404` 解析端 `to_digit(10)`
  → 序列化格式**表达不了 >9**，这是本包最贵的一块。
- `tiles: Vec<u8>`（`:34`）**本身有空间**（0..255），字段类型不用换。
- `solid: Vec<bool>`（`:35`）与瓦片身份无关；寻路只读 `solid`（`:174 find_path`、`:332 is_walkable`）
  → **不受影响**，不用动。
- 游戏侧所有 tileset 实测都是 `192x32`（6 列 1 行）。

## 影响文件

- `engine/crates/rustlua-ecs/src/tilemap.rs`：`tile_uv`、`TILE_PALETTE` 降级、`set_cell`/解析器上界、
  `to_component_str`/`from_component_str`、测试 `:504-514`
- `engine/crates/engine/src/host/runtime_scene/tilemap.rs:43 build_chunk`（传图集尺寸）
- `engine/crates/engine/src/host/runtime_scene/tests/tilemap.rs:16`（断言写死 `1.0/6.0`）
- `engine/crates/editor/src/panels/inspector.rs:734,739,763,791,798,812`
  （调色板 `0..=6`、`TILE_PALETTE[tile-1]`、`DragValue range(0..=6)`、`brush.min(6)` 去 clamp）
- `engine/crates/editor/src/panels/tilemap_tools.rs`（笔刷按图集网格出缩略图）
- `engine/crates/rustlua-scripting/src/script/api/tilemap.rs:130`（参数已是 `u8`，放开 Rust 侧校验即可）
- `examples/game_jhyt/scenes/*.scene`：**8 张全部重存**
- `docs/16-scene-format.md`：格式版本与编码说明要回写

## 设计要点

1. Tilemap 组件新增 `atlas_cols` / `atlas_rows`（或从贴图尺寸 + `tile_size` 推出，二选一，
   **倾向显式字段**：隐式推法会让"贴图里有几列"变成美术文件的隐式契约，改贴图尺寸就静默错位）。
2. 格子值从「列号 1..6」改成「图集索引」。**索引 0 仍表示空格**。
3. `.scene` 编码：单位数字 → **逗号分隔十进制**，并在组件串里带**格式版本号**。
   读旧格式要有迁移路径（`migrate_project` 通道），但注意：`migrate_project` **不改写任何组件载荷的
   规范形态**（实测过），所以 tiles 串的迁移得自己写，别指望它。
4. `TILE_PALETTE` 保留，但语义降级为"没有贴图时的调试色板"，不再充当列数真源。

## 风险

- **八道 UI 黄金表**（`jhyt_m59`…`jhyt_m71`）：理论上只比 UI 实体层、不碰 tilemap，
  但**必须逐道跑一次确认**。红了要能说出"为什么这次改动应当改变画面"，说不出就是 bug 不是门坏。
- 8 张 `.scene` 重存会产生巨大 diff，与并行会话撞车的概率高 → 这一笔要**单独成 commit**，
  不与任何美术改动混。
- 编辑器旧存档/缓存里可能存着旧格式 tiles 串（`examples/game_jhyt/.cache/`）。

## 验收方式

- **迁移正确性的唯一证据**：`M72-01` 落地后、任何美术改动之前，8 张 `.scene` 重存一遍，
  用 `py tools/preview_scene.py scenes/<name>.scene` 合成，与迁移前基线**像素级一致**。
  不一致就是编码迁移写错了，不是"美术顺手改了"。
- 新门 `jhyt_m72_tileset_atlas`：造一张 12×8 图集，逐格断言 UV 落在正确的 (col,row) 子矩形；
  断言索引 0 不产出 quad。
- **反面对照**（新门必须跑一次证明它会红）：把 `atlas_rows` 故意写成 1，门必须红在
  第二行的格子上——否则门对"多行"这件事其实是瞎的。
- `cargo test -p rustlua-ecs`、`cargo test -p engine --lib`（注意后者 HEAD 上有一条既有红：
  `sample_project_demos_cover_every_lua_api`，别算进本包）。
- 登记进 `verify.ps1` MVP 清单。

## 实现记录

- 状态：已完成（2026-10-08）。
- 落地内容（按"谁说了算"分层，不按文件罗列）：
  - **组件层**（`engine/crates/rustlua-ecs/src/tilemap.rs`）：新增 `tiles_format` /
    `atlas_cols` / `atlas_rows` / `tile_kinds` / `variant_count` / `autotile` /
    `variant_seed` / `tints` / `tints_rgb` 九个字段；`atlas_uv(index)` 按**声明的**行列切
    子矩形（不再 `V` 恒 `0..1`）；`max_tile_id()` 由布局算出；`validate_layout()` 负责
    "声明的布局装不下现有格子"这种事故当场报错；`to_component_str` 恒写 `tiles_format=1`
    与逗号编码，`from_component_str` 两种编码都读、**未知键报错**（`tile_kind=` 少个 s 不能
    静默忽略）。`TILE_PALETTE` 降级为"没有贴图时的调试色板"。
  - **schema 层**（`engine/crates/rustlua-ecs/src/component_schema.rs`）：九个字段全部
    `optional`，缺席即 6×1 / 无变体 / 无 autotile——旧串不会因为新增字段被判缺。
  - **渲染层**（`engine/crates/engine/src/host/runtime_scene/{tilemap.rs,mod.rs}`）：
    chunk 展平把图集尺寸带进去，`CachedTileSprite` 多一个 `tint` 分量并纳入相等性比较
    （chunk 缓存脏了要能被门看见）。
  - **脚本层**（`engine/crates/rustlua-scripting/src/script/api/tilemap.rs`）：新增
    `set_atlas` / `set_kinds` / `atlas_index` / `autotile_mask` / `set_tint` / `clear_tint`
    / `tint` 七个 API。
  - **编辑器层**（`engine/crates/editor/src/panels/{inspector,scene_view,tilemap_tools}.rs`）：
    笔刷上界从写死 `6` 改成按布局算（`max_brush`/`clamp_brush`），槽位标签在直接寻址档
    显示"第几列第几行"，调试色板对第 7 格以后给确定性色相而不是退回 6 号色。
  - **预览工具层**（`examples/game_jhyt/tools/preview_scene.py`）：按声明行列切图，并拿
    `atlas_cols*tile_size` 与 PNG 实际宽高**对账**，对不上直接报错。
  - **第三宿主层**（`plugins/webgl/web-runtime.js`）：JS 侧同步支持两种编码 + 按声明寻址，
    并加一份 Tilemap 解算的 **Lua 镜像**（`__rustlua_atlas_index` / `__rustlua_variant_for`
    / `__rustlua_autotile_mask` / `__rustlua_tint_at`）与逐帧 op 协议（`L` 布局行、`I` 染色行、
    `J` 清除行）；web 的乘法染色用 `multiply` + `destination-in` 离屏合成模拟。
  - **迁移工具**：`engine/crates/game/examples/tilemap_resave.rs`——只重写 `Tilemap` 那一行
    载荷，其余字节原样回写（含 CRLF），幂等。
- 与计划不同的一处：本包**没有**引入"从贴图尺寸反推列数"的备选实现，显式字段是唯一真源；
  理由已写进 `docs/16-scene-format.md`。
- 变体 / autotile / 逐格染色的**引擎侧解算**在本包一并落地（编码与布局不分家：布局没定，
  逻辑瓦 kind 无处可写），但**数据侧启用**与 per-tile tint 进门属于 M72-02。

## 验收证据

全部读数取于 2026-10-08，工作目录 `engine/`（cargo）与仓库根（node / py / powershell）。

- [x] `cargo fmt --all -- --check` ⇒ 退出码 0（无输出）。
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings` ⇒ 退出码 0。
  第一次跑并不干净，四条红都是本包改动引出来的或就在改动面上，逐条改到位而不是加 `allow`：
  ① 新门 `crates\game\examples\jhyt_m72_tileset_atlas.rs:330` 的
  `Result<Vec<([f32;2],[f32;2])>, …>` 触发 `type_complexity` ⇒ 抽 `type UvQuad`；
  ② 同文件 `path_of<'a>` 的显式生命周期可省略 ⇒ 改成 `&SceneMesh -> Option<&str>`；
  ③ `crates\editor\src\panels\tilemap_tools.rs:89` 的 `palette_len` 只有测试在用，
  `--all-targets` 下在 bin 里是死代码 ⇒ 删函数，测试改断言 `max_brush(..)+1 == 97`；
  ④ 同文件 `0.618_033_988_7` 是 f32 表达不了的精度 ⇒ 截到 `0.618_034`（数值不变）。
  顺手一条**既有**红（非本包引入，`git diff --stat HEAD` 对该文件为空）：
  `crates\game\examples\jhyt_m36_economy_sim.rs:120` 的 `manual_range_contains`，
  改成 `!(PROFIT_MIN..=PROFIT_MAX).contains(&profit)`。
- [x] **迁移正确性 = 像素一致**（不是"看起来一样"）：
  `py tools/preview_scene.py scenes/<name>.scene`（在 `examples/game_jhyt/` 运行）在
  编码迁移前后逐字节相同。7 张带 `Tilemap` 的场景全部命中：

  | 场景 | 迁移前 SHA-256(前16) | 迁移后 | 换用旧工具复算 |
  | --- | --- | --- | --- |
  | cold_spring_cave | `bd9de798cba62c0f` | 同 | 同 |
  | combat_arena | `342399500d0054c2` | 同 | 同 |
  | interior_forge | `a317a2d99f9e73e0` | 同 | 同 |
  | interior_herb | `62edebc0ad9bbf4a` | 同 | 同 |
  | manor | `51ccac7c35a25514` | 同 | 同 |
  | mountain_path | `e77e3b80d2dcac71` | 同 | 同 |
  | village | `4dff851acc636828` | 同 | 同 |

  7/7 一致。**注意**：计划里写的"8 张全部重存"实测是**8 张场景里带 `Tilemap` 的 7 张**——
  `scenes/main.scene`（标题屏）没有任何 Tilemap 组件，不重存是正确的，不是漏。
  复算列是把同一批 `.scene` 交给**迁移前**的 `preview_scene.py`（`.temp/m72-01-oldtool/`）
  合成：工具与数据两侧都换过，两边仍出同一张图，才算钉住"编码迁移没改语义"。
- [x] `jhyt_m72_tileset_atlas` 绿：
  `cargo run --quiet -p game --example jhyt_m72_tileset_atlas` ⇒
  `jhyt_m72_tileset_atlas_ok … probe_quads=18 legacy_v_stretched=true manor_cells=920`。
  内建两处反面对照（§③）：`atlas_rows` 改回 1 后条目 13 的 V 越出 `0..1`（证明 §① 的 UV
  断言对"多行"不是瞎的——旧实现的 `V=0..1` 恰好也在 `0..1` 里，只比"落在合法区间"抓不到），
  以及运行时 `tilemap.set_atlas(id,12,1)` 必须报错点名越界那一格。
- [x] **外置反面对照**（把门本身跑红一次，证明它真的在读发布数据）：把工程整份复制到
  临时目录，只把 `scenes/manor.scene` 地面那一行 `atlas_cols=6` 改成 `atlas_cols=2`
  （载荷其余字节不动），再拿这份工程跑门 ⇒ 退出码 1，红在 §⑤：
  `Lua API error: entity "GroundMap" has an invalid Tilemap: tile cell index 4 is out of atlas range`。
  对照组：同一份门指真工程 ⇒ 退出码 0。
- [x] 八道黄金表逐道读数（`cargo run --quiet -p game --example <name>`，全部指真工程）：

  | 门 | 退出码 | 读数 |
  | --- | --- | --- |
  | `jhyt_m61_prefab_runtime` | 0 | ok |
  | `jhyt_m62_prefab_write` | 0 | ok |
  | `jhyt_m63_title_prefab` | 0 | ok |
  | `jhyt_m65_shop_prefab` | 0 | ok |
  | `jhyt_m66_pause_prefab` | **1** | 归因见下 |
  | `jhyt_m67_inventory_prefab` | 0 | ok |
  | `jhyt_m68_quest_prefab` | 0 | ok |
  | `jhyt_m69_shipping_prefab` | 0 | ok |
  | `jhyt_m70_hud_prefab` | 0 | ok |
  | `jhyt_m71_dialogue_prefab` | 0 | ok |

  `jhyt_m59_*` 已在 M71 退役，不在名单里。
  **`jhyt_m66_pause_prefab` 的红与本包无关，且就在 HEAD 上**：它红在黄金表
  `engine/crates/game/examples/support/jhyt_m66_pause_golden.tsv:278` 写着
  `工程 settings.toml 的 [input] 段可改绑…`，而文案真源
  `examples/game_jhyt/scripts/ui/pause_menu.lua:138` 写的是 `project.toml`。
  两个文件在工作区都**未被修改**（`git status` 对它们为空，最后一起改在 `68ffcdef`），
  所以这是一份陈旧黄金表：`settings.toml → project.toml` 改名时没重 bless。
  本包不顺手 bless 别人门里的表（改的是"期望"而不是"实现"，会把真实的文案漂移洗掉），
  登记给 M72-13 台账收口。
- [x] `cargo test -p rustlua-ecs` ⇒ 150 passed / 0 failed。
- [x] `cargo test -p engine --lib` ⇒ **168 passed / 1 failed**，唯一一条红是
  `host::runtime_scene::tests::custom_fonts::text_render_mode_forces_vector_or_sdf_for_custom_fonts`
  （`custom_fonts.rs:157` `assertion failed: sdf.vector_vertices.is_empty()`）。
  **已证为 HEAD 既有红**：在 `HEAD`（`2b2eb5c7`）另开的干净 worktree 里、用独立的
  `CARGO_TARGET_DIR` 单跑这条测试 ⇒ 同样 panic。本包的改动面与文字/SDF 路径无交集
  （`git diff --name-only HEAD` 不含 `renderer/`、`text`、字体或 shader 任何文件）。
  计划里预言的 `sample_project_demos_cover_every_lua_api` **不是既有红，而是本包欠的账**：
  新增七个 API 后它精确报缺这七个 ⇒ 已补演示
  （`examples/game_api/scripts/demo_tilemap.lua`：菜地 `set_atlas(6,1)` +
  `set_kinds(6,1,false)` 声明恒等档、`plant()` 逐格 `set_tint`、`harvest()` 逐格
  `clear_tint`、状态行每帧读 `atlas_index` / `autotile_mask` / `tint`），
  现在 `cargo test -q -p engine --lib sample_project` ⇒ 11 passed / 0 failed。
- [x] 已登记进 `verify.ps1` MVP 清单：`scripts/verify.ps1:1057` `"jhyt_m72_tileset_atlas"`
  （带注释说明它钉什么、反面对对照什么）。
- [x] `docs/16-scene-format.md` 与 CHANGELOG 已更新：新增「瓦片映射 `Tilemap`（M72-01 / M72-02）」
  一节，含字段顺序、两种 `tiles` 编码表、图集寻址公式、恒等档与逻辑瓦档的分界、
  变体哈希与掩码位序的"为什么"、`tints`/`tints_rgb` 的稀疏存储理由，以及三条验证入口命令。
- [x] 第三宿主（web）与引擎的解算一致，量法是**三方对拍**而不是"看着一样"：
  Rust（真源 `from_component_str` + `resolve_atlas_index`）== Lua 镜像（跑在真 Lua 5.4 VM）
  == JS 镜像（跑在 node），15 张随机地图 / 750 格，覆盖恒等档、纯变体档、纯 autotile 档、
  autotile+变体档，图集 12×8 ⇒ `mismatches = 0`。变体序号另跑 1600 格
  （`variant_count` ∈ 2,3,4,5,6,7,8,11,16,100）⇒ 0 处不一致；这一步是**必须**的：
  Lua 的 `%` 是带符号 floor-mod，与 Rust 的 u64 mod 在 `n ∤ 2^64` 时不同，
  镜像里补了 `u64_mod`（用 `2^64 mod n` 修正），不补就直接错位。
- [x] API 台账同步：`cargo run -q -p engine --release --example dump_lua_api -- --write`
  重生成桌面基线（42 表 / 290 函数，含七个新 API）；
  `node scripts/check-web-api-overrides.mjs` ⇒ 退出码 0，读数
  `overrides=56 modules=11 wholesale=24 dropped=72 invented=3 referenced=6`；
  `plugins/webgl/lua_api_overrides.json` 里 `tilemap` 条目补了本包的账目说明，
  并补上 M71 遗留的 `prefab` 整表替换条目（走 JS 桥 `__rustlua_prefab_*`，`dropped=0`）。
- [x] L3 行为契约门：`cargo test -p rustlua-scripting api_surface` ⇒ 7 passed。
  这条在 Windows 上会**假红**：门把 `include_str!` 读到的工作区文件与当场输出逐字符比，
  生成侧写 `\n` 而 `core.autocrlf=true` 的克隆检出成 CRLF。已用
  `.gitattributes` 给 `engine/crates/rustlua-scripting/src/api_surface_snapshot/*.txt`
  钉 `text eol=lf`（只钉快照目录，不开 `* text=auto`，避免把 `scripts/*.ps1` 的
  UTF-8 BOM + CRLF 口径改成满屏脏 diff），并用 `run_lua_contract -- --write` 重生成。
- [x] `scripts/check-docs.ps1` 一条真 bug 顺手修掉：链接扫描的正则
  `\]\(([^)\s]+)\)` 对 CommonMark 合法的 `](<含空格路径>)` 会把 `<…>` 一起当路径喂给
  `Test-Path`，Windows 上抛 `ArgumentException`，而脚本开头是 `$ErrorActionPreference="Stop"`
  ⇒ **后面的索引可达 / 历史路径 / UTF-8 三类检查整段没跑**，输出只剩一条异常，看着像"就这一个问题"。
  现在识别尖括号目标并对非法路径按"链接不可解析"记账，不再崩门。
  修完后的读数：`issues=1`，唯一一条是
  `examples\game_jhyt\docs\32-江湖有田-开发计划.md: 相对链接指向不存在的路径 -> ../scripts/README.md`，
  **HEAD 上同样成立**（在 `2b2eb5c7` 的干净 worktree 里跑 HEAD 版脚本，
  得到同一条 `issues=1`；`examples/game_jhyt/scripts/README.md` 从来没写过）。
  这条红登记给 M72-13（台账收口），不在本包补别人的文档。

## 未完成项 / 已知限制

- **`examples/game_api/scenes/*.scene` 没有重存**：那批演示工程场景仍是 `tiles_format=0`
  旧编码（值域 `0..6`，读侧完全兼容）。判据只覆盖随包发布的 jhyt 七图；game_api 要等它
  自己的门或某次真改动一并迁移，现在动它只会把 diff 变大而证据不变。
- **变体 / autotile / 逐格染色在发布数据里还没有真实启用者**：本包落地的是解算与编码地基，
  仓库里所有 tileset 仍是 6×1。启用与门（`jhyt_m72_tile_variant_determinism`、
  `jhyt_m72_autotile_mask`）在 M72-02，美术数据在 M72-04 起。
- **web 的乘法染色只对二进制 alpha 的图集是精确的**：用 `multiply` + `destination-in`
  重采样模拟，`a<1` 时再补一次 `destination-in`。半透明图集在 web 上与 PC 顶点色会有
  边缘差——M72-12 三宿主目验要按这条口径看，不要当渲染 bug 报。
- **离屏画布在三个宿主上取法不同**：`OffscreenCanvas` → `document.createElement('canvas')`
  → `wx.createCanvas()`，而微信适配器返回的 stub 没有 `getContext`，且**第一次**
  `wx.createCanvas()` 返回的是主画布本身——必须显式拒绝 `canvas === mainCanvas`，
  否则染色会把屏幕自己吃掉。
- 变体哈希的四个常量（`VARIANT_HASH_OFFSET` / `VARIANT_HASH_PRIME` / `MIX_AXIS_X` /
  `MIX_AXIS_Y`）现在是**跨宿主契约**：Lua 镜像与 JS 镜像各自手写了一份，改任何一个
  等于全图重排，三处必须同步（门只保证解算一致，不保证你记得改另两份）。
- **`f89790f0` 更新了运行期 API 快照与对外符号面却没跑两个基线生成器**：新增七个 `tilemap.*`
  API 让待补文档桩从基线 275 涨到 282，`node scripts/gen-lua-api-docs.js --check` 当场红；
  同一批公开的常量/枚举（`AUTOTILE_SLOTS`、`DEFAULT_ATLAS_*`、`TILES_FORMAT_*`、`VARIANT_HASH_*`）
  又让 `node scripts/engine-pub-surface.mjs --check` 红在 `added=8`。两笔债都在 M72-02 还清
  （七个函数的说明补上、桩数落回 275，16 个模块 `绑定：<file>:<line>` 的行号漂移再生成进来；
  符号面基线跑 `--update` 到 2192 项），读数与逐条归因见
  `tasks/plan/M72-02-per-tile-tint与确定性变体与autotile.md` §验收证据。
  **口径补一句**：改 `.scene`/渲染之外的公开 Rust 符号时，`cargo doc`、`gen-lua-api-docs.js`、
  `engine-pub-surface.mjs` 这三道是连着的，只跑 clippy + test 会漏。
- `jhyt_m66_pause_prefab` 的陈旧黄金表、`docs/32` 的悬空 `../scripts/README.md` 链接、
  `custom_fonts` 的 SDF/vector 分流断言，三条既有红都登记在 M72-13 收口，见上表归因。
