#!/usr/bin/env node
// gen-sbom.mjs —— 生成/校验整个 workspace 的 CycloneDX 1.5 SBOM（P2-01 ④）
//
// 输入只有 `cargo metadata` 与 `engine/Cargo.lock`，不联网、不重新解析依赖。
// 之所以自己生成而不是只依赖 cargo-cyclonedx：
//   1. 离线机器也能生成并**复核**（-Check 不需要安装第三方子命令）；
//   2. 可把 Cargo.lock 的 checksum 直接落成 CycloneDX hashes，便于下游核对字节；
//   3. 输出排序稳定，diff 可读。
// CI 仍会用固定版本 cargo-cyclonedx 再产一份作交叉校验（见 release.yml / docs/69）。
//
// 用法：
//   node scripts/release/gen-sbom.mjs --output artifacts/reports/rustlua.cdx.json
//   node scripts/release/gen-sbom.mjs --check --output artifacts/reports/rustlua.cdx.json
//   node scripts/release/gen-sbom.mjs --metadata <cargo-metadata.json> --output ...
//
// 退出码：0 = 生成/校验通过；1 = 结构或内容不一致。

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "..", "..");
const engineRoot = join(repoRoot, "engine");
const lockPath = join(engineRoot, "Cargo.lock");
const forksPath = join(engineRoot, "vendor", "FORKS.json");

const args = process.argv.slice(2);
const has = (name) => args.includes(name);
const valueOf = (name) => {
  const i = args.indexOf(name);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : null;
};
const checkOnly = has("--check");
const metadataPath = valueOf("--metadata");
const cargoConfig = valueOf("--cargo-config");
const outputPath = valueOf("--output") || join(repoRoot, "artifacts", "reports", "rustlua.cdx.json");

function fail(msg) {
  console.error(`[FAIL] ${msg}`);
  process.exitCode = 1;
}
function pass(msg) {
  console.log(`[PASS] ${msg}`);
}

function loadMetadata() {
  if (metadataPath) {
    return JSON.parse(readFileSync(resolve(repoRoot, metadataPath), "utf8"));
  }
  // --cargo-config 用于镜像环境（例如 crates.io CDN 不可达时指向 rsproxy）。
  // CI 正常网络下不需要；它只影响元数据获取，不改变 SBOM 内容。
  const args = [];
  if (cargoConfig) args.push("--config", resolve(repoRoot, cargoConfig));
  args.push("metadata", "--locked", "--format-version", "1");
  try {
    const out = execFileSync("cargo", args, {
      cwd: engineRoot,
      encoding: "utf8",
      maxBuffer: 128 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    });
    return JSON.parse(out);
  } catch (err) {
    const tail = String(err.stderr || err.message).trim().split(/\r?\n/).slice(-6).join("\n");
    throw new Error(`cargo metadata failed:\n${tail}\n(tip: pass --metadata <saved.json> for offline runs)`);
  }
}

function parseLockChecksums(text) {
  const map = new Map();
  for (const block of text.split(/\[\[package\]\]/g)) {
    const name = block.match(/^\s*name\s*=\s*"([^"]+)"/m);
    const version = block.match(/^\s*version\s*=\s*"([^"]+)"/m);
    const checksum = block.match(/^\s*checksum\s*=\s*"([0-9a-f]{64})"/m);
    if (name && version && checksum) map.set(`${name[1]}@${version[1]}`, checksum[1]);
  }
  return map;
}

function gitInfo() {
  try {
    const commit = execFileSync("git", ["rev-parse", "HEAD"], { cwd: repoRoot, encoding: "utf8" }).trim();
    const status = execFileSync("git", ["status", "--porcelain"], { cwd: repoRoot, encoding: "utf8" }).trim();
    return { commit, dirty: status.length > 0 };
  } catch {
    return { commit: "unknown", dirty: null };
  }
}

// Cargo 的旧式 `/` 分隔许可证是历史 OR 写法（SPDX 已废弃斜杠）。
// 只做这一处规范化；其余表达式原样交给 CycloneDX expression。
function normalizeLicense(raw) {
  if (!raw) return null;
  const expr = raw.replace(/\s*\/\s*/g, " OR ").replace(/\s+/g, " ").trim();
  if (/^[A-Za-z0-9.+-]+$/.test(expr)) {
    return { license: { id: expr } };
  }
  return { expression: expr };
}

function purlFor(name, version) {
  return `pkg:cargo/${encodeURIComponent(name)}@${encodeURIComponent(version)}`;
}

function loadForks() {
  if (!existsSync(forksPath)) return new Map();
  const doc = JSON.parse(readFileSync(forksPath, "utf8"));
  const map = new Map();
  for (const fork of doc.forks || []) map.set(fork.crate, fork);
  return map;
}

function componentFor(pkg, source, checksum, isWorkspace, fork) {
  const c = {
    type: isWorkspace ? "application" : "library",
    "bom-ref": purlFor(pkg.name, pkg.version),
    name: pkg.name,
    version: pkg.version,
  };
  const licenses = normalizeLicense(pkg.license);
  if (licenses) c.licenses = [licenses];
  // 描述里的换行会让下游 diff/表格难读，压成单行再截断。
  if (pkg.description) c.description = pkg.description.replace(/\s+/g, " ").trim().slice(0, 500);
  const purl = purlFor(pkg.name, pkg.version);
  c.purl = purl;
  if (checksum) c.hashes = [{ alg: "SHA-256", content: checksum }];
  c.externalReferences = [
    { type: "distribution", url: `https://crates.io/crates/${encodeURIComponent(pkg.name)}/${encodeURIComponent(pkg.version)}` },
  ];
  const props = [];
  if (pkg.license) props.push({ name: "cargo:license-declared", value: pkg.license });
  if (fork) {
    // path 依赖的 source 在 cargo metadata 里是 null；用 FORKS.json 标出这是
    // vendored fork，并记录它相对上游是否有代码改动，避免 SBOM 把 fork 当普通 path 包。
    props.push({ name: "cargo:source", value: "vendored-fork" });
    props.push({ name: "cargo:patched", value: String(fork.patched === true) });
    props.push({ name: "cargo:fork-base-version", value: String(fork.baseVersion) });
  } else if (source) {
    props.push({ name: "cargo:source", value: source });
  }
  if (isWorkspace) props.push({ name: "cargo:workspace-member", value: "true" });
  if (props.length) c.properties = props;
  return c;
}

function build(metadata, checksums, forks) {
  const workspace = new Set(metadata.workspace_members);
  const byId = new Map(metadata.packages.map((p) => [p.id, p]));
  const refById = new Map();
  for (const p of metadata.packages) {
    if (byId.has(p.id)) refById.set(p.id, purlFor(p.name, p.version));
  }

  // 同名同版本可能来自不同 source（极少数），发现冲突直接判红，避免 bom-ref 重复。
  const seenRef = new Map();
  for (const p of metadata.packages) {
    const ref = refById.get(p.id);
    if (seenRef.has(ref)) {
      throw new Error(`duplicate bom-ref ${ref} (${seenRef.get(ref)} vs ${p.id})`);
    }
    seenRef.set(ref, p.id);
  }

  const components = metadata.packages
    .map((p) => componentFor(p, p.source || "path", checksums.get(`${p.name}@${p.version}`), workspace.has(p.id), forks.get(p.name)))
    .sort((a, b) => a["bom-ref"].localeCompare(b["bom-ref"]));

  const nodes = new Map((metadata.resolve?.nodes || []).map((n) => [n.id, n]));
  const dependencies = [];
  for (const p of metadata.packages) {
    const node = nodes.get(p.id);
    const deps = (node?.deps || [])
      .map((d) => refById.get(d.pkg))
      .filter(Boolean)
      .map((r) => ({ ref: r }))
      .sort((a, b) => a.ref.localeCompare(b.ref));
    // 去重（同一依赖可能因 target/dep_kind 出现多次）
    const unique = [];
    const used = new Set();
    for (const d of deps) {
      if (used.has(d.ref)) continue;
      used.add(d.ref);
      unique.push(d);
    }
    dependencies.push({ ref: refById.get(p.id), dependsOn: unique.map((d) => d.ref) });
  }
  dependencies.sort((a, b) => a.ref.localeCompare(b.ref));

  // workspace 根：以 engine crate 的版本为准（workspace.package.version 由它继承）。
  const enginePkg = metadata.packages.find((p) => p.name === "engine" && workspace.has(p.id));
  const version = enginePkg ? enginePkg.version : "0.0.0";
  const rootRef = `pkg:generic/rustlua@${version}`;
  const git = gitInfo();
  const cargoVersion = (() => {
    try {
      return execFileSync("cargo", ["--version"], { cwd: engineRoot, encoding: "utf8" }).trim();
    } catch {
      return "unknown";
    }
  })();
  const rustcVersion = (() => {
    try {
      return execFileSync("rustc", ["--version"], { cwd: engineRoot, encoding: "utf8" }).trim();
    } catch {
      return "unknown";
    }
  })();
  // 工具版本只保留 semver 数字：`cargo 1.97.1 (hash date)` / `rustc 1.97.1 (...)`。
  const shortVersion = (raw) => {
    const m = String(raw).match(/\d+\.\d+\.\d+/);
    return m ? m[0] : String(raw);
  };
  const epoch = process.env.SOURCE_DATE_EPOCH ? Number(process.env.SOURCE_DATE_EPOCH) : null;
  const timestamp = epoch && Number.isFinite(epoch) ? new Date(epoch * 1000).toISOString() : new Date().toISOString();
  const rootDeps = metadata.workspace_members
    .map((id) => refById.get(id))
    .filter(Boolean)
    .sort();

  return {
    bomFormat: "CycloneDX",
    specVersion: "1.5",
    serialNumber: `urn:uuid:${stableUuid(`rustlua@${version}:${git.commit}`)}`,
    version: 1,
    metadata: {
      timestamp,
      tools: {
        components: [
          { type: "application", name: "rustlua-gen-sbom", version: "1.0.0" },
          { type: "application", name: "cargo", version: shortVersion(cargoVersion) },
          { type: "application", name: "rustc", version: shortVersion(rustcVersion) },
        ],
      },
      component: {
        type: "application",
        "bom-ref": rootRef,
        name: "RustLua",
        version,
        description: "Rust 2D/3D game engine with Lua gameplay runtime",
      },
      properties: [
        { name: "rustlua:git-commit", value: git.commit },
        { name: "rustlua:git-dirty", value: String(git.dirty) },
        { name: "rustlua:metadata-packages", value: String(metadata.packages.length) },
      ],
    },
    components,
    dependencies: [{ ref: rootRef, dependsOn: rootDeps }, ...dependencies],
  };
}

// UUID v5-like：用 sha256 派生固定格式，保证同一 commit 的 serialNumber 稳定。
function stableUuid(seed) {
  const h = createHash("sha256").update(seed).digest("hex");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-5${h.slice(13, 16)}-a${h.slice(17, 20)}-${h.slice(20, 32)}`;
}

function validate(sbom, metadata, checksums, forks) {
  if (sbom.bomFormat !== "CycloneDX") fail("bomFormat != CycloneDX");
  if (sbom.specVersion !== "1.5") fail(`specVersion=${sbom.specVersion}, expect 1.5`);
  const refs = new Set();
  for (const c of sbom.components || []) {
    if (!c["bom-ref"] || refs.has(c["bom-ref"])) fail(`missing/duplicate bom-ref: ${c["bom-ref"]}`);
    refs.add(c["bom-ref"]);
    if (!c.purl) fail(`${c.name}: missing purl`);
    if (c.hashes) {
      const lock = checksums.get(`${c.name}@${c.version}`);
      if (!lock) fail(`${c.name}@${c.version}: has hash but no Cargo.lock checksum`);
      else if (c.hashes[0].content !== lock) fail(`${c.name}@${c.version}: hash != Cargo.lock`);
    }
  }
  const expected = new Set(metadata.packages.map((p) => purlFor(p.name, p.version)));
  for (const ref of expected) {
    if (!refs.has(ref)) fail(`missing component ${ref}`);
  }
  const rootDeps = new Set(sbom.dependencies?.find((d) => d.ref === sbom.metadata.component["bom-ref"])?.dependsOn || []);
  for (const d of sbom.dependencies || []) {
    if (!refs.has(d.ref) && d.ref !== sbom.metadata.component["bom-ref"]) fail(`dependency ref not a component: ${d.ref}`);
    for (const child of d.dependsOn || []) {
      if (!refs.has(child)) fail(`dependsOn target not a component: ${child}`);
    }
  }
  if (rootDeps.size === 0) fail("root component has no workspace dependencies");
  // vendored fork 必须被标出来：下游看到 path 包时，至少能知道它是不是有代码改动。
  for (const [name, fork] of forks) {
    const c = (sbom.components || []).find((x) => x.name === name);
    if (!c) continue; // 例如 wasm-bindgen-cli 只在 vendor 里，不在 Cargo.lock
    const props = new Map((c.properties || []).map((p) => [p.name, p.value]));
    if (props.get("cargo:source") !== "vendored-fork") fail(`${name}: not marked as vendored-fork`);
    if (props.get("cargo:patched") !== String(fork.patched === true)) fail(`${name}: cargo:patched != ${fork.patched}`);
  }
  const hashed = (sbom.components || []).filter((c) => c.hashes?.length).length;
  pass(`components=${sbom.components.length} hashed=${hashed} dependencies=${sbom.dependencies.length} forks=${forks.size}`);
}

function main() {
  const lock = parseLockChecksums(readFileSync(lockPath, "utf8"));
  const forks = loadForks();
  if (checkOnly && !existsSync(outputPath)) {
    fail(`missing ${outputPath}; run without --check to generate it`);
    console.log("sbom_failed");
    return;
  }
  if (checkOnly) {
    const sbom = JSON.parse(readFileSync(outputPath, "utf8"));
    const metadata = loadMetadata();
    validate(sbom, metadata, lock, forks);
    if (process.exitCode) {
      console.log("sbom_failed");
    } else {
      console.log(`sbom_ok check=1 path=${outputPath}`);
    }
    return;
  }

  const metadata = loadMetadata();
  const sbom = build(metadata, lock, forks);
  validate(sbom, metadata, lock, forks);
  if (process.exitCode) {
    console.log("sbom_failed");
    return;
  }
  mkdirSync(dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, JSON.stringify(sbom, null, 2) + "\n", "utf8");
  pass(`wrote ${outputPath}`);
  console.log(`sbom_ok components=${sbom.components.length} path=${outputPath}`);
}

main();
