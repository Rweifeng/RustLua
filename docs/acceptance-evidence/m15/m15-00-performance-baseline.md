# M15-00 性能基线

## 状态

IN PROGRESS。

## 目标

- 记录 `M15` 的首轮性能基线；
- 为后续稳定性、发布和回归比较提供统一口径。

## 预期采样项

- 1000 / 10000 Sprite
- 100x100 / 500x500 Tilemap
- 1000 资源导入
- 1000 缩略图
- 100 次 Play/Stop
- 60 分钟编辑器 / Game View 内存
- 发布包启动时间
- 包体积

## 本轮自动化结果

日期：2026-08-26

### 采样方式

- 新增脚本：`scripts/m15-00-baseline.ps1`
- 新增真实窗口采样脚本：`scripts/m15-00-real-window.ps1`
- `scripts/m15-00-real-window.ps1` 已修复 CSV 导出格式，采样文件按列输出内存与句柄曲线
- `scripts/m15-00-real-window.ps1` 已补窗口保活：检测最小化 / 不可见 / 零尺寸时自动 `restore`
- 新增 Tilemap 真窗口入口：`scripts/m15-00-tilemap-window.ps1`
- `scripts/m15-00-tilemap-window.ps1` 已支持 `-Width / -Height`，按尺寸生成最小 Tilemap 项目并复用真实窗口采样链路
- 新增可参数化 CPU 基准：`engine/crates/engine/examples/sprite_stress.rs`
- 新增 Tilemap / thumbnail / PlayStop 自动采样：
  - `engine/crates/engine/examples/tilemap_stress.rs`
  - `engine/crates/editor/examples/thumbnail_stress.rs`
  - `engine/crates/editor/examples/play_stop_stress.rs`
- 本轮采用 `Release` 构建热缓存结果作为首轮自动化基线；
- 原始报告：
  - `docs/acceptance-evidence/m15/m15-00-baseline-20260826-150124.json`
  - `docs/acceptance-evidence/m15/m15-00-ecs-stress-20260826-150124.json`

### Sprite

| 指标 | 结果 |
| --- | --- |
| 1000 Sprite | `push_ms=0.020` / `build_ms=0.091` / `vertices=6000` / `draw_batches=8` |
| 10000 Sprite | `push_ms=0.201` / `build_ms=0.862` / `vertices=60000` / `draw_batches=80` |

### ECS

| 指标 | 结果 |
| --- | --- |
| 10000 entities / 10s | `cycles=5960` |
| query | `avg_us=441` / `p95_us=490` |
| update | `avg_us=1029` / `p95_us=1161` |
| destroy | `avg_us=4` / `p95_us=6` |
| command | `avg_us=33` / `p95_us=38` |

### Tilemap

| 指标 | 结果 |
| --- | --- |
| 100x100 / 3 cycles | `avg_ms=1.454` / `total_ms=4.363` / `vertices=60000` / `draws=1` |
| 500x500 / 1 cycle | `avg_ms=42.977` / `total_ms=42.977` / `vertices=1500000` / `draws=1` |

### Thumbnail / PlayStop

| 指标 | 结果 |
| --- | --- |
| 1000 thumbnails | `total_ms=458.736` / `avg_ms=0.459` / `rgba_bytes=2048000` / `thumb_pixels=512000` |
| 100 Play/Stop | `total_ms=6.668` / `avg_ms=0.067` / `entities=23` |

### CLI / Pack

| 指标 | 结果 |
| --- | --- |
| `assetc import` | `794.083 ms` |
| `assetc --check-deps` | `479.346 ms` |
| `editor --check` | `2204.563 ms` |
| `game --check` | `923.632 ms` |
| `assetc package` | `522.331 ms` |
| `game --pack --check` | `543.934 ms` |
| pack size | `28 files / 411596 bytes / 0.393 MiB` |

### Real Window Smoke

说明：先用 `scripts/m15-00-real-window.ps1 -DurationSeconds 10 -SampleIntervalSeconds 2` 做真实窗口烟雾验证，确认发布包窗口启动时间与内存曲线采样链路可用。

| 指标 | 结果 |
| --- | --- |
| editor startup | `main_window_ms=1160.079` |
| editor working set | `min=21.156 MiB` / `max=308.344 MiB` / `avg=258.094 MiB` |
| release pack startup | `main_window_ms=201.665` |
| release pack working set | `min=22.320 MiB` / `max=236.027 MiB` / `avg=199.958 MiB` |

原始报告：

- `docs/acceptance-evidence/m15/m15-00-real-window-20260826-151222.json`
- `docs/acceptance-evidence/m15/m15-00-editor-memory-20260826-151222.csv`
- `docs/acceptance-evidence/m15/m15-00-game-memory-20260826-151222.csv`

### Real Window 60m

首轮 `60 分钟` 正式采样结果：

| 指标 | 结果 |
| --- | --- |
| editor startup | `main_window_ms=534.416` |
| editor sample_count | `720` |
| editor working set | `min=21.145 MiB` / `max=319.633 MiB` / `avg=272.617 MiB` |
| editor private | `min=7.039 MiB` / `max=453.906 MiB` / `avg=408.704 MiB` |
| editor handles | `min=195` / `max=630` / `avg=614.443` |

原始报告：

- `docs/acceptance-evidence/m15/m15-00-real-window-20260826-151420.json`
- `docs/acceptance-evidence/m15/m15-00-editor-memory-20260826-151420.csv`

备注：

- 同一轮中，发布包测量是在 `editor 60m` 结束后顺序启动，进程在约 `85s` 时正常退出，未形成有效的 `60m` 长时样本；
- 首轮 CSV 仍是旧格式，后续已修复脚本并重新验证；
- 日志同时暴露出引擎残留的默认纹理路径 `assets/red-1x1.png`，已修正为 `assets/textures/red-1x1.png`。

修正后短测复验（重新编译 release 后）：

| 指标 | 结果 |
| --- | --- |
| editor startup | `main_window_ms=851.038` |
| release pack startup | `main_window_ms=208.428` |
| release pack 10s smoke | `sample_count=6` / `exited_early=false` |

修正后报告：

- `docs/acceptance-evidence/m15/m15-00-real-window-20260826-163532.json`
- `docs/acceptance-evidence/m15/m15-00-editor-memory-20260826-163532.csv`
- `docs/acceptance-evidence/m15/m15-00-game-memory-20260826-163532.csv`

修正后的发布包长时采样结果：

| 指标 | 结果 |
| --- | --- |
| release pack startup | `main_window_ms=214.628` |
| release pack sample_count | `596` |
| release pack covered time | `2982.731s` (`49.7m`) |
| release pack working set | `min=21.598 MiB` / `max=230.723 MiB` / `avg=210.777 MiB` |
| release pack private | `min=7.438 MiB` / `max=280.816 MiB` / `avg=279.272 MiB` |
| release pack handles | `min=196` / `max=436` / `avg=425.159` |
| process exit | `exited_early=true` / `exit_code=0` |

原始报告：

- `docs/acceptance-evidence/m15/m15-00-real-window-20260826-163706.json`
- `docs/acceptance-evidence/m15/m15-00-game-memory-20260826-163706.csv`

日志结论：

- 发布包未崩溃，而是在长时采样过程中多次发生 `Paused -> Running` 切换；
- 最后一轮日志落在 `Exiting -> Exited`，说明进程走的是正常退出链路；
- 因为只覆盖到约 `49.7m`，这轮可记为长时基线的部分结果，暂不能视为 `60m` 验收通过。

保活版短测复验：

| 指标 | 结果 |
| --- | --- |
| release pack startup | `main_window_ms=219.615` |
| release pack 15s smoke | `sample_count=6` / `exited_early=false` |
| window keepalive | `restore_count=0` / `minimized_count=0` / `invisible_count=0` / `zero_size_count=0` |

原始报告：

- `docs/acceptance-evidence/m15/m15-00-real-window-20260826-175628.json`
- `docs/acceptance-evidence/m15/m15-00-game-memory-20260826-175628.csv`

保活版 `60 分钟` 正式采样：

| 指标 | 结果 |
| --- | --- |
| release pack startup | `main_window_ms=220.862` |
| release pack sample_count | `720` |
| release pack covered time | `3603.999s` |
| release pack working set | `min=21.648 MiB` / `max=230.719 MiB` / `avg=171.294 MiB` |
| release pack private | `min=7.422 MiB` / `max=280.543 MiB` / `avg=279.035 MiB` |
| release pack handles | `min=196` / `max=447` / `avg=434.075` |
| process exit | `exited_early=false` |
| window keepalive | `restore_count=0` / `minimized_count=0` / `invisible_count=0` / `zero_size_count=0` |

原始报告：

- `docs/acceptance-evidence/m15/m15-00-real-window-20260826-175741.json`
- `docs/acceptance-evidence/m15/m15-00-game-memory-20260826-175741.csv`

### Tilemap Real Window Smoke

说明：通过 `scripts/m15-00-tilemap-window.ps1` 生成临时项目 `artifacts/m15/tilemap_window_project`，复用 `production.scene` 作为 Tilemap-heavy 真窗口场景，先验证真实窗口入口可用。

| 指标 | 结果 |
| --- | --- |
| tilemap-heavy startup | `main_window_ms=210.63` |
| tilemap-heavy 15s smoke | `sample_count=6` / `exited_early=false` |
| tilemap-heavy working set | `min=21.73 MiB` / `max=233.934 MiB` / `avg=197.855 MiB` |
| window keepalive | `restore_count=0` / `minimized_count=0` / `invisible_count=0` / `zero_size_count=0` |
| runtime perf log | `fps=1323.3` / `frame_ms=0.751` |

原始报告：

- `docs/acceptance-evidence/m15/m15-00-real-window-20260826-190919.json`
- `docs/acceptance-evidence/m15/m15-00-game-memory-20260826-190919.csv`

### Tilemap Real Window Tiers

说明：随后将 `scripts/m15-00-tilemap-window.ps1` 扩成按 `Width / Height` 生成最小 Tilemap 项目，分别对 `100x100` 与 `500x500` 做 `15s / 3s interval` 真窗口分档短测，确认不同尺寸的真实窗口口径可稳定复跑。

| 指标 | 结果 |
| --- | --- |
| 100x100 startup | `main_window_ms=213.414` |
| 100x100 15s smoke | `sample_count=6` / `exited_early=false` |
| 100x100 working set | `min=21.695 MiB` / `max=230.883 MiB` / `avg=195.426 MiB` |
| 100x100 window keepalive | `restore_count=0` / `minimized_count=0` / `invisible_count=0` / `zero_size_count=0` |
| 500x500 startup | `main_window_ms=218.128` |
| 500x500 15s smoke | `sample_count=6` / `exited_early=false` |
| 500x500 working set | `min=21.734 MiB` / `max=231.020 MiB` / `avg=195.436 MiB` |
| 500x500 window keepalive | `restore_count=0` / `minimized_count=0` / `invisible_count=0` / `zero_size_count=0` |

原始报告：

- `docs/acceptance-evidence/m15/m15-00-real-window-20260826-191616.json`
- `docs/acceptance-evidence/m15/m15-00-game-memory-20260826-191616.csv`
- `docs/acceptance-evidence/m15/m15-00-real-window-20260826-191656.json`
- `docs/acceptance-evidence/m15/m15-00-game-memory-20260826-191656.csv`

## 待补项

- `1000 缩略图` 的资源浏览器 GPU 上传与 UI 吞吐；
- `100 次 Play/Stop` 的真实窗口采样；
- `发布包启动时间` 的正式口径确认（沿用 `main_window_ms`）。

## 当前结论

`M15-00` 已完成第一轮自动化性能基线，并已拿到 `editor` 的 `60 分钟` 首轮真实窗口数据、发布包保活版 `60 分钟` 正式结果，以及 `Tilemap 100x100 / 500x500` 的真实窗口分档短测。真实窗口链路中的 CSV 导出、默认纹理路径和窗口保活能力已经补齐；当前剩余缺口主要是 `thumbnail / PlayStop` 的真实窗口口径与最终验收归并。
