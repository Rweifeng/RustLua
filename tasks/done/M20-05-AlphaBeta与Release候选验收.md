# 任务：Alpha Beta 与 Release 候选验收

- 任务 ID：M20-05
- 状态：已完成
- 关联里程碑：M20
- 创建日期：2026-09-09
- 完成日期：2026-09-13

## 目标

建立从 Alpha 到 Beta 再到 Release Candidate 的质量门、内容冻结、缺陷分级、性能基线和发布清单，
确保 `game_jhyt` 可独立验收。

## 影响文件

- `docs/34-质量门与发布清单.md`（新增：门槛定义、缺陷分级、命令矩阵、基线格式、发布清单、P2 清单）
- `docs/36-游戏级回归与性能基线.md`（M19-04 提供证据入口，本任务引用执行）
- `artifacts/baselines/m19-soak.json`（性能基线）
- `artifacts/logs/quality/verify-*.log`（质量门日志留存）
- `tasks/done/*`（各任务的实现记录与验收证据）
- `CHANGELOG.md`（v0.15 收口）

## 实现记录

### 1. 缺陷分级与阻断条件（`docs/34` §1）

| 级别 | 定义 | 阻断 |
|---|---|---|
| P0 | 无法开始/继续游玩、数据损坏、崩溃、发布包不可启动 | 阻断任何阶段 |
| P1 | 核心剧本被卡住、规则错误、存档丢失、关键平台不可用 | 阻断 Alpha/Beta/RC 出门 |
| P2 | 体验缺陷、缺失的非核心内容、已知限制 | 允许记录后放行 |

判定原则：不允许用表现层修补掩盖规则错误；发现 P0/P1 阻断验收且**不得通过删除或放宽测试**降级通过；
不可复现（依赖硬件/浏览器/时间）的结果必须标注「环境受限未执行」并给出复现命令与预期，不得记为通过。

### 2. 三阶段门槛（`docs/34` §2）

- **Alpha（功能完整可玩通）**：M19-01/02/04 交付齐备 + 核心剧本 headless 回归通过 + 三张地图与
  全部玩法链路可达 + UI 面板可用 + 无 P0 + `fmt/clippy/test/build` 全绿。
- **Beta（内容冻结 + 性能基线）**：Alpha 出门 + 内容冻结（只修缺陷）+ 30/60/120 FPS 规则一致 +
  30 天长跑无漂移与 100 次读档循环 + 干净发行目录启动 + 基线落盘 + 无 P0/P1。
- **Release Candidate（回归全绿 + 发布清单）**：Beta 出门 + `verify.ps1` 与 `verify.ps1 -Mvp` 全绿 +
  原子发布回归 + 浏览器平台契约 + 任务文档/CHANGELOG 完整 + 无未解决 P0/P1。

### 3. 命令矩阵（`docs/34` §3）

质量门（`verify.ps1`）、MVP 退出门（`verify.ps1 -Mvp`）、`cargo fmt/clippy/test/build`、
`game --check` / `editor --check`、核心剧本（`jhyt_m19_slice`）、浸泡/帧率/基线（`jhyt_m19_soak`）、
覆盖回归（`jhyt_smoke`/`jhyt_m17*`/`jhyt_m18*`/`jhyt_m19_ui`/`jhyt_m20_audio`/`jhyt_m20_gamepad`）、
原子发布回归（`test-release-atomic.ps1`）、干净目录发布（`package.ps1 -SelfTest`）、
浏览器平台契约（`verify-webgl.ps1`）。

### 4. 性能基线（`docs/34` §4、`docs/36` §4）

`jhyt_m19_soak` 每次运行覆写 `artifacts/baselines/m19-soak.json` 并打印单行汇总；
发布体积由 `package.ps1` 打印（PC 平台约 32.5 MB / 703 files）。

### 5. 内容冻结与发布清单（`docs/34` §5、§5.1）

内容冻结口径：M19-01 收口后不再新增地图/核心系统/第二条主线，只修缺陷。
发布清单含产物布局、隔离目录 `--check`、无 staging/backup 残留、`release.toml` 一致、
第三方许可登记、**P2 已知限制清单**（8 条，逐条给出文档出处）与 CHANGELOG 完整性。

### 6. 阶段执行（`docs/34` §6 记录表）

见本文件「验收证据」；每次执行把 `verify.ps1` 日志留在 `artifacts/logs/quality/`。

## 验收方式

- 执行核心剧本：创建存档、种田、进村、接任务、采集/战斗、交付、制作、睡觉、读档。
- 执行 30 天长跑、100 次读档循环、30/60/120 FPS 一致性和干净目录启动测试。
- 分别完成 Alpha、Beta、Release Candidate 清单，记录阻断缺陷和已知限制。
- 检查任务文件实现记录、验收证据、文档和 CHANGELOG 完整。

## 验收证据

### Alpha 门槛

- [x] 核心剧本 headless 回归：`jhyt_m19_slice` exit 0，11 章全通
      （`jhyt_m19_slice_ok … day=2 orders=3 mainline=true money=465 lingzhi=2 gather_nodes=5`）
- [x] 三张地图 + NPC/种田/任务/商店/采集/战斗/制作/睡觉存档全链路可达（同上一行章节 ①–⑪）
- [x] HUD/背包/对话/任务/商店/暂停可用（`jhyt_m19_ui` exit 0；手柄侧见 M20-08）
- [x] 无 P0；无未关闭 P1（P2 清单见 `docs/34` §5.1）
- [x] 统一数据校验：`cargo run -p game -- --check --project ../examples/game_jhyt` → `game_check_ok`（exit 0）
- [x] 覆盖回归（**M20-08 命名动作迁移落地后**的 12 项实测，全部 exit 0）：`jhyt_smoke`/`jhyt_m17`/
      `jhyt_m17_npc`/`jhyt_m17_dialogue`/`jhyt_m17_shop`/`jhyt_m18_combat`/`jhyt_m18_stats`/
      `jhyt_m18_build`/`jhyt_m19_ui`/`jhyt_m20_audio`/`jhyt_m19_slice`/`jhyt_m19_soak`；
      迁移期集成缺陷已修：`main.lua` 调用的 `input.load_settings()` 在 `scripts/input.lua` 未转出，
      导致全部玩法示例启动即失败 → 补薄包装后 12 项全绿（详见 `tasks/done/M19-04-*.md` 验收证据）
- [x] `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets --all-features -- -D warnings` /
      `cargo test --workspace` / `cargo build --workspace` 实测通过（`verify.ps1` 质量门整体 exit 0）

### Beta 门槛

- [x] 30/60/120 FPS 规则结果一致：`jhyt_m19_soak` 第 1 章输出
      `frame-rate determinism: identical rule state across 30/60/120 fps`
- [x] 30 天长跑无状态漂移 + 读档循环：`day=31 harvested_total=465`、
      `disk save/load cycles: 100`、`real load cycles: 20`、`snapshot restores: 3000`
- [x] 干净发行目录启动：`verify.ps1 -Mvp` 内的 `package.ps1 -SelfTest`（**完整 release 构建**）退出 0
      （`assetc_package_ok files=707`、`Package size: 32,869.3 KB, 714 files`、原子交换 7 项替换、
      隔离目录 `--check` 通过）
- [x] 性能基线落盘：`artifacts/baselines/m19-soak.json`（`per_step_ms=0.4098`、`disk_save_load_cycles=100`）
- [x] 无 P0/P1（P2 逐条记录于 `docs/34` §5.1）

### Release Candidate 门槛

- [x] `scripts/verify.ps1` 全绿：quality gate 段实测 exit 0（cargo metadata / fmt / clippy / workspace tests /
      debug build / `git diff --check` / `game --check`=`game_check_ok` / `editor --check`=`editor_project_ok`），
      日志 `artifacts/logs/quality/verify-20260913-110624.log`
- [x] `scripts/verify.ps1 -Mvp` 全绿：**exit 0**——质量门 + MVP headless 套件（11 个在场示例全部 `*_ok`；
      `jhyt_m20_gamepad` 未落地时按设计 skip）+ 干净目录自测（`Directory-isolation self test passed`），
      结尾打印 `MVP gate passed`
- [x] `scripts/test-release-atomic.ps1` → `release_atomic_ok cases=4`（本轮实测 exit 0）
- [x] 浏览器平台契约：`scripts/verify-webgl.ps1` → `webgl_browser_ok`（Edge 153 无头，`webgl2=true`、
      manifest 707、启动场景与首资源取回、缺失资源 404）；WASM 构建因离线受限**明确标注未执行**
- [x] 任务文档（`tasks/done/*`）实现记录/验收证据/限制完整；CHANGELOG 已更新
- [x] 无未解决 P0/P1（P2 允许带清单发布，见 `docs/34` §5.1）

### 收尾轮统一复跑记录

- `scripts/verify.ps1 -Mvp` → **exit 0**（`Quality gate passed` + `MVP gate passed`），
  日志：`artifacts/logs/quality/verify-20260913-110624.log`（更早两轮分别暴露并修掉了下述两处门禁缺陷）。
- **收尾时修掉的两处门禁缺陷**（都属脚本自身，非玩法代码）：
  1. `scripts/verify.ps1` 把默认工程写死为 `examples`（示例重组后根目录已无 `project.toml`）→
     `Game/Editor project check` 必然失败；改为指向 `examples/game_jhyt`（`$defaultGameProject`），
     `package.ps1` 的缺省 `-ProjectRoot` 同步修正。
  2. `verify.ps1` 用数组 splat 调 `package.ps1`（`@("-ProjectRoot", …, "-SelfTest")`）：PowerShell 的
     数组 splat 按**位置**传参，`-SelfTest` 被当成 `-Platform` 的值而触发 `ValidateSet` 失败
     （`Cannot validate argument on parameter 'Platform'`）；改为新增 `Invoke-PackageSelfTest` 用**具名参数**
     并以子进程执行（`Write-Host` 不经过管道，只有子进程 stdout 能完整写进质量日志）。

## 未完成项 / 已知限制

- 本任务只定义并执行质量门，不替代各玩法、编辑器和平台任务的实现。
- Release Candidate 之前仍允许记录 P2 缺陷，但不得存在未解决的 P0/P1 阻断问题。
- `jhyt_m20_gamepad`（M20-08 的手柄垂直验收示例）**尚未落地**，因此 `verify.ps1 -Mvp` 的示例清单
  按既有 skip-if-missing 设计跳过它；M20-08 收尾后**无需改脚本**即可自动纳入，届时建议再跑一次
  `verify.ps1 -Mvp` 作为最终 RC 复核（本任务的门禁定义与执行记录不受影响）。
- **环境受限未执行（不计通过）**：WASM/浏览器内玩法（离线无法安装 `wasm32-unknown-unknown`）、
  真实手柄实机插拔（无物理设备）。复现命令与预期结果见 `docs/32`/`docs/33`。
