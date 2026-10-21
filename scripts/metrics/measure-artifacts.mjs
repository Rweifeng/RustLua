#!/usr/bin/env node
/**
 * measure-artifacts.mjs —— 二进制与资源包大小趋势（P4-06/07）。
 *
 * --kind binary：game/editor/assetc exe + game_web wasm/js；bytes_raw/gzip9/brotli11/sha256/
 *    mtime/strip 状态；llvm-size 可用时记录 .text/.rdata/.data 分节。
 * --kind package：PC / 引擎绿色版 / WebGL / 微信主包+分包 / 资源容器；配额余量。
 *
 * 用法：
 *   node scripts/metrics/measure-artifacts.mjs --kind binary --out artifacts/metrics/binaries.json
 *   node scripts/metrics/measure-artifacts.mjs --kind package --project examples/game_jhyt --out artifacts/metrics/packages.json
 *
 * 退出码：0 = 通过；1 = -RequireAll 时缺文件 / 解析失败；缺文件默认标 missing + partial=true，
 * 不能拿 partial 更新基线。
 */

import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.dirname(path.dirname(__dirname));

function sha256(p) {
  return crypto.createHash("sha256").update(fs.readFileSync(p)).digest("hex");
}

function dirBytes(dir) {
  let total = 0, files = 0;
  for (const f of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, f.name);
    if (f.isDirectory()) { const r = dirBytes(p); total += r.total; files += r.files; }
    else { total += fs.statSync(p).size; files += 1; }
  }
  return { total, files };
}

function measureFile(p) {
  const st = fs.statSync(p);
  const raw = fs.readFileSync(p);
  return {
    bytes_raw: raw.length,
    bytes_gzip9: zlib.gzipSync(raw, { level: 9 }).length,
    bytes_brotli11: zlib.brotliCompressSync(raw, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11 } }).length,
    sha256: crypto.createHash("sha256").update(raw).digest("hex"),
    mtime: st.mtime.toISOString(),
  };
}

function llvmSizeSections(exe) {
  // llvm-size 可用时记录分节；不可用打印 unavailable，不伪造 0
  try {
    const out = execFileSync("llvm-size", ["-A", exe], { encoding: "utf8", timeout: 20000 });
    const sec = {};
    for (const line of out.split(/\r?\n/)) {
      const m = line.match(/^\s*\[?\.(text|rdata|data)\s+(\d+)/);
      if (m) sec[m[1]] = Number(m[2]);
    }
    return Object.keys(sec).length ? sec : { unavailable: "llvm-size", reason: "no text/rdata/data rows" };
  } catch (e) {
    return { unavailable: "llvm-size", reason: e.message.split("\n")[0].slice(0, 80) };
  }
}

function collectBinary(binRoot) {
  const artifacts = [];
  const targets = [
    { name: "game.exe", rel: "release/game.exe", kind: "exe", expect: true },
    { name: "editor.exe", rel: "release/editor.exe", kind: "exe", expect: true },
    { name: "assetc.exe", rel: "release/assetc.exe", kind: "exe", expect: true },
    { name: "game_web.wasm", rel: "wasm32-unknown-unknown/release/game_web.wasm", kind: "wasm", expect: false },
    // wasm-bindgen 的 bg.wasm/js 由 package 脚本生成并落在 WebGL 包内；cargo target 里没有时回退到发布包，
    // 仍按真实发布字节计量（P4-06 五件产物）。
    { name: "game_web_bg.wasm", rel: "wasm32-unknown-unknown/release/game_web_bg.wasm", kind: "wasm", expect: false,
      fallback: "examples/game_jhyt/builds/webgl/pkg/game_web_bg.wasm" },
    { name: "game_web.js", rel: "wasm32-unknown-unknown/release/game_web.js", kind: "js", expect: false,
      fallback: "examples/game_jhyt/builds/webgl/pkg/game_web.js" },
  ];
  let missing = 0;
  for (const t of targets) {
    let p = path.join(binRoot, t.rel);
    let shownPath = t.rel;
    if (!fs.existsSync(p) && t.fallback) {
      const fb = path.join(ROOT, t.fallback);
      if (fs.existsSync(fb)) { p = fb; shownPath = t.fallback; }
    }
    if (!fs.existsSync(p)) {
      if (t.expect) missing++;
      artifacts.push({ name: t.name, kind: t.kind, path: t.rel, missing: true });
      continue;
    }
    const m = measureFile(p);
    const art = {
      name: t.name, kind: t.kind, path: shownPath, profile: "release", strip: false,
      ...m,
    };
    if (t.kind === "exe") {
      art.sections = llvmSizeSections(p);
      // Windows 发布构建通常不 strip；若后续引入 strip 在此标记
    }
    artifacts.push(art);
  }
  return { artifacts, missing };
}

function collectPackage(projectDir) {
  const packages = [];
  const pcDir = path.join(projectDir, "builds", "pc");
  const webglDir = path.join(projectDir, "builds", "webgl");
  const wechatDir = path.join(projectDir, "builds", "wechat-minigame");
  let partial = false;

  // PC
  if (fs.existsSync(pcDir)) {
    const r = dirBytes(pcDir);
    packages.push({ platform: "pc", name: "pc", form: "full", path: "builds/pc", bytes_total: r.total, files: r.files });
  } else partial = true;

  // 引擎绿色版（artifacts/packages/RustLua-Engine-*）
  const engineDir = path.join(ROOT, "artifacts", "packages");
  if (fs.existsSync(engineDir)) {
    for (const d of fs.readdirSync(engineDir, { withFileTypes: true })) {
      if (!d.isDirectory() || !d.name.startsWith("RustLua-Engine-")) continue;
      const r = dirBytes(path.join(engineDir, d.name));
      let fonts = 0, samples = 0;
      const walk = (dir) => {
        for (const f of fs.readdirSync(dir, { withFileTypes: true })) {
          const p = path.join(dir, f.name);
          if (f.isDirectory()) walk(p);
          else if (/\.(ttf|otf)$/i.test(f.name)) fonts += fs.statSync(p).size;
          else if (/(samples|fonts)/i.test(path.relative(path.join(engineDir, d.name), p))) samples += fs.statSync(p).size;
        }
      };
      walk(path.join(engineDir, d.name));
      packages.push({ platform: "engine", name: d.name, form: "full", path: "artifacts/packages/" + d.name, bytes_total: r.total, files: r.files, bytes_fonts: fonts, bytes_samples: samples });
    }
  } else partial = true;

  // WebGL
  if (fs.existsSync(webglDir)) {
    const r = dirBytes(webglDir);
    const pkgDir = path.join(webglDir, "pkg");
    let bytesWasm = null, bytesJs = null, bytesFonts = null, bytesPack = null;
    if (fs.existsSync(pkgDir)) {
      for (const f of fs.readdirSync(pkgDir)) {
        const p = path.join(pkgDir, f);
        if (f.endsWith(".wasm")) bytesWasm = fs.statSync(p).size;
        if (f.endsWith(".js")) bytesJs = fs.statSync(p).size;
      }
    }
    const packDir = path.join(webglDir, "pack");
    if (fs.existsSync(packDir)) bytesPack = dirBytes(packDir).total;
    const fonts = [];
    const collect = (dir) => {
      if (!fs.existsSync(dir)) return;
      for (const f of fs.readdirSync(dir, { withFileTypes: true })) {
        const p = path.join(dir, f.name);
        if (f.isDirectory()) collect(p);
        else if (/\.(ttf|otf)$/i.test(f.name)) fonts.push(fs.statSync(p).size);
      }
    };
    collect(webglDir);
    bytesFonts = fonts.reduce((a, b) => a + b, 0);
    packages.push({ platform: "webgl", name: "webgl", form: "full", path: "builds/webgl", bytes_total: r.total, files: r.files, bytes_wasm: bytesWasm, bytes_js: bytesJs, bytes_fonts: bytesFonts, bytes_pack: bytesPack });
  } else partial = true;

  // 微信：主包/分包/总包（game.json 口径）
  const launcherDir = path.join(wechatDir, "launcher");
  if (fs.existsSync(launcherDir) && fs.existsSync(path.join(launcherDir, "game.json"))) {
    const game = JSON.parse(fs.readFileSync(path.join(launcherDir, "game.json"), "utf8"));
    const totalR = dirBytes(launcherDir);
    const mainBytes = totalR.total;
    const subs = [];
    for (const sp of game.subpackages || []) {
      const root = sp.root || sp.name;
      const p = path.join(launcherDir, root);
      if (fs.existsSync(p)) {
        const r = dirBytes(p);
        subs.push({ name: root, bytes: r.total, files: r.files });
      }
    }
    // 主包 = 总 - 分包（微信口径：game.json 所在目录扣除分包根）
    const subBytes = subs.reduce((a, b) => a + b.bytes, 0);
    const main = Math.max(0, mainBytes - subBytes);
    packages.push({
      platform: "wechat", name: "main", form: "subpackage", path: "builds/wechat-minigame/launcher",
      bytes_main: main, bytes_sub: subBytes, bytes_total: mainBytes,
      main_of_limit: (main / (4 * 1024 * 1024)) * 100,
      total_of_limit: (mainBytes / (20 * 1024 * 1024)) * 100,
      limits: { main_bytes: 4 * 1024 * 1024, total_bytes: 20 * 1024 * 1024 },
    });
    for (const s of subs) {
      packages.push({ platform: "wechat", name: "sub|" + s.name, form: "subpackage", path: "builds/wechat-minigame/launcher/" + s.name, bytes_total: s.bytes, files: s.files });
    }
  } else partial = true;

  // 资源容器 .rlb
  const rlbs = [];
  const findRlb = (dir) => {
    if (!fs.existsSync(dir)) return;
    for (const f of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, f.name);
      if (f.isDirectory()) findRlb(p);
      else if (f.name.endsWith(".rlb")) rlbs.push({ name: f.name, bytes: fs.statSync(p).size });
    }
  };
  findRlb(path.join(ROOT, "artifacts"));
  findRlb(projectDir);
  for (const rlb of rlbs) {
    packages.push({ platform: "rlb", name: rlb.name, form: "resource", bytes_total: rlb.bytes, path: "?" });
  }

  return { packages, partial };
}

function main() {
  const args = process.argv.slice(2);
  const opt = { kind: "", out: "", project: "", requireAll: false };
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--kind") opt.kind = args[++i];
    else if (args[i] === "--out") opt.out = args[++i];
    else if (args[i] === "--project") opt.project = args[++i];
    else if (args[i] === "--require-all" || args[i] === "-RequireAll") opt.requireAll = true;
  }
  if (!opt.kind || !["binary", "package"].includes(opt.kind)) {
    console.error("measure_artifacts_failed reason=--kind 需要 binary 或 package");
    process.exit(1);
  }

  let payload;
  let summary;
  if (opt.kind === "binary") {
    const binRoot = path.join(ROOT, "engine", "target");
    const r = collectBinary(binRoot);
    const missingNames = r.artifacts.filter((a) => a.missing).map((a) => a.name);
    const partial = r.missing > 0;
    if (opt.requireAll && r.missing > 0) {
      console.error(`measure_artifacts_failed reason=missing_required artifacts=${missingNames.join(",")}`);
      process.exit(1);
    }
    payload = { schema: "binary-v1", generated_at: new Date().toISOString(), profile: "release", partial, artifacts: r.artifacts };
    summary = r.artifacts.filter((a) => !a.missing).map((a) => `${a.name}=${a.bytes_raw}`).join(" ") + (partial ? " partial=true" : "");
  } else {
    const projectDir = opt.project
      ? (path.isAbsolute(opt.project) ? opt.project : path.join(ROOT, opt.project))
      : path.join(ROOT, "examples", "game_jhyt");
    if (!fs.existsSync(projectDir)) {
      console.error("measure_artifacts_failed reason=project_dir_missing dir=" + projectDir);
      process.exit(1);
    }
    const r = collectPackage(projectDir);
    payload = { schema: "package-v1", generated_at: new Date().toISOString(), project: opt.project || "examples/game_jhyt", partial: r.partial, packages: r.packages };
    const w = r.packages.find((p) => p.platform === "wechat" && p.name === "main");
    summary = `pc=${(r.packages.find((p) => p.platform === "pc") || {}).bytes_total || "missing"} webgl=${(r.packages.find((p) => p.platform === "webgl") || {}).bytes_total || "missing"} wechat_main=${w ? w.bytes_main : "missing"} wechat_sub=${w ? w.bytes_sub : "missing"} total=${w ? w.bytes_total : "missing"} quota_ok=${w ? (w.main_of_limit <= 100 && w.total_of_limit <= 100 ? 1 : 0) : "n/a"}`;
  }

  if (opt.out) {
    const outPath = path.isAbsolute(opt.out) ? opt.out : path.join(ROOT, opt.out);
    fs.mkdirSync(path.dirname(outPath), { recursive: true });
    fs.writeFileSync(outPath, JSON.stringify(payload, null, 2));
  }
  const tag = opt.kind === "binary" ? "binaries_ok" : "packages_ok";
  console.log(`${tag} ${summary}`);
  process.exit(0);
}

main();
