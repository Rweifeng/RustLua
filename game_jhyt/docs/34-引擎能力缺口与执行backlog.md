# 引擎能力缺口与执行 Backlog

- 文档版本：0.2
- 适用工程：`game_jhyt`（《江湖有田》）
- 基线依据：`engine/crates/game/examples/farm_smoke.rs`（M16-01~M16-12 已验收能力）+ 引擎源码现状
- 说明：本表是对 [32-开发计划 §4](32-江湖有田-开发计划.md) 缺口矩阵的**修订版**。原矩阵写于 M16 农场底座落地之前，把 "WorldSnapshot / Tilemap 状态" 列为 P0 缺失，实际已由 M16 交付，本表下修。B-2 的细化边界与落地顺序见文末「B-2 WorldSnapshot 缺口细化」。

## 状态标记

- ✔ 已具备（有代码 + 有测试/Demo 证据）
- 🚧 部分具备（能用但未达生产级）
- ✖ 缺失

## P0 —— 底座（不补无法推进 B/C 阶段）

| ID | 能力 | 状态 | 说明 / 实现要点 | 验收 |
|---|---|---|---|---|
| B-1 | **PersistId**（稳定实体 ID） | ✔ | 已具备：`entity.spawn(name?, persist_id?)`、`entity.find_id(id)`、`entity.persist_id(e)`；`names`（显示名）之外的独立唯一 `persist` 命名空间，spawn 重复 ID 报错，destroy/切场景自动清理 | 同 ID 二次 spawn 报错；`find_id` 往返正确；destroy 后 `find_id` 为 nil |
| B-2 | **WorldSnapshot** | 🚧 | 引擎现状=版本化键值 `save` + `__blob:*` JSON 临时兼容层（覆盖面=玩家属性+背包+时钟+`map.state` 分图状态）。游戏侧 `main.lua` 仅 `on_day_end` 存一表。待补=统一序列化入口 `world.save/load`、实体快照绑定 PersistId（B-1）、schema 版本化+迁移；NPC/建筑/装备/关系/任务等实体面随 D 阶段落地后追加 | 30 天无漂移；100 次读档循环；旧存档可迁移或明确拒绝 |
| B-3 | **Tilemap 状态版本化** | ✔ | 已具备：`tilemap.get_meta/set_meta` 存单格 meta；脚本可按版本化结构保存 tile/solid/meta 到 `map.state`；新增 `map.schema(name,spec?)` 结构校验，写入时报告地图名与字段错误；`save.commit/load` 支持跨重启恢复 | 切图/重启后 tile/meta/solid 一致 |
| B-4 | **A\* 网格寻路** | ✔ | `tilemap.find_path(id, from_x, from_y, to_x, to_y, diagonal?)` 返回 `{x,y}` 路径表（含两端）或 `nil, 原因`；A\* 四/八方向、禁穿角 | 障碍寻路/对角/端点遮挡/越界已单测覆盖；"最近可达点"留作后续 |
| B-5 | **通用 Lua 事件总线** | ✔ | `event.emit(name, data?)` / `event.on`·`subscribe(name, cb)` / `event.off`·`unsubscribe(name, cb?)`；订阅按 VM 生命周期（热重载后由 `on_create` 重订阅）；回调可重入 emit/订阅/取消 | 订阅/取消/重入单测覆盖；命名约定 PascalCase（`ItemAdded` 等） |

## P0 —— 战斗（阶段 C）

| ID | 能力 | 状态 | 说明 | 验收 |
|---|---|---|---|---|
| C-1 | **战斗 Hitbox / 攻击事件** | ✖ | `attack_begin/end`、hitbox 生命周期、命中去重、伤害/击退/硬直/无敌帧；伤害来源/目标/招式 ID/帧号写入事件 | 固定输入序列在 30/60/120 FPS 一致；同目标单次攻击只结算一次 |

### B-1 PersistId —— 实现与验收证据

- 引擎侧：`ScriptState.persist: BTreeMap<String, EntityId>`（`engine/crates/engine/src/core/script.rs`），与 `names` 相互独立；`entity.spawn` 第二参注册（重复且存活则 `api_error`），`entity.find_id`/`entity.persist_id` 正反查询，`destroy` 用 `retain` 清理，`process_scene_transition` 重置 world 时同步清空。
- 单测：`entity_persist_id_registers_looks_up_and_rejects_duplicates`（`cargo test -p engine` 覆盖 注册→`find_id` 往返→重复 spawn 报错→destroy 后为 nil）；persistent-entity 场景切换恢复逻辑已有独立测试。
- 游戏侧：`game_jhyt/scripts/player.lua` 的 `M.ensure()` 用 `entity.find_id("player")` 恢复，缺失才 `entity.spawn("JianghuPlayer", "player")`，避免重复生成玩家。

## P0 —— 建筑（阶段 C/D）

| ID | 能力 | 状态 | 说明 | 验收 |
|---|---|---|---|---|
| D-0 | **建筑 footprint/占用** | ✖ | 网格 footprint，不重叠/不覆盖道路入口作物 | 放置预览 + 非法放置拒绝 |

## P1 —— 内容生产（阶段 D）

| ID | 能力 | 状态 | 说明 | 验收 |
|---|---|---|---|---|
| D-1 | NPC 日程模型 | ✖ | 时段计划数据 + 寻路执行器（依赖 B-4） | 纯数据新增 1 个 NPC 无需改 Rust |
| D-2 | 对话 schema | ✖ | 节点/条件/选项/变量修改/关系变化 | 数据错误定位到文件+字段 |
| D-3 | 任务 schema | ✖ | 状态机（未接取/进行中/可交付/完成/失败） | 纯数据新增 1 条任务无需改 Rust |
| D-4 | 装备属性查询 | 🚧 | `item.define` 有，无属性计算 | 武器伤害真实生效 |
| D-5 | 关系/声望数据模型 | ✖ | 好感 0–10、四类阵营声望，存档+来源去重 | 关系可存档恢复 |

## P1 —— 编辑器生产工具（阶段 D/F）

| ID | 能力 | 状态 | 说明 |
|---|---|---|---|
| E-1 | 2D Collider 编辑器 | ✖ | 地图碰撞可视化编辑 |
| E-2 | 2D Animation 编辑器（最小字段） | ✖ | 动画调参不用改文本 |
| E-3 | 大地图编辑 / Tilemap 批量操作 | ✖ | 内容扩展成本控制 |

## P2 —— 体验与发布（阶段 G/H）

| ID | 能力 | 优先级 | 说明 |
|---|---|---|---|
| F-1 | 中文本地化基础（溢出/换行/排版检查） | P1 | 有字体子集化，缺布局校验 |
| F-2 | UI 自动布局/滚动容器/键鼠手柄导航 | P2 | 动态 UI 是保留模型 |
| F-3 | 音频混音（分组音量） | P2 | |
| F-4 | 发布原子替换 + 失败回滚 | P2 | package 目前直接写目录 |
| F-5 | WebGL 真实浏览器集成测试 | P2 | 有构建入口缺实测 |
| F-6 | 微信小游戏 | 暂缓 | 完成专用链路后再评估 |

## B-2 WorldSnapshot 缺口细化

### 引擎已具备（M16 交付，勿重复实现）

- **版本化键值存储**：`save.load/commit` 原子写，`save_version=<n>` 头，损坏/不支持的版本被拒绝且不 panic（`decode_with_version` 接受任意版本号供 `on_migrate`，无版本头视为 v0）。
- **任意值兼容层**：`save.blob_write/blob_read` 走 `__blob:<key>` → JSON 序列化；当前覆盖面=玩家属性 + 背包 + 时钟（`clock`）+ 分图状态（`map.state` → `__mapstate:<name>`）。
- **旧档迁移钩子**：`save.load()` 读到旧版本时自动调用全局 `on_migrate(from, to)`，可迁移或明确拒绝。
- **验证渠道**：`farm_smoke.rs` 的保存/读档循环（M16-09）：F1 写档 → 全新运行时 F2 读档，验证 `clock.now().day` 与 `save.version()`。

### 缺口与落地顺序（游戏侧为主，先不引入 schema 大爆炸）

1. **统一入口缺失**：游戏侧 `main.lua` 只在 `on_day_end` 里 `blob_write("jianghu_youtian.state", state)` 一表，未触发保存由谁、何时、何处恢复；也缺显式「读档后重建世界」路径。
2. **序列化面不统一**：玩家/NPC/建筑/装备/关系/任务各自为政。当前先落一条**注册中心**链：`save_manager.lua` 聚合各模块的「快照键」，读档按注册表整体恢复，避免 `main.lua` 越积越厚。
3. **实体面绑定 B-1**：只有 D 阶段出现 NPC/建筑/装备/关系/任务后，实体快照才绑定 PersistId（B-1 先落地）。当前只把**玩家实体 + 时钟 + 地图状态**列入快照域。
4. **地图状态**：`map.state` 属 B-3，先保证「切图/重启后 tile/meta/solid 一致」再进统一快照。
5. **正式版前删除临时分支**：`save.blob_*` 属临时兼容层（32-开发计划回滚点），统一 `world.save/load` 落地后删除。

### 验收对照

| 验收 | 归属 | 当前状态 |
|---|---|---|
| 30 天无漂移 | B-2（阶段 B 验收） | 缺：需日历/作物跨天 smoke 长跑 |
| 100 次读档循环 | B-2 | 缺：无独立循环用例；farm_smoke 仅 1 次往返 |
| 旧存档可迁移或明确拒绝 | B-2 | 部分：`on_migrate` 已有；未验证迁移结果 |
| 切图/重启后 tile/meta/solid 一致 | B-3 | 已具备：`map_state_tilemap_snapshot_survives_scene_change_and_restart` 回归测试覆盖 schema、切图、存档提交、新运行时加载和 tile/meta/solid 恢复 |

> 结论：B-2 的验收不在「补全引擎 Rust 序列化」，而在「补游戏侧统一存档路径 + 自动化回归用例」。

## 执行顺序（依赖固定）

```text
B-1 PersistId → B-4 A* → B-5 事件总线 → C-1 Hitbox → D-0 建筑占用
→ D-1/2/3/5 NPC·任务·对话·关系 → 垂直切片
→ E-* 编辑器工具 → F-* 发布/体验
```

> 注意：战斗 C-1 依赖 B-4（敌人追击）与 B-5（AttackHit 事件）；事件总线和 A* 应先于战斗落地。