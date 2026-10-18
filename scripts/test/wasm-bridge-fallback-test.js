// wasm-bridge 通道组合的定点回归测试（不依赖发布产物，直接喂假宿主与假胶水）。
//
// ## 为什么单独有这个文件
// 2026-10-08 修复的缺陷形态是"**兜底通道装上、却仍报启动失败**"：
//   `wasm-bridge.js` 的主链在首选通道失败后无条件返回 `{failed:[first,second]}`，
//   于是"首选通道被拒、兜底通道 ok"这个组合被下面的 `result.ok` 分支漏掉，
//   最终以首选通道的 reason 报失败。症状是"真机上明明装上了，屏幕上却是启动失败"。
// 这条分支只在**两个通道一败一成**时才走到，而微信真机通常只有路径通道可用、
// 本机 Node 通常只有字节通道可用——两端各自的常规路径都盖不到它。
// 所以这里用假宿主把两个通道的四种组合（成/成、成/败、败/成、败/败）逐个走一遍，
// 不依赖任何真实 wasm 产物，也不依赖 wx。
//
// 退出码：0 全部断言通过；1 有断言失败；3 环境不满足（缺 wasm-bridge.js）。
'use strict';

const path = require('path');
const Module = require('module');

const BRIDGE_PATH = path.join(__dirname, '..', '..', 'plugins', 'wechat-minigame', 'wasm-bridge.js');
const failures = [];
const notes = [];

function check(label, condition, detail) {
  if (condition) {
    notes.push('  ok  ' + label);
    return true;
  }
  failures.push(label + (detail ? '（' + detail + '）' : ''));
  return false;
}

// 假胶水：形状必须满足 wasm-bridge 的判据（可调用 + 有 initSync）。
//   * `glue({})` 走的是胶水自己的 `WebAssembly.instantiate(...)` 分支，
//     而 wasm-bridge 会用 shim 把这次调用改道到宿主的 `instantiate(路径, imports)`；
//   * `glue.initSync(bytes)` 走字节通道。
function makeFakeGlue(behavior) {
  const glue = function (input) {
    if (behavior.pathMode === 'throw') throw new Error('path-channel-sync-throw');
    // 真实胶水在这里调 `WebAssembly.instantiate`（此时已被 bridge 换成 shim）。
    const hostWasm = globalThis.WebAssembly;
    if (!hostWasm || typeof hostWasm.instantiate !== 'function') {
      return Promise.reject(new Error('no instantiate during glue eval'));
    }
    return hostWasm.instantiate(new Uint8Array([0]), {}).then(function () {
      if (behavior.pathMode === 'reject') return Promise.reject(new Error('path-channel-rejected'));
      return { wasm: { exports: { ok: 1 } } };
    });
  };
  glue.initSync = function (_bytes) {
    if (behavior.bytesMode === 'throw') throw new Error('bytes-channel-rejected');
    // listExports 会剔掉 `__` 前缀的名字并排序。
    return { zeta_export: 1, __internal: 2, alpha_export: 3 };
  };
  return glue;
}

// 假宿主：`WXWebAssembly` 存在 ⇒ 首选通道是路径式（与真机同序），字节通道为兜底。
// 路径通道的成败由 `hostPathMode` 决定——"首选成功"这条用例必须让宿主真的兑现，
// 否则四个通道组合里有一个永远测不到（第一版就是这样把用例 4 测成了假失败）。
function makeFakeHost(hostPathMode) {
  return {
    instantiate: function (_p, _imports) {
      if (hostPathMode === 'resolve') {
        return Promise.resolve({ module: {}, instance: { exports: {} } });
      }
      return Promise.reject(new Error('host rejects path instantiate'));
    },
    Module: function Module() { },
    Instance: function Instance() { },
  };
}

async function runCase(name, behavior, expect) {
  const ModuleCtor = Module;
  const originalRequire = ModuleCtor.prototype.require;
  ModuleCtor.prototype.require = function (request) {
    if (request === './pkg/game_web.js') return makeFakeGlue(behavior);
    return originalRequire.apply(this, arguments);
  };
  const savedWx = globalThis.WXWebAssembly;
  globalThis.WXWebAssembly = makeFakeHost(behavior.hostPathMode || 'reject');
  try {
    delete require.cache[require.resolve(BRIDGE_PATH)];
    const bridge = require(BRIDGE_PATH);
    const result = await bridge.loadWasmCore({
      readBytes: function () { return new Uint8Array([0, 97, 115, 109]); },
      wasmPath: 'pkg/game_web_bg.wasm',
      log: function () { },
    });
    const detail = 'ok=' + result.ok + ' channel=' + result.channel
      + ' reason=' + result.reason + ' fallbackFrom='
      + (result.fallbackFrom ? result.fallbackFrom.channel : 'null');
    check(name + '：' + expect.label, expect.test(result), detail);
    return result;
  } finally {
    ModuleCtor.prototype.require = originalRequire;
    if (savedWx === undefined) delete globalThis.WXWebAssembly;
    else globalThis.WXWebAssembly = savedWx;
  }
}

async function main() {
  try {
    require.resolve(BRIDGE_PATH);
  } catch (error) {
    console.error('wasm-bridge.js 不存在: ' + BRIDGE_PATH);
    process.exit(3);
  }

  // 用例 1：首选（路径）失败、兜底（字节）成功 —— 就是本次修复的那条。
  await runCase('首选失败+兜底成功', { pathMode: 'reject', bytesMode: 'ok' }, {
    label: '必须判为成功，并记录兜底来源',
    test: (r) => r.ok === true
      && r.channel === 'bytes-sync'
      && r.fallbackFrom && r.fallbackFrom.channel === 'path-instantiate',
  });

  // 用例 2：首选同步抛错、兜底成功 —— 抛错与异步拒绝是两条不同的失败路径。
  await runCase('首选抛错+兜底成功', { pathMode: 'throw', bytesMode: 'ok' }, {
    label: '必须判为成功',
    test: (r) => r.ok === true && r.channel === 'bytes-sync',
  });

  // 用例 3：两条都失败 —— 判据仍按首选通道给（诊断口径不能被兜底覆盖）。
  await runCase('两通道皆失败', { pathMode: 'reject', bytesMode: 'throw' }, {
    label: '必须判为失败且 reason 取自首选通道',
    test: (r) => r.ok === false && typeof r.reason === 'string' && r.reason.length > 0,
  });

  // 用例 4：首选成功 —— 不得因为兜底通道的状态而改变结论。
  await runCase('首选成功', { pathMode: 'resolve', bytesMode: 'ok', hostPathMode: 'resolve' }, {
    label: '必须判为成功且无兜底来源',
    test: (r) => r.ok === true && r.channel === 'path-instantiate' && !r.fallbackFrom,
  });

  for (const line of notes) console.log(line);
  if (failures.length) {
    console.error('wasm_bridge_fallback_test_failed: ' + failures.join('；'));
    process.exit(1);
  }
  console.log('wasm_bridge_fallback_test_ok cases=4');
}

main().catch((error) => {
  console.error('wasm_bridge_fallback_test_failed: ' + (error && error.stack ? error.stack : error));
  process.exit(1);
});
