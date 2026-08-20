# RustLua Engine Demo

## 当前目标

本 Demo 验证 Rust Cargo workspace、`engine_core`、窗口生命周期、wgpu 清屏、Lua 脚本桥接、结构化日志、固定时间步和输入快照已经连成一条可重复运行的最小链路。

## 运行

在 `E:\Codex\p2` 执行：

```powershell
./scripts/run-demo.ps1
./scripts/run-demo.ps1 -Release
```

也可以在 `E:\Codex\p2\engine` 执行 `cargo run -p game_runner` 或 `cargo run --release -p game_runner`。窗口标题为 `P2 RustLua Engine - M1-06 Input Snapshot`，按 Escape 或关闭窗口退出。

## 输入快照行为

- winit 的键盘、鼠标、光标和焦点事件先写入 `InputSnapshot`；
- 固定逻辑帧读取快照，不直接依赖平台事件；
- 键盘和鼠标均支持 `held`、`just_pressed`、`just_released`；
- `W/A/S/D` 映射为 `horizontal`、`vertical` 轴；
- 快速按下/释放会保留到逻辑帧消费；
- 失去焦点或进入暂停时清空所有输入状态，避免恢复后卡键；
- Lua 可调用 `input.is_down("d")`、`input.just_pressed("space")`、`input.axis("horizontal")` 等函数。

示例：

```lua
function update()
    local dt = time.delta()
    local horizontal = input.axis("horizontal")
    local vertical = input.axis("vertical")
    local space_pressed = input.just_pressed("space")
    return string.format("lua-update-ok dt=%.6f axis=(%.1f,%.1f) space_pressed=%s", dt, horizontal, vertical, tostring(space_pressed))
end
```

## 预期日志

```text
2026-08-20T... INFO game: [window] created: WindowId(...)
2026-08-20T... INFO game: [lifecycle] Running
2026-08-20T... INFO script: Lua log.info project script loaded
2026-08-20T... INFO engine: [time] fixed-step stats: steps=1 dt=0.016667s accumulator=... total_steps=60 total_dropped=0
```

日志同时输出到控制台和 `E:\Codex\p2\engine\artifacts\logs\game-runner.log`。

## 质量门

```powershell
./scripts/verify.ps1
./scripts/verify.ps1 -Release
./scripts/test.ps1
./scripts/build-release.ps1
```

质量门日志位于 `E:\Codex\p2\artifacts\logs\quality\`。

M1-06 验收日志：

- `E:\Codex\p2\artifacts\logs\M1-06-input-snapshot-session-debug.stdout.log`
- `E:\Codex\p2\artifacts\logs\M1-06-input-snapshot-session-release.stdout.log`
- `E:\Codex\p2\artifacts\logs\quality\verify-20260820-162504.log`
- `E:\Codex\p2\artifacts\logs\quality\verify-20260820-162526.log`

## 当前验收边界

已验证 workspace、Debug/Release 构建、窗口、GPU 清屏、Lua VM 桥接、结构化日志、固定 60Hz simulation 和输入快照。action map 配置、手柄、ECS、资源系统、时间缩放、插值渲染和 FPS overlay 不属于当前 Demo。