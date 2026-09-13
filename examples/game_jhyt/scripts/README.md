# 《江湖有田》Lua 模块目录规范

本文件是 `scripts/` 目录的**结构事实来源**。新增模块时先在这里确定归属，再写代码。

## 1. 目录总览

```
scripts/
├── main.lua              # 游戏总入口（唯一入口，project.toml 的 main_script 指向它）
├── validate_data.lua     # 领域数据统一校验入口（game/editor --check 的约定路径）
├── core/                 # 底座：状态容器、存档、音频总线
├── player/               # 主角自身：实体、背包、属性、武学、关系声望
├── world/                # 世界模拟：地图、NPC、日程、日历天气、任务、对话
├── systems/              # 玩法系统：农田、采集、建造、制作、战斗、敌人、交互、商店
├── ui/                   # 表现与输入：HUD、输入分发、各面板、新手引导
└── data/                 # 纯数据表（内容注册表），不含逻辑
```

### 1.1 为什么 `main.lua` 与 `validate_data.lua` 留在根目录

这两个路径是**引擎与工具链的硬约定**，不是随意摆放：

| 路径 | 约定方 |
|---|---|
| `scripts/main.lua` | `project.toml` 的 `main_script`；引擎 `runtime_scene` / `LuaRuntime` 据此装载 |
| `scripts/validate_data.lua` | `game --check`（`crates/game/src/cli.rs`）、`editor --check`、编辑器领域数据面板（`UNIFIED_ENTRY_RELATIVE`） |

把它们搬进子目录需要同时改动 Rust 侧约定与 `project.toml`，收益为零、回归风险高，因此固定保留在根目录。

## 2. 各层职责与模块清单

### core/ —— 底座

| 模块 | `require` 名 | 职责 |
|---|---|---|
| `core/state.lua` | `core.state` | 全局运行态容器（模式机、体力、金钱、背包句柄、剧情变量、形象 id） |
| `core/save_manager.lua` | `core.save_manager` | 存档注册中心：各模块注册 `capture/restore`，读档按注册表整体恢复 |
| `core/audio.lua` | `core.audio` | BGM/SFX 选曲与播放、音量总线、`settings.toml` 的 `[audio]` 段持久化 |

### player/ —— 主角自身

| 模块 | `require` 名 | 职责 |
|---|---|---|
| `player/player.lua` | `player.player` | 主角实体挂载、朝向与动作序列帧、传送 |
| `player/inventory.lua` | `player.inventory` | 背包容器、新档载荷、装备槽 |
| `player/stats.lua` | `player.stats` | 三层属性模型（基础 / 装备 / 临时 Buff） |
| `player/skills.lua` | `player.skills` | 招式查询与伤害换算（读 `data.skills`） |
| `player/relationship.lua` | `player.relationship` | 角色关系点数、声望、幂等奖励锚点 |

### world/ —— 世界模拟

| 模块 | `require` 名 | 职责 |
|---|---|---|
| `world/map_manager.lua` | `world.map_manager` | 当前地图、出生点、出口注册与判定 |
| `world/npc.lua` | `world.npc` | NPC 实体同步、A* 跟随、跨图落位、诊断 |
| `world/schedule.lua` | `world.schedule` | 日程解析（跨午夜时段、雨天替代）与校验 |
| `world/calendar.lua` | `world.calendar` | 日期推进与季节 |
| `world/weather.lua` | `world.weather` | 当日天气与切换 |
| `world/quest.lua` | `world.quest` | 任务五态状态机、目标判定、幂等交付 |
| `world/dialogue.lua` | `world.dialogue` | 对话定义解析、节点推进、选项条件与副作用 |

### systems/ —— 玩法系统

| 模块 | `require` 名 | 职责 |
|---|---|---|
| `systems/farm.lua` | `systems.farm` | 农田棋盘、翻土/播种/浇水/收获、日结算 |
| `systems/gather.lua` | `systems.gather` | 采集点、刷新周期 |
| `systems/building.lua` | `systems.building` | 建筑占用、预览、放置/拆除/升级 |
| `systems/crafting.lua` | `systems.crafting` | 制作设施、配方任务队列 |
| `systems/combat.lua` | `systems.combat` | 固定步战斗、攻击判定窗、气血账本 |
| `systems/enemy.lua` | `systems.enemy` | 敌人实体生成、AI、存活账本 |
| `systems/interaction.lua` | `systems.interaction` | 通用交互目标注册与就近候选（E 键） |
| `systems/shop.lua` | `systems.shop` | 营业时间判定、原子买卖、每日限购 |

### ui/ —— 表现与输入

| 模块 | `require` 名 | 职责 |
|---|---|---|
| `ui/input.lua` | `ui.input` | 命名动作分发、模式路由、键位绑定与 `settings.toml` 的 `[input]` 段 |
| `ui/hud.lua` | `ui.hud` | 状态 HUD、提示/错误反馈、键位提示 |
| `ui/inventory_ui.lua` | `ui.inventory_ui` | 背包面板 |
| `ui/quest_ui.lua` | `ui.quest_ui` | 任务日志面板（只读） |
| `ui/pause_menu.lua` | `ui.pause_menu` | 暂停菜单（继续/保存/读取/设置/返回标题） |
| `ui/panel_nav.lua` | `ui.panel_nav` | 面板通用上下选择导航 |
| `ui/tutorial.lua` | `ui.tutorial` | 新手引导「匹配事件 → 推进步骤」 |

### data/ —— 纯数据

`data/**` 只放内容注册表与校验器（`validate_all`），不含运行逻辑；目录层级即 `require` 命名空间
（`data/maps.lua` → `data.maps`，`data/quests/index.lua` → `data.quests.index`）。新增数据模块后
必须登记进根目录的 `validate_data.lua`，否则 `check({ strict = true })` 会报「缺少 validate_all 校验入口」。

## 3. 约定

1. **模块名 = 相对 `scripts/` 的路径，目录用 `.` 连接**：`scripts/systems/shop.lua` → `require("systems.shop")`。
   引擎把 `scripts/` 加进 `package.path` 并据此解析，新增目录无需注册。
2. **跨层依赖方向**：`core` ← `player` / `world` / `systems` / `ui`，`main.lua` 只做编排。
   `data/**` 是叶子，只允许被引用，不允许反向 `require` 运行时模块（`data/tutorial.lua` 引用
   `world.quest` 是既有例外，仅用于校验任务 id）。
3. **加载期循环依赖**：模块在加载期（文件顶层）互相 `require` 会成环。需要「运行时才用」的依赖
   放到函数体内 `require`（如 `systems/gather.lua` 在函数里取 `systems.interaction`）；
   `systems/combat.lua` 必须先于 `systems/enemy.lua` 加载（enemy 在加载期向 combat 注册目标协议），
   该顺序由 `main.lua` 的 require 顺序保证，**不要调整**。
4. **校验器登记**：新增带 `validate_all` 的模块要加入 `validate_data.lua` 的 `validators()`。

## 4. 迁移对照（2026-09 目录重组）

| 重组前 | 重组后 |
|---|---|
| `scripts/state.lua` | `scripts/core/state.lua` |
| `scripts/save_manager.lua` | `scripts/core/save_manager.lua` |
| `scripts/audio.lua` | `scripts/core/audio.lua` |
| `scripts/player.lua` | `scripts/player/player.lua` |
| `scripts/inventory.lua` | `scripts/player/inventory.lua` |
| `scripts/stats.lua` | `scripts/player/stats.lua` |
| `scripts/skills.lua` | `scripts/player/skills.lua` |
| `scripts/relationship.lua` | `scripts/player/relationship.lua` |
| `scripts/map_manager.lua` | `scripts/world/map_manager.lua` |
| `scripts/npc.lua` | `scripts/world/npc.lua` |
| `scripts/schedule.lua` | `scripts/world/schedule.lua` |
| `scripts/calendar.lua` | `scripts/world/calendar.lua` |
| `scripts/weather.lua` | `scripts/world/weather.lua` |
| `scripts/quest.lua` | `scripts/world/quest.lua` |
| `scripts/dialogue.lua` | `scripts/world/dialogue.lua` |
| `scripts/farm.lua` | `scripts/systems/farm.lua` |
| `scripts/gather.lua` | `scripts/systems/gather.lua` |
| `scripts/building.lua` | `scripts/systems/building.lua` |
| `scripts/crafting.lua` | `scripts/systems/crafting.lua` |
| `scripts/combat.lua` | `scripts/systems/combat.lua` |
| `scripts/enemy.lua` | `scripts/systems/enemy.lua` |
| `scripts/interaction.lua` | `scripts/systems/interaction.lua` |
| `scripts/shop.lua` | `scripts/systems/shop.lua` |
| `scripts/hud.lua` | `scripts/ui/hud.lua` |
| `scripts/input.lua` | `scripts/ui/input.lua` |
| `scripts/inventory_ui.lua` | `scripts/ui/inventory_ui.lua` |
| `scripts/quest_ui.lua` | `scripts/ui/quest_ui.lua` |
| `scripts/pause_menu.lua` | `scripts/ui/pause_menu.lua` |
| `scripts/panel_nav.lua` | `scripts/ui/panel_nav.lua` |
| `scripts/tutorial.lua` | `scripts/ui/tutorial.lua` |
| `scripts/data/**` | 不变 |
| `scripts/main.lua` | 不变（引擎约定） |
| `scripts/validate_data.lua` | 不变（工具链约定） |

> `docs/` 下 `29`~`47` 号里程碑文档里的 `scripts/xxx.lua` 写法是**重组前的历史记录**，
> 未逐条改写；查当前路径以本文件为准。
