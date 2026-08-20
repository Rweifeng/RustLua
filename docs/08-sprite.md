# M3-03 Sprite

## 目标

实现单个 2D Sprite 的 CPU 几何生成和最小 GPU 绘制链路，为后续 SpriteBatch、图集和 Camera2D 提供稳定基础。

## 已实现

- `engine_core::sprite::Sprite`：尺寸、归一化 pivot、UV 区域和 tint；
- `Sprite::quad`：根据 `Transform`、`Camera2D` 和 `Viewport` 生成两个三角形、6 个顶点；
- 支持位置、非均匀缩放、弧度旋转、pivot 和 tint；
- 使用 M3-01 约定：世界 Y 向上，屏幕左上角为原点且屏幕 Y 向下；
- UV 使用左上到右下顺序，不发生上下翻转；
- 非法尺寸、pivot、UV、颜色和非有限 Transform 输入会被拒绝；
- `game_runner/examples/sprite_demo.rs` 完成 PNG 纹理上传、采样器、WGSL 管线和离屏 Sprite 绘制。

## 验收

### 自动化

```text
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace
cargo build --workspace --release
```

Sprite 单元测试覆盖：

- 默认尺寸、中心 pivot 和全纹理 UV；
- 6 顶点和稳定三角形顺序；
- UV 顺序；
- 缩放和旋转；
- 自定义 pivot；
- tint；
- 非法输入拒绝。

### 真实 GPU

```text
cargo run -p game_runner --example sprite_demo --release
```

通过标志：

```text
sprite_demo_ok adapter="NVIDIA GeForce GT 1030" vertices=6 texture=1x1 target=64x64
```

该 Demo 创建真实 wgpu Device/Queue，上传 PNG，创建纹理采样器和 Sprite pipeline，并将 6 个 Sprite 顶点绘制到离屏目标纹理。

## 范围边界

本任务只实现单 Sprite。相同纹理批处理、排序键、图集打包和 1000 个精灵压力测试留给 M3-04 SpriteBatch 与后续图集任务。