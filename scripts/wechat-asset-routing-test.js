// 微信适配层「资源路由」单元测试（无头、不需要 zig / wasm 产物）。
//
// 为什么需要它：`wechat-adapter.js` 的三条路由（主体 mainpack/pack、代码包改名、
// 远端缓存）过去只由整包 harness 覆盖，而 harness 必须先构建出 wasm 产物才能跑。
// 结果是这几条规则**改坏了也看不出来**——历史上"首屏音频指向不存在的 pack/ 路径"与
// "换版本后旧缓存永久遮蔽新内容"都属这一类。这里用假 wx 直接加载适配层，把这两条
// 规则的判据钉住（不依赖产物，可以在任何一次改动后立刻跑）。
//
// 用法：node scripts/wechat-asset-routing-test.js
'use strict';

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const ROOT = path.resolve(__dirname, '..');
const ADAPTER = path.join(ROOT, 'plugins', 'wechat-minigame', 'wechat-adapter.js');

const failures = [];
function check(name, condition, detail) {
  if (condition) {
    console.log(`  ok  ${name}`);
  } else {
    failures.push(name);
    console.log(`  FAIL ${name}${detail ? ' -> ' + detail : ''}`);
  }
}

// ------------------------------------------------------------------ 假宿主

const USER_ROOT = 'http://usr';

function makeVirtualFs() {
  const files = new Map(); // path -> Uint8Array
  const removed = [];
  const toBytes = value => (value instanceof Uint8Array ? value : new Uint8Array(Buffer.from(String(value), 'utf8')));
  const missing = target => {
    const error = new Error('no such file or directory: ' + target);
    error.errMsg = 'accessSync:fail no such file or directory ' + target;
    return error;
  };
  return {
    files,
    removed,
    put(target, value) { files.set(target, toBytes(value)); },
    has(target) { return files.has(target); },
    text(target) {
      const bytes = files.get(target);
      return bytes ? Buffer.from(bytes).toString('utf8') : null;
    },
    accessSync(target) { if (!files.has(target)) throw missing(target); },
    readFileSync(target, encoding) {
      if (!files.has(target)) throw missing(target);
      const bytes = files.get(target);
      return encoding ? Buffer.from(bytes).toString(encoding) : bytes;
    },
    writeFileSync(target, value) { files.set(target, toBytes(value)); },
    mkdirSync() { /* 虚拟 FS 不需要目录 */ },
    rmdirSync(target) {
      removed.push(target);
      for (const key of Array.from(files.keys())) {
        if (key === target || key.indexOf(target + '/') === 0) files.delete(key);
      }
    },
    readdirSync(target) {
      const out = [];
      for (const key of files.keys()) {
        if (key.indexOf(target + '/') === 0) out.push(key.slice(target.length + 1).split('/')[0]);
      }
      return Array.from(new Set(out));
    }
  };
}

// 加载一份适配层，返回 { api, vfs, storage, frames }。
// 每次加载都在**独立的 vm 上下文**里执行：适配层会把 window/document/Image 等一批
// 全局装到自己的 g 上，共用主 realm 会互相污染（第二个用例读到第一个的安装结果）。
function loadAdapter(options) {
  const opts = options || {};
  const vfs = makeVirtualFs();
  const storage = new Map();
  const rmdirCalls = vfs.removed;

  const canvas = {
    width: 1280,
    height: 720,
    getContext() { return makeContext(canvas); }
  };

  const wx = {
    env: { USER_DATA_PATH: USER_ROOT },
    getSystemInfoSync() {
      return { windowWidth: 1280, windowHeight: 720, pixelRatio: 1, platform: 'android' };
    },
    createCanvas() { return canvas; },
    getFileSystemManager() { return vfs; },
    getStorageSync(key) { return storage.has(key) ? storage.get(key) : ''; },
    setStorageSync(key, value) { storage.set(key, value); },
    downloadFile() { throw new Error('本用例不应触发远端下载'); },
    request() { throw new Error('本用例不应触发网络请求'); },
    // 适配层注册的各种生命周期回调：全部收下即可。
    onShow() {}, onHide() {}, onMemoryWarning() {}, onError() {}, onKeyDown() {}, onKeyUp() {},
    offKeyDown() {}, offKeyUp() {},
    getLaunchOptionsSync() { return { scene: 1001, query: {} }; }
  };
  if (opts.wxPatch) opts.wxPatch(wx, vfs, storage);

  const sandbox = {
    wx,
    console: { log() {}, warn() {}, error() {} },
    setTimeout, clearTimeout, setInterval, clearInterval,
    Promise, Date, Math, JSON, Object, Array, String, Number, Boolean, Error, Map, Set,
    Uint8Array, Uint8ClampedArray, ArrayBuffer, DataView, RegExp, Symbol, Proxy, Reflect,
    parseInt, parseFloat, isNaN, TextEncoder: global.TextEncoder, TextDecoder: global.TextDecoder
  };
  // 适配层把宿主 API 装到自己的全局对象上；`wx` 必须**在加载前**就在这个上下文里。
  // 故意不提供 window/document/Image：适配层要自己补出来（那正是它在真机上的职责）。
  const context = vm.createContext(sandbox);
  sandbox.globalThis = context;
  const source = fs.readFileSync(ADAPTER, 'utf8');
  vm.runInContext(source, context, { filename: 'wechat-adapter.js' });

  const api = context.__RustLuaWechat;
  if (!api) throw new Error('适配层没有导出 __RustLuaWechat');
  return { api, vfs, storage, canvas, rmdirCalls, context };
}

function makeContext(canvas) {
  const state = {};
  return new Proxy(state, {
    get(target, prop) {
      if (prop === 'canvas') return canvas;
      if (prop === 'measureText') return () => ({ width: 8 });
      if (prop === 'getImageData') return (x, y, w, h) => ({ data: new Uint8ClampedArray(w * h * 4) });
      if (prop === 'createLinearGradient') return () => ({ addColorStop() {} });
      if (prop in target) return target[prop];
      return () => undefined;
    },
    set(target, prop, value) { target[prop] = value; return true; }
  });
}

// 适配层算发布标识的口径：逐字符拆成两个字节（小端），再走同一份 FNV-1a。
function utf16Bytes(text) {
  const out = new Uint8Array(text.length * 2);
  for (let i = 0; i < text.length; i += 1) {
    const code = text.charCodeAt(i);
    out[i * 2] = code & 0xff;
    out[i * 2 + 1] = (code >>> 8) & 0xff;
  }
  return out;
}

// ------------------------------------------------------------------ 用例

// ① 音频/任何"宿主自己加载"的资源：分包形态下首屏文件在 mainpack/，不能自己拼 pack/。
function caseMainpackRouting() {
  console.log('\n[1] 首屏资源（mainpack 主体路由）');
  const host = loadAdapter({
    wxPatch(wx, vfs) {
      // `entries` 是**列表**（不是对象）：适配层按 list.length 判"清单形态"，
      // 写成对象会被当成"无首屏清单"而静默不启用主包路由。
      vfs.put('mainpack.json', JSON.stringify({
        entries: ['assets/audio/title.mp3'],
        layout: { subpackage_root: 'pack' }
      }));
      vfs.put('mainpack/assets/audio/title.mp3', 'ID3');
    }
  });
  const routed = host.api.assetSrcFor('pack/assets/audio/title.mp3');
  check('首屏音频路由到 mainpack/', routed.src === 'mainpack/assets/audio/title.mp3',
    `实际 ${routed.src}`);
  check('来源标记为 package', routed.source === 'package', `实际 ${routed.source}`);

  // 非首屏资源仍走 pack/（分包挂载后才可读）
  const later = host.api.assetSrcFor('pack/assets/audio/step.mp3');
  check('非首屏音频仍指向 pack/', later.src === 'pack/assets/audio/step.mp3', `实际 ${later.src}`);
}

// ② 远端形态：包内没有、配了远端 → 必须交远端 URL（不是那个不存在的包内路径）。
function caseRemoteUrl() {
  console.log('\n[2] 远端形态（包内没有时交远端 URL）');
  const host = loadAdapter({
    wxPatch(wx, vfs) {
      vfs.put('remote-assets.json', JSON.stringify({ packBaseUrl: 'https://cdn.example.com/assets/' }));
      // 远端形态下清单也会落地（`ensureManifest` 先下载它，再据此校验其余文件）。
      vfs.put('pack/manifest.txt', 'assetc_manifest\t1\nassets/audio/step.mp3\t3\t0123456789abcdef\n');
    }
  });
  const remote = host.api.assetSrcFor('pack/assets/audio/title.mp3');
  check('交远端 URL', remote.src === 'https://cdn.example.com/assets/assets/audio/title.mp3',
    `实际 ${remote.src}`);
  check('来源标记为 remote', remote.source === 'remote', `实际 ${remote.source}`);

  // 用户目录里已有**本版本**的缓存时优先用缓存（离线可播，而不是每次去拉网络）
  const key = host.api.releaseKey();
  if (key) {
    host.vfs.put(`${USER_ROOT}/v-${key}/pack/assets/audio/step.mp3`, 'ID3');
    const hit = host.api.assetSrcFor('pack/assets/audio/step.mp3');
    check('本版本缓存命中优先用缓存', hit.src === `${USER_ROOT}/v-${key}/pack/assets/audio/step.mp3`,
      `实际 ${hit.src}`);
    check('来源标记为 cache', hit.source === 'cache', `实际 ${hit.source}`);
  } else {
    check('远端用例应算得出发布标识', false, 'releaseKey 为空（清单不可得）');
  }
}

// ③ 缓存版本隔离：换一版资源后，旧的同名缓存**不得**再被命中。
function caseCacheVersioning() {
  console.log('\n[3] 远端缓存版本隔离');
  const manifestV1 = 'assetc_manifest\t1\nscripts/main.lua\t11\t1111111111111111\n';
  const manifestV2 = 'assetc_manifest\t1\nscripts/main.lua\t22\t2222222222222222\n';

  const host = loadAdapter({
    wxPatch(wx, vfs) {
      vfs.put('pack/manifest.txt', manifestV1);
    }
  });
  const before = host.api.cachePathFor('pack/scripts/main.lua');
  check('缓存路径带发布标识', /\/v-[0-9a-f]{16}\/pack\/scripts\/main\.lua$/.test(before), `实际 ${before}`);
  // 发布标识必须**只由清单内容决定**：按同样的口径（逐字符拆两个字节）在这里独立算一遍。
  const expectedKey = host.api.checksumOf(utf16Bytes(manifestV1));
  check('发布标识 = 清单文本指纹', host.api.releaseKey() === expectedKey,
    `实际 ${host.api.releaseKey()} 期望 ${expectedKey}`);

  // 同一个清单必须给出同一个键（否则每次启动都会当成新版本、整批重下）
  check('同一清单键稳定', host.api.cachePathFor('pack/scripts/main.lua') === before);

  // 清单自己**永远**不带标识：远端形态是"先落盘清单 → 再据它算标识"，清单若带标识就会
  // 用上一版的键写入，于是新清单永远读不回来、发布标识永远停在上一版。
  check('清单路径不带发布标识',
    host.api.cachePathFor('pack/manifest.txt') === USER_ROOT + '/pack/manifest.txt',
    `实际 ${host.api.cachePathFor('pack/manifest.txt')}`);

  // 换一版清单：路径必须变
  host.vfs.put('pack/manifest.txt', manifestV2);
  const reloaded = loadAdapter({
    wxPatch(wx, vfs) { vfs.put('pack/manifest.txt', manifestV2); }
  });
  const after = reloaded.api.cachePathFor('pack/scripts/main.lua');
  check('清单变化后键随之变化', after !== before, `v1=${before} v2=${after}`);

  // 这条才是原故障本身：盘上留着**旧版**的同名缓存（不带标识的历史路径 + 旧版本目录），
  // 读取**绝不能**命中它。修好之前 user-data 是第一候选且命中即返回，于是"改了 Lua，
  // 真机上还是老样子"，而且不会自愈。
  const shadowed = loadAdapter({
    wxPatch(wx, vfs) {
      vfs.put('pack/manifest.txt', manifestV1);
      vfs.put(`${USER_ROOT}/pack/scripts/main.lua`, '旧版本内容（历史路径）');
      vfs.put(`${USER_ROOT}/v-0000000000000000/pack/scripts/main.lua`, '旧版本内容（旧标识目录）');
    }
  });
  // 触发发布标识计算（真实启动里由第一次资源读取驱动）
  shadowed.api.releaseKey();
  const staleRead = shadowed.api.readText('pack/scripts/main.lua');
  check('旧版本缓存不再被读到', staleRead === null, `实际 ${JSON.stringify(staleRead)}`);
}

// ④ 旧版本目录必须被清掉（否则用户目录配额被一代代缓存吃满）。
function casePruneStaleCache() {
  console.log('\n[4] 清理上一版缓存目录');
  const manifest = 'assetc_manifest\t1\nscripts/main.lua\t11\tabcdefabcdefabcd\n';
  let staleRoot = null;
  const host = loadAdapter({
    wxPatch(wx, vfs, storage) {
      vfs.put('pack/manifest.txt', manifest);
      staleRoot = USER_ROOT + '/v-0000000000000000';
      vfs.put(staleRoot + '/pack/scripts/main.lua', '旧版本内容');
      storage.set('rustlua_asset_cache_root', staleRoot);
    }
  });
  // 触发一次：releaseKey() 里做这件事（只有算得出标识才谈得上"上一版"）
  host.api.releaseKey();
  check('旧版本目录被删除', host.rmdirCalls.indexOf(staleRoot) >= 0,
    `调用=${JSON.stringify(host.rmdirCalls)}`);
  check('旧缓存文件真的没了', !host.vfs.has(staleRoot + '/pack/scripts/main.lua'));
  check('当前版本目录被记下', host.storage.get('rustlua_asset_cache_root') === USER_ROOT + '/v-' + host.api.releaseKey(),
    `实际 ${host.storage.get('rustlua_asset_cache_root')}`);

  // 标记不是我们写的（例如玩家目录里别的 v-* 目录）时**不许**删
  const foreign = loadAdapter({
    wxPatch(wx, vfs, storage) {
      vfs.put('pack/manifest.txt', manifest);
      vfs.put(USER_ROOT + '/v-9999999999999999/pack/keep.bin', 'x');
      storage.set('rustlua_asset_cache_root', 'http://usr/other/thing');
    }
  });
  foreign.api.releaseKey();
  check('不删非我标记的目录', foreign.rmdirCalls.length === 0, `调用=${JSON.stringify(foreign.rmdirCalls)}`);
}

// ⑤ 清单不可得时必须**回落**到不带标识的旧路径，而不是把整批缓存判成未命中。
function caseNoManifestFallback() {
  console.log('\n[5] 清单缺失时的回落');
  const host = loadAdapter({});
  const target = host.api.cachePathFor('pack/scripts/main.lua');
  check('无清单时用旧路径', target === USER_ROOT + '/pack/scripts/main.lua', `实际 ${target}`);
  check('无清单时不报发布标识', !host.api.releaseKey(), `实际 ${host.api.releaseKey()}`);
}

// ⑥ 递归保护：读清单本身要走缓存路径，而缓存路径要读清单 —— 必须能自己停下来。
function caseNoRecursion() {
  console.log('\n[6] 读清单不得无限递归');
  const host = loadAdapter({
    wxPatch(wx, vfs) { vfs.put('pack/manifest.txt', 'assetc_manifest\t1\nscripts/main.lua\t11\t0123456789abcdef\n'); }
  });
  // 触发的路径：assetSrcFor → hostSrcFor → readSubjects（不读清单）
  // 真正会递归的是 readWithStrategies 的 user-data 候选 → cachePathFor → loadManifestChecksums。
  const routed = host.api.assetSrcFor('pack/scripts/main.lua');
  check('读取路径可用（未卡死）', !!routed && !!routed.src, `实际 ${JSON.stringify(routed)}`);
}

const cases = [
  ['mainpack 主体路由', caseMainpackRouting],
  ['远端 URL', caseRemoteUrl],
  ['缓存版本隔离', caseCacheVersioning],
  ['清理旧缓存目录', casePruneStaleCache],
  ['清单缺失回落', caseNoManifestFallback],
  ['读清单不递归', caseNoRecursion]
];

for (const [, run] of cases) {
  try {
    run();
  } catch (error) {
    failures.push('异常: ' + (error && error.message));
    console.log('  FAIL 抛异常 -> ' + (error && error.stack ? error.stack.split('\n').slice(0, 4).join('\n        ') : error));
  }
}

console.log('');
if (failures.length) {
  console.log(`wechat_asset_routing_test_failed cases=${cases.length} failed=${failures.length}`);
  for (const name of failures) console.log('  - ' + name);
  process.exit(1);
}
console.log(`wechat_asset_routing_test_ok cases=${cases.length}`);
