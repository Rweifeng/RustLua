# M3-05 Camera2D

## 目标

提供可用于 2D 渲染的 Camera2D：支持平移、缩放、旋转、屏幕/世界坐标转换和世界边界约束，并与 Sprite/SpriteBatch 保持同一套坐标约定。

## 实现

- `Camera2D.center`：世界空间相机中心；
- `Camera2D.zoom`：像素/世界单位；
- `Camera2D.rotation`：弧度旋转；
- `CameraBounds`：轴对齐世界边界；
- `with_rotation` 和 `with_bounds` 配置接口；
- `world_to_screen` 与 `screen_to_world` 精确互逆；
- 旋转相机边界约束按旋转后视口包围范围计算；
- 视口大于世界边界时使用稳定的中心点回退；
- 非法 zoom、rotation、center、viewport 和 bounds 会被拒绝。

坐标约定保持不变：世界 X 向右、世界 Y 向上；屏幕左上角为原点、屏幕 Y 向下。

## 验收

自动化：

```text
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace
cargo build --workspace --release
```

Camera2D 测试覆盖：

- 默认相机中心和轴方向；
- 多视口、多缩放坐标往返；
- 旋转后坐标轴方向；
- 旋转相机屏幕/世界坐标往返；
- 世界边界限制；
- 非法输入拒绝。

真实示例：

```text
cargo run -p game_runner --example camera_demo --release
```

通过输出：

```text
camera_demo_ok center=Vec2 { x: 0.0, y: 0.0 } corner=Vec2 { x: -9.730531, y: 5.7720675 } round_trip=Vec2 { x: 1.5000004, y: -2.0000005 } rotation=0.250 zoom=16.0
```

## 范围边界

本任务完成 Camera2D 基础数学和边界约束。相机跟随、平滑阻尼、死区和完整 2D 游戏场景留给后续 2D Demo 任务。