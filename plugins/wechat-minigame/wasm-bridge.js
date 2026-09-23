// wasm-bridge.js：把 wasm-bindgen 的 `--target no-modules` 胶水接成小游戏能用的形态。
//
// ## 为什么需要这一层
//
// 小游戏运行时按 **CommonJS** 解析包里每个 `.js`，顶层 `export { … }` 会直接让启动失败：
//   WAGame.js:1 SyntaxError: MiniProgramError Unexpected token 'export'
// 所以 **不能**用 webgl 那份 `--target web` 的浏览器 ESM 胶水。
// 构建期因此为微信单独生成 `--target no-modules` 的胶水（纯脚本、无 import/export、
// 整体包在一个 IIFE 里），本文件再把那个 IIFE 的返回值导出成本模块的 CommonJS 导出。
//
// ## 它做什么、不做什么
//
// * **做**：加载胶水 → 用**包内的 wasm 字节**同步实例化（`initSync`）→ 报告导出清单，
//   全部证据进 `__P2_DIAG().wasm` 与一行启动日志。
// * **不做**：不调用 `start_wechat` / `start`。小游戏的游戏逻辑由 Fengari + `pack/` 承载
//   （与 webgl 发布同一条执行路径）；wasm 核心目前只到"能装进来、能实例化、导出对得上"。
//   `start_wechat` 的状态见 `engine/crates/game_web/src/lib.rs`（**未验证、未接线**）。
//
// ## 为什么用 `initSync` 而不是默认导出的 async init
//
// 小游戏的 `WXWebAssembly` 只有 `instantiate(path, imports)`；wasm 字节我们已经从包内读到了
// 手里，用同步的 `initSync(bytes)` 少一层"把字节包装成宿主认的形态"的猜测，
// 也不需要 `fetch` / `Response` / `instantiateStreaming`（小游戏三者都没有）。
'use strict';

// 胶水在模块作用域里用到的宿主全局。**必须在 require 之前装好**：
//   * `TextEncoder` —— `passStringToWasm0` 用它编码字符串；
//   * `WebAssembly` —— 实例化；小游戏侧是 `WXWebAssembly`（能力面等价，见下）；
//   * `console` —— 胶水自己的告警路径。
function ensureGlobals() {
  var g = typeof globalThis !== 'undefined' ? globalThis : (typeof global !== 'undefined' ? global : this);
  if (typeof g.TextEncoder !== 'function' && typeof TextEncoder === 'function') {
    g.TextEncoder = TextEncoder;
  }
  // `WXWebAssembly` 是宿主注入的全域名；只有它才认小游戏的 wasm 文件。
  // 注意：**不**把 `WXWebAssembly` 改名成 `WebAssembly`——那会覆盖宿主自己的实现。
  return g;
}

// 小游戏侧没有 `WebAssembly.instantiateStreaming`，也不认 `Response`；
// 胶水的 `initSync` 路径用 `new WebAssembly.Module(bytes)` + `new WebAssembly.Instance(...)`，
// 因此这里要把 `WebAssembly` 指向 `WXWebAssembly`（它提供了 Module/Instance 构造器）。
// 宿主连 Module/Instance 都没有时**如实报告**，不假装成功。
function resolveWebAssembly(g) {
  if (typeof g.WXWebAssembly === 'object' && g.WXWebAssembly !== null) {
    return { wasm: g.WXWebAssembly, source: 'WXWebAssembly' };
  }
  if (typeof g.WebAssembly === 'object' && g.WebAssembly !== null) {
    return { wasm: g.WebAssembly, source: 'WebAssembly' };
  }
  return { wasm: null, source: 'none' };
}

function toBytes(value) {
  if (!value) return null;
  if (value instanceof Uint8Array) return value;
  if (typeof ArrayBuffer !== 'undefined' && value instanceof ArrayBuffer) return new Uint8Array(value);
  // `readBytes` 可能给 `{ data: ArrayBuffer }`（wx 的 readFileSync 就该给这个）。
  if (value.data && typeof ArrayBuffer !== 'undefined' && value.data instanceof ArrayBuffer) {
    return new Uint8Array(value.data);
  }
  if (typeof value.byteLength === 'number' && typeof value.length !== 'number') {
    return new Uint8Array(value);
  }
  return null;
}

/**
 * 加载并实例化 wasm 核心。
 *
 * @param {object} options
 * @param {(path:string)=>*} options.readBytes  读包内二进制（应为适配层的 readBytes）
 * @param {string} [options.wasmPath]           wasm 在包内的逻辑路径
 * @param {(msg:string)=>void} [options.log]    日志回调
 * @returns {{ok:boolean, reason?:string, source?:string, exports?:string[], bytes?:number}}
 *          永不抛错：失败以 `{ok:false, reason}` 返回，由调用方决定是记诊断还是报警。
 *          这样"wasm 没装进来"不会掀翻 Lua 那条已经在跑的启动路径。
 */
function loadWasmCore(options) {
  var opts = options || {};
  var log = typeof opts.log === 'function' ? opts.log : function () {};
  var wasmPath = opts.wasmPath || 'pkg/game_web_bg.wasm';
  var g = ensureGlobals();

  var picked = resolveWebAssembly(g);
  if (!picked.wasm) {
    return { ok: false, reason: 'no-webassembly' };
  }
  if (typeof picked.wasm.Module !== 'function' || typeof picked.wasm.Instance !== 'function') {
    // 只有 `instantiate` 而没有 Module/Instance 的宿主：胶水的 initSync 走不通。
    // 如实报出来，而不是退化成"看起来加载了"。
    return { ok: false, reason: 'webassembly-api-incomplete', source: picked.source };
  }

  var bytes = toBytes(typeof opts.readBytes === 'function' ? opts.readBytes(wasmPath) : null);
  if (!bytes || bytes.length === 0) {
    return { ok: false, reason: 'wasm-bytes-unreadable' };
  }

  // 胶水的 `initSync` 用 `new WebAssembly.Module/Instance`，因此这里临时把
  // `WebAssembly` 指向宿主那份可用的实现，调用完立刻还原（不污染宿主全局）。
  var previous = g.WebAssembly;
  g.WebAssembly = picked.wasm;
  var glue;
  var exports;
  try {
    glue = require('./pkg/game_web.js');
    if (typeof glue !== 'function' || typeof glue.initSync !== 'function') {
      return { ok: false, reason: 'glue-shape-unexpected', source: picked.source };
    }
    var wasmExports = glue.initSync(bytes);
    exports = Object.keys(wasmExports || {})
      .filter(function (name) { return name.indexOf('__') !== 0; })
      .sort();
  } catch (error) {
    return {
      ok: false,
      reason: 'instantiate-failed',
      source: picked.source,
      detail: error && error.message ? String(error.message) : String(error),
    };
  } finally {
    if (previous === undefined) {
      try { delete g.WebAssembly; } catch (error) { g.WebAssembly = undefined; }
    } else {
      g.WebAssembly = previous;
    }
  }

  log('wasm-bridge: 核心实例化成功（' + picked.source + '，' + bytes.length + ' 字节，导出 '
    + exports.length + ' 项：' + exports.join(',') + '）');
  return { ok: true, source: picked.source, exports: exports, bytes: bytes.length };
}

module.exports = { loadWasmCore: loadWasmCore };
