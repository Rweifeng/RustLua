# M10-01 运行时抽取为可复用 crate

## 状态

READY

## 目标

把 `game_runner` 的 `RuntimeScene`（场景→帧数据桥）与 `Renderer`（GPU 渲染）抽到独立 crate（如 `engine_host`），暴露「创建运行时 / 步进 / 渲染到目标」接口；`game_runner` 改用它，行为不变。这是编辑器内嵌游戏预览（M10-03）的地基。

## 背景

当前 `game_runner` 是独立二进制，编辑器无法运行游戏。M10-03 要在编辑器内嵌游戏预览，必须先让运行时可被 editor crate 复用。ADR-0012 已批准单一 wgpu 设备 + offscreen 渲染方向，本任务先完成代码抽取（行为不变），offscreen 目标在 M10-02。

## 输入

- `docs/28-编辑器强化实施计划.md`（M10-01）
- `docs/02-technical-decisions.md`（ADR-0012）
- `engine/crates/game_runner/src/runtime_scene.rs`、`renderer.rs`、`frame.rs`、`shader.rs`
- `engine/crates/game_runner/Cargo.toml`

## 不做什么

- 不改变渲染行为、画面或性能；
- 不引入 offscreen 渲染（M10-02）；
- 不修改场景格式、Lua API 或公共组件接口。

## 技术方案

（实施前由 AI 填写：crate 划分、接口、生命周期与错误处理）

## 修改范围

- 新建 `engine/crates/engine_host/`（或复用现有 crate 结构）
- `engine/crates/game_runner/`（改用 engine_host）
- `engine/Cargo.toml`（workspace member）

## 验收标准

- 抽取后 game_runner 运行行为与 v0.9 基线一致（2D 游戏 / 3D 游戏 / 发布包）；
- `engine_host` 暴露创建运行时、步进、渲染到目标的接口；
- Debug/Release 质量门通过。

## 测试命令

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
cargo build --workspace --release
cargo run -p game_runner -- --project C:\GitHub\p2\game --check
git diff --check
```

## 验收证据

- 抽取后实机运行日志与基线一致（draws/instances、fps、entities）；
- workspace 质量门日志。

## 风险

- 抽取破坏渲染行为：只移动代码，每步独立编译并跑质量门；
- crate 边界过度设计：以「先抽边界、不过度设计」为原则。

## AI 输出

（实施后填写）

## 已知限制

本任务不新增渲染功能；offscreen 渲染目标在 M10-02。
