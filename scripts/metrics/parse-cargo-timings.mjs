#!/usr/bin/env node
/**
 * parse-cargo-timings.mjs —— 解析 cargo --timings 的 HTML（P4-05）。
 *
 * 从 target/cargo-timings/cargo-timing*.html 提取 UNIT_DATA / CONCURRENCY_DATA，
 * 算 wall_ms / critical_path_ms / parallelism / per-crate duration / incremental_units。
 *
 * 用法：
 *   node scripts/metrics/parse-cargo-timings.mjs --input <html|dir> --out compile-timings.json
 *   node scripts/metrics/parse-cargo-timings.mjs --check            # 输出 compile_ok（默认扫 artifacts）
 *
 * 退出码：0 = 通过；1 = 无 timings / 坏 JSON。
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

function parseTimings(html) {
  const unitMatch = html.match(/const\s+UNIT_DATA\s*=\s*(\[[\s\S]*?\])\s*;/);
  const concMatch = html.match(/const\s+CONCURRENCY_DATA\s*=\s*(\[[\s\S]*?\])\s*;/);
  if (!unitMatch) throw new Error("UNIT_DATA 未找到");
  let units;
  try {
    units = JSON.parse(unitMatch[1]);
  } catch (e) {
    throw new Error("UNIT_DATA 不是合法 JSON: " + e.message);
  }
  let concurrency = [];
  if (concMatch) {
    try { concurrency = JSON.parse(concMatch[1]); } catch { concurrency = []; }
  }
  // UNIT_DATA 行：[name, duration_ms, start_ms, ...]
  const parsed = units.map((u) => ({
    name: String(u[0]),
    duration_ms: Number(u[1]),
    start_ms: Number(u[2]),
  }));
  // per-crate 聚合：name 形如 `crate::unit path (profile)`
  const crates = new Map();
  for (const u of parsed) {
    const crate = String(u.name).split("::")[0];
    if (!crates.has(crate)) crates.set(crate, { duration_ms: 0, units: 0 });
    const c = crates.get(crate);
    c.duration_ms += u.duration_ms;
    c.units += 1;
  }
  const wallMs = concurrency.length
    ? Math.max(...concurrency.map((r) => Number(r[0]) || 0))
    : parsed.reduce((mx, u) => Math.max(mx, u.start_ms + u.duration_ms), 0);
  const criticalMs = parsed.reduce((mx, u) => Math.max(mx, u.start_ms + u.duration_ms), 0);
  const parallelism = concurrency.length
    ? Math.max(...concurrency.map((r) => Number(r[1]) || 0))
    : 1;
  const topCrate = [...crates.entries()].sort((a, b) => b[1].duration_ms - a[1].duration_ms)[0];
  return {
    wall_ms: Math.round(wallMs),
    critical_path_ms: Math.round(criticalMs),
    parallelism,
    unit_count: parsed.length,
    top_crate: topCrate ? topCrate[0] : null,
    top_crate_ms: topCrate ? Math.round(topCrate[1].duration_ms) : 0,
    crates: Object.fromEntries(
      [...crates.entries()].sort((a, b) => b[1].duration_ms - a[1].duration_ms).map(([k, v]) => [k, v]),
    ),
  };
}

function findTimings(input) {
  if (fs.statSync(input).isDirectory()) {
    return fs.readdirSync(input)
      .filter((f) => f.startsWith("cargo-timing") && f.endsWith(".html"))
      .map((f) => path.join(input, f))
      .sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
  }
  return [input];
}

function main() {
  const args = process.argv.slice(2);
  const opt = { input: "", out: "", check: false };
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--input") opt.input = args[++i];
    else if (args[i] === "--out") opt.out = args[++i];
    else if (args[i] === "--check") opt.check = true;
  }
  const projectRoot = path.dirname(path.dirname(__dirname));
  const input = opt.input || path.join(projectRoot, "artifacts", "metrics", "target-compile", "cargo-timings");
  if (!fs.existsSync(input)) {
    console.error("parse_cargo_timings_blocked reason=no_timings dir=" + input);
    process.exit(1);
  }
  const files = findTimings(input);
  if (files.length === 0) {
    console.error("parse_cargo_timings_blocked reason=no_timing_html dir=" + input);
    process.exit(1);
  }
  let latest = null;
  for (const f of files) {
    const html = fs.readFileSync(f, "utf8");
    try {
      const r = parseTimings(html);
      if (!latest || r.wall_ms > latest.wall_ms) latest = { file: f, ...r };
    } catch (e) {
      // 跳过坏文件；全部坏才判 1
    }
  }
  if (!latest) {
    console.error("parse_cargo_timings_failed reason=all_timings_unparsable");
    process.exit(1);
  }
  if (opt.out) {
    const outPath = path.isAbsolute(opt.out) ? opt.out : path.join(projectRoot, opt.out);
    fs.mkdirSync(path.dirname(outPath), { recursive: true });
    fs.writeFileSync(outPath, JSON.stringify({ schema: "compile-timings-v1", ...latest }, null, 2));
  }
  console.log(
    `compile_ok units=${latest.unit_count} wall_ms=${latest.wall_ms} critical_ms=${latest.critical_path_ms} parallelism=${latest.parallelism} top_crate=${latest.top_crate} top_crate_ms=${latest.top_crate_ms} source=${path.basename(latest.file)}`,
  );
  process.exit(0);
}

main();
