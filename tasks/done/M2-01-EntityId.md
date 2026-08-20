# M2-01 EntityId / generation

## 状态

- 状态：PASS
- 完成日期：2026-08-20
- Git 提交：待提交

## 实施内容

- 新增 `engine_core::entity::EntityId`，由 `index` 和 `generation` 组成。
- 新增 `EntityAllocator`，支持 `spawn`、`despawn`、`is_alive`、`generation_at`、`len` 和 `is_empty`。
- 删除实体后递增 generation，并回收 slot。
- 复用 slot 时生成新句柄，旧句柄无法访问或删除新实体。
- generation 溢出时跳过 0，避免回到未使用状态。
- 提供 `EntityId::from_raw`、`index`、`generation` 和稳定 Display 格式，便于序列化与日志。

## 验收方案与结果

| 项目 | 结果 | 证据 |
|---|---|---|
| 新实体初始 generation | PASS | `new_entities_start_at_generation_one` |
| 删除使旧句柄失效 | PASS | `despawn_invalidates_the_old_handle` |
| slot 复用生成新 generation | PASS | `reused_slot_gets_a_new_generation` |
| 旧句柄不能删除新实体 | PASS | `stale_handles_cannot_despawn_a_reused_entity` |
| 原始句柄字段和 Display | PASS | `raw_handles_are_compared_by_index_and_generation` |
| EntityId 单元测试 | PASS，24 项 engine_core 测试通过 | `cargo test -p engine_core --all-features` |
| Debug 质量门 | PASS | `artifacts/logs/quality/verify-20260820-170812.log` |
| Release 质量门 | PASS | `artifacts/logs/quality/verify-20260820-170836.log` |

## 设计边界

本任务只提供实体句柄和分配生命周期，不实现 World、组件存储、系统调度或事件队列；这些内容分别由 M2-02 及后续任务负责。
