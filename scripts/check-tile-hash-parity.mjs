#!/usr/bin/env node
// M72-02 的跨宿主哈希契约：web 宿主里那**两份**抄本（JS 解算 + 内嵌 Lua 垫片）必须与引擎
// `rustlua-ecs/src/tilemap.rs` 给出同一个答案。
//
// 为什么要单独一个脚本：变体序号与 autotile 掩码是「同一串位运算的四份实现」，抄错一个字符
// 不会崩，只会让 web 与原生各画各的地表 —— 而 M72-12 的三宿主目验会把它当成美术问题查半天。
// Rust 侧由 `cargo test -p rustlua-ecs variant` 与门 `jhyt_m72_tile_variant_determinism` 钉住
// （同一组金标数字），这个脚本钉住 JS 侧，并把 Lua 垫片的常数与 JS 常数逐条对上（两侧同在一个
// 文件里，静态比对即可判定"是否同一串位"）。
//
// 用法：node scripts/check-tile-hash-parity.mjs [--print]
// 退出码：0 = 全等；1 = 有错位（打印首个不同格与两侧读数）。
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const RUNTIME = path.join(ROOT, 'plugins', 'webgl', 'web-runtime.js');

// 与 `jhyt_m72_tile_variant_determinism.rs` / `rustlua-ecs/src/tilemap.rs` 的同名常数**故意各写
// 一遍**：三份一起抄错成同一种错位，这条检查才真的白装。
const VARIANT_GOLDEN = [
  0, 2, 2, 1, 3, 1, 0, 3,
  3, 2, 2, 0, 2, 1, 3, 2,
  1, 1, 1, 0, 3, 1, 1, 3,
  3, 1, 2, 0, 3, 2, 1, 1,
];
const PROBE = { width: 8, height: 4, atlas_cols: 12, atlas_rows: 8, tile_kinds: 4, variant_count: 4, variant_seed: 0x12345678, autotile: false };

// autotile 探针（与 `jhyt_m72_autotile_mask.rs` 同一张图）：8×8 草（kind 1）嵌 4×4 土（kind 2），
// kinds=2 / variant_count=1 / 图集 12×8；掩码位序 bit0=y-1、bit1=x+1、bit2=y+1、bit3=x-1。
const AUTOTILE = { width: 8, height: 8, atlas_cols: 12, atlas_rows: 8, tile_kinds: 2, variant_count: 1, variant_seed: 0x5eed0002, autotile: true };
const autotileCells = [];
for (let y = 0; y < AUTOTILE.height; y += 1) {
  for (let x = 0; x < AUTOTILE.width; x += 1) {
    autotileCells.push(x >= 2 && x <= 5 && y >= 2 && y <= 5 ? 2 : 1);
  }
}
// 土块 16 格（行主序 y=2..5、x=2..5）按手写位序算出的条目：kind 2 的过渡块是 19..=34。
const DIRT_ENTRIES = [25, 33, 33, 31, 26, 34, 34, 32, 26, 34, 34, 32, 22, 30, 30, 28];
// 地图四角（草）与四块贴着土块的草格：这四组把"缺指向土块那一位"钉死。
const PINNED_CELLS = [
  [[0, 0], 9], [[7, 0], 15], [[0, 7], 6], [[7, 7], 12],
  [[1, 2], 16], [[2, 1], 14], [[6, 3], 10], [[3, 6], 17],
];

function slice(source, startMark, endMark, label) {
  const start = source.indexOf(startMark);
  if (start < 0 || source.indexOf(startMark, start + 1) >= 0) {
    throw new Error(`${label}：起始暗号 ${JSON.stringify(startMark)} 在 web-runtime.js 里不是唯一的一处，脚本得跟着改`);
  }
  const rest = source.slice(start);
  const end = rest.indexOf(endMark);
  if (end < 0) throw new Error(`${label}：找不到结束暗号 ${JSON.stringify(endMark)}`);
  return rest.slice(0, end);
}

const jsSource = slice(
  readFileSync(RUNTIME, 'utf8'),
  '// ---- Tilemap 的图集解算（M72-01/02）----',
  '// 解算结果按布局缓存',
  'JS 解算段',
);
const luaSource = slice(
  readFileSync(RUNTIME, 'utf8'),
  '__rustlua_cell_index = function(m, x, y)',
  '\ntilemap = {',
  'Lua 垫片镜像段',
);
// new Function 而不是 import：这段代码在宿主闭包内部，本身是自洽的 const + function 声明。
const api = new Function(`${jsSource}\nreturn { variantFor, autotileMask, resolveAtlasIndex, maxTileId, AUTOTILE_SLOTS, constants: { VARIANT_HASH_OFFSET, VARIANT_HASH_PRIME, MIX_AXIS_X, MIX_AXIS_Y, AVALANCHE_MUL_A, AVALANCHE_MUL_B } };`)();

function asSigned(value) {
  const limit = 1n << 63n;
  const modulus = 1n << 64n;
  return (value < limit ? value : value - modulus).toString();
}

// 运行时的常数是 [hi, lo] 两个无符号 32 位（ES2019 下不能用 BigInt 字面量：微信上传器拒
// `1n`，见 scripts/check-upload-ecma.mjs）。门要比的是**同一串位**，所以先拼回 64 位；
// 拼法本身就是一次校验——不是两元组就直接失败，免得"表示变了、门还在拿旧形状比"。
function constantToBigInt(name, pair) {
  if (!Array.isArray(pair) || pair.length !== 2
      || !Number.isInteger(pair[0]) || !Number.isInteger(pair[1])
      || pair[0] < 0 || pair[0] > 0xffffffff || pair[1] < 0 || pair[1] > 0xffffffff) {
    throw new Error(`常数 ${name} 不是 [hi, lo] 两个无符号 32 位：${JSON.stringify(pair)}`);
  }
  return ((BigInt(pair[0]) << 32n) | BigInt(pair[1]));
}

const failures = [];
let cellsChecked = 0;
function check(label, got, want) {
  cellsChecked += 1;
  if (String(got) !== String(want)) failures.push(`${label}：JS 侧 ${got}，期望 ${want}`);
}

// ① 常数：JS 的 [hi, lo] 与 Lua 的带符号十进制字面量必须是同一串位。
for (const [name, value] of Object.entries(api.constants)) {
  const literal = `local ${name} = ${asSigned(constantToBigInt(name, value))}`;
  if (!luaSource.includes(literal)) {
    failures.push(`Lua 垫片里 ${name} 不是 ${literal}（JS 侧那份已是同一串位，改了一边就是两份哈希）`);
  }
}
check('AUTOTILE_SLOTS', api.AUTOTILE_SLOTS, 16);
if (!luaSource.includes('local AUTOTILE_SLOTS = 16')) failures.push('Lua 垫片里 AUTOTILE_SLOTS 不是 16');

// ② 变体金标：JS 解出的 32 格必须逐格等于 Rust 金标。
PROBE.cells = new Array(PROBE.width * PROBE.height).fill(1);
for (let index = 0; index < VARIANT_GOLDEN.length; index += 1) {
  const x = index % PROBE.width;
  const y = Math.floor(index / PROBE.width);
  const entry = api.resolveAtlasIndex(PROBE, x, y);
  cellsChecked += 1;
  if (entry - 1 !== VARIANT_GOLDEN[index]) {
    failures.push(`变体 (x=${x},y=${y})：JS 条目 ${entry}（序号 ${entry - 1}），金标序号 ${VARIANT_GOLDEN[index]}`);
    break;
  }
}
if (api.variantFor({ ...PROBE, variant_seed: PROBE.variant_seed + 1 }, 0, 0) === api.variantFor(PROBE, 0, 0)) {
  failures.push('JS 侧换种子后 (0,0) 的变体没变：种子没进哈希，§② 的同值可能是巧合');
}

// ③ autotile：16 格土的条目 + 8 格草的条目（掩码位序与 Rust 一致）。
AUTOTILE.cells = autotileCells;
for (let row = 0; row < 4; row += 1) {
  for (let col = 0; col < 4; col += 1) {
    const x = 2 + col;
    const y = 2 + row;
    check(`土块 (${x},${y}) 条目`, api.resolveAtlasIndex(AUTOTILE, x, y), DIRT_ENTRIES[row * 4 + col]);
  }
}
for (const [[x, y], want] of PINNED_CELLS) {
  check(`草格 (${x},${y}) 条目`, api.resolveAtlasIndex(AUTOTILE, x, y), want);
}
if (api.autotileMask(AUTOTILE, 2, 2) !== 0b0110) {
  failures.push(`土块左下角 (2,2) 掩码 = ${api.autotileMask(AUTOTILE, 2, 2).toString(2)}，应为 110`);
}

// ④ 容量守卫：图集装不下的 kind 必须解出 0（什么都不画），两侧都得拒。
const tooSmall = { ...PROBE, atlas_cols: 6, atlas_rows: 1, tile_kinds: 4, variant_count: 4 };
tooSmall.cells = new Array(tooSmall.width * tooSmall.height).fill(4);
check('越界 kind 的条目', api.resolveAtlasIndex(tooSmall, 0, 0), 0);
if (api.maxTileId({ ...PROBE, variant_count: 1, autotile: false }) !== 96) {
  failures.push('恒等档的 max_tile_id 不该跟着 kinds 走（应等于图集容量）');
}

if (failures.length > 0) {
  console.error('tile_hash_parity_FAILED');
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}

if (process.argv.includes('--print')) {
  console.log(`  变体第一行 = ${VARIANT_GOLDEN.slice(0, 8).join(',')}`);
  console.log(`  土块条目 = ${DIRT_ENTRIES.join(',')}`);
}
console.log(
  `tile_hash_parity_ok js_cells=${VARIANT_GOLDEN.length + 24} lua_constants=6/6 slots=16 ` +
  'runtime=plugins/webgl/web-runtime.js',
);
