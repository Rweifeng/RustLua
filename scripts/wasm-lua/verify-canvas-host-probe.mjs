// `probe_canvas_host` 的离线契约测试（WP8 微信半边第一片）。
//
// 为什么值得单独一个用例：微信小游戏里没有浏览器那套宿主（`HTMLCanvasElement` 不是那个
// 构造器），而 wasm-bindgen 的 `dyn_into::<HtmlCanvasElement>()` **会**做构造器判定 ⇒
// 任何"先按 id 找画布再 dyn_into"的入口在小游戏里必然失败。本用例要证明的是：
// 新入口走的是**属性访问**那条路（`unchecked_into`），所以一个**普通对象**也能进到
// 引擎的 GL 层；失败时给的是**具名**原因（`probe: …`），不是 `undefined`、不是挂起。
//
// 这里**不**要求 GL 可用：Node 里没有 WebGL2，所以"进到 GL 层再具名失败"正是期望结果——
// 它同时证明了"入口可调用"与"路径不依赖浏览器构造器"。真机的成功读数由适配层的
// `glCapability` + 这个探针一起回答。
import { readFileSync, copyFileSync, appendFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const dir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(dir, '..', '..');
// `resolve`：argv 里可能给相对路径，而 `createRequire(...)` 是按**本文件所在目录**解析
// 相对说明符的（不是 cwd）⇒ 不转绝对路径就会 "Cannot find module …"（本用例第一次拿
// 打包产物做输入时就是这么失败的）。默认值本来就来自 import.meta，天然是绝对的。
const glueDir = resolve(process.argv[2] || process.env.RUSTLUA_GLUE_DIR ||
  join(repoRoot, 'artifacts', 'wasm-lua-verify', 'no-modules-test'));

let passed = 0, failed = 0;
const assert = (cond, label) => {
  if (cond) { passed += 1; console.log('  ok:', label); }
  else { failed += 1; console.error('  FAIL:', label); }
};

let glue;
try {
  // 与 package.ps1 的微信补丁同款：no-modules 胶水的 `let wasm_bindgen` 是模块作用域绑定，
  // 直接 require 拿不到 ⇒ 追加一行 `module.exports = wasm_bindgen;`。
  const patched = join(glueDir, 'game_web.probe.patched.js');
  copyFileSync(join(glueDir, 'game_web.js'), patched);
  appendFileSync(patched, '\nmodule.exports = wasm_bindgen;\n');
  const require = createRequire(import.meta.url);
  glue = require(patched);
  glue.initSync(readFileSync(join(glueDir, 'game_web_bg.wasm')));
  assert(true, `glue loaded from ${glueDir}`);
} catch (error) {
  console.error('  胶水/wasm 加载失败：' + error);
  process.exit(1);
}

// ① 导出存在（glue 包装层与原始 wasm 导出两侧都查——历史上"新增导出在 no-modules 下
//    静默 undefined"的疑虑就是靠这两侧对照证伪的）。
assert(typeof glue.probe_canvas_host === 'function', 'glue.probe_canvas_host is callable');
const rawExports = Object.keys(glue).filter(n => n.indexOf('__') !== 0);
assert(rawExports.indexOf('probe_canvas_host') >= 0, 'probe_canvas_host present on glue names');

// ② Node 里确实没有浏览器构造器：这正是"无构造器依赖"的判据前提。
assert(typeof globalThis.HTMLCanvasElement === 'undefined', 'no HTMLCanvasElement global (browser-constructor-free environment)');

const settle = (promise, ms) => Promise.race([
  promise.then(value => ({ kind: 'ok', value }), error => ({ kind: 'err', error })),
  new Promise(resolve => setTimeout(() => resolve({ kind: 'timeout' }), ms)),
]);

const reports = [];
const report = (text) => { reports.push(String(text)); };

function fakeCanvas(width, height, gl) {
  return {
    width, height,
    getContext(kind) { return gl && kind === 'webgl2' ? gl : null; },
    addEventListener() {}, removeEventListener() {},
  };
}

// ③ 零尺寸画布：具名拒绝，且**不**进 GL 层（读数应当是"画布没建好"而不是"GPU 不可用"）。
{
  reports.length = 0;
  const outcome = await settle(glue.probe_canvas_host(fakeCanvas(0, 0, null), report), 8000);
  assert(outcome.kind === 'err', 'zero-size canvas rejects (does not hang)');
  const text = String((outcome.error && outcome.error.message) || outcome.error || '');
  assert(text.indexOf('probe:') === 0, `zero-size failure is named (got ${JSON.stringify(text)})`);
  assert(text.indexOf('尺寸为 0') >= 0, `zero-size reason names the size (got ${JSON.stringify(text)})`);
  assert(reports.length === 1 && reports[0].indexOf('probe:') === 0, 'failure also reported through the report callback');
}

// ④ null 画布：具名拒绝（契约里的"调用方没给画布"）。
{
  reports.length = 0;
  const outcome = await settle(glue.probe_canvas_host(null, report), 8000);
  assert(outcome.kind === 'err', 'null canvas rejects (does not hang)');
  const text = String((outcome.error && outcome.error.message) || outcome.error || '');
  assert(text.indexOf('probe: no canvas') === 0, `null canvas failure is named (got ${JSON.stringify(text)})`);
}

// ⑤ 关键一条：**普通对象**（不是 HTMLCanvasElement）能进到引擎的 GL 层。
//    判据是失败原因**不是**类型转换错误——若代码里用了 `dyn_into`，这里会得到
//    "not of type HtmlCanvasElement"（那在小游戏上就是我们一开始要消灭的现象）。
{
  reports.length = 0;
  const outcome = await settle(glue.probe_canvas_host(fakeCanvas(64, 64, null), report), 15000);
  assert(outcome.kind !== 'timeout', 'plain-object canvas settles (no infinite wait)');
  console.log('  raw outcome:', JSON.stringify({
    kind: outcome.kind,
    value: outcome.value,
    errorType: typeof outcome.error,
    error: (() => { try { return String(outcome.error); } catch { return '<unstringifiable>'; } })(),
  }).slice(0, 400));
  const text = outcome.kind === 'ok'
    ? String(outcome.value === undefined ? '' : outcome.value)
    : String(outcome.error === undefined ? '' : outcome.error);
  assert(text.indexOf('probe:') === 0 || outcome.kind === 'ok', `plain-object path reaches the GL layer (got ${JSON.stringify(text)})`);
  // 判别式要精确：`dyn_into` 失败时的原话是 "…not of type `HtmlCanvasElement`…"，
  // 而 GL 层报错里出现 `HtmlCanvasElement` 是合理的（wgpu 的 webgl 后端会提到它）
  // ⇒ 只认"类型转换"这两种措辞，别把良性报错误判成构造器依赖。
  assert(!/not of type|instanceof/i.test(text),
    `no browser-constructor check in the path (got ${JSON.stringify(text)})`);
  assert(reports.length >= 1, 'plain-object path reports through the callback');
  console.log('  note: plain-object GL outcome =', JSON.stringify(text.slice(0, 200)));
}

// ⑥ 引擎宿主骨架（WP8 第二片）：`engine_host_begin/frame/state/end` 的**离线契约**。
//    离线拿不到 GL，所以这里能证的恰好是本片真正新增的东西：
//      · API 存在且能在 no-modules 胶水的两侧看到（glue 包装 + 原始导出名）；
//      · 状态机正确：没 begin 就 frame/state ⇒ 具名 `host: not-begun`（而不是 panic/静默 0）；
//      · 入参校验**先于** GPU 工作：坏 options/坏 dt 在碰画布之前就具名失败；
//      · 失败不留半个宿主：begin 失败之后 state 仍然报 not-begun（半安装状态是最难查的一类）。
//    真机上 `begin` 成功的读数（adapter/backend/尺寸）与每帧 steps/presented 由适配层侧回报。
const hostExports = [
  'engine_host_begin', 'engine_host_frame', 'engine_host_frame_into', 'engine_host_frame_layout_json',
  'engine_host_state', 'engine_host_end',
];
for (const name of hostExports) {
  assert(typeof glue[name] === 'function', `glue.${name} is callable`);
  assert(rawExports.indexOf(name) >= 0, `${name} present on glue names`);
}

const hostReports = [];
const hostReport = (text) => { hostReports.push(String(text)); };
const hostError = (fn) => {
  try { return { kind: 'ok', value: String(fn()) }; }
  catch (error) { return { kind: 'err', error }; }
};
const hostText = (outcome) => String(
  (outcome.kind === 'ok' ? outcome.value : (outcome.error && outcome.error.message) || outcome.error) || '');

// ⑥-0 定长帧 ABI（WP7-55）：布局是 JS 侧偏移的**唯一来源**，所以它本身必须可断言——
//      长度与槽位表一致、版本是正数、我依赖的字段真的在里面（名字打错就是"字段恒 0"）。
let hostAbi = null;
{
  hostAbi = JSON.parse(glue.engine_host_frame_layout_json());
  assert(hostAbi.abi === 'rustlua-engine-host-frame', `frame ABI names itself (got ${JSON.stringify(hostAbi.abi)})`);
  assert(Number.isInteger(hostAbi.version) && hostAbi.version >= 1, `frame ABI carries a version (got ${hostAbi.version})`);
  assert(Array.isArray(hostAbi.slots) && hostAbi.slots.length === hostAbi.len && hostAbi.len > 0,
    `frame ABI len matches its slot table (len=${hostAbi.len} slots=${hostAbi.slots && hostAbi.slots.length})`);
  const needed = ['frame_index', 'steps', 'delta_ms', 'accumulated_ms', 'dropped_total', 'clamped',
    'presented_total', 'failed_presents', 'input_applied', 'tick_steps_total', 'tick_failures'];
  const missing = needed.filter((name) => hostAbi.slots.indexOf(name) < 0);
  assert(missing.length === 0, `frame ABI carries every slot the adapter reads (missing ${JSON.stringify(missing)})`);
  assert(hostAbi.slots.indexOf('frame_index') === 0, `frame_index is slot 0 (got ${hostAbi.slots[0]})`);
  console.log('  frame abi:', JSON.stringify({ abi: hostAbi.abi, version: hostAbi.version, len: hostAbi.len }));
}

// ⑥a 没 begin 就 frame / state：具名、且给出下一步。
{
  const frame = hostError(() => glue.engine_host_frame(16.7, undefined));
  assert(frame.kind === 'err' && hostText(frame).indexOf('host: not-begun') === 0,
    `frame before begin is named (got ${JSON.stringify(hostText(frame))})`);
  const state = hostError(() => glue.engine_host_state());
  assert(state.kind === 'err' && hostText(state).indexOf('host: not-begun') === 0,
    `state before begin is named (got ${JSON.stringify(hostText(state))})`);
  // end 幂等：没跑起来也不许抛。
  assert(hostError(() => glue.engine_host_end()).kind === 'ok', 'end without a host does not throw');
}

// ⑥b 坏 dt 与坏输入类型：具名拒绝（NaN 绝不能渗进定步长时钟）。
//    输入批量只认 `Uint8Array`（RLFI）或 undefined/null——字符串/数字/普通对象必须被点名，
//    否则调用方会以为"我传了输入"而实际什么都没进去（静默丢输入是最难查的一类）。
{
  const bad = hostError(() => glue.engine_host_frame(Number.NaN, undefined));
  assert(bad.kind === 'err' && hostText(bad).indexOf('host: dt-ms') === 0,
    `NaN dt is named (got ${JSON.stringify(hostText(bad))})`);
  for (const [label, input] of [['string', 'not-a-batch'], ['number', 7], ['plain object', { bytes: 1 }]]) {
    const wrong = hostError(() => glue.engine_host_frame(16.7, input));
    assert(wrong.kind === 'err' && hostText(wrong).indexOf('host: input') === 0,
      `${label} input is rejected by name (got ${JSON.stringify(hostText(wrong))})`);
  }
}

// ⑥c 入参校验先于 GPU 工作：坏 JSON / 坏数值必须**不碰画布**就失败。
{
  for (const [label, options, expected] of [
    ['broken json', '{broken', 'host: options-json'],
    ['zero updates', '{"updates_per_second":0}', 'host: options'],
    ['negative cap', '{"max_frame_seconds":-1}', 'host: options'],
    ['bad clear', '{"clear":[1,2,3]}', 'host: options'],
  ]) {
    hostReports.length = 0;
    const outcome = await settle(glue.engine_host_begin(fakeCanvas(64, 64, null), options, hostReport), 8000);
    const text = hostText(outcome);
    assert(outcome.kind === 'err' && text.indexOf(expected) === 0,
      `${label} rejected before touching the canvas (got ${JSON.stringify(text)})`);
  }
}

// ⑥d null 画布与零尺寸画布：与探针同款的具名契约。
{
  hostReports.length = 0;
  const nullCanvas = await settle(glue.engine_host_begin(null, '{}', hostReport), 8000);
  assert(nullCanvas.kind === 'err' && hostText(nullCanvas).indexOf('host: canvas') === 0,
    `null canvas rejected (got ${JSON.stringify(hostText(nullCanvas))})`);
  hostReports.length = 0;
  const zero = await settle(glue.engine_host_begin(fakeCanvas(0, 0, null), '{}', hostReport), 8000);
  assert(zero.kind === 'err' && hostText(zero).indexOf('host: canvas') === 0 && hostText(zero).indexOf('尺寸为 0') >= 0,
    `zero-size canvas rejected by name (got ${JSON.stringify(hostText(zero))})`);
  assert(hostReports.length === 1 && hostReports[0].indexOf('host: ') === 0,
    'begin failure is also reported through the report callback');
}

// ⑥e 普通对象画布 + 合法 options：离线必须走到 GL 层再具名失败，且**不留半个宿主**。
{
  hostReports.length = 0;
  const outcome = await settle(glue.engine_host_begin(fakeCanvas(64, 64, null), '{"updates_per_second":60}', hostReport), 15000);
  assert(outcome.kind !== 'timeout', 'plain-object begin settles (no infinite wait)');
  const text = hostText(outcome);
  console.log('  begin raw outcome:', JSON.stringify({ kind: outcome.kind, text: text.slice(0, 240) }));
  assert(outcome.kind === 'err' && text.indexOf('host: ') === 0,
    `begin failure is named (got ${JSON.stringify(text)})`);
  assert(!/not of type|instanceof/i.test(text),
    `begin does not depend on browser constructors (got ${JSON.stringify(text)})`);
  const state = hostError(() => glue.engine_host_state());
  assert(state.kind === 'err' && hostText(state).indexOf('host: not-begun') === 0,
    `no half-installed host after a failed begin (got ${JSON.stringify(hostText(state))})`);
}

// ⑥f 定长帧入口（`engine_host_frame_into`）的入参契约：
//    · 槽位太短 ⇒ `host: frame-out`（**先于**状态检查：入参不合法就该在碰设备之前说清楚）；
//    · dt/输入类型与 JSON 入口同一套具名规矩；
//    · 没 begin ⇒ `host: not-begun`（通道正确时也不许悄悄返回 0 个槽位）。
{
  const full = () => new Float64Array(hostAbi.len);
  const short = hostError(() => glue.engine_host_frame_into(16.7, undefined, new Float64Array(hostAbi.len - 1)));
  assert(short.kind === 'err' && hostText(short).indexOf('host: frame-out') === 0,
    `too-short out buffer is named before anything else (got ${JSON.stringify(hostText(short))})`);
  const nan = hostError(() => glue.engine_host_frame_into(Number.NaN, undefined, full()));
  assert(nan.kind === 'err' && hostText(nan).indexOf('host: dt-ms') === 0,
    `NaN dt on the fixed ABI entry is named (got ${JSON.stringify(hostText(nan))})`);
  const wrongInput = hostError(() => glue.engine_host_frame_into(16.7, 'not-a-batch', full()));
  assert(wrongInput.kind === 'err' && hostText(wrongInput).indexOf('host: input') === 0,
    `wrong input type on the fixed ABI entry is named (got ${JSON.stringify(hostText(wrongInput))})`);
  const notBegun = hostError(() => glue.engine_host_frame_into(16.7, undefined, full()));
  assert(notBegun.kind === 'err' && hostText(notBegun).indexOf('host: not-begun') === 0,
    `fixed ABI entry before begin is named (got ${JSON.stringify(hostText(notBegun))})`);
  // 定长入口与 JSON 入口必须**同一条**失败路径（两个门面报不同读数是最难查的"仪器说谎"）。
  const jsonPath = hostError(() => glue.engine_host_frame(Number.NaN, undefined));
  assert(hostText(jsonPath).indexOf('host: dt-ms') === 0,
    `JSON frame entry rejects the same NaN dt (got ${JSON.stringify(hostText(jsonPath))})`);
}

console.log(`\nRESULT: ${passed} passed, ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);
