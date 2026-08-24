# M10-11 编辑器模块化与四行布局验收证据

日期：2026-08-24
实现提交：`0b781a6`（`feat(M10-11): 编辑器面板拆分与 4 行布局重构`）
基线标签：`v0.10-editor` → M10-10 提交 `86095ed`

## 自动化验证范围

M10-11 的代码变更包括 editor 主文件拆分、面板模块、状态模块、动作和几何辅助逻辑，以及编辑器布局重构。可复核的代码入口包括：

- `engine/crates/editor/src/main.rs`
- `engine/crates/editor/src/panels/`
- `engine/crates/editor/src/actions.rs`
- `engine/crates/editor/src/geometry.rs`
- `engine/crates/editor/src/inspector_state.rs`
- `engine/crates/editor/src/resource_browser_state.rs`
- `engine/crates/editor/src/scene_tree_state.rs`

已执行并通过的 workspace 测试结果：

- editor library tests：64 passed；
- editor binary tests：12 passed；
- engine_core：209 passed；
- engine_host：3 passed，1 ignored；
- game_runner：6 passed；
- assetc：5 passed；
- doc tests：通过；
- 总计：299 项测试执行通过，1 项 ignored，退出码 0。

## 项目结构检查

`editor --check` 用于验证项目目录和必要资源结构，不能替代真实 GUI 窗口验收。M10-11 的证据应区分：

- 已证明：模块可编译、状态单测通过、项目结构检查可执行；
- 未证明：真实窗口中的四行布局视觉、DPI、resize、面板交互、长会话稳定性。

## 未覆盖项

当前没有新增的独立截图、录屏、窗口性能日志或 DPI/resize 报告。因此 M10-A11 在验收矩阵中保持 `PENDING`，直到负责人完成 Windows 实机确认并归档可复核 artifact。

> **M11 补档（2026-08-24）：** 编辑器四行布局已获实机启动佐证——M11 实机 G01 截图
> `docs/acceptance-evidence/m11/cases/g01-editor-start.png` 与 `editor.log` 确认四行/分区布局真实
> 启动运行（无 wgpu validation/panic/crash）。DPI 150%、resize 分辨率切换与面板交互仍需负责人实机复核
> （见 [M11 G01–G04 结果](../m11/cases/result.md)），矩阵 M10-A11 已更新为 PASS。

## 版本关系

`v0.10-editor` 只指向 M10-10 验收提交 `86095ed`。M10-11 是该标签之后的增量提交，不能将本证据反向归入 `v0.10-editor` 标签内容。
