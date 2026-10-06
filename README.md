# RustLua

Rust 写的 2D/3D 游戏引擎 + Lua 玩法脚本运行时，配套一个可视化编辑器、一个资源打包器（assetc）、
一个 Windows 桌面游戏运行器，以及两个示例工程：完整玩法样板《江湖有田》（`examples/game_jhyt`）
与引擎 API 活文档（`examples/game_api`）。

- **引擎**：Rust 负责渲染（wgpu）、ECS、物理（rapier）、输入、音频、字体与资源；玩法规则全部用 Lua。
- **编辑**：`editor` 提供场景/层级/Inspector、Tilemap 与 2D Animation 编辑、Play 预览、资源面板与发布；
  场景里的变换与相机各只有**一个**组件名——`Transform` 与 `Camera`。
- **发布**：`scripts/package.ps1` 产 PC / WebGL / 微信小游戏三种平台产物；`scripts/package-engine.ps1`
  产免安装的编辑器绿色版（含便携模式，不写 `%APPDATA%`）。
- **验证**：`scripts/verify.ps1` 是唯一总门——文档门、cargo 三件套、`game --check`/`editor --check`、
  编辑器性能基线与 GUI smoke（`-Mvp` 追加 26 项 headless 样例与干净目录发布自测）。

## 仓库布局

| 目录 | 内容 |
|---|---|
| `engine/` | Rust workspace：`crates/engine`（运行时）、`editor`、`game`（桌面运行器）、`assetc`（打包/校验）、`game_web`（WebGL 宿主） |
| `examples/game_jhyt` | 完整游戏工程《江湖有田》：横版 1280×720、8 个场景动线（门面/山庄/村庄/山道/洞窟/演武场/两处室内），种田/采集/制作/战斗/任务/商店/7 日目标 |
| `examples/game_api` | 引擎 API 活文档：每个 Lua API 都有一处**可运行**演示，覆盖度由 `host::sample_project` 的 11 条门守 |
| `plugins/` | 平台运行时：`webgl/`（共用 `web-runtime.js` + 契约页）、`wechat-minigame/`（适配层与分包路由） |
| `scripts/` | 质量门与发布脚本：`verify.ps1`（总门）、`check-docs.ps1`（文档门）、`package.ps1`（发布）、`package-engine.ps1`（绿色版）、`test-bundle.ps1`（本地试玩产物）等 |
| `tasks/` | 任务生命周期：`plan/`（WP 工作包与 game_jhyt 执行台账）、`done/`（已归档 134 份） |
| `docs/` | 引擎能力、技术决策、质量门与发布契约（索引见 [docs/README.md](docs/README.md)） |
| `artifacts/` | 生成物：发布包、质量门日志（`artifacts/logs/`），已被 `.gitignore` 忽略；其中 `artifacts/baselines/` 为例外，随仓库提交性能基线快照，供 `verify.ps1 -Benchmarks` 做"这一次 vs 上一次"的对比 |

## 快速开始

```powershell
# 构建
cd engine
cargo build --workspace

# 跑 Demo（编辑器 / 桌面运行器）
cargo run -p editor -- --project ..\examples\game_jhyt
cargo run -p game   -- --project ..\examples\game_jhyt

# 只做数据与工程校验（无需 GPU，适合 CI）
cargo run -p game   -- --project ..\examples\game_jhyt --check
cargo run -p editor -- --project ..\examples\game_jhyt --check
```

## 平台与发布

下表命令省略公共前缀 `powershell -NoProfile -ExecutionPolicy Bypass -File`：

| 平台 | 命令 | 现状 |
|---|---|---|
| PC | `scripts\package.ps1 -ProjectRoot examples/game_jhyt` | **已验收**（M31）：干净目录可运行、`pack/` 只读、存档与设置写用户数据目录 |
| WebGL | 加 `-Platform webgl` | **产物层 + 完整剧本门**：缺失宿主 API 全局 = 0、启动路径不再因缺失全局停下（2026-09-19 无头 Edge 复测）；2026-10-02 起 `verify-webgl-bundle.ps1 -DriveJhyt` 在真实浏览器里跑完整玩法链路（新游戏→移动→背包/任务开合→山庄↔云泽村真实切图→回山庄，M1–M6 里程碑落定）并挂进 `verify.ps1` 总门 |
| 微信小游戏 | 加 `-Platform wechat-minigame`（默认主包 + 单分包） | **已真机验收**（2026-09-27，Android）：启动进山庄、虚拟摇杆移动、按钮交互全通；产物层切分、配额、白名单与 harness 用例（含摇杆链路）全绿 |
| 编辑器绿色版 | `scripts\package-engine.ps1 -Zip` | 免安装包：release 三件套 + 字体 + 样例工程 + 双击即用的 bat |

平台契约、体积账与已知边界分别见 [docs/32](docs/32-webgl-浏览器平台验收.md)、
[docs/52](docs/52-微信小游戏分包发布.md)、[docs/50](docs/50-发布包与用户数据目录.md)。

## 质量门

表内命令省略公共前缀 `powershell -NoProfile -ExecutionPolicy Bypass -File`：

| 目的 | 命令 |
|---|---|
| 总门（含 MVP 回归与发布自测） | `scripts\verify.ps1`（快速路径 `-MvpOnly`；`-SkipBuild -SkipClippy` 复用已有构建） |
| **快速读数：这棵树现在红不红** | `scripts\verify.ps1 -Quick`（文档门 + vendor 门 + fmt + `git diff --check` + `game --check` + `editor --check` 两工程；跳过 clippy/rustdoc/测试/构建/基准/GUI smoke，**跳过的每一步都在结尾逐个打印**——绿灯只读作「数据级与格式门通过」） |
| 提交前门禁（**只对本次提交负责**） | `git-hooks/pre-commit`（装：`scripts\install-git-hooks.ps1`；体检：`scripts\install-git-hooks.ps1 -Status`）。CI 的 lint job 看的是**这个提交**，所以门禁也只看提交内容：`rustlua.scope` 默认 `commit`（提交之外的失败只点名、按 `PARTIAL` 报告），`= worktree` 恢复"整棵树任何一处失败都拦"的旧口径。细则见 [docs/34](docs/34-质量门与发布清单.md) §3.5 |
| 格式 / 静态检查 / 测试 | `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` |
| 文档一致性（编号/索引/链接/历史路径/编码） | `scripts\check-docs.ps1` |
| vendor 补丁基线（wasm 必需补丁逐文件在位 + 版本耦合） | `scripts\check-vendor-patches.ps1` |
| 编辑器整帧 smoke（合成输入的 DPI/resize/拖放/停靠/多选/弹窗） | `cargo run -p editor -- --gui-smoke`（`--gui-report <文件>` 落报告） |
| 干净目录发布自测 | `scripts\package.ps1 -ProjectRoot examples/game_jhyt -SelfTest` |
| WebGL 产物层启动验收 | `scripts\verify-webgl-bundle.ps1` |
| 微信小游戏产物层验收 | `scripts\verify-wechat-minigame.ps1`（默认取 `examples/game_jhyt/builds/wechat-minigame`，`-BundleRoot` 可指定） |
| 本地试玩某个 web 平台产物（自动发现 + 验收 + 起服务） | `scripts\test-bundle.ps1`（`-Platform wechat-minigame` 切平台） |

门全绿的含义与逐项通过标准见 [docs/34-质量门与发布清单.md](docs/34-质量门与发布清单.md)；
`verify.ps1` 会依次跑文档门、cargo 门、`game --check`/`editor --check`、编辑器性能基线与 GUI smoke、
26 项 headless 验收样例和发布隔离自测。

## 文档入口

- 项目章程与协作规则：[docs/00-project-charter.md](docs/00-project-charter.md)（含路径换算：历史文档里的旧检出路径等价于当前 `E:\Codex\RustLua`）
- 全部文档索引与编号约定：[docs/README.md](docs/README.md)
- Lua API 参考：[docs/18-lua-api.md](docs/18-lua-api.md)；编辑器指南：[docs/25-editor-guide.md](docs/25-editor-guide.md)
- 发布包与用户数据目录：[docs/50-发布包与用户数据目录.md](docs/50-发布包与用户数据目录.md)
- 游戏工程总览：[examples/game_jhyt/docs/29-江湖有田-项目执行总览.md](examples/game_jhyt/docs/29-江湖有田-项目执行总览.md)
- 引擎改进台账：[docs/37-引擎与游戏工程复核及优化完善计划.md](docs/37-引擎与游戏工程复核及优化完善计划.md) 与
  [tasks/done/RustLua-game_jhyt-执行计划.md](tasks/done/RustLua-game_jhyt-执行计划.md)；
  引擎全面重构见 [docs/40](docs/40-引擎全面重构计划.md) 与 [docs/54](docs/54-2D3D统一优化方案与现状对比.md)

## 当前状态

> 本节最近一次核对：**2026-10-06**（当时 HEAD `4e5dd12`）。**这一节的规矩**：只写能一条命令复核的事实，或直接
> 指向权威来源；**不复述会变动的读数**。本节此前停在「M32」而项目已到 M6x——根因就是复述了会变的数字
> （里程碑号、测试条数），复述一次就过期一次。里程碑与在途工作以 `tasks/plan/` + `git log` 为准，
> 门禁读数以 `artifacts/logs/quality/` 的日志为准。

- **在途工作**：UI 预设化收口，工作包 `tasks/plan/M65-UI预设化收口-工作包.md` 覆盖 M65–M71
  （《江湖有田》运行时 UI 从「Lua 动态层手绘」收口到 `.prefab` 预设，逐屏迁移 + 每屏一份黄金对账门）；
  M59–M65a 已按 `git log` 落地。**这份 README 不追在途进度**——它每天在变，看工作包与日志。
- **PC 发行已验收**（M31）：干净目录运行、`pack/` 只读、存档与设置写 `<用户数据目录>`
  （`%APPDATA%\<应用名>`，`RUSTLUA_USER_DIR` 可覆盖）、日志写 `<用户数据目录>/logs/`；
  `package.ps1 -SelfTest` 断言隔离目录内无玩家数据。窗口尺寸改由工程声明
  （`project.toml` 的 `window_width/height`），不再写死。
- **游戏工程横版回归（「横屏方案A」，2026-09-27）**：`game_jhyt` 从 M32 的 **720×1280** 竖屏
  转回 **1280×720** 横版，8 个场景重生。设计尺寸的来源（`scripts/ui/layout.lua`、场景
  `meta ui_reference_*`、`project.toml`、场景生成器）由 `ui/layout_check.lua` 在启动期、
  `game --check`、`editor --check` 三条路径上守，漂移当场报错；**平台层另有三处必须同改**
  （微信适配层的 `VIEW_WIDTH/VIEW_HEIGHT`、`plugins/wechat-minigame/game.json` 的
  `deviceOrientation`、`game_web` 的 canvas 尺寸），适配层的 `reportViewAgreement` 是唯一能在
  无头环境里发现该错配的自检。相机跟随玩家、取景被场景 `bounds` 锁在地图内；换图后由内置系统
  `scene_camera_sync` 重绑新场景的 bounds（此前场景声明的相机只在启动场景生效）。
- **编辑器重构阶段 0-3 收口**（[docs/55](docs/55-编辑器商业引擎对标重构计划.md) /
  [docs/58](docs/58-编辑器重构阶段0-3收口与验收.md)）：整帧 GUI smoke 自动化（29 用例）已进总门；
  保存路径检测到**外部改动就拒绝写入**（不是提示，是真不写），显式出口是「强制保存」；
  补丁历史原型把 1000 次字段编辑的保留内存从约 398 MB 降到 216 KB（比值 0.054%，且撤销/重做等价）。
- **示例工程 `game_api` 重建**（[docs/59](docs/59-示例工程game_api与API覆盖率门.md)）：
  **42 个模块表 / 283 个 Lua 函数**（这个数由 `scripts/gen-lua-api-docs.js --check` 与覆盖率门自动守，
  不靠人记；1.3 版核对时把 README 里过期的 40/270 改正）/ 21 个运行时组件全部有演示；引擎新增一个
  Lua 函数，示例工程不补演示就会被覆盖率门判红。
- **组件统一**（第 57 轮）：场景/Prefab 里变换与相机各只有**一个**组件名——`Transform`
  （载荷 `position/rotation/scale`）与 `Camera`（判别字段 `projection=orthographic|perspective`）。
  旧名 `Transform3D`/`Camera2D`/`Camera3D` 与自动迁移逻辑都已删除。**外部老工程需要先跑一次
  一次性迁移工具**：`cd engine && cargo run -p editor --example migrate_project -- --write <项目根>`
  （默认 dry-run）。细节与验收证据见 [docs/56](docs/56-编辑器重构阶段0-1基线与验收.md) §3.52。
- **健康度**：**不在这里写数字**（此前写"1328 项通过"时是 M32，早已过期且没人更新）。权威读数是
  最近一次全量 `verify.ps1` 的日志（`artifacts/logs/quality/verify-*.log`，只保留最近 10 份）。
  想现在就复核：`scripts\verify.ps1 -Quick`（6.6 秒，回答"这棵树现在红不红"，并**逐个打印它跳过了什么**）。

## 约定

- 文档与注释用中文；文本文件统一 UTF-8。PowerShell 脚本**必须带 UTF-8 BOM**（Windows PowerShell 5.1 在
  CP936 环境会按 ANSI 读取，无 BOM 的中文会导致解析错误），改动 `.ps1` 后需重新核对首三字节 `239,187,191`。
- 给玩家双击的 `.bat` 反过来：必须 **CRLF + 纯 ASCII + 无 BOM**（cmd.exe 按 CR 找行尾，LF-only 的 bat
  遇到 GBK 注释行会被切碎成待执行命令；见 [docs/50](docs/50-发布包与用户数据目录.md) §8）。
- 不要手工编辑 `examples/**/builds/`、`**/.cache/`、`**/.data/`、`**/.temp/`、`engine/target/` 下的内容：
  都是生成物（已被 `.gitignore` 忽略）。
- 认领任务前先读 [tasks/README.md](tasks/README.md) 与章程 §9 的任务生命周期。
