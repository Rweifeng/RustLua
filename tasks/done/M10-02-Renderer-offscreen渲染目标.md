# M10-02 Renderer offscreen 渲染目标

## 状态

ACCEPTED

## 目标

`engine_host::Renderer` 支持渲染到 offscreen 纹理（而非仅窗口 surface）：新增 `render_into_texture(mesh, scene3d, view, width, height)`，共享 pass 渲染逻辑；depth 附件随目标尺寸自适应。窗口模式行为不回归。这是编辑器内嵌游戏预览（M10-03）的技术核心。

## 背景

M10-01 已把 Renderer 抽到 engine_host。ADR-0012 批准单一 wgpu 设备 + offscreen 渲染方向。M10-02 让 Renderer 具备离屏渲染能力，供编辑器把游戏/场景渲染到纹理再显示为 egui 纹理。

## 输入

- `docs/28-编辑器强化实施计划.md`（M10-02）
- `docs/02-technical-decisions.md`（ADR-0012）
- `engine/crates/engine_host/src/renderer.rs`（render 函数）

## 不做什么

- 不引入外部设备构造（M10-03 编辑器接入时再处理）；
- 不改变窗口渲染行为；
- 不修改场景格式、Lua API 或公共组件接口。

## 技术方案

- 新增 `Renderer::render_into_texture(mesh, scene3d, view, width, height)`：把 2D/3D pass 渲染到调用方提供的 offscreen 纹理视图，无 present；
- 抽出公共 `render_passes(encoder, mesh, scene3d, view)`：清屏 + shadow + 3D + 2D 全部 pass 复用；
- 新增 `ensure_depth_view(width, height)` 与 `depth_size` 字段，depth 附件随离屏目标尺寸自适应重建；
- 窗口 `render` 改为获取 surface view 后调用 `render_passes` + submit + present，行为不变。

## 修改范围

- `engine/crates/engine_host/src/renderer.rs`

## 验收标准

- `render_into_texture` 把 2D/3D pass 渲染到给定纹理视图；
- depth 视图随离屏目标尺寸自适应（`ensure_depth_view`）；
- 窗口 `render` 路径行为与之前一致；
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

- 实机窗口渲染与重构前一致（2D/3D 画面、draws/instances、fps）；
- `render_into_texture` 路径的集成验证（M10-03 编辑器接入时端到端）。

## 风险

- 重构破坏窗口渲染：抽公共 pass 时每步独立编译并跑质量门；
- 离屏 depth 尺寸不匹配：`ensure_depth_view` 按目标尺寸重建。

## AI 输出

已完成：

- 新增 `render_into_texture`（offscreen 目标，无 present）；
- 抽出公共 `render_passes`（清屏 + shadow + 3D + 2D）；
- 新增 `ensure_depth_view`/`depth_size`，depth 随目标尺寸自适应；
- 窗口 `render` 路径行为不变。

修改文件：

- `engine/crates/engine_host/src/renderer.rs`

验证结果：

- fmt/clippy -D warnings/282 测试/Debug/Release/--check/git diff --check 全通过；
- 实机 2D main（entities=7）与 3D game3d（entities=13）渲染与重构前一致，无错误；
- `render_into_texture` 端到端由 M10-03 编辑器内嵌预览验证。

已知问题：

- 无 P0/P1。offscreen 目标由调用方创建。

建议下一步：

- M10-02 提交后进入 M10-03 编辑器内嵌游戏预览（用 `render_into_texture` 接入 egui）。

## 已知限制

offscreen 目标纹理由调用方（编辑器）创建；Renderer 只负责渲染到给定视图。
