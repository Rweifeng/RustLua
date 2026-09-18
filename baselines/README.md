# 提交进版本库的性能基线（A/B 的"前一份"）

这个目录放**随仓库提交**的基准快照，作用只有一个：让 `scripts/verify.ps1 -Benchmarks` 里的
"这一次的 3D 报告 vs 上一次的报告"真的跑起来——此前 `compare-baseline.ps1` 只有自测（4 个
阈值用例），**从来没有和真实基线比过**，于是"没有超过门槛的 3D 回退"这句话没有可执行依据。

## 为什么只比结构性指标（`-GeometryOnly`）

`p95_us` / `p99_us` 是**机器相关**的量：跨机器（或同一台机器的不同负载）比时间必然产生假红。
`artifacts/` 又被 gitignore（证据不进版本库），所以仓库里没有"上一次在同一台机器上的时间"。

因此：

| 指标 | 跨机可比 | 进门时是否判红 |
| --- | --- | --- |
| `draw_calls` / `instances` / `triangles` | ✅ 只由场景与代码决定 | ✅ `-GeometryOnly` 判红 |
| `cpu_peak_bytes` | ✅ 分配计数对同一份代码与场景是确定的 | ✅ `-GeometryOnly` 判红 |
| `p95_us` / `p99_us` / `gpu_frame_us` | ❌ 机器相关 | 只在同机 A/B 时由完整模式判红 |

同机 A/B（把上一次的本地报告当 `-Previous`）请用**不带** `-GeometryOnly` 的完整模式：

```powershell
powershell -NoProfile -File scripts\compare-baseline.ps1 `
    -Current artifacts\baselines\wp0-3d-stress.json `
    -Previous artifacts\baselines\wp0-3d-stress.previous.json
```

## 更新流程

有意改动了 3D 路径并确认没有回退后：

1. 跑 `scripts\verify.ps1 -Benchmarks`，确认几何量与 `cpu_peak_bytes` 的变化是你**有意**的；
2. 把 `artifacts/baselines/wp0-3d-stress.json` 复制到这里同名文件；
3. 在提交信息里写明"几何量/内存为什么变"——这份文件是下一次判红的基准。

`baselines/wp0-3d-stress.json` 的 schema（`wp0-v3`）由 `verify.ps1` 与
`compare-baseline.ps1` 共同校验：**报告结构变了就必须同步改这两个脚本**，否则宁可判红。
