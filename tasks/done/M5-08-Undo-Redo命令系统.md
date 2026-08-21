- ???DONE

﻿# M5-08：Undo/Redo 命令系统

- 状态：PASS（命令历史库层）
- 完成日期：2026-08-21
- 所属里程碑：M5 编辑器 MVP
- 前置任务：M5-03 场景树与实体编辑

## 1. 目标

为编辑器提供可逆命令历史，支持编辑操作撤销、重做、Redo 分支清理以及连续 Transform 输入合并，并保证历史边界不会 panic。

## 2. 实施内容

### 2.1 可逆命令模型

在 `engine/crates/editor/src/history.rs` 新增：

- `CommandKind`：Generic、Transform、Create、Delete、Rename、Reparent、Property；
- `EditCommand`：保存操作前后的 `SceneDocument` 快照；
- `CommandHistory`：维护 undo/redo 两个栈；
- `HistoryError`：明确区分无可撤销和无可重做状态。

采用文档快照实现可逆操作，能够覆盖创建、删除子树、属性修改、重命名和父子关系等场景，避免为每种场景结构编写不完整的反向操作。

### 2.2 历史规则

- `execute` 只记录实际改变文档的操作；
- 新操作会清空 redo 栈；
- 连续同一实体的 `Transform(SceneEntityId)` 操作合并为一个历史项；
- `clear` 用于关闭或切换场景时释放历史；
- `undo`/`redo` 在边界返回错误，不修改文档。

### 2.3 公开 API

在 `engine/crates/editor/src/lib.rs` 导出：

```rust
CommandHistory
CommandKind
EditCommand
HistoryError
```

## 3. 测试

新增测试：

- `undo_redo_and_new_edit_branch`
  - 验证撤销、重做和新操作清空 redo 分支；
- `consecutive_transforms_merge`
  - 连续 100 次 Transform 修改合并为 1 条历史，并一次撤销恢复到初始状态。

本次验证同时通过已有 editor 测试，共 44 个测试通过。

## 4. 验收命令

```powershell
cargo fmt --manifest-path E:\Codex\p2\engine\Cargo.toml --all
cargo test --manifest-path E:\Codex\p2\engine\Cargo.toml -p editor --lib
git diff --check
```

实际结果：

```text
44 passed; 0 failed
```

## 5. 当前边界

本任务已完成命令历史库层，但尚未将 Undo/Redo 按钮、快捷键和所有 GUI 编辑入口完整接入 `editor/src/main.rs`。100 次随机操作序列化往返测试也尚未作为独立验收测试加入。后续接入 GUI 时应复用 `CommandHistory::execute`，不要绕过命令系统直接修改 `SceneDocument`。
