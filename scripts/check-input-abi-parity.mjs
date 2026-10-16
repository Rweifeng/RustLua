// 输入线格式（`RLFI`）键表的**静态一致性门**（不需要 wasm 产物）。
//
// ## 它防的是哪一类缺陷
// 批量 ABI 把键名从"每帧上线"改成"位图 + 固定键表"。于是**多了一个静默失效面**：
// 某个键在某一侧写得出来、另一侧不认识 —— 那个键就永远收不到，症状是"某个键按了没反应"，
// 只在该键上出现、只在该平台出现。第一次实测就真发生了：
// 绑定表里有 `volume_up = {'=', '+'}` / `volume_down = {'-', '_'}`，而键表漏了 `+` 与 `_`
// （Shift 之后的**字符形式**），也就是"Shift+= 调不了音量"。
//
// ## B8 之后它检查什么（三条腿，都在**活着的**名字空间里）
//   ① Rust `rustlua-frame-abi` 的 `KEY_NAMES` 自洽（线路位序的唯一真源：全小写、唯一、不超槽位）；
//   ② **工程绑定**：每个 `examples/*/project.toml` 的 `[input] map` 里 `key:<名>` 必须被引擎的
//      键名表认识（`rustlua-input/src/input.rs` 的 `define_key_codes!` 那张 `KeyCode => "name"` 表）。
//      这条是 **B8 新加的**：改之前这里比的是 `web-runtime.js` 预置层的 `bindings` 表，而那个表随
//      旧世界消失；而工程绑定走的是**另一套名字空间**（winit 风格 `key:keyj`，见 `project.toml` 的
//      注释），原来**没有任何静态门**看着它 —— 现在它比原来那条更贴近"真会发出去的东西"。
//   ③ **宿主 DOM 键名**：剧本/harness 里出现的 `key: '…'` 与常见 DOM 值，经归一后在 `KEY_NAMES` 里。
//      归一规则在门内**另写一份**（独立副本才有意义），并与 Rust 的 `frame_input::wire_key_name`
//      交叉验证（那才是生产实现，且它自己有 `dom_key_names_normalize_exactly_like_the_js_host` 测试）。
//
// 退出码：0 通过；1 有断言失败；3 找不到源文件/抽不出块（环境问题，不是通过）。
import { readFileSync, existsSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '..');

const abiPath = join(repoRoot, 'engine', 'crates', 'rustlua-frame-abi', 'src', 'lib.rs');
const engineKeysPath = join(repoRoot, 'engine', 'crates', 'rustlua-input', 'src', 'input.rs');
const wireKeyPath = join(repoRoot, 'engine', 'crates', 'game_web', 'src', 'frame_input.rs');
const harnessPath = join(repoRoot, 'scripts', 'wechat-minigame-harness.js');
const examplesRoot = join(repoRoot, 'examples');

for (const path of [abiPath, engineKeysPath, wireKeyPath]) {
  if (!existsSync(path)) {
    console.error(`missing source file: ${path}`);
    process.exit(3);
  }
}

const abiSource = readFileSync(abiPath, 'utf8');
const engineKeysSource = readFileSync(engineKeysPath, 'utf8');
const wireKeySource = readFileSync(wireKeyPath, 'utf8');

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

// ---- ② 工程绑定：`project.toml` 的 `[input] map` 里 key: 名字，引擎必须认识 ----
// 名字表 = `define_key_codes!(` 里那张 `KeyCode => "name"`（宏体，逐行 `Name => "name"`）。
const engineKeyNames = rustLiterals(engineKeysSource, 'define_key_codes!(', '\n);');
check(`引擎键名表抽得出来（${engineKeyNames.length} 项）`, engineKeyNames.length > 20);
const engineKeySet = new Set(engineKeyNames);

const projectMaps = [];
for (const entry of readdirSync(examplesRoot)) {
  const toml = join(examplesRoot, entry, 'project.toml');
  if (!existsSync(toml)) continue;
  const text = readFileSync(toml, 'utf8');
  // 只认 `[input]` 段里的 `map = "..."`（TOML 基本串：把 \n 还原成真换行）。
  const section = text.slice(text.indexOf('[input]'));
  const mapMatch = section.match(/^\s*map\s*=\s*"([^"]*)"/m);
  if (!mapMatch) continue;
  const body = mapMatch[1].replace(/\\n/g, '\n');
  const keyTokens = [];
  for (const line of body.split('\n')) {
    for (const part of line.split(',')) {
      // 一个条目形如 `<动作>=key:<名>`（可再跟 `,pad:2` / `,padaxis:1+`）。**动作名在前**，
      // 所以不能按 `key:` 开头判 —— 第一版就这么写，抽出 0 个键。
      const token = part.trim();
      const at = token.indexOf('key:');
      if (at < 0) continue;
      const name = token.slice(at + 'key:'.length).match(/^[A-Za-z0-9]+/);
      if (name) keyTokens.push(name[0]);
    }
  }
  projectMaps.push({ name: entry, keys: keyTokens });
}
const allProjectKeys = [...new Set(projectMaps.flatMap((project) => project.keys))].sort();
check(`工程 project.toml 的 [input] 抽得出来（${projectMaps.length} 个工程 / ${allProjectKeys.length} 个键）`,
  projectMaps.length > 0 && allProjectKeys.length > 0);
const unknownProjectKeys = allProjectKeys.filter((name) => !engineKeySet.has(name));
check(`工程绑定的键名引擎全认识（${allProjectKeys.length} 个）`,
  unknownProjectKeys.length === 0,
  `不认识 ${unknownProjectKeys.map((name) => `'${name}'`).join(', ')}（引擎键名表在 rustlua-input/src/input.rs）`);

// ---- ③ 宿主送进来的 DOM 键名，经归一后必须在 KEY_NAMES 里 ----
// 归一规则：' '→space、ArrowX→x、其余小写。**门内独立写一份**（复制运行时那份就等于测自己）。
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

// 归一规则必须与**生产实现**一致。B8 之前这里读的是 `web-runtime.js` 的 `keyName`；现在那条路
// 已经删掉，生产实现是 Rust 的 `frame_input::wire_key_name`（浏览器/微信都走它），所以交叉验证
// 换成 Rust 源码的两条特例（它自己另有 `dom_key_names_normalize_exactly_like_the_js_host` 测试）。
check('Rust wire_key_name 的两条特例仍在（空格与方向键）',
  wireKeySource.includes('" "') && wireKeySource.includes('"space"') && /arrow/i.test(wireKeySource),
  'frame_input.rs 的 wire_key_name 改了写法，本门的归一函数需要同步');

console.log(`\nRESULT: ${passed} passed, ${failed} failed`);
if (failed > 0) {
  console.error('input_abi_key_parity_FAILED');
  process.exit(1);
}
console.log(`input_abi_key_parity_ok keys=${keyNames.length} project_keys=${allProjectKeys.length} ` +
  `dom=${normalized.length} wire_len=${wireLen}`);
