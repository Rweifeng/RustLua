# M11 里程碑验收证据（编辑器可靠性收口）

日期：2026-08-24
基线提交：`fc7ba0a`（engine_core 与 engine_host 合并为 engine crate）
范围：按 [30-编辑器可靠性…实施计划](../30-编辑器可靠性-2D工作流-资源系统闭环实施计划.md) 执行
M11-01~M11-07，用户选定"M11 可靠性收口"。

## 总状态

**代码/自动化侧：PASS（质量门 A01–A08 全通过，329 项测试全绿）。**
**实机 GUI/GPU 证据（G01–G04）：已归档（2026-08-24，见 cases/result.md）**——
G01 实机 PASS；G04 Play 与"运行态修改不写回"实机 PASS，Stop 真实点击受无真实指针设备环境
限制（100 轮往返由自动化测试覆盖）；G02/G03 与 Stop 真实点击 PENDING（需负责人实机手动）。

## M11-01~M11-05 变更与自动化证据

### M11-01 PlaySession 状态机（R-01）
- `editor/src/main.rs`：`PlayState` 枚举（`Editing → Starting → Playing / StartFailed → Stopping → Editing`）。
- `editor/src/panels/game_view.rs`：`play()` 仅在预览初始化全部成功后才置 PlaySession 并进入 `Playing`；
  失败进入 `StartFailed` 并保持编辑态；`stop()` 经 `Stopping` 清理后回 `Editing`。
- 测试：`play_failed_without_gpu_never_enters_playing`、`stop_without_play_is_noop`、
  `is_playing_reflects_state_machine`、`play_stop_can_repeat_one_hundred_times_without_edit_state_drift`。

### M11-02 输入快照与帧边界（R-02）
- `editor/src/main.rs`：跨帧持久 `preview_input_state: InputSnapshot`。
- `game_view.rs`：`update_preview` 用 `mem::take` 取快照 → `bridge_preview_input` → `update` →
  `finish_frame()` 清除边沿；失焦/Play-Stop 时 `clear_all()`。
- 测试：`held_key_reuse_does_not_repeat_just_pressed_across_ticks`、`focus_loss_clears_held_state_prevents_stuck_input`。

### M11-03 Offscreen/readback/缓存一致性（R-03/R-04）
- `engine/src/host/renderer.rs`：`ReadbackSlot` 用 `map_result: Arc<AtomicU8>` + `map_error` 捕获映射失败；
  `poll_readback` 区分 NotReady/Ready/Failed；失败不 unmap、清槽可重试。
- `editor/src/panels/scene_view.rs`：缓存 key 记录实际渲染尺寸（交互期半分辨率），缓存仅在回读成功后提交，
  失败清缓存 + 用户可见错误 + 请求重绘。
- 测试：`scene_view_3d_cache_records_actual_render_size`、`failed_readback_mapping_reports_error_and_clears_slot`。

### M11-04 场景解析与运行时提取统一（R-05）
- `engine/src/host/runtime_scene.rs`：新增 `scene_3d_with_camera(aspect, camera_override)`——编辑器
  Scene 3D 视图与运行时共用同一条 ECS drawable/component 提取路径；相机覆盖为视图参数，不改场景数据。
- `editor/src/panels/scene_view.rs`：`build_scene3d` 优先走 `scene_runtime`（`from_scene_edit` 实例化的 ECS）
  + 编辑器相机覆盖 + Gizmo 预览 transform 覆盖；无 ECS 时回退 serialized extractor。
- 测试：`serialized_and_runtime_consistency_demo3d`、`scene_3d_with_camera_override_keeps_scene_data_intact`
  （覆盖前后 drawables 一致、覆盖后 `scene_3d` 仍回退场景相机）。

### M11-05 恢复、字体内存与错误可见性（R-06/R-09）
- recovery（`recovery.rs`）：带版本/校验和/时间戳；`recover()` 仅构造文档不写盘，损坏文件进 `invalid` 可丢弃；
  已有测试覆盖（校验和/场景损坏不碰主文件、版本不匹配拒绝、clear_for 清理）。
- 字体内存（`engine/src/core/font.rs`）：`FontAtlas` 缓存/插槽/像素在 AtlasFull 后停增长，`grow` 保留旧几何。
  新增测试：`glyph_cache_and_pixels_stay_bounded_after_atlas_full`、
  `repeated_load_rasterize_drop_keeps_memory_bounded`（50 轮 load/rasterize/drop 内存有界）。
- 错误可见性（`editor/src/panels/console.rs`）：控制台标题带项目名；`suggest_action` 为脚本诊断提供
  启发式建议动作；GPU/场景/资源错误经 tracing 汇入控制台 `log_sink`，脚本诊断红色显示于同一面板。
  新增测试：`diagnostics_include_phase_path_line_and_suggestion`、`unknown_diagnostic_has_no_suggestion_but_stays_structured`。

## 质量门结果（A01–A08，详见 m11-06-gui-gpu-evidence.md）

| 项 | 结果 |
|---|---|
| git diff --check | 通过 |
| cargo fmt --all -- --check | 通过 |
| cargo clippy --workspace --all-targets --all-features -- -D warnings | 通过（0 警告） |
| cargo test --workspace --all-features | 通过（329 项） |
| cargo build --workspace / --release | 通过 |
| game --project game_test --check | `game_check_ok` |
| editor --project game_test --check | `editor_project_ok` |
| scripts/package.ps1 -SelfTest | 通过（32 资源文件打包，隔离目录含空格路径自测 OK） |

## 实机状态（2026-08-24 归档，详见 cases/result.md）

- G01 GUI/GPU：✅ 实机 PASS（启动、main.scene 加载、中文 SDF 渲染、无 validation error、无 crash）。
- G02 DPI：⏳ PENDING（150% 需负责人系统缩放切换后实测）。
- G03 resize：⏳ PENDING（预览分辨率切换需点击 ComboBox，合成输入受限）。
- G04 Play/Stop：✅ Play 实机 PASS、运行态修改不写回 PASS（场景哈希前后一致）；⏳ Stop 真实点击
  PENDING（无真实指针设备环境限制，100 轮往返由自动化测试覆盖）。
- 环境：`environment.txt`（本机 GT 1030 / 100% DPI / 1920x1080）已记录，150% 需补充。

## 里程碑停止条件核对

- 所有 P0 关闭：✅（R-01/R-02 的 P0 已有测试且质量门全绿；实机无 P0 待 G01–G04 复核）
- 输入、缓存、DPI、resize、恢复稳定：输入/缓存/恢复由自动化测试覆盖 ✅；DPI/resize 待 G02/G03 实机
- 状态与代码一致：代码侧一致 ✅（Play 状态机、提取路径、缓存 key 与实现同步）

## 后续性能修复（2026-08-24，提交 `cbbb09b`）

用户报告 3D 场景运行时帧率极低。定位根因：Game 预览硬节流 `PREVIEW_FRAME_INTERVAL = 33ms`
（约 30fps）+ egui repaint 与 60Hz vsync 对齐 → 实际约 **24fps**；非 GPU 渲染瓶颈
（`run_script_steps` 各阶段持续 0ms、`render_into_texture` CPU 编码约 1ms、readback 拷贝 GPU 完全跟得上）。

修复：`engine/crates/editor/src/preview_session.rs` `PREVIEW_FRAME_INTERVAL` **33ms → 16ms**
（60fps 节流），并清理临时诊断代码（保留轻量 fps 打点作为日志证据）。

实机复测（AUTOPLAY 加载 `game3d.scene` + Play，GT 1030 / 60Hz）：
`[editor-perf] preview fps: 119 uploads / 120 ticks = 50.0 fps` 连续窗口稳定——
3D 场景 Game 预览从约 **24fps 提升到 50fps**（受 60Hz vsync 限制接近上限）。
质量门全绿（fmt / clippy 0 警告 / test 329 项 / build / build --release / git diff --check）。
