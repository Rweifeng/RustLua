# 任务：Web/微信发布链的工具链冲突与锁版治理（rustc 1.88 × spine2d MSRV 1.95）

- 任务 ID：M86-01
- 状态：进行中（2026-11-26：路线 A probe 已否决，改走路线 D 并落地工具链/一致性门；等两条打包链路端到端验收）
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

## 实现记录（2026-11-26）

### 路线 A probe（否决）
- 1.97.1 构建当前 HEAD 的 game_web wasm（打包同款 profile）：WAT 扫描 `try_table=674`、legacy `try`=2、
  `exnref=21` ⇒ **混合 EH**；
- 真实 **Edge 158** headless 用默认参数编译 wasm-bindgen 产物：`module uses a mix of legacy and new
  exception handling instructions`；Node 24 默认参数同样拒绝（`Invalid opcode 0x1f`，
  加 `--experimental-wasm-exnref` 才能编译）。
- 对照：rustc **1.88** 在旧 commit 上构建的产物 `try_table=0, exnref=0, legacy try=583`，Node 默认 PASS。
  ⇒ **路线 A（升 ≥1.95）不可行**；完整读数见 [docs/84](../docs/84-Web打包工具链EH探针与MSRV处置.md)。

### 路线 D（已落地）
- 当前 HEAD（含 spine2d 0.5.0）用 **1.88.0 + --ignore-rust-version** 构建成功（13,353,964 B），
  WAT 扫描 `try_table=0, exnref=0, legacy try=675`，Node 默认参数 **PASS** ⇒ 纯 legacy EH，与发布运行时兼容；
- `engine/Cargo.toml`：`spine2d = "0.5"` → **`"=0.5.0"`**（精确锁，防 0.5.x 漂移让前提失效）；
- `scripts/build/package/30-build.ps1`：wasm 构建加 **`--ignore-rust-version`**（注释指向 docs/84）；
- 工具链 pin 抽到单一事实源 `scripts/build/wasm-toolchain.ps1`，打包脚本与门共用；
- 新增 **`scripts/verify/check-wasm-release-recipe.ps1`**（+ `wasm-eh-check.mjs`）：recipe 构建 +
  V8 默认参数编译断言；缺工具链/target/zig 时 exit 3 BLOCKED；
- CI：`quality.yml` 的 `web-evidence` job 新增「Install Rust 1.88.0」+「Wasm release recipe」两步，
  打包 recipe 从"没人跑"变成 CI 常跑。

### 端到端验收（本机，2026-11-26）
- **WebGL**：`package.ps1 -Platform webgl` **exit 0**，`wasm_host_ok host=WebAssembly platform=webgl
  bytes=8191121 exports=162 globals=2 bridge_exports=11`、`webgl_package_ok files=8`、
  发布包 10,797.6 KB；`verify-webgl-bundle.ps1` 在本机是 **RED 但只红在环境**：`gpu=initializing`、
  `GL_INVALID_ENUM: glTexStorage3D`、`boot=pending`（与 `web-evidence` 的 BLOCKED 同因：本机无头
  WebGL2 不可用）；层面证据 `frame owner ok` / `page identity ok` / `missing_api_paths=0` 均通过，
  wasm 已加载执行。**不据此声称 WebGL 端到端绿**，只说"打包与加载链路恢复"。
- **微信小游戏**：`package.ps1 -Platform wechat-minigame` **exit 0**，
  `wechat_minigame_package_ok files=17 main_kb=905（22.1%）subpackage_kb=10,101（49.3%）total_kb=11,006（53.7%）`；
  `wasm_host_ok ... platform=wechat-minigame bytes=8193071 exports=160 bridge_exports=11`；
  `verify-wechat-minigame.ps1` **exit 0**：`eh_probe=accepted`（harness 认 legacy try/catch + tag 段）、
  核心实例化成功（导出 148 项）、13 组负例全过、L3 API contract ok；渲染为 `env-limited:no-webgl2`（fake host 无 WebGL2）。
- **新门本机实跑**：`wasm_recipe_ok toolchain=1.88.0 bytes=13353964 eh=legacy`（exit 0；status 文件
  `artifacts/logs/quality/wasm-recipe.status`）。

### 待完成
- 新门在 CI 上的首次绿（本次推送后，含 `quality.yml` 新增的两步）；
- 微信真机抽验（本机无法覆盖；harness 只能证明产物层与 EH 探针）。

## 验收证据

- [x] 复现命令在修复前 exit != 0（已在本文件 §背景 记录）
- [x] 路线 A probe 完成并否决（1.97 混合 EH，Edge 158/Node 默认拒绝；docs/84 §2）
- [x] 所选路线落地后，webgl 与 wechat-minigame 两条打包链路 exit 0（本机实测）
- [x] `verify-webgl-bundle.ps1` / `verify-wechat-minigame.ps1`：微信 exit 0；WebGL 在本机按环境显式受限（无头 WebGL2 不可用，成因已记录）
- [x] 工具链 pin × 依赖 MSRV 一致性门落地（check-wasm-release-recipe.ps1 + CI 步骤；1.97 反例见 docs/84 §2.1）
- [x] docs/32、docs/52、docs/65 已交叉引用 docs/84 与单一事实源；docs/83 不受影响（体积归因基于 1.97 对比，口径已注明）

## 未完成项 / 已知限制

- 微信真机仍需人工/真机证据；本任务只保证"能打包 + 产物层验收"。
- 选 A 时 1.97 的 EH 真机行为（旧版 Android WebView/X5）无法在本机完全覆盖，需真机抽验。
