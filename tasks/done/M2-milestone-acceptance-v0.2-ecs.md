# M2 里程碑验收：v0.2-ecs

## 状态

- 状态：PASS
- 验收日期：2026-08-20
- 目标标签：`v0.2-ecs`

## 验收范围

M2 ECS 核心包含：

- M2-01 EntityId 与 generation 句柄；
- M2-02 World 与最小组件存储；
- M2-03 Transform/Velocity 系统；
- M2-04 系统调度器；
- M2-05 事件队列；
- M2-06 Command 队列；
- M2-07 ECS 压力测试。

## 60 秒长跑命令

```text
cargo run -p engine_core --example ecs_stress --release -- --entities 10000 --seconds 60 --output artifacts/logs/m2-07-ecs-stress-60s.json
```

## 60 秒长跑结果

```text
entities=10000
requested_seconds=60
elapsed_ms=60001
cycles=35232
final_entities=10000
entity_range=10000..10000
```

耗时统计：

```text
create:  count=1     avg_us=1251  p95_us=1251 max_us=1251
query:   count=35232 avg_us=438   p95_us=488  max_us=4776
update:  count=35232 avg_us=1060  p95_us=1205 max_us=4434
destroy: count=35232 avg_us=3     p95_us=6    max_us=105
command: count=35232 avg_us=34    p95_us=38   max_us=424
```

完整报告：

```text
artifacts/logs/m2-07-ecs-stress-60s.json
```

## 验收结果

| 检查项 | 结果 | 证据 |
|---|---|---|
| M2-01 至 M2-07 任务记录 | PASS | `tasks/done/M2-01-EntityId.md` 至 `tasks/done/M2-07-ECS压力测试.md` |
| EntityId generation 安全 | PASS | M2-01 单元测试和验收记录 |
| World 组件存储 | PASS | M2-02 单元测试和验收记录 |
| Transform/Velocity 更新 | PASS | M2-03 单元测试和验收记录 |
| 系统调度与错误上下文 | PASS | M2-04 单元测试和验收记录 |
| 事件队列帧边界和单次消费 | PASS | M2-05 单元测试和验收记录 |
| Command 延迟变更和顺序 | PASS | M2-06 单元测试和验收记录 |
| 10000 实体连续运行 60 秒 | PASS | `m2-07-ecs-stress-60s.json`；35232 轮无崩溃 |
| 实体数量稳定 | PASS | `entity_range=10000..10000` |
| 创建、查询、更新、销毁、Command 耗时输出 | PASS | 60 秒长跑控制台输出和 JSON 报告 |
| Workspace 测试 | PASS，53 项通过 | `artifacts/logs/quality/verify-20260820-191742.log`；`artifacts/logs/quality/verify-20260820-191746.log` |
| Clippy | PASS，`-D warnings` | Debug/Release 质量门日志 |
| Debug 质量门 | PASS | `artifacts/logs/quality/verify-20260820-191742.log` |
| Release 质量门 | PASS | `artifacts/logs/quality/verify-20260820-191746.log` |

## 结论

M2 ECS 核心达到放行标准。10000 实体在 Release 模式下完成 60 秒连续压力运行，实体总量保持稳定，查询、更新、销毁和 Command 替换均成功，质量门全部通过。

允许创建并核验 Git 标签：

```text
v0.2-ecs
```

## 已知边界

本次长跑验证的是单进程、单线程 CPU ECS 基线，不覆盖 GPU、Lua 逻辑、跨线程调度、内存分配曲线和不同硬件之间的性能可比性。后续 M3 渲染和发布阶段继续补充对应基线。