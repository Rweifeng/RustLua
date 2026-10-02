# Lua 脚本错误处置策略（严格 / 容错）

> 文档版本：1.0
> 建立日期：2026-10-02
> 关联决策：[ADR-0023](02-technical-decisions.md)、[Lua API](18-lua-api.md)、[Lua 热重载](19-lua-hot-reload.md)、
> [Lua 脚本调试](20-lua-script-debug.md)、[质量门与发布清单](34-质量门与发布清单.md)

## 1. 修的是什么

在这条策略落地之前，Lua 回调的错误是**一律致命**的，而且致命得比"退出码非零"更早：

```text
update() 里的 Lua 错误
  → lua_update 系统返回 SystemError            （host/runtime_scene.rs）
  → Scheduler::run 返回 SchedulerError::SystemFailed
  → 调度器**放弃本步剩余的所有系统**            （物理、粒子、时钟、场景切换全被跳过）
  → game.exe 的 run_simulation_steps 看到 Err  → fail() → 进程退出
```

两个后果都不是我们想要的：

1. **玩家的一局因为一行脚本结束。** 一个只在特定输入下才走到的分支写错，等于整局中断。
2. **"脚本偶发错误"与"引擎故障"在退出码上不可区分**，排查时只能靠翻日志。

同时，容错不能变成"把错误吞掉"——那会把脚本错误从验收门里删掉。因此本策略的核心是
**把"错误怎么处置"变成宿主的显式选择**，而不是换一套默认行为。

## 2. 两档策略

| 档位 | 谁用 | 行为 |
|---|---|---|
| `Strict`（**默认**） | 无头样例、soak、性能基线、Web 宿主、`--check` | 首个错误即上报：`run_script_steps` 返回 `Err`。控制流与既有语义一致（UI 点击路径的报错文本补了回调名） |
| `Contain` | `game.exe`、编辑器 Play | 错误记入结构化诊断与计数器，固定步继续跑；连续报错达上限的回调被**停用** |

```rust
// 交互宿主显式开启容错（game_app.rs / preview_session.rs）
scene.set_script_error_policy(ScriptErrorPolicy::Contain);
```

默认必须是 `Strict`：26 个无头样例、soak 与性能基线全都 `unwrap()` `run_script_steps` 的返回值，
它们靠这条路径把"脚本报错"变成判红。容错能力**不改变**这些门的强度。

## 3. 容错档的细则

### 3.1 预算是按回调独立的

| 槽位 | 键 | 说明 |
|---|---|---|
| 工程级 `update()` | `update()` | `scripts/main.lua` 的 `update` |
| UI 点击分发 | `ui.click` | `UiEvents` 绑定的任意 Lua 函数；一个按钮的函数写错不会连带后面排队的按钮 |
| 每实体钩子 | `entity:<index>:g<generation>:on_spawn` / `:on_update` | 按 `EntityId` 各一份；某个实体的 `on_update` 报错只跳过它自己 |

每实体预算以 `EntityId`（含 generation）为键，在 `refresh_entity_script_requests` 里随实体消亡剪除——
`EntityId` 的槽位会被复用，不剪就是"每生成一次实体泄漏一条预算"。

### 3.2 连续错误达上限即停用

- 上限 `DEFAULT_CALLBACK_ERROR_LIMIT = 60`，即**可见 1 秒 @60Hz**。
- 取"1 秒"而不是"3 帧"：只在特定输入下出现的错误（读表越界、nil 字段）应当有机会自愈——
  **成功一次即清零**。停用只针对"每帧都炸"的那种。
- 停用后该回调**不再进 Lua**：既止损，也让日志不被同一个错误淹没。
- 停用是**一次性判定**：解除只能靠热重载、换场景或重开一局（这些路径都会重置预算）。

### 3.3 容错 ≠ 静默

三个出口把"脚本出过错"变成可观测、可断言的信号：

| 出口 | 用途 |
|---|---|
| `tracing`（target `script`，前缀 `[lua-guard]`） | 每个**不同**的错误只打一条（按 `path+phase+message` 去重）；停用时单独再吼一条 |
| `RuntimeScene::script_error_total()` | 累计被容错的错误数；宿主与验收脚本据此断言"这一局没出过错" |
| `RuntimeScene::script_disabled_callbacks()` | 已停用的回调键（按字典序，稳定输出便于断言） |

结构化诊断进的是同一个 `ScriptDebugger`：编辑器 Console 面板看到的内容与日志一致
（阶段口径由 `bridge_error_parts` 统一，见 `docs/20`）。

## 4. 宿主接线一览

| 宿主 | 档位 | 备注 |
|---|---|---|
| `game.exe`（`game_app.rs`） | `Contain` | `run_simulation_steps` 里保留的 `fail()` 只处理**引擎侧**失败（场景/输入/资源） |
| 编辑器 Play（`preview_session.rs`） | `Contain` | 容错后 Play 不再因为脚本错误停步；诊断进 Game 视图 overlay |
| 26 项无头样例 / soak / 性能基线 | `Strict` | 不设置策略即为默认档，`unwrap()` 语义不变 |
| Web 宿主（`game_web`） | `Strict` | 桥的返回值就是 JS 侧握手信号；容错会让"启动 OK 但游戏坏了"通过握手断言 |

## 5. 边界（诚实的部分）

- **不提供 Lua 沙箱**：`Lua::new()` 仍是全套 `io`/`os`/`package`，没有内存上限与指令预算。
  本策略只回答"回调报错之后怎么办"。
- **停用不等于修复**：被停用的回调不再执行，依赖它的玩法会**静默降级**（例如敌人不再移动）。
  目前的信号是日志（error 级、前缀 `[lua-guard]`）与 `script_disabled_callbacks()`；编辑器 Play
  另外把它显示在 Game 视图的诊断行上。**面向玩家的界面提示尚未实现**——这是有意留的缺口：
  在没有确定的表现形式（弹窗？角落提示？）之前不先做一个会挡住玩法的东西。
- **按名字注册的回调仍有typo风险**：`on_click` 拼错不会报错（沿用既有语义），本策略不覆盖这一类。
- **`Contain` 下 `update()` 的字符串返回值丢失**：容错路径统一返回空串。工程级 `update` 的返回值
  只有诊断/示例用途，因此不影响玩法；需要该返回值的调用方应使用严格档。
