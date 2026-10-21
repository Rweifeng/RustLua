// cfg-inventory.mjs —— wasm/平台条件编译清单扫描器（P4-02）
//
// 扫描 engine/crates/**/*.rs，提取 #[cfg(...)]、#[cfg_attr(...)] 与 cfg!(...)，
// 先剥掉行注释/块注释/字符串/字符/raw 字符串字面量，避免把文档示例算成分支。
//
// 用法：
//   node scripts/metrics/cfg-inventory.mjs                  # 生成到 metrics/inventory/cfg-sites.json
//   node scripts/metrics/cfg-inventory.mjs --check          # 重新生成并与跟踪文件逐 key 比对
//   node scripts/metrics/cfg-inventory.mjs --md             # 额外写 artifacts/reports/cfg-inventory.md
//
// 稳定 key（不含行号，抗格式化噪音）：
//   crate|file|predicate|item|occurrence
//
// 退出码：0 = 一致/生成成功；1 = --check 时清单漂移，或扫描失败。

import { readFileSync, writeFileSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { join, relative, sep, dirname } from "node:path";

const ROOT = process.cwd();
const ENGINE = join(ROOT, "engine");
const CRATES = join(ENGINE, "crates");
const OUT = join(ROOT, "metrics", "inventory", "cfg-sites.json");
const MD = join(ROOT, "artifacts", "reports", "cfg-inventory.md");
const CHECK = process.argv.includes("--check");
const WANT_MD = process.argv.includes("--md");

// ── Rust 源文件遍历（排除 vendor / target）───────────────────────────────────
function listRs(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (name === "vendor" || name === "target") continue;
    const st = statSync(full);
    if (st.isDirectory()) listRs(full, out);
    else if (name.endsWith(".rs")) out.push(full);
  }
  return out;
}

// ── 词法层：剥离注释与字面量，同时保留 cfg 起点位置 ─────────────────────────
// 逐字符状态机。返回 [{ start, end, text }]，text 是完整的 cfg(...) 表达式（含引号），
// start/end 是源文件中的字符偏移。
function extractCfgSites(src) {
  const sites = [];
  const n = src.length;
  let i = 0;
  let mode = "code"; // code | line_comment | block_comment | string | char | raw_string

  function skipRawString() {
    // 当前 i 指向 r。统计 # 个数。
    let hashes = 0;
    while (i + 1 + hashes < n && src[i + 1 + hashes] === "#") hashes++;
    const bodyStart = i + 1 + hashes;
    if (bodyStart >= n || src[bodyStart] !== '"') { mode = "code"; return; }
    const delim = '"' + "#".repeat(hashes);
    const close = bodyStart + 1;
    const endIdx = src.indexOf(delim, close);
    i = endIdx === -1 ? n : endIdx + delim.length;
    mode = "code";
  }

  while (i < n) {
    const c = src[i];
    if (mode === "line_comment") {
      if (c === "\n") mode = "code";
      i++;
      continue;
    }
    if (mode === "block_comment") {
      if (c === "*" && src[i + 1] === "/") { mode = "code"; i += 2; continue; }
      i++;
      continue;
    }
    if (mode === "string") {
      if (c === "\\") { i += 2; continue; }
      if (c === '"') mode = "code";
      i++;
      continue;
    }
    if (mode === "char") {
      if (c === "\\") { i += 2; continue; }
      if (c === "'") mode = "code";
      i++;
      continue;
    }
    if (mode === "raw_string") {
      // 在 skipRawString 里处理；这里不会进入
      i++;
      continue;
    }

    // mode === "code"
    if (c === "/" && src[i + 1] === "/") { mode = "line_comment"; i += 2; continue; }
    if (c === "/" && src[i + 1] === "*") { mode = "block_comment"; i += 2; continue; }
    if (c === '"') { mode = "string"; i++; continue; }
    if (c === "'") {
      // char 字面量 'x' 或转义 '\n'：完整消费后保持 code 模式。
      // 生命周期 'a 不构成闭合对，按普通字符跳过。
      let j = i + 1;
      let content = "";
      if (j < n && src[j] === "\\") { content = "\\"; j++; if (j < n) { content += src[j]; j++; } }
      else if (j < n && src[j] !== "'" && src[j] !== "\n") { content = src[j]; j++; }
      if (content && j < n && src[j] === "'") { i = j + 1; continue; } // 'x' 完整消费
      i++;
      continue;
    }
    if (c === "r") {
      // raw string r#"..."# 或 raw char r'...'（Rust 2021+）
      let hashes = 0;
      while (i + 1 + hashes < n && src[i + 1 + hashes] === "#") hashes++;
      if (i + 1 + hashes < n && (src[i + 1 + hashes] === '"' || src[i + 1 + hashes] === "'")) {
        if (src[i + 1 + hashes] === '"') { skipRawString(); continue; }
        // raw char r'x'
        const bodyStart = i + 1 + hashes + 1;
        const endIdx = src.indexOf("'" + "#".repeat(hashes), bodyStart);
        i = endIdx === -1 ? n : endIdx + 1 + hashes;
        continue;
      }
    }
    // cfg 属性 / 宏起点（outer `#[cfg` 与 inner `#![cfg` 都认；inner 用于 crate/模块级条件编译）
    if (c === "#" && (src[i + 1] === "[" || src[i + 1] === "!")) {
      // 可能是 #[cfg(...)]、#![cfg(...)]、#[cfg_attr(...)]、#![cfg_attr(...)]
      const m = /^#!?\s*\[\s*(cfg(?:_attr)?)\s*\(/.exec(src.slice(i));
      if (m) {
        const exprStart = i + m[0].length - 1; // 指向 '('
        let depth = 0;
        let j = exprStart;
        for (; j < n; j++) {
          const cc = src[j];
          if (cc === "(") depth++;
          else if (cc === ")") { depth--; if (depth === 0) break; }
          else if (cc === '"') {
            // 跳过 cfg 表达式内的字符串字面量（引号成对）
            j++;
            while (j < n && src[j] !== '"') { if (src[j] === "\\") j++; j++; }
          }
        }
        if (depth === 0 && j < n) {
          const text = src.slice(exprStart, j + 1);
          sites.push({ start: i, end: j + 1, text, attr: true, name: m[1] });
          i = j + 1;
          continue;
        }
      }
      i += 2;
      continue;
    }
    if (c === "c" && src[i + 1] === "f" && src[i + 2] === "g" && src[i + 3] === "!") {
      // cfg!(...) 宏（非注释/字符串内）
      const m = /^cfg!\s*\(/.exec(src.slice(i));
      if (m) {
        const exprStart = i + m[0].length - 1;
        let depth = 0;
        let j = exprStart;
        for (; j < n; j++) {
          const cc = src[j];
          if (cc === "(") depth++;
          else if (cc === ")") { depth--; if (depth === 0) break; }
          else if (cc === '"') {
            j++;
            while (j < n && src[j] !== '"') { if (src[j] === "\\") j++; j++; }
          }
        }
        if (depth === 0 && j < n) {
          const text = src.slice(exprStart, j + 1);
          sites.push({ start: i, end: j + 1, text, attr: false, name: "cfg" });
          i = j + 1;
          continue;
        }
      }
    }
    i++;
  }
  return sites;
}

// ── 谓词规范化 ────────────────────────────────────────────────────────────────
// 输入：完整 cfg 表达式文本（含最外层括号，如 `all(target_arch = "wasm32", ...)`）。
// 输出：规范化字符串（统一空白与引号），以及 wasm 相关判定。
function normalizePredicate(text) {
  // 去掉最外层 ( )
  let t = text.trim();
  if (t.startsWith("(") && t.endsWith(")")) t = t.slice(1, -1).trim();
  // 规范化：引号统一为双引号，空白压缩
  t = t.replace(/'([^']*)'/g, '"$1"').replace(/\s+/g, " ").replace(/\s*=\s*/g, " = ").replace(/\s*,\s*/g, ", ").trim();
  return t;
}

// 判断谓词是否涉及 wasm32 / wasi（在 not() 内则反向）
function wasmKind(pred) {
  // 直接匹配 target_arch = "wasm32" / target_os = "wasi" / target_os = "emscripten" 等
  const wasmRe = /(?:target_arch\s*=\s*"wasm32"|target_os\s*=\s*"wasi"|target_os\s*=\s*"emscripten")/;
  const insideNot = /not\s*\([^()]*(?:target_arch\s*=\s*"wasm32")[^()]*\)/.test(pred);
  if (wasmRe.test(pred)) {
    // 若该 wasm 谓词整体被 not() 包住 → not-wasm（桌面侧对偶）
    // 简化：用整串是否以 not( 开头且含 wasm 判定 not-wasm
    if (/^not\s*\(.*wasm32.*\)$/.test(pred) || /not\s*\(\s*all\([^)]*wasm32/.test(pred)) return "not-wasm";
    if (insideNot) return "not-wasm";
    return "wasm";
  }
  if (/target_(os|arch|family|vendor|env)\s*=/.test(pred) || /\b(windows|unix|linux|macos)\b/.test(pred)) return "platform";
  if (/\bfeature\b/.test(pred)) return "feature";
  if (/\btest\b/.test(pred) || /^\s*not\s*\(\s*test\s*\)/.test(pred)) return "test";
  return "other";
}

// ── 提取 cfg 后的条目 token（item）───────────────────────────────────────────
function itemAfter(src, end) {
  const rest = src.slice(end);
  const m = /^\s*([a-zA-Z_][a-zA-Z0-9_]*)/.exec(rest);
  return m ? m[1] : "";
}

// ── 主流程 ────────────────────────────────────────────────────────────────────
const files = listRs(CRATES);
const sites = [];

for (const full of files) {
  const src = readFileSync(full, "utf8");
  const rel = relative(ROOT, full).replace(/\\/g, "/");
  const crateM = rel.match(/crates\/([^/]+)\//);
  const crate = crateM ? crateM[1] : (rel.startsWith("engine/") ? "engine" : "unknown");
  const found = extractCfgSites(src);
  // 每个 (crate,file,predicate,item) 的出现计数
  const occ = new Map();
  for (const s of found) {
    const pred = normalizePredicate(s.text);
    const item = itemAfter(src, s.end);
    const key = `${crate}|${rel.replace(/^engine\//, "")}|${pred}|${item}`;
    const n = (occ.get(key) || 0) + 1;
    occ.set(key, n);
    sites.push({
      key: `${key}|${n}`,
      crate,
      file: rel.replace(/^engine\//, ""),
      predicate: pred,
      item: item || "(none)",
      occurrence: n,
      kind: wasmKind(pred),
      attr_or_macro: s.attr ? "attr" : "macro",
    });
  }
}

sites.sort((a, b) => a.key.localeCompare(b.key));

const counts = {
  wasm_attrs: sites.filter((s) => s.kind === "wasm" && s.attr_or_macro === "attr").length,
  wasm_macros: sites.filter((s) => s.kind === "wasm" && s.attr_or_macro === "macro").length,
  not_wasm_attrs: sites.filter((s) => s.kind === "not-wasm" && s.attr_or_macro === "attr").length,
  platform: sites.filter((s) => s.kind === "platform").length,
  feature: sites.filter((s) => s.kind === "feature").length,
  test: sites.filter((s) => s.kind === "test").length,
  other: sites.filter((s) => s.kind === "other").length,
  total: sites.length,
};

const doc = {
  schema: 1,
  generated_at: new Date().toISOString(),
  counts,
  sites,
};

mkdirSync(dirname(OUT), { recursive: true });

// ── --check：与跟踪文件逐 key 比对 ───────────────────────────────────────────
if (CHECK) {
  let prev = null;
  try { prev = JSON.parse(readFileSync(OUT, "utf8")); } catch { prev = null; }
  if (!prev || !Array.isArray(prev.sites)) {
    console.error(`cfg_inventory_failed reason=tracked_file_missing_or_invalid ${OUT}`);
    process.exit(1);
  }
  const prevKeys = new Map(prev.sites.map((s) => [s.key, s.kind + "|" + s.attr_or_macro]));
  const drift = [];
  for (const s of sites) {
    const v = prevKeys.get(s.key);
    if (v === undefined) drift.push(`added: ${s.key}`);
    else if (v !== s.kind + "|" + s.attr_or_macro) drift.push(`changed: ${s.key}`);
    prevKeys.delete(s.key);
  }
  for (const [k] of prevKeys) drift.push(`removed: ${k}`);
  if (drift.length > 0) {
    console.error(`cfg_inventory_drift items=${drift.length}`);
    for (const d of drift.slice(0, 20)) console.error("  " + d);
    process.exit(1);
  }
  console.log(`cfg_inventory_check_ok files=${files.length} sites=${sites.length} wasm_attrs=${counts.wasm_attrs} wasm_macros=${counts.wasm_macros} not_wasm_attrs=${counts.not_wasm_attrs}`);
  process.exit(0);
}

// ── 生成（默认）─────────────────────────────────────────────────────────────
writeFileSync(OUT, JSON.stringify(doc, null, 2));

if (WANT_MD) {
  const byCrate = {};
  for (const s of sites) (byCrate[s.crate] ||= []).push(s);
  const rows = Object.entries(byCrate).map(([c, arr]) => {
    const w = arr.filter((s) => s.kind === "wasm").length;
    const nw = arr.filter((s) => s.kind === "not-wasm").length;
    return `| ${c} | ${arr.length} | ${w} | ${nw} |`;
  }).join("\n");
  const md = `# cfg 条件编译清单（cfg-sites.json）

- 扫描：${files.length} 个 Rust 源文件；${sites.length} 个站点
- 计数：wasm 属性 ${counts.wasm_attrs}、wasm cfg! 宏 ${counts.wasm_macros}、not-wasm 属性 ${counts.not_wasm_attrs}、platform ${counts.platform}、feature ${counts.feature}、test ${counts.test}、other ${counts.other}

| crate | 站点 | wasm | not-wasm |
|---|---|---|---|
${rows}

> 稳定 key 不含行号；清单漂移由 \`cfg-inventory.mjs --check\` 判红。
> 文档注释/字符串里的 cfg 示例不计入（扫描器先剥字面量）。
`;
  mkdirSync(dirname(MD), { recursive: true });
  writeFileSync(MD, md);
}

console.log(`cfg_inventory_ok files=${files.length} sites=${sites.length} wasm_attrs=${counts.wasm_attrs} wasm_macros=${counts.wasm_macros} not_wasm_attrs=${counts.not_wasm_attrs}`);
