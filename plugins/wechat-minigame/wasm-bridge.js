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
// ## 实例化通道（这是本文件存在的真正理由）
//
// 微信文档里 `WXWebAssembly` 只有 **一个** 入口：
//   WXWebAssembly.instantiate(path, imports)  →  Promise<{module, instance}>
//   第一个参数**只接受代码包路径字符串**（<125 字符），不吃字节数组，也没有流式版本。
//
// 而胶水的同步 `initSync(bytes)` 走的是 `new WebAssembly.Module(bytes)`——在小游戏宿主上
// 这条路根本不存在。实测 2026-09-30 开发者工具：
//   [wasm=instantiate-failed/WXWebAssembly detail=WebAssembly.instantiate: first argument must be a string]
// 那句 "must be a string" 就是宿主把我们的字节数组转发给它的 path 型实现时报的。
// 所以这里按宿主能力排两条通道，**都试、都留证据**：
//   * `path-instantiate`：把代码包路径交给宿主，异步。WXWebAssembly 下这是首选
//     （顺带省掉把 2.3 MB wasm 读进 JS 堆那一步）。
//   * `bytes-sync`：`initSync(bytes)`，只对有 Module/Instance 构造器的宿主有效
//     （Node、以及任何真给了完整 WebAssembly 的宿主）。
// 调用方因此拿到的是 Promise——适配层必须把 wasm 挂载排在 boot 之前（见 wechat-adapter.js）。
'use strict';

// 胶水在模块作用域里用到的宿主全局。**必须在 require 之前装好**：
//   * `TextEncoder` —— `passStringToWasm0` 用它编码字符串；
//   * `WebAssembly` —— 实例化；小游戏侧只有 `WXWebAssembly`（见上）；
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

// 宿主选择：优先 `WXWebAssembly`（小游戏唯一的 wasm 入口），没有才回退到标准 `WebAssembly`。
// 注意这条回退只对本机验证有意义：真机上两个都不存在时就是 `no-webassembly`。
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

// 代码包路径的候选写法。开发者工具与真机对前导 `/` 的容忍度不同（适配层读普通文件时
// 就要探方言），wasm 这里同理：先按发布产物里的原样试，再试补/去前导斜杠的两种写法。
function pathCandidates(wasmPath) {
  var clean = String(wasmPath || '').replace(/^\/+/, '');
  var out = [];
  function push(value) {
    if (value && out.indexOf(value) < 0) out.push(value);
  }
  push(clean);
  push('/' + clean);
  return out;
}

function listExports(wasmExports) {
  return Object.keys(wasmExports || {})
    .filter(function (name) { return name.indexOf('__') !== 0; })
    .sort();
}

function errorDetail(error) {
  return error && error.message ? String(error.message) : String(error);
}

function requireGlue(g) {
  var glue = require('./pkg/game_web.js');
  if (typeof glue !== 'function') throw new Error('glue-shape-unexpected');
  return glue;
}

// 通道一：路径式异步实例化。
//
// 难点在于胶水的 `imports` 对象是闭包私有的（`__wbg_get_imports()`），外面拿不到。
// 这里不复制胶水逻辑，而是**反向利用它**：`glue({})`（即 `__wbg_init`）在非 Response 分支
// 会调用 `WebAssembly.instantiate(module, imports)`，而 imports 正是胶水自己造好的那一份。
// 于是临时把全局 `WebAssembly` 换成一个 shim：丢掉胶水给的第一参数（字节/undefined），
// 把第二参数原样交给 `WXWebAssembly.instantiate(路径, imports)`。
// `Instance` 给一个永不匹配的构造器，让胶水的 `instanceof WebAssembly.Instance` 判为假，
// 从而把宿主返回的 `{module, instance}` 整包透传——胶水最终仍会跑
// `__wbg_finalize_init`（设置内部导出句柄并调用 `__wbindgen_start`），后续 wasm 函数才可用。
function instantiateByPath(g, glue, wasm, wasmPath) {
  var previous = g.WebAssembly;
  var restore = function () {
    if (previous === undefined) {
      try { delete g.WebAssembly; } catch (error) { g.WebAssembly = undefined; }
    } else {
      g.WebAssembly = previous;
    }
  };
  var usedPath = null;
  var shim = {};
  shim.instantiate = function (_ignored, imports) {
    usedPath = wasmPath;
    var result = wasm.instantiate(wasmPath, imports);
    if (!result || typeof result.then !== 'function') {
      throw new Error('host instantiate did not return a Promise');
    }
    return result;
  };
  shim.Instance = function Instance() { };
  // 胶水不查这几个，但宿主有就转过去，别在 shim 后面凭空少掉。
  ['Memory', 'Table', 'Global'].forEach(function (name) {
    if (typeof wasm[name] === 'function') shim[name] = wasm[name];
  });
  g.WebAssembly = shim;
  return Promise.resolve()
    .then(function () { return glue({}); })
    .then(function (wasmExports) {
      restore();
      return { exports: listExports(wasmExports), path: usedPath };
    }, function (error) {
      restore();
      throw error;
    });
}

// 通道二：字节同步实例化（只对有 Module/Instance 的宿主成立）。
function instantiateByBytes(g, glue, wasm, bytes) {
  // 胶水的 `initSync` 用 `new WebAssembly.Module/Instance`，因此这里临时把
  // `WebAssembly` 指向宿主那份可用的实现，调用完立刻还原（不污染宿主全局）。
  var previous = g.WebAssembly;
  g.WebAssembly = wasm;
  try {
    if (typeof glue.initSync !== 'function') throw new Error('glue-shape-unexpected');
    return { exports: listExports(glue.initSync(bytes)) };
  } finally {
    if (previous === undefined) {
      try { delete g.WebAssembly; } catch (error) { g.WebAssembly = undefined; }
    } else {
      g.WebAssembly = previous;
    }
  }
}

/**
 * 加载并实例化 wasm 核心。
 *
 * @param {object} options
 * @param {(path:string)=>*} [options.readBytes] 读包内二进制（仅字节通道用）
 * @param {string} [options.wasmPath]            wasm 在代码包内的路径
 * @param {(msg:string)=>void} [options.log]     日志回调
 * @returns {Promise<{ok:boolean, channel?:string, source?:string, exports?:string[],
 *                    bytes?:number, path?:string, host?:Function,
 *                    reason?:string, detail?:string, attempts?:object[]}>}
 *          永不 reject：失败以 `{ok:false, reason, detail, attempts}` 兑现，由调用方决定
 *          是记诊断还是报警。两条通道都试过才判失败，`attempts` 带每条通道的原始原因。
 */
function loadWasmCore(options) {
  var opts = options || {};
  var log = typeof opts.log === 'function' ? opts.log : function () { };
  var wasmPath = opts.wasmPath || 'pkg/game_web_bg.wasm';
  var g = ensureGlobals();

  var picked = resolveWebAssembly(g);
  if (!picked.wasm) {
    return Promise.resolve({ ok: false, reason: 'no-webassembly' });
  }
  if (typeof picked.wasm.instantiate !== 'function'
    && (typeof picked.wasm.Module !== 'function' || typeof picked.wasm.Instance !== 'function')) {
    // 既没有 instantiate 也没有 Module/Instance 构造器：两条通道都无从谈起。
    return Promise.resolve({ ok: false, reason: 'webassembly-api-incomplete', source: picked.source });
  }

  // WXWebAssembly 上路径通道是文档保证的唯一形态，字节通道只是它"看起来也可能有"的
  // 兼容层；标准 WebAssembly（本机 Node）反过来——字节同步最直接，路径要按 cwd 解析。
  // 所以首选通道跟着宿主走。
  var order = picked.source === 'WXWebAssembly'
    ? ['path-instantiate', 'bytes-sync']
    : ['bytes-sync', 'path-instantiate'];

  var glue;
  try {
    glue = requireGlue(g);
  } catch (error) {
    return Promise.resolve({
      ok: false,
      reason: error && error.message === 'glue-shape-unexpected' ? 'glue-shape-unexpected' : 'glue-missing',
      source: picked.source,
      detail: errorDetail(error),
    });
  }

  var attempts = [];
  var bytes = null;
  var bytesError = null;
  function bytesAtHand() {
    if (bytes) return bytes;
    if (bytesError) throw bytesError;
    if (typeof opts.readBytes !== 'function') {
      bytesError = new Error('wasm-bytes-unreadable');
      throw bytesError;
    }
    bytes = toBytes(opts.readBytes(wasmPath));
    if (!bytes || bytes.length === 0) {
      bytesError = new Error('wasm-bytes-unreadable');
      throw bytesError;
    }
    return bytes;
  }

  function runBytes() {
    if (typeof picked.wasm.Module !== 'function' || typeof picked.wasm.Instance !== 'function') {
      return Promise.resolve({
        ok: false, reason: 'webassembly-api-incomplete', channel: 'bytes-sync',
        detail: '宿主没有 Module/Instance 构造器，只能走路径通道',
      });
    }
    try {
      var result = instantiateByBytes(g, glue, picked.wasm, bytesAtHand());
      return Promise.resolve({ ok: true, channel: 'bytes-sync', exports: result.exports, bytes: bytes.length });
    } catch (error) {
      // 读不到字节与"字节被宿主拒绝"是两件不同的事：前者是发布形态（wasm 没打进包），
      // 后者是产物本身（EH 版本、胶水不匹配）。合并成一个 status 会把排查方向带偏。
      var message = errorDetail(error);
      return Promise.resolve({
        ok: false,
        reason: message.indexOf('wasm-bytes-unreadable') >= 0 ? 'wasm-bytes-unreadable' : 'instantiate-failed',
        channel: 'bytes-sync',
        detail: message,
      });
    }
  }

  function runPath() {
    if (typeof picked.wasm.instantiate !== 'function') {
      return Promise.resolve({
        ok: false, reason: 'webassembly-api-incomplete', channel: 'path-instantiate',
        detail: '宿主没有 instantiate(path, imports)',
      });
    }
    var candidates = pathCandidates(wasmPath);
    var index = 0;
    function next() {
      if (index >= candidates.length) {
        // summarized：逐路径的原始宿主信息已经各自进了 attempts，这里只留一句汇总，
        // 不再重复登记一条（否则失败清单里同一件事出现两遍，看不出是几条独立证据）。
        // 宿主的原始信息排在最前——屏幕上的失败行只截得到 detail。
        var last = attempts.length ? String(attempts[attempts.length - 1].detail) + '（' : '（';
        return {
          ok: false, reason: 'path-instantiate-failed', channel: 'path-instantiate', summarized: true,
          detail: last + '试过 ' + candidates.join(' | ') + '）'
        };
      }
      var candidate = candidates[index++];
      return instantiateByPath(g, glue, picked.wasm, candidate)
        .then(function (result) {
          return { ok: true, channel: 'path-instantiate', exports: result.exports, path: result.path || candidate };
        }, function (error) {
          // 每条候选路径的原始宿主信息都留档：只有某条写法可用是"宿主方言"问题，
          // 全部失败才是"包里没有/编译被拒"问题，两者靠这几条区分。
          attempts.push({ channel: 'path-instantiate', path: candidate, detail: errorDetail(error) });
          return next();
        });
    }
    return Promise.resolve().then(next);
  }

  var chain = order[0] === 'path-instantiate' ? runPath : runBytes;
  var fallback = order[1] === 'path-instantiate' ? runPath : runBytes;
  var primaryChannel = order[0];

  return Promise.resolve()
    .then(chain)
    .then(function (first) {
      if (first.ok) return first;
      // 通道级失败汇总（路径通道的逐路径记录已经在 attempts 里，别再重复一遍）。
      if (!first.summarized) attempts.push(first);
      return fallback().then(function (second) {
        if (!second.ok && !second.summarized) attempts.push(second);
        return { failed: [first, second] };
      });
    })
    .then(function (result) {
      if (result.ok) {
        log('wasm-bridge: 核心实例化成功（' + picked.source + '/' + result.channel
          + (result.path ? '，路径 ' + result.path : '')
          + (result.bytes ? '，' + result.bytes + ' 字节' : '')
          + '，导出 ' + result.exports.length + ' 项：' + result.exports.join(',') + '）');
        // host = 胶水函数（wasm-bindgen `--target no-modules` 的导出载体）：调用方可用
        // `new host.HostApi(name)` 实例化宿主桥挂到 `window.__RUSTLUA_HOST_API`（WP10 微信侧接线）。
        return {
          ok: true,
          source: picked.source,
          channel: result.channel,
          path: result.path || null,
          exports: result.exports,
          bytes: result.bytes || null,
          host: glue,
          attempts: attempts,
        };
      }
      // 两条都没过时，**判据按首选通道给**：那条才是宿主能力面要求走的那条，
      // 兜底通道的失败原因（例如"没有 Module/Instance"）只是它的附注。
      // 曾经这里用兜底的结果当 reason，于是微信宿主上路径通道被拒时，屏幕上只剩
      // `webassembly-api-incomplete` ——把真正要修的那一层藏进了 attempts 里。
      var failed = result.failed || [];
      var primary = failed[0] || { reason: 'instantiate-failed', channel: primaryChannel };
      return {
        ok: false,
        reason: primary.reason,
        source: picked.source,
        channel: primary.channel || primaryChannel,
        detail: primary.detail || '',
        attempts: attempts
      };
    });
}

module.exports = { loadWasmCore: loadWasmCore, pathCandidates: pathCandidates };
