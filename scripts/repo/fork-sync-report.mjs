#!/usr/bin/env node
// fork-sync-report.mjs —— vendored fork 上游同步报告（P2-01 ⑦）
//
// 守护对象：`engine/vendor/FORKS.json` + `engine/Cargo.toml` 的 `[patch.crates-io]`
// + `engine/Cargo.lock` + 本地 crates.io 源码缓存。
//
// 它回答三个问题（现有 `check-vendor-patches.ps1` 只回答第 1 个）：
//   1. 补丁还在不在？（结构化断言，与 PowerShell 门互补）
//   2. 相对**同版本上游**改了哪些文件？（逐文件 SHA-256 比对本地 registry 缓存）
//   3. 上游有没有更新？（--online 查 crates.io；离线时显式标 UNKNOWN）
//
// 用法：
//   node scripts/repo/fork-sync-report.mjs --check
//   node scripts/repo/fork-sync-report.mjs --write artifacts/reports/fork-sync.md
//   node scripts/repo/fork-sync-report.mjs --online --write artifacts/reports/fork-sync.md
//
// 退出码：
//   0 = 结构一致（--check 时还要求上游源码可定位、patched fork 确有改动）
//   1 = 漂移（版本/路径/[patch] 不一致，或 patched fork 被覆盖成上游原样）
//   3 = 环境受限（--check 找不到上游源码且未给 --allow-missing-upstream；--online 网络失败）

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { get as httpsGet } from "node:https";
import { homedir } from "node:os";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "..", "..");
const engineRoot = join(repoRoot, "engine");
const vendorRoot = join(engineRoot, "vendor");
const manifestPath = join(vendorRoot, "FORKS.json");
const cargoTomlPath = join(engineRoot, "Cargo.toml");
const lockPath = join(engineRoot, "Cargo.lock");

const args = process.argv.slice(2);
const has = (name) => args.includes(name);
const valueOf = (name) => {
  const i = args.indexOf(name);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : null;
};
const checkMode = has("--check");
const online = has("--online");
const allowMissing = has("--allow-missing-upstream");
const fetchMissing = has("--fetch-missing");
const writePath = valueOf("--write");

const failures = [];
const warnings = [];
const blocked = [];
function pass(msg) {
  console.log(`[PASS] ${msg}`);
}
function fail(msg) {
  failures.push(msg);
  console.log(`[FAIL] ${msg}`);
}
function warn(msg) {
  warnings.push(msg);
  console.log(`[WARN] ${msg}`);
}
function block(msg) {
  blocked.push(msg);
  console.log(`[BLOCKED] ${msg}`);
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8").replace(/^\uFEFF/, ""));
}

function parseTomlPatches(text) {
  const lines = text.split(/\r?\n/);
  const patches = new Map();
  let inPatch = false;
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith("[")) {
      inPatch = trimmed === "[patch.crates-io]";
      continue;
    }
    if (!inPatch) continue;
    const m = trimmed.match(/^([A-Za-z0-9_-]+)\s*=\s*\{\s*path\s*=\s*"([^"]+)"/);
    if (m) patches.set(m[1], m[2].replace(/\\/g, "/"));
  }
  return patches;
}

function parseLock(text) {
  const map = new Map();
  for (const block of text.split(/\[\[package\]\]/g)) {
    const name = block.match(/^\s*name\s*=\s*"([^"]+)"/m);
    const version = block.match(/^\s*version\s*=\s*"([^"]+)"/m);
    const source = block.match(/^\s*source\s*=\s*"([^"]+)"/m);
    if (name && version) {
      map.set(name[1], { version: version[1], source: source ? source[1] : null });
    }
  }
  return map;
}

function readPackageVersion(dir, versionFile) {
  const p = join(dir, versionFile);
  if (!existsSync(p)) return "";
  const m = readFileSync(p, "utf8").match(/^\s*version\s*=\s*"([^"]+)"/m);
  return m ? m[1] : "";
}

function cargoHome() {
  return process.env.CARGO_HOME || join(homedir(), ".cargo");
}

// 在 <CARGO_HOME>/registry/src/<index>/<crate>-<version> 里找同版本上游源码。
// 不下载、不联网；找不到就返回 null（由调用方决定 BLOCKED 还是 WARN）。
function findUpstreamSource(crate, version) {
  const srcRoot = join(cargoHome(), "registry", "src");
  if (!existsSync(srcRoot)) return null;
  const want = `${crate}-${version}`;
  for (const indexDir of readdirSync(srcRoot, { withFileTypes: true })) {
    if (!indexDir.isDirectory()) continue;
    const candidate = join(srcRoot, indexDir.name, want);
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

function readTarString(buf, start, len) {
  const slice = buf.subarray(start, start + len);
  const nul = slice.indexOf(0);
  return slice.subarray(0, nul < 0 ? len : nul).toString("utf8");
}

// 只实现 `.crate` 用得到的 tar 子集：ustar 头、普通文件、目录。
// 不调用外部 tar（Windows 自带 bsdtar 的行为随版本变化），解压时拒绝绝对路径和 `..`。
function extractCrateTarball(buf, crate, version) {
  let tar;
  try {
    tar = gunzipSync(buf);
  } catch (err) {
    throw new Error(`gunzip failed: ${err.message}`);
  }
  const dest = join(cargoHome(), "registry", "src", "fork-upstream", `${crate}-${version}`);
  mkdirSync(dest, { recursive: true });
  let off = 0;
  let files = 0;
  while (off + 512 <= tar.length) {
    const header = tar.subarray(off, off + 512);
    if (header.every((b) => b === 0)) break;
    const name = readTarString(header, 0, 100);
    const sizeText = readTarString(header, 124, 12).trim();
    const size = parseInt(sizeText || "0", 8);
    if (!Number.isFinite(size) || size < 0) throw new Error(`bad tar size in ${crate}`);
    const type = String.fromCharCode(header[156] || 48);
    const prefix = readTarString(header, 345, 155);
    const full = prefix ? `${prefix}/${name}` : name;
    const dataStart = off + 512;
    // `.crate` 的第一段是 <crate>-<version>/，去掉它再落到目标目录。
    const rel = full.replace(/^[^/]+\//, "");
    if (rel && !rel.startsWith("/") && !rel.split("/").includes("..")) {
      if (type === "5") {
        mkdirSync(join(dest, rel), { recursive: true });
      } else if (type === "0" || type === "\0" || type === "7") {
        mkdirSync(dirname(join(dest, rel)), { recursive: true });
        writeFileSync(join(dest, rel), tar.subarray(dataStart, dataStart + size));
        files++;
      }
    }
    off = dataStart + Math.ceil(size / 512) * 512;
  }
  if (files === 0) throw new Error(`tar contained no regular files for ${crate}-${version}`);
  return dest;
}

function downloadCrate(crate, version, redirects = 0) {
  return new Promise((resolvePromise, reject) => {
    if (redirects > 5) {
      reject(new Error("too many redirects"));
      return;
    }
    const url = `https://static.crates.io/crates/${crate}/${crate}-${version}.crate`;
    const req = httpsGet(url, { headers: { "User-Agent": "rustlua-fork-sync-report/1" } }, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        res.resume();
        downloadCrate(crate, version, redirects + 1).then(resolvePromise, reject);
        return;
      }
      if (res.statusCode !== 200) {
        res.resume();
        reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        return;
      }
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => resolvePromise(Buffer.concat(chunks)));
      res.on("error", reject);
    });
    req.setTimeout(30000, () => req.destroy(new Error("timeout")));
    req.on("error", reject);
  });
}

async function fetchUpstreamSource(crate, version) {
  const existing = findUpstreamSource(crate, version);
  if (existing) return existing;
  const buf = await downloadCrate(crate, version);
  return extractCrateTarball(buf, crate, version);
}

function walkFiles(root, base = root, out = []) {
  for (const ent of readdirSync(root, { withFileTypes: true })) {
    if (ent.name === ".git" || ent.name === "target" || ent.name === ".cargo-ok") continue;
    const full = join(root, ent.name);
    if (ent.isDirectory()) {
      walkFiles(full, base, out);
    } else if (ent.isFile()) {
      out.push(relative(base, full).split(sep).join("/"));
    }
  }
  return out;
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function textLines(path) {
  try {
    const buf = readFileSync(path);
    if (buf.subarray(0, Math.min(buf.length, 8192)).includes(0)) return 0;
    const text = buf.toString("utf8");
    if (!text) return 0;
    return text.split(/\r?\n/).length - (text.endsWith("\n") ? 1 : 0);
  } catch {
    return 0;
  }
}

function diffTrees(vendorDir, upstreamDir) {
  const vFiles = walkFiles(vendorDir);
  const uFiles = walkFiles(upstreamDir);
  const vSet = new Set(vFiles);
  const uSet = new Set(uFiles);
  const added = [];
  const removed = [];
  const modified = [];
  for (const rel of vFiles) {
    if (!uSet.has(rel)) {
      added.push({ path: rel, lines: textLines(join(vendorDir, rel)) });
      continue;
    }
    const a = join(vendorDir, rel);
    const b = join(upstreamDir, rel);
    if (sha256(a) !== sha256(b)) {
      modified.push({ path: rel, lines: textLines(a) });
    }
  }
  for (const rel of uFiles) {
    if (!vSet.has(rel)) {
      removed.push({ path: rel, lines: textLines(join(upstreamDir, rel)) });
    }
  }
  const byPath = (a, b) => a.path.localeCompare(b.path);
  added.sort(byPath);
  removed.sort(byPath);
  modified.sort(byPath);
  return { added, removed, modified, files: vFiles.length, upstreamFiles: uFiles.length };
}

function semverParts(v) {
  const m = String(v).match(/^(\d+)\.(\d+)\.(\d+)/);
  return m ? m.slice(1).map(Number) : null;
}
function bumpKind(from, to) {
  const a = semverParts(from);
  const b = semverParts(to);
  if (!a || !b) return "unknown";
  if (b[0] !== a[0]) return "major";
  if (b[1] !== a[1]) return "minor";
  if (b[2] !== a[2]) return "patch";
  return "same";
}

async function fetchLatest(crate) {
  const url = `https://crates.io/api/v1/crates/${encodeURIComponent(crate)}`;
  const res = await fetch(url, {
    headers: { "User-Agent": "rustlua-fork-sync/1.0 (https://github.com/Rweifeng/RustLua)" },
    signal: AbortSignal.timeout(30000),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const json = await res.json();
  return json.crate?.max_stable_version || json.crate?.newest_version || "";
}

function renderMarkdown(report) {
  const lines = [];
  lines.push("# Vendored fork 上游同步报告");
  lines.push("");
  lines.push(`- 生成时间：${report.generatedAt}`);
  lines.push(`- 仓库：${report.repoRoot}`);
  lines.push(`- rustc：${report.rustc}`);
  lines.push(`- 结论：${report.ok ? "结构一致" : "存在漂移"}${report.blocked ? "（有 BLOCKED 项）" : ""}`);
  lines.push("");
  lines.push("| crate | vendored | 上游基线 | 上游源码 | 新增 | 修改 | 删除 | 在线最新 | 漂移 |");
  lines.push("|---|---|---|---|---:|---:|---:|---|---|");
  for (const f of report.forks) {
    const d = f.diff;
    lines.push(
      `| \`${f.crate}\` | ${f.baseVersion} | ${f.upstreamRef} | ${f.upstreamSource === "ok" ? "local" : f.upstreamSource} | ` +
        `${d ? d.added.length : "-"} | ${d ? d.modified.length : "-"} | ${d ? d.removed.length : "-"} | ` +
        `${f.latest || "UNKNOWN"} | ${f.bump || "-"} |`,
    );
  }
  lines.push("");
  lines.push("## 逐 fork 改动足迹");
  for (const f of report.forks) {
    lines.push("");
    lines.push(`### \`${f.crate}\` (${f.baseVersion}, patched=${f.patched})`);
    lines.push("");
    lines.push(`- 上游：${f.upstreamRepo} @ ${f.upstreamRef}`);
    lines.push(`- 用途：${f.reason}`);
    if (!f.diff) {
      lines.push(`- 差异：**未比对**（${f.upstreamSource}）`);
      continue;
    }
    lines.push(`- 文件数：vendored ${f.diff.files} / upstream ${f.diff.upstreamFiles}`);
    for (const [label, list] of [
      ["新增", f.diff.added],
      ["修改", f.diff.modified],
      ["删除", f.diff.removed],
    ]) {
      if (list.length === 0) continue;
      lines.push(`- ${label}（${list.length}）：`);
      for (const item of list.slice(0, 30)) {
        lines.push(`  - \`${item.path}\`${item.lines ? ` (+${item.lines} 行)` : ""}`);
      }
      if (list.length > 30) lines.push(`  - … 其余 ${list.length - 30} 条见 JSON`);
    }
  }
  if (report.warnings.length) {
    lines.push("");
    lines.push("## 警告");
    for (const w of report.warnings) lines.push(`- ${w}`);
  }
  if (report.blocked.length) {
    lines.push("");
    lines.push("## 未比对 / 环境受限");
    for (const b of report.blocked) lines.push(`- ${b}`);
  }
  lines.push("");
  lines.push("> 判据说明：`patched=true` 的 fork 相对同版本上游必须至少有 1 处改动；");
  lines.push("> `0 改动` 说明 vendor 目录可能被上游原样覆盖，补丁已丢（--check 判红）。");
  lines.push("> 在线最新版只作提示，不自动判红；升级由人决定并跑完整 wasm/平台门。");
  return lines.join("\n") + "\n";
}

async function main() {
  if (!existsSync(manifestPath)) {
    console.error(`[FAIL] missing ${manifestPath}`);
    process.exit(1);
  }
  if (!existsSync(cargoTomlPath) || !existsSync(lockPath)) {
    console.error(`[FAIL] missing engine/Cargo.toml or Cargo.lock`);
    process.exit(1);
  }

  const manifest = readJson(manifestPath);
  const patches = parseTomlPatches(readFileSync(cargoTomlPath, "utf8"));
  const lock = parseLock(readFileSync(lockPath, "utf8"));
  const rustcOut = (() => {
    try {
      return execFileSyncText("rustc", ["--version"]);
    } catch {
      return "unknown";
    }
  })();

  console.log(`==> fork manifest: ${manifest.forks.length} entries; [patch.crates-io]: ${patches.size}`);
  const forks = [];
  const seen = new Set();

  for (const fork of manifest.forks) {
    const vendorDir = join(vendorRoot, fork.path.replace(/^vendor[\\/]/, ""));
    const label = `${fork.crate} ${fork.baseVersion}`;
    seen.add(fork.crate);

    if (!existsSync(vendorDir)) {
      fail(`${label}: vendor path missing (${fork.path})`);
      forks.push({ ...fork, upstreamSource: "vendor-missing", diff: null });
      continue;
    }
    const vv = readPackageVersion(vendorDir, fork.versionFile || "Cargo.toml");
    if (vv !== fork.baseVersion) {
      fail(`${label}: ${fork.versionFile} version=${vv || "<missing>"} != manifest ${fork.baseVersion}`);
    } else {
      pass(`${label}: vendor version file matches manifest`);
    }
    const locked = lock.get(fork.crate);
    if (!locked && fork.inLock === false) {
      pass(`${label}: not in Cargo.lock by design (tool-only fork)`);
    } else if (!locked) {
      fail(`${label}: not found in engine/Cargo.lock`);
    } else if (locked.version !== fork.baseVersion) {
      fail(`${label}: Cargo.lock version=${locked.version} != manifest ${fork.baseVersion}`);
    } else {
      pass(`${label}: Cargo.lock version matches`);
    }
    if (fork.patched) {
      const patchPath = patches.get(fork.crate);
      if (!patchPath) {
        fail(`${label}: patched fork has no [patch.crates-io] entry`);
      } else if (patchPath !== fork.path) {
        fail(`${label}: [patch] path=${patchPath} != manifest ${fork.path}`);
      } else {
        pass(`${label}: [patch.crates-io] path matches`);
      }
    }

    // `inLock:false` 且 `patched:false` 的是"工具源码"（wasm-bindgen-cli）：
    // 它不是 workspace 依赖，cargo 不会把它下载到 registry/src；要求上游源码
    // 会让干净的 CI 永远 BLOCKED。未打补丁的工具没有"补丁足迹"要证明，
    // 因此不比对上游，但仍校验 vendor 版本与清单一致。
    const toolOnlyUnpatched = !locked && fork.inLock === false && !fork.patched;
    let diff = null;
    let upstreamSource = "missing";
    if (toolOnlyUnpatched) {
      upstreamSource = "not-applicable";
      pass(`${label}: tool-only unpatched fork; upstream diff not required`);
    } else {
      let upstream = findUpstreamSource(fork.crate, fork.baseVersion);
      if (!upstream && fetchMissing) {
        try {
          upstream = await fetchUpstreamSource(fork.crate, fork.baseVersion);
          pass(`${label}: fetched upstream ${fork.crate}-${fork.baseVersion} from crates.io`);
        } catch (err) {
          warn(`${label}: upstream fetch failed (${err.message})`);
        }
      }
      if (!upstream) {
        if (checkMode && !allowMissing) {
          block(`${label}: upstream source for ${fork.crate}-${fork.baseVersion} not in local registry cache (try --fetch-missing)`);
        } else {
          warn(`${label}: upstream source missing; diff not computed`);
        }
      } else {
        upstreamSource = "ok";
        diff = diffTrees(vendorDir, upstream);
        pass(`${label}: diff vs upstream added=${diff.added.length} modified=${diff.modified.length} removed=${diff.removed.length}`);
        if (fork.patched && diff.added.length + diff.modified.length + diff.removed.length === 0) {
          fail(`${label}: patched fork is byte-identical to upstream (patch footprint lost)`);
        }
      }
    }

    forks.push({ ...fork, upstreamSource, diff, latest: null, bump: null });
  }

  for (const crate of patches.keys()) {
    if (!seen.has(crate)) {
      fail(`[patch.crates-io] ${crate} has no entry in FORKS.json`);
    }
  }
  for (const fork of manifest.forks.filter((f) => f.patched)) {
    if (!patches.has(fork.crate)) fail(`${fork.crate}: patched=true but absent from [patch.crates-io]`);
  }

  if (online) {
    console.log("==> querying crates.io for latest stable versions");
    for (const f of forks) {
      try {
        const latest = await fetchLatest(f.crate);
        f.latest = latest;
        f.bump = latest ? bumpKind(f.baseVersion, latest) : "unknown";
        console.log(`[INFO] ${f.crate}: ${f.baseVersion} -> ${latest || "UNKNOWN"} (${f.bump})`);
      } catch (err) {
        block(`${f.crate}: crates.io query failed (${err.message})`);
      }
    }
  }

  const report = {
    schema: 1,
    generatedAt: new Date().toISOString(),
    repoRoot,
    rustc: rustcOut,
    ok: failures.length === 0,
    warnings,
    blocked,
    forks,
  };

  if (writePath) {
    const out = resolve(repoRoot, writePath);
    mkdirSync(dirname(out), { recursive: true });
    const jsonPath = out.toLowerCase().endsWith(".md") ? out.replace(/\.md$/i, ".json") : out + ".json";
    const mdPath = out.toLowerCase().endsWith(".md") ? out : out + ".md";
    writeFileSync(jsonPath, JSON.stringify(report, null, 2) + "\n", "utf8");
    writeFileSync(mdPath, renderMarkdown(report), "utf8");
    console.log(`[INFO] wrote ${relative(repoRoot, mdPath)} and ${relative(repoRoot, jsonPath)}`);
  }

  console.log("");
  console.log(`fork sync: ${failures.length} failed, ${warnings.length} warnings, ${blocked.length} blocked`);
  if (failures.length > 0) {
    console.log("fork_sync_failed");
    process.exit(1);
  }
  if (blocked.length > 0) {
    console.log("fork_sync_blocked");
    process.exit(3);
  }
  console.log(`fork_sync_ok forks=${forks.length} patched=${forks.filter((f) => f.patched).length}`);
  process.exit(0);
}

function execFileSyncText(cmd, argv) {
  return execFileSync(cmd, argv, { encoding: "utf8" }).trim();
}

main().catch((err) => {
  console.error(`[FAIL] ${err.stack || err.message}`);
  process.exit(1);
});
