# M10-13 质量门与编辑器实机证据

## 状态

PENDING（自动化质量门已补强；真实 GUI/GPU 证据需负责人使用 Windows 实机归档）

## 目标

补齐 CI 与本地质量门的工程/资源完整性检查，并为编辑器真实窗口的 GUI/GPU、DPI、resize、Play/Stop 验收提供可复核记录格式。

## 自动化证据

| 编号 | 检查 | 命令/入口 | 预期产物 |
|---|---|---|---|
| A01 | 工作树空白 | `git diff --check` | 退出码 0 |
| A02 | 游戏项目资源 | `cargo run --manifest-path engine/Cargo.toml -p game_runner -- --project <repo>/game --check` | `game_runner_check_ok` |
| A03 | 编辑器项目结构 | `cargo run --manifest-path engine/Cargo.toml -p editor -- --project <repo> --check` | `editor_project_ok` |
| A04 | 发布包字体 | `scripts/package.ps1 -SelfTest` | `fonts/NotoSansSC-VF.ttf` 存在且非空，隔离目录 `--check` 退出码 0 |
| A05 | 资源缺失失败 | 删除临时副本中的字体/脚本/场景或场景引用资源后运行对应 `--check` | 非零退出码，并包含缺失路径 |

CI workflow 还对 `game/**`、`docs/**`、`tasks/**` 变更触发，并执行同一组双 `--check` 和 `git diff --check`。

## 实机证据归档规范

将原始文件保存到 `artifacts/acceptance/m10-13/<case>/`（截图、录屏、PowerShell 输出、`editor.log`、GPU/驱动信息）。每条记录必须写明日期、commit、Windows 版本、编辑器版本、GPU 型号/驱动、wgpu backend/adapter、窗口尺寸和 DPI。

### G01 GUI/GPU

1. 启动 `editor.exe --project <repo>`，记录启动日志和 GPU adapter/backend。
2. 打开 Scene 和 Game 视图，显示 `ui_test.scene` 的中文 SDF 文本。
3. 保存启动窗口、Scene/Game 视图截图或录屏；确认无 wgpu validation error、中文 glyph 无 tofu/裁切。

**结论字段：** `environment.txt`、`editor-start.png`、`scene-view.png`、`game-view.png`、`editor.log`、`result.md`。

### G02 DPI

在 Windows 显示缩放 100% 和 150%（或项目支持的非 100% 值）分别启动编辑器，记录系统缩放、窗口客户区尺寸和截图。确认四行布局、中文文本、按钮命中区域无重叠或裁切。

### G03 resize

在默认预览分辨率（640x360）和另一个预设/自定义分辨率间切换，并调整主窗口大小；记录工具栏选择、实际预览尺寸、截图和日志。确认纵横比居中、渲染不黑屏、无崩溃，恢复默认后画面正常。

### G04 Play/Stop

打开可运行场景，点击 Play，记录 Game 视图和 `[preview] created` 日志；在运行态进行可观察修改或移动，再点击 Stop。确认状态回到 Scene、运行态修改没有写回编辑态，并重复 Play/Stop 至少 10 轮。归档前后场景文件哈希、操作录屏和 editor log。

## 边界

`--check`、单元测试、offscreen readback 只能证明结构/数据路径，不能替代真实窗口和 GPU 视觉验收。未提供上述原始产物时，G01–G04 必须保持 `PENDING`，不得写成 PASS。
