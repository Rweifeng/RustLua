# M15-00 性能基线

## 状态

DONE。

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
- 新增 Game View 预览实机采样脚本：`scripts/m15-00-preview-window.ps1`，通过 `P2_EDITOR_AUTOPLAY=1` 自动进入预览并采集 `[editor-perf] preview fps` 日志
- 新增缩略图真实窗口采样脚本：`scripts/m15-00-thumbnail-window.ps1`，通过 `P2_EDITOR_THUMBNAIL_BENCH` 在编辑器窗口内完成纹理加载、缩放和 GPU 上传采样
- 新增 Play/Stop 真实窗口采样脚本：`scripts/m15-00-playstop-window.ps1`，通过 `P2_EDITOR_PLAYSTOP_BENCH` 在编辑器窗口内循环启动/停止并校验编辑态无漂移
- 新增统一入口：`scripts/m15-00-full-baseline.ps1`，把 headless / real-window / preview-window / thumbnail-window / playstop-window / tilemap-window 采样链路收口到同一份总报告
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
| 10000 entities / 10s | `cycles=6579` |
| query | `avg_us=289` / `p95_us=438` |
| update | `avg_us=929` / `p95_us=1358` |
| destroy | `avg_us=5` / `p95_us=10` |
| command | `avg_us=290` / `p95_us=515` |

### Tilemap

| 指标 | 结果 |
| --- | --- |
| 100x100 / 3 cycles | `avg_ms=1.949` / `total_ms=5.846` / `vertices=60000` / `draws=1` |
| 500x500 / 1 cycle | `avg_ms=34.353` / `total_ms=34.353` / `vertices=1500000` / `draws=1` |
| 500x500 / 3 cycles（补充短测） | `avg_ms=35.098` / `total_ms=105.294` / `vertices=1500000` / `draws=1` |

### Thumbnail / PlayStop

| 指标 | 结果 |
| --- | --- |
| 1000 thumbnails | `total_ms=497.068` / `avg_ms=0.497` / `rgba_bytes=2048000` / `thumb_pixels=512000` |
| 100 Play/Stop | `total_ms=6.241` / `avg_ms=0.062` / `entities=23` |

### Thumbnail / PlayStop Real Window

| 指标 | 结果 |
| --- | --- |
| 1000 thumbnails window | `total_ms=39.053` / `avg_ms=0.039` / `rgba_bytes=2048000` / `thumb_pixels=512000` / `source=assets/textures/slime.png` |
| 100 Play/Stop window | `total_ms=767.058` / `avg_ms=7.671` / `entities=23` |

原始报告：

- `docs/acceptance-evidence/m15/m15-00-thumbnail-window-20260827-121737.json`
- `docs/acceptance-evidence/m15/m15-00-playstop-window-20260827-121752.json`

### Preview Window

| 指标 | 结果 |
| --- | --- |
| editor preview autoplay | `window_ready_ms=887.780` / `autoplay_scene=scenes/production.scene` |
| preview fps | `avg=49.867 fps` / `min=49.6` / `max=50.0` / `samples=3` |

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

### Final Real Window Confirmation

说明：最终确认轮使用 `scripts/m15-00-real-window.ps1` 复跑短窗口与 `60 分钟` 长窗口采样，并将 `release_pack_window.startup.main_window_ms` 固定为发布包启动时间正式口径。

短窗口确认（`15s / 3s interval`）：

| 指标 | 结果 |
| --- | --- |
| editor startup | `main_window_ms=1456.591` |
| editor 15s smoke | `sample_count=6` / `exited_early=false` |
| editor working set | `min=21.086 MiB` / `max=305.887 MiB` / `avg=258.420 MiB` |
| release pack startup | `main_window_ms=204.633` |
| release pack 15s smoke | `sample_count=6` / `exited_early=false` |
| release pack working set | `min=21.684 MiB` / `max=230.676 MiB` / `avg=195.268 MiB` |
| window keepalive | `restore_count=0` / `minimized_count=0` / `invisible_count=0` / `zero_size_count=0` |

正式长窗口确认（`3600s / 5s interval`）：

| 指标 | 结果 |
| --- | --- |
| editor startup | `main_window_ms=834.824` |
| editor 60m | `sample_count=720` / `exited_early=false` |
| editor working set | `min=34.418 MiB` / `max=331.742 MiB` / `avg=309.235 MiB` |
| editor private | `min=46.230 MiB` / `max=412.465 MiB` / `avg=410.531 MiB` |
| editor handles | `min=281` / `max=639` / `avg=616.986` |
| release pack startup | `main_window_ms=864.138` |
| release pack 60m | `sample_count=720` / `exited_early=false` |
| release pack working set | `min=21.875 MiB` / `max=231.047 MiB` / `avg=186.874 MiB` |
| release pack private | `min=7.270 MiB` / `max=287.656 MiB` / `avg=279.963 MiB` |
| release pack handles | `min=197` / `max=438` / `avg=427.118` |
| window keepalive | `restore_count=0` / `minimized_count=0` / `invisible_count=0` / `zero_size_count=0` |

原始报告：

- `docs/acceptance-evidence/m15/m15-00-real-window-20260827-130057.json`
- `docs/acceptance-evidence/m15/m15-00-editor-memory-20260827-130057.csv`
- `docs/acceptance-evidence/m15/m15-00-game-memory-20260827-130057.csv`
- `docs/acceptance-evidence/m15/m15-00-real-window-20260827-130209.json`
- `docs/acceptance-evidence/m15/m15-00-editor-memory-20260827-130209.csv`
- `docs/acceptance-evidence/m15/m15-00-game-memory-20260827-130209.csv`

## 待补项

无。

## 当前结论

`M15-00` 已完成自动化性能基线的最终收敛：`ECS` 查询成本已下降，`Tilemap 500x500` 首帧 CPU 成本从 `42.977 ms` 压到 `34.353 ms`，并且已补上 `Game View preview`、`thumbnail window` 与 `Play/Stop window` 三条真实窗口口径。最终确认轮已完成短窗口和 `60 分钟` 长窗口采样，`editor` 与 `release pack` 均达到 `sample_count=720` 且 `exited_early=false`；发布包启动时间正式采用 `release_pack_window.startup.main_window_ms`。
