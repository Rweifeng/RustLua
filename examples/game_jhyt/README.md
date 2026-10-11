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
├── systems/              # 玩法系统：农田、采集、建造、制作、战斗、敌人、交互、商店、七日目标、掉落与战败
├── ui/                   # 表现与输入：HUD、输入分发、各面板、新手引导、中文排版
├── tools/                # 验收辅助（不被玩法代码引用，只被 headless 样例调用）
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
| `core/audio.lua` | `core.audio` | BGM/SFX 选曲与播放、音量总线、`project.toml` 的 `[audio]` 段持久化 |

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
| `world/season_visual.lua` | `world.season_visual` | 按季节换地表贴图（进图/跨天两处调用，数据在 `data/season_art.lua`） |
| `world/camera_view.lua` | `world.camera_view` | 2D 相机跟随玩家（`camera2d.follow`，玩家始终在视野正中）+ 每帧由场景 `GameCamera` 的 `bounds` 把取景夹在地图内；`validate_all` 守"场景 bounds == 真实地图矩形" |
| `world/weather.lua` | `world.weather` | 当日天气：`(seed, day, season)` 确定性抽签 + 季节概率表，跨天重抽、可钉住当天 |
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
| `systems/goals.lua` | `systems.goals` | 七日目标的进度/结算/里程碑（数据在 `data/goals.lua`） |
| `systems/loot.lua` | `systems.loot` | 击杀掉落结算、当日待入库袋、跨天入库（数据在 `data/drops.lua`） |
| `systems/death.lua` | `systems.death` | 战败结算（扣未入库掉落 50% + 铜钱 5%）→ 回山庄 → 状态回满 |

### ui/ —— 表现与输入

| 模块 | `require` 名 | 职责 |
|---|---|---|
| `ui/input.lua` | `ui.input` | 命名动作分发、模式路由、键位绑定与 `project.toml` 的 `[input]` 段 |
| `ui/hud.lua` | `ui.hud` | 状态 HUD、提示/错误反馈、键位提示 |
| `ui/inventory_ui.lua` | `ui.inventory_ui` | 背包面板 |
| `ui/quest_ui.lua` | `ui.quest_ui` | 任务日志面板（只读） |
| `ui/pause_menu.lua` | `ui.pause_menu` | 暂停菜单（继续/保存/读取/设置/返回标题） |
| `ui/panel_nav.lua` | `ui.panel_nav` | 面板通用上下选择导航 |
| `ui/tutorial.lua` | `ui.tutorial` | 新手引导「匹配事件 → 推进步骤」 |
| `ui/text_fit.lua` | `ui.text_fit` | **纯数学排版层**：按真实字宽量宽（`width`/`char_width`）、裁剪（`fit`）、断行（`wrap`，CJK 逐字可断 / 连续 ASCII 视作一个词 / 标点禁则 / `max_lines` 省略号收尾） |
| `ui/ui_text.lua` | `ui.ui_text` | **运行时文字唯一切口**：`draw`/`draw_exact`/`draw_wrapped` 绘制并登记 `(文字, 字号, 可用宽度)`；`record` 给实体层载体（`.prefab` + `prefab.set_text`）只登记宽度契约；`audit()` 逐条量宽报出 缺字 / 字号 < 11px / 超宽 |

**中文排版契约（M30）**：`scripts/**` 里**不要**直接调 `ui.text_at`——一律走 `ui.ui_text`，
否则"画了但没登记"的文字会从排版检查里漏掉（正是那种零报错的坏行为，静态扫描会失败）。
每处绘制把**该元素真实可用的宽度**作为预算传进去（`ui_text.draw(owner, id, x, y, size, budget, text)`；
搬进 `.prefab` 的行改成 `ui_text.record(owner, id, size, budget, text)`——只删不登记等于把宽度契约删掉），
预算就写在绘制调用处，因此改面板几何只需要改一个数字；容器预算表见 `docs/49`。
字号下限 11px（当前最小 13px），文案本身随便改——首包字体子集由 `char_collect` 自动收集，
不要手工维护字符清单。

### tools/ —— 验收辅助

| 模块 | `require` 名 | 职责 |
|---|---|---|
| `tools/text_audit.lua` | `tools.text_audit` | 从**运行时**收集全部可能画出的文案并查缺字：数据表 + HUD/反馈/目标/引导的运行时行 + 全部对话节点与选项 + `last_action` 前缀族 + 最坏数字形态（当前 371 条 / 601 字符）；导出 `corpus()`/`unique_chars()`/`missing()`/`audit()`/`stats()` |

`tools/**` 不被玩法代码引用，只被 headless 验收样例调用（`jhyt_m30_cjk_text` 用它的字符集
与引擎 `char_collect` 集合核对"运行时字符 ⊆ 首包字体子集"）。

### data/ —— 纯数据

`data/**` 只放内容注册表与校验器（`validate_all`），不含运行逻辑；目录层级即 `require` 命名空间
（`data/maps.lua` → `data.maps`，`data/quests/index.lua` → `data.quests.index`）。新增数据模块后
必须登记进根目录的 `validate_data.lua`，否则 `check({ strict = true })` 会报「缺少 validate_all 校验入口」。

**例外：纯表注册表**（`data/items.lua`、`data/tools.lua`、`data/crops.lua`）返回的是**注册表本身**，
`player/inventory.lua` 会 `for id, def in pairs(items) do item.define(id, def) end`——往这些表里加
`validate_all`/`version` 会被当成一个物品定义注册。它们的校验集中在 `data/registry_check.lua`
（`M.validate_all` 合并 items/tools/crops/farm 与敌人美术引用），同样登记在 `validate_data.lua`。
"带 `validate_all` 的模块必须登记"这条不变量由 `jhyt_m23_data_validation` 枚举 `scripts/**/*.lua` 守住，
所以**不要**再写"直通另一个模块校验"的同名函数（历史上 `player/skills.lua` 有一份，已删除）。

**战斗数值的归属**（M25）：招式造成的效果写在招式记录里（`data/skills.lua` 的
`hitstun`/`knockback`），受击方的通用反应写在 `data/combat.lua`（玩家无敌帧、敌人默认抵抗），
敌人个体差异写在 `data/enemies.lua`（`knockback_resist`/`hitstun_resist`）。
同一个数字只有一个来源：改"这一招推多远"改 skills，改"玩家挨打后多久无敌"改 combat。

**七日目标的归属**（M27）：目标表（7 天 × 每天 3–4 条 + 当日奖励 + 里程碑）写在 `data/goals.lua`，
运行时只在 `systems/goals.lua`；进度与结算结果全部落到 `state.vars` 标量（`goals_day`、
`goals_p_<id>`、`goals_day_<n>`、`goals_day_<n>_reward`、`goals_milestone`、`goals_milestone_reward`），
因此不需要新增存档 provider 就能进出存档。两条硬约定：① **每天最后一条目标必须是
`match.kind = "day_start"`**（日结算挂在跨天事件上），② 目标只写"匹配口径"一个字段
（`match = { kind, value }`，可选 `count`），呈现文案在 `text` 里，别再往目标里塞逻辑。

**季节与天气的归属**（M29）：地表贴图的"地图 × 季节"映射写在 `data/season_art.lua`，
换图运行时只在 `world/season_visual.lua`（调用点只有两个：`main.enter_playable_map` 与
`main.on_day_start`）；天气抽签只在 `world/weather.lua`。三条硬约定：
① **天气必须可复现**——取值由 `(seed, day, season)` 的确定性签名算出（`roll_for` 是纯函数），
季节概率写在 `RAIN_CHANCE`，**不要**引入 `math.random`（读档后序列会跳，headless 无法断言）；
② 地表换图只动 `Tilemap.texture`（`tilemap.set_texture`），**不动**格子/碰撞/每格 meta，
失败只记日志不中断进图与跨天（表现层不能拖垮玩法流程）；
③ 未被引用的资源不能"就这么躺着"：新资源要么接线，要么进 `docs/48` 的决定表并写明理由与
后续里程碑——`jhyt_m29_season_weather` ⑦ 会重扫 `assets/**` 逐行核对数量，对不上即红灯。

**掉落的归属**（M28）：掉落表（每只敌人一条 `{ enemy, money, items }`）写在 `data/drops.lua`，
运行时只在 `systems/loot.lua`，战败规则只在 `systems/death.lua`。三条硬约定：
① **"未入库"是字面意思**——物品先落到 `loot_p_<item>` 标量袋里，跨天 `bank()` 才进背包，死亡时
袋里减半（铜钱当场入账，因此只按 5% 扣）；② 掉落掷骰用持久化的确定性 LCG（`loot_rng` 标量），
**不要**换成 `math.random`（读档后序列会跳，headless 也就无法断言具体数量）；
③ 掉落物必须是 `data/items.lua`/`data/tools.lua` 里真实存在的 id，且 `docs/33` §6.2 的
「MVP 落地口径」表要与实现逐格一致（`jhyt_m28_loop` 直接解析该表比对）。

**中文排版的归属**（M30）：字宽的**唯一来源**是引擎 `ui.measure_text`（与渲染同口径：同字体、
同 advance、同字间距 `spacing = 1.0`）；`ui/text_fit.lua` 只做纯数学（裁剪/断行），
`ui/ui_text.lua` 是运行时文字的唯一切口。四条硬约定：
① **不要在 `scripts/**` 直接调 `ui.text_at`**（静态扫描会失败）——绕过切口就等于绕过了排版检查；
② 每处绘制把该元素**真实可用的宽度**当预算传进去，预算写在绘制调用处（改几何只改一个数字）；
③ 字号 ≥ 11px（`docs/42` §1 的可读下限，当前最小 13px），文案任意改、**不要**手工维护字符清单
（首包字体子集由 `char_collect` 收，`jhyt_m30_cjk_text` 会核对"运行时字符 ⊆ 首包集合"）；
④ 宽度比较容差用 `text_fit.EPSILON = 0.05px`——引擎整串度量是 f32 累加、Lua 逐字符求和是 f64，
两者在几百像素上有 ~1e-5 的自然差异，用 `1e-6` 判定会误报溢出。

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
4. **校验器登记**：新增带 `validate_all` 的模块要加入 `validate_data.lua` 的 `validators()`
   （第三个字段写 `require` 路径，`M.registered()` 与验收样例靠它做覆盖断言）。纯表注册表按上节
   走 `data/registry_check.lua`。

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
