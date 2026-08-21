# RustLua Engine Demo

## 当前目标

本 Demo 验证 Rust Cargo workspace、`engine_core`、窗口生命周期、wgpu 清屏、Lua 脚本桥接、结构化日志、固定时间步、输入快照和性能 overlay 已连接为一条可重复运行的最小链路。

## 运行

在 `C:\GitHub\p2` 执行：

```powershell
./scripts/run-demo.ps1
./scripts/run-demo.ps1 -Release
```

窗口标题为 `P2 RustLua Engine - M1-07 Performance Overlay`。按 F3 开关性能 overlay，按 Escape 或关闭窗口退出。

## 性能 overlay

overlay 默认显示在左上角，包含面板和性能条：FPS、Frame ms、Fixed update ms、Lua ms。

性能数值同时以 `[perf] fps=... frame_ms=... update_ms=... lua_ms=... frames=... overlay=...` 结构化日志输出。

## 质量门

```powershell
./scripts/verify.ps1
./scripts/verify.ps1 -Release
```

质量门日志位于 `C:\GitHub\p2\artifacts\logs\quality\`。

## M1-07 验收证据

- `C:\GitHub\p2\artifacts\logs\M1-07-overlay-session-debug.stdout.log`
- `C:\GitHub\p2\artifacts\logs\M1-07-overlay-session-debug.stderr.log`
- `C:\GitHub\p2\artifacts\logs\M1-07-overlay-session-release.stdout.log`
- `C:\GitHub\p2\artifacts\logs\M1-07-overlay-session-release.stderr.log`
- `C:\GitHub\p2\artifacts\logs\quality\verify-20260820-164230.log`
- `C:\GitHub\p2\artifacts\logs\quality\verify-20260820-164231.log`
