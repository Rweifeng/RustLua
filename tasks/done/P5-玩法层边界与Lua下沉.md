# 任务：P5 玩法层边界（gameplay 数据模型的 Lua 化裁定与体裁假设收口）

- 任务 ID：P5-00（总包；子项 P5-01 … P5-05）
- 状态：**P5-01/P5-02 已落地、P5-05 文档已回写（ADR-0017 待单独确认）；P5-03/P5-04 未触发，
  不排期（2026-10-21）**。2026-10-22 全量 `verify.ps1` 在隔离树通过；2026-10-23 收口归档，
  P5 已提交 `main`（`7ef0eebe` + `24442fef`，未推送），不再位于 `stash@{0}`。落地读数与门见
  本文末「实现记录」「验收证据」；§2 的旧读数已按 P1 收口后的工作树复核。
- 关联：[docs/62 成熟引擎差距分析](../../docs/62-成熟引擎差距分析.md) §6、
  [docs/18 Lua API](../../docs/18-lua-api.md)、
  [docs/19 Lua 热重载](../../docs/19-lua-hot-reload.md)、
  [docs/02 ADR-0017](../../docs/02-technical-decisions.md)、
  [P1 降耦合收口](../done/P1-降耦合收口.md)（前置：同一批文件在途）、
  [P3 渲染与编辑器能力](P3-渲染与编辑器能力.md)、[P4 性能与测试度量](P4-性能与测试度量.md)
- 创建日期：2026-10-20
- 基线：`b7958560`（2026-10-20）+ **未提交工作树**（P1 ⑤ 在途）。本文只新增计划文件，
  不改代码、不碰在途改动
- 估时口径：**人周**（一个熟悉仓库的工程师）。这是改动面估算，不是日历承诺；每包开工前先做
  1–2 天探针把估算收窄

> **命名冲突先说清**：本文件 P5 是 `tasks/plan/` 的第五批，接 P1「降耦合收口」、P2「发布与
> 安全工程」、P3「渲染与编辑器能力」、P4「性能与测试度量」。它**不是** `docs/62` §6 的
> P3/P4 档位，也不认领 `docs/60` 的 P 编号。

---

## 0. 裁定（先看这一段）

问题原文：**能不能把 gameplay 也搬到 Lua 层，Rust 只实现游戏引擎底层的支持？**

**能，而且主体已经发生；不建议把剩下的部分整体搬。**

三条事实：

1. **玩法规则已经在 Lua**：`examples/game_jhyt/scripts` **155 个文件 / 37,489 非空行**
   （全文 39,766 行；种植、战斗、商店、任务、对话、UI）。Rust 侧没有任务/种植/战斗/对话/经济规则。
2. **Rust 侧可称为 gameplay 的只有 1,968 行 / 47 个 Lua 函数**（P5-02 后 2026-10-21 复核，
   P1 ⑤ 已把 item/inv/container/shop 绑定搬进本 crate）：`gameplay.rs` 425 + `clock_lua.rs` 403 +
   `grid.rs` 91 + `grid_lua.rs` 69 + `inventory_state.rs` 391 + `inventory_lua.rs` 589；
   这 47 个函数占 **42 个模块 / 296 个函数**的 **15.9%**
   （[lua_api_surface.json](../../engine/crates/rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json)）。
3. **`rustlua-gameplay` 这个 crate 59.6% 不是玩法**：settings 1,295 行（26.6%，编辑器与 input
   共用）+ save 761（15.6%）+ world_data 568（11.7%）+ 绑定胶水 237（4.9%）+ `lib.rs` 46，
   合计 2,907 行；全 crate 4,875 非空行（口径：含测试与文档注释）。

所以问题不是"能不能分层"（已经分完了），而是"**跨 VM 生命周期的可变状态归谁**"。
§4 的四条边界决定：`save`/`data`/`map`/`health`/`tag` **必须**留 Rust；`inv`/`item`/`clock`
留 Rust 是**为了热重载与固定步进**（技术上是能搬的）；只有 `shop`/`grid`/`calendar` 的季节
常量属于"该搬的体裁假设"。

**建议**（不做 P5-03/04 也能交付）：

- **P5-01（1–2 天）账本与命名**：crate 内五个文件域写清；`docs/18` 标出"只在 demo/契约里用"的 API。**✅ 2026-10-21**
- **P5-02（2–3 天）体裁假设参数化**：日历 `28 天/季 × 4 季`；背包容量/堆叠已有 `item.define` 数据面。**✅ 2026-10-21**
- 只有 §7 的触发条件出现，才开 **P5-03（两个缺失机制）→ P5-04（按顺序下沉）**。**当前仍未触发。**

---

## 1. 要回答的问题

| # | 问题 | 现在能回答吗 | 依据 / 缺口 |
|---|---|---|---|
| 1 | 玩法到底写在哪？Rust 还剩多少玩法？ | **能** | 本文件 §2.1/§2.2；剩下 1,968 行数据模型（47 个 Lua 函数） |
| 2 | 把 `inv`/`clock` 搬进 Lua，会丢什么？ | **能** | §4.1 热重载换 VM；§4.3 ECS 身份 |
| 3 | Rust 侧哪些"玩法"其实不是玩法？ | **能** | §2.1：settings + save + world_data 占 80% |
| 4 | 搬过去要补什么机制？多久？ | **能** | §5 P5-03/P5-04：内置 Lua stdlib + 跨 VM 状态桥 |
| 5 | 现在该不该搬？ | **能（裁定）** | §0/§7：不整体搬；先参数化体裁假设 |
| 6 | 怎么证明搬了没退化？ | **能** | §6：API 面快照 + 行为契约 + 100 个示例 + 三道门 |
| 7 | 多贵的 Rust 面值得为它建 stdlib？ | **不能（缺量尺）** | 需要"第二体裁工程"或一次 P5-03 探针；本文件不臆造 |

---

## 2. 现有事实（2026-10-21 复核）

### 2.1 代码面

> P5-02 后 2026-10-21 复核（P1 ⑤ 已收口，绑定搬进本 crate）：下面的行数是当前读数；2026-10-20
> 的旧表（3,441 行）只保留作历史对照，别再引用。口径：非空行，含测试与文档注释。

| 域 | 文件 | 非空行 | 是不是玩法 |
|---|---|---|---|
| 设置 | `settings.rs` | **1,295** | ❌ 工程 `settings.toml` 的读/写/迁移；编辑器 `menu.rs`/`input_bindings.rs`/`preview_session.rs` 都读它 |
| 存档 | `save.rs` 301 + `save_lua.rs` 317 + `save_store.rs` 143 | **761** | ❌ v1/v2 格式、原子写、平台槽 |
| 世界数据 | `world_data_lua.rs` 486 + `world_data_state.rs` 82 | **568** | ❌ `data.load` 的缓存/schema、`map.state` 的持久键 |
| 玩法模型 | `gameplay.rs` 425 + `clock_lua.rs` 403 + `grid.rs` 91 + `grid_lua.rs` 69 + `inventory_state.rs` 391 + `inventory_lua.rs` 589 | **1,968** | ✅ 只有这一块（含 item/inv/container/shop 绑定） |
| 绑定胶水 | `audio_lua.rs` 88 + `input_lua.rs` 149 | **237** | ❌ 别的领域的注册入口 |
| crate 级 | `lib.rs` | 46 | —（P5-01 后是域账本文档） |
| **合计** | 15 个文件 | **4,875**（全文 5,209） | 玩法占 **40.4%** |

> 复现：`Get-ChildItem engine\crates\rustlua-gameplay\src -File | ForEach-Object { "{0,5} {1}" -f (Get-Content -LiteralPath $_ | Measure-Object -Line).Lines, $_.Name }`
> （`Measure-Object -Line` 不计空行；合计行的"全文"是 `ReadAllLines().Count`）

### 2.2 API 面

> P5-02 之后（2026-10-21）：函数 296（+`calendar.configure`）；模块表仍 42。

| 事实 | 读数 |
|---|---|
| Lua 全局模块表 | **42 个** |
| Lua 函数 | **296 个** |
| 玩法味模块的函数 | **47 个（15.9%）**：`item` 2 / `inv` 15 / `container` 4 / `shop` 2 / `clock` 6 / `calendar` 3 / `day` 2 / `health` 3 / `tag` 2 / `interact` 4 / `grid` 4 |
| 服务类（不属于玩法） | `save` 9 / `map` 9 / `data` 4 |

> 复现：`$j = Get-Content -Raw engine\crates\rustlua-scripting\src\api_surface_snapshot\lua_api_surface.json | ConvertFrom-Json; $j.tables.Count; ($j.tables | ForEach-Object { $_.funcs.Count } | Measure-Object -Sum).Sum`

### 2.3 使用面（`examples/game_jhyt/scripts` 静态调用点）

> 2026-10-21 复跑（P1 收口后）；括号里是 2026-10-20 的旧读数。

| 模块 | 调用点 | 说明 |
|---|---|---|
| `inv` | **113**（115） | 背包主路径；`ui/inventory_ui.lua:126` 每帧一次 `inv.get` |
| `item` | **42**（31） | `item.define` 在数据加载期 |
| `shop` | **33**（36） | 目录 + 价格查询 |
| `container` | **0** | 只在 `game_api` demo 与契约用例里 |
| `clock` / `calendar` / `day` | **44 / 25 / 15** | 时间推进与季节 |
| `health` | **5** | 战斗 |
| `tag` | **0** | 只在 demo/契约里（`interact.query` 内部读它） |
| `interact` | **0** | 只在 demo/契约里 |
| `grid` | **21**（22） | 格子换算 |
| `save` | **8**（9） | 全部集中在 [save_manager.lua](../../examples/game_jhyt/scripts/core/save_manager.lua)：一个 blob + 版本 + 摘要 |
| `map` / `data` | **61 / 580**（69 / 589） | 场景持久状态 / 数据表加载 |

> 复现（模块循环，按 `\b<模块>\.` 计）：`foreach ($m in 'item','inv','container','shop','clock','calendar','day','health','tag','interact','grid','save','map','data') { ... }`
> 数字是**静态调用点**，不是运行期频率；性能结论见 §4.4。

### 2.4 门与测试面

| 量具 | 读数 / 入口 |
|---|---|
| `rustlua-gameplay` 单测 | **68 个**（运行期，2026-10-21；含 P5-02 新增 5 条：默认/自定义/非法模型值 + 两条 Lua 面） |
| `rustlua-scripting` 单测 | **120 条**（运行期，2026-10-21；P5-02 未改本 crate 的单测，只重生成基线） |
| API 面快照门 | `lua_api_surface_matches_baseline`：42 模块 / **296 函数**逐项比对（P5-02 只加 `calendar.configure`） |
| 行为契约门 | `lua_api_contract.lua` + `lua_api_contract_transcript.txt`，**38 条 / 玩法相关 22 条**（clock 6 / item 5 / calendar 5 / interact 2 / day 1 / container 1 / shop 1 / tag 1；**`inv`/`save`/`health`/`grid`/`map`/`data` 仍为 0 条**） |
| 端到端 | `engine/crates/game/examples` **100 个**（大多数是 jhyt 里程碑门） |
| 架构门 | `scripts/verify/check-decoupling.ps1`（`verify.ps1` 已接） |
| 文档门 | `scripts/verify/check-docs.ps1`（编号/H1/索引/链接/编码） |

### 2.5 平台面

| 事实 | 位置 |
|---|---|
| 存档落盘按平台分叉，但**格式只在 Rust** | `save_lua.rs::store_save` → `rustlua_infra::save_slot`（浏览器 `localStorage` / 微信 `wx.setStorageSync` 经 adapter 映射） |
| 历史漂移先例 | Web 侧一度有 **57 个宿主补丁**覆盖引擎实现，`clock.now()` 少三个字段而名字门抓不到 → 这才有行为契约门（见 [lua_api_contract.lua](../../engine/crates/rustlua-scripting/src/api_surface_snapshot/lua_api_contract.lua) 文件头） |
| wasm 侧的 `HostApi` 已是死码 | [host_api.rs](../../engine/crates/game_web/src/host_api.rs)：clock/calendar/day 的镜像实现，`wechat-adapter.js:5163` 注明"仅保留为诊断兼容，web-runtime.js 不再经它注册宿主全局" |
| 引擎**没有** Lua 标准库这层 | `require` 只搜工程 `scripts/`（[require.rs](../../engine/crates/rustlua-scripting/src/script/require.rs)）；无 `engine/lua/`、无 `package.preload` 注入 |
| Lua 侧**没有** JSON 编解码 | 42 个表里没有 `json`；`data.load` 返回已解析表，`save.blob_*` 在 Rust 侧转 JSON |

---

## 3. 逐模块判定

判定口径：**是否满足"两个体裁测试"**——同一份 Rust API 能否被两个体裁差异显著的工程复用
而不 fork？能 → 留；不能 → 参数化或下沉。

> 表中 `item`/`inv`/`container` 与 `container`/`interact`/`tag` 两行是"按 API 面"看的，
> 与上面按"状态归属"看的行**有重叠**，不要纵向相加；每行只回答"这一块归谁"。

| 模块 | 函数 | 判定 | 理由 | 前置 |
|---|---|---|---|---|
| `save` | 9 | **留（底层）** | 格式版本、原子写、平台槽；Lua 只决定"存什么"（现有 `save_manager.lua` 已是范例） | — |
| `data` / `map.state` | 13 | **留（服务）** | 读盘 + JSON + schema + 缓存 + 持久键约定；Lua 在 wasm 上无文件系统 | — |
| `health` / `tag` / `interact` | 9 | **留（ECS 身份）** | 状态在 `EntityState`，`entity.destroy` 与切场景由 Rust 清理；搬到 Lua 要镜像并有遗漏风险 | — |
| `grid` | 4 | **可搬（低风险）** | 纯函数、无状态；但它是坐标约定的唯一来源（docs/06），搬走要与 tilemap/camera 同维护 | P5-03 |
| `shop` | 2 | **可搬 / 可删** | 只是目录 + 查表；`data.load` 可替代 | P5-03 |
| `calendar` 的季节/年 | 3 | **搬常量 → 已参数化** | `28 天/季 × 4 季` 是农场体裁假设，不是引擎语义；P5-02 已让 `calendar.configure` 改写，默认值不变 | ✅ P5-02 落地；继续下沉仍需 P5-03 |
| `clock` | 6 | **留（推进）** | 固定步进帧系统 `clock_step` 在 Lua `update` 之后推进；状态要跨 VM 存活 | P5-03 |
| `item` / `inv` / `container` | 21 | **暂不搬** | 热重载会清空背包（§4.1）；`inv.get` 每帧一次，Rust 每帧还要分配 20 个 String，Lua 原生表反而更省；但**体裁耦合**真实存在（固定槽位 + `max_stack`） | P5-03 + 第二体裁 |
| `container` / `interact` / `tag`（作为 API 面） | 10 | **标废弃，不删** | 出货工程 0 调用点；删除是破坏性改动，需 v0.2 兼容窗口 | P5-01 |
| `settings` | — | **留（编辑器共用）** | 它不是玩法（体积最大的"误伤"） | — |

---

## 4. 四条硬边界

### 4.1 热重载换 VM（唯一真 blocker）

事实：[docs/19](../../docs/19-lua-hot-reload.md) 明写"不迁移旧 VM 的全局变量、闭包和协程；
**Rust 侧共享的引擎状态继续保留**"，提交顺序是"候选加载 → 候选 `on_create` → 旧 `on_destroy`
→ 替换 VM"。

推论：Lua 持有的背包/时钟在每次改脚本时会**丢**；且不能用旧 `on_destroy` 补救——新 VM 的
`on_create` 已经跑完。要救只有两条路：

- **P5-03 状态桥**：在候选 VM 创建**之前**把注册过的 Lua 状态序列化进 Rust，新 VM 装完之后、
  `on_create` 之前恢复；
- **接受重置**：只对无状态模块（`shop`/`grid`/`calendar` 常量）搬，不碰 `inv`/`clock`。

这条决定了 P5-04 的顺序：能先搬的恰好是**不持有自己状态**的那三个（`shop`、`grid`、`calendar`
的推导），`inv`/`clock` 要等状态桥。

### 4.2 存档格式与平台槽

事实：[save.rs](../../engine/crates/rustlua-gameplay/src/save.rs) 是 v1/v2 转义格式
（多行文本曾让整档不可读，是已复现的 P0）；`save_lua.rs::commit` 走 `atomic_write`（先写临时
文件再替换），wasm 走 `save_slot` provider。键名与 JS 侧逐字一致是硬要求（分叉 = 清空所有玩家进度）。

推论：**存档引擎不能进 Lua**，否则要在 Lua 里重写版本、转义、原子性与跨平台分叉；Lua 能做的
只有"决定 blob 的内容"。`inv` 真搬走时，也必须照 `save_manager.lua` 的形状（Lua 存表 →
`save.blob_write`），而不是让 Rust 自动存背包。

### 4.3 ECS 身份与清理

事实：`health`/`tags` 存在 `EntityState`，`entity.destroy` 删它们，切场景走
`clear_for_scene_transition`（[entity.rs](../../engine/crates/rustlua-scripting/src/script/api/entity.rs)、
`script/mod.rs:285`）。

推论：Lua 若按实体 id 建表，必须自己接管 destroy 与切场景两条清理路径；`entity.destroy` 是
Rust 函数，Lua 包一层也拦不住引擎内部（切场景）的批量清理 → **staleness 是必然而不是可能**。
所以 `health`/`tag`/`interact` 留 Rust。

### 4.4 没有 Lua 标准库这层（搬之前必须先造）

事实：`require` 只搜工程 `scripts/`；没有 `engine/lua/`；没有 `package.preload` 注入；
没有 `json` 模块；`Lua::new()` 加载全部安全标准库（桌面 `io.open` 可用，wasm 无文件系统）。

推论：玩法下沉到 Lua 后，代码放哪只有三个选项——每个工程复制一份（漂移，历史已证明会漂）、
打进发布包（版本跟工程走，引擎升级带不动）、或**引擎内置 stdlib**（需要新机制，见 P5-03）。
没有第三种，就没有"引擎级玩法 API"。

**性能不是理由**：37k 行里这些模块的静态调用点是个位到百位；最热的 `inv.get` 每帧一次；
mlua 单次调用量级在百纳秒，相对每帧一次可忽略。反方向（Lua 原生表替代跨语言 + 分配）在
`inv.get` 这条路径上**更快**。

---

## 5. 工作包

### 5.1 P5-01 账本与废弃标记（✅ 已落地 2026-10-21，1–2 天包）

**做什么**

1. `rustlua-gameplay/src/lib.rs` 顶部加 crate 级 doc：四个域、行数、哪些不是玩法（本文 §2.1 的表）。
2. `docs/18-lua-api.md` 的玩法 API 表加一列"层"（规则 / 数据模型 / 服务），并把
   `container`/`interact`/`tag` 标为"出货工程 0 调用点（2026-10-20 静态审计），保留给
   `game_api` 与契约；删除需 v0.2 兼容窗口"。
3. `calendar`/`GameClock` 的常量加注释："28×4 是农场体裁默认值，不是引擎语义"（为 P5-02 铺路）。

**判据**

- `cargo check -p rustlua-gameplay -p rustlua-scripting` 绿；
- `cargo test -p rustlua-scripting api_surface` 绿（纯注释/文档，**API 面必须零 diff**）；
- `powershell -File scripts\verify\check-docs.ps1` 绿。

**不做**：不改 crate 名、不拆 crate。理由：P1 ⑤ 正在把绑定搬进这个 crate，改名会与在途改动
抢同一批文件；改名（`rustlua-gameplay` → `rustlua-persistence` + `rustlua-gameplay-model`）
登记为 **P5-01b，等 P1 收口后再评估**。

**风险**：低。只动注释与文档。

### 5.2 P5-02 体裁常量参数化（✅ 已落地 2026-10-21，2–3 天包）

**做什么**

1. `GameClock` 增加两个字段：`days_per_season`（默认 28）、`seasons_per_year`（默认 4），
   `season()/year()/day_of_season()` 改读字段；`Default` 保持现值。
2. 新增 Lua API `calendar.configure{ days_per_season = N, seasons_per_year = M }`：
   正整数校验、非法值报错且不改状态；`calendar.now()` 形状不变。
3. `game_web/src/host_api.rs`（死码）要么同步，要么就地注明"不同步，仅诊断"——**不允许**留
   两份语义不同又不说明的镜像。

**为什么不做成工程设置**：`settings.toml` 是编辑器/输入/音频的域（`ProjectSettings` 1,295 行），
把玩法常量塞进去会让"改玩法"变成"改工程设置"，语义错位；先做 Lua API，等真有第二体裁再谈配置。

**判据**

- `cargo test -p rustlua-gameplay` 绿（新增 3 条：默认值 / 自定义值 / 非法值）；
- `cargo test -p rustlua-scripting api_surface`：**只允许** `calendar.configure` 这一处新增
  （需重生成基线并说明理由）；
- 契约 transcript **只允许**新增用例行，既有行零 diff；
- `cargo run -q -p game --example jhyt_m29_season_weather` 与 `jhyt_m34_festival` 绿（默认值行为不变）。

**风险**：低-中。`GameClock` 是 `Copy` 且被 `clock_lua` 直接读；存档只存 day/minute（游戏侧），
不存 season，加字段不影响旧档。除零/0 季必须拒绝。

### 5.3 P5-03 两个缺失机制（条件开工，4–6 天）

只有 §7 的触发条件成立才开工。**这一包不搬任何模块**，只造地基。

1. **引擎内置 Lua stdlib**
   - 新目录 `engine/lua/`（std lib 源码）+ `rustlua-scripting/src/builtin_lua.rs`：
     用 `include_str!` 嵌入，`package.preload` 注册，按 `__rustlua.<模块>` 命名空间；
   - 装配点：`script/api/mod.rs::register_all` 的 `installers.install(lua)` **之后**；
   - 沙箱：stdlib 禁止 `io`/`os`（wasm 没有文件系统，桌面有会诱使脚本走错路）；
   - 失败策略：VM 构造期 fail fast，错误信息带模块名与行号。
2. **跨 VM 状态桥**
   - `Handle<StateBag>`：注册式"名字 → JSON"，重载**前**快照、新 VM 装完 stdlib **后**恢复；
   - 序列化用 `serde_json`（Lua 表 → JSON 已有 `lua_value_to_json` 先例，见 `rustlua-lua-api::values`）；
   - 明确限制：函数/闭包/协程不可迁移（与 docs/19 同口径），只迁数据。

**判据**

- 一个只存在于 stdlib 的探针模块在 **4 个宿主**可见：桌面运行时、编辑器预览、headless 示例、
  wasm（`verify-wasm-lua.ps1` 或微信 harness）；
- 热重载后 `inv.get` 的 20 格内容逐格不变（新测试：改脚本 → 重载 → 断言）；
- `api_surface` 零 diff（探针模块不在 `_G` 暴露，或走 `__rustlua.*` 前缀不进口径）。

**风险**：中-高。stdlib 执行失败会让所有脚本起不来（必须 fail fast 且可定位）；
与工程 `require` 重名；wasm 体积（预计 < 20 KB，可接受）。

### 5.4 P5-04 按顺序下沉（条件开工，2–3 周）

**顺序与每步判据**（前一步全绿才做下一步）：

| 步 | 搬什么 | 估时 | 判据 |
|---|---|---|---|
| 0 | 给 `inv`/`save`/`health`/`grid` 补**特征化**契约用例（先写期望值，再搬） | 1 天 | 契约覆盖从 20 条涨到 ≥ 40 条，且全是当前实现的事实 |
| 1 | `shop`（2 函数） | 0.5–1 天 | 契约 + `demo_gameplay` 绿；Rust 实现删除后 surface 零 diff（Lua 版同名同形补上） |
| 2 | `grid`（4 函数） | 0.5–1 天 | 同上；`docs/06` 的坐标约定仍单一来源（std lib 内唯一实现） |
| 3 | `calendar` 的季节/年推导（2 函数） | 1–2 天 | 默认值下 `calendar.now()` transcript 零 diff；自定义日历用例过 |
| 4 | `item`/`inv`/`container`（21 函数） | 5–8 天 | 热重载后背包不丢；`ui/inventory_ui.lua` 每帧路径无回退；`jhyt_m19_ui` / `jhyt_m20_gamepad` / `jhyt_m22_save_isolation` / `jhyt_m67_inventory_prefab` 四条绿 |

**不做**：`save` / `data` / `map.state` / `health` / `tag` / `settings`（§3/§4 已定）。

**风险**：高（第 4 步）。回滚方式：每一步是一次独立提交，删除 Rust 实现放在该步**最后**；
任何一步判红就停在该步，Lua 版与 Rust 版可以并存一轮（surface 由 Lua 版覆盖、Rust 版标 deprecated）。

### 5.5 P5-05 文档与 ADR（随批；文档已落地，ADR 待单独确认）

- `docs/18`：模块表加"层"列；新增"引擎 stdlib vs 工程 scripts"一节的入口（若 P5-03 落地）；
- `docs/02`：按 §9 的修订稿更新 ADR-0017（**改 ADR 是治理动作，需单独确认**）；
- `tasks/README.md`：`plan/` 清单加 P5；
- `CHANGELOG`：本文是计划不是能力增量，**不写**；P5-01/02 落地时按模板补。

---

## 6. 验收门（命令级）

```powershell
# 仓库根 → engine/（cargo 工作区在 engine/）
cd engine

# ① 单测（P5-01/02 必跑；P5-04 每步必跑）
cargo test -p rustlua-gameplay
cargo test -p rustlua-scripting

# ② API 面与行为契约（两个门都在下一条里；`--write` 只在**有意**变更语义/API 时手动跑）
cargo test -p rustlua-scripting api_surface
# 确认要改基线后，才取消下面两行的注释：
# cargo run -q -p engine --example dump_lua_api -- --write
# cargo run -q -p engine --example run_lua_contract -- --write

# ③ 端到端（受影响的三条；P5-04 第 4 步另加 jhyt_m67_inventory_prefab）
cargo run -q -p game --example jhyt_m17_shop
cargo run -q -p game --example jhyt_m22_save_isolation
cargo run -q -p game --example jhyt_m29_season_weather

# ④ 数据级校验（与 verify.ps1 同一入口）
cargo run -q -p game -- --project ../examples/game_jhyt --check

# ⑤ 门
powershell -NoProfile -ExecutionPolicy Bypass -File ..\scripts\verify\check-decoupling.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File ..\scripts\verify\check-docs.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File ..\scripts\verify\verify.ps1 -Quick
# 收尾全量（clippy + 全测试 + 构建 + 编辑器实机步骤）
powershell -NoProfile -ExecutionPolicy Bypass -File ..\scripts\verify\verify.ps1
```

> `verify.ps1 -Quick` 是"这棵树现在是不是红的"的便宜读数；**收尾必须跑全量**，
> 不接受用 `-Quick` 代替。

---

## 7. 触发条件（什么时候改判）

任一条成立，P5-03/04 从"不排期"转为"开工"：

1. **出现第二个体裁差异显著的工程**（ARPG / 卡牌 / 模拟经营之外的品类），且它开始绕开
   `inv` 的固定槽位模型或 `calendar` 的 28×4 —— 判据：该工程 scripts 里出现自己的
   `inventory` 模块且不再调用 `inv.*`。
2. **热重载状态桥因别的原因必须先做**（例如编辑器要支持"改脚本保留运行时状态"的更通用能力）
   —— 判据：P5-03 的第 2 个机制被另一个任务立项。
3. **`inv`/`item` 的补丁频率超过引擎其他 API** —— 判据：连续两个版本里 `gameplay.rs` +
   `inventory.rs` 的改动次数排进 core 前 20%。
4. **引擎 API 面继续膨胀到维护不动** —— 判据：模块数 > 50 或函数数 > 350，且新增集中在
   玩法语义（而非渲染/编辑器）。

反向条件（**不要**开工）：只有一个工程、且它没有绕开现有模型；或 stdlib 机制没有别的消费者
（为 47 个函数造一套分发机制，本身就不划算）。

---

## 8. 风险与回滚

| 风险 | 影响 | 回滚 |
|---|---|---|
| P5-04 第 4 步搬 `inv` 后热重载丢背包 | 开发者体验回退（每次改脚本重置） | 保留 Rust 实现一个版本，Lua 版只做 facade；或回退该步提交 |
| stdlib 与工程 `require` 重名 | 工程脚本静默加载错版本 | 命名空间 `__rustlua.*`；重名加载期报错 |
| 两套实现漂移 | 历史已发生（57 个覆盖点） | 行为契约 transcript 是唯一裁判；双跑一轮再删旧实现 |
| API 面基线被"顺手"改大 | 契约失去意义 | `dump_lua_api --write` 必须在提交说明里逐条列出增删与理由 |
| 参数化日历改坏默认值 | 所有季节相关玩法错位 | 默认值单测 + `jhyt_m29_season_weather` / `jhyt_m34_festival` 双门 |

---

## 9. ADR-0017 修订建议（**未落盘**，改 ADR 需单独确认）

> 现状：[ADR-0017](../../docs/02-technical-decisions.md) 写"把农场循环需要的数据结构作为引擎核心
> 数据模型实现"，代价一栏自认"扩大了 core 表面积"，重新评估条件是"玩法数据模型与通用引擎
> 职责冲突，或 Lua 层使用率低导致抽象过度"。2026-10-20 的评估认为**冲突已经局部成立**
> （日历 28×4、固定槽位背包），但**没有**到"使用率低"（`inv` 115 个调用点、`data` 589）。

建议替换为：

```markdown
## ADR-0017：玩法数据模型下沉为 Lua API（2026-10-20 修订）

- 状态：Accepted（修订）
- 日期：2026-10-20
- 决定：引擎只保留三类玩法相关能力：
  ① 持久化与平台 IO（save 的格式/原子写/槽位、data 表加载与 schema、map.state 的持久键）；
  ② 需要跨 VM 生命周期或引擎身份的运行时状态（clock 的推进与状态、inventory/item 的槽位模型、
     health/tag 的 ECS 组件）；
  ③ 通用换算（grid）。
  体裁相关语义（日历的季节划分、商店目录、未来的装备/属性体系）由工程 Lua 或引擎内置 Lua
  stdlib 实现；引擎不预置第二套体裁假设。
- 判据：同一份 Rust API 能否被两个体裁差异显著的工程复用而不 fork？能 → 留；不能 → 参数化或下沉。
- 代价：引擎侧保留 1,968 行数据模型；新增"内置 Lua stdlib + 跨 VM 状态桥"两个机制（仅在需要
  下沉时建设）；`container`/`interact`/`tag` 标废弃但保留到 v0.2。
- 非目标：不在引擎侧实现具体玩法规则（交易资金、任务、剧情）；不为单一工程预置体裁语义。
- 重新评估条件：出现第二个体裁差异显著的工程；或 P5-03 的状态桥落地使 Lua 持有运行时状态
  不再有额外代价；或 API 面 > 350 个函数且新增集中在玩法语义。
```

---

## 10. 明确不做

- **不整体搬迁**：`save`/`data`/`map`/`settings`/`health`/`tag` 留 Rust（§3/§4）。
- **不为单一工程建 stdlib**：P5-03 只在 §7 条件成立时开工。
- **不删 `container`/`interact`/`tag`**：只标废弃；删除需要 v0.2 兼容窗口与第二次审计。
- **不改 crate 名**（P5-01b 等 P1 收口）。
- **不动无关工作树**：P1 已于 2026-10-21 收口归档；本轮只改 P5 相关源码/文档/基线，未碰
  工作树里原有的 `docs/69`、`scripts/release/sign-windows.ps1` 与未跟踪的 `sign-local.ps1`。
- **不把"性能"当搬迁理由**：反方向也不把"性能"当不搬理由（§4.4）。

---

## 11. 未完成项 / 已知限制

- [x] §2 的读数已按 P1 收口后的**当前工作树**复核（2026-10-21）；§2.3 调用点已复跑。
      残留：工作树仍含 P2 的未提交文件（`docs/69`、`scripts/release/*`），与本任务无关。
- [x] 运行期测试条数已取：`rustlua-gameplay` **68**、`rustlua-scripting` **120**（2026-10-21）。
- [ ] 契约覆盖缺 `inv`/`save`/`health`/`grid`/`map`/`data`（0 条）；P5-04 第 0 步必须先补。
- [ ] §2.3 是静态调用点，不是运行期频率；若将来要论证性能，需 `jhyt_m19_soak` 级别的读数。
- [x] `calendar.configure` 的 +1 函数理由已写入 `docs/18`、CHANGELOG 与本文「实现记录」
      （形状不变、唯一新增；非法值不改状态）。
- [x] CHANGELOG 已写（P5-01/02 是能力增量）。
- [ ] **ADR-0017 修订未落盘**：§9 只是修订稿，改 ADR 是治理动作，需用户单独确认；
      `docs/02` 当前仍是旧口径。
- [ ] **已知外部红**：`jhyt_m29_season_weather` 在 M29-01⑦ 的 docs/48 资产普查处判红
      （引用 1065 / 实际 1066，`peach.png` 来自 HEAD `756d8962`），错误自述“不是功能回归”；
      P5 未改资产，按既有 P2 打包待办处理，不在本任务内改 docs/48。
- [ ] **环境备注①**：zh-CN PowerShell 5.1（代码页 936）会把 `assetc` 的 UTF-8 中文 stderr
      解码错位，导致 `test-release-closure.ps1` 的「容器不在」断言误判；`chcp 65001` 下通过。
- [ ] **环境备注②**：`test-git-hooks.ps1` 的 B10 前提要求 git 执行 hook 时自带 grep/sed/tr；
      本机 mingw64 git 在 PATH 缺 `C:\Program Files\Git\bin`（`sh.exe`）时 hook 直接
      “cannot spawn .pre-commit”，把该目录加回 PATH 即绿。两条都**与本任务无关**，
      记录以免复跑时误归因。
- [x] **环境备注③（已解除）**：2026-10-22 GitHub Desktop 把整棵未提交工作树存入 `stash@{0}`
      （含 P5 全部改动与 P2 的 `docs/69`、`scripts/release/*`），主树转入 P3-R01。
      P5 的全量验收在隔离树 `E:\RustLua-p5-verify` 完成；2026-10-23 已按路径取回并提交 `main`
      （`7ef0eebe` + `24442fef`）。**不要整条 pop `stash@{0}`**（会夹带 P2 文件并与 P3-R01
      冲突）；隔离树与补丁仅作历史证据。

---

## 实现记录

### P5-01 账本与废弃标记（2026-10-21）

- `rustlua-gameplay/src/lib.rs`：crate doc 改为五域账本（玩法模型 1,968 / 设置 1,295 / 存档 761 /
  世界数据 568 / 绑定胶水 237 / `lib.rs` 46 = 4,875 非空行，玩法占 40.4%），写清“留 Rust /
  参数化 / 废弃候选”三组裁定、判定口径与复现命令；`pub mod` 声明原样保留。
- `docs/18-lua-api.md`：农场 API 表新增“层”列（18 行全部填写）；新增“分层口径”段；
  `container` / `interact` / `tag` 标注出货工程 0 静态调用点与 v0.2 删除窗口。
- `gameplay.rs` 的 `DAYS_PER_SEASON` / `SEASONS_PER_YEAR` 与 `GameClock` 文档注明
  “农场体裁默认值，不是引擎语义”。

### P5-02 日历参数化（2026-10-21）

- `GameClock` 新增 `days_per_season` / `seasons_per_year`（Default 28/4）；
  `season()` / `year()` / `day_of_season()` 改读字段；新增 `days_per_year()` 与
  `configure_calendar()`；新增 `CalendarConfigError`（0 天 / 0 季 / 乘积溢出）。
- `clock_lua.rs` 新增 `calendar.configure(options)`：缺字段、非正整数、乘积溢出都在**改状态前**
  报错；`calendar.set` 的日/季上界改读当前配置，绝对天数用 `checked_mul` / `checked_add`
  并在超出 `u32::MAX` 时拒绝。
- 为什么用表参数而不是位置参数：两个维度必须同时成立，表参数能一次校验完再落状态，
  也避免“只改一半”；`calendar.now()` 的返回形状保持不变。
- `game_web/src/host_api.rs`：保留为诊断镜像/死码，模块文档注明不同步 `calendar.configure`
  且固定 28×4；web 实际走 Rust wasm Lua 运行时，不受影响。

### 基线与文档

- `lua_api_surface.json` 重生成：42 个模块表不变，函数 295→296（唯一新增 `calendar.configure`）。
- `lua_api_contract.lua` 新增 2 条（`calendar.configure.custom` / `calendar.configure.invalid`），
  transcript 只追加 2 行；用例先快照再恢复日期，避免字母序（`configure.*` 在 `now.*` 之前）
  污染后续用例。
- docgen：`calendar.md` 补正文（不再是 TODO 桩），`baseline.json` 更新为
  `funcs=296 / stubs=277 / documented=19`；`clock.md` / `day.md` 仅绑定行号刷新。
- 示例工程 `examples/game_api/scripts/demo_gameplay.lua` 补 `calendar.configure` 的真实调用
  （7×3 收紧后 `calendar.set(8,1,1,0)` 被拒，再恢复 28×4），全量测试抓到的
  `sample_project_demos_cover_every_lua_api` 缺口已闭合。
- `CHANGELOG.md`、`tasks/README.md`、本文件读数已同步。

### 隔离验收（2026-10-22）

- 主树被 P3-R01（render tonemap）占用，原工作树被 GitHub Desktop 整体存入 `stash@{0}`；
  在 `E:\RustLua-p5-verify`（detached HEAD `5f1b0533`）按路径从 stash 恢复 P5 文件，
  完成全量 `verify.ps1`（含 clippy / workspace tests / 构建 / 编辑器 GUI smoke）。
- 隔离树为通过门额外修的两处既有问题（M75 的 rustdoc 链接、CRLF 检出的 AI 基线）
  只属于本机验收环境，不随 P5 补丁交付。

### 未做 / 未触发

- P5-03/P5-04 按 §7 未触发：仍只有一个体裁工程；API 面 42 模块 / 296 函数（< 50 / < 350）；
  未建 stdlib、未搬模块、未删 `container` / `interact` / `tag`。
- ADR-0017 修订稿（§9）未落盘——需用户单独确认。

## 验收证据

- [x] `cargo fmt --all -- --check` 通过（2026-10-21）
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过
      （修掉新增测试触发的 `field_reassign_with_default`）
- [x] `cargo test -p rustlua-gameplay`：**68 passed**
- [x] `cargo test -p rustlua-scripting`：**119 lib + 1 集成 = 120 passed**（含 API 面与契约两条门）
- [x] 端到端（2026-10-22 隔离树复跑）：`jhyt_m17_shop`（`shops=3 money=925`）、
      `jhyt_m22_save_isolation`（`providers=16 rows=8 day=2`）、
      `jhyt_m34_festival`（`event_days=24/112 windows=8x3`）exit 0；
      `jhyt_m29_season_weather` 的季节/天气各步绿，止于既有的 docs/48 资产普查
      （1065→1066，见 §11），不是 P5 回归
- [x] `cargo run -q -p game -- --project ../examples/game_jhyt --check`：`game_check_ok`
- [x] Debug 构建通过
- [x] `scripts/verify/check-decoupling.ps1` 与 `check-docs.ps1` 绿
- [x] 全量 `verify.ps1`：**exit 0 / `Quality gate passed`**（2026-10-22，隔离工作树
      `E:\RustLua-p5-verify`，HEAD `5f1b0533` + P5 补丁；日志
      `E:\RustLua-p5-verify\artifacts\logs\quality\verify-20261022-104624.log`）。
      环境：`chcp 65001`、PATH 前置 `C:\Program Files\Git\bin`（hook 需要 `sh.exe`）、
      使用隔离树自己的 `engine/target`（复用主 target 会让编辑器字体测试按 exe 路径
      找到主工作区）。隔离树额外修了两处与本任务无关的既有问题才让门可跑：
      `rustlua-editor-ai/src/provider.rs` 的冗余 intra-doc 链接（M75）与
      `ai_tool_surface.json` 的 CRLF 检出（内容哈希未变），不随 P5 补丁交付。
- [x] `verify.ps1 -Quick`：**exit 0 / `Quick gate passed (PARTIAL)`**（2026-10-22，隔离树）
- [x] 示例工程 API 覆盖门：`sample_project_demos_cover_every_lua_api` 9/9 绿
      （`demo_gameplay.lua` 真实调用 `calendar.configure`，7×3 越界被拒后恢复 28×4）
- [x] 注释符合 [章程 §9.7 代码与注释规范](../../docs/00-project-charter.md)（精简中文）
- [x] 文档与 CHANGELOG 已更新

## 归档记录（2026-10-23）

- 归档性质：**收口归档**（非“主树验收通过”）。P5-01/P5-02/P5-05 文档已落地；
  P5-03/P5-04 按 §7 未触发、不排期，不是欠账。
- 已入库：`7ef0eebe feat(p5): 收口玩法层边界与日历体裁参数化`（17 文件）与
  `24442fef fix(p5): 恢复被 P5 提交覆盖的 M75 文档条目`；`main` ahead 2，**未推送**。
- 验收来源：隔离树 `E:\RustLua-p5-verify`（detached `5f1b0533` + P5 补丁）2026-10-22 全量
  `verify.ps1` exit 0、`-Quick` exit 0（日志见「验收证据」）。**主树 HEAD 未复跑全量门**，
  归档时工作树由 P3-R02 占用，当前工作树读数不能当作 P5 读数。
- 隔离树绿门的两处非 P5 修复（M75 rustdoc 链接、AI 基线 CRLF 检出）不随 P5 交付；其中
  `rustlua-editor-ai/src/provider.rs` 的 rustdoc 修复在归档时仍只在工作树未提交。
- 未收口项（只登记，不另开包）：
  1. **ADR-0017 修订未落盘**：§9 修订稿待用户单独确认；2026-10-23 用户裁定“先归档，ADR 待办”，
     `docs/02` 仍为旧口径。
  2. P5-03/P5-04 未触发：仍只有一个体裁工程；API 面 42 模块 / 296 函数（< 50 / < 350）。
  3. `container`/`interact`/`tag` 只标废弃未删除，需 v0.2 兼容窗口。
  4. `jhyt_m29_season_weather` 的 docs/48 资产普查外部红（1065→1066），不是 P5 回归。
  5. P5 两笔提交未推送到远端。
