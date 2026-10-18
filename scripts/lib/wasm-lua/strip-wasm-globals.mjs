// 从 wasm 的 Export section 里剔掉 kind=Global 的条目（其余 section 原样保留）。
//
// ## 为什么要
// iOS 上的 `WXWebAssembly` 不支持 Global 导出（docs/52 R12 尾段口径）。当前 wasm-bindgen
// 产物会导出 `__abort_handler` / `__instance_terminated` 两个内部 global。
//
// ## 一条**成对**的约束（2026-10-03 实测纠正本文件旧口径）
// 这里曾写过"JS 胶水根本不引用它们、剔掉不破坏任何运行时行为"——那是错的：
// 胶水的 `__wbg_call_guard()`（每个导出函数开头都调）无条件读
//   `__wbg_terminated_addr ??= wasm.__instance_terminated.value / 4;`
// 于是剔掉 Global 导出后，**第一次跨 wasm 调用**就抛
//   `TypeError: Cannot read properties of undefined (reading 'value')`
// 微信包整体 boot=failed（症状：实例化成功、宿主桥 error、一帧不跑）。
// 现在由 `scripts/build/package.ps1` 的微信分支在构建期给那句加缺席短路（global 读不到就跳过守卫——
// 它答的是"实例已终止就别再调"，读不到标记时本来也无从判断；同份胶水的 abort hook 自己就包在
// try/catch 里）。**剔 Global 与该补丁必须成对存在**：只留一个就是死包。
//
// ## 它做什么
// * 解析 wasm section 头（magic+version + 若干 [id][LEB128 size][payload]），
//   只对 Export 段（id=7）动刀：遍历条目，跳过 kind=3(Global)，把剩下的原样重排。
// * **不**动 Global 段（id=6）本身：定义留着，只是不再对外暴露。模块仍然自洽。
// * 其他所有 section（含 name/data/code/custom）逐字节透传；只有 Export 段的
//   LEB128 size 字段随内容重算。
//
// ## 用法
//   node scripts/lib/wasm-lua/strip-wasm-globals.mjs <in.wasm> <out.wasm>
// 退出码：0 成功；1 wasm 解析失败或 Export 段异常；3 参数/文件缺失。
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';

const [inPath, outPath] = process.argv.slice(2);
if (!inPath || !outPath) {
  console.error('usage: strip-wasm-globals.mjs <in.wasm> <out.wasm>');
  process.exit(3);
}
if (!existsSync(inPath)) {
  console.error(`input wasm not found: ${inPath}`);
  process.exit(3);
}

const bytes = readFileSync(inPath);
const MAGIC = [0x00, 0x61, 0x73, 0x6d];
for (let i = 0; i < 4; i++) if (bytes[i] !== MAGIC[i]) die('not a wasm module (magic mismatch)');
if (bytes[4] !== 0x01 || bytes[5] !== 0x00 || bytes[6] !== 0x00 || bytes[7] !== 0x00) die('unsupported wasm version (expected 1)');

function die(msg) {
  console.error(`strip_wasm_globals_failed: ${msg}`);
  process.exit(1);
}

// LEB128 unsigned. Returns [value, nextOffset]; guards against 32-bit overflow (sizes are u32).
function readU32(buf, off) {
  let result = 0, shift = 0, pos = off;
  while (true) {
    if (pos >= buf.length) die('LEB128 truncated at offset ' + off);
    const b = buf[pos++];
    result |= (b & 0x7f) << shift;
    if ((b & 0x80) === 0) break;
    shift += 7;
    if (shift > 35) die('LEB128 u32 overflow at offset ' + off);
  }
  return [result >>> 0, pos];
}

function writeU32(value) {
  const out = [];
  let v = value >>> 0;
  while (true) {
    let b = v & 0x7f;
    v >>>= 7;
    if (v !== 0) b |= 0x80;
    out.push(b);
    if (v === 0) break;
  }
  return Buffer.from(out);
}

// Section id 7 = Export; entry kind 3 = Global.
const EXPORT_SECTION_ID = 7;
const KIND_GLOBAL = 3;

const parts = [bytes.subarray(0, 8)]; // magic + version
let cursor = 8;
let exportSectionSeen = false;
let totalBefore = 0, removed = 0, kept = 0;
const removedNames = [];

while (cursor < bytes.length) {
  const sectionId = bytes[cursor];
  const [sectionSize, contentStart] = readU32(bytes, cursor + 1);
  const contentEnd = contentStart + sectionSize;
  if (contentEnd > bytes.length) die(`section ${sectionId} size runs past EOF`);

  if (sectionId === EXPORT_SECTION_ID) {
    exportSectionSeen = true;
    let pos = contentStart;
    const [count, entriesStart] = readU32(bytes, pos);
    pos = entriesStart;
    const entries = [];
    for (let i = 0; i < count; i++) {
      totalBefore++;
      const entryStart = pos;
      const [nameLen, nameStart] = readU32(bytes, pos);
      const nameEnd = nameStart + nameLen;
      if (nameEnd > contentEnd) die(`export entry ${i} name runs past section`);
      const name = bytes.subarray(nameStart, nameEnd).toString('utf8');
      const kind = bytes[nameEnd];
      const [, idxEnd] = readU32(bytes, nameEnd + 1);
      if (idxEnd > contentEnd) die(`export entry ${i} index runs past section`);
      const rawEntry = bytes.subarray(entryStart, idxEnd);
      if (kind === KIND_GLOBAL) {
        removed++;
        removedNames.push(name);
      } else {
        entries.push(rawEntry);
        kept++;
      }
      pos = idxEnd;
    }
    // Rebuild section content: [new count][entries...]
    const body = [writeU32(entries.length)];
    for (const e of entries) body.push(e);
    const newContent = Buffer.concat(body);
    parts.push(Buffer.from([sectionId]));
    parts.push(writeU32(newContent.length));
    parts.push(newContent);
    if (pos !== contentEnd) die(`export section trailing ${contentEnd - pos} bytes unparsed`);
  } else {
    parts.push(bytes.subarray(cursor, contentEnd));
  }
  cursor = contentEnd;
}

if (!exportSectionSeen) die('no export section in module');

const out = Buffer.concat(parts);
writeFileSync(resolve(outPath), out);
console.log(
  `strip_wasm_globals_ok in=${bytes.length}B out=${out.length}B ` +
  `exports_before=${totalBefore} kept=${kept} removed=${removed} ` +
  `names=[${removedNames.join(',')}]`
);
