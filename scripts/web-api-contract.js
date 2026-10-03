// web-api-contract.js — L3 行为契约的 **web 侧执行器**的共用件（页面表达式 + 对拍）。
//
// 为什么需要它：wasm 里的 Lua **注册层**已实测与桌面逐字节相同（L1，41 张表 / 275 个函数），
// 但 `web-runtime.js` 的宿主补丁层重定义了引擎已注册的一批实现（L2 清单——覆盖点数由
// `node scripts/check-web-api-overrides.mjs` 给，2026-10-03 实测 54 个覆盖点 / 11 个模块）。
// 名字两边都在，所以比名字的门抓不到分叉——分叉在**返回值形状与状态归属**。已实测的一例：
//   引擎版 clock.now() -> day,hour,minute,minute_of_day,paused,rate
//   补丁版 clock.now() -> day,minute,minute_of_day（少三个字段，Lua 读到 nil 且不报错）
//
// 这一层把"两侧理应相同"的语义跑出来：桌面侧 `cargo run -q -p engine --example run_lua_contract`
// 生成基线 transcript（并由 `lua_api_contract_matches_transcript` 钉住），web 侧用**同一份 Lua**
// 在**打完补丁的活体运行时**里跑，两边对同一份基线。
//
// 为什么不把契约 Lua 打进发布包：那是 10 KB 的**验证资产**，会挤占微信 4 MB 主包配额
// （`Assert-WeChatPackageVolume` 的门），而它一帧都不参与游戏。所以契约文本由 CDP
// `Runtime.evaluate` 注入（见 scripts/webgl-cdp-wait.js 的 `contract` 模式），运行时不带它。
//
// 约定（两侧都靠它，别改）：
// - `bridge.lua_do_string(source, name)` 加载契约；`bridge.lua_call_global('__contract_run', '[]')`
//   取回整篇 transcript（`'[]'` = **零个参数**，见 lua_bridge.rs 的 MultiValue 修复）；
// - 一条用例一行，行序由 `__contract_run()` 内部排序决定，所以差异是纯文本行 diff。
'use strict';

const fs = require('fs');
const path = require('path');

const repoRoot = path.resolve(__dirname, '..');
const SNAPSHOT_DIR = path.join(
  repoRoot,
  'engine', 'crates', 'rustlua-scripting', 'src', 'api_surface_snapshot'
);
const CONTRACT_PATH = path.join(SNAPSHOT_DIR, 'lua_api_contract.lua');
const TRANSCRIPT_PATH = path.join(SNAPSHOT_DIR, 'lua_api_contract_transcript.txt');

function readContractSource() {
  return fs.readFileSync(CONTRACT_PATH, 'utf8');
}

function readBaselineTranscript() {
  return fs.readFileSync(TRANSCRIPT_PATH, 'utf8').trimEnd();
}

// 在页面（或任何带 `__RUSTLUA_LUA_BRIDGE` 的宿主环境）里跑契约的 JS 表达式。
// 返回值是**一个 JSON 字符串**：`{transcript}` 或 `{error}`，调用方按值取回，
// 不用处理 CDP 的 RemoteObject 结构。
function pageExpression(source) {
  return (
    '(function(){' +
    'try {' +
    'var bridge = (typeof window !== "undefined" && window.__RUSTLUA_LUA_BRIDGE) || null;' +
    'if (!bridge) return JSON.stringify({error: "no __RUSTLUA_LUA_BRIDGE (web runtime not booted)"});' +
    'if (typeof bridge.lua_do_string !== "function" || typeof bridge.lua_call_global !== "function") {' +
    'return JSON.stringify({error: "bridge lacks lua_do_string/lua_call_global (stale bundle)"});' +
    '}' +
    'bridge.lua_do_string(' + JSON.stringify(source) + ', "lua_api_contract.lua");' +
    'var out = bridge.lua_call_global("__contract_run", "[]");' +
    'return JSON.stringify({transcript: out === undefined ? null : out});' +
    '} catch (e) { return JSON.stringify({error: String((e && e.message) || e)}); }' +
    '})()'
  );
}

// transcript 逐行对拍，返回差异描述数组（空 = 一致）。
function diffTranscript(current, baseline) {
  const a = baseline.split('\n');
  const b = String(current == null ? '' : current).split('\n');
  const issues = [];
  const max = Math.max(a.length, b.length);
  for (let i = 0; i < max; i++) {
    const lhs = a[i] === undefined ? '<行数少>' : a[i];
    const rhs = b[i] === undefined ? '<行数多>' : b[i];
    if (lhs !== rhs) issues.push('L' + (i + 1) + ' PC基线 ' + lhs + ' / web ' + rhs);
  }
  return issues;
}

module.exports = {
  CONTRACT_PATH,
  TRANSCRIPT_PATH,
  readContractSource,
  readBaselineTranscript,
  pageExpression,
  diffTranscript,
};
