# M1-03 Lua VM 桥接

状态：ACCEPTED

## 目标
接入 `mlua`，创建主线程 Lua VM，注册最小宿主 API，并让项目脚本可被加载、执行和由 Rust 调用。

## 不做什么
本任务不实现结构化日志系统、固定时间步、热重载、Lua 多线程执行、ECS 或游戏玩法 API。

## 技术方案
- `engine_core::script::LuaRuntime` 封装 `mlua::Lua`。
- 使用 `mlua 0.11` 的 `lua54 + vendored` 特性，避免依赖本机 Lua 安装。
- 注册 `log.info(message)`、`time.delta()`、`app.quit()`。
- 通过 `Rc<RefCell<ScriptState>>` 保存 delta 和退出请求；VM 只在主线程使用。
- 脚本加载和运行错误统一转换为 `LuaBridgeError`，包含脚本路径和 Lua 原始行号/错误信息。

## 修改范围
- `engine/crates/engine_core/Cargo.toml`
- `engine/crates/engine_core/src/script.rs`
- `engine/crates/engine_core/src/lib.rs`
- `engine/crates/game_runner/src/main.rs`
- `game/scripts/main.lua`
- `docs/04-acceptance-matrix.md`
- `docs/demos/README.md`

## 验收标准与结果
- [x] Lua 可以调用 Rust API：`log.info`、`time.delta`、`app.quit` 单元测试通过。
- [x] Rust 可以调用 Lua `update()`，返回 `lua-update-ok`。
- [x] 语法错误包含脚本路径和行号信息。
- [x] 运行时错误返回可读日志，不 panic 或无日志退出。
- [x] VM 使用 vendored Lua 5.4，Debug/Release 均可构建。
- [x] 真实窗口 Demo 输出 Lua 加载和回调日志，GPU 清屏继续运行。

## 测试命令与证据
```powershell
$env:PATH = "C:\Users\Administrator\.cargo\bin;" + $env:PATH
cargo test --workspace
./scripts/verify.ps1
./scripts/verify.ps1 -Release
```

证据：
- `artifacts/logs/quality/verify-20260820-144431.log`
- `artifacts/logs/quality/verify-20260820-144146.log`
- `artifacts/logs/M1-03-lua-session.stdout.log`
- `artifacts/logs/M1-03-lua-session.stderr.log`
- `artifacts/logs/M1-03-lua-session-release.stdout.log`
- `artifacts/logs/M1-03-lua-session-release.stderr.log`

真实 Demo 关键输出：
```text
[render] initialized: adapter=NVIDIA GeForce GT 1030 format=Bgra8UnormSrgb present_mode=Mailbox
[script] project script loaded
[script] update returned: lua-update-ok
```

## 已知限制
- 当前仅在启动阶段执行一次脚本和一次 `update()`；每帧脚本调度留给 M1-05。
- 脚本路径暂按项目根目录或 `engine` 工作目录解析；后续资产系统接管项目路径。
- `LuaBridgeError::Api` 尚未接入 M1-04 的结构化错误上下文。

## AI 输出
M1-03 已实现并通过 Debug/Release 质量门、工作区测试和真实窗口 Demo 验收。