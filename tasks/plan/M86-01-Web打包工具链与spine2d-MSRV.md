# 任务：Web/微信发布链的工具链冲突与锁版治理（rustc 1.88 × spine2d MSRV 1.95）

- 任务 ID：M86-01
- 状态：待办（P0：Web/微信 release 打包链当前**编不出来**）
- 关联里程碑：M86
- 创建日期：2026-11-26

## 目标

恢复 WebGL 与微信小游戏的**可发布性**：当前打包 recipe 把 wasm 工具链冻在 rustc **1.88.0**
（为 LLVM18 的纯 `try_table` EH，见 `scripts/build/package/30-build.ps1` 第 56/73–86 行注释），
而 M84 引入的 `spine2d@0.5.0` 声明 `rust-version = 1.95` ⇒ 打包构建直接失败。需要选定一条
**可验证**的路线并落地，同时把"工具链 pin 与依赖 MSRV 的一致性"变成机器门（下次不能再靠人撞）。

## 背景与证据（2026-11-26 实测）

复现（`engine/` 目录，`$env:ZIG` 指向 `engine/.tools/zig/.../zig.exe`）：

```powershell
cargo +1.88.0 build -p game_web --release --target wasm32-unknown-unknown
# error: rustc 1.88.0 is not supported by the following package:
#   spine2d@0.5.0 requires rustc 1.95
#   Either upgrade rustc or select compatible dependency versions with
#   `cargo update <name>@<current-ver> --precise <compatible-ver>`
```

- 打包 recipe 位置：`scripts/build/package/30-build.ps1`（`$wasmToolchain = "+1.88.0"`、
  `rustup target add wasm32-unknown-unknown --toolchain 1.88.0`、`cargo $wasmToolchain build -p game_web ...`）；
- 1.88 pin 的理由（同文件注释）：rustc 1.97（LLVM 19）会混用 legacy/new EH（vendored Lua C 的
  setjmp 生成 `try_table`、Rust `catch_unwind` 生成 legacy `try`），V8 拒绝加载
  （"module uses a mix of legacy and new exception handling instructions"）；
- `spine2d` 引入于 M84（2026-11-24，`c75b7fa8`），当时只跑了 `cargo check --target wasm32`
  与默认工具链的 wasm-lua 证据，**没有跑打包 recipe 的 1.88 构建**；
- CI 为什么没红：`web-evidence` 的 `verify.ps1 -Web` 在无硬件 WebGL2 的环境里按设计提前
  `exit 3 BLOCKED`（在启动打包之前结束）；`verify-wasm-lua.ps1` 用的是**默认工具链**，
  不是 1.88。所以这条断链没有自动化覆盖。

## 影响

- `scripts/build/package.ps1 -Platform webgl` 与 `-Platform wechat-minigame` 在**当前 HEAD 上
  走不到产物**（打包 fail），与 README/文档里"产物层每天可复核"的宣称直接冲突；
- 微信主包 4 MiB 配额与 WebGL 体积预算的当期读数因此也是**旧值**（最后一次可构建的产物）；
- 若发布走 `.github/workflows/release.yml`（PC/引擎包），暂不受影响；Web/微信目前本来就是手工发布，
  但手工发布同样跑这条 recipe。

## 路线（三选一，按验证成本排序）

| # | 路线 | 动作 | 验证判据 | 风险 |
|---|---|---|---|---|
| A（推荐先试） | **wasm 工具链升到 ≥1.95**（建议 1.97.1 与桌面一致） | 改 `30-build.ps1` 的 `$wasmToolchain`；重新验证 WebGL/微信产物的 EH：Node V8 编译 + 无头 Edge 启动 + 微信产物层 `wasm_host_ok` | `verify-webgl-bundle.ps1`/`verify-wechat-minigame.ps1` 产物层绿；`web-runtime.js` 不报 mixed EH；若 V8 拒绝则回退 B/C | 1.97 的 mixed EH 可能仍拒绝（2026-09-29 实测）；需先做最小 probe |
| B | 选 **MSRV ≤1.88 的 spine2d 版本**（或 vendor patch 其 rust-version） | `cargo update -p spine2d --precise <旧版>`；确认骨架/golden 8/8 不变 | `cargo +1.88.0 build ...` 成功；M84-02 的 golden 门仍 8/8；Spine 支持范围表不缩水 | 旧版可能缺 P2 能力/修 bug；patch manifest 是隐性偏离上游 |
| C | **把 spine2d 移出 wasm 构建**（feature gate；Web/微信不含 Spine） | `rustlua-spine` 依赖改 optional / wasm 下不链；Lua `spine` 表在 wasm 面裁剪 | Web/微信产物可构建；`lua_api_surface_2d` 与平台契约门同步；文档写明 Web/微信不支持 Spine | 2D 游戏在 Web/微信失去 Spine 能力（对目标品类可能不可接受） |

**建议**：先做 A 的 30 分钟最小 probe（1.97 产物在 Node V8 / 无头 Edge 里能否 load+run），
能过就选 A；不能过再评估 B；C 只作为最后兜底并需负责人签字（能力面缩水）。

## 影响文件（按路线不同）

- 路线 A：`scripts/build/package/30-build.ps1`、`docs/52`（微信）、`docs/32`（WebGL）、`docs/65`（统一 wasm）
- 路线 B：`engine/Cargo.toml`（`spine2d` 版本）、`engine/Cargo.lock`、`tasks/done/M84-02` 复验记录
- 路线 C：`engine/crates/rustlua-spine/Cargo.toml`、`engine/crates/game_web/Cargo.toml`、
  `rustlua-gameplay` 的平台 registry、`docs/18`/`docs/82` 支持范围表

## 风险

- EH 兼容只能靠**运行**验证（V8/微信真机），不能只看编译通过；
- 改工具链会同时改 wasm 体积与优化行为，需重跑体积归因（docs/83）与包体预算；
- 三条路线都会动发布链，需按 docs/69 的发布回归清单走一次**干净目录打包自测**。

## 验收方式

```powershell
# 0) 复现当前断链（应先红）
cd engine; $env:ZIG=".tools/zig/zig-x86_64-windows-0.16.0/zig.exe"
cargo +1.88.0 build -p game_web --release --target wasm32-unknown-unknown   # 现在 exit != 0

# 1) 所选路线落地后：打包 recipe 能产出
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build\package.ps1 -ProjectRoot examples\game_api -Platform webgl
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify\verify-webgl-bundle.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build\package.ps1 -ProjectRoot examples\game_api -Platform wechat-minigame
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify\verify-wechat-minigame.ps1

# 2) 防止复发：新增机器门（工具链 pin × 依赖 MSRV 一致性）
#    例：解析 engine/Cargo.lock 的 rust-version，与 30-build.ps1 的 $wasmToolchain 比较；
#    不一致时在 check-toolchain-lock.ps1 或独立脚本里判红。
```

判据：两条平台打包 + 产物层验收 exit 0；EH 在 V8/无头 Edge（及微信 harness）里不报 mixed；
新增的 MSRV×工具链一致性门有正反对照（改 pin 必红）。

## 实现记录

待回填：所选路线、probe 读数（Node V8/无头 Edge/微信 harness）、打包产物字节、
新增一致性门的正反读数。

## 验收证据

- [ ] 复现命令在修复前 exit != 0（已在本文件 §背景 记录）
- [ ] 所选路线落地后，webgl 与 wechat-minigame 两条打包链路 exit 0
- [ ] `verify-webgl-bundle.ps1` / `verify-wechat-minigame.ps1` 产物层绿（或按环境显式 BLOCKED 并说明）
- [ ] 工具链 pin × 依赖 MSRV 一致性门落地并有反面对照
- [ ] docs/32、docs/52、docs/65、docs/83（体积）同步

## 未完成项 / 已知限制

- 微信真机仍需人工/真机证据；本任务只保证"能打包 + 产物层验收"。
- 选 A 时 1.97 的 EH 真机行为（旧版 Android WebView/X5）无法在本机完全覆盖，需真机抽验。
