# M5-10：编辑器 MVP 综合验收

- 状态：REWORK（质量门通过，综合工作流未完全闭环）
- 验收日期：2026-08-21
- 所属里程碑：M5 编辑器 MVP
- 前置任务：M5-01 至 M5-09

## 1. 验收目标

验证编辑器 MVP 的完整工作流：打开项目、创建场景、放置实体、编辑 Transform、绑定 Lua、保存、Play/Stop、资源刷新、Undo/Redo、自动保存/崩溃恢复以及 Demo 构建运行。

## 2. 已验证内容

### 2.1 质量门

以下质量门全部通过：

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `cargo build --workspace`
- `cargo build --workspace --release`
- `git diff --check`

测试结果：

```text
editor：47 passed; 0 failed
engine_core：100 passed; 0 failed
workspace：147 个单元测试通过
```

### 2.2 项目检查 CLI

合法项目检查通过：

```text
editor_project_ok root=\\?\E:\Codex\p2 baseline=v0.4-scripting
directory=engine status=present
directory=game status=present
directory=assets status=present
directory=docs status=present
```

非法项目检查也符合预期：退出码为 `1`，错误包含项目路径和“项目路径不存在”原因。

### 2.3 已具备的功能模块

- M5-01：editor crate、项目打开和 `--check` CLI；
- M5-02：场景文档创建、打开、保存、另存为和 dirty 状态；
- M5-03：场景树实体创建、删除、重命名和父子关系；
- M5-04：Inspector Transform、Sprite 和 Lua 字段；
- M5-05：资源扫描、筛选和刷新；
- M5-06：Lua 绑定、Console 和诊断；
- M5-07：Play/Stop 编辑态与运行态隔离；
- M5-08：Undo/Redo 命令历史库层；
- M5-09：自动保存/恢复文件库层。

## 3. 未通过的综合验收项

当前不能将 M5-10 标记为 ACCEPTED，原因是 M5-08 和 M5-09 的 GUI 闭环尚未完成：

1. `editor/src/main.rs` 尚未完整接入 Undo/Redo 按钮、快捷键以及所有编辑入口；
2. `AutoSaveController` 尚未接入实际 GUI 更新循环；
3. 启动时恢复文件扫描结果尚未显示“恢复/丢弃”交互提示；
4. M5 综合十步流程尚未完成可重复的 GUI 自动化验收；
5. 当前未生成 M5 截图证据，也未创建 `v0.5-editor-mvp` 标签；
6. Save/Play/Stop、资源刷新和恢复系统之间尚未组成完整的最终 GUI 工作流。

因此本次结论为：质量门 PASS，M5-10 综合验收 REWORK。

## 4. 验收证据

- `artifacts/logs/m5/m5-10-quality-summary.txt`
- `artifacts/logs/m5/m5-10-editor-project-check.txt`
- `artifacts/logs/m5/m5-10-editor-invalid-project-check.txt`
- `tasks/done/M5-01-编辑器启动与项目打开.md`
- `tasks/done/M5-02-项目与场景文档模型.md`
- `tasks/done/M5-03-场景树与实体编辑.md`
- `tasks/done/M5-04-Inspector第一批组件.md`
- `tasks/done/M5-05-资源浏览器与刷新.md`
- `tasks/done/M5-06-绑定与Console.md`
- `tasks/done/M5-07-Play-Stop运行态隔离.md`
- `tasks/done/M5-08-Undo-Redo命令系统.md`
- `tasks/done/M5-09-自动保存与崩溃恢复.md`

本次未生成 `artifacts/screenshots/m5/`，因为完整 GUI 验收流程尚未闭环。

## 5. 后续整改

- 将 `CommandHistory` 接入 `EditorApp` 的创建、删除、重命名、Inspector 和父子关系编辑；
- 增加 Undo/Redo 工具栏、Ctrl/Cmd+Z、Ctrl/Cmd+Y 和场景切换清理；
- 将 `AutoSaveController` 接入 `EditorApp::update`；
- 启动时扫描恢复文件并提供恢复/丢弃提示；
- 接入正式保存后的恢复文件清理；
- 增加综合 GUI/CLI 验收脚本和截图；
- 完成后重新运行十步流程，再创建 `v0.5-editor-mvp` 标签。

## 6. 已知风险

当前若直接标记 M5-10 完成，会把库层完成误认为完整编辑器交互完成，并掩盖恢复提示、自动保存循环和 Undo/Redo GUI 接入缺口。因此保留 `REWORK` 状态，不创建版本标签。

## ??????

1. ????????????2. ?????3. ??????????4. ?? Inspector?5. ???????6. ??? Undo/Redo?7. ?? Ctrl/Cmd+Z?Ctrl/Cmd+Y?8. Save ??????????9. ????? Recover/Discard?10. Play/Stop???????? release runner?

CLI ???`artifacts/logs/m5/m5-10-comprehensive-acceptance.txt`?
GUI ???`artifacts/screenshots/m5/editor-main.png`?`artifacts/screenshots/m5/editor-recovery-undo.png`?
