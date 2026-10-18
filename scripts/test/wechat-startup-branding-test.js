'use strict';

// Deterministic, no packages/WASM/device required. node scripts/test/wechat-startup-branding-test.js
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const root = path.resolve(__dirname, '..', '..');
const helperSource = fs.readFileSync(path.join(root, 'plugins/wechat-minigame/startup-branding.js'), 'utf8');
const adapterSource = fs.readFileSync(path.join(root, 'plugins/wechat-minigame/wechat-adapter.js'), 'utf8');

function clock() {
  let id = 0;
  const pending = new Map();
  return {
    setTimeout(fn, ms) { pending.set(++id, { fn, ms }); return id; },
    clearTimeout(key) { pending.delete(key); },
    run(ms) {
      for (const [key, task] of Array.from(pending)) {
        if (task.ms === ms) { pending.delete(key); task.fn(); }
      }
    }
  };
}
function helper(timers) {
  const context = { module: { exports: {} }, ...timers };
  vm.runInNewContext(helperSource, context);
  return context.module.exports;
}
function fake2d() {
  const calls = [];
  const ctx = { calls };
  for (const method of ['save', 'restore', 'setTransform', 'fillRect', 'drawImage', 'fillText']) {
    ctx[method] = (...args) => calls.push([method, ...args]);
  }
  return ctx;
}
function fakeGl(failure) {
  const gl = {}, state = new Map(), calls = [], objects = new Set();
  const constants = helperSource.match(/gl\.[A-Z][A-Z_0-9]*/g);
  for (const token of constants) gl[token.slice(3)] = token.slice(3);
  for (const key of Object.keys(gl)) state.set(gl[key], 'old-' + key);
  state.set(gl.VIEWPORT, [4, 5, 600, 400]);
  state.set(gl.COLOR_CLEAR_VALUE, [0.2, 0.3, 0.4, 0.5]);
  state.set(gl.COLOR_WRITEMASK, [false, true, false, true]);
  const enabled = new Set([gl.BLEND, gl.SCISSOR_TEST, gl.RASTERIZER_DISCARD]);
  gl.getParameter = key => state.get(key);
  gl.isEnabled = key => enabled.has(key);
  gl.enable = key => enabled.add(key);
  gl.disable = key => enabled.delete(key);
  for (const [method, key] of Object.entries({ useProgram: 'CURRENT_PROGRAM', bindVertexArray: 'VERTEX_ARRAY_BINDING', activeTexture: 'ACTIVE_TEXTURE' })) {
    gl[method] = value => state.set(gl[key], value);
  }
  for (const [method, key] of Object.entries({ bindFramebuffer: 'DRAW_FRAMEBUFFER_BINDING', bindTexture: 'TEXTURE_BINDING_2D', bindSampler: 'SAMPLER_BINDING', bindBuffer: 'PIXEL_UNPACK_BUFFER_BINDING' })) {
    gl[method] = (target, value) => state.set(gl[key], value);
  }
  for (const [method, key] of Object.entries({ viewport: 'VIEWPORT', clearColor: 'COLOR_CLEAR_VALUE', colorMask: 'COLOR_WRITEMASK' })) {
    gl[method] = (...values) => state.set(gl[key], values);
  }
  gl.pixelStorei = (key, value) => state.set(key, value);
  for (const type of ['Shader', 'Program', 'Texture', 'VertexArray']) {
    gl['create' + type] = () => { const object = { type }; objects.add(object); return object; };
    gl['delete' + type] = object => objects.delete(object);
  }
  for (const method of ['shaderSource', 'compileShader', 'attachShader', 'linkProgram', 'texParameteri', 'texImage2D', 'uniform2f', 'uniform1i', 'clear', 'drawArrays', 'flush']) {
    gl[method] = (...args) => {
      calls.push([method, ...args]);
      if (method === failure) throw new Error('mock ' + failure);
    };
  }
  gl.getShaderParameter = () => failure !== 'compile';
  gl.getProgramParameter = () => failure !== 'link';
  gl.getShaderInfoLog = gl.getProgramInfoLog = () => 'mock failure';
  gl.getUniformLocation = (program, name) => name;
  const initial = new Map(state), initialEnabled = new Set(enabled);
  return { gl, calls, restored() {
    assert.deepStrictEqual(state, initial, 'all touched GL state restored');
    assert.deepStrictEqual(enabled, initialEnabled);
    assert.strictEqual(objects.size, 0, 'all GL resources deleted');
  } };
}

async function drawCase(gpu, failure) {
  const timers = clock(), ctx = fake2d(), mock = fakeGl(failure), warnings = [], frames = [];
  const image = { width: 400, height: 400 }, canvas = { width: 800, height: 400 };
  const result = helper(timers)({
    canvas, gpu, createImage: () => image, warn: line => warnings.push(line),
    requestFrame: callback => frames.push(callback),
    getContext(kind, attributes) {
      assert.strictEqual(this, canvas);
      assert.strictEqual(kind, gpu ? 'webgl2' : '2d');
      if (gpu) assert.strictEqual(attributes.preserveDrawingBuffer, true);
      return gpu ? mock.gl : ctx;
    }
  });
  assert.strictEqual(image.src, 'branding/splash.jpg');
  const lateLoad = image.onload;
  image.onload(); image.onload();
  assert.strictEqual(frames.length, 1, 'only one rendering callback');
  assert.strictEqual(mock.calls.length + ctx.calls.length, 0, 'wait for platform frame');
  frames[0](); frames[0]();
  timers.run(0);
  assert.strictEqual(await result, !failure);
  if (gpu) {
    mock.restored();
    if (!failure) {
      assert.strictEqual(mock.calls.filter(call => call[0] === 'drawArrays').length, 1);
      assert.deepStrictEqual(mock.calls.find(call => call[0] === 'uniform2f').slice(2), [0.5, 1]);
    }
  } else {
    assert.deepStrictEqual(ctx.calls.find(call => call[0] === 'drawImage').slice(2), [200, 0, 400, 400]);
    assert.strictEqual(ctx.fillStyle, '#0f192b');
    assert.strictEqual(ctx.calls.at(-1)[0], 'restore');
  }
  const count = mock.calls.length + ctx.calls.length;
  lateLoad(); frames[0](); timers.run(3000);
  assert.strictEqual(mock.calls.length + ctx.calls.length, count, 'no painting after handoff');
  assert.strictEqual(warnings.length, failure ? 1 : 0);
}

async function failureCase(mode) {
  const timers = clock(), image = { width: 100, height: 100 }, frames = [], warnings = [];
  const result = helper(timers)({
    canvas: {}, gpu: true,
    createImage() { if (mode === 'create') throw new Error('create'); return image; },
    getContext() { throw new Error('must not paint'); },
    requestFrame: callback => frames.push(callback), warn: line => warnings.push(line)
  });
  const lateLoad = image.onload;
  if (mode === 'error') image.onerror();
  if (mode === 'frame-timeout') image.onload();
  timers.run(3000);
  assert.strictEqual(await result, false);
  assert.strictEqual(warnings.length, 1);
  if (lateLoad) lateLoad();
  if (frames[0]) frames[0]();
  assert.strictEqual(warnings.length, 1, 'late callbacks inert');
}

async function adapterCase(layout) {
  const timers = clock(), events = [], image = { width: 400, height: 400 }, frames = [];
  const ctx = fake2d(), mock = fakeGl();
  const canvas = { getContext(kind) {
    events.push('context:' + kind);
    return kind === 'webgl2' ? mock.gl : ctx;
  } };
  const files = {
    'pack/project.toml': 'window_width = 800\nwindow_height = 400',
    'pack/manifest.txt': '',
    'code-package.json': '{"renamed":{}}'
  };
  if (layout === 'subpackage') files['pack.json'] = JSON.stringify({ entries: [], layout: { subpackage_root: 'pack-sub' } });
  if (layout === 'remote') files['remote-assets.json'] = JSON.stringify({ base_url: 'https://example.invalid/assets/' });
  const vfs = { readFileSync(name) {
    const key = String(name).replace(/^\.\//, '').replace(/^\//, '');
    if (!(key in files)) throw new Error('missing ' + key);
    return files[key];
  } };
  const sandbox = {
    ...timers, console: { log() {}, warn() {}, error() {} },
    __RUSTLUA_LUA_BRIDGE: { lua_execute_main() { events.push('direct'); } },
    wx: {
      getSystemInfoSync: () => ({ windowWidth: 800, windowHeight: 400, pixelRatio: 1 }),
      createCanvas: () => canvas,
      createOffscreenCanvas: () => ({ getContext: kind => kind === '2d' ? fake2d() : null }),
      createImage() { events.push('image'); return image; },
      getFileSystemManager: () => vfs,
      getStorageSync: () => '', setStorageSync() {},
      requestAnimationFrame: callback => frames.push(callback),
      loadSubpackage(options) { events.push('pack'); options.success(); return {}; }
    },
    require(name) {
      if (name === './startup-branding.js') return helper(timers);
      if (name === './wasm-bridge.js') return { loadWasmCore() {
        events.push('wasm');
        mock.restored();
        return Promise.resolve({ ok: false, reason: 'test-no-wasm' });
      } };
      throw new Error('unexpected require ' + name);
    }
  };
  vm.runInNewContext(adapterSource, sandbox);
  events.length = 0;
  const result = sandbox.__RustLuaWechat.start(() => events.push('legacy-boot'));
  assert.deepStrictEqual(events, ['image'], 'no pack/WASM/boot before image');
  image.onload();
  assert.deepStrictEqual(events, ['image'], 'no work before draw callback');
  frames.shift()();
  assert(!events.includes('wasm'));
  timers.run(0);
  assert.strictEqual(await result, true);
  // B8：`engineVisibleTakeover` 恒真（`wechat-adapter.js` 里那个开关已随原子模式退休），
  // 所以适配层递给 helper 的 `gpu` 不再有"非接管"这一档——主画布只会以 webgl2 被取一次。
  assert(events.includes('context:webgl2'));
  assert(events.indexOf('wasm') > events.indexOf('context:webgl2'));
  assert(events.indexOf('direct') > events.indexOf('wasm'));
  if (layout === 'subpackage') assert(events.indexOf('pack') < events.indexOf('wasm'));
  assert(!events.includes('context:2d'), 'visible GPU canvas never locked to 2D');
  timers.run(3000);
}

(async function () {
  await drawCase(false);
  await drawCase(true);
  for (const failure of ['compile', 'link', 'texImage2D', 'drawArrays']) await drawCase(true, failure);
  for (const mode of ['error', 'create', 'image-timeout', 'frame-timeout']) await failureCase(mode);
  for (const layout of ['full', 'remote', 'subpackage']) {
    await adapterCase(layout);
  }
  console.log('wechat_startup_branding_test_ok cases=15 (mock GL/2D and adapter startup ordering)');
})().catch(error => { console.error(error); process.exitCode = 1; });
