# M3-05：Camera2D

- 状态：PASS
- 完成日期：2026-08-20

## 交付物

- `engine/crates/engine_core/src/math2d.rs`
- `engine/crates/game_runner/examples/camera_demo.rs`
- `docs/10-camera2d.md`

## 验收证据

- Camera2D 专项测试：8 passed；
- Workspace 测试、Clippy、Debug/Release 构建和质量门通过；
- 真实示例输出：
  `camera_demo_ok center=Vec2 { x: 0.0, y: 0.0 } corner=Vec2 { x: -9.730531, y: 5.7720675 } round_trip=Vec2 { x: 1.5000004, y: -2.0000005 } rotation=0.250 zoom=16.0`。

## 结论

M3-05 已完成 Camera2D 平移、缩放、旋转、坐标转换和边界约束，允许进入后续图集或 2D Demo 任务。