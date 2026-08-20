# RustLua Engine Demo

## 当前目标

本 Demo 验证 Rust Cargo workspace、`engine_core`、窗口生命周期、wgpu 清屏、Lua 脚本桥接、结构化日志和固定时间步已经连成一条可重复运行的最小链路。

## 运行

在 `E:\Codex\p2` 执行：

```powershell
./scripts/run-demo.ps1
./scripts/run-demo.ps1 -Release
```

也可以在 `E:\Codex\p2\engine` 执行 `cargo run -p game_runner` 或 `cargo run --release -p game_runner`。窗口标题为 `P2 RustLua Engine - M1-05 Fixed Time Step`，按 Escape 或关闭窗口退出。

## 固定时间步行为

- simulation 默认固定 60Hz；Lua `update()` 每次接收固定 `dt`，可通过 `time.delta()` 读取；
- 渲染按窗口实际重绘频率执行；
- 单帧最多追赶 8 个 simulation step，超出部分丢弃，防止螺旋死亡；
- 暂停/恢复时清空 accumulator，不重放暂停期间积累的旧时间。

示例：

```lua
function update()
    local dt = time.delta()
    return string.format("lua-update-ok dt=%.6f", dt)
end
```

## 预期日志

```text
2026-08-20T... INFO engine: [time] fixed-step stats: steps=1 dt=0.016667s accumulator=... total_steps=60 total_dropped=0
```

`steps` 是本次逻辑步数，`dt` 是固定步长，`accumulator` 是剩余时间，`total_steps` 和 `total_dropped` 是累计执行/丢弃步数。日志同时输出到控制台和 `E:\Codex\p2\engine\artifacts\logs\game-runner.log`。

## 质量门

```powershell
./scripts/verify.ps1
./scripts/verify.ps1 -Release
./scripts/test.ps1
./scripts/build-release.ps1
```

质量门日志位于 `E:\Codex\p2\artifacts\logs\quality\`。

M1-05 验收日志：

- `E:\Codex\p2\artifacts\logs\M1-05-fixed-timestep-session-final.stdout.log`
- `E:\Codex\p2\artifacts\logs\M1-05-fixed-timestep-session-release.stdout.log`
- `E:\Codex\p2\artifacts\logs\quality\verify-20260820-160225.log`
- `E:\Codex\p2\artifacts\logs\quality\verify-20260820-160242.log`

## 当前验收边界

已验证 workspace、Debug/Release 构建、窗口、GPU 清屏、Lua VM 桥接、结构化日志和固定 60Hz simulation。输入系统、ECS、资源系统、时间缩放、插值渲染和 FPS overlay 不属于当前 Demo。