// 把 wasm 里 GC 终版的 `ref.null noextern`(D0 72) 降级成扁平的 `ref.null extern`(D0 6F)。
//
// ## 为什么要
// 真机 WXWebAssembly 只认扁平 externref，不认 GC typed null。实测（2026-10-03，
// vconsole_1791039452052.log）：
//   `WebAssembly.instantiate(): Compiling function #950 failed: Unknown heap type -14 @+1486867`
// heap type -14 == `noextern`（s33 编码字节 0x72）。LLVM(+reference-types) 把 externref 表的
// `ref.null extern` 发射成 `ref.null noextern`，旧设备引擎编到第一个就中止；本机 V8 / 微信开发者
// 工具都认，所以门全绿也照样真机白屏。修法：binaryen 133 在 --disable-gc（保留 reference-types）
// 下重编码，把 D0 72 降级为 D0 6F —— noextern <: extern，新旧引擎都类型安全。
//
// ## 两个反例（都实测过，别再走）
//   * `--all-features` 往返 → binaryen 133 把某个 import 重编码成非法 kind `0x7f`，Node V8
//     直接拒（`unknown import kind 0x7f @+1681`）。
//   * 自动检测（不带 enable 列表）→ 产物已被发布链路前面的 `wasm-opt -Oz` 剥掉 target-features
//     section，wasm-opt 退回 MVP，对 memory.copy / i64.trunc_sat 报
//     "all used features should be allowed"。
// 所以显式列出本工具链（Rust wasm32 + `+exception-handling,+reference-types`）实际用到的特性。
// 版本锁 binaryen 133（与 package.ps1 的 Optimize-WasmBin 同源）。若将来工具链新增特性，wasm-opt
// 会 validator error → 这里退出码非 0 → package.ps1 throw（宁可发布期红，也别把 noextern 静默发出去）。
//
// ## 它做什么
// * 跑 `wasm-opt <in> <features> --disable-gc -O0 -o <tmp>`（-O0：只重编码，不优化、不改段结构）。
// * 逐字节校验：大小不变、且每一处改动都恰是 0x72→0x6f。任何别的字节变动都说明 wasm-opt 顺带
//   改了 EH/段结构（见上方反例），此时**不写回**、退出码 1。
// * 校验通过才把 <tmp> 覆盖回 <in>。
//
// ## 用法
//   node scripts/wasm-lua/downgrade-noextern.mjs <wasm-opt 路径> <in.wasm>
// 退出码：0 成功；1 wasm-opt 失败或字节校验不通过；3 参数/文件缺失。
// 成功时打印：downgrade_noextern_ok patched=<处数> size=<字节数>
import { readFileSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

const [wasmOpt, inPath] = process.argv.slice(2);
if (!wasmOpt || !inPath) {
  console.error('usage: downgrade-noextern.mjs <wasm-opt-path> <in.wasm>');
  process.exit(3);
}
if (!existsSync(wasmOpt)) { console.error(`wasm-opt not found: ${wasmOpt}`); process.exit(3); }
if (!existsSync(inPath)) { console.error(`input wasm not found: ${inPath}`); process.exit(3); }

const FEATURES = [
  '--enable-exception-handling', '--enable-reference-types', '--enable-bulk-memory',
  '--enable-nontrapping-float-to-int', '--enable-multivalue', '--enable-sign-ext',
  '--enable-mutable-globals', '--disable-gc',
];
const tmpPath = inPath + '.nogc';
const before = readFileSync(inPath);

const run = spawnSync(wasmOpt, [inPath, ...FEATURES, '-O0', '-o', tmpPath], { encoding: 'utf8' });
if (run.status !== 0 || !existsSync(tmpPath)) {
  const detail = ((run.stderr || '') + (run.stdout || '')).trim().split('\n').slice(0, 6).join(' | ');
  console.error(`wasm-opt --disable-gc failed (exit ${run.status}): ${detail}`);
  try { rmSync(tmpPath, { force: true }); } catch { /* ignore */ }
  process.exit(1);
}

const after = readFileSync(tmpPath);
if (before.length !== after.length) {
  console.error(`size changed ${before.length} -> ${after.length}: wasm-opt 重排了段，非最小改动`);
  rmSync(tmpPath, { force: true });
  process.exit(1);
}
let patched = 0;
for (let i = 0; i < before.length; i++) {
  if (before[i] !== after[i]) {
    if (before[i] !== 0x72 || after[i] !== 0x6f) {
      console.error(`non-0x72->0x6f byte change at offset ${i}: ${before[i].toString(16)}->${after[i].toString(16)}`);
      rmSync(tmpPath, { force: true });
      process.exit(1);
    }
    patched++;
  }
}

writeFileSync(inPath, after);
rmSync(tmpPath, { force: true });
console.log(`downgrade_noextern_ok patched=${patched} size=${after.length}`);
