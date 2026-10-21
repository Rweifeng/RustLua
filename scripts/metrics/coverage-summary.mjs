// coverage-summary.mjs —— 把 cargo llvm-cov 的原始 JSON 归一成 coverage-v1（P4-01）
//
// 用法：
//   node scripts/metrics/coverage-summary.mjs \
//     --input artifacts/metrics/coverage.json \
//     --out   artifacts/metrics/coverage-summary.json \
//     [--markdown artifacts/metrics/coverage-summary.md] \
//     [--env-fingerprint sha256:...] [--git-commit abc1234] [--git-dirty true|false]
//
// 输入：`cargo llvm-cov --workspace --all-features --locked --no-fail-fast
//   --ignore-filename-regex 'engine[\\/]vendor[\\/]|engine[\\/]target[\\/]'
//   --json --summary-only --output-path <file>` 产生的 JSON。
// 输出：coverage-v1（overall 行/函数/区域 + per-crate 表）。
//
// 口径（metrics/README.md §10）：
//   - 只统计 workspace 源码：engine/vendor/**、target/**、OUT_DIR 不计入；
//   - 内联测试代码（#[cfg(test)] 模块）会被 LLVM 计入行覆盖，报告写明"含测试代码"，
//     不伪装成"产品代码覆盖率"。
// 退出码：0=成功；1=输入缺失/JSON 结构不认。

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, basename, sep } from "node:path";

const args = process.argv.slice(2);
function arg(name, fallback = "") {
  const i = args.indexOf(name);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : fallback;
}
const input = arg("--input", "artifacts/metrics/coverage.json");
const out = arg("--out", "artifacts/metrics/coverage-summary.json");
const markdown = arg("--markdown", "");
const fingerprint = arg("--env-fingerprint", "");
const gitCommit = arg("--git-commit", "");
const gitDirty = arg("--git-dirty", "false") === "true";

function fail(msg) {
  console.error(`coverage_summary_failed ${msg}`);
  process.exit(1);
}

let raw;
try {
  raw = JSON.parse(readFileSync(input, "utf8"));
} catch (e) {
  fail(`input unreadable or invalid JSON: ${input} (${e.message})`);
}
if (!Array.isArray(raw.data)) {
  fail(`unexpected llvm-cov JSON shape: expected top-level "data" array (got ${Object.keys(raw).join(",")})`);
}

// ── 归一化 ────────────────────────────────────────────────────────────────────
// 文件路径形如：engine/crates/rustlua-ecs/src/lib.rs 或绝对路径 ...\engine\crates\xxx\src\...
// crate 解析优先级：crates/<name>/ → engine（本体 src/ 在 engine/src）→ (unknown)
function crateOf(filePath) {
  const norm = filePath.replace(/\\/g, "/");
  if (/\/vendor\//.test(norm) || /\/target\//.test(norm) || /\/OUT_DIR\//.test(norm)) return null;
  const m = norm.match(/\/crates\/([^/]+)\//);
  if (m) return m[1];
  if (/\/engine\/(src|examples|benches|tests)\//.test(norm)) return "engine";
  return null; // workspace 外的文件（依赖源码等）不计入
}

const totals = { functions: { count: 0, covered: 0 }, instantiations: { count: 0, covered: 0 }, lines: { count: 0, covered: 0 }, regions: { count: 0, covered: 0 }, branches: { count: 0, covered: 0 } };
const byCrate = new Map(); // name -> totals 累加
let includedFiles = 0;
let excludedFiles = 0;

for (const block of raw.data) {
  if (!block || !Array.isArray(block.files)) continue;
  for (const f of block.files) {
    const cr = crateOf(f.filename ?? "");
    if (!cr) { excludedFiles++; continue; }
    includedFiles++;
    if (!byCrate.has(cr)) byCrate.set(cr, JSON.parse(JSON.stringify(totals)));
    const acc = byCrate.get(cr);
    for (const k of ["functions", "instantiations", "lines", "regions", "branches"]) {
      const src = f[k] || {};
      acc[k].count += src.count ?? 0;
      acc[k].covered += src.covered ?? 0;
    }
  }
}
for (const acc of byCrate.values()) {
  for (const k of ["functions", "instantiations", "lines", "regions", "branches"]) {
    totals[k].count += acc[k].count;
    totals[k].covered += acc[k].covered;
  }
}

function pct(t) { return t.count > 0 ? (t.covered / t.count) * 100 : 0; }

const crates = [...byCrate.entries()]
  .map(([name, t]) => ({
    name,
    lines: { count: t.lines.count, covered: t.lines.covered, percent: +pct(t.lines).toFixed(2) },
    functions: { count: t.functions.count, covered: t.functions.covered, percent: +pct(t.functions).toFixed(2) },
    regions: { count: t.regions.count, covered: t.regions.covered, percent: +pct(t.regions).toFixed(2) },
    source_lines: t.lines.count, // 含测试代码
  }))
  .sort((a, b) => b.lines.count - a.lines.count);

const summary = {
  schema: "coverage-v1",
  generated_at: new Date().toISOString(),
  git_commit: gitCommit || "",
  git_dirty: gitDirty,
  env_fingerprint: fingerprint || "",
  overall: {
    lines: { count: totals.lines.count, covered: totals.lines.covered, percent: +pct(totals.lines).toFixed(2) },
    functions: { count: totals.functions.count, covered: totals.functions.covered, percent: +pct(totals.functions).toFixed(2) },
    regions: { count: totals.regions.count, covered: totals.regions.covered, percent: +pct(totals.regions).toFixed(2) },
  },
  crates,
  included_files: includedFiles,
  excluded_files: excludedFiles,
  notes: [
    "含测试代码：inline #[cfg(test)] 模块被 LLVM 计入行覆盖，本口径不是'产品代码覆盖率'。",
    "engine/vendor/**、target/**、OUT_DIR 不计入。",
  ],
};

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify(summary, null, 2));

// ── Markdown 报告（可选）──────────────────────────────────────────────────────
if (markdown) {
  const rows = crates.map((c) =>
    `| ${c.name} | ${c.lines.count} | ${c.lines.percent.toFixed(1)}% | ${c.functions.percent.toFixed(1)}% | ${c.regions.percent.toFixed(1)}% |`
  ).join("\n");
  const md = `# 覆盖率报告（coverage-v1，含测试代码口径）

- commit: ${gitCommit || "-"}（dirty=${gitDirty}）
- 环境指纹: ${fingerprint || "-"}
- 文件：workspace 源码 ${includedFiles} 个计入，${excludedFiles} 个排除（vendor/target/OUT_DIR）

| overall | 行 | 函数 | 区域 |
|---|---|---|---|
| percent | ${summary.overall.lines.percent.toFixed(1)}% | ${summary.overall.functions.percent.toFixed(1)}% | ${summary.overall.regions.percent.toFixed(1)}% |

## per-crate（按源码行数降序）

| crate | 行数（含测试） | 行覆盖 | 函数覆盖 | 区域覆盖 |
|---|---|---|---|---|
${rows}

## 口径

- ${summary.notes.join("\n- ")}
`;
  mkdirSync(dirname(markdown), { recursive: true });
  writeFileSync(markdown, md);
}

console.log(`coverage_summary_ok files=${includedFiles} excluded=${excludedFiles} crates=${crates.length} lines=${totals.lines.count} covered=${totals.lines.covered} percent=${pct(totals.lines).toFixed(2)}`);
