# M8-08 视锥剔除与实例化证据

日期：2026-08-22  
GPU：NVIDIA GeForce RTX 3060 Laptop GPU  
窗口：1280×720；场景含 8×8=64 个相同 triangle 实例 + 地面 + 蒙皮骷髅战士 + 相机/光源

## 视锥平面与包围球单测（engine_core camera3d.rs）

- `Frustum::planes()`：Gribb–Hartmann 从 view-projection（clip z∈[0,1]）提取 6 个世界空间平面并归一化。
- `Frustum::contains_sphere(center, radius)`：任一面 `signed_distance < -radius` 即剔除，接触边界保守保留。
- 单测：`frustum_sphere_culling_respects_radius`（近/侧/边界/NaN 半径）、`frustum_planes_agree_with_point_classification`（原点在内、相机后方点违反平面）通过。

## 实例化与剔除（game_runner）

- 每个 CachedPrimitive 缓存局部包围球（AABB 球心+半径）；逐实体用 model 变换为世界球（半径乘最大轴缩放）CPU 剔除。
- 静态网格按 (mesh_path, primitive) 分组，材质 uniform 移除 model，实例 model 存入 storage buffer（`instance_models`），`draw_indexed(..., instance_count)` 一次绘制。
- 蒙皮网格每实例单独绘制（独立 joint palette）；`--no-cull` / `--no-instance` 提供回退。
- 每 120 帧输出 `[render] 3D baseline draws/instances cull=.. instancing=..`。

## 基线（真实窗口，同一场景）

| 配置 | draws | instances |
|---|---|---|
| 默认（剔除+实例化） | 12 | 48–55 |
| `--no-cull`（实例化开） | 12 | 76 |
| `--no-instance`（剔除开） | 55 | 55 |
| `--no-cull --no-instance` | 76 | 76 |

- **实例化收益**：76 个实例（65 个相同 triangle + 地面 + 战士）→ 12 次绘制；65 个相同 triangle 合并为 1 次 instanced draw。
- **剔除收益**：76 → 48–55 实例（相机视锥外被剔除，可见结果不变——被剔除的都在视野外）。
- 回退路径与默认路径在 `game_runner --project game --check` 下均可启动（`game_runner_check_ok`）。

## 质量门

- `cargo fmt --all --check` 通过；`cargo clippy --workspace --all-targets --all-features -- -D warnings` 无告警；
- `cargo test --workspace --all-features` 全部通过（exit 0，engine_core 188 项）；
- `cargo build --workspace` / `--release` 通过；`git diff --check` 通过；
- `game_runner --check` / `assetc --check` 通过。
