# M6-01：Tilemap

- 状态：ACCEPTED
- 验收日期：2026-08-21
- 所属里程碑：M6 2D 游戏能力与样板游戏
- 前置任务：M6-00 运行时场景实例化与玩家移动

## 目标

提供网格化地图能力：创建/保存/加载地图、可配置图块大小、碰撞层与视觉层分离、运行时渲染不逐图块提交独立 Draw Call。地图作为场景组件，与 M6-00 建立的"场景→World 实例化 + 共享 World + 批量渲染"管线一致。

## 背景

M6-00 让玩家实体可见且可 WASD 移动，但场景中还没有地图。样板游戏（俯视角 Roguelite）需要地图作为玩法地基。Tilemap 是主计划 M6 的第一个地图能力。

## 输入

- `engine/crates/engine_core/src/tilemap.rs`（新模块）
- `engine/crates/engine_core/src/script.rs`（Lua API 与场景实例化）
- `engine/crates/game_runner/src/main.rs`（渲染）
- `game/scenes/main.scene`、`game/scripts/main.lua`

## 不做什么

- 图块纹理图集（暂用调色板纯色块）
- 物理碰撞求解（solid 只作数据，碰撞解析属 M6-03 物理）
- 编辑器 tilemap 绘制工具
- 场景格式改动

## 技术方案

1. **数据模型**（`tilemap.rs`）：`Tilemap { tile_size: Vec2, width, height, tiles: Vec<u8>, solid: Vec<bool> }`——`tiles` 存视觉调色板索引（0=空，1..6=调色板），`solid` 独立存碰撞层。
2. **序列化**：`to_component_str`/`from_component_str` 用紧凑字符串（`tile_size=32x32;width=6;height=4;tiles=...;solid=...`），可随场景 round-trip 保存/加载。
3. **场景实例化**：`LuaRuntime::instantiate_scene` 解析 `Tilemap` 组件字符串并插入 World。
4. **Lua API**：`tilemap.get`（尺寸/图块大小）、`tilemap.is_solid`（碰撞查询）、`tilemap.get_cell`（视觉索引）。
5. **渲染**：`RuntimeScene::push_tiles` 把每个非空图块推入 SpriteBatch（统一 texture_id=0、layer=0），整张地图合成 1 个 draw range；精灵实体在 layer 1+ 绘制于地图之上。调色板统一为 `TILE_PALETTE`（script.rs 精灵调色板改为复用同一常量）。
6. **游戏工程**：main.scene 新增 `Tilemap` 实体（6x4、tile_size 32、带边框与一个橙色方块、solid 边框）；main.lua `on_create` 用 `tilemap.get` 打印地图尺寸，update 实时计算玩家所在格的 `is_solid` 并随返回值输出。

## 修改范围

- `engine/crates/engine_core/src/tilemap.rs`：新增（结构 + 序列化 + 调色板 + 4 个测试）
- `engine/crates/engine_core/src/lib.rs`：注册 `pub mod tilemap`
- `engine/crates/engine_core/src/script.rs`：实例化解析 Tilemap、tilemap Lua API、调色板复用 + 1 个集成测试
- `engine/crates/game_runner/src/main.rs`：`push_tiles` 渲染、批处理计数日志
- `game/scenes/main.scene`：Tilemap 实体
- `game/scripts/main.lua`：地图查询

## 验收标准（对照主计划 M6-01）

- [x] 创建、保存和加载地图：组件字符串 round-trip 测试 `round_trips_through_component_string`
- [x] 可配置图块大小：`tile_size=32x32`，Lua `tilemap.get` 返回 32.0
- [x] 碰撞层与视觉层分离：`tiles`（视觉）与 `solid`（碰撞）独立存储与序列化
- [x] 运行时渲染不逐图块 Draw Call：`items=19 draw_ranges=3`（17 图块 + 2 精灵，图块全合为 1 个 draw range）
- [x] 窗口运行：场景 4 实体、`tilemap loaded 6x4 tile_size=32.0`、实时 `cell=(1,2)false` 碰撞查询
- [x] 质量门：fmt / clippy -D warnings / 全量测试（109 engine_core + 48 editor）/ Debug / Release 全过

## 测试命令

```powershell
cargo test -p engine_core --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo run -p game_runner -- --project ../game --log-filter debug
# 地图画面视觉验收（窗口画面）待项目负责人人工确认
```

## 风险与应对

| 风险 | 应对 |
|---|---|
| 图块字符串过长难维护 | 紧凑字符编码（0-6/0-1），编辑器工具后续提供可视化编辑 |
| tile_size 矩形 vs 方形 | 结构支持 Vec2 矩形，Lua API 暂暴露 tile_size.x（方形为主） |
| 大量图块渲染 | SpriteBatch 统一 texture/layer 合批，验证 draw_ranges 远小于图块数 |
| solid 无碰撞解析 | 明确为非目标（M6-03 物理），当前只作数据查询 |

## AI 输出

已完成：Tilemap 模块、序列化 round-trip、场景实例化、Lua 查询 API、批量渲染、游戏工程地图与查询脚本。

验证结果：格式检查通过；静态检查通过；单元测试 160 通过 0 失败；Debug/Release 构建通过；窗口运行地图加载与碰撞查询已验证；批处理证据 `items=19 draw_ranges=3`；地图画面视觉验收待项目负责人。

## 已知限制

- 图块为纯色四边形（调色板），纹理图集未接入
- `tilemap.get` 只暴露 tile_size.x（方形图块），矩形图块完整信息后续补充
- 碰撞层无解析逻辑（M6-03 物理适配时接入）
- 无编辑器 tilemap 绘制工具

## 建议下一步

- M6-03 物理适配（Rapier）：把 `solid` 层解析为碰撞，让玩家/敌人被地图阻挡
- 或先补精灵/图块纹理渲染，替换调色板
