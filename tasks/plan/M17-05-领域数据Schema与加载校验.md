# 任务：领域数据 Schema 与加载校验

- 任务 ID：M17-05
- 状态：待办
- 关联里程碑：M17
- 创建日期：2026-09-09

## 目标
为 NPC、日程、对话、任务、商店、配方、地图出口建立统一的数据字段、引用校验、版本字段和错误诊断约定。

## 影响文件

- `examples/game_jhyt/scripts/data/`
- `engine/crates/engine/src/core/data_schema.rs` 及 Lua API（仅在现有能力不足时）
- `engine/crates/editor/src/` 数据检查入口（仅在现有能力不足时）
- `examples/game_jhyt/scripts/` 数据加载模块
- `docs/02-technical-decisions.md`（若改变公共格式）

## 风险

- 不改变已有场景、存档和资源格式而不提供迁移方案。
- Schema 应区分公共引擎能力与 game_jhyt 领域规则，避免把剧情规则硬编码进 Rust。
- 旧数据必须给出明确版本和错误位置。

## 验收方式

- 错误数据能定位到文件、对象 ID 和字段路径。
- 引用不存在的 NPC、物品、地图、任务或对话在加载阶段失败。
- 纯数据增加一个 NPC、任务、对话、商店条目无需修改 Rust 核心代码。
- Schema 测试覆盖默认值、枚举、范围和引用错误。

## 实现记录

待实现。

## 验收证据

- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过
- [ ] 相关测试通过
- [ ] Debug 构建通过
- [ ] Demo / 手工步骤可复现
- [ ] 文档与 CHANGELOG 已更新

## 未完成项 / 已知限制

- 本任务不要求立即完成专用编辑器面板，先保证加载和诊断链路。
