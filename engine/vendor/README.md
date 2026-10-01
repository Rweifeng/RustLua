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

## 版本耦合

- `wasm-bindgen-macro` / `wasm-bindgen-cli` / `crates/game_web` 的 `wasm-bindgen`
  运行时必须是**同一 0.2.x**：`scripts/wasm-bindgen.ps1` 会做版本校验，不一致直接失败。
- `lua-src` 编译出的 Lua 版本由项目选择（`mlua` 的 feature，如 `lua54`）；升级 Lua
  源码目录时保留现用版本目录。

## 升级步骤（每次跟进上游时照此过一遍）

1. 从 crates.io / 上游 tag 取新版本，覆盖对应目录（保留我们自己的文件：
   `mlua-sys/wasm-shim-c`、`src/wasm_shim.rs`、`lua-src/src/lib.rs` 的 wasm32 分支、
   `wasm-bindgen-macro` 的 `extern "C"` 导出改动）。
2. 与 `Cargo.toml.orig` 对照 diff，确认清单（依赖、features）没有把 patch 引入的
   条目弄丢；同步三处版本号（macro/support/cli 与 `game_web` 运行时）。
3. 更新 `engine/Cargo.toml` `[patch.crates-io]` 注释里的版本说明。
4. 验证清单（全部通过才算升级完成）：
   - `cargo test -p engine --lib`（桌面全量）+ `cargo fmt` / `cargo clippy -- -D warnings`
   - `cargo check -p game_web --target wasm32-unknown-unknown`（需要 `ZIG` 环境变量）
   - wasm 桥契约：`artifacts/verify-no-modules-bridge.mjs`
   - webgl / 微信发布门（docs 里的 p2-scene-verify 场景切换回归）
