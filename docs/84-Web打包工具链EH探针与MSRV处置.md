# 84 Web/微信 wasm 打包工具链的 EH 探针与 MSRV 处置（M86-01）

- 目的：回答 M86-01 的核心问题——**打包 recipe（rustc 1.88）与 M84 引入的 `spine2d@0.5`
  （manifest 声明 rust-version=1.95）冲突时，能不能把 wasm 工具链升到 ≥1.95？**
- 结论：**不能**。≥1.95 的 rustc 产出的是 **legacy + new 混合 EH** 模块，真实 Edge（158）与 Node
  默认参数都直接拒绝；而 **1.88 在当前代码（含 spine2d 0.5）上仍可编译，产物是纯 legacy EH**。
  因此路线定为 **D：保留 1.88 + 显式 `--ignore-rust-version` + 机器门**（不升工具链、不降 spine2d、不砍能力）。
- 依据来自 2026-11-26 的本地 probe（本文件 §2），可复现命令见 §4。

## 1. 背景（为什么这是个"能不能发包"的问题）

`scripts/build/package/30-build.ps1` 把 wasm 构建冻在 rustc **1.88.0**（LLVM18），理由是 1.97（LLVM19）
会产出 EH 混合模块，V8 拒绝加载；M84 的 `spine2d@0.5.0` 声明 `rust-version = "1.95"` ⇒
打包时 cargo 在编译前就报 `spine2d@0.5.0 requires rustc 1.95`，**WebGL/微信两条发布链整体编不出来**
（详见 [M86-01](../tasks/plan/M86-01-Web打包工具链与spine2d-MSRV.md)）。

## 2. Probe 读数（同一台机器，2026-11-26）

### 2.1 路线 A（升到 1.97.1）——不通过

用打包同款 profile 环境（strip=none / opt=z / fat-LTO / CGU=1）构建当前 HEAD 的 `game_web` wasm：

| 产物 | 指令族统计（WAT 计数） | V8/Edge 默认参数 |
|---|---|---|
| `current.wasm`（rustc 1.97.1） | `try_table=674`、**legacy `try`=2**、`exnref=21` | **拒绝** |
| `current_bg.wasm`（wasm-bindgen 产物，7.64 MB） | 同源混合 | Edge 158 headless 报 **`module uses a mix of legacy and new exception handling instructions`**；Node 24 默认参数报 `Invalid opcode 0x1f (enable with --experimental-wasm-exnref)` |

⇒ 1.97 的模块**同时**含 new EH（`try_table`/`exnref`，来自 vendored Lua C 的 setjmp）与 legacy EH
（`try`/`catch`，来自 Rust `catch_unwind`）。这不是"新 EH 需要 flag"的小问题，而是**混合**：Chrome/Edge
与微信 X5 都不会接受。**路线 A 否。**

### 2.2 对照：rustc 1.88 产物——纯 legacy EH

用同样 profile 环境、在**旧 commit `0c0c6ced`**（无 spine2d）上用 1.88.0 构建：

| 产物 | 指令族统计 | V8 默认参数 |
|---|---|---|
| `baseline-188.wasm`（9,200,576 B） | `try_table=0`、`exnref=0`、legacy `try`=583 | **PASS** |

⇒ 1.88 产物只含 legacy EH，这正是它能进 V8/Edge/微信的原因。

### 2.3 路线 D（保留 1.88，忽略 MSRV 声明）——通过

当前 HEAD（含 `spine2d@0.5.0`）用 1.88.0 + `--ignore-rust-version` 构建：

| 产物 | 读数 | 结论 |
|---|---|---|
| `current-188-if-ignored.wasm`（13,353,964 B） | `try_table=0`、`exnref=0`、legacy `try`=675 | **构建成功 + Node 默认参数 PASS** |

⇒ `spine2d@0.5` 的 manifest 声明（1.95）**比它的实际代码需求保守**；在 1.88 上能编译，产物仍是纯 legacy EH。
`spine2d` 版本与 MSRV：0.1.0/0.2.0 = 1.85；**0.3.0/0.4.0/0.5.0 = 1.95**（crates.io 稀疏索引实测）。

### 2.4 路线 B（降到 0.2.x，MSRV 1.85）——不选

0.1/0.2 的 MSRV 够低，但等于把 M84 之后修 bug/对齐 Spine 4.3 的版本退回去，且要重跑全部
Spine golden 与支持范围表。能力风险 > 收益，作为兜底备选而非首选。

## 3. 裁决与落地

**路线 D**（已落地）：

1. `engine/Cargo.toml` 把 `spine2d` 从 `"0.5"` 收紧到 **`"=0.5.0"`**——允许 0.5.x 漂移会让"1.88 能编"
   这个前提随时失效；
2. `scripts/build/package/30-build.ps1` 的 wasm 构建加 **`--ignore-rust-version`**，并注释指向本节；
3. 工具链 pin 抽到单一事实源 **`scripts/build/wasm-toolchain.ps1`**（`Get-RustLuaWasmToolchain → "+1.88.0"`），
   打包脚本与门读同一份，杜绝两处写死不同版本（M86-01 的根因之一）；
4. 新增 CI 门 **`scripts/verify/check-wasm-release-recipe.ps1`**（`wasm-eh-check.mjs` 做 EH 断言）：
   ①工具链/target/zig 缺失 → exit 3 BLOCKED；②按打包 recipe 用 1.88 构建 `game_web`（`--ignore-rust-version`）；
   ③产物必须在 **Node 默认参数**下编译成功（纯 legacy EH）。已接入 `quality.yml` 的 `web-evidence` job
   （先装 1.88.0 toolchain + wasm32 target，再跑门）。

边界：这条路线依赖"spine2d 0.5.0 的代码确实兼容 1.88"。精确锁版本 + CI 门一起把风险关在
"升级依赖时门会红"上；**不要**在没有重跑本门的情况下放宽 `spine2d` 的精确锁或移除
`--ignore-rust-version`。

## 4. 复现命令

```powershell
# 工具链与 target
rustup toolchain install 1.88.0 --profile minimal
rustup target add wasm32-unknown-unknown --toolchain 1.88.0

# 打包 recipe（与 CI 门同款）
pwsh -NoProfile -File scripts/verify/check-wasm-release-recipe.ps1
# 期望尾行：wasm_recipe_ok toolchain=1.88.0 bytes=<n> eh=legacy

# 反面证据（路线 A 不通过）：用 1.97.1 构建后，Node 默认参数编译失败
#   node -e "new WebAssembly.Module(require('fs').readFileSync('<game_web.wasm>'))"
#   → Invalid opcode 0x1f / mixed legacy and new EH
```

## 5. 关联

- 任务：[M86-01](../tasks/plan/M86-01-Web打包工具链与spine2d-MSRV.md)；
- 包体口径：[83 包体逐依赖体积归因](83-包体逐依赖体积归因.md)（同一次归因顺手发现本阻塞）；
- 微信/WebGL 平台文档：docs/52、docs/32、docs/65（打包工具链口径以本文件为准）。
