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
//   * `TextEncoder` —— `passStringToWasm0` 用它编码字符串；`game_web.js:1214` 求值期就 `new TextEncoder()`；
//   * `TextDecoder` —— `decodeText` 用它解码 wasm 内存里的字符串；`game_web.js:1208` 求值期就 `new TextDecoder(...)`；
//   * `WebAssembly` —— 求值期就要（tag 导入 + `wrapError` 造 Exception）。小游戏侧只有
//     `WXWebAssembly` 域名，标准全局缺席时由下面的 facade 转发补上；
//   * `console` —— 胶水自己的告警路径。
//
// **文本编解码器为何要 polyfill**：真机小游戏（2026-10-03 vconsole 实测）把 `WebAssembly`、
// `TextEncoder`、`TextDecoder` 三个标准全局**全部缺席**（读数：TextEncoder=absent TextDecoder=absent）。
// 胶水在 require 求值期就无条件 `new TextDecoder('utf-8', …)`（见上面 facade 段的同源问题），
// 于是 require 直接抛 `ReferenceError: TextDecoder is not defined`，被判成 `glue-missing`，
// wasm 桥装不上，最终 "wasm lua bridge not loaded"。仅"宿主有就转发"顶不住这条——宿主根本没有。
// 所以这里在转发之外再补一层纯 JS 的 UTF-8 polyfill，保证胶水求值期这两个构造器一定存在。
function ensureGlobals(g) {
  if (!g) g = typeof globalThis !== 'undefined' ? globalThis : (typeof global !== 'undefined' ? global : this);
  // 优先用宿主原生（本机 Node / 有实现的宿主走这条）；`typeof x` 对未声明全局安全，不会抛。
  if (typeof g.TextEncoder !== 'function' && typeof TextEncoder === 'function') {
    g.TextEncoder = TextEncoder;
  }
  if (typeof g.TextDecoder !== 'function' && typeof TextDecoder === 'function') {
    g.TextDecoder = TextDecoder;
  }
  // 宿主确实没有时，装纯 JS UTF-8 polyfill（真机小游戏走这条）。返回补位清单给调用方落证据。
  var installed = [];
  if (typeof g.TextEncoder !== 'function') { g.TextEncoder = makeTextEncoderPolyfill(); installed.push('TextEncoder'); }
  if (typeof g.TextDecoder !== 'function') { g.TextDecoder = makeTextDecoderPolyfill(); installed.push('TextDecoder'); }
  // `WXWebAssembly` 是宿主注入的全域名；只有它才认小游戏的 wasm 文件。
  // **不**把整份 `WXWebAssembly` 直接改名成 `WebAssembly`——那会覆盖宿主自己的实现；
  // 只在宿主确实没有标准全局时，按 installWebAssemblyFacade 逐条转发构造器。
  return installed;
}

// ---------- TextEncoder / TextDecoder polyfill（真机小游戏缺这两个全局时的兜底） ----------
//
// 只实现胶水真正用到的那部分 API，够 `--target no-modules` 胶水跑起来即可：
//   * `new TextEncoder().encode(str) -> Uint8Array`；
//     **不**定义 `encodeInto`——胶水自己有 `if (!('encodeInto' in cachedTextEncoder))` 的回退分支，
//     会用我们的 `encode()` 现造一个，交给胶水那段久经考验的逻辑，比我们重写更稳。
//   * `new TextDecoder('utf-8', { fatal, ignoreBOM }).decode(input?)`；
//     `decode()` 无参返回 ''（与原生一致，胶水求值期就调了一次空 decode），
//     `decode(Uint8Array)` 按 UTF-8 解码；`fatal:true` 时遇非法字节抛 TypeError，否则替换成 U+FFFD。
// 输入总是来自 wasm 内存里的合法 UTF-8（Rust/Lua 字符串），fatal 分支实际不会触发。
// 全部用 ES2019 及以下语法（上传器实测只到 ES2019）：`codePointAt` / `String.fromCharCode` 都在范围内。
function makeTextEncoderPolyfill() {
  function TextEncoderPolyfill() { this.encoding = 'utf-8'; }
  TextEncoderPolyfill.prototype.encode = function (str) {
    str = str === undefined ? '' : String(str);
    var bytes = [];
    var i = 0;
    var len = str.length;
    while (i < len) {
      var cp = str.codePointAt(i);
      i += cp > 0xffff ? 2 : 1;
      if (cp < 0x80) {
        bytes.push(cp);
      } else if (cp < 0x800) {
        bytes.push(0xc0 | (cp >> 6), 0x80 | (cp & 0x3f));
      } else if (cp < 0x10000) {
        bytes.push(0xe0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
      } else {
        bytes.push(0xf0 | (cp >> 18), 0x80 | ((cp >> 12) & 0x3f), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
      }
    }
    return new Uint8Array(bytes);
  };
  return TextEncoderPolyfill;
}

function makeTextDecoderPolyfill() {
  function TextDecoderPolyfill(label, options) {
    options = options || {};
    this.encoding = 'utf-8';
    this.fatal = !!options.fatal;
    this.ignoreBOM = !!options.ignoreBOM;
  }
  TextDecoderPolyfill.prototype.decode = function (input) {
    if (!input) return '';
    var bytes = input instanceof Uint8Array
      ? input
      : new Uint8Array(input.buffer ? input.buffer : input);
    var fatal = this.fatal;
    var out = '';
    var n = bytes.length;
    var i = 0;
    // ignoreBOM 为假时剥掉开头一个 EF BB BF（胶水用 ignoreBOM:true，不剥）。
    if (!this.ignoreBOM && n >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
      i = 3;
    }
    while (i < n) {
      var b = bytes[i];
      var cp;
      var extra;
      if (b < 0x80) { cp = b; extra = 0; }
      else if ((b & 0xe0) === 0xc0) { cp = b & 0x1f; extra = 1; }
      else if ((b & 0xf0) === 0xe0) { cp = b & 0x0f; extra = 2; }
      else if ((b & 0xf8) === 0xf0) { cp = b & 0x07; extra = 3; }
      else {
        if (fatal) throw new TypeError('invalid UTF-8 lead byte 0x' + b.toString(16) + ' at ' + i);
        out += '\ufffd'; i += 1; continue;
      }
      if (i + extra >= n) {
        if (fatal) throw new TypeError('truncated UTF-8 sequence at ' + i);
        out += '\ufffd'; break;
      }
      var ok = true;
      for (var k = 1; k <= extra; k++) {
        var cb = bytes[i + k];
        if ((cb & 0xc0) !== 0x80) { ok = false; break; }
        cp = (cp << 6) | (cb & 0x3f);
      }
      if (!ok) {
        if (fatal) throw new TypeError('invalid UTF-8 continuation at ' + i);
        out += '\ufffd'; i += 1; continue;
      }
      i += extra + 1;
      if (cp > 0xffff) {
        cp -= 0x10000;
        out += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
      } else {
        out += String.fromCharCode(cp);
      }
    }
    return out;
  };
  return TextDecoderPolyfill;
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

// ---------- 标准域名 facade（2026-10-03 开发者工具实测的缺口） ----------
//
// 胶水不只是"实例化时用 WebAssembly"，它在 **require 求值期**就无条件读标准域名：
//   let cachedTextDecoder = new TextDecoder('utf-8', …)      // IIFE 前半段
//   const __wbindgen_jstag_polyfill = new WebAssembly.Tag({ parameters: ['externref'] })
// 那两条 Tag 是 wasm 的**导入**（`node -e … Module.imports` 实测：56 条导入里 54 function + 2 tag，
// 名字是 `__wbindgen_jstag` / `__wbindgen_wrapped_jstag`），所以它们必须是**宿主真的 Tag 对象**
// ——facade 只能转发 `WXWebAssembly` 上已有的构造器，伪造一个 JS 对象会在 instantiate 时被拒。
// 开发者工具的小游戏运行时只注入 `WXWebAssembly` 域名，标准 `WebAssembly` 全局**不存在**，
// 于是 `require('./pkg/game_web.js')` 直接抛 ReferenceError，被判成误导性的
// `[wasm=glue-missing/WXWebAssembly detail=WebAssembly is not defined]`（胶水其实在包里）。
// 本机 Node 有真 `WebAssembly` 全局，所以这条缺口在本机永远不响——与 R12 同一类
// "本机全绿、工具里起不来"。harness 现在复刻这个形态（摘掉标准全局、假宿主给 Tag/Exception），
// 判据是证据行里的 `wasm=…:…:…:Tag+Exception+…`，见 scripts/wechat-minigame-harness.js。
var GLUE_EVAL_CTORS = ['Tag', 'Exception', 'Memory', 'Table', 'Global'];

// ---------- Tag / Exception polyfill（宿主 WXWebAssembly 缺这两个时的降级） ----------
//
// 胶水在模块作用域无条件执行：
//   const __wbindgen_jstag_polyfill = new WebAssembly.Tag({ parameters: ['externref'] })
//   const __wbindgen_wrapped_jstag = new WebAssembly.Tag({ parameters: ['externref'] })
// 而 `wrapError` 在每次 JS→wasm 桥接调用时执行：
//   throw new WebAssembly.Exception(__wbindgen_jstag_polyfill, [e], { traceStack: true })
// 微信开发者工具（2026-10-03 实测）的 WXWebAssembly 有 Global/Instance/Memory/Module/Table/
// compile/instantiate，唯独缺 Tag 与 Exception。没有 polyfill 时胶水在 require 求值期就抛
// ReferenceError，整个 wasm 模块加载失败——也就是你看到的那条
// "wasm lua bridge not loaded (fengari execution layer removed)"。
//
// polyfill 不模拟 wasm 引擎的 tag 匹配语义（JS 层面做不到），只提供足够的 API 表面让：
//   1) 胶水求值期 `new Tag(...)` 不抛；
//   2) `wrapError` 的 `new Exception(tag, values)` 能造出对象；
//   3) `instanceof WebAssembly.Exception` 判真，使已有 `if (e instanceof WebAssembly.Exception) throw e`
//      路径正常透传，不会把 wasm 异常吞成普通 JS Error。
//
// 这个"关键验证点"已被实测回答（见 verify-wechat-minigame.ps1 用例 1a3 的 polyfill_tag_probe）：
// WXWebAssembly.instantiate 收到 JS 伪造的 polyfill tag 时**拒绝**（要求真实的 WebAssembly.Tag），
// 所以 PolyfillTag 顶不住 tag 导入——修法是把 tag 导入从 wasm 二进制里剥掉（scripts/wasm-lua
// /strip-wasm-tags.mjs，只剥两个 externref 导入、留着内部 $tag$0）。剥完之后剩下的设备问题换成
// "引擎认不认 EH 本身"，由下面的 EH 真机探针（probeEHAcceptance）负责，不再是这里。
//
// 已知限制：wasm 侧 `__wbg_handle_catch` 用 `e.is(__wbindgen_wrapped_jstag)` 判断异常类型，
// polyfill 的 `is()` 只比 tag 标识，不与 wasm 引擎的内部 tag 索引对齐——所以 wasm 内部 catch
// （Rust catch_unwind / Lua pcall 跨 JS 边界场景）可能拦不住从 JS 抛回去的异常。
// 不影响"正常无异常"的代码路径；影响的是"JS 回调抛异常 → Rust catch"这条错误处理路径。
function PolyfillTag(descriptor) {
  this._params = (descriptor && descriptor.parameters) || [];
}
PolyfillTag.prototype.parameter = function (index) {
  return this._params[index] || null;
};

function PolyfillException(tag, values, options) {
  if (Array.isArray(tag)) {
    // Exception(tag) 形式（无 values）
    this._tag = tag[0] || null;
    this._values = [];
  } else {
    this._tag = tag;
    this._values = values || [];
  }
  if (options && options.traceStack) {
    this.stack = (new Error()).stack;
  }
}
PolyfillException.prototype.is = function (tag) {
  return this._tag === tag;
};
PolyfillException.prototype.getArg = function (tag, index) {
  if (this._tag !== tag) throw new TypeError('exception tag mismatch');
  return this._values[index];
};

function hostKeys(host) {
  try { return Object.keys(host || {}).sort().join(',') || 'none'; } catch (error) { return 'unreadable'; }
}

// 胶水求值期要读的宿主全局，缺席时一次性报出来：屏幕上只截得到 detail，值得把整张能力面带上，
// 免得"改好 Tag 又死在 TextDecoder 上"要再跑一轮开发者工具。
// **取的是装 polyfill / facade 之前的快照**（loadWasmCore 里存成 hostGlobals）：这两层一装
// `WebAssembly`/`TextEncoder`/`TextDecoder` 就恒为 present，这句读数会开始撒谎。
function evalGlobalsReadout(g) {
  var names = ['WebAssembly', 'TextEncoder', 'TextDecoder'];
  return names.map(function (name) {
    return name + '=' + (typeof g[name] === 'undefined' ? 'absent' : 'present');
  }).join(' ');
}

// 把宿主那份构造器按标准域名转发一遍；宿主缺的（微信小游戏实测缺 Tag 与 Exception）用上面的
// polyfill 补位，让胶水求值期 `new WebAssembly.Tag(...)` 与 `wrapError` 里的 `new WebAssembly.Exception(...)`
// 都不抛。返回里 `native` 记宿主真给的、`polyfilled` 记我们补的——调用方据此判"facade 是不是全原生"。
function installWebAssemblyFacade(g, host) {
  var facade = {};
  var native = [];
  var polyfilled = [];
  GLUE_EVAL_CTORS.forEach(function (name) {
    if (host && typeof host[name] === 'function') {
      facade[name] = host[name];
      native.push(name);
    } else if (name === 'Tag') {
      facade[name] = PolyfillTag;
      polyfilled.push(name);
    } else if (name === 'Exception') {
      facade[name] = PolyfillException;
      polyfilled.push(name);
    }
  });
  g.WebAssembly = facade;
  return { native: native, polyfilled: polyfilled };
}

// ---------- EH 真机探针（2026-10-03）----------
//
// 剥掉 tag 导入之后，产物里仍然留着**本地 tag 段**（`$tag$0`，实测在 wasm 里出现 3 次：
// 一条声明 + 一条 `throw $tag$0` + 一条 `catch $tag$0`）与**legacy EH 指令**
// （try=92 / catch_all=501 / rethrow=92 / delegate=3，try_table=0）。那是 Rust `catch_unwind`
// / Lua `pcall` 的实现，**不能剥**（上一批"整块剥 EH"把 play 路径打成 `Error: unreachable` 就是
// 证据）。于是剩下的决定性问题是：**微信 `WXWebAssembly` 底下那台引擎到底实没实现 wasm 异常处理
// 这个 proposal？**
//   实现   ⇒ 剥完 tag 导入的产物能在真机上编译/实例化，pcall 语义有地方落；
//   没实现 ⇒ 引擎连本地 tag 段都拒绝，剥二进制这条路救不了微信包，只能换"根本不用 EH"的产物形态
//     （RUSTFLAGS 去掉 -fwasm-exceptions），并重新验 Lua 错误处理怎么走。
// 本机 V8 支持 EH，所以 harness **永远测不出**"设备不支持"这一侧——这条只能真机读，harness 那侧
// 的价值是证明"探针没接错"（正对照：V8 必须 accepted）。
//
// 探针用一份**隔离**的极小模块 `pkg/eh-probe.wasm`（53 字节：一个本地 `(tag (param i32))`
// + 一条 legacy `try/catch/throw`、**零导入**，由发布链路里同一个 binaryen-133 `wasm-as` 汇编），
// 而不是直接 compile 主产物——主产物还顺带用了 GC/reference-types/bulk-memory 等一片特性，
// compile 失败分不清是不是 EH 的锅。极小模块把变量收到只剩 EH：它被接受就证明引擎实现 EH，
// 被拒绝的话宿主原话直指 EH。
// 用 `compile(path)`：只校验解码、不链接，EH 指令不支持会在这一步就报出来。没有 `compile`
// 就退到 `instantiate(path, {})`——这份模块零导入，空 imports 正好能一路跑到底，"实例化成功"
// 是比"编译成功"更强的证据。
// **必须自带超时**：这个 Promise 与实例化并发排在启动链路上，宿主不兑现它也不能拖垮 boot。
var PROBE_TIMEOUT_MS = 3000;
var DEFAULT_EH_PROBE_PATH = 'pkg/eh-probe.wasm';

// 永不 reject：任何分支都兑现成一句 `eh_probe=…` 读数，交给调用方（适配层）落进诊断里。
function probeEHAcceptance(host, probePath) {
  var entry = null;
  var entryName = '';
  if (host && typeof host.compile === 'function') { entry = function (p) { return host.compile(p); }; entryName = 'compile'; }
  else if (host && typeof host.instantiate === 'function') { entry = function (p) { return host.instantiate(p, {}); }; entryName = 'instantiate-empty-imports'; }
  if (!entry) return Promise.resolve('eh_probe=host-compile-missing');

  var candidates = pathCandidates(probePath || DEFAULT_EH_PROBE_PATH);
  var index = 0;
  function attempt() {
    if (index >= candidates.length) return Promise.resolve('eh_probe=all-paths-rejected');
    var candidate = candidates[index++];
    var settled = false;
    var raced = new Promise(function (resolve) {
      var timer = setTimeout(function () {
        if (settled) return;
        settled = true;
        resolve('eh_probe=' + entryName + '-timeout');
      }, PROBE_TIMEOUT_MS);
      var result;
      try {
        result = entry(candidate);
      } catch (error) {
        clearTimeout(timer);
        if (settled) return;
        settled = true;
        resolve('eh_probe=' + entryName + '-throw[' + candidate + ']=' + errorDetail(error));
        return;
      }
      if (!result || typeof result.then !== 'function') {
        clearTimeout(timer);
        if (settled) return;
        settled = true;
        resolve('eh_probe=' + entryName + '-not-promise');
        return;
      }
      result.then(function () {
        clearTimeout(timer);
        if (settled) return;
        settled = true;
        resolve('eh_probe=accepted（引擎实现 wasm EH：本地 tag 段 + legacy try/catch 都认）');
      }, function (error) {
        clearTimeout(timer);
        if (settled) return;
        settled = true;
        // 第一条候选路径的原始信息就够定方向了，剩下的写法差异不必再刷进 detail。
        resolve('eh_probe=' + entryName + '-reject[' + candidate + ']=' + errorDetail(error));
      });
    });
    return raced.then(function (text) {
      // compile 被拒的原因可能是"路径写法宿主不认"而不是"模块被拒"，两种要分开：
      // 只有像是文件不存在的报错才继续试下一条写法。
      if (index < candidates.length && /no such file|not found|permission denied/i.test(text)) return attempt();
      return text;
    });
  }
  return attempt();
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

// docs/65 P0-⑥ 的 JS 侧读数通道：记住实例的**线性内存对象**并暴露一个只读函数。
// `memory` 句柄是活的——内存增长后 `memory.buffer.byteLength` 就是新值，无需重新拿实例。
// 这与 Rust 侧 `wasm_memory_bytes`（begin summary / state JSON 里的 `wasm_mem_bytes`）
// 是两个独立口径，同问对拍用；只诊断、不改变任何行为，装不上就算了（不打扰启动）。
function installWasmMemoryReader(g, wasmExports) {
  var memory = wasmExports && wasmExports.memory;
  if (!memory) return false;
  try {
    g.__RUSTLUA_WASM_MEMORY_BYTES = function () {
      try { return memory.buffer.byteLength; } catch (error) { return -1; }
    };
    return true;
  } catch (error) { return false; }
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
  // `Tag` / `Exception` 也在其列：shim 生效期间 `__wbg_finalize_init` 会跑 `wasm.__wbindgen_start()`，
  // Rust 侧一旦抛 JS 异常就走 `wrapError` → `new WebAssembly.Exception(...)`，
  // 那时读的正是这个被换掉的全局。
  GLUE_EVAL_CTORS.forEach(function (name) {
    if (typeof wasm[name] === 'function') shim[name] = wasm[name];
  });
  // Tag / Exception 宿主可能没给（WXWebAssembly 实测缺这两个），但 facade 层已经用 polyfill 补上
  // （installWebAssemblyFacade 装在 `previous` 里）。shim 必须也带上——`wrapError` 在 shim 生效期间
  // 执行，读的是 shim.Exception / shim.Tag，不是 facade。没有就从 facade 兜底拿。
  if (typeof shim.Tag !== 'function' && previous && typeof previous.Tag === 'function') {
    shim.Tag = previous.Tag;
  }
  if (typeof shim.Exception !== 'function' && previous && typeof previous.Exception === 'function') {
    shim.Exception = previous.Exception;
  }
  g.WebAssembly = shim;
  return Promise.resolve()
    .then(function () { return glue({}); })
    .then(function (wasmExports) {
      restore();
      installWasmMemoryReader(g, wasmExports);
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
    var wasmExports = glue.initSync(bytes);
    installWasmMemoryReader(g, wasmExports);
    return { exports: listExports(wasmExports) };
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
 * @param {string} [options.ehProbePath]         EH 真机探针模块在代码包内的路径（默认 pkg/eh-probe.wasm）
 * @param {(verdict:string)=>void} [options.onEhProbe] EH 探针读数回调（`eh_probe=…`）：与实例化并发、
 *        不挡 boot，宿主不兑现也在 3s 内自带超时收尾；调用方据此把"设备引擎认不认 EH"落进诊断。
 * @returns {Promise<{ok:boolean, channel?:string, source?:string, exports?:string[],
 *                    bytes?:number, path?:string, host?:Function, facade?:string,
 *                    reason?:string, detail?:string, attempts?:object[]}>}
 *          永不 reject：失败以 `{ok:false, reason, detail, attempts}` 兑现，由调用方决定
 *          是记诊断还是报警。两条通道都试过才判失败，`attempts` 带每条通道的原始原因。
 */
function loadWasmCore(options) {
  var opts = options || {};
  var log = typeof opts.log === 'function' ? opts.log : function () { };
  var wasmPath = opts.wasmPath || 'pkg/game_web_bg.wasm';
  var g = typeof globalThis !== 'undefined' ? globalThis : (typeof global !== 'undefined' ? global : this);
  // 宿主自己给的全局面，装 polyfill / facade **之前**取（读数要说的是宿主，不是我们补出来的东西）。
  var hostGlobals = evalGlobalsReadout(g);
  var textPolyfilled = ensureGlobals(g);
  if (textPolyfilled.length) {
    log('wasm-bridge: 宿主缺 ' + textPolyfilled.join('/') + '，已装 UTF-8 polyfill（胶水求值期需要）');
  }

  var picked = resolveWebAssembly(g);
  if (!picked.wasm) {
    return Promise.resolve({ ok: false, reason: 'no-webassembly', detail: hostGlobals });
  }
  if (typeof picked.wasm.instantiate !== 'function'
    && (typeof picked.wasm.Module !== 'function' || typeof picked.wasm.Instance !== 'function')) {
    // 既没有 instantiate 也没有 Module/Instance 构造器：两条通道都无从谈起。
    return Promise.resolve({ ok: false, reason: 'webassembly-api-incomplete', source: picked.source, detail: 'host keys=' + hostKeys(picked.wasm) + '｜' + hostGlobals });
  }

  // **必须在 requireGlue 之前**补标准域名：胶水求值期就 `new WebAssembly.Tag(...)`（见上面 facade 段）。
  // 宿主本来就有标准 `WebAssembly` 时不碰它（本机 Node / 浏览器走这条）。
  var facade = null;
  if (picked.source === 'WXWebAssembly' && typeof g.WebAssembly === 'undefined') {
    facade = installWebAssemblyFacade(g, picked.wasm);
  }
  // Tag / Exception 缺席时由 installWebAssemblyFacade 装 polyfill（见上面 PolyfillTag /
  // PolyfillException）：让胶水求值期万一还读到标准域名时不抛 ReferenceError。
  // 注意产物已经**不再导入 tag**（发布链路 strip-wasm-tags.mjs 剥掉两个 externref tag 导入 +
  // package.ps1 简化掉胶水的 Tag/Exception 用法），所以这里只是安全网；instantiate 收 polyfill
  // tag 会被宿主拒这条早已被实测确认（用例 1a3 polyfill_tag_probe=rejected），正是剥二进制的理由。
  // 剥完之后真机上还留着本地 tag 段 + legacy EH，那个"引擎认不认 EH"的问题交给下面的 EH 探针。

  // WXWebAssembly 上路径通道是文档保证的唯一形态，字节通道只是它"看起来也可能有"的
  // 兼容层；标准 WebAssembly（本机 Node）反过来——字节同步最直接，路径要按 cwd 解析。
  // 所以首选通道跟着宿主走。
  var order = picked.source === 'WXWebAssembly'
    ? ['path-instantiate', 'bytes-sync']
    : ['bytes-sync', 'path-instantiate'];

  // EH 真机探针：与实例化**并发**跑，绝不挡 boot——结果只经 opts.onEhProbe 回调落到诊断里
  // （它自带 3s 超时，且这条读数要的是"设备引擎认不认 EH"，与本次实例化成败是两条独立证据）。
  // 宿主连 compile/instantiate 都没有时前面已 return，这里 entry 不会为空。
  probeEHAcceptance(picked.wasm, opts.ehProbePath).then(function (verdict) {
    if (typeof opts.onEhProbe === 'function') {
      try { opts.onEhProbe(verdict); } catch (probeCbError) { /* 回调抛错不许影响主链路 */ }
    }
  });

  var glue;
  try {
    glue = requireGlue(g);
  } catch (error) {
    return Promise.resolve({
      ok: false,
      reason: error && error.message === 'glue-shape-unexpected' ? 'glue-shape-unexpected' : 'glue-missing',
      source: picked.source,
      // 胶水**在包里**却 require 失败时，真正的原因几乎一定是某个求值期全局缺席；
      // 只报 ReferenceError 那句话看不出是哪个全局，所以把整张能力面一起带出来。
      detail: errorDetail(error) + '｜' + hostGlobals
        + (textPolyfilled.length ? '｜text polyfilled=' + textPolyfilled.join(',') : '')
        + (facade ? '｜facade native=' + facade.native.join(',')
          + (facade.polyfilled.length ? ' polyfilled=' + facade.polyfilled.join(',') : '') : ''),
    });
  }
  if (facade && (facade.native.length || facade.polyfilled.length)) {
    log('wasm-bridge: 宿主无标准 WebAssembly 域名，已装 facade（native='
      + facade.native.join(',') + (facade.polyfilled.length ? '；polyfilled=' + facade.polyfilled.join(',') : '') + '）');
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
        // 兜底成功就是成功：曾经这里无条件返回 `{ failed: [first, second] }`，
        // 于是"首选通道被拒、兜底通道装上"这个本该成功的组合仍报启动失败
        // （下面的 `result.ok` 分支永远进不去）。首选通道的失败仍留在 attempts 里，
        // 排查时不会丢证据——只是它不再决定成败。
        if (second.ok) {
          second.fallbackFrom = {
            channel: first.channel || primaryChannel,
            reason: first.reason,
            detail: first.detail || '',
          };
          return second;
        }
        return { failed: [first, second] };
      });
    })
    .then(function (result) {
      if (result.ok) {
        log('wasm-bridge: 核心实例化成功（' + picked.source + '/' + result.channel
          + (result.path ? '，路径 ' + result.path : '')
          + (result.bytes ? '，' + result.bytes + ' 字节' : '')
          + '，导出 ' + result.exports.length + ' 项：' + result.exports.join(',') + '）');
        // 兜底通道装上时，首选通道的拒绝原因必须留痕：它通常意味着宿主能力面与预期
        // 不符（例如路径通道被拒），排查时这是最早的一个信号，不能因为"最后成功了"丢掉。
        if (result.fallbackFrom) {
          log('wasm-bridge: 首选通道 ' + result.fallbackFrom.channel + ' 被拒（'
            + result.fallbackFrom.reason + '：' + result.fallbackFrom.detail + '），已由兜底通道 '
            + result.channel + ' 装上');
        }
        // host = 胶水函数（wasm-bindgen `--target no-modules` 的导出载体）：调用方可用
        // `new host.HostApi(name)` 实例化宿主桥挂到 `window.__RUSTLUA_HOST_API`（WP10 微信侧接线）。
        return {
          ok: true,
          source: picked.source,
          channel: result.channel,
          path: result.path || null,
          exports: result.exports,
          bytes: result.bytes || null,
          fallbackFrom: result.fallbackFrom || null,
          // 标准域名从哪来：转发了 WXWebAssembly 的哪些构造器，或宿主自己就有标准全局。
          facade: facade
            ? ('native=' + facade.native.join('+')
              + (facade.polyfilled.length ? '+polyfilled=' + facade.polyfilled.join('+') : ''))
            : 'host-standard',
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
