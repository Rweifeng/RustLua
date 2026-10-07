# 任务：tileset 多行多列寻址与 `.scene` 编码迁移

- 任务 ID：M72-01
- 状态：待办
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

（待回填）

## 验收证据

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] 8 张 `.scene` 重存后 `preview_scene.py` 合成图与迁移前像素级一致
- [ ] `jhyt_m72_tileset_atlas` 绿，且反面对照跑红
- [ ] `jhyt_m59`…`jhyt_m71` 八道黄金表逐道读数记录（绿/红+归因）
- [ ] 已登记进 `verify.ps1` MVP 清单
- [ ] `docs/16-scene-format.md` 与 CHANGELOG 已更新

## 未完成项 / 已知限制

（待回填）
