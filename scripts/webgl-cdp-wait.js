// webgl-cdp-wait.js — 通过 Chrome DevTools Protocol 轮询页面状态，状态落定后 dump DOM。
//
// 背景：`--dump-dom` 在页面 load 事件后立即 dump，而本游戏在 module 顶层 await 里
// 加载 wasm + boot（`rustluaRunLuaGame`），dump 常常早于 boot（body 仍是
// data-rustlua-boot="pending"）。CDP 轮询可以等真实时钟里状态真正落定：
//
//   用法: node webgl-cdp-wait.js <port> <outFile> <mode> [timeoutSec]
//   mode: boot   —— 等 data-rustlua-boot 为非 pending（running/ok）
//         scene  —— 等 data-rustlua-scene-verify 落定（done/failed）
//         drive  —— 等 data-rustlua-drive 落定（done/failed）
//         contract —— 等 boot 落定后，把 L3 行为契约注入**打完宿主补丁的活体运行时**跑一遍，
//                     outFile 写的是对拍报告（不是 DOM）。见 scripts/web-api-contract.js。
//   退出码: 0=状态落定并 dump（contract 模式 = 与 PC 基线一致）；
//          1=超时（仍 dump 当前 DOM 供诊断；contract 模式 = 对拍不一致或页面报错）；
//          2=CDP 不可用
//
// 注意：headless 下加 `--virtual-time-budget` 会让 rAF 停调度（剧本永远跑不完），
// 所以这里只用真实时钟 + run-all-compositor-stages-before-draw（调用方负责参数）。

const port = Number(process.argv[2]);
const outFile = process.argv[3];
const mode = process.argv[4] || 'boot';
const timeoutSec = Number(process.argv[5] || 120);
const pollMs = 1500;
const fs = require('fs');
const apiContract = require('./web-api-contract.js');
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function stateOf(dom, attr) {
  const match = dom.match(new RegExp(attr + '="([^"]*)"'));
  return match ? match[1] : 'absent';
}

function settled(dom, mode) {
  // contract 模式的前置条件与 boot 模式相同：补丁层是随启动注入的，boot 没落定就跑契约，
  // 测到的是"半打补丁"的运行时——那种结果既不是绿也不是有意义的红。
  if (mode === 'contract') return settled(dom, 'boot');
  if (mode === 'boot') {
    const s = stateOf(dom, 'data-rustlua-boot');
    return s === 'running' || s === 'ok' ? 'done' : '';
  }
  if (mode === 'scene') {
    const s = stateOf(dom, 'data-rustlua-scene-verify');
    return s === 'done' || s === 'failed' ? s : '';
  }
  if (mode === 'drive') {
    const s = stateOf(dom, 'data-rustlua-drive');
    return s === 'done' || s === 'failed' ? s : '';
  }
  return '';
}

// 契约模式：把 L3 用例注入活体运行时、取回 transcript、与仓库里的 PC 基线对拍。
// 报告写进 outFile（其它模式写 DOM），退出码 0=一致 / 1=不一致或页面侧拿不到结果。
async function runContract(send) {
  const lines = [];
  let webTranscript = null;
  let reason = '';
  try {
    const r = await send('Runtime.evaluate', {
      expression: apiContract.pageExpression(apiContract.readContractSource()),
      returnByValue: true,
    });
    if (r && r.exceptionDetails) {
      reason = 'evaluate threw: ' + ((r.exceptionDetails.exception || {}).description || r.exceptionDetails.text);
    } else {
      const value = r && r.result && r.result.value;
      const payload = typeof value === 'string' ? JSON.parse(value) : null;
      if (payload && payload.error) reason = 'page: ' + payload.error;
      else if (!payload) reason = 'page returned no JSON payload';
      else webTranscript = payload.transcript;
    }
  } catch (e) {
    reason = 'cdp send failed: ' + e;
  }

  if (webTranscript == null) {
    lines.push('web_api_contract_unavailable ' + reason);
    lines.push('(拿不到 web 侧 transcript 就不是"通过"；确认 bundle 是最新发布的、boot 已落定)');
    fs.writeFileSync(outFile, lines.join('\n'), 'utf8');
    console.log('[cdp] contract unavailable: ' + reason);
    return 1;
  }
  const issues = apiContract.diffTranscript(webTranscript, apiContract.readBaselineTranscript());
  if (issues.length === 0) {
    lines.push('web_api_contract_ok cases=' + String(webTranscript.trim().split('\n').length));
    fs.writeFileSync(outFile, lines.join('\n'), 'utf8');
    console.log('[cdp] contract settled: ok (cases=' + String(webTranscript.trim().split('\n').length) + ')');
    return 0;
  }
  lines.push('web_api_contract_failed mismatches=' + issues.length +
    // cases 也带上：红的时候"跑了几条、错了几条"是一个读数的两半，只报一半会让调用方
    // 去打一条假数（PS 侧曾经就是这样显示 `cases=0`，因为失败分支没人填它）。
    ' cases=' + String(webTranscript.trim().split('\n').length));
  lines.push(...issues);
  lines.push('--- web transcript（与 PC 基线同口径，逐行对）---');
  lines.push(String(webTranscript));
  fs.writeFileSync(outFile, lines.join('\n'), 'utf8');
  console.log('[cdp] contract settled: failed (' + issues.length + ' 处与 PC 基线不一致)');
  return 1;
}

async function findPage() {
  // Chrome 启动后 /json/list 才可用；最多等 20 秒（首次启动可能较慢）。
  for (let i = 0; i < 40; i++) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/json/list`);
      const list = await res.json();
      const page = list.find(
        (t) => t.type === 'page' && t.url.includes('index.html') && t.webSocketDebuggerUrl
      );
      if (page) return page;
    } catch (e) { /* 端口还没起来，继续等 */ }
    await sleep(500);
  }
  return null;
}

async function main() {
  const page = await findPage();
  if (!page) {
    console.log('[cdp] no page target found on port ' + port);
    process.exit(2);
  }
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  let msgId = 0;
  const pending = new Map();
  const send = (method, params) =>
    new Promise((resolve, reject) => {
      const id = ++msgId;
      pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id).resolve(msg.result);
      pending.delete(msg.id);
    }
  };
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = () => reject(new Error('websocket connect failed'));
  });

  const deadline = Date.now() + timeoutSec * 1000;
  let lastDom = '';
  while (Date.now() < deadline) {
    let dom = '';
    try {
      const r = await send('Runtime.evaluate', {
        expression: 'document.body ? document.body.outerHTML : ""',
        returnByValue: true,
      });
      dom = r && r.result && typeof r.result.value === 'string' ? r.result.value : '';
    } catch (e) { /* 页面可能还在导航，下一轮再试 */ }
    if (dom) {
      lastDom = dom;
      const outcome = settled(dom, mode);
      if (outcome) {
        if (mode === 'contract') {
          const code = await runContract(send);
          process.exit(code);
        }
        fs.writeFileSync(outFile, dom, 'utf8');
        console.log('[cdp] ' + mode + ' settled: ' + outcome);
        process.exit(0);
      }
    }
    await sleep(pollMs);
  }
  if (lastDom) fs.writeFileSync(outFile, lastDom, 'utf8');
  console.log('[cdp] timeout waiting for ' + mode + ' (last boot=' + stateOf(lastDom, 'data-rustlua-boot') + ')');
  process.exit(1);
}

main().catch((e) => {
  console.log('[cdp] fatal: ' + e);
  process.exit(2);
});
