// 从 wasm 里彻底去掉 Exception Handling：tag 导入 / 本地 tag 定义 / throw / rethrow /
// try（旧式 catch/catch_all/delegate）/ try_table。
//
// ## 为什么必须剥（而不是转发/polyfill）
// wasm-bindgen 0.2 的产物导入两个 tag（`__wbindgen_jstag` / `__wbindgen_wrapped_jstag`），
// 用于跨 wasm 边界桥接 JS 异常。微信的 `WXWebAssembly` 没有 `WebAssembly.Tag` 构造器，
// 实测把 polyfill 对象塞进 imports 也会被引擎拒绝（`tag import requires a WebAssembly.Tag`，
// 见 scripts/verify/verify-wechat-minigame.ps1 用例 1a3 的 `polyfill_tag_probe=rejected`）。
// 唯一可行的路径：**让 wasm 根本不涉及 tag**。
//
// ## 为什么走 binaryen 文本管线，而不是手工字节 walker
// 上一版在 Code 段手工 walk 指令流，因为 legacy EH 的 blocktype/子句编码错位，5523 个函数
// 从第 2 个就开始跑偏（LEB128 越界）。改用 `wasm-dis` 出文本 → s-表达式结构化改写 → `wasm-as`
// 合回去：指令流的正确性由 binaryen 兜底，我们只做括号配平的结构改写，不再碰 LEB 立即数。
//
// ## 语义代价（务必知情）
// 剥完之后，Rust→JS 的边界异常传播从"干净 unwind 进 JS catch 包装"塌成 **wasm trap**
// （`throw`→`unreachable`），JS→Rust 方向的异常接收（`catch $tag`）一并移除。
// 这对 jhyt 是否有影响，以真机/工具里 Lua pcall 的实测为准，由 verify-wechat 门把关，
// 不在本脚本里假设它安全。
//
// ## 成对约束
// 见 scripts/build/package.ps1：胶水侧的 WebAssembly.Tag 构造、Exception 包装、tag 引用要同步简化，
// 否则只剥 wasm 不补胶水会在 require 期 ReferenceError；只补胶水不剥 wasm 则 instantiate 报缺 tag。
//
// ## 用法
//   node scripts/lib/wasm-lua/strip-wasm-tags.mjs <in.wasm> <out.wasm>
// 退出码：0 成功；1 剥不干净/校验失败；3 参数/文件/工具缺失。
import { readFileSync, writeFileSync, existsSync, mkdtempSync, rmSync, copyFileSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const HERE = dirname(fileURLToPath(import.meta.url));

const [inPath, outPath] = process.argv.slice(2);
if (!inPath || !outPath) {
  console.error('usage: strip-wasm-tags.mjs <in.wasm> <out.wasm>');
  process.exit(3);
}
if (!existsSync(inPath)) {
  console.error(`input wasm not found: ${inPath}`);
  process.exit(3);
}

// ---- binaryen 定位 ----
// vendored 在 engine/.tools/binaryen/binaryen-version_*/bin/，不依赖 PATH。
function findBinaryenBin() {
  const roots = [
    resolve(HERE, '..', '..', '..', 'engine', '.tools', 'binaryen'),
  ];
  const exe = process.platform === 'win32' ? '.exe' : '';
  for (const root of roots) {
    if (!existsSync(root)) continue;
    // 取版本号最大的那个目录
    const versions = readdirNames(root).filter((n) => /^binaryen-/.test(n)).sort();
    for (let i = versions.length - 1; i >= 0; i--) {
      const bin = join(root, versions[i], 'bin');
      const dis = join(bin, `wasm-dis${exe}`);
      const as = join(bin, `wasm-as${exe}`);
      if (existsSync(dis) && existsSync(as)) return { dis, as };
    }
  }
  console.error('找不到 vendored binaryen（engine/.tools/binaryen/binaryen-version_*/bin/wasm-dis,wasm-as）');
  process.exit(3);
}
function readdirNames(dir) {
  return readdirSync(dir);
}

// ---- WAT 解析：只配平括号，跳过 ;; 行注释与字符串字面量 ----
// 节点形态：{k:'l', items:[...]} 列表；{k:'a', v:'token'} 原子（含字符串字面量，v 保留引号）。
// 用显式栈做括号配平，比旧字节 walker 的 LEB 立即数解码可靠得多。
function parseWat(text) {
  const len = text.length;
  const stack = [];              // 打开的列表栈，元素是父节点（顶层用 null 占位）
  let current = null;            // 当前正在填充的列表
  let top = null;                // 最外层 (module ...)
  let i = 0;
  function err(msg) { throw new Error(`WAT parse error: ${msg} (offset ${i})`); }
  function pushAtom(v) {
    if (!current) err('atom outside any list');
    current.items.push({ k: 'a', v });
  }
  while (i < len) {
    const c = text[i];
    if (c === ' ' || c === '\t' || c === '\n' || c === '\r') { i++; continue; }
    if (c === ';' && text[i + 1] === ';') {                 // ;; 行注释
      while (i < len && text[i] !== '\n') i++;
      continue;
    }
    if (c === '(') {
      stack.push(current);
      current = { k: 'l', items: [] };
      i++;
      continue;
    }
    if (c === ')') {
      if (!current) err('unexpected )');
      const done = current;
      current = stack.pop() || null;
      if (current) current.items.push(done);
      else if (top) err('two top-level forms');
      else top = done;
      i++;
      continue;
    }
    if (c === '"') {                                         // 字符串字面量，整段当一个原子
      let j = i + 1;
      while (j < len) {
        if (text[j] === '\\') { j += 2; continue; }
        if (text[j] === '"') { j++; break; }
        j++;
      }
      pushAtom(text.slice(i, j));
      i = j;
      continue;
    }
    // 普通原子 token：到空白/括号/引号/分号为止
    let j = i;
    while (j < len) {
      const d = text[j];
      if (d === ' ' || d === '\t' || d === '\n' || d === '\r' || d === '(' || d === ')' || d === '"') break;
      if (d === ';' && text[j + 1] === ';') break;
      j++;
    }
    pushAtom(text.slice(i, j));
    i = j;
  }
  if (current || !top) err('unbalanced parentheses');
  return top;
}

// ---- WAT 序列化 ----
// 与 wasm-dis 的排版无关，只要 token 之间留白、括号成对即可被 wasm-as 读回。
function emit(node, out) {
  if (node.k === 'a') { out.push(node.v); return; }
  out.push('(');
  for (let i = 0; i < node.items.length; i++) {
    if (i) out.push(' ');
    emit(node.items[i], out);
  }
  out.push(')');
}

// ---- 改写 ----
const headAtom = (node) => (node.k === 'l' && node.items[0] && node.items[0].k === 'a' ? node.items[0].v : null);
const tagImportBoundName = (importNode) => {
  const tagNode = importNode.items.find((it) => it.k === 'l' && headAtom(it) === 'tag');
  if (!tagNode) return null;
  const nameAtom = tagNode.items[1];                 // (tag $eimport$0 …) → $eimport$0
  return nameAtom && nameAtom.k === 'a' ? nameAtom.v : null;
};
const isTagImport = (node) =>
  headAtom(node) === 'import' && node.items.some((it) => it.k === 'l' && headAtom(it) === 'tag');

const stats = { tagsRemoved: 0, throwsNeutralized: 0, catchClausesDropped: 0, tryDowngradedToBlock: 0 };
const removedTagNames = [];
const removedNameSet = new Set();                  // 只被"tag 导入"绑定的名字（externref JS 桥那几个）

// 一个 catch / catch_ref 子句是否引用了被删的导入 tag（是 → 丢；引用内部 tag 的 catch → 留）
function isExternRefCatchClause(node) {
  const h = headAtom(node);
  if (h !== 'catch' && h !== 'catch_ref') return false;
  const tagAtom = node.items[1];
  return !!(tagAtom && tagAtom.k === 'a' && removedNameSet.has(tagAtom.v));
}

// try / try_table：只丢引用被删导入 tag 的子句；剩余子句 ≥1 就仍是合法 try（递归其内容），
// 全空才降级成 block。这样内部 tag（Rust catch_unwind / Lua 错误）的 unwind 结构被完整保留。
function rewriteTry(node) {
  const kind = headAtom(node);              // 'try' | 'try_table'
  let labelAtom = null;
  const annotations = [];
  const bodyItems = [];
  const keptClauses = [];
  let sawDo = false;
  for (let idx = 1; idx < node.items.length; idx++) {
    const child = node.items[idx];
    if (child.k === 'a') { labelAtom = child; continue; }
    const h = headAtom(child);
    if (h === 'result' || h === 'param') { annotations.push(transformNode(child)); continue; }
    if (h === 'do') {                        // 旧式 try 的 body 包在 (do …) 里
      sawDo = true;
      for (let j = 1; j < child.items.length; j++) bodyItems.push(child.items[j]);
      continue;
    }
    if (h === 'catch' || h === 'catch_ref' || h === 'catch_all' || h === 'catch_all_ref' || h === 'delegate') {
      if (isExternRefCatchClause(child)) { stats.catchClausesDropped++; continue; } // 丢 externref 桥 catch
      keptClauses.push(transformNode(child));                                        // 内部 tag catch 留着并递归
      continue;
    }
    bodyItems.push(child);                   // try_table 的 body 是直接子节点
  }

  if (keptClauses.length === 0) {            // 没有任何非桥子句 → 整个 try 塌成 block（body 保序）
    const block = { k: 'l', items: [{ k: 'a', v: 'block' }] };
    if (labelAtom) block.items.push(labelAtom);
    for (const a of annotations) block.items.push(a);
    for (const b of bodyItems) block.items.push(transformNode(b));
    stats.tryDowngradedToBlock++;
    return block;
  }

  const retry = { k: 'l', items: [{ k: 'a', v: kind }] };
  if (labelAtom) retry.items.push(labelAtom);
  for (const a of annotations) retry.items.push(a);
  if (kind === 'try') {                      // 旧式：body 必须重新包回 (do …)
    retry.items.push({ k: 'l', items: [{ k: 'a', v: 'do' }, ...bodyItems.map(transformNode)] });
  } else {
    for (const b of bodyItems) retry.items.push(transformNode(b));
  }
  for (const c of keptClauses) retry.items.push(c);
  void sawDo;
  return retry;
}

function transformNode(node) {
  if (node.k === 'a') return node;
  const h = headAtom(node);

  // throw <被删导入名> → unreachable；throw <内部 tag> 原样保留
  if (h === 'throw') {
    const tagAtom = node.items[1];
    if (tagAtom && tagAtom.k === 'a' && removedNameSet.has(tagAtom.v)) {
      stats.throwsNeutralized++;
      return { k: 'a', v: 'unreachable' };
    }
    // 内部 tag 的 throw：保留结构，只递归操作数里可能混进的桥 throw
    const kept = { k: 'l', items: node.items.slice() };
    for (let idx = 1; idx < kept.items.length; idx++) kept.items[idx] = transformNode(kept.items[idx]);
    return kept;
  }
  if (h === 'throw_ref' || h === 'rethrow') {
    return node;                             // 无导入 tag 操作数：原样保留（rethrow 随保留的 try 仍合法）
  }
  if (h === 'try' || h === 'try_table') return rewriteTry(node);

  if (h === 'module') {                       // 顶层：删 tag 导入、留本地 tag 定义
    const items = [{ k: 'a', v: 'module' }];
    for (let idx = 1; idx < node.items.length; idx++) {
      const child = node.items[idx];
      if (isTagImport(child)) {
        stats.tagsRemoved++;
        const nm = tagImportBoundName(child);
        if (nm) { removedTagNames.push(childField(child)); removedNameSet.add(nm); }
        continue;
      }
      items.push(transformNode(child));
    }
    node.items = items;
    return node;
  }

  const items = node.items.slice();
  for (let idx = 0; idx < items.length; idx++) items[idx] = transformNode(items[idx]);
  node.items = items;
  return node;
}

function childField(importNode) {
  const atoms = importNode.items.filter((it) => it.k === 'a').slice(1, 3).map((a) => a.v.replace(/^"|"$/g, ''));
  return atoms.join('.');
}

// 递归收集：先扫一遍顶层把 tag 导入名登记进 removedNameSet，再正式改写
// （两趟，否则处理到某个 throw/catch 时还不知道它引用的 tag 是否是"被删导入"）
function collectRemovedTagNames(moduleNode) {
  for (const child of moduleNode.items) {
    if (!isTagImport(child)) continue;
    const nm = tagImportBoundName(child);
    if (nm) removedNameSet.add(nm);
  }
}

// ---- 主流程 ----
const { dis, as } = findBinaryenBin();
const inSize = readFileSync(inPath).length;   // 覆盖前取真实剥前大小（package.ps1 是原地 strip，in==out）
const work = mkdtempSync(join(tmpdir(), 'strip-tags-'));
try {
  const watText = execFileSync(dis, [inPath], { maxBuffer: 512 * 1024 * 1024, encoding: 'utf8' });
  const moduleNode = parseWat(watText);
  if (headAtom(moduleNode) !== 'module') {
    console.error('strip_wasm_tags_failed: parsed root is not (module ...)');
    process.exit(1);
  }
  collectRemovedTagNames(moduleNode);
  transformNode(moduleNode);

  // 结构断言（代替旧"零 EH"硬保证）：改写后**顶层不得再有任何 tag 导入**。本地 tag 允许留着。
  for (const child of moduleNode.items) {
    if (isTagImport(child)) {
      console.error('strip_wasm_tags_failed: tag import survived the rewrite');
      process.exit(1);
    }
  }

  const out = [];
  emit(moduleNode, out);
  const outWat = out.join('\n');
  const watPath = join(work, 'stripped.wat');
  writeFileSync(watPath, outWat, 'utf8');

  // 窄修**保留**内部 tag 的 EH（Rust catch_unwind / Lua 错误），所以 wasm-as 必须开
  // --enable-exception-handling。"剥干净了没"由上面的结构断言（零 tag 导入）保证，而非靠关 flag。
  const FEATURES = [
    '--enable-sign-ext', '--enable-mutable-globals', '--enable-nontrapping-float-to-int',
    '--enable-bulk-memory', '--enable-bulk-memory-opt', '--enable-tail-call',
    '--enable-reference-types', '--enable-multivalue', '--enable-gc', '--enable-extended-const',
    '--enable-exception-handling',
  ];
  const rebuilt = join(work, 'stripped.wasm');
  try {
    execFileSync(as, [watPath, '-o', rebuilt, ...FEATURES], { maxBuffer: 512 * 1024 * 1024 });
  } catch (e) {
    const msg = String((e && (e.stderr?.toString() || e.message)) || e).split('\n').filter(Boolean).slice(0, 4).join(' | ');
    console.error(`strip_wasm_tags_failed: wasm-as 校验未过（多半是残留 EH 或漏开 feature）：${msg}`);
    process.exit(1);
  }

  copyFileSync(rebuilt, resolve(outPath));
  const outSize = readFileSync(resolve(outPath)).length;
  console.log(
    `strip_wasm_tags_ok in=${inSize}B out=${outSize}B ` +
    `tag_imports_removed=${stats.tagsRemoved} throws_neutralized=${stats.throwsNeutralized} ` +
    `catch_clauses_dropped=${stats.catchClausesDropped} try_downgraded=${stats.tryDowngradedToBlock} ` +
    `names=[${removedTagNames.join(',')}]`
  );
} catch (err) {
  console.error(`strip_wasm_tags_failed: ${err && err.message ? err.message : String(err)}`);
  process.exit(1);
} finally {
  try { rmSync(work, { recursive: true, force: true }); } catch (_) { }
}
