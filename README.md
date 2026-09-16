# RustLua

Rust 写的 2D/3D 游戏引擎 + Lua 玩法脚本运行时，配套一个可视化编辑器、一个资源打包器（assetc）、
一个 Windows 桌面游戏运行器，以及一个完整的样板游戏工程《江湖有田》（`examples/game_jhyt`）。

- **引擎**：Rust 负责渲染（wgpu）、ECS、物理（rapier）、输入、音频、字体与资源；玩法规则全部用 Lua。
- **编辑**：`editor` 提供场景/层级/Inspector、Tilemap 与 2D Animation 编辑、Play 预览、资源面板与发布。
- **发布**：`scripts/package.ps1` 生成自包含发布包（PC 干净发布 + WebGL 产物），玩家数据写用户目录。

## 仓库布局

| 目录 | 内容 |
|---|---|
| `engine/` | Rust workspace：`crates/engine`（运行时）、`editor`、`game`（桌面运行器）、`assetc`（打包/校验）、`game_web`（WebGL 宿主） |
| `examples/` | 样板工程；`examples/game_jhyt` 是完整游戏项目（三地图动线、种田/采集/制作/战斗/任务/商店/7 日目标） |
| `scripts/` | 质量门与发布脚本：`verify.ps1`（总门）、`check-docs.ps1`（文档门）、`package.ps1`（发布）、`run-demo.ps1` 等 |
| `tasks/` | 任务生命周期：`plan/`（待做，当前只剩 game_jhyt 执行台账）、`done/`（已归档 133 份，M0–M20 与 M22 输入层专题全部归档） |
| `docs/` | 引擎能力、技术决策、质量门与发布契约（索引见 [docs/README.md](docs/README.md)） |
| `third_party/` | 未接入或已下架的第三方素材（许可证见 [docs/THIRD_PARTY.md](docs/THIRD_PARTY.md)） |

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

## 质量门

| 目的 | 命令 |
|---|---|
| 总门（含 MVP 回归与发布自测） | `pwsh -File scripts/verify.ps1`（快速路径 `-MvpOnly`） |
| 格式 / 静态检查 / 测试 | `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` |
| 文档一致性（编号/索引/链接/历史路径/编码） | `pwsh -File scripts/check-docs.ps1` |
| 干净目录发布自测 | `pwsh -File scripts/package.ps1 -ProjectRoot examples/game_jhyt -SelfTest` |
| WebGL 产物层启动验收 | `pwsh -File scripts/verify-webgl-bundle.ps1` |
| 本地试玩某个 web 平台产物（自动发现 + 验收 + 起服务） | `pwsh -File scripts/test-bundle.ps1`（`-Platform wechat-minigame` 切平台） |

门全绿的含义与逐项通过标准见 [docs/34-质量门与发布清单.md](docs/34-质量门与发布清单.md)；
`verify.ps1` 会依次跑文档门、cargo 门、`game --check`/`editor --check`、24 项 headless 验收样例与发布隔离自测。

## 文档入口

- 项目章程与协作规则：[docs/00-project-charter.md](docs/00-project-charter.md)（含路径换算：历史文档里的旧检出路径等价于当前 `E:\Codex\RustLua`）
- 全部文档索引与编号约定：[docs/README.md](docs/README.md)
- Lua API 参考：[docs/18-lua-api.md](docs/18-lua-api.md)；编辑器指南：[docs/25-editor-guide.md](docs/25-editor-guide.md)
- 发布包与用户数据目录：[docs/50-发布包与用户数据目录.md](docs/50-发布包与用户数据目录.md)
- 游戏工程总览：[examples/game_jhyt/docs/29-江湖有田-项目执行总览.md](examples/game_jhyt/docs/29-江湖有田-项目执行总览.md)
- 引擎改进台账：[docs/37-引擎与游戏工程复核及优化完善计划.md](docs/37-引擎与游戏工程复核及优化完善计划.md) 与 [tasks/plan/RustLua-game_jhyt-执行计划.md](tasks/plan/RustLua-game_jhyt-执行计划.md)

## 当前状态（v0.22 / M31）

- **PC 发行已验收**：干净目录运行、`pack/` 只读、存档与设置写 `<用户数据目录>`（`%APPDATA%\<应用名>`，
  `RUSTLUA_USER_DIR` 可覆盖）、日志写 `<用户数据目录>/logs/`；`package.ps1 -SelfTest` 断言隔离目录内无玩家数据。
- **WebGL 仅到产物层**：wasm 能加载、页面能进入 Lua，但运行时缺 17 个宿主 API 全局，**浏览器内可玩尚未实现**
  （见 [docs/32](docs/32-webgl-浏览器平台验收.md) §4 与 backlog 4-24）；降级为"实验性"，不作平台承诺。
- **健康度**：`cargo test --workspace` 834 项通过（0 失败）、clippy 零告警、24 项 headless MVP 样例全绿。

## 约定

- 文档与注释用中文；文本文件统一 UTF-8。PowerShell 脚本**必须带 UTF-8 BOM**（Windows PowerShell 5.1 在
  CP936 环境会按 ANSI 读取，无 BOM 的中文会导致解析错误），改动 `.ps1` 后需重新核对首三字节 `239,187,191`。
- 不要手工编辑 `examples/**/builds/`、`**/.cache/`、`**/.data/`、`engine/target/` 下的内容：都是生成物（已被 `.gitignore` 忽略）。
- 认领任务前先读 [tasks/README.md](tasks/README.md) 与章程 §9 的任务生命周期。
