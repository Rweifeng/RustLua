# M5-07：Play/Stop 运行态隔离

- 状态：PASS
- 完成日期：2026-08-21
- 所属里程碑：M5 编辑器 MVP
- 前置任务：M5-06 Lua 绑定与 Console

## 1. 目标

为编辑器提供 Play/Stop 运行态，使运行时修改只作用于场景副本。停止后恢复 Play 前的编辑态，不污染场景文件和 dirty 状态。

## 2. 实施内容

### 2.1 PlaySession

在 `editor::document` 中新增 `PlaySession`：

- `start(&SceneDocument)` 深拷贝编辑态文档，形成编辑态快照和运行态副本；
- `runtime_document_mut()` 提供运行态修改入口；
- `stop()` 丢弃运行态修改并返回 Play 前快照。

### 2.2 编辑器 Play/Stop

`EditorApp` 顶部新增 Play/Stop 控件：

- 没有场景时 Play 会拒绝并提示；
- Play 期间禁止新建、创建、删除、重命名和 Inspector 编辑；
- Stop 后丢弃运行态副本，恢复编辑态。

## 3. 测试

- `play_session_isolates_runtime_changes_and_restores_editor_snapshot`
- `play_stop_can_repeat_one_hundred_times_without_edit_state_drift`

第二个测试执行 100 次 Play/Stop 循环，验证编辑态场景内容、路径和 dirty 状态不发生漂移。

## 4. 验收命令

```powershell
cargo fmt --manifest-path E:\Codex\p2\engine\Cargo.toml --all -- --check
cargo clippy --manifest-path E:\Codex\p2\engine\Cargo.toml --workspace --all-targets --all-features -- -D warnings
cargo test --manifest-path E:\Codex\p2\engine\Cargo.toml --workspace --all-features
cargo build --manifest-path E:\Codex\p2\engine\Cargo.toml --workspace
cargo build --manifest-path E:\Codex\p2\engine\Cargo.toml --workspace --release
git diff --check
```

预期结果：`editor` 42 个测试通过，`engine_core` 100 个测试通过，并且 Debug/Release 构建通过。

## 5. 当前边界

本任务隔离编辑器场景文档副本，不包含完整游戏运行循环、物理模拟或 ECS 世界同步。当前不创建 `v0.5-editor-mvp` 标签。
