// wasm-lua-spike 验证：实例化 wasm_lua_spike.wasm 并调用全部 spike 导出。
// 验证 vendored mlua 的 wasm32-unknown-unknown 移植（EH/longjmp、libc shim、%g 精度）。
//
// 由 scripts/verify-wasm-lua.ps1 驱动（构建 + node --experimental-wasm-exnref 调用）。
// spike wasm 路径解析顺序：argv[2] → $RUSTLUA_SPIKE_WASM → <repo>/engine/target/...
// （相对本文件解析 repo 根，不再硬编码绝对路径，因此可在任意机器/CI 上跑）。
//
// 退出码：0 = 全通过；1 = 有用例失败；3 = 输入 wasm 缺失（构建没跑/失败，环境受限）。
import { readFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '..', '..');
const wasmPath =
  process.argv[2] ||
  process.env.RUSTLUA_SPIKE_WASM ||
  join(repoRoot, 'engine', 'target', 'wasm32-unknown-unknown', 'release', 'wasm_lua_spike.wasm');

if (!existsSync(wasmPath)) {
  console.error(`spike wasm not found: ${wasmPath}`);
  console.error('先构建：cargo build -p wasm-lua-spike --target wasm32-unknown-unknown --release');
  process.exitCode = 3;
} else {
  await run(readFileSync(wasmPath));
}

async function run(bytes) {
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const { exports } = instance;
  const mem = exports.memory;

  function withBuf(fn) {
    // 借用 wasm 线性内存尾部做输出缓冲区
    const cap = 256;
    const ptr = mem.buffer.byteLength - cap - 8;
    const buf = new Uint8Array(mem.buffer, ptr, cap);
    const r = fn(ptr, cap);
    let s = '';
    for (let i = 0; i < cap; i++) {
      if (buf[i] === 0) break;
      s += String.fromCharCode(buf[i]);
    }
    return { r, s };
  }

  const results = [];
  function check(name, actual, expect) {
    const ok = actual === expect;
    results.push({ name, ok, actual, expect });
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}  actual=${actual} expect=${expect}`);
  }

  check('spike_add(2,3)', Number(exports.spike_add(2n, 3n)), 5);
  check('spike_arith(2.5,4)', exports.spike_arith(2.5, 4), 11.5);
  check('spike_error()', exports.spike_error(), 1);
  check('spike_math()', Math.abs(exports.spike_math() - (Math.sqrt(2) + Math.sin(1) + Math.floor(2.9))) < 1e-9, true);
  check('spike_type()', exports.spike_type(), 1);
  check('spike_rust_fn()', Number(exports.spike_rust_fn()), 45);

  {
    const { r, s } = withBuf((p, c) => exports.spike_format(1234.5678, p, c));
    const expect = '  1234.568 1.234568e+03 1234.57';
    check('spike_format', s.trimEnd(), expect);
    if (r !== expect.length) console.log(`  (note: returns ${r}, expected ${expect.length})`);
  }
  {
    const { r, s } = withBuf((p, c) => exports.spike_sort(p, c));
    const expect = '1,3,5,7,9';
    check('spike_sort', s, expect);
    if (r !== expect.length) console.log(`  (note: returns ${r}, expected ${expect.length})`);
  }
  {
    const { r, s } = withBuf((p, c) => exports.spike_tostring(3.14, p, c));
    check('spike_tostring', s, '3.14');
  }
  {
    // 回归：%.14g 在 exp ∈ [-4,-1] 的小数（走 format_g 的 0.000…ddd 分支）
    // 必须去尾零，否则 0.1 -> "0.10000000000000"（提交 92880f8 修的 bug）。
    const cases = [[0.1, '0.1'], [0.01, '0.01'], [0.0001, '0.0001'], [1.5, '1.5']];
    for (const [x, expect] of cases) {
      const { s } = withBuf((p, c) => exports.spike_tostring(x, p, c));
      check(`spike_tostring(${x})`, s, expect);
    }
  }

  const failed = results.filter(x => !x.ok);
  console.log(`\n${results.length - failed.length}/${results.length} passed`);
  // 用 exitCode 而非 process.exit：后者在 Windows/Node 上会于 teardown 触发
  // libuv 断言 (UV_HANDLE_CLOSING) 并把退出码盖成 127，令结果对 CI 不可靠。
  process.exitCode = failed.length ? 1 : 0;
}
