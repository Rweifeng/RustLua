# 引擎能力缺口与执行 Backlog

- 文档版本：0.3
- 适用工程：`game_jhyt`（《江湖有田》）
- 基线依据：引擎 `E:\Codex\RustLua`（`engine/` workspace）源码实测 + `docs/18-lua-api.md` + 回归测试名，非历史声称
- 对齐记录：**最新 0.9（2026-09-15 / M31）**，历史各版见文末「对齐记录」；0.3 版（2026-09-07）以引擎 HEAD（`8f5bd5c`，此前含兼容层收口 `a7b43a1`）静态审计 + `game_jhyt` 侧 `jhyt_smoke` headless 验证为准，重新核对每个 ID 的真实状态
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
| D-4 | 装备属性查询 | 🚧 | `inv.equip/equip_get/equip_list` **存储骨架已具备**（源码自注 "no runtime effect yet, stats land in M18-00"）；引擎的 `ItemDefinition` 确实没有属性字段——**属性在 Lua 侧**：`data/items.lua` / `data/tools.lua` 的定义带 `stats` 表（旧字段 `damage` 兼容为 `stats.attack`），经 `player/stats.lua` 三层模型（基础/装备/Buff）合成，`player/skills.lua:weapon_bonus()` 是消费方（练习剑 8 → 普攻 2+8=10，`jhyt_m18_stats` 逐格断言）；M34-04b 起 `systems/combat.lua:apply_damage` 另读 `defense` 做平减（`jhyt_m34_skills` ④/④′）。仍缺的是**引擎侧**的"物品 → 属性"查询：换装收益全靠游戏脚本，绑定层不参与 | 武器伤害真实生效 —— 已达成（M18-04 装备层 / M34-04b 防御层，都在游戏侧）；引擎侧属性查询仍未做 |
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

- **版本化键值存储**：`save.load/commit` 原子写盘，`save_version=2` 头（转义行格式，写入口）；**损坏/版本不符被拒绝且不 panic、不覆盖原文件**。读取接受 v1（原始行格式，原样读入、下次保存升级为 v2）与 v2；无版本头 / 版本 0 / 高于当前版本明确拒绝。多行文本、TAB 与 CJK 现在都能安全往返（此前一条含 `\n` 的值会破坏整档行结构）。`save.load()` 返回 `(ok, reason)`，`reason ∈ {"no_save_path","missing","corrupt: <err>"}`，游戏侧据此区分「首启动无档」与「存档损坏」。新增 `save.clear()` 与 `map.state_clear([name])` 供新游戏清理内存键（`main.lua new_game()` 已接线）。
- **任意值兼容层**：`save.blob_write/blob_read` 走 `__blob:<key>` → JSON 序列化；`lua_table_to_json` 对稀疏整数 key 表会退化为 object、读回后 key 变字符串——**游戏侧快照须稠密输出槽位**（`game_jhyt/scripts/inventory.lua` 已按此实现）。
- **web/微信上没有这份 Rust 存档后端**（2026-10-05 补齐前是缺口）：wasm 侧只有只读 VFS，落盘出口
  由 `plugins/webgl/web-runtime.js` 里 Lua 补丁自写（同一套 v2 行格式 + 长度前缀值层，落到
  `localStorage` / `wx.setStorageSync`）。补齐前游戏侧看到的形状是：`save.blob_write` 在 shim 里
  就是 `nil`，`M.save()` 静默返回 false（只有日志），而 `save.load()` 无条件 `return true` 谎报
  "磁盘档已载入"——即玩家报的"存档不生效"。工程侧对此**不需要任何改动**（`save_manager` 的
  provider 契约没变），但要知道两件事：**落的是宿主本地存储**（清站点数据/换设备/删除小游戏即丢档，
  无跨端同步），**没有原子写**（半档会被下次 `load` 判成 `corrupt` 并保留原文件，但救不回来）。
  还有一条对游戏侧有用：**宿主写失败会真的报失败**（`save.commit()` 返回 `false` + 原因，
  `save_manager.M.save()` 的 `commit_reason` 分支因此在这两个宿主上也会走到，不再只在原生侧）。
  复现命令与 46 次判定见 [docs/51](../../../docs/51-平台插件适配.md)「存档落盘」一节
  （`node scripts/wechat-minigame-harness.js … --drive-new-game --check-save-persist` →
  `save_persist="ok,probe=518,game=15074"`）。
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
| 旧存档迁移或明确拒绝 | B-2 | 已具备：引擎读路径接受 v1 存档（零数据丢失，下次保存升 v2），写入 v2 使旧二进制明确拒绝而非误读；游戏侧快照 v1→v2 由 `save_manager.migrate` 迁移（不再硬拒绝），`jhyt_m22_save_isolation` 覆盖 |
| 跨会话恢复时钟/背包/装备/玩家 | B-2（游戏侧） | 已具备：`jhyt_smoke`（`E:\Codex\RustLua\.../game/examples/jhyt_smoke.rs`）验证睡觉 → 存档 → 全新会话读档恢复 day/money/稀疏槽位/装备/PersistId 玩家 |
| 切图/重启后 tile/meta/solid 一致 | B-3 | 已具备：`map_state_tilemap_snapshot_survives_scene_change_and_restart` |

## 对齐记录（1.0，2026-09-23 / M32 相机收口）

1. **相机跟随与取景边界（本轮需求）**：改造前工程**没有任何场景声明相机**，Lua 侧也没有一处
   `camera2d.follow` ⇒ 运行时一直是默认相机（中心 `(640,360)`、`zoom=1`、无 bounds）。现在：
   * 每张玩法地图一个 `GameCamera` 实体，`Camera` 组件声明
     `projection=orthographic;center=<地图中心>;zoom=1;bounds=0,0,W*32,H*32`
     （由 `tools/pixel_kit.camera_line` 单点产出，8 个场景重跑生成器落地）；
   * `scripts/world/camera_view.lua` 在每次进图后 `camera2d.follow(player)`，
     offset/死区/平滑全 0 ⇒ 玩家始终在视野正中；
   * 竖屏（视口 720×1280）下的边界事实：**x 轴**视口(720) < 地图(1280) ⇒ 夹取区间
     `[360, 920]` 有效，跟随与"不出图"同时成立；**y 轴**视口(1280) > 地图(704) ⇒ 该轴
     不可夹取，引擎按 `clamp_center` 取边界中点，相机**钉在地图纵向中心**（地图比屏幕矮时
     几何上不可避免；要消掉只能 `zoom > 1.82`，已按 `docs/52` §4 裁定不缩放世界）。
     验收：`jhyt_m32_camera`（7 节，含四角反投影回世界的可见矩形断言）。

2. **发现并修掉一个引擎缺陷：场景声明的相机只在启动场景生效**。
   `process_scene_transition` 会重建整个世界（`ScriptState::world = World::new()` 后重新
   实例化），于是场景声明的 `Camera` 在换图后是**新的一份**；而宿主只在 `RuntimeScene::init`
   读过一次 `declared_camera` ⇒ 换图后 `self.camera` 的 center/zoom/**bounds 全部停在旧值**。
   症状很隐蔽：每张地图各声明一份 bounds 的工程里，只有开工那张图受约束，之后所有地图的
   相机都"忘了边界"（表现只是相机看着不对，没有任何报错）。
   修法：新增内置固定步系统 `scene_camera_sync`（紧跟 `scene_transition`，同格、显式依赖），
   重读新场景声明的相机并把 **bounds** 同步给宿主；**不取 center/zoom**——换图时相机姿态的
   权威仍是每帧的 `camera2d.*`，重置回新场景的作者中心会在换图那一帧闪一下。
   验收：`jhyt_m32_camera` ⑥ 节（换图后仍被夹在东界 920、室内图两轴都退化到地图中心）。

3. **数据门**：`world/camera_view.validate_all` 登记进 `scripts/validate_data.lua`，在启动与
   `game --check` / `editor --check` 三条路径上核对"场景恰好 1 台 `Camera`、正交投影、
   `zoom=1`、`bounds` 逐值等于该场景 GroundMap 的真实地图矩形"。这条契约横跨
   Python 生成器 → `.scene` → 引擎夹取三层，漂移的表现同样是"相机看起来怪但零报错"。

4. **顺带修掉：两个"跨文件契约校验器"此前一直在空跑**。Lua 侧没有拿到工程根的通路，而
   `io.open` 的相对路径按**进程工作目录**解析、`game --check` 又不 chdir 到工程根
   （在 `engine/` 下跑 `--check --project ../examples/game_jhyt` 时
   `io.open("scenes/main.scene")` 返回 nil；`debug.getinfo` 已被 Lua 沙箱剥掉）。于是
   **`ui/layout_check.lua` 的设计分辨率契约从未真正执行过**（场景 `meta ui_reference_*` 与
   `project.toml` 的 `window_width/height` 必须等于 720×1280），两个门都是恒绿——比"没有门"
   更糟，因为它看起来在工作。
   修法：引擎新增 `app.project_root()`（工程根绝对路径，正斜杠；未配置时为空串，Web 一类
   没有文件系统的宿主据此跳过），两个校验器改用它拼绝对路径。**负向证据**：把
   `scenes/manor.scene` 的 `ui_reference_width` 改成 700 ⇒ `[layout] ... = nil×1280，应为
   720×1280`；把相机 `bounds` 的 max_y 改成 700 ⇒ `[camera_view] ... bounds = 0.0,0.0,1280.0,700.0`。
   改回即 `game_check_ok`。这与既有裁决一致：**一条不能失败的门不算门**，新门必须给出
   它确实会红的证据（见 `docs/34` §验收口径与 `jhyt_m23_data_validation` ⑤ 的同款做法）。

## 对齐记录（0.9，2026-09-15 / M31）

1. **发布包不再把玩家数据写进程序目录（计划 P3）**：发布包形态（`<exe>/pack` + 同级
   `release.toml`，或 `--pack`）现在把 `save.sav` 与 `settings.toml` 都写到用户数据目录
   （Windows `%APPDATA%\<app>`，`RUSTLUA_USER_DIR` 可覆盖）；开发工程形态不变
   （设置 = 工程根 `settings.toml`，存档 = `<project>/.cache/save.sav`）。首次运行把包内
   `settings.toml` 播种为默认值，此后用户目录那份是唯一真相——**升级/重发布不再覆盖玩家
   改键、音量、手柄死区**。引擎新增 `core::user_dir` / `core::project_config` /
   `LuaRuntime::set_settings_root`，`game --check` 打印 `player_data_roots`；契约见 `docs/50`。
2. **游戏侧零改动即受益**：`ui/pause_menu.lua`、`ui/hud.lua`、`world/dialogue.lua` 等仍按
   `input.settings_path()` / `audio.settings_path()` 读写，不需要知道自己跑在发布包还是工程里；
   手柄死区（Rust 设备后端）与面板设置同源，不再出现"面板显示一套、设备用另一套"。
3. **新增引擎能力缺口（P1，WebGL 平台，实测确认）**：`scripts/verify-webgl-bundle.ps1` 把
   真实 WebGL 发布包在无头浏览器里启动后发现——wasm 加载成功、页面进入 Lua，但
   `web-runtime.js` 只有演示级子集，缺 17 个桌面引擎顶层全局
   （`app, asset, calendar, clock, container, data, day, event, grid, interact, inv, item,
   light, map, shop, sprite, tag`），本工程在 `item` 上停住（`data-rustlua-boot="failed"`）。
   现状与两条修法（宿主 API 编进 wasm / 把子集定为平台契约）见 `docs/32` §4；
   **PC 发行不受影响**，WebGL 仍按"实验性、未承诺可玩"处理。
4. **一条发布卫生规则**：发布目录不允许出现 `.cache`/`.data`/`*.sav` 与 `.staging-*` 残留，
   `pack/` 内不得有玩家数据——`jhyt_m31_pc_release` ① 节直接扫描真实发布目录，本轮据此清掉了
   P2 留下的一处 staging 残留。**运行期日志同样移出安装目录**：发布形态下
   `game.log`/`crash.log` 写 `<用户数据目录>/logs/`（`DataRoots::log_dir()`），否则在发布目录里
   跑一次游戏就会留下 `artifacts/logs/game.log`（装到 `Program Files` 时还会静默丢日志）。
5. **发布自检此前是空跑（本轮修掉）**：发布版 `game.exe` 是 GUI 子系统程序，
   `& game.exe --check` 在 PowerShell 里不等待也不捕获 stdout ⇒ `package.ps1` 的发布包检查与
   隔离自测**永远不会失败**；改用 `Start-Process -Wait` + 重定向后才真正生效（并立刻抓出
   "带空格的隔离路径被拆成多参数"这个真问题）。

## 对齐记录（0.8，2026-09-15 / M30）

1. **运行时排版从"没人检查"变成"机器验收"（计划 P2）**：此前 HUD 用「CJK 记 2 单位、
   其余记 1 单位」的启发式截断（`金钱 99999 文`、`WASD 移动…` 都会算错），对话正文既没换行
   也没裁剪（长台词画到面板外），背包/任务/商店/暂停的行同样只按字符数截断。现在：
   `ui/text_fit.lua`（纯数学层：按真实字宽裁剪/断行 + 标点禁则 + `max_lines` 省略号）+
   `ui/ui_text.lua`（**运行时文字唯一切口**：绘制同时登记 `(文字, 字号, 可用宽度)`，
   `audit()` 逐条量宽报出 缺字/字号 < 11px/超宽），44 处绘制全部接线。
2. **新增两个引擎 API**：`ui.measure_text(text, size)`（与渲染同口径的度量：
   同字体、同 advance、同字间距，返回 `width/height/lines/glyphs/missing`；
   只度量不光栅化，headless 便宜可用）与 `ui.text_get(id)`（读回画出来的文字与字号）。
   矩阵里的引擎项无新增缺口；`docs/18-lua-api.md` 已同步这两个绑定。
3. **修掉一个真实引擎缺陷**：`TextLayout::build` 的高度用 `ascent + descent` 算（`descent` 为负
   ⇒ 只有 0.8em），正确值是字身框 `ascent + |descent|`（1.2em）。新单测
   `measure_matches_layout_size` 先复现（`"江湖有田"@13px`：measured 15.6 / mesh 10.4）再修。
4. **缺字与首包字体子集契约**：正式包的字体是 `char_collect` 收的字符子集裁出来的，
   运行时少一个字符就是豆腐块。新增 `scripts/tools/text_audit.lua` 从运行时收集
   **371 条文案 / 601 字符**并逐条查缺字；验收再与 `char_collect` 集合（2649 字符）逐个核对。
5. **一条架构规则**：`scripts/**` 禁止直接调 `ui.text_at`（唯一出口 `ui/ui_text.lua`）——
   否则"画了但没登记"的文字会从排版检查里漏掉，正是那种零报错的坏行为。
   排版契约与容器预算表见 `docs/49`，验收 `jhyt_m30_cjk_text`。

## 对齐记录（0.7，2026-09-15 / M29）

1. **季节与天气从"数据存在"变成"画面可见"（计划 P1，阶段五起点）**：`calendar` 算出的季节此前只影响
   作物与 HUD 文字，四季地表贴图躺在 `assets/` 里无人引用；`weather` 只有晴一种取值，
   `farm` 的雨天免浇水与 NPC 的 `rain_place` 日程在真实玩法里**走不到**。本轮：
   新增 `data/season_art.lua`（地图 × 季节 → 贴图，`validate_all`，校验器 22）+ `world/season_visual.lua`
   （进图/跨天换图）、重写 `world/weather.lua`（`(seed, day, season)` 确定性抽签 + 季节概率表）、
   `ui/hud.lua` 增雨天薄雾/天气图标与三条资源条（按真实数值比例画宽度），验收见 `jhyt_m29_season_weather`。
2. **新增三个引擎 API（都是"让表现可验收"这一类，不是玩法新能力）**：
   `tilemap.set_texture(id, texture)`（运行期换地表贴图；渲染器每帧重读组件、变化时重建该层网格，
   不动格子/碰撞/meta）、`tilemap.get().texture`（幂等判断需要读回）、`ui.image_get(id)`
   （`ui.panel_get` 的图片版，让"血条宽度/天气图标"这类表现可以被 headless 直接断言）。
   矩阵里的 P1/P2 项无新增缺口；`docs/18-lua-api.md` 已同步这三个绑定。
3. **打包侧新增一条规则**：`assetc::is_dev_artifact` —— `_` 前缀、名字含 `preview` 的 PNG、
   `manifest.json` 一律不进发行包（生成器输出留在美术目录旁边是美术的期望，但没有任何运行期读取点）。
   单测 `dev_artifacts_are_not_packaged` 守住边界，特别是"含 `pixel` 但含 `preview` 的不能误伤"。
4. **修掉两个真实缺陷（都由本轮样例先复现，属于"看不见的坏"）**：
   ① `ui/hud.lua` 里两个绘制函数用 `local function` 定义在 `M.draw` **之后**——Lua 局部变量只在声明
   之后可见，调用落到 nil，而 `pcall` 把错误吞掉：画面上什么都不出现，日志里一行错误都没有
   （改成显式前向声明，并在样例里直接断言元素存在）；
   ② `boot_game` 的继续游戏路径是**先进图、后读档**，进图时 VM 时钟还是第 1 天 ⇒ 读档恢复到第 30 天后
   天气与季节地表停在第 1 天（此前季节只影响 HUD 文字所以看不出来）。现在读档成功后补一次
   `weather.sync()` + `season_visual.refresh()`。
5. **资源缺口不再是"没人知道"**：`docs/48` 把 102 个未引用资源逐个定为「计划」并写明理由与后续里程碑，
   由 `jhyt_m29_season_weather` ⑦ 重扫核对（数量不符即红灯）；同时真的下架两项
   （第三方包移出工程、标题原图改名）。此前"manifest.json 里出现过"被当成"已引用"的口径 bug 一并修正
   ——它是生成器清单，引擎与 Lua 运行期都没有读取点。

## 对齐记录（0.6，2026-09-14 / M28）

1. **战斗掉落、战败回庄与订单闭环（本轮新增，计划 G3，阶段四收官项）**：设计案 §6.2 的掉落列与
   §6.3 的死亡规则此前**只在文档里**——`data/enemies.lua` 没有掉落字段，`combat.respawn()` 是
   "原地复活"（回山庄/扣未入库掉落/扣铜钱一条都没实现），任务链里也没有吃战斗产出的订单。
   现落地为 `data/drops.lua`（掉落表 + `validate_all`，校验器 21）+ `systems/loot.lua`
   （铜钱当场入账、物品进"当日待入库"袋、跨天 `bank()`、确定性持久化 LCG）+ `systems/death.lua`
   （−50% 待入库 / −5% 铜钱 / 回山庄出生点 / 状态回满 / HUD 中文明细）+ 订单 `order_hunt`，
   验收见 `jhyt_m28_loop`（9 章，真实 J/T/R 键路径）。
2. **同样是纯游戏侧实现，没有新增引擎 API**：待入库袋用 `state.vars` 标量（随既有 state provider 进出存档）、
   掉落掷骰用自己维护的 LCG 标量（**不能用 `math.random`**：读档后序列会跳，headless 无法断言语义）、
   回庄用既有 `map.goto`、状态回满取 `player/inventory.lua` 新暴露的 `max_energy()`/`max_inner_power()`
   （把"上限 100"从两处硬编码收敛成一个来源）。矩阵项无新增缺口。
3. **文档不再"各说各话"**：`docs/33` §6.2 原本写的是 Alpha 数值与"草药/木材"这类注册表里**不存在**的
   掉落物；现拆成"Alpha 目标口径"与"**MVP 落地口径**"两张表，后者由 `jhyt_m28_loop` ①b **直接解析**
   并与 `data/enemies.lua`/`data/drops.lua` 逐格比对（敌人数量也要一致：新增敌人不补表即失败）。
4. **顺带修掉两个"随机失败"（引擎/测试侧，都是真实缺陷）**：
   ① 编辑器 `domain_data` 的测试夹具清理是 `fs::remove_dir_all(root).expect("cleanup")`，Windows 上会因
   Lua 运行时尚未释放文件句柄而偶发 `错误 32`，全量并行跑测试时随机判失败 → 改为有界重试的 `cleanup()`；
   ② 引擎资源数据库 `save()` 的原子写用**固定名**临时文件 `resources.db.tmp`：两个写入者
   （编辑器导入后台线程 + `game/editor --check`，或并行测试）同时保存同一工程时，先完成的 `rename`
   会把临时文件取走，另一个立刻以「系统找不到指定的文件（os error 2）」失败——本轮在
   `import::import_tests::import_spawn_end_to_end` 上实测复现过一次。现临时名带进程号 + 自增序号
   （`unique_temp_path`，与编辑器 `document.rs`/`util.rs` 已有的唯一临时名约定对齐），并新增
   `concurrent_saves_on_the_same_database_both_succeed` 守住它。**同一模式**在
   `import_cache.rs`（两处）/`prefab.rs`/`scene.rs`/`script.rs`/`logging.rs` 里还存在（固定名 `.tmp`），
   已记录为后续项（`docs/37` 4-21）。

## 对齐记录（0.5，2026-09-14 / M23）

按"先质量门、再数据安全、再引擎可靠性、再内容"的顺序执行后的复核（证据见
`tasks/done/RustLua-game_jhyt-执行计划.md` 的执行记录与 `scripts/verify.ps1 -Mvp` 日志）：

1. **数据校验（本轮新增，计划 G4）**：`validate_data.lua` 的登记表补上 `data.gather` 与
   `data.enemy_art`——两个模块**早就有** `validate_all` 却从未登记，等于它们的校验从来没跑过；
   纯表注册表（items/tools/crops/farm）的校验集中在新的 `data/registry_check.lua`
   （那三张表返回注册表本身，塞 `validate_all`/`version` 会被 `player/inventory.lua` 当物品注册）。
   跨文件不变量：种子/收获物必须存在且分类正确、作物土地类型必须被 `data/farm.lua` 区划覆盖、
   `crops[].sell_price` 必须等于 `items[harvest].sell`、工具 id 不得与物品同名、敌人 `art` 引用必须存在
   且每个美术条目都要被引用（"生成了美术但没接线"从此是加载期错误）。
   验收样例 `jhyt_m23_data_validation`：18 个已登记校验器、15 个负向注入用例、28 个资源引用逐个存在性检查。
2. **由此修出的两个真实缺陷**：`herb_tonic`（M18-04）与 `timber`（M18-02）的图标路径指向**不存在**的
   文件（背包/商店只会显示缺图兜底）→ `tools/gen_item_icons.py` 补两个画法并重新生成；
   演武场的训练木人与灰狼明明有 `assets/art/enemies/<id>/` 全套美术却没有 `art` 字段
   （一直是灰盒贴图）→ 已接线。
3. **数值对齐（本轮新增，计划 G5）**：`systems/farm.lua` 收获读 `crop.yield`，而 `data/crops.lua`
   没有该字段 ⇒ 每茬恒收 1（稻谷按设计案应为 2）。现 `yield` 显式声明（稻谷 2 / 灵芝 1）且为
   校验期必填；新增 `jhyt_m24_numeric_alignment` **直接解析 `docs/33` 的作物表与商店价格表**
   与 Lua 数据逐格比对（设计案侧同步修正稻谷售价 18→30、木材 10→15），并用真实耕作链路断言
   "改 `yield` 立即改变收获数量"。
4. **战斗受击反馈（本轮新增，计划 G2）**：M18-01 只有 hitbox/去重/冷却，命中后**没有打断、没有击退、
   玩家没有无敌帧**。现 `data/skills.lua` 每招带 `hitstun`/`knockback`（必填），新增
   `data/combat.lua` 存放玩家无敌帧（48 步 = 0.8 秒）与敌人默认抵抗，`data/enemies.lua` 可按个体覆盖
   （训练木人 1/1 = 完全不动）；`combat.lua` 应用无敌帧吸收/硬直/击退（不穿墙），`enemy.lua` 硬直期间
   停手并撤销寻路，`input.lua` 硬直期间锁操作并把攻击失败原因写进 HUD（`docs/37` 4-12 的落地部分）。
   矩阵 C-1 由「✖ 无 combat 命名空间」更新为**已具备**（hitbox/去重/硬直/击退/无敌帧/战败重生），
   见 `jhyt_m25_combat_feedback`。
5. **HUD 反馈闭环（本轮新增，计划 G6 / `docs/37` 4-13）**：`last_action` → HUD 中文反馈此前有三类缺口：
   一批取值（`missing_map`/`gather_blocked`/`unknown_recipe`/`no_station`/`busy`/`closed`/建造冲突键…）
   没有中文条目；`select_seed_*` 被 `select_` 前缀误命中显示成「已选建筑」；未知取值原样把英文键摆给玩家。
   现已补齐条目、修正前缀顺序（并新增 `equip_/tutorial_reward_/need_/gather_/exit_`，`exit_<map>`
   出地图短名）、未知值改为「未知反馈（原值）」。防回归由 `jhyt_m26_hud_feedback` 承担：它扫描
   `scripts/**` 的 last_action 来源（27 字面量 + 19 前缀 + 73 reason 词）逐个断言有中文映射。
6. **矩阵未变项**：D-0（建筑 footprint）、D-1/D-3（日程/任务 schema）、F-1（中文排版检查）、
   F-5（WASM/浏览器完整玩法）状态不变，仍是后续阶段任务。
7. **七日目标与日结算（本轮新增，计划 G1）**：这是**纯游戏侧新增系统**，没有对应矩阵缺口——它完全建立在
   既有引擎能力上：进度/结算/奖励锚点全部写 `state.vars` 标量（因此不需要新存档 API 就自动进 `save_manager`
   快照）、跨天用既有的 `on_day_start` 事件、呈现用 `ui` 文本。落地为 `data/goals.lua`（目标表 +
   `validate_all`）+ `systems/goals.lua`（进度/结算/里程碑）+ HUD 目标行，验收见 `jhyt_m27_goals`
   （7 天全部走真实玩法链路）。**顺带记录两个 API 边界**：① 引擎全局 `log` 只有 `info`/`warn`/`error`
   之类，**没有 `log.debug`**（误用会在运行期报 `attempt to call a nil value (field 'debug')`，本轮踩过一次）；
   ② 引擎全局 `inv` 才是背包 API（`inv.add/count/equip`），`player/inventory.lua` 只负责 `setup/use/存档`，
   业务代码里 `require("player.inventory")` 拿不到 `add`（教程奖励与目标奖励都直接调全局 `inv`）。

## 对齐记录（0.4，2026-09-13）

M19/M20 收尾后按实际落地能力复核（证据见 `tasks/done/M19-*`、`M20-*`）：

1. **状态上调**：F-2（自动布局 + 焦点/滚动导航 + 手柄设备层 + 命名动作 + 编辑器重绑定面板）由 🚧 → ✔（仅剩场景级 `Scroll` 容器变体）；F-3（音频分组总线）由 ✖ → ✔；F-4（发布原子替换与回滚）由 ✖ → ✔。
2. **状态更新**：F-5（WebGL 浏览器集成）由 ✖ → 🚧——真实浏览器契约检查与无头断言已通过（`webgl_browser_ok`），但 WASM 模块构建受离线环境阻塞，浏览器内完整玩法未执行（`docs/32`）。
3. **能力变更**：输入层从「物理键直读」升级为**命名动作**（`ActionMap` + `settings.toml` 的 `[input]` 段 + `input.action*`/`input.pad*` Lua API）；`game_jhyt` 全部脚本已迁移，键盘行为逐键等价（12 个 `jhyt_*` headless 回归全绿），手柄默认绑定覆盖 `confirm`/`cancel`/`ui_*`，其余动作由玩家在编辑器重绑定。
4. **新增已知限制（P2）**：发布包内改键写入 `<release>/pack/settings.toml`，重新发布会覆盖（需迁移到用户目录，`docs/35` §6）；天气恒为晴（`weather.set()` 无调用方）；真实手柄实机插拔未验证。

## 对齐记录（0.3，2026-09-07）

对引擎 `E:\Codex\RustLua` 源码与 `game_jhyt` 脚本做静态审计 + headless 验证后：

1. **状态上调**：E-1（Tilemap 碰撞编辑）、E-2（Animation 编辑）由 ✖ → ✔；E-3、F-1、F-2 修正为 🚧（局部已具备）；D-2、D-4 补记为 🚧（通用校验/存储骨架已具备，领域模型/属性效果缺失）。
2. **发现能力回退**：`a7b43a1` 移除存档迁移钩子链（B-2 相关），docs/18-lua-api 与旧版本文档过期，已在本表纠正。
3. **game_jhyt 修复**：`main.scene` Text 组件旧字段（`x/y/space/anchor`）违反 v2 场景规范会导致**启动即失败**，已改用 `UiLayout`；`scripts/main.lua` 补存档注册中心并删除死 `on_migrate`；`on_create` 设 `clock.set_rate(2)` 否则日期永不推进；`inventory.setup` 补注册 `data.tools`；快照稠密化规避稀疏 key 序列化坑。
4. **验证**：`jhyt_smoke`（临时 example，`E:\Codex\RustLua\engine\crates\game\examples\jhyt_smoke.rs`）通过：`main.scene` 实例化 4 实体、睡觉到 day2、全新会话读档恢复 day/money/种子/装备/稀疏槽位/玩家。该 example 属引擎仓临时文件，是否保留待定。

## 执行顺序（依赖固定）

```text
B-1 PersistId → B-4 A* → B-5 事件总线 → C-1 Hitbox → D-0 建筑占用
→ D-1/2/3/5 NPC·任务·对话·关系 → 垂直切片
→ E-* 编辑器工具（E-1/E-2 已具备） → F-* 发布/体验
```

> 注意：战斗 C-1 依赖 B-4（敌人追击）与 B-5（AttackHit 事件）；事件总线和 A* 应先于战斗落地。
