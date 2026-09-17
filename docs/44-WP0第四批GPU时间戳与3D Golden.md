# WP0 第四批：GPU Timestamp、3D Semantic Golden 与基准比较

## 目标

第四批完成 WP0 的证据收口：GPU timestamp 不再永久为空、3D 回归从“像素非空”提升为语义 golden，并提供可重复基准比较入口。

## GPU timestamp

Renderer 采用可选整帧 timestamp：

- 只有 adapter 同时支持 `TIMESTAMP_QUERY` 与 `TIMESTAMP_QUERY_INSIDE_ENCODERS` 时才申请 feature；
- 使用开始/结束两个 query；
- 每个 timestamp frame 使用独立 resolve buffer 与 MAP_READ buffer；
- 正常渲染路径使用零超时非阻塞轮询；
- benchmark 可调用有限等待收集已完成样本；
- 不支持设备输出 `gpu_frame_us=null` 与 `unsupported`；
- 映射失败输出 `map_error`；
- 支持设备输出 GPU 时间统计，不用 CPU 时间替代。

当前 fallback DX12 设备实测可产生 GPU 时间，例如 100 Mesh smoke 约 9,474 µs；GPU 时间存在异步延迟，因此首个未完成帧允许是 `pending`。

## 3D semantic golden

新增 `engine/crates/engine/examples/wp0_3d_golden.rs`，固定加载 `examples/game_api/scenes/demo3d.scene`，离屏渲染 640×360 并校验：

- drawable 数量与名称：`Ground`、`ShadowTriangle`、`SkeletonWarrior`；
- Camera position / target；
- shadow + main pass 的 draw call 证据；
- instances / triangles 非零；
- 非背景像素数量；
- 中心区域非背景像素；
- 内容 bounding box。

不使用跨 GPU 的精确整图 hash。报告保存为 `wp0-golden-v1`，同时记录 adapter name/backend 作为诊断信息。

## 重复基准比较

新增：

- `scripts/compare-baseline.ps1`
- `scripts/test-compare-baseline.ps1`

比较规则：

- `report_version` 必须一致；
- draw calls / instances / triangles 变化为 error，退出码 2；
- p95 超过 5% 输出 warning；
- p99 超过 10% 输出 warning；
- 性能 warning 不阻断，几何/渲染语义变化阻断。

## 验证

已通过：

- `cargo check -p engine --lib --example wp0_3d_stress`；
- `cargo run -p engine --example wp0_3d_golden`；
- GPU timestamp smoke，fallback DX12 取得非空 GPU 时间；
- `scripts/test-compare-baseline.ps1`，`baseline_compare_tests_ok cases=2`。

## WP0 状态

第四批完成后，WP0 的基准、Renderer 指标、3D 阶梯、CPU 分配、GPU timestamp、semantic golden 和重复比较入口均已具备。正式关闭 WP0 前仍需运行一次完整 `scripts/verify.ps1 -Benchmarks`，并保存当机报告与环境信息。
