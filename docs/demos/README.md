# M0 最小 Demo

## 目标

验证 Rust Cargo workspace、`engine_core` 库和 `game_runner` 可执行程序已经形成一个可重复运行的最小基线。

这个 Demo 当前验证窗口生命周期入口，不包含 wgpu 清屏、Lua、ECS 或游戏玩法。渲染和脚本能力将在后续 M1 任务中实现。

## 环境要求

- Windows；
- Rust stable 工具链；
- `cargo`、`rustc` 和 `rustup` 可调用；
- 如果当前终端尚未刷新 PATH，可临时执行：

```powershell
$env:PATH = "C:\Users\Administrator\.cargo\bin;" + $env:PATH
```

项目使用 `engine/rust-toolchain.toml` 声明 stable、rustfmt 和 clippy。

## 运行 Demo

在项目根目录 `E:\Codex\p2` 执行：

### Debug

```powershell
./scripts/run-demo.ps1
```

预期输出：

```text
p2 game runner 0.1.0
[lifecycle] Running
[window] created: WindowId(...)
[render] initialized: adapter=<GPU> format=<sRGB> present_mode=<Mailbox/Fifo>

窗口标题：P2 RustLua Engine - M1-01 Window
按 Escape 或关闭窗口退出。
```

### Release

```powershell
./scripts/run-demo.ps1 -Release
```

预期输出同上：

```text
p2 game runner 0.1.0
[lifecycle] Running
[window] created: WindowId(...)
[render] initialized: adapter=<GPU> format=<sRGB> present_mode=<Mailbox/Fifo>

窗口标题：P2 RustLua Engine - M1-01 Window
按 Escape 或关闭窗口退出。
```

也可以直接在 `engine` 目录执行：

```powershell
cargo run -p game_runner
cargo run --release -p game_runner
```

## 完整验收

```powershell
./scripts/verify.ps1
./scripts/verify.ps1 -Release
./scripts/test.ps1
./scripts/build-release.ps1
```

Release 运行器复制到：

```text
E:\Codex\p2\artifacts\builds\release\game_runner.exe
```

质量日志位于：

```text
E:\Codex\p2\artifacts\logs\quality\
```

## 故障排查

### 找不到 cargo

确认 Rust stable 已安装，并检查 `cargo` 是否在 PATH 中。可以重新打开终端，或临时加入：

```powershell
$env:PATH = "C:\Users\Administrator\.cargo\bin;" + $env:PATH
```

### 格式检查失败

执行：

```powershell
Set-Location E:\Codex\p2\engine
cargo fmt --all
cargo fmt --all -- --check
```

### Clippy 或测试失败

先执行：

```powershell
Set-Location E:\Codex\p2
./scripts/verify.ps1
```

查看脚本输出和 `artifacts/logs/quality/` 下最新日志，不得通过删除测试或放宽质量门来规避问题。

## 当前验收边界

当前 Demo 证明：

- workspace 能被 Cargo 识别；
- `engine_core` 能编译并通过最小测试；
- `game_runner` 能在 Debug/Release 下创建窗口并退出；
- 本地质量门可以重复执行。

后续 M1 任务将增加 wgpu 清屏、Lua VM、结构化日志、固定时间步和输入系统。
