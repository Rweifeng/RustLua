# M2-02 World 和最小组件存储

## 状态

- 状态：PASS
- 完成日期：2026-08-20
- Git 提交：待提交

## 实施内容

- 在 `engine_core::world` 新增 `World`，统一管理实体生命周期和组件存储。
- 基于 `EntityAllocator` 创建、销毁实体，并通过 generation 校验拒绝旧句柄和非法句柄。
- 使用 `HashMap<TypeId, Box<dyn ComponentStorage>>` 实现类型擦除的组件存储；每个组件类型内部使用 `HashMap<EntityId, T>`。
- 提供组件插入、读取、修改、删除和存在性查询。
- 提供单组件查询 `entities_with<T>()` 与双组件组合查询 `entities_with2<T, U>()`，结果按 `EntityId` 稳定排序。
- 销毁实体时自动从所有组件存储中清理该实体的组件。

## 主要 API

```rust
World::new()
World::spawn()
World::despawn(entity)
World::is_alive(entity)
World::entity_count()

World::insert(entity, component)
World::get::<T>(entity)
World::get_mut::<T>(entity)
World::remove::<T>(entity)
World::has::<T>(entity)

World::entities_with::<T>()
World::entities_with2::<T, U>()
```

## 验收方案与结果

| 项目 | 结果 | 证据 |
|---|---|---|
| 创建和销毁实体、数量统计 | PASS | `world::tests::creates_and_destroys_entities` |
| 组件插入、读取、修改、删除 | PASS | `world::tests::inserts_reads_modifies_and_removes_components` |
| 销毁实体自动清理全部组件 | PASS | `world::tests::despawn_removes_all_components` |
| 单组件和双组件组合查询 | PASS | `world::tests::queries_component_combinations_in_sorted_entity_order` |
| slot 复用后旧句柄失效 | PASS | `world::tests::stale_handles_are_rejected_after_slot_reuse` |
| 任意非法 EntityId 拒绝访问 | PASS | `world::tests::arbitrary_invalid_handles_are_rejected` |
| `engine_core` 测试 | PASS，30 项测试通过 | `cargo test -p engine_core --all-features`；`artifacts/logs/quality/verify-20260820-171636.log` |
| Debug 质量门 | PASS | `artifacts/logs/quality/verify-20260820-171636.log` |
| Release 质量门 | PASS | `artifacts/logs/quality/verify-20260820-171647.log` |

## 设计边界

本任务实现的是可验证的最小组件存储，不是最终高性能 ECS。当前方案暂不包含 archetype/chunk、稠密数组、组件注册表、高性能批量查询、事件队列或 Command 队列；这些内容留给后续 M2 任务，在性能数据证明需要时再演进存储结构。
