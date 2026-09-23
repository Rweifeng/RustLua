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

本任务完成 Camera2D 基础数学和边界约束。

## 相机控制器（WP4 增补；WP5 相机收敛后并入统一相机）

`core/camera/controller.rs` 里的 `Camera2dController` 提供跟随 / 死区 / 平滑 / 抖动；相机数据仍只有
`Camera2D` 一份，bounds 钳制复用相机自己的 `Camera2D::constrained_center`（与剔除用的可见域
同源），因此不会出现两套夹取规则。逐精灵/逐瓦片的投影不再走 `Camera2D::world_to_screen`，
而是走每帧烘焙一次的 `core/camera/bake.rs`（`OrthoTerms` / `CameraView`）。

WP5 收敛后控制器的边界改成"给出**本帧应当渲染的相机**"：

- `resolve_camera(current, follow_target, viewport, dt) -> Camera2D`：把"脚本请求消费 → 跟随 →
  advance → bounds 钳制"的**次序固定在类型内部**（此前宿主自己拼这套顺序，而且宿主与控制器
  各自夹一次 bounds）；
- `resolve_view(...) -> RenderView`：直接给出统一视图描述（`Projection` + 视口 + 清屏 + 顺序）。

- **跟随**：`follow(entity_id)`，相机中心 = 目标世界位置 + `offset`；
- **死区**：目标在 `dead_zone`（像素，半宽/半高）内时相机不动，超出后才推到"目标刚好贴住边界"；
- **平滑**：`smoothing` 是每秒收敛率，用 `1 - e^(-s·dt)` 做**帧率无关**的指数收敛（同样的总时长给同样的结果）；`0` = 立即跟随；
- **抖动**：`shake(amplitude, duration)`，强度随剩余时间线性衰减；相位由累计时间决定、**不用随机数**，因此不破坏规则回归的确定性；
- **单帧步长上限 0.1 s**：卡帧/断点后相机不瞬移。

运行期由 `camera2d_controller` 帧级系统每帧推进（排在 `integrate_velocity` 之后），该系统调用的
就是 `resolve_camera`。

Lua 侧（此前 **Lua 完全无法控制 2D 相机**，`camera` 表只管 3D 的 `Camera3D` 组件）：

| 调用 | 说明 |
| --- | --- |
| `camera2d.set(x, y)` / `camera2d.get()` | 设置/读取相机中心 |
| `camera2d.set_zoom(z)` / `camera2d.get_zoom()` | 设置/读取缩放（`z > 0`） |
| `camera2d.follow(id [, opts])` | 跟随实体；`opts` = `{offset_x, offset_y, dead_zone_x, dead_zone_y, smoothing}` |
| `camera2d.unfollow()` | 取消跟随（此后 `set` 的效果不再被覆盖） |
| `camera2d.shake(amplitude, duration)` | 触发抖动 |

相机数据在宿主而不在 Lua 状态里，所以 `set`/`set_zoom` 是**登记请求、宿主下一帧消费**（与
`camera.set_active` 同一做法）；`get`/`get_zoom` 读宿主每帧回填的**真实值**，不是脚本的自述。

## 渲染器消费的视图描述（WP5）

清屏色与视口不再是渲染器里写死的常量：宿主每帧声明 `FrameViews`（`main` + `overlay`），
渲染器只消费。`RuntimeScene::render_view_2d(w, h)` 给出的 2D overlay 视图与 `vertices(w, h)`
用的是**同一个相机**；编辑器 2D/3D/游戏画面三个视图各自声明 `RenderView::main_2d` /
`main_3d`。不声明（`FrameViews::new()`）即回到收敛前行为——默认清屏色 `DEFAULT_3D_CLEAR`
加整目标视口，像素逐字节相同，这就是回滚档。
