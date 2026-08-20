# RustLua Engine Demo

## 当前目标

本 Demo 验证 Rust Cargo workspace、`engine_core`、窗口生命周期、wgpu 清屏、Lua 脚本桥接，以及结构化日志和错误上下文已经连成一条可重复运行的最小链路。

## 运行

在 `E:\Codex\p2` 执行：

```powershell
./scripts/run-demo.ps1
./scripts/run-demo.ps1 -Release
```

也可以在 `E:\Codex\p2\engine` 执行：

```powershell
cargo run -p game_runner
cargo run --release -p game_runner
```

窗口标题为 `P2 RustLua Engine - M1-03 Lua Bridge`。按 Escape 或关闭窗口退出。

## 预期日志

```text
2026-08-20T... INFO game: game runner starting version="0.1.0"
2026-08-20T... INFO game: [render] initialized: adapter=<GPU> format=<sRGB> present_mode=<Mailbox/Fifo>
2026-08-20T... INFO script: Lua log.info project script loaded
2026-08-20T... INFO game: [script] update returned: lua-update-ok
```

日志同时输出到控制台和：

```text
E:\Codex\p2\engine\artifacts\logs\game-runner.log
```

默认过滤器会保留业务日志，并将 `wgpu_hal`、`wgpu_core` 和 `naga` 的输出限制为 warning 及以上。

## 质量门

```powershell
./scripts/verify.ps1
./scripts/verify.ps1 -Release
./scripts/test.ps1
./scripts/build-release.ps1
```

质量门日志位于：

```text
E:\Codex\p2\engine\artifacts\logs\quality\
```

## 当前验收边界

- 已验证 workspace、Debug/Release 构建、窗口、GPU 清屏、Lua VM 桥接和结构化日志。
- `EngineError` 已覆盖 Lua 加载/执行、渲染初始化和 GPU 内存不足等关键路径。
- `editor`、`asset` 分类已定义，具体业务日志待后续模块实现。
- 固定时间步、输入系统、ECS 和资源系统不属于当前 Demo。
