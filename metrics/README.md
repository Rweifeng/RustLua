# metrics/ —— 性能与测试度量底座（P4-00）

本目录回答七个问题：**这棵树的测试跑了多少代码、wasm 条件分支有没有被证明、
别的 OS 上还行不行、这次 p95 比历史慢了多少、改一行要重编多久、二进制和包体
是变大还是变小**。度量本身不优化性能——它让每一次优化/回退能被看见、可判红、
可离线复核、有历史。

## 1. 分层原则

| 层 | 位置 | 角色 | 能否删 |
|---|---|---|---|
| 历史事实源 | `metrics/history/YYYY-MM.jsonl`（被 git 跟踪） | append-only run 记录，**删它才丢数据** | 否 |
| 派生索引 | `artifacts/metrics/trend.sqlite`（/artifacts/ 已 gitignore） | 由 history 重建的只读查询库 | 是，可随时 `rebuild` |
| 人工基线 | `metrics/baselines/*.json`（被跟踪） | 人工确认过的参考点，只由 `--update` 生成 | 需评审 |
| 原始证据 | `artifacts/metrics/*`（gitignore） | 本次运行的原始报告、日志、HTML | 是 |
| 条件清单 | `metrics/inventory/cfg-sites.json`、`metrics/wasm-cells.json`、`metrics/wasm-evidence.json` | 被跟踪的声明式清单 | 需评审 |

> 删掉 SQLite 不会丢数据；删掉 `history/` 才会。`baselines/`（仓库根，gitignore）仍
> 保留为本机快速 A/B 目录，与 `metrics/baselines/`（长期）并存，见 §7。

## 2. run 记录 schema（`metrics-run-v1`）

`metrics/history/YYYY-MM.jsonl` 每行一个完整 run（约 2–10 KB，不存原始采样全量；
原始报告留在 `artifacts/metrics/`）：

```json
{
  "schema": "metrics-run-v1",
  "run": {
    "run_id": "b7958560-20261020T140000Z-ci-perf-01",
    "created_at": "2026-10-20T14:00:00Z",
    "git_commit": "b7958560",
    "git_dirty": false,
    "source": "ci",
    "workflow": "metrics.yml",
    "workflow_run_id": "1234567890",
    "env_fingerprint": "sha256:…",
    "os": "windows",
    "arch": "x86_64",
    "cpu_model": "…",
    "logical_cores": 4,
    "ram_gib": 16,
    "rustc": "1.97.1",
    "node": "20",
    "python": "3.12",
    "profile": "release",
    "features": "default",
    "cargo_jobs": 4,
    "runner_label": "github-windows-latest"
  },
  "samples": [
    {
      "domain": "perf",
      "workload": "wp0_3d_stress/mesh=1000",
      "metric": "p95_us",
      "value": 123.4,
      "unit": "us",
      "better": "lower",
      "labels": {"mesh_count": 1000},
      "source": {"kind": "report", "path": "artifacts/.../wp0-3d-stress.json", "sha256": "…"}
    }
  ]
}
```

约束：

- `domain` 固定为 `perf | coverage | compile | binary | package | cfg`。
- `unit` 只用 `us | ms | s | bytes | count | percent | pp`；`better` 只用 `lower | higher`。
- 所有数值必须带 `source.kind`（`report | log | manual | unparsed`）+ `source.path` + `sha256`。
- 手填的历史数据只能标 `source.kind=manual`，**不能直接成为门基线**。
- `git_dirty=true` 的 run 可入库，但**不能生成/更新基线**；基线只来自干净工作树。

## 3. 环境指纹

```
env_fingerprint = sha256(
  os | arch | cpu_model | logical_cores | ram_gib |
  rustc | node | python | profile | features | runner_label
)
```

- **时间类**指标（p50/p95/p99、编译 wall time、增量时间）只与**同指纹**的样本比；
  跨指纹只打印不判红。GitHub runner 的 CPU 型号会漂，`runner_label` 只能粗分，
  所以时间类还要 `min_samples >= 3` 且用中位数，不用均值。
- **确定性**指标（draw_calls/instances/triangles、分配字节/次数、exe/wasm/包体字节、
  覆盖率百分比）可以跨指纹比；仍要记录指纹，便于归因。

## 4. 退出码契约

| 退出码 | 含义 | 例子 |
|---|---|---|
| `0` | 通过 | 有基线、有足够样本、无回退 |
| `1` | 配置/输入错误、缺必需文件、schema 不认、门自测失败 | `metrics/policy.json` 写错、报告 schema 不匹配 |
| `2` | 有证据的回归/超阈值 | p95 +8% > 5%、覆盖率掉 1.2pp、exe +6% |
| `3` | **BLOCKED**：工具缺失、环境受限、历史样本不足；**不等于通过** | 无 `cargo-llvm-cov`、无浏览器、`min_samples` 不够 |

`verify.ps1` 对 `3` 打印 `PARTIAL/BLOCKED` 并以 3 退出；CI 的必需 job 把 `3` 当红。
**"没跑"永远不等于"通过"。**

## 5. 基线、棘轮与豁免

- **只降不升**默认：覆盖率、性能、包体都以 `metrics/baselines/*` 为参考，超阈值判 `2`。
- **更新基线必须显式**：`--update` 生成新基线，附带 `reason`；提交信息写清"为什么这次
  增长是合理的"；评审看 diff。`git_dirty=true` 的 run 不能更新基线。
- **暂缓项有到期日**：`metrics/allowlist.json`：

```json
{
  "schema": 1,
  "entries": [
    {
      "domain": "binary",
      "subject": "editor.exe",
      "owner": "P4",
      "reason": "新功能一次性增长，计划在 PBR 后回收",
      "expires": "2026-12-31",
      "max_delta_pct": 8
    }
  ]
}
```

到期后门直接判 `2`，不打印黄字放行。豁免必须能指到具体 artifact/metric，不能整域豁免。
加豁免的流程：改 `allowlist.json` → 提交（owner/reason/expires 三件套齐全）→ 门自测通过。

## 6. 工具固定版本

- `scripts/release/tools.json` 是单一事实源（与 P2 同哲学）：
  - `cargoLlvmCov`：精确 semver（安装后实测版本写死）；
  - `python`：CI 固定 `3.12`（`actions/setup-python@v5`），本地允许 `>=3.11`，门打印实际版本；
  - `rust`/`node`：沿用既有锁定。
- `scripts/release/install-metrics-tools.ps1`：`-Check` 只读校验；安装
  `cargo install --locked cargo-llvm-cov --version <pin>` + `rustup component add llvm-tools-preview`。
  缺工具时门退 `3`，CI `-RequireTools` 判红。
- `scripts/verify/check-toolchain-lock.ps1` 扩展后校验：`cargoLlvmCov` 是精确 semver、
  CI 的 Python 版本与 `tools.json` 一致；**不允许 `latest`**。
- 不新增 Rust 依赖、不动 `Cargo.lock`；工具装在 `CARGO_HOME`，不进 workspace。

## 7. 与既有门的关系

- `compare-baseline.ps1` 保留：它是"两份报告"的硬比较；`check-metrics.ps1` / `trend_db.py check`
  是"历史 + trailing median"的补充，两者都跑，不互相替代。
- `baselines/`（仓库根，gitignore）保留为本机快速 A/B；`verify.ps1 -Benchmarks` 先读
  `metrics/baselines/wp0-3d-stress.json`，不存在再回退 `baselines/wp0-3d-stress.committed.json`，
  避免"基线换位置"造成假红。
- `metrics/baselines/m74-frame-profile.json`（2026-10-21）是**帧内调度分档台账**，不是
  `--update` 生成的回归基线：它记录一次判定性测量（M74-02 的 P0 闸门，结论"不做"），
  条目与口径见 `tasks/done/M74-02-ECS并行调度.md` §实现记录与 `docs/53` §3.11。
  判定读的是"可并行 stage 的最大占帧比"，`Script`（Lua）单列不计入。
- `verify.ps1` 新增两个开关：
  - `-Coverage`：只跑覆盖率（重活，需工具）；
  - `-Metrics`：只跑"已有产物的尺寸采集 + cfg 门 + 趋势检查"，不重跑 benchmark/构建。
  两者与 `-Quick`/`-Mvp`/`-MvpOnly`/`-Benchmarks`/`-Web`/`-WasmLua`/`-Security` 互斥，
  混用直接报错；`-Quick` 的跳过清单会登记它们。

## 8. 各域命令速查

```powershell
# 工具与基线（P4-00）
powershell -NoProfile -File scripts/release/install-metrics-tools.ps1 -Check
powershell -NoProfile -File scripts/verify/check-toolchain-lock.ps1

# 覆盖率（P4-01）
powershell -NoProfile -File scripts/verify/check-coverage.ps1 -RequireTool
powershell -NoProfile -File scripts/test/test-coverage-gate.ps1

# wasm 条件编译（P4-02）
node scripts/metrics/cfg-inventory.mjs --check
powershell -NoProfile -File scripts/verify/check-cfg-coverage.ps1

# 平台矩阵（P4-03）
powershell -NoProfile -File scripts/verify/check-matrix-policy.ps1 -List

# 趋势库（P4-04）
python scripts/metrics/trend_db.py rebuild --history metrics/history --db artifacts/metrics/trend.sqlite
python scripts/metrics/trend_db.py ingest --input artifacts/metrics --history metrics/history
python scripts/metrics/trend_db.py report --domain all --since 90d
python scripts/metrics/trend_db.py check --policy metrics/policy.json
python scripts/test/test-trend-db.py

# 编译时间（P4-05）
powershell -NoProfile -File scripts/metrics/measure-build.ps1 -Mode check -Cold -Yes -Out artifacts/metrics/compile-check.json
node scripts/metrics/parse-cargo-timings.mjs --check

# 体积（P4-06/07）
node scripts/metrics/measure-artifacts.mjs --kind binary --out artifacts/metrics/binaries.json
node scripts/metrics/measure-artifacts.mjs --kind package --project examples/game_jhyt --out artifacts/metrics/packages.json
powershell -NoProfile -File scripts/test/test-package-metrics.mjs

# 总门与文档
powershell -NoProfile -File scripts/verify/verify.ps1 -Metrics
powershell -NoProfile -File scripts/verify/check-docs.ps1
```

> 任何一步打印 `BLOCKED`/`skipped` 都不是通过；`3`（BLOCKED）的原因必须写出来。
> 首次跑覆盖率/趋势时没有基线是**预期**的：先产出报告，人工审阅后 `--update` 一次，
> 下一次才进入判红状态。

### 8.1 history 的写模式（CI）

`metrics/history/*.jsonl` 是**被 git 跟踪**的事实源；`metrics.yml`（`.github/workflows/metrics.yml`）
负责 nightly perf / weekly package 的采集与入库，PR 只读（从 history 重建派生库并 check）。

写入模式两种（计划 §P4-04-5，默认方式由仓库管理员定）：

- **bot commit**：CI 用 `contents: write` 权限只提交 `metrics/history/**`（不碰其他文件）；
  前提是仓库策略允许 bot push 且不留门禁缺口。
- **artifact + 人工 merge**（当前默认）：CI 上传 `metrics-history` artifact
  （history jsonl + trend.sqlite + 原始证据），人工审阅后 merge 进 main。

两种模式都**只追加**：`run_id` 由稳定内容生成，同文件重复 ingest 自动 `skipped_dup=1`；
删 history 里的行 = 丢数据，不做。

## 9. 哪些数字跨机可比、哪些只是记录

| 可比（跨指纹仍判红） | 只记录（跨指纹不判红，同指纹中位数才判） |
|---|---|
| draw_calls / instances / triangles | p50 / p95 / p99 |
| 分配字节 / 分配次数 | 编译 wall time / 增量时间 |
| exe / wasm / 包体字节（原始 + gzip + brotli） | editor --perf-baseline 场景时间 |
| 覆盖率百分比（含测试代码口径） | — |

## 10. 口径声明

- 覆盖率口径 = **测试实际执行到的 workspace 源码**，`engine/vendor/**`、`target/**`、
  生成的 `OUT_DIR` 不计入；**内联测试代码会被 LLVM 计入行覆盖**，首轮接受并在报告里写明
  "含测试代码"，不把它伪装成"产品代码覆盖率"。
- 不追求"覆盖率 80%"这类绝对目标：首轮只做棘轮（不许掉），绝对下限等实测后再定。
  **不允许为了绿去删测试或写无意义断言。**
- wasm 行覆盖率本轮不做；P4-02 只做"条件编译覆盖 + 运行证据映射"。
- Linux/macOS 矩阵只证明"编译/测试子集在那些 OS 上成立"，**不是产品支持承诺**。
