# M2-03 Transform 和 Velocity 系统

## 状态

- 状态：PASS
- 完成日期：2026-08-20
- Git 提交：待提交

## 实施内容

- 新增 `engine_core::transform::Vec2`，提供二维向量、零向量和基础构造方法。
- 新增 `Transform` 组件，包含 `position`、`rotation` 和 `scale`，默认值为原点、零旋转、单位缩放。
- 新增 `Velocity` 组件，包含线速度 `linear` 和角速度 `angular`。
- 新增 `integrate_velocity(world, delta_seconds)` 系统：查询同时拥有 `Transform` 与 `Velocity` 的实体，并按 `dt` 更新位置和旋转。
- 组件不完整的实体会被安全忽略；已销毁实体不会残留或参与更新。
- 拒绝负数、NaN 和无穷大的时间步长，避免污染 Transform 数据。

## 主要 API

```rust
Vec2::new(x, y)
Vec2::zero()

Transform::identity()
Transform::from_position(position)

Velocity::new(linear, angular)

integrate_velocity(&mut world, delta_seconds)
```

## 验收方案与结果

| 项目 | 结果 | 证据 |
|---|---|---|
| Transform 默认值和 identity | PASS | `transform::tests::identity_transform_has_expected_defaults` |
| 线速度更新位置 | PASS | `transform::tests::integrates_linear_and_angular_velocity` |
| 角速度更新旋转 | PASS | `transform::tests::integrates_linear_and_angular_velocity` |
| 缺少组件时明确跳过 | PASS | `transform::tests::ignores_entities_with_incomplete_components` |
| 非法 delta time 被拒绝 | PASS | `transform::tests::rejects_invalid_delta_time` |
| 销毁实体不会参与更新 | PASS | `transform::tests::despawned_entities_are_not_updated` |
| `engine_core` 测试 | PASS，35 项测试通过 | `cargo test -p engine_core --all-features` |
| Debug 质量门 | PASS | `artifacts/logs/quality/verify-20260820-172853.log` |
| Release 质量门 | PASS | `artifacts/logs/quality/verify-20260820-172857.log` |
| 工作区 Clippy、格式化和构建 | PASS | 两轮质量门日志 |

## 设计边界

本任务实现的是最小确定性 Transform/Velocity 更新系统，不包含层级变换、父子实体、碰撞检测、加速度、阻尼、物理约束、插值渲染或系统调度器。这些能力留给后续 ECS、物理和渲染任务。
