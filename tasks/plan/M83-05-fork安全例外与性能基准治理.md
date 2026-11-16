# 任务：fork、安全例外与性能基准治理

- 任务 ID：M83-05
- 状态：治理台账已定稿（2026-11-16）；三项升级动作待执行
- 关联里程碑：M83
- 创建日期：2026-11-16

## 目标

把"本地 vendored fork""供应链安全例外""渲染剔除该不该做"这三件容易变成口头承诺的事，
落成**有事实源、有周期、有判据、能判不做**的治理台账。

## 一、vendored fork 台账（2026-11-16 核对）

来源：`engine/Cargo.toml` 的 `[patch.crates-io]`（第 115–129 行）+ `engine/Cargo.lock`。

| 包 | vendor 版本 | lock 版本 | 补丁目的 | 上游状态 |
|---|---|---|---|---|
| `mlua` | 0.11.6 | 0.11.6 | wasm 侧 `catch_unwind` 跳过、`RawLua`/`State` wasm 分支 | 上游活跃，本地 ahead |
| `mlua-sys` | 0.10.0 | 0.10.0 | wasm C 垫片（`wasm-shim-c/`：trampoline/sjlj/stdio）+ `wasm_shim.rs` Rust 半 | 上游活跃，本地 ahead |
| `lua-src` | 550.0.0 | 550.0.0 | wasm EH 编译参数（`-fwasm-exceptions` / `-wasm-enable-sjlj`）、排除 `liolib` | 上游活跃，本地 ahead |
| `wasm-bindgen-macro` | 0.2.129 | 0.2.129 | 导出由 `extern "C-unwind"` 改 `extern "C"`，消除 EH landing pad | 上游持续发版 |
| `wasm-bindgen-macro-support` | 0.2.129 | 0.2.129 | 同上 | 同上 |
| `paste`（兼容包名） | 1.0.15（实为 pastey 0.2.3 代码） | 1.0.15 | `paste` 已归档，充当同名兼容包 | **上游归档**，属长期维护自担 |

### 1.1 现状门（2026-11-16 实测读数）

- `scripts/verify/check-vendor-patches.ps1`：**30 passed / 0 failed**（patch 在位、wasm shim 文件齐全、
  `build.rs` 集成点、EH 参数、无 `C-unwind` 残留、vendor↔lock 版本耦合、
  文档里"30 条"的计数一致）；
- `scripts/repo/fork-sync-report.mjs --check`：**`fork_sync_ok forks=7 patched=5`**
  （7 个 fork 中 5 个打补丁；`wasm-bindgen-cli` 是随仓工具、不打补丁，
  `wasm-bindgen-macro` 与 `-support` 算同一 fork 的一对包）——以脚本读数为准，本文表格是分类说明；
- `scripts/verify/check-security.ps1 -RequireTools`（本机离线）：**`security: 2 passed, 0 failed,
  2 blocked` → exit 3 BLOCKED**（cargo-audit / cargo-deny 的 advisory DB 网络不可达）。
  这正是"缺工具/网络是 BLOCKED，不是通过"的实证；CI 的 `security` job 联网跑同一入口；
- `engine/deny.toml` 的 `[sources]` 只允许 crates.io + path，git 依赖为 0。


### 1.2 缺口（本任务要补的）

1. **没有"上游差异评审"的落档位置**：`fork-sync-report.mjs` 产出报告，但没有"本次差异是否
   可接受 / 谁看 / 下次看什么时候"的登记。建议新增 `engine/vendor/REVIEW.md`（台账：每条
   fork 的差异摘要、上次核对 SHA/日期、下次到期日、升级触发条件）。
2. **没有升级回归集清单**：升级 fork 必须同时过 native + wasm 的负向路径
   （`pcall` 错误传播、wasm EH trap、`mlua` 崩溃注入）。建议固化成一条脚本命令，
   列进 `check-vendor-patches.ps1` 的文档头而不是靠记忆。
3. **`paste` 兼容包是长期债**：上游已归档，本地靠 pastey 代码续命。触发升级的条件应写死：
   `egui_dock`/`simba`/`metal` 任一去掉 `paste` 依赖，就应立即删除该 patch（而不是无限期保留）。

## 二、供应链安全例外台账

来源：`engine/deny.toml` 第 18–31 行 + `docs/69-发布与供应链安全.md` §4.2。

| 例外 | 包/版本 | 原因 | 现状 | 复核截止 |
|---|---|---|---|---|
| `RUSTSEC-2026-0192` | `ttf-parser` 0.25.1 | 作者声明不再维护；**没有 patched 版本、也没有 drop-in 替代**；`ab_glyph`/`epaint`/`rustlua-render` 直接依赖 | `deny.toml` 的 `ignore`（到期后门会重新变红，强制重评） | **2027-10-20** |

机制本身是可靠的：`cargo-deny` 的 ignore 只接受 `id` + `reason`，到期靠**人工复核**；
`check-security.ps1` 的 `-RequireTools` 在缺工具/网络时显式 BLOCKED(3) 而不是通过。

### 2.1 缺口与建议

- 台账要求在 2027-10-20 前给出"替换/自维护/接受"三者之一的书面决策，**目前没有跟踪项**。
  建议：在 `docs/69` §4.2 落一个带日期的决策表（选项/成本/风险），并把复核日期同步进
  `metrics/allowlist.json` 风格的到期机制（与 M83-04 的 `metrics/web-evidence.json` 同一模式：
  到期不处理就判红）。
- `ttf-parser` 若自维护，成本是"字体解析器"级别的安全责任，需明确**不做**或**fork + 定期合上游**；
  这条不建议顺手做，建议在 2027 上半年专项评估。

## 三、渲染剔除治理（承 M79 结论）

M79 已有两组读数，结论是**先补基准、再谈结构**：

- 2D 空间索引（`SpatialGrid2D`）release 档收益 12.7–15.1% < 20% 判据 ⇒ **维持 opt-in**；
- 现有 `wp0_3d_stress` 的 100/1000/10000 网格**全在视锥内**（`--cull-headroom` 实测
  `outside_pct=0.0%`）⇒ 在该基准上写 BVH/HZB，`instances` 与帧时间都不会动。

本任务只补**判据与读数工具**，不写结构：

1. 新增/改造可重复场景：网格铺开到 1000×1000、相机贴近一角，使 `outside_pct` 显著非零；
2. 输出三项读数：每帧提交实例数、CPU 遍历耗时、帧时间；
3. 判据沿用 M79 的 20%：三者之一改善 ≥20% 才进入"做剔除"；否则保留读数工具并写"不做"结论。

### 3.1 已落地（2026-11-16）：`--scatter` 布局

`engine/crates/engine/examples/wp0_3d_stress.rs` 增加 `--scatter`（默认关闭，默认布局与报告逐字不变）：

- 布局：`ceil(sqrt(n))` 边长、格距 10 的方格（10,000 ⇒ 约 990×990），居中于原点；
- 相机：贴在场地 +X 一侧（离最近一排 20 单位）向 -X 看，`far=5000`；
- 阳性对照：同位置**背向**看，必须 100% 外（否则读数本身坏了）。

实测读数（`cargo run -p engine --example wp0_3d_stress -- --cull-headroom --scatter`）：

| mesh | inside | outside | outside_pct | 对照 |
|---|---|---|---|---|
| 100 | 88 | 12 | **12.0%** | 100.0% |
| 1,000 | 804 | 196 | **19.6%** | 100.0% |
| 10,000 | 7,712 | 2,288 | **22.9%** | 100.0% |

对比默认布局（同一命令不加 `--scatter`）：三档 `outside_pct=0.0`（全在视锥内）——
这正是 M79"现有基准上写 BVH 一定是结构好看、读数不动"的量化依据。

三项读数怎么取：
- **提交实例数**：剔除后应提交的数量 = `inside`（上表）；当前渲染仍提交全部，作为对照；
- **遍历耗时 / 帧时间**：用 `--scatter` 跑一次渲染档
  （`cargo run --release -p engine --example wp0_3d_stress -- --scatter --cycles 30 --profile --output artifacts/wp0-3d-scatter.json`），
  报告的 `samples.p50_us` 是帧时间、`profile.extract_us` 是提取（遍历）耗时。

**注意**：`--scatter` 的报告**不要**写进 `metrics/baselines/`——它是判"做不做剔除"的实验场景，
不是发布基线；默认布局那份才是门用的结构性基线（见 M83-07）。

> 与 M83-07 的分工：M83-07 管**度量基线**（`cpu_peak_bytes`/每帧分配），本任务管**剔除结构**的
> 进入判据。两者不要混在一轮里改。

## 四、验收方式

```powershell
# fork 补丁基线（30 条断言）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify\check-vendor-patches.ps1
# fork 上游差异报告（离线）
node scripts/repo/fork-sync-report.mjs --check
# 供应链门（需要工具；缺失时 BLOCKED=3，不是通过）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify\check-security.ps1 -RequireTools
# 剔除上限读数（现有基准，应为 0%）
cargo run -p engine --example wp0_3d_stress -- --cull-headroom
```

## 五、未完成项 / 已知限制

- 三件升级动作均未开工：`vendor/REVIEW.md` 台账、`ttf-parser` 决策表、可剔除场景基准。
- 本机 `gh` 不可用、Actions 日志 API 需认证，安全门失败时只能看到步骤名，读不到正文；
  这条限制与 M78 §1.1 记录一致，未改变。
- 渲染剔除"不做"也是合法结论：M79 的 20% 判据不因本台账而放宽。
