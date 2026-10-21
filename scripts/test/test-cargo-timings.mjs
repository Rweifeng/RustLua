#!/usr/bin/env node
/**
 * test-cargo-timings.mjs —— parse-cargo-timings 自测（P4-05）。
 * 用固定 HTML fixture 断言：UNIT_DATA 解析、critical path、wall、per-crate、坏 JSON 退 1。
 * 输出：cargo_timings_selftest_ok cases=5
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const script = path.join(__dirname, "..", "metrics", "parse-cargo-timings.mjs");

const FIXTURE = `<!DOCTYPE html><html><head><title>cargo timing report</title></head><body>
<script>
const UNIT_DATA = [
  ["rustlua-infra::lib.rs (lib)", 1000.0, 0.0, "Fresh"],
  ["rustlua-math::lib.rs (lib)", 500.0, 1000.0, "Fresh"],
  ["rustlua-infra::io.rs (lib)", 700.0, 100.0, "Fresh"],
  ["game::main.rs (bin)", 300.0, 1700.0, "Fresh"]
];
const CONCURRENCY_DATA = [
  [0.0, 1],
  [100.0, 2],
  [200.0, 2],
  [2000.0, 1]
];
</script>
</body></html>`;

// 期望：unit_count=4；critical = max(start+duration) = max(1000, 1500, 800, 2000) = 2000ms；
// wall = 2000ms；parallelism = 2；per-crate: rustlua-infra 1700ms、rustlua-math 500ms、game 300ms
const BAD_FIXTURE = `const UNIT_DATA = [this is not json];`;

function run(nodeArgs) {
  return execFileSync(process.execPath, [script, ...nodeArgs], { encoding: "utf8" });
}

function main() {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "cargo-timings-test-"));
  const okHtml = path.join(tmp, "cargo-timing-2026-01-01T00_00_00.html");
  const badHtml = path.join(tmp, "cargo-timing-bad.html");
  fs.writeFileSync(okHtml, FIXTURE);
  fs.writeFileSync(badHtml, BAD_FIXTURE);
  let failed = 0;

  const out = path.join(tmp, "out.json");
  let stdout;
  try {
    stdout = run(["--input", okHtml, "--out", out]);
  } catch (e) {
    console.error("[FAIL] 解析失败: " + e.message);
    return 1;
  }
  const data = JSON.parse(fs.readFileSync(out, "utf8"));
  const checks = [
    ["unit_count=4", data.unit_count === 4],
    ["critical=2000", data.critical_path_ms === 2000],
    ["wall=2000", data.wall_ms === 2000],
    ["parallelism=2", data.parallelism === 2],
    ["per-crate infra=1700", data.crates["rustlua-infra"] && data.crates["rustlua-infra"].duration_ms === 1700],
    ["top_crate=rustlua-infra", data.top_crate === "rustlua-infra"],
    ["compile_ok 输出", /compile_ok units=4 wall_ms=2000/.test(stdout)],
  ];
  for (const [name, pass] of checks) {
    if (pass) console.log("[PASS] " + name);
    else { console.log("[FAIL] " + name); failed++; }
  }

  // 坏 JSON：目录里全是坏文件 -> 退 1
  try {
    run(["--input", tmp + "\\bad-dir".replace("\\", path.sep)]); // 不存在，走到不存在分支
  } catch (e) { /* expected */ }
  const badDir = path.join(tmp, "all-bad");
  fs.mkdirSync(badDir);
  fs.writeFileSync(path.join(badDir, "cargo-timing-bad.html"), BAD_FIXTURE);
  try {
    run(["--input", badDir]);
    console.log("[FAIL] 全坏 timings 应退 1");
    failed++;
  } catch (e) {
    const code = e.status;
    if (code === 1) console.log("[PASS] 坏 JSON 退 1");
    else { console.log(`[FAIL] 坏 JSON 应退 1，实际 ${code}`); failed++; }
  }

  fs.rmSync(tmp, { recursive: true, force: true });
  console.log("");
  if (failed > 0) {
    console.log(`cargo_timings_selftest_failed cases=5 failures=${failed}`);
    process.exit(1);
  }
  console.log("cargo_timings_selftest_ok cases=5");
  process.exit(0);
}

main();
