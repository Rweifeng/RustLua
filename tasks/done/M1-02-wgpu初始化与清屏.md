# M1-02 wgpu 初始化与清屏

## 状态

ACCEPTED

## 目标

为 M1-01 窗口运行器接入 wgpu，完成 Instance、Adapter、Device、Queue、Surface 初始化、Surface 配置、清屏渲染和窗口缩放重配置。

## 不做什么

- 不实现 Sprite、纹理、材质或 2D 渲染管线；
- 不接入 Lua、ECS、资源系统或编辑器；
- 不实现多窗口和跨平台发布配置。

## 技术方案

- 使用 `wgpu 27.0.1` 和 `pollster 0.4`；
- 在 `game_runner` 中创建 wgpu Instance、Surface、Adapter、Device 和 Queue；
- 根据 Surface capabilities 选择 sRGB 格式、Mailbox/Fifo 呈现模式和 Alpha 模式；
- 使用 RenderPass 的 `LoadOp::Clear` 清屏，颜色为深蓝色；
- 对 `SurfaceError::Lost`、`Outdated`、`Timeout`、`OutOfMemory` 和 `Other` 分别处理；
- 宽度或高度为 0 时不重新配置 Surface，恢复有效尺寸后重新配置；
- 初始化失败输出可理解错误并请求应用退出，不使用未处理 panic。

## 修改范围

- `engine/Cargo.toml`；
- `engine/Cargo.lock`；
- `engine/crates/game_runner/Cargo.toml`；
- `engine/crates/game_runner/src/main.rs`；
- 本任务记录、Demo README、验收矩阵和实施计划。

## 验收标准

- [x] Debug 构建通过并能初始化 wgpu；
- [x] Release 构建通过并能初始化 wgpu；
- [x] GPU Adapter、Device、Queue 和 Surface 初始化成功；
- [x] 清屏 RenderPass 和 Present 提交成功；
- [x] 调整窗口尺寸后 Surface 重新配置并继续运行；
- [x] SurfaceLost、Outdated、Timeout、OutOfMemory 和 Other 均有处理分支；
- [x] 初始化失败输出可理解错误并请求退出；
- [x] 真实窗口关闭后进程退出且生命周期日志完整；
- [x] 无 Clippy 警告、测试失败或未处理 panic。

## 测试命令

```powershell
$env:PATH = "C:\Users\Administrator\.cargo\bin;" + $env:PATH
./scripts/verify.ps1
./scripts/verify.ps1 -Release
```

## 验收证据

- Debug 质量日志：`artifacts/logs/quality/verify-20260820-142125.log`；
- Release 质量日志：`artifacts/logs/quality/verify-20260820-142201.log`；
- GPU 窗口日志：`artifacts/logs/M1-02-wgpu-session.stdout.log`；
- 实际 Adapter：`NVIDIA GeForce GT 1030`；
- 实际格式：`Bgra8UnormSrgb`；
- 实际呈现模式：`Mailbox`；
- 实际日志包含 Surface 初始化、窗口缩放重配置、关闭和 `Exited`；
- 真实窗口进程正常退出。

## 已知限制

当前只完成纯色清屏，没有绘制几何体或纹理。清屏颜色和渲染管线将在后续 2D 任务中扩展。
