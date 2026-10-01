# 任务：WP11 script.rs 拆分（12k 行单文件 → `core/script/` 子目录）

- 任务 ID：WP11（工作包级）
- 状态：**阶段 0–4 代码完成，待验收**（落地结构与偏差见 §13；验收通过后本文移入 `tasks/done/`）
- 关联里程碑：M33（沿用当前推进节奏）
- 创建日期：2026-09-30｜实现日期：2026-10-01
- 上游依据：[docs/37](../../docs/37-引擎与游戏工程复核及优化完善计划.md) 引擎改进台账、
  [docs/54](../../docs/54-2D3D统一优化方案与现状对比.md) §4.1/§8（每实体脚本与相机快照的落地记录）

## 1. 目标

把 `engine/crates/engine/src/core/script.rs`（11963 行，仓库最大单文件）拆成 `core/script/` 子目录，
**行为零变化**：Lua 侧 API 面（41 个全局模块表 / 272 个绑定函数）逐字节不变，
Rust 侧对外可见符号（7 个）不变，`editor`/`game`/`game_web`/`assetc` 一行不改。

不做的事见 §8「范围控制」。

## 2. 现状解剖（实测行段，认领后仍需复核）

| 区段 | 行范围 | 行数 | 性质 |
|---|---|---|---|
| `Transform3dSetArgs` 类型别名 | 45-57 | 13 | 参数类型（跟 transform3d 走） |
| `ScriptState` 结构体（~40 字段） | 59-201 | 143 | **全运行时共享状态** |
| `ActionRepeatState` + `impl ScriptState` | 203-272 | 70 | `font_probe` / `action_repeat` |
| `LuaBridgeError` / `ScriptFingerprint` / `ScriptDiagnostic` / `ScriptDebugger` | 273-423 | 151 | 错误与诊断 |
| `LuaRuntime` 结构 + `probe_startup_stage` | 424-460 | 37 | VM 生命周期 |
| `impl LuaRuntime` | 461-1831 | 1371 | 场景实例化、`spawn_entity_components`(744-1060)、物理步进、切场景、热重载 |
| 自由 helper | 1832-2460 | 629 | `api_error`/`entity_from_lua`/`table_*`/lua↔json/`*_inventories`/`solid_cell_colliders` |
| **`register_api`** | 2461-7389 | **4929** | 41 模块表 / 272 `create_function`，单函数 |
| `world_error` … `parse_mouse_button` | 7390-7481 | 92 | 尾部 helper |
| **`mod tests`** | 7482-11963 | **4482** | 测试 |

痛点集中在两块：`register_api` 一个函数 4929 行，测试 4482 行，合计占文件 78%。

### 2.1 41 个模块表的实测规模（决定分组）

`globals().set(...)` 的行号即各模块块尾：

```
log 2484(23)  time 2492(7)  clock 2564(71)  calendar 2608(43)  day 2637(28)
input 3076(438)  entity 3158(81)  sprite 3225(66)  quad 3271(45)
transform 3526(254)  transform3d 3651(124)  mesh 3694(42)
camera 3801(106)  camera3d 3910(108)  camera2d 4071(160)  light 4145(73)
health 4187(41)  tilemap 4400(212)  grid 4437(36)  tag 4473(35)
interact 4562(88)  animation 4707(144)  world_text 4739(31)  animation3d 4832(92)
visibility 4872(39)  physics 5074(201)  physics3d 5225(150)  particles 5302(76)
audio 5456(153)  scene 5477(20)  map 5691(213)  asset 5711(22)  data 5816(105)
ui 6786(970，起点 5817)  item 7041(255)  inv 7090(48)  container 7130(39)
shop 7276(145)  save 7311(34)  app 7386(74)  event 7386(~40)
```

`ui` 一块 970 行，且 **5951 行就 `globals().set("ui", &ui)`、之后继续 `ui.set(...)` 到 6786**
（mlua `Table` 是引用式克隆，中途注册后仍可继续填充）。这是拆分时唯一需要特别处理的注册形态。

### 2.2 对外可见面（实测，跨 crate 引用）

```
LuaRuntime · LuaBridgeError · read_project_script · install_web_require
probe_startup_stage · entity_lua_id · parse_key
```

`game_web` 用后四个；`engine` 内其他模块只有文档链接（`input.rs:1230` 的
`script::ScriptState::set_input`、`host/keymap.rs:5` 的 `script::parse_key`）和一处真实调用
（`host/runtime_scene.rs:7811` 的 `script::entity_lua_id`）。
→ `script/mod.rs` 用 `pub use` 重导出这 7 项即可，跨 crate 零改动。

## 3. 关键工程事实（决定目录形态）

**Rust 可见性规则：私有项对定义它的模块及其所有后代模块可见。**

- `ScriptState` 的 40 个字段全部私有。**把它留在 `script/mod.rs`**，则
  `script::api::*`、`script::runtime` 等所有子模块作为后代天然可读写这些字段，**属性零改动**。
- 若移进 `script/state.rs`，字段的私有域变成 `script::state`，兄弟子模块 `script::api::*` 访问不到
  → 必须给 40 个字段逐个加 `pub(super)`。

裁定：**阶段 1 宁可让 `mod.rs` 背 143 行结构体 + 逐字段中文注释，也不扩大可见性。**
这些字段注释是资产（记录着「为什么是这个字段、谁消费」），搬动时逐字保留。
若后续要瘦身 `mod.rs`，另开一个纯机械 commit 加 `pub(super)`，不与逻辑改动混在一起。

同理，`values.rs`/`error.rs`/`require.rs` 里的 helper 用 `pub(super)`（私有域 = `script`，后代可用）。

## 4. 目标结构与行数预算

```
core/script/
  mod.rs              ~230   ScriptState + impl ScriptState + pub use 外表面
  error.rs            ~180   LuaBridgeError / api_error / world_error / ScriptDebugger
  runtime.rs          ~280   LuaRuntime 结构 + VM 生命周期 + 热重载 + load_candidate
  scene_spawn.rs      ~380   instantiate_scene / spawn_entity_components / process_ui_clicks
  step.rs             ~200   step_physics* / writeback* / step_particles / step_clock / sync_kinematic*
  transition.rs       ~120   process_scene_transition + day 钩子
  require.rs          ~150   read_project_script / install_encoded_require_searcher
                             / install_web_require / file_fingerprint / atomic_write
  values.rs           ~220   entity_from_lua / entity_lua_id / require_alive / table_vec
                             table_ray / table_i32_pair / format_lua_value / lua↔json 三件
  api/
    mod.rs             ~70   register_api → 顺序调度
    globals.rs        ~290   log time clock calendar day app event
    input.rs          ~470   input（+ parse_key_strict / resolve_input_settings_path
                             / settings_root / push_input_diagnostic / finite_ui_rect）
    entity.rs         ~290   entity tag health visibility interact
    transform.rs      ~400   transform transform3d（含 Transform3dSetArgs）
    renderable.rs     ~270   sprite quad mesh light world_text
    camera.rs         ~390   camera camera2d camera3d（+ camera_view / camera3d_view）
    tilemap.rs        ~330   tilemap grid（+ solid_cell_colliders / refresh_tilemap_physics
                             / require_positive_tile_size）
    animation.rs      ~250   animation animation3d
    physics.rs        ~440   physics physics3d particles
    audio.rs          ~170   audio（+ parse_audio_bus）
    world_data.rs     ~370   scene map asset data
    ui.rs             ~340   ui：文本 / 图片 / measure / 可见性
    ui_panel.rs       ~330   ui：panel / grid / images / layout
    ui_nav.rs         ~320   ui：drag / focus / scroll / click
    item.rs           ~260   item
    inventory.rs      ~150   inv container（+ inventory_index / inventory_clone
                             / swap_inventories / move_inventories）
    shop.rs           ~150   shop
    save.rs            ~80   save（+ save_path_for_slot）
```

统一签名，20 个 api 文件同构：

```rust
pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()>
```

最大文件 ~470 行（input），平均 ~290。`ui` 三段共享同一个 `Table`：第一段末尾
`globals().set("ui", ui)`，后两段继续对同一 `Table` `set` —— 与现状形态一致，Lua 侧无感知。

## 5. 先造守卫，再动手（阶段 0）

拆 12k 行不难，难的是**证明拆完没变**。这个文件有过真实事故：
`input.action` 被注册两次（旧行号 1810 为死代码、1886 覆盖），语义变更无版本迁移，
M22 批 1 才修掉（docs/37 §记录）。说明「重复注册 / 漏注册 / 顺序覆盖」是这里的固有故障模式，
人眼 diff 抓不住。

**交付物**：API 快照门。

- 复用现成的 `API_PROBE`（`host/sample_project.rs:474`）与 `LuaRuntime::eval_json`，
  导出「表名 + 每表函数名（排序）」的完整清单，落 `artifacts/baselines/lua-api.json`
  （该目录已随仓库提交，正是为基线快照准备的）。
- 新增测试 `lua_api_surface_matches_baseline`：逐字节对比基线，不匹配时打印 diff，
  并在断言消息里写明「拆分不得改变 API 面；有意新增才更新基线」。
- 顺带锁数量不变量：`create_function` 272、`Rc::clone(&state)` 258、模块表 41。

有了这条门，后续每阶段都是「搬 → 跑门 → 绿了才提交」，把大重构降级成机械劳动。
这条门本身也是长期资产（今后新增 Lua 函数必须显式更新基线，等价于 API review）。

## 6. 阶段划分与验收

| 阶段 | 内容 | 风险 | 验收 |
|---|---|---|---|
| **0** | 建 API 快照门 + 基线 JSON（§5） | 无（只加测试与基线文件） | 新门绿；基线可复现 |
| **1** | `script.rs` → `script/mod.rs`；抽 `error.rs` `values.rs` `require.rs`；`ScriptState` 留 `mod.rs` | 极低，纯文本搬运 | 快照门 + `cargo test -p engine --all-features` |
| **2** | `register_api` 逐组搬进 `api/*`，**一次一个文件**。顺序：globals → save → audio → world_data → animation → renderable → entity → tilemap → camera → transform → physics → item/inventory/shop → **ui 三段** → input（由小到大，ui/input 最后） | 低 | 每组：快照门 + `cargo check -p engine`；整阶段：§7 全量 |
| **3** | `impl LuaRuntime` 拆 `runtime/scene_spawn/step/transition` | **中**：这批方法是 `host/runtime_scene.rs` 与编辑器 Play 预览的真实调用路径 | §7 全量 + `verify.ps1`（含 26 项 headless 样例、29 项 GUI smoke） |
| **4** | 测试 4482 行按域搬进 `script/tests/`（镜像 api 分组；`use super::*` 改为显式引入被测私有 helper，如 `parse_key_strict`、`settings_root`） | 低 | `cargo test -p engine --all-features` 测试项数不减 |

阶段 2 与阶段 4 文件不冲突，可并行。每阶段一个 PR，diff 应呈现为「整块移动」，
评审用 `git diff -w --find-copies-harder`；阶段 0-2/4 任一时刻可停，不依赖后续阶段。

## 7. 每阶段统一验收命令

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features        # 基线 1328 项通过、0 失败，不得减少
cargo run -p game   -- --project examples/game_jhyt --check
cargo run -p editor -- --project examples/game_jhyt --check
scripts/verify.ps1                           # 阶段 3 必须；阶段 0/1/2/4 可 -MvpOnly
```

## 8. 范围控制（本工作包刻意不做）

- **不引入 proc-macro / `#[lua_api]` 代码生成**。收益是减样板，代价是把「看得见的重复」
  换成「看不见的生成」，且 `api_error`/`require_alive`/`finite_*` 这类逐函数的手写校验策略
  无法统一表达。要做另开工作包。
- **不改 `ScriptState` 字段结构**，不把 gameplay 子系统（`inventories`/`shops`/`game_clock`）
  抽成独立状态对象。那会改到真实数据流，必须与纯拆分分开。
- **不合并已知重复**（见 §9.5），混进拆分 PR 会让 diff 失去「纯移动」的可验证性。
- 搬移中发现的可疑实现，逐条记进本文件 §11，不在拆分 PR 里修。

## 9. 坑清单

1. **可见性**（§3）：唯一会让拆分卡在编译上的点。字段留 `mod.rs` = 零属性改动。
2. **`api/mod.rs` 的调度顺序必须与现状逐字一致**。各表彼此独立，但 `globals().set` 的先后
   就是现状语义，不要顺手「整理」。
3. **`probe_startup_stage` 原样搬**（`#[doc(hidden)]`，注释写「验证后移除」，`game_web` 在用）。
   它是 iOS `WXWebAssembly` 启动崩溃排查链的探针，**不要顺手删**；清理单独开任务。
4. **`ui` 表的中途注册**（§2.1）：三段拆分后仍只需一次 `globals().set("ui", ...)`，
   放在第一段末尾。
5. **重复代码的合并诱惑**：docs/37 点名 `map.goto`/`map.go`（旧行号 3691-3751）与
   `data.load`/`data.reload`（3928-3990）近乎逐行重复。拆分会让这两段物理相邻（同进
   `world_data.rs`），**必须另开任务**处理。
6. **文档路径失效**：docs 里有 30 处 `script.rs` 引用，其中 `docs/37` 是**活动台账**且带行号
   （如 `script.rs:5133`）。`scripts/check-docs.ps1` 的门只查历史检出路径
   （`C:\GitHub\p2` / `E:\Codex\p2`），**不校验源码行号**，所以不会变红，只是人读会迷路。
   收尾处理：在 `docs/37` 顶部加一行说明「`script.rs` 已拆为 `core/script/`，本台账保留旧行号
   作为历史坐标」；此后新引用一律写符号路径（`script::api::save`）而非行号。
   历史归档文档（`tasks/done/`、已归档 docs）**不改**（章程口径）。
7. **`tasks/README.md` 会再次失真**：它当前写「`plan/` 只剩一份」「无待认领任务」，
   实际已有 5 份文件。本任务落地时同步校正该目录说明。
8. **`Transform3dSetArgs`**（45 行）跟 `transform3d` 走，别留在 `mod.rs`。

## 10. 验收证据清单

- [x] 阶段 0：`lua_api_surface_matches_baseline` 通过；基线入库路径改为
  `src/core/api_surface_snapshot/lua_api_surface.json`（`artifacts/baselines/` 被 gitignore，见 §13.2-3）
- [x] 阶段 1-4：§7 命令读数见 §13.3，`cargo test --workspace` **1389 项通过、0 失败**（基线 1328 只增不减）
- [x] API 面：41 模块表 / 275 个 Lua 可见函数（`create_function` 272 处），与基线逐字节一致
- [x] 跨 crate 零改动：`editor`/`game`/`game_web`/`assetc` 四个 crate 一行未改。
  engine crate 内的改动除 `core/script/**` 外只有 4 处，都是门自身需要的：
  `core/mod.rs`（架构门跳过 `tests` 目录）、`core/api_surface_snapshot/`（阶段 0 新增）、
  `examples/dump_lua_api.rs`（基线生成器，新增）、`host/sample_project.rs`
  （删掉重复的 `API_PROBE` 常量，改用 `script::LUA_API_PROBE`）
- [~] 拆分后最大单文件 ≤ 500 行：**新建文件全部 ≤ 500**，4 个既有/聚合文件超限
  （`mod.rs` 1177、`scene_spawn.rs` 530、`tests/input.rs` 817、`tests/save_data.rs` 784），
  原因与继续瘦身的代价见 §13.2-8
- [ ] `scripts/verify.ps1 -MvpOnly` 全量通过：当前被 §13.5 的既有回归（`settings.toml` 丢了
  `pause` 键盘绑定）挡在第 1 个样例，**不是拆分引入**；26 个无头样例改用工程副本跑，读数见 §13.6
- [x] `docs/37` 顶部行号口径说明已加；`tasks/README.md` 目录状态已校正
- [x] 注释符合[章程 §9.7](../../docs/00-project-charter.md)（精简中文，字段注释逐字保留）
- [x] CHANGELOG 记录（纯重构条目，无行为变化）

## 11. 未完成项 / 已知限制

- 本文件行数为**规划期实测快照**，认领时必须按 §2 复核（该文件仍在被其他任务改动）。
- `runtime.rs`/`scene_spawn.rs`/`step.rs`/`transition.rs` 的边界是 §2 的粗划，阶段 3 开工前
  需按真实方法依赖图细化（`spawn_entity_components` 317 行内部是否再分层，届时决定）。
- `ui` 三段的具体函数归属未在规划期逐一定名（需读完 970 行按功能聚类），阶段 2 时补。
- 拆分后 `ScriptState` 的「40 字段靠 `derive(Default)` + `new()` 里手工补默认」这一隐式契约
  （`action_map` 就因此必须在 `LuaRuntime::new` 里 `merge_missing`）**未解决**，只是搬了位置。
  加固方案（显式 `impl Default for ScriptState` 或分子系统状态）另开任务。
- 未做：热重载/每实体脚本的「每次 `Lua::new()` 重跑 272 次注册」成本优化。
  wasm 与低端设备上是可见开销，属独立性能工作包。

## 12. 工作量

阶段 0 约 0.5 人日｜阶段 1 约 0.5｜阶段 2 约 2-3（主体，20 文件 + 每步跑门）｜
阶段 3 约 1-2（需全量 verify）｜阶段 4 约 1。合计 **5-7 人日**，无不可逆节点。

## 13. 实现记录（2026-10-01，阶段 0–4 代码完成，待验收）

### 13.1 落地结构与实测行数

`engine/crates/engine/src/core/script/`（全部 CRLF，无 BOM）。原 `script.rs` 11963 行 → 目录形态：

| 层 | 文件（行数） |
|---|---|
| 状态与外表面 | `mod.rs` 1177（`ScriptState` 40 字段 + `impl ScriptState` + `LuaRuntime` 结构与其剩余访问器 + 跨模块私有 helper + `pub use` 7 项 + `LUA_API_PROBE`） |
| 基础设施 | `error.rs` 140｜`values.rs` 219｜`require.rs` 87 |
| 运行时职责 | `runtime.rs` 324｜`scene_spawn.rs` 530｜`step.rs` 131｜`transition.rs` 97 |
| Lua 绑定 | `api/mod.rs` 53（顺序调度）+ `globals` 307｜`input` 462｜`entity` 305｜`transform` 405｜`renderable` 292｜`camera` 398｜`tilemap` 271｜`animation` 260｜`physics` 449｜`audio` 170｜`world_data` 376｜`ui` 232｜`ui_panel` 497｜`ui_nav` 230｜`inventory` 422｜`save` 163 |
| 测试 | `tests/mod.rs` 100（脚手架 + 共享 `use`）+ 17 个域文件，共 4534 行 / **94 个 `#[test]`** |

api 各文件与 Lua 模块表的对应关系（41 表全覆盖）：

```
globals.rs   log time clock calendar day app event
input.rs     input                      entity.rs      entity tag health visibility interact
transform.rs transform transform3d      renderable.rs  sprite quad mesh light world_text
camera.rs    camera camera2d camera3d   tilemap.rs     tilemap grid
animation.rs animation animation3d      physics.rs     physics physics3d particles
audio.rs     audio                      world_data.rs  scene map asset data
ui + ui_panel + ui_nav → 同一张 ui 表（45 函数）
inventory.rs item inv container shop    save.rs        save
```

### 13.2 与规划的偏差（逐条，含理由）

1. **api 文件 17 个，不是 §4 预算的 20 个**：`item`/`shop` 没有独立文件，与 `inv`/`container`
   同进 `inventory.rs`（422 行）；`scene`/`map`/`asset`/`data` 同进 `world_data.rs`。
   理由是划分轴取「同一份运行时数据的表」而不是「一张表一个文件」——`inventories`/`shops`
   状态被这四张表共同读写，拆开只会让跨文件跳读变多，且拆出的文件仍会逼近 500 行上限。
2. **自由 helper 留在 `mod.rs`，没有按 §4 下沉到 api 各文件**（`camera_view`/`camera3d_view`/
   `call_optional`/`solid_cell_colliders`/`settings_root`/`parse_key_strict`/`inventory_*`/
   `save_path_for_slot`/`resolve_input_settings_path` 等 21 个，约 419 行）。
   实测这些 helper **同时被 `mod.rs` 的 `impl LuaRuntime` 与 api 子模块使用**
   （例如 `camera_view` 被 `mod.rs`、`api/camera.rs`、`api/globals.rs` 三处调用）。
   下沉后 `mod.rs` 作为祖先反而访问不到（api 子模块里的 `pub(super)` 只对 `api` 及其后代可见，
   `script` 不是 `api` 的后代），必须改 `pub(in crate::core::script)`——那是 §3 明令避免的
   「扩大可见性」。留在 `mod.rs` 私有 = **属性零改动**，与 §3 对 `ScriptState` 的裁定同口径。
3. **API 基线没落在 §5 说的 `artifacts/baselines/lua-api.json`**：`.gitignore` 第 13 行
   `/artifacts/baselines/*` 把整个目录忽略，只放行 `README.md` 与 `wp0-3d-stress.committed.json`
   两条显式白名单。基线放那儿会**静默不入库**，门在别人机器上直接不可复现。
   实际落点：`core/api_surface_snapshot/lua_api_surface.json`（`include_str!` 读，与测试同生命周期），
   重新生成用 `cargo run -p engine --example dump_lua_api -- --write`。
4. **计数口径澄清**：`create_function` 272 处（api/* 271 + `require.rs` 1），与 §2 的规划值一致；
   Lua 可见面是 **41 表 / 275 函数**（probe 口径），差的 3 个不是漏注册，而是探针枚举到的
   非 `create_function` 注册项。两个数各自都被门锁住，别再互相换算。
5. **`spawn_entity_components` 改为 `pub(super)`**：它是 `scene_spawn.rs` 的私有域，
   `mod.rs` 与 `runtime.rs` 都要调；`pub(super)` 后私有域是 `script`，后代可用（§3 末段口径）。
6. **架构门 `core_does_not_depend_on_host` 现在跳过名为 `tests` 的目录**（`core/mod.rs`）：
   该门只约束生产代码，而阶段 4 之后测试是目录形态，`tests/input.rs` 里
   `use crate::core::script::tests` 的脚手架会踩线。跳过条件写在门的 `collect()` 里并带注释。
7. **§11 的待定项已定名**：`ui` 970 行按功能聚成三段——`ui.rs` 基础图元与取值
   （`text`/`bar`/`show`/`get`/`measure_text`/`scene_text_visible`…）、
   `ui_panel.rs` 面板与栅格（`panel`/`grid`/`image`/`circle`/`tooltip`/`modal`/`drag_*`）、
   `ui_nav.rs` 焦点环与滚动（`focus_*`/`scroll_*`）。`globals().set("ui", …)` 只在
   `api/mod.rs` 三段填完后执行一次。
8. **「最大单文件 ≤ 500 行」（§10）未全线达成**，4 个文件超限：`mod.rs` 1177、
   `scene_spawn.rs` 530、`tests/input.rs` 816、`tests/save_data.rs` 784。
   前两条的超额部分正是偏差 2 的 helper（419 行）与 `impl LuaRuntime` 剩余访问器（371 行）；
   后两条是测试域天然大。**要继续瘦身 `mod.rs`**：可行做法是新建 `script/helpers.rs` 并给
   这 21 个 helper 加 `pub(in crate::core::script)`，另开一个**纯机械 commit**（不与逻辑混），
   按 §3 的裁定，这一步不塞进本次拆分。

### 13.3 验收门读数（全部以 `-j 2` 限流跑，避免占满 CPU）

| 门 | 读数 |
|---|---|
| `cargo test --workspace --all-features --exclude wasm-lua-spike` | **1389 passed / 0 failed**（规划基线 1328，只增不减） |
| `cargo test -p engine --lib` | **892 passed / 0 failed**，拆分前后同数 |
| `lua_api_surface_matches_baseline` | 绿：41 表 / 275 函数与基线一致 |
| `cargo fmt --all -- --check` | 绿（拆分后 `tests/*.rs` 需重排一次，已由 `cargo fmt --all` 落地） |
| `cargo clippy --workspace --all-targets --all-features` | 只剩 5 条既有 `core/vfs.rs` 警告（`-D warnings` 因此判红，见 §13.4） |
| `cargo run -p game -- --project examples/game_jhyt --check` | `game_check_ok` |
| `cargo run -p editor -- --project examples/game_jhyt --check` | `editor_project_ok baseline=v0.21 schema_errors=0` |
| `scripts/check-docs.ps1`（随 verify 跑） | `check_docs_ok docs=53 h1=79 links=268 ps1_bom_ok=24` |
| `scripts/verify.ps1 -MvpOnly` | **被 §13.5 的既有回归挡住**，非本次拆分引入 |

**纯移动证明**：把 HEAD 的 `script.rs` 与拆分后的 `core/script/**.rs` 做行多重集差，
再对「只存在于 HEAD」的行做空白归一子串查找（rustfmt 在不同缩进深度会合并/拆行，
所以子串查找才是判据），命中 25 种行，逐条核过：17 行是 `use` 的续行（按文件重新切分），
3 行是签名加了可见性前缀（`register_api`→`api::register_all`、`file_fingerprint`→`pub(super)`），
5 行是 rustfmt 的换行重排（字面量与断言原样存在，已 grep 回查）。
**没有任何一条是丢失的逻辑行。**

### 13.4 过程中撞到的既有问题（非本次引入，不做顺手修复）

- `engine/crates/wasm-lua-spike/src/lib.rs:19` 调 `JsValue::byte_length` 编译不过
  （`cargo test/clippy --workspace` 因此中断）。这是未提交的 `[DIAG]` 探针代码，属 iOS
  `WXWebAssembly` 启动排查链，需要单独裁定去留；本次全量门用 `--exclude wasm-lua-spike` 绕过。
- `core/vfs.rs` 5 条 clippy `type_complexity`(4)/`collapsible_if`(1)，行号 38/61/65/72/86：
  与 README「零 clippy 警告」的口径冲突。本次未触碰 `vfs.rs`（`git status` 里它是干净的）。
- Windows 链接器 `LNK1207`（`game` 测试二进制的 PDB 陈旧/格式不兼容）：
  处理方式是 `cargo clean -p game`（清掉该 crate 自身产物，依赖缓存不动，随后需重新链接）。
  **这是构建缓存动作，不改源码**；下次跑到同一条链接错误按同样口径处理。
- 工作树里 `core/script/**.rs` 的行尾不统一：`mod.rs`/`error.rs` 等是 CRLF，
  `api/*` 与 `tests/*` 是 LF。`core.autocrlf=true` 且无 `.gitattributes`，
  **提交时 git 一律归一成 LF**，所以这不是待修项；别再为此做一次全树换行改写。

### 13.5 `verify.ps1 -MvpOnly` 的第一条红是既有回归，不是拆分

现象：MVP 无头回归第 1 个样例 `jhyt_smoke` 在 M16-01 断言失败——
`require('core.state').mode = "Playing", want "Pause"`。

定位（结论：与 WP11 无关）：

- 触发链是 `input.action_pressed("pause")` → `pause_menu.open()`，
  引擎默认动作表里 `("pause", &[key('p')])`（`core/input.rs:798`）。
- `examples/game_jhyt/settings.toml` 的 `[input] map` 会**整体替换**默认表。
  提交 `a4288fc`（2026-09-30，wasm Lua pcall 崩溃修复）把该文件覆盖成
  「M20-08 手柄验收设置（示例结束时按原样恢复）」的手柄专用表，
  `pause=pad:5` **丢了键盘绑定 `key:keyp`**（同一张表还删了 `inventory=key:keyi` 等）。
  即 PC 键盘玩家现在按 P 无法暂停——这是**产品级回归**，不只是测试红。
- 反证：把 `a4288fc` 之前的 `settings.toml` 复制到工程副本，用**当前拆分后的引擎**跑
  `jhyt_smoke` → `jhyt_smoke_ok day=8 inner_power=55 lingzhi=1 energy_after=98`，
  M16-01..M16-04 全绿。同一条命令在仓库原工程上失败。
  时间线也对得上：最后一次绿的 verify 是 `2026-09-27 21:11`，早于 `a4288fc`。

待用户裁定（不在 WP11 范围内，故未动）：`examples/game_jhyt/settings.toml` 要么恢复
键盘绑定（`git show a4288fc^:examples/game_jhyt/settings.toml`），要么把那句
「示例结束时按原样恢复」真的执行掉。修好后 `-MvpOnly` 才能作为 WP11 阶段 3 的收口证据。


