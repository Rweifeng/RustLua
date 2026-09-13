# 引擎能力缺口与执行 Backlog

- 文档版本：0.3
- 适用工程：`game_jhyt`（《江湖有田》）
- 基线依据：引擎 `E:\Codex\p2`（`engine/` workspace）源码实测 + `docs/18-lua-api.md` + 回归测试名，非历史声称
- 对齐记录：0.3 版（2026-09-07）以引擎 HEAD（`8f5bd5c`，此前含兼容层收口 `a7b43a1`）静态审计 + `game_jhyt` 侧 `jhyt_smoke` headless 验证为准，重新核对每个 ID 的真实状态
- 说明：本表是对 [32-江湖有田-开发计划 §4](32-江湖有田-开发计划.md) 缺口矩阵的修订版。B-2 的细化边界与落地顺序见文末「B-2 WorldSnapshot 缺口细化」。

## 状态标记

- ✔ 已具备（有代码 + 有测试/Demo 证据）
- 🚧 部分具备（能用但未达生产级）
- ✖ 缺失

## P0 —— 底座（不补无法推进 B/C 阶段）

| ID | 能力 | 状态 | 说明 / 实现要点 | 验收 |
|---|---|---|---|---|
| B-1 | **PersistId**（稳定实体 ID） | ✔ | 引擎侧已具备：`entity.spawn(name?, persist_id?)`、`entity.find_id(id)`、`entity.persist_id(e)`；独立于 `names` 的唯一 `persist` 命名空间（`ScriptState.persist`，`engine/.../core/script.rs`），spawn 重复 ID 且旧实体存活时报错，destroy/切场景自动清理。**边界**：场景实例化路径（`spawn_entity_components`）只注册 `names`，场景摆置实体暂不能带 persist_id；编辑器无 persist_id 显示（阶段 B 编辑器任务仍待补）。游戏侧 `scripts/player.lua` 已用 `"player"` 接入 | 同 ID 二次 spawn 报错；`find_id` 往返正确；destroy 后 `find_id` 为 nil；上述均已单测覆盖 |
| B-2 | **WorldSnapshot / 统一存档** | 🚧 | 引擎侧=版本化键值 `save.load/commit` + `__blob:*` JSON 兼容层（`save.blob_write/read`）。**注意（0.3 修正）：兼容层收口 `a7b43a1` 已移除 `decode_with_version`、`save.version()` 与 Lua `on_migrate(from,to)` 迁移钩子**——现存档是严格 `save_version=1` 门，旧档读入即 `warn + 拒绝（不覆盖原文件）`。运行时结构（背包/时钟/health）不会自动序列化，持久化是游戏侧责任。游戏侧已落地注册中心：`scripts/save_manager.lua` 聚合 state/inventory/clock 快照，`on_day_start` 里 `save.blob_write + save.commit`，`on_create` 里 `save.load + restore`。统一引擎级 `world.save/load` 入口仍缺失 | 30 天无漂移；100 次读档循环；旧档明确拒绝且保留原文件（引擎已满足）；跨会话恢复时钟/背包/装备/玩家经 `jhyt_smoke` 验证 |
| B-3 | **Tilemap 状态版本化** | ✔ | `tilemap.get_meta/set_meta` 单格 meta；`map.schema(name, spec?)` 结构校验，`map.state` 写入时报地图名与字段错误；`map_state_tilemap_snapshot_survives_scene_change_and_restart` 覆盖切图、存档提交、新运行时加载与 tile/solid/meta 恢复。约定：meta 不进场景文件，跨重启持久化依赖脚本写入 `map.state` | 切图/重启后 tile/meta/solid 一致（已验证） |
| B-4 | **A\* 网格寻路** | ✔ | `tilemap.find_path(id, from_x, from_y, to_x, to_y, diagonal?)` → 路径表（含两端）或 `nil, reason`；`PathError::{OutOfBounds, Blocked, Unreachable}`；四/八方向、禁穿角（对角需两个正交邻格可走）。**补记（0.3）：该能力在 docs/CHANGELOG 均未记载，实际已实现且有 4 个单测 + Lua 侧测试** | 障碍寻路/对角/端点遮挡/越界已单测覆盖；"最近可达点"留作后续 |
| B-5 | **通用 Lua 事件总线** | ✔ | `event.emit(name, data?)` / `event.on`·`subscribe`（别名）/ `event.off`·`unsubscribe(name, cb?)`（cb 缺省清空）；订阅表随 VM 生命周期（热重载后由 `on_create` 重订阅）；回调可重入 emit/订阅/取消（emit 先快照监听表） | 订阅/取消/重入单测覆盖；命名约定 PascalCase（`ItemAdded` 等） |

## P0 —— 战斗（阶段 C）

| ID | 能力 | 状态 | 说明 | 验收 |
|---|---|---|---|---|
| C-1 | **战斗 Hitbox / 攻击事件** | ✖ | 已就位地基：固定时间步（`FixedTimeStep` 60Hz 已接入主循环与编辑器预览，测试覆盖确定性）、裸数值 `health.get/set/damage`、通用 `physics.overlap/raycast`、通用事件总线。**缺失战斗系统层**：无 `combat` 命名空间、无 hitbox 生命周期、无攻击帧/命中去重、无击退/硬直/无敌帧；`AttackHit` 仅作为事件总线测试里的字符串。现有样板游戏的"攻击"是 Lua 侧一次性锥形距离判定（`examples/scripts/player.lua`） | 固定输入序列在 30/60/120 FPS 一致；同目标单次攻击只结算一次 |

### B-1 PersistId —— 实现与验收证据

- 引擎侧：`ScriptState.persist: BTreeMap<String, EntityId>`（`engine/crates/engine/src/core/script.rs`），与 `names` 相互独立；`entity.spawn` 第二参注册（重复且存活则 `api_error`），`entity.find_id`/`entity.persist_id` 正反查询，`destroy` 用 `retain` 清理，`process_scene_transition` 重置 world 时同步清空。
- 单测：`entity_persist_id_registers_looks_up_and_rejects_duplicates`（`cargo test -p engine` 覆盖 注册→`find_id` 往返→重复 spawn 报错→destroy 后为 nil）。
- 游戏侧：`game_jhyt/scripts/player.lua` 的 `M.ensure()` 用 `entity.find_id("player")` 恢复，缺失才 `entity.spawn("JianghuPlayer", "player")`，避免重复生成玩家。
- 编辑器/场景边界（待补）：场景内实体不带 persist_id；编辑器无该字段显示——列入阶段 B 编辑器任务，暂未交付。

## P0 —— 建筑（阶段 C/D）

| ID | 能力 | 状态 | 说明 | 验收 |
|---|---|---|---|---|
| D-0 | **建筑 footprint/占用** | ✖ | 网格 footprint，不重叠/不覆盖道路入口作物。引擎仅 Tilemap **单格** solid/meta 原语，无多格 footprint、放置预览、非法放置拒绝 | 放置预览 + 非法放置拒绝 |

## P1 —— 内容生产（阶段 D）

| ID | 能力 | 状态 | 说明 | 验收 |
|---|---|---|---|---|
| D-1 | NPC 日程模型 | ✖ | 时段计划数据 + 寻路执行器。引擎无 `npc`/日程概念，但有全部地基：A\* `find_path`（B-4）、`GameClock`、`grid.*` 坐标、`data.load/schema`；缺日程 schema 与"沿路径移动实体/按时钟切状态"的薄执行层 | 纯数据新增 1 个 NPC 无需改 Rust |
| D-2 | 对话 schema | 🚧 | **通用层已具备**：`data_schema::validate`（`data.schema` / `map.schema` 双入口）能给出文件+字段级错误（如 `farm.plots[1].x`），有 4 个单测。**对话领域模型为零**：无节点/条件/选项/变量副作用/关系变更运行时 | 数据错误定位到文件+字段（通用层已满足） |
| D-3 | 任务 schema | ✖ | 全引擎无 `quest`/任务状态机；仅可复用的 blob 存档 / 事件总线 / schema 校验原语 | 纯数据新增 1 条任务无需改 Rust |
| D-4 | 装备属性查询 | 🚧 | `inv.equip/equip_get/equip_list` **存储骨架已具备**（源码自注 "no runtime effect yet, stats land in M18-00"）；`ItemDefinition` 无伤害/防御等属性字段；武器伤害未真实生效 | 武器伤害真实生效 |
| D-5 | 关系/声望数据模型 | ✖ | 全引擎无好感/阵营/声望；仅通用 KV+blob 存档可兜底，无模型、范围校验或汇总 | 关系可存档恢复（需游戏侧自实现） |

## P1 —— 编辑器生产工具（阶段 D/F）

> 0.3 实测修正：E-1/E-2 已实现并有保存后重开回归测试，由 ✖ 上调。

| ID | 能力 | 状态 | 说明 |
|---|---|---|---|
| E-1 | 2D Collider 编辑器 | ✔ | Tilemap 碰撞层可视化编辑两条链路：Inspector 网格"画碰撞层"（`paint_collision`，碰撞格渲染 `■`）+ Scene View `TilePaint` 工具（左画右擦/拖拽连画/单条 History Undo）；`sample_project_tile_paint_persists_after_save_and_reopen` |
| E-2 | 2D Animation 编辑器（最小字段） | ✔ | Inspector `Animation` 分支纯 UI 调参：默认状态、Clip 名称/Sheet/列数/FPS/循环、增删 Clip、default 引用自动校正；`sample_project_animation_edit_persists_after_save_and_reopen` |
| E-3 | 大地图编辑 / Tilemap 批量操作 | 🚧 | 具备单格点画/拖拽连画/擦除/碰撞层/Undo；**缺批量**：无矩形填充/油漆桶/区域选择复制/autotile；网格尺寸上限 256×256，Inspector 预览仅 24×24 |

## P2 —— 体验与发布（阶段 G/H）

| ID | 能力 | 优先级 | 说明 |
|---|---|---|---|
| F-1 | 中文本地化基础（溢出/换行/排版检查） | P1 | 🚧：字体子集化（`font_subset.rs`）+ 字符收集（`char_collect`）已具备；**缺文本自动换行/溢出/截断/省略检查工具**（`UiText` 仅手动 `\n` 折行） |
| F-2 | UI 自动布局/滚动容器/键鼠手柄导航 | P2 | ✔（M20-02/M20-06/M20-07/M20-08）：自动布局（`UiRectLayout` + `ContainerKind` + `resolve_scene_ui_layout` + 布局缓存）、**焦点环与滚动状态** `ui_navigation::{FocusRing,ScrollState}`（Lua `ui.focus_*`/`ui.scroll_*`）、**手柄设备层**（XInput，按钮 0..15 / 轴 0..5 契约，死区可配置）、**命名动作映射**（36 个默认动作、`ActionMap` 持久化到 `settings.toml` 的 `[input]` 段）、**编辑器「项目设置 → 输入」重绑定面板**（捕获式改绑、跨动作冲突提示、死区滑块、恢复默认）。剩余：场景级 `Scroll` 容器变体仍缺（滚动由 `ui.scroll_*` + 面板自绘实现，见 `docs/18-lua-api.md`、`docs/33`、`docs/35`） |
| F-3 | 音频混音（分组音量） | P2 | ✔（M20-02）：`audio.buses/set_bus_volume/bus_volume/effective_volume`（实际音量＝主×分组）、`play_on(bus,name)`、`[audio]` 段随 `settings.toml` 持久化、暂停菜单「设置」页四路音量增减与保存；`game_jhyt` 已走 BGM/SFX/UI 总线（`docs/18-lua-api.md`） |
| F-4 | 发布原子替换 + 失败回滚 | P2 | ✔（M20-03）：`package.ps1` staging 优先构建 + `Complete-ReleaseSwap` 原子交换（旧内容整体重命名进 `.backup-*`）+ 失败回滚（旧版本原位恢复）+ 遗留 staging/backup 清理 + `-SkipBuild`；独立回归 `scripts/test-release-atomic.ps1` → `release_atomic_ok cases=4` |
| F-5 | WebGL 真实浏览器集成测试 | P2 | 🚧（M20-04）：已有**真实浏览器契约检查页**（`examples/game_jhyt/web/`）+ `scripts/verify-webgl.ps1` 无头 Edge 断言（manifest/启动场景/首资源/WebGL2/存储降级/缺失 404，实测 `webgl_browser_ok`）；**仍缺** WASM 模块构建与浏览器内完整玩法（本机离线无法安装 `wasm32-unknown-unknown`，见 `docs/32`） |
| F-6 | 微信小游戏 | 暂缓 | 仅预留声明（发布枚举/docs 项），无 `wx.*`/game.json 等实现分支，选中按 PC 处理——维持隐藏，不进 MVP |

## B-2 WorldSnapshot 缺口细化

### 引擎现状（0.3 实测修正，勿重复实现或误声称）

- **版本化键值存储**：`save.load/commit` 原子写盘，`save_version=1` 头；**损坏/版本不符被拒绝且不 panic、不覆盖原文件**。
- **任意值兼容层**：`save.blob_write/blob_read` 走 `__blob:<key>` → JSON 序列化；`lua_table_to_json` 对稀疏整数 key 表会退化为 object、读回后 key 变字符串——**游戏侧快照须稠密输出槽位**（`game_jhyt/scripts/inventory.lua` 已按此实现）。
- **迁移钩子已移除**：兼容层收口 `a7b43a1`（2026-09-06）删除 `decode_with_version`、`save.version()` 与 Lua `on_migrate(from,to)`。旧版本文档 0.2 声称"已交付迁移钩子"**作废**；`scripts/main.lua` 中的 `on_migrate` 已删除（死代码）。版本迁移改由游戏侧快照自带 `version` 字段处理（`save_manager.lua`）。
- **运行时结构不自动持久化**：背包、时钟、health 等是 `ScriptState` 上的 Rust 结构，不会进 KV 存档；持久化是游戏侧责任。

### 当前落地与剩余缺口（游戏侧为主）

1. **注册中心已落地**：`scripts/save_manager.lua` 聚合各模块的「快照提供器」（`register(name, capture, restore)`），读档按注册表整体恢复，`main.lua` 不越积越厚。
2. **触发/恢复路径已闭环**：`main.lua on_create` → `save.load + restore`；`on_day_start`（由 `day.sleep()` 或时钟自然跨午夜触发）→ `save.blob_write + commit`，落"清晨醒来"快照。
3. **当前快照域**：state（金钱/体力/内力/朝向）+ inventory（槽位稠密 + 装备槽）+ clock（day/minute）。玩家实体不入档，由 PersistId 恢复（`player.ensure()`）。
4. **仍缺失**：引擎级统一 `world.save/load` 入口（当前 blob 兼容层继续作为临时层使用）；NPC/建筑/装备属性/关系/任务等实体面随 D 阶段落地后追加；30 天长跑与 100 次读档循环用例未建立。
5. **删除临时分支的时机不变**：统一 `world.save/load` 落地并过验收后删除 `save.blob_*` 路径（32-开发计划回滚点）。

### 验收对照（0.3 更新）

| 验收 | 归属 | 当前状态 |
|---|---|---|
| 30 天无漂移 | B-2（阶段 B 验收） | 缺：需日历/作物跨天 smoke 长跑 |
| 100 次读档循环 | B-2 | 缺：无独立循环用例；`jhyt_smoke` 仅 1 次往返 |
| 旧存档迁移或明确拒绝 | B-2 | 明确拒绝 ✔（引擎 `save_version=1` 门，`warn` 后不覆盖）；游戏侧快照版本迁移由 `save_manager` 处理，尚未有跨版本用例 |
| 跨会话恢复时钟/背包/装备/玩家 | B-2（游戏侧） | 已具备：`jhyt_smoke`（`E:\Codex\p2\.../game/examples/jhyt_smoke.rs`）验证睡觉 → 存档 → 全新会话读档恢复 day/money/稀疏槽位/装备/PersistId 玩家 |
| 切图/重启后 tile/meta/solid 一致 | B-3 | 已具备：`map_state_tilemap_snapshot_survives_scene_change_and_restart` |

## 对齐记录（0.4，2026-09-13）

M19/M20 收尾后按实际落地能力复核（证据见 `tasks/done/M19-*`、`M20-*`）：

1. **状态上调**：F-2（自动布局 + 焦点/滚动导航 + 手柄设备层 + 命名动作 + 编辑器重绑定面板）由 🚧 → ✔（仅剩场景级 `Scroll` 容器变体）；F-3（音频分组总线）由 ✖ → ✔；F-4（发布原子替换与回滚）由 ✖ → ✔。
2. **状态更新**：F-5（WebGL 浏览器集成）由 ✖ → 🚧——真实浏览器契约检查与无头断言已通过（`webgl_browser_ok`），但 WASM 模块构建受离线环境阻塞，浏览器内完整玩法未执行（`docs/32`）。
3. **能力变更**：输入层从「物理键直读」升级为**命名动作**（`ActionMap` + `settings.toml` 的 `[input]` 段 + `input.action*`/`input.pad*` Lua API）；`game_jhyt` 全部脚本已迁移，键盘行为逐键等价（12 个 `jhyt_*` headless 回归全绿），手柄默认绑定覆盖 `confirm`/`cancel`/`ui_*`，其余动作由玩家在编辑器重绑定。
4. **新增已知限制（P2）**：发布包内改键写入 `<release>/pack/settings.toml`，重新发布会覆盖（需迁移到用户目录，`docs/35` §6）；天气恒为晴（`weather.set()` 无调用方）；真实手柄实机插拔未验证。

## 对齐记录（0.3，2026-09-07）

对引擎 `E:\Codex\p2` 源码与 `game_jhyt` 脚本做静态审计 + headless 验证后：

1. **状态上调**：E-1（Tilemap 碰撞编辑）、E-2（Animation 编辑）由 ✖ → ✔；E-3、F-1、F-2 修正为 🚧（局部已具备）；D-2、D-4 补记为 🚧（通用校验/存储骨架已具备，领域模型/属性效果缺失）。
2. **发现能力回退**：`a7b43a1` 移除存档迁移钩子链（B-2 相关），docs/18-lua-api 与旧版本文档过期，已在本表纠正。
3. **game_jhyt 修复**：`main.scene` Text 组件旧字段（`x/y/space/anchor`）违反 v2 场景规范会导致**启动即失败**，已改用 `UiLayout`；`scripts/main.lua` 补存档注册中心并删除死 `on_migrate`；`on_create` 设 `clock.set_rate(2)` 否则日期永不推进；`inventory.setup` 补注册 `data.tools`；快照稠密化规避稀疏 key 序列化坑。
4. **验证**：`jhyt_smoke`（临时 example，`E:\Codex\p2\engine\crates\game\examples\jhyt_smoke.rs`）通过：`main.scene` 实例化 4 实体、睡觉到 day2、全新会话读档恢复 day/money/种子/装备/稀疏槽位/玩家。该 example 属引擎仓临时文件，是否保留待定。

## 执行顺序（依赖固定）

```text
B-1 PersistId → B-4 A* → B-5 事件总线 → C-1 Hitbox → D-0 建筑占用
→ D-1/2/3/5 NPC·任务·对话·关系 → 垂直切片
→ E-* 编辑器工具（E-1/E-2 已具备） → F-* 发布/体验
```

> 注意：战斗 C-1 依赖 B-4（敌人追击）与 B-5（AttackHit 事件）；事件总线和 A* 应先于战斗落地。
