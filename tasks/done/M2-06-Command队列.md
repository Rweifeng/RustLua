# M2-06 Command 队列

## 状态

- 状态：PASS
- 完成日期：2026-08-20
- Git 提交：64b256f（完成 M2-06 Command 队列）

## 目标

在 ECS 系统迭代期间，将实体生命周期和组件变更延迟到显式 `apply` 阶段执行，避免系统持有迭代数据时直接修改 `World`，并保证命令按 FIFO 顺序、每批只应用一次。

## 实施内容

- 新增 `engine_core::commands::CommandQueue`；
- 支持 `spawn`、`despawn`、`insert`、`remove` 命令；
- 使用 `SpawnToken` 引用同一批次中尚未创建的实体；
- 支持 `spawn -> insert`、`spawn -> despawn` 等同批次命令链；
- 通过 generation 安全的 `EntityId` 防止过期句柄误操作复用槽位；
- `apply` 在执行前取走当前批次，失败后不会重复执行已消费的命令；
- 错误包含命令索引、操作名称和底层 World 错误。

## 主要 API

```rust
CommandQueue::new()
CommandQueue::spawn() -> SpawnToken
CommandQueue::despawn(target)
CommandQueue::insert::<T>(target, component)
CommandQueue::remove::<T>(target)
CommandQueue::len()
CommandQueue::is_empty()
CommandQueue::apply(&mut world)
```

目标可以是已有 `EntityId`，也可以是本批次 `spawn()` 返回的 `SpawnToken`。

## 验收结果

| 验收项 | 结果 | 自动化证据 |
|---|---|---|
| 系统迭代期间销毁请求不立即修改 World | PASS | `commands::tests::despawn_during_iteration_is_deferred` |
| 命令按入队顺序执行 | PASS | `commands::tests::queues_mutations_until_apply_and_preserves_order` |
| 同批次 `spawn -> insert` 可解析临时句柄 | PASS | `commands::tests::spawn_token_can_be_used_by_later_commands_in_same_batch` |
| 同批次 `spawn -> despawn` 安全执行 | PASS | `commands::tests::spawn_then_despawn_is_applied_safely` |
| 过期 EntityId 不会误操作新实体 | PASS | `commands::tests::stale_existing_targets_fail_without_touching_reused_entities` |
| 非法临时句柄返回明确上下文错误 | PASS | `commands::tests::missing_token_reports_context_and_batch_is_consumed` |
| 失败批次不会被重复执行 | PASS | `commands::tests::missing_token_reports_context_and_batch_is_consumed` |
| 组件移除命令可用 | PASS | `commands::tests::invalid_entity_id_is_not_accepted_as_another_entity` |
| engine_core 测试 | PASS | `cargo test -p engine_core --all-features`，53 项通过 |
| Debug 质量门 | PASS | `artifacts/logs/quality/verify-20260820-180924.log` |
| Release 质量门 | PASS | `artifacts/logs/quality/verify-20260820-180929.log` |

## 设计边界

当前实现是单线程、显式批处理模型，不提供跨线程无锁队列、命令撤销、事务回滚或自动并行调度。命令失败时，失败前的命令已经生效；调用方应依据错误上下文决定是否重建 World 或提交补偿命令。

## 后续衔接

- M2-07：在 10000 实体场景下测量创建、查询、更新、销毁和命令应用开销；
- M2 里程碑：结合 Events、Scheduler 和压力测试完成 `v0.2-ecs` 验收。