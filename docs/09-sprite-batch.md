# M3-04 SpriteBatch

## 目标

在 M3-03 单 Sprite 几何基础上，提供 CPU 侧 Sprite 收集、稳定排序、相同纹理/图层合批，以及一次动态顶点缓冲绘制多个 Sprite 的最小链路。

## 实现

- `SpriteBatchItem`：纹理 ID、图层、Sprite 和 Transform；
- `SpriteBatch`：提交、清空、容量预分配和构建批次网格；
- 排序键：图层升序、纹理 ID 升序、同键保持提交顺序；
- `SpriteDrawBatch`：记录纹理、图层、顶点起始位置和顶点数量；
- 复用 M3-03 的 UV 区域，因此支持图集子区域；
- 生成连续的 6 顶点 Sprite Quad，并把相同纹理/图层合并为一个 draw range；
- 非法 Transform 和 Sprite 输入返回带原始提交索引的错误；
- `sprite_batch_demo` 使用动态顶点缓冲绘制 1000 个 Sprite。

## 验收

自动化：

```text
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace
cargo build --workspace --release
```

SpriteBatch 单元测试覆盖：

- 相同纹理和图层合批；
- 图层、纹理排序和稳定提交顺序；
- 图集 UV 与 tint 传递；
- 非法 Transform 错误索引；
- 顶点数量与 draw range 连续性。

真实 GPU：

```text
cargo run -p game_runner --example sprite_batch_demo --release
```

通过输出：

```text
sprite_batch_demo_ok adapter="NVIDIA GeForce GT 1030" sprites=1000 vertices=6000 draw_calls=3 texture=1x1 target=256x256
```

## 范围边界

本任务实现 CPU SpriteBatch 和最小 GPU 批量绘制。纹理资源数据库、自动图集打包、实例化优化和更复杂的透明排序留给后续任务。