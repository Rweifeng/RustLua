// 64 位瓦片变体哈希的回归测试（无头、不需要 wasm 产物与浏览器）。
//
// 为什么值得单独钉：这段哈希决定"同一格地表永远长同一副样子"，它必须与三处逐位一致——
// Rust 的 `engine/crates/rustlua-ecs/src/tilemap.rs`、Lua 垫片的镜像、以及**它自己的旧实现**。
// 任何一处写错都不会崩，只会让地表变个样子：M72-12 三宿主目验会把它当"美术问题"查半天。
//
// 本测试的存在理由是一处**实现替换**：原实现用 BigInt（`1n << 64n`），而微信上传器的解析器只到
// ES2019，`1n` 会让**整包上传被拒**（官方报 `Identifier directly after number`，
// 见 scripts/check-upload-ecma.mjs）。替换成"两个 32 位字"的手写算术之后，
// 正确性不能再靠"看着对"——所以这里把**旧 BigInt 实现原样留作参考实现（oracle）**，
// 对新实现做逐位对拍。
//
// 被测实现是从 `plugins/webgl/web-runtime.js` **抽取源码**执行的（不复制一份到测试里）：
// 复制一份就变成了"测我自己写的副本"。抽取锚点不在了会**直接失败**，绝不静默跳过。
//
// 用法：node scripts/web-u64-hash-test.js
'use strict';

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..');
const RUNTIME = path.join(ROOT, 'plugins', 'webgl', 'web-runtime.js');

const failures = [];
function check(name, condition, detail) {
  if (condition) {
    console.log(`  ok  ${name}`);
  } else {
    failures.push(name);
    console.log(`  FAIL ${name}${detail ? ' -> ' + detail : ''}`);
  }
}

function slice(source, startMark, endMark, label) {
  const start = source.indexOf(startMark);
  if (start < 0 || source.indexOf(startMark, start + 1) >= 0) {
    throw new Error(`${label}：起始暗号 ${JSON.stringify(startMark)} 不是唯一的一处，脚本得跟着改`);
  }
  const rest = source.slice(start);
  const end = rest.indexOf(endMark);
  if (end < 0) throw new Error(`${label}：找不到结束暗号 ${JSON.stringify(endMark)}`);
  return rest.slice(0, end);
}

// ---- 参考实现：改这一段的**那版 BigInt 源码**，原样抄在这里（只在测试里跑）----
const U64_MASK = (1n << 64n) - 1n;
const ORACLE_OFFSET = 0xcbf29ce484222325n;
const ORACLE_PRIME = 0x100000001b3n;
const ORACLE_MIX_X = 0x9e3779b97f4a7c15n;
const ORACLE_MIX_Y = 0xbf58476d1ce4e5b9n;
const ORACLE_MUL_A = 0xbf58476d1ce4e5b9n;
const ORACLE_MUL_B = 0x94d049bb133111ebn;

function oracleMix(state, value) {
  let mixed = state;
  for (let index = 0; index < 8; index += 1) {
    const byte = (value >> BigInt(8 * index)) & 0xffn;
    mixed = ((mixed ^ byte) * ORACLE_PRIME) & U64_MASK;
  }
  return mixed;
}

function oracleAvalanche(value) {
  let stage = value ^ (value >> 30n);
  stage = (stage * ORACLE_MUL_A) & U64_MASK;
  stage = stage ^ (stage >> 27n);
  stage = (stage * ORACLE_MUL_B) & U64_MASK;
  return stage ^ (stage >> 31n);
}

function oracleVariantFor(map, x, y) {
  const count = map.variant_count;
  if (count < 2) return 0;
  let hash = ORACLE_OFFSET;
  for (const value of [BigInt(x), BigInt(y), BigInt(map.variant_seed), ORACLE_MIX_X, ORACLE_MIX_Y]) {
    hash = oracleMix(hash, value);
  }
  return Number(oracleAvalanche(hash) % BigInt(count));
}

const ORACLE_CONSTANTS = {
  VARIANT_HASH_OFFSET: ORACLE_OFFSET,
  VARIANT_HASH_PRIME: ORACLE_PRIME,
  MIX_AXIS_X: ORACLE_MIX_X,
  MIX_AXIS_Y: ORACLE_MIX_Y,
  AVALANCHE_MUL_A: ORACLE_MUL_A,
  AVALANCHE_MUL_B: ORACLE_MUL_B,
};

// ---- 被测实现 ----
const runtimeSource = fs.readFileSync(RUNTIME, 'utf8');
const jsSource = slice(
  runtimeSource,
  '// ---- Tilemap 的图集解算（M72-01/02）----',
  '// 解算结果按布局缓存',
  'JS 解算段',
);
const api = new Function(
  `${jsSource}\nreturn { variantFor, autotileMask, resolveAtlasIndex, maxTileId, AUTOTILE_SLOTS, constants: { VARIANT_HASH_OFFSET, VARIANT_HASH_PRIME, MIX_AXIS_X, MIX_AXIS_Y, AVALANCHE_MUL_A, AVALANCHE_MUL_B } };`,
)();

// 新实现的常数是 [hi, lo] 两个无符号 32 位：拼回 64 位再比位。
function pairToBigInt(pair) {
  if (!Array.isArray(pair) || pair.length !== 2) {
    throw new Error(`常数不是 [hi, lo] 两元组：${JSON.stringify(pair)}（ES2019 下不能用 BigInt 字面量）`);
  }
  return ((BigInt(pair[0]) << 32n) | BigInt(pair[1])) & U64_MASK;
}

const subjectConstants = api.constants;
check('导出 6 个哈希常数', Object.keys(subjectConstants).length === 6,
  `实际 ${Object.keys(subjectConstants).join(',')}`);

let constantsChecked = 0;
for (const [name, want] of Object.entries(ORACLE_CONSTANTS)) {
  const got = pairToBigInt(subjectConstants[name]);
  constantsChecked += 1;
  check(`常数 ${name} 逐位一致`, got === want, `新 ${got}，旧 ${want}`);
}

// ---- 对拍：随机 (x, y, seed, count) + 边界值 ----
let cases = 0;
function compare(label, map, x, y) {
  cases += 1;
  let want;
  try {
    want = oracleVariantFor(map, x, y);
  } catch (error) {
    // 参考实现在这个输入上就抛（例如种子不是整数）：新实现也必须拒，不能悄悄算出一个数。
    let threw = false;
    try { api.variantFor(map, x, y); } catch (ignored) { threw = true; }
    check(`${label}：非法输入两边都拒`, threw, `参考实现抛 ${error.message}，新实现没抛`);
    return;
  }
  const got = api.variantFor(map, x, y);
  if (got !== want) {
    check(`${label} x=${x} y=${y} seed=${map.variant_seed} count=${map.variant_count}`, false,
      `新 ${got}，旧 ${want}`);
  }
}

const COUNTS = [2, 3, 4, 5, 7, 8, 16, 17, 31, 64, 255, 1024];
const SEEDS = [
  0, 1, 2, 7, 42, 1337, 65535, 65536, 2147483647, -1, -2, -1337, -2147483648,
  4294967295, 4294967296, 9007199254740991, -9007199254740991, 1099511628211, -3750763034362895579,
];
const COORDS = [0, 1, 2, 7, 8, 15, 16, 31, 63, 100, 999, 65535, 65536, 2147483647];

for (const count of COUNTS) {
  for (const seed of SEEDS) {
    for (const coord of COORDS) {
      compare('坐标与种子边界', { variant_count: count, variant_seed: seed }, coord, coord ^ 7);
    }
  }
}

// 确定性伪随机（不用 Math.random：失败要能复现出同一个输入）。
let lcg = 0x2545f491;
function nextRandom() {
  lcg = (Math.imul(lcg, 1103515245) + 12345) >>> 0;
  return lcg;
}
const RANDOM_CASES = 4000;
for (let index = 0; index < RANDOM_CASES; index += 1) {
  const count = COUNTS[nextRandom() % COUNTS.length];
  const seed = (nextRandom() % 0x100000000) - 0x80000000;
  const x = nextRandom() % 0x10000;
  const y = nextRandom() % 0x10000;
  compare('随机', { variant_count: count, variant_seed: seed }, x, y);
}

// ---- 语义面：不能只是"和旧的一样"，还要满足它存在的理由 ----
const grid = { variant_count: 4, variant_seed: 12345 };
const histogram = [0, 0, 0, 0];
const period = [];
for (let y = 0; y < 64; y += 1) {
  for (let x = 0; x < 64; x += 1) {
    histogram[api.variantFor(grid, x, y)] += 1;
    if (y === 0) period.push(api.variantFor(grid, x, y));
  }
}
check('count=4 时四个变体都出现', histogram.every((n) => n > 0), JSON.stringify(histogram));
check('四个变体分布不悬殊（各占 15%~35%）',
  histogram.every((n) => n > 4096 * 0.15 && n < 4096 * 0.35), JSON.stringify(histogram));
// 旧实现的注释点名过这个退化：原始 FNV 低比特会给出 3,2,1,0 的周期点阵。这里钉住"不是周期"。
const isPeriodic = period.slice(0, 32).every((value, index) => value === period[index % 4]);
check('第一行不是 4 周期点阵', !isPeriodic, period.slice(0, 8).join(','));
check('换种子会改变结果',
  api.variantFor({ variant_count: 4, variant_seed: 12346 }, 0, 0) !== api.variantFor(grid, 0, 0));
check('count<2 恒为 0',
  api.variantFor({ variant_count: 1, variant_seed: 7 }, 3, 9) === 0
  && api.variantFor({ variant_count: 0, variant_seed: 7 }, 3, 9) === 0);

// ---- 顺带记一笔成本：这条替换的动因之一是 BigInt 的分配，读数只打印不断言 ----
function timeIt(label, fn, iterations) {
  const start = process.hrtime.bigint();
  let accumulator = 0;
  for (let index = 0; index < iterations; index += 1) {
    accumulator += fn(index);
  }
  const elapsed = Number(process.hrtime.bigint() - start) / 1e6;
  console.log(`  计时 ${label}: ${iterations} 次 ${elapsed.toFixed(1)} ms（校验和 ${accumulator}）`);
  return elapsed;
}
const TIMED = 20000;
const subjectMs = timeIt('新实现', (index) => api.variantFor(grid, index & 0xff, index >> 8), TIMED);
const oracleMs = timeIt('旧 BigInt', (index) => oracleVariantFor(grid, index & 0xff, index >> 8), TIMED);
console.log(`  提速 ${(oracleMs / Math.max(subjectMs, 1e-6)).toFixed(1)}×`);
check('新实现不比旧实现慢', subjectMs <= oracleMs * 1.5,
  `新 ${subjectMs.toFixed(1)} ms，旧 ${oracleMs.toFixed(1)} ms`);

if (failures.length > 0) {
  console.error('web_u64_hash_test_FAILED');
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log(`web_u64_hash_test_ok cases=${cases + constantsChecked + 5} constants=${constantsChecked}/6`);
