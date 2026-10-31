# M4-07 Lua 脚本调试

- 状态：PASS
- 完成日期：2026-08-20

## 实现内容

- `ScriptDiagnostic` 结构化脚本诊断记录；
- 记录脚本路径、错误阶段、错误消息、行号、调用栈文本和出现次数；
- `call_update_debug()` 记录运行时错误；
- `ScriptDebugger` 对相同路径、阶段和消息进行去重；
- 支持按文件路径或错误文本过滤；
- 支持清空诊断记录；
- Lua 语法错误继续保留文件路径和行号信息；
- Debug API 只通过显式诊断方法暴露，发布运行路径不会自动开启调试控制台。

## 使用示例

```rust
runtime.execute_project_script(path)?;
if let Err(error) = runtime.call_update_debug() {
    eprintln!("{error}");
}

runtime.set_debug_filter(Some("player.lua".to_string()));
for diagnostic in runtime.diagnostics() {
    println!("{}:{} {}", diagnostic.path.display(), diagnostic.line.unwrap_or(0), diagnostic.message);
}
```

## 边界

- 当前调用栈使用 mlua 返回的错误文本保存；
- 错误去重按 `path + phase + message` 判定；
- `occurrences` 统计重复错误出现次数，避免每帧无限新增记录；
- 未来编辑器 Console 可直接消费 `diagnostics()`。

## 验收

```text
cargo clippy --workspace --all-targets --all-features -- -D warnings
通过

cargo test --workspace --all-features
98 passed; 0 failed

cargo run -p game_runner --example lua_script_debug_demo --release
lua_script_debug_demo_ok diagnostics=1 deduplicated=true filtered=true stack=true
```

## 2026-10-31 — 断点 / 单步 / 局部变量 / 栈帧（headless 内核）

上面这一档解决的是"错误去哪了"，这一档解决"停在哪、当时变量是多少"。实现在
`engine/crates/rustlua-scripting/src/script/debug.rs`，编辑器侧的暂停 UI 尚未接线。

### API

```rust
use rustlua_scripting::{Breakpoint, DebugAction};

runtime.set_debug_breakpoints([Breakpoint::new("scripts/player.lua", 12)])?;
runtime.set_debug_handler(|snapshot| {
    // snapshot.reason   : Breakpoint | Step
    // snapshot.location : { path, line }
    // snapshot.locals   : [{ name, kind, value }]
    // snapshot.frames   : [{ function, location }]（由内到外）
    DebugAction::StepOver
})?;
runtime.debug_pause_count(); // 命中次数
runtime.clear_debugger()?;   // 撤断点 + 卸行事件
```

暂停是**同步回调**：钩子内调用 handler，返回值决定 `Continue` / `StepInto` / `StepOver` / `StepOut`。
这样无头测试可以直接断言，将来编辑器可以在回调里阻塞等 UI 决策。

### 三条实现约束

- **一个 Lua 状态只有一个全局钩子**：断点行事件与指令预算共用 `script/progress.rs::install`。
  调试未激活时不加 `every_line`（保持零额外成本）；指令计数**只在 `Count` 事件累加**，
  行事件不能把 1e9 的预算放大成按行计费。
- **局部变量与栈帧必须借 Lua 自己的 `debug` 库**：mlua 没有 locals / 栈遍历 API，钩子里调
  `debug.getinfo(level, "nSl")` 与 `debug.getlocal(level, i)`（跳过编译器临时变量 `(…)`）。
- **`_G.debug` 仍是 `nil`**：VM 用 `unsafe_new_with(StdLib::ALL, …)` 建，`debug` 表移进命名注册表
  `__rustlua_debug` 后清空全局——脚本可见 API 面不变，也堵住 `debug.sethook` 关掉指令预算。
- **尾调用会丢栈帧**：`return inner(x)` 不保留调用者帧，正是标准 Lua 语义；需要看完整栈就别写尾调用。

### 验收

```text
cargo test -p rustlua-scripting --lib
140 passed; 0 failed
```

- `breakpoint_pauses_with_line_and_locals`：暂停帧 + 行号 + `local("a")`；
- `step_advances_to_the_next_line`：单步后行号前进；
- `stack_frames_carry_file_and_offset`：栈帧文件与偏移正确；
- `no_breakpoints_means_no_pause`、`clearing_the_debugger_removes_the_hook`。

**反面对照已真跑**：删掉 `progress::install` 的 `every_line` 注册后，前三条判红
（`2 passed; 3 failed`）；还原后回到 140 绿。

### 边界

- 编辑器还没有"命中暂停 / 继续 / 单步按钮 / 栈帧点击 → 源码跳转"的界面。内核回调是同步的，
  接 UI 需要把 Play 的脚本执行搬离 UI 线程或做可阻塞桥，这是下一步。
- 没有条件断点、watch 表达式与远程调试；LuaLS 服务端仍未提供（`docs/62` §5）。
