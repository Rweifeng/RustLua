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
//   退出码: 0=状态落定并 dump；1=超时（仍 dump 当前 DOM 供诊断）；2=CDP 不可用
//
// 注意：headless 下加 `--virtual-time-budget` 会让 rAF 停调度（剧本永远跑不完），
// 所以这里只用真实时钟 + run-all-compositor-stages-before-draw（调用方负责参数）。

const port = Number(process.argv[2]);
const outFile = process.argv[3];
const mode = process.argv[4] || 'boot';
const timeoutSec = Number(process.argv[5] || 120);
const pollMs = 1500;
const fs = require('fs');
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function stateOf(dom, attr) {
  const match = dom.match(new RegExp(attr + '="([^"]*)"'));
  return match ? match[1] : 'absent';
}

function settled(dom, mode) {
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
