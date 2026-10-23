# 可视化脚本（`.vgraph`）格式、代码生成与画布编辑

> 状态（2026-10-23）：**E03-1、E03-2、E03-3、E03-4a、E03-4b 已交付**——格式、严格校验、确定性 Lua 生成、golden 与负向门；
> 编辑器画布（打开/平移缩放/增删节点/连线/框选/撤销/编译诊断/原子保存并生成）；
> 运行时把 `.vgraph` 直接挂到实体 `Script` 上，4 类事件走现有 VM，改图热重载且坏图保留旧状态；
> E03-4a 起内置与插件节点共用同一张注册表（`register_plugin_node` + 模板展开，示例见
> `tests/plugin_nodes.rs`）；E03-4b 在 Play 时把最近进入的节点回传画布高亮（探针默认关闭）。
> E04 插件生命周期（`visual_script_node` 能力）尚未接线。

## 1. 范围

- 资产是 JSON 文本，扩展名 `.vgraph`；图是唯一源，生成的 Lua 不手改。
- 节点类型注册在 `rustlua-visual-script` 的 v1 注册表里（18 个内置节点）；插件节点经
  `register_plugin_node` 注册到同一张表，**加载插件生命周期**属于 E04。
- 生成物只调用**现有** Lua API：`transform3d.set`、`animation3d.set`、`audio.play`、
  `ui.text`、`event.emit`、`save.commit`。运行时不新增 VM。

暂缓的节点（当前 Lua API 面没有稳定函数，生成出来只会在运行时炸）：

- `spawn_prefab`：`prefab.*` 只有 despawn/node/root/set_rect；
- `get_component`：实体没有组件查询接口；
- `vec2`：v1 唯一的位置动作是 `transform3d.set`（收三个 Number），没有 Vec2 消费端。

`Vec2/Vec3/Any` 类型仍保留在类型系统里，供图变量与将来的插件节点使用。

## 2. 资产格式

```json
{
  "version": 1,
  "name": "p3-e03-interact",
  "variables": [{ "name": "score", "value_type": "number", "default": 0 }],
  "nodes": [
    { "id": "start", "kind": "on_start", "position": [0, 0] },
    { "id": "put", "kind": "set_transform", "position": [360, 0] },
    { "id": "x", "kind": "number", "position": [0, 140], "params": { "value": 1.5 } }
  ],
  "edges": [
    { "from": { "node": "start", "port": "then" }, "to": { "node": "put", "port": "in" } },
    { "from": { "node": "start", "port": "self" }, "to": { "node": "put", "port": "entity" } },
    { "from": { "node": "x", "port": "value" }, "to": { "node": "put", "port": "x" } }
  ]
}
```

- 节点 `id`：`[A-Za-z0-9_-]{1,64}`，全图唯一；`-` 在生成变量名时归一化为 `_`，因此
  `a-b` 与 `a_b` 同时出现会以「归一化撞名」拒绝。
- `position` 只给编辑器用，解析/生成不依赖它，往返不丢。
- `variables`：v1 只落盘与校验（名字、唯一性、默认值类型），还没有 get/set 变量节点。
- `version` 目前只接受 `1`；其他版本直接拒绝，不做隐式迁移。

## 3. v1 内置节点（18）

| 类别 | 节点 | 输入 | 输出 / 参数 |
|---|---|---|---|
| 事件 | `on_start` | — | `then` flow、`self` entity |
| 事件 | `on_update` | — | `then`、`self`、`dt` number |
| 事件 | `on_interact` | — | `then`、`self`、`target` entity |
| 事件 | `on_collision` | — | `then`、`self`、`other` entity |
| 流程 | `branch` | `in`、`condition` bool | `true` / `false` flow |
| 流程 | `sequence` | `in` | `then0` / `then1` flow（先 0 后 1） |
| 流程 | `gate` | `in`、`open` bool | `then` flow |
| 数据 | `number` | — | `value` number；参数 `value` |
| 数据 | `bool` | — | `value` bool；参数 `value` |
| 数据 | `string` | — | `value` string；参数 `value` |
| 数据 | `math` | `a`、`b` number | `result` number；参数 `op` = add/sub/mul/div |
| 数据 | `random` | `min`、`max` number | `value` number（`min + random()*(max-min)`） |
| 动作 | `set_transform` | `in`、`entity`、`x/y/z` number | 调 `transform3d.set(entity, x, y, z)` |
| 动作 | `play_animation` | `in`、`entity` | 参数 `clip`；调 `animation3d.set` |
| 动作 | `play_audio` | `in` | 参数 `name`；调 `audio.play` |
| 动作 | `emit_event` | `in` | 参数 `name`；调 `event.emit` |
| 动作 | `ui_set_text` | `in`、`text` string | 参数 `element`；调 `ui.text` |
| 动作 | `save_game` | `in` | 调 `save.commit()` |

每个事件最多一个入口（重复 `on_start` 等直接报错），整张图至少一个事件。

## 4. 校验口径

所有错误一次性收齐（`Vec<GraphError>`），编辑器可把整张图一起标红：

- 重复 id、非法 id、归一化撞名、坐标非有限数；
- 悬空边（起点/终点节点不存在、端口不存在）、重复边；
- 端口类型不匹配（Flow 与数据互不相通；`Any` 兼容任意类型）；
- 数据输入只能接一条；流程输入允许汇合（这是 gate/sequence 回边的合法入口）；
- 流程输出不允许扇出，需要并行/顺序时显式放 `sequence`；
- 必填输入未连接（动作的 `in` 也算必填）；
- 数据环拒绝；流程环必须经过 `gate` 或 `sequence`；
- 参数缺失/未知/类型或取值非法、图变量名与默认值非法；
- 未知节点类型、缺事件入口、节点数超过 256。

## 5. 生成契约

每个事件生成一个函数，文件名与路径由调用方决定（编辑器保存落到
`scripts/generated/<name>.lua`；运行时直接由图编译，不读这份生成文件）：

```lua
-- generated from assets/graphs/p3-e03-interact.vgraph; do not edit
-- graph: p3-e03-interact (vgraph 1)

local M = {}

function M.on_start(owner)
    -- node seq-start: sequence
    ::flow_2::
    -- node move: set_transform
    ::flow_4::
    do
        local n_num_x = 1.5
        local n_math_y = (n_num_a + n_num_b)
        transform3d.set(owner, n_num_x, n_math_y, n_num_z)
    end
    goto cont_3
    ::cont_3::
    -- node save-start: save_game
    ::flow_5::
    do
        save.commit()
    end
    goto fend_1
    ::fend_1::
    -- on_start.
end

return M
```

- 函数签名：`on_start(owner)`、`on_update(owner, dt)`、`on_interact(owner, target)`、
  `on_collision(owner, other)`；`owner` 是脚本所属实体，由 E03-3 的运行时传入。
- 节点 id → 局部变量名：`n_` + id（`-`→`_`），例如 `num-x` → `n_num_x`。
- 流程用 `goto` + 标签线性化；数据节点在首次使用处按依赖顺序生成 `local`。
- 所有局部变量都在 `do ... end` 内，避免 `goto` 跳进局部变量作用域这一 Lua 语法错误。
- 同一张图重复生成逐字节一致；生成头固定，golden 逐字节比对。

## 6. 编辑器画布（E03-2）

入口：资源面板双击 `.vgraph`（视图菜单的面板开关里也有「可视化脚本」）。标签默认隐藏，
布局键 `PANEL_GRAPH = "graph"` 默认 `false`；打开资源后 `show_graph_panel = true`，
`sync_visibility` 下一帧把标签补到 Scene 右侧。

交互：

- 左键拖节点 → 移动（一段拖动只压一步撤销）；空白处拖 → 框选；中键拖 / 滚轮 → 平移/以指针为中心缩放；
- 「适配」把全部节点缩放到视口；
- 从输出端口拖到输入端口，或点输出再点输入 → 连线；点已连接的输入端口 → 断开该输入的全部入边；
- `Delete` 删除选中节点；`Ctrl+Z` / `Ctrl+Y`（或 `Ctrl+Shift+Z`）撤销/重做；
- 右侧检查器编辑参数（`math.op` 是下拉框；同一字段的连续输入合并为一步撤销），
  列出该节点的入边并可逐条断开；编译错误里提到该节点的条目会直接列在下方；
- 底部诊断面板显示全部 `GraphError`（解析错误带 line/column），画布上相关节点描红边。

保存语义（`graph_ops::GraphDocument::save`）：

1. 先 `compile`（内部先 `validate`）；失败只更新错误列表，**磁盘上的旧 `.vgraph` 与旧 Lua 都不动**；
2. 成功后把 `.vgraph` 与 `scripts/generated/<sanitized-name>.lua` 各写一份临时文件，再原子替换；
3. 生成头里的来源路径是工程的 `/` 分隔相对路径；图名只保留 `[A-Za-z0-9_-]`，其余字符换成 `_`；
4. 只接受工程内相对路径；绝对路径、盘符与 `..` 一律拒绝。

边界：画布只改 `GraphDocument`，不写场景文档、不发 `EditorCommand`；架构棘轮
`panels_only_emit_actions` 把 `graph.rs` 的直接写入钉在 0。`dirty` 由 `graph != saved` 派生，
撤销回到保存点会自动变干净。

明确不做：复制粘贴、多选整体拖动、连线折线命中删除、节点重命名、图级变量编辑 UI。

## 7. 运行时接入（E03-3）

实体 `Script` 的 `file` 可以直接指向 `.vgraph`。运行时按扩展名走同一条加载路径：

1. 经 VFS 读源文件，`Graph::from_json` + `compile` 生成 Lua；
2. 把生成物包成模块表，再暴露为全局 `on_spawn/on_update/on_interact/on_collision`；
3. 注册到该实体已有的 Lua 状态与预算槽，**不新增 VM**。

编辑器保存产生的 `scripts/generated/<name>.lua` 仍是可打包/可审阅的生成物，但运行时**不依赖**它：
图是唯一源，加载时现编译，避免「改了图忘了重新生成」的陈旧 Lua。`.lua` 与 `.vgraph` 共用
同一套指纹、热重载、错误与预算路径。

| 图事件 | 实体脚本钩子 | 参数 |
|---|---|---|
| `on_start` | `on_spawn(owner)` | 所属实体 |
| `on_update` | `on_update(owner, dt)` | 所属实体、固定步长 |
| `on_interact` | `on_interact(owner, target)` | 所属实体、交互目标 |
| `on_collision` | `on_collision(owner, other)` | 所属实体、碰撞对端 |

- **on_interact**：现有 UI 点击分发（`process_ui_clicks`）把点击目标名经 `EntityState.names`
  解析成实体，再投给该实体的脚本。v1 没有跨实体点击目标，`target` 与 `owner` 相同。
- **on_collision**：2D/3D 物理把每个生成的 `PhysicsContact` 同时写进原 `contacts` 队列和独立的
  `event_contacts` 队列；固定步结束后宿主调用 `dispatch_collision_contacts`，只投 `started` 边沿，
  双方都挂脚本时各收一次。`physics.events()` 仍然消费原队列，两条路径互不偷事件。
- **预算**：四个钩子各自使用 `BudgetSlot::Entity(EntityId, EntityHook)` 槽位，互不挤占。
- **热重载**：`reload_entity_scripts()` 重读源文件、重新解析/编译，成功才替换该实体的 Lua 状态；
  解析或编译失败返回错误并**保留旧 VM**，不会停在半替换状态。重载不重复 `on_spawn`，下一帧
  `on_update` 就使用新图——「改图后行为变」由此成立。
- **错误**：钩子错误带 `on_interact: ...` / `on_collision: ...` 前缀，沿 UI 点击与物理收尾路径上抛。

集成测试 `vgraph_entity_runs_four_events_and_hot_reloads` 用真实场景验证：带 `Transform`/
`Script`/`Collider` 的 Actor 与静态 Wall 重叠，4 个事件各写一个 UI 文本；`on_interact` 由真实
`UiClickTarget` 触发，`on_collision` 由真实物理接触触发；图从 v1 改成 v2 后重载，只有
`on_update` 的行为变化，`on_start` 不重跑；写入坏 JSON 重载报错且 v2 行为仍在。
`vgraph_on_start_sets_the_scene_transform` 另用带 `Light` 3D 标记的实体验证 `on_start` →
`set_transform` 真写进运行时 `Transform3D`（位置 `1.5/2.5/3.5`，旋转单位四元数、缩放不变），
可观测量不是 UI 文本。

### 7.1 插件节点注册（E03-4a）

- 内置 18 个节点与插件节点共用 `registry::node_spec()` / `all_nodes()`：校验、Lua 生成、
  编辑器调色板与画布布局都走同一张表，没有第二套"内置分支"。
- `register_plugin_node(&'static NodeSpec, template, returns_value)` 是唯一入口（进程级、一次性）：
  - 拒绝事件/流程分类，拒绝 kind 与内置或已注册节点撞名；
  - `returns_value = true` 必须是 Data（模板是表达式），false 必须是 Action（模板是语句）；
  - 参数只支持 Number/Bool/String；模板占位符 `{port}` / `{param:name}` 必须在声明的输入/
    参数里，未闭合花括号或孤立 `}` 在注册期报错。
- `tests/plugin_nodes.rs` 注册两个示例节点并断言：
  - 动作模板生成 `plugin_demo.show(n_text, "hud")`；
  - 数据模板生成 `local n_double = (n_three * 2)`，并被下游 `transform3d.set` 消费；
  - 未注册 kind 与缺输入的错误都带节点 id（编辑器据此描红到节点）。
- 未接线：编辑器进程还没有插件加载入口（E04 的 `visual_script_node` 能力未开工），示例节点
  只在集成测试进程注册。

### 7.2 运行时当前节点高亮（E03-4b）

- `compile_traced` 与 `compile` 同语义，只在每个节点入口多一行
  `if __vgraph_trace then __vgraph_trace("<id>") end`；普通 `compile` 与 golden 逐字节不变。
- 只有实体脚本是 `.vgraph` 时，`LuaRuntime::load_entity_script` 才在该 Lua 状态装
  `__vgraph_trace` 回调；回调把「工程相对脚本路径 + 节点 id + 单调序号」写进共享探针。
  **探针默认关闭**，`set_vgraph_trace_enabled(true)` 后才记录；关闭时调用点直接返回，
  不分配事件，也不影响脚本语义。
- 编辑器只在 Play 会话存在且画布被绘制时打开探针；事件的 `script` 必须与当前画布
  `doc.source()` 相同才高亮，避免同进程多张图串台。节点描青边并标「▶ 最近执行」。
- 这是**最近一次进入**的节点，不是断点式「停在此节点」：没有暂停/单步/变量查看。
  数据节点作为上游表达式内联求值，没有独立进入点；同帧多次进入同一节点只保留最后一次；
  事件只带节点 id 与序号，不带帧号/实体 id。

## 8. 验收命令与读数（2026-10-23）

```text
cargo test --manifest-path engine/Cargo.toml -p rustlua-visual-script
  -> 23 passed（7 单元 + 13 集成 + 3 插件节点集成；含格式往返/悬空边/类型错/数据环/
     无守卫流程环/守卫环/gate 环 Lua 加载/golden 逐字节/以 Lua stub 断言
     四个事件实际调用 transform3d.set、animation3d.set、audio.play、ui.text、
     event.emit、save.commit 及调用顺序；插件动作/数据节点生成与注册期负向门；
     `traced_compile` 的埋点行与普通生成物逐行等价）
cargo test --manifest-path engine/Cargo.toml -p rustlua-visual-script traced_compile
  -> 1 passed / 0 failed（6 filtered）
cargo run --manifest-path engine/Cargo.toml -p rustlua-visual-script \
  --example wp0_vgraph_golden
  -> {"gate":"wp0_vgraph_golden","graph":"p3-e03-interact","nodes":25,"edges":23,
      "functions":4,"bytes":2048,"lua_load":"ok","golden":"match"}
cargo clippy --manifest-path engine/Cargo.toml -p rustlua-visual-script \
  --all-targets --all-features -- -D warnings
  -> exit 0
cargo fmt --manifest-path engine/Cargo.toml --all
  -> exit 0

cargo test --manifest-path engine/Cargo.toml -p editor graph_editor
  -> 7 passed（图往返 + 生成；编辑/撤销/重做回到保存点；坏图保存保留旧文件；
     路径逃逸拒绝；类型检查与参数撤销合并；未注册节点诊断；无头画一帧不落盘）
cargo test --manifest-path engine/Cargo.toml -p editor
  -> lib 209 passed；bin 467 passed / 2 ignored；exit 0
cargo clippy --manifest-path engine/Cargo.toml -p editor \
  --all-targets --all-features -- -D warnings
  -> exit 0（仅预存在 paste linker warning）

cargo test --manifest-path engine/Cargo.toml -p rustlua-scripting vgraph_entity
  -> 1 passed / 0 failed（123 filtered；含 E03-4b：默认无事件 → 开探针后
     `action-update`、序号 2、路径 `scripts/behavior.vgraph`、clear 后可重填）
cargo test --manifest-path engine/Cargo.toml -p rustlua-scripting \
  vgraph_on_start_sets_the_scene_transform
  -> 1 passed / 0 failed
cargo test --manifest-path engine/Cargo.toml -p rustlua-scripting
  -> 124 passed / 0 failed；集成测试 1 passed
cargo test --manifest-path engine/Cargo.toml -p rustlua-physics
  -> 35 passed / 0 failed
cargo test --manifest-path engine/Cargo.toml -p engine --lib
  -> 313 passed / 0 failed（含 `runtime_scene::tests::schedule`）
cargo test --manifest-path engine/Cargo.toml \
  -p rustlua-infra --test architecture_edges
  -> 5 passed / 0 failed（BUDGET 新增 `visual-script` 叶子与
     `scripting → visual-script` 边）
cargo clippy --manifest-path engine/Cargo.toml \
  -p rustlua-scripting -p rustlua-physics -p engine \
  --all-targets --all-features -- -D warnings
  -> exit 0
cargo fmt --manifest-path engine/Cargo.toml --all -- --check
  -> exit 0
```

fixture 在 `engine/crates/rustlua-visual-script/tests/fixtures/p3-e03-interact.vgraph`，
golden 在
`engine/crates/rustlua-visual-script/tests/fixtures/p3-e03-interact.generated.lua`。`--write` 仅用于节点集/生成器明确
变更后重刷 baseline；校验失败的图不会触碰 golden。

## 9. 未完成项

- E03-4a 已交付注册表与两个示例插件节点（动作/数据模板、注册期负向门、错误带节点 id）；
  E04 的 `visual_script_node` 插件生命周期仍未接线（编辑器进程不会自动加载插件节点），
  GUI 调色板未目视；
- E03-4b 只做到「最近进入节点」高亮：探针默认关闭，编辑器 Play + 画布可见时才打开；
  没有断点/单步/变量查看；数据节点与同帧多次进入只留最后一次；没有开窗画面证据，
  高亮路径由无头绘制测试覆盖；
- 运行时验收目前由**测试内构造的场景**覆盖（真实 UI 点击与真实物理接触），示例工程里还没有
  落盘 `.vgraph`/场景资产；`on_start` → `set_transform` 已断言写入运行时 `Transform3D`，
  但 `play_animation` 仍未断言真推进 `Animation3D`，`on_update` 的计数语义也未断言；
  没有开窗 GUI 目视；
- 画布尚未在真实窗口里做手势目视（本轮只有无头绘制测试）；复制粘贴/多选整体拖动/节点重命名未做；
- `spawn_prefab` / `get_component` / `vec2` 节点按 §1 暂缓；
- `verify.ps1`、workspace 全量、Web/wasm、真实 GPU 未跑。
