#!/usr/bin/env node
// 打包产物的 Lua 注释剥离 + manifest 同步重写。
//
// 为什么需要它：微信小游戏主包上限 4,096 KB，实测 subpackage 形态的主包 4,269 KB（超 173 KB）。
// 编译侧的杠杆已经全部用完（wasm 走 opt-level=z + lto=fat + codegen-units=1 + wasm-opt -Oz，
// JS 走 terser，name section 已 --remove-name-section），剩下的可削空间只在**内容侧**：
// 主包里 1,275 KB 的 mainpack/ 有 ~878 KB 是 Lua 脚本，而这些脚本给人看的注释占了其中 ~150 KB。
// 剥注释是这批体积里唯一**完全不改变运行时行为**的减法——不改语义、不改 API、不改渲染。
//
// 为什么必须同时重写 manifest.txt：`assetc` 生成的清单是 `路径\t字节数\tFNV-1a-64(16 位十六进制)`，
// 而**运行时逐条校验**（plugins/wechat-minigame/wechat-adapter.js 的 fnv1a64Hex + 清单查表；
// 桌面/assetc 侧同一套）。只改文件不改清单 = 首屏每一个脚本都校验失败。
// 因此本工具把"清单能逐条复现"当成**前置硬条件**（默认模式就是干这个的），复现不了就拒绝动手。
//
// 为什么保行数（剥掉注释文字但保留换行）：Lua 的栈回溯、报错信息里的行号、
// 以及文档里对源码行号的引用全部以**行号**为准。工程里就有这样一个耦合
// （改 script/api/*.rs 的行数会红 lua_api_docs 门，见 memory）。行数不变 ⇒ 发布包的报错
// 仍然能直接对着仓库源码读。代价是每行留一个空行/行尾空白，本来就要留换行，无额外体积损失。
//
// 只作用在 pack/ 里的**产物副本**，源工程 `examples/game_jhyt/scripts/**.lua` 一个字都不改。
//
// 用法：
//   node scripts/strip-lua-comments.mjs <packRoot>            # 体检：清单可复现 + 预计能省多少
//   node scripts/strip-lua-comments.mjs <packRoot> --write     # 落盘：剥注释 + 重写 manifest.txt
//   加 --json 输出机器可读摘要（package.ps1 与自动化门用）
// 退出码：0 正常 / 1 拒绝（清单复现不了、Lua 词法不成形、行数被改动）——任何"没做成"都是 1，
// 绝不返回"环境问题算了"这种软失败：静默不剥等于把体积超交给微信发布去发现。
'use strict';

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const MANIFEST_NAME = 'manifest.txt';
const MANIFEST_HEADER = 'assetc_manifest=1';
// Lua 的 FNV-1a 64 参数：offset basis 0xcbf29ce484222325、prime 0x100000001b3。
const FNV_LO = 0x84222325;
const FNV_HI = 0xcbf29ce4;
const FNV_PRIME_LO = 435;      // 0x1b3
const FNV_PRIME_MID = 256;     // 0x100
const TWO32 = 4294967296;

// 与适配层同一套两个 32 位半字手算的 FNV-1a 64（不用 BigInt：小游戏部分基础库版本没有）。
// 推导：hash = hi·2³² + lo，乘 P = 0x100·2³² + 0x1b3 时
//   lo' = (lo·0x1b3) 的低 32 位
//   hi' = ((lo·0x1b3) 的高位 + hi·0x1b3 + lo·0x100) mod 2³²
// 不能把两个半字拼回 Number（hi·2³² 远超 2⁵³），所以按 8+8 位十六进制拼接。
function pad8(value) {
  let text = (value >>> 0).toString(16);
  while (text.length < 8) text = '0' + text;
  return text;
}
function fnv1a64Hex(bytes) {
  let lo = FNV_LO, hi = FNV_HI;
  for (let i = 0; i < bytes.length; i++) {
    lo = (lo ^ bytes[i]) >>> 0;
    const m = lo * FNV_PRIME_LO;
    const mHi = Math.floor(m / TWO32);
    const mLo = m % TWO32;
    const cross = hi * FNV_PRIME_LO + lo * FNV_PRIME_MID;
    const cLo = cross % TWO32;
    hi = (mHi + cLo) % TWO32;
    lo = mLo;
  }
  return pad8(hi) + pad8(lo);
}

// `[==[` 形状（长括号，level = 中间 `=` 的个数）识别：pos 处必须是 `[`，然后 0..N 个 `=`，再一个 `[`。
function longBracketLevel(text, pos) {
  if (text.charCodeAt(pos) !== 91 /* [ */) return -1;
  let k = pos + 1, level = 0;
  while (text.charCodeAt(k) === 61 /* = */) { level++; k++; }
  if (text.charCodeAt(k) !== 91 /* [ */) return -1;
  return level;
}
function longBracketClose(text, pos, level) {
  const close = ']' + '='.repeat(level) + ']';
  const index = text.indexOf(close, pos);
  return index < 0 ? -1 : index + close.length;
}

// 剥注释：**逐字符词法**，不是正则。理由就在数据里——工程里的 Lua 有
// `print("a -- b")` 这种串内含 `--`、有 `--[[ ... ]]` 跨行块注释、有 `[=[ ... ]=]` 嵌套等号，
// 任何"按行匹配 `--`"的做法都会把字符串内容当注释吃掉，产出一个能过词法但语义错的脚本。
// 返回 { text, error }；error 非空表示这份文件**不能安全处理**（引号没闭合等）。
function stripLuaComments(source) {
  const out = [];
  const length = source.length;
  let i = 0;
  while (i < length) {
    const ch = source[i];

    // 注释起点
    if (ch === '-' && source[i + 1] === '-') {
      let j = i + 2;
      const level = longBracketLevel(source, j);
      if (level < 0) {
        // 行注释：吃到行尾（换行本身留着 ⇒ 行数不变）。
        while (j < length && source[j] !== '\n' && source[j] !== '\r') j++;
        trimTrailingSpaces(out);
        i = j;
        continue;
      }
      // 块注释：`--[[` / `--[=[`，结束必须同等级 `]==]`。
      const end = longBracketClose(source, j + level + 2, level);
      if (end < 0) return { text: '', error: '块注释没有闭合（--[' + '='.repeat(level) + '[）' };
      const span = source.slice(i, end);
      trimTrailingSpaces(out);
      // 只保留跨度内的换行：注释文字消失、行号保留。
      for (let k = 0; k < span.length; k++) if (span[k] === '\n') out.push('\n');
      i = end;
      continue;
    }

    // 短字符串：`"…"` / `'…'`，反斜杠转义整个吃掉（`\"`、`\'`、`\\`）。
    if (ch === '"' || ch === "'") {
      let k = i + 1;
      let closed = false;
      let text = '';
      while (k < length) {
        const c = source[k];
        if (c === '\\') { text += source.slice(k, k + 2); k += 2; continue; }
        if (c === '\n' || c === '\r') break;   // Lua 的短字符串里不允许裸换行 → 判为词法异常
        if (c === ch) { closed = true; break; }
        text += c; k++;
      }
      if (!closed) return { text: '', error: '第 ' + lineNumberAt(source, i) + ' 行的引号字符串没有在同一行闭合' };
      out.push(ch, text, ch);
      i = k + 1;
      continue;
    }

    // 长字符串：`[[…]]` / `[=[…]=]`，原样复制（里面有 `--` 也与我们无关）。
    if (ch === '[') {
      const level = longBracketLevel(source, i);
      if (level >= 0) {
        const end = longBracketClose(source, i + level + 2, level);
        if (end < 0) return { text: '', error: '第 ' + lineNumberAt(source, i) + ' 行的长字符串没有闭合' };
        out.push(source.slice(i, end));
        i = end;
        continue;
      }
    }

    out.push(ch);
    i++;
  }
  return { text: out.join(''), error: '' };
}

// 弹出当前行末尾的空白。**安全性论证**：能走到这里说明我们正处在"注释起点"，
// 也就是 out 的末尾是已输出的**代码**字符。字符串内容若要成为末尾，最后一字符必须是
// 闭引号 `"` `'` 或长括号闭 `]`——都不是空白，弹出会在它前面停下。
// 所以弹出的只可能是 token 之间的空白，不可能是字符串内容。换行同样停下（不跨行）。
function trimTrailingSpaces(out) {
  while (out.length > 0) {
    const tail = out[out.length - 1];
    if (tail === ' ' || tail === '\t') out.pop();
    else break;
  }
}

function lineNumberAt(source, index) {
  let line = 1;
  for (let i = 0; i < index && i < source.length; i++) if (source[i] === '\n') line++;
  return line;
}

function countNewlines(text) {
  let n = 0;
  for (let i = 0; i < text.length; i++) if (text[i] === '\n') n++;
  return n;
}

// 清单：首行 `assetc_manifest=1\tassetc-package-1`，其余 `相对路径\t字节数\t哈希`。
// 相对路径用 `/`，与 pack/ 磁盘路径一一对应（改名映射发生在**之后**，所以这里看到的全是逻辑名）。
function parseManifest(text) {
  const lines = text.split('\n');
  if (lines.length === 0 || !lines[0].startsWith(MANIFEST_HEADER)) {
    throw new Error('清单首行不是 ' + MANIFEST_HEADER + '（不是 assetc 清单？）');
  }
  const entries = [];
  for (let i = 1; i < lines.length; i++) {
    const line = lines[i].replace(/\r$/, '');
    if (line === '') continue;
    const parts = line.split('\t');
    if (parts.length < 3) throw new Error('清单第 ' + (i + 1) + ' 行不是「路径\\t字节\\t哈希」: ' + line);
    entries.push({ rel: parts[0], size: Number(parts[1]), hash: parts[2], index: i });
  }
  return { lines, entries };
}

function main(argv) {
  const args = argv.slice(2);
  const write = args.includes('--write');
  const json = args.includes('--json');
  const packRoot = args.find((a) => !a.startsWith('--'));
  if (!packRoot) {
    process.stderr.write('用法: node scripts/strip-lua-comments.mjs <packRoot> [--write] [--json]\n');
    return 1;
  }
  const manifestPath = path.join(packRoot, MANIFEST_NAME);
  if (!fs.existsSync(manifestPath)) {
    process.stderr.write('找不到清单: ' + manifestPath + '\n');
    return 1;
  }

  const manifestText = fs.readFileSync(manifestPath, 'utf8');
  let manifest;
  try { manifest = parseManifest(manifestText); }
  catch (error) { process.stderr.write('清单解析失败: ' + error.message + '\n'); return 1; }

  // 第 1 步：整份清单逐条复现（哈希与字节数都对得上磁盘内容）才继续。
  // 这一步同时是本工具自身正确性的证据：若我的 FNV 实现与 assetc 差一个 bit，
  // 这里就会报出几十条 mismatch，而**绝不能**带着对不上的实现去重写清单。
  const checksumFailures = [];
  for (const entry of manifest.entries) {
    const file = path.join(packRoot, entry.rel.replace(/\//g, path.sep));
    if (!fs.existsSync(file)) { checksumFailures.push(entry.rel + ' (磁盘上没有)'); continue; }
    const bytes = fs.readFileSync(file);
    if (bytes.length !== entry.size) checksumFailures.push(entry.rel + ' (字节数 ' + bytes.length + ' ≠ 清单 ' + entry.size + ')');
    else if (fnv1a64Hex(bytes) !== entry.hash) checksumFailures.push(entry.rel + ' (哈希不符)');
  }
  if (checksumFailures.length > 0) {
    process.stderr.write('清单无法逐条复现（拒绝改动，先查 assetc 的清单格式是否变了）: ' +
      checksumFailures.length + '/' + manifest.entries.length + ' 条不符\n' +
      checksumFailures.slice(0, 10).map((f) => '  - ' + f).join('\n') + '\n');
    return 1;
  }

  // 第 2 步：只处理 `.lua`。其余后缀（png/ttf/toml/json/scene…）要么是二进制要么没有 Lua 注释。
  const luaEntries = manifest.entries.filter((e) => e.rel.endsWith('.lua'));
  const stripped = [];
  const lexErrors = [];
  let bytesBefore = 0, bytesAfter = 0;
  for (const entry of luaEntries) {
    const file = path.join(packRoot, entry.rel.replace(/\//g, path.sep));
    const source = fs.readFileSync(file, 'utf8');
    const result = stripLuaComments(source);
    if (result.error) { lexErrors.push(entry.rel + ': ' + result.error); continue; }
    // 行数不变是**承诺**而不是意图：不成立就整个工具失败，因为工程里有人按行号读发布包的报错。
    if (countNewlines(result.text) !== countNewlines(source)) {
      lexErrors.push(entry.rel + ': 剥注释后行数变了（' + countNewlines(source) + ' → ' +
        countNewlines(result.text) + '），拒绝写出');
      continue;
    }
    if (result.text !== source) {
      stripped.push({ entry, file, next: Buffer.from(result.text, 'utf8') });
      // 按**字节**而不是字符数算：注释是中文的，一个汉字 1 char 但 3 byte（utf8）。
      // 用 source.length 汇报会把 265 KB 的实际节省说成 140 KB——体积门看的正是字节。
      bytesBefore += Buffer.byteLength(source);
      bytesAfter += result.text.length;
    }
  }
  // 任何一份文件处理不了就**全部不落盘**：半成品产物（一部分剥了一部分没剥、清单只改了一半）
  // 比不剥更糟——它会以"首屏偶发校验失败"的形态在真机上出现。
  if (lexErrors.length > 0) {
    process.stderr.write('存在无法安全剥注释的 Lua（拒绝改动）:\n' +
      lexErrors.slice(0, 10).map((e) => '  - ' + e).join('\n') + '\n');
    return 1;
  }

  const saved = bytesBefore - bytesAfter;
  if (write && stripped.length > 0) {
    for (const item of stripped) fs.writeFileSync(item.file, item.next);
    // 重写清单：只改动了的文件更新字节数与哈希，其余行原样保留（含首行与制表符形状）。
    const lines = manifest.lines.slice();
    for (const item of stripped) {
      lines[item.entry.index] = item.entry.rel + '\t' + item.next.length + '\t' + fnv1a64Hex(item.next);
    }
    fs.writeFileSync(manifestPath, lines.join('\n'));
  }

  if (json) {
    process.stdout.write(JSON.stringify({
      ok: true, packRoot, wrote: write && stripped.length > 0,
      luaFiles: luaEntries.length, changedFiles: stripped.length,
      bytesBefore, bytesAfter, savedBytes: saved,
      savedKb: Math.round(saved / 1024), manifestEntries: manifest.entries.length,
    }) + '\n');
  } else {
    const verb = write && stripped.length > 0 ? '已剥注释' : '预计可省（未 --write）';
    process.stdout.write('strip_lua_comments ' + verb +
      ' lua=' + luaEntries.length + ' changed=' + stripped.length +
      ' saved=' + Math.round(saved / 1024) + 'KB manifest=' + manifest.entries.length + '\n');
  }
  return 0;
}

export { fnv1a64Hex, stripLuaComments, countNewlines, parseManifest };

// 只有"作为命令跑"才执行（被 import 时不碰磁盘）：这是 --check 门与自动化用例共用一份实现的代价。
const selfPath = path.resolve(fileURLToPath(import.meta.url));
const invokedDirectly = process.argv[1] !== undefined && path.resolve(process.argv[1]) === selfPath;
if (invokedDirectly) {
  try { process.exitCode = main(process.argv); }
  catch (error) { process.stderr.write('strip-lua-comments 失败: ' + error.stack + '\n'); process.exitCode = 1; }
}
