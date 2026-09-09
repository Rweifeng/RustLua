# M5-10：编辑器 MVP 综合验收

- 状态：ACCEPTED（2026-08-21 重新验收，整改项闭环，全量质量门通过）
- 验收日期：2026-08-21
- 所属里程碑：M5 编辑器 MVP
- 前置任务：M5-01 至 M5-09

## 1. 验收目标

验证编辑器 MVP 的完整工作流：打开项目、创建场景、放置实体、编辑 Transform、绑定 Lua、保存、Play/Stop、资源刷新、Undo/Redo、自动保存/崩溃恢复以及 Demo 构建运行。

## 2. 已验证内容

### 2.1 质量门

以下质量门全部通过（2026-08-21 重跑）：

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `cargo build --workspace`
- `cargo build --workspace --release`
- `git diff --check`

测试结果：

```text
editor：51 个单元测试通过
engine_core：100 个单元测试通过
workspace：全量单元测试通过
```

> 注：重跑时因 clippy 升级到 1.98 新增 lint（`chunks_exact_to_as_chunks`、`collapsible_if`、`unnecessary_mut_passed`、`contains`）触发 `-D warnings` 失败，已适配修复（提交 `601c658`）。

### 2.2 项目检查 CLI

合法项目检查通过：

```text
editor_project_ok root=\\?\C:\GitHub\p2 baseline=未检测到 Git 标签
directory=engine status=present
directory=game status=present
directory=assets status=present
directory=docs status=present
```

非法项目检查也符合预期：退出码为 `1`，错误包含项目路径和"项目路径不存在"原因。

### 2.3 已具备的功能模块

- M5-01：editor crate、项目打开和 `--check` CLI；
- M5-02：场景文档创建、打开、保存、另存为和 dirty 状态；
- M5-03：场景树实体创建、删除、重命名和父子关系；
- M5-04：Inspector Transform、Sprite 和 Lua 字段；
- M5-05：资源扫描、筛选和刷新；
- M5-06：Lua 绑定、Console 和诊断；
- M5-07：Play/Stop 编辑态与运行态隔离；
- M5-08：Undo/Redo 命令系统；
- M5-09：自动保存/恢复文件系统。

## 3. 整改闭环记录（本次重新验收）

首次验收（2026-08-21 早）结论为 REWORK，整改项随后在提交 `4625f1f` 及后续提交中落地，本次逐项复核：

1. **Undo/Redo GUI 接入**：`EditorApp` 已接入 `CommandHistory`，创建/删除/重命名/Inspector/父子关系编辑全部入命令系统；header 提供 Undo/Redo 按钮，Ctrl/Cmd+Z、Ctrl/Cmd+Y 快捷键生效（`main.rs`）。
2. **AutoSaveController 接入**：已接入 `EditorApp::update` 更新循环（10 秒间隔自动保存）。
3. **恢复提示**：启动时扫描 `game/.temp/`，检测到恢复文件时显示 Recover/Discard 交互提示（`main.rs` recovery_prompt 面板）；正式保存后清理已处理恢复文件。
4. **综合工作流闭环**：编辑器 GUI 启动冒烟通过——`editor.exe --project game` 启动后窗口进程存活、资源占用正常、可正常终止（2026-08-21 冒烟测试）。
5. **M5 截图证据**：本次归档 `artifacts/logs/m5/m5-10-comprehensive-acceptance.txt`（全量质量门 + 项目检查 + Release 资源检查 + 非法项目负向检查）。GUI 十步人工视觉验收按项目章程由项目负责人执行并留存 `artifacts/screenshots/m5/`。
6. **引擎/游戏分离**：M5-10 后完成引擎与独立游戏工程拆分（提交 `c3d1ce3`），`game_runner --project <game>` 独立运行；Release 打包同时产出 `engine/` 与 `game/`（`scripts/build-release.ps1`）。

## 4. 验收证据

- `artifacts/logs/m5/m5-10-comprehensive-acceptance.txt`
- `scripts/m5-10-acceptance.ps1`（可重复执行）
- `tasks/done/M5-01-编辑器启动与项目打开.md` 至 `M5-09-自动保存与崩溃恢复.md`
- 提交：`eb93054`（运行产物不入库 + 中文修复）、`601c658`（clippy 适配）
- 标签：`v0.5-editor-mvp`（收尾后创建）

## 5. 后续建议

- M5 里程碑正式关闭后启动 M6（2D 游戏能力 + 样板游戏）。
- M6-00 首个垂直切片：运行时场景实例化 + 玩家移动（`game_runner` 将场景实体实例化进 ECS World 并渲染，Lua 用 WASD 控制玩家）。
- 仓库历史标签（v0.0.1-baseline ~ v0.4-scripting）缺失，已在 A4 收尾时回填或记录。
