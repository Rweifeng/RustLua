// 每帧输入批量线格式（`RLFI`）的**语义对拍**门：新协议必须与旧协议逐字符等价。
//
// ## 为什么必须有这条
// M73-00 把"每帧把输入快照拼成一段 Lua 源码再 `lua_do_string` 编译执行"换成了"打包 164 字节
// 二进制、一次跨界、原地改常驻表"。省下的开销很容易量（`measure-lua-boundary-wasm.mjs`），
// 但**语义漂移**不会让任何东西变红：松开方向键后 `keys[name]` 若还留在 `true`，表现是
// "角色一直走"；边沿没清干净，表现是"点一次触发好几次"。这类缺陷只能在浏览器里靠手感发现。
// 所以这里给旧协议留一个 **oracle**，一串输入逐步走完两条路，每步比对 Lua 侧的可观察内容。
//
// ## 为什么 oracle 与"出厂打包器"都从源码里抽
// 两条路都必须是被测的那一份，而不是测试里另写的一份：
//   - oracle（旧协议）= `web-runtime.js` 里的 `luaQuote`/`luaValue`（出厂的源码序列化器）；
//   - 新协议 = `web-runtime.js` 里的 `getInputFramePacker`/`applyInputFrame`（出厂的打包器），
//     配上真 wasm 的 `lua_input_frame` 与 `input_frame_layout_json`。
// 任何一边在源码里改了而没同步，这条门就会红。
//
// ## 已知且有意的差异（不是缺陷，别"修好"它）
// 旧协议下 `axis_x` 是 Lua **整数**（`1`），新协议一律是 **浮点**（`1.0`）——线格式里就是 f64。
// 消费侧 `input.axis()` 本来就有 `+ 0.0`，所以对游戏不可见；本门把这件事**当断言写出来**
// （`math.type` 明确要求 'float'），这样它是"记录在案的行为"而不是"哪天冒出来的惊奇"。
//
// 退出码：0 全通过；1 有断言失败；3 胶水缺失或与 wasm 不同源（环境受限，不是通过）。
//
// 用法：node [--experimental-wasm-exnref] scripts/wasm-lua/input-frame-abi-test.mjs [glueDir]
import { readFileSync, existsSync, copyFileSync, appendFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '..', '..');
const runtimePath = join(repoRoot, 'plugins', 'webgl', 'web-runtime.js');
const glueDir =
  process.argv[2] ||
  process.env.RUSTLUA_GLUE_DIR ||
  join(repoRoot, 'artifacts', 'wasm-lua-verify', 'no-modules-test');

const glueJs = join(glueDir, 'game_web.js');
const glueWasm = join(glueDir, 'game_web_bg.wasm');
if (!existsSync(glueJs) || !existsSync(glueWasm)) {
  console.error(`no-modules glue not found in: ${glueDir}`);
  console.error('先跑 scripts/verify-wasm-lua.ps1（不带 -SkipBuild）生成胶水与 wasm。');
  process.exit(3);
}

let passed = 0;
const failures = [];
function check(label, condition, detail) {
  if (condition) {
    passed += 1;
    console.log(`  ok  ${label}`);
  } else {
    failures.push(label);
    console.error(`  FAIL ${label}${detail ? ' -> ' + detail : ''}`);
  }
}

const require = createRequire(import.meta.url);
const runtimeSource = readFileSync(runtimePath, 'utf8');

// ---- 从出厂源码抽块 ----
// 按**缩进**配对而不是数字花括号：这些函数体里有正则字面量（`luaQuote` 的 `.replace(/'/g…)`
// 里就有单引号），花括号计数会被字符串/正则里的引号带偏，抽出来的块残缺不全。
// 本文件的函数都是"同缩进的 `}` 收尾"，于是"下一行恰好等于 `缩进+}`"就是真正的结束。
function blockAt(source, anchor) {
  // 按 `\r?\n` 切：本仓的 JS 是 CRLF，直接按 '\n' 切会让每行尾部留一个 `\r`，
  // 于是"行内容恰好等于 `  }`"永远不成立（这道门第一次就是栽在这里）。
  const lines = source.split(/\r?\n/);
  const start = lines.findIndex((line) => line.includes(anchor));
  if (start < 0) throw new Error(`anchor not found: ${anchor}`);
  const indent = lines[start].match(/^ */)[0];
  const closing = `${indent}}`;
  for (let index = start + 1; index < lines.length; index += 1) {
    if (lines[index] === closing) return lines.slice(start, index + 1).join('\n');
  }
  throw new Error(`unbalanced block for anchor: ${anchor}`);
}

const oracleSource = [
  blockAt(runtimeSource, 'function luaQuote(text) {'),
  blockAt(runtimeSource, 'function luaValue(value) {'),
].join('\n');
// eslint-disable-next-line no-new-func -- 抽出厂源码当 oracle，见文件头的说明。
const oracle = new Function(`${oracleSource}\nreturn { luaValue };`)();

const packerSource = [
  'let inputFramePacker = null;',
  blockAt(runtimeSource, 'function getInputFramePacker() {'),
  blockAt(runtimeSource, 'function applyInputFrame(snapshot) {'),
  blockAt(runtimeSource, 'function inputStatsReadout() {'),
].join('\n');

// 旧协议建表语句（出厂的初始化那段，逐字取自 web-runtime.js）。
const declarationLine = runtimeSource
  .split('\n')
  .find((line) => line.trim().startsWith('__rustlua_input = { axis_x = 0'));
if (!declarationLine) throw new Error('未找到 __rustlua_input 初始化语句');

// ---- 载入 wasm（与桥验证同款 CommonJS 补丁）----
const patchedGlue = join(glueDir, 'game_web.patched.js');
copyFileSync(glueJs, patchedGlue);
appendFileSync(patchedGlue, '\nmodule.exports = wasm_bindgen;\n');
const glue = require(patchedGlue);
const wasmBytes = readFileSync(glueWasm);
try {
  glue.initSync({ module: wasmBytes });
} catch (error) {
  glue.initSync(wasmBytes);
}
const required = ['lua_boot_with_reader', 'lua_do_string', 'lua_input_frame', 'input_frame_layout_json'];
const missing = required.filter((name) => typeof glue[name] !== 'function');
if (missing.length > 0) {
  console.error(`glue is stale or mismatched（缺 ${missing.join(',')}）：` +
    'verify-wasm-lua.ps1 -SkipBuild 会跳过胶水生成，请不带该开关重跑。');
  process.exit(3);
}

glue.lua_boot_with_reader(() => null);
// 新旧两条路都要能被读出来：dump 里必须区分 nil / false / number，且按键名排序（确定性）。
glue.lua_do_string(`
local function __fmt(v)
  if v == nil then return 'nil' end
  if type(v) == 'boolean' then return tostring(v) end
  if type(v) == 'number' then return string.format('%.17g', v) end
  return type(v)
end
local function __pairs_sorted(t)
  local names = {}
  for k in pairs(t) do names[#names + 1] = tostring(k) end
  table.sort(names)
  local parts = {}
  for i = 1, #names do parts[i] = names[i] .. '=' .. __fmt(t[names[i]]) end
  return table.concat(parts, ',')
end
function __dump_input()
  local out = {}
  out[#out + 1] = 'axis_x=' .. __fmt(__rustlua_input.axis_x)
  out[#out + 1] = 'axis_y=' .. __fmt(__rustlua_input.axis_y)
  out[#out + 1] = 'cursor=' .. __pairs_sorted(__rustlua_input.cursor)
  out[#out + 1] = 'press_cursor=' .. __pairs_sorted(__rustlua_input.press_cursor)
  out[#out + 1] = 'cursor_delta=' .. __pairs_sorted(__rustlua_input.cursor_delta)
  out[#out + 1] = 'scroll=' .. __pairs_sorted(__rustlua_input.scroll)
  out[#out + 1] = 'keys=' .. __pairs_sorted(__rustlua_input.keys)
  out[#out + 1] = 'just=' .. __pairs_sorted(__rustlua_input.just)
  out[#out + 1] = 'released=' .. __pairs_sorted(__rustlua_input.released)
  out[#out + 1] = 'mouse=' .. __pairs_sorted(__rustlua_input.mouse)
  out[#out + 1] = 'mouse_just=' .. __pairs_sorted(__rustlua_input.mouse_just)
  out[#out + 1] = 'mouse_just_released=' .. __pairs_sorted(__rustlua_input.mouse_just_released)
  return table.concat(out, '|')
end
function __axis_type() return math.type(__rustlua_input.axis_x) end
`, '__abi_dump');

const dump = () => String(glue.lua_call_global('__dump_input', '[]') || '');
const reset = () => glue.lua_do_string(declarationLine, '__rustlua_input');

// 假 window：出厂的打包器只从这里取桥（与运行时同一入口）。
const fakeWindow = { __RUSTLUA_LUA_BRIDGE: glue };
const factory = new Function('window', `${packerSource}\nreturn { getInputFramePacker, inputStatsReadout };`)(fakeWindow);
const packer = factory.getInputFramePacker();

// ---- 快照构造 ----
// **必须与 `createWebInput` 同构**：真实运行时的 `state` 是一份**常驻对象**，每帧只是改字段，
// `snapshot()` 把 `state.keys` 等**按引用**放进返回对象。所以 `keys` 松开后保留 `false` 项、
// 并且这份 `false` 项会出现在**后续每一帧**的快照里——第一版测试每帧新建对象，于是
// "多键同时松开"那组第 4 帧出现了"新的比旧的多几个 false 项"的假差异。
// 差异是测试数据的锅，不是协议的锅；这条门正是靠"两边都要像出厂那样"才有意义。
function inputState() {
  return {
    axis_x: 0,
    axis_y: 0,
    keys: {},
    just: {},
    released: {},
    mouse: {},
    // 内部命名与 `createWebInput` 的 state 一致（`mouseJust`/`mouseJustReleased`），
    // `takeSnapshot` 再映射成快照字段名。第一版这里写成了快照名，于是 `mouse_just` 变成 nil。
    mouseJust: {},
    mouseJustReleased: {},
    cursor: [0, 0],
    press_cursor: [0, 0],
    cursor_delta: [0, 0],
    scroll: [0, 0],
  };
}

// 与 createWebInput 的事件处理同款：按下记边沿 + 置 true；松开时**保留 false 项**。
function press(state, key) {
  if (!state.keys[key]) state.just[key] = true;
  state.keys[key] = true;
}
function release(state, key) {
  if (state.keys[key]) state.released[key] = true;
  state.keys[key] = false;
}
function mousePress(state, button) {
  state.mouseJust[button] = true;
  state.mouse[button] = true;
}
function mouseRelease(state, button) {
  if (state.mouse[button]) state.mouseJustReleased[button] = true;
  state.mouse[button] = false;
}
// snapshot()：边沿集合按引用交出后即被清空，与出厂一致。
function takeSnapshot(state) {
  const out = {
    axis_x: state.axis_x,
    axis_y: state.axis_y,
    keys: state.keys,
    just: state.just,
    released: state.released,
    mouse: state.mouse,
    mouse_just: state.mouseJust,
    mouse_just_released: state.mouseJustReleased,
    cursor: state.cursor,
    press_cursor: state.press_cursor,
    cursor_delta: state.cursor_delta,
    scroll: state.scroll,
  };
  state.just = {};
  state.released = {};
  state.mouseJust = {};
  state.mouseJustReleased = {};
  return out;
}

// 每组序列 = 一串"改状态"的动作，动作之间取一次快照（对应一次固定步）。
function sequence(name, steps) {
  return { name, steps };
}

const sequences = [
  sequence('按住跨帧再松开（差分必须清掉旧键）', [
    (s) => { press(s, 'w'); },
    () => {},
    (s) => { press(s, 'space'); },
    (s) => { release(s, 'w'); },
    () => {},
  ]),
  sequence('边沿只活一帧（just/released 必须逐帧消失）', [
    (s) => { press(s, 'space'); },
    () => {},
    (s) => { release(s, 'space'); },
    () => {},
    (s) => { press(s, 'escape'); press(s, 'enter'); },
    (s) => { release(s, 'enter'); },
  ]),
  sequence('标量与两项表（含负数/小数/零回收）', [
    (s) => { s.axis_x = 1; s.axis_y = -1; s.cursor = [627, 318]; s.cursor_delta = [3, -2]; },
    (s) => { s.cursor = [630.5, 316.25]; s.cursor_delta = [3.5, -1.75]; },
    (s) => { s.axis_x = 0; s.axis_y = 0; s.cursor = [0.1, 0.2]; s.scroll = [0.5, -8]; },
    () => {},
  ]),
  sequence('鼠标按钮与边沿', [
    (s) => { mousePress(s, 'left'); s.press_cursor = [100, 200]; },
    () => {},
    (s) => { mouseRelease(s, 'left'); },
    (s) => { mousePress(s, 'right'); },
    (s) => { mouseRelease(s, 'right'); },
  ]),
  sequence('多个键同时按下/松开 + 虚拟键并回', [
    (s) => { press(s, 'w'); press(s, 'a'); press(s, 'shift'); },
    (s) => { press(s, 'd'); },
    (s) => { release(s, 'w'); release(s, 'shift'); },
    (s) => { release(s, 'a'); release(s, 'd'); },
  ]),
];

console.log(`input frame ABI parity（wasm=${wasmBytes.length} 字节，序列=${sequences.length}）`);
for (const item of sequences) {
  // A：旧协议（每帧编译源码）。每次都从出厂初始化语句起步，保证两条路起点相同。
  reset();
  const oracleState = inputState();
  const oracleDumps = [];
  for (const step of item.steps) {
    step(oracleState);
    glue.lua_do_string('__rustlua_input = ' + oracle.luaValue(takeSnapshot(oracleState)),
      '__rustlua_input');
    oracleDumps.push(dump());
  }
  // B：新协议（批量二进制，原地改常驻表）。先把差分基准压回零，再重置表。
  reset();
  packer.pack(takeSnapshot(inputState()));
  glue.lua_input_frame(packer.pack(takeSnapshot(inputState())));
  reset();
  const abiState = inputState();
  const abiDumps = [];
  const touchedCounts = [];
  for (const step of item.steps) {
    step(abiState);
    touchedCounts.push(glue.lua_input_frame(packer.pack(takeSnapshot(abiState))) >>> 0);
    abiDumps.push(dump());
  }
  let firstDiff = -1;
  for (let index = 0; index < oracleDumps.length; index += 1) {
    if (oracleDumps[index] !== abiDumps[index]) {
      firstDiff = index;
      break;
    }
  }
  check(`序列等价：${item.name}`, firstDiff < 0,
    firstDiff < 0
      ? ''
      : `第 ${firstDiff + 1} 帧不同\n      旧: ${oracleDumps[firstDiff]}\n      新: ${abiDumps[firstDiff]}`);
  const changed = touchedCounts.filter((count) => count > 0).length;
  console.log(`     改动帧 ${changed}/${touchedCounts.length}，touched=[${touchedCounts.join(',')}]`);
}

// 已知差异：轴在旧协议下是整数、新协议是浮点（消费侧 `input.axis()` 有 `+ 0.0`）。
reset();
{
  const state = inputState();
  state.axis_x = 1;
  glue.lua_do_string('__rustlua_input = ' + oracle.luaValue(takeSnapshot(state)), 'oracle-axis');
}
const oracleAxisType = String(glue.lua_call_global('__axis_type', '[]'));
reset();
{
  const state = inputState();
  state.axis_x = 1;
  glue.lua_input_frame(packer.pack(takeSnapshot(state)));
}
const abiAxisType = String(glue.lua_call_global('__axis_type', '[]'));
check('轴的数值类型差异是有意的（旧=integer，新=float）',
  oracleAxisType === 'integer' && abiAxisType === 'float',
  `旧=${oracleAxisType} 新=${abiAxisType}`);

// 负例：畸形帧必须响亮失败，而不是"尽量解释"。
function expectThrow(label, bytes) {
  let thrown = null;
  try {
    glue.lua_input_frame(bytes);
  } catch (error) {
    thrown = String((error && error.message) || error);
  }
  check(`畸形帧被拒：${label}`, thrown !== null, thrown === null ? '没有抛错' : thrown);
}
const layout = JSON.parse(glue.input_frame_layout_json());
const good = new Uint8Array(packer.pack(takeSnapshot(inputState())));
expectThrow('长度不足', good.slice(0, layout.wire_len - 1));
expectThrow('长度超出', new Uint8Array(layout.wire_len + 1));
const badMagic = Uint8Array.from(good);
badMagic[layout.offsets.magic] = 0x58;
expectThrow('magic 不符', badMagic);
const badVersion = Uint8Array.from(good);
badVersion[layout.offsets.version] = 2;
expectThrow('版本不符', badVersion);
// 表外位：宿主用了新键而 wasm 还是旧表 → 必须报出来，不能当成"没有这个键"。
// 位地址 = 字节偏移 + slot/8，位内偏移 = slot%8（第一版写成 `(slot-1)/8` 得到小数，
// `Uint8Array[15.875] |= …` 是静默 no-op，于是"应该抛错"的用例假通过）。
const outside = Uint8Array.from(good);
const outsideSlot = layout.key_slots - 1;
outside[layout.offsets.keys_held + (outsideSlot >> 3)] |= 1 << (outsideSlot & 7);
expectThrow('表外键位', outside);
check('布局自报长度与打包器一致', layout.wire_len === good.length,
  `layout=${layout.wire_len} packed=${good.length}`);
check('键表包含游戏绑定表用到的键',
  ['w', 'a', 's', 'd', 'e', 'j', 'k', 'g', 't', 'q', 'i', 'p', 'b', 'f', 'x', 'u', 'l', 'r', 'c',
    '=', '+', '-', '_', 'space', 'escape', 'up', 'down', 'left', 'right', '1', '9', '0']
    .every((name) => layout.key_names.includes(name)));

// 落不进键表的键名必须**被计数**（而不是静默丢弃）。
packer.pack(takeSnapshot(inputState()));
{
  const state = inputState();
  state.keys.dead = true;
  packer.pack(takeSnapshot(state));
}
const statsText = factory.inputStatsReadout();
check('表外键名被计数', /unmapped=[1-9]/.test(statsText) && statsText.includes('dead'), statsText);

console.log(`\nRESULT: ${passed} passed, ${failures.length} failed`);
if (failures.length > 0) {
  console.error('input_frame_abi_FAILED');
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log('input_frame_abi_ok sequences=' + sequences.length + ' wasm=' + wasmBytes.length);
