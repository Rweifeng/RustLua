# M5-04：Inspector 第一批组件

## 目标

为编辑器实体提供第一批可编辑属性：名称、Transform、Sprite 引用和 Lua 脚本引用，并接入中央 Inspector 面板。

## 实施条目

- [x] 新增 `InspectorState`、`InspectorSnapshot` 和 `TransformFields`。
- [x] 实现实体选择、快照读取和字段写回。
- [x] 校验 Transform 有限数值与正缩放。
- [x] 校验 Sprite/Lua 相对路径、扩展名和路径穿越。
- [x] 接入场景树选择同步。
- [x] 接入中央 Inspector GUI 和应用按钮。
- [x] 保持现有 M4 场景组件格式不变。
- [x] 增加保存/重载字段保持测试。
- [x] 更新验收矩阵和使用手册。

## 验收方案

1. `cargo fmt --manifest-path engine/Cargo.toml --all -- --check` 通过。
2. `cargo clippy --manifest-path engine/Cargo.toml --workspace --all-targets --all-features -- -D warnings` 通过。
3. `cargo test --manifest-path engine/Cargo.toml --workspace --all-features` 通过。
4. 选择实体后，Inspector 能显示并编辑名称、Transform、Sprite 和 Lua 字段。
5. 非法字段不会写入场景，也不会错误地设置 dirty。
6. 场景文本序列化后重新加载，字段值保持一致。
7. `git diff --check` 通过。

## 边界

本任务不实现资源浏览器、资源存在性检查、Lua 加载/调试、保存按钮、Undo/Redo。

## 状态

- 状态：PASS
- 完成日期：2026-08-21
