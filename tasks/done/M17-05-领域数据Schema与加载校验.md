# 任务：领域数据 Schema 与加载校验

- 任务 ID：M17-05
- 状态：已完成
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

- 为地图、地点、NPC、日程、任务、对话和商店数据增加 `version = 1`，任务/对话注册表改为 `{ version, ids }`。
- 在 Lua 加载模块中增加统一 `validate_all`，覆盖默认字段、枚举、范围、对象 ID 唯一性和 NPC/地点/地图/物品/任务/对话引用完整性。
- `main.lua` 在进入玩法前聚合所有校验错误并通过 `error` 终止启动，诊断包含数据文件、对象 ID 或数组路径、字段路径。
- 保持领域数据为 Lua 纯数据，新增内容不需要修改 Rust 核心；未增加专用编辑器面板。
- 增加 headless 验收中的默认值、枚举、范围和引用错误检查。

## 验收证据

- [x] `cargo fmt --all -- --check` 通过
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过（现有 `runtime_scene.rs` 的 7 个 Clippy 错误，与本任务改动无关）
- [x] 相关测试通过：`jhyt_m17`、`jhyt_m17_dialogue`、`jhyt_m17_shop`
- [x] Debug 构建通过：`cargo build --workspace`
- [x] Demo / 手工步骤可复现：三个 headless acceptance 示例均通过
- [ ] 文档与 CHANGELOG 已更新（未改变公共 Rust/资源格式，无需新增迁移文档）

## 未完成项 / 已知限制

- 本任务不要求立即完成专用编辑器面板，先保证加载和诊断链路。
