# 任务：统一运行时 Scheduler 接入

- 任务 ID：M18-03
- 状态：待办
- 关联里程碑：M18
- 创建日期：2026-09-09

## 目标
将现有 RuntimeScene 手工固定阶段逐步迁移到已存在的 Scheduler，使正式运行时、编辑器预览和 headless 执行共享一致的系统顺序、依赖检查和性能统计。

## 影响文件

- `engine/crates/engine/src/core/scheduler.rs`
- `engine/crates/engine/src/host/runtime_scene.rs`
- `engine/crates/game/src/game_app.rs`
- `engine/crates/editor/src/preview_session.rs`
- 相关调度测试

## 风险

- 这是公共运行时架构变更，必须保持现有脚本、物理、时钟、动画和场景切换顺序。
- 不一次性重写所有系统；保留明确兼容路径并逐阶段替换。
- 性能统计和错误传播不能因调度器接入丢失。

## 验收方式

- RuntimeScene、正式 game 和 editor Preview 使用同一调度入口。
- 现有 RuntimeScene、游戏和编辑器测试全部通过。
- 调度依赖、循环依赖和 panic 错误有可定位诊断。
- 新增交互/NPC/战斗系统可通过注册阶段接入，而无需继续扩大手工编排函数。

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

- 本任务不改 ECS 存储结构，不引入 archetype ECS。
