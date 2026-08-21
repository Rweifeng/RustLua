# M5-06：Lua 绑定与 Console

- 状态：PASS
- 完成日期：2026-08-21
- 所属里程碑：M5 编辑器 MVP
- 前置任务：M5-05 资源浏览器与刷新

## 1. 目标

将已有 Lua Runtime 和 ScriptDebugger 接入编辑器，提供可执行的 Lua Console、选中实体脚本运行入口和错误诊断。

## 2. 实施内容

### 2.1 Lua Runtime Console API

在 `engine_core::script::LuaRuntime` 中新增：

```rust
execute_console(&mut self, source: &str) -> Result<String, LuaBridgeError>
```

- 在当前 Lua VM 执行表达式或语句；
- 返回 Lua 值的可读字符串；
- 保持 VM 全局状态，支持多条命令共享变量；
- 执行失败记录 `<console>` 路径和 `runtime` 阶段诊断。

### 2.2 编辑器 Console

编辑器底部新增 Lua Console，支持：

- 输入 Lua 命令并点击“执行”；
- 按 Enter 快捷执行；
- 显示命令结果和错误；
- 按关键词筛选诊断；
- 清空 Console 输出和诊断；
- 运行 Inspector 当前实体引用的 Lua 脚本。

## 3. 验收

```powershell
cargo fmt --manifest-path E:\Codex\p2\engine\Cargo.toml --all -- --check
cargo clippy --manifest-path E:\Codex\p2\engine\Cargo.toml --workspace --all-targets --all-features -- -D warnings
cargo test --manifest-path E:\Codex\p2\engine\Cargo.toml --workspace --all-features
cargo build --manifest-path E:\Codex\p2\engine\Cargo.toml --workspace
cargo build --manifest-path E:\Codex\p2\engine\Cargo.toml --workspace --release
git diff --check
```

预期结果：`engine_core` 100 个测试通过，`editor` 40 个测试通过，格式、Clippy 和 Debug/Release 构建全部通过。

## 4. 已知边界

不包含 Play/Stop 运行态隔离（M5-07）、断点、单步、变量查看、沙箱隔离、多 VM 和命令历史。
