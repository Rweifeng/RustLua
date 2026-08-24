# M11-06 实机 GUI/GPU 质量门验收清单

状态：**已完成归档（2026-08-24）**。自动化证据 A01–A08 全通过；实机 G01 通过、G04 部分通过
（Play 与运行态不写回 PASS；Stop 真实点击受无输入设备环境限制，100 轮往返由自动化覆盖），
G02/G03 与 G04-Stop 真实点击 PENDING（需负责人实机手动，见 cases/result.md）。

## 目标

对照 [30-编辑器可靠性…实施计划](../30-编辑器可靠性-2D工作流-资源系统闭环实施计划.md) M11-06 与
m10-13 的 G01–G04 规范，为编辑器真实窗口的 GUI/GPU、DPI、resize、Play/Stop 提供可复核记录。
本里程碑的代码侧可靠性（Play 状态机、输入快照、readback 缓存、场景提取统一、恢复/字体/错误可见性）
已由 M11-01~M11-05 的自动化测试覆盖（见下方"自动化证据"），以下 G01–G04 为视觉/交互实机项。

## 自动化证据（M11-07 质量门执行，退出码全 0 视为通过）

| 编号 | 检查 | 命令/入口 | 预期产物 | 状态 |
|---|---|---|---|---|
| A01 | 工作树空白 | `git diff --check` | 退出码 0 | ✅ 通过（M11-07） |
| A02 | 格式 | `cargo fmt --all -- --check` | 退出码 0 | ✅ 通过（M11-07） |
| A03 | Clippy | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 退出码 0 | ✅ 通过（M11-07） |
| A04 | 全量测试 | `cargo test --workspace --all-features` | 全通过（329 项） | ✅ 通过（M11-07） |
| A05 | 构建 | `cargo build --workspace` 与 `--release` | 退出码 0 | ✅ 通过（M11-07） |
| A06 | 游戏项目检查 | `cargo run --manifest-path engine/Cargo.toml -p game -- --project <repo>/game_test --check` | `game_check_ok` | ✅ 通过（M11-07） |
| A07 | 编辑器项目检查 | `cargo run --manifest-path engine/Cargo.toml -p editor -- --project <repo>/game_test --check` | `editor_project_ok` | ✅ 通过（M11-07） |
| A08 | 发布包自测 | `scripts/package.ps1 -SelfTest` | 隔离目录 `--check` 退出码 0 | ✅ 通过（M11-07，32 资源文件打包） |

## 自动化单元/集成证据（M11-01~M11-05，已通过）

- R-01 Play 状态机：`play_failed_without_gpu_never_enters_playing`、`stop_without_play_is_noop`、
  `is_playing_reflects_state_machine`、`play_stop_can_repeat_one_hundred_times`（document.rs）。
- R-02 输入帧边界：`held_key_reuse_does_not_repeat_just_pressed_across_ticks`、`focus_loss_clears_held_state_prevents_stuck_input`。
- R-03/R-04 缓存/readback：`scene_view_3d_cache_records_actual_render_size`、
  `failed_readback_mapping_reports_error_and_clears_slot`（renderer.rs）。
- R-05 场景提取统一：`serialized_and_runtime_consistency_demo3d`、`scene_3d_with_camera_override_keeps_scene_data_intact`。
- R-06/R-09：`glyph_cache_and_pixels_stay_bounded_after_atlas_full`、`repeated_load_rasterize_drop_keeps_memory_bounded`（font.rs）、
  recovery 校验和/版本/损坏丢弃（recovery.rs）、`diagnostics_include_phase_path_line_and_suggestion`（console.rs）。

## 实机证据（G01–G04，已归档 2026-08-24）

> 原则（m10-13 边界）：`--check`、单元测试、offscreen readback 只能证明结构/数据路径，不能替代真实窗口和 GPU 视觉验收。
> 原始产物与完整结论见 `cases/result.md`（含环境限制说明与手动验收指引）。
> 本机环境：GT 1030 / 驱动 32.0.15.8228 / Windows 11 26200 / 1920x1080 / DPI 96（100%）——见 `environment.txt`。

### G01 GUI/GPU（PASS，实机）
1. `P2_EDITOR_AUTOPLAY=1 editor.exe --project <repo>/game_test` 启动成功，wgpu 选用 GT 1030
   （日志 backend: Vulkan 582.28 / Dx12 32.0.15.8228）。
2. AUTOPLAY 自动打开 `main.scene`（`entities=8`）；editor.log 显示 `[render] scene ui draws=14 sdf=5`
   与 `font atlas ready .../NotoSansSC-VF.ttf`（中文 SDF 文本已渲染）。
3. 全程无 wgpu validation / error / panic；多次启动与关闭均优雅退出（exit 0），无 crash.log。

**产物：** `cases/g01-editor-start.png`、`cases/g04-autoplay.png`、`editor.log`、`cases/result.md`。
**边界：** 中文 glyph 无 tofu/裁切最终目视确认需负责人复核（脚本侧以字体加载 + SDF 计数佐证）。

### G02 DPI（PENDING，需负责人手动）
100% 为默认（本机实测环境）。150% 需系统设置切换后重测并更新 `environment.txt`；脚本不修改用户缩放。

### G03 resize（PENDING，需负责人手动）
预览分辨率切换依赖点击工具栏 ComboBox，本环境合成输入无法触发（见 G04 限制）；主窗口缩放无黑屏/崩溃。
操作指引见 `cases/result.md`。

### G04 Play/Stop（部分 PASS）
- **Play 实机 PASS**：AUTOPLAY `[preview] created 640x360` → `main entities=8` → game3d Lua
  `enemies=2 pickups=3`、`audio hit`、玩家 `hp 100→0`（运行态可观察修改）。
- **运行态修改不写回 PASS**：Play 前后 `main.scene`/`game3d.scene` SHA256 完全一致
  （`f1548f7b…` / `d4f501f9…`）。
- **Stop 真实点击受限**：本环境无真实指针设备，`SendInput`/`mouse_event`/`PostMessage`
  （鼠标 down/up 与键盘）均无法到达 egui/winit（仅鼠标 move/hover 有效）；eframe 未启用
  accesskit，UI Automation 不可用。100 轮 Play/Stop 往返已由自动化测试
  `play_stop_can_repeat_one_hundred_times` 覆盖。PID 27424 实例曾连续三次 `[preview] created`
  （Stop→AUTOPLAY 自动重 Play 循环迹象）。

**产物：** `cases/g04-autoplay.png`、`cases/g04-located.png`、`cases/result.md`（哈希、时间线、限制、指引）。

## 里程碑停止条件核对（M11-07 结论）

- 所有 P0 关闭：✅（R-01/R-02 等 P0 均有自动化测试且质量门全绿；实机 G01/G04 无 P0 阻塞）
- 输入、缓存、DPI、resize、恢复稳定：输入/缓存/恢复由自动化覆盖 ✅；DPI/resize 实机项
  PENDING（需负责人手动，见 cases/result.md）
- 状态与代码一致：✅（Play 状态机、场景提取路径、缓存 key 与实现同步；实机 Play/运行态不写回已验证）
