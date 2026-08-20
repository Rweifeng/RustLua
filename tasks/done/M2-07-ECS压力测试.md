# M2-07 ECS 压力测试

## 状态

- 状态：PASS
- 完成日期：2026-08-20
- Git 提交：待提交

## 目标

建立可重复运行的 ECS 压力测试，覆盖 10000 个实体的创建、组件组合查询、Transform/Velocity 更新、批量销毁和 CommandQueue 替换，并持续运行指定时长，输出耗时统计和实体数量稳定性。

## 实施内容

新增示例：

```text
engine/crates/engine_core/examples/ecs_stress.rs
```

默认参数：

```text
entities = 10000
seconds = 60
```

支持参数：

```text
cargo run -p engine_core --example ecs_stress --release -- --entities 10000 --seconds 60 --output artifacts/logs/m2-07-ecs-stress.json
```

每轮执行：

1. 查询同时拥有 `Transform` 和 `Velocity` 的实体；
2. 执行固定步长的 Transform/Velocity 更新；
3. 延迟销毁一批实体；
4. 使用 CommandQueue 创建等量实体并插入组件；
5. 验证实体总量始终保持 10000；
6. 记录创建、查询、更新、销毁和命令应用耗时。

输出指标：

- 样本数量；
- 最小耗时；
- 平均耗时；
- P95 耗时；
- 最大耗时；
- 运行轮数；
- 初始、最终、最小和最大实体数。

## 短跑验收结果

Release 短跑命令：

```text
cargo run -p engine_core --example ecs_stress --release -- --entities 10000 --seconds 2 --output artifacts/logs/m2-07-ecs-stress-smoke.json
```

结果：

```text
cycles=1217
final_entities=10000
entity_range=10000..10000
create: count=1 min_us=1390 avg_us=1390 p95_us=1390 max_us=1390
query: count=1217 min_us=376 avg_us=420 p95_us=456 max_us=709
update: count=1217 min_us=927 avg_us=1023 p95_us=1154 max_us=1988
destroy: count=1217 min_us=3 avg_us=3 p95_us=5 max_us=104
command: count=1217 min_us=29 avg_us=32 p95_us=35 max_us=337
```

报告文件：

```text
artifacts/logs/m2-07-ecs-stress-smoke.json
```

5 秒补充运行结果：`cycles=3068`、`final_entities=10000`、`entity_range=10000..10000`，查询平均 `420us`，更新平均 `1007us`，命令平均 `32us`。`artifacts/logs/m2-07-ecs-stress-5s.json`。\r\n\r\n## 验收结果

| 验收项 | 结果 | 证据 |
|---|---|---|
| 10000 实体可创建 | PASS | 压力测试 Release 短跑，`final_entities=10000` |
| 10000 实体持续更新 | PASS | 1217 轮 Transform/Velocity 更新无崩溃 |
| 组件组合查询可运行 | PASS | 1217 轮查询，查询结果始终为 10000 |
| 批量销毁与重建可运行 | PASS | 每轮销毁/重建 100 个实体，实体数保持稳定 |
| CommandQueue 可承载压力场景 | PASS | 每轮提交 spawn、insert、insert 命令批次 |
| 实体数量无异常增长或下降 | PASS | `entity_range=10000..10000` |
| 创建、查询、更新、销毁耗时输出 | PASS | 控制台统计和 JSON 报告 |
| engine_core 全量测试 | PASS | `cargo test --workspace --all-features`，53 项通过 |
| Debug 质量门 | PASS | `artifacts/logs/quality/verify-20260820-183231.log` |
| Release 质量门 | PASS | `artifacts/logs/quality/verify-20260820-183255.log` |

## 设计边界

当前压力测试是可重复的单进程、单线程 CPU 基线，不等同于完整 60 秒最终验收。默认 60 秒长跑应在稳定机器和独立验收窗口执行；当前已完成 2 秒 10000 实体 Release 短跑、全量测试和 Debug/Release 质量门。

该测试暂不测量 GPU、Lua、内存分配曲线和跨线程调度；这些内容分别放在后续渲染、脚本和发布性能基线任务中。

## 后续衔接

- 执行一次默认 60 秒 Release 长跑，补齐长期稳定性证据；
- 完成 M2 里程碑验收并打标签 `v0.2-ecs`。