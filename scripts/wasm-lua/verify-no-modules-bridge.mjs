// no-modules 胶水下 Lua 桥最小验证（WP10 迁移的微信侧风险点）：
// lib.rs 曾记录"no-modules 胶水下新增导出 + 函数参数静默返回 undefined"未定位，
// 本用例直接证明这条路径在迁移后的桥导出上可用（lua_boot/lua_do_string/
// lua_call_global/lua_register_js_callback/lua_missing_globals/lua_execute_main）。
//
// 由 scripts/verify-wasm-lua.ps1 驱动（cargo build game_web wasm + wasm-bindgen
// --target no-modules，再 node --experimental-wasm-exnref 调用本脚本）。
// 胶水目录解析顺序：argv[2] → $RUSTLUA_GLUE_DIR → <repo>/artifacts/wasm-lua-verify/no-modules-test
// （该目录是 gitignored 构建产物；本脚本只读它、并把 patched 胶水写回同一目录）。
//
// 退出码：0 = 全通过；1 = 有断言失败；3 = 胶水缺失（构建没跑/失败，环境受限）。
import { readFileSync, existsSync, copyFileSync, appendFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '..', '..');
const glueDir =
  process.argv[2] ||
  process.env.RUSTLUA_GLUE_DIR ||
  join(repoRoot, 'artifacts', 'wasm-lua-verify', 'no-modules-test');

const require = createRequire(import.meta.url);

let passed = 0, failed = 0;
const assert = (cond, label) => {
  if (cond) { passed += 1; console.log('  ok:', label); }
  else { failed += 1; console.error('  FAIL:', label); }
};

const glueJs = join(glueDir, 'game_web.js');
const glueWasm = join(glueDir, 'game_web_bg.wasm');
if (!existsSync(glueJs) || !existsSync(glueWasm)) {
  console.error(`no-modules glue not found in: ${glueDir}`);
  console.error('需先构建 game_web wasm 并跑 wasm-bindgen --target no-modules（见 verify-wasm-lua.ps1）。');
  process.exitCode = 3;
} else {
  runBridge();
  console.log(`\nRESULT: ${passed} passed, ${failed} failed`);
  process.exitCode = failed === 0 ? 0 : 1;
}

function runBridge() {
  // 1. 模拟 package.ps1 的微信补丁：胶水末尾追加 `module.exports = wasm_bindgen;`
  //（no-modules 胶水的 `let wasm_bindgen` 是模块作用域绑定，CommonJS require 拿不到，
  //  微信代码包正是靠这一步把胶水函数变成可 require 的 CommonJS 导出）。
  const patchedGlue = join(glueDir, 'game_web.patched.js');
  copyFileSync(glueJs, patchedGlue);
  appendFileSync(patchedGlue, '\nmodule.exports = wasm_bindgen;\n');

  const glue = require(patchedGlue);
  assert(typeof glue === 'function' && typeof glue.initSync === 'function', 'glue is CommonJS initSync (after module.exports patch)');

  // 2. 同步实例化。注意：initSync 返回**原始 wasm 导出**（返回值不解码，如
  // Option<String> 是 [tag, ptr, len] 数组）；JS 包装层（lua_call_global 等解码
  // 函数）挂在**胶水函数对象**上——微信侧 wechat-adapter 用 result.host.lua_*，
  // 这里同样用 glue.lua_*。
  const wasmBytes = readFileSync(glueWasm);
  let rawExports;
  try {
    // wasm-bindgen 0.2.129 起 initSync 只接受单对象参数（`{ module }`）；旧胶水是
    // 位置参数 `initSync(bytes)`。先按新形式，失败再回退旧形式，兼容两代产物。
    try {
      rawExports = glue.initSync({ module: wasmBytes });
    } catch (deprecated) {
      rawExports = glue.initSync(wasmBytes);
    }
    assert(true, 'initSync ok');
  } catch (e) {
    assert(false, 'initSync failed: ' + e);
    return;
  }

  // 3. 新增导出存在（lib.rs 风险点：此前"新增导出"在 no-modules 下静默 undefined）
  const names = Object.keys(glue).filter(n => n.indexOf('__') !== 0).sort();
  const bridgeNames = ['lua_boot', 'lua_boot_with_reader', 'lua_do_string', 'lua_call_global',
    'lua_register_js_callback', 'lua_missing_globals', 'lua_execute_main', 'start', 'start_wechat'];
  for (const n of bridgeNames) {
    assert(typeof glue[n] === 'function', `export ${n} is callable`);
  }
  console.log('  exports:', names.join(','));
  // 原始 wasm 导出（initSync 返回）应包含 lua_*（微信 wasm-bridge 的导出清单核对用）
  const rawNames = Object.keys(rawExports).filter(n => n.indexOf('__') !== 0).sort();
  for (const n of bridgeNames) {
    assert(typeof rawExports[n] === 'function', `raw export ${n} present`);
  }

  // 4. 资源表（模拟微信适配层 readText：pack/ 明文）
  const resources = {
    'scripts/main.lua': [
      "local helper = require('helper')",
      "function update() return 'ok:' .. helper.greet() .. ':' .. probe() end",
      "function probe() return 'probe-ok' end",
      "function calljs() return __p2_test_js('hello', 42) end",
    ].join('\n'),
    'scripts/helper.lua': "return { greet = function() return 'hi' end }",
  };
  function readFile(path) {
    const value = resources[path];
    return value === undefined ? null : value;
  }

  // 5. lua_boot_with_reader（新增导出 + 函数参数）
  try {
    glue.lua_boot_with_reader(readFile);
    assert(true, 'lua_boot_with_reader ok');
  } catch (e) {
    assert(false, 'lua_boot_with_reader failed: ' + e);
  }

  // 6. 补丁注入（lua_do_string）
  try {
    glue.lua_do_string("function patch() return 'patched' end", 'test-patch');
    assert(true, 'lua_do_string ok');
  } catch (e) {
    assert(false, 'lua_do_string failed: ' + e);
  }

  // 7. 执行 main.lua（lua_execute_main：vfs provider 读 require 模块）
  try {
    glue.lua_execute_main();
    assert(true, 'lua_execute_main ok');
  } catch (e) {
    assert(false, 'lua_execute_main failed: ' + e);
  }

  // 8. 调全局 update()（走 require('helper') + probe()）
  try {
    const result = glue.lua_call_global('update', '[]');
    assert(result === 'ok:hi:probe-ok', `update() = ${JSON.stringify(result)}`);
  } catch (e) {
    assert(false, 'lua_call_global update failed: ' + e);
  }

  // 9. 调补丁注入的全局（lua_do_string 的产物）
  try {
    const result = glue.lua_call_global('patch', '[]');
    assert(result === 'patched', `patch() = ${JSON.stringify(result)}`);
  } catch (e) {
    assert(false, 'lua_call_global patch failed: ' + e);
  }

  // 10. 不存在的全局 → null
  // wasm-bindgen 对 Ok(None) 的 JS 映射是 undefined；web-runtime.js 的 callGlobal
  // 已把 undefined 归一成 null（fengari 版语义），这里接受胶水原始形态（undefined）。
  try {
    const result = glue.lua_call_global('not_a_function', '[]');
    assert(result === undefined || result === null, `missing global yields undefined|null (got ${JSON.stringify(result)})`);
  } catch (e) {
    assert(false, 'lua_call_global missing failed: ' + e);
  }

  // 11. 缺失全局清单（lua_missing_globals）
  try {
    const missing = JSON.parse(glue.lua_missing_globals(JSON.stringify(['input', 'audio', 'totally_missing'])));
    assert(Array.isArray(missing) && missing.indexOf('totally_missing') >= 0 && missing.indexOf('input') < 0,
      `missing globals probe (got ${JSON.stringify(missing)})`);
  } catch (e) {
    assert(false, 'lua_missing_globals failed: ' + e);
  }

  // 12. JS 回调注册（lua_register_js_callback + 多值协议）
  try {
    glue.lua_register_js_callback('__p2_test_js', function (text, n) {
      if (n === 42) return [true, 'js-echo:' + text];
      return [false, 'bad-n'];
    });
    const result = glue.lua_call_global('calljs', '[]');
    assert(true, `js callback invoked (side effect verified below); calljs raw=${JSON.stringify(result)}`);
    // 独立验证多值：Lua 函数返回两值，取第二个
    glue.lua_do_string("function twoval() local ok, reason = __p2_test_js('x', 42); return reason end", 'test-twoval');
    const reason = glue.lua_call_global('twoval', '[]');
    assert(reason === 'js-echo:x', `multivalue js callback reason = ${JSON.stringify(reason)}`);
  } catch (e) {
    assert(false, 'lua_register_js_callback failed: ' + e);
  }

  // 13. Lua 运行错误 → JS 异常（callGlobal 协议的错误路径）
  try {
    glue.lua_do_string("function boom() error('kaboom') end", 'test-boom');
    glue.lua_call_global('boom', '[]');
    assert(false, 'lua error should throw');
  } catch (e) {
    const text = String((e && e.message) || e);
    assert(text.indexOf('kaboom') >= 0, `lua error propagates (${text})`);
  }
}
