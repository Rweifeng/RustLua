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
// 证明（--platform=wechat-minigame 时）：wasm 的 Export section **不含 Global 条目**——iOS 上的
// WXWebAssembly 不支持 Global 导出（docs/52 R12 尾段），带 Global 的产物会整模块被拒；本仓的
// wasm-bindgen 目前会导出 `__abort_handler`/`__instance_terminated` 两条内部 global，需要
// `scripts/wasm-lua/strip-wasm-globals.mjs` 从 Export 段削掉后再发布。
// 不证明：微信真机的 WXWebAssembly 行为（本机无法模拟宿主实现，见 docs/52 R1 同一口径）；
// iOS 具体拒绝哪条也仍以真机为准，这条守卫只是把"明知 iOS 会崩"的产物挡在发布前。
//
// 用法：node scripts/wasm-lua/verify-wasm-host.mjs <path/to/game_web_bg.wasm> [--platform=<name>]
//   --platform=webgl            (默认) Global 仅**报告**，不失败（浏览器都支持 Global）。
//   --platform=wechat-minigame  Global 视为**致命**（iOS WXWebAssembly 硬约束）。
// 退出码：0 通过；1 宿主拒绝解析、导出缺失、或微信侧检测到 Global 导出；3 参数/文件缺失。
import { readFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';

const positional = [];
let platform = 'webgl';
for (const arg of process.argv.slice(2)) {
  if (arg.startsWith('--platform=')) platform = arg.slice('--platform='.length);
  else positional.push(arg);
}
if (!['webgl', 'wechat-minigame'].includes(platform)) {
  console.error(`unsupported --platform=${platform} (expected webgl | wechat-minigame)`);
  process.exit(3);
}

const wasmPath = positional[0];
if (!wasmPath) {
  console.error('usage: verify-wasm-host.mjs <game_web_bg.wasm> [--platform=<webgl|wechat-minigame>]');
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
  // M73-00：逐帧写 `__rustlua_dt` 走它（不再编译 chunk）。运行时**启动第 0 帧**就会调，
  // 所以它是硬依赖：缺了就是"启动即失败"，而且报错原文只有 is not a function。
  // 放进这里，漏加 `#[wasm_bindgen]`（或门面白名单没同步）在打包前就会被拦住。
  'lua_set_number',
  // M73-00：每帧输入批量线格式（`RLFI`）。同样是硬依赖——运行时**第一个固定步**就会调
  // `input_frame_layout_json` 建打包器、随后每步调 `lua_input_frame`；缺任一个都会让
  // "启动后一有输入就失败"。两者一起放，是因为漏一个与漏两个的症状完全一样。
  'lua_input_frame',
  'input_frame_layout_json',
  'lua_call_global',
  // M73-00：**无参无返回**的全局钩子（每步的虚拟键注入 / 一次性边沿清理）走它，取代
  // `doString("__rustlua_apply_virtual_input()")`。运行时**每个固定步**都调，缺了就是
  // "启动后一按键就失败"，所以同样按硬依赖放进清单。
  'lua_call_global_void',
  'lua_register_js_callback',
  'lua_missing_globals',
  'lua_execute_main',
];
const allExports = host.Module.exports(mod);
const exported = new Set(allExports.map((e) => e.name));
const missing = required.filter((name) => !exported.has(name));
if (missing.length > 0) {
  console.error(`wasm_host_missing_exports: ${missing.join(',')}`);
  process.exit(1);
}

// Global 导出：iOS WXWebAssembly 明确拒绝（docs/52 R12）。当前 wasm-bindgen 会把
// __abort_handler / __instance_terminated 两条内部 global 塞进 Export 段；JS 胶水实测
// 完全不读它们（`__wbg_call_guard` 等 helper 一条也没落进 game_web.js），所以从 Export 段
// 削掉不影响运行时行为。webgl 侧只报告（现代浏览器都支持 Global，不需要处理）；
// 微信侧失败退出，让 package.ps1 在发布前先跑 strip-wasm-globals.mjs 把两条剔除。
const globals = allExports.filter((e) => e.kind === 'global').map((e) => e.name);
if (globals.length > 0) {
  const line = `wasm_host_globals host=${hostName} platform=${platform} names=[${globals.join(',')}]`;
  if (platform === 'wechat-minigame') {
    console.error(
      line +
      ' : iOS WXWebAssembly 不支持 Global 导出（docs/52 R12）；' +
      '发布链路应在 verify 之前跑 scripts/wasm-lua/strip-wasm-globals.mjs 从 Export 段剔除。'
    );
    process.exit(1);
  }
  console.error(line + ' (webgl 忽略：浏览器都支持 Global)');
}

console.log(
  `wasm_host_ok host=${hostName} platform=${platform} bytes=${bytes.length} ` +
  `exports=${allExports.length} globals=${globals.length} bridge_exports=${required.length}`
);
