# engine/vendor 上游差异评审台账

本目录下的 `mlua` / `mlua-sys` / `lua-src` / `wasm-bindgen-macro*` / `paste` 都是
**本地 vendored fork**（`engine/Cargo.toml` 的 `[patch.crates-io]` 注入）。它们修的是
wasm 侧的真实问题（Lua C 的 wasm EH、`extern "C-unwind"` 的 landing pad、`paste` 归档后的
同名兼容包），代价是**上游的安全与兼容修复不会自动流进来**。这份台账就是这笔技术债的账本。

## 1. 自动化部分（已有，不需要人记）

| 门/工具 | 做什么 | 频率 |
|---|---|---|
| `scripts/verify/check-vendor-patches.ps1` | 30 条结构断言：patch 在位、wasm shim 文件齐全、`build.rs` 集成点、EH 编译参数、无 `C-unwind` 残留、vendor↔lock 版本耦合、文档计数一致 | 每次质量门 |
| `node scripts/repo/fork-sync-report.mjs --check` | 7 个 fork 的结构一致性 + **上游源码差异**（离线读 `registry/src`）；`--online` 额外查 crates.io 是否有新版本 | `check-vendor-patches` 同批；`--online` 由 `.github/workflows/security-schedule.yml` **每周一 03:00 UTC** 跑 |
| `cargo-deny` / `cargo-audit`（`check-security.ps1 -RequireTools`） | advisory / license / source 门；缺工具或网络时显式 `BLOCKED(3)`，不冒充通过 | 质量门 + 每周定时 |

生成完整报告：

```powershell
node scripts/repo/fork-sync-report.mjs --write artifacts/reports/fork-sync.md
node scripts/repo/fork-sync-report.mjs --online --write artifacts/reports/fork-sync-online.md
```

## 2. 当前台账（2026-11-16 实测，数据源 `artifacts/reports/fork-sync.md`）

`fork_sync_ok forks=7 patched=5`、`check-vendor-patches` 30 passed / 0 failed。

| fork | vendored | 上游基线 | 差异（新增/修改/删除） | 补丁目的 | 下次评审 |
|---|---|---|---|---|---|
| `mlua` | 0.11.6 | crates.io 0.11.6 | 4 / 3 / 4 | wasm：`RawLua`/`State` 的 wasm 分支、`catch_unwind` 跳过、错误路径适配 | 2027-02-28 |
| `mlua-sys` | 0.10.0 | crates.io 0.10.0 | 15 / 4 / 0 | wasm C 垫片 `wasm-shim-c/`（trampoline / sjlj / stdio）+ `wasm_shim.rs` Rust 半 + `build.rs` 集成 | 2027-02-28 |
| `lua-src` | 550.0.0 | crates.io 550.0.0 | 1 / 2 / 0 | wasm EH 编译参数（`-fwasm-exceptions` / `-wasm-enable-sjlj`）、排除 `liolib` | 2027-02-28 |
| `wasm-bindgen-macro` | 0.2.129 | crates.io 0.2.129 | 0 / 0 / 0 | **待复核**：报告显示内容与上游相同（只多 `Cargo.toml.orig`），但 vendor 门仍断言"无 C-unwind 残留"。见 §4 观察 ① | 2027-02-28 |
| `wasm-bindgen-macro-support` | 0.2.129 | crates.io 0.2.129 | 0 / 1 / 0 | 导出模板改 `extern "C"`（若上游已含，则本 patch 可删） | 2027-02-28 |
| `paste`（兼容包名） | 1.0.15 | pastey `as1100k/pastey` 0.2.3 | 4 / 8 / 36 | `paste` 已归档；用 pastey 维护代码挂 `paste 1.0.15` 名供 `egui_dock`/`simba`/`metal` 的 `paste ^1.0` 解析 | 见 §4 观察 ② |
| `wasm-bindgen-cli` | 0.2.129 | crates.io 0.2.129 | 不打补丁 | 随仓工具（离线构建用），不进依赖图 | — |

## 3. fork 升级/复审清单（每周自动报告之后，人工按需执行）

1. 看 `fork-sync-online.md` 的"在线最新 / 漂移"两列——有上游新版再看 §2 的补丁是否仍需要；
2. **先跑 native，再跑 wasm**：`cargo check --workspace --all-targets`、
   `cargo check -p game_web -p gpu-web-probe --target wasm32-unknown-unknown`；
3. 跑 **负向路径**（fork 的价值全在错误路径）：Lua `pcall` 里的宿主错误传播、
   wasm 侧 trap/`unreachable`、`mlua` 崩溃注入用例；
4. `check-vendor-patches.ps1` + `fork-sync-report.mjs --check` + `verify.ps1 -Quick` 全绿；
5. 更新本文件 §2 的版本/差异/日期，并在 `docs/changelog/最新条目.md` 记一条（含"为什么这次补丁仍有必要"）。

## 4. 已知观察（下次评审必须给结论）

① **`wasm-bindgen-macro` 的 patch 可能已过时**：`--write` 报告显示它与 crates.io 0.2.129
内容零差异，而 `check-vendor-patches.ps1` 仍断言"无 `C-unwind` 残留"。两种可能：(a) 上游已修复，
本地 patch 只剩 `Cargo.toml.orig`；(b) 报告的比较口径没覆盖到被改的文件。
下次评审要**明确选一个**：删 patch（并同步删断言）或修正报告口径——不允许两处口径长期不一致。

② **`paste` shim 的删除触发条件（写死，防止无限期保留）**：`egui_dock`、`simba`、`metal`
任一不再声明 `paste ^1.0`，就删 `engine/vendor/paste` 与对应 `[patch.crates-io]` 条目，
改为直接依赖 `pastey`。检查方法：
`cargo tree -i paste --all-features | Select-String 'egui_dock|simba|metal'`（无输出即可删）。

## 5. 与其它台账的关系

- 供应链例外（`ttf-parser` RUSTSEC-2026-0192，复核截止 2027-10-20）记在
  [docs/69 §4.2.1](../../docs/69-发布与供应链安全.md)，本文件只管 vendored fork；
- 平台升档与 Web 证据受限分别记在 `metrics/promotion.json` 与 `metrics/web-evidence.json`，
  三者都用"到期不处理即判红"的口径，不要在这里重复登记。
