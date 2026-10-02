# Vendored（打过补丁的）依赖

`[patch.crates-io]`（见 `engine/Cargo.toml`）把这些 crate 指到本目录的 fork。
每个 crate 目录里有 `Cargo.toml.orig`（crates.io 原版清单），升级时可拿它和上游
对照 diff，确认我们的 patch 都重新打上了。

**为什么要 vendored**：`wasm32-unknown-unknown`（微信小游戏 / WebGL 发布目标）上
Lua 与 wasm-bindgen 的错误传播在对应上游版本里没有可用实现，只能自己补。

## 清单

| crate | 版本 | 上游 | patch 内容 |
|---|---|---|---|
| `mlua` | 0.11.6 | <https://github.com/mlua-rs/mlua> | wasm 支持链路（配合下面两个 crate 的 shim 与 EH 约定）；桌面行为不变。 |
| `mlua-sys` | 0.10.0 | 同上（`mlua-sys` crate） | 新增 `wasm-shim-c/`（最小 libc 头/源）与 `src/wasm_shim.rs`；build script 导出 `DEP_WASM_LUA_SHIM_INCLUDE_DIR` 供 lua-src 使用。 |
| `lua-src` | 550.0.0（Lua 5.5.0 源码，多版本目录均在） | <https://github.com/mlua-rs/mlua/tree/main/lua-src> | wasm32 分支：用 `zig cc --target=wasm32-freestanding -fwasm-exceptions -mllvm -wasm-enable-sjlj` 编译 Lua C 源码（Lua 的 setjmp/longjmp 靠 wasm EH 提供），排除 `lua.c`/`luac.c`/`liolib.c`；需要 `ZIG` 环境变量（本机 `.tools/zig/...`，约定见 `.cargo/config.toml` 注释）。 |
| `wasm-bindgen-macro`（+ `macro-support`） | 0.2.129 | <https://github.com/rustwasm/wasm-bindgen> | 宏导出的 `extern "C-unwind"` 改为 `extern "C"`：消除 rustc 为 C-unwind 导出生成的 EH landing pad（`try_table`），它与 Lua 的 legacy EH 混合后 V8 的 legacy catch 失效（宿主函数返回 Err 的 pcall 崩溃）。本工程所有 wasm 目标 `panic=abort`，无行为损失。 |
| `wasm-bindgen-cli` | 0.2.129 | 同上 | **未打 patch**。随仓提供源码是为了离线机器也能构建与运行时同版本的 `wasm-bindgen` CLI（`scripts/wasm-bindgen.ps1` 的第三回退：`vendor/wasm-bindgen-cli/cli/target/release/wasm-bindgen.exe`）。 |

## 逐文件补丁清单（与 `scripts/check-vendor-patches.ps1` 断言一一对应）

升级/覆盖任何 fork 目录后跑
`powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-vendor-patches.ps1`，
**29 条断言全过**才算补丁在位（该门已进 `verify.ps1` 总门，`-MvpOnly` 也跑）。下表是断言的人读版本；
**改补丁必须两边同步**——新增补丁点要在脚本里加断言，脚本断言漂移要在表里改描述。

| crate | 文件 | 改动内容 | 对应断言 |
|---|---|---|---|
| `mlua` | `src/state/util.rs` | wasm32：宿主闭包返回 Err 时**不在 Rust 里 raise**（panic=abort 下 Lua 的 setjmp/longjmp throw 穿不过 Rust 帧），改经 mlua-sys 的 `mlua_trampoline.c` 在 C 帧内调 `lua_error`；同时跳过 `catch_unwind`（避免 landing pad） | 含 `#[cfg(target_arch = "wasm32")]` |
| `mlua` | `src/state/raw.rs` | wasm32：`Err` 不在 Rust 里 raise（throw 穿不过 Rust 帧 → unreachable trap 的规避） | 含 `target_arch = "wasm32"` |
| `mlua` | `src/util/error.rs` | wasm32 跳过 `catch_unwind`（保留它会令 rustc 在 wasm EH 下生成 landing pad，与 Lua C 的 legacy EH 混合） | 含 `target_arch = "wasm32"` |
| `mlua-sys` | `wasm-shim-c/include/` | **新增**：11 个最小 libc 头（assert/ctype/errno/locale/math/setjmp/signal/stdio/stdlib/string/time） | include 下 `.h` ≥ 10 |
| `mlua-sys` | `wasm-shim-c/csrc/` | **新增**：`mlua_trampoline.c` / `sjlj.c` / `stdio.c`（C 半：setjmp/longjmp 靠 wasm EH、stdio、abort/exit） | 三个 `.c` 都在 |
| `mlua-sys` | `src/wasm_shim.rs` | **新增**（≈37 KB）：Rust 半 shim | 存在且 > 10 KB |
| `mlua-sys` | `src/lib.rs` | `#[path = "wasm_shim.rs"] mod wasm_shim;` 挂入 | 含 `#[path = "wasm_shim.rs"]` 与 `mod wasm_shim` |
| `mlua-sys` | `build/main_inner.rs` | `build_wasm_shim()`：`zig cc` 编 wasm-shim-c → `liblua_shim.a`；导出 `DEP_WASM_LUA_SHIM_INCLUDE_DIR` 供 lua-src 用；`rerun-if-changed=wasm-shim-c` | 含 `build_wasm_shim` / `DEP_WASM_LUA_SHIM_INCLUDE_DIR` / `rerun-if-changed=wasm-shim-c` |
| `mlua-sys` | `Cargo.toml` | features（lua55+vendored）与 wasm 注释；`Cargo.toml.orig` 留作升级对照 | `.orig` 存在 |
| `lua-src` | `src/lib.rs` | wasm32-unknown-unknown 分支：`zig cc --target=wasm32-freestanding -fwasm-exceptions -mllvm -wasm-enable-sjlj` 编译 Lua C 源码；排除 `lua.c`/`luac.c`/`liolib.c`（独立工具与 libc 依赖不编入）；`ZIG` 环境变量解析 | 含 `wasm32-unknown-unknown` / `-fwasm-exceptions`+`-wasm-enable-sjlj` / `liolib.c` |
| `lua-src` | `lua-5.5.0/` | 现用 Lua 5.5.0 源码目录（多版本目录均保留） | 目录存在 |
| `wasm-bindgen-macro` | `macro-support/src/codegen.rs` 等 | 宏导出的 `extern "C-unwind"` 全部改为 `extern "C"`（消除 rustc 为 C-unwind 导出生成的 EH landing pad `try_table`——与 Lua legacy EH 混合后 V8 的 legacy catch 失效） | macro/macro-support 全目录 `C-unwind` 出现 **0 次**；codegen.rs 含 `extern "C"` |
| `wasm-bindgen-cli` | `cli/` | **未打 patch**，随仓源码仅供离线构建 CLI | `cli/` 目录存在 |

> 版本耦合（`Cargo.toml.orig` 对照 + 三处同改）与升级验证清单见下两节；其中「vendor 版本 == `engine/Cargo.lock` 版本」已由脚本第 F 组断言（mlua / mlua-sys / lua-src / wasm-bindgen 三处）。

## 版本耦合

- `wasm-bindgen-macro` / `wasm-bindgen-cli` / `crates/game_web` 的 `wasm-bindgen`
  运行时必须是**同一 0.2.x**：`scripts/wasm-bindgen.ps1` 会做版本校验，不一致直接失败。
- `lua-src` 编译出的 Lua 版本由项目选择（`mlua` 的 feature，如 `lua54`）；升级 Lua
  源码目录时保留现用版本目录。

## 升级步骤（每次跟进上游时照此过一遍）

0. **升级前先跑基线**：`powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-vendor-patches.ps1`
   应全绿（29 条），留作"升级前补丁在位"的证据。
1. 从 crates.io / 上游 tag 取新版本，覆盖对应目录（保留我们自己的文件：
   `mlua-sys/wasm-shim-c`、`src/wasm_shim.rs`、`lua-src/src/lib.rs` 的 wasm32 分支、
   `wasm-bindgen-macro` 的 `extern "C"` 导出改动）。
2. 与 `Cargo.toml.orig` 对照 diff，确认清单（依赖、features）没有把 patch 引入的
   条目弄丢；同步三处版本号（macro/support/cli 与 `game_web` 运行时）。
3. 更新 `engine/Cargo.toml` `[patch.crates-io]` 注释里的版本说明。
4. **升级后立刻跑补丁门**：`scripts\check-vendor-patches.ps1` 必须恢复全绿；
   任何 `[FAIL]` 都说明补丁在覆盖中被弄丢/漂移，回到第 1 步补齐，不要带着红门往下走。
5. 验证清单（全部通过才算升级完成）：
   - `cargo test -p engine --lib`（桌面全量）+ `cargo fmt` / `cargo clippy -- -D warnings`
   - `cargo check -p game_web --target wasm32-unknown-unknown`（需要 `ZIG` 环境变量）
   - wasm 桥契约：`artifacts/verify-no-modules-bridge.mjs`
   - webgl / 微信发布门（docs 里的 rustlua-scene-verify 场景切换回归）
