// 输入线格式（`RLFI`）键表的**静态一致性门**（不需要 wasm 产物）。
//
// ## 它防的是哪一类缺陷
// 批量 ABI 把键名从"每帧上线"改成"位图 + 固定键表"。于是**多了一个静默失效面**：
// 游戏绑定表里写了某个键，而线路键表里没有它 —— 那个键就永远收不到，症状是
// "某个键按了没反应"，只在该键上出现、只在该平台出现。第一次实测就真发生了：
// 绑定表里有 `volume_up = {'=', '+'}` / `volume_down = {'-', '_'}`，而键表漏了 `+` 与 `_`
// （Shift 之后的**字符形式**），也就是"Shift+= 调不了音量"。
//
// 这条门在**构建之前**把三份东西对起来（都不需要产物，纯静态）：
//   ① Rust `rustlua-frame-abi` 的 `KEY_NAMES`（线路位序的唯一真源）；
//   ② `web-runtime.js` 预置层的 `bindings` 表（游戏绑定了哪些键）；
//   ③ 宿主送进来的 DOM 键名（剧本/harness 里的 `key: '…'`）经 `keyName()` 归一后的结果。
// 任一处出现表外键名即红。另外顺手钉住"顺序即位序"这件事的周边约束：唯一、全小写、不超槽位。
//
// 退出码：0 通过；1 有断言失败；3 找不到源文件/抽不出块（环境问题，不是通过）。
import { readFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '..');

const abiPath = join(repoRoot, 'engine', 'crates', 'rustlua-frame-abi', 'src', 'lib.rs');
const runtimePath = join(repoRoot, 'plugins', 'webgl', 'web-runtime.js');
const harnessPath = join(repoRoot, 'scripts', 'wechat-minigame-harness.js');

for (const path of [abiPath, runtimePath]) {
  if (!existsSync(path)) {
    console.error(`missing source file: ${path}`);
    process.exit(3);
  }
}

const abiSource = readFileSync(abiPath, 'utf8');
const runtimeSource = readFileSync(runtimePath, 'utf8');

let failed = 0;
let passed = 0;
function check(label, condition, detail) {
  if (condition) {
    passed += 1;
    console.log(`  ok  ${label}`);
  } else {
    failed += 1;
    console.error(`  FAIL ${label}${detail ? ' -> ' + detail : ''}`);
  }
}

// Rust 字符串字面量抽取：**先跳过 `//` 注释**。注释里为了说明问题会写 `'+'`/`_`，
// 把注释里的名字当表内容会让这条门自欺（那种"注释里补上了"的通过最危险）。
function rustLiterals(source, startAnchor, endAnchor) {
  const start = source.indexOf(startAnchor);
  if (start < 0) throw new Error(`anchor not found: ${startAnchor}`);
  const endIndex = source.indexOf(endAnchor, start);
  if (endIndex < 0) throw new Error(`end anchor not found: ${endAnchor}`);
  const body = source.slice(start + startAnchor.length, endIndex);
  const out = [];
  for (let index = 0; index < body.length; index += 1) {
    if (body[index] === '/' && body[index + 1] === '/') {
      const lineEnd = body.indexOf('\n', index);
      index = lineEnd < 0 ? body.length : lineEnd;
      continue;
    }
    if (body[index] !== '"') continue;
    let cursor = index + 1;
    let value = '';
    while (cursor < body.length && body[cursor] !== '"') {
      if (body[cursor] === '\\') {
        // Rust 转义：`\\` → `\`、`\"` → `"`；其余按字面量收（键表里只有这两种）。
        value += body[cursor + 1];
        cursor += 2;
        continue;
      }
      value += body[cursor];
      cursor += 1;
    }
    out.push(value);
    index = cursor;
  }
  return out;
}

const keyNames = rustLiterals(abiSource, 'pub const KEY_NAMES: &[&str] = &[', '\n];');
const mouseNames = rustLiterals(abiSource, 'pub const MOUSE_NAMES: &[&str] = &[', '];');
const keySlots = Number((abiSource.match(/pub const KEY_SLOTS: usize = (\d+)/) || [])[1]);
const mouseSlots = Number((abiSource.match(/pub const MOUSE_SLOTS: usize = (\d+)/) || [])[1]);
const wireLen = Number((abiSource.match(/pub const WIRE_LEN: usize = (\d+)/) || [])[1]);

if (keyNames.length === 0 || !keySlots || !wireLen) {
  console.error('抽不出键表/槽位/线长（源码结构变了？）');
  process.exit(3);
}

console.log(`input ABI key table（${keyNames.length} 键 / ${keySlots} 槽，鼠标 ${mouseNames.length} 项）`);
console.log(`  ${keyNames.join(' ')}`);

// ---- ① 键表自洽 ----
check('键名全小写', keyNames.every((name) => name === name.toLowerCase()),
  keyNames.filter((name) => name !== name.toLowerCase()).join(','));
check('键名唯一', new Set(keyNames).size === keyNames.length,
  keyNames.filter((name, index) => keyNames.indexOf(name) !== index).join(','));
check('键名非空', keyNames.every((name) => name.length > 0));
check('键数不超槽位', keyNames.length <= keySlots, `${keyNames.length} > ${keySlots}`);
check('鼠标名不超槽位', mouseNames.length <= mouseSlots, `${mouseNames.length} > ${mouseSlots}`);
console.log(`  剩余键槽 ${keySlots - keyNames.length} 个（新增键名要同步 version+1 并跑 ABI 对拍）`);

// ---- ② 预置层绑定表（游戏绑定了哪些键）----
const bindingsStart = runtimeSource.indexOf('local bindings = {');
if (bindingsStart < 0) {
  console.error('抽不出 web-runtime.js 的 bindings 表');
  process.exit(3);
}
const bindingsEnd = runtimeSource.indexOf('\n}', bindingsStart);
const bindingsBody = runtimeSource.slice(bindingsStart, bindingsEnd);
const bindingKeys = [...bindingsBody.matchAll(/'([^']*)'/g)].map((match) => match[1]);
const bindingUnique = [...new Set(bindingKeys)].sort();
const missingFromTable = bindingUnique.filter((name) => !keyNames.includes(name));
check(`绑定表用到的键全在线路键表里（${bindingUnique.length} 个）`,
  missingFromTable.length === 0,
  `缺 ${missingFromTable.map((name) => `'${name}'`).join(', ')}`);

// ---- ③ 宿主送进来的 DOM 键名，经 `keyName()` 归一后必须在表里 ----
// 与 web-runtime.js 的 keyName 逐条对齐（' '→space、ArrowX→x、其余小写）。
function normalizeDomKey(key) {
  const lowered = String(key).toLowerCase();
  if (lowered === ' ') return 'space';
  if (lowered.startsWith('arrow')) return lowered.slice(5);
  return lowered;
}
const domKeys = ['a', 'd', 'w', 's', 'e', 'j', 'k', 'g', 't', 'q', 'i', 'p', 'b', 'f', 'x', 'u', 'l',
  'r', 'c', '=', '+', '-', '_', '1', '9', ' ', 'Escape', 'ArrowUp', 'ArrowDown', 'ArrowLeft',
  'ArrowRight', 'Shift', 'Enter', 'Tab'];
if (existsSync(harnessPath)) {
  const harnessSource = readFileSync(harnessPath, 'utf8');
  for (const match of harnessSource.matchAll(/key:\s*'([^']*)'/g)) domKeys.push(match[1]);
}
const normalized = [...new Set(domKeys.map(normalizeDomKey))].sort();
const missingDom = normalized.filter((name) => !keyNames.includes(name));
check(`宿主 DOM 键名归一后全在表里（${normalized.length} 个）`,
  missingDom.length === 0,
  `缺 ${missingDom.map((name) => `'${name}'`).join(', ')}`);

// 归一函数本身也要与运行时一致：抽运行时 keyName 的 " " 与 arrowX 两条特例做交叉验证。
const keyNameSource = runtimeSource.slice(
  runtimeSource.indexOf('const keyName = event =>'),
  runtimeSource.indexOf('window.addEventListener(\'keydown\''));
check('运行时的 keyName 归一规则与门内一致',
  keyNameSource.includes("=== ' '") && keyNameSource.includes("'space'")
  && keyNameSource.includes("startsWith('arrow')") && keyNameSource.includes('slice(5)'),
  'keyName 改了写法，这条门的归一函数需要同步');

console.log(`\nRESULT: ${passed} passed, ${failed} failed`);
if (failed > 0) {
  console.error('input_abi_key_parity_FAILED');
  process.exit(1);
}
console.log(`input_abi_key_parity_ok keys=${keyNames.length} bindings=${bindingUnique.length} ` +
  `dom=${normalized.length} wire_len=${wireLen}`);
