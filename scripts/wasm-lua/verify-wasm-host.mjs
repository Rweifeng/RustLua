// wasm 产物宿主接受性校验（发布期守卫，由 scripts/package.ps1 调用）。
//
// ## 为什么需要
// wasm 里混入新旧两代异常处理（EH）指令时，宿主会**整模块拒绝编译**，运行时的症状却只有一句
// `wasm lua bridge not loaded` + `boot=failed`、0 帧 —— 看不出是"桥没接线"还是"wasm 装不进来"。
// 实测 2026-09-30：微信侧用默认 toolchain（LLVM 19）构建的 mlua wasm 就是这样发出去的。
// 这条守卫把判别点从"真机/提审之后"提前到"发布之前"。
//
// ## 它证明什么、不证明什么
// 证明：这份模块能被**标准 V8** 解析、且 Lua 桥导出齐全。
// 不证明：微信真机的 WXWebAssembly 行为（本机无法模拟宿主实现，见 docs/52 R1 同一口径）。
//
// 用法：node scripts/wasm-lua/verify-wasm-host.mjs <path/to/game_web_bg.wasm>
// 退出码：0 通过；1 宿主拒绝解析或导出缺失；3 参数/文件缺失。
import { readFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';

const wasmPath = process.argv[2];
if (!wasmPath) {
  console.error('usage: verify-wasm-host.mjs <game_web_bg.wasm>');
  process.exit(3);
}
const abs = resolve(wasmPath);
if (!existsSync(abs)) {
  console.error(`wasm file not found: ${abs}`);
  process.exit(3);
}

// 宿主选择与 plugins/wechat-minigame/wasm-bridge.js 的 resolveWebAssembly 同口径：
// WXWebAssembly 优先，退到 WebAssembly。
const host = globalThis.WXWebAssembly ?? globalThis.WebAssembly;
const hostName = globalThis.WXWebAssembly ? 'WXWebAssembly' : 'WebAssembly';
if (typeof host?.Module !== 'function') {
  console.error('no WebAssembly.Module in this host');
  process.exit(3);
}

const bytes = readFileSync(abs);
let mod;
try {
  mod = new host.Module(bytes);
} catch (error) {
  console.error(`wasm_host_reject host=${hostName} bytes=${bytes.length}: ${String(error.message).split('\n')[0]}`);
  process.exit(1);
}

// Lua 桥 + 入口导出：缺任何一个都意味着发布链路拿不到执行层（漏了 #[wasm_bindgen] 或
// wasm-bindgen 版本不匹配），而运行时只会表现为"桥没挂上"。
const required = [
  'lua_boot',
  'lua_boot_with_reader',
  'lua_do_string',
  'lua_call_global',
  'lua_register_js_callback',
  'lua_missing_globals',
  'lua_execute_main',
];
const exported = new Set(host.Module.exports(mod).map((e) => e.name));
const missing = required.filter((name) => !exported.has(name));
if (missing.length > 0) {
  console.error(`wasm_host_missing_exports: ${missing.join(',')}`);
  process.exit(1);
}
console.log(
  `wasm_host_ok host=${hostName} bytes=${bytes.length} exports=${exported.size} bridge_exports=${required.length}`
);
