// wasm 侧跨边界成本基线（M73-00 WP0）——原生读数的对应物。
//
// 为什么必须单独测一遍：WP3（批量二进制 ABI）的设计判据是"现行每帧协议有多贵"，
// 而**原生读数不能直接外推**——Lua VM 在 wasm 里是提前编译好的机器码（没有 JIT、
// 没有内联、调用要过 wasm-bindgen 的胶水编组），通常比原生更贵，但贵多少没测就不能说。
// 这份脚本在 Node/V8 里加载**真实发布口径的 wasm**（no-modules 胶水，与微信同形态）测同一批操作，
// 于是原生那份 `engine/crates/rustlua-scripting/examples/lua_boundary_cost.rs` 有了可比对象。
//
// 量的东西（与原生基线逐条对应）：
//   ① 写 `__rustlua_dt`：编译一小段源码（旧做法） vs `lua_set_number` 直接写全局（本次已改）
//   ② 注入输入快照：编译 326 字节源码（**现行每帧协议**）
//   ③ 空 chunk 编译（底价：把"编译"与"建表"分开）
//   ④ Lua 侧自建同一张输入表 N 次（一次跨界）——用 ② − ④ 隔离出"编译+编组"那一段
//   ⑤ `lua_call_global` 走 JSON 参数（运行期另一条逐帧协议）
// 每个操作同时给 **µs/次** 与 **Lua 堆字节/次**（停用增量 GC 后量堆增长）：原生那份基线已经
// 证明"值钱的是分配量不是 CPU"，所以 wasm 侧同样必须两项都量。
//
// 用法：node [--experimental-wasm-exnref] scripts/lib/wasm-lua/measure-lua-boundary-wasm.mjs [glueDir]
//   胶水目录解析顺序：argv[2] → $RUSTLUA_GLUE_DIR → <repo>/artifacts/wasm-lua-verify/no-modules-test
//   （该目录由 scripts/verify/verify-wasm-lua.ps1 生成，gitignored。）
//
// 退出码：0 = 量完并输出；1 = 断言失败；3 = 胶水缺失或**与 wasm 不同源**（环境受限）。
//
// ⚠ 胶水不同源是这个脚本最容易踩的坑：`verify-wasm-lua.ps1 -SkipBuild` 会**连胶水生成一起跳过**，
//   于是"新 wasm + 旧胶水"能跑出 11 个看起来像真回归的失败（实测 2026-10-08）。这里显式检查
//   `lua_set_number` 存在，把这类不同源当场拦下。
import { readFileSync, existsSync, copyFileSync, appendFileSync } from 'node:fs';import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '..', '..', '..');
const glueDir =
  process.argv[2] ||
  process.env.RUSTLUA_GLUE_DIR ||
  join(repoRoot, 'artifacts', 'wasm-lua-verify', 'no-modules-test');

const glueJs = join(glueDir, 'game_web.js');
const glueWasm = join(glueDir, 'game_web_bg.wasm');
if (!existsSync(glueJs) || !existsSync(glueWasm)) {
  console.error(`no-modules glue not found in: ${glueDir}`);
  console.error('先跑 scripts/verify/verify-wasm-lua.ps1（不带 -SkipBuild）生成胶水与 wasm。');
  process.exit(3);
}

const require = createRequire(import.meta.url);

// 与 package.ps1 的微信补丁同款：no-modules 胶水的 `let wasm_bindgen` 是模块作用域绑定，
// CommonJS require 拿不到，末尾补一句 module.exports 才能 require 到。
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

// 断言桥面完整：缺任何一个都说明胶水与 wasm 不同源，量出来的数没有意义。
const required = ['lua_boot_with_reader', 'lua_do_string', 'lua_set_number', 'lua_call_global'];
const missing = required.filter((name) => typeof glue[name] !== 'function');
if (missing.length > 0) {
  console.error(`glue is stale or mismatched（缺 ${missing.join(',')}）：` +
    'verify-wasm-lua.ps1 -SkipBuild 会跳过胶水生成，请不带该开关重跑。');
  process.exit(3);
}

glue.lua_boot_with_reader(() => null);

// M73-00 第二片（每帧源码编译清零）的对照物：两个**与被测钩子同形**的 Lua 全局——
// 一个无参无返回（`__rustlua_apply_virtual_input` 的形状），一个无参但返回字符串
// （`__rustlua_export_ui` 的形状）。用它们来量"同一件事"的旧写法（编译一小段固定源码）
// 与新写法（直接调用），所以两组数字可以直接相减。
//
// 为什么不直接调真钩子：真钩子由工程预置层定义，而这里只加载了 wasm。用同形替身量的是
// **跨界与编译的固定开销**，那才是这一片要消掉的东西。
glue.lua_do_string(
  "__m73_hook = function() end\n__m73_ui = function() return 'ui' end",
  'm73-second-slice-setup',
);
if (typeof glue.lua_call_global_void !== 'function') {
  console.error('glue is stale or mismatched（缺 lua_call_global_void）：' +
    'verify-wasm-lua.ps1 -SkipBuild 会跳过胶水生成，请不带该开关重跑。');
  process.exit(3);
}

// ---- 与被测协议同形的输入（与 lua_boundary_cost.rs 的 input_snapshot_source 逐字节一致）----
const INPUT_FIELDS = '["axis_x"]=1,["axis_y"]=-1,'
  + '["keys"]={["w"]=true,["d"]=true,["space"]=true},'
  + '["just"]={["space"]=true},'
  + '["released"]={},'
  + '["mouse"]={["left"]=false,["right"]=false},'
  + '["mouse_just"]={["left"]=true},'
  + '["mouse_just_released"]={},'
  + '["cursor"]={627,318},'
  + '["press_cursor"]={627,318},'
  + '["cursor_delta"]={3,-2},'
  + '["scroll"]={0,0}';
const INPUT_SOURCE = `__rustlua_input = {${INPUT_FIELDS}}`;
const DT_SOURCE = '__rustlua_dt = 0.016666666666666666';
const DT_VALUE = 1 / 60;

// 原生基线（cargo run --release -p rustlua-scripting --example lua_boundary_cost，MSVC）留在这里
// 只为**打印对比**，不作为判据：跨机器、跨宿主，比值才是信息。
const NATIVE = {
  'dt 源码编译': 1.78,
  'dt 直接写全局': 0.107,
  '输入快照 源码编译(326B)': 8.41,
  '空 chunk 编译': 1.45,
};

if (Buffer.byteLength(INPUT_SOURCE) !== 326) {
  console.error(`输入快照源码是 ${Buffer.byteLength(INPUT_SOURCE)} 字节，原生基线是 326 字节——` +
    '两边不同形就没有可比性，先对齐再量。');
  process.exit(1);
}

const g = (args) => glue.lua_call_global('collectgarbage', JSON.stringify(args));
const luaHeapKb = () => Number(g(['count']));

const failures = [];
function check(label, condition, detail) {
  if (condition) console.log(`  ok  ${label}`);
  else { failures.push(label); console.error(`  FAIL ${label}${detail ? ' -> ' + detail : ''}`); }
}

// ---- 计时：每项跑 ROUNDS 轮，取中位数（与原生那份同口径）----
const ITERATIONS = 3000;
const ROUNDS = 5;
function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
}
function timeOp(run) {
  for (let index = 0; index < 200; index += 1) run(); // 预热：让胶水与 Lua 侧都热起来
  const rounds = [];
  for (let round = 0; round < ROUNDS; round += 1) {
    const start = process.hrtime.bigint();
    for (let index = 0; index < ITERATIONS; index += 1) run();
    rounds.push(Number(process.hrtime.bigint() - start) / 1000 / ITERATIONS);
  }
  return median(rounds);
}

// ---- 分配：停用增量 GC 再量堆增长（原生那份踩过"锯齿振幅当分配量"的坑）----
function bytesPerOp(run, iterations) {
  g(['collect']);
  g(['stop']);
  const before = luaHeapKb();
  for (let index = 0; index < iterations; index += 1) run();
  const after = luaHeapKb();
  g(['restart']);
  g(['collect']);
  return ((after - before) * 1024) / iterations;
}

const OPS = [
  { name: 'dt 源码编译', run: () => glue.lua_do_string(DT_SOURCE, 'm73-dt') },
  { name: 'dt 直接写全局', run: () => glue.lua_set_number('__rustlua_dt', DT_VALUE) },
  { name: '输入快照 源码编译(326B)', run: () => glue.lua_do_string(INPUT_SOURCE, 'm73-input') },
  { name: '空 chunk 编译', run: () => glue.lua_do_string('', 'm73-empty') },
  { name: '全局调用 JSON 参数', run: () => glue.lua_call_global('type', '["__rustlua_dt"]') },
  // M73-00 第二片：每帧最后两处"为调一个无参函数而编译源码"的清零对照。
  // 「空 JSON 数组」与「空串」是同一个调用的两种写法，所以这两行可以直接相减。
  { name: '无参钩子 源码编译(旧)', run: () => glue.lua_do_string('__m73_hook()', 'm73-virtual') },
  { name: '无参钩子 直接调用(新)', run: () => glue.lua_call_global_void('__m73_hook') },
  { name: '全局调用 空JSON数组', run: () => glue.lua_call_global('__m73_ui', '[]') },
  { name: '全局调用 空串无参(新)', run: () => glue.lua_call_global('__m73_ui', '') },
];

// M73-00 的新协议：批量二进制（`RLFI`）+ 原地改常驻表。**必须和上面那条同源**：
// 帧**按 wasm 自己给的布局现搭**（不再从出厂源码抽打包器，见下），应用用真 wasm 的 `lua_input_frame`。
// 这一组数字就是"换掉每帧源码编译"到底省了多少的答案。
let abiAvailable = false;
if (typeof glue.lua_input_frame === 'function' && typeof glue.input_frame_layout_json === 'function') {
  // **不再从出厂源码抽打包器**：那要读 `plugins/webgl/web-runtime.js`，而那个文件正是"删旧世界"要删的
  // （B8 的删除清单第 4 条）。改成**按 wasm 自己给的布局现搭一个合法帧** —— 被测物没变（仍是真 wasm 的
  // `lua_input_frame` 跨界解码），而构造方式不可能与布局漂移：它就是照布局写的。
  const layout = JSON.parse(glue.input_frame_layout_json());
  const off = layout.offsets;
  const keySlot = new Map(layout.key_names.map((name, index) => [name, index]));
  const mouseSlot = new Map(layout.mouse_names.map((name, index) => [name, index]));
  function buildAbiFrame() {
    const buffer = new Uint8Array(layout.wire_len);
    const view = new DataView(buffer.buffer);
    for (let index = 0; index < layout.magic.length; index += 1) {
      view.setUint8(off.magic + index, layout.magic.charCodeAt(index));
    }
    view.setUint16(off.version, layout.version, true);
    view.setUint16(off.flags, 0, true);
    view.setUint32(off.frame, 1, true);
    view.setFloat64(off.axis, 1, true);
    view.setFloat64(off.axis + 8, -1, true);
    view.setFloat64(off.cursor, 627, true);
    view.setFloat64(off.cursor + 8, 318, true);
    view.setFloat64(off.press_cursor, 627, true);
    view.setFloat64(off.press_cursor + 8, 318, true);
    view.setFloat64(off.cursor_delta, 3, true);
    view.setFloat64(off.cursor_delta + 8, -2, true);
    view.setFloat64(off.scroll, 0, true);
    view.setFloat64(off.scroll + 8, 0, true);
    // 键位图是 u128：DataView 没有 u128 写入口，按低/高两个 u64 小端写。
    const held = ['w', 'd', 'space'].reduce((bits, name) => bits | (1n << BigInt(keySlot.get(name))), 0n);
    view.setBigUint64(off.keys_held, held & 0xffffffffffffffffn, true);
    view.setBigUint64(off.keys_held + 8, held >> 64n, true);
    const pressed = 1n << BigInt(keySlot.get('space'));
    view.setBigUint64(off.keys_pressed, pressed & 0xffffffffffffffffn, true);
    view.setBigUint64(off.keys_pressed + 8, pressed >> 64n, true);
    view.setBigUint64(off.mouse_just, 1n << BigInt(mouseSlot.get('left')), true);
    return buffer;
  }
  OPS.push({
    name: '输入快照 批量二进制(新)',
    run: () => glue.lua_input_frame(buildAbiFrame()),
  });
  abiAvailable = true;
}


// ④ Lua 侧自建同一张表 N 次（一次跨界）：用来把"编译+编组"从"建表"里分出来。
//    声明与测量都在下面（`perBuildUs`），这里只留位置说明，避免两处声明。

console.log(`wasm 跨边界成本（host=node ${process.version}, wasm=${wasmBytes.length} 字节, ` +
  `每项 ${ROUNDS} 轮 × ${ITERATIONS} 次取中位数）`);
console.log('  操作                              µs/次    Lua堆B/次   原生µs/次   倍数');
const rows = [];
for (const op of OPS) {
  const us = timeOp(op.run);
  const bytes = bytesPerOp(op.run, 1500);
  const native = NATIVE[op.name];
  rows.push({ name: op.name, us, bytes, native });
  console.log(`  ${op.name.padEnd(30)} ${us.toFixed(3).padStart(7)} ${bytes.toFixed(0).padStart(11)} ` +
    `${(native === undefined ? '-' : native.toFixed(3)).padStart(10)} ` +
    `${(native === undefined ? '-' : (us / native).toFixed(2) + '×').padStart(7)}`);
}

// 建表本身（无跨界编译）的成本：一次跨界里让 Lua 自己建同一张表 N 次，再摊到每次。
// 与 ② 相减即"编译 + 跨界编组"那一段——那正是批量 ABI 能省掉的部分。
const LUA_BUILD_LOOP = 5000;
const buildLoopSource = `for __i = 1, ${LUA_BUILD_LOOP} do __rustlua_input = {${INPUT_FIELDS}} end`;
function perBuildUs() {
  for (let index = 0; index < 3; index += 1) glue.lua_do_string(buildLoopSource, 'm73-build-loop');
  const calls = 20;
  const start = process.hrtime.bigint();
  for (let index = 0; index < calls; index += 1) glue.lua_do_string(buildLoopSource, 'm73-build-loop');
  return Number(process.hrtime.bigint() - start) / 1000 / (calls * LUA_BUILD_LOOP);
}
const buildUs = perBuildUs();
const inputRow = rows.find((row) => row.name === '输入快照 源码编译(326B)');
const dtSourceRow = rows.find((row) => row.name === 'dt 源码编译');
const dtSetRow = rows.find((row) => row.name === 'dt 直接写全局');
console.log(`  ${'（② 之内）Lua 侧建同一张表'.padEnd(30)} ${buildUs.toFixed(3).padStart(7)} ` +
  `${'-'.padStart(11)} ${'-'.padStart(10)} ${'-'.padStart(7)}`);
console.log(`  ⇒ ② 里"编译+跨界编组"那一段 = ${(inputRow.us - buildUs).toFixed(3)} µs/次` +
  `（占 ② 的 ${(((inputRow.us - buildUs) / inputRow.us) * 100).toFixed(0)}%）`);
console.log(`  ⇒ dt：源码编译 ${dtSourceRow.us.toFixed(3)} µs → 直接写全局 ${dtSetRow.us.toFixed(3)} µs` +
  `（${(dtSourceRow.us / dtSetRow.us).toFixed(1)}×，原生是 ${(1.78 / 0.107).toFixed(1)}×）`);

// ---- 判据 ----
check('dt 直接写全局比源码编译便宜', dtSetRow.us < dtSourceRow.us,
  `${dtSetRow.us.toFixed(3)} vs ${dtSourceRow.us.toFixed(3)}`);
check('dt 直接写全局零分配', dtSetRow.bytes <= 1, `${dtSetRow.bytes.toFixed(0)} B/次`);
check('源码编译路径有分配', dtSourceRow.bytes > 0 && inputRow.bytes > 0,
  `dt ${dtSourceRow.bytes.toFixed(0)} B、输入 ${inputRow.bytes.toFixed(0)} B`);
const emptyRow = rows.find((row) => row.name === '空 chunk 编译');
check('两条源码编译路径都不低于空 chunk 的编译底价',
  dtSourceRow.us >= emptyRow.us && inputRow.us >= emptyRow.us,
  `空 ${emptyRow.us.toFixed(3)}、dt ${dtSourceRow.us.toFixed(3)}、输入 ${inputRow.us.toFixed(3)}`);
check('Lua 侧建表成本低于整条源码协议', buildUs < inputRow.us,
  `建表 ${buildUs.toFixed(3)} µs vs 协议 ${inputRow.us.toFixed(3)} µs`);

// M73-00 第二片的方向断言：把"每帧源码编译清零"这件事钉成机器判据，而不是只留在读数里。
// 两组的**唯一差别**就是编译/JSON 解析，所以不等号必须成立；新路径还必须零分配
// （分配量才是这一片真正的目标——它决定 GC 频率与帧时间分布）。
const hookOldRow = rows.find((row) => row.name === '无参钩子 源码编译(旧)');
const hookNewRow = rows.find((row) => row.name === '无参钩子 直接调用(新)');
const jsonArgsRow = rows.find((row) => row.name === '全局调用 空JSON数组');
const emptyArgsRow = rows.find((row) => row.name === '全局调用 空串无参(新)');
check('无参钩子：直接调用比编译源码便宜', hookNewRow.us < hookOldRow.us,
  `${hookNewRow.us.toFixed(3)} vs ${hookOldRow.us.toFixed(3)} µs`);
check('无参钩子：编译路径有分配、直接调用零分配',
  hookOldRow.bytes > 0 && hookNewRow.bytes <= 1,
  `编译 ${hookOldRow.bytes.toFixed(0)} B、直接 ${hookNewRow.bytes.toFixed(0)} B`);
check('无参全局调用：空串不比空 JSON 数组慢', emptyArgsRow.us <= jsonArgsRow.us * 1.15,
  `空串 ${emptyArgsRow.us.toFixed(3)} vs 空数组 ${jsonArgsRow.us.toFixed(3)} µs`);

if (failures.length > 0) {
  console.error('wasm_lua_boundary_FAILED');
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log(`wasm_lua_boundary_ok ops=${OPS.length} iterations=${ITERATIONS} rounds=${ROUNDS} ` +
  `wasm=${wasmBytes.length} host=node`);
