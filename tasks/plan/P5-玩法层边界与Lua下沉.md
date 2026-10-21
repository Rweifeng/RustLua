# 任务：P5 玩法层边界（gameplay 数据模型的 Lua 化裁定与体裁假设收口）

- 任务 ID：P5-00（总包；子项 P5-01 … P5-05）
- 状态：**裁定已给、未开工**（2026-10-20）。P5-01/P5-02 建议做；P5-03/P5-04 **只在 §7 的触发条件下开工**，不默认排期
- 关联：[docs/62 成熟引擎差距分析](../../docs/62-成熟引擎差距分析.md) §6、
  [docs/18 Lua API](../../docs/18-lua-api.md)、
  [docs/19 Lua 热重载](../../docs/19-lua-hot-reload.md)、
  [docs/02 ADR-0017](../../docs/02-technical-decisions.md)、
  [P1 降耦合收口](P1-降耦合收口.md)（前置：同一批文件在途）、
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

1. **玩法规则已经在 Lua**：`examples/game_jhyt/scripts` **155 个文件 / 约 3.74 万非空行**
   （全文 39,767 行；种植、战斗、商店、任务、对话、UI）。Rust 侧没有任务/种植/战斗/对话/经济规则。
2. **Rust 侧可称为 gameplay 的只有 703 行 / 46 个 Lua 函数**：
   `gameplay.rs` 321 + `clock_lua.rs` 291 + `grid.rs` 91；这 46 个函数占 **42 个模块 / 295 个函数**
   的 **15.6%**（[lua_api_surface.json](../../engine/crates/rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json)）。
3. **`rustlua-gameplay` 这个 crate 80% 不是玩法**：settings 1,295 行（37.6%，编辑器与 input
   共用）+ save 706（20.5%）+ world_data 524（15.2%）+ 绑定胶水 202（5.9%），合计 3,441 行。

所以问题不是"能不能分层"（已经分完了），而是"**跨 VM 生命周期的可变状态归谁**"。
§4 的四条边界决定：`save`/`data`/`map`/`health`/`tag` **必须**留 Rust；`inv`/`item`/`clock`
留 Rust 是**为了热重载与固定步进**（技术上是能搬的）；只有 `shop`/`grid`/`calendar` 的季节
常量属于"该搬的体裁假设"。

**建议**（不做 P5-03/04 也能交付）：

- **P5-01（1–2 天）账本与命名**：crate 内四个域写清；`docs/18` 标出"只在 demo/契约里用"的 API。
- **P5-02（2–3 天）体裁假设参数化**：日历 `28 天/季 × 4 季`；背包容量/堆叠已有 `item.define` 数据面。
- 只有 §7 的触发条件出现，才开 **P5-03（两个缺失机制）→ P5-04（按顺序下沉）**。

---

## 1. 要回答的问题

| # | 问题 | 现在能回答吗 | 依据 / 缺口 |
|---|---|---|---|
| 1 | 玩法到底写在哪？Rust 还剩多少玩法？ | **能** | 本文件 §2.1/§2.2；剩下 703 行数据模型 |
| 2 | 把 `inv`/`clock` 搬进 Lua，会丢什么？ | **能** | §4.1 热重载换 VM；§4.3 ECS 身份 |
| 3 | Rust 侧哪些"玩法"其实不是玩法？ | **能** | §2.1：settings + save + world_data 占 80% |
| 4 | 搬过去要补什么机制？多久？ | **能** | §5 P5-03/P5-04：内置 Lua stdlib + 跨 VM 状态桥 |
| 5 | 现在该不该搬？ | **能（裁定）** | §0/§7：不整体搬；先参数化体裁假设 |
| 6 | 怎么证明搬了没退化？ | **能** | §6：API 面快照 + 行为契约 + 100 个示例 + 三道门 |
| 7 | 多贵的 Rust 面值得为它建 stdlib？ | **不能（缺量尺）** | 需要"第二体裁工程"或一次 P5-03 探针；本文件不臆造 |

---

## 2. 现有事实（2026-10-20 实测）

### 2.1 代码面

| 域 | 文件 | 非空行 | 是不是玩法 |
|---|---|---|---|
| 设置 | `settings.rs` | **1,295** | ❌ 工程 `settings.toml` 的读/写/迁移；编辑器 `menu.rs`/`input_bindings.rs`/`preview_session.rs` 都读它 |
| 存档 | `save.rs` 301 + `save_lua.rs` 289 + `save_store.rs` 116 | **706** | ❌ v1/v2 格式、原子写、平台槽 |
| 世界数据 | `world_data_lua.rs` 456 + `world_data_state.rs` 68 | **524** | ❌ `data.load` 的缓存/schema、`map.state` 的持久键 |
| 玩法模型 | `gameplay.rs` 321 + `clock_lua.rs` 291 + `grid.rs` 91 | **703** | ✅ 只有这一块 |
| 绑定胶水 | `audio_lua.rs` 72 + `input_lua.rs` 130 | **202** | ❌ 别的领域的注册入口 |
| crate 级 | `lib.rs` 11 | 11 | — |
| **合计** | 12 个文件 | **3,441**（全文 3,859） | 玩法占 **20.4%** |

> 复现：`Get-ChildItem engine\crates\rustlua-gameplay\src -File | ForEach-Object { "{0,5} {1}" -f (Get-Content -LiteralPath $_ | Measure-Object -Line).Lines, $_.Name }`
> （`Measure-Object -Line` 不计空行；合计行的"全文"是 `ReadAllLines().Count`）

### 2.2 API 面

| 事实 | 读数 |
|---|---|
| Lua 全局模块表 | **42 个** |
| Lua 函数 | **295 个** |
| 玩法味模块的函数 | **46 个（15.6%）**：`item` 2 / `inv` 15 / `container` 4 / `shop` 2 / `clock` 6 / `calendar` 2 / `day` 2 / `health` 3 / `tag` 2 / `interact` 4 / `grid` 4 |
| 服务类（不属于玩法） | `save` 9 / `map` 9 / `data` 4 |

> 复现：`$j = Get-Content -Raw engine\crates\rustlua-scripting\src\api_surface_snapshot\lua_api_surface.json | ConvertFrom-Json; $j.tables.Count; ($j.tables | ForEach-Object { $_.funcs.Count } | Measure-Object -Sum).Sum`

### 2.3 使用面（`examples/game_jhyt/scripts` 静态调用点）

| 模块 | 调用点 | 说明 |
|---|---|---|
| `inv` | **115** | 背包主路径；`ui/inventory_ui.lua:126` 每帧一次 `inv.get` |
| `item` | 31 | `item.define` 在数据加载期 |
| `shop` | 36 | 目录 + 价格查询 |
| `container` | **0** | 只在 `game_api` demo 与契约用例里 |
| `clock` / `calendar` / `day` | 44 / 25 / 15 | 时间推进与季节 |
| `health` | 5 | 战斗 |
| `tag` | **0** | 只在 demo/契约里（`interact.query` 内部读它） |
| `interact` | **0** | 只在 demo/契约里 |
| `grid` | 22 | 格子换算 |
| `save` | 9 | 全部集中在 [save_manager.lua](../../examples/game_jhyt/scripts/core/save_manager.lua)：一个 blob + 版本 + 摘要 |
| `map` / `data` | 69 / **589** | 场景持久状态 / 数据表加载 |

> 复现（模块循环，按 `\b<模块>\.` 计）：`foreach ($m in 'item','inv','container','shop','clock','calendar','day','health','tag','interact','grid','save','map','data') { ... }`
> 数字是**静态调用点**，不是运行期频率；性能结论见 §4.4。

### 2.4 门与测试面

| 量具 | 读数 / 入口 |
|---|---|
| `rustlua-gameplay` 单测 | **52 个 `#[test]`**（静态计数，2026-10-20 工作树；P1 ⑤-4c 记录的运行期读数是 47 条，差集来自 ⑤ 新增的 `clock_lua`/`save_lua`/`world_data` 测试） |
| `rustlua-scripting` 单测 | **111 条**（P1 记录） |
| API 面快照门 | `lua_api_surface_matches_baseline`：42 模块 / 295 函数逐项比对 |
| 行为契约门 | `lua_api_contract.lua` + `lua_api_contract_transcript.txt`，**玩法相关 20 条**（clock 6 / item 5 / calendar 3 / interact 2 / day 1 / container 1 / shop 1 / tag 1；**`inv`/`save`/`health`/`grid`/`map`/`data` 为 0 条**） |
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
| `calendar` 的季节/年 | 2 | **搬常量** | `28 天/季 × 4 季` 是农场体裁假设，不是引擎语义 | P5-02（参数化）/ P5-03（下沉） |
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

### 5.1 P5-01 账本与废弃标记（建议做，1–2 天）

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

### 5.2 P5-02 体裁常量参数化（建议做，2–3 天）

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

### 5.5 P5-05 文档与 ADR（随批，0.5 天）

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
（为 46 个函数造一套分发机制，本身就不划算）。

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
- 代价：引擎侧保留 703 行数据模型；新增"内置 Lua stdlib + 跨 VM 状态桥"两个机制（仅在需要
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
- **不动在途工作树**：P1 ⑤ 的未提交改动不碰；本任务提交时只暂存本文件与 P5-01/02 的改动。
- **不把"性能"当搬迁理由**：反方向也不把"性能"当不搬理由（§4.4）。

---

## 11. 未完成项 / 已知限制

- [ ] 本文所有读数来自**未提交工作树**（P1 在途），不是 HEAD 干净快照；P1 收口后需复核 §2.3 的调用点数。
- [ ] `rustlua-gameplay` 52 个 `#[test]` 是静态计数；运行期条数本轮没取到（`cargo test` 被并发
      的 `cargo fetch` 持有 package-cache 锁），开工前先跑一次 `cargo test -p rustlua-gameplay`
      并把读数回填 §2.4。`rustlua-scripting` 111 条同属 P1 记录值，未本轮复核。
- [ ] 契约覆盖缺 `inv`/`save`/`health`/`grid`/`map`/`data`（0 条）；P5-04 第 0 步必须先补。
- [ ] §2.3 是静态调用点，不是运行期频率；若将来要论证性能，需 `jhyt_m19_soak` 级别的读数。
- [ ] P5-02 的 `calendar.configure` 会增加 1 个 API 函数，需在 `docs/18` 与基线的提交说明里
      写明"为什么值这一个函数"。
- [ ] 本文未写 CHANGELOG（计划不是能力增量）。

---

## 实现记录

（开工后回填：改了哪些文件、为什么这样设计、每步的判据读数）

## 验收证据

- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过
- [ ] 相关测试通过（§6 ①–③）
- [ ] Debug 构建通过
- [ ] Demo / 手工步骤可复现（§6 ③–④）
- [ ] 注释符合 [章程 §9.7 代码与注释规范](../../docs/00-project-charter.md)（精简中文）
- [ ] 文档与 CHANGELOG 已更新
